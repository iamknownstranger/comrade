/*!
 * Milestone 3b — Vault: End-to-End Encrypted Messaging Engine
 *
 * Implements:
 *  • NIP-44 encrypted rumors wrapped per NIP-17/NIP-59 ("Private Direct
 *    Message" / "Gift Wrap") for all new direct messages — content *and*
 *    metadata (real sender, exact timestamp) stay hidden from relays; only
 *    the outer wrapper's recipient `p` tag and a randomized timestamp are
 *    public. NIP-04 (Kind 4) decryption is kept, read-only, so DMs a peer
 *    sent before this upgrade still open (AUDIT.md task M1-1).
 *  • On-device regex processor that detects `/pay <amount> to <vpa>` patterns
 *    and compiles them to standard UPI payment string intents.
 */

use std::sync::Arc;

use nostr_sdk::nips::nip04;
use nostr_sdk::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::error::VaultError;
use crate::sabha::{wait_for_any_relay, CONNECT_WAIT};

// ── UPI payment intent ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, uniffi::Record)]
pub struct UpiPaymentIntent {
    pub amount_inr: f64,
    pub vpa: String,
    pub uri: String,
}

fn build_upi_uri(amount: f64, vpa: &str) -> String {
    format!("upi://pay?pa={vpa}&am={amount:.2}&cu=INR&mode=00")
}

pub fn extract_upi_intents(content: &str, re: &Regex) -> Vec<UpiPaymentIntent> {
    re.captures_iter(content)
        .filter_map(|caps| {
            let amount_str = caps.name("amount")?.as_str();
            let vpa = caps.name("vpa")?.as_str();
            let amount: f64 = amount_str.parse().ok()?;
            if amount <= 0.0 {
                return None;
            }
            Some(UpiPaymentIntent {
                amount_inr: amount,
                vpa: vpa.to_string(),
                uri: build_upi_uri(amount, vpa),
            })
        })
        .collect()
}

pub fn build_pay_regex() -> Result<Regex, VaultError> {
    Regex::new(
        r"(?i)/pay\s+(?P<amount>\d+(?:\.\d{1,2})?)\s+to\s+(?P<vpa>[a-zA-Z0-9.\-_]+@[a-zA-Z0-9]+)",
    )
    .map_err(|e| VaultError::UpiParseFailed(e.to_string()))
}

// ── Decrypted message envelope ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultMessage {
    pub event_id: String,
    pub sender_pubkey: String,
    pub content: String,
    pub created_at: u64,
    pub upi_intents: Vec<UpiPaymentIntent>,
    /// The event id (hex) this message replies to, from a NIP-10 `e` tag, if
    /// any. Lets the chat UI render a quoted "replying to…" preview.
    #[serde(default)]
    pub reply_to: Option<String>,
}

/// Extract the reply-target event id (hex) from a DM's NIP-10 `e` tags.
/// Prefers an explicit `reply` marker, then `root`, then the last `e` tag —
/// mirroring the Sabha thread resolver so DM and feed replies agree.
///
/// Generic over anything tag-bearing and `Serialize` so it works both for a
/// legacy signed [`Event`] (NIP-04) and an [`UnsignedEvent`] rumor
/// (NIP-44/NIP-17), whose canonical JSON shapes the tags identically. Reading
/// via JSON keeps this robust across nostr-sdk tag representations (the same
/// approach the Sabha thread parser uses).
fn reply_target(tagged: &impl Serialize) -> Option<String> {
    let val = serde_json::to_value(tagged).ok()?;
    let tags = val.get("tags")?.as_array()?;
    let mut e_tags: Vec<(String, String)> = Vec::new(); // (id, marker)
    for tag in tags {
        let arr = match tag.as_array() {
            Some(a) => a,
            None => continue,
        };
        if arr.first().and_then(|v| v.as_str()) != Some("e") {
            continue;
        }
        let id = arr
            .get(1)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let marker = arr
            .get(3)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !id.is_empty() {
            e_tags.push((id, marker));
        }
    }
    if let Some((id, _)) = e_tags.iter().find(|(_, m)| m == "reply") {
        return Some(id.clone());
    }
    if let Some((id, _)) = e_tags.iter().find(|(_, m)| m == "root") {
        return Some(id.clone());
    }
    e_tags.last().map(|(id, _)| id.clone())
}

