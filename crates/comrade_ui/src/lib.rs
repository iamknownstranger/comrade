/*!
 * comrade_ui — Framework-agnostic view-model / service layer
 *
 * Track 2: Cross-Platform UI Shell.
 *
 * This crate holds the *logic* a UI needs — workspace switching, identity
 * management, encrypted persistence, payment parsing — exposed as plain methods
 * returning serializable DTOs. It contains no rendering code, so the same
 * [`UiService`] backs every frontend: the Tauri desktop shell (via
 * `#[tauri::command]` wrappers), a native Slint/Iced app, or Android over JNI.
 *
 * Keeping this layer pure means the entire UI contract is unit-testable without
 * a display server, which is exactly what a headless CI can verify.
 */

use std::path::Path;
use std::sync::Arc;

use comrade_core::crypto::KeyProfile;
use comrade_core::vault::{build_pay_regex, extract_upi_intents};
use comrade_state::{AppWorkspace, RuntimeContext};
use comrade_storage::{EncryptedStore, StoredIdentity};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// This crate's own `UniFfiTag` — lets `UiError` and every DTO in `runtime`
// derive `uniffi::Record`/`uniffi::Enum`/`uniffi::Error` directly, so
// `comrade_jni`'s FFI object can use them with no hand-written mirror types.
// See the note on `comrade_core::setup_scaffolding!` for why each crate that
// contributes FFI types needs its own call.
uniffi::setup_scaffolding!("comrade_ui");

pub mod runtime;

// The together wire types a bridge has to name: the desktop shell takes
// `TogetherContent` as JSON and deserialises it, and both bridges hand the
// verdict and the state change straight to a player. Re-exported here so a
// bridge depends on `comrade_ui` alone, exactly as it does for every DTO.
pub use comrade_core::share::transport::{RefusalReason, RelayPolicy};
pub use comrade_core::share::{
    ReadSample, ReadVerdict, ShareOffer, ShareSignal, TransferSignal, SHARE_PLAYABLE_RUNWAY_MS,
    SHARE_STALL_FLOOR_MS,
};
// The starve policy as a free function, not only as a `ComradeRuntime` method.
// A bridge answering from inside a `MediaDataSource.readAt` has a decoder thread
// and no runtime handle worth taking a lock on — and there is nothing in the
// answer that needs one, since every input comes from the caller's own bitmap.
pub use comrade_core::share::read_verdict as share_read_verdict;
pub use comrade_core::together::{
    MusicLink, PlayheadControl, Recording, ServiceAccess, StateChange, SyncVerdict, TogetherContent,
};
// The handoff wire types and the road decision, for the same reason: the desktop
// shell takes a `HandoffSignal` as JSON and asks which road a size takes, and it
// must get that answer from the core rather than keeping its own copy of the
// threshold — a frontend with its own 10 MB constant is a frontend that
// disagrees with the core the day the cap moves.
pub use comrade_core::handoff::{
    route_for_bytes, AttachmentHandoff, AttachmentRoute, HandoffSignal,
};

// The in-chat command grammar a bridge has to name: a composer calls
// `parse_chat_command` on every keystroke and switches on the result, and the
// `/`-autocomplete reads `chat_command_catalog`. Re-exported here for the same
// reason the together wire types are — so a bridge depends on `comrade_ui` alone.
pub use comrade_core::command::{AppAction, ChatCommand, CommandSpec, Mention, MusicService};
pub use comrade_core::karya::TaskState;
pub use runtime::play_route;
// The catalogue rung: one network call and one pure decision, both free
// functions so no caller can hold a lock across the lookup (see their docs).
pub use runtime::{audio_plan, catalogue_lookup, download_track, download_verdict};
// The streaming-source search: one network call, a free function for the same
// reason the catalogue one is — no caller can hold the runtime lock across it.
pub use runtime::{subsonic_search, StreamCandidateDto, StreamSearchOutcome};
// The public sources: Internet Archive, podcast feeds, synced lyrics.
pub use runtime::{
    archive_search, archive_tracks, lyrics_lookup, podcast_episodes, ArchiveItemDto,
    CollectionSearchOutcome, LyricsOutcome, TrackListOutcome,
};
// The player's own library — favourites, history, playlists, saved queue.
pub use runtime::{
    prune_history, HistoryEntryDto, PlayerTrackDto, PlayerTrackKind, PlaylistDto, SavedQueueDto,
    HISTORY_MAX_ENTRIES,
};
// The travel guide: one network call, deliberately a free function so no caller
// can hold the runtime lock across three round trips (see its doc).
pub use runtime::{travel_guide, TravelCache, TRAVEL_NO_KEY_NOTICE};

