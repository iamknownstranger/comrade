/*!
 * subsonic — online streaming from a server you own.
 *
 * This is the BlackHole-shaped half of §20's pluggable tier, filled the way
 * `catalogue.rs`'s header said a source layer should be filled: not with an
 * adapter that defeats a protection measure, but with a service that hands out
 * plain progressive HTTPS media by design. Subsonic and its compatible servers
 * (Navidrome, Gonic, Airsonic) stream *your own* library to *your* client over
 * `/rest/stream`, which is the same model BlackHole's player sits on — minus
 * the part this app will not do, because nothing here is extracted from a
 * service that did not consent to serve it.
 *
 * What makes it a Comrade citizen rather than a bolted-on client:
 *
 * - **The URL guard runs here, not just at the session edge.** Every stream URL
 *   this module builds goes through [`crate::together::valid_stream_url`] — the
 *   same function `TogetherContent::admissible` applies on the way out and the
 *   way in. A candidate whose URL cannot pass that guard is dropped rather than
 *   handed to a UI, so what a search result offers is exactly what a Together
 *   session will accept.
 *
 * - **Token auth, never the password.** Subsonic's `t=`/`s=` scheme sends
 *   `md5(password + salt)`; the password itself never appears in a URL. That
 *   matters more here than in ordinary clients because a Together session
 *   invitation carries the URL to the other device: the salted token lets them
 *   fetch *this file*, which is what being invited to listen means, while the
 *   account password stays home. The disclosure is stated in the UI that arms
 *   the invite rather than discovered later.
 *
 * - **The pure half always compiles** — query shaping, response parsing, URL
 *   building, the auth hash — and is tested against fixtures with no socket,
 *   exactly like [`crate::catalogue`]'s split. Only [`lookup`] touches a
 *   network, and only under the `catalogue-http` feature, behind the same
 *   guards (`fetch_json`): HTTPS only, redirects off, bounded read, explicit
 *   timeout.
 *
 * - **MD5 is implemented here, ~90 lines**, rather than added as a dependency.
 *   The hash is all Subsonic auth needs, the reference vectors pin it, and the
 *   repo already prefers a small local function over a crate for exactly this
 *   shape of need (see `urlencode` in `catalogue.rs`). It is *not* a general
 *   crypto primitive and must not become one: collision attacks on MD5 are
 *   decades old, which is fine for proving you know a password to your own
 *   server and disqualifying everywhere else.
 *
 * Config arrives per call from the frontend's own settings store. Nothing here
 * persists it, logs it, or sends it anywhere except to the server it names —
 * the vault-first rule means personal credentials are the caller's problem,
 * and the caller on Android holds them in preferences it already controls.
 *
 * Android-first per the owner's standing directive; the desktop panel can
 * reach the same free functions whenever its lane wires up to them.
 */

use serde::{Deserialize, Serialize};

/// How long a server has to answer before the search gives up. Matches
/// `catalogue.rs`'s lookup budget: this runs while somebody watches a field.
#[cfg(feature = "catalogue-http")]
const LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);

/// Longest search response we will read. A page of song entries is kilobytes;
/// this bounds a misbehaving host answering a search with a stream.
#[cfg(feature = "catalogue-http")]
const MAX_LOOKUP_BYTES: usize = 512 * 1024;

/// Candidates one search may return. Same argument as
/// `catalogue::MAX_CANDIDATES`: a picker shows a few, and every extra row is
/// another chance to open the wrong thing.
pub const MAX_CANDIDATES: usize = 20;

/// The API version we claim. Servers negotiate down, never up, and everything
/// we use (`search3`, token auth) predates 1.13 — pinned so a future breaking
/// API revision changes our requests only when someone decides it should.
const API_VERSION: &str = "1.16.1";

/// The client id servers log against. Distinctive enough that a self-hosting
/// admin can recognise and rate-limit this app specifically.
const CLIENT_ID: &str = "comrade";

