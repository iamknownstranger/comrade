/*!
 * public_sources — Internet Archive, podcast feeds, LRCLIB.
 *
 * Three more residents of §23's pluggable tier, chosen by one shared rule:
 * **keyless, HTTPS, and serving plain progressive media by design** — the same
 * line `subsonic.rs` sits behind, extended to sources nobody has to configure:
 *
 * - **Internet Archive** — millions of public-domain and Creative Commons
 *   recordings with an open search API and direct file URLs. The closest thing
 *   the open web has to BlackHole's catalogue-and-stream experience, minus the
 *   part this app will not do: nothing here is extracted from anyone.
 * - **Podcast feeds** — §11a named the podcast episode the best-syncing online
 *   source there is and built `TogetherContent::Stream` around exactly this
 *   shape. A feed URL pasted once yields every episode as a guarded candidate.
 * - **LRCLIB** — community-synced lyrics, keyless, for the now-playing sheet.
 *
 * Everything that leaves this module has already been through
 * [`crate::together::valid_stream_url`] (and, where a whole URL is returned,
 * [`crate::together::direct_media_url`] — these sources name real files, so
 * the stricter guard applies and passing it is the point). What reaches a
 * screen is what a session accepts.
 *
 * Parsing is pure and fixture-tested; sockets sit under `catalogue-http`
 * behind bounded reads and no redirects, like every other lane here.
 */

use crate::together::{direct_media_url, valid_stream_url};

#[cfg(feature = "catalogue-http")]
const LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);
#[cfg(feature = "catalogue-http")]
const MAX_LOOKUP_BYTES: usize = 512 * 1024;

/// Rows one listing may return. Same arithmetic as everywhere else: a picker
/// shows a few.
pub const MAX_ITEMS: usize = 8;
pub const MAX_TRACKS: usize = 40;

// ── Shared shapes ────────────────────────────────────────────────────────────

/// One playable answer, shaped for reuse across every source here. The URL is
/// final — ready for a player or a Together invitation — and guaranteed to
/// have passed both guards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: u64,
    pub url: String,
    pub artwork_url: Option<String>,
}

/// Guard and construct in one step: anything that fails either guard becomes
/// no row, never a broken one.
fn guarded_track(
    title: String,
    artist: String,
    album: Option<String>,
    duration_ms: u64,
    url: String,
    artwork_url: Option<String>,
) -> Option<PublicTrack> {
    let ok = valid_stream_url(&url) && direct_media_url(&url);
    ok.then_some(PublicTrack {
        title,
        artist,
        album,
        duration_ms,
        url,
        artwork_url,
    })
}

