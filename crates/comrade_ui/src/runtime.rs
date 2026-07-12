/*!
 * comrade_ui::runtime — the async IPC bridge orchestrator.
 *
 * [`ComradeRuntime`] is the live "runtime context" the Command & Event Bridge
 * manages behind an `Arc<RwLock<…>>`. It is the single, framework-agnostic
 * aggregate that both the **Tauri desktop** shell (`#[tauri::command]` wrappers)
 * and the **Android** layer (`comrade_jni`'s uniffi-generated Kotlin bindings)
 * drive — keeping all real logic inside the workspace where it is unit-tested
 * and Send/Sync-checked. This crate itself stays uniffi-agnostic beyond
 * deriving `Record`/`Enum`/`Error` on its DTOs — `comrade_jni` is the only
 * place that wraps this type behind actual FFI scaffolding.
 *
 * It composes the sync view-model ([`UiService`] — workspace state, identity,
 * encrypted store) with the live Nostr engines (Sabha public feed, Vault E2E
 * DMs, Sakha couple ledger) and a [`tokio::sync::broadcast`] **event bus**.
 *
 * Naming: the IPC spec refers to this as the `RuntimeContext` app-state handle.
 * It is named `ComradeRuntime` here to avoid colliding with the pure, I/O-free
 * [`comrade_state::RuntimeContext`] (the workspace state machine) that it wraps.
 *
 * Design guarantees the bindings rely on:
 *  • Every method returns a typed [`UiError`] — no `.unwrap()`, no panics — so a
 *    failure becomes a `Promise.reject` (Tauri) or a thrown exception (uniffi).
 *  • Heavy work (relay connect, feed subscription, DM decryption) runs in
 *    spawned Tokio tasks via [`ComradeRuntime::spawn_event_loops`], never on the
 *    caller's UI thread.
 */

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use comrade_core::call::{
    ice_servers_for, new_call_id, parse_call_envelope, CallEnvelope, CallMediaKind, CallSignal,
    HangupReason, IceServer, IceStrategy,
};
use comrade_core::crypto::derive_media_key;
use comrade_core::dm::{parse_profile_share, parse_receipt, ProfileShare, Receipt, ReceiptKind};
use comrade_core::media::{
    build_file_metadata_event, encrypt_media, fetch_and_decrypt_media, FileMetadata,
    MAX_MEDIA_BYTES,
};
use comrade_core::saathi::SaathiEngine;
use comrade_core::sabha::{display_name_of, ChitthiCallback, SabhaEngine, DEFAULT_RELAYS};
use comrade_core::sakha::{LedgerEntry, SakhaEngine, SakhaSyncCallback};
use comrade_core::vault::{VaultCallback, VaultEngine, VaultMessage};
use nostr_sdk::{EventId, Metadata, PublicKey, ToBech32};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::warn;

use crate::{IdentityDto, UiError, UiService, UpiIntentDto, WorkspaceDto};

/// Capacity of the event bus. Slow consumers lag rather than block producers —
/// the relay loop never stalls waiting on the webview.
const EVENT_BUS_CAPACITY: usize = 256;

/// HKDF label binding the ECDH shared secret to media encryption.
const MEDIA_LABEL: &str = "comrade-media-v1";
/// Encrypted-store tree mapping a NIP-94 event id → local [`MediaRef`].
const MEDIA_REFS_TREE: &str = "comrade_media_refs";
/// Encrypted-store tree caching peers' published Kind-0 profiles
/// (npub → [`PeerProfileRecord`]). This is what lets the chat UI show
/// "@charlie" instead of a raw public key.
const PEER_PROFILES_TREE: &str = "peer_profiles";
/// Re-fetch a cached peer profile with a known name after this long (seconds).
const PROFILE_TTL_SECS: u64 = 24 * 60 * 60;
/// Re-fetch a cached record with **no** name after this long (seconds).
/// Short: an offline fetch is indistinguishable from "peer has no profile",
/// and it must not freeze a peer as key-only for a whole day.
const PROFILE_NEGATIVE_TTL_SECS: u64 = 5 * 60;
/// Upper bound on network fetches per [`ComradeRuntime::refresh_peer_profiles`] call.
const PROFILE_REFRESH_CAP: usize = 16;
/// Publish attempts before giving up until the next launch (see
/// [`publish_profile_with_retry`]).
const PUBLISH_ATTEMPTS: u32 = 5;
/// Encrypted-store tree for app settings that are not per-peer (e.g. the TURN
/// relay a user has configured for calls).
const SETTINGS_TREE: &str = "app_settings";
/// Settings key holding the optional [`TurnConfig`] for WebRTC calls.
const TURN_CONFIG_KEY: &str = "turn_server";
/// Encrypted-store tree holding the Sakha/Sakhi pairing record (there is only
/// ever one partner per device, but a tree keeps the storage shape uniform
/// with the rest of the repository layer).
const SAKHA_TREE: &str = "sakha_pairing";
const SAKHA_PAIRING_KEY: &str = "partner";

/// Conversation gate states (persisted in `ConversationMeta.state`).
const STATE_PENDING: &str = "pending";
const STATE_ACCEPTED: &str = "accepted";
const STATE_BLOCKED: &str = "blocked";

// ── Event DTOs (serialised across the IPC / FFI boundary) ────────────────────

/// A public Chitthi (Kind-1) as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ChitthiDto {
    pub id: String,
    pub author: String,
    pub content: String,
    pub created_at: u64,
    pub reply_to: Option<String>,
}

impl ChitthiDto {
    /// Build from a live Nostr Kind-1 event captured in the Tokio feed loop.
    pub fn from_event(event: &nostr_sdk::Event) -> Self {
        let author = event
            .pubkey
            .to_bech32()
            .unwrap_or_else(|_| event.pubkey.to_hex());
        Self {
            id: event.id.to_hex(),
            author,
            content: event.content.clone(),
            created_at: event.created_at.as_secs(),
            reply_to: None,
        }
    }

    /// Build from a row of the offline encrypted Chitthi cache.
    pub fn from_cached(c: &comrade_storage::Chitthi) -> Self {
        Self {
            id: c.id.clone(),
            author: c.author_npub.clone(),
            content: c.content.clone(),
            created_at: c.created_at,
            reply_to: c.reply_to.clone(),
        }
    }
}

/// An incoming encrypted direct message (Kind-4) as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct DirectMessageDto {
    pub id: String,
    pub sender: String,
    pub content: String,
    pub created_at: u64,
    pub upi_intents: Vec<UpiIntentDto>,
    /// Event id (hex) this message replies to, if any (for a quoted preview).
    pub reply_to: Option<String>,
}

impl From<VaultMessage> for DirectMessageDto {
    fn from(m: VaultMessage) -> Self {
        Self {
            id: m.event_id,
            sender: to_npub(&m.sender_pubkey),
            content: m.content,
            created_at: m.created_at,
            reply_to: m.reply_to,
            upi_intents: m
                .upi_intents
                .into_iter()
                .map(|i| UpiIntentDto {
                    amount_inr: i.amount_inr,
                    vpa: i.vpa,
                    uri: i.uri,
                })
                .collect(),
        }
    }
}

/// A WebRTC ICE server (STUN/TURN) for the frontend's `RTCConfiguration`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct IceServerDto {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

impl From<comrade_core::call::IceServer> for IceServerDto {
    fn from(s: comrade_core::call::IceServer) -> Self {
        Self {
            urls: s.urls,
            username: s.username,
            credential: s.credential,
        }
    }
}

/// Everything a frontend needs to begin negotiating a call: the call id, the
/// peer, the media kind, and the ICE servers to hand to `RTCPeerConnection`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct CallSessionDto {
    pub call_id: String,
    pub peer: String,
    pub media: String,
    pub ice_servers: Vec<IceServerDto>,
}

/// One incoming call-signaling payload (offer/answer/ICE/hangup/…) routed to
/// the frontend. `signal` is the actual [`CallSignal`] value (not a JSON blob)
/// so the WebRTC layer — and uniffi, which has no "arbitrary JSON" type — gets
/// a closed enum to `switch` on directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct CallSignalDto {
    pub call_id: String,
    pub peer: String,
    pub media: String,
    pub signal: CallSignal,
}

/// A voice/video call-log entry as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct CallRecordDto {
    pub id: String,
    pub peer: String,
    pub media: String,
    pub incoming: bool,
    pub outcome: String,
    pub started_at: u64,
    pub duration_secs: u64,
}

impl From<comrade_storage::CallRecord> for CallRecordDto {
    fn from(c: comrade_storage::CallRecord) -> Self {
        Self {
            id: c.id,
            peer: c.peer_npub,
            media: c.media,
            incoming: c.incoming,
            outcome: c.outcome,
            started_at: c.started_at,
            duration_secs: c.duration_secs,
        }
    }
}

/// A pending message request: a stranger's DM that is gated out of the chat
/// list until the user accepts it. Only the preview and timing are exposed —
/// the peer's chosen handle is not shared until they, in turn, accept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct MessageRequestDto {
    pub peer: String,
    pub last_message: String,
    pub last_at: u64,
}

/// This device's Sakha/Sakhi pairing state — lets the frontend show "pair
/// with your partner" or, for a returning paired couple, "continue as
/// {role}" without asking for the partner's key again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct SakhaStatusDto {
    pub paired: bool,
    pub partner_npub: Option<String>,
    /// `"sakha"` or `"sakhi"` — which role this device paired as, if known.
    pub role: Option<String>,
}

/// A persisted Sakha/Sakhi pairing, so a returning couple survives a
/// relaunch without re-exchanging keys. Never holds the derived symmetric
/// key — that is re-derived from the partner's pubkey plus our own secret
/// key every time [`SakhaEngine::pair_with`] runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SakhaPairingRecord {
    /// Partner's public key, hex-encoded.
    partner_pubkey_hex: String,
    /// `"sakha"` or `"sakhi"`.
    role: String,
}

/// A NIP-94 encrypted-media reference as the frontend sees it (no key material).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct MediaMessageDto {
    /// NIP-94 event id — the handle passed back to `download_and_decrypt_media`.
    pub event_id: String,
    pub url: String,
    pub mime_type: String,
    pub caption: String,
    /// Bech32/hex pubkey of the counterpart (sender for incoming).
    pub sender: String,
    pub created_at: u64,
    /// Size of the encrypted blob in bytes.
    pub size: u64,
    /// Whether *this device* sent it (mirrors `MessageDto::outgoing`) — needed
    /// to tell the two apart once media from both directions is merged into
    /// one history by [`ComradeRuntime::media_with`].
    pub outgoing: bool,
}

/// Decrypted media handed back to the frontend. Bytes are base64-encoded so the
/// IPC payload stays compact (the webview rebuilds a `Blob` from it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct MediaBytesDto {
    pub mime_type: String,
    pub base64: String,
}

/// Locally persisted pointer to an encrypted blob, keyed by NIP-94 event id.
/// Holds everything needed to *re-derive* the key — but never the key itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MediaRef {
    /// NIP-94 event id (hex) — duplicates the store key this row lives under,
    /// so a full-tree scan ([`ComradeRuntime::media_with`]) can rebuild a
    /// complete [`MediaMessageDto`] without a second round trip per row.
    /// Defaulted so refs written before this field are still readable.
    #[serde(default)]
    event_id: String,
    url: String,
    /// Hex pubkey of the other party (recipient if outgoing, sender if incoming).
    peer_pubkey: String,
    mime_type: String,
    caption: String,
    size: u64,
    /// SHA-256 of the *ciphertext* blob (NIP-94 `x`), verified before decrypt.
    /// Defaulted so refs written before this field are still readable.
    #[serde(default)]
    sha256_hex: String,
    outgoing: bool,
    created_at: u64,
}

/// The private envelope carried inside the E2E DM that points at the blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MediaEnvelope {
    /// Format marker / version; must equal 1.
    comrade_media: u8,
    event_id: String,
    url: String,
    mime: String,
    caption: String,
    size: u64,
    /// SHA-256 of the ciphertext (NIP-94 `x`) so the recipient can fail fast on
    /// a wrong/tampered blob. Defaulted for envelopes sent before this field.
    #[serde(default)]
    sha256_hex: String,
}

/// Detect and parse a Comrade media envelope out of a decrypted DM body.
fn parse_media_envelope(content: &str) -> Option<MediaEnvelope> {
    let env: MediaEnvelope = serde_json::from_str(content).ok()?;
    (env.comrade_media == 1).then_some(env)
}

/// Parse an npub (bech32) or hex public key.
fn parse_pubkey(s: &str) -> Result<PublicKey, UiError> {
    PublicKey::parse(s).map_err(|e| UiError::Engine(format!("invalid pubkey: {e}")))
}

/// Normalise a hex or bech32 public key to a canonical bech32 `npub` for the
/// frontend. Both the incoming and outgoing sides emit the same form, so the UI
/// can key conversations (and the couple panel, which is keyed by the pasted
/// npub) consistently. Falls back to the input unchanged if it cannot be parsed.
fn to_npub(pubkey: &str) -> String {
    PublicKey::parse(pubkey)
        .ok()
        .and_then(|pk| pk.to_bech32().ok())
        .unwrap_or_else(|| pubkey.to_string())
}

/// The local user's profile: the unforgeable identity (npub) plus the chosen
/// display handle. The handle is an alias, never an identifier — see
/// [`ComradeRuntime::set_username`] for the trust model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ProfileDto {
    pub npub: String,
    pub username: Option<String>,
}

/// A profile discovered via relay search. `npub` is the identity; `name` is a
/// self-declared, non-unique handle — the UI must always show both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct FoundProfileDto {
    pub npub: String,
    pub name: Option<String>,
    pub about: Option<String>,
}

/// A saved contact: an npub pinned on first add (trust-on-first-use) with a
/// local alias. A different key later claiming the same handle can never
/// silently replace this entry — contacts are keyed by npub, not by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ContactDto {
    pub npub: String,
    /// The *user-chosen* local alias (petname). Empty = none set.
    pub alias: String,
    /// The peer's own published @handle, from the local profile cache.
    /// Display precedence is alias → name → key; never trust name alone.
    pub name: Option<String>,
}

/// One entry of the chat list: a peer plus the newest message in the thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ConversationDto {
    /// Peer npub (canonical bech32) — the conversation key.
    pub peer: String,
    /// Saved contact alias for the peer, when one exists (user-chosen).
    pub alias: Option<String>,
    /// The peer's own published @handle, from the local profile cache.
    pub peer_name: Option<String>,
    pub last_message: String,
    pub last_at: u64,
    pub last_outgoing: bool,
}

/// Locally cached snapshot of a peer's published Kind-0 profile. `name` is a
/// self-declared, non-unique handle — a display aid, never an identifier.
/// Every field defaults so rows written by older builds keep deserialising
/// when the record grows (e.g. the planned avatar field).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerProfileRecord {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    about: Option<String>,
    /// When this record was last written (unix seconds) — drives the TTL.
    #[serde(default)]
    updated_at: u64,
}

/// A private journal entry as the frontend sees it. Journal entries are
/// **strictly local**: they are never published to a relay or any network —
/// the only copy lives sealed inside the encrypted store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct JournalEntryDto {
    pub id: String,
    pub text: String,
    /// Optional self-reported mood marker (an emoji or short tag).
    pub mood: Option<String>,
    pub created_at: u64,
}

impl From<comrade_storage::JournalEntry> for JournalEntryDto {
    fn from(e: comrade_storage::JournalEntry) -> Self {
        Self {
            id: e.id,
            text: e.text,
            mood: e.mood,
            created_at: e.created_at,
        }
    }
}

/// A single direct message in a conversation, from the offline history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct MessageDto {
    pub id: String,
    /// Peer npub the thread is keyed by (sender if incoming, recipient if outgoing).
    pub peer: String,
    pub content: String,
    pub created_at: u64,
    pub outgoing: bool,
    /// Delivery status of an outgoing message: `sent` / `delivered` / `read`.
    /// `None` for incoming messages (no ticks shown on the receiver's side).
    pub status: Option<String>,
    /// Event id (hex) this message replies to, if any.
    pub reply_to: Option<String>,
}

