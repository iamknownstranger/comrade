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
 *  • [`ReactionEnvelope`] — an **emoji reaction** to one earlier message. Sent
 *    this way rather than as a public NIP-25 Kind-7 event, which would announce
 *    to every relay that two keys are talking; see its own doc comment.
 *
 * Each envelope carries a distinct required marker field (`comrade_profile`,
 * `comrade_receipt`, `comrade_reaction`), so its `parse_*` function accepts only its own shape and
 * returns `None` for chat text, media/call/presence envelopes, or any other
 * JSON. That lets the runtime try each handler in turn and fall through to a
 * plain DM. The same pattern is used by [`crate::call::CallEnvelope`]
 * (`comrade_call`) and [`crate::presence::PresenceBeacon`]
 * (`comrade_presence`), which live next to their own protocols.
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

// ── Emoji reactions ───────────────────────────────────────────────────────────

/// Longest reaction body accepted, in bytes.
///
/// A reaction is one grapheme in intent, but a legitimate one is not one byte:
/// a family emoji with skin-tone modifiers and zero-width joiners runs past 30.
/// 64 leaves generous room for that while keeping the field from being a place
/// to put a paragraph — this string is attacker-controlled, arrives from anyone
/// with an accepted conversation, and gets rendered in a chip under a message
/// bubble. Over-long bodies are rejected at the parser rather than truncated:
/// truncating a ZWJ sequence yields a *different* emoji, and silently showing
/// someone a reaction they did not send is worse than showing none.
pub const MAX_REACTION_BYTES: usize = 64;

/// A reaction to one earlier message, riding the same sealed DM channel as the
/// message itself.
///
/// Deliberately **not** a public NIP-25 Kind-7 event. A kind-7 reaction is
/// published in the clear and tags both the event reacted to and its author, so
/// reacting to a gift-wrapped DM would announce to every relay that these two
/// keys are talking — exactly the metadata NIP-17 sealing exists to withhold.
/// Sent as a control envelope inside an ordinary sealed DM, a reaction inherits
/// the whole existing path: NIP-44 encryption, gift-wrap, mesh transport when
/// there is no relay, and the outbox retry.
///
/// One reaction per person per message, replacing rather than accumulating —
/// Telegram's model, and the one that makes the store key `(target, reactor)`.
/// An empty [`Self::emoji`] is the retraction: "I take mine back".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionEnvelope {
    /// Format marker; must equal [`CONTROL_ENVELOPE_MARKER`].
    pub comrade_reaction: u8,
    /// Nostr event id (hex) of the message being reacted to. Any event id will
    /// do — a text message or an attachment — for the same reason a reply tag
    /// does not care which: both are ordinary events.
    pub target_id: String,
    /// The emoji, or empty to withdraw a previous reaction.
    pub emoji: String,
}

impl ReactionEnvelope {
    pub fn new(target_id: impl Into<String>, emoji: impl Into<String>) -> Self {
        Self {
            comrade_reaction: CONTROL_ENVELOPE_MARKER,
            target_id: target_id.into(),
            emoji: emoji.into(),
        }
    }

    /// The retraction form: same target, no emoji.
    pub fn clearing(target_id: impl Into<String>) -> Self {
        Self::new(target_id, "")
    }

