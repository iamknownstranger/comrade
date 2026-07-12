/*!
 * Async `#[tauri::command]` handlers — the desktop half of the Command & Event
 * Bridge.
 *
 * Every command is a thin marshalling shim over [`comrade_ui::ComradeRuntime`]
 * (the workspace-tested orchestrator). The shared state is
 * `tauri::State<Arc<RwLock<ComradeRuntime>>>`, accessed with a Tokio `RwLock`
 * so concurrent invocations are serialised safely.
 *
 * Error policy (Architecture Quality Gate): every handler returns
 * `Result<_, String>`. A `UiError` is stringified and surfaced to JavaScript as
 * a rejected `Promise` — there are no `.unwrap()`s and no panics on this path.
 */

use std::sync::Arc;

use comrade_ui::{
    ChitthiDto, ComradeRuntime, ContactDto, ConversationDto, FoundProfileDto, IdentityDto,
    JournalEntryDto, MediaBytesDto, MediaMessageDto, MessageDto, ProfileDto, UpiIntentDto,
    WorkspaceDto,
};
use tokio::sync::RwLock;

/// The live IPC runtime context, as referenced by the bridge spec.
pub type Runtime = Arc<RwLock<ComradeRuntime>>;

/// Hard cap mirrored on the frontend; defends the backend against oversized reads.
const MAX_MEDIA_BYTES: usize = 10 * 1024 * 1024;

/// Best-effort MIME guess from a file extension.
fn guess_mime(path: &str) -> String {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" | "aac" => "audio/aac",
        "mp4" => "video/mp4",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
    .to_string()
}

// ── Milestone 1: vault, timeline, broadcast ──────────────────────────────────

/// Unlock the encrypted storage repository and fire up the core relay loop.
#[tauri::command]
pub async fn unlock_comrade_vault(
    state: tauri::State<'_, Runtime>,
    path: String,
    passphrase: String,
) -> Result<IdentityDto, String> {
    let mut rt = state.write().await;
    let identity = rt
        .unlock_vault(&path, &passphrase)
        .await
        .map_err(|e| e.to_string())?;
    // Connect + start the Tokio feed/DM loops; events flow to the webview via
    // the forwarder spawned in `run()`'s setup hook.
    rt.spawn_event_loops();
    Ok(identity)
}

/// Load the Sabha timeline from the encrypted offline cache.
#[tauri::command]
pub async fn fetch_sabha_timeline(
    state: tauri::State<'_, Runtime>,
) -> Result<Vec<ChitthiDto>, String> {
    state
        .read()
        .await
        .fetch_sabha_timeline()
        .map_err(|e| e.to_string())
}

/// Broadcast a Chitthi, optionally as a NIP-10 reply. Returns the event id.
#[tauri::command]
pub async fn broadcast_chitthi(
    state: tauri::State<'_, Runtime>,
    content: String,
    reply_to: Option<String>,
) -> Result<String, String> {
    state
        .read()
        .await
        .broadcast_chitthi(&content, reply_to)
        .await
        .map_err(|e| e.to_string())
}

/// Sync the Sakha/Sakhi shared CRDT ledger to the partner. Returns the event id.
#[tauri::command]
pub async fn sync_ledger(state: tauri::State<'_, Runtime>) -> Result<String, String> {
    state.read().await.sync_ledger().await.map_err(|e| e.to_string())
}

// ── Direct messages, profile & contacts (Telegram-like flow) ──────────────────

/// Send an E2E-encrypted DM to `target` (npub or hex pubkey). The message is
/// persisted to the offline history; returns the stored message DTO.
#[tauri::command]
pub async fn send_dm(
    state: tauri::State<'_, Runtime>,
    target: String,
    content: String,
) -> Result<MessageDto, String> {
    state
        .read()
        .await
        .send_dm(&target, &content)
        .await
        .map_err(|e| e.to_string())
}

/// The chat list (one entry per peer, newest first) from the offline history.
#[tauri::command]
pub async fn conversations(
    state: tauri::State<'_, Runtime>,
) -> Result<Vec<ConversationDto>, String> {
    state.read().await.conversations().map_err(|e| e.to_string())
}

/// Full offline message history with `peer`, oldest first.
#[tauri::command]
pub async fn messages_with(
    state: tauri::State<'_, Runtime>,
    peer: String,
) -> Result<Vec<MessageDto>, String> {
    state.read().await.messages_with(&peer).map_err(|e| e.to_string())
}

