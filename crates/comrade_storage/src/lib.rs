/*!
 * comrade_storage — Encrypted-at-rest local persistence
 *
 * Track 1: Local Encrypted Storage.
 *
 * A thin, privacy-first persistence layer so that identity keys, contacts,
 * cached messages and CRDT ledger snapshots survive app restarts — without
 * ever writing plaintext to disk.
 *
 * Design (envelope encryption / KEK pattern):
 *  • Embedded `sled` key-value store (pure Rust, no system dependencies).
 *  • Every stored *value* is sealed with AES-256-GCM (random 96-bit nonce per
 *    record, prepended to the ciphertext) under a random 32-byte **master
 *    key**. Relays/disk see only opaque bytes.
 *  • The master key itself is stored sealed under a **PIN key** derived from
 *    the user's PIN/password via Argon2id (memory-hard) over a per-store
 *    random salt. Keys live only in memory (zeroized on drop) — neither is
 *    ever written to disk in plaintext.
 *  • A verification token (a known magic value, sealed with the PIN key)
 *    lets `open` detect an incorrect PIN up front instead of failing later.
 *  • Changing the PIN re-seals only the master key — a single atomic sled
 *    batch on the meta tree — so an interrupted `change_pin` can never leave
 *    the store half re-encrypted (values are never rewritten).
 *
 * Stores created before the master-key record existed sealed values directly
 * under the PIN key; they open unchanged (the PIN key doubles as the master
 * key) and are upgraded to the envelope layout atomically on their next
 * `change_pin`.
 *
 * This composes standard, audited primitives (Argon2id + AES-256-GCM) in the
 * conventional envelope-encryption pattern — it does not invent new crypto.
 *
 * NOTE: logical *keys* within a tree (e.g. a contact's npub) are stored as-is
 * so the store remains queryable; only values are encrypted. Callers that need
 * key confidentiality should hash keys before insertion.
 */

mod error;
mod repository;

use std::path::Path;

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use argon2::Argon2;
use rand::RngCore;
use serde::{de::DeserializeOwned, Serialize};
use tracing::{debug, info};
use zeroize::Zeroizing;

