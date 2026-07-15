/*!
 * Call signaling — voice & video over WebRTC, signalled over the Vault DM channel.
 *
 * Comrade has no media server. Following the design in `AUDIT.md` §8.1, a call is
 * a peer-to-peer **WebRTC** session whose *signaling* (SDP offer/answer + ICE
 * candidates + call control) is carried as ephemeral, end-to-end-encrypted
 * events over the **existing Vault DM channel** (NIP-04 today, NIP-44 later).
 * The direction the wider Nostr community is taking is the NIP-100 draft; we
 * carry the same payloads inside a small JSON envelope so the frontends' WebRTC
 * stacks (`org.webrtc` on Android, the webview's built-in WebRTC on desktop) can
 * negotiate directly.
 *
 * This module is the **wire protocol only** — it is pure, framework-free, and
 * fully unit-tested. The media (mic/camera capture, `RTCPeerConnection`) lives
 * in each frontend; the call *session state* lives in [`comrade_ui`].
 *
 * ## NAT traversal — the honest limit
 * With STUN alone a direct P2P path exists for perhaps 60-70% of real-world
 * pairs; the rest (notably CGNAT, very common on Indian mobile carriers) need a
 * **TURN relay**, which is server infrastructure someone must run. We ship
 * public STUN by default ([`default_ice_servers`]) and let a TURN server be
 * configured; a call that cannot find a path fails with an honest "couldn't
 * connect" rather than pretending. Group calls (SFU territory) are out of scope.
 */

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};

/// Envelope marker for a call-signaling payload riding inside a DM body. A DM
/// whose decrypted body is JSON with `comrade_call == 1` is a call signal, not
/// chat text — the same detection pattern the media pipeline uses.
pub const CALL_ENVELOPE_MARKER: u8 = 1;

/// Whether a call negotiates audio only or audio + video. The signaling and
/// state machine are identical for both; this only tells the frontend whether
/// to capture a camera track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "lowercase")]
pub enum CallMediaKind {
    /// Voice call — microphone only.
    Audio,
    /// Video call — microphone + camera.
    Video,
}

impl CallMediaKind {
    /// Stable wire/string form (`"audio"` / `"video"`), for the FFI bridges.
    pub fn as_str(self) -> &'static str {
        match self {
            CallMediaKind::Audio => "audio",
            CallMediaKind::Video => "video",
        }
    }

    /// Parse the wire/string form; anything unrecognised falls back to audio
    /// (the safe default — never surprise a user with the camera).
    pub fn from_str_lenient(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "video" => CallMediaKind::Video,
            _ => CallMediaKind::Audio,
        }
    }
}

/// Why a call ended — carried on [`CallSignal::Hangup`] so the other side and
/// the call log can distinguish "declined" from "missed" from "network failed".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum HangupReason {
    /// Either party hung up a connected call normally.
    Normal,
    /// The callee actively rejected the incoming call.
    Declined,
    /// The callee was already in another call.
    Busy,
    /// The callee never answered (ring timed out).
    Missed,
    /// The caller cancelled before the callee answered.
    Cancelled,
    /// WebRTC could not establish a media path (e.g. no TURN behind CGNAT).
    Failed,
    /// Reason not specified / unknown build.
    #[serde(other)]
    Unknown,
}

impl HangupReason {
    pub fn as_str(self) -> &'static str {
        match self {
            HangupReason::Normal => "normal",
            HangupReason::Declined => "declined",
            HangupReason::Busy => "busy",
            HangupReason::Missed => "missed",
            HangupReason::Cancelled => "cancelled",
            HangupReason::Failed => "failed",
            HangupReason::Unknown => "unknown",
        }
    }

    pub fn from_str_lenient(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "normal" => HangupReason::Normal,
            "declined" => HangupReason::Declined,
            "busy" => HangupReason::Busy,
            "missed" => HangupReason::Missed,
            "cancelled" | "canceled" => HangupReason::Cancelled,
            "failed" => HangupReason::Failed,
            _ => HangupReason::Unknown,
        }
    }
}