    /// Whether this withdraws a reaction rather than adding one.
    pub fn is_clearing(&self) -> bool {
        self.emoji.is_empty()
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Detect and parse a reaction envelope out of a decrypted DM body.
///
/// Rejects an empty `target_id` (a reaction to nothing is not renderable) and an
/// over-long `emoji` (see [`MAX_REACTION_BYTES`]). Both are returned as `None`
/// rather than an error, because the caller's next move either way is to fall
/// through to the other envelope parsers — a malformed reaction must never
/// surface as a chat bubble full of JSON.
pub fn parse_reaction(content: &str) -> Option<ReactionEnvelope> {
    let env: ReactionEnvelope = serde_json::from_str(content).ok()?;
    if env.comrade_reaction != CONTROL_ENVELOPE_MARKER {
        return None;
    }
    if env.target_id.is_empty() || env.emoji.len() > MAX_REACTION_BYTES {
        return None;
    }
    Some(env)
}

// ── Delete request ("delete for everyone") ───────────────────────────────────

/// A courtesy request, sent over the same sealed DM channel, asking the peer
/// to hide one earlier message on their side too.
///
/// **This is not a NIP-09 (kind-5) deletion request, and cannot be.** NIP-09
/// only works when the deletion is signed by the same key that authored the
/// event, but this app's DMs are NIP-17 gift wraps: `VaultEngine::send_dm_reply`
/// signs the *published* Kind-1059 event with a fresh, one-time key that is
/// generated and discarded inside that call (see its doc — the whole point of
/// NIP-59 sealing is keeping the real sender's key off the wrapper). By the
/// time a user asks to delete a message they sent, this device no longer
/// holds — and by design never retained — the key that would need to sign a
/// real retraction. There is nothing to check the nostr crate for here: it
/// implements NIP-09 correctly (`nostr::nips::nip09::EventDeletionRequest`),
/// the architecture this envelope rides on is what makes it inapplicable.
///
/// What this *is*: exactly the trust model WhatsApp's and Signal's "delete for
/// everyone" already run on — a protocol message the recipient's client is
/// expected to honour, not a cryptographic guarantee. A recipient on a client
/// that ignores this envelope, or one who already read and screenshotted the
/// message, is unaffected, same as those apps. Only the sender of the
/// original message may issue one — see `comrade_ui`'s
/// `RuntimeHandles::delete_message_for_everyone`, which enforces that before
/// this is ever built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteRequest {
    /// Format marker; must equal [`CONTROL_ENVELOPE_MARKER`].
    pub comrade_delete_request: u8,
    /// Nostr event id (hex) of the message to hide.
    pub target_id: String,
}

impl DeleteRequest {
    pub fn new(target_id: impl Into<String>) -> Self {
        Self {
            comrade_delete_request: CONTROL_ENVELOPE_MARKER,
            target_id: target_id.into(),
        }
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Detect and parse a delete-request envelope out of a decrypted DM body.
/// Rejects an empty `target_id` (nothing to hide) for the same reason
/// [`parse_reaction`] rejects one: a malformed envelope must fall through to
/// plain chat text, never surface as a bubble full of JSON.
pub fn parse_delete_request(content: &str) -> Option<DeleteRequest> {
    let env: DeleteRequest = serde_json::from_str(content).ok()?;
    if env.comrade_delete_request != CONTROL_ENVELOPE_MARKER || env.target_id.is_empty() {
        return None;
    }
    Some(env)
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
    fn reaction_round_trips_and_clears() {
        let env = ReactionEnvelope::new("abc123", "🔥");
        let json = env.to_json().unwrap();
        let back = parse_reaction(&json).unwrap();
        assert_eq!(back, env);
        assert_eq!(back.emoji, "🔥");
        assert!(!back.is_clearing());

        // The retraction is the same envelope with nothing in it, so a peer
        // needs no second message type to take a reaction back.
        let cleared = ReactionEnvelope::clearing("abc123");
        let back = parse_reaction(&cleared.to_json().unwrap()).unwrap();
        assert!(back.is_clearing());
        assert_eq!(back.target_id, "abc123");
    }

    #[test]
    fn reaction_survives_a_multi_codepoint_emoji() {
        // A skin-toned, zero-width-joined family is one reaction to a human and
        // 25 bytes to a parser. The byte cap must not be so tight that ordinary
        // emoji fail to arrive.
        let family = "👨‍👩‍👧‍👦";
        let thumb = "👍🏽";
        for e in [family, thumb] {
            let back = parse_reaction(&ReactionEnvelope::new("t", e).to_json().unwrap()).unwrap();
            assert_eq!(back.emoji, e, "{e} must round-trip intact");
            assert!(!back.is_clearing());
        }
    }

    #[test]
    fn an_oversized_or_targetless_reaction_is_refused() {
        // The emoji field is attacker-controlled and ends up rendered in a chip
        // under a bubble. Rejected, not truncated: cutting a ZWJ sequence short
        // produces a *different* emoji, and showing someone a reaction they did
        // not send is worse than showing none.
        let huge = ReactionEnvelope::new("t", "🔥".repeat(MAX_REACTION_BYTES))
            .to_json()
            .unwrap();
        assert!(parse_reaction(&huge).is_none());
        // A reaction to nothing cannot be drawn anywhere.
        let orphan = ReactionEnvelope::new("", "🔥").to_json().unwrap();
        assert!(parse_reaction(&orphan).is_none());
        // The boundary itself is allowed.
        let exact = ReactionEnvelope::new("t", "a".repeat(MAX_REACTION_BYTES))
            .to_json()
            .unwrap();
        assert!(parse_reaction(&exact).is_some());
    }

    #[test]
    fn envelopes_are_mutually_exclusive_and_ignore_chat_text() {
        let profile = ProfileShare::new(Some("neo".into())).to_json().unwrap();
        let receipt = Receipt::new(ReceiptKind::Read, vec!["m1".into()])
            .to_json()
            .unwrap();
        let reaction = ReactionEnvelope::new("m1", "🔥").to_json().unwrap();

        // Each parser accepts only its own shape.
        assert!(parse_profile_share(&profile).is_some());
        assert!(parse_receipt(&profile).is_none());
        assert!(parse_receipt(&receipt).is_some());
        assert!(parse_profile_share(&receipt).is_none());
        assert!(parse_reaction(&reaction).is_some());
        assert!(parse_reaction(&receipt).is_none());
        assert!(parse_reaction(&profile).is_none());
        assert!(parse_receipt(&reaction).is_none());
        assert!(parse_profile_share(&reaction).is_none());

        // Neither is fooled by chat text, a call envelope, or a media envelope.
        assert!(parse_profile_share("just saying hi").is_none());
        assert!(parse_receipt("just saying hi").is_none());
        assert!(parse_reaction("just saying hi").is_none());
        assert!(parse_reaction("🔥").is_none());
        let call =
            r#"{"comrade_call":1,"call_id":"c","media":"audio","signal":{"kind":"ringing"}}"#;
        assert!(parse_profile_share(call).is_none());
        assert!(parse_receipt(call).is_none());
        let media =
            r#"{"comrade_media":1,"event_id":"e","url":"u","mime":"m","caption":"","size":1}"#;
        assert!(parse_profile_share(media).is_none());
        assert!(parse_receipt(media).is_none());
        let presence = r#"{"comrade_presence":1,"state":"online","ttl_secs":480}"#;
        assert!(parse_profile_share(presence).is_none());
        assert!(parse_receipt(presence).is_none());
    }

    #[test]
    fn wrong_marker_value_is_rejected() {
        assert!(parse_profile_share(r#"{"comrade_profile":2,"username":"x"}"#).is_none());
        assert!(
            parse_receipt(r#"{"comrade_receipt":0,"status":"read","message_ids":[]}"#).is_none()
        );
        assert!(parse_reaction(r#"{"comrade_reaction":2,"target_id":"t","emoji":"🔥"}"#).is_none());
    }

    #[test]
    fn delete_request_round_trips() {
        let env = DeleteRequest::new("abc123");
        let json = env.to_json().unwrap();
        let back = parse_delete_request(&json).unwrap();
        assert_eq!(back, env);
        assert_eq!(back.target_id, "abc123");
    }

    #[test]
    fn a_targetless_delete_request_is_refused() {
        let orphan = DeleteRequest::new("").to_json().unwrap();
        assert!(parse_delete_request(&orphan).is_none());
    }

    #[test]
    fn delete_request_is_not_fooled_by_other_envelopes_or_chat_text() {
        let reaction = ReactionEnvelope::new("m1", "🔥").to_json().unwrap();
        let receipt = Receipt::new(ReceiptKind::Read, vec!["m1".into()])
            .to_json()
            .unwrap();
        assert!(parse_delete_request(&reaction).is_none());
        assert!(parse_delete_request(&receipt).is_none());
        assert!(parse_delete_request("just saying hi").is_none());
        assert!(parse_delete_request(r#"{"comrade_delete_request":2,"target_id":"t"}"#).is_none());
    }
}