pub use runtime::{
    AttachmentHandoffDto, AttentionDayDto, AttentionSummaryDto, BleRouter, BridgeEvent,
    CallRecordDto, CallSessionDto, CallSignalDto, ChitthiDto, ComradeDto, ComradeRuntime,
    ContactDto, ConversationDto, CrisisResourceDto, DirectMessageDto, DownloadVerdictDto,
    DownloadedTrackDto, FocusSessionDto, FoundProfileDto, IceServerDto, JournalEntryDto,
    JournalRecordingDto, LibraryCandidateDto, MediaBytesDto, MediaMessageDto, MentionMatchDto,
    MeshStatusDto, MessageAuthor, MessageDto, MessageRequestDto, MetricDto, OfferOutcomeDto,
    PeerProfileDto, PlayPlan, PlayRoute, PlayTargetDto, PresenceDto, ProfileDto, ReactionDto,
    RideSignalDto, RuntimeHandles, SakhaStatusDto, SavedReadDto, SavedReadSummaryDto,
    ShareVerdictDto, SharedNoteDto, StretchStepDto, TaraChatDto, TaraMessageDto, TaskDto,
    ThreadDto, ThreadSummaryDto, TogetherCommandDto, TogetherCorrectionDto, TogetherInviteDto,
    TogetherSessionDto, TogetherShareDto, TopicDto, TravelFactDto, TravelGuideDto, TravelPlaceDto,
    TurnServerStatusDto,
};

// ── Errors ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Error, uniffi::Error)]
pub enum UiError {
    #[error("unknown workspace key: {0}")]
    UnknownWorkspace(String),

    #[error("workspace transition failed: {0}")]
    Transition(String),

    #[error("no identity loaded — generate or load one first")]
    NoIdentity,

    #[error("store is locked — call unlock first")]
    StoreLocked,

    #[error("crypto error: {0}")]
    Crypto(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("vault is locked — call unlock_vault first")]
    VaultLocked,

    #[error("engine error: {0}")]
    Engine(String),

    #[error("catalogue lookup failed: {0}")]
    Catalogue(String),

    /// A download was refused or failed. Stringly-typed like [`Self::Catalogue`]
    /// and [`Self::Engine`] because the interesting distinctions — *why* a
    /// licence refused it — are answered before a download starts, by
    /// `download_verdict`, which is typed. What reaches here is a transfer that
    /// went wrong, and those are network sentences.
    #[error("download failed: {0}")]
    Download(String),

    /// The build has no catalogue at all, which is a **different answer from
    /// "no results"** and must stay one.
    ///
    /// `catalogue-http` is off in the lean workspace test build, and a lookup in
    /// that build cannot reach a socket. Returning an empty list would render as
    /// "we searched and that recording does not exist" — a wrong answer, silently
    /// produced, and exactly the "fail fast on missing config rather than quietly
    /// defaulting" case. A frontend seeing this should say the build cannot
    /// search, not that the search found nothing.
    #[error("this build has no catalogue support — rebuild with the `catalogue-http` feature")]
    CatalogueUnavailable,

    /// A travel lookup failed outright — every provider refused, and there was
    /// no cached guide for this cell to fall back to. Partial failures are
    /// *not* this: they arrive as [`runtime::TravelGuideDto::notice`], because a
    /// guide missing only its ratings is still worth reading.
    #[error("travel lookup failed: {0}")]
    Travel(String),

    /// Same argument as [`Self::CatalogueUnavailable`], one feature over: this
    /// build was compiled without `travel-http`, so it cannot reach a places
    /// provider at all. An empty guide would read as "there is nothing near
    /// you", which is a wrong answer produced silently.
    #[error("this build has no travel support — rebuild with the `travel-http` feature")]
    TravelUnavailable,
}

