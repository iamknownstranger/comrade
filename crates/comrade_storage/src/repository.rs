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

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{EncryptedStore, StorageError};

/// Wall clock in whole seconds, for the local-only timestamps
/// ([`StarredMessage::starred_at`], [`PinnedMessage::pinned_at`]) that order
/// those lists. Never used for anything a replay could exploit — those fields
/// come from the caller, as [`MessageReaction::created_at`] does.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Monotonic rank of a delivery status so receipts only ever move forward:
/// queued (0) < sent (1) < delivered (2) < read (3).
///
/// `queued` is the pre-relay state of a message sitting in the sender outbox,
/// and `failed` is its terminal counterpart; both rank 0, and `failed` gets an
/// explicit escape hatch in [`EncryptedStore::set_message_status`] because it
/// must be able to replace `sent` (a flush that ran out of attempts) without
/// ever overwriting a real delivered/read receipt. Unknown strings rank 0.
fn status_rank(status: &str) -> u8 {
    match status {
        "read" => 3,
        "delivered" => 2,
        "sent" => 1,
        _ => 0,
    }
}

/// Terminal local failure: the outbox gave up on this message.
const STATUS_FAILED: &str = "failed";

// ── Tree names ────────────────────────────────────────────────────────────────

const IDENTITY_TREE: &str = "identity";
const CONTACTS_TREE: &str = "contacts";
const CHITTHI_TREE: &str = "chitthi_cache";
const MESSAGES_TREE: &str = "vault_cache";
const LEDGER_TREE: &str = "ledger";
const JOURNAL_TREE: &str = "journal";
/// The Tara reflective-companion conversation (strictly local, like the journal).
const TARA_TREE: &str = "tara_companion";
/// Tasks named in a conversation, keyed by task id. Unlike the journal and the
/// Tara thread this one *has* a networked twin — the other party holds their own
/// row for the same id — but the copy here is still the only one this device
/// trusts, and it is sealed like everything else.
const KARYA_TREE: &str = "karya";
/// Per-peer conversation gate: request / accepted / blocked (message requests).
const CONVERSATIONS_TREE: &str = "conversation_meta";
/// Daily usage rollups for the attention mirror (strictly local, like the journal).
const ATTENTION_DAYS_TREE: &str = "attention_days";
/// Focus-session history (attention pillar — strictly local).
const FOCUS_SESSIONS_TREE: &str = "focus_sessions";
/// The single saved long-read (title, full text, reading position).
/// Superseded by [`READING_LIBRARY_TREE`]; kept so a vault written before the
/// library existed can migrate its one read instead of losing it
/// (`comrade_ui::ComradeRuntime::saved_reads` does the move).
const READING_TREE: &str = "reading_state";
/// The reading library: every saved long-read, keyed by id.
const READING_LIBRARY_TREE: &str = "reading_library";
/// Attention preferences: the user's own doom-app package list.
const ATTENTION_META_TREE: &str = "attention_meta";
/// Voice/video call log, keyed by call id.
const CALLS_TREE: &str = "call_log";
/// Last known presence of a comrade, keyed by their npub.
const PRESENCE_TREE: &str = "peer_presence";
/// Transfer preferences: the relay policy for peer-to-peer file handover.
const SHARE_META_TREE: &str = "share_meta";
/// Emoji reactions on cached messages, keyed `<target event id>:<reactor npub>`
/// — one row per person per message, so reacting again replaces rather than
/// stacks. See [`MessageReaction`].
const REACTIONS_TREE: &str = "message_reactions";
/// Topics named inside a conversation, keyed `<peer npub>:<slug>` — one row per
/// name per conversation. See [`StoredTopic`], and `comrade_core::topic` for why
/// the slug is the id.
const TOPICS_TREE: &str = "conversation_topics";
/// Which topic a thread is filed under, keyed by the thread root's event id.
/// See [`ThreadTopic`].
const THREAD_TOPICS_TREE: &str = "thread_topics";
/// Starred (bookmarked) messages, keyed `<peer npub>:<message id>` — one row
/// per starred message. Local device state only; see [`StarredMessage`].
const STARRED_TREE: &str = "starred_messages";
/// Pinned messages, keyed `<peer npub>:<message id>`, capped per conversation
/// at [`EncryptedStore::MAX_PINNED_PER_CONVERSATION`]. See [`PinnedMessage`].
const PINNED_TREE: &str = "pinned_messages";
/// "Delete for me" tombstones, keyed `<peer npub>:<message id>` — presence of
/// a row means the message is hidden on this device regardless of whether the
/// message row itself exists. See [`EncryptedStore::delete_message_for_me`]
/// for why a tombstone tree exists at all instead of just removing the row.
const DELETED_FOR_ME_TREE: &str = "deleted_for_me";

const IDENTITY_KEY: &str = "self";
const LEDGER_SNAPSHOT_KEY: &str = "hisab_kitab";
const LEDGER_STATE_KEY: &str = "hisab_kitab_state";
const READING_KEY: &str = "current";
const ATTENTION_PREFS_KEY: &str = "prefs";
const SHARE_PREFS_KEY: &str = "prefs";

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
    /// Whether the user chose this contact as a **comrade**: someone they
    /// announce their presence to, and want to be told about when that
    /// person comes online. Defaulted so contacts saved before the feature
    /// existed keep deserialising as "not a comrade" (opt-in, never
    /// retroactive — presence is disclosure, so it may only ever be turned
    /// on deliberately).
    #[serde(default)]
    pub comrade: bool,
}

/// What a comrade's last presence beacon said, and until when to believe it.
///
/// This is *soft* state: a device that dies sends no goodbye, so `online`
/// alone is never enough — every read must also check `expires_at` against
/// the current clock (`comrade_core::presence::is_online_at`). Stored per
/// peer so a green dot survives an app restart only for as long as the peer's
/// own claim does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerPresence {
    pub peer_npub: String,
    /// What the last beacon claimed. Combine with `expires_at`, never alone.
    pub online: bool,
    /// Send time (unix seconds) of the last beacon received from this peer —
    /// the honest "last seen" for the UI.
    pub last_seen_at: u64,
    /// When the last `online` claim stops being believable (unix seconds).
    pub expires_at: u64,
    /// Whether this peer has marked *us* as their comrade. Receiving any
    /// beacon proves it: beacons are only ever sent to comrades. Drives the
    /// "they chose you — choose them back to see when they're online" hint,
    /// which is the only way a mutual-by-construction model stays
    /// discoverable.
    #[serde(default)]
    pub peer_marked_us: bool,
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

/// One person's emoji reaction to one cached message.
///
/// Keyed on `(target_id, reactor_npub)` by [`EncryptedStore::set_reaction`], so
/// a person reacting a second time to the same message replaces their first —
/// there is exactly one reaction per person per message, which is what makes
/// "tap the emoji you already sent to take it back" expressible at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageReaction {
    /// Nostr event id (hex) of the message reacted to.
    pub target_id: String,
    /// The conversation this belongs to, so a thread's reactions can be read
    /// without walking every message. Denormalised on purpose: the target may be
    /// an event we have not cached (a reaction can outrun a backfill), and a
    /// reaction with nowhere to live would be lost.
    pub peer_npub: String,
    /// Who reacted, as an npub.
    pub reactor_npub: String,
    /// The emoji, or empty for a **tombstone**: a reaction its sender withdrew.
    /// Tombstones are kept rather than deleted so their timestamp survives to
    /// refuse a replay — see [`EncryptedStore::set_reaction`]. Readers
    /// ([`EncryptedStore::reactions_with`], [`EncryptedStore::reaction_by`])
    /// filter them out, so nothing above this layer sees one.
    pub emoji: String,
    /// When the reaction was sent (unix seconds). Used to reject replays.
    pub created_at: u64,
    /// Whether *this device* sent it — drives the "your reaction" highlight, and
    /// stored rather than derived because the store does not know the identity.
    pub outgoing: bool,
}

/// One message the user starred (bookmarked) for themselves.
///
/// Unlike [`MessageReaction`] this never travels anywhere: there is no wire
/// form, no peer copy, and no relay ever sees it, so
/// [`EncryptedStore::star_message`] has no clock to defend the way
/// [`EncryptedStore::set_reaction`] does — the caller's boolean always wins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarredMessage {
    pub peer_npub: String,
    pub message_id: String,
    /// When the star was placed (unix seconds) — the order the starred list
    /// reads in.
    pub starred_at: u64,
}

/// One message pinned to the top of a conversation.
///
/// Same standing as [`StarredMessage`]: Comrade has no protocol message that
/// announces a pin to the other person, so "pinned" means pinned on *this*
/// device, not on both ends of the conversation. Bounded per conversation by
/// [`EncryptedStore::MAX_PINNED_PER_CONVERSATION`] — see that constant for why
/// a flat cap rather than "as many as Telegram allows".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedMessage {
    pub peer_npub: String,
    pub message_id: String,
    /// When the pin was placed (unix seconds) — the order the pinned list
    /// reads in.
    pub pinned_at: u64,
}

/// A private journal entry — the wellbeing pillar's core record. Strictly
/// local: this tree is never synchronised, published or uploaded, and the only
/// copy of an entry lives sealed inside this encrypted store.
///
/// Since `comrade_core::note`, a user may *hand one entry to one person* from
/// the journal screen. That is a copy of the text into an ordinary end-to-end
/// DM, on a deliberate per-entry tap; the entry itself is not read by anything
/// else, not marked, and not changed. The guarantee this type carries is
/// therefore about the store and not about the words: nothing here moves
/// unless the person who wrote it moves it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Store key. Zero-padded-timestamp-prefixed so ids sort chronologically.
    pub id: String,
    /// What the entry is called, when the user gave it a name. Optional
    /// because most typed entries never had one and must keep reading after
    /// the field arrived — a note is its text first, and a title second.
    #[serde(default)]
    pub title: Option<String>,
    pub text: String,
    /// Optional self-reported mood marker (an emoji or short tag).
    #[serde(default)]
    pub mood: Option<String>,
    /// Set when the entry is a recording — spoken or filmed — rather than (or
    /// as well as) written words. See [`JournalRecording`] for what is and is
    /// not sealed.
    ///
    /// `alias = "video"` is load-bearing, not tidiness: this field shipped
    /// under that name when only video entries existed, and every entry already
    /// written to a phone spells it that way. Removing the alias silently drops
    /// the recording off those entries, which reads as the app having lost
    /// somebody's footage.
    #[serde(default, alias = "video")]
    pub recording: Option<JournalRecording>,
    pub created_at: u64,
}