/// The local profile: npub plus the chosen @handle, if any.
#[tauri::command]
pub async fn current_profile(state: tauri::State<'_, Runtime>) -> Result<ProfileDto, String> {
    state.read().await.profile().map_err(|e| e.to_string())
}

/// Claim a display @handle (persisted locally, published best-effort).
#[tauri::command]
pub async fn set_username(
    state: tauri::State<'_, Runtime>,
    name: String,
) -> Result<ProfileDto, String> {
    state
        .write()
        .await
        .set_username(&name)
        .await
        .map_err(|e| e.to_string())
}

/// Save a contact pinned by npub (trust-on-first-use). An empty alias keeps
/// any alias already set.
#[tauri::command]
pub async fn add_contact(
    state: tauri::State<'_, Runtime>,
    npub: String,
    alias: String,
) -> Result<ContactDto, String> {
    state.read().await.add_contact(&npub, &alias).map_err(|e| e.to_string())
}

/// Set (non-empty) or clear (empty) the user-chosen alias for a contact.
#[tauri::command]
pub async fn set_contact_alias(
    state: tauri::State<'_, Runtime>,
    npub: String,
    alias: String,
) -> Result<ContactDto, String> {
    state
        .read()
        .await
        .set_contact_alias(&npub, &alias)
        .map_err(|e| e.to_string())
}

/// Remove a saved contact (message history stays). Returns whether one existed.
#[tauri::command]
pub async fn remove_contact(
    state: tauri::State<'_, Runtime>,
    npub: String,
) -> Result<bool, String> {
    state.read().await.remove_contact(&npub).map_err(|e| e.to_string())
}

/// Refresh cached peer profiles (bounded, TTL-gated). Returns how many
/// display names changed; reload the chat list when > 0.
#[tauri::command]
pub async fn refresh_peer_profiles(state: tauri::State<'_, Runtime>) -> Result<usize, String> {
    // Detach the refresher under a briefly-held guard, then run guard-free:
    // holding the shared lock across relay round-trips would block every
    // other command (AUDIT P2: no guard held across network awaits).
    let refresher = { state.read().await.profile_refresher().map_err(|e| e.to_string())? };
    refresher.run().await.map_err(|e| e.to_string())
}

/// All saved contacts, alias-sorted.
#[tauri::command]
pub async fn list_contacts(state: tauri::State<'_, Runtime>) -> Result<Vec<ContactDto>, String> {
    state.read().await.list_contacts().map_err(|e| e.to_string())
}

/// Best-effort people search by handle over NIP-50-capable relays.
#[tauri::command]
pub async fn search_profiles(
    state: tauri::State<'_, Runtime>,
    query: String,
) -> Result<Vec<FoundProfileDto>, String> {
    state
        .read()
        .await
        .search_profiles(&query)
        .await
        .map_err(|e| e.to_string())
}

// ── Journal (strictly local, never networked) ──────────────────────────────────

/// Save a journal entry. The entry never leaves the device.
#[tauri::command]
pub async fn add_journal_entry(
    state: tauri::State<'_, Runtime>,
    text: String,
    mood: Option<String>,
) -> Result<JournalEntryDto, String> {
    state
        .read()
        .await
        .add_journal_entry(&text, mood.as_deref())
        .map_err(|e| e.to_string())
}

/// All journal entries, newest first.
#[tauri::command]
pub async fn journal_entries(
    state: tauri::State<'_, Runtime>,
) -> Result<Vec<JournalEntryDto>, String> {
    state.read().await.journal_entries().map_err(|e| e.to_string())
}

/// Delete a journal entry by id; returns whether one existed.
#[tauri::command]
pub async fn delete_journal_entry(
    state: tauri::State<'_, Runtime>,
    id: String,
) -> Result<bool, String> {
    state
        .read()
        .await
        .delete_journal_entry(&id)
        .map_err(|e| e.to_string())
}

// ── Milestone 3: progressive-disclosure workspace controller ──────────────────

/// Switch visual scope (Base / OffGridTravel / CoupleSandbox*), enforcing the
/// `comrade_state` transition rules. Invalid transitions reject with a typed
/// error message.
#[tauri::command]
pub async fn toggle_app_workspace(
    state: tauri::State<'_, Runtime>,
    target: String,
) -> Result<WorkspaceDto, String> {
    state
        .write()
        .await
        .toggle_workspace(&target)
        .map_err(|e| e.to_string())
}

