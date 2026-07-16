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
    CallRecordDto, CallSessionDto, ChitthiDto, ComradeRuntime, ContactDto, ConversationDto,
    FoundProfileDto, IceServerDto, IdentityDto, JournalEntryDto, MediaBytesDto, MediaMessageDto,
    MessageDto, MessageRequestDto, ProfileDto, SakhaStatusDto, TurnServerStatusDto, UpiIntentDto,
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
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn broadcast_chitthi(
    state: tauri::State<'_, Runtime>,
    content: String,
    reply_to: Option<String>,
) -> Result<String, String> {
    let handles = state.read().await.handles();
    handles
        .broadcast_chitthi(&content, reply_to)
        .await
        .map_err(|e| e.to_string())
}

/// Sync the Sakha/Sakhi shared CRDT ledger to the partner. Returns the event id.
///
/// Takes a cheap, synchronous handle snapshot under the guard (see
/// `comrade_ui::ComradeRuntime::handles`) and runs the relay round-trip after
/// it is dropped — AUDIT P2: never hold the runtime lock across a network
/// await, or one slow/unreachable relay stalls every other command behind it.
#[tauri::command]
pub async fn sync_ledger(state: tauri::State<'_, Runtime>) -> Result<String, String> {
    let handles = state.read().await.handles();
    handles.sync_ledger().await.map_err(|e| e.to_string())
}

// ── Sakha/Sakhi pairing handshake + shared ledger ────────────────────────────

/// Perform the DH pairing handshake with `partner_pubkey` (npub or hex) as
/// `role` ("sakha"/"sakhi"), persist it, and start the background sync loop.
/// Returns the resulting pairing status.
#[tauri::command]
pub async fn pair_sakha(
    state: tauri::State<'_, Runtime>,
    partner_pubkey: String,
    role: String,
) -> Result<SakhaStatusDto, String> {
    state
        .write()
        .await
        .pair_sakha(&partner_pubkey, &role)
        .await
        .map_err(|e| e.to_string())
}

/// This device's Sakha/Sakhi pairing state — lets the frontend offer
/// "continue as paired partner" without asking for the partner's key again.
#[tauri::command]
pub async fn sakha_status(state: tauri::State<'_, Runtime>) -> Result<SakhaStatusDto, String> {
    state.read().await.sakha_status().map_err(|e| e.to_string())
}

/// Append an entry to the shared Sakha/Sakhi ledger. Returns the merged
/// ledger text. Requires a completed pairing (see [`pair_sakha`]).
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn sakha_add_entry(
    state: tauri::State<'_, Runtime>,
    description: String,
    amount_inr: f64,
    paid_by: String,
) -> Result<String, String> {
    let handles = state.read().await.handles();
    handles
        .sakha_add_entry(&description, amount_inr, &paid_by)
        .await
        .map_err(|e| e.to_string())
}

/// The current Sakha/Sakhi ledger text (local CRDT state, no network round trip).
#[tauri::command]
pub async fn sakha_read_ledger(state: tauri::State<'_, Runtime>) -> Result<String, String> {
    state
        .read()
        .await
        .sakha_read_ledger()
        .await
        .map_err(|e| e.to_string())
}

// ── Direct messages, profile & contacts (Telegram-like flow) ──────────────────