/// Percent-encode a query-string component (see `subsonic::urlencode`; third
/// copy would be one too many — this module IS the third, so the next move is
/// a shared util module, recorded here so it happens deliberately).
pub(crate) fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(feature = "catalogue-http")]
async fn fetch_bounded(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(LOOKUP_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status()));
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_LOOKUP_BYTES as u64 {
            return Err(format!("response exceeded {MAX_LOOKUP_BYTES} bytes"));
        }
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if buf.len() + chunk.len() > MAX_LOOKUP_BYTES {
            return Err(format!("response exceeded {MAX_LOOKUP_BYTES} bytes"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

// ── Internet Archive ─────────────────────────────────────────────────────────

/// One Archive collection: an item identifier plus the words to show for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveItem {
    pub identifier: String,
    pub title: String,
    pub creator: String,
}

/// Parse the advancedsearch response down to items.
///
/// Pure. The API answers JSON with a `response.docs[]` array; missing fields
/// become empty strings rather than dropped rows, because a recording whose
/// creator line is absent is still playable — only the identifier decides.
pub fn parse_archive_search(body: &[u8]) -> Result<Vec<ArchiveItem>, String> {
    let doc: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("unreadable JSON: {e}"))?;
    let docs = doc
        .get("response")
        .and_then(|r| r.get("docs"))
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(docs
        .iter()
        .take(MAX_ITEMS)
        .filter_map(|item| {
            let identifier = item.get("identifier")?.as_str()?.to_string();
            if identifier.is_empty() {
                return None;
            }
            Some(ArchiveItem {
                identifier,
                title: item
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Untitled recording")
                    .to_string(),
                creator: item
                    .get("creator")
                    // The API sends either a string or an array of strings.
                    .map(|c| match c {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Array(a) => a
                            .iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                        _ => String::new(),
                    })
                    .unwrap_or_default(),
            })
        })
        .collect())
}

/// Build the advancedsearch URL for one free-text query.
pub fn archive_search_url(query: &str) -> String {
    format!(
        "https://archive.org/advancedsearch.php?q={}&fl%5B%5D=identifier&fl%5B%5D=title&fl%5B%5D=creator&rows={MAX_ITEMS}&page=1&output=json",
        // The query goes inside parentheses the API treats as a phrase; quotes
        // themselves are escaped by the encoder, which is enough.
        urlencode(&format!("({query}) AND mediatype:(audio)")),
    )
}

/// `mm:ss`, `hh:mm:ss`, seconds-as-string, or seconds-as-float — the metadata
/// API says "length" and means any of these. Answers milliseconds.
pub fn parse_archive_length(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(secs) = raw.parse::<f64>() {
        return Some((secs * 1000.0) as u64);
    }
    let parts: Vec<u64> = raw.split(':').filter_map(|p| p.parse().ok()).collect();
    match parts.as_slice() {
        [ss] => Some(ss * 1000),
        [mm, ss] => Some((mm * 60 + ss) * 1000),
        [hh, mm, ss] => Some(((hh * 60 + mm) * 60 + ss) * 1000),
        _ => None,
    }
}

/// Audio filenames worth offering, in the order the item lists them.
///
/// Derivative MP3s are the safe play — the original FLAC may exist, but the
/// derivative is what the Archive itself serves fastest — so `.mp3` sorts
/// first and everything else keeps item order.
fn is_audio_name(lowered: &str) -> bool {
    [".mp3", ".ogg", ".oga", ".flac", ".m4a", ".wav"]
        .iter()
        .any(|ext| lowered.ends_with(ext))
}

/// Audio filenames worth offering, derivatives-first: the Archive's own MP3
/// derivatives are what it serves fastest, so they lead and everything else
/// keeps item order.
fn audio_files(files: &[serde_json::Value]) -> Vec<(String, String)> {
    // (name, title-or-empty)
    let mut mp3s = Vec::new();
    let mut others = Vec::new();
    for f in files {
        let Some(name) = f.get("name").and_then(|n| n.as_str()).map(str::to_string) else {
            continue;
        };
        let lowered = name.to_ascii_lowercase();
        if !is_audio_name(&lowered) {
            continue;
        }
        let title = f
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string();
        if lowered.ends_with(".mp3") {
            mp3s.push((name, title));
        } else {
            others.push((name, title));
        }
    }
    mp3s.extend(others);
    mp3s.truncate(MAX_TRACKS);
    mp3s
}

/// Turn one item's metadata document into guarded, playable tracks.
pub fn parse_archive_item(identifier: &str, body: &[u8]) -> Result<Vec<PublicTrack>, String> {
    let doc: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("unreadable JSON: {e}"))?;
    let album = doc
        .get("metadata")
        .and_then(|m| m.get("title"))
        .and_then(|t| t.as_str())
        .map(str::to_string);
    let creator = doc
        .get("metadata")
        .and_then(|m| m.get("creator"))
        .map(|c| match c {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(a) => a
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            _ => String::new(),
        })
        .unwrap_or_default();
    let files = doc
        .get("files")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();
    let artwork = format!("https://archive.org/services/img/{identifier}");
    Ok(audio_files(&files))
        .map(|list| {
            list.into_iter()
                .filter_map(|(name, title)| {
                    let url = format!(
                        "https://archive.org/download/{}/{}",
                        urlencode(identifier),
                        // Names carry slashes (subdirectories); encode each
                        // segment but keep the separators real.
                        name.split('/').map(urlencode).collect::<Vec<_>>().join("/"),
                    );
                    let display = if title.is_empty() {
                        std::path::Path::new(&name)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or(&name)
                            .to_string()
                    } else {
                        title
                    };
                    let length = files
                        .iter()
                        .find(|f| f.get("name").and_then(|n| n.as_str()) == Some(name.as_str()))
                        .and_then(|f| f.get("length"))
                        .and_then(|l| l.as_str())
                        .and_then(parse_archive_length)
                        .unwrap_or(0);
                    guarded_track(
                        display,
                        creator.clone(),
                        album.clone(),
                        length,
                        url,
                        Some(artwork.clone()),
                    )
                })
                .collect::<Vec<_>>()
        })
        .map(|mut tracks| {
            tracks.truncate(MAX_TRACKS);
            tracks
        })
}

// ── Podcast feeds ────────────────────────────────────────────────────────────

