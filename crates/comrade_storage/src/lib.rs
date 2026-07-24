/*!
 * comrade_storage — Encrypted-at-rest local persistence
 *
 * Track 1: Local Encrypted Storage.
 *
 * A thin, privacy-first persistence layer so that identity keys, contacts,
 * cached messages and CRDT ledger snapshots survive app restarts — without
 * ever writing plaintext to disk.
 *
 * Design:
 *  • Embedded `redb` key-value store (pure Rust, no system dependencies,
 *    single-file, ACID transactions — see the decision log in AUDIT.md for
 *    why this replaced `sled`).
 *  • Every stored *value* is sealed with AES-256-GCM (random 96-bit nonce per
 *    record, prepended to the ciphertext). Relays/disk see only opaque bytes.
 *  • The AES key is derived at runtime from a user PIN/password via Argon2id
 *    (memory-hard) over a per-store random salt. The key lives only in memory
 *    and is zeroized on drop. It is never written to disk.
 *  • A verification token (a known magic value, sealed with the derived key)
 *    lets `open` detect an incorrect PIN up front instead of failing later.
 *
 * This composes standard, audited primitives (Argon2id + AES-256-GCM) in the
 * conventional envelope-encryption pattern — it does not invent new crypto.
 *
 * `open` transparently migrates a pre-existing `sled` store the first time it
 * is opened after upgrading (see [`migrate`]); every caller keeps using the
 * same directory path it always has.
 *
 * NOTE: logical *keys* within a tree (e.g. a contact's npub) are stored as-is
 * so the store remains queryable; only values are encrypted. Callers that need
 * key confidentiality should hash keys before insertion.
 */

mod error;
mod migrate;
mod repository;

use std::fs;
use std::path::Path;

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use argon2::Argon2;
use rand::RngCore;
use redb::{ReadableDatabase, ReadableTable, TableDefinition, TableHandle};
use serde::{de::DeserializeOwned, Serialize};
use tracing::{debug, info};
use zeroize::Zeroizing;

pub use error::StorageError;
pub use repository::{
    CallRecord, Chitthi, ChitthiCache, Contact, ConversationMeta, JournalEntry, LedgerState,
    StoredIdentity, StoredMessage, TaraMessage, VaultCache,
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// The single on-disk redb file, created inside the directory callers pass to
/// [`EncryptedStore::open`] (kept directory-shaped for compatibility with
/// every existing caller, which historically pointed at a `sled` directory).
const REDB_FILE_NAME: &str = "comrade.redb";

/// Reserved redb table holding the KDF salt and PIN-verification token.
const META_TREE: &str = "__comrade_meta__";
const SALT_KEY: &str = "argon2_salt";
const VERIFY_KEY: &str = "verify_token";
/// Plaintext sealed under the derived key to verify the PIN on reopen.
const VERIFY_MAGIC: &[u8] = b"comrade-storage-v1";

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

/// Build a redb table definition for a dynamically-named "tree". Every table
/// in this store uses the same shape: `&str` keys to opaque sealed bytes.
fn table_def(tree: &str) -> TableDefinition<'_, &'static str, &'static [u8]> {
    TableDefinition::new(tree)
}

// ── Encrypted store ───────────────────────────────────────────────────────────

/// An encrypted-at-rest key-value store unlocked by a user PIN/password.
///
/// Cloning is intentionally not derived: the unlock key is sensitive and the
/// underlying `redb::Database` already manages its own internal sharing, so
/// share an `EncryptedStore` behind an `Arc` if it must cross threads.
pub struct EncryptedStore {
    db: redb::Database,
    /// 32-byte AES-256 key, zeroized on drop. Never persisted.
    key: Zeroizing<[u8; 32]>,
}

