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

use comrade_core::pukar::{CallCallback, CallEvent, CallMedia, CallSession, PukarEngine};
use comrade_core::sabha::{ChitthiCallback, SabhaEngine, DEFAULT_RELAYS};
use comrade_core::sakha::SakhaEngine;
use comrade_core::vault::{VaultCallback, VaultEngine, VaultMessage};
use nostr_sdk::{EventId, ToBech32};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::warn;

use crate::{IdentityDto, UiError, UiService, UpiIntentDto, WorkspaceDto};

/// Capacity of the event bus. Slow consumers lag rather than block producers —
/// the relay loop never stalls waiting on the webview. Sized for the public
/// Kind-1 firehose sharing the bus with DMs and call signals; lagged consumers
/// recover via the encrypted caches (timeline, DM history, call log).
const EVENT_BUS_CAPACITY: usize = 1024;

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
            // Thread live events exactly like the cached path does.
            reply_to: comrade_core::sabha::resolve_parent_id(event),
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
    /// A call state change (incoming ring, answer, ICE, connect, end). The
    /// inner [`CallEvent`] is itself `type`-tagged for the frontend switch.
    Call { call: CallEvent },
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
    pukar: Option<Arc<PukarEngine>>,
    events: broadcast::Sender<BridgeEvent>,
    /// Set once [`spawn_event_loops`](Self::spawn_event_loops) has run, so a
    /// second unlock can never spawn duplicate loops onto the same bus.
    loops_started: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
            pukar: None,
            events,
            loops_started: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        // Idempotent: a second unlock (e.g. Android activity re-creation)
        // must not rebuild engines — the old ones own live subscriptions, and
        // duplicates would double-deliver every event.
        if self.is_vault_unlocked() {
            if let Some(identity) = self.ui.current_identity() {
                return Ok(identity);
            }
        }
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
            VaultEngine::new(&keys, relays.clone())
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?,
        ));
        self.sakha = Some(Arc::new(
            SakhaEngine::new(&keys, vec![])
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?,
        ));
        let pukar = Arc::new(
            PukarEngine::new(&keys, relays)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?,
        );
        // Incoming-call consent gate: ring only for saved contacts. An app
        // that wants open inbound calling opts in via `set_call_policy`.
        pukar.set_policy(self.contact_call_policy());
        self.pukar = Some(pukar);

        Ok(identity)
    }

    /// Build the contacts-only [`CallPolicy`] from the store's saved contacts
    /// (their pubkeys normalised to hex, matching signal sender identities).
    ///
    /// [`CallPolicy`]: comrade_core::pukar::CallPolicy
    fn contact_call_policy(&self) -> comrade_core::pukar::CallPolicy {
        let mut allowed = std::collections::HashSet::new();
        if let Some(store) = self.ui.store_ref() {
            if let Ok(contacts) = store.list_contacts() {
                for c in contacts {
                    if let Ok(pk) = nostr_sdk::PublicKey::parse(&c.npub) {
                        allowed.insert(pk.to_hex());
                    }
                }
            }
        }
        comrade_core::pukar::CallPolicy::ContactsOnly(allowed)
    }

    /// Override who may ring this device (see [`comrade_core::pukar::CallPolicy`]).
    pub fn set_call_policy(&self, policy: comrade_core::pukar::CallPolicy) -> Result<(), UiError> {
        let engine = self.pukar.as_ref().ok_or(UiError::VaultLocked)?;
        engine.set_policy(policy);
        Ok(())
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
    /// Every subscription is **supervised**: `handle_notifications` returns
    /// `Ok(())` when its internal channel lags (a steady-state condition on
    /// the public Kind-1 firehose), which previously killed the loop silently
    /// — the app would just never receive another event until restart. Each
    /// loop therefore reconnects and resubscribes with a backoff, forever.
    pub fn spawn_event_loops(&self) {
        // A second unlock must never double-spawn onto the same bus.
        if self
            .loops_started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            warn!("event loops already running; skipping duplicate spawn");
            return;
        }
        const RESUBSCRIBE_BACKOFF: std::time::Duration = std::time::Duration::from_secs(10);

        if let Some(sabha) = self.sabha.clone() {
            let tx = self.events.clone();
            let store = self.ui.store_arc();
            tokio::spawn(async move {
                sabha.connect().await;
                loop {
                    let tx = tx.clone();
                    let store = store.clone();
                    let cb: ChitthiCallback = Box::new(move |event| {
                        // Persist to the encrypted cache so the offline
                        // timeline holds more than the user's own posts and
                        // survives event-bus lag.
                        if let Some(store) = &store {
                            let row = comrade_storage::Chitthi {
                                id: event.id.to_hex(),
                                author_npub: event
                                    .pubkey
                                    .to_bech32()
                                    .unwrap_or_else(|_| event.pubkey.to_hex()),
                                content: event.content.clone(),
                                created_at: event.created_at.as_secs(),
                                reply_to: comrade_core::sabha::resolve_parent_id(&event),
                            };
                            if let Err(e) = store.cache_chitthi(&row) {
                                warn!("failed to cache incoming chitthi: {e}");
                            }
                        }
                        // A send error only means no subscribers are listening
                        // yet; the relay loop must keep running regardless.
                        let _ =
                            tx.send(BridgeEvent::IncomingChitthi(ChitthiDto::from_event(&event)));
                    });
                    match sabha.subscribe_chitthi_feed(3600, cb).await {
                        Ok(()) => warn!("sabha feed loop lagged out; resubscribing"),
                        Err(e) => warn!("sabha feed loop ended: {e}; resubscribing"),
                    }
                    tokio::time::sleep(RESUBSCRIBE_BACKOFF).await;
                }
            });
        }

        if let Some(vault) = self.vault.clone() {
            let tx = self.events.clone();
            let store = self.ui.store_arc();
            tokio::spawn(async move {
                vault.connect().await;
                loop {
                    let tx = tx.clone();
                    let store = store.clone();
                    let cb: VaultCallback = Box::new(move |msg| {
                        // Persist decrypted DMs: if the UI misses the bus
                        // event (lag, backgrounded app), the message is still
                        // recoverable from the encrypted vault cache.
                        if let Some(store) = &store {
                            let row = comrade_storage::StoredMessage {
                                id: msg.event_id.clone(),
                                peer_npub: msg.sender_pubkey.clone(),
                                content: msg.content.clone(),
                                created_at: msg.created_at,
                                outgoing: false,
                            };
                            if let Err(e) = store.save_message(&row) {
                                warn!("failed to cache incoming DM: {e}");
                            }
                        }
                        let _ = tx.send(BridgeEvent::IncomingDirectMessage(
                            DirectMessageDto::from(msg),
                        ));
                    });
                    match vault.subscribe_inbox_with_callback(cb).await {
                        Ok(()) => warn!("vault inbox loop lagged out; resubscribing"),
                        Err(e) => warn!("vault inbox loop ended: {e}; resubscribing"),
                    }
                    tokio::time::sleep(RESUBSCRIBE_BACKOFF).await;
                }
            });
        }

        if let Some(pukar) = self.pukar.clone() {
            // Incoming signaling → state machine → UI events.
            let tx = self.events.clone();
            let store = self.ui.store_arc();
            let engine = pukar.clone();
            tokio::spawn(async move {
                engine.connect().await;
                loop {
                    let tx = tx.clone();
                    let store = store.clone();
                    let cb: CallCallback = Box::new(move |call| {
                        if let (Some(store), CallEvent::CallEnded { call }) = (&store, &call) {
                            persist_call(store, call);
                        }
                        let _ = tx.send(BridgeEvent::Call { call });
                    });
                    match engine.subscribe_signals(cb).await {
                        Ok(()) => warn!("pukar signaling loop lagged out; resubscribing"),
                        Err(e) => warn!("pukar signaling loop ended: {e}; resubscribing"),
                    }
                    tokio::time::sleep(RESUBSCRIBE_BACKOFF).await;
                }
            });

            // Ring/connect-timeout ticker (~5 s granularity suffices for a
            // 60 s ring). Also leaves a persistent "missed call" DM for
            // callees who were offline — ephemeral signals leave no trace.
            let tx = self.events.clone();
            let store = self.ui.store_arc();
            let vault = self.vault.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    interval.tick().await;
                    for call in pukar.tick().await {
                        if let CallEvent::CallEnded { call: session } = &call {
                            if let Some(store) = &store {
                                persist_call(store, session);
                            }
                            if session.cause == Some(comrade_core::pukar::EndCause::NoAnswer) {
                                notify_missed_call(vault.as_deref(), session).await;
                            }
                        }
                        let _ = tx.send(BridgeEvent::Call { call });
                    }
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
        self.broadcast_chitthi_task(content.to_string(), reply_to)?
            .await
    }

    /// Detached variant of [`broadcast_chitthi`](Self::broadcast_chitthi):
    /// captures everything it needs up front and returns a `'static` future.
    /// FFI bridges hold the shared `RwLock<ComradeRuntime>` only while
    /// *building* the task, not across the multi-second relay send — holding
    /// it that long stalls every other bridge call behind the fair lock.
    pub fn broadcast_chitthi_task(
        &self,
        content: String,
        reply_to: Option<String>,
    ) -> Result<impl std::future::Future<Output = Result<String, UiError>> + Send + 'static, UiError>
    {
        let sabha = self.sabha.clone().ok_or(UiError::VaultLocked)?;
        let store = self.ui.store_arc();
        let author_npub = self
            .ui
            .current_identity()
            .map(|i| i.npub)
            .unwrap_or_default();

        let parent = match reply_to.as_deref() {
            Some(hex) => Some(
                EventId::from_hex(hex)
                    .map_err(|e| UiError::Engine(format!("invalid reply_to id: {e}")))?,
            ),
            None => None,
        };

        Ok(async move {
            let id = sabha
                .broadcast_chitthi_reply(&content, parent)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?;

            // Best-effort: persist our own Chitthi to the encrypted cache so
            // it shows up in the offline timeline immediately.
            if let Some(store) = store {
                let row = comrade_storage::Chitthi {
                    id: id.to_hex(),
                    author_npub,
                    content,
                    created_at: now_secs(),
                    reply_to,
                };
                if let Err(e) = store.cache_chitthi(&row).and_then(|()| store.flush()) {
                    warn!("failed to cache outgoing chitthi: {e}");
                }
            }

            Ok(id.to_hex())
        })
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

    // ── Pukar: audio/video calls ─────────────────────────────────────────────

    /// Start an audio (`video = false`) or video call to `peer` (npub or hex).
    /// `sdp_offer` comes from the platform's WebRTC stack. Returns the session
    /// (with its `call_id`) while the peer's device rings.
    pub async fn place_call(
        &self,
        peer: &str,
        video: bool,
        sdp_offer: &str,
    ) -> Result<CallSession, UiError> {
        self.place_call_task(peer, video, sdp_offer.to_string())?
            .await
    }

    /// Detached variant of [`place_call`](Self::place_call) — see
    /// [`broadcast_chitthi_task`](Self::broadcast_chitthi_task) for why FFI
    /// bridges must not hold the shared runtime lock across relay sends.
    pub fn place_call_task(
        &self,
        peer: &str,
        video: bool,
        sdp_offer: String,
    ) -> Result<
        impl std::future::Future<Output = Result<CallSession, UiError>> + Send + 'static,
        UiError,
    > {
        let engine = self.pukar.clone().ok_or(UiError::VaultLocked)?;
        let peer = parse_pubkey(peer)?;
        let media = if video {
            CallMedia::Video
        } else {
            CallMedia::Audio
        };
        Ok(async move {
            engine
                .place_call(&peer, media, &sdp_offer)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))
        })
    }

    /// Accept the ringing incoming call with the platform's SDP answer. ICE
    /// candidates withheld while ringing are flushed onto the event bus.
    pub async fn answer_call(&self, call_id: &str, sdp_answer: &str) -> Result<(), UiError> {
        self.answer_call_task(call_id, sdp_answer.to_string())?
            .await
    }

    /// Detached variant of [`answer_call`](Self::answer_call).
    pub fn answer_call_task(
        &self,
        call_id: &str,
        sdp_answer: String,
    ) -> Result<impl std::future::Future<Output = Result<(), UiError>> + Send + 'static, UiError>
    {
        let engine = self.pukar.clone().ok_or(UiError::VaultLocked)?;
        let events = self.events.clone();
        let call_id = call_id.to_string();
        Ok(async move {
            let flushed = engine
                .accept(&call_id, &sdp_answer)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?;
            for call in flushed {
                let _ = events.send(BridgeEvent::Call { call });
            }
            Ok(())
        })
    }

    /// Decline the ringing incoming call. Emits `CallEnded` so the UI can
    /// drive its whole call screen from the event stream alone.
    pub async fn decline_call(&self, call_id: &str) -> Result<(), UiError> {
        self.decline_call_task(call_id)?.await
    }

    /// Detached variant of [`decline_call`](Self::decline_call).
    pub fn decline_call_task(
        &self,
        call_id: &str,
    ) -> Result<impl std::future::Future<Output = Result<(), UiError>> + Send + 'static, UiError>
    {
        let engine = self.pukar.clone().ok_or(UiError::VaultLocked)?;
        let events = self.events.clone();
        let store = self.ui.store_arc();
        let call_id = call_id.to_string();
        Ok(async move {
            let ended = engine
                .reject(&call_id)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?;
            finish_call(store.as_deref(), &events, ended);
            Ok(())
        })
    }

    /// Hang up the active call (or cancel an outgoing ring). Emits `CallEnded`.
    pub async fn end_call(&self, call_id: &str) -> Result<(), UiError> {
        self.end_call_task(call_id)?.await
    }

    /// Detached variant of [`end_call`](Self::end_call).
    pub fn end_call_task(
        &self,
        call_id: &str,
    ) -> Result<impl std::future::Future<Output = Result<(), UiError>> + Send + 'static, UiError>
    {
        let engine = self.pukar.clone().ok_or(UiError::VaultLocked)?;
        let events = self.events.clone();
        let store = self.ui.store_arc();
        let call_id = call_id.to_string();
        Ok(async move {
            let ended = engine
                .hangup(&call_id)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?;
            finish_call(store.as_deref(), &events, ended);
            Ok(())
        })
    }

    /// Forward a locally-gathered ICE candidate to the peer.
    pub async fn send_call_ice(
        &self,
        call_id: &str,
        candidate: &str,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u32>,
    ) -> Result<(), UiError> {
        self.send_call_ice_task(call_id, candidate.to_string(), sdp_mid, sdp_mline_index)?
            .await
    }

    /// Detached variant of [`send_call_ice`](Self::send_call_ice).
    pub fn send_call_ice_task(
        &self,
        call_id: &str,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u32>,
    ) -> Result<impl std::future::Future<Output = Result<(), UiError>> + Send + 'static, UiError>
    {
        let engine = self.pukar.clone().ok_or(UiError::VaultLocked)?;
        let call_id = call_id.to_string();
        Ok(async move {
            engine
                .send_ice(&call_id, &candidate, sdp_mid, sdp_mline_index)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))
        })
    }

    /// Platform WebRTC reports the media path is established. Emits
    /// `CallConnected` so every frontend surface sees the transition.
    pub fn call_connected(&self, call_id: &str) -> Result<CallSession, UiError> {
        let engine = self.pukar.as_ref().ok_or(UiError::VaultLocked)?;
        let session = engine
            .mark_connected(call_id)
            .map_err(|e| UiError::Engine(e.to_string()))?;
        let _ = self.events.send(BridgeEvent::Call {
            call: CallEvent::CallConnected {
                call_id: session.call_id.clone(),
            },
        });
        Ok(session)
    }

    /// The live call, if any.
    pub fn active_call(&self) -> Result<Option<CallSession>, UiError> {
        let engine = self.pukar.as_ref().ok_or(UiError::VaultLocked)?;
        Ok(engine.active_call())
    }

    /// Ended calls, newest first. Read from the encrypted store (survives
    /// restarts) when unlocked, falling back to the engine's in-memory log.
    pub fn call_log(&self) -> Result<Vec<CallSession>, UiError> {
        let engine = self.pukar.as_ref().ok_or(UiError::VaultLocked)?;
        if let Some(store) = self.ui.store_ref() {
            let mut log: Vec<CallSession> = store
                .values(CALL_LOG_TREE)
                .map_err(|e| UiError::Storage(e.to_string()))?;
            log.sort_by(|a, b| b.ended_at.cmp(&a.ended_at));
            return Ok(log);
        }
        Ok(engine.call_log())
    }

    // ── Companion (private, anonymous journal) ───────────────────────────────

    /// A supportive prompt for the given companion mode.
    pub fn companion_prompt(&self, mode: &str) -> Result<String, UiError> {
        self.ui.companion_prompt(mode)
    }

    /// Offline crisis-signal scan (no persistence, no network).
    pub fn scan_companion_text(&self, text: &str) -> comrade_core::companion::SafetyAssessment {
        self.ui.scan_companion_text(text)
    }

    /// Write an anonymous journal entry (typed or voice) into the encrypted
    /// store; returns the entry, a safety assessment, and the next prompt.
    pub fn write_journal_entry(
        &self,
        mode: &str,
        voice: bool,
        body: &str,
        mood: Option<i8>,
    ) -> Result<crate::CompanionResponse, UiError> {
        self.ui.write_journal_entry(mode, voice, body, mood)
    }

    /// All journal entries, newest first.
    pub fn list_journal_entries(
        &self,
    ) -> Result<Vec<comrade_core::companion::JournalEntry>, UiError> {
        self.ui.list_journal_entries()
    }

    /// Delete a journal entry by id.
    pub fn delete_journal_entry(&self, id: &str) -> Result<bool, UiError> {
        self.ui.delete_journal_entry(id)
    }

    /// On-device journaling insights. `tz_offset_secs` is the device's offset
    /// from UTC so streaks roll at local midnight.
    pub fn journal_insights(
        &self,
        tz_offset_secs: i32,
    ) -> Result<comrade_core::companion::Insights, UiError> {
        self.ui.journal_insights_at(tz_offset_secs)
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Parse an npub or hex public key into a [`nostr_sdk::PublicKey`].
fn parse_pubkey(s: &str) -> Result<nostr_sdk::PublicKey, UiError> {
    nostr_sdk::PublicKey::parse(s)
        .map_err(|e| UiError::Engine(format!("invalid peer public key: {e}")))
}

/// sled tree persisting ended calls (encrypted like everything else).
const CALL_LOG_TREE: &str = "call_log";

/// Best-effort write of an ended call into the encrypted call log.
fn persist_call(store: &comrade_storage::EncryptedStore, call: &CallSession) {
    if let Err(e) = store.put(CALL_LOG_TREE, &call.call_id, call) {
        warn!("failed to persist call log entry: {e}");
    }
}

/// Persist an ended session and emit its `CallEnded` bus event.
fn finish_call(
    store: Option<&comrade_storage::EncryptedStore>,
    events: &broadcast::Sender<BridgeEvent>,
    ended: Option<CallSession>,
) {
    if let Some(session) = ended {
        if let Some(store) = store {
            persist_call(store, &session);
        }
        let _ = events.send(BridgeEvent::Call {
            call: CallEvent::CallEnded { call: session },
        });
    }
}

/// Leave a persistent "missed call" DM for a callee who was offline while the
/// ephemeral signaling rang out — otherwise they would never learn about it.
async fn notify_missed_call(vault: Option<&VaultEngine>, session: &CallSession) {
    let Some(vault) = vault else { return };
    let Ok(peer) = nostr_sdk::PublicKey::from_hex(&session.peer) else {
        return;
    };
    let kind = match session.media {
        comrade_core::pukar::CallMedia::Audio => "audio",
        comrade_core::pukar::CallMedia::Video => "video",
    };
    let note = format!("\u{1F4DE} Missed {kind} call \u{2014} I tried to reach you.");
    if let Err(e) = vault.send_dm(&peer, &note).await {
        warn!("failed to leave missed-call DM: {e}");
    }
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
    async fn call_commands_reject_gracefully_when_locked() {
        let rt = ComradeRuntime::new();
        assert!(matches!(
            rt.place_call("npub1whatever", false, "sdp").await,
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.answer_call("c1", "sdp").await,
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(rt.end_call("c1").await, Err(UiError::VaultLocked)));
        assert!(matches!(rt.call_log(), Err(UiError::VaultLocked)));
        assert!(matches!(rt.active_call(), Err(UiError::VaultLocked)));
    }

    #[tokio::test]
    async fn unlocked_runtime_rejects_bad_peer_key_and_has_empty_call_log() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        // Garbage peer key is a typed error, not a panic.
        let err = rt.place_call("not-a-key", true, "sdp").await;
        assert!(matches!(err, Err(UiError::Engine(_))));

        // No calls yet.
        assert!(rt.call_log().unwrap().is_empty());
        assert!(rt.active_call().unwrap().is_none());
    }

    #[test]
    fn call_bridge_event_serialises_with_nested_type_tags() {
        let event = BridgeEvent::Call {
            call: CallEvent::CallConnected {
                call_id: "abc".into(),
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"call\""));
        assert!(json.contains("\"type\":\"call_connected\""));
        let back: BridgeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
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
}