/// Live connectivity status of the off-grid Saathi mesh (mDNS discovery +
/// Gossipsub), for a persistent UI indicator — the one signal that still works
/// with zero cellular or relay reachability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct MeshStatusDto {
    /// Whether the mesh engine is running at all (the workspace is `OffGridTravel`).
    pub active: bool,
    /// Peers currently reachable over the local network via mDNS. `u64`, not
    /// `SaathiEngine::peer_count`'s native `usize` — uniffi has no FFI-safe
    /// representation for a platform-width integer.
    pub peer_count: u64,
}

/// A push event emitted by the background Tokio loops and forwarded across the
/// webview boundary (`window.emit`) or delivered to Android through a uniffi
/// callback interface (see `comrade_jni::BridgeEventListener`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Enum)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// A new public Chitthi (Kind-1) arrived on the Sabha timeline.
    IncomingChitthi(ChitthiDto),
    /// A new encrypted DM (Kind-4) was decrypted in the Vault inbox — from an
    /// already-accepted conversation.
    IncomingDirectMessage(DirectMessageDto),
    /// A new encrypted-media reference (NIP-94) arrived over the DM channel.
    IncomingMedia(MediaMessageDto),
    /// A call-signaling payload (offer/answer/ICE/hangup) arrived for the
    /// frontend's WebRTC layer.
    IncomingCallSignal(CallSignalDto),
    /// A stranger (not yet accepted) sent a DM — surfaced as a message request,
    /// not a chat. Accepting it moves the conversation into the chat list.
    IncomingMessageRequest(MessageRequestDto),
    /// A delivered/read receipt advanced the status of one or more of our
    /// outgoing messages in `peer`'s thread.
    MessageStatus {
        peer: String,
        message_ids: Vec<String>,
        status: String,
    },
    /// A peer shared (or updated) their display handle — e.g. by accepting our
    /// message request. The chat list should re-title their conversation.
    PeerProfileUpdated { peer: String, name: Option<String> },
    /// The off-grid mesh's connectivity changed: it started/stopped, or a peer
    /// joined/left via mDNS. Drives the persistent local-mesh status indicator.
    MeshStatusChanged(MeshStatusDto),
    /// The Sakha/Sakhi shared ledger changed — a partner's entry merged in
    /// over the sync channel. Carries the fresh, fully-merged ledger text.
    LedgerUpdated { ledger: String },
}

// ── Runtime ───────────────────────────────────────────────────────────────────

/// The live IPC runtime context. Wrap in `Arc<RwLock<ComradeRuntime>>` and hand
/// to Tauri's managed state / the JNI global so command handlers can reach the
/// core systems thread-safely.
pub struct ComradeRuntime {
    ui: UiService,
    sabha: Option<Arc<SabhaEngine>>,
    vault: Option<Arc<VaultEngine>>,
    sakha: Option<Arc<SakhaEngine>>,
    /// The off-grid mesh engine — running iff the active workspace is
    /// `OffGridTravel` (see [`Self::sync_saathi_lifecycle`]). Unlike the Nostr
    /// engines above, it is started and stopped on the fly rather than built
    /// once at unlock, since mDNS/Gossipsub only make sense while off-grid.
    saathi: Option<Arc<SaathiEngine>>,
    events: broadcast::Sender<BridgeEvent>,
    /// Guards [`spawn_event_loops`] against re-spawning the feed/DM tasks if it
    /// is called more than once. [`spawn_event_loops`]: ComradeRuntime::spawn_event_loops
    loops_spawned: bool,
    /// Guards [`spawn_sakha_sync_loop`] the same way `loops_spawned` guards
    /// the feed/DM loops. [`spawn_sakha_sync_loop`]: ComradeRuntime::spawn_sakha_sync_loop
    sakha_sync_spawned: bool,
    /// The one live profile-publish retry task. Replaced (old one aborted)
    /// whenever the handle changes, so a stale retry loop can never republish
    /// an old name over a new one.
    publish_task: Option<tokio::task::JoinHandle<()>>,
}

