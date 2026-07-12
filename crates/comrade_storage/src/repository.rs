/*!
 * Typed repositories layered over [`EncryptedStore`].
 *
 * These give the rest of the app a domain-shaped API (identity, Chitthi cache,
 * vault cache, ledger state) without every caller hard-coding tree names.
 * The storage crate stays free of `comrade_core`/`nostr` dependencies to avoid
 * a dependency cycle, so identities and contacts are represented as plain data.
 *
 * # Encryption pipeline (Milestone 3)
 *
 * Initialisation is a single secure routine — [`EncryptedStore::open`] — that
 * accepts a user-defined passphrase from the application thread:
 *
 * 1. **Key derivation** — the passphrase is stretched with **Argon2id**
 *    (memory-hard) over a per-store random salt into a 32-byte AES-256 key that
 *    lives only in memory ([`Zeroizing`]) and is zeroized on drop.
 * 2. **Envelope encryption** — every repository write (`save_identity`,
 *    `save_message`, `cache_chitthi`, `save_ledger_state`, …) serialises to
 *    JSON and seals it with **AES-256-GCM** (random 96-bit nonce per record)
 *    before it touches disk. Reads authenticate-then-decrypt.
 *
 * The upshot: sensitive profiles, raw direct messages, and private identity
 * keys (`nsec`) are ciphertext at rest — see the `nsec_never_plaintext_at_rest`
 * test below, which scans the on-disk files to prove it.
 *
 * [`Zeroizing`]: zeroize::Zeroizing
 */

use serde::{Deserialize, Serialize};

use crate::{EncryptedStore, StorageError};

/// Monotonic rank of a delivery status so receipts only ever move forward:
/// sent (0) < delivered (1) < read (2). Unknown strings rank as sent.
fn status_rank(status: &str) -> u8 {
    match status {
        "read" => 2,
        "delivered" => 1,
        _ => 0,
    }
}

// ── Tree names ────────────────────────────────────────────────────────────────

const IDENTITY_TREE: &str = "identity";
const CONTACTS_TREE: &str = "contacts";
const CHITTHI_TREE: &str = "chitthi_cache";
const MESSAGES_TREE: &str = "vault_cache";
const LEDGER_TREE: &str = "ledger";
const JOURNAL_TREE: &str = "journal";
/// Per-peer conversation gate: request / accepted / blocked (message requests).
const CONVERSATIONS_TREE: &str = "conversation_meta";
/// Voice/video call log, keyed by call id.
const CALLS_TREE: &str = "call_log";

const IDENTITY_KEY: &str = "self";
const LEDGER_SNAPSHOT_KEY: &str = "hisab_kitab";
const LEDGER_STATE_KEY: &str = "hisab_kitab_state";

// ── Domain types ──────────────────────────────────────────────────────────────

/// The local user's Nostr identity, plus the relay setup it was last seen with.
///
/// The `nsec` is the secret key in Bech32 form and is only ever stored sealed
/// inside the [`EncryptedStore`]. `relays` carries the user's current NIP-65
/// relay-list URLs so the outbox model can be reconstructed on a cold start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredIdentity {
    pub npub: String,
    pub nsec: String,
    pub label: Option<String>,
    /// Advertised relay URLs (NIP-65). Defaulted for backward-compatible reads.
    #[serde(default)]
    pub relays: Vec<String>,
}

impl StoredIdentity {
    /// Construct an identity with no relay list yet.
    pub fn new(npub: impl Into<String>, nsec: impl Into<String>, label: Option<String>) -> Self {
        Self {
            npub: npub.into(),
            nsec: nsec.into(),
            label,
            relays: Vec::new(),
        }
    }
}

/// A saved contact with an optional petname and their advertised relays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub npub: String,
    pub petname: String,
    #[serde(default)]
    pub relays: Vec<String>,
}

/// A single cached public Chitthi (Kind-1 note) for instant offline rendering
/// of the Sabha timeline. Keyed in the store by its event `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chitthi {
    /// Nostr event id (hex).
    pub id: String,
    /// Author public key (npub or hex).
    pub author_npub: String,
    pub content: String,
    pub created_at: u64,
    /// Parent event id if this Chitthi is a reply (NIP-10), else `None`.
    #[serde(default)]
    pub reply_to: Option<String>,
}