/// Extract `<item>` episodes' title + enclosure URL (+ duration) from an RSS
/// document.
///
/// Pure, and deliberately a *tolerant* parser rather than a validating one:
/// feed generators disagree about namespaces (`itunes:`), attribute quoting
/// and CDATA, and the honest failure mode for a shape we misread is "fewer
/// rows", not "no feed". Only three facts are read per episode, each found
/// the same way — locate the tag, take the attribute/text, stop at the close.
pub fn parse_rss_episodes(body: &str) -> Vec<PublicTrack> {
    let mut out = Vec::new();
    for item in split_items(body).into_iter().take(MAX_TRACKS) {
        let title = xml_text(&item, "title").unwrap_or_default();
        let Some(enclosure_url) = enclosure_attr(&item, "url") else {
            continue;
        };
        if enclosure_url.is_empty() {
            continue;
        }
        let duration_ms = xml_text(&item, "itunes:duration")
            .or_else(|| xml_text(&item, "duration"))
            .as_deref()
            .and_then(parse_archive_length)
            .unwrap_or(0);
        let artist = xml_text(&item, "itunes:author").or_else(|| channel_author(body));
        if let Some(track) = guarded_track(
            title,
            artist.unwrap_or_default(),
            None,
            duration_ms,
            enclosure_url,
            None,
        ) {
            out.push(track);
        }
    }
    out
}

fn split_items(body: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("<item") {
        let Some(end) = rest[start..].find("</item>") else {
            break;
        };
        items.push(rest[start..start + end].to_string());
        rest = &rest[start + end + "</item>".len()..];
        if items.len() >= MAX_TRACKS {
            break;
        }
    }
    items
}

fn xml_text(fragment: &str, tag: &str) -> Option<String> {
    // Two openings are legal: `<tag>` and `<tag attr…>`. Find either, then
    // read plain text up to the closing tag. CDATA is not handled — feeds
    // that wrap titles in it lose their first row, which is a tolerated
    // miss recorded in the module header rather than a parser to maintain.
    let open_a = format!("<{tag}>");
    let text_start = if let Some(i) = fragment.find(&open_a) {
        i + open_a.len()
    } else {
        let open_b = format!("<{tag} ");
        let i = fragment.find(&open_b)?;
        i + fragment[i..].find('>')? + 1
    };
    let close = format!("</{tag}>");
    let end = text_start + fragment[text_start..].find(&close)?;
    decode_entities(fragment[text_start..end].trim())
}

fn enclosure_attr(fragment: &str, attr: &str) -> Option<String> {
    const TAG: &str = "<enclosure";
    let start = fragment.find(TAG)?;
    let end = fragment[start..].find('>')? + start;
    let tag = &fragment[start..end];
    let needle = format!("{attr}=\"");
    let at = tag.find(&needle)? + needle.len();
    let rest = &tag[at..];
    let close = rest.find('"')?;
    decode_entities(&rest[..close])
}

fn channel_author(body: &str) -> Option<String> {
    // The channel-level author sits outside <item>s; approximate by reading
    // the first <itunes:author> before the first <item>.
    let head = body.split("<item").next()?;
    xml_text(head, "itunes:author").or_else(|| xml_text(head, "author"))
}

fn decode_entities(s: &str) -> Option<String> {
    if !s.contains('&') {
        return Some(s.to_string());
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let semi = tail.find(';')?;
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => "&",
            "lt" => "<",
            "gt" => ">",
            "quot" => "\"",
            "apos" => "'",
            _ => {
                out.push('&');
                rest = &tail[1..];
                continue;
            }
        };
        out.push_str(decoded);
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
    Some(out)
}

// ── LRCLIB ───────────────────────────────────────────────────────────────────

/// One timed lyric line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    pub at_ms: u64,
    pub text: String,
}

/// Pick the best synced answer for a recording, from the search results.
///
/// Pure. The rule is duration first, sync second: among results within ±3 s of
/// the playing length, prefer the smallest difference; among equals, one with
/// synced lyrics beats one without (the API marks `instrumental`). An answer
/// without synced text is no answer — plain lyrics cannot highlight.
pub fn pick_lrc(body: &[u8], want_duration_ms: u64) -> Result<Option<Vec<LyricLine>>, String> {
    let doc: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("unreadable JSON: {e}"))?;
    let results = doc.as_array().cloned().unwrap_or_default();
    let want = (want_duration_ms / 1000) as f64;
    let best = results
        .into_iter()
        .filter_map(|r| {
            let synced = r.get("syncedLyrics")?.as_str()?.to_string();
            if synced.trim().is_empty() {
                return None;
            }
            let theirs = r.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.0);
            let diff = (theirs - want).abs();
            if diff > 3.0 {
                return None;
            }
            Some((diff, synced))
        })
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(best.map(|(_, doc)| parse_lrc_document(&doc)))
}

