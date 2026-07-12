/*!
 * comrade_ui::runtime — the async IPC bridge orchestrator.
 *
 * [`ComradeRuntime`] is the live "runtime context" the Command & Event Bridge
 * manages behind an `Arc<RwLock<…>>`. It is the single, framework-agnostic
 * aggregate that both the **Tauri desktop** shell (`#[tauri::command]` wrappers)
 * and the **Android JNI** layer (`extern "C"` exports) drive — keeping all real
 * logic inside the workspace where it is unit-tested and Send/Sync-checked.
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
 *    failure becomes a `Promise.reject` (Tauri) or JSON error payload (JNI).
 *  • Heavy work (relay connect, feed subscription, DM decryption) runs in
 *    spawned Tokio tasks via [`ComradeRuntime::spawn_event_loops`], never on the
 *    caller's UI thread.
 */

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use comrade_core::crypto::derive_media_key;
use comrade_core::media::{
    build_file_metadata_event, encrypt_media, fetch_and_decrypt_media, FileMetadata,
    MAX_MEDIA_BYTES,
};
use comrade_core::sabha::{display_name_of, ChitthiCallback, SabhaEngine, DEFAULT_RELAYS};
use comrade_core::sakha::SakhaEngine;
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

// ── Event DTOs (serialised across the IPC / FFI boundary) ────────────────────

/// A public Chitthi (Kind-1) as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectMessageDto {
    pub id: String,
    pub sender: String,
    pub content: String,
    pub created_at: u64,
    pub upi_intents: Vec<UpiIntentDto>,
}

impl From<VaultMessage> for DirectMessageDto {
    fn from(m: VaultMessage) -> Self {
        Self {
            id: m.event_id,
            sender: to_npub(&m.sender_pubkey),
            content: m.content,
            created_at: m.created_at,
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

/// A NIP-94 encrypted-media reference as the frontend sees it (no key material).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// Decrypted media handed back to the frontend. Bytes are base64-encoded so the
/// IPC payload stays compact (the webview rebuilds a `Blob` from it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaBytesDto {
    pub mime_type: String,
    pub base64: String,
}

/// Locally persisted pointer to an encrypted blob, keyed by NIP-94 event id.
/// Holds everything needed to *re-derive* the key — but never the key itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MediaRef {
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDto {
    pub npub: String,
    pub username: Option<String>,
}

/// A profile discovered via relay search. `npub` is the identity; `name` is a
/// self-declared, non-unique handle — the UI must always show both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundProfileDto {
    pub npub: String,
    pub name: Option<String>,
    pub about: Option<String>,
}

/// A saved contact: an npub pinned on first add (trust-on-first-use) with a
/// local alias. A different key later claiming the same handle can never
/// silently replace this entry — contacts are keyed by npub, not by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactDto {
    pub npub: String,
    /// The *user-chosen* local alias (petname). Empty = none set.
    pub alias: String,
    /// The peer's own published @handle, from the local profile cache.
    /// Display precedence is alias → name → key; never trust name alone.
    pub name: Option<String>,
}

/// One entry of the chat list: a peer plus the newest message in the thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// A single direct message in a conversation, from the offline history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageDto {
    pub id: String,
    /// Peer npub the thread is keyed by (sender if incoming, recipient if outgoing).
    pub peer: String,
    pub content: String,
    pub created_at: u64,
    pub outgoing: bool,
}

