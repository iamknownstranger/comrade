//! Milestone 5 — durability & error-handling integration tests.
//!
//! These exercise the *public* `comrade_storage` API only (as the app would),
//! simulating client reboots by dropping the `EncryptedStore` (which releases
//! the underlying redb file handle) and reopening it at the same path with the
//! same passphrase. Every assertion checks that data survives uncorrupted and
//! that failures come back as `Result::Err` — never a panic that would take
//! down the core thread.

use comrade_storage::{
    Chitthi, EncryptedStore, LedgerState, StorageError, StoredIdentity, StoredMessage,
};
use tempfile::TempDir;

/// Open a store at `dir` with `pin`, run `f`, then drop it (simulating shutdown).
fn with_store(dir: &TempDir, pin: &str, f: impl FnOnce(&EncryptedStore)) {
    let store = EncryptedStore::open(dir.path(), pin).expect("open store");
    f(&store);
    store.flush().expect("flush");
    // store dropped here -> redb file handle released, key zeroized.
}

#[test]
fn full_state_survives_a_reboot_uncorrupted() {
    let dir = TempDir::new().unwrap();
    let pin = "ek-do-teen-char";

    // ── Boot 1: write one of every persisted structure ──────────────────────
    with_store(&dir, pin, |s| {
        let mut id = StoredIdentity::new("npub1self", "nsec1secret", Some("primary".into()));
        id.relays = vec!["wss://relay.damus.io".into(), "wss://nos.lol".into()];
        s.save_identity(&id).unwrap();

        s.cache_chitthi(&Chitthi {
            id: "chit-1".into(),
            author_npub: "npub1a".into(),
            content: "first post".into(),
            created_at: 100,
            reply_to: None,
        })
        .unwrap();
        s.cache_chitthi(&Chitthi {
            id: "chit-2".into(),
            author_npub: "npub1b".into(),
            content: "a reply".into(),
            created_at: 200,
            reply_to: Some("chit-1".into()),
        })
        .unwrap();

        s.save_message(&StoredMessage {
            id: "dm-1".into(),
            peer_npub: "npub1peer".into(),
            content: "encrypted hello".into(),
            created_at: 150,
            outgoing: false,
            status: None,
            reply_to: None,
        })
        .unwrap();

        s.save_ledger_state(&LedgerState {
            snapshot: vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0xFF],
            updated_at: 1_700_000_000,
        })
        .unwrap();
    });

    // ── Boot 2: everything is present, intact and correctly typed ───────────
    with_store(&dir, pin, |s| {
        let id = s.load_identity().unwrap().expect("identity persisted");
        assert_eq!(id.npub, "npub1self");
        assert_eq!(id.nsec, "nsec1secret");
        assert_eq!(id.relays, vec!["wss://relay.damus.io", "wss://nos.lol"]);

        let feed = s.chitthi_cache().unwrap();
        assert_eq!(feed.len(), 2);
        assert_eq!(feed[0].content, "a reply"); // newest first
        assert_eq!(feed[1].content, "first post");
        assert_eq!(feed[0].reply_to.as_deref(), Some("chit-1"));

        let dms = s.vault_cache().unwrap();
        assert_eq!(dms.len(), 1);
        assert_eq!(dms[0].content, "encrypted hello");

        let ledger = s.load_ledger_state().unwrap().expect("ledger persisted");
        assert_eq!(ledger.snapshot, vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0xFF]);
        assert_eq!(ledger.updated_at, 1_700_000_000);
    });
}

#[test]
fn many_reboots_keep_appending_without_corruption() {
    let dir = TempDir::new().unwrap();
    let pin = "persist";

    // Ten boot cycles, each adds one Chitthi; the cache must grow monotonically
    // and every earlier entry must still decrypt cleanly.
    for n in 0..10u64 {
        with_store(&dir, pin, |s| {
            s.cache_chitthi(&Chitthi {
                id: format!("c{n}"),
                author_npub: "npub1a".into(),
                content: format!("boot {n}"),
                created_at: n,
                reply_to: None,
            })
            .unwrap();
            // All prior writes remain readable on every boot.
            let feed = s.chitthi_cache().unwrap();
            assert_eq!(feed.len() as u64, n + 1);
        });
    }

    with_store(&dir, pin, |s| {
        let feed = s.chitthi_cache().unwrap();
        assert_eq!(feed.len(), 10);
        // Newest-first ordering holds across all reboots.
        assert_eq!(feed.first().unwrap().content, "boot 9");
        assert_eq!(feed.last().unwrap().content, "boot 0");
    });
}

#[test]
fn binary_ledger_chunk_is_byte_exact_across_reboot() {
    let dir = TempDir::new().unwrap();
    let pin = "crdt";
    // A non-trivial binary blob with all byte values, to catch any truncation
    // or text-encoding corruption in the AES-GCM envelope.
    let blob: Vec<u8> = (0..=255u8).cycle().take(4096).collect();

    with_store(&dir, pin, |s| {
        s.save_ledger_snapshot(&blob).unwrap();
    });
    with_store(&dir, pin, |s| {
        assert_eq!(
            s.load_ledger_snapshot().unwrap().as_deref(),
            Some(blob.as_slice())
        );
    });
}

#[test]
fn change_pin_survives_reboot_and_revokes_old_passphrase() {
    let dir = TempDir::new().unwrap();

    {
        let mut s = EncryptedStore::open(dir.path(), "old").unwrap();
        s.save_identity(&StoredIdentity::new("npub1x", "nsec1x", None))
            .unwrap();
        s.change_pin("new").unwrap();
        s.flush().unwrap();
    }

    // Old passphrase is now rejected — as a Result, not a panic.
    let err = EncryptedStore::open(dir.path(), "old");
    assert!(matches!(err, Err(StorageError::InvalidPin)));

    // New passphrase unlocks and the data is intact after the reboot.
    let s = EncryptedStore::open(dir.path(), "new").unwrap();
    assert_eq!(s.load_identity().unwrap().unwrap().npub, "npub1x");
}

#[test]
fn wrong_passphrase_returns_error_not_panic() {
    let dir = TempDir::new().unwrap();
    {
        let s = EncryptedStore::open(dir.path(), "right").unwrap();
        s.save_message(&StoredMessage {
            id: "dm".into(),
            peer_npub: "npub1p".into(),
            content: "secret".into(),
            created_at: 1,
            outgoing: true,
            status: Some("sent".into()),
            reply_to: None,
        })
        .unwrap();
        s.flush().unwrap();
    }
    // The unified error type carries the failure cleanly; the caller decides.
    match EncryptedStore::open(dir.path(), "WRONG") {
        Err(StorageError::InvalidPin) => {}
        Err(other) => panic!("expected InvalidPin, got {other:?}"),
        Ok(_) => panic!("a wrong passphrase must not unlock the store"),
    }
}

#[test]
fn missing_lookups_are_ok_none_across_reboot() {
    let dir = TempDir::new().unwrap();
    let pin = "p";
    // Reading from a never-written store yields Ok(None)/empty, never an error.
    with_store(&dir, pin, |s| {
        assert!(s.load_identity().unwrap().is_none());
        assert!(s.get_chitthi("nope").unwrap().is_none());
        assert!(s.chitthi_cache().unwrap().is_empty());
        assert!(s.vault_cache().unwrap().is_empty());
        assert!(s.load_ledger_state().unwrap().is_none());
    });
    // And again after a reboot, the empty state is stable.
    with_store(&dir, pin, |s| {
        assert!(s.chitthi_cache().unwrap().is_empty());
    });
}
