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
use comrade_core::sabha::{ChitthiCallback, SabhaEngine, DEFAULT_RELAYS};
use comrade_core::sakha::SakhaEngine;
use comrade_core::vault::{VaultCallback, VaultEngine, VaultMessage};
use nostr_sdk::{EventId, PublicKey, ToBech32};
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
            sender: m.sender_pubkey,
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
        }
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
    /// [`spawn_event_loops`]: ComradeRuntime::spawn_event_loops
    pub async fn unlock_vault(
        &mut self,
        path: impl AsRef<std::path::Path>,
        passphrase: &str,
    ) -> Result<IdentityDto, UiError> {
        self.ui.unlock_store(path, passphrase)?;

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
    pub fn spawn_event_loops(&self) {
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
                            sender: msg.sender_pubkey,
                            created_at: msg.created_at,
                            size: env.size,
                        }));
                    } else {
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
