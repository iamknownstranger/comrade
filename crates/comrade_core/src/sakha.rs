/*!
 * Milestone 5 — Sakha/Sakhi: Shared CRDT Ledger ("Hisab-Kitab")
 *
 * Provides a cryptographically isolated shared workspace for a paired couple.
 * Key properties:
 *  • Client-side CRDT (Yrs / Yjs) for the shared "Hisab-Kitab" transaction log
 *  • DH shared secret (from `crypto`) as the AES-256-GCM key
 *  • Ledger updates are serialised as binary Yrs state diffs, encrypted, and
 *    published to Nostr as Kind-30078 — relays see only opaque ciphertext
 *  • On receipt the peer decrypts and applies the Yrs update (CRDT merge)
 */

use std::sync::Arc;

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use nostr_sdk::prelude::*;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use yrs::{updates::decoder::Decode, Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

use crate::{
    crypto::{compute_dh_shared_secret, derive_symmetric_key},
    error::SakhaError,
};

// ── Custom Nostr event kind ──────────────────────────────────────────────────

const LEDGER_SYNC_KIND: u16 = 30078;

// ── AES-256-GCM helpers ──────────────────────────────────────────────────────

fn aes_encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, SakhaError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let mut ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| SakhaError::EncryptionError(e.to_string()))?;

    let mut out = nonce_bytes.to_vec();
    out.append(&mut ciphertext);
    Ok(out)
}

fn aes_decrypt(key: &[u8; 32], nonce_then_ciphertext: &[u8]) -> Result<Vec<u8>, SakhaError> {
    const NONCE_LEN: usize = 12;
    if nonce_then_ciphertext.len() <= NONCE_LEN {
        return Err(SakhaError::SyncDecodeFailed("ciphertext too short".into()));
    }
    let (nonce_bytes, ciphertext) = nonce_then_ciphertext.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| SakhaError::EncryptionError(e.to_string()))
}

// ── Ledger entry ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub description: String,
    pub amount_inr: f64,
    pub paid_by: String,
    pub timestamp: u64,
}

impl LedgerEntry {
    pub fn new(
        description: impl Into<String>,
        amount_inr: f64,
        paid_by: impl Into<String>,
    ) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            description: description.into(),
            amount_inr,
            paid_by: paid_by.into(),
            timestamp,
        }
    }

    fn to_line(&self) -> String {
        format!(
            "[{}] {} | ₹{:.2} | paid by {}",
            self.timestamp, self.description, self.amount_inr, self.paid_by
        )
    }
}

// ── Sakha engine ─────────────────────────────────────────────────────────────

pub struct SakhaEngine {
    our_keys: Keys,
    partner_pk: Option<PublicKey>,
    symmetric_key: Option<[u8; 32]>,
    pub doc: Arc<RwLock<Doc>>,
    client: Client,
}

impl SakhaEngine {
    pub async fn new(our_keys: &Keys, relay_urls: Vec<String>) -> Result<Self, SakhaError> {
        let client = Client::new(our_keys.clone());
        for url in &relay_urls {
            client
                .add_relay(url.as_str())
                .await
                .map_err(|e| SakhaError::RelayError(e.to_string()))?;
        }
        Ok(Self {
            our_keys: our_keys.clone(),
            partner_pk: None,
            symmetric_key: None,
            doc: Arc::new(RwLock::new(Doc::new())),
            client,
        })
    }

    pub async fn connect(&self) {
        self.client.connect().await;
        info!("Sakha engine connected");
    }

    pub fn pair_with(&mut self, partner_pk: PublicKey) -> Result<(), SakhaError> {
        let shared_secret = compute_dh_shared_secret(self.our_keys.secret_key(), &partner_pk)
            .map_err(|e| SakhaError::EncryptionError(e.to_string()))?;

        self.symmetric_key = Some(derive_symmetric_key(&shared_secret, "sakha-hisab-kitab-v1"));
        self.partner_pk = Some(partner_pk);
        info!("Sakha pairing handshake complete");
        Ok(())
    }