/// Upper bound of NIP-59's random timestamp tweak applied to gift-wrap
/// events (`nip59::RANGE_RANDOM_TIMESTAMP_TWEAK` is 0..2 days) — a wrapper
/// sent this instant can carry a `created_at` up to this far in the past.
const GIFT_WRAP_TIMESTAMP_SKEW_SECS: u64 = 172_800;

/// Unwrap and decrypt a NIP-17 gift-wrapped (Kind-1059) DM, returning the
/// [`VaultMessage`] with `upi_intents` left empty (the caller fills it in —
/// extraction needs the shared regex, which this free function doesn't have).
/// Returns `None` (after logging) on any decrypt/verify failure — a bad or
/// foreign gift wrap must never crash the notification loop.
async fn decrypt_gift_wrapped_dm(our_keys: &Keys, event: &Event) -> Option<VaultMessage> {
    let unwrapped = match UnwrappedGift::from_gift_wrap(our_keys, event).await {
        Ok(u) => u,
        Err(e) => {
            warn!(event_id = %event.id, "Vault: failed to unwrap gift-wrapped DM: {e}");
            return None;
        }
    };
    if unwrapped.rumor.kind != Kind::PrivateDirectMessage {
        debug!(event_id = %event.id, kind = %unwrapped.rumor.kind, "Vault: ignoring non-DM rumor");
        return None;
    }
    debug!(event_id = %event.id, sender = %unwrapped.sender, "Vault DM (NIP-44) decrypted");
    Some(VaultMessage {
        event_id: event.id.to_hex(),
        sender_pubkey: unwrapped.sender.to_hex(),
        content: unwrapped.rumor.content.clone(),
        // The rumor's own timestamp is the true send time; the outer
        // wrapper's is deliberately randomized for privacy (see above).
        created_at: unwrapped.rumor.created_at.as_secs(),
        upi_intents: Vec::new(),
        reply_to: reply_target(&unwrapped.rumor),
    })
}

/// Decrypt a legacy NIP-04 (Kind-4) DM, kept read-only for backward
/// compatibility with peers who haven't sent a NIP-44 message yet (AUDIT.md
/// task M1-1). Returns `None` (after logging) on decrypt failure.
fn decrypt_legacy_nip04_dm(our_keys: &Keys, event: &Event) -> Option<VaultMessage> {
    let decrypted =
        match nip04::decrypt(our_keys.secret_key(), &event.pubkey, event.content.clone()) {
            Ok(d) => d,
            Err(e) => {
                warn!(event_id = %event.id, "Vault: failed to decrypt legacy NIP-04 DM: {e}");
                return None;
            }
        };
    debug!(event_id = %event.id, sender = %event.pubkey, "Vault DM (legacy NIP-04) decrypted");
    Some(VaultMessage {
        event_id: event.id.to_hex(),
        sender_pubkey: event.pubkey.to_hex(),
        content: decrypted,
        created_at: event.created_at.as_secs(),
        upi_intents: Vec::new(),
        reply_to: reply_target(event),
    })
}

// ── Vault Engine ─────────────────────────────────────────────────────────────

/// Callback invoked for every successfully decrypted incoming DM. Used by the
/// IPC bridge to push decoded DM events across the webview / JNI boundary as
/// they arrive. Must be `Send + Sync` to live inside the Tokio notification
/// loop.
pub type VaultCallback = Box<dyn Fn(VaultMessage) + Send + Sync + 'static>;

pub struct VaultEngine {
    client: Client,
    our_keys: Keys,
    pay_regex: Arc<Regex>,
    inbox: Arc<RwLock<Vec<VaultMessage>>>,
}

impl VaultEngine {
    pub async fn new(keys: &Keys, relay_urls: Vec<String>) -> Result<Self, VaultError> {
        let client = Client::new(keys.clone());
        for url in &relay_urls {
            client
                .add_relay(url.as_str())
                .await
                .map_err(|e| VaultError::EncryptionFailed(e.to_string()))?;
        }
        let pay_regex = Arc::new(build_pay_regex()?);
        Ok(Self {
            client,
            our_keys: keys.clone(),
            pay_regex,
            inbox: Arc::new(RwLock::new(Vec::new())),
        })
    }

