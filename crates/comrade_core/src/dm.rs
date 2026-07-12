/*!
 * DM control envelopes — structured, non-chat payloads that ride inside E2E DMs.
 *
 * A decrypted Vault DM body is normally plain chat text. A few messenger
 * features need to send *structured* data over the same encrypted channel
 * without it showing up as a chat bubble:
 *
 *  • [`ProfileShare`] — sent when you **accept a message request**, so the peer
 *    learns your @handle. This realises "your username is only shared once the
 *    request is accepted": strangers who DM you never receive your profile until
 *    you let them in.
 *  • [`Receipt`] — a **delivered / read** acknowledgement referencing the ids of
 *    the messages it covers, so the sender's UI can show ticks.
 *
 * Each envelope carries a distinct required marker field (`comrade_profile`,
 * `comrade_receipt`), so its `parse_*` function accepts only its own shape and
 * returns `None` for chat text, media/call envelopes, or any other JSON. That
 * lets the runtime try each handler in turn and fall through to a plain DM.
 *
 * Pure and framework-free — fully unit-tested here.
 */

use serde::{Deserialize, Serialize};

/// Marker value identifying a well-formed control envelope of a given type.
pub const CONTROL_ENVELOPE_MARKER: u8 = 1;

// ── Profile share (sent on accepting a message request) ───────────────────────

/// Delivered privately to a peer the moment you accept their message request,
/// telling them the display handle to title the conversation with. The npub is
/// already known (it is the DM sender), so only the mutable, self-declared
/// fields travel here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileShare {
    /// Format marker; must equal [`CONTROL_ENVELOPE_MARKER`].
    pub comrade_profile: u8,
    /// The sender's chosen @handle, if they have set one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

impl ProfileShare {
    pub fn new(username: Option<String>) -> Self {
        Self {
            comrade_profile: CONTROL_ENVELOPE_MARKER,
            username,
        }
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Detect and parse a profile-share envelope out of a decrypted DM body.
pub fn parse_profile_share(content: &str) -> Option<ProfileShare> {
    let env: ProfileShare = serde_json::from_str(content).ok()?;
    (env.comrade_profile == CONTROL_ENVELOPE_MARKER).then_some(env)
}

// ── Read / delivered receipts ─────────────────────────────────────────────────

/// The two acknowledgement levels, mirroring the sender-visible ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptKind {
    /// The message reached the peer's device and was decrypted + stored.
    Delivered,
    /// The peer opened the conversation and saw the message.
    Read,
}

impl ReceiptKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ReceiptKind::Delivered => "delivered",
            ReceiptKind::Read => "read",
        }
    }
}

/// A delivered/read acknowledgement covering one or more message ids, sent back
/// to the original sender over the encrypted DM channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// Format marker; must equal [`CONTROL_ENVELOPE_MARKER`].
    pub comrade_receipt: u8,
    pub status: ReceiptKind,
    /// Nostr event ids (hex) of the messages this receipt acknowledges.
    pub message_ids: Vec<String>,
}

impl Receipt {
    pub fn new(status: ReceiptKind, message_ids: Vec<String>) -> Self {
        Self {
            comrade_receipt: CONTROL_ENVELOPE_MARKER,
            status,
            message_ids,
        }
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Detect and parse a receipt envelope out of a decrypted DM body.
pub fn parse_receipt(content: &str) -> Option<Receipt> {
    let env: Receipt = serde_json::from_str(content).ok()?;
    (env.comrade_receipt == CONTROL_ENVELOPE_MARKER).then_some(env)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_share_round_trips() {
        let env = ProfileShare::new(Some("chandra_m".into()));
        let json = env.to_json().unwrap();
        assert_eq!(parse_profile_share(&json).unwrap(), env);
        // A handle-less identity still shares (username omitted).
        let anon = ProfileShare::new(None);
        let json = anon.to_json().unwrap();
        assert!(!json.contains("username"), "None handle is skipped: {json}");
        assert_eq!(parse_profile_share(&json).unwrap(), anon);
    }

    #[test]
    fn receipt_round_trips_both_kinds() {
        for status in [ReceiptKind::Delivered, ReceiptKind::Read] {
            let env = Receipt::new(status, vec!["abc".into(), "def".into()]);
            let json = env.to_json().unwrap();
            let back = parse_receipt(&json).unwrap();
            assert_eq!(back, env);
            assert_eq!(back.status, status);
            assert_eq!(back.message_ids, vec!["abc", "def"]);
        }
    }

    #[test]
    fn envelopes_are_mutually_exclusive_and_ignore_chat_text() {
        let profile = ProfileShare::new(Some("neo".into())).to_json().unwrap();
        let receipt = Receipt::new(ReceiptKind::Read, vec!["m1".into()])
            .to_json()
            .unwrap();

        // Each parser accepts only its own shape.
        assert!(parse_profile_share(&profile).is_some());
        assert!(parse_receipt(&profile).is_none());
        assert!(parse_receipt(&receipt).is_some());
        assert!(parse_profile_share(&receipt).is_none());

        // Neither is fooled by chat text, a call envelope, or a media envelope.
        assert!(parse_profile_share("just saying hi").is_none());
        assert!(parse_receipt("just saying hi").is_none());
        let call =
            r#"{"comrade_call":1,"call_id":"c","media":"audio","signal":{"kind":"ringing"}}"#;
        assert!(parse_profile_share(call).is_none());
        assert!(parse_receipt(call).is_none());
        let media =
            r#"{"comrade_media":1,"event_id":"e","url":"u","mime":"m","caption":"","size":1}"#;
        assert!(parse_profile_share(media).is_none());
        assert!(parse_receipt(media).is_none());
    }

    #[test]
    fn wrong_marker_value_is_rejected() {
        assert!(parse_profile_share(r#"{"comrade_profile":2,"username":"x"}"#).is_none());
        assert!(
            parse_receipt(r#"{"comrade_receipt":0,"status":"read","message_ids":[]}"#).is_none()
        );
    }
}