impl Default for ComradeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl ComradeRuntime {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(EVENT_BUS_CAPACITY);
        Self {
            ui: UiService::new(),
            sabha: None,
            vault: None,
            sakha: None,
            saathi: None,
            events,
            loops_spawned: false,
            sakha_sync_spawned: false,
            publish_task: None,
        }
    }

    /// Abort any in-flight profile-publish retry loop and start one for
    /// `name`. Last spawn wins — the relays only ever see the newest handle.
    fn spawn_profile_publish(&mut self, sabha: Arc<SabhaEngine>, name: String) {
        if let Some(task) = self.publish_task.take() {
            task.abort();
        }
        self.publish_task = Some(tokio::spawn(publish_profile_with_retry(sabha, name)));
    }

    // ── Event bus ──────────────────────────────────────────────────────────

    /// Subscribe to the push-event stream. The desktop layer forwards each event
    /// to the webview; the JNI layer drains it via `pollEvent`.
    pub fn subscribe_events(&self) -> broadcast::Receiver<BridgeEvent> {
        self.events.subscribe()
    }

    /// A clonable handle to the event bus, for hosts that want to inject events
    /// (e.g. forwarding mesh/Saathi traffic) onto the same stream.
    pub fn event_sender(&self) -> broadcast::Sender<BridgeEvent> {
        self.events.clone()
    }

    // ── Milestone 1 + 4: unlock the vault & start the engines ────────────────

    /// Open the encrypted storage repository with `passphrase`, restore (or
    /// seed) the identity, and construct the core Nostr engines.
    ///
    /// Engine construction is offline — relays are registered but not dialled —
    /// so this never blocks on the network. Call [`spawn_event_loops`] afterward
    /// to connect and begin streaming.
    ///
    /// The Argon2id key stretch + sled open run on Tokio's blocking pool, so a
    /// deliberately slow KDF never stalls a reactor thread that other tasks
    /// (relay loops, other IPC commands) are scheduled on.
    ///
    /// [`spawn_event_loops`]: ComradeRuntime::spawn_event_loops
    pub async fn unlock_vault(
        &mut self,
        path: impl AsRef<std::path::Path>,
        passphrase: &str,
    ) -> Result<IdentityDto, UiError> {
        // Idempotent: a second unlock (both bridges call this at startup) must
        // not rebuild the engines — that would orphan the running ones and,
        // with spawn_event_loops, duplicate the relay connections and event
        // loops. Return the already-loaded identity instead.
        if self.is_vault_unlocked() {
            return self.ui.current_identity().ok_or(UiError::NoIdentity);
        }

        let started = std::time::Instant::now();
        let store = {
            let path = path.as_ref().to_path_buf();
            let passphrase = passphrase.to_string();
            tokio::task::spawn_blocking(move || {
                comrade_storage::EncryptedStore::open(path, &passphrase)
            })
            .await
            .map_err(|e| UiError::Storage(format!("unlock task failed: {e}")))?
            .map_err(|e| UiError::Storage(e.to_string()))?
        };
        let kdf_ms = started.elapsed().as_millis() as u64;
        self.ui.attach_store(store);

        // Restore the saved identity, or seed and persist a fresh one so the
        // engines always have keys to sign with.
        let identity = match self.ui.load_identity()? {
            Some(id) => id,
            None => {
                let id = self.ui.generate_identity()?;
                self.ui.save_identity()?;
                id
            }
        };

        let keys = self.ui.identity_keys().ok_or(UiError::NoIdentity)?;
        let relays: Vec<String> = DEFAULT_RELAYS.iter().map(|r| r.to_string()).collect();

        self.sabha = Some(Arc::new(
            SabhaEngine::new(&keys)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?,
        ));
        self.vault = Some(Arc::new(
            VaultEngine::new(&keys, relays)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?,
        ));
        self.sakha = Some(Arc::new(
            SakhaEngine::new(&keys, vec![])
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?,
        ));
        self.restore_sakha_pairing().await;

        // Startup observability: the unlock is the gate every frontend waits
        // on, so record how long its two phases actually took.
        tracing::info!(
            kdf_ms,
            total_ms = started.elapsed().as_millis() as u64,
            "vault unlocked: store opened and engines built"
        );

        Ok(identity)
    }

    /// Whether the vault has been unlocked and the engines built.
    pub fn is_vault_unlocked(&self) -> bool {
        self.sabha.is_some()
    }

    // ── Milestone 2: connect & stream into the event bus ─────────────────────

    /// Connect the engines and spawn the background Tokio loops that capture
    /// incoming Chitthis (Kind-1) and encrypted DMs (Kind-4) and push them onto
    /// the event bus. Must be called from within a Tokio runtime context.
    ///
    /// All network and decryption work happens inside these spawned tasks,
    /// keeping the UI thread free (Architecture Quality Gate).
    ///
    /// Idempotent: calling it more than once (e.g. after a repeated unlock) is
    /// a no-op, so the feed/DM loops are spawned at most once per runtime.
    pub fn spawn_event_loops(&mut self) {
        if self.loops_spawned {
            return;
        }
        self.loops_spawned = true;

        // Re-publish the saved @handle on every launch. Kind-0 is replaceable
        // (newest wins) and the publish merges into the currently published
        // profile, so this is idempotent — and it heals identities whose
        // original publish was dropped (offline onboarding, relay hiccup),
        // which otherwise stay undiscoverable forever.
        if let (Some(sabha), Some(name)) = (self.sabha.clone(), self.ui.username()) {
            self.spawn_profile_publish(sabha, name);
        }

        if let Some(sabha) = self.sabha.clone() {
            let tx = self.events.clone();
            tokio::spawn(async move {
                sabha.connect().await;
                let cb: ChitthiCallback = Box::new(move |event| {
                    // A send error only means no subscribers are listening yet;
                    // the relay loop must keep running regardless.
                    let _ = tx.send(BridgeEvent::IncomingChitthi(ChitthiDto::from_event(&event)));
                });
                if let Err(e) = sabha.subscribe_chitthi_feed(3600, cb).await {
                    warn!("sabha feed loop ended: {e}");
                }
            });
        }

        if let Some(vault) = self.vault.clone() {
            let tx = self.events.clone();
            let store = self.ui.store_arc();
            // A clone of the vault handle the callback can use to send back
            // delivered receipts (the callback itself is sync; it spawns).
            let vault_cb = vault.clone();
            tokio::spawn(async move {
                vault.connect().await;
                let cb: VaultCallback = Box::new(move |msg| {
                    dispatch_incoming_dm(&vault_cb, store.as_ref(), &tx, msg);
                });
                if let Err(e) = vault.subscribe_inbox_with_callback(cb).await {
                    warn!("vault inbox loop ended: {e}");
                }
            });
        }

        // A pairing restored from a previous launch (see `restore_sakha_pairing`,
        // called from `unlock_vault`) should start syncing immediately too —
        // a fresh pairing via `pair_sakha` starts it itself.
        if self.sakha.as_ref().is_some_and(|s| s.is_paired()) {
            self.spawn_sakha_sync_loop();
        }
    }

    // ── Milestone 1: timeline + broadcast ────────────────────────────────────

    /// Load the Sabha timeline from the encrypted on-disk cache (offline-first).
    pub fn fetch_sabha_timeline(&self) -> Result<Vec<ChitthiDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let feed = store
            .chitthi_cache()
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(feed.iter().map(ChitthiDto::from_cached).collect())
    }

    /// Broadcast a Chitthi to the public relay set, optionally as a NIP-10 reply.
    /// On success the Chitthi is also cached locally for offline rendering.
    /// Returns the new event id (hex).
    pub async fn broadcast_chitthi(
        &self,
        content: &str,
        reply_to: Option<String>,
    ) -> Result<String, UiError> {
        let sabha = self.sabha.clone().ok_or(UiError::VaultLocked)?;

        let parent = match reply_to.as_deref() {
            Some(hex) => Some(
                EventId::from_hex(hex)
                    .map_err(|e| UiError::Engine(format!("invalid reply_to id: {e}")))?,
            ),
            None => None,
        };

        let id = sabha
            .broadcast_chitthi_reply(content, parent)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;

        // Best-effort: persist our own Chitthi to the encrypted cache so it
        // shows up in the offline timeline immediately.
        if let Some(store) = self.ui.store_ref() {
            let row = comrade_storage::Chitthi {
                id: id.to_hex(),
                author_npub: self
                    .ui
                    .current_identity()
                    .map(|i| i.npub)
                    .unwrap_or_default(),
                content: content.to_string(),
                created_at: now_secs(),
                reply_to,
            };
            if let Err(e) = store.cache_chitthi(&row).and_then(|()| store.flush()) {
                warn!("failed to cache outgoing chitthi: {e}");
            }
        }

        Ok(id.to_hex())
    }

    // ── Direct messages (Telegram-like chat flow) ────────────────────────────

    /// Send an end-to-end encrypted DM to `target` (npub or hex pubkey) and
    /// persist it to the offline history. Returns the stored message DTO.
    pub async fn send_dm(&self, target: &str, content: &str) -> Result<MessageDto, UiError> {
        self.send_dm_reply(target, content, None).await
    }

    /// Send an E2E DM, optionally as a reply to a prior message (`reply_to` is
    /// the replied message's event id, hex). Sending to someone accepts the
    /// conversation on our side and shares our @handle once (so they can title
    /// the chat) — the sender-side half of "username shared once engaged".
    pub async fn send_dm_reply(
        &self,
        target: &str,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<MessageDto, UiError> {
        if content.trim().is_empty() {
            return Err(UiError::Engine("message is empty".into()));
        }
        let vault = self.vault.clone().ok_or(UiError::VaultLocked)?;
        let peer = parse_pubkey(target)?;
        let id = vault
            .send_dm_reply(&peer, content, reply_to)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;

        let peer_npub = to_npub(target);
        let dto = MessageDto {
            id: id.to_hex(),
            peer: peer_npub.clone(),
            content: content.to_string(),
            created_at: now_secs(),
            outgoing: true,
            status: Some("sent".into()),
            reply_to: reply_to.map(str::to_string),
        };
        if let Some(store) = self.ui.store_ref() {
            let row = comrade_storage::StoredMessage {
                id: dto.id.clone(),
                peer_npub: dto.peer.clone(),
                content: dto.content.clone(),
                created_at: dto.created_at,
                outgoing: true,
                status: Some("sent".into()),
                reply_to: dto.reply_to.clone(),
            };
            if let Err(e) = store.save_message(&row).and_then(|()| store.flush()) {
                warn!("failed to persist outgoing DM: {e}");
            }
        }
        self.mark_accepted_and_share_profile(&peer_npub, &peer);
        Ok(dto)
    }

    /// The chat list: one entry per **accepted** peer, newest thread first, with
    /// saved contact aliases joined in. Pending message requests and blocked
    /// peers are excluded (see [`Self::message_requests`]).
    pub fn conversations(&self) -> Result<Vec<ConversationDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let aliases: std::collections::HashMap<String, String> = store
            .list_contacts()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(|c| (c.npub, c.petname))
            .collect();
        // Peers gated out of the chat list: pending requests + blocked. A peer
        // with no meta at all (e.g. history from before this feature) is treated
        // as an ordinary accepted conversation.
        let gated = self.gated_peers(store)?;

        let mut newest: std::collections::HashMap<String, comrade_storage::StoredMessage> =
            std::collections::HashMap::new();
        for msg in store
            .all_messages()
            .map_err(|e| UiError::Storage(e.to_string()))?
        {
            if gated.contains(&msg.peer_npub) {
                continue;
            }
            match newest.get(&msg.peer_npub) {
                Some(existing) if existing.created_at >= msg.created_at => {}
                _ => {
                    newest.insert(msg.peer_npub.clone(), msg);
                }
            }
        }

        let mut list: Vec<ConversationDto> = newest
            .into_values()
            .map(|m| ConversationDto {
                alias: aliases
                    .get(&m.peer_npub)
                    .and_then(|a| user_alias(a, &m.peer_npub)),
                peer_name: cached_peer_name(store, &m.peer_npub),
                peer: m.peer_npub,
                last_message: m.content,
                last_at: m.created_at,
                last_outgoing: m.outgoing,
            })
            .collect();
        list.sort_by_key(|c| std::cmp::Reverse(c.last_at));
        Ok(list)
    }

    /// Full offline message history with `peer` (npub or hex), oldest first —
    /// carrying each message's delivery status and reply target. Not gated, so
    /// a pending request's thread is viewable before it is accepted.
    pub fn messages_with(&self, peer: &str) -> Result<Vec<MessageDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let peer = to_npub(peer);
        let mut msgs: Vec<MessageDto> = store
            .messages_with(&peer)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(|m| MessageDto {
                id: m.id,
                peer: m.peer_npub,
                content: m.content,
                created_at: m.created_at,
                status: if m.outgoing {
                    Some(m.status.unwrap_or_else(|| "sent".into()))
                } else {
                    None
                },
                reply_to: m.reply_to,
                outgoing: m.outgoing,
            })
            .collect();
        msgs.sort_by_key(|m| m.created_at);
        Ok(msgs)
    }

    // ── Message requests (gate strangers; accept/block; profile on accept) ────

    /// Peers to hide from the chat list: those with a `pending` or `blocked`
    /// conversation gate. A peer with no gate record is shown (accepted).
    fn gated_peers(
        &self,
        store: &comrade_storage::EncryptedStore,
    ) -> Result<std::collections::HashSet<String>, UiError> {
        Ok(store
            .list_conversation_meta()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .filter(|m| m.state == STATE_PENDING || m.state == STATE_BLOCKED)
            .map(|m| m.peer_npub)
            .collect())
    }

    /// Pending message requests — strangers' DMs awaiting accept/block, newest
    /// first, with a preview of their latest message.
    pub fn message_requests(&self) -> Result<Vec<MessageRequestDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let pending: std::collections::HashSet<String> = store
            .list_conversation_meta()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .filter(|m| m.state == STATE_PENDING)
            .map(|m| m.peer_npub)
            .collect();
        if pending.is_empty() {
            return Ok(vec![]);
        }
        let mut newest: std::collections::HashMap<String, comrade_storage::StoredMessage> =
            std::collections::HashMap::new();
        for msg in store
            .all_messages()
            .map_err(|e| UiError::Storage(e.to_string()))?
        {
            if !pending.contains(&msg.peer_npub) {
                continue;
            }
            match newest.get(&msg.peer_npub) {
                Some(existing) if existing.created_at >= msg.created_at => {}
                _ => {
                    newest.insert(msg.peer_npub.clone(), msg);
                }
            }
        }
        let mut list: Vec<MessageRequestDto> = newest
            .into_values()
            .map(|m| MessageRequestDto {
                peer: m.peer_npub,
                last_message: m.content,
                last_at: m.created_at,
            })
            .collect();
        list.sort_by_key(|r| std::cmp::Reverse(r.last_at));
        Ok(list)
    }

    /// Accept a pending message request: mark the conversation accepted, share
    /// our @handle with the peer (this is the moment "the username is shared"),
    /// and acknowledge their messages as read. The conversation now appears in
    /// the chat list. Idempotent for an already-accepted peer.
    pub fn accept_request(&self, peer: &str) -> Result<(), UiError> {
        let peer_pk = parse_pubkey(peer)?;
        let peer_npub = peer_pk
            .to_bech32()
            .map_err(|e| UiError::Engine(e.to_string()))?;
        if self.ui.store_ref().is_none() {
            return Err(UiError::VaultLocked);
        }
        self.mark_accepted_and_share_profile(&peer_npub, &peer_pk);
        // Their messages are now read — acknowledge them.
        let ids = self.incoming_ids(&peer_npub);
        self.spawn_receipt(&peer_pk, ReceiptKind::Read, ids);
        Ok(())
    }

    /// Block a peer: hide them from the chat list and drop their future DMs in
    /// the inbox loop. The message history is left intact locally.
    pub fn block_conversation(&self, peer: &str) -> Result<(), UiError> {
        let peer_pk = parse_pubkey(peer)?;
        let peer_npub = peer_pk
            .to_bech32()
            .map_err(|e| UiError::Engine(e.to_string()))?;
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let meta = comrade_storage::ConversationMeta {
            peer_npub,
            state: STATE_BLOCKED.to_string(),
            profile_shared: false,
            updated_at: now_secs(),
        };
        store
            .set_conversation_meta(&meta)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Mark a conversation read: send a read receipt covering the peer's
    /// incoming messages. The frontend calls this when the thread is opened
    /// (accepted conversations only — we never ack a pending request).
    pub fn mark_conversation_read(&self, peer: &str) -> Result<(), UiError> {
        let peer_pk = parse_pubkey(peer)?;
        let peer_npub = peer_pk
            .to_bech32()
            .map_err(|e| UiError::Engine(e.to_string()))?;
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        // Only ack accepted conversations — acking a pending request would leak
        // that we saw it before deciding to accept.
        let accepted = store
            .get_conversation_meta(&peer_npub)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .map(|m| m.state == STATE_ACCEPTED)
            .unwrap_or(true); // no gate record ⇒ ordinary conversation
        if !accepted {
            return Ok(());
        }
        let ids = self.incoming_ids(&peer_npub);
        self.spawn_receipt(&peer_pk, ReceiptKind::Read, ids);
        Ok(())
    }

    /// Event ids of the peer's incoming (received) messages in this thread.
    fn incoming_ids(&self, peer_npub: &str) -> Vec<String> {
        self.ui
            .store_ref()
            .and_then(|s| s.messages_with(peer_npub).ok())
            .map(|msgs| {
                msgs.into_iter()
                    .filter(|m| !m.outgoing)
                    .map(|m| m.id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Record the conversation as accepted and, once, share our @handle with the
    /// peer over the encrypted channel. The share runs in the background so the
    /// user's action is never blocked on the network; the `profile_shared` flag
    /// flips only on a successful send, so a failed share retries next time.
    fn mark_accepted_and_share_profile(&self, peer_npub: &str, peer: &PublicKey) {
        let Some(store) = self.ui.store_arc() else {
            return;
        };
        let already_shared = store
            .get_conversation_meta(peer_npub)
            .ok()
            .flatten()
            .map(|m| m.profile_shared)
            .unwrap_or(false);
        let meta = comrade_storage::ConversationMeta {
            peer_npub: peer_npub.to_string(),
            state: STATE_ACCEPTED.to_string(),
            profile_shared: already_shared,
            updated_at: now_secs(),
        };
        if let Err(e) = store
            .set_conversation_meta(&meta)
            .and_then(|()| store.flush())
        {
            warn!("failed to record accepted conversation: {e}");
        }
        if already_shared {
            return;
        }
        let (Some(vault), username) = (self.vault.clone(), self.ui.username()) else {
            return;
        };
        let peer = *peer;
        let peer_npub = peer_npub.to_string();
        tokio::spawn(async move {
            let Ok(json) = ProfileShare::new(username).to_json() else {
                return;
            };
            if vault.send_dm(&peer, &json).await.is_ok() {
                let meta = comrade_storage::ConversationMeta {
                    peer_npub,
                    state: STATE_ACCEPTED.to_string(),
                    profile_shared: true,
                    updated_at: now_secs(),
                };
                let _ = store
                    .set_conversation_meta(&meta)
                    .and_then(|()| store.flush());
            }
        });
    }

    /// Fire-and-forget a receipt DM (delivered/read) to `peer`.
    fn spawn_receipt(&self, peer: &PublicKey, kind: ReceiptKind, message_ids: Vec<String>) {
        if message_ids.is_empty() {
            return;
        }
        let Some(vault) = self.vault.clone() else {
            return;
        };
        let peer = *peer;
        tokio::spawn(async move {
            if let Ok(json) = Receipt::new(kind, message_ids).to_json() {
                let _ = vault.send_dm(&peer, &json).await;
            }
        });
    }

    // ── Calls (voice/video · WebRTC signalled over the DM channel) ────────────

    /// The configured TURN relay, if any (see [`Self::set_turn_server`]).
    fn configured_turn_server(&self) -> Option<IceServer> {
        let store = self.ui.store_ref()?;
        let turn = store
            .get::<TurnConfig>(SETTINGS_TREE, TURN_CONFIG_KEY)
            .ok()??;
        (!turn.url.trim().is_empty())
            .then(|| IceServer::turn(turn.url, turn.username, turn.credential))
    }

    /// The ICE servers to hand a frontend `RTCPeerConnection`: public STUN by
    /// default, plus a user-configured TURN relay if one has been set.
    ///
    /// This is the "give me everything" list; [`Self::call_ice_servers_for`]
    /// exposes the STUN-first, TURN-on-failure strategy new calls should use.
    pub fn call_ice_servers(&self) -> Vec<IceServerDto> {
        ice_servers_for(
            IceStrategy::StunAndTurn,
            self.configured_turn_server().as_ref(),
        )
        .into_iter()
        .map(IceServerDto::from)
        .collect()
    }

    /// The ICE servers for one connection attempt under `strategy`
    /// (`"stun_only"` or `"stun_and_turn"`, see [`comrade_core::call::IceStrategy`]).
    ///
    /// Every call should start with `"stun_only"` (what [`Self::place_call`]
    /// uses): STUN is free and blind to the call, unlike a TURN relay. If the
    /// frontend's `RTCPeerConnection` reports its ICE connection state never
    /// reaches `connected`/`completed` — the CGNAT case a TURN server exists
    /// for — it calls this again with `"stun_and_turn"` and restarts ICE with
    /// the widened server list, now actually routing through the configured
    /// relay.
    pub fn call_ice_servers_for(&self, strategy: &str) -> Vec<IceServerDto> {
        let strategy = IceStrategy::from_str_lenient(strategy);
        ice_servers_for(strategy, self.configured_turn_server().as_ref())
            .into_iter()
            .map(IceServerDto::from)
            .collect()
    }

    /// Configure (or, with a blank `url`, clear) the TURN relay used for calls
    /// that cannot connect over STUN alone. Persisted in the encrypted store.
    pub fn set_turn_server(
        &self,
        url: &str,
        username: &str,
        credential: &str,
    ) -> Result<(), UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        if url.trim().is_empty() {
            store
                .delete(SETTINGS_TREE, TURN_CONFIG_KEY)
                .map_err(|e| UiError::Storage(e.to_string()))?;
        } else {
            let cfg = TurnConfig {
                url: url.trim().to_string(),
                username: username.to_string(),
                credential: credential.to_string(),
            };
            store
                .put(SETTINGS_TREE, TURN_CONFIG_KEY, &cfg)
                .map_err(|e| UiError::Storage(e.to_string()))?;
        }
        store.flush().map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Begin a call to `peer`: mint a call id and return the session the
    /// frontend needs (id, peer, media kind, ICE servers). No signal is sent
    /// yet — the frontend creates the WebRTC offer, then calls
    /// [`Self::send_call_signal`]. `media` is `"audio"` or `"video"`.
    ///
    /// `ice_servers` starts STUN-only (see [`Self::call_ice_servers_for`]) —
    /// if the connection can't complete, the frontend retries with
    /// `call_ice_servers_for("stun_and_turn")` before falling back to a
    /// `HangupReason::Failed`.
    pub fn place_call(&self, peer: &str, media: &str) -> Result<CallSessionDto, UiError> {
        if self.vault.is_none() {
            return Err(UiError::VaultLocked);
        }
        let _ = parse_pubkey(peer)?; // validate the target up front
        Ok(CallSessionDto {
            call_id: new_call_id(),
            peer: to_npub(peer),
            media: CallMediaKind::from_str_lenient(media).as_str().to_string(),
            ice_servers: self.call_ice_servers_for(IceStrategy::StunOnly.as_str()),
        })
    }

    /// Send one call-signaling payload to `peer` over the encrypted DM channel.
    /// `signal_json` is a serialised [`comrade_core::call::CallSignal`], e.g.
    /// `{"kind":"offer","sdp":"…"}` or `{"kind":"ice","candidate":"…"}`.
    pub async fn send_call_signal(
        &self,
        peer: &str,
        call_id: &str,
        media: &str,
        signal_json: &str,
    ) -> Result<(), UiError> {
        let vault = self.vault.clone().ok_or(UiError::VaultLocked)?;
        let peer_pk = parse_pubkey(peer)?;
        let signal: CallSignal = serde_json::from_str(signal_json)
            .map_err(|e| UiError::Engine(format!("invalid call signal: {e}")))?;
        let env = CallEnvelope::new(
            call_id.to_string(),
            CallMediaKind::from_str_lenient(media),
            signal,
        );
        let json = env.to_json().map_err(|e| UiError::Engine(e.to_string()))?;
        vault
            .send_dm(&peer_pk, &json)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;
        Ok(())
    }

    /// Convenience: send a `Hangup` signal with `reason` (`normal`, `declined`,
    /// `busy`, `missed`, `cancelled`, `failed`) to end/reject a call.
    pub async fn hangup_call(
        &self,
        peer: &str,
        call_id: &str,
        media: &str,
        reason: &str,
    ) -> Result<(), UiError> {
        let signal = CallSignal::Hangup {
            reason: HangupReason::from_str_lenient(reason),
        };
        let json = serde_json::to_string(&signal).map_err(|e| UiError::Engine(e.to_string()))?;
        self.send_call_signal(peer, call_id, media, &json).await
    }

    /// Persist a finished call to the call log. `outcome` is one of
    /// `connected` / `missed` / `declined` / `cancelled` / `busy` / `failed`.
    #[allow(clippy::too_many_arguments)]
    pub fn log_call(
        &self,
        peer: &str,
        call_id: &str,
        media: &str,
        incoming: bool,
        outcome: &str,
        started_at: u64,
        duration_secs: u64,
    ) -> Result<CallRecordDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let record = comrade_storage::CallRecord {
            id: call_id.to_string(),
            peer_npub: to_npub(peer),
            media: CallMediaKind::from_str_lenient(media).as_str().to_string(),
            incoming,
            outcome: outcome.to_string(),
            started_at: if started_at == 0 {
                now_secs()
            } else {
                started_at
            },
            duration_secs,
        };
        store
            .save_call_record(&record)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(record.into())
    }

    /// The call log, newest first — for a single `peer` (npub/hex) or, with
    /// `None`, across every peer.
    pub fn call_history(&self, peer: Option<&str>) -> Result<Vec<CallRecordDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let calls = match peer {
            Some(p) => store
                .calls_with(&to_npub(p))
                .map_err(|e| UiError::Storage(e.to_string()))?,
            None => store
                .all_calls()
                .map_err(|e| UiError::Storage(e.to_string()))?,
        };
        Ok(calls.into_iter().map(CallRecordDto::from).collect())
    }

    // ── Profile & contacts (username = alias, identity = keypair) ────────────

    /// The local profile: npub plus the chosen @handle (if set).
    pub fn profile(&self) -> Result<ProfileDto, UiError> {
        let id = self.ui.current_identity().ok_or(UiError::NoIdentity)?;
        Ok(ProfileDto {
            npub: id.npub,
            username: self.ui.username(),
        })
    }

    /// Claim a display handle for this identity.
    ///
    /// Trust model — why this cannot be globally unique: Comrade has no central
    /// registry, so nothing can stop a second keypair from publishing the same
    /// handle. The unforgeable identifier is the keypair (npub); the handle is
    /// a discovery alias published as Kind-0 metadata. Contacts pin the npub on
    /// first use, so a later "@same_handle" under a different key shows up as a
    /// different person and can never read or receive this thread's messages.
    ///
    /// The handle is persisted locally first; relay publication happens in a
    /// background task with retries (and again on every launch), so an offline
    /// claim still succeeds and becomes discoverable once a relay is reachable.
    pub async fn set_username(&mut self, handle: &str) -> Result<ProfileDto, UiError> {
        let handle = normalize_handle(handle)?;
        self.ui.set_username(handle.clone())?;
        if let Some(sabha) = self.sabha.clone() {
            // Never block (or fail) the claim on network state — but do keep
            // trying: a single dropped publish is exactly how a fresh identity
            // ends up unfindable by everyone else. Replaces (aborts) any
            // earlier retry loop so a stale name can't win the publish race.
            self.spawn_profile_publish(sabha, handle.clone());
        }
        self.profile()
    }

    /// Canonicalise a contact key: vault must be open, key must parse. One
    /// rule for every contact method, so junk input behaves identically
    /// across add/alias/remove on every bridge.
    fn canonical_contact_npub(&self, npub: &str) -> Result<String, UiError> {
        if self.ui.store_ref().is_none() {
            return Err(UiError::VaultLocked);
        }
        parse_pubkey(npub)?
            .to_bech32()
            .map_err(|e| UiError::Engine(e.to_string()))
    }

    /// Save a contact, pinned by npub — trust on first use. An empty `alias`
    /// leaves any existing alias untouched (so opening a chat with a known
    /// contact never wipes the name the user gave them); a non-empty alias
    /// sets it. Use [`Self::set_contact_alias`] to explicitly clear one.
    pub fn add_contact(&self, npub: &str, alias: &str) -> Result<ContactDto, UiError> {
        let canonical = self.canonical_contact_npub(npub)?;
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let alias = alias.trim();
        let existing = store
            .get_contact(&canonical)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        if alias.is_empty() {
            if let Some(contact) = existing {
                // Already pinned and nothing to change — don't rewrite the
                // record (this path runs on every chat open).
                return Ok(ContactDto {
                    name: cached_peer_name(store, &contact.npub),
                    alias: user_alias(&contact.petname, &contact.npub).unwrap_or_default(),
                    npub: contact.npub,
                });
            }
        }
        self.write_contact(canonical, alias.to_string())
    }

    /// Set (non-empty) or clear (empty) the user-chosen alias for a contact.
    /// Creates the contact if it doesn't exist yet.
    pub fn set_contact_alias(&self, npub: &str, alias: &str) -> Result<ContactDto, UiError> {
        let canonical = self.canonical_contact_npub(npub)?;
        self.write_contact(canonical, alias.trim().to_string())
    }

    fn write_contact(&self, npub: String, petname: String) -> Result<ContactDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let contact = comrade_storage::Contact {
            npub,
            petname,
            relays: vec![],
        };
        store
            .upsert_contact(&contact)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(ContactDto {
            name: cached_peer_name(store, &contact.npub),
            npub: contact.npub,
            alias: contact.petname,
        })
    }

    /// Remove a saved contact. Returns whether one existed. The message
    /// history with that peer is untouched — only the pin/alias goes.
    pub fn remove_contact(&self, npub: &str) -> Result<bool, UiError> {
        let canonical = self.canonical_contact_npub(npub)?;
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let removed = store
            .remove_contact(&canonical)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        store.flush().map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(removed)
    }

    /// All saved contacts, sorted by their display title (alias, else
    /// published name, else key).
    pub fn list_contacts(&self) -> Result<Vec<ContactDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let mut contacts: Vec<ContactDto> = store
            .list_contacts()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(|c| ContactDto {
                name: cached_peer_name(store, &c.npub),
                alias: user_alias(&c.petname, &c.npub).unwrap_or_default(),
                npub: c.npub,
            })
            .collect();
        contacts.sort_by_key(|c| {
            if !c.alias.is_empty() {
                c.alias.to_lowercase()
            } else {
                c.name.as_deref().unwrap_or(c.npub.as_str()).to_lowercase()
            }
        });
        Ok(contacts)
    }

    /// Best-effort people search by handle over NIP-50-capable relays. An empty
    /// result means no search relay knew the name — offer add-by-npub instead.
    ///
    /// A query that *is* a key (npub/hex) resolves that identity's profile
    /// directly instead of a name search. Every result is cached into the
    /// local profile store so the chat UI can name the peer immediately.
    pub async fn search_profiles(&self, query: &str) -> Result<Vec<FoundProfileDto>, UiError> {
        let sabha = self.sabha.clone().ok_or(UiError::VaultLocked)?;
        let query = query.trim().trim_start_matches('@');
        if query.is_empty() {
            return Ok(vec![]);
        }

        // Exact-key lookup: fetch that author's Kind-0 (name may be absent —
        // the key alone is still a valid, addressable result). Otherwise a
        // NIP-50 name search. Both branches share the DTO mapping and cache.
        let dtos: Vec<FoundProfileDto> = if let Ok(pk) = PublicKey::parse(query) {
            let meta = sabha
                .fetch_profile(&pk)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?;
            vec![found_profile_dto(&pk, meta.as_ref())]
        } else {
            sabha
                .search_profiles(query, 10)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?
                .into_iter()
                .map(|(pk, meta)| found_profile_dto(&pk, Some(&meta)))
                .collect()
        };
        self.cache_found_profiles(&dtos);
        Ok(dtos)
    }

    /// Persist discovered profiles into the local cache (best-effort) so the
    /// chat list can title peers by their handle without another fetch.
    fn cache_found_profiles(&self, found: &[FoundProfileDto]) {
        let Some(store) = self.ui.store_ref() else {
            return;
        };
        let now = now_secs();
        let mut wrote = false;
        for profile in found {
            if profile.name.is_none() {
                continue; // nothing displayable; don't shadow a future fetch
            }
            let record = PeerProfileRecord {
                name: profile.name.clone(),
                about: profile.about.clone(),
                updated_at: now,
            };
            wrote |= store_profile_record(store, &profile.npub, &record);
        }
        if wrote {
            if let Err(e) = store.flush() {
                warn!("failed to flush profile cache: {e}");
            }
        }
    }

    /// Detach a [`ProfileRefresher`] holding only the engine/store handles.
    ///
    /// The refresh does slow network work; callers behind the shared
    /// `Arc<RwLock<ComradeRuntime>>` (JNI, Tauri) MUST take this under a
    /// briefly-held guard, **drop the guard**, and then await
    /// [`ProfileRefresher::run`] — holding the runtime lock across relay
    /// round-trips stalls every other bridge call (AUDIT P2 discipline:
    /// no guard held across network awaits).
    pub fn profile_refresher(&self) -> Result<ProfileRefresher, UiError> {
        Ok(ProfileRefresher {
            sabha: self.sabha.clone().ok_or(UiError::VaultLocked)?,
            store: self.ui.store_arc().ok_or(UiError::VaultLocked)?,
        })
    }

    /// Convenience wrapper over [`Self::profile_refresher`] for callers that
    /// own the runtime directly (tests, CLI). Bridge code should use the
    /// refresher so the shared lock is not held across the network work.
    pub async fn refresh_peer_profiles(&self) -> Result<usize, UiError> {
        self.profile_refresher()?.run().await
    }

    // ── Journal (wellbeing pillar #1 — strictly local, never networked) ──────

    /// Save a new journal entry. `mood` is an optional self-reported marker.
    /// The entry never leaves the device: no relay, no network — only the
    /// encrypted store.
    pub fn add_journal_entry(
        &self,
        text: &str,
        mood: Option<&str>,
    ) -> Result<JournalEntryDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let text = text.trim();
        if text.is_empty() {
            return Err(UiError::Engine("journal entry is empty".into()));
        }
        let created_at = now_secs();
        let entry = comrade_storage::JournalEntry {
            id: journal_entry_id(created_at),
            text: text.to_string(),
            mood: mood
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(String::from),
            created_at,
        };
        store
            .save_journal_entry(&entry)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(entry.into())
    }

    /// All journal entries, newest first.
    pub fn journal_entries(&self) -> Result<Vec<JournalEntryDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        Ok(store
            .journal_entries()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(JournalEntryDto::from)
            .collect())
    }

    /// Delete a journal entry by id. Returns whether one existed.
    pub fn delete_journal_entry(&self, id: &str) -> Result<bool, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let removed = store
            .remove_journal_entry(id)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        store.flush().map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(removed)
    }

    // ── Encrypted media pipeline (NIP-94/96 · Blossom) ───────────────────────

    /// Encrypt `bytes` for `target_pubkey`, upload the opaque blob to Blossom,
    /// build a zero-knowledge NIP-94 reference, persist it locally, and deliver
    /// the reference privately over the E2E DM channel. Returns the media DTO.
    ///
    /// The AES key is derived from the ECDH shared secret, so it is never
    /// uploaded and never placed in the public event — the recipient re-derives
    /// it from their own private key and our pubkey.
    pub async fn upload_and_send_media(
        &self,
        target_pubkey: &str,
        bytes: Vec<u8>,
        mime_type: &str,
        caption: &str,
    ) -> Result<MediaMessageDto, UiError> {
        if bytes.len() > MAX_MEDIA_BYTES {
            return Err(UiError::Engine(format!(
                "media is {} bytes; the limit is {MAX_MEDIA_BYTES}",
                bytes.len()
            )));
        }
        let keys = self.ui.identity_keys().ok_or(UiError::NoIdentity)?;
        let peer = parse_pubkey(target_pubkey)?;
        let key = derive_media_key(keys.secret_key(), &peer, MEDIA_LABEL)
            .map_err(|e| UiError::Engine(e.to_string()))?;

        let (media, _secret) =
            encrypt_media(&bytes, mime_type, &key).map_err(|e| UiError::Engine(e.to_string()))?;
        let size = media.size as u64;
        let sha256_hex = media.sha256_hex.clone();

        // Upload ciphertext only — the host sees opaque bytes.
        let url = self.upload_blob(media.ciphertext, mime_type).await?;

        // Zero-knowledge NIP-94 event: URL + ciphertext hash, no key, no `ox`.
        let meta = FileMetadata {
            url: url.clone(),
            mime_type: mime_type.to_string(),
            sha256_hex,
            original_sha256_hex: None,
            size: Some(media.size),
            caption: caption.to_string(),
        };
        let event =
            build_file_metadata_event(&keys, &meta).map_err(|e| UiError::Engine(e.to_string()))?;
        let event_id = event.id.to_hex();
        let created_at = now_secs();

        // Persist a local ref so download_and_decrypt_media(event_id) resolves.
        let reff = MediaRef {
            event_id: event_id.clone(),
            url: url.clone(),
            peer_pubkey: peer.to_hex(),
            mime_type: mime_type.to_string(),
            caption: caption.to_string(),
            size,
            sha256_hex: media.sha256_hex.clone(),
            outgoing: true,
            created_at,
        };
        if let Some(store) = self.ui.store_ref() {
            store
                .put(MEDIA_REFS_TREE, &event_id, &reff)
                .and_then(|()| store.flush())
                .map_err(|e| UiError::Storage(e.to_string()))?;
        }

        // Privately deliver the reference to the recipient over NIP-04.
        let envelope = MediaEnvelope {
            comrade_media: 1,
            event_id: event_id.clone(),
            url: url.clone(),
            mime: mime_type.to_string(),
            caption: caption.to_string(),
            size,
            sha256_hex: media.sha256_hex.clone(),
        };
        let envelope_json =
            serde_json::to_string(&envelope).map_err(|e| UiError::Engine(e.to_string()))?;
        let vault = self.vault.clone().ok_or(UiError::VaultLocked)?;
        vault
            .send_dm(&peer, &envelope_json)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;

        let sender = keys
            .public_key()
            .to_bech32()
            .unwrap_or_else(|_| keys.public_key().to_hex());
        Ok(MediaMessageDto {
            event_id,
            url,
            mime_type: mime_type.to_string(),
            caption: caption.to_string(),
            sender,
            created_at,
            size,
            outgoing: true,
        })
    }

    /// Resolve a NIP-94 reference by event id, fetch the encrypted blob, and
    /// decrypt it with the re-derived ECDH key. Returns base64 bytes + MIME.
    pub async fn download_and_decrypt_media(
        &self,
        event_id: &str,
    ) -> Result<MediaBytesDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let reff: MediaRef = store
            .get(MEDIA_REFS_TREE, event_id)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .ok_or_else(|| UiError::Engine(format!("unknown media event {event_id}")))?;

        let keys = self.ui.identity_keys().ok_or(UiError::NoIdentity)?;
        let peer = parse_pubkey(&reff.peer_pubkey)?;
        let key = derive_media_key(keys.secret_key(), &peer, MEDIA_LABEL)
            .map_err(|e| UiError::Engine(e.to_string()))?;

        // Verify the ciphertext hash when we recorded one (fail fast on a
        // wrong/tampered blob; older refs without it fall back to the AES-GCM
        // tag alone, which still rejects tampering).
        let expected = (!reff.sha256_hex.is_empty()).then_some(reff.sha256_hex.as_str());
        let bytes = fetch_and_decrypt_media(&reff.url, &key, expected)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;

        Ok(MediaBytesDto {
            mime_type: reff.mime_type,
            base64: B64.encode(&bytes),
        })
    }

    /// Full encrypted-media history with `peer` (npub or hex), oldest first —
    /// the media counterpart of [`Self::messages_with`]. Lets a frontend
    /// render past attachments inline after a restart, not just ones that
    /// arrived live this session (references are persisted the moment they're
    /// sent or received — see [`Self::upload_and_send_media`] and
    /// `dispatch_incoming_dm`).
    pub fn media_with(&self, peer: &str) -> Result<Vec<MediaMessageDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let peer_hex = parse_pubkey(peer)?.to_hex();
        let own_npub = self
            .ui
            .current_identity()
            .map(|i| i.npub)
            .unwrap_or_default();

        let mut items: Vec<MediaMessageDto> = store
            .values::<MediaRef>(MEDIA_REFS_TREE)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .filter(|r| r.peer_pubkey == peer_hex)
            .map(|r| MediaMessageDto {
                event_id: r.event_id,
                url: r.url,
                mime_type: r.mime_type,
                caption: r.caption,
                sender: if r.outgoing {
                    own_npub.clone()
                } else {
                    to_npub(&r.peer_pubkey)
                },
                created_at: r.created_at,
                size: r.size,
                outgoing: r.outgoing,
            })
            .collect();
        items.sort_by_key(|m| m.created_at);
        Ok(items)
    }

    /// Upload an encrypted blob to Blossom, signed with a BUD-01 auth event.
    /// Gated on the `media-http` feature; degrades to a typed error otherwise.
    ///
    /// The BUD-01 auth event is signed with a **fresh ephemeral key**, never the
    /// user's chat identity: the blob is already zero-knowledge, and signing
    /// with the identity key would let the host link "npub X uploaded blob Y at
    /// time T from IP Z" — a metadata leak at odds with the privacy model.
    #[cfg(feature = "media-http")]
    async fn upload_blob(&self, blob: Vec<u8>, mime: &str) -> Result<String, UiError> {
        use comrade_core::media::{
            upload_to_first_available, BlossomUploader, MediaUploader, DEFAULT_BLOSSOM_SERVERS,
        };
        // Try each configured Blossom host in turn so a single server being
        // down (or unreachable from this network) doesn't fail the send — the
        // symptom that made every attachment error out on one hardcoded host.
        // Each attempt re-signs with a fresh ephemeral key (identity-unlinkable)
        // and re-uploads the same ciphertext.
        upload_to_first_available(DEFAULT_BLOSSOM_SERVERS, |server| {
            let server = server.to_string();
            let blob = blob.clone();
            let mime = mime.to_string();
            async move {
                BlossomUploader::new(server, nostr_sdk::Keys::generate())
                    .upload(&blob, &mime)
                    .await
                    .map(|receipt| receipt.url)
            }
        })
        .await
        .map_err(|e| UiError::Engine(format!("all media servers failed: {e}")))
    }

    #[cfg(not(feature = "media-http"))]
    async fn upload_blob(&self, _blob: Vec<u8>, _mime: &str) -> Result<String, UiError> {
        Err(UiError::Engine(
            "media upload requires the `media-http` feature".into(),
        ))
    }

    // ── Milestone 3: progressive-disclosure workspace controller ─────────────

    /// Switch the active workspace, enforcing the [`comrade_state`] transition
    /// rules. An invalid or un-paired transition returns a typed [`UiError`]
    /// (surfaced to the frontend as a rejected promise / JSON error).
    ///
    /// On success, also brings the Saathi mesh engine's lifecycle in line with
    /// the new workspace (see [`Self::sync_saathi_lifecycle`]) — entering
    /// `OffGridTravel` really starts mDNS discovery, it doesn't just flip a label.
    pub async fn toggle_workspace(&mut self, target: &str) -> Result<WorkspaceDto, UiError> {
        let dto = self.ui.switch_workspace(target)?;
        self.sync_saathi_lifecycle().await;
        Ok(dto)
    }

    /// Step back to the previous workspace, syncing the Saathi mesh lifecycle
    /// exactly as [`Self::toggle_workspace`] does.
    pub async fn back(&mut self) -> WorkspaceDto {
        let dto = self.ui.back();
        self.sync_saathi_lifecycle().await;
        dto
    }

    /// Snapshot of the off-grid mesh's live status — for seeding a UI's
    /// connectivity indicator before any [`BridgeEvent::MeshStatusChanged`]
    /// has arrived (e.g. right after a cold start or an activity recreation).
    pub fn mesh_status(&self) -> MeshStatusDto {
        match &self.saathi {
            Some(engine) => MeshStatusDto {
                active: true,
                peer_count: engine.peer_count() as u64,
            },
            None => MeshStatusDto {
                active: false,
                peer_count: 0,
            },
        }
    }

    /// Ensure the Saathi engine is running iff the current workspace is
    /// `OffGridTravel`. Centralised here (rather than duplicated in
    /// `toggle_workspace`/`back`) so every path that can change the workspace —
    /// a voice command, a future UI toggle, stepping back — drives the same
    /// real engine the persistent mesh-status indicator reads from.
    async fn sync_saathi_lifecycle(&mut self) {
        let should_run = self.ui.current_workspace().mesh_active;
        match (should_run, self.saathi.is_some()) {
            (true, false) => self.start_saathi().await,
            (false, true) => self.stop_saathi().await,
            _ => {}
        }
    }

    /// Start the Saathi mesh engine and spawn the task that forwards its live
    /// peer-count stream onto the shared event bus. Best-effort: if the swarm
    /// fails to initialise (e.g. no usable socket), the workspace switch still
    /// succeeds — the indicator just reports `active: false` rather than
    /// hanging in a perpetual "connecting" state.
    async fn start_saathi(&mut self) {
        let label = self
            .ui
            .username()
            .or_else(|| self.ui.current_identity().map(|i| i.npub))
            .unwrap_or_else(|| "comrade-mesh-peer".to_string());
        match SaathiEngine::new(label).await {
            Ok(engine) => {
                let engine = Arc::new(engine);
                self.spawn_mesh_status_forwarder(engine.clone());
                self.saathi = Some(engine);
            }
            Err(e) => {
                warn!("Saathi: failed to start mesh engine: {e}");
                let _ = self
                    .events
                    .send(BridgeEvent::MeshStatusChanged(MeshStatusDto {
                        active: false,
                        peer_count: 0,
                    }));
            }
        }
    }

    /// Shut down the Saathi mesh engine and tell the UI it is gone.
    async fn stop_saathi(&mut self) {
        if let Some(engine) = self.saathi.take() {
            engine.shutdown().await;
        }
        let _ = self
            .events
            .send(BridgeEvent::MeshStatusChanged(MeshStatusDto {
                active: false,
                peer_count: 0,
            }));
    }

    /// Forward the engine's peer-count stream onto the bridge event bus as
    /// [`BridgeEvent::MeshStatusChanged`] — once immediately (the starting
    /// snapshot) and again every time a peer joins or leaves.
    fn spawn_mesh_status_forwarder(&self, engine: Arc<SaathiEngine>) {
        let mut peer_count_rx = engine.peer_count_stream();
        let tx = self.events.clone();
        tokio::spawn(async move {
            let _ = tx.send(BridgeEvent::MeshStatusChanged(MeshStatusDto {
                active: true,
                peer_count: *peer_count_rx.borrow() as u64,
            }));
            while peer_count_rx.changed().await.is_ok() {
                let _ = tx.send(BridgeEvent::MeshStatusChanged(MeshStatusDto {
                    active: true,
                    peer_count: *peer_count_rx.borrow() as u64,
                }));
            }
        });
    }

    // ── Sakha/Sakhi CRDT ledger: pairing + entries + sync ────────────────────

    /// Restore a previously-completed pairing (and its ledger snapshot) from
    /// the encrypted store, so a returning paired couple doesn't have to
    /// re-exchange keys every launch. Called once from [`Self::unlock_vault`],
    /// right after the Sakha engine is constructed. Best-effort: a missing or
    /// unreadable record just leaves the engine unpaired, exactly as if this
    /// were the first launch.
    async fn restore_sakha_pairing(&mut self) {
        let Some(store) = self.ui.store_ref() else {
            return;
        };
        let record: Option<SakhaPairingRecord> =
            store.get(SAKHA_TREE, SAKHA_PAIRING_KEY).ok().flatten();
        if let (Some(record), Some(sakha)) = (record, self.sakha.clone()) {
            match PublicKey::parse(&record.partner_pubkey_hex) {
                Ok(partner_pk) => {
                    if let Err(e) = sakha.pair_with(partner_pk) {
                        warn!("failed to restore Sakha pairing: {e}");
                    }
                }
                Err(e) => warn!("stored Sakha partner key is invalid: {e}"),
            }
        }
        // The ledger snapshot restores independently of pairing succeeding —
        // the CRDT text itself doesn't need a partner key to read locally.
        if let Ok(Some(state)) = store.load_ledger_state() {
            if let Some(sakha) = self.sakha.clone() {
                if let Err(e) = sakha.load_snapshot(&state.snapshot).await {
                    warn!("failed to restore Sakha ledger snapshot: {e}");
                }
            }
        }
    }

    /// Start the background loop that merges the partner's incoming ledger
    /// updates: each successful merge pushes [`BridgeEvent::LedgerUpdated`]
    /// and persists a fresh snapshot. Idempotent and safe to call whether
    /// triggered by a fresh [`Self::pair_sakha`] or a pairing restored at
    /// unlock — spawned at most once per runtime.
    fn spawn_sakha_sync_loop(&mut self) {
        if self.sakha_sync_spawned {
            return;
        }
        let Some(sakha) = self.sakha.clone() else {
            return;
        };
        self.sakha_sync_spawned = true;
        let tx = self.events.clone();
        let store = self.ui.store_arc();
        let sakha_for_snapshot = sakha.clone();
        tokio::spawn(async move {
            sakha.connect().await;
            let cb: SakhaSyncCallback = Box::new(move |ledger| {
                let _ = tx.send(BridgeEvent::LedgerUpdated { ledger });
                let Some(store) = store.clone() else { return };
                let sakha = sakha_for_snapshot.clone();
                tokio::spawn(async move { persist_ledger_snapshot(&store, &sakha).await });
            });
            if let Err(e) = sakha.subscribe_sync(cb).await {
                warn!("sakha sync loop ended: {e}");
            }
        });
    }

    /// Perform the Sakha/Sakhi pairing handshake with `partner_pubkey` (npub
    /// or hex) as `role` (`"sakha"`/`"sakhi"`): derives the shared ledger key,
    /// persists the pairing so it survives a restart, and starts the
    /// background sync loop that merges the partner's future ledger updates
    /// live. Returns the resulting pairing status.
    pub async fn pair_sakha(
        &mut self,
        partner_pubkey: &str,
        role: &str,
    ) -> Result<SakhaStatusDto, UiError> {
        let sakha = self.sakha.clone().ok_or(UiError::VaultLocked)?;
        let peer = parse_pubkey(partner_pubkey)?;
        sakha
            .pair_with(peer)
            .map_err(|e| UiError::Engine(e.to_string()))?;

        let role = normalize_pair_role(role);
        if let Some(store) = self.ui.store_ref() {
            let record = SakhaPairingRecord {
                partner_pubkey_hex: peer.to_hex(),
                role,
            };
            store
                .put(SAKHA_TREE, SAKHA_PAIRING_KEY, &record)
                .and_then(|()| store.flush())
                .map_err(|e| UiError::Storage(e.to_string()))?;
        }

        self.spawn_sakha_sync_loop();
        self.sakha_status()
    }

    /// This device's Sakha/Sakhi pairing state.
    pub fn sakha_status(&self) -> Result<SakhaStatusDto, UiError> {
        let sakha = self.sakha.clone().ok_or(UiError::VaultLocked)?;
        let partner_npub = sakha
            .partner_pubkey()
            .map(|pk| pk.to_bech32().unwrap_or_else(|_| pk.to_hex()));
        let role = self
            .ui
            .store_ref()
            .and_then(|s| {
                s.get::<SakhaPairingRecord>(SAKHA_TREE, SAKHA_PAIRING_KEY)
                    .ok()
                    .flatten()
            })
            .map(|r| r.role);
        Ok(SakhaStatusDto {
            paired: sakha.is_paired(),
            partner_npub,
            role,
        })
    }

    /// Append an entry to the shared Sakha/Sakhi CRDT ledger, persist a fresh
    /// local snapshot, and return the merged ledger text. Requires a
    /// completed pairing — use [`Self::pair_sakha`] first.
    pub async fn sakha_add_entry(
        &self,
        description: &str,
        amount_inr: f64,
        paid_by: &str,
    ) -> Result<String, UiError> {
        if description.trim().is_empty() {
            return Err(UiError::Engine("description is empty".into()));
        }
        let sakha = self.sakha.clone().ok_or(UiError::VaultLocked)?;
        if !sakha.is_paired() {
            return Err(UiError::Engine(
                "not paired with a partner yet — open the Partner Portal first".into(),
            ));
        }
        let entry = LedgerEntry::new(description, amount_inr, paid_by);
        sakha
            .add_entry(entry)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;
        let ledger = sakha.read_ledger().await;
        if let Some(store) = self.ui.store_arc() {
            persist_ledger_snapshot(&store, &sakha).await;
        }
        Ok(ledger)
    }

    /// The current Sakha/Sakhi ledger text (local CRDT state — no network
    /// round trip). Empty until entries exist or a snapshot/sync restores some.
    pub async fn sakha_read_ledger(&self) -> Result<String, UiError> {
        let sakha = self.sakha.clone().ok_or(UiError::VaultLocked)?;
        Ok(sakha.read_ledger().await)
    }

    /// Publish the current Sakha/Sakhi shared CRDT ledger state to the partner.
    /// Returns the sync event id (hex). Without a completed pairing handshake the
    /// engine returns a typed error rather than panicking.
    pub async fn sync_ledger(&self) -> Result<String, UiError> {
        let sakha = self.sakha.clone().ok_or(UiError::VaultLocked)?;
        let id = sakha
            .publish_sync()
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;
        Ok(id.to_hex())
    }

    // ── Sync view-model delegations (shared with the existing desktop UI) ────

    pub fn workspaces(&self) -> Vec<WorkspaceDto> {
        self.ui.workspaces()
    }

    pub fn current_workspace(&self) -> WorkspaceDto {
        self.ui.current_workspace()
    }

    pub fn generate_identity(&mut self) -> Result<IdentityDto, UiError> {
        self.ui.generate_identity()
    }

    pub fn current_identity(&self) -> Option<IdentityDto> {
        self.ui.current_identity()
    }

    pub fn extract_payments(&self, text: &str) -> Result<Vec<UpiIntentDto>, UiError> {
        self.ui.extract_payments(text)
    }

    /// Whether the encrypted store is unlocked (a superset state of the vault).
    pub fn is_store_unlocked(&self) -> bool {
        self.ui.is_store_unlocked()
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Normalise a pairing-role string to exactly `"sakha"` or `"sakhi"` —
/// anything else (including case variants) falls back to `"sakha"`, mirroring
/// the lenient `from_str_lenient` pattern already used for `CallMediaKind`/
/// `HangupReason` elsewhere in this bridge.
fn normalize_pair_role(role: &str) -> String {
    if role.eq_ignore_ascii_case("sakhi") {
        "sakhi".to_string()
    } else {
        "sakha".to_string()
    }
}

/// Snapshot the Sakha CRDT doc and persist it, so the ledger survives a
/// restart without needing a fresh sync from the partner. Best-effort: a
/// write failure is logged, not propagated — losing a snapshot write is far
/// less bad than failing the ledger update that triggered it.
async fn persist_ledger_snapshot(store: &comrade_storage::EncryptedStore, sakha: &SakhaEngine) {
    let bytes = sakha.snapshot_bytes().await;
    let state = comrade_storage::LedgerState {
        snapshot: bytes,
        updated_at: now_secs(),
    };
    if let Err(e) = store.save_ledger_state(&state).and_then(|()| store.flush()) {
        warn!("failed to persist Sakha ledger snapshot: {e}");
    }
}

/// Store key for a journal entry: a zero-padded timestamp prefix (so ids sort
/// chronologically) plus a random tail (so two entries in the same second
/// never collide). The randomness comes from a throwaway secp256k1 key — no
/// extra dependency, and cryptographically unpredictable.
fn journal_entry_id(created_at: u64) -> String {
    let tail = nostr_sdk::Keys::generate().public_key().to_hex();
    format!("{created_at:020}-{}", &tail[..12])
}

/// The peer's published @handle from the local profile cache, if known.
fn cached_peer_name(store: &comrade_storage::EncryptedStore, npub: &str) -> Option<String> {
    store
        .get::<PeerProfileRecord>(PEER_PROFILES_TREE, npub)
        .ok()
        .flatten()
        .and_then(|r| r.name)
}

/// Legacy builds auto-filled an empty alias with the first 12 characters of
/// the peer's npub. Normalise those placeholders (and blanks) to "no alias"
/// so the peer's published handle can title the chat — otherwise every
/// pre-existing key-added contact is stuck displaying `npub1abcdefg` forever.
const LEGACY_PLACEHOLDER_LEN: usize = 12;

fn user_alias(petname: &str, npub: &str) -> Option<String> {
    let trimmed = petname.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() == LEGACY_PLACEHOLDER_LEN && npub.starts_with(trimmed) {
        return None;
    }
    Some(trimmed.to_string())
}

/// Map a fetched Kind-0 (or its absence) to the search-result DTO. One
/// mapping for both the direct-key branch and the name-search branch, so the
/// two can never render the same profile differently.
fn found_profile_dto(pk: &PublicKey, meta: Option<&Metadata>) -> FoundProfileDto {
    FoundProfileDto {
        npub: pk.to_bech32().unwrap_or_else(|_| pk.to_hex()),
        name: meta.and_then(display_name_of),
        about: meta.and_then(|m| m.about.clone()),
    }
}

/// Best-effort single-record write into the profile cache; returns whether
/// the write succeeded. Callers flush once per batch.
fn store_profile_record(
    store: &comrade_storage::EncryptedStore,
    npub: &str,
    record: &PeerProfileRecord,
) -> bool {
    match store.put(PEER_PROFILES_TREE, npub, record) {
        Ok(()) => true,
        Err(e) => {
            warn!("failed to cache peer profile: {e}");
            false
        }
    }
}

/// A user-configured TURN relay for WebRTC calls, sealed in the encrypted store.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnConfig {
    url: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    credential: String,
}