    pub async fn connect(&self) {
        self.client.connect().await;
        info!("Vault engine connected");
    }

    pub async fn disconnect(&self) {
        self.client.disconnect().await;
    }

    /// Send an E2E encrypted direct message (NIP-44, gift-wrapped per NIP-17)
    /// to `recipient`.
    pub async fn send_dm(
        &self,
        recipient: &PublicKey,
        plaintext: &str,
    ) -> Result<EventId, VaultError> {
        self.send_dm_reply(recipient, plaintext, None).await
    }

    /// Send an E2E encrypted DM, optionally as a reply to a prior message.
    ///
    /// The message is a NIP-17 "Private Direct Message": a Kind-14 rumor
    /// (never signed or sent on its own) is NIP-44-encrypted into a Kind-13
    /// seal, which is itself NIP-44-encrypted and signed by a fresh, one-time
    /// key into the Kind-1059 event actually published (NIP-59 gift wrap).
    /// The only metadata a relay observer learns is the recipient's `p` tag
    /// and a timestamp randomized by up to two days — the real sender and
    /// send time live inside the encrypted rumor.
    ///
    /// When `reply_to` is `Some(event_id_hex)`, a NIP-10 `["e", <id>, "",
    /// "reply"]` tag is attached to the *rumor* (so it travels encrypted,
    /// same as the message body) so the recipient can render a quoted
    /// preview. Junk reply ids are dropped rather than failing the send — a
    /// broken quote link must never cost the user their message.
    pub async fn send_dm_reply(
        &self,
        recipient: &PublicKey,
        plaintext: &str,
        reply_to: Option<&str>,
    ) -> Result<EventId, VaultError> {
        let mut rumor_tags = Vec::new();
        if let Some(id) = reply_to.filter(|s| !s.is_empty()) {
            match Tag::parse(["e", id, "", "reply"]) {
                Ok(tag) => rumor_tags.push(tag),
                Err(e) => warn!("dropping invalid reply tag {id}: {e}"),
            }
        }
        let event = EventBuilder::private_msg(&self.our_keys, *recipient, plaintext, rumor_tags)
            .await
            .map_err(|e| VaultError::EncryptionFailed(e.to_string()))?;

        // A send right after unlock races the relay dials: the pool "succeeds"
        // against zero relays and the message is silently lost while the UI
        // marks it sent. Wait for one live relay and require an acceptance so
        // the failure is loud and the user can retry.
        wait_for_any_relay(&self.client, CONNECT_WAIT).await;
        let output = self
            .client
            .send_event(&event)
            .await
            .map_err(|e| VaultError::EncryptionFailed(e.to_string()))?;
        if output.success.is_empty() {
            return Err(VaultError::EncryptionFailed(
                "no relay accepted the message — check your connection and try again".into(),
            ));
        }

        info!(event_id = %output.id(), recipient = %recipient, "Vault DM sent");
        Ok(*output.id())
    }

    /// Subscribe to incoming DMs addressed to our public key, caching each
    /// decrypted message in the inbox snapshot.
    pub async fn subscribe_inbox(&self) -> Result<(), VaultError> {
        self.subscribe_inbox_with_callback(Box::new(|_| {})).await
    }

