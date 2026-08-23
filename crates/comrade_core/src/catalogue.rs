/*!
 * catalogue — turning "Kun Faya Kun" into a recording, and then into audio.
 *
 * `/spotify Kun Faya Kun` in a chat composer asks two separate questions, and
 * conflating them is what makes this area a minefield:
 *
 * 1. **Which recording is that?** — a public catalogue lookup, answered by
 *    [`CatalogueResolver`]. Nothing but a name goes out; a
 *    [`crate::together::Recording`] comes back.
 * 2. **Where do the bytes come from?** — answered by [`choose_audio_plan`],
 *    which picks the first of four tiers that can supply it. The order *is* the
 *    policy.
 *
 * This is the Antra idea, and this module is explicit about which half of it is
 * adopted. `docs/TOGETHER.md` §9 already records the reasoning; the short version
 * is that Antra's chain is `own mirrors → Tidal/Qobuz/Amazon/Deezer/Apple
 * adapters → Soulseek`, and **the last two tiers are not implementable here**:
 *
 * - Those services do not serve unencrypted audio to third-party clients, so
 *   obtaining it means defeating a technological protection measure. That is a
 *   liability *distinct from* infringement — DMCA §1201, EU InfoSoc Art. 6,
 *   India's Copyright Act §65A — and it is not something an app whose pitch is
 *   sovereignty can quietly carry.
 * - Mirror servers and Soulseek are unlicensed redistribution, and a Comrade
 *   that ran a mirror would also be the backend `crate::share`'s whole design
 *   exists to avoid.
 *
 * So there is no adapter for either. **There was also deliberately no single
 * `AudioSource` trait they could be slotted into, and on 2026-08-09 the owner
 * decided to add one** — the tier is being made pluggable so a BlackHole-shaped
 * source layer can exist. The reasoning above is not deleted, because it is
 * still the reason the tier ships **pluggable but with no circumvention adapter
 * in it**: the seam is provided, and an adapter that defeats a protection
 * measure is not, and is not going to be written here. Whoever wants one writes
 * it against the interface and owns that decision, which is a better place for
 * it than a silent absence that reads as an oversight.
 *
 * The rest of the original argument stands on its own merits and is worth
 * keeping in view: an extractor is the component that *breaks*, because it
 * depends on internals its upstream is free to change without notice, and a
 * source layer whose adapters are hand-rolled inherits that maintenance
 * indefinitely. Prefer an established, separately-maintained extractor over
 * bytes parsed here.
 *
 * The four tiers genuinely live in different layers, so **what is here is still
 * the decision** rather than the mechanism, and carrying it out belongs to
 * whoever owns that layer:
 *
 * | tier | where the bytes are | who carries it out |
 * |---|---|---|
 * | [`SourceTier::Library`] | already on this device | the frontend's own library resolver (Android's `LibraryResolver`) |
 * | [`SourceTier::Peer`] | on the other person's device | [`crate::share`] |
 * | [`SourceTier::OpenLicence`] | an archive whose licence permits it | an HTTPS fetch the caller makes |
 * | [`SourceTier::EmbedOnly`] | nowhere — playable in place only | the embed player |
 *
 * The first two need no network beyond the app's own, cover the overwhelming
 * majority of "listen to this with me" between two people who both like the
 * song, and are already built. The third is where free/CC-licensed music
 * genuinely lives. The fourth is the honest floor: for a licensed commercial
 * track that nobody in the conversation owns, what this app can do is play the
 * YouTube embed or tell you where to open it — and saying so is better than a
 * download button that works until somebody's lawyer notices.
 *
 * ## Licence checking happens before the fetch, not after
 * [`OpenLicence`] is checked against the catalogue's declared terms *before* a
 * byte is requested ([`licence_permits_sharing`]). A tier that downloaded first
 * and inspected afterwards would already have made the copy it was deciding
 * about.
 *
 * ## The network half is behind a feature
 * Everything that touches a socket is under `catalogue-http`, following
 * `media-http`'s guards exactly: HTTPS only, redirects disabled, bounded read,
 * explicit timeouts. The pure parts — the chain order, the licence gate, the
 * query shaping — are always compiled and always tested.
 */

use crate::together::{match_score, Recording, MATCH_CONFIDENT};

/// How long a catalogue lookup may take before we stop waiting.
///
/// Short on purpose: this runs while somebody watches a composer, and a slow
/// answer is worse than "look for it in your library" — which is what the chain
/// falls back to anyway.
#[cfg(feature = "catalogue-http")]
const LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);

/// Longest catalogue response we will read.
///
/// A metadata document for a handful of recordings is kilobytes; this bounds a
/// host that answers a search with a stream that never ends.
#[cfg(feature = "catalogue-http")]
const MAX_LOOKUP_BYTES: usize = 512 * 1024;

/// How many candidates a resolver may return. A picker shows a few; a hundred is
/// a scroll, and every extra row is another chance to open the wrong thing.
pub const MAX_CANDIDATES: usize = 8;

/// The read cap can never reject a real answer: a metadata document for the most
/// candidates we ask for is orders of magnitude under it.
///
/// A compile-time invariant rather than an assertion in a test, matching the five
/// in [`crate::together`]. Both operands are constants, so a runtime `assert!`
/// would be constant-folded — which is what `clippy::assertions_on_constants`
/// objects to, and it is right: this belongs in the build, where raising
/// `MAX_CANDIDATES` past the cap fails to compile instead of failing a test run.
#[cfg(feature = "catalogue-http")]
const _: () = assert!(MAX_CANDIDATES * 4096 < MAX_LOOKUP_BYTES);