/// A cached direct message (incoming or outgoing) for offline reading. One row
/// of the [`VaultCache`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub peer_npub: String,
    pub content: String,
    pub created_at: u64,
    pub outgoing: bool,
    /// Delivery status of an outgoing message: `"sent"`, `"delivered"`, or
    /// `"read"`. Incoming messages are always `"read"`. Defaulted to `"sent"`
    /// so rows written before receipts existed keep deserialising.
    #[serde(default)]
    pub status: Option<String>,
    /// Event id (hex) this message replies to (NIP-10 `e` tag), if any.
    #[serde(default)]
    pub reply_to: Option<String>,
}

/// A private journal entry — the wellbeing pillar's core record. Strictly
/// local: journal entries are never published to a relay or any network; the
/// only copy lives sealed inside this encrypted store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Store key. Zero-padded-timestamp-prefixed so ids sort chronologically.
    pub id: String,
    pub text: String,
    /// Optional self-reported mood marker (an emoji or short tag).
    #[serde(default)]
    pub mood: Option<String>,
    pub created_at: u64,
}

/// Per-peer conversation gate — the storage half of message requests. A DM from
/// a peer with no `Accepted` record lands in the *requests* bucket instead of
/// the chat list, and no profile/receipts are shared with them until accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationMeta {
    pub peer_npub: String,
    /// `"pending"` (incoming request), `"accepted"`, or `"blocked"`.
    pub state: String,
    /// Whether we have shared our @handle with this peer (sent on accept, or
    /// implicitly when we started the conversation).
    #[serde(default)]
    pub profile_shared: bool,
    pub updated_at: u64,
}

/// One entry of the voice/video call log, keyed by call id. Mirrors the shape
/// of [`StoredMessage`] so the chat UI can interleave calls and messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallRecord {
    /// The call id minted by the caller (hex).
    pub id: String,
    pub peer_npub: String,
    /// `"audio"` or `"video"`.
    pub media: String,
    /// True if this device received the call, false if it placed it.
    pub incoming: bool,
    /// `"connected"`, `"missed"`, `"declined"`, `"cancelled"`, `"busy"`, or `"failed"`.
    pub outcome: String,
    pub started_at: u64,
    /// Connected duration in seconds; 0 if the call never connected.
    #[serde(default)]
    pub duration_secs: u64,
}

/// A binary CRDT (Yrs) snapshot of the Sakha/Sakhi shared ledger, plus the wall
/// clock at which it was captured. The bytes are opaque AES-256-GCM ciphertext
/// once written to disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerState {
    pub snapshot: Vec<u8>,
    pub updated_at: u64,
}

/// The locally cached slice of the public Sabha timeline (newest first).
pub type ChitthiCache = Vec<Chitthi>;