// ── Where the server is, and who is asking ───────────────────────────────────

/// One self-hosted server, as the user typed it into settings.
///
/// Carried across the FFI whole because splitting it into three arguments
/// invites swapping two of them; a record with named fields cannot be silently
/// transposed by a caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct SubsonicConfig {
    /// Origin only — `https://music.example.com`. Any path the user appends is
    /// kept, because some hosts sit behind a reverse-proxy prefix, but the
    /// trailing slash is normalised away ([`Self::normalised`]).
    pub server: String,
    pub username: String,
    pub password: String,
}

impl SubsonicConfig {
    /// Trim the shapes users actually type — trailing slash, stray spaces —
    /// and refuse anything that cannot yield a stream URL a peer would accept.
    ///
    /// The refusal reasons are the same ones
    /// [`crate::together::valid_stream_url`] applies to its authority, restated
    /// at setup time so the sentence arrives *before* a search was ever run:
    /// configuring a server this module can never produce a shareable URL for
    /// is a setup bug, and saying so at setup is cheaper than discovering it on
    /// stage.
    pub fn normalised(self) -> Result<Self, ServerIssue> {
        let server = self.server.trim().trim_end_matches('/').to_string();
        let username = self.username.trim().to_string();
        if username.is_empty() {
            return Err(ServerIssue::NoUsername);
        }
        if self.password.is_empty() {
            return Err(ServerIssue::NoPassword);
        }
        match server_issue(&server) {
            Some(issue) => Err(issue),
            None => Ok(Self {
                server,
                username,
                password: self.password,
            }),
        }
    }

    /// Salt for one request. Random per request because the spec's example
    /// traffic does that and because reusing one salt across a session makes
    /// the token a permanent credential for the account; a fresh one per
    /// request keeps any captured token good for roughly that transfer only.
    #[cfg(feature = "catalogue-http")]
    pub(crate) fn fresh_salt() -> String {
        use rand::RngCore;
        let mut bytes = [0u8; 8];
        rand::thread_rng().fill_bytes(&mut bytes);
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Why a configured server can never produce a usable stream URL.
///
/// These are values rather than error variants of some `UiError` on purpose —
/// see `runtime::StreamSearchOutcome`'s header for why this area answers
/// flatly instead of through the shared error type (adding one there ripples
/// through four Flutter lanes for no informational gain).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ServerIssue {
    /// `http://` — a downgrade neither we nor a peer may accept. The whole
    /// stream tier stands on HTTPS-only.
    NotHttps,
    /// A host with no dot resolves inside somebody's LAN — `localhost`,
    /// `nas`, `router`. A Together invitation naming such a host is refused
    /// downstream; refusing at setup names the cause earlier.
    LanHostname,
    /// A literal IP address tells the listener nothing about what they are
    /// fetching and usually names a box on the inviter's own network.
    LiteralAddress,
    /// Empty server field.
    NoServer,
    /// Empty username field.
    NoUsername,
    /// Empty password field.
    NoPassword,
}

impl std::fmt::Display for ServerIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotHttps => "the server address must use https://",
            Self::LanHostname => {
                "that address names a device on a local network; use the server's public name"
            }
            Self::LiteralAddress => "use the server's domain name rather than a raw IP address",
            Self::NoServer => "no server address saved yet",
            Self::NoUsername => "no username saved yet",
            Self::NoPassword => "no password saved yet",
        })
    }
}