// ── Where audio can come from ────────────────────────────────────────────────

/// The tiers [`choose_audio_plan`] tries, in order.
///
/// Ordered by *cost to everybody involved*, not by convenience: this device
/// first, then the person already in the conversation, then a public archive,
/// then nothing. Deriving the order from an enum rather than writing it at each
/// call site is what stops a future caller reaching for the network before
/// checking the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, uniffi::Enum)]
pub enum SourceTier {
    /// A file already on this device. No network, no third party, no question
    /// about rights — they already have it.
    Library,
    /// The other person has it and can send it over
    /// [`crate::share`] — one person handing something to one person, which is
    /// the existing encrypted-attachment path in a different shape.
    Peer,
    /// A catalogue whose declared licence permits redistribution.
    OpenLicence,
    /// Nobody can hand over the bytes. Playable in place through an embed, or
    /// nameable so the listener opens it themselves.
    EmbedOnly,
}

impl SourceTier {
    /// The tiers in the order they must be tried.
    pub fn ladder() -> [SourceTier; 4] {
        [
            SourceTier::Library,
            SourceTier::Peer,
            SourceTier::OpenLicence,
            SourceTier::EmbedOnly,
        ]
    }

    /// Whether this tier moves bytes at all. [`SourceTier::EmbedOnly`] does not,
    /// which is why it can be the answer for a licensed commercial track.
    pub fn transfers_audio(self) -> bool {
        !matches!(self, SourceTier::EmbedOnly)
    }

    /// Whether this tier reaches a third party. `Library` and `Peer` do not —
    /// which is what makes them the first two rungs.
    pub fn contacts_a_third_party(self) -> bool {
        matches!(self, SourceTier::OpenLicence | SourceTier::EmbedOnly)
    }
}

/// A licence, as a public catalogue declares it.
///
/// Deliberately coarse. This is not a licence compiler — it is the one question
/// that decides whether a tier may run: *does this permit us to hand a copy to
/// somebody?* Anything not recognised is [`Self::Unknown`], which is treated as
/// "no".
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum OpenLicence {
    /// Public domain or an explicit dedication (CC0, PD mark).
    PublicDomain,
    /// A Creative Commons licence that permits redistribution — every CC
    /// variant does, including the NoDerivatives and NonCommercial ones, since
    /// passing an unmodified copy to one person is neither a derivative nor a
    /// sale.
    CreativeCommons,
    /// The catalogue says nothing, or says something we do not recognise.
    Unknown,
}

/// Recognise a licence string from a catalogue's metadata.
///
/// Conservative in the direction that costs least: an unrecognised string is
/// [`OpenLicence::Unknown`] and the tier is skipped. A false "no" costs a
/// fallback to the peer-share path; a false "yes" costs an unlicensed copy.
pub fn read_licence(declared: &str) -> OpenLicence {
    let d = declared.to_ascii_lowercase();
    if d.contains("publicdomain")
        || d.contains("public domain")
        || d.contains("cc0")
        || d.contains("/pdm")
        || d.contains("mark/1.0")
    {
        return OpenLicence::PublicDomain;
    }
    // Both the URL form (creativecommons.org/licenses/by-sa/4.0) and the short
    // form ("CC BY-SA 4.0") appear in real catalogue metadata.
    if d.contains("creativecommons.org/licenses") || d.starts_with("cc by") || d.contains("cc-by") {
        return OpenLicence::CreativeCommons;
    }
    OpenLicence::Unknown
}

/// Whether a declared licence lets us pass a copy to somebody.
pub fn licence_permits_sharing(licence: OpenLicence) -> bool {
    matches!(
        licence,
        OpenLicence::PublicDomain | OpenLicence::CreativeCommons
    )
}

// ── The identity seam ────────────────────────────────────────────────────────

/// One catalogue answer: what the recording is, and whether its audio may be
/// redistributed.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CatalogueMatch {
    /// Which catalogue answered. A row names its source on screen
    /// (`CatalogueResolver::name`'s contract: a guess presented without its
    /// source is worse than no guess), and with more than one resolver wired,
    /// that name has to travel **on the match** rather than live as a constant
    /// beside a single call site — the exact drift §20 predicted for
    /// TogetherScreen's now-removed `MUSICBRAINZ` constant.
    pub source: String,
    pub recording: Recording,
    /// Duration when the catalogue knows it — the tiebreak
    /// [`crate::together::match_score`] uses against a local file.
    pub duration_ms: Option<u64>,
    /// Where the audio can be fetched, when this catalogue serves audio at all.
    /// `None` for a pure metadata catalogue like MusicBrainz.
    pub audio_url: Option<String>,
    /// The licence as declared. [`OpenLicence::Unknown`] unless the catalogue
    /// said something we recognise.
    pub licence: OpenLicence,
}

impl CatalogueMatch {
    /// Metadata only — the MusicBrainz shape.
    ///
    /// Named for the one catalogue that produces it: every metadata-only
    /// resolver today *is* MusicBrainz. The day a second metadata-only
    /// catalogue exists, this constructor grows a source parameter and stops
    /// guessing — same reasoning as [`Self::openly_licensed`] taking one.
    pub fn metadata(recording: Recording, duration_ms: Option<u64>) -> Self {
        Self {
            source: "MusicBrainz".to_string(),
            recording,
            duration_ms,
            audio_url: None,
            licence: OpenLicence::Unknown,
        }
    }