/// The locally cached direct-message history across all conversations.
pub type VaultCache = Vec<StoredMessage>;

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

    // Chitthi cache (public Sabha timeline) -----------------------------------

    /// Cache a public Chitthi, keyed by its event id. Idempotent on re-insert.
    pub fn cache_chitthi(&self, chitthi: &Chitthi) -> Result<(), StorageError> {
        self.put(CHITTHI_TREE, &chitthi.id, chitthi)
    }

    /// Fetch a single cached Chitthi by event id.
    pub fn get_chitthi(&self, id: &str) -> Result<Option<Chitthi>, StorageError> {
        self.get(CHITTHI_TREE, id)
    }

    /// The whole cached Sabha timeline, newest-first, for offline rendering.
    pub fn chitthi_cache(&self) -> Result<ChitthiCache, StorageError> {
        let mut feed: ChitthiCache = self.values(CHITTHI_TREE)?;
        feed.sort_by_key(|b| std::cmp::Reverse(b.created_at));
        Ok(feed)
    }

    /// Remove a cached Chitthi by id. Returns `true` if one was removed.
    pub fn remove_chitthi(&self, id: &str) -> Result<bool, StorageError> {
        self.delete(CHITTHI_TREE, id)
    }

    // Vault cache (encrypted DM history) --------------------------------------

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

    /// The entire [`VaultCache`] across every conversation, sorted oldest-first.
    pub fn vault_cache(&self) -> Result<VaultCache, StorageError> {
        let mut msgs: VaultCache = self.values(MESSAGES_TREE)?;
        msgs.sort_by_key(|m| m.created_at);
        Ok(msgs)
    }

    /// Alias for [`Self::vault_cache`] kept for call-site readability.
    pub fn all_messages(&self) -> Result<VaultCache, StorageError> {
        self.vault_cache()
    }

    /// Fetch a single stored message by its event id.
    pub fn get_message(&self, id: &str) -> Result<Option<StoredMessage>, StorageError> {
        self.get(MESSAGES_TREE, id)
    }

    /// Advance an outgoing message's delivery `status` (sent → delivered →
    /// read) in response to a receipt. Never downgrades — a late "delivered"
    /// receipt can't unset a "read" already recorded. Returns whether the row
    /// existed and changed.
    pub fn set_message_status(&self, id: &str, status: &str) -> Result<bool, StorageError> {
        let Some(mut msg) = self.get_message(id)? else {
            return Ok(false);
        };
        if status_rank(status) <= status_rank(msg.status.as_deref().unwrap_or("sent")) {
            return Ok(false);
        }
        msg.status = Some(status.to_string());
        self.save_message(&msg)?;
        Ok(true)
    }

    // Conversation gate (message requests) ------------------------------------

    /// Insert or update the conversation gate for a peer.
    pub fn set_conversation_meta(&self, meta: &ConversationMeta) -> Result<(), StorageError> {
        self.put(CONVERSATIONS_TREE, &meta.peer_npub, meta)
    }

    /// Fetch a peer's conversation gate, if one has been recorded.
    pub fn get_conversation_meta(
        &self,
        peer_npub: &str,
    ) -> Result<Option<ConversationMeta>, StorageError> {
        self.get(CONVERSATIONS_TREE, peer_npub)
    }

    /// All recorded conversation gates.
    pub fn list_conversation_meta(&self) -> Result<Vec<ConversationMeta>, StorageError> {
        self.values(CONVERSATIONS_TREE)
    }

    /// Remove a peer's conversation gate. Returns `true` if one existed.
    pub fn remove_conversation_meta(&self, peer_npub: &str) -> Result<bool, StorageError> {
        self.delete(CONVERSATIONS_TREE, peer_npub)
    }

    // Call log ----------------------------------------------------------------

    /// Persist (or overwrite) a call-log entry, keyed by call id.
    pub fn save_call_record(&self, record: &CallRecord) -> Result<(), StorageError> {
        self.put(CALLS_TREE, &record.id, record)
    }

    /// All call-log entries exchanged with `peer_npub`, newest first.
    pub fn calls_with(&self, peer_npub: &str) -> Result<Vec<CallRecord>, StorageError> {
        let mut calls: Vec<CallRecord> = self
            .values::<CallRecord>(CALLS_TREE)?
            .into_iter()
            .filter(|c| c.peer_npub == peer_npub)
            .collect();
        calls.sort_by_key(|c| std::cmp::Reverse(c.started_at));
        Ok(calls)
    }

    /// The whole call log across every peer, newest first.
    pub fn all_calls(&self) -> Result<Vec<CallRecord>, StorageError> {
        let mut calls: Vec<CallRecord> = self.values(CALLS_TREE)?;
        calls.sort_by_key(|c| std::cmp::Reverse(c.started_at));
        Ok(calls)
    }

    // Journal (local-only, never networked) ------------------------------------

    /// Persist a journal entry, keyed by its id.
    pub fn save_journal_entry(&self, entry: &JournalEntry) -> Result<(), StorageError> {
        self.put(JOURNAL_TREE, &entry.id, entry)
    }

    /// All journal entries, newest first.
    pub fn journal_entries(&self) -> Result<Vec<JournalEntry>, StorageError> {
        let mut entries: Vec<JournalEntry> = self.values(JOURNAL_TREE)?;
        entries.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(entries)
    }

    /// Remove a journal entry by id. Returns `true` if one was removed.
    pub fn remove_journal_entry(&self, id: &str) -> Result<bool, StorageError> {
        self.delete(JOURNAL_TREE, id)
    }

    // Ledger ------------------------------------------------------------------

    /// Persist a binary CRDT (Yrs) ledger snapshot (raw bytes).
    pub fn save_ledger_snapshot(&self, state: &[u8]) -> Result<(), StorageError> {
        self.put_bytes(LEDGER_TREE, LEDGER_SNAPSHOT_KEY, state)
    }

    /// Load the most recent CRDT ledger snapshot, if any (raw bytes).
    pub fn load_ledger_snapshot(&self) -> Result<Option<Vec<u8>>, StorageError> {
        self.get_bytes(LEDGER_TREE, LEDGER_SNAPSHOT_KEY)
    }

    /// Persist a [`LedgerState`] (binary CRDT chunk + capture timestamp).
    pub fn save_ledger_state(&self, state: &LedgerState) -> Result<(), StorageError> {
        self.put(LEDGER_TREE, LEDGER_STATE_KEY, state)
    }

    /// Load the most recent [`LedgerState`], if one has been captured.
    pub fn load_ledger_state(&self) -> Result<Option<LedgerState>, StorageError> {
        self.get(LEDGER_TREE, LEDGER_STATE_KEY)
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
            relays: vec!["wss://relay.damus.io".into()],
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
            status: Some("sent".into()),
            reply_to: None,
        })
        .unwrap();
        s.save_message(&StoredMessage {
            id: "e1".into(),
            peer_npub: "npub1alice".into(),
            content: "first".into(),
            created_at: 100,
            outgoing: false,
            status: None,
            reply_to: None,
        })
        .unwrap();
        s.save_message(&StoredMessage {
            id: "e3".into(),
            peer_npub: "npub1bob".into(),
            content: "other".into(),
            created_at: 150,
            outgoing: false,
            status: None,
            reply_to: Some("e1".into()),
        })
        .unwrap();

        let with_alice = s.messages_with("npub1alice").unwrap();
        assert_eq!(with_alice.len(), 2);
        assert_eq!(with_alice[0].content, "first");
        assert_eq!(with_alice[1].content, "second");
        assert_eq!(s.all_messages().unwrap().len(), 3);
    }

    #[test]
    fn journal_crud_and_ordering() {
        let (_d, s) = store();
        assert!(s.journal_entries().unwrap().is_empty());
        for (id, text, mood, at) in [
            ("00000000000000000010-aaaa", "first thought", None, 10u64),
            (
                "00000000000000000030-cccc",
                "grateful today",
                Some("🙂"),
                30,
            ),
            ("00000000000000000020-bbbb", "rough morning", Some("😕"), 20),
        ] {
            s.save_journal_entry(&JournalEntry {
                id: id.into(),
                text: text.into(),
                mood: mood.map(String::from),
                created_at: at,
            })
            .unwrap();
        }
        let entries = s.journal_entries().unwrap();
        assert_eq!(
            entries.iter().map(|e| e.created_at).collect::<Vec<_>>(),
            [30, 20, 10],
            "newest first"
        );
        assert_eq!(entries[0].mood.as_deref(), Some("🙂"));

        assert!(s.remove_journal_entry("00000000000000000020-bbbb").unwrap());
        assert!(!s.remove_journal_entry("00000000000000000020-bbbb").unwrap());
        assert_eq!(s.journal_entries().unwrap().len(), 2);
    }

    #[test]
    fn journal_text_never_plaintext_at_rest() {
        // The journal holds the most sensitive words a user writes — prove the
        // entry body is ciphertext on disk, same guarantee as the nsec test.
        let dir = TempDir::new().unwrap();
        let secret_thought = "my-very-private-journal-thought-0123456789";
        {
            let s = EncryptedStore::open(dir.path(), "passphrase").unwrap();
            s.save_journal_entry(&JournalEntry {
                id: "00000000000000000001-test".into(),
                text: secret_thought.into(),
                mood: None,
                created_at: 1,
            })
            .unwrap();
            s.flush().unwrap();
        }
        let mut leaked = false;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                if let Ok(bytes) = std::fs::read(&path) {
                    if bytes
                        .windows(secret_thought.len())
                        .any(|w| w == secret_thought.as_bytes())
                    {
                        leaked = true;
                    }
                }
            }
        }
        assert!(!leaked, "journal text must never be written in plaintext");

        let s = EncryptedStore::open(dir.path(), "passphrase").unwrap();
        assert_eq!(s.journal_entries().unwrap()[0].text, secret_thought);
    }

    #[test]
    fn ledger_snapshot_roundtrip() {
        let (_d, s) = store();
        assert!(s.load_ledger_snapshot().unwrap().is_none());
        let snap = vec![10u8, 20, 30, 40];
        s.save_ledger_snapshot(&snap).unwrap();
        assert_eq!(s.load_ledger_snapshot().unwrap(), Some(snap));
    }

    #[test]
    fn ledger_state_roundtrip() {
        let (_d, s) = store();
        assert!(s.load_ledger_state().unwrap().is_none());
        let state = LedgerState {
            snapshot: vec![1u8, 2, 3, 4, 5],
            updated_at: 1_700_000_000,
        };
        s.save_ledger_state(&state).unwrap();
        assert_eq!(s.load_ledger_state().unwrap(), Some(state));
    }

    #[test]
    fn chitthi_cache_sorted_newest_first() {
        let (_d, s) = store();
        assert!(s.chitthi_cache().unwrap().is_empty());

        s.cache_chitthi(&Chitthi {
            id: "c1".into(),
            author_npub: "npub1a".into(),
            content: "older".into(),
            created_at: 100,
            reply_to: None,
        })
        .unwrap();
        s.cache_chitthi(&Chitthi {
            id: "c2".into(),
            author_npub: "npub1b".into(),
            content: "newer".into(),
            created_at: 200,
            reply_to: Some("c1".into()),
        })
        .unwrap();

        let feed = s.chitthi_cache().unwrap();
        assert_eq!(feed.len(), 2);
        assert_eq!(feed[0].content, "newer");
        assert_eq!(feed[1].content, "older");
        assert_eq!(
            s.get_chitthi("c2").unwrap().unwrap().reply_to.as_deref(),
            Some("c1")
        );
        assert!(s.remove_chitthi("c1").unwrap());
        assert_eq!(s.chitthi_cache().unwrap().len(), 1);
    }

    #[test]
    fn nsec_never_plaintext_at_rest() {
        // Milestone 3: prove the private key is AES-GCM ciphertext on disk when
        // persisted through the repository API (not just the raw KV layer).
        let dir = TempDir::new().unwrap();
        let secret = "nsec1averysecretprivatekeyvalue000000000000000000000000000000";
        {
            let s = EncryptedStore::open(dir.path(), "passphrase").unwrap();
            s.save_identity(&StoredIdentity::new(
                "npub1pub",
                secret,
                Some("primary".into()),
            ))
            .unwrap();
            s.flush().unwrap();
        }
        let mut leaked = false;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                if let Ok(bytes) = std::fs::read(&path) {
                    if bytes.windows(secret.len()).any(|w| w == secret.as_bytes()) {
                        leaked = true;
                    }
                }
            }
        }
        assert!(!leaked, "private nsec must never be written in plaintext");

        // And it round-trips correctly through the encryption pipeline.
        let s = EncryptedStore::open(dir.path(), "passphrase").unwrap();
        assert_eq!(s.load_identity().unwrap().unwrap().nsec, secret);
    }

    #[test]
    fn vault_cache_returns_all_sorted() {
        let (_d, s) = store();
        s.save_message(&StoredMessage {
            id: "m2".into(),
            peer_npub: "npub1x".into(),
            content: "second".into(),
            created_at: 20,
            outgoing: true,
            status: Some("sent".into()),
            reply_to: None,
        })
        .unwrap();
        s.save_message(&StoredMessage {
            id: "m1".into(),
            peer_npub: "npub1y".into(),
            content: "first".into(),
            created_at: 10,
            outgoing: false,
            status: None,
            reply_to: None,
        })
        .unwrap();
        let cache = s.vault_cache().unwrap();
        assert_eq!(cache.len(), 2);
        assert_eq!(cache[0].content, "first");
    }

    #[test]
    fn message_status_only_moves_forward() {
        let (_d, s) = store();
        s.save_message(&StoredMessage {
            id: "m1".into(),
            peer_npub: "npub1x".into(),
            content: "hi".into(),
            created_at: 1,
            outgoing: true,
            status: Some("sent".into()),
            reply_to: None,
        })
        .unwrap();
        // sent → delivered → read all advance.
        assert!(s.set_message_status("m1", "delivered").unwrap());
        assert_eq!(
            s.get_message("m1").unwrap().unwrap().status.as_deref(),
            Some("delivered")
        );
        assert!(s.set_message_status("m1", "read").unwrap());
        // A late "delivered" can't downgrade "read"; idempotent no-ops return false.
        assert!(!s.set_message_status("m1", "delivered").unwrap());
        assert!(!s.set_message_status("m1", "read").unwrap());
        assert_eq!(
            s.get_message("m1").unwrap().unwrap().status.as_deref(),
            Some("read")
        );
        // Unknown message id is a clean false, not an error.
        assert!(!s.set_message_status("nope", "read").unwrap());
    }

    #[test]
    fn conversation_meta_crud() {
        let (_d, s) = store();
        assert!(s.get_conversation_meta("npub1x").unwrap().is_none());
        let meta = ConversationMeta {
            peer_npub: "npub1x".into(),
            state: "pending".into(),
            profile_shared: false,
            updated_at: 5,
        };
        s.set_conversation_meta(&meta).unwrap();
        assert_eq!(s.get_conversation_meta("npub1x").unwrap(), Some(meta));
        // Upsert to accepted.
        s.set_conversation_meta(&ConversationMeta {
            peer_npub: "npub1x".into(),
            state: "accepted".into(),
            profile_shared: true,
            updated_at: 6,
        })
        .unwrap();
        let got = s.get_conversation_meta("npub1x").unwrap().unwrap();
        assert_eq!(got.state, "accepted");
        assert!(got.profile_shared);
        assert_eq!(s.list_conversation_meta().unwrap().len(), 1);
        assert!(s.remove_conversation_meta("npub1x").unwrap());
        assert!(!s.remove_conversation_meta("npub1x").unwrap());
    }

    #[test]
    fn call_log_filtered_by_peer_and_newest_first() {
        let (_d, s) = store();
        for (id, peer, at, outcome) in [
            ("c1", "npub1a", 100u64, "connected"),
            ("c2", "npub1a", 300, "missed"),
            ("c3", "npub1b", 200, "declined"),
        ] {
            s.save_call_record(&CallRecord {
                id: id.into(),
                peer_npub: peer.into(),
                media: "audio".into(),
                incoming: false,
                outcome: outcome.into(),
                started_at: at,
                duration_secs: if outcome == "connected" { 42 } else { 0 },
            })
            .unwrap();
        }
        let with_a = s.calls_with("npub1a").unwrap();
        assert_eq!(with_a.len(), 2);
        assert_eq!(with_a[0].id, "c2", "newest first");
        assert_eq!(with_a[1].id, "c1");
        assert_eq!(s.all_calls().unwrap().len(), 3);
        // Overwrite (same id) updates in place rather than duplicating.
        s.save_call_record(&CallRecord {
            id: "c1".into(),
            peer_npub: "npub1a".into(),
            media: "video".into(),
            incoming: true,
            outcome: "connected".into(),
            started_at: 100,
            duration_secs: 99,
        })
        .unwrap();
        assert_eq!(s.calls_with("npub1a").unwrap().len(), 2);
        assert_eq!(
            s.calls_with("npub1a")
                .unwrap()
                .iter()
                .find(|c| c.id == "c1")
                .unwrap()
                .media,
            "video"
        );
    }
}
