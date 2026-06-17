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
}

// ── Vault Engine ─────────────────────────────────────────────────────────────

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
        let encrypted = nip04::encrypt(self.our_keys.secret_key(), recipient, plaintext.as_bytes())
            .map_err(|e| VaultError::EncryptionFailed(e.to_string()))?;

        let event = EventBuilder::new(Kind::EncryptedDirectMessage, encrypted)
            .tag(Tag::public_key(*recipient))
            .sign_with_keys(&self.our_keys)
            .map_err(|e| VaultError::EncryptionFailed(e.to_string()))?;

        let output = self
            .client
            .send_event(&event)
            .await
            .map_err(|e| VaultError::EncryptionFailed(e.to_string()))?;

        info!(event_id = %output.id(), recipient = %recipient, "Vault DM sent");
        Ok(*output.id())
    }

    /// Subscribe to incoming DMs addressed to our public key.
    pub async fn subscribe_inbox(&self) -> Result<(), VaultError> {
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

        self.client
            .handle_notifications(move |notification| {
                let our_keys = our_keys.clone();
                let pay_regex = pay_regex.clone();
                let inbox = inbox.clone();

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

                        let msg = VaultMessage {
                            event_id: event.id.to_hex(),
                            sender_pubkey: sender_pk.to_hex(),
                            content: decrypted,
                            created_at: event.created_at.as_secs(),
                            upi_intents,
                        };

                        inbox.write().await.push(msg);
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
}
