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

use comrade_ui::{ChitthiDto, ComradeRuntime, IdentityDto, UpiIntentDto, WorkspaceDto};
use tokio::sync::RwLock;

/// The live IPC runtime context, as referenced by the bridge spec.
pub type Runtime = Arc<RwLock<ComradeRuntime>>;

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