// ── DTOs (serializable across any IPC/FFI boundary) ──────────────────────────────

/// A workspace entry for the UI, including whether it is currently active.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, uniffi::Record)]
pub struct WorkspaceDto {
    pub key: String,
    pub label: String,
    pub active: bool,
    pub relay_connected: bool,
    pub mesh_active: bool,
    pub couple_sandbox: bool,
}

impl WorkspaceDto {
    fn from(ws: &AppWorkspace, active: bool) -> Self {
        Self {
            key: ws.key().to_string(),
            label: ws.label().to_string(),
            active,
            relay_connected: ws.is_relay_connected(),
            mesh_active: ws.is_mesh_active(),
            couple_sandbox: ws.is_couple_sandbox(),
        }
    }
}

/// The local identity as the UI sees it. Never exposes the secret key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, uniffi::Record)]
pub struct IdentityDto {
    pub npub: String,
    pub has_secret: bool,
}

/// A detected UPI payment intent for display/confirmation in the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, uniffi::Record)]
pub struct UpiIntentDto {
    pub amount_inr: f64,
    pub vpa: String,
    pub uri: String,
}

// ── Service ─────────────────────────────────────────────────────────────────────

/// The single stateful entry point a frontend drives. Wrap it in a `Mutex` when
/// sharing across async command handlers (e.g. Tauri's managed state).
pub struct UiService {
    ctx: RuntimeContext,
    identity: Option<KeyProfile>,
    /// The user's chosen @handle. A display alias only — never an identifier;
    /// identity is always the keypair (see the runtime's username docs).
    username: Option<String>,
    /// Behind an `Arc` so background Tokio loops (e.g. the media-aware DM
    /// listener in [`runtime`]) can hold their own handle to the open store.
    store: Option<Arc<EncryptedStore>>,
}

impl Default for UiService {
    fn default() -> Self {
        Self::new()
    }
}

impl UiService {
    pub fn new() -> Self {
        Self {
            ctx: RuntimeContext::new(),
            identity: None,
            username: None,
            store: None,
        }
    }

    // Workspaces -------------------------------------------------------------

    /// All workspaces with the current one flagged `active`.
    pub fn workspaces(&self) -> Vec<WorkspaceDto> {
        let current = self.ctx.current();
        AppWorkspace::all()
            .iter()
            .map(|ws| WorkspaceDto::from(ws, ws == current))
            .collect()
    }

    /// The currently active workspace.
    pub fn current_workspace(&self) -> WorkspaceDto {
        let ws = self.ctx.current();
        WorkspaceDto::from(ws, true)
    }

    /// Switch to the workspace identified by its stable key.
    pub fn switch_workspace(&mut self, key: &str) -> Result<WorkspaceDto, UiError> {
        let target = AppWorkspace::from_key(key)
            .ok_or_else(|| UiError::UnknownWorkspace(key.to_string()))?;
        self.ctx
            .transition(target)
            .map_err(|e| UiError::Transition(e.to_string()))?;
        Ok(self.current_workspace())
    }

    /// Step back to the previous workspace, if any.
    pub fn back(&mut self) -> WorkspaceDto {
        self.ctx.step_back();
        self.current_workspace()
    }

    // Identity ---------------------------------------------------------------

    /// Generate a fresh identity, replacing any in memory.
    pub fn generate_identity(&mut self) -> Result<IdentityDto, UiError> {
        let profile = KeyProfile::generate().map_err(|e| UiError::Crypto(e.to_string()))?;
        let dto = IdentityDto {
            npub: profile.npub.clone(),
            has_secret: true,
        };
        self.identity = Some(profile);
        Ok(dto)
    }

    /// The current identity, if one is loaded.
    pub fn current_identity(&self) -> Option<IdentityDto> {
        self.identity.as_ref().map(|p| IdentityDto {
            npub: p.npub.clone(),
            has_secret: true,
        })
    }

    // Encrypted store --------------------------------------------------------