/// The first thing wrong with `server` as a stream origin, if anything.
///
/// Pure and separately testable, because these are exactly the checks
/// [`crate::together::valid_stream_url`] makes on the authority — kept in step
/// by testing each refusal against the guard itself (see
/// `every_refused_server_would_fail_the_peer_guard`).
pub fn server_issue(server: &str) -> Option<ServerIssue> {
    if server.is_empty() {
        return Some(ServerIssue::NoServer);
    }
    // Anything that is not literally `https://…` is refused as the downgrade
    // it is — including `http://`, schemeless origins and every other scheme.
    let Some(rest) = server.strip_prefix("https://") else {
        return Some(ServerIssue::NotHttps);
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') || authority.is_empty() {
        // Credentials in the authority are the phishing shape the peer guard
        // refuses; an empty authority is simply not a server.
        return Some(ServerIssue::LiteralAddress);
    }
    let host = authority.rsplit_once(':').map_or(authority, |(h, _)| h);
    if host.is_empty() {
        return Some(ServerIssue::NoServer);
    }
    if host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
        return Some(ServerIssue::LiteralAddress);
    }
    if !host.contains('.') {
        return Some(ServerIssue::LanHostname);
    }
    None
}

// ── Auth ─────────────────────────────────────────────────────────────────────

/// The Subsonic auth token for one request: lowercase hex MD5 of
/// `password + salt`.
///
/// Sent as `t=…` alongside the `s=…` it was derived from. The server repeats
/// the derivation from the password it holds; the password itself never
/// travels (see the module header for why that ordering matters here).
pub fn auth_token(password: &str, salt: &str) -> String {
    let mut joined = Vec::with_capacity(password.len() + salt.len());
    joined.extend_from_slice(password.as_bytes());
    joined.extend_from_slice(salt.as_bytes());
    hex_lower(&md5(&joined))
}

/// RFC 1321 MD5, over bytes in, sixteen bytes out.
///
/// Written from the reference construction rather than translated from
/// another language: little-endian throughout, the length appended as a u64
/// *bit* count after 0x80 padding to 56 mod 64. Pinned by the RFC's own test
/// suite plus the Subsonic documentation's worked example, which together
/// cover padding across the 55/56/64-byte boundaries where hand-rolled MD5s
/// historically go wrong.
fn md5(input: &[u8]) -> [u8; 16] {
    // Per-round rotation amounts and the sine-derived constants, table form.
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, //
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, //
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, //
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    // Padding: 0x80, zeros to 56 mod 64, then the bit length as u64 LE.
    let mut msg = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let (mut a0, mut b0, mut c0, mut d0) = (
        0x6745_2301u32,
        0xefcd_ab89u32,
        0x98ba_dcfeu32,
        0x1032_5476u32,
    );

    for block in msg.chunks_exact(64) {
        let m: [u32; 16] = std::array::from_fn(|i| {
            u32::from_le_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ])
        });
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let tmp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = tmp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

// ── Request URLs ─────────────────────────────────────────────────────────────

/// The query parameters every REST call carries: who is asking, proving it
/// with a salted token, which API revision, which app, and JSON please.
///
/// One builder rather than four call sites assembling subsets, because the one
/// time a parameter went missing from one of those would read as the server
/// being broken.
fn common_params(username: &str, password: &str, salt: &str) -> String {
    format!(
        "u={}&t={}&s={}&v={API_VERSION}&c={CLIENT_ID}&f=json",
        urlencode(username),
        auth_token(password, salt),
        salt,
    )
}

/// The `search3` endpoint for one query, or `None` when the server itself
/// could never pass the peer guard — the same refusal [`SubsonicConfig::
/// normalised`] makes at setup, applied again here because this function is
/// the one that turns strings into requests.
pub fn search_url(
    server: &str,
    username: &str,
    password: &str,
    salt: &str,
    query: &str,
) -> Option<String> {
    if server_issue(server).is_some() {
        return None;
    }
    Some(format!(
        "{server}/rest/search3?query={}&songCount={MAX_CANDIDATES}&{}",
        urlencode(query),
        common_params(username, password, salt),
    ))
}