/// A push event emitted by the background Tokio loops and forwarded across the
/// webview boundary (`window.emit`) or polled over JNI. Internally tagged so the
/// frontend can `switch (evt.type)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// A new public Chitthi (Kind-1) arrived on the Sabha timeline.
    IncomingChitthi(ChitthiDto),
    /// A new encrypted DM (Kind-4) was decrypted in the Vault inbox.
    IncomingDirectMessage(DirectMessageDto),
    /// A new encrypted-media reference (NIP-94) arrived over the DM channel.
    IncomingMedia(MediaMessageDto),
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
    events: broadcast::Sender<BridgeEvent>,
    /// Guards [`spawn_event_loops`] against re-spawning the feed/DM tasks if it
    /// is called more than once. [`spawn_event_loops`]: ComradeRuntime::spawn_event_loops
    loops_spawned: bool,
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
            events,
            loops_spawned: false,
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
            tokio::spawn(async move {
                vault.connect().await;
                let cb: VaultCallback = Box::new(move |msg| {
                    // A media envelope rides inside an ordinary E2E DM. Surface it
                    // as a media event (and persist the ref so it can later be
                    // decrypted by event id); everything else is a plain DM.
                    if let Some(env) = parse_media_envelope(&msg.content) {
                        if let Some(store) = store.as_ref() {
                            let reff = MediaRef {
                                url: env.url.clone(),
                                peer_pubkey: msg.sender_pubkey.clone(),
                                mime_type: env.mime.clone(),
                                caption: env.caption.clone(),
                                size: env.size,
                                sha256_hex: env.sha256_hex.clone(),
                                outgoing: false,
                                created_at: msg.created_at,
                            };
                            // A dropped ref means download_and_decrypt_media
                            // later can't resolve this event — surface it rather
                            // than silently losing the media.
                            if let Err(e) = store
                                .put(MEDIA_REFS_TREE, &env.event_id, &reff)
                                .and_then(|()| store.flush())
                            {
                                warn!("failed to persist incoming media ref: {e}");
                            }
                        }
                        let _ = tx.send(BridgeEvent::IncomingMedia(MediaMessageDto {
                            event_id: env.event_id,
                            url: env.url,
                            mime_type: env.mime,
                            caption: env.caption,
                            sender: to_npub(&msg.sender_pubkey),
                            created_at: msg.created_at,
                            size: env.size,
                        }));
                    } else {
                        // Persist plain DMs so conversations survive restarts —
                        // the chat list is rebuilt from this offline history.
                        if let Some(store) = store.as_ref() {
                            let row = comrade_storage::StoredMessage {
                                id: msg.event_id.clone(),
                                peer_npub: to_npub(&msg.sender_pubkey),
                                content: msg.content.clone(),
                                created_at: msg.created_at,
                                outgoing: false,
                            };
                            if let Err(e) = store.save_message(&row).and_then(|()| store.flush()) {
                                warn!("failed to persist incoming DM: {e}");
                            }
                        }
                        let _ = tx.send(BridgeEvent::IncomingDirectMessage(
                            DirectMessageDto::from(msg),
                        ));
                    }
                });
                if let Err(e) = vault.subscribe_inbox_with_callback(cb).await {
                    warn!("vault inbox loop ended: {e}");
                }
            });
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
        if content.trim().is_empty() {
            return Err(UiError::Engine("message is empty".into()));
        }
        let vault = self.vault.clone().ok_or(UiError::VaultLocked)?;
        let peer = parse_pubkey(target)?;
        let id = vault
            .send_dm(&peer, content)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;

        let dto = MessageDto {
            id: id.to_hex(),
            peer: to_npub(target),
            content: content.to_string(),
            created_at: now_secs(),
            outgoing: true,
        };
        if let Some(store) = self.ui.store_ref() {
            let row = comrade_storage::StoredMessage {
                id: dto.id.clone(),
                peer_npub: dto.peer.clone(),
                content: dto.content.clone(),
                created_at: dto.created_at,
                outgoing: true,
            };
            if let Err(e) = store.save_message(&row).and_then(|()| store.flush()) {
                warn!("failed to persist outgoing DM: {e}");
            }
        }
        Ok(dto)
    }

    /// The chat list: one entry per peer, newest thread first, with saved
    /// contact aliases joined in. Built from the offline message history.
    pub fn conversations(&self) -> Result<Vec<ConversationDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let aliases: std::collections::HashMap<String, String> = store
            .list_contacts()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(|c| (c.npub, c.petname))
            .collect();

        let mut newest: std::collections::HashMap<String, comrade_storage::StoredMessage> =
            std::collections::HashMap::new();
        for msg in store
            .all_messages()
            .map_err(|e| UiError::Storage(e.to_string()))?
        {
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

    /// Full offline message history with `peer` (npub or hex), oldest first.
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
                outgoing: m.outgoing,
            })
            .collect();
        msgs.sort_by_key(|m| m.created_at);
        Ok(msgs)
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

    /// Upload an encrypted blob to Blossom, signed with a BUD-01 auth event.
    /// Gated on the `media-http` feature; degrades to a typed error otherwise.
    ///
    /// The BUD-01 auth event is signed with a **fresh ephemeral key**, never the
    /// user's chat identity: the blob is already zero-knowledge, and signing
    /// with the identity key would let the host link "npub X uploaded blob Y at
    /// time T from IP Z" — a metadata leak at odds with the privacy model.
    #[cfg(feature = "media-http")]
    async fn upload_blob(&self, blob: Vec<u8>, mime: &str) -> Result<String, UiError> {
        use comrade_core::media::{BlossomUploader, MediaUploader, DEFAULT_BLOSSOM_SERVER};
        let uploader = BlossomUploader::new(DEFAULT_BLOSSOM_SERVER, nostr_sdk::Keys::generate());
        let receipt = uploader
            .upload(&blob, mime)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;
        Ok(receipt.url)
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
    pub fn toggle_workspace(&mut self, target: &str) -> Result<WorkspaceDto, UiError> {
        self.ui.switch_workspace(target)
    }

    /// Step back to the previous workspace.
    pub fn back(&mut self) -> WorkspaceDto {
        self.ui.back()
    }

    // ── Milestone 1: Sakha/Sakhi CRDT ledger sync ────────────────────────────

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
    fn toggle_workspace_enforces_state_machine() {
        let mut rt = ComradeRuntime::new();
        let dto = rt.toggle_workspace("OffGridTravel").unwrap();
        assert_eq!(dto.key, "OffGridTravel");
        assert!(dto.mesh_active);
        // OffGridTravel -> CoupleSandbox is blocked by the transition graph.
        assert!(matches!(
            rt.toggle_workspace("CoupleSandboxSakha"),
            Err(UiError::Transition(_))
        ));
        // Unknown keys are a distinct typed error.
        assert!(matches!(
            rt.toggle_workspace("Nope"),
            Err(UiError::UnknownWorkspace(_))
        ));
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
}