    /// Open the encrypted store at `path` with `pin`.
    ///
    /// Argon2id key stretching makes this deliberately expensive — async
    /// callers should open the store on a blocking thread and hand it over via
    /// [`Self::attach_store`] instead (see `ComradeRuntime::unlock_vault`).
    pub fn unlock_store(&mut self, path: impl AsRef<Path>, pin: &str) -> Result<(), UiError> {
        let store = EncryptedStore::open(path, pin).map_err(|e| UiError::Storage(e.to_string()))?;
        self.attach_store(store);
        Ok(())
    }

    /// Attach an already-opened encrypted store (e.g. one unlocked on a
    /// blocking thread by the async bridge). Replaces any store held so far.
    pub fn attach_store(&mut self, store: EncryptedStore) {
        self.store = Some(Arc::new(store));
    }

    pub fn is_store_unlocked(&self) -> bool {
        self.store.is_some()
    }

    /// Drop the open store and every in-memory identity/handle — the
    /// view-model half of [`crate::runtime::ComradeRuntime::lock_vault`]. Safe
    /// to call when already locked (each field is simply already `None`).
    /// Background tasks holding their own `Arc<EncryptedStore>` clone are
    /// unaffected by this alone; the runtime aborts those separately.
    pub fn lock(&mut self) {
        self.store = None;
        self.identity = None;
        self.username = None;
    }

    /// Crate-internal: clone the live keypair for engine construction. Kept
    /// `pub(crate)` so the secret never leaks into the public UI surface — only
    /// the [`runtime`] bridge, which builds the Nostr engines, can reach it.
    pub(crate) fn identity_keys(&self) -> Option<nostr_sdk::prelude::Keys> {
        self.identity.as_ref().map(|p| p.keys.clone())
    }

    /// Crate-internal borrow of the unlocked encrypted store, for cache reads
    /// (e.g. the offline Sabha timeline) performed by the [`runtime`] bridge.
    pub(crate) fn store_ref(&self) -> Option<&EncryptedStore> {
        self.store.as_deref()
    }

    /// Crate-internal: a cloned `Arc` handle to the open store, so background
    /// Tokio tasks (the media-aware DM loop) can persist independently.
    pub(crate) fn store_arc(&self) -> Option<Arc<EncryptedStore>> {
        self.store.clone()
    }

    /// Persist the current identity to the unlocked store. The identity label
    /// carries the chosen @handle ("primary" is the legacy no-username marker).
    pub fn save_identity(&self) -> Result<(), UiError> {
        let store = self.store.as_ref().ok_or(UiError::StoreLocked)?;
        let profile = self.identity.as_ref().ok_or(UiError::NoIdentity)?;
        let identity = StoredIdentity::new(
            profile.npub.clone(),
            profile.nsec.clone(),
            Some(self.username.clone().unwrap_or_else(|| "primary".into())),
        );
        store
            .save_identity(&identity)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Load the saved identity from the unlocked store into memory.
    pub fn load_identity(&mut self) -> Result<Option<IdentityDto>, UiError> {
        let store = self.store.as_ref().ok_or(UiError::StoreLocked)?;
        let stored = store
            .load_identity()
            .map_err(|e| UiError::Storage(e.to_string()))?;
        match stored {
            None => Ok(None),
            Some(id) => {
                let profile =
                    KeyProfile::from_nsec(&id.nsec).map_err(|e| UiError::Crypto(e.to_string()))?;
                let dto = IdentityDto {
                    npub: profile.npub.clone(),
                    has_secret: true,
                };
                self.identity = Some(profile);
                // "primary" was the fixed label before usernames existed.
                self.username = id.label.filter(|l| l != "primary");
                Ok(Some(dto))
            }
        }
    }

    /// The chosen @handle, if one was set.
    pub fn username(&self) -> Option<String> {
        self.username.clone()
    }

    /// Set the @handle and persist it with the identity. Validation (charset,
    /// length) happens in the runtime so every bridge shares the same rules.
    pub fn set_username(&mut self, handle: String) -> Result<(), UiError> {
        if self.identity.is_none() {
            return Err(UiError::NoIdentity);
        }
        self.username = Some(handle);
        self.save_identity()
    }

    // Payments ---------------------------------------------------------------

    /// Extract UPI `/pay` intents from a message for UI confirmation.
    pub fn extract_payments(&self, text: &str) -> Result<Vec<UpiIntentDto>, UiError> {
        let re = build_pay_regex().map_err(|e| UiError::Crypto(e.to_string()))?;
        Ok(extract_upi_intents(text, &re)
            .into_iter()
            .map(|i| UpiIntentDto {
                amount_inr: i.amount_inr,
                vpa: i.vpa,
                uri: i.uri,
            })
            .collect())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn lists_all_workspaces_with_base_active() {
        let svc = UiService::new();
        let all = svc.workspaces();
        assert_eq!(all.len(), 4);
        let active: Vec<_> = all.iter().filter(|w| w.active).collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].key, "Base");
    }

