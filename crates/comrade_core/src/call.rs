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

use rand::RngCore;
use serde::{Deserialize, Serialize};

/// Envelope marker for a call-signaling payload riding inside a DM body. A DM
/// whose decrypted body is JSON with `comrade_call == 1` is a call signal, not
/// chat text — the same detection pattern the media pipeline uses.
pub const CALL_ENVELOPE_MARKER: u8 = 1;

/// Whether a call negotiates audio only or audio + video. The signaling and
/// state machine are identical for both; this only tells the frontend whether
/// to capture a camera track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
}