impl EncryptedStore {
    /// Open (or create) an encrypted store rooted at `path`, unlocked with
    /// `pin`. `path` is a directory (created if missing) so that every
    /// existing caller — which historically pointed this at a `sled`
    /// directory — keeps working unchanged; the actual redb file lives at
    /// `path/comrade.redb`.
    ///
    /// On first use a random salt + verification token are written. On
    /// subsequent opens the PIN is verified against that token; a wrong PIN
    /// returns [`StorageError::InvalidPin`] rather than silently corrupting data.
    ///
    /// If `path` holds a pre-existing `sled` store from before the redb
    /// migration, it is transparently migrated in place first — see
    /// [`migrate::migrate_if_needed`].
    pub fn open(path: impl AsRef<Path>, pin: &str) -> Result<Self, StorageError> {
        let dir = path.as_ref();
        fs::create_dir_all(dir)
            .map_err(|e| StorageError::Corrupt(format!("cannot create store directory: {e}")))?;

        migrate::migrate_if_needed(dir, pin)?;

        let db = redb::Database::create(dir.join(REDB_FILE_NAME))?;

        // Load or create the Argon2 salt.
        let salt = match Self::meta_get(&db, SALT_KEY)? {
            Some(s) => s,
            None => {
                let mut s = vec![0u8; SALT_LEN];
                rand::thread_rng().fill_bytes(&mut s);
                Self::meta_put(&db, SALT_KEY, &s)?;
                debug!("storage: generated new Argon2 salt");
                s
            }
        };

        let key = derive_key(pin.as_bytes(), &salt)?;

        // Verify PIN, or write the verification token on first use.
        match Self::meta_get(&db, VERIFY_KEY)? {
            Some(token) => {
                let decrypted = aes_decrypt(&key, &token).map_err(|_| StorageError::InvalidPin)?;
                if decrypted != VERIFY_MAGIC {
                    return Err(StorageError::InvalidPin);
                }
            }
            None => {
                let token = aes_encrypt(&key, VERIFY_MAGIC)?;
                Self::meta_put(&db, VERIFY_KEY, &token)?;
                debug!("storage: wrote PIN verification token");
            }
        }

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
        let txn = self.db.begin_read()?;
        let Some(table) = open_read_table(&txn, tree)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for item in table.iter()? {
            let (_, sealed) = item?;
            let plaintext = self.unseal(sealed.value())?;
            out.push(serde_json::from_slice(&plaintext)?);
        }
        Ok(out)
    }

    // ── Raw byte access (for CRDT snapshots etc.) ────────────────────────────

