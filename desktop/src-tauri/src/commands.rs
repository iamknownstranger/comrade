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
    AppAction, AttachmentRoute, AttentionDayDto, AttentionSummaryDto, CallRecordDto,
    CallSessionDto, ChatCommand, ChitthiDto, CommandSpec, ComradeDto, ComradeRuntime, ContactDto,
    ConversationDto, CrisisResourceDto, FocusSessionDto, FoundProfileDto, IceServerDto,
    IdentityDto, JournalEntryDto, MediaBytesDto, MediaMessageDto, Mention, MentionMatchDto,
    MessageDto, MessageRequestDto, MusicService, OfferOutcomeDto, PeerProfileDto, PlayPlan,
    PlayRoute, PlayTargetDto, PresenceDto, ProfileDto, SakhaStatusDto, SavedReadDto,
    SavedReadSummaryDto, StretchStepDto, TaraChatDto, TaraMessageDto, TaskDto, TaskState,
    ThreadDto, ThreadSummaryDto, TopicDto, TurnServerStatusDto, UpiIntentDto, WorkspaceDto,
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
    handles
        .send_dm(&target, &content)
        .await
        .map_err(|e| e.to_string())
}

/// The chat list (one entry per peer, newest first) from the offline history.
#[tauri::command]
pub async fn conversations(
    state: tauri::State<'_, Runtime>,
) -> Result<Vec<ConversationDto>, String> {
    state
        .read()
        .await
        .conversations()
        .map_err(|e| e.to_string())
}

/// Full offline message history with `peer`, oldest first.
#[tauri::command]
pub async fn messages_with(
    state: tauri::State<'_, Runtime>,
    peer: String,
) -> Result<Vec<MessageDto>, String> {
    state
        .read()
        .await
        .messages_with(&peer)
        .map_err(|e| e.to_string())
}

/// Full encrypted-media history with `peer`, oldest first — the media
/// counterpart of [`messages_with`], for rendering past attachments inline
/// after a restart.
#[tauri::command]
pub async fn media_with(
    state: tauri::State<'_, Runtime>,
    peer: String,
) -> Result<Vec<MediaMessageDto>, String> {
    state
        .read()
        .await
        .media_with(&peer)
        .map_err(|e| e.to_string())
}

// ── Message actions (local device state — see `message_actions.mjs`) ─────────
//
// Star/pin/delete-for-me are plain, synchronous `ComradeRuntime` methods (no
// network `.await` inside), so — unlike [`delete_message_for_everyone`] and
// `forward_message`, deliberately *not* exposed here yet — holding the read
// guard across the call is the same discipline [`messages_with`]/[`media_with`]
// above already use, not the AUDIT P2 hazard [`sync_ledger`]'s doc warns about.

/// Star or un-star one of `peer`'s messages, for the "starred messages" list.
/// Local device state only — see `comrade_ui::ComradeRuntime::star_message`.
/// Returns whether the stored state actually changed.
#[tauri::command]
pub async fn star_message(
    state: tauri::State<'_, Runtime>,
    peer: String,
    message_id: String,
    starred: bool,
) -> Result<bool, String> {
    state
        .read()
        .await
        .star_message(&peer, &message_id, starred)
        .map_err(|e| e.to_string())
}

/// Pin one of `peer`'s messages. Refused once the conversation is already at
/// its per-conversation cap — see `comrade_ui::ComradeRuntime::pin_message`.
/// `false` (not an error) if it was already pinned.
#[tauri::command]
pub async fn pin_message(
    state: tauri::State<'_, Runtime>,
    peer: String,
    message_id: String,
) -> Result<bool, String> {
    state
        .read()
        .await
        .pin_message(&peer, &message_id)
        .map_err(|e| e.to_string())
}

/// Unpin one of `peer`'s messages. `true` if it was pinned.
#[tauri::command]
pub async fn unpin_message(
    state: tauri::State<'_, Runtime>,
    peer: String,
    message_id: String,
) -> Result<bool, String> {
    state
        .read()
        .await
        .unpin_message(&peer, &message_id)
        .map_err(|e| e.to_string())
}

/// Hide one of `peer`'s messages on this device only — a tombstone, so a
/// relay's cold-start rescan (or a mesh replay) cannot bring it back. See
/// `comrade_ui::ComradeRuntime::delete_message_for_me`.
///
/// The *other* half of Android's and Telegram's pair, "delete for everyone",
/// is deliberately not a command here: `ComradeRuntime::delete_message_for_everyone`
/// is `async` and ends in a relay send with no `handles()`-detached form (unlike
/// [`send_dm`]/[`assign_thread`]), so wiring it the same way as this command
/// would hold the runtime lock across a network `.await` — exactly what
/// [`sync_ledger`]'s doc comment calls out as AUDIT P2. Giving it a
/// detached path is a `comrade_ui` change, not one this file makes alone.
#[tauri::command]
pub async fn delete_message_for_me(
    state: tauri::State<'_, Runtime>,
    peer: String,
    message_id: String,
) -> Result<(), String> {
    state
        .read()
        .await
        .delete_message_for_me(&peer, &message_id)
        .map_err(|e| e.to_string())
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
    state
        .read()
        .await
        .message_requests()
        .map_err(|e| e.to_string())
}

