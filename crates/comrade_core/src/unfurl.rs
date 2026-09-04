/*!
 * Link previews, Telegram-style — built by the sender, never fetched by the
 * receiver.
 *
 * **The load-bearing design decision.** When a message contains a link, the
 * *sending* device fetches the page, builds a [`LinkPreview`] card, and
 * attaches it to the message body before it ever leaves the device (see
 * [`attach_preview`]). The receiving device renders that card with **zero
 * network requests** — it never dereferences the URL itself. That is the
 * entire point: fetching a link tells the linked host "this npub opened this
 * link, from this IP, at this time" — exactly the metadata this app exists
 * to not leak. A sender who typed the link already made that trade for
 * themselves; a receiver who merely *received* a message should not be
 * opted into it silently.
 *
 * **What this protects, and what it does not.**
 *  - It does not stop the *sender's* fetch from being observable to the
 *    linked host — the sender chose to visit the link the moment they pasted
 *    it into the composer, and unfurling only adds one more fetch to that
 *    same, already-made choice.
 *  - It does not stop a malicious sender from attaching a preview that
 *    misrepresents the link (a card titled "your bank" pointing at
 *    `evil.example`). Nothing here authenticates the card against the URL —
 *    it is exactly as trustworthy as the rest of the message, which is to
 *    say: the sending Comrade built this, and no more. That is why
 *    [`display_domain`] exists and why it is defined to read the *URL*,
 *    never [`LinkPreview::site_name`] (a page can claim to be named
 *    anything) — the domain shown on the card is the one thing a spoofed
 *    `og:` block cannot lie about.
 *
 * **Receiver-side fetch is opt-in only, and off by default.** For the case a
 * message arrives with no attached card (an older sender, or a client that
 * never unfurled it) and the *receiving* user explicitly wants a preview
 * anyway, [`fetch_preview`] exists behind the `unfurl-http` cargo feature —
 * the same shape as `media.rs`'s `media-http` guard. A frontend may only
 * reach it from a setting the user turned on themselves ("load previews for
 * links I receive"); nothing in this crate calls it on the receive path.
 *
 * **Wire format.** [`attach_preview`] appends the preview as a suffix after
 * the message text, marked the way `dm.rs`'s control envelopes and
 * `note.rs`'s journal marker are — a token nobody but this feature writes,
 * so an older client (or one that never learns to parse it) still renders
 * the actual words the sender typed, followed by one line of preview data it
 * shows as plain text rather than losing entirely. That is also why the
 * marker rides as a *suffix* rather than `note.rs`'s prefix: a shared journal
 * entry *is* the whole message, but the text around a link is the sender's
 * own and must survive untouched for a client that does not understand the
 * marker. As with every marker in this codebase, this is a **label, not an
 * attestation** — anyone can type it by hand, so [`split_preview`] treats a
 * match as "the sending Comrade attached this card", never as proof, and a
 * marker that fails to parse degrades to plain text rather than erroring.
 *
 * Everything above the `unfurl-http` feature line is pure, unconditionally
 * compiled, and fully unit-tested.
 */

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::UnfurlError;

// ── Field caps ────────────────────────────────────────────────────────────────
//
// A hostile (or just sloppy) page can put arbitrarily long text in an `<meta>`
// tag. None of these are hard protocol limits — they exist so a card cannot
// grow to the size of the screen it is drawn on. Truncation is on `char`
// boundaries so a multi-byte codepoint is never split into invalid UTF-8.

/// Longest a card's title may be after [`parse_preview`].
pub const MAX_PREVIEW_TITLE_LEN: usize = 300;
/// Longest a card's description may be after [`parse_preview`].
pub const MAX_PREVIEW_DESCRIPTION_LEN: usize = 500;
/// Longest a card's site name may be after [`parse_preview`].
pub const MAX_PREVIEW_SITE_NAME_LEN: usize = 100;
/// Longest a URL field (the page URL, its canonical form, or its image) may
/// be after [`parse_preview`].
pub const MAX_PREVIEW_URL_LEN: usize = 2048;