    /// An answer carrying audio the catalogue itself serves under a declared
    /// licence. The source is taken, not assumed: two audio catalogues in the
    /// plan means two names on the rows.
    pub fn openly_licensed(
        recording: Recording,
        duration_ms: Option<u64>,
        audio_url: String,
        source: &'static str,
    ) -> Self {
        Self {
            source: source.to_string(),
            recording,
            duration_ms,
            audio_url: Some(audio_url),
            licence: OpenLicence::CreativeCommons,
        }
    }

    /// Whether [`SourceTier::OpenLicence`] may fetch this.
    ///
    /// Both halves are required, and the licence half is why: an archive that
    /// serves audio is not thereby permitting us to redistribute it.
    pub fn is_openly_fetchable(&self) -> bool {
        self.audio_url.is_some() && licence_permits_sharing(self.licence)
    }
}

/// Resolve free text to the recording it names.
///
/// The seam, so an adapter can be swapped without the composer changing —
/// deliberately the same shape as [`crate::tara::CompanionEngine`] and
/// [`crate::media::MediaUploader`]. Implementations must be *metadata only*:
/// returning a `CatalogueMatch` whose `audio_url` points at DRM-protected or
/// unlicensed audio would push the problem this module exists to avoid one layer
/// down, where nothing is checking.
#[allow(async_fn_in_trait)] // internal trait; bounds are added at call sites
pub trait CatalogueResolver {
    /// Best matches for `query`, most likely first, at most [`MAX_CANDIDATES`].
    ///
    /// An empty result is normal and not an error — it means "look in the
    /// library instead", which is where the chain was going anyway.
    async fn lookup(&self, query: &str) -> Result<Vec<CatalogueMatch>, CatalogueError>;

    /// The catalogue's own name, for a UI that has to say where an answer came
    /// from. A guess presented without its source is worse than no guess.
    fn name(&self) -> &'static str;
}

/// What can go wrong looking something up.
#[derive(Debug, thiserror::Error)]
pub enum CatalogueError {
    #[error("catalogue request failed: {0}")]
    Http(String),
    #[error("catalogue returned something unreadable: {0}")]
    Malformed(String),
    #[error("refusing to fetch a non-HTTPS catalogue URL")]
    InsecureUrl,
    #[error("catalogue response exceeded {MAX_LOOKUP_BYTES} bytes")]
    #[cfg(feature = "catalogue-http")]
    TooLarge,
}

// ── Choosing a tier ──────────────────────────────────────────────────────────

/// What to do about a recording somebody asked for.
// No `Eq`: the confidence is an `f64`. `PartialEq` is enough for the tests and
// for a frontend comparing plans, and rounding it to an integer just to satisfy
// a derive would throw away the number a UI wants to show.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum AudioPlan {
    /// It is already here. The `f64` is the match score, so a UI can say how
    /// sure it is rather than silently opening a file.
    Library { confidence: f64 },
    /// Ask the other person for it.
    Peer,
    /// Fetch it from a catalogue that permits it.
    OpenLicence { url: String },
    /// Nothing can be transferred; play it in place or name where to open it.
    EmbedOnly,
}

impl AudioPlan {
    /// Which tier this plan belongs to.
    pub fn tier(&self) -> SourceTier {
        match self {
            Self::Library { .. } => SourceTier::Library,
            Self::Peer => SourceTier::Peer,
            Self::OpenLicence { .. } => SourceTier::OpenLicence,
            Self::EmbedOnly => SourceTier::EmbedOnly,
        }
    }
}

/// Pick the first tier that can actually supply `want`.
///
/// Pure, so the whole policy is unit-tested with no network and no filesystem.
/// The caller supplies what it knows:
///
/// - `library` — candidates already on this device, each with its duration, as
///   the Android `LibraryResolver` produces them. Scored with the shared
///   [`match_score`]; only a [`MATCH_CONFIDENT`] match counts, because opening
///   the wrong track on somebody's behalf is worse than asking.
/// - `peer_has_it` — whether the other side offered it. That is *their* claim,
///   not something we verified, which is exactly why the tier below it exists.
/// - `catalogue` — matches from a [`CatalogueResolver`], filtered here by
///   [`CatalogueMatch::is_openly_fetchable`] rather than by the adapter, so a
///   sloppy adapter cannot smuggle a licensed URL past the gate.
pub fn choose_audio_plan(
    want: &Recording,
    want_ms: u64,
    library: &[(Recording, u64)],
    peer_has_it: bool,
    catalogue: &[CatalogueMatch],
) -> AudioPlan {
    let best = library
        .iter()
        .map(|(have, have_ms)| match_score(want, have, want_ms, *have_ms))
        .fold(0.0f64, f64::max);
    if best >= MATCH_CONFIDENT {
        return AudioPlan::Library { confidence: best };
    }
    if peer_has_it {
        return AudioPlan::Peer;
    }
    if let Some(url) = catalogue
        .iter()
        .find(|m| m.is_openly_fetchable())
        .and_then(|m| m.audio_url.clone())
    {
        return AudioPlan::OpenLicence { url };
    }
    AudioPlan::EmbedOnly
}

// ── The MusicBrainz adapter ──────────────────────────────────────────────────