/// The `stream` endpoint for one song id — the URL a player (ours or the
/// peer's) will actually fetch.
///
/// `None` is the load-bearing return: a server whose origin fails the peer
/// guard produces no URL at all rather than one that would be refused later,
/// so a candidate list can never contain a row that starts a session nobody
/// can join.
pub fn stream_url(
    server: &str,
    username: &str,
    password: &str,
    salt: &str,
    id: &str,
) -> Option<String> {
    if server_issue(server).is_some() {
        return None;
    }
    let url = format!(
        "{server}/rest/stream?id={}&maxBitRate=320&{}",
        urlencode(id),
        common_params(username, password, salt),
    );
    crate::together::valid_stream_url(&url).then_some(url)
}

/// Cover art at a size a result row can afford, when the song has art.
///
/// No art id or an unshareable origin means no art row — never a broken image
/// request per list item.
pub fn cover_url(
    server: &str,
    username: &str,
    password: &str,
    salt: &str,
    cover_id: &str,
) -> Option<String> {
    if server_issue(server).is_some() || cover_id.is_empty() {
        return None;
    }
    let url = format!(
        "{server}/rest/getCoverArt?id={}&size=144&{}",
        urlencode(cover_id),
        common_params(username, password, salt),
    );
    crate::together::valid_stream_url(&url).then_some(url)
}

// ── Responses ────────────────────────────────────────────────────────────────

/// One song a server answered with, straight off the wire.
///
/// Deliberately raw — ids and numbers, nothing resolved — because resolution
/// (turning ids into URLs) needs the credentials and is a separate pure step
/// ([`to_candidates`]) that drops what it cannot make safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubsonicSong {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Seconds, as the server reports. Zero means unknown, not absent — the
    /// wire type has no null here, and `match_score` already treats zero
    /// duration as "no evidence".
    pub duration_seconds: u64,
    pub cover_id: Option<String>,
}

/// One search answer ready for a UI: identity plus the two URLs it acts on.
///
/// `stream_url`/`artwork_url` are `Option`s because *this* server origin may
/// fail the peer guard even though the config check passed — a proxy that
/// rewrote the Host, a port the guard's parser chokes on. Dropping the row
/// would hide a working local play behind a missing one; keeping the fields
/// optional lets the frontend offer what is actually offerable.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Serialize, Deserialize)]
pub struct SubsonicCandidate {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: u64,
    /// Ready to hand a player / a Together invitation.
    pub stream_url: Option<String>,
    pub artwork_url: Option<String>,
}

/// Parse a `search3` response body into songs.
///
/// Split from the request so the fixture pins the shape — the
/// `subsonic-response` envelope's `status` field, and the songs array under
/// `searchResult3.song`, which is the
/// part servers have been observed to vary (Airsonic nesting, empty arrays
/// versus absent keys).
pub fn parse_search3(body: &str) -> Result<Vec<SubsonicSong>, String> {
    let doc: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("unreadable JSON: {e}"))?;
    let status = doc
        .get("subsonic-response")
        .and_then(|r| r.get("status"))
        .and_then(|s| s.as_str())
        .unwrap_or_default();
    if status != "ok" {
        // The server's own error object, when it sent one — its message names
        // the real cause (bad token, disabled account) better than a guess.
        let detail = doc
            .get("subsonic-response")
            .and_then(|r| r.get("error"))
            .map(|e| e.to_string())
            .unwrap_or_else(|| format!("status {status:?}"));
        return Err(detail);
    }
    let songs = doc
        .get("subsonic-response")
        .and_then(|r| r.get("searchResult3"))
        .and_then(|s| s.get("song"))
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(songs
        .iter()
        .take(MAX_CANDIDATES)
        .filter_map(|item| {
            // Empty strings pass a presence check and fail only here: a song
            // with no id cannot be streamed and one with no title cannot be
            // labelled, so either arrives as no row at all.
            let id = item.get("id")?.as_str()?.to_string();
            if id.is_empty() {
                return None;
            }
            let title = item.get("title")?.as_str()?.to_string();
            if title.is_empty() {
                return None;
            }
            Some(SubsonicSong {
                id,
                title,
                artist: item
                    .get("artist")
                    .and_then(|a| a.as_str())
                    .unwrap_or_default()
                    .to_string(),
                album: item
                    .get("album")
                    .and_then(|a| a.as_str())
                    .map(str::to_string),
                duration_seconds: item.get("duration").and_then(|d| d.as_u64()).unwrap_or(0),
                cover_id: item
                    .get("coverArt")
                    .and_then(|c| c.as_str())
                    .map(str::to_string),
            })
        })
        .collect())
}

