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
    ChitthiDto, ComradeRuntime, IdentityDto, MediaBytesDto, MediaMessageDto, UpiIntentDto,
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
    let bytes = tokio::fs::read(&file_path)
        .await
        .map_err(|e| format!("read file: {e}"))?;
    if bytes.len() > MAX_MEDIA_BYTES {
        return Err(format!(
            "file is {:.1} MB; the limit is 10 MB",
            bytes.len() as f64 / 1_048_576.0
        ));
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