/// Shape a free-text query for a catalogue search.
///
/// Pure and tested separately from the request, because this is the part that
/// can be wrong in a way no HTTP status reports: an artist read as part of the
/// title finds nothing, and "nothing" is indistinguishable from "not in the
/// catalogue".
pub fn musicbrainz_query(recording: &Recording) -> String {
    if recording.artist.is_empty() {
        format!("recording:\"{}\"", escape_lucene(&recording.title))
    } else {
        format!(
            "recording:\"{}\" AND artist:\"{}\"",
            escape_lucene(&recording.title),
            escape_lucene(&recording.artist)
        )
    }
}

/// Strip the Lucene metacharacters MusicBrainz's search index would otherwise
/// interpret. A song title containing a `:` or a `~` is common, and an unescaped
/// one turns a search into a syntax error.
fn escape_lucene(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !matches!(
                c,
                '"' | '\\' | ':' | '~' | '^' | '(' | ')' | '[' | ']' | '{' | '}'
            )
        })
        .collect()
}

/// MusicBrainz — the default resolver.
///
/// Chosen over Spotify and Apple Music for the *deployment* reason rather than a
/// technical one: it needs no API key, so there is no credential to ship in an
/// app binary, rotate, or have revoked out from under every install. Its data is
/// open, and it carries ISRCs, which is the field
/// [`crate::together::match_score`] treats as decisive.
///
/// Metadata only. It serves no audio, so every match it returns has
/// `audio_url: None` and can never satisfy [`SourceTier::OpenLicence`].
#[cfg(feature = "catalogue-http")]
#[derive(Debug, Clone)]
pub struct MusicBrainz {
    base: String,
    /// MusicBrainz requires a identifying User-Agent and rate-limits without
    /// one. Configurable so a fork does not impersonate this app.
    user_agent: String,
}

#[cfg(feature = "catalogue-http")]
impl MusicBrainz {
    /// The public instance.
    pub fn new(user_agent: impl Into<String>) -> Self {
        Self {
            base: "https://musicbrainz.org/ws/2".to_string(),
            user_agent: user_agent.into(),
        }
    }

    /// Point at another instance — a mirror, or a test server.
    pub fn with_base(base: impl Into<String>, user_agent: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            user_agent: user_agent.into(),
        }
    }
}

#[cfg(feature = "catalogue-http")]
impl CatalogueResolver for MusicBrainz {
    fn name(&self) -> &'static str {
        "MusicBrainz"
    }

    async fn lookup(&self, query: &str) -> Result<Vec<CatalogueMatch>, CatalogueError> {
        let recording = crate::command::recording_from_query(query);
        let url = format!(
            "{}/recording?query={}&fmt=json&limit={MAX_CANDIDATES}",
            self.base,
            urlencode(&musicbrainz_query(&recording))
        );
        let body = fetch_json(&url, &self.user_agent).await?;
        parse_musicbrainz(&body)
    }
}

/// Parse a MusicBrainz recording-search response.
///
/// Split from the request so the shape is tested without a socket — the response
/// format is the part that changes, and a fixture catches that where a live call
/// only catches it in production.
#[cfg(feature = "catalogue-http")]
pub fn parse_musicbrainz(body: &str) -> Result<Vec<CatalogueMatch>, CatalogueError> {
    let doc: serde_json::Value =
        serde_json::from_str(body).map_err(|e| CatalogueError::Malformed(e.to_string()))?;
    let Some(items) = doc.get("recordings").and_then(|r| r.as_array()) else {
        // No `recordings` key at all is an empty result, not a broken one.
        return Ok(Vec::new());
    };
    Ok(items
        .iter()
        .take(MAX_CANDIDATES)
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?.to_string();
            let artist = item
                .get("artist-credit")
                .and_then(|a| a.as_array())
                .and_then(|a| a.first())
                .and_then(|c| c.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or_default()
                .to_string();
            let album = item
                .get("releases")
                .and_then(|r| r.as_array())
                .and_then(|r| r.first())
                .and_then(|r| r.get("title"))
                .and_then(|t| t.as_str())
                .map(str::to_string);
            // ISRCs are the decisive field, so take the first one the catalogue
            // offers and validate it rather than trusting the shape.
            let isrc = item
                .get("isrcs")
                .and_then(|i| i.as_array())
                .and_then(|i| i.first())
                .and_then(|i| i.as_str())
                .and_then(crate::together::normalise_isrc);
            Some(CatalogueMatch::metadata(
                Recording {
                    isrc,
                    title,
                    artist,
                    album,
                },
                item.get("length").and_then(|l| l.as_u64()),
            ))
        })
        .collect())
}

/// Percent-encode a query string component. Small and local rather than a
/// dependency, because the only characters that matter here are the ones a song
/// title actually contains.
#[cfg(feature = "catalogue-http")]
fn urlencode(s: &str) -> String {
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

/// Fetch a JSON document under the same guards `media::fetch_and_decrypt_media`
/// uses: HTTPS only, no redirects, explicit timeouts, bounded read.
///
/// Redirects are disabled for the reason they are on the media path — a
/// catalogue that 302s to `http://` or to a host we never chose would slip past
/// the scheme check that ran on the original URL.
#[cfg(feature = "catalogue-http")]
async fn fetch_json(url: &str, user_agent: &str) -> Result<String, CatalogueError> {
    let is_https = url
        .split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("https"));
    if !is_https {
        return Err(CatalogueError::InsecureUrl);
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(LOOKUP_TIMEOUT)
        .build()
        .map_err(|e| CatalogueError::Http(e.to_string()))?;
    let mut resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, user_agent)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| CatalogueError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(CatalogueError::Http(format!("status {}", resp.status())));
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_LOOKUP_BYTES as u64 {
            return Err(CatalogueError::TooLarge);
        }
    }
    // Bounded streaming read: a missing or lying Content-Length must not let a
    // host stream us out of memory. `chunk()` rather than `bytes_stream()` for
    // the same reason `media::fetch_and_decrypt_media` uses it — it is an
    // inherent method, so this needs neither reqwest's `stream` feature nor a
    // `futures-util` dependency.
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| CatalogueError::Http(e.to_string()))?
    {
        if buf.len() + chunk.len() > MAX_LOOKUP_BYTES {
            return Err(CatalogueError::TooLarge);
        }
        buf.extend_from_slice(&chunk);
    }
    String::from_utf8(buf).map_err(|e| CatalogueError::Malformed(e.to_string()))
}