/// The conversation gate for an incoming DM's sender.
enum IncomingGate {
    /// Peer is blocked — drop everything from them silently.
    Blocked,
    /// Peer is an established conversation — deliver normally + ack.
    Accepted,
    /// Peer is a stranger (or an unaccepted request) — route to requests.
    Pending,
}

/// Classify an incoming DM's sender against the conversation gate. A peer with
/// no gate record is treated as `Pending` (a new stranger); [`send_dm`] and
/// [`accept_request`] are what flip a peer to `Accepted`.
///
/// [`send_dm`]: ComradeRuntime::send_dm
/// [`accept_request`]: ComradeRuntime::accept_request
fn conversation_gate(store: &comrade_storage::EncryptedStore, peer_npub: &str) -> IncomingGate {
    match store.get_conversation_meta(peer_npub).ok().flatten() {
        Some(m) if m.state == STATE_BLOCKED => IncomingGate::Blocked,
        Some(m) if m.state == STATE_ACCEPTED => IncomingGate::Accepted,
        _ => IncomingGate::Pending,
    }
}

/// Record a peer as a pending request if they have no gate record yet.
fn ensure_pending(store: Option<&Arc<comrade_storage::EncryptedStore>>, peer_npub: &str) {
    let Some(store) = store else { return };
    if store
        .get_conversation_meta(peer_npub)
        .ok()
        .flatten()
        .is_none()
    {
        let meta = comrade_storage::ConversationMeta {
            peer_npub: peer_npub.to_string(),
            state: STATE_PENDING.to_string(),
            profile_shared: false,
            updated_at: now_secs(),
        };
        if let Err(e) = store
            .set_conversation_meta(&meta)
            .and_then(|()| store.flush())
        {
            warn!("failed to record message request: {e}");
        }
    }
}