/// LRC → lines. Mirrors `TogetherDecisions.parseLrc` on Android — the same
/// grammar, parsed twice, kept honest by shared fixtures in the tests.
pub fn parse_lrc_document(text: &str) -> Vec<LyricLine> {
    let mut out: Vec<LyricLine> = Vec::new();
    for raw in text.lines() {
        let mut stamps: Vec<(u64, usize)> = Vec::new();
        let mut search = 0usize;
        while let Some(open) = raw[search..].find('[') {
            let abs = search + open;
            let Some(close_rel) = raw[abs..].find(']') else {
                break;
            };
            let inner = &raw[abs + 1..abs + close_rel];
            search = abs + close_rel + 1;
            // Timestamps are digits:mm:ss(.frac); metadata tags fail the parse
            // and are skipped, exactly like the Kotlin parser drops them.
            let bits: Vec<&str> = inner.split(':').collect();
            if bits.len() < 2 || bits.len() > 3 {
                continue;
            }
            let mm = bits[0].trim().parse::<u64>().ok();
            let sec_bits: Vec<&str> = bits[1].split('.').collect();
            let ss = sec_bits.first().and_then(|s| s.trim().parse::<u64>().ok());
            let (Some(mm), Some(ss)) = (mm, ss) else {
                continue;
            };
            let frac = match sec_bits.get(1) {
                None => 0u64,
                Some(f) if f.len() <= 3 && !f.is_empty() => {
                    let padded = format!("{f:0<3}");
                    padded.parse().unwrap_or(0)
                }
                _ => continue,
            };
            stamps.push((mm * 60_000 + ss * 1_000 + frac, search));
        }
        // Each timestamp on the line sings the same words at its own moment.
        let Some(&(_, body_start)) = stamps.last() else {
            continue;
        };
        let line_text = raw[body_start..].trim().to_string();
        if line_text.is_empty() {
            continue;
        }
        for (at, _) in stamps {
            out.push(LyricLine {
                at_ms: at,
                text: line_text.clone(),
            });
        }
    }
    out.sort_by_key(|l| l.at_ms);
    out.dedup_by(|a, b| a.at_ms == b.at_ms && a.text == b.text);
    out
}

// The timestamp scanning lives inline in [`parse_lrc_document`] — a tiny hand
// loop beat importing a regex dependency for four fixed-width digit groups.

// ── The network half ─────────────────────────────────────────────────────────

/// Search the Archive for audio collections matching `query`.
#[cfg(feature = "catalogue-http")]
pub async fn archive_search(query: &str) -> Result<Vec<ArchiveItem>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let body = fetch_bounded(&archive_search_url(q)).await?;
    parse_archive_search(&body)
}

/// One item's playable files, guarded and named.
#[cfg(feature = "catalogue-http")]
pub async fn archive_tracks(identifier: &str) -> Result<Vec<PublicTrack>, String> {
    if identifier.trim().is_empty()
        || !identifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err("not an archive identifier".into());
    }
    let url = format!(
        "https://archive.org/metadata/{}",
        urlencode(identifier.trim())
    );
    let body = fetch_bounded(&url).await?;
    parse_archive_item(identifier.trim(), &body)
}

/// Every playable episode of one podcast feed.
///
/// The feed URL itself must pass the peer guard — a feed is a URL this device
/// will fetch on its own behalf, so the same bar applies as to anything a peer
/// might have sent.
#[cfg(feature = "catalogue-http")]
pub async fn podcast_episodes(feed_url: &str) -> Result<Vec<PublicTrack>, String> {
    let url = feed_url.trim();
    if !valid_stream_url(url) {
        return Err("that is not a shareable https feed address".into());
    }
    let body = fetch_bounded(url).await?;
    let text = String::from_utf8(body).map_err(|_| "the feed is not text".to_string())?;
    Ok(parse_rss_episodes(&text))
}