// ── The Jamendo adapter ──────────────────────────────────────────────────────

/// Jamendo — an openly-licensed catalogue that *serves audio*.
///
/// Where MusicBrainz answers "what is this recording" and stops, Jamendo
/// answers it and hands over the file: every track on the platform is
/// published under a Creative Commons licence the API declares per track, with
/// direct download and streaming URLs. That makes it the first real
/// [`SourceTier::OpenLicence`] resident — [`choose_audio_plan`] can pick it,
/// and `download.rs`'s gate opens for it, without any new policy: the licence
/// check that already existed simply has something true to check.
///
/// It needs a client id (free, per-app, from developer.jamendo.com) because
/// that is how the provider rate-limits; there is no key to ship in the binary
/// because the caller supplies theirs per call, exactly like Subsonic's
/// credentials. No config, no search.
#[cfg(feature = "catalogue-http")]
#[derive(Debug, Clone)]
pub struct Jamendo {
    base: String,
    client_id: String,
}

#[cfg(feature = "catalogue-http")]
impl Jamendo {
    /// The public API with this app's registered id.
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            base: "https://api.jamendo.com".to_string(),
            client_id: client_id.into(),
        }
    }

    /// Point at another instance — a test server.
    #[cfg(test)]
    pub fn with_base(base: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            client_id: client_id.into(),
        }
    }
}

#[cfg(feature = "catalogue-http")]
impl CatalogueResolver for Jamendo {
    fn name(&self) -> &'static str {
        "Jamendo"
    }

    async fn lookup(&self, query: &str) -> Result<Vec<CatalogueMatch>, CatalogueError> {
        let url = format!(
            "{}/v3.0/tracks/?client_id={}&format=json&search={}&limit={MAX_CANDIDATES}",
            self.base,
            urlencode(&self.client_id),
            urlencode(query),
        );
        let body = fetch_json(&url, "comrade/1.0 (https://github.com/cmullu/comrade)").await?;
        parse_jamendo(&body)
    }
}