    #[test]
    fn switch_and_back_workspace() {
        let mut svc = UiService::new();
        let dto = svc.switch_workspace("OffGridTravel").unwrap();
        assert_eq!(dto.key, "OffGridTravel");
        assert!(dto.mesh_active);
        assert!(!dto.relay_connected);

        let back = svc.back();
        assert_eq!(back.key, "Base");
    }

    #[test]
    fn unknown_workspace_key_errors() {
        let mut svc = UiService::new();
        assert!(matches!(
            svc.switch_workspace("Nope"),
            Err(UiError::UnknownWorkspace(_))
        ));
    }

    #[test]
    fn illegal_transition_is_rejected() {
        // OffGridTravel -> CoupleSandbox is blocked by the state machine.
        let mut svc = UiService::new();
        svc.switch_workspace("OffGridTravel").unwrap();
        assert!(matches!(
            svc.switch_workspace("CoupleSandboxSakha"),
            Err(UiError::Transition(_))
        ));
    }

    #[test]
    fn generate_and_read_identity() {
        let mut svc = UiService::new();
        assert!(svc.current_identity().is_none());
        let dto = svc.generate_identity().unwrap();
        assert!(dto.npub.starts_with("npub1"));
        assert!(dto.has_secret);
        assert_eq!(svc.current_identity().unwrap().npub, dto.npub);
    }

    #[test]
    fn save_requires_unlocked_store() {
        let mut svc = UiService::new();
        svc.generate_identity().unwrap();
        assert!(matches!(svc.save_identity(), Err(UiError::StoreLocked)));
    }

    #[test]
    fn save_requires_identity() {
        let dir = TempDir::new().unwrap();
        let mut svc = UiService::new();
        svc.unlock_store(dir.path(), "pin").unwrap();
        assert!(matches!(svc.save_identity(), Err(UiError::NoIdentity)));
    }

    #[test]
    fn identity_persists_through_store() {
        let dir = TempDir::new().unwrap();
        let npub = {
            let mut svc = UiService::new();
            svc.unlock_store(dir.path(), "pin").unwrap();
            let dto = svc.generate_identity().unwrap();
            svc.save_identity().unwrap();
            dto.npub
        };
        // Fresh service, same store + PIN, load back.
        let mut svc2 = UiService::new();
        svc2.unlock_store(dir.path(), "pin").unwrap();
        let loaded = svc2.load_identity().unwrap().unwrap();
        assert_eq!(loaded.npub, npub);
        assert_eq!(svc2.current_identity().unwrap().npub, npub);
    }

    #[test]
    fn extract_payments_parses_intent() {
        let svc = UiService::new();
        let intents = svc.extract_payments("/pay 250 to friend@upi").unwrap();
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].amount_inr, 250.0);
        assert_eq!(intents[0].vpa, "friend@upi");
        assert!(intents[0].uri.starts_with("upi://pay"));
    }

    #[test]
    fn wrong_pin_maps_to_ui_storage_error() {
        // A storage-layer failure must surface as a UiError, not a panic, so the
        // frontend thread keeps running. (Unified error architecture, M5.)
        let dir = TempDir::new().unwrap();
        {
            let mut svc = UiService::new();
            svc.unlock_store(dir.path(), "correct").unwrap();
        }
        let mut svc = UiService::new();
        let err = svc.unlock_store(dir.path(), "incorrect");
        assert!(matches!(err, Err(UiError::Storage(_))));
        assert!(!svc.is_store_unlocked());
    }
}
