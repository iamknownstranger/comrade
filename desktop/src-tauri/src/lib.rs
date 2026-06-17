/*!
 * Comrade desktop — Tauri 2 backend.
 *
 * Thin `#[tauri::command]` wrappers over the framework-agnostic
 * [`comrade_ui::UiService`]. All real logic lives in `comrade_ui`, which is
 * unit-tested in the main workspace; this layer only marshals to/from the
 * webview over Tauri's IPC.
 */

use std::sync::Mutex;

use comrade_ui::{IdentityDto, UiService, UpiIntentDto, WorkspaceDto};
use tauri::State;

/// Managed application state. A `Mutex` because Tauri command handlers may run
/// concurrently; the guard is mapped to an error string rather than unwrapped.
struct AppState(Mutex<UiService>);

impl AppState {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, UiService>, String> {
        self.0.lock().map_err(|_| "ui state lock poisoned".to_string())
    }
}

#[tauri::command]
fn workspaces(state: State<AppState>) -> Result<Vec<WorkspaceDto>, String> {
    Ok(state.lock()?.workspaces())
}

#[tauri::command]
fn current_workspace(state: State<AppState>) -> Result<WorkspaceDto, String> {
    Ok(state.lock()?.current_workspace())
}

#[tauri::command]
fn switch_workspace(state: State<AppState>, key: String) -> Result<WorkspaceDto, String> {
    state.lock()?.switch_workspace(&key).map_err(|e| e.to_string())
}

#[tauri::command]
fn back(state: State<AppState>) -> Result<WorkspaceDto, String> {
    Ok(state.lock()?.back())
}

#[tauri::command]
fn generate_identity(state: State<AppState>) -> Result<IdentityDto, String> {
    state.lock()?.generate_identity().map_err(|e| e.to_string())
}

#[tauri::command]
fn current_identity(state: State<AppState>) -> Result<Option<IdentityDto>, String> {
    Ok(state.lock()?.current_identity())
}

#[tauri::command]
fn unlock_store(state: State<AppState>, path: String, pin: String) -> Result<(), String> {
    state.lock()?.unlock_store(&path, &pin).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_identity(state: State<AppState>) -> Result<(), String> {
    state.lock()?.save_identity().map_err(|e| e.to_string())
}

#[tauri::command]
fn load_identity(state: State<AppState>) -> Result<Option<IdentityDto>, String> {
    state.lock()?.load_identity().map_err(|e| e.to_string())
}

#[tauri::command]
fn extract_payments(state: State<AppState>, text: String) -> Result<Vec<UpiIntentDto>, String> {
    state.lock()?.extract_payments(&text).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState(Mutex::new(UiService::new())))
        .invoke_handler(tauri::generate_handler![
            workspaces,
            current_workspace,
            switch_workspace,
            back,
            generate_identity,
            current_identity,
            unlock_store,
            save_identity,
            load_identity,
            extract_payments,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Comrade desktop application");
}