/// Cache a peer's shared display handle (from a profile-share envelope).
fn cache_pushed_peer_name(store: &comrade_storage::EncryptedStore, npub: &str, name: &str) {
    let record = PeerProfileRecord {
        name: Some(name.to_string()),
        about: None,
        updated_at: now_secs(),
    };
    if store_profile_record(store, npub, &record) {
        let _ = store.flush();
    }
}

/// Fire a delivered receipt back to `sender_hex` for `message_id` (best-effort;
/// only ever called for accepted conversations).
fn send_delivered_receipt(vault: &Arc<VaultEngine>, sender_hex: &str, message_id: &str) {
    let Ok(peer) = PublicKey::parse(sender_hex) else {
        return;
    };
    let Ok(json) = Receipt::new(ReceiptKind::Delivered, vec![message_id.to_string()]).to_json()
    else {
        return;
    };
    let vault = vault.clone();
    tokio::spawn(async move {
        if let Err(e) = vault.send_dm(&peer, &json).await {
            tracing::debug!("delivered receipt not sent: {e}");
        }
    });
}

/// Route one decrypted incoming DM: block-drop, control envelopes
/// (receipt/profile-share/call), media, or plain chat — applying the message
/// -request gate throughout. Runs inside the Vault inbox Tokio task.
fn dispatch_incoming_dm(
    vault: &Arc<VaultEngine>,
    store: Option<&Arc<comrade_storage::EncryptedStore>>,
    tx: &broadcast::Sender<BridgeEvent>,
    msg: VaultMessage,
) {
    let peer_npub = to_npub(&msg.sender_pubkey);
    let gate = store
        .map(|s| conversation_gate(s, &peer_npub))
        .unwrap_or(IncomingGate::Pending);
    if matches!(gate, IncomingGate::Blocked) {
        return;
    }

    // 1) Receipt — advance our outgoing statuses (accepted conversations only).
    if let Some(receipt) = parse_receipt(&msg.content) {
        if matches!(gate, IncomingGate::Accepted) {
            if let Some(store) = store {
                let status = receipt.status.as_str();
                let mut changed = Vec::new();
                for id in &receipt.message_ids {
                    if store.set_message_status(id, status).unwrap_or(false) {
                        changed.push(id.clone());
                    }
                }
                let _ = store.flush();
                if !changed.is_empty() {
                    let _ = tx.send(BridgeEvent::MessageStatus {
                        peer: peer_npub,
                        message_ids: changed,
                        status: status.to_string(),
                    });
                }
            }
        }
        return;
    }

    // 2) Profile share — cache the peer's shared @handle (any non-blocked peer;
    //    they revealed it by reaching out or accepting).
    if let Some(profile) = parse_profile_share(&msg.content) {
        if let (Some(store), Some(name)) = (store, profile.username) {
            cache_pushed_peer_name(store, &peer_npub, &name);
            let _ = tx.send(BridgeEvent::PeerProfileUpdated {
                peer: peer_npub,
                name: Some(name),
            });
        }
        return;
    }

    // 3) Call signaling — only from an established conversation, so a stranger
    //    cannot ring you before their message request is accepted.
    if let Some(env) = parse_call_envelope(&msg.content) {
        if matches!(gate, IncomingGate::Accepted) {
            let _ = tx.send(BridgeEvent::IncomingCallSignal(CallSignalDto {
                call_id: env.call_id,
                peer: peer_npub,
                media: env.media.as_str().to_string(),
                signal: env.signal,
            }));
        }
        return;
    }

    // 4) Media envelope — persist the NIP-94 ref, then surface (gated).
    if let Some(env) = parse_media_envelope(&msg.content) {
        if let Some(store) = store {
            let reff = MediaRef {
                event_id: env.event_id.clone(),
                url: env.url.clone(),
                peer_pubkey: msg.sender_pubkey.clone(),
                mime_type: env.mime.clone(),
                caption: env.caption.clone(),
                size: env.size,
                sha256_hex: env.sha256_hex.clone(),
                outgoing: false,
                created_at: msg.created_at,
            };
            if let Err(e) = store
                .put(MEDIA_REFS_TREE, &env.event_id, &reff)
                .and_then(|()| store.flush())
            {
                warn!("failed to persist incoming media ref: {e}");
            }
        }
        if matches!(gate, IncomingGate::Accepted) {
            let _ = tx.send(BridgeEvent::IncomingMedia(MediaMessageDto {
                event_id: env.event_id,
                url: env.url,
                mime_type: env.mime,
                caption: env.caption,
                sender: to_npub(&msg.sender_pubkey),
                created_at: msg.created_at,
                size: env.size,
                outgoing: false,
            }));
            send_delivered_receipt(vault, &msg.sender_pubkey, &msg.event_id);
        } else {
            ensure_pending(store, &peer_npub);
            let preview = if env.caption.is_empty() {
                "📎 Attachment".to_string()
            } else {
                format!("📎 {}", env.caption)
            };
            let _ = tx.send(BridgeEvent::IncomingMessageRequest(MessageRequestDto {
                peer: peer_npub,
                last_message: preview,
                last_at: msg.created_at,
            }));
        }
        return;
    }

    // 5) Plain chat text — persist, then deliver or gate into a request.
    if let Some(store) = store {
        let row = comrade_storage::StoredMessage {
            id: msg.event_id.clone(),
            peer_npub: peer_npub.clone(),
            content: msg.content.clone(),
            created_at: msg.created_at,
            outgoing: false,
            status: None,
            reply_to: msg.reply_to.clone(),
        };
        if let Err(e) = store.save_message(&row).and_then(|()| store.flush()) {
            warn!("failed to persist incoming DM: {e}");
        }
    }
    if matches!(gate, IncomingGate::Accepted) {
        send_delivered_receipt(vault, &msg.sender_pubkey, &msg.event_id);
        let _ = tx.send(BridgeEvent::IncomingDirectMessage(DirectMessageDto::from(
            msg,
        )));
    } else {
        ensure_pending(store, &peer_npub);
        let _ = tx.send(BridgeEvent::IncomingMessageRequest(MessageRequestDto {
            peer: peer_npub,
            last_message: msg.content,
            last_at: msg.created_at,
        }));
    }
}