    /// Subscribe to incoming DMs, invoking `callback` for every decrypted
    /// message *in addition to* caching it in the inbox snapshot.
    ///
    /// The IPC bridge passes a callback here that forwards each `VaultMessage`
    /// onto the event bus so the desktop webview / Android layer can render new
    /// encrypted DMs the instant they land — all inside the background Tokio
    /// task, leaving the UI thread free.
    pub async fn subscribe_inbox_with_callback(
        &self,
        callback: VaultCallback,
    ) -> Result<(), VaultError> {
        let our_pk = self.our_keys.public_key();
        // Gift-wrapped (NIP-17/59) events carry a randomized `created_at` up
        // to `GIFT_WRAP_TIMESTAMP_SKEW` in the past (see `send_dm_reply`), so
        // a naive `since(now())` would drop messages sent this instant. Widen
        // the window by the same amount; legacy Kind-4 DMs are unaffected —
        // they just get a slightly wider (harmless) backfill too.
        let since = Timestamp::now() - GIFT_WRAP_TIMESTAMP_SKEW_SECS;
        let filter = Filter::new()
            .kinds([Kind::GiftWrap, Kind::EncryptedDirectMessage])
            .pubkey(our_pk)
            .since(since);

        self.client
            .subscribe(filter, None)
            .await
            .map_err(|e| VaultError::EncryptionFailed(e.to_string()))?;

        info!("Vault inbox subscription active");

        let our_keys = self.our_keys.clone();
        let pay_regex = self.pay_regex.clone();
        let inbox = self.inbox.clone();
        let callback = Arc::new(callback);

        self.client
            .handle_notifications(move |notification| {
                let our_keys = our_keys.clone();
                let pay_regex = pay_regex.clone();
                let inbox = inbox.clone();
                let callback = callback.clone();

                async move {
                    if let RelayPoolNotification::Event { event, .. } = notification {
                        let msg = match event.kind {
                            Kind::GiftWrap => {
                                match decrypt_gift_wrapped_dm(&our_keys, &event).await {
                                    Some(m) => m,
                                    None => return Ok::<bool, Box<dyn std::error::Error>>(false),
                                }
                            }
                            Kind::EncryptedDirectMessage => {
                                match decrypt_legacy_nip04_dm(&our_keys, &event) {
                                    Some(m) => m,
                                    None => return Ok::<bool, Box<dyn std::error::Error>>(false),
                                }
                            }
                            _ => return Ok::<bool, Box<dyn std::error::Error>>(false),
                        };

                        let upi_intents = extract_upi_intents(&msg.content, &pay_regex);
                        if !upi_intents.is_empty() {
                            info!(
                                count = upi_intents.len(),
                                "Vault: UPI payment intents detected"
                            );
                        }
                        let msg = VaultMessage { upi_intents, ..msg };

                        inbox.write().await.push(msg.clone());
                        callback(msg);
                    }
                    Ok::<bool, Box<dyn std::error::Error>>(false)
                }
            })
            .await
            .map_err(|e| VaultError::EncryptionFailed(e.to_string()))
    }