    pub async fn add_entry(&self, entry: LedgerEntry) -> Result<(), SakhaError> {
        let line = entry.to_line();
        let doc = self.doc.write().await;
        let text = doc.get_or_insert_text("hisab_kitab");
        let mut txn = doc.transact_mut();
        let current = text.get_string(&txn);
        let pos = current.len() as u32;
        if pos > 0 {
            text.insert(&mut txn, pos, "\n");
        }
        let new_pos = text.get_string(&txn).len() as u32;
        text.insert(&mut txn, new_pos, &line);
        drop(txn);
        debug!(entry = %line, "Sakha ledger entry appended");
        Ok(())
    }

    pub async fn read_ledger(&self) -> String {
        let doc = self.doc.read().await;
        let text = doc.get_or_insert_text("hisab_kitab");
        let txn = doc.transact();
        text.get_string(&txn)
    }

    /// Encode current Yrs state diff, encrypt with AES-GCM, and publish as
    /// a base64-encoded Kind-30078 Nostr event. Relays see only opaque bytes.
    pub async fn publish_sync(&self) -> Result<EventId, SakhaError> {
        let key = self.symmetric_key.ok_or(SakhaError::NoSharedSecret)?;

        let update_bytes = {
            let doc = self.doc.read().await;
            let txn = doc.transact();
            txn.encode_diff_v1(&StateVector::default())
        };

        let ciphertext = aes_encrypt(&key, &update_bytes)?;
        let content_b64 = base64_encode(&ciphertext);

        let event = EventBuilder::new(Kind::Custom(LEDGER_SYNC_KIND), content_b64)
            .sign_with_keys(&self.our_keys)
            .map_err(|e| SakhaError::RelayError(e.to_string()))?;

        let output = self
            .client
            .send_event(&event)
            .await
            .map_err(|e| SakhaError::RelayError(e.to_string()))?;

        info!(event_id = %output.id(), "Sakha sync update published");
        Ok(*output.id())
    }

    /// Subscribe to incoming sync events from the partner and CRDT-merge them.
    pub async fn subscribe_sync(&self) -> Result<(), SakhaError> {
        let partner_pk = self.partner_pk.ok_or(SakhaError::NoSharedSecret)?;
        let key = self.symmetric_key.ok_or(SakhaError::NoSharedSecret)?;

        let filter = Filter::new()
            .kind(Kind::Custom(LEDGER_SYNC_KIND))
            .author(partner_pk)
            .since(Timestamp::now());

        self.client
            .subscribe(filter, None)
            .await
            .map_err(|e| SakhaError::RelayError(e.to_string()))?;

        info!("Sakha sync subscription active");

        let doc = self.doc.clone();

        self.client
            .handle_notifications(move |notification| {
                let doc = doc.clone();
                async move {
                    if let RelayPoolNotification::Event { event, .. } = notification {
                        if event.kind != Kind::Custom(LEDGER_SYNC_KIND) {
                            return Ok::<bool, Box<dyn std::error::Error>>(false);
                        }

                        let ciphertext = match base64_decode(&event.content) {
                            Ok(b) => b,
                            Err(e) => {
                                warn!(event_id = %event.id, "Sakha: base64 decode failed: {e}");
                                return Ok::<bool, Box<dyn std::error::Error>>(false);
                            }
                        };

                        let plaintext = match aes_decrypt(&key, &ciphertext) {
                            Ok(p) => p,
                            Err(e) => {
                                warn!(event_id = %event.id, "Sakha: decrypt failed: {e}");
                                return Ok::<bool, Box<dyn std::error::Error>>(false);
                            }
                        };

                        let update = match Update::decode_v1(&plaintext) {
                            Ok(u) => u,
                            Err(e) => {
                                warn!(event_id = %event.id, "Sakha: Yrs decode failed: {e}");
                                return Ok::<bool, Box<dyn std::error::Error>>(false);
                            }
                        };

                        let doc_guard = doc.write().await;
                        let mut txn = doc_guard.transact_mut();
                        if let Err(e) = txn.apply_update(update) {
                            warn!(event_id = %event.id, "Sakha: Yrs apply failed: {e}");
                        } else {
                            debug!(event_id = %event.id, "Sakha: CRDT sync applied");
                        }
                    }
                    Ok::<bool, Box<dyn std::error::Error>>(false)
                }
            })
            .await
            .map_err(|e| SakhaError::RelayError(e.to_string()))
    }