/// Accept a message request (into the chat list; share handle; ack messages).
#[tauri::command]
pub async fn accept_request(state: tauri::State<'_, Runtime>, peer: String) -> Result<(), String> {
    state
        .read()
        .await
        .accept_request(&peer)
        .map_err(|e| e.to_string())
}

/// Block a peer (hide + drop future DMs).
#[tauri::command]
pub async fn block_conversation(
    state: tauri::State<'_, Runtime>,
    peer: String,
) -> Result<(), String> {
    state
        .read()
        .await
        .block_conversation(&peer)
        .map_err(|e| e.to_string())
}

/// Send a read receipt for a conversation (call when the thread is opened).
///
/// Resolves to the read position the thread had *before* this call (unix
/// seconds, 0 = never opened). The webview ignores it today — it has no
/// unread-divider UI yet — but the value is the same one Android and the
/// Flutter app position their threads with, so wiring it here later needs no
/// engine change.
#[tauri::command]
pub async fn mark_conversation_read(
    state: tauri::State<'_, Runtime>,
    peer: String,
) -> Result<u64, String> {
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
/// `local_sdp`/`remote_sdp`. `None` when either side's SDP has no
/// `a=fingerprint:` line to derive one from — an honest "can't verify", not an
/// error. See `comrade_ui::ComradeRuntime::call_sas`.
///
/// **No frontend calls this any more.** The in-call SAS row was replaced by a
/// network-strength indicator: Comrade's SDP rides the NIP-44 gift-wrapped DM
/// channel, so both fingerprints are already authenticated by the peer's Nostr
/// key before a call is answered, which made the out-of-band read-aloud check
/// near-redundant. The primitive and its tests are kept deliberately — it is
/// the one piece a future explicit "verify this call" flow would need, and
/// deleting a tested crypto primitive to save a registration line is the wrong
/// trade.
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
    state
        .read()
        .await
        .place_call(&peer, &media)
        .map_err(|e| e.to_string())
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

/// Invite `peer` to watch or listen to something together.
///
/// `content_json` is a `comrade_core::together::TogetherContent` — either
/// `{"kind":"local_file","duration_ms":N}` or `{"kind":"youtube","video_id":"…"}`.
/// The video id is validated in core, on send *and* receive, so no UI can put an
/// unchecked peer-supplied string into an `<iframe src>`.
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn together_start(
    state: tauri::State<'_, Runtime>,
    peer: String,
    content_json: String,
) -> Result<comrade_ui::TogetherSessionDto, String> {
    let content: comrade_ui::TogetherContent =
        serde_json::from_str(&content_json).map_err(|e| format!("invalid content: {e}"))?;
    let handles = state.read().await.handles();
    handles
        .together_start(&peer, content)
        .await
        .map_err(|e| e.to_string())
}

/// Accept the invitation we were sent.
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn together_join(state: tauri::State<'_, Runtime>) -> Result<(), String> {
    let handles = state.read().await.handles();
    handles.together_join().await.map_err(|e| e.to_string())
}

/// Play, pause or seek — one command, because all three are one statement.
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn together_set_state(
    state: tauri::State<'_, Runtime>,
    pos_ms: u64,
    playing: bool,
    effective_in_ms: u64,
) -> Result<(), String> {
    let handles = state.read().await.handles();
    handles
        .together_set_state(pos_ms, playing, effective_in_ms)
        .await
        .map_err(|e| e.to_string())
}

/// Leave the session.
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn together_end(state: tauri::State<'_, Runtime>) -> Result<(), String> {
    let handles = state.read().await.handles();
    handles.together_end().await.map_err(|e| e.to_string())
}

/// Report where our own player is, without sending anything.
///
/// The player calls this on a timer while it plays, so it must not queue behind
/// a vault unlock; it is skipped under contention like `note_draft`, which fails
/// in the harmless direction (the next report is a second away).
#[tauri::command]
pub async fn together_report_position(
    state: tauri::State<'_, Runtime>,
    pos_ms: u64,
    playing: bool,
    output_latency_ms: u64,
) -> Result<(), String> {
    state
        .read()
        .await
        .together_report_position(pos_ms, playing, output_latency_ms);
    Ok(())
}

/// The live session, if there is one.
#[tauri::command]
pub async fn together_session(
    state: tauri::State<'_, Runtime>,
) -> Result<Option<comrade_ui::TogetherSessionDto>, String> {
    Ok(state.read().await.together_session())
}

/// Declare whether the direct peer channel is carrying this session's signals,
/// so they take it rather than a relay — tens of milliseconds against hundreds,
/// which is what decides how tight the sync can be.
///
/// Should be set back to `false` when the channel closes, and the runtime no
/// longer depends on that happening: a declaration expires after two heartbeats
/// of silence on the channel and sends go back to the relay by themselves
/// (`comrade_core::together::direct_path_live`). Reporting it promptly still
/// matters — it moves the fallback from twenty seconds away to immediate.
#[tauri::command]
pub async fn together_direct_ready(
    state: tauri::State<'_, Runtime>,
    ready: bool,
) -> Result<(), String> {
    state.read().await.together_direct_ready(ready);
    Ok(())
}

/// Hand over an envelope that arrived on the direct channel.
///
/// Deliberately less privileged than the relay path: it cannot open a session,
/// and the sender is the session's peer by definition rather than by anything in
/// the payload. Treated as unvalidated input and dropped on anything unexpected.
#[tauri::command]
pub async fn together_receive_direct(
    state: tauri::State<'_, Runtime>,
    json: String,
) -> Result<(), String> {
    state.read().await.together_receive_direct(&json);
    Ok(())
}

/// Send one step of handing the file over, for when only one of you has it.
///
/// `signal_json` is a `comrade_core::share::ShareSignal`. It crosses as JSON
/// rather than as separate commands per step because the transfer protocol is
/// the sort of thing that grows a step, and the UI's job is to relay them, not
/// to have an opinion about how many there are.
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn together_share(
    state: tauri::State<'_, Runtime>,
    signal_json: String,
) -> Result<(), String> {
    let signal: comrade_ui::ShareSignal =
        serde_json::from_str(&signal_json).map_err(|e| format!("invalid share signal: {e}"))?;
    let handles = state.read().await.handles();
    handles
        .together_share(signal)
        .await
        .map_err(|e| e.to_string())
}

/// Which road an attachment of `total_bytes` takes: `"hosted"` or `"peer_to_peer"`.
///
/// A command rather than a constant in the webview. The threshold *is* the hosted
/// ceiling (`comrade_core::media::MAX_MEDIA_BYTES`), so a frontend keeping its own
/// copy of 10 MB is a frontend that disagrees with the core the day that number
/// moves — and the two roads have different failure modes a person must be told
/// about, which makes this a question worth asking rather than assuming.
#[tauri::command]
pub async fn attachment_route_for_bytes(total_bytes: u64) -> Result<AttachmentRoute, String> {
    Ok(comrade_ui::route_for_bytes(total_bytes))
}

/// Send one step of handing a large attachment to `peer`, scoped to `transfer_id`.
///
/// `signal_json` is a `comrade_core::handoff::HandoffSignal`. It crosses as JSON
/// for the same reason [`together_share`]'s does: the protocol is the sort of
/// thing that grows a step, and this layer's job is to relay one, not to have an
/// opinion about how many there are.
///
/// Unlike [`together_share`] there is no session to be inside — nobody starts a
/// watch-together session to send a video file — so the gate is the one the
/// runtime applies on *receipt* (an accepted conversation, the same bar a call
/// signal clears). See `comrade_ui::ComradeRuntime::attachment_handoff_send`.
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn attachment_handoff_send(
    state: tauri::State<'_, Runtime>,
    peer: String,
    transfer_id: String,
    signal_json: String,
) -> Result<(), String> {
    let signal: comrade_ui::HandoffSignal =
        serde_json::from_str(&signal_json).map_err(|e| format!("invalid handoff signal: {e}"))?;
    let handles = state.read().await.handles();
    handles
        .attachment_handoff_send(&peer, &transfer_id, signal)
        .await
        .map_err(|e| e.to_string())
}

/// Which ICE servers a *transfer* connection may be built with.
///
/// Not the same list a call gets, and that is the point: a relayed call is a
/// few tens of kilobits and entirely reasonable, while a relayed film is
/// gigabytes through a machine that volunteered for neither. Under the default
/// policy the TURN entries are dropped here, so the transfer connection cannot
/// gather a relay candidate at all — the check further down is the second line,
/// not the first.
#[tauri::command]
pub async fn share_ice_servers(
    state: tauri::State<'_, Runtime>,
) -> Result<Vec<comrade_ui::IceServerDto>, String> {
    let rt = state.read().await;
    let strategy = if rt.share_ice_servers_allowed() {
        "stun_and_turn"
    } else {
        "stun_only"
    };
    Ok(rt.call_ice_servers_for(strategy))
}

/// Judge the path ICE actually chose for a transfer connection.
///
/// The two candidate types come from the selected pair in the webview's own
/// `RTCStatsReport`. Anything unrecognised — including "ICE has not settled
/// yet" — classifies as unknown and is refused, because "we could not tell"
/// must never be read as "it was fine".
#[tauri::command]
pub async fn share_transfer_verdict(
    state: tauri::State<'_, Runtime>,
    local_candidate_type: String,
    remote_candidate_type: String,
    total_bytes: u64,
    consent_granted: bool,
) -> Result<comrade_ui::ShareVerdictDto, String> {
    Ok(state.read().await.share_transfer_verdict(
        &local_candidate_type,
        &remote_candidate_type,
        total_bytes,
        consent_granted,
    ))
}

/// What this device does when the only path is a relay, as a JSON
/// `comrade_core::share::transport::RelayPolicy`.
#[tauri::command]
pub async fn share_relay_policy(state: tauri::State<'_, Runtime>) -> Result<String, String> {
    serde_json::to_string(&state.read().await.share_relay_policy()).map_err(|e| e.to_string())
}

/// Change it. Takes effect on the next transfer connection; one already running
/// keeps the rules it started under, because tearing down a transfer someone is
/// watching from is a worse answer than letting it finish.
#[tauri::command]
pub async fn set_share_relay_policy(
    state: tauri::State<'_, Runtime>,
    policy_json: String,
) -> Result<(), String> {
    let policy: comrade_ui::RelayPolicy =
        serde_json::from_str(&policy_json).map_err(|e| format!("invalid relay policy: {e}"))?;
    state
        .read()
        .await
        .set_share_relay_policy(policy)
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

/// Set (or clear, with an empty string) this identity's bio, and republish.
#[tauri::command]
pub async fn set_about(
    state: tauri::State<'_, Runtime>,
    about: String,
) -> Result<ProfileDto, String> {
    state
        .write()
        .await
        .set_about(&about)
        .await
        .map_err(|e| e.to_string())
}

/// Everything a profile page draws for one peer, from the local cache alone —
/// no relay round trip, so it answers offline and immediately.
#[tauri::command]
pub async fn peer_profile(
    state: tauri::State<'_, Runtime>,
    npub: String,
) -> Result<PeerProfileDto, String> {
    state
        .read()
        .await
        .peer_profile(&npub)
        .map_err(|e| e.to_string())
}

/// A peer's cached avatar bytes, base64, or `None` to draw initials. Reads the
/// encrypted store and never the network.
#[tauri::command]
pub async fn peer_avatar(
    state: tauri::State<'_, Runtime>,
    npub: String,
) -> Result<Option<MediaBytesDto>, String> {
    state
        .read()
        .await
        .peer_avatar(&npub)
        .map_err(|e| e.to_string())
}

/// Whether peer-published pictures may be fetched at all (default on).
#[tauri::command]
pub async fn remote_avatars_enabled(state: tauri::State<'_, Runtime>) -> Result<bool, String> {
    state
        .read()
        .await
        .remote_avatars_enabled()
        .map_err(|e| e.to_string())
}

/// Turn peer-published picture fetching on or off.
#[tauri::command]
pub async fn set_remote_avatars_enabled(
    state: tauri::State<'_, Runtime>,
    on: bool,
) -> Result<(), String> {
    state
        .read()
        .await
        .set_remote_avatars_enabled(on)
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
    state
        .read()
        .await
        .add_contact(&npub, &alias)
        .map_err(|e| e.to_string())
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
    state
        .read()
        .await
        .remove_contact(&npub)
        .map_err(|e| e.to_string())
}

/// Refresh cached peer profiles (bounded, TTL-gated). Returns how many
/// display names changed; reload the chat list when > 0.
#[tauri::command]
pub async fn refresh_peer_profiles(state: tauri::State<'_, Runtime>) -> Result<usize, String> {
    // Detach the refresher under a briefly-held guard, then run guard-free:
    // holding the shared lock across relay round-trips would block every
    // other command (AUDIT P2: no guard held across network awaits).
    let refresher = {
        state
            .read()
            .await
            .profile_refresher()
            .map_err(|e| e.to_string())?
    };
    refresher.run().await.map_err(|e| e.to_string())
}

/// All saved contacts, alias-sorted.
#[tauri::command]
pub async fn list_contacts(state: tauri::State<'_, Runtime>) -> Result<Vec<ContactDto>, String> {
    state
        .read()
        .await
        .list_contacts()
        .map_err(|e| e.to_string())
}

// ── Comrades (chosen-peer presence) ───────────────────────────────────────────

/// Choose (or un-choose) a contact as a comrade: announce our presence to
/// them, and start believing what they say about theirs. See
/// `comrade_ui::ComradeRuntime::set_comrade` for what that discloses (and
/// what it deliberately cannot do — presence is mutual by construction).
#[tauri::command]
pub async fn set_comrade(
    state: tauri::State<'_, Runtime>,
    npub: String,
    comrade: bool,
) -> Result<ContactDto, String> {
    state
        .read()
        .await
        .set_comrade(&npub, comrade)
        .map_err(|e| e.to_string())
}

/// Every comrade with their live presence, online first.
#[tauri::command]
pub async fn comrades(state: tauri::State<'_, Runtime>) -> Result<Vec<ComradeDto>, String> {
    state.read().await.comrades().map_err(|e| e.to_string())
}

/// One peer's live presence, or `null` if no beacon ever arrived from them.
#[tauri::command]
pub async fn peer_presence(
    state: tauri::State<'_, Runtime>,
    npub: String,
) -> Result<Option<PresenceDto>, String> {
    state
        .read()
        .await
        .peer_presence(&npub)
        .map_err(|e| e.to_string())
}

/// Announce this window's presence to every comrade (on focus/blur, or on the
/// way out), returning how many beacons a relay accepted. Never errors: a
/// relay hiccup must not surface in a UI that only calls this in passing.
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
/// Infallible in substance, `Result` in signature: Tauri requires an async
/// command that borrows its state (`State<'_, _>`) to return a `Result`, so the
/// count is wrapped in `Ok`. `invoke` resolves with the number either way, so
/// the SPA sees no difference.
#[tauri::command]
pub async fn announce_presence(
    state: tauri::State<'_, Runtime>,
    online: bool,
) -> Result<u64, String> {
    let handles = state.read().await.handles();
    Ok(handles.announce_presence(online).await)
}

// ── The nudge (abandoned drafts — see `comrade_core::nudge`) ──────────────────

/// There is unsent text in `peer`'s composer, as of now.
///
/// Discloses nothing by itself — it starts a local clock. If that draft is
/// later abandoned and `peer` is a comrade, their device is told *that it
/// happened*: never the text, its length, or how the writing ended.
///
/// `Result` only because Tauri requires it of a borrowing async command (see
/// [`announce_presence`]); there is no failure to report.
#[tauri::command]
pub async fn note_draft(state: tauri::State<'_, Runtime>, peer: String) -> Result<(), String> {
    state.read().await.note_draft(&peer);
    Ok(())
}

/// That draft is gone — cleared, or left behind when the conversation was
/// switched away from. Safe to call unconditionally; an empty composer does
/// nothing.
#[tauri::command]
pub async fn abandon_draft(state: tauri::State<'_, Runtime>, peer: String) -> Result<(), String> {
    state.read().await.abandon_draft(&peer);
    Ok(())
}

/// Tell every comrade, once, that this person might need them — the deliberate
/// trigger behind the breathing screen. Returns how many nudges a relay
/// accepted.
///
/// The same envelope an abandoned draft sends, so a comrade cannot tell which
/// happened, and the same cooldown, so the two never add up to two
/// notifications. Never errors: a locked vault or no comrades is a quiet `0`.
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn nudge_comrades(state: tauri::State<'_, Runtime>) -> Result<u64, String> {
    let handles = state.read().await.handles();
    Ok(handles.nudge_comrades().await)
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
    handles
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
    state
        .read()
        .await
        .journal_entries()
        .map_err(|e| e.to_string())
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

/// Hand one journal entry to one peer, as an ordinary DM — a copy; the entry is
/// not marked, moved or changed (`RuntimeHandles::share_journal_entry`).
///
/// Registered ahead of this window's journal UI, which does not exist yet
/// (`docs/ATTENTION.md` OQ14) — the same staging the three commands above went
/// through. Receiving a shared note already works here: `messages_with` carries
/// `shared_note` on every message, so a note sent from a phone draws as a card
/// in this window today.
///
/// See [`sync_ledger`]'s doc comment for the lock discipline.
#[tauri::command]
pub async fn share_journal_entry(
    state: tauri::State<'_, Runtime>,
    peer: String,
    entry_id: String,
) -> Result<MessageDto, String> {
    let handles = state.read().await.handles();
    handles
        .share_journal_entry(&peer, &entry_id)
        .await
        .map_err(|e| e.to_string())
}

// ── Tara (reflective companion — strictly local, not therapy) ──────────────────

/// Send a message to Tara and get her on-device reply. A `crisis == true`
/// reply obliges the frontend to render [`tara_crisis_resources`] with it.
#[tauri::command]
pub async fn tara_send(
    state: tauri::State<'_, Runtime>,
    text: String,
) -> Result<TaraMessageDto, String> {
    state
        .read()
        .await
        .tara_send(&text)
        .map_err(|e| e.to_string())
}

/// The whole Tara thread, oldest-first (chat order).
#[tauri::command]
pub async fn tara_thread(state: tauri::State<'_, Runtime>) -> Result<Vec<TaraMessageDto>, String> {
    state.read().await.tara_thread().map_err(|e| e.to_string())
}

/// Delete the entire Tara thread; returns how many turns were removed.
#[tauri::command]
pub async fn clear_tara_thread(state: tauri::State<'_, Runtime>) -> Result<u64, String> {
    state
        .read()
        .await
        .clear_tara_thread()
        .map_err(|e| e.to_string())
}

/// The opener shown while the thread is empty — journal mood markers only.
#[tauri::command]
pub async fn tara_opener(state: tauri::State<'_, Runtime>) -> Result<String, String> {
    state.read().await.tara_opener().map_err(|e| e.to_string())
}

/// The crisis helplines Tara hands off to.
#[tauri::command]
pub async fn tara_crisis_resources(
    state: tauri::State<'_, Runtime>,
) -> Result<Vec<CrisisResourceDto>, String> {
    Ok(state.read().await.tara_crisis_resources())
}

// ── In-chat commands, tasks and offers ─────────────────────────────────────────

/// What the text in a composer means. Pure — the composer calls this as the user
/// types, which is what drives the `/` picker and the mention chips.
#[tauri::command]
pub async fn parse_chat_command(
    state: tauri::State<'_, Runtime>,
    text: String,
) -> Result<ChatCommand, String> {
    Ok(state.read().await.parse_chat_command(&text))
}

/// Every command the composer offers, for `/`-autocomplete and `/help`.
#[tauri::command]
pub async fn chat_command_catalog(
    state: tauri::State<'_, Runtime>,
) -> Result<Vec<CommandSpec>, String> {
    Ok(state.read().await.chat_command_catalog())
}

/// Every `@handle` in `text`, unresolved.
#[tauri::command]
pub async fn chat_mentions(
    state: tauri::State<'_, Runtime>,
    text: String,
) -> Result<Vec<Mention>, String> {
    Ok(state.read().await.chat_mentions(&text))
}

/// Every `@handle` in `text`, resolved against the saved contacts. A match with
/// no `npub` but a non-empty `candidates` is an ambiguity to ask about.
#[tauri::command]
pub async fn resolve_mentions(
    state: tauri::State<'_, Runtime>,
    text: String,
) -> Result<Vec<MentionMatchDto>, String> {
    state
        .read()
        .await
        .resolve_mentions(&text)
        .map_err(|e| e.to_string())
}

/// How far a `/play` query gets without a network or a library.
#[tauri::command]
pub async fn play_query(
    state: tauri::State<'_, Runtime>,
    query: String,
    service: Option<MusicService>,
) -> Result<PlayTargetDto, String> {
    Ok(state.read().await.play_query(&query, service))
}

/// What to do about a `/play`, once the caller has searched its own library.
///
/// Pure, so it never touches the runtime — it is a command only so the decision
/// stays in one place across the frontends rather than being reimplemented in JS
/// the day desktop grows a player (`docs/TOGETHER.md` §9).
///
/// `link` and `access` are what decide a streaming-service track: the same
/// Spotify URL routes to a session on a window with a Premium account connected
/// and to a signpost on one without, and nothing in the URL tells them apart.
/// This window connects to nothing yet, so it passes an empty `access` and gets
/// exactly the behaviour it had before — see `docs/TOGETHER.md` §11.
#[tauri::command]
pub fn play_route(
    plan: PlayPlan,
    found_local_copy: bool,
    link: Option<comrade_ui::MusicLink>,
    access: Option<comrade_ui::ServiceAccess>,
) -> PlayRoute {
    // `None` is "this frontend has no service integration", not "I forgot to
    // say" — and it must resolve to no access rather than to a default that
    // could claim an account the window does not have.
    comrade_ui::play_route(
        plan,
        found_local_copy,
        link,
        access.unwrap_or_else(comrade_ui::ServiceAccess::none),
    )
}

// ── Threads and topics (see `comrade_core::topic`) ───────────────────────────

/// Every topic in `peer`'s conversation, oldest first, with live counts.
/// Closed ones are included — the archive has to be reachable.
#[tauri::command]
pub async fn topics(
    state: tauri::State<'_, Runtime>,
    peer: String,
) -> Result<Vec<TopicDto>, String> {
    state.read().await.topics(&peer).map_err(|e| e.to_string())
}

/// Every thread in `peer`'s conversation, most recently active first.
/// `topic_slug` of `None` is *all* threads, not the unfiled ones.
#[tauri::command]
pub async fn threads(
    state: tauri::State<'_, Runtime>,
    peer: String,
    topic_slug: Option<String>,
) -> Result<Vec<ThreadSummaryDto>, String> {
    state
        .read()
        .await
        .threads(&peer, topic_slug)
        .map_err(|e| e.to_string())
}

/// One thread in full. `root_id` may name any message in it — the walk up the
/// reply chain happens in core, so this window and the phone cannot disagree
/// about which thread a bubble belongs to.
#[tauri::command]
pub async fn thread(
    state: tauri::State<'_, Runtime>,
    peer: String,
    root_id: String,
) -> Result<ThreadDto, String> {
    state
        .read()
        .await
        .thread(&peer, &root_id)
        .map_err(|e| e.to_string())
}

/// The id of the thread a message belongs to.
#[tauri::command]
pub async fn thread_root(
    state: tauri::State<'_, Runtime>,
    peer: String,
    message_id: String,
) -> Result<String, String> {
    state
        .read()
        .await
        .thread_root(&peer, &message_id)
        .map_err(|e| e.to_string())
}

/// Name a topic and tell the peer. Idempotent: naming one that exists returns
/// it, because the slug is the id.
#[tauri::command]
pub async fn create_topic(
    state: tauri::State<'_, Runtime>,
    peer: String,
    name: String,
) -> Result<TopicDto, String> {
    let handles = state.read().await.handles();
    handles
        .create_topic(&peer, &name)
        .await
        .map_err(|e| e.to_string())
}

/// File the thread containing `message_id` under `topic_name`, creating the
/// topic if it is new — or, with `None`, take it out of wherever it was.
#[tauri::command]
pub async fn assign_thread(
    state: tauri::State<'_, Runtime>,
    peer: String,
    message_id: String,
    topic_name: Option<String>,
) -> Result<ThreadSummaryDto, String> {
    let handles = state.read().await.handles();
    handles
        .assign_thread(&peer, &message_id, topic_name)
        .await
        .map_err(|e| e.to_string())
}

/// Archive a topic, or bring it back.
#[tauri::command]
pub async fn set_topic_closed(
    state: tauri::State<'_, Runtime>,
    peer: String,
    slug: String,
    closed: bool,
) -> Result<TopicDto, String> {
    let handles = state.read().await.handles();
    handles
        .set_topic_closed(&peer, &slug, closed)
        .await
        .map_err(|e| e.to_string())
}

/// Reply inside a thread — addressed to the thread's root, whichever message in
/// it the caller happens to name.
#[tauri::command]
pub async fn send_thread_reply(
    state: tauri::State<'_, Runtime>,
    peer: String,
    root_id: String,
    content: String,
) -> Result<MessageDto, String> {
    let handles = state.read().await.handles();
    handles
        .send_thread_reply(&peer, &root_id, &content)
        .await
        .map_err(|e| e.to_string())
}

/// Name a piece of work. `peer` of `None` is a note to self — no relay.
#[tauri::command]
pub async fn assign_task(
    state: tauri::State<'_, Runtime>,
    peer: Option<String>,
    text: String,
) -> Result<TaskDto, String> {
    let handles = state.read().await.handles();
    handles
        .assign_task(peer, &text)
        .await
        .map_err(|e| e.to_string())
}

/// Every task this device knows about, newest first.
#[tauri::command]
pub async fn tasks(state: tauri::State<'_, Runtime>) -> Result<Vec<TaskDto>, String> {
    state.read().await.tasks().map_err(|e| e.to_string())
}

/// Move a task to `state_name` and tell the other party.
#[tauri::command]
pub async fn set_task_state(
    state: tauri::State<'_, Runtime>,
    id: String,
    task_state: TaskState,
) -> Result<TaskDto, String> {
    let handles = state.read().await.handles();
    handles
        .set_task_state(&id, task_state)
        .await
        .map_err(|e| e.to_string())
}

/// Offer an in-app action to comrades. The outcome names who was told and why
/// the others were not — a bare count could not tell "the cooldown is running"
/// from "that person is not your comrade".
#[tauri::command]
pub async fn offer_action(
    state: tauri::State<'_, Runtime>,
    action: AppAction,
    peers: Vec<String>,
) -> Result<OfferOutcomeDto, String> {
    let handles = state.read().await.handles();
    handles
        .offer_action(action, peers)
        .await
        .map_err(|e| e.to_string())
}

/// Say something to Tara from inside a conversation — a private aside that never
/// reaches the peer.
#[tauri::command]
pub async fn tara_aside(
    state: tauri::State<'_, Runtime>,
    text: String,
) -> Result<TaraMessageDto, String> {
    state
        .read()
        .await
        .tara_aside(&text)
        .map_err(|e| e.to_string())
}

/// Ask Tara **in** the conversation — `@tara …`, which the peer sees, as against
/// `/tara`'s private aside above.
///
/// See `RuntimeHandles::tara_in_chat`: the answer is computed on this device, the
/// peer's messages are never handed to her, and a question that trips the
/// distress detector is answered without sending anything at all.
#[tauri::command]
pub async fn tara_in_chat(
    state: tauri::State<'_, Runtime>,
    peer: String,
    text: String,
) -> Result<TaraChatDto, String> {
    let handles = state.read().await.handles();
    handles
        .tara_in_chat(&peer, &text)
        .await
        .map_err(|e| e.to_string())
}

// ── Attention (usage mirror · focus sessions · long read) ──────────────────────
//
// Strictly local, like the journal and Tara. The web UI's Focus tab renders
// the focus-session and long-read halves (`desktop/ui/main.js`); the usage
// mirror is registered for parity with the Android bridge but has nothing to
// draw here, because the rollups come from Android's UsageStatsManager and the
// store is per-device — see `docs/ATTENTION.md` §7 and OQ14.
//
// `date`/`today` are `YYYY-MM-DD` in the frontend's timezone.

/// Record (or update) one day's usage rollup.
#[tauri::command]
pub async fn record_attention_day(
    state: tauri::State<'_, Runtime>,
    date: String,
    screen_minutes: u32,
    pickups: u32,
    doom_minutes: u32,
) -> Result<AttentionDayDto, String> {
    state
        .read()
        .await
        .record_attention_day(&date, screen_minutes, pickups, doom_minutes)
        .map_err(|e| e.to_string())
}

/// Every recorded usage day, newest first.
#[tauri::command]
pub async fn attention_days(
    state: tauri::State<'_, Runtime>,
) -> Result<Vec<AttentionDayDto>, String> {
    state
        .read()
        .await
        .attention_days()
        .map_err(|e| e.to_string())
}

/// Today's rollup against the user's own recent medians.
#[tauri::command]
pub async fn attention_summary(
    state: tauri::State<'_, Runtime>,
    today: String,
) -> Result<AttentionSummaryDto, String> {
    state
        .read()
        .await
        .attention_summary(&today)
        .map_err(|e| e.to_string())
}

/// The package names the user tagged as their own scroll traps.
#[tauri::command]
pub async fn doom_apps(state: tauri::State<'_, Runtime>) -> Result<Vec<String>, String> {
    state.read().await.doom_apps().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_doom_apps(
    state: tauri::State<'_, Runtime>,
    packages: Vec<String>,
) -> Result<Vec<String>, String> {
    state
        .read()
        .await
        .set_doom_apps(packages)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn start_focus_session(
    state: tauri::State<'_, Runtime>,
    intent: String,
    planned_minutes: u32,
) -> Result<FocusSessionDto, String> {
    state
        .read()
        .await
        .start_focus_session(&intent, planned_minutes)
        .map_err(|e| e.to_string())
}

/// Finish the running session; `None` if none was running.
#[tauri::command]
pub async fn finish_focus_session(
    state: tauri::State<'_, Runtime>,
    completed: bool,
) -> Result<Option<FocusSessionDto>, String> {
    state
        .read()
        .await
        .finish_focus_session(completed)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn active_focus_session(
    state: tauri::State<'_, Runtime>,
) -> Result<Option<FocusSessionDto>, String> {
    state
        .read()
        .await
        .active_focus_session()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn focus_sessions(
    state: tauri::State<'_, Runtime>,
) -> Result<Vec<FocusSessionDto>, String> {
    state
        .read()
        .await
        .focus_sessions()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn suggested_focus_minutes(state: tauri::State<'_, Runtime>) -> Result<u32, String> {
    state
        .read()
        .await
        .suggested_focus_minutes()
        .map_err(|e| e.to_string())
}

/// The session lengths to offer. Vault-free, so the Focus view can draw its
/// duration chips before unlock — see `ComradeRuntime::focus_presets`.
#[tauri::command]
pub async fn focus_presets(state: tauri::State<'_, Runtime>) -> Result<Vec<u32>, String> {
    Ok(state.read().await.focus_presets())
}

#[tauri::command]
pub async fn focus_prompt(state: tauri::State<'_, Runtime>) -> Result<String, String> {
    state.read().await.focus_prompt().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn focus_reflection(
    state: tauri::State<'_, Runtime>,
    outcome: String,
) -> Result<String, String> {
    state
        .read()
        .await
        .focus_reflection(&outcome)
        .map_err(|e| e.to_string())
}

/// The guided stretch break, in order. Vault-free like `focus_presets` — a
/// stretch must not need a passphrase.
#[tauri::command]
pub async fn stretch_routine(
    state: tauri::State<'_, Runtime>,
) -> Result<Vec<StretchStepDto>, String> {
    Ok(state.read().await.stretch_routine())
}

#[tauri::command]
pub async fn save_read(
    state: tauri::State<'_, Runtime>,
    title: String,
    text: String,
) -> Result<SavedReadDto, String> {
    state
        .read()
        .await
        .save_read(&title, &text)
        .map_err(|e| e.to_string())
}

/// The reading library, newest first — rows only, not the texts.
#[tauri::command]
pub async fn saved_reads(
    state: tauri::State<'_, Runtime>,
) -> Result<Vec<SavedReadSummaryDto>, String> {
    state.read().await.saved_reads().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_saved_read(
    state: tauri::State<'_, Runtime>,
    id: String,
) -> Result<Option<SavedReadDto>, String> {
    state
        .read()
        .await
        .open_saved_read(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_saved_read_position(
    state: tauri::State<'_, Runtime>,
    id: String,
    position: u32,
) -> Result<Option<SavedReadDto>, String> {
    state
        .read()
        .await
        .set_saved_read_position(&id, position)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_saved_read(
    state: tauri::State<'_, Runtime>,
    id: String,
) -> Result<bool, String> {
    state
        .read()
        .await
        .delete_saved_read(&id)
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
    state
        .write()
        .await
        .generate_identity()
        .map_err(|e| e.to_string())
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
    state
        .read()
        .await
        .extract_payments(&text)
        .map_err(|e| e.to_string())
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