/// Detached profile-refresh worker. Holds only the engine and store handles,
/// so the shared `Arc<RwLock<ComradeRuntime>>` guard can be dropped before
/// the slow network work starts (see [`ComradeRuntime::profile_refresher`]).
pub struct ProfileRefresher {
    sabha: Arc<SabhaEngine>,
    store: Arc<comrade_storage::EncryptedStore>,
}

impl ProfileRefresher {
    /// Refresh the cached Kind-0 profiles of everyone we talk to
    /// (conversation peers and saved contacts) in **one** relay round-trip,
    /// bounded by [`PROFILE_REFRESH_CAP`] and per-record freshness windows.
    /// Returns how many display names changed — the frontend reloads its
    /// chat list when > 0.
    pub async fn run(self) -> Result<usize, UiError> {
        let mut peers: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for msg in self
            .store
            .all_messages()
            .map_err(|e| UiError::Storage(e.to_string()))?
        {
            if seen.insert(msg.peer_npub.clone()) {
                peers.push(msg.peer_npub);
            }
        }
        for contact in self
            .store
            .list_contacts()
            .map_err(|e| UiError::Storage(e.to_string()))?
        {
            if seen.insert(contact.npub.clone()) {
                peers.push(contact.npub);
            }
        }

        // Select the stale records. A record that has a name is trusted for
        // the full TTL; a nameless record only briefly — an offline launch
        // yields Ok(no events) from the pool (indistinguishable from "peer
        // has no profile"), and that outcome must not freeze the peer as
        // key-only for a whole day.
        let now = now_secs();
        let mut stale: Vec<(String, PublicKey, Option<PeerProfileRecord>)> = Vec::new();
        for npub in peers {
            if stale.len() >= PROFILE_REFRESH_CAP {
                break;
            }
            let previous: Option<PeerProfileRecord> = self
                .store
                .get(PEER_PROFILES_TREE, &npub)
                .unwrap_or_default();
            let ttl = if previous.as_ref().is_some_and(|p| p.name.is_some()) {
                PROFILE_TTL_SECS
            } else {
                PROFILE_NEGATIVE_TTL_SECS
            };
            let fresh = previous
                .as_ref()
                .is_some_and(|r| now.saturating_sub(r.updated_at) < ttl);
            if fresh {
                continue;
            }
            let Ok(pk) = PublicKey::parse(&npub) else {
                continue;
            };
            stale.push((npub, pk, previous));
        }
        if stale.is_empty() {
            return Ok(0);
        }

        let authors: Vec<PublicKey> = stale.iter().map(|(_, pk, _)| *pk).collect();
        let found = match self.sabha.fetch_profiles(&authors).await {
            Ok(found) => found,
            Err(e) => {
                // Transport error: stamp nothing, so the next refresh retries.
                warn!("peer profile refresh failed: {e}");
                return Ok(0);
            }
        };

        let mut wrote = false;
        let mut changed = 0usize;
        for (npub, pk, previous) in stale {
            let meta = found.get(&pk);
            let record = PeerProfileRecord {
                // A silent relay set must not erase a name we already knew.
                name: meta
                    .and_then(display_name_of)
                    .or_else(|| previous.as_ref().and_then(|p| p.name.clone())),
                about: meta
                    .and_then(|m| m.about.clone())
                    .or_else(|| previous.as_ref().and_then(|p| p.about.clone())),
                updated_at: now,
            };
            let name_changed = record.name != previous.as_ref().and_then(|p| p.name.clone());
            if store_profile_record(&self.store, &npub, &record) {
                wrote = true;
                if name_changed {
                    changed += 1;
                }
            }
        }
        if wrote {
            if let Err(e) = self.store.flush() {
                warn!("failed to flush profile cache: {e}");
            }
        }
        Ok(changed)
    }
}

/// Publish the Kind-0 profile with retries and exponential backoff.
///
/// Why this exists: at onboarding the relays are still dialling when the
/// handle is claimed, and a single fire-and-forget publish that fails leaves
/// the identity permanently undiscoverable — peers searching the handle find
/// nothing. `publish_profile` itself waits (bounded) for a connection; this
/// wrapper keeps trying across transient failures. It is also spawned on
/// every launch (Kind-0 is replaceable, so republishing is idempotent).
async fn publish_profile_with_retry(sabha: Arc<SabhaEngine>, name: String) {
    // Make sure dials were at least initiated, even if the feed loop that
    // normally calls connect() hasn't run yet. Idempotent.
    sabha.connect().await;
    let mut delay = std::time::Duration::from_secs(2);
    for attempt in 1..=PUBLISH_ATTEMPTS {
        match sabha.publish_profile(&name, None).await {
            Ok(_) => {
                tracing::info!(attempt, "profile handle published to relays");
                return;
            }
            Err(e) => warn!(attempt, "profile publish failed (will retry): {e}"),
        }
        tokio::time::sleep(delay).await;
        delay = delay.saturating_mul(2);
    }
    warn!("profile publish gave up after {PUBLISH_ATTEMPTS} attempts; will retry on next launch");
}