pub use error::StorageError;
pub use repository::{
    Chitthi, ChitthiCache, Contact, LedgerState, StoredIdentity, StoredMessage, VaultCache,
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Reserved sled tree holding the KDF salt, PIN-verification token, and the
/// sealed master key.
const META_TREE: &str = "__comrade_meta__";
const SALT_KEY: &str = "argon2_salt";
const VERIFY_KEY: &str = "verify_token";
/// The random master key (which seals all values), itself sealed under the
/// PIN-derived key. Absent on legacy stores, whose values are sealed directly
/// under the PIN key.
const MASTER_KEY: &str = "master_key_sealed";
/// Plaintext sealed under the derived key to verify the PIN on reopen.
const VERIFY_MAGIC: &[u8] = b"comrade-storage-v1";

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

// ── Encrypted store ───────────────────────────────────────────────────────────

/// An encrypted-at-rest key-value store unlocked by a user PIN/password.
///
/// Cloning is intentionally not derived: the unlock key is sensitive and the
/// underlying `sled::Db` is already an `Arc`, so share an `EncryptedStore`
/// behind an `Arc` if it must cross threads.
pub struct EncryptedStore {
    db: sled::Db,
    /// 32-byte AES-256 **value key** (the unsealed master key, or on legacy
    /// stores the PIN-derived key). Zeroized on drop. Never persisted raw.
    key: Zeroizing<[u8; 32]>,
}

impl EncryptedStore {
    /// Open (or create) an encrypted store at `path`, unlocked with `pin`.
    ///
    /// On first use a random salt, verification token, and sealed master key
    /// are written. On subsequent opens the PIN is verified against that
    /// token; a wrong PIN returns [`StorageError::InvalidPin`] rather than
    /// silently corrupting data.
    pub fn open(path: impl AsRef<Path>, pin: &str) -> Result<Self, StorageError> {
        let db = sled::open(path)?;
        let meta = db.open_tree(META_TREE)?;

        // Load or create the Argon2 salt.
        let salt = match meta.get(SALT_KEY)? {
            Some(s) => s.to_vec(),
            None => {
                let mut s = vec![0u8; SALT_LEN];
                rand::thread_rng().fill_bytes(&mut s);
                meta.insert(SALT_KEY, s.clone())?;
                debug!("storage: generated new Argon2 salt");
                s
            }
        };

        let pin_key = derive_key(pin.as_bytes(), &salt)?;

        // Verify PIN, or write the verification token on first use.
        let brand_new = match meta.get(VERIFY_KEY)? {
            Some(token) => {
                let decrypted =
                    aes_decrypt(&pin_key, &token).map_err(|_| StorageError::InvalidPin)?;
                if decrypted != VERIFY_MAGIC {
                    return Err(StorageError::InvalidPin);
                }
                false
            }
            None => {
                let token = aes_encrypt(&pin_key, VERIFY_MAGIC)?;
                meta.insert(VERIFY_KEY, token)?;
                debug!("storage: wrote PIN verification token");
                true
            }
        };

        // Resolve the value key (envelope pattern): unseal the stored master
        // key; mint one for brand-new stores; fall back to the PIN key for
        // legacy stores (upgraded atomically on their next `change_pin`).
        let key = match meta.get(MASTER_KEY)? {
            Some(sealed) => {
                let raw = aes_decrypt(&pin_key, &sealed)
                    .map_err(|_| StorageError::Corrupt("master key unseal failed".into()))?;
                let arr: [u8; 32] = raw
                    .try_into()
                    .map_err(|_| StorageError::Corrupt("master key length".into()))?;
                Zeroizing::new(arr)
            }
            None if brand_new => {
                let mut master = Zeroizing::new([0u8; 32]);
                rand::thread_rng().fill_bytes(master.as_mut());
                meta.insert(MASTER_KEY, aes_encrypt(&pin_key, master.as_ref())?)?;
                debug!("storage: minted new sealed master key");
                master
            }
            // Legacy layout: values are sealed directly under the PIN key.
            None => pin_key,
        };

        db.flush()?;
        info!("storage: encrypted store unlocked");
        Ok(Self { db, key })
    }

    // ── Typed value access ───────────────────────────────────────────────────

    /// Serialize `value` to JSON, seal it, and store it under `tree`/`key`.
    pub fn put<T: Serialize>(&self, tree: &str, key: &str, value: &T) -> Result<(), StorageError> {
        let plaintext = serde_json::to_vec(value)?;
        self.put_bytes(tree, key, &plaintext)
    }

    /// Fetch and decrypt the value at `tree`/`key`, deserializing from JSON.
    pub fn get<T: DeserializeOwned>(
        &self,
        tree: &str,
        key: &str,
    ) -> Result<Option<T>, StorageError> {
        match self.get_bytes(tree, key)? {
            Some(plaintext) => Ok(Some(serde_json::from_slice(&plaintext)?)),
            None => Ok(None),
        }
    }

    /// Decrypt and deserialize every value in `tree`.
    pub fn values<T: DeserializeOwned>(&self, tree: &str) -> Result<Vec<T>, StorageError> {
        let t = self.db.open_tree(tree)?;
        let mut out = Vec::new();
        for item in t.iter() {
            let (_, sealed) = item?;
            let plaintext = self.unseal(&sealed)?;
            out.push(serde_json::from_slice(&plaintext)?);
        }
        Ok(out)
    }

    // ── Raw byte access (for CRDT snapshots etc.) ────────────────────────────

    /// Seal and store raw bytes (used for binary blobs like Yrs state diffs).
    pub fn put_bytes(&self, tree: &str, key: &str, value: &[u8]) -> Result<(), StorageError> {
        let sealed = self.seal(value)?;
        let t = self.db.open_tree(tree)?;
        t.insert(key.as_bytes(), sealed)?;
        Ok(())
    }

    /// Fetch and decrypt raw bytes at `tree`/`key`.
    pub fn get_bytes(&self, tree: &str, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let t = self.db.open_tree(tree)?;
        match t.get(key.as_bytes())? {
            Some(sealed) => Ok(Some(self.unseal(&sealed)?)),
            None => Ok(None),
        }
    }

    // ── Maintenance ──────────────────────────────────────────────────────────

    /// Whether `tree`/`key` exists.
    pub fn contains(&self, tree: &str, key: &str) -> Result<bool, StorageError> {
        let t = self.db.open_tree(tree)?;
        Ok(t.contains_key(key.as_bytes())?)
    }

    /// Delete `tree`/`key`. Returns `true` if a value was removed.
    pub fn delete(&self, tree: &str, key: &str) -> Result<bool, StorageError> {
        let t = self.db.open_tree(tree)?;
        Ok(t.remove(key.as_bytes())?.is_some())
    }

    /// List all logical keys in `tree` (keys are stored in plaintext).
    pub fn keys(&self, tree: &str) -> Result<Vec<String>, StorageError> {
        let t = self.db.open_tree(tree)?;
        let mut out = Vec::new();
        for item in t.iter() {
            let (k, _) = item?;
            let key = String::from_utf8(k.to_vec())
                .map_err(|e| StorageError::Corrupt(format!("non-utf8 key: {e}")))?;
            out.push(key);
        }
        Ok(out)
    }

    /// Flush all pending writes to disk.
    pub fn flush(&self) -> Result<(), StorageError> {
        self.db.flush()?;
        Ok(())
    }

    /// Re-key the store under a new PIN.
    ///
    /// Only the master key is re-sealed — values are never rewritten — and
    /// the salt, verification token, and sealed master key are replaced in a
    /// **single atomic sled batch**, so an interruption (crash, power loss)
    /// leaves the store fully openable with either the old or the new PIN,
    /// never half re-encrypted. Legacy stores (values sealed directly under
    /// the PIN key) are upgraded to the envelope layout by the same batch.
    /// After this returns, only `new_pin` unlocks the store.
    pub fn change_pin(&mut self, new_pin: &str) -> Result<(), StorageError> {
        // Derive the new PIN key over a fresh salt.
        let mut new_salt = vec![0u8; SALT_LEN];
        rand::thread_rng().fill_bytes(&mut new_salt);
        let new_pin_key = derive_key(new_pin.as_bytes(), &new_salt)?;

        // The current value key becomes (or remains) the master key, sealed
        // under the new PIN key. Values stay untouched.
        let mut batch = sled::Batch::default();
        batch.insert(SALT_KEY, new_salt);
        batch.insert(VERIFY_KEY, aes_encrypt(&new_pin_key, VERIFY_MAGIC)?);
        batch.insert(MASTER_KEY, aes_encrypt(&new_pin_key, self.key.as_ref())?);

        let meta = self.db.open_tree(META_TREE)?;
        meta.apply_batch(batch)?;
        self.db.flush()?;

        info!("storage: PIN changed (master key re-sealed atomically)");
        Ok(())
    }

    // ── Internal seal/unseal ─────────────────────────────────────────────────

    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
        aes_encrypt(&self.key, plaintext)
    }

    fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>, StorageError> {
        aes_decrypt(&self.key, sealed)
    }
}