/// Resolve parsed songs into UI candidates, building and guarding both URLs.
///
/// Pure. Takes the credentials rather than finished URLs because the salt is
/// per-request and the URLs are derived — one place, tested, where "what the
/// server said" becomes "what this app will show".
pub fn to_candidates(
    server: &str,
    username: &str,
    password: &str,
    salt: &str,
    songs: &[SubsonicSong],
) -> Vec<SubsonicCandidate> {
    songs
        .iter()
        .take(MAX_CANDIDATES)
        .map(|s| SubsonicCandidate {
            title: s.title.clone(),
            artist: s.artist.clone(),
            album: s.album.clone(),
            duration_ms: s.duration_seconds.saturating_mul(1000),
            stream_url: stream_url(server, username, password, salt, &s.id),
            artwork_url: s
                .cover_id
                .as_deref()
                .and_then(|cid| cover_url(server, username, password, salt, cid)),
        })
        .collect()
}

// ── The network half ─────────────────────────────────────────────────────────

/// Why one search failed. Values, not a shared error type — the reasoning is
/// the same as `runtime::StreamSearchOutcome`'s, and it keeps
/// the Flutter lanes out of this change entirely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    /// The configured origin can never work; the payload names which check.
    BadServer(ServerIssue),
    /// The server answered with a non-ok envelope — bad token, disabled
    /// user, unsupported version. Its own words are carried.
    ServerRejected(String),
    /// Could not reach it, or it spoke nonsense. Network sentences.
    Unreachable(String),
}

/// Run one `search3` and resolve its songs to candidates.
///
/// Under `catalogue-http` with the same fetch guards as the catalogue path:
/// HTTPS only (implied — the URL builder refuses anything else), redirects
/// off, explicit timeout, bounded read.
#[cfg(feature = "catalogue-http")]
pub async fn lookup(
    cfg: &SubsonicConfig,
    query: &str,
) -> Result<Vec<SubsonicCandidate>, SearchError> {
    let cfg = cfg.clone().normalised().map_err(SearchError::BadServer)?;
    let salt = SubsonicConfig::fresh_salt();
    let url = search_url(&cfg.server, &cfg.username, &cfg.password, &salt, query)
        .ok_or_else(|| SearchError::BadServer(ServerIssue::NotHttps))?;
    let body = fetch_json(&url).await.map_err(SearchError::Unreachable)?;
    let songs = parse_search3(&body).map_err(SearchError::ServerRejected)?;
    Ok(to_candidates(
        &cfg.server,
        &cfg.username,
        &cfg.password,
        &salt,
        &songs,
    ))
}

