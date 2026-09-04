/**
 * The Telegram-style link-preview card: whether a message earns one, and what
 * domain it names.
 *
 * Mirrors `android/app/src/main/java/mullu/comrade/ui/LinkPreviewDecisions.kt`
 * — same cases, same answers, ported vector-for-vector in
 * `link_preview.test.mjs`. Kept free of DOM/Tauri imports, like
 * `message_actions.mjs` — the fetch itself (following the URL, reading
 * `title`/`description`/`site_name`/an image) is `comrade_core::unfurl`'s job,
 * run once on the *sending* device and carried to this one on the wire as
 * `MessageDto.link_preview` (see `comrade_ui::runtime::LinkPreviewDto`, which
 * already computes `display_domain` on the Rust side before this module ever
 * sees the message — this file exists for the composer's own "you're about to
 * send a link" affordance and so desktop's answer is provably the same one
 * Android gives, not because the receive path needs it recomputed).
 *
 * **The domain shown is derived from the URL, never from `site_name`, and
 * there is no parameter here that could change that.** A link preview's
 * `title`, `description` and `site_name` are supplied by whatever page the
 * URL points to — which is to say, by the sender, since they chose the URL.
 * A message card is trusted chrome: it sits in the bubble looking like
 * Comrade vouches for it. If the domain line could be filled from
 * `site_name`, a message linking to
 * `https://paypal-secure.example.net/login` could carry a `site_name` of
 * "PayPal" and the card would say one site while the link goes to another —
 * the exact bait a phishing preview needs. `displayDomain` takes only the
 * URL. Not "ignores `site_name` if given" — it is never given one, so there
 * is no call site left that could pass it in a year from now and quietly
 * reopen this.
 */

/**
 * A bare `http(s)://` URL in already-typed message text.
 *
 * Trailing punctuation most people leave attached to a link when they write a
 * sentence around it — `.`, `,`, `!`, `?`, closing brackets and quotes — is
 * trimmed off the match, because none of those characters end a real URL path
 * often enough to be worth breaking the far more common "check this out:
 * https://example.com/foo." case.
 */
const URL_REGEX = /https?:\/\/\S+/;
const TRAILING_PUNCTUATION = new Set([
  ".", ",", "!", "?", ")", "]", "}", "\"", "'",
]);

function trimTrailingPunctuation(raw) {
  let end = raw.length;
  while (end > 0 && TRAILING_PUNCTUATION.has(raw[end - 1])) end -= 1;
  return raw.slice(0, end);
}

/**
 * The first URL in `text`, or `null` if it has none.
 *
 * The *first* one, deliberately, even when a message names several: one card
 * per message is the same restraint the quick-reaction row's single emoji
 * has, and picking the first matches what the sender typed first rather than
 * an arbitrary later link nobody was looking at when they hit send.
 */
export function firstUrl(text) {
  const match = URL_REGEX.exec(text || "");
  if (!match) return null;
  const trimmed = trimTrailingPunctuation(match[0]);
  return trimmed.length > 0 ? trimmed : null;
}

/**
 * Whether `text` earns a preview card at all.
 *
 * Yes even when `text` is *only* the URL and nothing else — that is the
 * single most common way a link gets shared, not an edge case to
 * special-case away. Suppressing the card there would remove the preview for
 * exactly the message it exists to serve, and unlike a card competing with a
 * paragraph of typed text, a bare-link message has nothing else in the
 * bubble for it to crowd.
 */
export function hasLinkPreview(text) {
  return firstUrl(text) !== null;
}

/**
 * The domain `displayDomain` would put on the card for the message's first
 * URL, or `null` when `text` carries none. `{ url, domain }`.
 */
export function linkPreviewFor(text) {
  const url = firstUrl(text);
  if (url === null) return null;
  const domain = displayDomain(url);
  if (domain === null) return null;
  return { url, domain };
}

/**
 * The host `url` actually resolves to, lower-cased and stripped of a leading
 * `www.` — never anything supplied by the page itself. See the file header
 * for why this takes no other input.
 *
 * Parsed with the platform `URL` constructor rather than a
 * `substringAfter("://")` scan on purpose, for the identical reason Android
 * parses with `java.net.URI` rather than hand-rolling it:
 * `https://example.com@evil.example/path` is a URL a naive parser can easily
 * read as `example.com` (everything between the scheme and the next `/`),
 * when the host a real client connects to — and the one this card must name
 * — is `evil.example`. The WHATWG `URL` the browser/webview already ships
 * resolves the authority component the same correct way `URI.getHost()`
 * does, so this only has to trust it.
 *
 * `null` for anything `URL` cannot parse, or that parses with no host at all
 * (a `file:`-style URL with an empty authority, the WHATWG equivalent of
 * Java's `null` host) — a broken or hostless URL gets no card rather than a
 * guessed one.
 */
export function displayDomain(url) {
  let host;
  try {
    host = new URL(url).hostname;
  } catch {
    return null;
  }
  if (!host) return null;
  const lower = host.toLowerCase();
  return lower.startsWith("www.") ? lower.slice(4) : lower;
}