    /// Seal and store raw bytes (used for binary blobs like Yrs state diffs).
    pub fn put_bytes(&self, tree: &str, key: &str, value: &[u8]) -> Result<(), StorageError> {
        let sealed = self.seal(value)?;
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(table_def(tree))?;
            table.insert(key, sealed.as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Fetch and decrypt raw bytes at `tree`/`key`.
    pub fn get_bytes(&self, tree: &str, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let txn = self.db.begin_read()?;
        let Some(table) = open_read_table(&txn, tree)? else {
            return Ok(None);
        };
        match table.get(key)? {
            Some(sealed) => Ok(Some(self.unseal(sealed.value())?)),
            None => Ok(None),
        }
    }

    // ── Maintenance ──────────────────────────────────────────────────────────

    /// Whether `tree`/`key` exists.
    pub fn contains(&self, tree: &str, key: &str) -> Result<bool, StorageError> {
        let txn = self.db.begin_read()?;
        let Some(table) = open_read_table(&txn, tree)? else {
            return Ok(false);
        };
        Ok(table.get(key)?.is_some())
    }

    /// Delete `tree`/`key`. Returns `true` if a value was removed.
    pub fn delete(&self, tree: &str, key: &str) -> Result<bool, StorageError> {
        let txn = self.db.begin_write()?;
        let existed = {
            let mut table = txn.open_table(table_def(tree))?;
            let removed = table.remove(key)?;
            removed.is_some()
        };
        txn.commit()?;
        Ok(existed)
    }

    /// List all logical keys in `tree` (keys are stored in plaintext).
    pub fn keys(&self, tree: &str) -> Result<Vec<String>, StorageError> {
        let txn = self.db.begin_read()?;
        let Some(table) = open_read_table(&txn, tree)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for item in table.iter()? {
            let (k, _) = item?;
            out.push(k.value().to_string());
        }
        Ok(out)
    }

    /// Flush all pending writes to disk. A no-op: every redb write
    /// transaction already commits durably (fsync'd) by default, unlike the
    /// old sled store which needed an explicit flush.
    pub fn flush(&self) -> Result<(), StorageError> {
        Ok(())
    }

    /// Re-key the entire store under a new PIN.
    ///
    /// Every value across every user tree is decrypted with the current key
    /// and re-sealed with a freshly derived key over a new salt, then the
    /// salt and verification token are rewritten — all inside a single redb
    /// write transaction, so the rekey is atomic: a crash or error at any
    /// point before `commit()` leaves the store exactly as it was under the
    /// old PIN (this closes AUDIT.md finding S2 / task M1-2, which sled's
    /// tree-at-a-time rekey could not guarantee).
    pub fn change_pin(&mut self, new_pin: &str) -> Result<(), StorageError> {
        self.change_pin_impl(new_pin, usize::MAX)
    }

    /// Body of [`Self::change_pin`] with a crash-injection seam for tests:
    /// returns an error (without ever calling `commit()`) after re-sealing
    /// `abort_after` values, simulating a process death mid-rekey. Because
    /// the whole rekey lives in one uncommitted write transaction, nothing it
    /// touched is visible afterward. Production callers pass `usize::MAX`.
    fn change_pin_impl(&mut self, new_pin: &str, abort_after: usize) -> Result<(), StorageError> {
        // Derive the new key over a fresh salt.
        let mut new_salt = vec![0u8; SALT_LEN];
        rand::thread_rng().fill_bytes(&mut new_salt);
        let new_key = derive_key(new_pin.as_bytes(), &new_salt)?;

        let txn = self.db.begin_write()?;
        {
            let tree_names: Vec<String> = txn
                .list_tables()?
                .map(|t| t.name().to_string())
                .filter(|name| name != META_TREE)
                .collect();

            let mut resealed_count = 0usize;
            for tree_name in tree_names {
                // Collect this tree's rows before reopening it for writing —
                // a table can't be read and mutated through overlapping
                // handles.
                let rows: Vec<(String, Vec<u8>)> = {
                    let table = txn.open_table(table_def(&tree_name))?;
                    let mut rows = Vec::new();
                    for item in table.iter()? {
                        let (k, v) = item?;
                        rows.push((k.value().to_string(), v.value().to_vec()));
                    }
                    rows
                };

                let mut table = txn.open_table(table_def(&tree_name))?;
                for (k, sealed) in rows {
                    if resealed_count >= abort_after {
                        // Test failpoint: bail before anything commits,
                        // simulating a power loss mid-rekey.
                        return Err(StorageError::Corrupt(
                            "rekey aborted by test failpoint".into(),
                        ));
                    }
                    let plaintext = self.unseal(&sealed)?;
                    let resealed = aes_encrypt(&new_key, &plaintext)?;
                    table.insert(k.as_str(), resealed.as_slice())?;
                    resealed_count += 1;
                }
            }

            // Rewrite salt + verification token under the new key.
            let mut meta = txn.open_table(table_def(META_TREE))?;
            meta.insert(SALT_KEY, new_salt.as_slice())?;
            meta.insert(VERIFY_KEY, aes_encrypt(&new_key, VERIFY_MAGIC)?.as_slice())?;
        }
        txn.commit()?;

        self.key = new_key;
        info!("storage: PIN changed and store re-encrypted");
        Ok(())
    }

    // ── Internal seal/unseal ─────────────────────────────────────────────────

    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
        aes_encrypt(&self.key, plaintext)
    }

    fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>, StorageError> {
        aes_decrypt(&self.key, sealed)
    }

    // ── Meta-table helpers (used before `self.key` exists) ──────────────────

    fn meta_get(db: &redb::Database, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let txn = db.begin_read()?;
        let Some(table) = open_read_table(&txn, META_TREE)? else {
            return Ok(None);
        };
        Ok(table.get(key)?.map(|v| v.value().to_vec()))
    }

    fn meta_put(db: &redb::Database, key: &str, value: &[u8]) -> Result<(), StorageError> {
        let txn = db.begin_write()?;
        {
            let mut table = txn.open_table(table_def(META_TREE))?;
            table.insert(key, value)?;
        }
        txn.commit()?;
        Ok(())
    }
}

/// Open `tree` for reading, treating a not-yet-created table as empty rather
/// than an error — redb (unlike `sled`) only auto-creates tables on write.
fn open_read_table(
    txn: &redb::ReadTransaction,
    tree: &str,
) -> Result<Option<redb::ReadOnlyTable<&'static str, &'static [u8]>>, StorageError> {
    match txn.open_table(table_def(tree)) {
        Ok(t) => Ok(Some(t)),
        Err(redb::TableError::TableDoesNotExist(_)) => Ok(None),
        Err(e) => Err(e.into()),
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
        } // drop releases the redb file handle
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
        // Scan every file in the store directory for the plaintext bytes.
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

    /// Regression test for AUDIT.md finding S2 / task M1-2 (crash-safe rekey).
    ///
    /// Asserts the atomic semantics: if the process dies mid-rekey, the
    /// interrupted `change_pin` must behave as if it never happened — the old
    /// PIN unlocks the store and every value still decrypts. This now
    /// genuinely holds under redb: the whole rekey runs inside one write
    /// transaction, and the failpoint returns before `commit()` is ever
    /// called, so nothing it touched is durable.
    #[test]
    fn interrupted_change_pin_leaves_store_fully_readable_with_old_pin() {
        let dir = TempDir::new().unwrap();
        {
            let mut store = EncryptedStore::open(dir.path(), "old-pin").unwrap();
            for i in 0..4u32 {
                store
                    .put("contacts", &format!("k{i}"), &format!("v{i}"))
                    .unwrap();
            }
            store.flush().unwrap();
            // Simulate a crash after 2 of the 4 values were re-sealed: the
            // failpoint bails before the transaction ever commits.
            let err = store.change_pin_impl("new-pin", 2);
            assert!(err.is_err(), "failpoint must abort the rekey");
        } // process "dies" here

        // Atomic semantics: the aborted rekey never happened.
        let store = EncryptedStore::open(dir.path(), "old-pin").expect("old PIN must still unlock");
        let vals: Vec<String> = store
            .values("contacts")
            .expect("every value must still decrypt under the old key");
        assert_eq!(vals.len(), 4, "no value may be lost to a torn rekey");
    }

    #[test]
    fn change_pin_reencrypts_and_revokes_old_pin() {
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
}