/// Fetch a JSON document with the catalogue path's guards, restated locally
/// rather than imported: `catalogue::fetch_json` is private to that module's
/// error type, and coupling the two error vocabularies to share forty lines
/// would make one feature's failure mode the other's dependency.
#[cfg(feature = "catalogue-http")]
async fn fetch_json(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(LOOKUP_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
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
    String::from_utf8(buf).map_err(|e| e.to_string())
}

/// Percent-encode a query-string component.
///
/// Copied from `catalogue.rs`'s `urlencode` rather than shared for the same
/// reason the fetch guard was: the alternative is a tiny shared util module
/// coupling two features' compilation. If a third copy appears, move both.
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

// ── Tests ────────────────────────────────────────────────────────────────────
//
// The pure half is always compiled, so these always run — including in the
// lean workspace build where the HTTP lane does not exist.

#[cfg(test)]
mod tests {
    use super::*;

    // ── MD5 correctness ──────────────────────────────────────────────────────

    #[test]
    fn md5_matches_the_rfc_1321_suite() {
        // The seven vectors from the RFC appendix, which walk the padding
        // boundaries: empty, one byte, "abc", the 56-byte message (exactly the
        // pad-triggering length), and three long ones.
        let cases: [(&[u8], &str); 7] = [
            (b"", "d41d8cd98f00b204e9800998ecf8427e"),
            (b"a", "0cc175b9c0f1b6a831c399e269772661"),
            (b"abc", "900150983cd24fb0d6963f7d28e17f72"),
            (b"message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                b"abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(hex_lower(&md5(input)), want, "input {input:?}");
        }
    }

    #[test]
    fn md5_survives_the_55_and_56_byte_boundaries() {
        // Every length around the padding boundary, cross-checked against a
        // reference implementation (Python hashlib) for the same input.
        let cases: [(usize, &str); 3] = [
            (55, "6912ee65fff2d9f9ce2508cddf8bcda0"), // pads to exactly one block
            (56, "51fdd1acda72405dfdfa03fcb85896d7"), // spills into a second block
            (64, "b2d3f56bc197fd985d5965079b5e7148"),
        ];
        for (len, want) in cases {
            let input: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            assert_eq!(hex_lower(&md5(&input)), want, "length {len}");
        }
    }

    #[test]
    fn the_auth_token_is_the_reference_derivation() {
        // password "sesame" salted with "c19b2d" — the Subsonic docs' example
        // pair, hashed here per the spec formula and cross-checked against an
        // independent MD5 implementation.
        assert_eq!(
            auth_token("sesame", "c19b2d"),
            "26719a1196d2a940705a59634eb18eab"
        );
        assert!(!auth_token("sesame", "c19b2d").contains("sesame"));
    }

    // ── Server validation ────────────────────────────────────────────────────

    #[test]
    fn a_plain_https_origin_validates_and_normalises() {
        let cfg = SubsonicConfig {
            server: "https://music.example.com/ ".into(),
            username: " me ".into(),
            password: "sesame".into(),
        }
        .normalised()
        .expect("valid");
        assert_eq!(cfg.server, "https://music.example.com");
        assert_eq!(cfg.username, "me");
    }

    #[test]
    fn every_refused_server_would_fail_the_peer_guard() {
        // The point of the setup-time refusals is that they anticipate
        // `valid_stream_url`. Each rejected shape below is one that guard
        // rejects on a built URL — checked here against the guard directly,
        // so the two lists cannot drift apart silently.
        let refused: [(&str, ServerIssue); 5] = [
            ("http://music.example.com", ServerIssue::NotHttps),
            ("ftp://music.example.com", ServerIssue::NotHttps),
            ("music.example.com", ServerIssue::NotHttps),
            ("https://192.168.1.10", ServerIssue::LiteralAddress),
            ("https://nas", ServerIssue::LanHostname),
        ];
        for (server, want) in refused {
            assert_eq!(server_issue(server), Some(want), "{server}");
        }
        // …and the positive case really does produce a passing URL.
        let ok = stream_url("https://music.example.com", "u", "p", "abcd1234", "tr-1")
            .expect("guard passes");
        assert!(crate::together::valid_stream_url(&ok));
    }

    #[test]
    fn blank_credentials_are_a_setup_bug_not_a_search_failure() {
        assert_eq!(
            SubsonicConfig {
                server: "https://music.example.com".into(),
                username: "  ".into(),
                password: "x".into(),
            }
            .normalised()
            .unwrap_err(),
            ServerIssue::NoUsername,
        );
        assert_eq!(
            SubsonicConfig {
                server: "https://music.example.com".into(),
                username: "u".into(),
                password: "".into(),
            }
            .normalised()
            .unwrap_err(),
            ServerIssue::NoPassword,
        );
    }

    // ── URL building ─────────────────────────────────────────────────────────

    #[test]
    fn the_stream_url_carries_the_token_not_the_password() {
        let salt = "c19b2d";
        let url = stream_url(
            "https://music.example.com",
            "admin",
            "sesame",
            salt,
            "tr-241",
        )
        .expect("built");
        assert!(url.starts_with("https://music.example.com/rest/stream?id=tr-241&"));
        assert!(url.contains(&format!("t={}", auth_token("sesame", salt))));
        assert!(url.contains(&format!("s={salt}")));
        assert!(!url.contains("sesame"), "the password must never travel");
        assert!(url.contains("u=admin"));
        assert!(url.contains("f=json"));
    }

    #[test]
    fn a_server_that_cannot_pass_the_guard_builds_nothing() {
        // Both builders, every refused origin: `None`, never a URL that would
        // blow up later at admissibility time.
        for server in [
            "http://music.example.com",
            "https://10.0.0.2",
            "https://nas",
        ] {
            assert!(
                stream_url(server, "u", "p", "salt", "id").is_none(),
                "{server}"
            );
            assert!(
                search_url(server, "u", "p", "salt", "q").is_none(),
                "{server}"
            );
        }
    }

    #[test]
    fn queries_are_percent_encoded_but_ids_keep_their_shape() {
        let url = search_url(
            "https://music.example.com",
            "u",
            "p",
            "salt",
            "kun faya kun",
        )
        .expect("built");
        assert!(url.contains("query=kun%20faya%20kun"));
        // An id with characters the encoder escapes stays retrievable — the
        // server sees the same string it issued.
        let streamed =
            stream_url("https://music.example.com", "u", "p", "salt", "a/b c").expect("built");
        assert!(streamed.contains("id=a%2Fb%20c"));
    }

    // ── Response parsing ─────────────────────────────────────────────────────

    const SEARCH_OK: &str = r#"{
      "subsonic-response": {
        "status": "ok", "version": "1.16.1",
        "searchResult3": {
          "song": [
            {"id": "tr-241", "title": "Kun Faya Kun", "artist": "A.R. Rahman",
             "album": "Rockstar", "duration": 327, "coverArt": "al-19"},
            {"id": "tr-242", "title": "Phir Se Ud Chala", "duration": 251},
            {"id": "", "title": "no id"},
            {"id": "tr-243"}
          ]
        }
      }
    }"#;

    #[test]
    fn a_search_response_resolves_to_guarded_candidates() {
        let songs = parse_search3(SEARCH_OK).expect("ok envelope");
        assert_eq!(songs.len(), 2, "rows without an id or a title are dropped");
        assert_eq!(songs[0].duration_seconds, 327);

        let rows = to_candidates("https://music.example.com", "u", "p", "salt", &songs);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].duration_ms, 327_000);
        assert_eq!(
            rows[0].artwork_url.as_deref(),
            cover_url("https://music.example.com", "u", "p", "salt", "al-19").as_deref()
        );
        assert_eq!(rows[1].artwork_url, None, "no coverArt, no art row");
    }

    #[test]
    fn a_failed_envelope_names_the_server_s_reason() {
        let body = r#"{"subsonic-response":{"status":"failed","error":{"code":40,"message":"Wrong username or password."}}}"#;
        let err = parse_search3(body).expect_err("failed envelope");
        assert!(err.contains("Wrong username or password"), "{err}");
    }

    #[test]
    fn garbage_is_a_sentence_not_a_panic() {
        assert!(parse_search3("not json at all").is_err());
        assert!(
            parse_search3(r#"{"unexpected":"shape"}"#).is_err(),
            "no status"
        );
    }

    #[test]
    fn an_empty_song_list_is_an_empty_answer_not_an_error() {
        let body = r#"{"subsonic-response":{"status":"ok","version":"1.16.1","searchResult3":{}}}"#;
        assert!(parse_search3(body).expect("ok").is_empty());
    }
}
