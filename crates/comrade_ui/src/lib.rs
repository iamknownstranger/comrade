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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use comrade_core::companion::{
    prompt_for, scan_safety, CompanionMode, EntrySource, Insights, JournalEntry, SafetyAssessment,
};
use comrade_core::crypto::KeyProfile;
use comrade_core::vault::{build_pay_regex, extract_upi_intents};
use comrade_state::{AppWorkspace, RuntimeContext};
use comrade_storage::{EncryptedStore, StoredIdentity};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// sled tree holding the companion journal (anonymous, encrypted at rest).
const JOURNAL_TREE: &str = "companion_journal";

pub mod runtime;

pub use runtime::{BridgeEvent, ChitthiDto, ComradeRuntime, DirectMessageDto};

// ── Errors ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
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

    #[error("unknown companion mode: {0}")]
    UnknownMode(String),
}

// ── DTOs (serializable across any IPC/FFI boundary) ──────────────────────────────

/// A workspace entry for the UI, including whether it is currently active.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdentityDto {
    pub npub: String,
    pub has_secret: bool,
}

/// A detected UPI payment intent for display/confirmation in the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpiIntentDto {
    pub amount_inr: f64,
    pub vpa: String,
    pub uri: String,
}

/// The outcome of writing a companion entry: the stored (anonymous) entry, an
/// offline safety assessment of what was written, and a fresh supportive prompt
/// for the next step. Everything here stays on-device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanionResponse {
    pub entry: JournalEntry,
    pub safety: SafetyAssessment,
    pub prompt: String,
}

// ── Service ─────────────────────────────────────────────────────────────────────

/// The single stateful entry point a frontend drives. Wrap it in a `Mutex` when
/// sharing across async command handlers (e.g. Tauri's managed state).
pub struct UiService {
    ctx: RuntimeContext,
    identity: Option<KeyProfile>,
    store: Option<EncryptedStore>,
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
    pub fn unlock_store(&mut self, path: impl AsRef<Path>, pin: &str) -> Result<(), UiError> {
        let store = EncryptedStore::open(path, pin).map_err(|e| UiError::Storage(e.to_string()))?;
        self.store = Some(store);
        Ok(())
    }

    pub fn is_store_unlocked(&self) -> bool {
        self.store.is_some()
    }

    /// Crate-internal: clone the live keypair for engine construction. Kept
    /// `pub(crate)` so the secret never leaks into the public UI surface — only
    /// the [`runtime`] bridge, which builds the Nostr engines, can reach it.
    pub(crate) fn identity_keys(&self) -> Option<nostr_sdk::Keys> {
        self.identity.as_ref().map(|p| p.keys.clone())
    }

    /// Crate-internal borrow of the unlocked encrypted store, for cache reads
    /// (e.g. the offline Sabha timeline) performed by the [`runtime`] bridge.
    pub(crate) fn store_ref(&self) -> Option<&EncryptedStore> {
        self.store.as_ref()
    }

    /// Persist the current identity to the unlocked store.
    pub fn save_identity(&self) -> Result<(), UiError> {
        let store = self.store.as_ref().ok_or(UiError::StoreLocked)?;
        let profile = self.identity.as_ref().ok_or(UiError::NoIdentity)?;
        let identity = StoredIdentity::new(
            profile.npub.clone(),
            profile.nsec.clone(),
            Some("primary".into()),
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
                Ok(Some(dto))
            }
        }
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

    // Companion — private, anonymous journal ---------------------------------

    /// A supportive prompt for `mode` ("journal" / "vent" / "brainstorm" /
    /// "reflect"), seeded by how many entries already exist so it rotates
    /// naturally. Works whether or not the store is unlocked.
    pub fn companion_prompt(&self, mode: &str) -> Result<String, UiError> {
        let m =
            CompanionMode::from_key(mode).ok_or_else(|| UiError::UnknownMode(mode.to_string()))?;
        Ok(prompt_for(m, self.journal_count()).to_string())
    }

    /// Offline crisis-signal scan of arbitrary text. No persistence, no network
    /// — the words never leave the device. See [`comrade_core::companion`].
    pub fn scan_companion_text(&self, text: &str) -> SafetyAssessment {
        scan_safety(text)
    }

    /// Write an anonymous journal entry into the encrypted store, returning it
    /// with a safety assessment and the next supportive prompt.
    pub fn write_journal_entry(
        &self,
        mode: &str,
        voice: bool,
        body: &str,
        mood: Option<i8>,
    ) -> Result<CompanionResponse, UiError> {
        let m =
            CompanionMode::from_key(mode).ok_or_else(|| UiError::UnknownMode(mode.to_string()))?;
        let store = self.store_ref().ok_or(UiError::StoreLocked)?;

        let source = if voice {
            EntrySource::Voice
        } else {
            EntrySource::Typed
        };
        let mut entry = JournalEntry::new(next_id(), now_secs(), m, source, body);
        if let Some(v) = mood {
            entry = entry.with_mood(v);
        }

        store
            .put(JOURNAL_TREE, &entry.id, &entry)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;

        let safety = scan_safety(body);
        let prompt = prompt_for(m, self.journal_count()).to_string();
        Ok(CompanionResponse {
            entry,
            safety,
            prompt,
        })
    }

    /// All journal entries, newest first. Requires an unlocked store.
    pub fn list_journal_entries(&self) -> Result<Vec<JournalEntry>, UiError> {
        let store = self.store_ref().ok_or(UiError::StoreLocked)?;
        let mut entries: Vec<JournalEntry> = store
            .values(JOURNAL_TREE)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        // Newest first. `created_at` is second-granular, so entries written in
        // the same second are tie-broken by the monotonic id (`<nanos>-<seq>`).
        entries.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(entries)
    }