/// One step of the WebRTC negotiation, or a call-control message. Internally
/// tagged (`{"kind":"offer","sdp":"…"}`) so a frontend can `switch` on `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CallSignal {
    /// Caller → callee: the WebRTC session description offer.
    Offer { sdp: String },
    /// Callee → caller: the WebRTC session description answer.
    Answer { sdp: String },
    /// Either direction: a trickled ICE candidate.
    Ice {
        candidate: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sdp_mid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sdp_m_line_index: Option<u16>,
    },
    /// Callee → caller: "your call is ringing on my device" (pre-answer).
    Ringing,
    /// Callee → caller: "I'm already in a call" (auto-reject).
    Busy,
    /// Either direction: tear the call down, with a reason.
    Hangup {
        #[serde(default = "hangup_default")]
        reason: HangupReason,
    },
}

fn hangup_default() -> HangupReason {
    HangupReason::Normal
}

impl CallSignal {
    /// Stable discriminant string for logging / the call log.
    pub fn kind_str(&self) -> &'static str {
        match self {
            CallSignal::Offer { .. } => "offer",
            CallSignal::Answer { .. } => "answer",
            CallSignal::Ice { .. } => "ice",
            CallSignal::Ringing => "ringing",
            CallSignal::Busy => "busy",
            CallSignal::Hangup { .. } => "hangup",
        }
    }
}

/// The JSON envelope carried inside an E2E DM that delivers one [`CallSignal`].
///
/// `call_id` groups every signal of a single call so both peers (and the call
/// log) can correlate offer → ice… → answer → hangup even if signals interleave
/// with other DMs. It is minted once by the caller ([`new_call_id`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallEnvelope {
    /// Format marker / version; must equal [`CALL_ENVELOPE_MARKER`].
    pub comrade_call: u8,
    pub call_id: String,
    pub media: CallMediaKind,
    pub signal: CallSignal,
}

impl CallEnvelope {
    pub fn new(call_id: impl Into<String>, media: CallMediaKind, signal: CallSignal) -> Self {
        Self {
            comrade_call: CALL_ENVELOPE_MARKER,
            call_id: call_id.into(),
            media,
            signal,
        }
    }

    /// Serialise to the JSON string that becomes the DM body.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Detect and parse a call envelope out of a decrypted DM body. Returns `None`
/// for chat text, media envelopes, or any JSON without the call marker — so the
/// runtime can fall through to its other DM handlers.
pub fn parse_call_envelope(content: &str) -> Option<CallEnvelope> {
    let env: CallEnvelope = serde_json::from_str(content).ok()?;
    (env.comrade_call == CALL_ENVELOPE_MARKER).then_some(env)
}

/// Mint a fresh, unguessable call id (128 bits of randomness, hex-encoded).
pub fn new_call_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

// ── ICE server configuration ─────────────────────────────────────────────────

/// A WebRTC ICE server (STUN or TURN), shaped to drop straight into an
/// `RTCConfiguration.iceServers` entry on either frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

impl IceServer {
    /// A STUN server (no credentials).
    pub fn stun(url: impl Into<String>) -> Self {
        Self {
            urls: vec![url.into()],
            username: None,
            credential: None,
        }
    }