    pub async fn inbox_snapshot(&self) -> Vec<VaultMessage> {
        self.inbox.read().await.clone()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn regex() -> Regex {
        build_pay_regex().unwrap()
    }

    #[test]
    fn detects_pay_command() {
        let re = regex();
        let result = extract_upi_intents("/pay 500 to user@upi", &re);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].amount_inr, 500.0);
        assert_eq!(result[0].vpa, "user@upi");
        assert!(result[0].uri.contains("upi://pay"));
        assert!(result[0].uri.contains("pa=user@upi"));
        assert!(result[0].uri.contains("am=500.00"));
    }

    #[test]
    fn detects_decimal_amount() {
        let re = regex();
        let result = extract_upi_intents("/PAY 12.50 to merchant@okaxis", &re);
        assert_eq!(result.len(), 1);
        assert!((result[0].amount_inr - 12.5).abs() < f64::EPSILON);
    }

    #[test]
    fn no_false_positive_on_plain_text() {
        let re = regex();
        let result = extract_upi_intents("just a normal message", &re);
        assert!(result.is_empty());
    }

    #[test]
    fn multiple_pay_commands_in_one_message() {
        let re = regex();
        let msg = "/pay 100 to a@b  and also /pay 200 to c@d";
        let result = extract_upi_intents(msg, &re);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn zero_amount_is_rejected() {
        let re = regex();
        let result = extract_upi_intents("/pay 0 to user@upi", &re);
        assert!(result.is_empty());
    }

    #[test]
    fn upi_uri_format_is_correct() {
        let re = regex();
        let result = extract_upi_intents("/pay 999 to test@paytm", &re);
        let uri = &result[0].uri;
        assert!(uri.starts_with("upi://pay?"), "scheme present: {uri}");
        assert!(uri.contains("cu=INR"), "currency present: {uri}");
    }

    #[test]
    fn nip04_roundtrip_encrypts_and_decrypts() {
        // The legacy primitive itself, kept alive read-only for M1-1
        // backward compat — exercised end-to-end below via
        // `decrypt_legacy_nip04_dm`.
        let alice = Keys::generate();
        let bob = Keys::generate();
        let plaintext = "Hello from Alice to Bob";

        let encrypted = nip04::encrypt(alice.secret_key(), &bob.public_key(), plaintext.as_bytes())
            .expect("encrypt");

        let decrypted =
            nip04::decrypt(bob.secret_key(), &alice.public_key(), encrypted).expect("decrypt");

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn legacy_nip04_dm_decrypts_with_sender_and_content() {
        let alice = Keys::generate();
        let bob = Keys::generate();

        let encrypted =
            nip04::encrypt(alice.secret_key(), &bob.public_key(), b"legacy hello").unwrap();
        let event = EventBuilder::new(Kind::EncryptedDirectMessage, encrypted)
            .tag(Tag::public_key(bob.public_key()))
            .sign_with_keys(&alice)
            .unwrap();

        let msg = decrypt_legacy_nip04_dm(&bob, &event).expect("decrypts");
        assert_eq!(msg.content, "legacy hello");
        assert_eq!(msg.sender_pubkey, alice.public_key().to_hex());
        assert_eq!(msg.event_id, event.id.to_hex());
    }

    #[test]
    fn reply_target_reads_the_nip10_e_tag() {
        let keys = Keys::generate();
        let parent = "b".repeat(64);

        // A DM carrying a reply e-tag exposes the parent id.
        let reply = EventBuilder::new(Kind::EncryptedDirectMessage, "ciphertext")
            .tag(Tag::public_key(keys.public_key()))
            .tag(Tag::parse(["e", parent.as_str(), "", "reply"]).unwrap())
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(reply_target(&reply).as_deref(), Some(parent.as_str()));

        // A DM with no e-tag has no reply target.
        let plain = EventBuilder::new(Kind::EncryptedDirectMessage, "ciphertext")
            .tag(Tag::public_key(keys.public_key()))
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(reply_target(&plain), None);
    }

    #[test]
    fn reply_target_also_reads_the_e_tag_from_an_unsigned_rumor() {
        // NIP-44/NIP-17 replies carry the e-tag on the *rumor*, not the outer
        // gift wrap — `reply_target` must work on an `UnsignedEvent` too.
        let keys = Keys::generate();
        let parent = "c".repeat(64);
        let rumor = EventBuilder::private_msg_rumor(keys.public_key(), "hi")
            .tag(Tag::parse(["e", parent.as_str(), "", "reply"]).unwrap())
            .build(keys.public_key());
        assert_eq!(reply_target(&rumor).as_deref(), Some(parent.as_str()));
    }

    #[tokio::test]
    async fn gift_wrapped_dm_roundtrips_content_sender_and_reply_tag() {
        let alice = Keys::generate();
        let bob = Keys::generate();
        let parent = "a".repeat(64);
        let reply_tag = Tag::parse(["e", parent.as_str(), "", "reply"]).unwrap();

        let wrapped = EventBuilder::private_msg(&alice, bob.public_key(), "hi bob", [reply_tag])
            .await
            .expect("gift wrap");

        // The outer wrapper leaks nothing about Alice: it's signed by a
        // one-time key, not hers.
        assert_ne!(wrapped.pubkey, alice.public_key());
        assert_eq!(wrapped.kind, Kind::GiftWrap);

        let msg = decrypt_gift_wrapped_dm(&bob, &wrapped)
            .await
            .expect("bob can unwrap and decrypt");
        assert_eq!(msg.content, "hi bob");
        assert_eq!(msg.sender_pubkey, alice.public_key().to_hex());
        assert_eq!(msg.event_id, wrapped.id.to_hex());
        assert_eq!(msg.reply_to.as_deref(), Some(parent.as_str()));
    }

    #[tokio::test]
    async fn gift_wrapped_dm_meant_for_someone_else_does_not_decrypt() {
        let alice = Keys::generate();
        let bob = Keys::generate();
        let eve = Keys::generate();

        let wrapped = EventBuilder::private_msg(&alice, bob.public_key(), "secret", [])
            .await
            .unwrap();

        assert!(decrypt_gift_wrapped_dm(&eve, &wrapped).await.is_none());
    }
}
