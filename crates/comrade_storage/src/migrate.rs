/*!
 * One-time migration from the legacy `sled`-backed store to `redb`.
 *
 * `sled` stays a dependency of this crate *only* for this reader — nothing
 * else in `comrade_storage` touches it. Every call to [`EncryptedStore::open`]
 * checks first: if `path` holds a `sled` store that hasn't been migrated yet,
 * every value is decrypted under the caller's PIN and re-ingested into a
 * fresh `redb` file, after which the old `sled` files are archived alongside
 * it. Once migrated, this check is a cheap no-op (`comrade.redb` exists) on
 * every later open.
 *
 * Crash safety: the new `redb` file is built to a staging path and only
 * `rename`d to its final name — a single filesystem op — once it is fully
 * populated and committed. That rename is the linearization point: before it,
 * an interrupted migration leaves the original `sled` store untouched and
 * migration is retried from scratch on the next open; after it, the store is
 * fully on `redb` and archiving the old files is a best-effort cleanup that
 * can safely fail without risking data loss.
 */

use std::path::Path;

use rand::RngCore;
use tracing::{info, warn};

use crate::{
    aes_decrypt, aes_encrypt, derive_key, table_def, StorageError, META_TREE, REDB_FILE_NAME,
    SALT_KEY, SALT_LEN, VERIFY_KEY, VERIFY_MAGIC,
};

/// Directory the legacy sled files are archived into after a successful
/// migration, so the store directory never shows a mix of old and new state.
const SLED_ARCHIVE_DIR: &str = "sled-archive";

/// `sled`'s own reserved name for the tree created when none is specified.
const SLED_DEFAULT_TREE: &[u8] = b"__sled__default";

/// True if `dir` holds a legacy, un-migrated `sled` store. `sled` always
/// writes a `conf` file at the root of its directory; a `redb` store at this
/// path already existing means migration already ran.
fn is_legacy_sled_store(dir: &Path) -> bool {
    dir.join("conf").is_file() && !dir.join(REDB_FILE_NAME).exists()
}

/// Migrate a legacy `sled` store found at `dir` into a fresh `redb` file,
/// re-encrypting every value under a newly derived key, then archive the old
/// files. A no-op returning `Ok(())` if no legacy store is present.
///
/// `pin` must be the same passphrase the legacy store was unlocked with; a
/// mismatch surfaces as [`StorageError::InvalidPin`], exactly as a normal
/// open would.
pub(crate) fn migrate_if_needed(dir: &Path, pin: &str) -> Result<(), StorageError> {
    if !is_legacy_sled_store(dir) {
        return Ok(());
    }
    info!(
        ?dir,
        "storage: legacy sled store detected, migrating to redb"
    );

    let old_db = sled::open(dir)?;
    let old_meta = old_db.open_tree(META_TREE)?;

    let old_salt = old_meta
        .get(SALT_KEY)?
        .ok_or_else(|| StorageError::Corrupt("legacy store missing Argon2 salt".into()))?
        .to_vec();
    let old_key = derive_key(pin.as_bytes(), &old_salt)?;

    let token = old_meta
        .get(VERIFY_KEY)?
        .ok_or_else(|| StorageError::Corrupt("legacy store missing verification token".into()))?;
    let decrypted = aes_decrypt(&old_key, &token).map_err(|_| StorageError::InvalidPin)?;
    if decrypted != VERIFY_MAGIC {
        return Err(StorageError::InvalidPin);
    }

    // Build the new store at a staging path alongside the old one, so a
    // failure here never disturbs the still-intact legacy data.
    let staging_path = dir.join(format!("{REDB_FILE_NAME}.migrating"));
    let _ = std::fs::remove_file(&staging_path);
    let new_db = redb::Database::create(&staging_path)?;

    // Fresh salt/key for the migrated store — every value is being rewritten
    // anyway, so there's no reason to keep the old Argon2 salt.
    let mut new_salt = vec![0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut new_salt);
    let new_key = derive_key(pin.as_bytes(), &new_salt)?;

    let txn = new_db.begin_write()?;
    {
        for tree_name in old_db.tree_names() {
            if tree_name.as_ref() == META_TREE.as_bytes() || tree_name.as_ref() == SLED_DEFAULT_TREE
            {
                continue;
            }
            let tree_name = std::str::from_utf8(&tree_name)
                .map_err(|e| StorageError::Corrupt(format!("non-utf8 legacy tree name: {e}")))?
                .to_string();

            let old_tree = old_db.open_tree(&tree_name)?;
            let mut table = txn.open_table(table_def(&tree_name))?;
            for item in old_tree.iter() {
                let (k, sealed) = item?;
                let plaintext = aes_decrypt(&old_key, &sealed)?;
                let resealed = aes_encrypt(&new_key, &plaintext)?;
                let key = std::str::from_utf8(&k)
                    .map_err(|e| StorageError::Corrupt(format!("non-utf8 legacy key: {e}")))?;
                table.insert(key, resealed.as_slice())?;
            }
        }

        let mut meta = txn.open_table(table_def(META_TREE))?;
        meta.insert(SALT_KEY, new_salt.as_slice())?;
        meta.insert(VERIFY_KEY, aes_encrypt(&new_key, VERIFY_MAGIC)?.as_slice())?;
    }
    txn.commit()?;
    drop(new_db);
    drop(old_db); // release the sled lock before touching its files

    // Linearization point: once this rename lands, the store is fully on
    // redb — a crash before it retries migration from scratch next open; a
    // crash after it just leaves stray (but harmless) legacy files behind.
    std::fs::rename(&staging_path, dir.join(REDB_FILE_NAME))
        .map_err(|e| StorageError::Corrupt(format!("cannot finalize migrated redb file: {e}")))?;

    archive_legacy_files(dir);

    info!("storage: migration to redb complete");
    Ok(())
}