    /// Delete an entry by id. Returns whether one was removed.
    pub fn delete_journal_entry(&self, id: &str) -> Result<bool, UiError> {
        let store = self.store_ref().ok_or(UiError::StoreLocked)?;
        let removed = store
            .delete(JOURNAL_TREE, id)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        if removed {
            store.flush().map_err(|e| UiError::Storage(e.to_string()))?;
        }
        Ok(removed)
    }

    /// On-device insights (streak, momentum, mood trend, top tags).
    pub fn journal_insights(&self) -> Result<Insights, UiError> {
        let entries = self.list_journal_entries()?;
        Ok(Insights::from_entries(&entries, now_secs()))
    }

    /// Number of stored entries, or 0 if the store is locked (used as a prompt
    /// rotation seed).
    fn journal_count(&self) -> u64 {
        self.store_ref()
            .and_then(|s| s.keys(JOURNAL_TREE).ok())
            .map(|k| k.len() as u64)
            .unwrap_or(0)
    }
}

// ── Local time / id helpers ───────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// A locally-unique, identity-free entry id: `<unix_nanos>-<seq>`. Carries no
/// link to the user's Nostr key — the journal is deliberately anonymous.
fn next_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{nanos}-{seq}")
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
    fn journal_requires_unlocked_store() {
        let svc = UiService::new();
        assert!(matches!(
            svc.write_journal_entry("journal", false, "hi", None),
            Err(UiError::StoreLocked)
        ));
        assert!(matches!(
            svc.list_journal_entries(),
            Err(UiError::StoreLocked)
        ));
    }

    #[test]
    fn journal_unknown_mode_is_typed_error() {
        let dir = TempDir::new().unwrap();
        let mut svc = UiService::new();
        svc.unlock_store(dir.path(), "pin").unwrap();
        assert!(matches!(
            svc.write_journal_entry("astrology", false, "x", None),
            Err(UiError::UnknownMode(_))
        ));
    }

    #[test]
    fn journal_write_persists_and_lists_newest_first() {
        let dir = TempDir::new().unwrap();
        let mut svc = UiService::new();
        svc.unlock_store(dir.path(), "pin").unwrap();

        svc.write_journal_entry("journal", false, "first entry #calm", Some(1))
            .unwrap();
        svc.write_journal_entry("vent", true, "second, dictated", None)
            .unwrap();

        let entries = svc.list_journal_entries().unwrap();
        assert_eq!(entries.len(), 2);
        // Newest first; the voice vent was written last.
        assert_eq!(entries[0].body, "second, dictated");
        assert_eq!(entries[0].source, EntrySource::Voice);
        assert_eq!(entries[1].tags, vec!["calm".to_string()]);
        assert_eq!(entries[1].mood, Some(1));
    }

    #[test]
    fn journal_survives_reopen_and_stays_encrypted() {
        let dir = TempDir::new().unwrap();
        let secret_words = "the-very-private-thing-i-wrote";
        {
            let mut svc = UiService::new();
            svc.unlock_store(dir.path(), "pin").unwrap();
            svc.write_journal_entry("reflect", false, secret_words, None)
                .unwrap();
        }
        // Plaintext must never hit disk.
        let mut leaked = false;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                if let Ok(bytes) = std::fs::read(&path) {
                    if bytes
                        .windows(secret_words.len())
                        .any(|w| w == secret_words.as_bytes())
                    {
                        leaked = true;
                    }
                }
            }
        }
        assert!(!leaked, "journal body must be ciphertext at rest");

        // And it round-trips after reopening with the right PIN.
        let mut svc2 = UiService::new();
        svc2.unlock_store(dir.path(), "pin").unwrap();
        let entries = svc2.list_journal_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].body, secret_words);
    }

    #[test]
    fn journal_delete_and_insights() {
        let dir = TempDir::new().unwrap();
        let mut svc = UiService::new();
        svc.unlock_store(dir.path(), "pin").unwrap();

        let r = svc
            .write_journal_entry("journal", false, "keep #focus", None)
            .unwrap();
        svc.write_journal_entry("journal", false, "drop this", None)
            .unwrap();

        let insights = svc.journal_insights().unwrap();
        assert_eq!(insights.total, 2);
        assert!(insights.current_streak_days >= 1);

        // Delete one and confirm the count drops.
        assert!(svc.delete_journal_entry(&r.entry.id).unwrap());
        assert!(!svc.delete_journal_entry(&r.entry.id).unwrap());
        assert_eq!(svc.list_journal_entries().unwrap().len(), 1);
    }

    #[test]
    fn companion_prompt_and_safety_do_not_need_store() {
        let svc = UiService::new();
        let prompt = svc.companion_prompt("reflect").unwrap();
        assert!(!prompt.is_empty());
        assert!(matches!(
            svc.companion_prompt("bogus"),
            Err(UiError::UnknownMode(_))
        ));

        // Safety scan works with no store and flags crisis language.
        assert!(!svc.scan_companion_text("nice quiet evening").concerning);
        assert!(svc.scan_companion_text("i want to die").concerning);
    }

    #[test]
    fn write_returns_safety_assessment_for_concerning_entry() {
        let dir = TempDir::new().unwrap();
        let mut svc = UiService::new();
        svc.unlock_store(dir.path(), "pin").unwrap();
        let r = svc
            .write_journal_entry("vent", false, "i can't go on like this", None)
            .unwrap();
        assert!(r.safety.concerning);
        assert!(!r.safety.resources.is_empty());
        // The entry is still saved — we never block the person from writing.
        assert_eq!(svc.list_journal_entries().unwrap().len(), 1);
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