/// Normalise and validate a chosen @handle: strip a leading '@', lowercase,
/// then require 3–24 chars of `[a-z0-9_]`. One rule shared by every bridge.
fn normalize_handle(raw: &str) -> Result<String, UiError> {
    let handle = raw.trim().trim_start_matches('@').to_lowercase();
    if handle.len() < 3 || handle.len() > 24 {
        return Err(UiError::Engine("username must be 3–24 characters".into()));
    }
    if !handle
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(UiError::Engine(
            "username may only contain a–z, 0–9 and _".into(),
        ));
    }
    // "primary" is the legacy no-username marker inside the store.
    if handle == "primary" {
        return Err(UiError::Engine("that username is reserved".into()));
    }
    Ok(handle)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn runtime_is_send_sync_for_shared_state() {
        // The Tauri managed state and JNI global both require this bound; a
        // regression here is exactly the Send/Sync compile boundary M5 guards.
        assert_send_sync::<ComradeRuntime>();
        assert_send_sync::<std::sync::Arc<tokio::sync::RwLock<ComradeRuntime>>>();
        assert_send_sync::<BridgeEvent>();
    }

    #[test]
    fn bridge_futures_are_send() {
        // Tauri's #[tauri::command] requires every command future to be Send;
        // the workspace itself never demands that, so without this
        // compile-time probe a non-Send future (e.g. a borrowed iterator held
        // across an await deep inside an engine) only surfaces in the desktop
        // CI lane — which is exactly how the search_profiles regression
        // escaped local checks once.
        fn require_send<T: Send>(_t: T) {}
        #[allow(dead_code)]
        fn probe(rt: &ComradeRuntime, wrt: &mut ComradeRuntime, urt: &mut ComradeRuntime) {
            require_send(rt.search_profiles("q"));
            require_send(rt.refresh_peer_profiles());
            require_send(rt.send_dm("npub1x", "hi"));
            require_send(rt.broadcast_chitthi("x", None));
            require_send(rt.sync_ledger());
            require_send(rt.upload_and_send_media("x", vec![], "image/png", ""));
            require_send(rt.download_and_decrypt_media("x"));
            require_send(rt.sakha_add_entry("desc", 1.0, "sakha"));
            require_send(rt.sakha_read_ledger());
            require_send(wrt.set_username("neo"));
            require_send(wrt.pair_sakha("npub1x", "sakha"));
            require_send(urt.unlock_vault("/tmp/x", "p"));
            require_send(wrt.toggle_workspace("Base"));
            require_send(urt.back());
        }
        let _ = probe;
    }

    #[tokio::test]
    async fn toggle_workspace_enforces_state_machine() {
        let mut rt = ComradeRuntime::new();
        let dto = rt.toggle_workspace("OffGridTravel").await.unwrap();
        assert_eq!(dto.key, "OffGridTravel");
        assert!(dto.mesh_active);
        // OffGridTravel -> CoupleSandbox is blocked by the transition graph.
        assert!(matches!(
            rt.toggle_workspace("CoupleSandboxSakha").await,
            Err(UiError::Transition(_))
        ));
        // Unknown keys are a distinct typed error.
        assert!(matches!(
            rt.toggle_workspace("Nope").await,
            Err(UiError::UnknownWorkspace(_))
        ));
    }

    #[tokio::test]
    async fn toggle_workspace_starts_and_stops_the_mesh_engine() {
        let mut rt = ComradeRuntime::new();
        assert_eq!(
            rt.mesh_status(),
            MeshStatusDto {
                active: false,
                peer_count: 0
            }
        );

        rt.toggle_workspace("OffGridTravel").await.unwrap();
        assert_eq!(
            rt.mesh_status(),
            MeshStatusDto {
                active: true,
                peer_count: 0
            }
        );

        rt.toggle_workspace("Base").await.unwrap();
        assert_eq!(
            rt.mesh_status(),
            MeshStatusDto {
                active: false,
                peer_count: 0
            }
        );
    }

    #[tokio::test]
    async fn back_also_stops_the_mesh_engine() {
        let mut rt = ComradeRuntime::new();
        rt.toggle_workspace("OffGridTravel").await.unwrap();
        assert!(rt.mesh_status().active);

        let dto = rt.back().await;
        assert_eq!(dto.key, "Base");
        assert!(!rt.mesh_status().active);
    }

    #[test]
    fn commands_reject_gracefully_when_vault_locked() {
        let rt = ComradeRuntime::new();
        assert!(!rt.is_vault_unlocked());
        assert!(matches!(
            rt.fetch_sabha_timeline(),
            Err(UiError::VaultLocked)
        ));
    }

    #[tokio::test]
    async fn broadcast_rejects_when_locked_without_panicking() {
        let rt = ComradeRuntime::new();
        let err = rt.broadcast_chitthi("hello sabha", None).await;
        assert!(matches!(err, Err(UiError::VaultLocked)));
        let err = rt.sync_ledger().await;
        assert!(matches!(err, Err(UiError::VaultLocked)));
    }

    #[tokio::test]
    async fn unlock_vault_seeds_identity_and_builds_engines() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        let id = rt.unlock_vault(dir.path(), "passphrase").await.unwrap();
        assert!(id.npub.starts_with("npub1"));
        assert!(rt.is_vault_unlocked());
        assert!(rt.is_store_unlocked());
        // Timeline is reachable (empty cache) once unlocked.
        assert!(rt.fetch_sabha_timeline().unwrap().is_empty());
    }

    #[tokio::test]
    async fn second_unlock_is_idempotent_and_keeps_the_same_identity() {
        // A repeated unlock must return the existing identity without rebuilding
        // engines (which would orphan the running ones and duplicate loops).
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        let first = rt.unlock_vault(dir.path(), "pin").await.unwrap().npub;
        let sabha_ptr = Arc::as_ptr(rt.sabha.as_ref().unwrap()) as usize;
        // Second unlock — same or different args — is a no-op that returns the
        // current identity and leaves the engine instances untouched.
        let second = rt.unlock_vault(dir.path(), "pin").await.unwrap().npub;
        assert_eq!(first, second);
        assert_eq!(
            sabha_ptr,
            Arc::as_ptr(rt.sabha.as_ref().unwrap()) as usize,
            "engines must not be rebuilt on a repeated unlock"
        );
    }

    #[tokio::test]
    async fn unlock_then_reopen_restores_same_identity() {
        let dir = TempDir::new().unwrap();
        let first = {
            let mut rt = ComradeRuntime::new();
            rt.unlock_vault(dir.path(), "pin").await.unwrap().npub
        };
        let mut rt2 = ComradeRuntime::new();
        let second = rt2.unlock_vault(dir.path(), "pin").await.unwrap().npub;
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn sakha_status_and_ledger_reject_gracefully_before_pairing() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let status = rt.sakha_status().unwrap();
        assert!(!status.paired);
        assert_eq!(status.partner_npub, None);

        assert!(matches!(
            rt.sakha_add_entry("Coffee", 150.0, "Sakha").await,
            Err(UiError::Engine(_))
        ));
        // Reading the (empty) local ledger doesn't require pairing.
        assert_eq!(rt.sakha_read_ledger().await.unwrap(), "");
    }

    #[tokio::test]
    async fn pair_sakha_rejects_an_invalid_partner_key() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert!(rt.pair_sakha("not-a-valid-key", "sakha").await.is_err());
        assert!(!rt.sakha_status().unwrap().paired);
    }

    #[tokio::test]
    async fn pair_sakha_add_entry_and_status_roundtrip() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let partner = comrade_core::crypto::KeyProfile::generate().unwrap();
        let status = rt.pair_sakha(&partner.npub, "Sakhi").await.unwrap();
        assert!(status.paired);
        assert_eq!(status.partner_npub.as_deref(), Some(partner.npub.as_str()));
        assert_eq!(status.role.as_deref(), Some("sakhi"));

        let ledger = rt
            .sakha_add_entry("Groceries", 300.0, "Sakhi")
            .await
            .unwrap();
        assert!(ledger.contains("Groceries"), "entry must appear: {ledger}");
        assert_eq!(rt.sakha_read_ledger().await.unwrap(), ledger);
    }

    #[test]
    fn sakha_pairing_and_ledger_survive_a_relaunch() {
        // Regression guard for AUDIT A3/A8: pairing state and the local
        // ledger must not evaporate on restart just because the in-memory
        // Yrs doc and the paired-partner key live nowhere but RAM otherwise.
        //
        // This uses two independent Tokio runtimes (rather than one shared
        // `#[tokio::test]` runtime) to actually simulate a process restart:
        // `pair_sakha` spawns a detached background sync task that holds its
        // own `Arc` clone of the encrypted store, so within a single runtime
        // that task outlives the `{ }` scope below and keeps the redb file
        // open — dropping the whole `Runtime` (unlike a scope exit) forcibly
        // tears down every task it owns, exactly as a real process exit
        // would, and only then is the file lock actually released.
        let dir = TempDir::new().unwrap();
        let partner = comrade_core::crypto::KeyProfile::generate().unwrap();

        {
            let rt_tokio = tokio::runtime::Runtime::new().unwrap();
            rt_tokio.block_on(async {
                let mut rt = ComradeRuntime::new();
                rt.unlock_vault(dir.path(), "pin").await.unwrap();
                rt.pair_sakha(&partner.npub, "sakha").await.unwrap();
                rt.sakha_add_entry("Rent", 12000.0, "Sakha").await.unwrap();
            });
        }

        let rt_tokio2 = tokio::runtime::Runtime::new().unwrap();
        rt_tokio2.block_on(async {
            let mut rt2 = ComradeRuntime::new();
            rt2.unlock_vault(dir.path(), "pin").await.unwrap();
            let status = rt2.sakha_status().unwrap();
            assert!(status.paired, "pairing must survive a relaunch");
            assert_eq!(status.partner_npub.as_deref(), Some(partner.npub.as_str()));
            let ledger = rt2.sakha_read_ledger().await.unwrap();
            assert!(
                ledger.contains("Rent"),
                "ledger snapshot must survive a relaunch: {ledger}"
            );
        });
    }

    #[tokio::test]
    async fn fetch_timeline_reads_from_encrypted_cache() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        // Seed the encrypted cache directly (the relay loop does this in prod).
        rt.ui
            .store_ref()
            .unwrap()
            .cache_chitthi(&comrade_storage::Chitthi {
                id: "abc123".into(),
                author_npub: "npub1author".into(),
                content: "Namaste".into(),
                created_at: 42,
                reply_to: None,
            })
            .unwrap();

        let feed = rt.fetch_sabha_timeline().unwrap();
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].id, "abc123");
        assert_eq!(feed[0].content, "Namaste");
    }

    #[tokio::test]
    async fn event_bus_delivers_serialisable_events() {
        let rt = ComradeRuntime::new();
        let mut rx = rt.subscribe_events();

        let event = BridgeEvent::IncomingChitthi(ChitthiDto {
            id: "id1".into(),
            author: "npub1x".into(),
            content: "over the wire".into(),
            created_at: 7,
            reply_to: None,
        });
        rt.event_sender().send(event.clone()).unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received, event);

        // It must round-trip through serde_json (the IPC payload format).
        let json = serde_json::to_string(&received).unwrap();
        assert!(json.contains("\"type\":\"incoming_chitthi\""));
        let back: BridgeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn media_envelope_detection() {
        let env = MediaEnvelope {
            comrade_media: 1,
            event_id: "e1".into(),
            url: "https://blob/x".into(),
            mime: "image/png".into(),
            caption: "hi".into(),
            size: 10,
            sha256_hex: "a".repeat(64),
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(parse_media_envelope(&json).is_some());
        // A plain DM is not mistaken for a media envelope.
        assert!(parse_media_envelope("just a normal message").is_none());
        assert!(parse_media_envelope(r#"{"hello":"world"}"#).is_none());
        // An envelope written before the sha256_hex field still parses (the
        // field is #[serde(default)]) — back-compat for already-sent media.
        assert!(parse_media_envelope(
            r#"{"comrade_media":1,"event_id":"e","url":"https://b/x","mime":"image/png","caption":"","size":1}"#
        )
        .is_some());
    }

    #[test]
    fn to_npub_canonicalises_incoming_and_outgoing_to_the_same_key() {
        // Regression guard: incoming media/DM senders arrive as hex, outgoing
        // DTOs emit bech32. Both must normalise to the identical npub so the
        // frontend keys one conversation (and the couple panel) per peer.
        let keys = nostr_sdk::Keys::generate();
        let hex = keys.public_key().to_hex();
        let npub = keys.public_key().to_bech32().unwrap();
        assert_eq!(to_npub(&hex), npub, "hex must normalise to npub");
        assert_eq!(to_npub(&npub), npub, "npub is already canonical");
        // Unparseable input falls back unchanged rather than panicking.
        assert_eq!(to_npub("not-a-key"), "not-a-key");
    }

    #[test]
    fn incoming_media_event_serialises_with_tag() {
        let ev = BridgeEvent::IncomingMedia(MediaMessageDto {
            event_id: "e".into(),
            url: "https://blob/x".into(),
            mime_type: "image/jpeg".into(),
            caption: "pic".into(),
            sender: "npub1x".into(),
            created_at: 1,
            size: 42,
            outgoing: false,
        });
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"incoming_media\""));
        let back: BridgeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn handle_normalisation_rules() {
        assert_eq!(normalize_handle("@Abc_User").unwrap(), "abc_user");
        assert_eq!(normalize_handle("  neo42 ").unwrap(), "neo42");
        assert!(normalize_handle("ab").is_err(), "too short");
        assert!(normalize_handle(&"x".repeat(25)).is_err(), "too long");
        assert!(normalize_handle("has space").is_err());
        assert!(normalize_handle("emoji🙂").is_err());
        assert!(normalize_handle("primary").is_err(), "reserved");
    }

    #[tokio::test]
    async fn username_round_trips_through_the_encrypted_store() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert_eq!(rt.profile().unwrap().username, None);

        let profile = rt.set_username("@Chandra_M").await.unwrap();
        assert_eq!(profile.username.as_deref(), Some("chandra_m"));

        // Reopen: the handle must survive alongside the identity.
        drop(rt);
        let mut rt2 = ComradeRuntime::new();
        rt2.unlock_vault(dir.path(), "pin").await.unwrap();
        assert_eq!(
            rt2.profile().unwrap().username.as_deref(),
            Some("chandra_m")
        );
    }

    #[tokio::test]
    async fn contacts_are_pinned_by_npub_not_by_alias() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let a = nostr_sdk::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        let b = nostr_sdk::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();

        // Two different keys may claim the same alias — both entries survive,
        // because the store is keyed by npub (TOFU), not by the display name.
        rt.add_contact(&a, "abc_user").unwrap();
        rt.add_contact(&b, "abc_user").unwrap();
        let contacts = rt.list_contacts().unwrap();
        assert_eq!(contacts.len(), 2);
        assert!(contacts.iter().any(|c| c.npub == a));
        assert!(contacts.iter().any(|c| c.npub == b));

        // Re-adding the same npub renames in place instead of duplicating.
        rt.add_contact(&a, "renamed").unwrap();
        let contacts = rt.list_contacts().unwrap();
        assert_eq!(contacts.len(), 2);
        assert_eq!(
            contacts.iter().find(|c| c.npub == a).unwrap().alias,
            "renamed"
        );

        // An empty alias on re-add (opening an existing chat) must never wipe
        // the alias the user chose.
        rt.add_contact(&a, "  ").unwrap();
        assert_eq!(
            rt.list_contacts()
                .unwrap()
                .iter()
                .find(|c| c.npub == a)
                .unwrap()
                .alias,
            "renamed"
        );

        // Junk npubs are a typed error, not a stored contact.
        assert!(rt.add_contact("not-a-key", "x").is_err());
    }

    #[tokio::test]
    async fn contact_alias_lifecycle_set_clear_remove() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let peer = nostr_sdk::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();

        // Adding by key alone stores no alias (no fake npub-prefix names).
        let added = rt.add_contact(&peer, "").unwrap();
        assert_eq!(added.alias, "");

        // A conversation exists with this peer, so the chat list reflects the
        // alias lifecycle end to end.
        rt.ui
            .store_ref()
            .unwrap()
            .save_message(&comrade_storage::StoredMessage {
                id: "m1".into(),
                peer_npub: peer.clone(),
                content: "hello".into(),
                created_at: 1,
                outgoing: true,
                status: None,
                reply_to: None,
            })
            .unwrap();

        // The alias feature: set…
        let set = rt.set_contact_alias(&peer, "Charlie ❤").unwrap();
        assert_eq!(set.alias, "Charlie ❤");
        assert_eq!(
            rt.conversations().unwrap()[0].alias.as_deref(),
            Some("Charlie ❤")
        );

        // …and clear (empty = explicit clear, unlike add_contact).
        let cleared = rt.set_contact_alias(&peer, "").unwrap();
        assert_eq!(cleared.alias, "");
        assert_eq!(rt.conversations().unwrap()[0].alias, None);
        assert!(rt.remove_contact(&peer).unwrap());
        assert!(
            !rt.remove_contact(&peer).unwrap(),
            "second remove is a no-op"
        );
        assert_eq!(rt.messages_with(&peer).unwrap().len(), 1);
        assert!(matches!(
            rt.set_contact_alias("junk", "x"),
            Err(UiError::Engine(_))
        ));
    }

    #[tokio::test]
    async fn journal_lifecycle_add_list_delete() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();

        // Locked → typed errors, no panics.
        assert!(matches!(
            rt.add_journal_entry("hi", None),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(rt.journal_entries(), Err(UiError::VaultLocked)));
        assert!(matches!(
            rt.delete_journal_entry("x"),
            Err(UiError::VaultLocked)
        ));

        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let first = rt
            .add_journal_entry("  rough morning  ", Some("😕"))
            .unwrap();
        assert_eq!(first.text, "rough morning", "text is trimmed");
        assert_eq!(first.mood.as_deref(), Some("😕"));
        let second = rt.add_journal_entry("grateful today", Some("  ")).unwrap();
        assert_eq!(second.mood, None, "blank mood normalises to none");
        assert_ne!(first.id, second.id);

        // Whitespace-only text is rejected.
        assert!(matches!(
            rt.add_journal_entry("   ", None),
            Err(UiError::Engine(_))
        ));

        let entries = rt.journal_entries().unwrap();
        assert_eq!(entries.len(), 2);
        // Newest first; same-second entries fall back to id ordering.
        assert!(entries[0].created_at >= entries[1].created_at);

        assert!(rt.delete_journal_entry(&first.id).unwrap());
        assert!(!rt.delete_journal_entry(&first.id).unwrap());
        assert_eq!(rt.journal_entries().unwrap().len(), 1);

        // Entries survive a restart (encrypted at rest).
        drop(rt);
        let mut rt2 = ComradeRuntime::new();
        rt2.unlock_vault(dir.path(), "pin").await.unwrap();
        let entries = rt2.journal_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "grateful today");
    }

    #[test]
    fn journal_ids_sort_chronologically_and_never_collide() {
        let a = journal_entry_id(5);
        let b = journal_entry_id(5);
        let later = journal_entry_id(1_700_000_000);
        assert_ne!(a, b, "same-second ids must differ");
        assert!(a < later && b < later, "timestamp prefix sorts");
    }

    #[tokio::test]
    async fn legacy_placeholder_petnames_no_longer_mask_published_names() {
        // Old builds auto-filled an empty alias with the first 12 chars of
        // the npub. Those placeholders must read as "no alias" so the peer's
        // published handle can title the chat after an upgrade.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let store = rt.ui.store_ref().unwrap();

        let peer = nostr_sdk::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        let placeholder: String = peer.chars().take(12).collect();
        store
            .upsert_contact(&comrade_storage::Contact {
                npub: peer.clone(),
                petname: placeholder.clone(),
                relays: vec![],
            })
            .unwrap();
        store
            .save_message(&comrade_storage::StoredMessage {
                id: "m1".into(),
                peer_npub: peer.clone(),
                content: "hi".into(),
                created_at: 1,
                outgoing: false,
                status: None,
                reply_to: None,
            })
            .unwrap();
        store
            .put(
                PEER_PROFILES_TREE,
                &peer,
                &PeerProfileRecord {
                    name: Some("charlie".into()),
                    about: None,
                    updated_at: 1,
                },
            )
            .unwrap();

        let convo = &rt.conversations().unwrap()[0];
        assert_eq!(convo.alias, None, "placeholder is not a user alias");
        assert_eq!(convo.peer_name.as_deref(), Some("charlie"));
        assert_eq!(rt.list_contacts().unwrap()[0].alias, "");

        // A real alias — even one that looks key-ish but isn't this npub's
        // prefix — still wins.
        assert_eq!(user_alias("Mom", &peer).as_deref(), Some("Mom"));
        assert_eq!(user_alias(&placeholder, &peer), None);
        assert_eq!(user_alias("  ", &peer), None);
        assert_eq!(
            user_alias("npub1someone", &peer).as_deref(),
            Some("npub1someone"),
            "a 12-char alias that is not this peer's prefix is kept"
        );
    }

    #[tokio::test]
    async fn conversations_and_contacts_carry_cached_peer_names() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let store = rt.ui.store_ref().unwrap();

        let peer = nostr_sdk::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        store
            .save_message(&comrade_storage::StoredMessage {
                id: "m1".into(),
                peer_npub: peer.clone(),
                content: "hi".into(),
                created_at: 10,
                outgoing: false,
                status: None,
                reply_to: None,
            })
            .unwrap();
        // Simulate a discovered/refreshed profile in the cache.
        store
            .put(
                PEER_PROFILES_TREE,
                &peer,
                &PeerProfileRecord {
                    name: Some("charlie".into()),
                    about: None,
                    updated_at: 1,
                },
            )
            .unwrap();

        let convos = rt.conversations().unwrap();
        assert_eq!(convos.len(), 1);
        assert_eq!(convos[0].alias, None, "no user alias was set");
        assert_eq!(
            convos[0].peer_name.as_deref(),
            Some("charlie"),
            "published handle from the profile cache titles the chat"
        );

        rt.add_contact(&peer, "").unwrap();
        let contacts = rt.list_contacts().unwrap();
        assert_eq!(contacts[0].name.as_deref(), Some("charlie"));

        // A user alias always outranks the published handle in the DTO —
        // display precedence is enforced by returning both.
        rt.set_contact_alias(&peer, "My Buddy").unwrap();
        let convos = rt.conversations().unwrap();
        assert_eq!(convos[0].alias.as_deref(), Some("My Buddy"));
        assert_eq!(convos[0].peer_name.as_deref(), Some("charlie"));
    }

    #[tokio::test]
    async fn conversations_group_history_by_peer_newest_first() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let store = rt.ui.store_ref().unwrap();

        let alice = nostr_sdk::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        let bob = nostr_sdk::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        for (id, peer, content, at, outgoing) in [
            ("m1", &alice, "hi alice", 10u64, true),
            ("m2", &alice, "hello back", 20, false),
            ("m3", &bob, "yo bob", 15, true),
        ] {
            store
                .save_message(&comrade_storage::StoredMessage {
                    id: id.into(),
                    peer_npub: peer.to_string(),
                    content: content.into(),
                    created_at: at,
                    outgoing,
                    status: None,
                    reply_to: None,
                })
                .unwrap();
        }
        rt.add_contact(&alice, "Alice").unwrap();

        let convos = rt.conversations().unwrap();
        assert_eq!(convos.len(), 2);
        // Alice's thread is newest (t=20) and carries her saved alias.
        assert_eq!(convos[0].peer, alice);
        assert_eq!(convos[0].alias.as_deref(), Some("Alice"));
        assert_eq!(convos[0].last_message, "hello back");
        assert!(!convos[0].last_outgoing);
        assert_eq!(convos[1].peer, bob);
        assert_eq!(convos[1].alias, None);

        // Per-thread history comes back oldest-first for rendering.
        let msgs = rt.messages_with(&alice).unwrap();
        assert_eq!(
            msgs.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["m1", "m2"]
        );
    }

    #[tokio::test]
    async fn dm_and_profile_commands_reject_when_locked() {
        let mut rt = ComradeRuntime::new();
        assert!(matches!(
            rt.send_dm("npub1x", "hi").await,
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(rt.conversations(), Err(UiError::VaultLocked)));
        assert!(matches!(rt.messages_with("x"), Err(UiError::VaultLocked)));
        assert!(matches!(rt.list_contacts(), Err(UiError::VaultLocked)));
        assert!(matches!(rt.profile(), Err(UiError::NoIdentity)));
        assert!(matches!(
            rt.set_username("neo").await,
            Err(UiError::NoIdentity)
        ));
        assert!(matches!(
            rt.search_profiles("neo").await,
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.refresh_peer_profiles().await,
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.set_contact_alias("npub1x", "a"),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.remove_contact("npub1x"),
            Err(UiError::VaultLocked)
        ));
    }

    #[tokio::test]
    async fn send_dm_rejects_empty_and_bad_recipient() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert!(rt.send_dm("npub1notvalid", "hello").await.is_err());
        let ok = nostr_sdk::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        assert!(matches!(
            rt.send_dm(&ok, "   ").await,
            Err(UiError::Engine(_))
        ));
    }

    #[tokio::test]
    async fn media_send_rejects_when_locked() {
        // No identity/engines yet → graceful typed error, no panic.
        let rt = ComradeRuntime::new();
        let err = rt
            .upload_and_send_media("npub1xxx", vec![1, 2, 3], "image/png", "")
            .await;
        assert!(err.is_err());
        let err = rt.download_and_decrypt_media("deadbeef").await;
        assert!(matches!(err, Err(UiError::VaultLocked)));
    }

    #[tokio::test]
    async fn media_send_pipeline_to_self_without_http_feature() {
        // Exercises identity + ECDH key derivation + encrypt + local-ref logic
        // up to the network boundary. With `media-http` off, the upload step
        // returns a typed error (no panic); the run never touches a relay.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        let id = rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let res = rt
            .upload_and_send_media(&id.npub, vec![9, 8, 7, 6], "image/png", "selfie")
            .await;
        #[cfg(not(feature = "media-http"))]
        assert!(matches!(res, Err(UiError::Engine(_))));
        // (With the feature on this would attempt a real Blossom upload.)
        let _ = res;
    }

    #[tokio::test]
    async fn download_resolves_persisted_ref() {
        // A persisted ref is resolved and key-derivation runs; only the final
        // network fetch is gated by the feature.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        let id = rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let peer_hex = nostr_sdk::PublicKey::parse(&id.npub).unwrap().to_hex();
        let reff = MediaRef {
            event_id: "evt1".into(),
            url: "https://blob.example/abc".into(),
            peer_pubkey: peer_hex,
            mime_type: "image/png".into(),
            caption: "x".into(),
            size: 3,
            sha256_hex: String::new(),
            outgoing: false,
            created_at: 1,
        };
        rt.ui
            .store_ref()
            .unwrap()
            .put(MEDIA_REFS_TREE, "evt1", &reff)
            .unwrap();

        let out = rt.download_and_decrypt_media("evt1").await;
        // Unknown id is a clean error; known id reaches the (gated) fetch step.
        assert!(rt.download_and_decrypt_media("nope").await.is_err());
        #[cfg(not(feature = "media-http"))]
        assert!(matches!(out, Err(UiError::Engine(_))));
        let _ = out;
    }

    #[tokio::test]
    async fn media_with_lists_history_oldest_first_with_correct_direction() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        let id = rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (peer_hex, peer_npub) = stranger();

        let incoming = MediaRef {
            event_id: "evt_in".into(),
            url: "https://blob.example/in".into(),
            peer_pubkey: peer_hex.clone(),
            mime_type: "image/png".into(),
            caption: "from them".into(),
            size: 3,
            sha256_hex: String::new(),
            outgoing: false,
            created_at: 10,
        };
        let outgoing = MediaRef {
            event_id: "evt_out".into(),
            url: "https://blob.example/out".into(),
            peer_pubkey: peer_hex,
            mime_type: "audio/ogg".into(),
            caption: "from me".into(),
            size: 5,
            sha256_hex: String::new(),
            outgoing: true,
            created_at: 20,
        };
        let store = rt.ui.store_ref().unwrap();
        store.put(MEDIA_REFS_TREE, "evt_in", &incoming).unwrap();
        store.put(MEDIA_REFS_TREE, "evt_out", &outgoing).unwrap();

        let history = rt.media_with(&peer_npub).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].event_id, "evt_in");
        assert!(!history[0].outgoing);
        assert_eq!(history[0].sender, peer_npub);
        assert_eq!(history[1].event_id, "evt_out");
        assert!(history[1].outgoing);
        assert_eq!(history[1].sender, id.npub);
    }

    #[tokio::test]
    async fn media_with_rejects_when_locked_and_is_empty_for_a_stranger() {
        let rt = ComradeRuntime::new();
        let (_, peer_npub) = stranger();
        assert!(matches!(
            rt.media_with(&peer_npub),
            Err(UiError::VaultLocked)
        ));

        let dir = TempDir::new().unwrap();
        let mut rt2 = ComradeRuntime::new();
        rt2.unlock_vault(dir.path(), "pin").await.unwrap();
        assert!(rt2.media_with(&peer_npub).unwrap().is_empty());
    }

    // ── Message requests, receipts, and calls ────────────────────────────────

    fn stranger() -> (String, String) {
        let pk = nostr_sdk::Keys::generate().public_key();
        (pk.to_hex(), pk.to_bech32().unwrap())
    }

    #[tokio::test]
    async fn strangers_are_gated_into_requests_then_accepted() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let store = rt.ui.store_ref().unwrap();

        // Simulate the inbox loop recording a stranger's DM: pending + message.
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "pending".into(),
                profile_shared: false,
                updated_at: 1,
            })
            .unwrap();
        store
            .save_message(&comrade_storage::StoredMessage {
                id: "in1".into(),
                peer_npub: peer.clone(),
                content: "hi, can we talk?".into(),
                created_at: 5,
                outgoing: false,
                status: None,
                reply_to: None,
            })
            .unwrap();

        // Gated out of the chat list; present as a request.
        assert!(rt.conversations().unwrap().is_empty());
        let reqs = rt.message_requests().unwrap();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].peer, peer);
        assert_eq!(reqs[0].last_message, "hi, can we talk?");

        // Accept → into the chat list, out of requests.
        rt.accept_request(&peer).unwrap();
        assert!(rt.message_requests().unwrap().is_empty());
        let convos = rt.conversations().unwrap();
        assert_eq!(convos.len(), 1);
        assert_eq!(convos[0].peer, peer);
    }

    #[tokio::test]
    async fn blocked_peer_is_hidden_from_both_lists() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        rt.ui
            .store_ref()
            .unwrap()
            .save_message(&comrade_storage::StoredMessage {
                id: "in1".into(),
                peer_npub: peer.clone(),
                content: "spam".into(),
                created_at: 1,
                outgoing: false,
                status: None,
                reply_to: None,
            })
            .unwrap();
        rt.block_conversation(&peer).unwrap();
        assert!(rt.conversations().unwrap().is_empty());
        assert!(rt.message_requests().unwrap().is_empty());
    }

    #[tokio::test]
    async fn call_log_roundtrips_per_peer_and_globally() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let rec = rt
            .log_call(&peer, "call1", "video", true, "connected", 100, 42)
            .unwrap();
        assert_eq!(rec.media, "video");
        assert_eq!(rec.outcome, "connected");
        assert_eq!(rec.duration_secs, 42);
        assert_eq!(rt.call_history(Some(&peer)).unwrap().len(), 1);
        assert_eq!(rt.call_history(None).unwrap().len(), 1);
        // Unknown media string is coerced to audio.
        let rec2 = rt
            .log_call(&peer, "call2", "hologram", false, "missed", 0, 0)
            .unwrap();
        assert_eq!(rec2.media, "audio");
    }

    #[tokio::test]
    async fn ice_servers_default_stun_and_configurable_turn() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let defaults = rt.call_ice_servers();
        assert!(!defaults.is_empty());
        assert!(defaults.iter().all(|s| s.username.is_none()));
        rt.set_turn_server("turn:turn.example.com:3478", "u", "p")
            .unwrap();
        let with_turn = rt.call_ice_servers();
        assert_eq!(with_turn.len(), defaults.len() + 1);
        assert_eq!(with_turn.last().unwrap().username.as_deref(), Some("u"));
        rt.set_turn_server("", "", "").unwrap();
        assert_eq!(rt.call_ice_servers().len(), defaults.len());
    }

    #[tokio::test]
    async fn call_ice_servers_for_stun_only_never_leaks_turn_credentials() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        rt.set_turn_server("turn:turn.example.com:3478", "u", "p")
            .unwrap();

        // Even with a TURN relay configured, an explicit stun_only ask (what
        // place_call uses) must never include it.
        let stun_only = rt.call_ice_servers_for("stun_only");
        assert!(stun_only.iter().all(|s| s.username.is_none()));

        // The fallback a frontend calls after ICE fails to connect does
        // include it.
        let fallback = rt.call_ice_servers_for("stun_and_turn");
        assert_eq!(fallback.last().unwrap().username.as_deref(), Some("u"));
        assert_eq!(fallback.len(), stun_only.len() + 1);

        // Garbage input defaults to the private stun_only behavior.
        assert_eq!(rt.call_ice_servers_for("nonsense").len(), stun_only.len());
    }

    #[tokio::test]
    async fn place_call_starts_stun_only_even_with_turn_configured() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        rt.set_turn_server("turn:turn.example.com:3478", "u", "p")
            .unwrap();

        let (_hex, peer) = stranger();
        let session = rt.place_call(&peer, "audio").unwrap();
        assert!(
            session.ice_servers.iter().all(|s| s.username.is_none()),
            "the initial offer must not contact the TURN relay unless STUN fails"
        );
    }

    #[tokio::test]
    async fn place_call_mints_session_and_validates() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        assert!(matches!(
            rt.place_call("npub1x", "audio"),
            Err(UiError::VaultLocked)
        ));
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let session = rt.place_call(&peer, "video").unwrap();
        assert_eq!(session.peer, peer);
        assert_eq!(session.media, "video");
        assert_eq!(session.call_id.len(), 32);
        assert!(!session.ice_servers.is_empty());
        assert!(rt.place_call("not-a-key", "audio").is_err());
    }

    #[tokio::test]
    async fn send_call_signal_rejects_malformed_json() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let err = rt
            .send_call_signal(&peer, "c1", "audio", "{not valid")
            .await;
        assert!(matches!(err, Err(UiError::Engine(_))));
    }

    async fn test_vault() -> Arc<VaultEngine> {
        let keys = nostr_sdk::Keys::generate();
        Arc::new(
            VaultEngine::new(&keys, vec!["wss://relay.damus.io".into()])
                .await
                .unwrap(),
        )
    }

    fn incoming(sender_hex: &str, event_id: &str, content: &str) -> VaultMessage {
        VaultMessage {
            event_id: event_id.into(),
            sender_pubkey: sender_hex.into(),
            content: content.into(),
            created_at: 3,
            upi_intents: vec![],
            reply_to: None,
        }
    }

    #[tokio::test]
    async fn dispatch_gates_unknown_sender_as_request() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let (hex, peer) = stranger();

        dispatch_incoming_dm(&vault, Some(&store), &tx, incoming(&hex, "e1", "hello?"));

        assert_eq!(
            store.get_conversation_meta(&peer).unwrap().unwrap().state,
            "pending"
        );
        assert_eq!(store.messages_with(&peer).unwrap().len(), 1);
        match rx.try_recv().unwrap() {
            BridgeEvent::IncomingMessageRequest(r) => {
                assert_eq!(r.peer, peer);
                assert_eq!(r.last_message, "hello?");
            }
            other => panic!("expected request, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_delivers_accepted_and_advances_receipts() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let (hex, peer) = stranger();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "accepted".into(),
                profile_shared: true,
                updated_at: 1,
            })
            .unwrap();

        // Plain text from an accepted peer is delivered (not gated).
        dispatch_incoming_dm(&vault, Some(&store), &tx, incoming(&hex, "e1", "yo"));
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::IncomingDirectMessage(_)
        ));

        // A read receipt advances one of our outgoing messages.
        store
            .save_message(&comrade_storage::StoredMessage {
                id: "out1".into(),
                peer_npub: peer.clone(),
                content: "sup".into(),
                created_at: 2,
                outgoing: true,
                status: Some("sent".into()),
                reply_to: None,
            })
            .unwrap();
        let receipt = Receipt::new(ReceiptKind::Read, vec!["out1".into()])
            .to_json()
            .unwrap();
        dispatch_incoming_dm(&vault, Some(&store), &tx, incoming(&hex, "e2", &receipt));
        assert_eq!(
            store
                .get_message("out1")
                .unwrap()
                .unwrap()
                .status
                .as_deref(),
            Some("read")
        );
        match rx.try_recv().unwrap() {
            BridgeEvent::MessageStatus {
                message_ids,
                status,
                ..
            } => {
                assert_eq!(status, "read");
                assert_eq!(message_ids, vec!["out1".to_string()]);
            }
            other => panic!("expected MessageStatus, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_drops_blocked_and_caches_profile_share() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let (hex, peer) = stranger();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "blocked".into(),
                profile_shared: false,
                updated_at: 1,
            })
            .unwrap();
        dispatch_incoming_dm(&vault, Some(&store), &tx, incoming(&hex, "e1", "let me in"));
        assert!(store.messages_with(&peer).unwrap().is_empty());
        assert!(rx.try_recv().is_err(), "blocked peer emits nothing");

        // A profile share (any non-blocked peer) caches the name + emits update.
        let (other_hex, other_npub) = stranger();
        let share = ProfileShare::new(Some("charlie".into())).to_json().unwrap();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            incoming(&other_hex, "e2", &share),
        );
        match rx.try_recv().unwrap() {
            BridgeEvent::PeerProfileUpdated { peer, name } => {
                assert_eq!(peer, other_npub);
                assert_eq!(name.as_deref(), Some("charlie"));
            }
            other => panic!("expected PeerProfileUpdated, got {other:?}"),
        }
    }
}