fn cap_len(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

// ── The card itself ───────────────────────────────────────────────────────────

/// What kind of thing a link points at, as the page's own `og:type`/
/// `twitter:card` tags describe it. Purely descriptive — a frontend may use
/// it to pick a layout (a `Photo` card can skip the title line a `Video` one
/// needs) but nothing here treats it as anything stronger than a hint the
/// linked page volunteered about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewKind {
    Article,
    Photo,
    Video,
    Profile,
    Unknown,
}

/// A link preview card, built once by the sender and carried with the
/// message from then on. See the module doc for why this crosses the wire
/// instead of being re-derived on read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkPreview {
    /// The URL exactly as it appeared in the message (after
    /// [`first_previewable_url`] normalisation, if it was bare).
    pub url: String,
    /// The page's own idea of its canonical address (`og:url`), or [`Self::url`]
    /// again when the page does not declare one. Never used for
    /// [`display_domain`] — see the module doc for why.
    pub canonical_url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_name: Option<String>,
    /// Resolved to an absolute URL against the page's own address if the page
    /// gave a relative one.
    pub image_url: Option<String>,
    pub kind: PreviewKind,
}

// ── Finding a link in a message ──────────────────────────────────────────────

/// Scheme-qualified, `www.`-prefixed, or bare-with-a-path URLs. A bare domain
/// with *no* path and no `www.` (`"see example.com"`) is deliberately not
/// matched — the alternative is a public-suffix table to tell a real TLD from
/// "3.14" or "e.g.", which this module does not carry.
const URL_PATTERN: &str = r#"(?ix)
    (?: https?://[^\s<>"'`]+ )
  | (?: www\.[^\s<>"'`]+ )
  | (?: [a-z0-9][a-z0-9-]*(?:\.[a-z0-9][a-z0-9-]*)+\.[a-z]{2,24}/[^\s<>"'`]* )
"#;

fn url_regex() -> &'static Regex {
    static URL_RE: OnceLock<Regex> = OnceLock::new();
    URL_RE.get_or_init(|| Regex::new(URL_PATTERN).expect("URL_PATTERN is a valid regex"))
}

/// Segments of `text` outside of single-backtick code spans, in order.
///
/// `text.split('`')` alternates outside/inside/outside/…, so every
/// even-indexed piece (`step_by(2)` starting at the first) is outside a span.
/// An unterminated trailing backtick drops whatever follows it — the same
/// "malformed markup loses the tail" trade a markdown renderer makes, and
/// better than the alternative of treating a stray backtick as plain text and
/// unfurling a link the sender fenced off on purpose.
fn outside_code_spans(text: &str) -> impl Iterator<Item = &str> {
    text.split('`').step_by(2)
}

/// Trim trailing punctuation that ends a *sentence* rather than the URL:
/// `.`, `,`, `;`, `:`, `!`, `?`, quotes, and a closing `)`/`]` that has no
/// matching opener in what is left — which is exactly what leaves a Wikipedia
/// URL's balanced `(programming_language)` alone while still stripping the
/// closing paren of `(see https://example.com)`.
fn trim_trailing_punctuation(raw: &str) -> &str {
    let mut end = raw.len();
    loop {
        let candidate = &raw[..end];
        let Some(last) = candidate.chars().next_back() else {
            break;
        };
        let strip = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '\u{2019}' | '\u{201d}' | '*' => true,
            ')' => candidate.matches(')').count() > candidate.matches('(').count(),
            ']' => candidate.matches(']').count() > candidate.matches('[').count(),
            _ => false,
        };
        if !strip {
            break;
        }
        end -= last.len_utf8();
    }
    &raw[..end]
}

/// Every URL in `text` — bare or scheme-qualified, outside of code spans,
/// with sentence-ending punctuation trimmed off. See [`URL_PATTERN`] for
/// exactly what counts as bare.
pub fn extract_urls(text: &str) -> Vec<String> {
    let re = url_regex();
    let mut out = Vec::new();
    for segment in outside_code_spans(text) {
        for m in re.find_iter(segment) {
            let trimmed = trim_trailing_punctuation(m.as_str());
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
    }
    out
}

/// The one URL a message earns a preview card for — Telegram's rule: the
/// first http(s) link only, never one card per URL. A bare match is
/// normalised to `https://` here, since that is what any client would do
/// with it before fetching.
pub fn first_previewable_url(text: &str) -> Option<String> {
    let url = extract_urls(text).into_iter().next()?;
    let lower = url.to_ascii_lowercase();
    Some(
        if lower.starts_with("https://") || lower.starts_with("http://") {
            url
        } else {
            format!("https://{url}")
        },
    )
}

// ── The card shown on the message — read straight from the URL ─────────────

/// The domain a card shows, and the anti-spoof affordance: read from `url`
/// itself, never from [`LinkPreview::site_name`] (a page's `og:site_name` can
/// claim to be anything) or [`LinkPreview::canonical_url`] (equally
/// page-supplied). `www.` is stripped and the host lowercased, matching how a
/// person reads a domain rather than how a browser's address bar spells it.
pub fn display_domain(url: &str) -> Option<String> {
    let parsed = Url::parse(url)
        .or_else(|_| Url::parse(&format!("https://{url}")))
        .ok()?;
    let host = parsed.host_str()?;
    Some(
        host.strip_prefix("www.")
            .unwrap_or(host)
            .to_ascii_lowercase(),
    )
}

// ── Parsing a fetched page into a card ───────────────────────────────────────

fn attr_value(tag_attrs: &str, attr: &str) -> Option<String> {
    let escaped = regex::escape(attr);
    let pattern = format!(r#"(?is){escaped}\s*=\s*"([^"]*)"|{escaped}\s*=\s*'([^']*)'"#);
    let re = Regex::new(&pattern).ok()?;
    let caps = re.captures(tag_attrs)?;
    caps.get(1)
        .or_else(|| caps.get(2))
        .map(|m| m.as_str().to_string())
}

/// `(key, content)` for every `<meta property="…">`/`<meta name="…">` tag —
/// `key` lowercased so `OG:Title` and `og:title` are the same lookup.
fn collect_meta_tags(html: &str) -> Vec<(String, String)> {
    static META_RE: OnceLock<Regex> = OnceLock::new();
    let re = META_RE.get_or_init(|| Regex::new(r"(?is)<meta\b([^>]*)>").expect("valid regex"));
    re.captures_iter(html)
        .filter_map(|caps| {
            let attrs = caps.get(1)?.as_str();
            let key = attr_value(attrs, "property").or_else(|| attr_value(attrs, "name"))?;
            let content = attr_value(attrs, "content")?;
            Some((key.to_ascii_lowercase(), content))
        })
        .collect()
}

fn meta_value(metas: &[(String, String)], key: &str) -> Option<String> {
    metas.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

fn extract_title_tag(html: &str) -> Option<String> {
    static TITLE_RE: OnceLock<Regex> = OnceLock::new();
    let re = TITLE_RE
        .get_or_init(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").expect("valid regex"));
    let text = re.captures(html)?.get(1)?.as_str().trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Decode the handful of HTML entities that actually turn up in page titles
/// and descriptions, plus numeric references. Anything else is left as-is
/// rather than guessed at — an unrecognised entity rendered literally is a
/// cosmetic wart, not a wrong card.
fn decode_entities(s: &str) -> String {
    static ENTITY_RE: OnceLock<Regex> = OnceLock::new();
    let re = ENTITY_RE
        .get_or_init(|| Regex::new(r"&(#x?[0-9a-fA-F]+|[a-zA-Z]+);").expect("valid regex"));
    re.replace_all(s, |caps: &regex::Captures<'_>| {
        let body = &caps[1];
        match body {
            "amp" => "&".to_string(),
            "lt" => "<".to_string(),
            "gt" => ">".to_string(),
            "quot" => "\"".to_string(),
            "apos" => "'".to_string(),
            "nbsp" => " ".to_string(),
            _ if body.starts_with("#x") || body.starts_with("#X") => {
                u32::from_str_radix(&body[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
                    .map(String::from)
                    .unwrap_or_else(|| caps[0].to_string())
            }
            _ if body.starts_with('#') => body[1..]
                .parse::<u32>()
                .ok()
                .and_then(char::from_u32)
                .map(String::from)
                .unwrap_or_else(|| caps[0].to_string()),
            _ => caps[0].to_string(),
        }
    })
    .to_string()
}

/// Resolve `candidate` (possibly relative) against `base`. `None` if neither
/// parses — a page's `og:image` pointing at garbage should drop the image,
/// not poison the rest of the card.
fn resolve_url(base: &str, candidate: &str) -> Option<String> {
    let candidate = candidate.trim();
    if candidate.is_empty() {
        return None;
    }
    match Url::parse(candidate) {
        Ok(u) => Some(u.to_string()),
        Err(_) => Url::parse(base)
            .ok()?
            .join(candidate)
            .ok()
            .map(|u| u.to_string()),
    }
}

fn kind_from_tags(og_type: Option<&str>, twitter_card: Option<&str>) -> PreviewKind {
    if let Some(t) = og_type.map(|t| t.trim().to_ascii_lowercase()) {
        if t.starts_with("article") {
            return PreviewKind::Article;
        }
        if t.starts_with("video") {
            return PreviewKind::Video;
        }
        if t.starts_with("profile") {
            return PreviewKind::Profile;
        }
        if t == "image" || t == "photo" {
            return PreviewKind::Photo;
        }
    }
    if let Some(c) = twitter_card.map(|c| c.trim().to_ascii_lowercase()) {
        if c == "photo" {
            return PreviewKind::Photo;
        }
        if c == "player" {
            return PreviewKind::Video;
        }
    }
    PreviewKind::Unknown
}

/// Build a [`LinkPreview`] from a fetched page's HTML. The fallback chain is
/// Telegram/Twitter's: OpenGraph first, then the Twitter card tags, then the
/// plain `<title>`/`<meta name="description">` every page has. `source_url`
/// is what a relative `og:image` resolves against and what [`Self`] falls
/// back to when the page names no canonical URL of its own.
pub fn parse_preview(html: &str, source_url: &str) -> LinkPreview {
    let metas = collect_meta_tags(html);

    let og_title = meta_value(&metas, "og:title");
    let og_description = meta_value(&metas, "og:description");
    let og_site_name = meta_value(&metas, "og:site_name");
    let og_image = meta_value(&metas, "og:image");
    let og_url = meta_value(&metas, "og:url");
    let og_type = meta_value(&metas, "og:type");

    let tw_title = meta_value(&metas, "twitter:title");
    let tw_description = meta_value(&metas, "twitter:description");
    let tw_image = meta_value(&metas, "twitter:image");
    let tw_card = meta_value(&metas, "twitter:card");

    let plain_description = meta_value(&metas, "description");
    let plain_title = extract_title_tag(html);

    let title = og_title
        .or(tw_title)
        .or(plain_title)
        .map(|t| cap_len(&decode_entities(&t), MAX_PREVIEW_TITLE_LEN));
    let description = og_description
        .or(tw_description)
        .or(plain_description)
        .map(|d| cap_len(&decode_entities(&d), MAX_PREVIEW_DESCRIPTION_LEN));
    let site_name = og_site_name.map(|s| cap_len(&decode_entities(&s), MAX_PREVIEW_SITE_NAME_LEN));

    let image_url = og_image
        .or(tw_image)
        .and_then(|raw| resolve_url(source_url, &raw))
        .map(|u| cap_len(&u, MAX_PREVIEW_URL_LEN));

    let canonical_url = og_url
        .and_then(|raw| resolve_url(source_url, &raw))
        .unwrap_or_else(|| source_url.to_string());

    LinkPreview {
        url: cap_len(source_url, MAX_PREVIEW_URL_LEN),
        canonical_url: cap_len(&canonical_url, MAX_PREVIEW_URL_LEN),
        title,
        description,
        site_name,
        image_url,
        kind: kind_from_tags(og_type.as_deref(), tw_card.as_deref()),
    }
}

// ── Wire form: riding an ordinary DM body ────────────────────────────────────

/// Newline + marker that begins a link-preview suffix. Carries its own
/// leading newline so [`split_preview`] can search for it in one pass with no
/// extra allocation.
const LINK_PREVIEW_SUFFIX: &str = "\ncomrade-preview:";

/// Attach `preview` to `body`, riding as a suffix so an older client (or a
/// frontend that has not learned to parse it yet) still shows exactly what
/// the sender typed. See the module doc for why a suffix and not `note.rs`'s
/// prefix.
pub fn attach_preview(body: &str, preview: &LinkPreview) -> String {
    // Every field is a `String`/`Option<String>`/plain enum, so this cannot
    // fail in practice; a failure here would be a serde bug, not attacker
    // input, and `unwrap_or_default` degrades to an empty marker rather than
    // panicking on the (unreachable) error path.
    let json = serde_json::to_string(preview).unwrap_or_default();
    format!("{body}{LINK_PREVIEW_SUFFIX}{json}")
}

/// Split a stored/wire message body into the text a bubble draws and the
/// preview it carries, if any.
///
/// A malformed marker — the JSON does not parse as a [`LinkPreview`] —
/// degrades to the untouched body and `None`, exactly like `note.rs`'s
/// header-with-no-note case: never invent a card from data that did not
/// parse, and never error on it either.
pub fn split_preview(body: &str) -> (String, Option<LinkPreview>) {
    match body.rsplit_once(LINK_PREVIEW_SUFFIX) {
        Some((text, json)) => match serde_json::from_str::<LinkPreview>(json) {
            Ok(preview) => (text.to_string(), Some(preview)),
            Err(_) => (body.to_string(), None),
        },
        None => (body.to_string(), None),
    }
}

// ── Receiver-side fetch: opt-in only, off by default ─────────────────────────

#[cfg(feature = "unfurl-http")]
mod http {
    use super::*;
    use std::time::Duration;

    /// A `<head>` fits comfortably in this; a page that has not said
    /// everything it needs to in 256 KiB is not one worth buffering further
    /// for a preview card.
    pub const MAX_PREVIEW_BYTES: usize = 256 * 1024;

    /// Same reasoning as `media.rs`'s `CONNECT_TIMEOUT`: short, and separate
    /// from the transfer timeout, so a host that never accepts the connection
    /// fails fast instead of spinning for the full transfer budget.
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
    const TRANSFER_TIMEOUT: Duration = Duration::from_secs(20);
    /// A shortener chain is normal for a shared link; an open-ended one is
    /// not, and an SSRF-via-redirect attempt looks exactly like the latter.
    const MAX_REDIRECTS: usize = 5;

    /// Fetch `url` (opted into explicitly by the *receiving* user — see the
    /// module doc) and parse it into a [`LinkPreview`].
    ///
    /// HTTPS-only, refused before a socket opens; a bounded redirect chain
    /// rather than none at all, since ordinary shared links (link shorteners)
    /// routinely redirect once or twice; a connect and a transfer timeout;
    /// and the body capped at [`MAX_PREVIEW_BYTES`], checked against
    /// `Content-Length` up front and again while streaming, since the header
    /// may be absent or lie — the same two-layer guard `media.rs`'s
    /// `fetch_guarded_bytes` uses, duplicated rather than shared so this
    /// feature and `media-http` stay independently toggleable.
    pub async fn fetch_preview(url: &str) -> Result<LinkPreview, UnfurlError> {
        let is_https = url
            .split_once("://")
            .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("https"));
        if !is_https {
            return Err(UnfurlError::Http(
                "refusing to fetch a non-HTTPS URL for a link preview".into(),
            ));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TRANSFER_TIMEOUT)
            .build()
            .map_err(|e| UnfurlError::Http(e.to_string()))?;
        let mut resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| UnfurlError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(UnfurlError::Http(format!("fetch status {}", resp.status())));
        }
        if let Some(len) = resp.content_length() {
            if len > MAX_PREVIEW_BYTES as u64 {
                return Err(UnfurlError::TooLarge {
                    size: len,
                    max: MAX_PREVIEW_BYTES as u64,
                });
            }
        }
        // Bounded streaming read — never buffer more than the cap even if the
        // server omits or understates Content-Length.
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| UnfurlError::Http(e.to_string()))?
        {
            if bytes.len() + chunk.len() > MAX_PREVIEW_BYTES {
                return Err(UnfurlError::TooLarge {
                    size: (bytes.len() + chunk.len()) as u64,
                    max: MAX_PREVIEW_BYTES as u64,
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        let html = String::from_utf8_lossy(&bytes);
        Ok(parse_preview(&html, url))
    }
}

#[cfg(feature = "unfurl-http")]
pub use http::{fetch_preview, MAX_PREVIEW_BYTES};

/// Stub so the rest of the workspace compiles (and degrades gracefully) when
/// `unfurl-http` is off — a caller behind the opt-in setting calls this
/// unconditionally and only finds out here that the build cannot reach a
/// socket for it.
#[cfg(not(feature = "unfurl-http"))]
pub async fn fetch_preview(_url: &str) -> Result<LinkPreview, UnfurlError> {
    Err(UnfurlError::FeatureDisabled)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(url: &str) -> LinkPreview {
        LinkPreview {
            url: url.to_string(),
            canonical_url: url.to_string(),
            title: Some("A Title".into()),
            description: Some("A description.".into()),
            site_name: Some("Example".into()),
            image_url: Some("https://example.com/card.png".into()),
            kind: PreviewKind::Article,
        }
    }

    // ── extract_urls ──────────────────────────────────────────────────────

    #[test]
    fn extracts_bare_and_scheme_qualified_urls() {
        let text = "see https://example.com/a and also www.example.org/b, thanks";
        assert_eq!(
            extract_urls(text),
            vec![
                "https://example.com/a".to_string(),
                "www.example.org/b".to_string(),
            ]
        );
    }

    #[test]
    fn does_not_match_a_url_inside_a_code_span() {
        let text = "run `curl https://example.com/api` then open https://real.example";
        assert_eq!(extract_urls(text), vec!["https://real.example".to_string()]);
    }

    #[test]
    fn strips_sentence_ending_punctuation_but_not_the_url() {
        assert_eq!(
            extract_urls("check this out: https://example.com/page."),
            vec!["https://example.com/page".to_string()]
        );
        assert_eq!(
            extract_urls("(see https://example.com)"),
            vec!["https://example.com".to_string()]
        );
        assert_eq!(
            extract_urls("is this it? https://example.com/x!"),
            vec!["https://example.com/x".to_string()]
        );
    }

    #[test]
    fn balanced_parens_are_part_of_the_url() {
        // Wikipedia links routinely disambiguate with a trailing parenthetical
        // that is part of the path, not a sentence's own parens.
        let text = "https://en.wikipedia.org/wiki/Rust_(programming_language).";
        assert_eq!(
            extract_urls(text),
            vec!["https://en.wikipedia.org/wiki/Rust_(programming_language)".to_string()]
        );
    }

    #[test]
    fn a_bare_domain_with_no_path_and_no_www_is_not_matched() {
        // The known limitation the module doc names: distinguishing "example.com"
        // from "3.14" needs a TLD table this module does not carry.
        assert!(extract_urls("my version is 3.14, see you at noon").is_empty());
    }

    // ── first_previewable_url ─────────────────────────────────────────────

    #[test]
    fn first_previewable_url_is_the_first_one_only() {
        let text = "first https://one.example then https://two.example";
        assert_eq!(
            first_previewable_url(text),
            Some("https://one.example".to_string())
        );
    }

    #[test]
    fn first_previewable_url_normalises_a_bare_match() {
        assert_eq!(
            first_previewable_url("go to www.example.com/page"),
            Some("https://www.example.com/page".to_string())
        );
    }

    #[test]
    fn first_previewable_url_is_none_with_no_link() {
        assert_eq!(first_previewable_url("just chatting, no links here"), None);
    }

    // ── display_domain ────────────────────────────────────────────────────

    #[test]
    fn display_domain_strips_www_and_lowercases() {
        assert_eq!(
            display_domain("https://WWW.Example.COM/path?x=1"),
            Some("example.com".to_string())
        );
        assert_eq!(
            display_domain("https://sub.example.com/path"),
            Some("sub.example.com".to_string())
        );
    }

    #[test]
    fn display_domain_reads_the_url_never_the_claimed_site_name() {
        // The anti-spoof affordance: a card claiming `site_name: "Your Bank"`
        // must still show the real host underneath it.
        let card = LinkPreview {
            site_name: Some("Your Bank".into()),
            ..preview("https://totally-not-a-bank.example/login")
        };
        assert_eq!(
            display_domain(&card.url),
            Some("totally-not-a-bank.example".to_string())
        );
        assert_ne!(
            card.site_name.as_deref(),
            display_domain(&card.url).as_deref()
        );
    }

    // ── parse_preview ─────────────────────────────────────────────────────

    #[test]
    fn opengraph_wins_over_twitter_and_plain_tags() {
        let html = r#"
            <html><head>
            <title>Plain Title</title>
            <meta name="description" content="Plain description">
            <meta name="twitter:title" content="Twitter Title">
            <meta name="twitter:description" content="Twitter description">
            <meta property="og:title" content="OG Title">
            <meta property="og:description" content="OG description">
            <meta property="og:site_name" content="Example Site">
            <meta property="og:type" content="article">
            </head></html>
        "#;
        let card = parse_preview(html, "https://example.com/story");
        assert_eq!(card.title.as_deref(), Some("OG Title"));
        assert_eq!(card.description.as_deref(), Some("OG description"));
        assert_eq!(card.site_name.as_deref(), Some("Example Site"));
        assert_eq!(card.kind, PreviewKind::Article);
    }

    #[test]
    fn falls_back_through_twitter_then_plain_tags() {
        let html = r#"
            <title>Plain Title</title>
            <meta name="description" content="Plain description">
        "#;
        let card = parse_preview(html, "https://example.com/story");
        assert_eq!(card.title.as_deref(), Some("Plain Title"));
        assert_eq!(card.description.as_deref(), Some("Plain description"));

        let html_twitter = r#"
            <title>Plain Title</title>
            <meta name="twitter:title" content="Twitter Title">
            <meta name="twitter:description" content="Twitter description">
        "#;
        let card2 = parse_preview(html_twitter, "https://example.com/story");
        assert_eq!(card2.title.as_deref(), Some("Twitter Title"));
        assert_eq!(card2.description.as_deref(), Some("Twitter description"));
    }

    #[test]
    fn relative_og_image_resolves_against_the_source_url() {
        let html = r#"<meta property="og:image" content="/static/card.png">"#;
        let card = parse_preview(html, "https://example.com/blog/post");
        assert_eq!(
            card.image_url.as_deref(),
            Some("https://example.com/static/card.png")
        );
    }

    #[test]
    fn an_absolute_og_image_is_kept_as_is() {
        let html = r#"<meta property="og:image" content="https://cdn.example/card.png">"#;
        let card = parse_preview(html, "https://example.com/blog/post");
        assert_eq!(
            card.image_url.as_deref(),
            Some("https://cdn.example/card.png")
        );
    }

    #[test]
    fn canonical_url_falls_back_to_the_source_when_the_page_names_none() {
        let card = parse_preview("<title>x</title>", "https://example.com/a");
        assert_eq!(card.canonical_url, "https://example.com/a");
    }

    #[test]
    fn og_type_maps_to_the_matching_kind() {
        for (og_type, expected) in [
            ("article", PreviewKind::Article),
            ("video.movie", PreviewKind::Video),
            ("profile", PreviewKind::Profile),
            ("website", PreviewKind::Unknown),
        ] {
            let html = format!(r#"<meta property="og:type" content="{og_type}">"#);
            assert_eq!(
                parse_preview(&html, "https://example.com").kind,
                expected,
                "og:type={og_type}"
            );
        }
    }

    #[test]
    fn html_entities_in_text_fields_are_decoded() {
        let html = r#"<title>Fish &amp; Chips &mdash; caf&#233;</title>"#;
        let card = parse_preview(html, "https://example.com");
        // `&mdash;` is not in the small recognised set and is left as-is —
        // pinning that this degrades gracefully rather than guessing.
        assert_eq!(card.title.as_deref(), Some("Fish & Chips &mdash; café"));
    }

    #[test]
    fn every_text_field_is_capped_so_a_hostile_page_cannot_fill_the_screen() {
        let huge = "x".repeat(10_000);
        let html = format!(
            r#"<meta property="og:title" content="{huge}">
               <meta property="og:description" content="{huge}">
               <meta property="og:site_name" content="{huge}">
               <meta property="og:url" content="https://example.com/{huge}">
               <meta property="og:image" content="https://example.com/{huge}">"#
        );
        let card = parse_preview(&html, "https://example.com");
        assert_eq!(card.title.unwrap().chars().count(), MAX_PREVIEW_TITLE_LEN);
        assert_eq!(
            card.description.unwrap().chars().count(),
            MAX_PREVIEW_DESCRIPTION_LEN
        );
        assert_eq!(
            card.site_name.unwrap().chars().count(),
            MAX_PREVIEW_SITE_NAME_LEN
        );
        assert_eq!(card.canonical_url.chars().count(), MAX_PREVIEW_URL_LEN);
        assert_eq!(card.image_url.unwrap().chars().count(), MAX_PREVIEW_URL_LEN);
    }

    #[test]
    fn a_page_with_no_recognised_tags_still_yields_a_usable_card() {
        let card = parse_preview(
            "<html><body>nothing here</body></html>",
            "https://x.example",
        );
        assert_eq!(card.url, "https://x.example");
        assert_eq!(card.canonical_url, "https://x.example");
        assert_eq!(card.title, None);
        assert_eq!(card.description, None);
        assert_eq!(card.site_name, None);
        assert_eq!(card.image_url, None);
        assert_eq!(card.kind, PreviewKind::Unknown);
    }

    // ── wire format ───────────────────────────────────────────────────────

    #[test]
    fn attach_then_split_round_trips() {
        let card = preview("https://example.com/story");
        let body = attach_preview("check this out", &card);
        assert!(body.starts_with("check this out\ncomrade-preview:"));
        let (text, parsed) = split_preview(&body);
        assert_eq!(text, "check this out");
        assert_eq!(parsed, Some(card));
    }

    #[test]
    fn an_ordinary_message_has_no_preview() {
        let (text, parsed) = split_preview("just a normal message, no card here");
        assert_eq!(text, "just a normal message, no card here");
        assert_eq!(parsed, None);
    }

    #[test]
    fn a_malformed_marker_degrades_to_plain_text_never_errors() {
        let body = "look at this\ncomrade-preview:{not valid json";
        let (text, parsed) = split_preview(body);
        // The whole body, marker included, is shown rather than guessed at —
        // never invent a card, and never lose the message either.
        assert_eq!(text, body);
        assert_eq!(parsed, None);
    }

    #[test]
    fn a_hand_typed_marker_is_a_label_not_an_attestation() {
        // Same standing as `note.rs`'s and `dm.rs`'s markers: anybody can type
        // this, so a match means "the sending Comrade attached this", never
        // proof of anything about the page.
        let card = preview("https://example.com");
        let json = serde_json::to_string(&card).unwrap();
        let typed = format!("hand typed\ncomrade-preview:{json}");
        let (text, parsed) = split_preview(&typed);
        assert_eq!(text, "hand typed");
        assert_eq!(parsed, Some(card));
    }

    // ── feature-gated fetch ───────────────────────────────────────────────

    #[cfg(not(feature = "unfurl-http"))]
    #[tokio::test]
    async fn fetch_preview_degrades_gracefully_without_the_feature() {
        assert!(matches!(
            fetch_preview("https://example.com").await,
            Err(UnfurlError::FeatureDisabled)
        ));
    }

    #[cfg(feature = "unfurl-http")]
    #[tokio::test]
    async fn fetch_preview_rejects_non_https_before_any_request() {
        for url in ["http://example.com", "file:///etc/passwd", "ftp://x/y", ""] {
            let err = fetch_preview(url).await;
            assert!(
                matches!(err, Err(UnfurlError::Http(_))),
                "must reject {url:?}"
            );
        }
    }
}