/// A journal recording — a voice entry or a video entry — as the store knows it.
///
/// **The store holds the description, not the recording.** Everything in this
/// struct is sealed with the rest of the entry — the title, the length, when it
/// was taken — but the media file itself is far too large to keep as a redb
/// value, so it lives on the frontend's own disk and this record only names it.
///
/// That split is deliberate and it has a cost worth stating plainly rather than
/// leaving to be discovered: the words are protected by the vault passcode and
/// the recording is not. The file is protected by the platform's app sandbox
/// and whatever full-disk encryption the device has — real protection, but a
/// different and weaker promise than the one the rest of the journal makes.
/// Tracked as AUDIT J-1; nothing in this crate may describe the recording as
/// sealed until that closes.
///
/// [`mime`](Self::mime) is what says whether this is audio or video, and it is
/// the only thing that does. There is deliberately no second `kind` field: two
/// sources of truth for one fact is two sources that can disagree, and the
/// frontends already have to read the mime to choose a player.
///
/// [`file_name`](Self::file_name) is a bare name and never a path. The
/// directory is the frontend's decision (`filesDir/journal-videos` and
/// `filesDir/journal-audio` on Android), so a record does not go stale when an
/// OS moves an app's private storage — and so the store never learns a
/// filesystem layout it has no business knowing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRecording {
    /// Name of the file inside the frontend's directory for this kind.
    pub file_name: String,
    /// The recording's MIME type — `video/mp4` or `audio/aac`. Also the only
    /// thing that says which kind of recording this is.
    pub mime: String,
    /// How long the recording runs, in milliseconds. Zero when the frontend
    /// could not read it — a length of zero must render as "unknown", never
    /// as an empty clip.
    #[serde(default)]
    pub duration_ms: u64,
    /// Size of the file on disk in bytes, as it was when the entry was saved.
    #[serde(default)]
    pub size_bytes: u64,
}

/// One turn of the Tara reflective-companion conversation — the user's message
/// or Tara's reply. Strictly local, exactly like [`JournalEntry`]: never
/// published, never networked; the only copy lives sealed in this store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaraMessage {
    /// Store key. Zero-padded-timestamp-prefixed so ids sort chronologically.
    pub id: String,
    pub text: String,
    /// `true` for Tara's replies, `false` for the user's messages.
    pub from_tara: bool,
    /// Whether this turn tripped the distress detector (drives the crisis
    /// card the next time the thread renders). Defaulted so old rows read.
    #[serde(default)]
    pub crisis: bool,
    pub created_at: u64,
}

/// One day's usage rollup for the attention mirror (wellbeing pillar #5).
/// Strictly local, like [`JournalEntry`] — and deliberately only a rollup:
/// the raw per-app event stream is reduced on the frontend and dropped, so
/// what apps were opened, and when, is never persisted anywhere
/// (`docs/ATTENTION.md`, gate 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionDay {
    /// Store key: the local calendar date, `YYYY-MM-DD` — which also makes
    /// lexicographic key order chronological.
    pub date: String,
    pub screen_minutes: u32,
    pub pickups: u32,
    /// Minutes in the apps the user themself tagged as doom apps.
    pub doom_minutes: u32,
    pub updated_at: u64,
}

/// One focus session (attention pillar). Persisted from the moment it starts
/// so an app kill cannot make the history lie about a session that ran; a
/// session with no `outcome` yet is the (at most one) currently-running one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusSession {
    /// Store key. Zero-padded-timestamp-prefixed so ids sort chronologically.
    pub id: String,
    /// What the user said they'd give the time to. May be empty; never leaves
    /// the device (it can name anything — a person, a worry, a deadline).
    pub intent: String,
    pub planned_minutes: u32,
    pub started_at: u64,
    #[serde(default)]
    pub ended_at: Option<u64>,
    /// `"completed"`, `"abandoned"`, or `"lapsed"` — `None` while running.
    /// See `comrade_core::attention::FocusOutcome`.
    #[serde(default)]
    pub outcome: Option<String>,
}

/// One task named in a conversation — yours, or one a comrade asked of you.
///
/// `state` is a string rather than an enum for the same reason
/// [`FocusSession::outcome`] is: this crate stays free of `comrade_core`, so the
/// state machine lives there ([`comrade_core::karya::TaskState`]) and a row
/// carrying a state this build does not recognise is skipped on read rather than
/// failing the whole tree's deserialisation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTask {
    /// Store key — 128 random bits, hex, minted by whoever named the task, so
    /// both devices key the same row.
    pub id: String,
    /// What needs doing. Bounded and stripped before it gets here; see
    /// `comrade_core::karya::bound_text`.
    pub text: String,
    pub assigner_npub: String,
    /// `None` for a note to self, which never reaches a relay.
    #[serde(default)]
    pub assignee_npub: Option<String>,
    pub created_at: u64,
    /// `"open"`, `"done"`, `"declined"` or `"withdrawn"`. See
    /// `comrade_core::karya::TaskState`.
    pub state: String,
    /// When the state last moved; equals `created_at` until it does.
    pub updated_at: u64,
}

/// One topic named inside one conversation — a heading threads are filed under.
///
/// Keyed `(peer_npub, slug)`, which is why there is no separate id field: the
/// slug *is* the id, so both devices reach the same row from the same word with
/// no merge. `comrade_core::topic`'s module header has the full argument and
/// what it costs (a topic cannot be renamed into a different word).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTopic {
    /// The conversation, as the peer's npub. Topics are per-conversation:
    /// `#deposit` with two different people are two different subjects.
    pub peer_npub: String,
    /// Canonical slug — half the store key. See `comrade_core::topic::slugify`.
    pub slug: String,
    /// Display name, bounded and stripped before it gets here; see
    /// `comrade_core::topic::bound_name`.
    pub name: String,
    /// npub of whoever first named it.
    pub created_by: String,
    pub created_at: u64,
    /// Archived: still readable, out of the picker. Nothing deletes a topic —
    /// see `comrade_core::topic::Topic`.
    #[serde(default)]
    pub closed: bool,
    /// When `closed` last moved; equals `created_at` until it does.
    pub updated_at: u64,
}

/// One thread filed under one topic, keyed by the thread root's event id.
///
/// A separate tree rather than a column on [`StoredMessage`], for two reasons
/// that are both about arrival order. A thread can be filed **before its root
/// message is cached** — a reply arrives, the peer files the thread, and the
/// root turns up on the next backfill — and a row hanging off a message we do
/// not have would have nowhere to live. And filing must not rewrite a message
/// row, because `set_message_status`'s monotonic receipt guard is the only
/// writer that may touch one; a second writer racing it is how a `read` tick
/// gets rolled back to `sent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadTopic {
    /// Event id of the thread's root message — the store key.
    pub root_id: String,
    /// The conversation this thread belongs to, denormalised so a conversation's
    /// filings can be read without walking every message. Same reasoning as
    /// [`MessageReaction::peer_npub`], and the same reason: the root may be an
    /// event we have not cached yet.
    pub peer_npub: String,
    /// Slug of the topic it is filed under, or `None` for "taken out of
    /// wherever it was". Kept as a row rather than deleted so the timestamp
    /// survives to refuse a replay — exactly the reaction-tombstone argument in
    /// [`EncryptedStore::set_reaction`].
    pub slug: Option<String>,
    /// When the filing last moved. Refuses anything not newer.
    pub updated_at: u64,
}

/// The one saved long-read for the distraction-free reader: user-supplied
/// text, held whole (sealed like everything else) with the reading position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadingState {
    pub title: String,
    pub text: String,
    /// Index of the chunk the reader is on (chunking is recomputed from the
    /// text — see `comrade_core::attention::chunk_reading` — so the position
    /// is the only state worth storing).
    pub position: u32,
    pub updated_at: u64,
}

/// One saved long-read in the reading library — an article shared or pasted
/// into Comrade, held whole (sealed like everything else) with its own
/// reading position. What someone chose to keep and how far they got is
/// sensitive in exactly the way a journal is, which is why the library lives
/// in the vault and not in a preference file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedRead {
    /// Store key. Zero-padded-timestamp-prefixed so ids sort chronologically,
    /// like [`FocusSession::id`].
    pub id: String,
    pub title: String,
    /// Where it came from — the host of the first link in the text
    /// (`comrade_core::attention::reading_source`), or empty for plain pasted
    /// text. A label, never something to fetch.
    #[serde(default)]
    pub source: String,
    pub text: String,
    /// Index of the chunk the reader is on (chunking is recomputed from the
    /// text — see `comrade_core::attention::chunk_reading` — so the position
    /// is the only progress worth storing).
    pub position: u32,
    pub added_at: u64,
    pub updated_at: u64,
}

/// Attention preferences. Package names are stored *inside* the encrypted
/// vault on purpose: which apps someone considers their compulsion is
/// sensitive in exactly the way a journal is.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AttentionPrefs {
    #[serde(default)]
    pub doom_packages: Vec<String>,
}

/// Transfer preferences — currently just what this device does when the only
/// path to a peer is a relay.
///
/// The policy is stored as a string rather than as `comrade_core`'s
/// `RelayPolicy` because this crate deliberately has no `comrade_core`
/// dependency (see the crate docs on cycles); the mapping lives in
/// `comrade_ui`, the same way [`ConversationMeta::state`] is a string here and
/// an enum upstream.
///
/// Inside the encrypted vault because it is read only after unlock, and because
/// a device that relays bulk is a fact about how someone uses the app. An
/// unrecognised value must read as the safe policy, never as permission — see
/// `comrade_ui`'s mapping and its test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharePrefs {
    /// `"direct_only"` (default), `"under_bytes"`, `"ask_each_time"`, `"always"`.
    pub relay_policy: String,
    /// The allowance, meaningful only when `relay_policy` is `"under_bytes"`.
    #[serde(default)]
    pub relay_limit_bytes: u64,
}

impl Default for SharePrefs {
    fn default() -> Self {
        Self {
            relay_policy: "direct_only".to_string(),
            relay_limit_bytes: 0,
        }
    }
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
    /// `created_at` (unix seconds) of the newest message the user has actually
    /// seen in this thread; 0 when they have never opened it.
    ///
    /// Lives here rather than in a device preference because it is a property
    /// of the conversation, not of the device: it belongs beside the gate state
    /// in the encrypted store so every frontend — Android, desktop — opens the
    /// thread in the same place. `#[serde(default)]` so vaults written before
    /// this field existed still deserialize (as "never read", which is the
    /// honest answer for them).
    #[serde(default)]
    pub last_read_at: u64,
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

    /// Mark (or unmark) a contact as a comrade, creating the contact record if
    /// this key isn't saved yet — marking someone you've only ever chatted
    /// with must not require adding them by hand first. Every other field of
    /// an existing contact is preserved. Returns the stored contact.
    pub fn set_contact_comrade(&self, npub: &str, comrade: bool) -> Result<Contact, StorageError> {
        let mut contact = self.get_contact(npub)?.unwrap_or_else(|| Contact {
            npub: npub.to_string(),
            petname: String::new(),
            relays: Vec::new(),
            comrade: false,
        });
        contact.comrade = comrade;
        self.upsert_contact(&contact)?;
        Ok(contact)
    }

    /// Every contact the user has marked as a comrade.
    pub fn list_comrades(&self) -> Result<Vec<Contact>, StorageError> {
        Ok(self
            .list_contacts()?
            .into_iter()
            .filter(|c| c.comrade)
            .collect())
    }

    // Comrade presence -----------------------------------------------------------

    /// Record a peer's latest presence, keyed by npub (one row per peer).
    pub fn set_peer_presence(&self, presence: &PeerPresence) -> Result<(), StorageError> {
        self.put(PRESENCE_TREE, &presence.peer_npub, presence)
    }

    /// The last recorded presence for a peer, if any beacon ever arrived.
    pub fn get_peer_presence(&self, peer_npub: &str) -> Result<Option<PeerPresence>, StorageError> {
        self.get(PRESENCE_TREE, peer_npub)
    }