// ── Key derivation & AEAD helpers ─────────────────────────────────────────────

/// Derive a 32-byte AES-256 key from a PIN/password via Argon2id.
fn derive_key(pin: &[u8], salt: &[u8]) -> Result<Zeroizing<[u8; 32]>, StorageError> {
    let argon2 = Argon2::default();
    let mut out = Zeroizing::new([0u8; 32]);
    argon2
        .hash_password_into(pin, salt, out.as_mut())
        .map_err(|e| StorageError::KeyDerivation(e.to_string()))?;
    Ok(out)
}

/// AES-256-GCM seal: output is `[nonce (12) | ciphertext+tag]`.
fn aes_encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let mut ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| StorageError::Encrypt)?;

    let mut out = nonce_bytes.to_vec();
    out.append(&mut ciphertext);
    Ok(out)
}

/// AES-256-GCM unseal of a `[nonce (12) | ciphertext+tag]` buffer.
fn aes_decrypt(key: &[u8; 32], nonce_then_ciphertext: &[u8]) -> Result<Vec<u8>, StorageError> {
    if nonce_then_ciphertext.len() <= NONCE_LEN {
        return Err(StorageError::Corrupt("sealed record too short".into()));
    }
    let (nonce_bytes, ciphertext) = nonce_then_ciphertext.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| StorageError::Decrypt)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_store(pin: &str) -> (TempDir, EncryptedStore) {
        let dir = TempDir::new().expect("tempdir");
        let store = EncryptedStore::open(dir.path(), pin).expect("open");
        (dir, store)
    }

    #[test]
    fn put_get_roundtrip() {
        let (_dir, store) = temp_store("1234");
        store.put("contacts", "alice", &"npub1alice").unwrap();
        let got: Option<String> = store.get("contacts", "alice").unwrap();
        assert_eq!(got.as_deref(), Some("npub1alice"));
    }

    #[test]
    fn missing_key_returns_none() {
        let (_dir, store) = temp_store("1234");
        let got: Option<String> = store.get("contacts", "nobody").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn data_persists_across_reopen() {
        let dir = TempDir::new().unwrap();
        {
            let store = EncryptedStore::open(dir.path(), "hunter2").unwrap();
            store.put("identity", "self", &"secret-value").unwrap();
            store.flush().unwrap();
        } // drop releases the sled lock
        let store = EncryptedStore::open(dir.path(), "hunter2").unwrap();
        let got: Option<String> = store.get("identity", "self").unwrap();
        assert_eq!(got.as_deref(), Some("secret-value"));
    }

    #[test]
    fn wrong_pin_is_rejected_on_reopen() {
        let dir = TempDir::new().unwrap();
        {
            let store = EncryptedStore::open(dir.path(), "correct-pin").unwrap();
            store.put("identity", "self", &"data").unwrap();
            store.flush().unwrap();
        }
        let result = EncryptedStore::open(dir.path(), "wrong-pin");
        assert!(matches!(result, Err(StorageError::InvalidPin)));
    }

    #[test]
    fn values_at_rest_are_not_plaintext() {
        let dir = TempDir::new().unwrap();
        let plaintext = "super-secret-nsec-xyz";
        {
            let store = EncryptedStore::open(dir.path(), "pin").unwrap();
            store.put("identity", "self", &plaintext).unwrap();
            store.flush().unwrap();
        }
        // Scan every file in the sled directory for the plaintext bytes.
        let mut found = false;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                if let Ok(bytes) = std::fs::read(&path) {
                    if bytes
                        .windows(plaintext.len())
                        .any(|w| w == plaintext.as_bytes())
                    {
                        found = true;
                    }
                }
            }
        }
        assert!(!found, "plaintext must never appear on disk");
    }

    #[test]
    fn delete_and_contains() {
        let (_dir, store) = temp_store("pin");
        store.put("contacts", "bob", &"x").unwrap();
        assert!(store.contains("contacts", "bob").unwrap());
        assert!(store.delete("contacts", "bob").unwrap());
        assert!(!store.contains("contacts", "bob").unwrap());
        assert!(!store.delete("contacts", "bob").unwrap());
    }

    #[test]
    fn list_keys_and_values() {
        let (_dir, store) = temp_store("pin");
        store.put("contacts", "a", &1u32).unwrap();
        store.put("contacts", "b", &2u32).unwrap();
        let mut keys = store.keys("contacts").unwrap();
        keys.sort();
        assert_eq!(keys, vec!["a".to_string(), "b".to_string()]);
        let mut vals: Vec<u32> = store.values("contacts").unwrap();
        vals.sort();
        assert_eq!(vals, vec![1, 2]);
    }

    #[test]
    fn raw_bytes_roundtrip() {
        let (_dir, store) = temp_store("pin");
        let blob = vec![0u8, 1, 2, 255, 254, 100];
        store.put_bytes("ledger", "snapshot", &blob).unwrap();
        let got = store.get_bytes("ledger", "snapshot").unwrap();
        assert_eq!(got, Some(blob));
    }

    #[test]
    fn change_pin_revokes_old_pin_without_rewriting_values() {
        let dir = TempDir::new().unwrap();
        {
            let mut store = EncryptedStore::open(dir.path(), "old-pin").unwrap();
            store.put("identity", "self", &"persistent-data").unwrap();
            store.change_pin("new-pin").unwrap();
            store.flush().unwrap();
            // New key works immediately for reads.
            let got: Option<String> = store.get("identity", "self").unwrap();
            assert_eq!(got.as_deref(), Some("persistent-data"));
        }
        // Old PIN no longer unlocks.
        assert!(matches!(
            EncryptedStore::open(dir.path(), "old-pin"),
            Err(StorageError::InvalidPin)
        ));
        // New PIN unlocks and data is intact.
        let store = EncryptedStore::open(dir.path(), "new-pin").unwrap();
        let got: Option<String> = store.get("identity", "self").unwrap();
        assert_eq!(got.as_deref(), Some("persistent-data"));
    }

    #[test]
    fn change_pin_chained_thrice_keeps_data() {
        let dir = TempDir::new().unwrap();
        {
            let mut store = EncryptedStore::open(dir.path(), "p0").unwrap();
            store.put("t", "k", &"v").unwrap();
            for pin in ["p1", "p2", "p3"] {
                store.change_pin(pin).unwrap();
            }
            store.flush().unwrap();
        }
        for stale in ["p0", "p1", "p2"] {
            assert!(matches!(
                EncryptedStore::open(dir.path(), stale),
                Err(StorageError::InvalidPin)
            ));
        }
        let store = EncryptedStore::open(dir.path(), "p3").unwrap();
        assert_eq!(store.get::<String>("t", "k").unwrap().as_deref(), Some("v"));
    }

    /// Stores written before the master-key record existed sealed values
    /// directly under the PIN key. They must open unchanged, and `change_pin`
    /// must upgrade them to the envelope layout without touching values.
    #[test]
    fn legacy_store_without_master_key_opens_and_upgrades() {
        let dir = TempDir::new().unwrap();

        // Hand-craft the legacy layout: salt + verify token + one value, all
        // under the PIN-derived key, with no master-key record.
        {
            let db = sled::open(dir.path()).unwrap();
            let meta = db.open_tree(META_TREE).unwrap();
            let mut salt = vec![0u8; SALT_LEN];
            rand::thread_rng().fill_bytes(&mut salt);
            let pin_key = derive_key(b"legacy-pin", &salt).unwrap();
            meta.insert(SALT_KEY, salt).unwrap();
            meta.insert(VERIFY_KEY, aes_encrypt(&pin_key, VERIFY_MAGIC).unwrap())
                .unwrap();
            let tree = db.open_tree("identity").unwrap();
            let plaintext = serde_json::to_vec(&"legacy-value").unwrap();
            tree.insert("self", aes_encrypt(&pin_key, &plaintext).unwrap())
                .unwrap();
            db.flush().unwrap();
        }

        // Opens with the legacy PIN; value readable.
        {
            let mut store = EncryptedStore::open(dir.path(), "legacy-pin").unwrap();
            let got: Option<String> = store.get("identity", "self").unwrap();
            assert_eq!(got.as_deref(), Some("legacy-value"));

            // Upgrade to envelope layout via change_pin.
            store.change_pin("fresh-pin").unwrap();
            store.flush().unwrap();
        }

        // Old PIN revoked; new PIN reads the (never rewritten) value.
        assert!(matches!(
            EncryptedStore::open(dir.path(), "legacy-pin"),
            Err(StorageError::InvalidPin)
        ));
        let store = EncryptedStore::open(dir.path(), "fresh-pin").unwrap();
        assert_eq!(
            store.get::<String>("identity", "self").unwrap().as_deref(),
            Some("legacy-value")
        );
    }
}