/// Synced lyrics for a recording, or an empty answer when nothing matched.
#[cfg(feature = "catalogue-http")]
pub async fn lrc_lookup(
    title: &str,
    artist: &str,
    want_duration_ms: u64,
) -> Result<Vec<LyricLine>, String> {
    let title = title.trim();
    if title.is_empty() {
        return Ok(Vec::new());
    }
    // The API takes either structured params or one q=; q= tolerates a missing
    // artist, which is the common case here — a session knows its title first.
    let q = if artist.trim().is_empty() {
        title.to_string()
    } else {
        format!("{title} {artist}")
    };
    let url = format!("https://lrclib.net/api/search?q={}", urlencode(&q));
    let body = fetch_bounded(&url).await?;
    // No synced answer within tolerance is an empty success — "none exists",
    // not a failure.
    pick_lrc(&body, want_duration_ms).map(|opt| opt.unwrap_or_default())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_search_reads_docs_and_survives_odd_creators() {
        let body = br#"{"response":{"docs":[
            {"identifier":"lp_kun_faya_kun","title":"Kun Faya Kun","creator":"A. R. Rahman"},
            {"identifier":"mix_2005","title":"Mixtape 2005","creator":["One","Two"]},
            {"identifier":"","title":"no id"},
            {"title":"no identifier"}
        ]}}"#;
        let out = parse_archive_search(body).expect("ok");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].creator, "A. R. Rahman");
        assert_eq!(out[1].creator, "One, Two");
    }

    #[test]
    fn archive_lengths_come_in_three_shapes() {
        assert_eq!(parse_archive_length("209"), Some(209_000));
        assert_eq!(parse_archive_length("3:29"), Some(209_000));
        assert_eq!(parse_archive_length("1:03:29"), Some(3_809_000));
        assert_eq!(parse_archive_length(""), None);
        assert_eq!(parse_archive_length("later"), None);
    }

    #[test]
    fn archive_tracks_are_guarded_and_named_from_files() {
        let body = br#"{"metadata":{"title":"Great Album","creator":"Someone"},
            "files":[
              {"name":"01 - First Song.mp3","format":"VBR MP3","length":"3:29"},
              {"name":"artwork.png","format":"PNG"},
              {"name":"02 - Second.flac","format":"Flac","length":"199.0"}
            ]}"#;
        let tracks = parse_archive_item("lp_great_album", body).expect("ok");
        assert_eq!(tracks.len(), 2, "only audio survives");
        assert!(tracks[0]
            .url
            .starts_with("https://archive.org/download/lp_great_album/01%20-%20First%20Song.mp3"));
        assert_eq!(tracks[0].duration_ms, 209_000);
        assert_eq!(tracks[0].album.as_deref(), Some("Great Album"));
        assert_eq!(tracks[0].artist, "Someone");
        assert_eq!(
            tracks[1].title, "02 - Second",
            "stem names the untitled file"
        );
    }

    #[test]
    fn rss_reads_enclosures_and_drops_the_rest() {
        let feed = r#"<?xml version="1.0"?>
        <rss><channel><title>Feed</title>
        <itunes:author>The Channel</itunes:author>
        <item><title>Ep 1</title><itunes:duration>31:00</itunes:duration>
          <enclosure url="https://cdn.example.org/ep1.mp3" type="audio/mpeg" length="1"/></item>
        <item><title>Ep 2</title>
          <enclosure url="https://cdn.example.org/ep2.ogg" type="audio/ogg"/></item>
        <item><title>No enclosure here</title></item>
        </channel></rss>"#;
        let eps = parse_rss_episodes(feed);
        assert_eq!(eps.len(), 2);
        assert_eq!(eps[0].url, "https://cdn.example.org/ep1.mp3");
        assert_eq!(eps[0].duration_ms, 1_860_000);
        assert_eq!(eps[0].artist, "The Channel");
    }

    #[test]
    fn rss_entities_decode_instead_of_leaking() {
        let feed = r#"<item><title>A &amp; B</title>
          <enclosure url="https://cdn.example.org/a&amp;b.mp3"/></item>"#;
        let eps = parse_rss_episodes(feed);
        assert_eq!(eps[0].title, "A & B");
        assert_eq!(eps[0].url, "https://cdn.example.org/a&b.mp3");
    }

    #[test]
    fn lrc_picks_the_closest_synced_answer_within_tolerance() {
        let body = br#"[
          {"duration": 209.0, "syncedLyrics": "[00:01.00]far off"},
          {"duration": 210.5, "syncedLyrics": "[00:01.50]closest"},
          {"duration": 300.0, "syncedLyrics": "[00:02.00]way long"},
          {"duration": 210.4, "plainLyrics": "no sync at all"}
        ]"#;
        let picked = pick_lrc(body, 210_400).expect("ok").expect("some");
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].text, "closest");
    }

    #[test]
    fn lrc_parses_multi_stamps_sorts_and_drops_metadata_tags() {
        let doc = "[ar:X]\n[00:30.50]second\n[01:05][00:10]chorus\n";
        let lines = parse_lrc_document(doc);
        assert_eq!(
            lines.iter().map(|l| l.at_ms).collect::<Vec<_>>(),
            vec![10_000, 30_500, 65_000]
        );
        assert_eq!(lines[0].text, "chorus");
    }
}