// ── Sync view-model commands (kept compatible with the existing frontend) ─────

#[tauri::command]
pub async fn workspaces(state: tauri::State<'_, Runtime>) -> Result<Vec<WorkspaceDto>, String> {
    Ok(state.read().await.workspaces())
}

#[tauri::command]
pub async fn current_workspace(state: tauri::State<'_, Runtime>) -> Result<WorkspaceDto, String> {
    Ok(state.read().await.current_workspace())
}

/// Alias retained for the existing webview (`main.js` calls `switch_workspace`).
#[tauri::command]
pub async fn switch_workspace(
    state: tauri::State<'_, Runtime>,
    key: String,
) -> Result<WorkspaceDto, String> {
    state.write().await.toggle_workspace(&key).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn back(state: tauri::State<'_, Runtime>) -> Result<WorkspaceDto, String> {
    Ok(state.write().await.back())
}

#[tauri::command]
pub async fn generate_identity(state: tauri::State<'_, Runtime>) -> Result<IdentityDto, String> {
    state.write().await.generate_identity().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn current_identity(
    state: tauri::State<'_, Runtime>,
) -> Result<Option<IdentityDto>, String> {
    Ok(state.read().await.current_identity())
}

#[tauri::command]
pub async fn extract_payments(
    state: tauri::State<'_, Runtime>,
    text: String,
) -> Result<Vec<UpiIntentDto>, String> {
    state.read().await.extract_payments(&text).map_err(|e| e.to_string())
}

// ── Encrypted media pipeline (NIP-94/96 · Blossom) ────────────────────────────

/// Read a file from disk, encrypt + upload it, and deliver the reference to
/// `target_pubkey`. For path-based callers (e.g. a native file dialog).
#[tauri::command]
pub async fn upload_and_send_media(
    state: tauri::State<'_, Runtime>,
    file_path: String,
    target_pubkey: String,
) -> Result<MediaMessageDto, String> {
    // Reject oversized files by their metadata before reading them into memory.
    let meta = tokio::fs::metadata(&file_path)
        .await
        .map_err(|e| format!("stat file: {e}"))?;
    if meta.len() > MAX_MEDIA_BYTES as u64 {
        return Err(format!(
            "file is {:.1} MB; the limit is 10 MB",
            meta.len() as f64 / 1_048_576.0
        ));
    }
    let bytes = tokio::fs::read(&file_path)
        .await
        .map_err(|e| format!("read file: {e}"))?;
    if bytes.len() > MAX_MEDIA_BYTES {
        return Err("file exceeds the 10 MB limit".to_string());
    }
    let mime = guess_mime(&file_path);
    let caption = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    state
        .read()
        .await
        .upload_and_send_media(&target_pubkey, bytes, &mime, &caption)
        .await
        .map_err(|e| e.to_string())
}

/// Encrypt + upload media supplied as base64 bytes (the webview `<input type=file>`
/// path, which has no real filesystem path to hand to Rust).
#[tauri::command]
pub async fn send_media_bytes(
    state: tauri::State<'_, Runtime>,
    target_pubkey: String,
    mime_type: String,
    caption: String,
    base64: String,
) -> Result<MediaMessageDto, String> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    // Bound the encoded string before decoding so a huge payload can't force a
    // large transient allocation: base64 inflates 4/3, so cap the string length.
    if base64.len() > (MAX_MEDIA_BYTES / 3 + 1) * 4 {
        return Err("file exceeds the 10 MB limit".to_string());
    }
    let bytes = B64
        .decode(base64.as_bytes())
        .map_err(|e| format!("invalid base64: {e}"))?;
    if bytes.len() > MAX_MEDIA_BYTES {
        return Err("file exceeds the 10 MB limit".to_string());
    }
    state
        .read()
        .await
        .upload_and_send_media(&target_pubkey, bytes, &mime_type, &caption)
        .await
        .map_err(|e| e.to_string())
}

/// Resolve a NIP-94 reference, fetch the encrypted blob, and decrypt it.
/// Returns `{ mime_type, base64 }` for the frontend to turn into a `Blob`.
#[tauri::command]
pub async fn download_and_decrypt_media(
    state: tauri::State<'_, Runtime>,
    event_id: String,
) -> Result<MediaBytesDto, String> {
    state
        .read()
        .await
        .download_and_decrypt_media(&event_id)
        .await
        .map_err(|e| e.to_string())
}
