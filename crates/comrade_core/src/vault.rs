/*!
 * Milestone 3b — Vault: End-to-End Encrypted Messaging Engine
 *
 * Implements:
 *  • NIP-04 (Kind 4) E2E direct-message pipeline
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

/// Extract the reply-target event id (hex) from an incoming DM's NIP-10 `e`
/// tags. Prefers an explicit `reply` marker, then `root`, then the last `e`
/// tag — mirroring the Sabha thread resolver so DM and feed replies agree.
///
/// Read from the event's canonical JSON so it is robust across nostr-sdk tag
/// representations (the same approach the Sabha thread parser uses).
fn reply_target(event: &Event) -> Option<String> {
    let val = serde_json::to_value(event).ok()?;
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

// ── Vault Engine ─────────────────────────────────────────────────────────────

/// Callback invoked for every successfully decrypted incoming DM. Used by the
/// IPC bridge to push `Kind::EncryptedDirectMessage` events across the webview /
/// JNI boundary as they arrive. Must be `Send + Sync` to live inside the Tokio
/// notification loop.
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

    /// Send an E2E encrypted direct message (NIP-04, Kind 4) to `recipient`.
    pub async fn send_dm(
        &self,
        recipient: &PublicKey,
        plaintext: &str,
    ) -> Result<EventId, VaultError> {
        self.send_dm_reply(recipient, plaintext, None).await
    }

    /// Send an E2E encrypted DM, optionally as a reply to a prior message.
    ///
    /// When `reply_to` is `Some(event_id_hex)`, a NIP-10 `["e", <id>, "",
    /// "reply"]` tag is attached so the recipient can render a quoted preview.
    /// The message body stays plain ciphertext — no envelope — so ordinary
    /// Nostr clients still show the text; only the thread link is metadata.
    pub async fn send_dm_reply(
        &self,
        recipient: &PublicKey,
        plaintext: &str,
        reply_to: Option<&str>,
    ) -> Result<EventId, VaultError> {
        let encrypted = nip04::encrypt(self.our_keys.secret_key(), recipient, plaintext.as_bytes())
            .map_err(|e| VaultError::EncryptionFailed(e.to_string()))?;

        // The single `p` tag stays the unambiguous NIP-04 recipient; a reply
        // adds one `e` tag. Junk reply ids are dropped rather than failing the
        // send — a broken quote link must never cost the user their message.
        let mut builder = EventBuilder::new(Kind::EncryptedDirectMessage, encrypted)
            .tag(Tag::public_key(*recipient));
        if let Some(id) = reply_to.filter(|s| !s.is_empty()) {
            match Tag::parse(["e", id, "", "reply"]) {
                Ok(tag) => builder = builder.tag(tag),
                Err(e) => warn!("dropping invalid reply tag {id}: {e}"),
            }
        }
        let event = builder
            .sign_with_keys(&self.our_keys)
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
        let filter = Filter::new()
            .kind(Kind::EncryptedDirectMessage)
            .pubkey(our_pk)
            .since(Timestamp::now());

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
                        if event.kind != Kind::EncryptedDirectMessage {
                            return Ok::<bool, Box<dyn std::error::Error>>(false);
                        }

                        let sender_pk = event.pubkey;
                        let decrypted = match nip04::decrypt(
                            our_keys.secret_key(),
                            &sender_pk,
                            event.content.clone(),
                        ) {
                            Ok(d) => d,
                            Err(e) => {
                                warn!(event_id = %event.id, "Vault: failed to decrypt DM: {e}");
                                return Ok::<bool, Box<dyn std::error::Error>>(false);
                            }
                        };

                        debug!(event_id = %event.id, sender = %sender_pk, "Vault DM decrypted");

                        let upi_intents = extract_upi_intents(&decrypted, &pay_regex);
                        if !upi_intents.is_empty() {
                            info!(
                                count = upi_intents.len(),
                                "Vault: UPI payment intents detected"
                            );
                        }

                        let reply_to = reply_target(&event);
                        let msg = VaultMessage {
                            event_id: event.id.to_hex(),
                            sender_pubkey: sender_pk.to_hex(),
                            content: decrypted,
                            created_at: event.created_at.as_secs(),
                            upi_intents,
                            reply_to,
                        };

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
}