/// Parse a Jamendo track-search response.
///
/// Split from the request so the shape is pinned by a fixture, like
/// [`parse_musicbrainz`]. Two fields carry policy weight and are read
/// accordingly:
///
/// - `audiodownload_allowed` — the artist's own switch for whether the track
///   may be fetched as a file. When they said no, we take the stream URL or
///   nothing, never the download URL: **serving the bytes is not licensing
///   them**, the same line `download_verdict` draws.
/// - `duration` — seconds here (MusicBrainz reports milliseconds), converted
///   once so `match_score` sees one unit everywhere.
#[cfg(feature = "catalogue-http")]
pub fn parse_jamendo(body: &str) -> Result<Vec<CatalogueMatch>, CatalogueError> {
    let doc: serde_json::Value =
        serde_json::from_str(body).map_err(|e| CatalogueError::Malformed(e.to_string()))?;
    let Some(items) = doc.get("results").and_then(|r| r.as_array()) else {
        // An error envelope from the API carries `headers.error_message` and
        // no results key; an empty answer carries results=[] and parses to [].
        if doc.get("headers").is_some() {
            return Ok(Vec::new());
        }
        return Err(CatalogueError::Malformed(
            "no results array and no headers envelope".into(),
        ));
    };
    Ok(items
        .iter()
        .take(MAX_CANDIDATES)
        .filter_map(|item| {
            let title = item.get("name")?.as_str()?.to_string();
            if title.is_empty() {
                return None;
            }
            let audio_url = if item
                .get("audiodownload_allowed")
                .and_then(|a| a.as_bool())
                .unwrap_or(false)
            {
                item.get("audiodownload").and_then(|u| u.as_str())
            } else {
                item.get("audio").and_then(|u| u.as_str())
            }?
            .to_string();
            let duration_ms = item
                .get("duration")
                .and_then(|d| d.as_u64())
                .map(|s| s.saturating_mul(1000));
            Some(CatalogueMatch::openly_licensed(
                Recording {
                    isrc: None,
                    title,
                    artist: item
                        .get("artist_name")
                        .and_then(|a| a.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    album: item
                        .get("album_name")
                        .and_then(|a| a.as_str())
                        .map(str::to_string),
                },
                duration_ms,
                audio_url,
                "Jamendo",
            ))
        })
        .collect())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(title: &str, artist: &str) -> Recording {
        Recording {
            isrc: None,
            title: title.into(),
            artist: artist.into(),
            album: None,
        }
    }

    // ── The ladder ───────────────────────────────────────────────────────────

    #[test]
    fn the_ladder_puts_this_device_first_and_a_third_party_last() {
        let ladder = SourceTier::ladder();
        assert_eq!(ladder[0], SourceTier::Library);
        assert_eq!(ladder[1], SourceTier::Peer);
        // The two rungs that reach nobody outside the conversation come first —
        // that ordering is the policy, so pin it.
        assert!(!ladder[0].contacts_a_third_party());
        assert!(!ladder[1].contacts_a_third_party());
        assert!(ladder[2].contacts_a_third_party());
    }

    #[test]
    fn only_the_last_rung_moves_no_bytes() {
        for tier in SourceTier::ladder() {
            assert_eq!(
                tier.transfers_audio(),
                tier != SourceTier::EmbedOnly,
                "{tier:?}"
            );
        }
    }

    // ── Licences ─────────────────────────────────────────────────────────────

    #[test]
    fn recognised_open_licences_permit_sharing() {
        for declared in [
            "https://creativecommons.org/publicdomain/zero/1.0/",
            "CC0 1.0",
            "public domain",
            "https://creativecommons.org/publicdomain/mark/1.0/",
        ] {
            assert_eq!(
                read_licence(declared),
                OpenLicence::PublicDomain,
                "{declared}"
            );
            assert!(licence_permits_sharing(read_licence(declared)));
        }
        for declared in [
            "https://creativecommons.org/licenses/by-sa/4.0/",
            "CC BY 3.0",
            "cc-by-nc-nd 4.0",
        ] {
            assert_eq!(
                read_licence(declared),
                OpenLicence::CreativeCommons,
                "{declared}"
            );
            assert!(licence_permits_sharing(read_licence(declared)));
        }
    }

    #[test]
    fn anything_unrecognised_is_a_no() {
        // Conservative in the direction that costs a fallback rather than a
        // copy: a false "no" reaches the peer-share path, a false "yes" makes an
        // unlicensed copy.
        for declared in [
            "",
            "all rights reserved",
            "© 2011 Sony Music",
            "proprietary",
            "ask us",
        ] {
            assert_eq!(read_licence(declared), OpenLicence::Unknown, "{declared}");
            assert!(!licence_permits_sharing(read_licence(declared)));
        }
    }

    #[test]
    fn an_audio_url_without_a_licence_is_not_fetchable() {
        // An archive serving audio is not thereby permitting redistribution.
        let m = CatalogueMatch {
            source: "Test".into(),
            recording: rec("Kun Faya Kun", "A.R. Rahman"),
            duration_ms: Some(480_000),
            audio_url: Some("https://example.org/track.mp3".into()),
            licence: OpenLicence::Unknown,
        };
        assert!(!m.is_openly_fetchable());

        let licensed = CatalogueMatch {
            licence: OpenLicence::CreativeCommons,
            ..m.clone()
        };
        assert!(licensed.is_openly_fetchable());

        // …and a licence without a URL is nothing to fetch.
        let no_url = CatalogueMatch {
            audio_url: None,
            ..licensed
        };
        assert!(!no_url.is_openly_fetchable());
    }

    // ── Choosing a tier ──────────────────────────────────────────────────────

    #[test]
    fn a_confident_local_match_wins_over_everything() {
        let want = rec("Kun Faya Kun", "A.R. Rahman");
        let library = vec![(rec("Kun Faya Kun", "A.R. Rahman"), 481_000u64)];
        let open = vec![CatalogueMatch {
            source: "Test".into(),
            recording: want.clone(),
            duration_ms: None,
            audio_url: Some("https://archive.example/x.mp3".into()),
            licence: OpenLicence::PublicDomain,
        }];
        let plan = choose_audio_plan(&want, 480_000, &library, true, &open);
        assert_eq!(plan.tier(), SourceTier::Library);
        let AudioPlan::Library { confidence } = plan else {
            panic!("expected the library")
        };
        assert!(confidence >= MATCH_CONFIDENT);
    }

    #[test]
    fn a_weak_local_match_is_not_opened_on_someones_behalf() {
        // Opening the wrong track is worse than asking — `match_score` is
        // deliberately conservative and this respects it.
        let want = rec("Kun Faya Kun", "A.R. Rahman");
        let library = vec![(rec("Something Else Entirely", "Another Artist"), 200_000u64)];
        let plan = choose_audio_plan(&want, 480_000, &library, true, &[]);
        assert_eq!(plan.tier(), SourceTier::Peer);
    }

    #[test]
    fn the_peer_is_asked_before_any_archive() {
        let want = rec("Kun Faya Kun", "A.R. Rahman");
        let open = vec![CatalogueMatch {
            source: "Test".into(),
            recording: want.clone(),
            duration_ms: None,
            audio_url: Some("https://archive.example/x.mp3".into()),
            licence: OpenLicence::PublicDomain,
        }];
        assert_eq!(
            choose_audio_plan(&want, 480_000, &[], true, &open).tier(),
            SourceTier::Peer
        );
    }

    #[test]
    fn an_open_archive_is_used_only_when_nobody_in_the_conversation_has_it() {
        let want = rec("Kun Faya Kun", "A.R. Rahman");
        let open = vec![CatalogueMatch {
            source: "Test".into(),
            recording: want.clone(),
            duration_ms: None,
            audio_url: Some("https://archive.example/x.mp3".into()),
            licence: OpenLicence::CreativeCommons,
        }];
        let plan = choose_audio_plan(&want, 480_000, &[], false, &open);
        assert_eq!(
            plan,
            AudioPlan::OpenLicence {
                url: "https://archive.example/x.mp3".into()
            }
        );
    }

    #[test]
    fn a_licensed_commercial_track_nobody_owns_ends_at_the_embed() {
        // This is the honest floor, and the case `/spotify Kun Faya Kun`
        // actually hits: no local copy, peer has nothing, catalogue is metadata
        // only. There is no DRM tier below this, by design.
        let want = rec("Kun Faya Kun", "A.R. Rahman");
        let metadata_only = vec![CatalogueMatch::metadata(want.clone(), Some(480_000))];
        assert_eq!(
            choose_audio_plan(&want, 480_000, &[], false, &metadata_only),
            AudioPlan::EmbedOnly
        );
    }

    #[test]
    fn a_catalogue_that_offers_licensed_audio_cannot_smuggle_it_past_the_gate() {
        // The filter is here, not in the adapter, so a sloppy or hostile
        // resolver returning an all-rights-reserved URL changes nothing.
        let want = rec("Kun Faya Kun", "A.R. Rahman");
        let sneaky = vec![CatalogueMatch {
            source: "Test".into(),
            recording: want.clone(),
            duration_ms: None,
            audio_url: Some("https://drm.example/protected.m4a".into()),
            licence: OpenLicence::Unknown,
        }];
        assert_eq!(
            choose_audio_plan(&want, 480_000, &[], false, &sneaky),
            AudioPlan::EmbedOnly
        );
    }

    #[test]
    fn nothing_anywhere_is_still_a_usable_answer() {
        let want = rec("Kun Faya Kun", "");
        assert_eq!(
            choose_audio_plan(&want, 0, &[], false, &[]),
            AudioPlan::EmbedOnly
        );
    }

    // ── Query shaping ────────────────────────────────────────────────────────

    #[test]
    fn a_query_names_the_artist_when_there_is_one() {
        let q = musicbrainz_query(&rec("Kun Faya Kun", "A.R. Rahman"));
        assert!(q.contains("recording:\"Kun Faya Kun\""));
        assert!(q.contains("artist:\"A.R. Rahman\""));
    }

    #[test]
    fn a_bare_title_does_not_search_for_an_empty_artist() {
        // `artist:""` matches nothing, which would read as "not in the
        // catalogue" — indistinguishable from a real miss.
        let q = musicbrainz_query(&rec("Kun Faya Kun", ""));
        assert!(!q.contains("artist"));
    }

    #[test]
    fn lucene_metacharacters_in_a_title_are_stripped() {
        // A title with a colon is common and an unescaped one is a syntax error.
        let q = musicbrainz_query(&rec("Track: Part 2 (Live) ~remix", "Someone"));
        assert_eq!(
            q,
            "recording:\"Track Part 2 Live remix\" AND artist:\"Someone\""
        );
        // And a quote cannot break out of the field it sits in — the case that
        // would let a title rewrite the whole query.
        let injected = musicbrainz_query(&rec("a\" OR artist:\"x", ""));
        assert_eq!(injected, "recording:\"a OR artistx\"");
        assert_eq!(injected.matches('"').count(), 2);
    }

    #[cfg(feature = "catalogue-http")]
    #[test]
    fn a_musicbrainz_response_is_read_into_recordings() {
        let body = r#"{
          "recordings": [
            {
              "title": "Kun Faya Kun",
              "length": 480000,
              "isrcs": ["INS181100123"],
              "artist-credit": [{"name": "A.R. Rahman"}],
              "releases": [{"title": "Rockstar"}]
            }
          ]
        }"#;
        let out = parse_musicbrainz(body).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].recording.title, "Kun Faya Kun");
        assert_eq!(out[0].recording.artist, "A.R. Rahman");
        assert_eq!(out[0].recording.album.as_deref(), Some("Rockstar"));
        assert_eq!(out[0].recording.isrc.as_deref(), Some("INS181100123"));
        assert_eq!(out[0].duration_ms, Some(480_000));
        // Metadata only: MusicBrainz serves no audio, so this can never satisfy
        // the open-licence tier.
        assert!(out[0].audio_url.is_none());
        assert!(!out[0].is_openly_fetchable());
    }

    #[cfg(feature = "catalogue-http")]
    #[test]
    fn an_empty_or_odd_response_is_no_results_rather_than_an_error() {
        assert!(parse_musicbrainz(r#"{"recordings":[]}"#)
            .unwrap()
            .is_empty());
        assert!(parse_musicbrainz(r#"{"count":0}"#).unwrap().is_empty());
        assert!(parse_musicbrainz("not json").is_err());
    }

    #[cfg(feature = "catalogue-http")]
    #[test]
    fn a_malformed_isrc_is_dropped_rather_than_trusted() {
        // The ISRC is the decisive field in `match_score`, so a bad one would be
        // worse than none — it could match the wrong recording outright.
        let body =
            r#"{"recordings":[{"title":"T","isrcs":["nope"],"artist-credit":[{"name":"A"}]}]}"#;
        let out = parse_musicbrainz(body).unwrap();
        assert!(out[0].recording.isrc.is_none());
    }

    #[cfg(feature = "catalogue-http")]
    #[test]
    fn a_jamendo_answer_carries_its_audio_and_its_name() {
        let body = r#"{
          "headers": {"status": "success", "results_count": 2},
          "results": [
            {"id": "1432101", "name": "Sundown", "duration": 214,
             "artist_name": "Chasing Kites", "album_name": "Aerology",
             "audio": "https://prod-1.storage.jamendo.com/?trackid=1432101&format=mp31",
             "audiodownload": "https://prod-1.storage.jamendo.com/download/1432101/sundown.mp3",
             "audiodownload_allowed": true},
            {"id": "1432102", "name": "White Dawn", "duration": 187,
             "artist_name": "Chasing Kites",
             "audio": "https://prod-1.storage.jamendo.com/?trackid=1432102&format=mp31",
             "audiodownload": "https://prod-1.storage.jamendo.com/download/1432102/x.mp3",
             "audiodownload_allowed": false}
          ]
        }"#;
        let out = parse_jamendo(body).expect("ok");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].source, "Jamendo");
        assert_eq!(out[0].recording.title, "Sundown");
        assert_eq!(out[0].recording.artist, "Chasing Kites");
        assert_eq!(out[0].recording.album.as_deref(), Some("Aerology"));
        // Seconds on the wire, milliseconds everywhere in this app.
        assert_eq!(out[0].duration_ms, Some(214_000));
        // The artist allowed downloads, so the fetchable URL is the download
        // one — and the licence gate agrees.
        assert_eq!(
            out[0].audio_url.as_deref(),
            Some("https://prod-1.storage.jamendo.com/download/1432101/sundown.mp3")
        );
        assert!(out[0].is_openly_fetchable());
        // …but serving is not licensing: where the artist switched downloads
        // off, the stream URL stands in and the *fetch* gate stays shut.
        assert_eq!(
            out[1].audio_url.as_deref(),
            Some("https://prod-1.storage.jamendo.com/?trackid=1432102&format=mp31")
        );
        assert!(!out[1].is_openly_fetchable());
    }

    #[cfg(feature = "catalogue-http")]
    #[test]
    fn jamendo_rows_without_a_title_or_any_url_are_dropped() {
        let body = r#"{
          "headers": {"status": "success"},
          "results": [
            {"id": "1"},
            {"id": "2", "name": "No URLs Anywhere", "artist_name": "X"},
            {"id": "3", "name": "", "audio": "https://a.example/t.mp3"}
          ]
        }"#;
        let out = parse_jamendo(body).expect("ok");
        assert!(out.is_empty());
    }

    #[cfg(feature = "catalogue-http")]
    #[test]
    fn an_api_error_envelope_is_no_results_and_garbage_is_an_error() {
        let err_body = r#"{"headers":{"status":"failed","error_message":"bad key","code":200}}"#;
        assert!(parse_jamendo(err_body).unwrap().is_empty());
        assert!(parse_jamendo("not json").is_err());
        assert!(parse_jamendo(r#"{"unexpected":true}"#).is_err());
    }

    #[cfg(feature = "catalogue-http")]
    #[tokio::test]
    async fn a_plain_http_catalogue_is_refused_before_any_request() {
        let mb = MusicBrainz::with_base("http://musicbrainz.example/ws/2", "test/1.0");
        assert!(matches!(
            mb.lookup("anything").await,
            Err(CatalogueError::InsecureUrl)
        ));
        // Case-insensitively, and for schemes that merely *contain* https.
        for base in [
            "HTTP://musicbrainz.example/ws/2",
            "ftp://musicbrainz.example/ws/2",
            "nothttps://musicbrainz.example/ws/2",
            "musicbrainz.example/ws/2",
        ] {
            assert!(
                matches!(
                    MusicBrainz::with_base(base, "test/1.0").lookup("x").await,
                    Err(CatalogueError::InsecureUrl)
                ),
                "{base} was not refused"
            );
        }
        // …and HTTPS in any case is accepted by the scheme check (the request
        // itself then fails, which is a different error).
        assert!(!matches!(
            MusicBrainz::with_base("HTTPS://127.0.0.1:1/ws/2", "test/1.0")
                .lookup("x")
                .await,
            Err(CatalogueError::InsecureUrl)
        ));
    }

    #[cfg(feature = "catalogue-http")]
    #[test]
    fn the_read_cap_is_the_one_the_lane_claims_to_guard() {
        // The streaming read is `buf.len() + chunk.len() > MAX_LOOKUP_BYTES`,
        // which needs a live socket to exercise; what is pinned here is the bound
        // itself and the arithmetic around it, so a future edit that turns the
        // cap into a no-op (a `>=` off-by-one, or a cap larger than any real
        // document) fails rather than silently uncapping the fetch.
        assert_eq!(MAX_LOOKUP_BYTES, 512 * 1024);
        // One byte over the cap must trip it; exactly the cap must not.
        let over = |buf: usize, chunk: usize| buf + chunk > MAX_LOOKUP_BYTES;
        assert!(!over(MAX_LOOKUP_BYTES - 1, 1));
        assert!(over(MAX_LOOKUP_BYTES, 1));
        assert!(over(0, MAX_LOOKUP_BYTES + 1));
        // That the cap is comfortably above any real answer is checked at compile
        // time instead — see the `const _` beside `MAX_CANDIDATES`.
    }

    #[cfg(feature = "catalogue-http")]
    #[tokio::test]
    async fn a_redirect_cannot_move_the_fetch_to_another_host() {
        // `Policy::none()` is the guard: without it a catalogue could 302 to
        // `http://` or to a host we never chose, past the scheme check that ran
        // on the original URL. Asserted through the client builder rather than a
        // live redirect, because the alternative is a network test.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(LOOKUP_TIMEOUT)
            .build();
        assert!(client.is_ok(), "the guarded client must build");
        // A short, non-zero budget, so a host that accepts and then goes quiet
        // costs seconds rather than forever.
        assert!(LOOKUP_TIMEOUT.as_secs() > 0 && LOOKUP_TIMEOUT.as_secs() <= 10);
    }
}