    /// A TURN relay with long-term credentials.
    pub fn turn(
        url: impl Into<String>,
        username: impl Into<String>,
        credential: impl Into<String>,
    ) -> Self {
        Self {
            urls: vec![url.into()],
            username: Some(username.into()),
            credential: Some(credential.into()),
        }
    }
}

/// The default, no-infrastructure ICE set: well-known public STUN servers.
///
/// STUN only discovers a peer's public address; it never relays media, so these
/// are free to use and see no call traffic. They are enough for direct P2P when
/// at least one side is not behind a symmetric NAT / CGNAT. For the rest, add a
/// TURN server (see [`comrade_ui`]'s `set_turn_server`).
pub fn default_ice_servers() -> Vec<IceServer> {
    vec![
        IceServer::stun("stun:stun.l.google.com:19302"),
        IceServer::stun("stun:stun1.l.google.com:19302"),
        IceServer::stun("stun:stun.cloudflare.com:3478"),
    ]
}

// ── TURN server configuration: validation + time-limited REST credentials ───
//
// Two related but distinct concerns a deployable TURN capability needs beyond
// "STUN plus one static relay" (see AUDIT.md COMMS-02):
//  1. Whatever URL a user pastes into settings must at least be a plausible
//     `turn:`/`turns:` URI before it is persisted or handed to a
//     `RTCPeerConnection` — [`validate_turn_url`].
//  2. A credential a client holds indefinitely is a standing liability: if the
//     app ever shipped one fixed username/password (or, worse, the operator's
//     shared secret itself) baked into every install, it could never be
//     revoked short of rotating it for every user at once. coturn's
//     `use-auth-secret` mode (the "TURN REST API" convention documented at
//     <https://datatracker.ietf.org/doc/html/draft-uberti-behave-turn-rest>,
//     configured in `deploy/coturn/turnserver.conf`) solves this: an operator
//     keeps one shared secret on the *server* (and on a small credential
//     broker they run — never inside the app), and mints a fresh
//     username/password pair per request that coturn accepts only until the
//     encoded expiry — [`mint_turn_rest_credentials`] is that minting
//     function. Comrade ships no broker of its own (it has no account server
//     to host one on), but any operator-run broker can call this directly.

/// Validate a TURN/STUN server URI well enough to catch obviously-wrong input
/// before it is persisted or handed to a `RTCPeerConnection`: scheme must be
/// `turn`/`turns` (case-insensitive), written the RFC 7065 way (a single
/// colon, e.g. `turn:example.com:3478`, never `turn://example.com`), with a
/// non-empty host and no whitespace/control characters anywhere.
///
/// This is deliberately a lightweight presence/shape check, not a full RFC
/// 3986 URI parser — it exists to reject blank fields, copy-paste mistakes
/// (a `https://` pasted into the wrong box), and typos, not to validate every
/// exotic host form (bracketed IPv6 literals are accepted but not fully
/// parsed). Returns `Ok(())` for an acceptable URL, or an `Err` string safe to
/// show the user directly.
pub fn validate_turn_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("TURN server URL is required".to_string());
    }
    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("URL must not contain whitespace".to_string());
    }
    let Some((scheme, rest)) = trimmed.split_once(':') else {
        return Err("must be a turn:/turns: URL, e.g. turn:example.com:3478".to_string());
    };
    let scheme_lower = scheme.to_lowercase();
    if scheme_lower != "turn" && scheme_lower != "turns" {
        return Err(format!(
            "unsupported scheme '{scheme}' — use turn: or turns: (STUN-only needs no configuration here)"
        ));
    }
    if let Some(after) = rest.strip_prefix('/') {
        let _ = after;
        return Err("TURN URIs use a single colon, not turn://".to_string());
    }
    let host = rest
        .split('?')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    if host.is_empty() {
        return Err("missing host".to_string());
    }
    Ok(())
}

/// Mint one coturn "TURN REST API" time-limited long-term credential pair:
/// `username = "<unix-expiry>"` (or `"<unix-expiry>:<label>"` when `label` is
/// given — coturn accepts an arbitrary suffix after the colon, commonly used
/// for a per-caller identifier in server logs), `password =
/// base64(HMAC-SHA1(shared_secret, username))`. coturn (configured with
/// `use-auth-secret` + the matching `static-auth-secret`, see
/// `deploy/coturn/turnserver.conf`) independently recomputes the same HMAC to
/// accept the pair, and rejects it once `now` passes the encoded expiry — so
/// the credential is only ever useful for `ttl_secs` from `now_unix_secs`,
/// unlike a static long-term username/password that works forever until
/// someone manually revokes it.
///
/// `shared_secret` is the operator's server-side TURN REST secret — it must
/// never reach a client device; only the *minted* username/password pair
/// (this function's return value) is meant to travel to a caller. `now_unix_secs`
/// is a parameter rather than read from the clock so this stays a pure,
/// deterministically testable function; callers pass real wall-clock time.
pub fn mint_turn_rest_credentials(
    shared_secret: &[u8],
    now_unix_secs: u64,
    ttl_secs: u64,
    label: Option<&str>,
) -> (String, String) {
    let expiry = now_unix_secs.saturating_add(ttl_secs);
    let username = match label {
        Some(l) if !l.is_empty() => format!("{expiry}:{l}"),
        _ => expiry.to_string(),
    };
    // A `Hmac<Sha1>` key can be any length (it's hashed down internally when
    // longer than the block size), so this never fails in practice — but the
    // API is fallible, and an empty/misconfigured secret is exactly the kind
    // of operator mistake worth surfacing loudly rather than silently, so a
    // panic (not a swallowed default) is the right failure mode here.
    let mut mac = Hmac::<Sha1>::new_from_slice(shared_secret)
        .expect("HMAC-SHA1 accepts a key of any length, including empty");
    mac.update(username.as_bytes());
    let password = B64.encode(mac.finalize().into_bytes());
    (username, password)
}

// ── STUN-first, TURN-on-failure ──────────────────────────────────────────────