    /// Every recorded presence row, newest sighting first.
    pub fn list_peer_presence(&self) -> Result<Vec<PeerPresence>, StorageError> {
        let mut rows: Vec<PeerPresence> = self.values(PRESENCE_TREE)?;
        rows.sort_by_key(|p| std::cmp::Reverse(p.last_seen_at));
        Ok(rows)
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

    /// Advance an outgoing message's delivery `status` (queued → sent →
    /// delivered → read) in response to a receipt. Never downgrades — a late
    /// "delivered" receipt can't unset a "read" already recorded. Returns
    /// whether the row existed and changed.
    ///
    /// `failed` is the one non-monotonic transition: the sender outbox reaching
    /// its attempt cap must be able to mark a `queued` or `sent` message failed,
    /// so the UI stops showing it as in-flight. It still cannot overwrite a
    /// delivered/read receipt — the peer demonstrably has the message, whatever
    /// the local queue thinks.
    pub fn set_message_status(&self, id: &str, status: &str) -> Result<bool, StorageError> {
        let Some(mut msg) = self.get_message(id)? else {
            return Ok(false);
        };
        let current = msg.status.as_deref().unwrap_or("sent");
        let allowed = if status == STATUS_FAILED {
            current != STATUS_FAILED && status_rank(current) < status_rank("delivered")
        } else {
            status_rank(status) > status_rank(current)
        };
        if !allowed {
            return Ok(false);
        }
        msg.status = Some(status.to_string());
        self.save_message(&msg)?;
        Ok(true)
    }

    /// Delete a stored message. Used when a queued message is re-keyed to the
    /// event id a relay finally assigned it, so the conversation does not end up
    /// holding both rows. Returns whether a row existed.
    pub fn remove_message(&self, id: &str) -> Result<bool, StorageError> {
        self.delete(MESSAGES_TREE, id)
    }

    // Emoji reactions ---------------------------------------------------------

    /// Store key for one person's reaction to one message.
    fn reaction_key(target_id: &str, reactor_npub: &str) -> String {
        format!("{target_id}:{reactor_npub}")
    }

    /// Record (or replace, or withdraw) a reaction. Returns whether the *visible*
    /// state changed, so a caller only pushes a UI event when there is news.
    ///
    /// **Older reactions are refused.** A relay redelivers, and this app re-scans
    /// a two-day gift-wrap window on every launch, so a reaction that has already
    /// been superseded is not merely possible but expected on a normal cold
    /// start. Without the timestamp check, replaying an old 👍 after a newer 🔥
    /// would resurrect the 👍. Equal timestamps are refused too: within one second
    /// the store cannot order two reactions, and keeping what is already there is
    /// at least stable.
    ///
    /// **A withdrawal is stored, not deleted.** An empty `emoji` writes a
    /// tombstone row. Deleting looks tidier and is wrong: the timestamp goes with
    /// the row, so the very next replay of the withdrawn reaction would find an
    /// empty slot, pass the check vacuously, and bring back an emoji its sender
    /// had taken away. Keeping the row keeps the clock. Tombstones are bounded by
    /// "reactions ever withdrawn in this conversation" and carry one emoji-shaped
    /// hole each, so there is nothing to prune.
    pub fn set_reaction(&self, reaction: &MessageReaction) -> Result<bool, StorageError> {
        let key = Self::reaction_key(&reaction.target_id, &reaction.reactor_npub);
        let existing: Option<MessageReaction> = self.get(REACTIONS_TREE, &key)?;
        if let Some(prev) = &existing {
            if prev.created_at >= reaction.created_at {
                return Ok(false);
            }
        }
        let was = existing.map(|p| p.emoji).unwrap_or_default();
        self.put(REACTIONS_TREE, &key, reaction)?;
        // Advancing the timestamp is always worth writing; only a different
        // emoji (including to or from "none") is worth redrawing.
        Ok(was != reaction.emoji)
    }

    /// Every reaction in one conversation, oldest first. Withdrawn ones are
    /// tombstones (see [`Self::set_reaction`]) and are not returned.
    pub fn reactions_with(&self, peer_npub: &str) -> Result<Vec<MessageReaction>, StorageError> {
        let mut rows: Vec<MessageReaction> = self
            .values::<MessageReaction>(REACTIONS_TREE)?
            .into_iter()
            .filter(|r| r.peer_npub == peer_npub && !r.emoji.is_empty())
            .collect();
        rows.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.target_id.cmp(&b.target_id))
                .then_with(|| a.reactor_npub.cmp(&b.reactor_npub))
        });
        Ok(rows)
    }

    /// One person's current reaction to one message, if any. What "tapping the
    /// emoji you already sent takes it back" is decided against — so a tombstone
    /// reads as no reaction, which is exactly what it means.
    pub fn reaction_by(
        &self,
        target_id: &str,
        reactor_npub: &str,
    ) -> Result<Option<MessageReaction>, StorageError> {
        let row: Option<MessageReaction> =
            self.get(REACTIONS_TREE, &Self::reaction_key(target_id, reactor_npub))?;
        Ok(row.filter(|r| !r.emoji.is_empty()))
    }

    // Starred (bookmarked) messages ---------------------------------------------

    /// Store key for one message in one conversation, shared by the starred,
    /// pinned and delete-for-me trees — they all key the same way and none of
    /// them need a reactor/sender component the way [`REACTIONS_TREE`] does.
    fn peer_message_key(peer_npub: &str, message_id: &str) -> String {
        format!("{peer_npub}:{message_id}")
    }

    /// Star or un-star a message. Returns whether the stored state changed, so
    /// a caller only redraws when there is news — same contract as
    /// [`Self::set_reaction`], but there is no timestamp to compare: this is
    /// local device state with no sender to race against, so the caller's
    /// `starred` always wins.
    pub fn star_message(
        &self,
        peer_npub: &str,
        message_id: &str,
        starred: bool,
    ) -> Result<bool, StorageError> {
        let key = Self::peer_message_key(peer_npub, message_id);
        let was_starred = self.get::<StarredMessage>(STARRED_TREE, &key)?.is_some();
        if was_starred == starred {
            return Ok(false);
        }
        if starred {
            self.put(
                STARRED_TREE,
                &key,
                &StarredMessage {
                    peer_npub: peer_npub.to_string(),
                    message_id: message_id.to_string(),
                    starred_at: unix_now(),
                },
            )?;
        } else {
            self.delete(STARRED_TREE, &key)?;
        }
        Ok(true)
    }

    /// Every starred message in one conversation, oldest star first.
    pub fn starred_with(&self, peer_npub: &str) -> Result<Vec<StarredMessage>, StorageError> {
        let mut rows: Vec<StarredMessage> = self
            .values::<StarredMessage>(STARRED_TREE)?
            .into_iter()
            .filter(|s| s.peer_npub == peer_npub)
            .collect();
        rows.sort_by_key(|s| s.starred_at);
        Ok(rows)
    }

    /// Every starred message across every conversation, oldest star first —
    /// the "Starred Messages" screen, which (like Telegram's) reads across
    /// conversations rather than one at a time.
    pub fn all_starred(&self) -> Result<Vec<StarredMessage>, StorageError> {
        let mut rows: Vec<StarredMessage> = self.values(STARRED_TREE)?;
        rows.sort_by_key(|s| s.starred_at);
        Ok(rows)
    }

    // Pinned messages -------------------------------------------------------------

    /// Cap on pinned messages per conversation. Telegram allows many more, but
    /// nothing here pages the pinned list — [`Self::pinned_with`] is one
    /// `values` scan — and 50 is comfortably past "the handful of messages
    /// worth keeping at the top of a chat" without inviting the list to grow
    /// into a second unbounded feed. Raise it if the pinned-message UI ever
    /// grows pagination to match.
    pub const MAX_PINNED_PER_CONVERSATION: usize = 50;

    /// Pin a message. Refuses once the conversation already holds
    /// [`Self::MAX_PINNED_PER_CONVERSATION`] pins; unpin one first. Returns
    /// `false` (not an error) if the message was already pinned — pinning is
    /// idempotent, only the cap is enforced.
    pub fn pin_message(&self, peer_npub: &str, message_id: &str) -> Result<bool, StorageError> {
        let key = Self::peer_message_key(peer_npub, message_id);
        if self.get::<PinnedMessage>(PINNED_TREE, &key)?.is_some() {
            return Ok(false);
        }
        let current = self.pinned_with(peer_npub)?;
        if current.len() >= Self::MAX_PINNED_PER_CONVERSATION {
            return Err(StorageError::LimitExceeded(format!(
                "this conversation already has {} pinned messages — unpin one first",
                Self::MAX_PINNED_PER_CONVERSATION
            )));
        }
        self.put(
            PINNED_TREE,
            &key,
            &PinnedMessage {
                peer_npub: peer_npub.to_string(),
                message_id: message_id.to_string(),
                pinned_at: unix_now(),
            },
        )?;
        Ok(true)
    }

    /// Unpin a message. `true` if it was pinned.
    pub fn unpin_message(&self, peer_npub: &str, message_id: &str) -> Result<bool, StorageError> {
        self.delete(PINNED_TREE, &Self::peer_message_key(peer_npub, message_id))
    }

    /// Every pinned message in one conversation, oldest pin first — the order
    /// a pinned-messages bar scrolls through.
    pub fn pinned_with(&self, peer_npub: &str) -> Result<Vec<PinnedMessage>, StorageError> {
        let mut rows: Vec<PinnedMessage> = self
            .values::<PinnedMessage>(PINNED_TREE)?
            .into_iter()
            .filter(|p| p.peer_npub == peer_npub)
            .collect();
        rows.sort_by_key(|p| p.pinned_at);
        Ok(rows)
    }

    // Delete for me ---------------------------------------------------------------

    /// Hide one message on this device, permanently. [`Self::remove_message`]
    /// deletes the *row*, which is not enough on its own: a relay redelivers a
    /// gift wrap on a cold-start rescan, and the mesh replays frames it is
    /// unsure were received, so a message a backfill can resurrect needs a
    /// marker a backfill cannot un-write. This tombstone is checked
    /// independently of whether the message row exists at all — see
    /// [`Self::is_deleted_for_me`] — so a re-delivery finds the message hidden
    /// again the instant it lands, row or no row.
    pub fn delete_message_for_me(
        &self,
        peer_npub: &str,
        message_id: &str,
    ) -> Result<(), StorageError> {
        self.put(
            DELETED_FOR_ME_TREE,
            &Self::peer_message_key(peer_npub, message_id),
            &true,
        )
    }

    /// Whether a message carries a delete-for-me tombstone.
    pub fn is_deleted_for_me(
        &self,
        peer_npub: &str,
        message_id: &str,
    ) -> Result<bool, StorageError> {
        Ok(self
            .get::<bool>(
                DELETED_FOR_ME_TREE,
                &Self::peer_message_key(peer_npub, message_id),
            )?
            .unwrap_or(false))
    }

    // Topics and threads ------------------------------------------------------

    /// Store key for a topic: the conversation and the slug, in that order so a
    /// prefix scan over one conversation stays possible if this ever needs one.
    fn topic_key(peer_npub: &str, slug: &str) -> String {
        format!("{peer_npub}:{slug}")
    }

    /// Insert or update a topic.
    ///
    /// A plain upsert with no clock check, unlike [`Self::set_reaction`] and
    /// [`Self::set_thread_topic`], because the caller has already merged: the
    /// slug is the id, so a second creation of the same word is the *same*
    /// topic, and which display spelling and which `closed` flag survive is
    /// `comrade_core::topic::Topic::merge_name` / `set_closed`'s decision. A
    /// second timestamp rule here would be a second answer to that question.
    pub fn save_topic(&self, topic: &StoredTopic) -> Result<(), StorageError> {
        self.put(
            TOPICS_TREE,
            &Self::topic_key(&topic.peer_npub, &topic.slug),
            topic,
        )
    }

    /// One topic, or `None`.
    pub fn get_topic(
        &self,
        peer_npub: &str,
        slug: &str,
    ) -> Result<Option<StoredTopic>, StorageError> {
        self.get(TOPICS_TREE, &Self::topic_key(peer_npub, slug))
    }

    /// Every topic in one conversation, oldest first.
    ///
    /// Closed ones are included: the archive has to be reachable, and hiding it
    /// here would mean every caller that wants it re-reads the whole tree. The
    /// picker filters; the sheet does not.
    pub fn topics_with(&self, peer_npub: &str) -> Result<Vec<StoredTopic>, StorageError> {
        let mut rows: Vec<StoredTopic> = self
            .values::<StoredTopic>(TOPICS_TREE)?
            .into_iter()
            .filter(|t| t.peer_npub == peer_npub)
            .collect();
        rows.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.slug.cmp(&b.slug))
        });
        Ok(rows)
    }

    /// File a thread under a topic (or, with `slug` of `None`, take it out of
    /// wherever it was). Returns whether the *visible* filing actually changed,
    /// so a caller only pushes a UI event when there is news.
    ///
    /// **Older filings are refused**, for the reason [`Self::set_reaction`]
    /// spells out: relays redeliver and this app re-scans a two-day gift-wrap
    /// window on every launch, so replaying a stale filing is expected on a
    /// normal cold start rather than exotic. Equal timestamps are refused too —
    /// within one second the store cannot order two filings, and keeping what is
    /// already there is at least stable.
    pub fn set_thread_topic(&self, filing: &ThreadTopic) -> Result<bool, StorageError> {
        let existing: Option<ThreadTopic> = self.get(THREAD_TOPICS_TREE, &filing.root_id)?;
        if let Some(prev) = &existing {
            if prev.updated_at >= filing.updated_at {
                return Ok(false);
            }
        }
        let was = existing.and_then(|p| p.slug);
        self.put(THREAD_TOPICS_TREE, &filing.root_id, filing)?;
        Ok(was != filing.slug)
    }

    /// Where one thread is filed, or `None` if it never was — which reads the
    /// same as an unfiling, and means the same thing.
    pub fn thread_topic(&self, root_id: &str) -> Result<Option<String>, StorageError> {
        let row: Option<ThreadTopic> = self.get(THREAD_TOPICS_TREE, root_id)?;
        Ok(row.and_then(|r| r.slug))
    }

    /// Every filing in one conversation, including the unfiled tombstones — the
    /// caller is building a map and a `None` there is an answer, not an absence.
    pub fn thread_topics_with(&self, peer_npub: &str) -> Result<Vec<ThreadTopic>, StorageError> {
        let mut rows: Vec<ThreadTopic> = self
            .values::<ThreadTopic>(THREAD_TOPICS_TREE)?
            .into_iter()
            .filter(|t| t.peer_npub == peer_npub)
            .collect();
        rows.sort_by(|a, b| {
            a.updated_at
                .cmp(&b.updated_at)
                .then_with(|| a.root_id.cmp(&b.root_id))
        });
        Ok(rows)
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

    /// Record that the user has read this thread up to `seen_at`, and report
    /// where they had got to *before* this call.
    ///
    /// Read-and-advance in one step so a caller can position the thread at the
    /// first unread message without a read-then-write window in which the
    /// answer it is about to rely on has already been overwritten.
    ///
    /// The advance is monotonic: a thread opened while scrolled up, or an
    /// out-of-order delivery, must never drag the watermark backwards and
    /// resurrect messages the user has already seen.
    ///
    /// A peer with no gate record yet (an ordinary conversation, never routed
    /// through message requests) needs one created to hold the position;
    /// `state_if_new` is the gate state to stamp on it. It is a parameter
    /// rather than a literal here because the gate vocabulary belongs to the
    /// caller — this crate owns the record's shape, not its state machine.
    pub fn advance_read_position(
        &self,
        peer_npub: &str,
        seen_at: u64,
        now: u64,
        state_if_new: &str,
    ) -> Result<u64, StorageError> {
        let existing = self.get_conversation_meta(peer_npub)?;
        let previous = existing.as_ref().map(|m| m.last_read_at).unwrap_or(0);
        if seen_at <= previous {
            return Ok(previous);
        }
        let meta = match existing {
            Some(m) => ConversationMeta {
                last_read_at: seen_at,
                updated_at: now,
                ..m
            },
            None => ConversationMeta {
                peer_npub: peer_npub.to_string(),
                state: state_if_new.to_string(),
                profile_shared: false,
                last_read_at: seen_at,
                updated_at: now,
            },
        };
        self.set_conversation_meta(&meta)?;
        Ok(previous)
    }

    /// A peer's read position without changing it (0 when never read).
    pub fn read_position(&self, peer_npub: &str) -> Result<u64, StorageError> {
        Ok(self
            .get_conversation_meta(peer_npub)?
            .map(|m| m.last_read_at)
            .unwrap_or(0))
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

    /// One journal entry by id.
    pub fn journal_entry(&self, id: &str) -> Result<Option<JournalEntry>, StorageError> {
        self.get(JOURNAL_TREE, id)
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

    // Tara companion thread (local-only, never networked) ----------------------

    /// Persist one Tara conversation turn, keyed by its id.
    pub fn save_tara_message(&self, message: &TaraMessage) -> Result<(), StorageError> {
        self.put(TARA_TREE, &message.id, message)
    }

    /// The whole Tara thread, oldest-first (chat order).
    pub fn tara_messages(&self) -> Result<Vec<TaraMessage>, StorageError> {
        let mut messages: Vec<TaraMessage> = self.values(TARA_TREE)?;
        messages.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(messages)
    }

    /// Delete the entire Tara thread. Returns how many turns were removed.
    pub fn clear_tara_messages(&self) -> Result<u64, StorageError> {
        let mut removed = 0u64;
        for key in self.keys(TARA_TREE)? {
            if self.delete(TARA_TREE, &key)? {
                removed += 1;
            }
        }
        Ok(removed)
    }

    // Attention (wellbeing pillar #5 — strictly local, never networked) --------

    /// Persist (or overwrite) one day's usage rollup, keyed by its date.
    pub fn save_attention_day(&self, day: &AttentionDay) -> Result<(), StorageError> {
        self.put(ATTENTION_DAYS_TREE, &day.date, day)
    }

    /// All recorded usage days, newest first.
    pub fn attention_days(&self) -> Result<Vec<AttentionDay>, StorageError> {
        let mut days: Vec<AttentionDay> = self.values(ATTENTION_DAYS_TREE)?;
        days.sort_by(|a, b| b.date.cmp(&a.date));
        Ok(days)
    }

    /// Persist one focus session, keyed by its id (insert or update-in-place).
    pub fn save_focus_session(&self, session: &FocusSession) -> Result<(), StorageError> {
        self.put(FOCUS_SESSIONS_TREE, &session.id, session)
    }

    /// Every focus session, newest first.
    pub fn focus_sessions(&self) -> Result<Vec<FocusSession>, StorageError> {
        let mut sessions: Vec<FocusSession> = self.values(FOCUS_SESSIONS_TREE)?;
        sessions.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(sessions)
    }

    // Tasks (karya) -----------------------------------------------------------

    /// Persist one task, keyed by its id. Upserts, so applying a state change is
    /// the same call as creating it.
    pub fn save_task(&self, task: &StoredTask) -> Result<(), StorageError> {
        self.put(KARYA_TREE, &task.id, task)
    }

    /// One task by id.
    pub fn task(&self, id: &str) -> Result<Option<StoredTask>, StorageError> {
        self.get(KARYA_TREE, id)
    }

    /// Every task, newest first — the order a list wants, and the same ordering
    /// rule [`Self::focus_sessions`] uses so the two lists agree.
    pub fn tasks(&self) -> Result<Vec<StoredTask>, StorageError> {
        let mut tasks: Vec<StoredTask> = self.values(KARYA_TREE)?;
        tasks.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(tasks)
    }

    /// Forget one task. `true` if there was one.
    pub fn delete_task(&self, id: &str) -> Result<bool, StorageError> {
        self.delete(KARYA_TREE, id)
    }

    /// Persist the current long-read (there is exactly one slot).
    pub fn save_reading_state(&self, state: &ReadingState) -> Result<(), StorageError> {
        self.put(READING_TREE, READING_KEY, state)
    }

    /// The saved long-read, if any.
    pub fn load_reading_state(&self) -> Result<Option<ReadingState>, StorageError> {
        self.get(READING_TREE, READING_KEY)
    }

    /// Delete the saved long-read. Returns `true` if one existed.
    pub fn clear_reading_state(&self) -> Result<bool, StorageError> {
        self.delete(READING_TREE, READING_KEY)
    }

    /// Persist one saved read, keyed by its id (insert or update-in-place —
    /// moving the reading position is the same call as saving the text).
    pub fn save_saved_read(&self, read: &SavedRead) -> Result<(), StorageError> {
        self.put(READING_LIBRARY_TREE, &read.id, read)
    }

    /// The whole reading library, newest first — the order a list wants, the
    /// same ordering rule [`Self::focus_sessions`] uses.
    pub fn saved_reads(&self) -> Result<Vec<SavedRead>, StorageError> {
        let mut reads: Vec<SavedRead> = self.values(READING_LIBRARY_TREE)?;
        reads.sort_by(|a, b| b.added_at.cmp(&a.added_at).then_with(|| b.id.cmp(&a.id)));
        Ok(reads)
    }

    /// One saved read by id.
    pub fn saved_read(&self, id: &str) -> Result<Option<SavedRead>, StorageError> {
        self.get(READING_LIBRARY_TREE, id)
    }

    /// Forget one saved read. `true` if there was one.
    pub fn delete_saved_read(&self, id: &str) -> Result<bool, StorageError> {
        self.delete(READING_LIBRARY_TREE, id)
    }

    /// Persist the attention preferences (doom-app list).
    pub fn save_attention_prefs(&self, prefs: &AttentionPrefs) -> Result<(), StorageError> {
        self.put(ATTENTION_META_TREE, ATTENTION_PREFS_KEY, prefs)
    }

    /// The attention preferences; an empty default when never saved.
    pub fn load_attention_prefs(&self) -> Result<AttentionPrefs, StorageError> {
        Ok(self
            .get(ATTENTION_META_TREE, ATTENTION_PREFS_KEY)?
            .unwrap_or_default())
    }

    // Share --------------------------------------------------------------------

    /// Persist the transfer preferences (relay policy).
    pub fn save_share_prefs(&self, prefs: &SharePrefs) -> Result<(), StorageError> {
        self.put(SHARE_META_TREE, SHARE_PREFS_KEY, prefs)
    }

    /// The transfer preferences; the direct-only default when never saved.
    pub fn load_share_prefs(&self) -> Result<SharePrefs, StorageError> {
        Ok(self
            .get(SHARE_META_TREE, SHARE_PREFS_KEY)?
            .unwrap_or_default())
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

    fn topic(peer: &str, slug: &str, at: u64) -> StoredTopic {
        StoredTopic {
            peer_npub: peer.into(),
            slug: slug.into(),
            name: slug.into(),
            created_by: "npub_me".into(),
            created_at: at,
            closed: false,
            updated_at: at,
        }
    }

    fn filing(root: &str, peer: &str, slug: Option<&str>, at: u64) -> ThreadTopic {
        ThreadTopic {
            root_id: root.into(),
            peer_npub: peer.into(),
            slug: slug.map(str::to_string),
            updated_at: at,
        }
    }

    #[test]
    fn topics_are_per_conversation() {
        // `#deposit` with two people is two subjects; merging them would leak
        // one conversation's structure into the other.
        let (_d, s) = store();
        s.save_topic(&topic("npub_a", "deposit", 10)).unwrap();
        s.save_topic(&topic("npub_b", "deposit", 20)).unwrap();
        assert_eq!(s.topics_with("npub_a").unwrap().len(), 1);
        assert_eq!(s.topics_with("npub_a").unwrap()[0].created_at, 10);
        assert_eq!(s.topics_with("npub_b").unwrap()[0].created_at, 20);
        assert!(s.get_topic("npub_a", "repairs").unwrap().is_none());
    }

    #[test]
    fn saving_a_topic_twice_is_one_row() {
        // The slug is the id, so both people typing `/assign #deposit` before
        // either envelope lands must not end up filing into two topics.
        let (_d, s) = store();
        s.save_topic(&topic("npub_a", "deposit", 10)).unwrap();
        s.save_topic(&topic("npub_a", "deposit", 99)).unwrap();
        assert_eq!(s.topics_with("npub_a").unwrap().len(), 1);
    }

    #[test]
    fn a_thread_moves_between_topics_and_can_be_unfiled() {
        let (_d, s) = store();
        assert!(s
            .set_thread_topic(&filing("root", "npub_a", Some("deposit"), 10))
            .unwrap());
        assert_eq!(s.thread_topic("root").unwrap().as_deref(), Some("deposit"));
        assert!(s
            .set_thread_topic(&filing("root", "npub_a", Some("repairs"), 20))
            .unwrap());
        assert_eq!(s.thread_topic("root").unwrap().as_deref(), Some("repairs"));
        // Unfiling is a row, not a delete — see `ThreadTopic::slug`.
        assert!(s
            .set_thread_topic(&filing("root", "npub_a", None, 30))
            .unwrap());
        assert_eq!(s.thread_topic("root").unwrap(), None);
        assert_eq!(s.thread_topics_with("npub_a").unwrap().len(), 1);
    }

    #[test]
    fn a_replayed_filing_cannot_undo_a_newer_one() {
        // The two-day gift-wrap re-scan on every launch makes this the normal
        // case, not an exotic one.
        let (_d, s) = store();
        s.set_thread_topic(&filing("root", "npub_a", Some("deposit"), 10))
            .unwrap();
        s.set_thread_topic(&filing("root", "npub_a", Some("repairs"), 20))
            .unwrap();
        assert!(
            !s.set_thread_topic(&filing("root", "npub_a", Some("deposit"), 10))
                .unwrap(),
            "a redelivered older filing changes nothing"
        );
        assert_eq!(s.thread_topic("root").unwrap().as_deref(), Some("repairs"));
        // Equal timestamps too: the store cannot order two filings inside one
        // second, and stable beats arbitrary.
        assert!(!s
            .set_thread_topic(&filing("root", "npub_a", Some("deposit"), 20))
            .unwrap());
        assert_eq!(s.thread_topic("root").unwrap().as_deref(), Some("repairs"));
    }

    #[test]
    fn refiling_to_the_same_topic_is_not_news() {
        // Advancing the clock is worth writing; redrawing is not.
        let (_d, s) = store();
        s.set_thread_topic(&filing("root", "npub_a", Some("deposit"), 10))
            .unwrap();
        assert!(!s
            .set_thread_topic(&filing("root", "npub_a", Some("deposit"), 20))
            .unwrap());
        assert_eq!(
            s.thread_topics_with("npub_a").unwrap()[0].updated_at,
            20,
            "the clock still moved, so the next replay is refused"
        );
    }

    #[test]
    fn a_thread_can_be_filed_before_its_root_message_is_cached() {
        // The arrival order that put filings in their own tree: the peer files
        // a thread whose root turns up on the next backfill.
        let (_d, s) = store();
        s.set_thread_topic(&filing("not_yet_here", "npub_a", Some("deposit"), 10))
            .unwrap();
        assert!(s.get_message("not_yet_here").unwrap().is_none());
        assert_eq!(
            s.thread_topic("not_yet_here").unwrap().as_deref(),
            Some("deposit")
        );
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
            comrade: false,
        };
        let bob = Contact {
            npub: "npub1bob".into(),
            petname: "Bob".into(),
            relays: vec![],
            comrade: false,
        };
        s.upsert_contact(&alice).unwrap();
        s.upsert_contact(&bob).unwrap();
        assert_eq!(s.list_contacts().unwrap().len(), 2);
        assert_eq!(s.get_contact("npub1alice").unwrap(), Some(alice));
        assert!(s.remove_contact("npub1bob").unwrap());
        assert_eq!(s.list_contacts().unwrap().len(), 1);
    }

    #[test]
    fn comrade_flag_is_opt_in_preserves_the_contact_and_survives_old_rows() {
        let (_d, s) = store();
        s.upsert_contact(&Contact {
            npub: "npub1alice".into(),
            petname: "Alice".into(),
            relays: vec!["wss://relay.one".into()],
            comrade: false,
        })
        .unwrap();
        assert!(s.list_comrades().unwrap().is_empty(), "opt-in, not default");

        // Marking preserves the alias/relays the contact already had.
        let marked = s.set_contact_comrade("npub1alice", true).unwrap();
        assert!(marked.comrade);
        assert_eq!(marked.petname, "Alice");
        assert_eq!(marked.relays, vec!["wss://relay.one".to_string()]);
        assert_eq!(s.list_comrades().unwrap().len(), 1);

        // Marking a key with no contact record yet creates one (you can make
        // someone a comrade straight from a conversation).
        s.set_contact_comrade("npub1bob", true).unwrap();
        assert_eq!(s.list_comrades().unwrap().len(), 2);
        assert_eq!(s.get_contact("npub1bob").unwrap().unwrap().petname, "");

        // Unmarking leaves the contact in place — only presence stops.
        s.set_contact_comrade("npub1alice", false).unwrap();
        assert_eq!(s.list_comrades().unwrap().len(), 1);
        assert!(s.get_contact("npub1alice").unwrap().is_some());

        // A contact row written before the field existed reads as "not a
        // comrade" rather than failing to deserialise.
        let legacy: Contact =
            serde_json::from_str(r#"{"npub":"npub1old","petname":"Old"}"#).unwrap();
        assert!(!legacy.comrade);
        assert!(legacy.relays.is_empty());
    }

    #[test]
    fn peer_presence_crud_and_ordering() {
        let (_d, s) = store();
        assert!(s.get_peer_presence("npub1alice").unwrap().is_none());
        for (npub, online, seen, expires) in [
            ("npub1alice", true, 100u64, 580u64),
            ("npub1bob", false, 300, 300),
        ] {
            s.set_peer_presence(&PeerPresence {
                peer_npub: npub.into(),
                online,
                last_seen_at: seen,
                expires_at: expires,
                peer_marked_us: true,
            })
            .unwrap();
        }
        let rows = s.list_peer_presence().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].peer_npub, "npub1bob", "newest sighting first");

        // Same key upserts in place rather than duplicating.
        s.set_peer_presence(&PeerPresence {
            peer_npub: "npub1alice".into(),
            online: false,
            last_seen_at: 900,
            expires_at: 900,
            peer_marked_us: true,
        })
        .unwrap();
        assert_eq!(s.list_peer_presence().unwrap().len(), 2);
        let alice = s.get_peer_presence("npub1alice").unwrap().unwrap();
        assert!(!alice.online);
        assert_eq!(alice.last_seen_at, 900);
    }

    fn reaction(target: &str, reactor: &str, emoji: &str, at: u64) -> MessageReaction {
        MessageReaction {
            target_id: target.into(),
            peer_npub: "npub1alice".into(),
            reactor_npub: reactor.into(),
            emoji: emoji.into(),
            created_at: at,
            outgoing: reactor == "npub1me",
        }
    }

    #[test]
    fn a_second_reaction_from_one_person_replaces_the_first() {
        let (_d, s) = store();
        assert!(s
            .set_reaction(&reaction("e1", "npub1alice", "👍", 100))
            .unwrap());
        assert!(s
            .set_reaction(&reaction("e1", "npub1alice", "🔥", 200))
            .unwrap());
        let rows = s.reactions_with("npub1alice").unwrap();
        assert_eq!(
            rows.len(),
            1,
            "one reaction per person per message: {rows:?}"
        );
        assert_eq!(rows[0].emoji, "🔥");
    }

    #[test]
    fn different_people_reacting_to_one_message_all_count() {
        let (_d, s) = store();
        s.set_reaction(&reaction("e1", "npub1alice", "👍", 100))
            .unwrap();
        s.set_reaction(&reaction("e1", "npub1me", "🔥", 110))
            .unwrap();
        let rows = s.reactions_with("npub1alice").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            s.reaction_by("e1", "npub1me").unwrap().unwrap().emoji,
            "🔥",
            "our own reaction is what the toggle is decided against"
        );
        assert!(s.reaction_by("e1", "npub1me").unwrap().unwrap().outgoing);
    }

    #[test]
    fn a_replayed_older_reaction_cannot_overwrite_a_newer_one() {
        // Not hypothetical: the inbox re-scans a two-day gift-wrap window on
        // every launch, so a superseded reaction is redelivered on a normal cold
        // start.
        let (_d, s) = store();
        s.set_reaction(&reaction("e1", "npub1alice", "🔥", 200))
            .unwrap();
        assert!(
            !s.set_reaction(&reaction("e1", "npub1alice", "👍", 100))
                .unwrap(),
            "an older reaction is not news"
        );
        assert_eq!(
            s.reaction_by("e1", "npub1alice").unwrap().unwrap().emoji,
            "🔥"
        );
        // Same second, different emoji: unorderable, so what is already there wins.
        assert!(!s
            .set_reaction(&reaction("e1", "npub1alice", "😀", 200))
            .unwrap());
        assert_eq!(
            s.reaction_by("e1", "npub1alice").unwrap().unwrap().emoji,
            "🔥"
        );
    }

    #[test]
    fn withdrawing_a_reaction_hides_it_and_a_replay_cannot_bring_it_back() {
        let (_d, s) = store();
        s.set_reaction(&reaction("e1", "npub1alice", "🔥", 100))
            .unwrap();
        assert!(
            s.set_reaction(&reaction("e1", "npub1alice", "", 200))
                .unwrap(),
            "a withdrawal changes what is drawn"
        );
        assert!(s.reactions_with("npub1alice").unwrap().is_empty());
        assert!(s.reaction_by("e1", "npub1alice").unwrap().is_none());

        // The reason the withdrawal is a tombstone rather than a delete. Had the
        // row been removed, this replay would find an empty slot, pass the
        // timestamp check vacuously, and resurrect an emoji its sender took away.
        assert!(!s
            .set_reaction(&reaction("e1", "npub1alice", "🔥", 100))
            .unwrap());
        assert!(
            s.reactions_with("npub1alice").unwrap().is_empty(),
            "a withdrawn reaction must stay withdrawn"
        );

        // …and a genuinely newer reaction still lands, so withdrawing is not
        // permanent for the person who did it.
        assert!(s
            .set_reaction(&reaction("e1", "npub1alice", "👍", 300))
            .unwrap());
        assert_eq!(
            s.reaction_by("e1", "npub1alice").unwrap().unwrap().emoji,
            "👍"
        );
    }

    #[test]
    fn re_sending_the_same_emoji_is_not_news() {
        let (_d, s) = store();
        s.set_reaction(&reaction("e1", "npub1alice", "🔥", 100))
            .unwrap();
        assert!(
            !s.set_reaction(&reaction("e1", "npub1alice", "🔥", 200))
                .unwrap(),
            "nothing to redraw, so no UI event"
        );
        // The timestamp still advanced, which is what stops the 100 replay below.
        assert!(!s
            .set_reaction(&reaction("e1", "npub1alice", "😀", 150))
            .unwrap());
        assert_eq!(
            s.reaction_by("e1", "npub1alice").unwrap().unwrap().emoji,
            "🔥"
        );
    }

    #[test]
    fn reactions_are_scoped_to_their_conversation() {
        let (_d, s) = store();
        s.set_reaction(&reaction("e1", "npub1alice", "🔥", 100))
            .unwrap();
        let mut other = reaction("e9", "npub1bob", "👍", 110);
        other.peer_npub = "npub1bob".into();
        s.set_reaction(&other).unwrap();
        assert_eq!(s.reactions_with("npub1alice").unwrap().len(), 1);
        assert_eq!(s.reactions_with("npub1bob").unwrap().len(), 1);
        assert!(s.reactions_with("npub1nobody").unwrap().is_empty());
    }

    #[test]
    fn a_reaction_survives_arriving_before_the_message_it_is_about() {
        // The target event id is not a foreign key: a reaction can outrun the
        // backfill of the message it names, and dropping it then would lose it
        // for good.
        let (_d, s) = store();
        assert!(s.get_message("e-unseen").unwrap().is_none());
        assert!(s
            .set_reaction(&reaction("e-unseen", "npub1alice", "🔥", 100))
            .unwrap());
        assert_eq!(
            s.reactions_with("npub1alice").unwrap()[0].target_id,
            "e-unseen"
        );
    }

    // Starred, pinned, delete-for-me -------------------------------------------

    #[test]
    fn starring_and_unstarring_reports_whether_anything_changed() {
        let (_d, s) = store();
        assert!(s.star_message("npub1alice", "e1", true).unwrap());
        // Starring an already-starred message is not news.
        assert!(!s.star_message("npub1alice", "e1", true).unwrap());
        assert_eq!(s.starred_with("npub1alice").unwrap().len(), 1);
        assert!(s.star_message("npub1alice", "e1", false).unwrap());
        // Unstarring an already-unstarred message is not news either.
        assert!(!s.star_message("npub1alice", "e1", false).unwrap());
        assert!(s.starred_with("npub1alice").unwrap().is_empty());
    }

    #[test]
    fn all_starred_reads_across_every_conversation() {
        let (_d, s) = store();
        s.star_message("npub1alice", "e1", true).unwrap();
        s.star_message("npub1bob", "e9", true).unwrap();
        let all = s.all_starred().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(s.starred_with("npub1alice").unwrap().len(), 1);
        assert_eq!(s.starred_with("npub1bob").unwrap().len(), 1);
    }

    #[test]
    fn pinning_is_idempotent_and_unpinning_says_whether_one_was_pinned() {
        let (_d, s) = store();
        assert!(s.pin_message("npub1alice", "e1").unwrap());
        // Pinning twice is not an error and not news — no cap tripped either.
        assert!(!s.pin_message("npub1alice", "e1").unwrap());
        assert_eq!(s.pinned_with("npub1alice").unwrap().len(), 1);
        assert!(s.unpin_message("npub1alice", "e1").unwrap());
        assert!(!s.unpin_message("npub1alice", "e1").unwrap());
        assert!(s.pinned_with("npub1alice").unwrap().is_empty());
    }

    #[test]
    fn pins_are_capped_per_conversation() {
        // Telegram allows many more; MAX_PINNED_PER_CONVERSATION exists so the
        // pinned list — one flat `values` scan, no pagination — stays cheap to
        // read. This pins the number rather than letting it drift unnoticed.
        let (_d, s) = store();
        for i in 0..EncryptedStore::MAX_PINNED_PER_CONVERSATION {
            assert!(s.pin_message("npub1alice", &format!("e{i}")).unwrap());
        }
        let over_cap = s.pin_message("npub1alice", "one-too-many");
        assert!(
            matches!(over_cap, Err(StorageError::LimitExceeded(_))),
            "the cap must refuse, not silently drop: {over_cap:?}"
        );
        assert_eq!(
            s.pinned_with("npub1alice").unwrap().len(),
            EncryptedStore::MAX_PINNED_PER_CONVERSATION
        );
        // Unpinning one frees a slot.
        s.unpin_message("npub1alice", "e0").unwrap();
        assert!(s.pin_message("npub1alice", "one-too-many").unwrap());
    }

    #[test]
    fn pins_are_scoped_to_their_conversation() {
        let (_d, s) = store();
        s.pin_message("npub1alice", "e1").unwrap();
        s.pin_message("npub1bob", "e9").unwrap();
        assert_eq!(s.pinned_with("npub1alice").unwrap().len(), 1);
        assert_eq!(s.pinned_with("npub1bob").unwrap().len(), 1);
    }

    #[test]
    fn a_backfill_cannot_undelete_a_message_deleted_for_me() {
        // The whole reason `delete_message_for_me` is a tombstone and not just
        // `remove_message`: a relay's cold-start rescan of its gift-wrap window,
        // or a mesh replay, redelivers the same event id, and `save_message`
        // would otherwise happily recreate the row the user hid.
        let (_d, s) = store();
        let msg = StoredMessage {
            id: "e1".into(),
            peer_npub: "npub1alice".into(),
            content: "hi".into(),
            created_at: 100,
            outgoing: false,
            status: None,
            reply_to: None,
        };
        s.save_message(&msg).unwrap();
        assert!(!s.is_deleted_for_me("npub1alice", "e1").unwrap());

        s.remove_message("e1").unwrap();
        s.delete_message_for_me("npub1alice", "e1").unwrap();
        assert!(s.is_deleted_for_me("npub1alice", "e1").unwrap());

        // The backfill redelivers the same event id.
        s.save_message(&msg).unwrap();
        assert!(
            s.get_message("e1").unwrap().is_some(),
            "the row is back — the cache does not know about the tombstone"
        );
        assert!(
            s.is_deleted_for_me("npub1alice", "e1").unwrap(),
            "but the tombstone survives the replay, which is the point"
        );
    }

    #[test]
    fn delete_for_me_is_scoped_per_conversation_key() {
        // Same message id in two conversations (a forwarded message, say) is
        // two different tombstone slots — deleting it from one thread must not
        // hide it in the other.
        let (_d, s) = store();
        s.delete_message_for_me("npub1alice", "e1").unwrap();
        assert!(s.is_deleted_for_me("npub1alice", "e1").unwrap());
        assert!(!s.is_deleted_for_me("npub1bob", "e1").unwrap());
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
                title: None,
                text: text.into(),
                mood: mood.map(String::from),
                recording: None,
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
    fn a_recording_entry_roundtrips_for_either_kind() {
        let (_d, s) = store();
        for (id, file_name, mime) in [
            (
                "00000000000000000042-vvvv",
                "jv-1723800000000-abc123.mp4",
                "video/mp4",
            ),
            (
                "00000000000000000043-aaaa",
                "ja-1723800000000-abc124.aac",
                "audio/aac",
            ),
        ] {
            let entry = JournalEntry {
                id: id.into(),
                title: Some("The walk after the argument".into()),
                text: String::new(),
                mood: Some("😕".into()),
                recording: Some(JournalRecording {
                    file_name: file_name.into(),
                    mime: mime.into(),
                    duration_ms: 47_000,
                    size_bytes: 12_345_678,
                }),
                created_at: 42,
            };
            s.save_journal_entry(&entry).unwrap();
            assert_eq!(s.journal_entry(&entry.id).unwrap().as_ref(), Some(&entry));
        }
        // …and both come back through the list read, not just the keyed one:
        // the journal screen renders every kind from `journal_entries`.
        assert_eq!(s.journal_entries().unwrap().len(), 2);
    }

    #[test]
    fn journal_entry_written_before_titles_still_reads() {
        // Entries are serde_json values sealed one at a time, so a store that
        // predates `title`/`recording` holds rows without those keys. They must
        // keep opening: the alternative is a journal that silently loses every
        // entry someone wrote before this feature shipped.
        let (_d, s) = store();
        s.put(
            JOURNAL_TREE,
            "00000000000000000007-old",
            &serde_json::json!({
                "id": "00000000000000000007-old",
                "text": "written before the fields existed",
                "mood": "🙂",
                "created_at": 7,
            }),
        )
        .unwrap();
        let entries = s.journal_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "written before the fields existed");
        assert_eq!(entries[0].mood.as_deref(), Some("🙂"));
        assert_eq!(entries[0].title, None);
        assert_eq!(entries[0].recording, None);
    }

    #[test]
    fn a_video_entry_written_under_the_old_field_name_still_reads() {
        // `recording` shipped as `video`, so every entry already on a phone
        // spells it that way. Without `#[serde(alias = "video")]` this
        // deserialises to `recording: None` — the entry survives, the card
        // stops offering the recording, and it reads as the app having lost
        // somebody's footage rather than as a rename.
        let (_d, s) = store();
        s.put(
            JOURNAL_TREE,
            "00000000000000000008-vid",
            &serde_json::json!({
                "id": "00000000000000000008-vid",
                "title": "The walk after the argument",
                "text": "",
                "mood": "😕",
                "video": {
                    "file_name": "jv-1723800000000-abc123.mp4",
                    "mime": "video/mp4",
                    "duration_ms": 47_000,
                    "size_bytes": 12_345_678,
                },
                "created_at": 8,
            }),
        )
        .unwrap();
        let entries = s.journal_entries().unwrap();
        assert_eq!(entries.len(), 1);
        let recording = entries[0]
            .recording
            .as_ref()
            .expect("a pre-rename video entry must keep its recording");
        assert_eq!(recording.file_name, "jv-1723800000000-abc123.mp4");
        assert_eq!(recording.mime, "video/mp4");
        assert_eq!(recording.duration_ms, 47_000);
    }

    #[test]
    fn journal_recording_title_never_plaintext_at_rest() {
        // A video's *title* is as revealing as an entry's text — often more so,
        // because it is the one line someone writes to find the clip again. It
        // gets the same ciphertext-on-disk guarantee, proven the same way as
        // `journal_text_never_plaintext_at_rest`.
        let dir = TempDir::new().unwrap();
        let secret_title = "the-conversation-i-could-not-have-0123456789";
        {
            let s = EncryptedStore::open(dir.path(), "passphrase").unwrap();
            s.save_journal_entry(&JournalEntry {
                id: "00000000000000000001-test".into(),
                title: Some(secret_title.into()),
                text: String::new(),
                mood: None,
                recording: Some(JournalRecording {
                    file_name: "jv-1-a.mp4".into(),
                    mime: "video/mp4".into(),
                    duration_ms: 1_000,
                    size_bytes: 2_048,
                }),
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
                        .windows(secret_title.len())
                        .any(|w| w == secret_title.as_bytes())
                    {
                        leaked = true;
                    }
                }
            }
        }
        assert!(
            !leaked,
            "a video journal title must never hit disk in plaintext"
        );

        let s = EncryptedStore::open(dir.path(), "passphrase").unwrap();
        assert_eq!(
            s.journal_entries().unwrap()[0].title.as_deref(),
            Some(secret_title)
        );
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
                title: None,
                text: secret_thought.into(),
                mood: None,
                recording: None,
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
    fn tara_thread_crud_ordering_and_clear() {
        let (_d, s) = store();
        assert!(s.tara_messages().unwrap().is_empty());
        for (id, text, from_tara, crisis, at) in [
            (
                "00000000000000000020-bbbb",
                "that sounds heavy",
                true,
                false,
                20u64,
            ),
            ("00000000000000000010-aaaa", "rough day", false, false, 10),
            (
                "00000000000000000030-cccc",
                "I want to end it all",
                false,
                true,
                30,
            ),
        ] {
            s.save_tara_message(&TaraMessage {
                id: id.into(),
                text: text.into(),
                from_tara,
                crisis,
                created_at: at,
            })
            .unwrap();
        }
        let thread = s.tara_messages().unwrap();
        assert_eq!(
            thread.iter().map(|m| m.created_at).collect::<Vec<_>>(),
            [10, 20, 30],
            "chat order: oldest first"
        );
        assert!(thread[2].crisis);
        assert!(thread[1].from_tara);

        assert_eq!(s.clear_tara_messages().unwrap(), 3);
        assert!(s.tara_messages().unwrap().is_empty());
        assert_eq!(s.clear_tara_messages().unwrap(), 0);
    }

    #[test]
    fn tasks_round_trip_newest_first() {
        let (_d, s) = store();
        assert!(s.tasks().unwrap().is_empty());
        assert!(s.task("nope").unwrap().is_none());

        for (id, text, assignee, at) in [
            ("aa", "water the plants", None, 10u64),
            (
                "bb",
                "get some work done",
                Some("npub1bina".to_string()),
                30,
            ),
            ("cc", "ship the thing", Some("npub1bina".to_string()), 20),
        ] {
            s.save_task(&StoredTask {
                id: id.into(),
                text: text.into(),
                assigner_npub: "npub1ana".into(),
                assignee_npub: assignee,
                created_at: at,
                state: "open".into(),
                updated_at: at,
            })
            .unwrap();
        }
        let all = s.tasks().unwrap();
        assert_eq!(
            all.iter().map(|t| t.created_at).collect::<Vec<_>>(),
            [30, 20, 10],
            "list order: newest first"
        );
        assert_eq!(s.task("aa").unwrap().unwrap().text, "water the plants");
        assert!(
            all[2].assignee_npub.is_none(),
            "a note to self has no assignee"
        );

        // Saving the same id again is how a state change lands — an upsert, not
        // a second row, or a list would grow a duplicate every time.
        let mut done = s.task("bb").unwrap().unwrap();
        done.state = "done".into();
        done.updated_at = 99;
        s.save_task(&done).unwrap();
        assert_eq!(s.tasks().unwrap().len(), 3);
        assert_eq!(s.task("bb").unwrap().unwrap().state, "done");

        assert!(s.delete_task("bb").unwrap());
        assert!(!s.delete_task("bb").unwrap());
        assert_eq!(s.tasks().unwrap().len(), 2);
    }

    #[test]
    fn task_text_never_plaintext_at_rest() {
        // A task can name anything — a person, a diagnosis, a deadline nobody
        // else should learn. Same ciphertext-on-disk guarantee as the journal.
        let dir = TempDir::new().unwrap();
        let secret = "collect-the-biopsy-results-0123456789";
        {
            let s = EncryptedStore::open(dir.path(), "passphrase").unwrap();
            s.save_task(&StoredTask {
                id: "aa".into(),
                text: secret.into(),
                assigner_npub: "npub1ana".into(),
                assignee_npub: None,
                created_at: 1,
                state: "open".into(),
                updated_at: 1,
            })
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
        assert!(!leaked, "tasks must never be written in plaintext");

        let s = EncryptedStore::open(dir.path(), "passphrase").unwrap();
        assert_eq!(s.tasks().unwrap()[0].text, secret);
    }

    #[test]
    fn panic_wipe_reaches_the_task_tree() {
        // The wipe enumerates the database's actual tables, so this holds by
        // construction — pin it anyway, for the same reason the attention trees
        // are pinned: a wipe that left someone's task list behind would name the
        // people and the obligations they were trying to erase.
        let (_d, s) = store();
        s.save_task(&StoredTask {
            id: "aa".into(),
            text: "something private".into(),
            assigner_npub: "npub1ana".into(),
            assignee_npub: Some("npub1bina".into()),
            created_at: 1,
            state: "open".into(),
            updated_at: 1,
        })
        .unwrap();
        s.panic_wipe().unwrap();
        assert!(s.tasks().unwrap().is_empty());
    }

    #[test]
    fn tara_text_never_plaintext_at_rest() {
        // Tara receives the same class of disclosure as the journal — hold it
        // to the same ciphertext-on-disk guarantee.
        let dir = TempDir::new().unwrap();
        let secret_confession = "my-very-private-tara-confession-0123456789";
        {
            let s = EncryptedStore::open(dir.path(), "passphrase").unwrap();
            s.save_tara_message(&TaraMessage {
                id: "00000000000000000001-test".into(),
                text: secret_confession.into(),
                from_tara: false,
                crisis: false,
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
                        .windows(secret_confession.len())
                        .any(|w| w == secret_confession.as_bytes())
                    {
                        leaked = true;
                    }
                }
            }
        }
        assert!(!leaked, "tara messages must never be written in plaintext");

        let s = EncryptedStore::open(dir.path(), "passphrase").unwrap();
        assert_eq!(s.tara_messages().unwrap()[0].text, secret_confession);
    }

    #[test]
    fn attention_days_upsert_by_date_and_sort_newest_first() {
        let (_d, s) = store();
        assert!(s.attention_days().unwrap().is_empty());
        for (date, screen) in [
            ("2026-07-29", 210u32),
            ("2026-07-31", 95),
            ("2026-07-30", 180),
        ] {
            s.save_attention_day(&AttentionDay {
                date: date.into(),
                screen_minutes: screen,
                pickups: 40,
                doom_minutes: 30,
                updated_at: 1,
            })
            .unwrap();
        }
        let days = s.attention_days().unwrap();
        assert_eq!(
            days.iter().map(|d| d.date.as_str()).collect::<Vec<_>>(),
            ["2026-07-31", "2026-07-30", "2026-07-29"],
            "newest first"
        );
        // Same date upserts in place (the reader re-records today as it grows).
        s.save_attention_day(&AttentionDay {
            date: "2026-07-31".into(),
            screen_minutes: 120,
            pickups: 55,
            doom_minutes: 45,
            updated_at: 2,
        })
        .unwrap();
        let days = s.attention_days().unwrap();
        assert_eq!(days.len(), 3);
        assert_eq!(days[0].screen_minutes, 120);
    }

    #[test]
    fn focus_sessions_crud_ordering_and_update_in_place() {
        let (_d, s) = store();
        assert!(s.focus_sessions().unwrap().is_empty());
        for (id, at) in [
            ("00000000000000000020-bbbb", 20u64),
            ("00000000000000000010-aaaa", 10),
        ] {
            s.save_focus_session(&FocusSession {
                id: id.into(),
                intent: "draft the essay".into(),
                planned_minutes: 25,
                started_at: at,
                ended_at: None,
                outcome: None,
            })
            .unwrap();
        }
        let sessions = s.focus_sessions().unwrap();
        assert_eq!(sessions[0].started_at, 20, "newest first");

        // Finishing updates the same row rather than duplicating.
        s.save_focus_session(&FocusSession {
            id: "00000000000000000020-bbbb".into(),
            intent: "draft the essay".into(),
            planned_minutes: 25,
            started_at: 20,
            ended_at: Some(20 + 25 * 60),
            outcome: Some("completed".into()),
        })
        .unwrap();
        let sessions = s.focus_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].outcome.as_deref(), Some("completed"));

        // A row written before ended_at/outcome existed still loads.
        let legacy: FocusSession =
            serde_json::from_str(r#"{"id":"x","intent":"","planned_minutes":45,"started_at":5}"#)
                .unwrap();
        assert_eq!(legacy.outcome, None);
        assert_eq!(legacy.ended_at, None);
    }

    #[test]
    fn reading_state_roundtrip_and_clear() {
        let (_d, s) = store();
        assert!(s.load_reading_state().unwrap().is_none());
        let state = ReadingState {
            title: "Walden".into(),
            text: "I went to the woods because I wished to live deliberately.".into(),
            position: 0,
            updated_at: 9,
        };
        s.save_reading_state(&state).unwrap();
        assert_eq!(s.load_reading_state().unwrap(), Some(state.clone()));
        // Saving again overwrites the one slot.
        s.save_reading_state(&ReadingState {
            position: 3,
            ..state
        })
        .unwrap();
        assert_eq!(s.load_reading_state().unwrap().unwrap().position, 3);
        assert!(s.clear_reading_state().unwrap());
        assert!(!s.clear_reading_state().unwrap());
        assert!(s.load_reading_state().unwrap().is_none());
    }

    #[test]
    fn saved_reads_crud_ordering_and_position_update_in_place() {
        let (_d, s) = store();
        assert!(s.saved_reads().unwrap().is_empty());
        assert!(s.saved_read("missing").unwrap().is_none());
        for (id, at) in [
            ("00000000000000000020-bbbb", 20u64),
            ("00000000000000000010-aaaa", 10),
        ] {
            s.save_saved_read(&SavedRead {
                id: id.into(),
                title: "Essay".into(),
                source: "example.org".into(),
                text: "para one\n\npara two".into(),
                position: 0,
                added_at: at,
                updated_at: at,
            })
            .unwrap();
        }
        let reads = s.saved_reads().unwrap();
        assert_eq!(reads[0].added_at, 20, "newest first");

        // Turning a page updates the same row rather than duplicating it.
        let mut open = reads[1].clone();
        open.position = 1;
        open.updated_at = 30;
        s.save_saved_read(&open).unwrap();
        let reads = s.saved_reads().unwrap();
        assert_eq!(reads.len(), 2);
        assert_eq!(reads[1].position, 1);

        assert!(s.delete_saved_read("00000000000000000010-aaaa").unwrap());
        assert!(!s.delete_saved_read("00000000000000000010-aaaa").unwrap());
        assert_eq!(s.saved_reads().unwrap().len(), 1);

        // A row written before `source` existed still loads.
        let legacy: SavedRead = serde_json::from_str(
            r#"{"id":"x","title":"t","text":"b","position":0,"added_at":1,"updated_at":1}"#,
        )
        .unwrap();
        assert_eq!(legacy.source, "");
    }

    #[test]
    fn attention_prefs_default_empty_and_roundtrip() {
        let (_d, s) = store();
        assert!(s.load_attention_prefs().unwrap().doom_packages.is_empty());
        s.save_attention_prefs(&AttentionPrefs {
            doom_packages: vec!["com.example.shortvideo".into()],
        })
        .unwrap();
        assert_eq!(
            s.load_attention_prefs().unwrap().doom_packages,
            vec!["com.example.shortvideo".to_string()]
        );
    }

    #[test]
    fn reading_text_and_focus_intent_never_plaintext_at_rest() {
        // A focus intent can name anything (a person, a worry); the saved
        // long-read is whatever the user chose to keep. Same ciphertext-on-disk
        // guarantee as the journal, proven the same way.
        let dir = TempDir::new().unwrap();
        let secret_intent = "my-very-private-focus-intent-0123456789";
        let secret_text = "my-very-private-reading-text-0123456789";
        let secret_saved = "my-very-private-saved-article-0123456789";
        {
            let s = EncryptedStore::open(dir.path(), "passphrase").unwrap();
            s.save_focus_session(&FocusSession {
                id: "00000000000000000001-test".into(),
                intent: secret_intent.into(),
                planned_minutes: 25,
                started_at: 1,
                ended_at: None,
                outcome: None,
            })
            .unwrap();
            s.save_reading_state(&ReadingState {
                title: "t".into(),
                text: secret_text.into(),
                position: 0,
                updated_at: 1,
            })
            .unwrap();
            s.save_saved_read(&SavedRead {
                id: "00000000000000000002-test".into(),
                title: "t".into(),
                source: String::new(),
                text: secret_saved.into(),
                position: 0,
                added_at: 1,
                updated_at: 1,
            })
            .unwrap();
            s.flush().unwrap();
        }
        let mut leaked = false;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                if let Ok(bytes) = std::fs::read(&path) {
                    for secret in [secret_intent, secret_text, secret_saved] {
                        if bytes.windows(secret.len()).any(|w| w == secret.as_bytes()) {
                            leaked = true;
                        }
                    }
                }
            }
        }
        assert!(
            !leaked,
            "attention records must never be written in plaintext"
        );

        let s = EncryptedStore::open(dir.path(), "passphrase").unwrap();
        assert_eq!(s.focus_sessions().unwrap()[0].intent, secret_intent);
        assert_eq!(s.load_reading_state().unwrap().unwrap().text, secret_text);
        assert_eq!(s.saved_reads().unwrap()[0].text, secret_saved);
    }

    #[test]
    fn panic_wipe_reaches_the_attention_trees() {
        // The wipe enumerates the database's *actual* tables, so this holds by
        // construction — but pin it: a wipe that left a usage ledger or a
        // focus history behind would betray exactly the users it exists for.
        let (_d, s) = store();
        s.save_attention_day(&AttentionDay {
            date: "2026-07-31".into(),
            screen_minutes: 100,
            pickups: 10,
            doom_minutes: 5,
            updated_at: 1,
        })
        .unwrap();
        s.save_focus_session(&FocusSession {
            id: "00000000000000000001-test".into(),
            intent: "…".into(),
            planned_minutes: 25,
            started_at: 1,
            ended_at: None,
            outcome: None,
        })
        .unwrap();
        s.save_reading_state(&ReadingState {
            title: "t".into(),
            text: "body".into(),
            position: 0,
            updated_at: 1,
        })
        .unwrap();
        s.save_attention_prefs(&AttentionPrefs {
            doom_packages: vec!["com.example".into()],
        })
        .unwrap();
        s.save_saved_read(&SavedRead {
            id: "00000000000000000002-test".into(),
            title: "t".into(),
            source: String::new(),
            text: "body".into(),
            position: 0,
            added_at: 1,
            updated_at: 1,
        })
        .unwrap();

        s.panic_wipe().unwrap();

        assert!(s.attention_days().unwrap().is_empty());
        assert!(s.focus_sessions().unwrap().is_empty());
        assert!(s.load_reading_state().unwrap().is_none());
        assert!(s.load_attention_prefs().unwrap().doom_packages.is_empty());
        assert!(s.saved_reads().unwrap().is_empty());
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
    fn queued_messages_advance_and_can_fail_terminally() {
        let (_d, s) = store();
        let queued = StoredMessage {
            id: "q1".into(),
            peer_npub: "npub1x".into(),
            content: "are you around?".into(),
            created_at: 1,
            outgoing: true,
            status: Some("queued".into()),
            reply_to: None,
        };
        s.save_message(&queued).unwrap();

        // A queued message that reaches a relay advances to sent…
        assert!(s.set_message_status("q1", "sent").unwrap());
        // …and one the outbox gives up on is marked failed, so the UI stops
        // showing it as in flight.
        assert!(s.set_message_status("q1", "failed").unwrap());
        assert_eq!(
            s.get_message("q1").unwrap().unwrap().status.as_deref(),
            Some("failed")
        );
        assert!(
            !s.set_message_status("q1", "failed").unwrap(),
            "marking failed twice is a no-op"
        );
        // A receipt still wins over a local failure verdict.
        assert!(s.set_message_status("q1", "delivered").unwrap());
    }

    #[test]
    fn failed_never_overwrites_a_delivery_receipt() {
        let (_d, s) = store();
        s.save_message(&StoredMessage {
            id: "m1".into(),
            peer_npub: "npub1x".into(),
            content: "hi".into(),
            created_at: 1,
            outgoing: true,
            status: Some("read".into()),
            reply_to: None,
        })
        .unwrap();
        assert!(
            !s.set_message_status("m1", "failed").unwrap(),
            "the peer demonstrably has it, whatever the local queue thinks"
        );
        assert_eq!(
            s.get_message("m1").unwrap().unwrap().status.as_deref(),
            Some("read")
        );
    }

    #[test]
    fn remove_message_drops_the_row() {
        let (_d, s) = store();
        s.save_message(&StoredMessage {
            id: "queued:abc".into(),
            peer_npub: "npub1x".into(),
            content: "hi".into(),
            created_at: 1,
            outgoing: true,
            status: Some("queued".into()),
            reply_to: None,
        })
        .unwrap();
        assert!(s.remove_message("queued:abc").unwrap());
        assert!(s.get_message("queued:abc").unwrap().is_none());
        assert!(!s.remove_message("queued:abc").unwrap());
        assert!(s.messages_with("npub1x").unwrap().is_empty());
    }

    #[test]
    fn conversation_meta_crud() {
        let (_d, s) = store();
        assert!(s.get_conversation_meta("npub1x").unwrap().is_none());
        let meta = ConversationMeta {
            peer_npub: "npub1x".into(),
            state: "pending".into(),
            profile_shared: false,
            last_read_at: 0,
            updated_at: 5,
        };
        s.set_conversation_meta(&meta).unwrap();
        assert_eq!(s.get_conversation_meta("npub1x").unwrap(), Some(meta));
        // Upsert to accepted.
        s.set_conversation_meta(&ConversationMeta {
            peer_npub: "npub1x".into(),
            state: "accepted".into(),
            profile_shared: true,
            last_read_at: 0,
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
    fn read_position_starts_at_zero_and_reports_the_previous_value() {
        let (_d, s) = store();
        // Never opened.
        assert_eq!(s.read_position("npub1x").unwrap(), 0);
        // First open: nothing was read before, so the caller gets 0 and the
        // UI knows not to draw a divider.
        assert_eq!(
            s.advance_read_position("npub1x", 100, 1, "accepted")
                .unwrap(),
            0
        );
        assert_eq!(s.read_position("npub1x").unwrap(), 100);
        // Second open, later messages: the caller learns where they had been.
        assert_eq!(
            s.advance_read_position("npub1x", 300, 2, "accepted")
                .unwrap(),
            100
        );
        assert_eq!(s.read_position("npub1x").unwrap(), 300);
    }

    #[test]
    fn read_position_never_moves_backwards() {
        let (_d, s) = store();
        s.advance_read_position("npub1x", 300, 1, "accepted")
            .unwrap();
        // An out-of-order delivery, or a thread re-opened while scrolled up,
        // must not resurrect messages the user has already seen.
        assert_eq!(
            s.advance_read_position("npub1x", 100, 2, "accepted")
                .unwrap(),
            300
        );
        assert_eq!(s.read_position("npub1x").unwrap(), 300);
        // Re-reading the same watermark is a no-op that still reports it.
        assert_eq!(
            s.advance_read_position("npub1x", 300, 3, "accepted")
                .unwrap(),
            300
        );
    }

    #[test]
    fn advancing_a_read_position_preserves_the_gate_state() {
        let (_d, s) = store();
        s.set_conversation_meta(&ConversationMeta {
            peer_npub: "npub1x".into(),
            state: "accepted".into(),
            profile_shared: true,
            last_read_at: 0,
            updated_at: 1,
        })
        .unwrap();
        s.advance_read_position("npub1x", 100, 2, "pending")
            .unwrap();
        let got = s.get_conversation_meta("npub1x").unwrap().unwrap();
        // `state_if_new` must not touch an existing record — a peer whose
        // handle we already shared must not be re-shared, and an accepted
        // conversation must not slide back to pending.
        assert_eq!(got.state, "accepted");
        assert!(got.profile_shared);
        assert_eq!(got.last_read_at, 100);
        assert_eq!(got.updated_at, 2);
    }

    #[test]
    fn advancing_a_read_position_creates_a_record_when_there_is_none() {
        let (_d, s) = store();
        s.advance_read_position("npub1new", 50, 9, "accepted")
            .unwrap();
        let got = s.get_conversation_meta("npub1new").unwrap().unwrap();
        assert_eq!(got.state, "accepted");
        assert!(!got.profile_shared);
        assert_eq!(got.last_read_at, 50);
    }

    #[test]
    fn a_meta_record_written_before_read_positions_existed_still_loads() {
        let (_d, s) = store();
        // Exactly the JSON an older build wrote: no `last_read_at` key at all.
        // `#[serde(default)]` has to carry it, or every existing vault fails to
        // open its chat list on upgrade.
        let legacy =
            br#"{"peer_npub":"npub1old","state":"accepted","profile_shared":true,"updated_at":7}"#;
        s.put_bytes(CONVERSATIONS_TREE, "npub1old", legacy).unwrap();
        let got = s.get_conversation_meta("npub1old").unwrap().unwrap();
        assert_eq!(got.state, "accepted");
        assert!(got.profile_shared);
        assert_eq!(got.last_read_at, 0, "absent means never read");
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
