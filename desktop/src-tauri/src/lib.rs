/*!
 * Comrade desktop — Tauri 2 backend (Command & Event Bridge).
 *
 * Thin async `#[tauri::command]` wrappers (in [`commands`]) over the
 * framework-agnostic [`comrade_ui::ComradeRuntime`]. All real logic lives in the
 * workspace crates, which are unit-tested and Send/Sync-checked; this layer only
 * marshals to/from the webview over Tauri's IPC and forwards background events.
 *
 * Event stream (Milestone 2): on startup we subscribe to the runtime's event bus
 * and forward every [`BridgeEvent`] to the webview with `emit("comrade://event")`.
 * Incoming Chitthis (Kind-1) and encrypted DMs (Kind-4) captured inside the core
 * Tokio loops therefore arrive in JavaScript as native window events, without
 * ever touching the rendering thread.
 */

mod commands;

use std::sync::Arc;

use comrade_ui::ComradeRuntime;
use tauri::{Emitter, Manager};
use tokio::sync::RwLock;

use commands::Runtime;

/// The window event name carrying every [`comrade_ui::BridgeEvent`] payload.
pub const EVENT_CHANNEL: &str = "comrade://event";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let runtime: Runtime = Arc::new(RwLock::new(ComradeRuntime::new()));

    tauri::Builder::default()
        .manage(runtime)
        .setup(|app| {
            // Forward the runtime's event bus to the webview. Subscribing here —
            // before any unlock — guarantees we never miss the first events.
            let handle = app.handle().clone();
            let state = app.state::<Runtime>().inner().clone();

            tauri::async_runtime::spawn(async move {
                let mut rx = state.read().await.subscribe_events();
                loop {
                    match rx.recv().await {
                        Ok(event) => {
                            if let Err(e) = handle.emit(EVENT_CHANNEL, &event) {
                                tracing::warn!("failed to emit bridge event: {e}");
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("webview event forwarder lagged by {n} events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // IPC bridge (Milestones 1 & 3)
            commands::unlock_comrade_vault,
            commands::fetch_sabha_timeline,
            commands::broadcast_chitthi,
            commands::sync_ledger,
            commands::toggle_app_workspace,
            // Encrypted media pipeline
            commands::upload_and_send_media,
            commands::send_media_bytes,
            commands::download_and_decrypt_media,
            // Direct messages, profile & contacts
            commands::send_dm,
            commands::conversations,
            commands::messages_with,
            commands::current_profile,
            commands::set_username,
            commands::add_contact,
            commands::set_contact_alias,
            commands::remove_contact,
            commands::refresh_peer_profiles,
            commands::list_contacts,
            commands::search_profiles,
            // View-model commands (existing frontend)
            commands::workspaces,
            commands::current_workspace,
            commands::switch_workspace,
            commands::back,
            commands::generate_identity,
            commands::current_identity,
            commands::extract_payments,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Comrade desktop application");
}