/// Which ICE candidate types a connection attempt should gather.
///
/// A TURN relay sees the (encrypted) media's packet timing/size and both
/// parties' IP addresses — a cost a privacy-first app should only pay when it
/// has to. Comrade therefore tries [`Self::StunOnly`] first on every call; a
/// frontend that observes its `RTCPeerConnection` fail to reach a connected
/// ICE state (the CGNAT case: STUN discovers a public address but no direct
/// or server-reflexive candidate pair actually connects) calls
/// [`ice_servers_for`] with [`Self::StunAndTurn`] and restarts ICE with that
/// widened server list — see `comrade_ui::ComradeRuntime::call_ice_servers_for`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum IceStrategy {
    /// Every call's first attempt: public STUN only.
    StunOnly,
    /// Fallback after a `StunOnly` attempt failed to connect: STUN plus the
    /// configured TURN relay, if any.
    StunAndTurn,
}

impl IceStrategy {
    /// Stable wire/string form, for the FFI bridges.
    pub fn as_str(self) -> &'static str {
        match self {
            IceStrategy::StunOnly => "stun_only",
            IceStrategy::StunAndTurn => "stun_and_turn",
        }
    }

    /// Parse the wire/string form; anything unrecognised falls back to
    /// `StunOnly` — the safe default that never contacts a TURN relay by
    /// accident on malformed input.
    pub fn from_str_lenient(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "stun_and_turn" => IceStrategy::StunAndTurn,
            _ => IceStrategy::StunOnly,
        }
    }
}

/// Build the ICE server list for one connection attempt under `strategy`.
///
/// `turn` is the caller's configured TURN relay, if one has been set (see
/// `comrade_ui`'s `set_turn_server`). With `StunAndTurn` but no TURN
/// configured, this degrades to the same STUN-only list — an honest "no
/// relay available to fall back to" rather than silently doing nothing.
pub fn ice_servers_for(strategy: IceStrategy, turn: Option<&IceServer>) -> Vec<IceServer> {
    let mut servers = default_ice_servers();
    if strategy == IceStrategy::StunAndTurn {
        if let Some(turn) = turn {
            servers.push(turn.clone());
        }
    }
    servers
}

// ── Short authentication string (SAS) for call verification ─────────────────
//
// A SAS lets both participants of a call *verbally* confirm they hold the
// same DTLS-SRTP session — i.e. that no man-in-the-middle re-terminated the
// media path with its own certificate. This module only derives the human-
// readable code from the two sides' already-negotiated SDPs; nothing here
// touches key material, and a failure to verify never blocks or degrades the
// call itself (it is an out-of-band, optional check, exactly like Signal's
// "safety numbers").

/// Extract the DTLS-SRTP certificate fingerprint from one line of an SDP body
/// (`a=fingerprint:<algorithm> <hex-with-colons>`), if present.
pub fn extract_fingerprint(sdp: &str) -> Option<String> {
    const PREFIX: &str = "a=fingerprint:";
    sdp.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix(PREFIX)
            .map(|rest| rest.trim().to_string())
    })
}

/// Emoji alphabet the short authentication string is drawn from.
///
/// All 32 entries are common animal emoji chosen deliberately: each is a
/// single Unicode scalar value with default emoji presentation (no
/// variation selector needed), none has a skin-tone or gender modifier, none
/// is a flag or a multi-glyph ZWJ sequence, and every entry renders
/// identically across current Android/iOS/desktop fonts. That combination
/// matters here specifically because two people are reading the same 4
/// symbols off two different screens and comparing them out loud — a glyph
/// that could silently render as a different sequence of codepoints on one
/// platform (as some ZWJ/skin-tone emoji do) would defeat the whole point.
/// `emoji_alphabet_entries_are_single_codepoint_and_unique` (below) checks
/// this assumption rather than just asserting it.
const EMOJI_ALPHABET: &[&str] = &[
    "🐶", "🐱", "🐭", "🐹", "🐰", "🦊", "🐻", "🐼", "🐨", "🐯", "🦁", "🐮", "🐷", "🐸", "🐵", "🐔",
    "🐧", "🐦", "🐤", "🦆", "🦉", "🐴", "🐗", "🐺", "🐢", "🐍", "🐙", "🦋", "🐝", "🐞", "🐬", "🐳",
];