/// Best-effort: move every pre-existing entry in `dir` (the old sled files)
/// into `dir/sled-archive/`. The migration has already succeeded by the time
/// this runs, so a failure here is logged, not propagated — the worst case is
/// leftover sled files sitting alongside the now-authoritative redb file.
fn archive_legacy_files(dir: &Path) {
    let archive_dir = dir.join(SLED_ARCHIVE_DIR);
    if let Err(e) = std::fs::create_dir_all(&archive_dir) {
        warn!("storage: could not create sled archive dir, leaving legacy files in place: {e}");
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            warn!("storage: could not read store dir to archive legacy sled files: {e}");
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == SLED_ARCHIVE_DIR || name == REDB_FILE_NAME {
            continue;
        }
        let dest = archive_dir.join(&name);
        if let Err(e) = std::fs::rename(entry.path(), &dest) {
            warn!(file = ?name, "storage: could not archive legacy sled file: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a legacy `sled` store exactly the way the pre-redb
    /// `EncryptedStore::open` used to: an Argon2 salt + AES-GCM-sealed
    /// verification token in `__comrade_meta__`, plus sealed raw values in
    /// arbitrary user trees.
    fn seed_legacy_sled_store(dir: &Path, pin: &str, rows: &[(&str, &str, &[u8])]) {
        let db = sled::open(dir).unwrap();
        let mut salt = vec![0u8; SALT_LEN];
        rand::thread_rng().fill_bytes(&mut salt);
        let key = derive_key(pin.as_bytes(), &salt).unwrap();

        let meta = db.open_tree(META_TREE).unwrap();
        meta.insert(SALT_KEY, salt).unwrap();
        meta.insert(VERIFY_KEY, aes_encrypt(&key, VERIFY_MAGIC).unwrap())
            .unwrap();

        for (tree, k, plaintext) in rows {
            let t = db.open_tree(*tree).unwrap();
            t.insert(*k, aes_encrypt(&key, plaintext).unwrap()).unwrap();
        }
        db.flush().unwrap();
    }

    #[test]
    fn migrates_legacy_store_and_archives_old_files() {
        let dir = TempDir::new().unwrap();
        seed_legacy_sled_store(
            dir.path(),
            "old-pin",
            &[
                ("identity", "self", b"nsec1secretvalue"),
                ("contacts", "npub1alice", b"alice-contact-blob"),
            ],
        );
        assert!(is_legacy_sled_store(dir.path()));

        migrate_if_needed(dir.path(), "old-pin").unwrap();

        assert!(
            !is_legacy_sled_store(dir.path()),
            "migration must be one-shot"
        );
        assert!(dir.path().join(REDB_FILE_NAME).exists());
        assert!(
            dir.path().join(SLED_ARCHIVE_DIR).join("conf").exists(),
            "legacy sled files must be archived, not left at the top level"
        );
        assert!(
            !dir.path().join("conf").exists(),
            "the top level must no longer look like a sled store"
        );

        // The migrated data is byte-exact through the normal public API.
        let store = crate::EncryptedStore::open(dir.path(), "old-pin").unwrap();
        assert_eq!(
            store.get_bytes("identity", "self").unwrap(),
            Some(b"nsec1secretvalue".to_vec())
        );
        assert_eq!(
            store.get_bytes("contacts", "npub1alice").unwrap(),
            Some(b"alice-contact-blob".to_vec())
        );
    }

    #[test]
    fn wrong_pin_during_migration_is_rejected_without_touching_legacy_store() {
        let dir = TempDir::new().unwrap();
        seed_legacy_sled_store(dir.path(), "right-pin", &[("identity", "self", b"secret")]);

        let err = migrate_if_needed(dir.path(), "WRONG-pin");
        assert!(matches!(err, Err(StorageError::InvalidPin)));

        // Untouched: still detected as legacy, still openable with the right pin.
        assert!(is_legacy_sled_store(dir.path()));
        migrate_if_needed(dir.path(), "right-pin").unwrap();
        let store = crate::EncryptedStore::open(dir.path(), "right-pin").unwrap();
        assert_eq!(
            store.get_bytes("identity", "self").unwrap(),
            Some(b"secret".to_vec())
        );
    }

    #[test]
    fn no_legacy_store_is_a_clean_no_op() {
        let dir = TempDir::new().unwrap();
        migrate_if_needed(dir.path(), "any-pin").unwrap();
        assert!(!dir.path().join(REDB_FILE_NAME).exists());
    }
}