    pub async fn disconnect(&self) {
        self.client.disconnect().await;
    }

    pub fn is_paired(&self) -> bool {
        self.symmetric_key.is_some()
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, SakhaError> {
    B64.decode(s.trim())
        .map_err(|e| SakhaError::SyncDecodeFailed(format!("base64: {e}")))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::KeyProfile;

    #[test]
    fn base64_roundtrip() {
        let data = b"Hello Comrade \x00\xFF\xAB";
        let enc = base64_encode(data);
        let dec = base64_decode(&enc).unwrap();
        assert_eq!(data.as_slice(), dec.as_slice());
    }

    #[test]
    fn aes_roundtrip() {
        let key = [0x42u8; 32];
        let plaintext = b"hisab-kitab entry: Rs 500 dinner";
        let ct = aes_encrypt(&key, plaintext).unwrap();
        let pt = aes_decrypt(&key, &ct).unwrap();
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn aes_wrong_key_fails() {
        let key1 = [0x01u8; 32];
        let key2 = [0x02u8; 32];
        let ct = aes_encrypt(&key1, b"secret").unwrap();
        assert!(aes_decrypt(&key2, &ct).is_err());
    }

    #[tokio::test]
    async fn ledger_append_and_read() {
        let keys = Keys::generate();
        let engine = SakhaEngine::new(&keys, vec![]).await.unwrap();
        engine
            .add_entry(LedgerEntry::new("Coffee", 150.0, "Sakha"))
            .await
            .unwrap();
        let ledger = engine.read_ledger().await;
        assert!(ledger.contains("Coffee"), "entry must appear: {ledger}");
        assert!(ledger.contains("150.00"), "amount must appear: {ledger}");
    }

    #[tokio::test]
    async fn paired_keys_crdt_sync_roundtrip() {
        let alice = KeyProfile::generate().unwrap();
        let bob = KeyProfile::generate().unwrap();

        let mut engine_alice = SakhaEngine::new(&alice.keys, vec![]).await.unwrap();
        engine_alice.pair_with(bob.public_key()).unwrap();

        engine_alice
            .add_entry(LedgerEntry::new("Groceries", 300.0, "Sakhi"))
            .await
            .unwrap();

        // Encode and encrypt the state diff
        let key = engine_alice.symmetric_key.unwrap();
        let update_bytes = {
            let doc = engine_alice.doc.read().await;
            let txn = doc.transact();
            txn.encode_diff_v1(&StateVector::default())
        };
        let ciphertext = aes_encrypt(&key, &update_bytes).unwrap();

        // Bob decrypts and applies
        let mut engine_bob = SakhaEngine::new(&bob.keys, vec![]).await.unwrap();
        engine_bob.pair_with(alice.public_key()).unwrap();
        let bob_key = engine_bob.symmetric_key.unwrap();

        let plaintext = aes_decrypt(&bob_key, &ciphertext).unwrap();
        let update = Update::decode_v1(&plaintext).unwrap();

        let doc_guard = engine_bob.doc.write().await;
        let mut txn = doc_guard.transact_mut();
        txn.apply_update(update).unwrap();
        drop(txn);

        let text = doc_guard.get_or_insert_text("hisab_kitab");
        let read_tx = doc_guard.transact();
        let content = text.get_string(&read_tx);
        assert!(
            content.contains("Groceries"),
            "CRDT merge must propagate entry: {content}"
        );
    }
}