/// Derive a 4-emoji short authentication string from both sides' SDP
/// fingerprints, for the two call participants to verbally compare and catch
/// a man-in-the-middle. `None` if either SDP lacks a fingerprint — an honest
/// "can't verify" rather than a fabricated code.
///
/// Symmetric by construction: `derive_sas(a, b) == derive_sas(b, a)` always —
/// this is the property that makes the same 4 emoji show up on both phones
/// regardless of which side is "local" and which is "remote". Achieved by
/// sorting the two fingerprints before hashing, so caller/callee order never
/// affects the result.
pub fn derive_sas(local_sdp: &str, remote_sdp: &str) -> Option<Vec<String>> {
    let mut fingerprints = [
        extract_fingerprint(local_sdp)?,
        extract_fingerprint(remote_sdp)?,
    ];
    // Sorting the pair — not the two SDPs, just the two extracted fingerprint
    // strings — is what makes the result order-independent: whichever side
    // calls this, the same two strings get hashed in the same order.
    fingerprints.sort();

    // Hash each fingerprint as a length-prefixed byte string rather than
    // joining the pair with a plain separator. A fingerprint is conventionally
    // `<algorithm-name> <hex-with-colons>` (e.g. "sha-256 AB:CD:…"), so a
    // separator such as `|` that can't appear in that alphabet would in
    // practice be safe too — but length-prefixing removes the need to trust
    // that invariant at all: `("a", "bc")` and `("ab", "c")` hash to different
    // digests even though a naive `a|bc` vs `ab|c` concatenation would not
    // (if either half ever contained the separator byte). `u64` length prefix
    // in big-endian bytes; the exact encoding only needs to be fixed and
    // unambiguous, not compact.
    let mut hasher = Sha256::new();
    for fp in &fingerprints {
        hasher.update((fp.len() as u64).to_be_bytes());
        hasher.update(fp.as_bytes());
    }
    let digest = hasher.finalize();

    Some(
        digest[..4]
            .iter()
            .map(|b| EMOJI_ALPHABET[*b as usize % EMOJI_ALPHABET.len()].to_string())
            .collect(),
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trips_through_json() {
        for signal in [
            CallSignal::Offer {
                sdp: "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\n".into(),
            },
            CallSignal::Answer {
                sdp: "answer-sdp".into(),
            },
            CallSignal::Ice {
                candidate: "candidate:1 1 UDP 2130706431 192.0.2.1 54321 typ host".into(),
                sdp_mid: Some("0".into()),
                sdp_m_line_index: Some(0),
            },
            CallSignal::Ringing,
            CallSignal::Busy,
            CallSignal::Hangup {
                reason: HangupReason::Declined,
            },
        ] {
            let env = CallEnvelope::new("call123", CallMediaKind::Video, signal.clone());
            let json = env.to_json().unwrap();
            let back = parse_call_envelope(&json).expect("must parse back");
            assert_eq!(back, env);
            assert_eq!(back.signal, signal);
        }
    }

    #[test]
    fn parse_rejects_non_call_bodies() {
        // Plain chat text is not JSON.
        assert!(parse_call_envelope("hey, are you free for a call?").is_none());
        // A media envelope is JSON but carries a different marker.
        assert!(parse_call_envelope(
            r#"{"comrade_media":1,"event_id":"e","url":"u","mime":"m","caption":"","size":1}"#
        )
        .is_none());
        // JSON without the marker.
        assert!(parse_call_envelope(r#"{"hello":"world"}"#).is_none());
        // Wrong marker value is rejected.
        assert!(parse_call_envelope(
            r#"{"comrade_call":2,"call_id":"x","media":"audio","signal":{"kind":"ringing"}}"#
        )
        .is_none());
    }

    #[test]
    fn ice_signal_omits_absent_optional_fields() {
        let env = CallEnvelope::new(
            "c",
            CallMediaKind::Audio,
            CallSignal::Ice {
                candidate: "cand".into(),
                sdp_mid: None,
                sdp_m_line_index: None,
            },
        );
        let json = env.to_json().unwrap();
        assert!(!json.contains("sdp_mid"), "absent fields skipped: {json}");
        assert!(!json.contains("sdp_m_line_index"));
        // …and they round-trip back to None.
        assert_eq!(parse_call_envelope(&json).unwrap(), env);
    }

    #[test]
    fn hangup_reason_defaults_when_absent() {
        // An old/other client may send a hangup with no reason.
        let env = parse_call_envelope(
            r#"{"comrade_call":1,"call_id":"c","media":"audio","signal":{"kind":"hangup"}}"#,
        )
        .unwrap();
        assert_eq!(
            env.signal,
            CallSignal::Hangup {
                reason: HangupReason::Normal
            }
        );
    }

    #[test]
    fn unknown_hangup_reason_degrades_gracefully() {
        let env = parse_call_envelope(
            r#"{"comrade_call":1,"call_id":"c","media":"video","signal":{"kind":"hangup","reason":"from_the_future"}}"#,
        )
        .unwrap();
        assert_eq!(
            env.signal,
            CallSignal::Hangup {
                reason: HangupReason::Unknown
            }
        );
    }

    #[test]
    fn media_kind_string_forms_are_stable_and_lenient() {
        assert_eq!(CallMediaKind::Audio.as_str(), "audio");
        assert_eq!(CallMediaKind::Video.as_str(), "video");
        assert_eq!(
            CallMediaKind::from_str_lenient("VIDEO"),
            CallMediaKind::Video
        );
        assert_eq!(
            CallMediaKind::from_str_lenient(" audio "),
            CallMediaKind::Audio
        );
        // Unknown → audio, never surprise the user with the camera.
        assert_eq!(
            CallMediaKind::from_str_lenient("hologram"),
            CallMediaKind::Audio
        );
    }

    #[test]
    fn hangup_reason_string_forms_round_trip() {
        for r in [
            HangupReason::Normal,
            HangupReason::Declined,
            HangupReason::Busy,
            HangupReason::Missed,
            HangupReason::Cancelled,
            HangupReason::Failed,
            HangupReason::Unknown,
        ] {
            assert_eq!(HangupReason::from_str_lenient(r.as_str()), r);
        }
        assert_eq!(
            HangupReason::from_str_lenient("canceled"),
            HangupReason::Cancelled
        );
    }

    #[test]
    fn new_call_id_is_unique_and_hex() {
        let a = new_call_id();
        let b = new_call_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32, "128 bits as hex");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn default_ice_servers_are_stun_only() {
        let servers = default_ice_servers();
        assert!(!servers.is_empty());
        for s in &servers {
            assert!(s.urls.iter().all(|u| u.starts_with("stun:")));
            assert!(s.username.is_none(), "public STUN needs no credentials");
            assert!(s.credential.is_none());
        }
    }

    #[test]
    fn turn_server_carries_credentials() {
        let t = IceServer::turn("turn:turn.example.com:3478", "user", "pass");
        assert_eq!(t.username.as_deref(), Some("user"));
        assert_eq!(t.credential.as_deref(), Some("pass"));
        // Serialises with the credential fields present.
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"username\":\"user\""));
        assert!(json.contains("\"credential\":\"pass\""));
    }

    #[test]
    fn stun_only_strategy_never_includes_turn_even_if_configured() {
        let turn = IceServer::turn("turn:turn.example.com:3478", "u", "p");
        let servers = ice_servers_for(IceStrategy::StunOnly, Some(&turn));
        assert_eq!(servers, default_ice_servers());
        assert!(servers.iter().all(|s| s.username.is_none()));
    }

    #[test]
    fn stun_and_turn_strategy_appends_the_configured_relay() {
        let turn = IceServer::turn("turn:turn.example.com:3478", "u", "p");
        let servers = ice_servers_for(IceStrategy::StunAndTurn, Some(&turn));
        assert_eq!(servers.len(), default_ice_servers().len() + 1);
        assert_eq!(servers.last(), Some(&turn));
    }

    #[test]
    fn stun_and_turn_strategy_degrades_honestly_with_no_turn_configured() {
        // No relay to fall back to — must not silently claim one.
        let servers = ice_servers_for(IceStrategy::StunAndTurn, None);
        assert_eq!(servers, default_ice_servers());
    }

    #[test]
    fn ice_strategy_string_forms_round_trip_and_default_to_stun_only() {
        assert_eq!(
            IceStrategy::from_str_lenient(IceStrategy::StunOnly.as_str()),
            IceStrategy::StunOnly
        );
        assert_eq!(
            IceStrategy::from_str_lenient(IceStrategy::StunAndTurn.as_str()),
            IceStrategy::StunAndTurn
        );
        // Unknown input never accidentally opts into contacting a TURN relay.
        assert_eq!(
            IceStrategy::from_str_lenient("garbage"),
            IceStrategy::StunOnly
        );
    }

    /// A minimal, plausible SDP fragment carrying `fingerprint` as its DTLS
    /// certificate fingerprint line, plus a handful of other `a=` lines that
    /// real SDP always has around it (so tests exercise line-scanning, not a
    /// document containing nothing but the one line we care about).
    fn fake_sdp(fingerprint: &str) -> String {
        format!(
            "v=0\r\n\
             o=- 4028539873384368828 2 IN IP4 127.0.0.1\r\n\
             s=-\r\n\
             t=0 0\r\n\
             a=group:BUNDLE 0\r\n\
             m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
             c=IN IP4 0.0.0.0\r\n\
             a=ice-ufrag:abcd\r\n\
             a=ice-pwd:efghijklmnopqrstuvwxyz012345\r\n\
             a=fingerprint:{fingerprint}\r\n\
             a=setup:actpass\r\n\
             a=mid:0\r\n"
        )
    }

    #[test]
    fn extract_fingerprint_parses_realistic_multiline_sdp() {
        let sdp = fake_sdp(
            "sha-256 AB:CD:EF:12:34:56:78:90:AB:CD:EF:12:34:56:78:90:AB:CD:EF:12:34:56:78:90:AB:CD:EF:12:34:56:78:90",
        );
        assert_eq!(
            extract_fingerprint(&sdp),
            Some(
                "sha-256 AB:CD:EF:12:34:56:78:90:AB:CD:EF:12:34:56:78:90:AB:CD:EF:12:34:56:78:90:AB:CD:EF:12:34:56:78:90"
                    .to_string()
            )
        );
    }

    #[test]
    fn extract_fingerprint_is_none_without_a_fingerprint_line() {
        let sdp = "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\na=setup:actpass\r\na=mid:0\r\n";
        assert!(extract_fingerprint(sdp).is_none());
    }

    #[test]
    fn extract_fingerprint_trims_surrounding_whitespace() {
        // Leading whitespace on the line itself, and trailing whitespace after
        // the value — neither should end up in the returned fingerprint.
        let sdp = "v=0\r\n   a=fingerprint:sha-256 AA:BB   \r\ns=-\r\n";
        assert_eq!(extract_fingerprint(sdp), Some("sha-256 AA:BB".to_string()));
    }

    #[test]
    fn derive_sas_is_symmetric() {
        let sdp_a = fake_sdp("sha-256 AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99");
        let sdp_b = fake_sdp("sha-256 11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00");

        let sas_ab = derive_sas(&sdp_a, &sdp_b);
        let sas_ba = derive_sas(&sdp_b, &sdp_a);
        assert_eq!(
            sas_ab, sas_ba,
            "the same 4 emoji must show up regardless of which side is local/remote"
        );

        let sas = sas_ab.expect("both sides have a fingerprint");
        assert_eq!(sas.len(), 4);
    }

    #[test]
    fn derive_sas_differs_for_different_fingerprint_pairs() {
        let sdp_a = fake_sdp("sha-256 AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA:AA");
        let sdp_b = fake_sdp("sha-256 BB:BB:BB:BB:BB:BB:BB:BB:BB:BB:BB:BB:BB:BB:BB:BB");
        let sdp_c = fake_sdp("sha-256 CC:CC:CC:CC:CC:CC:CC:CC:CC:CC:CC:CC:CC:CC:CC:CC");
        let sdp_d = fake_sdp("sha-256 DD:DD:DD:DD:DD:DD:DD:DD:DD:DD:DD:DD:DD:DD:DD:DD");

        let sas_1 = derive_sas(&sdp_a, &sdp_b).expect("a/b have fingerprints");
        let sas_2 = derive_sas(&sdp_c, &sdp_d).expect("c/d have fingerprints");
        assert_ne!(
            sas_1, sas_2,
            "two clearly different fingerprint pairs should not collide to the same code"
        );
    }

    #[test]
    fn derive_sas_is_deterministic() {
        let sdp_a = fake_sdp("sha-256 AA:BB:CC:DD");
        let sdp_b = fake_sdp("sha-256 EE:FF:00:11");
        assert_eq!(derive_sas(&sdp_a, &sdp_b), derive_sas(&sdp_a, &sdp_b));
    }

    #[test]
    fn derive_sas_is_none_when_either_side_lacks_a_fingerprint() {
        let with_fp = fake_sdp("sha-256 AA:BB:CC:DD");
        let without_fp =
            "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\na=setup:actpass\r\n".to_string();

        assert!(derive_sas(&with_fp, &without_fp).is_none());
        assert!(derive_sas(&without_fp, &with_fp).is_none());
        assert!(derive_sas(&without_fp, &without_fp).is_none());
    }

    #[test]
    fn validate_turn_url_accepts_well_formed_turn_and_turns_uris() {
        assert!(validate_turn_url("turn:example.com:3478").is_ok());
        assert!(validate_turn_url("turns:example.com:5349").is_ok());
        assert!(validate_turn_url("turn:example.com:3478?transport=tcp").is_ok());
        assert!(
            validate_turn_url("TURN:example.com:3478").is_ok(),
            "scheme is case-insensitive"
        );
        assert!(
            validate_turn_url("  turn:example.com:3478  ").is_ok(),
            "surrounding whitespace is trimmed"
        );
    }

    #[test]
    fn validate_turn_url_rejects_blank_and_missing_host() {
        assert!(validate_turn_url("").is_err());
        assert!(validate_turn_url("   ").is_err());
        assert!(validate_turn_url("turn:").is_err());
        assert!(validate_turn_url("turn::3478").is_err());
    }

    #[test]
    fn validate_turn_url_rejects_wrong_scheme() {
        assert!(validate_turn_url("https://example.com").is_err());
        assert!(
            validate_turn_url("stun:example.com:19302").is_err(),
            "STUN needs no TURN configuration"
        );
        assert!(validate_turn_url("example.com:3478").is_err());
    }

    #[test]
    fn validate_turn_url_rejects_double_slash_and_whitespace() {
        assert!(
            validate_turn_url("turn://example.com:3478").is_err(),
            "turn URIs use one colon, not turn://"
        );
        assert!(validate_turn_url("turn:exa mple.com:3478").is_err());
        // A leading/trailing newline is exactly the copy-paste noise `trim()`
        // is supposed to absorb (see the "surrounding whitespace is trimmed"
        // acceptance test) — this checks an *embedded* control character,
        // which trimming cannot and must not hide.
        assert!(validate_turn_url("turn:exam\nple.com:3478").is_err());
    }

    #[test]
    fn turn_rest_credentials_match_independently_computed_hmac_sha1() {
        // Known-answer test computed independently in Python:
        //   hmac.new(b"test-shared-secret", b"1799999999:alice", hashlib.sha1)
        // — pins this function to the exact coturn REST API construction,
        // not just "produces something base64-shaped".
        let (username, password) =
            mint_turn_rest_credentials(b"test-shared-secret", 1_799_999_999, 0, Some("alice"));
        assert_eq!(username, "1799999999:alice");
        assert_eq!(password, "mduJh+ql8gT3UMViKWYnFuZf/rQ=");
    }

    #[test]
    fn turn_rest_credentials_encode_expiry_as_now_plus_ttl() {
        let (username, _) = mint_turn_rest_credentials(b"secret", 1_000, 3_600, None);
        assert_eq!(username, "4600");
    }

    #[test]
    fn turn_rest_credentials_omit_label_when_none_or_empty() {
        let (username, _) = mint_turn_rest_credentials(b"secret", 1_000, 60, None);
        assert_eq!(username, "1060");
        let (username, _) = mint_turn_rest_credentials(b"secret", 1_000, 60, Some(""));
        assert_eq!(username, "1060");
    }

    #[test]
    fn turn_rest_credentials_are_deterministic_and_secret_dependent() {
        let a = mint_turn_rest_credentials(b"secret-one", 1_000, 60, Some("bob"));
        let b = mint_turn_rest_credentials(b"secret-one", 1_000, 60, Some("bob"));
        assert_eq!(a, b, "same inputs must mint the same pair");

        let c = mint_turn_rest_credentials(b"secret-two", 1_000, 60, Some("bob"));
        assert_ne!(a.1, c.1, "different secrets must mint different passwords");
    }

    #[test]
    fn emoji_alphabet_entries_are_single_codepoint_and_unique() {
        // Verifies the assumption the doc comment on `EMOJI_ALPHABET` makes,
        // rather than just asserting it: every entry must be exactly one
        // Unicode scalar value (no ZWJ sequence, no variation selector) and
        // no two entries may be the same glyph.
        assert!(EMOJI_ALPHABET.len() >= 32);
        let mut seen = std::collections::HashSet::new();
        for emoji in EMOJI_ALPHABET {
            assert_eq!(
                emoji.chars().count(),
                1,
                "{emoji:?} must be exactly one Unicode scalar value"
            );
            assert!(seen.insert(*emoji), "{emoji:?} appears more than once");
        }
    }
}
