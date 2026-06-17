/*!
 * Typed repositories layered over [`EncryptedStore`].
 *
 * These give the rest of the app a domain-shaped API (identity, contacts,
 * messages, ledger snapshots) without every caller hard-coding tree names.
 * The storage crate stays free of `comrade_core`/`nostr` dependencies to avoid
 * a dependency cycle, so identities and contacts are represented as plain data.
 */

use serde::{Deserialize, Serialize};

use crate::{EncryptedStore, StorageError};

// ── Tree names ────────────────────────────────────────────────────────────────

const IDENTITY_TREE: &str = "identity";
const CONTACTS_TREE: &str = "contacts";
const MESSAGES_TREE: &str = "messages";
const LEDGER_TREE: &str = "ledger";

const IDENTITY_KEY: &str = "self";
const LEDGER_SNAPSHOT_KEY: &str = "hisab_kitab";

// ── Domain types ──────────────────────────────────────────────────────────────

/// The local user's Nostr identity. The `nsec` is the secret key in Bech32 form
/// and is only ever stored sealed inside the [`EncryptedStore`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredIdentity {
    pub npub: String,
    pub nsec: String,
    pub label: Option<String>,
}

/// A saved contact with an optional petname and their advertised relays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub npub: String,
    pub petname: String,
    #[serde(default)]
    pub relays: Vec<String>,
}

/// A cached direct message (incoming or outgoing) for offline reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub peer_npub: String,
    pub content: String,
    pub created_at: u64,
    pub outgoing: bool,
}

// ── Repository methods ────────────────────────────────────────────────────────

impl EncryptedStore {
    // Identity ----------------------------------------------------------------

    /// Persist (or overwrite) the local user's identity.
    pub fn save_identity(&self, identity: &StoredIdentity) -> Result<(), StorageError> {
        self.put(IDENTITY_TREE, IDENTITY_KEY, identity)
    }

    /// Load the local user's identity, if one has been saved.
    pub fn load_identity(&self) -> Result<Option<StoredIdentity>, StorageError> {
        self.get(IDENTITY_TREE, IDENTITY_KEY)
    }

    // Contacts ----------------------------------------------------------------

    /// Insert or update a contact, keyed by npub.
    pub fn upsert_contact(&self, contact: &Contact) -> Result<(), StorageError> {
        self.put(CONTACTS_TREE, &contact.npub, contact)
    }

    /// Fetch a single contact by npub.
    pub fn get_contact(&self, npub: &str) -> Result<Option<Contact>, StorageError> {
        self.get(CONTACTS_TREE, npub)
    }

    /// Remove a contact by npub. Returns `true` if one was removed.
    pub fn remove_contact(&self, npub: &str) -> Result<bool, StorageError> {
        self.delete(CONTACTS_TREE, npub)
    }

    /// List all saved contacts.
    pub fn list_contacts(&self) -> Result<Vec<Contact>, StorageError> {
        self.values(CONTACTS_TREE)
    }

    // Messages ----------------------------------------------------------------

    /// Cache a direct message, keyed by its event id.
    pub fn save_message(&self, message: &StoredMessage) -> Result<(), StorageError> {
        self.put(MESSAGES_TREE, &message.id, message)
    }

    /// All cached messages exchanged with `peer_npub`, sorted oldest-first.
    pub fn messages_with(&self, peer_npub: &str) -> Result<Vec<StoredMessage>, StorageError> {
        let mut msgs: Vec<StoredMessage> = self
            .values::<StoredMessage>(MESSAGES_TREE)?
            .into_iter()
            .filter(|m| m.peer_npub == peer_npub)
            .collect();
        msgs.sort_by_key(|m| m.created_at);
        Ok(msgs)
    }

    /// All cached messages across every conversation, sorted oldest-first.
    pub fn all_messages(&self) -> Result<Vec<StoredMessage>, StorageError> {
        let mut msgs: Vec<StoredMessage> = self.values(MESSAGES_TREE)?;
        msgs.sort_by_key(|m| m.created_at);
        Ok(msgs)
    }

    // Ledger ------------------------------------------------------------------

    /// Persist a binary CRDT (Yrs) ledger snapshot.
    pub fn save_ledger_snapshot(&self, state: &[u8]) -> Result<(), StorageError> {
        self.put_bytes(LEDGER_TREE, LEDGER_SNAPSHOT_KEY, state)
    }

    /// Load the most recent CRDT ledger snapshot, if any.
    pub fn load_ledger_snapshot(&self) -> Result<Option<Vec<u8>>, StorageError> {
        self.get_bytes(LEDGER_TREE, LEDGER_SNAPSHOT_KEY)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, EncryptedStore) {
        let dir = TempDir::new().unwrap();
        let store = EncryptedStore::open(dir.path(), "pin").unwrap();
        (dir, store)
    }

    #[test]
    fn identity_roundtrip() {
        let (_d, s) = store();
        assert!(s.load_identity().unwrap().is_none());
        let id = StoredIdentity {
            npub: "npub1abc".into(),
            nsec: "nsec1xyz".into(),
            label: Some("primary".into()),
        };
        s.save_identity(&id).unwrap();
        assert_eq!(s.load_identity().unwrap(), Some(id));
    }

    #[test]
    fn contacts_crud() {
        let (_d, s) = store();
        let alice = Contact {
            npub: "npub1alice".into(),
            petname: "Alice".into(),
            relays: vec!["wss://relay.one".into()],
        };
        let bob = Contact {
            npub: "npub1bob".into(),
            petname: "Bob".into(),
            relays: vec![],
        };
        s.upsert_contact(&alice).unwrap();
        s.upsert_contact(&bob).unwrap();
        assert_eq!(s.list_contacts().unwrap().len(), 2);
        assert_eq!(s.get_contact("npub1alice").unwrap(), Some(alice));
        assert!(s.remove_contact("npub1bob").unwrap());
        assert_eq!(s.list_contacts().unwrap().len(), 1);
    }

    #[test]
    fn messages_filtered_by_peer_and_sorted() {
        let (_d, s) = store();
        s.save_message(&StoredMessage {
            id: "e2".into(),
            peer_npub: "npub1alice".into(),
            content: "second".into(),
            created_at: 200,
            outgoing: true,
        })
        .unwrap();
        s.save_message(&StoredMessage {
            id: "e1".into(),
            peer_npub: "npub1alice".into(),
            content: "first".into(),
            created_at: 100,
            outgoing: false,
        })
        .unwrap();
        s.save_message(&StoredMessage {
            id: "e3".into(),
            peer_npub: "npub1bob".into(),
            content: "other".into(),
            created_at: 150,
            outgoing: false,
        })
        .unwrap();

        let with_alice = s.messages_with("npub1alice").unwrap();
        assert_eq!(with_alice.len(), 2);
        assert_eq!(with_alice[0].content, "first");
        assert_eq!(with_alice[1].content, "second");
        assert_eq!(s.all_messages().unwrap().len(), 3);
    }

    #[test]
    fn ledger_snapshot_roundtrip() {
        let (_d, s) = store();
        assert!(s.load_ledger_snapshot().unwrap().is_none());
        let snap = vec![10u8, 20, 30, 40];
        s.save_ledger_snapshot(&snap).unwrap();
        assert_eq!(s.load_ledger_snapshot().unwrap(), Some(snap));
    }
}