/// Send an E2E-encrypted DM to `target` (npub or hex pubkey). The message is
/// persisted to the offline history; returns the stored message DTO.
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn send_dm(
    state: tauri::State<'_, Runtime>,
    target: String,
    content: String,
) -> Result<MessageDto, String> {
    let handles = state.read().await.handles();
    handles.send_dm(&target, &content).await.map_err(|e| e.to_string())
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

/// Full encrypted-media history with `peer`, oldest first — the media
/// counterpart of [`messages_with`], for rendering past attachments inline
/// after a restart.
#[tauri::command]
pub async fn media_with(
    state: tauri::State<'_, Runtime>,
    peer: String,
) -> Result<Vec<MediaMessageDto>, String> {
    state.read().await.media_with(&peer).map_err(|e| e.to_string())
}

/// Send a DM as a reply to a prior message (`reply_to` = replied event id hex).
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn send_dm_reply(
    state: tauri::State<'_, Runtime>,
    target: String,
    content: String,
    reply_to: Option<String>,
) -> Result<MessageDto, String> {
    let handles = state.read().await.handles();
    handles
        .send_dm_reply(&target, &content, reply_to.as_deref())
        .await
        .map_err(|e| e.to_string())
}

// ── Message requests (gate strangers) + receipts ──────────────────────────────

/// Pending message requests (strangers' DMs awaiting accept/block).
#[tauri::command]
pub async fn message_requests(
    state: tauri::State<'_, Runtime>,
) -> Result<Vec<MessageRequestDto>, String> {
    state.read().await.message_requests().map_err(|e| e.to_string())
}

/// Accept a message request (into the chat list; share handle; ack messages).
#[tauri::command]
pub async fn accept_request(state: tauri::State<'_, Runtime>, peer: String) -> Result<(), String> {
    state.read().await.accept_request(&peer).map_err(|e| e.to_string())
}

/// Block a peer (hide + drop future DMs).
#[tauri::command]
pub async fn block_conversation(
    state: tauri::State<'_, Runtime>,
    peer: String,
) -> Result<(), String> {
    state.read().await.block_conversation(&peer).map_err(|e| e.to_string())
}

/// Send a read receipt for a conversation (call when the thread is opened).
#[tauri::command]
pub async fn mark_conversation_read(
    state: tauri::State<'_, Runtime>,
    peer: String,
) -> Result<(), String> {
    state
        .read()
        .await
        .mark_conversation_read(&peer)
        .map_err(|e| e.to_string())
}

// ── Calls (voice/video · WebRTC signalling over the DM channel) ───────────────

/// ICE servers (public STUN + any configured TURN) for `RTCPeerConnection`.
#[tauri::command]
pub async fn call_ice_servers(
    state: tauri::State<'_, Runtime>,
) -> Result<Vec<IceServerDto>, String> {
    Ok(state.read().await.call_ice_servers())
}

/// ICE servers for one connection attempt under `strategy` (`"stun_only"` or
/// `"stun_and_turn"`) — see `comrade_ui::ComradeRuntime::call_ice_servers_for`.
/// Every call starts `"stun_only"` (see [`place_call`]); the frontend retries
/// with `"stun_and_turn"` if the connection never reaches
/// `connected`/`completed`, restarting ICE against the widened server list.
#[tauri::command]
pub async fn call_ice_servers_for(
    state: tauri::State<'_, Runtime>,
    strategy: String,
) -> Result<Vec<IceServerDto>, String> {
    Ok(state.read().await.call_ice_servers_for(&strategy))
}

/// The 4-emoji short authentication string (SAS) for the in-progress call's
/// `local_sdp`/`remote_sdp`, to read aloud and compare. `None` when either
/// side's SDP has no `a=fingerprint:` line to derive one from — an honest
/// "can't verify", not an error. See `comrade_ui::ComradeRuntime::call_sas`.
#[tauri::command]
pub async fn call_sas(
    state: tauri::State<'_, Runtime>,
    local_sdp: String,
    remote_sdp: String,
) -> Result<Option<Vec<String>>, String> {
    Ok(state.read().await.call_sas(&local_sdp, &remote_sdp))
}

/// Configure (blank `url` clears) the TURN relay used for calls.
#[tauri::command]
pub async fn set_turn_server(
    state: tauri::State<'_, Runtime>,
    url: String,
    username: String,
    credential: String,
) -> Result<(), String> {
    state
        .read()
        .await
        .set_turn_server(&url, &username, &credential)
        .map_err(|e| e.to_string())
}

/// The "is a relay configured" diagnostic status for a settings screen — the
/// URL only, never the username/credential, so it is safe to show or log
/// directly. See `comrade_ui::ComradeRuntime::turn_server_status`.
#[tauri::command]
pub async fn turn_server_status(
    state: tauri::State<'_, Runtime>,
) -> Result<TurnServerStatusDto, String> {
    Ok(state.read().await.turn_server_status())
}

/// Begin a call to `peer` (`media` = "audio"/"video"); returns the call session.
#[tauri::command]
pub async fn place_call(
    state: tauri::State<'_, Runtime>,
    peer: String,
    media: String,
) -> Result<CallSessionDto, String> {
    state.read().await.place_call(&peer, &media).map_err(|e| e.to_string())
}

/// Send one call-signaling payload (`signal_json` = a CallSignal) to `peer`.
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn send_call_signal(
    state: tauri::State<'_, Runtime>,
    peer: String,
    call_id: String,
    media: String,
    signal_json: String,
) -> Result<(), String> {
    let handles = state.read().await.handles();
    handles
        .send_call_signal(&peer, &call_id, &media, &signal_json)
        .await
        .map_err(|e| e.to_string())
}

/// Send a `Hangup` with `reason` to end/reject a call.
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn hangup_call(
    state: tauri::State<'_, Runtime>,
    peer: String,
    call_id: String,
    media: String,
    reason: String,
) -> Result<(), String> {
    let handles = state.read().await.handles();
    handles
        .hangup_call(&peer, &call_id, &media, &reason)
        .await
        .map_err(|e| e.to_string())
}

/// Persist a finished call to the call log.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn log_call(
    state: tauri::State<'_, Runtime>,
    peer: String,
    call_id: String,
    media: String,
    incoming: bool,
    outcome: String,
    started_at: u64,
    duration_secs: u64,
) -> Result<CallRecordDto, String> {
    state
        .read()
        .await
        .log_call(
            &peer,
            &call_id,
            &media,
            incoming,
            &outcome,
            started_at,
            duration_secs,
        )
        .map_err(|e| e.to_string())
}

/// The call log (single `peer`, or all peers when `None`), newest first.
#[tauri::command]
pub async fn call_history(
    state: tauri::State<'_, Runtime>,
    peer: Option<String>,
) -> Result<Vec<CallRecordDto>, String> {
    state
        .read()
        .await
        .call_history(peer.as_deref())
        .map_err(|e| e.to_string())
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
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn search_profiles(
    state: tauri::State<'_, Runtime>,
    query: String,
) -> Result<Vec<FoundProfileDto>, String> {
    let handles = state.read().await.handles();
    handles.search_profiles(&query).await.map_err(|e| e.to_string())
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
        .await
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
    state
        .write()
        .await
        .toggle_workspace(&key)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn back(state: tauri::State<'_, Runtime>) -> Result<WorkspaceDto, String> {
    Ok(state.write().await.back().await)
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
///
/// See [`sync_ledger`]'s doc comment for the lock discipline (the guard here
/// is taken right before the upload, after the file I/O above — which never
/// touches the runtime state at all).
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
    let handles = state.read().await.handles();
    handles
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
    let handles = state.read().await.handles();
    handles
        .upload_and_send_media(&target_pubkey, bytes, &mime_type, &caption)
        .await
        .map_err(|e| e.to_string())
}

/// Resolve a NIP-94 reference, fetch the encrypted blob, and decrypt it.
/// Returns `{ mime_type, base64 }` for the frontend to turn into a `Blob`.
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn download_and_decrypt_media(
    state: tauri::State<'_, Runtime>,
    event_id: String,
) -> Result<MediaBytesDto, String> {
    let handles = state.read().await.handles();
    handles
        .download_and_decrypt_media(&event_id)
        .await
        .map_err(|e| e.to_string())
}
