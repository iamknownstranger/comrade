/*!
 * comrade_jni — Android JNI bridge
 *
 * Exposes Comrade core functions to the Android/Kotlin layer via JNI.
 * Each function maps to a `native` method in `ComradeCore.kt`.
 *
 * Naming convention: Java_<package_underscored>_<ClassName>_<methodName>
 * Package: global.auros.comrade  →  global_auros_comrade
 */

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, OnceLock};

use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint, jstring};
use jni::JNIEnv;

use serde_json::{json, Value};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::{broadcast, RwLock};

use comrade_core::crypto::KeyProfile;
use comrade_state::AppWorkspace;
use comrade_ui::{BridgeEvent, ComradeRuntime};

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Convert a Rust `&str` to a Java `jstring`, returning null on failure.
fn to_jstring<'a>(env: &mut JNIEnv<'a>, s: &str) -> jstring {
    match env.new_string(s) {
        Ok(js) => js.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Read a `JString` argument into an owned `String`, or `None` if marshalling
/// fails (e.g. the Java side passed null or invalid UTF-16).
fn jni_string(env: &mut JNIEnv, s: &JString) -> Option<String> {
    env.get_string(s).ok().map(|js| js.into())
}

// ── Async runtime & shared state (M4) ─────────────────────────────────────────

/// Process-global multi-thread Tokio runtime. Owned in a `OnceLock` so the
/// spawned relay/feed loops survive across JNI calls. `get_or_init` makes the
/// initialisation race-free (the old check-then-set could build two runtimes
/// and silently drop one); a build failure panics inside `guard_json`, which
/// converts it to an error JSON instead of crossing the FFI boundary.
fn runtime() -> Option<&'static Runtime> {
    static RT: OnceLock<Runtime> = OnceLock::new();
    Some(RT.get_or_init(|| {
        Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build Tokio runtime")
    }))
}

/// Process-global handle to the live [`ComradeRuntime`], mirroring the desktop's
/// Tauri managed state. Shared behind `Arc<RwLock<…>>` exactly as the bridge's
/// Send/Sync contract requires.
fn state() -> Arc<RwLock<ComradeRuntime>> {
    static STATE: OnceLock<Arc<RwLock<ComradeRuntime>>> = OnceLock::new();
    STATE
        .get_or_init(|| Arc::new(RwLock::new(ComradeRuntime::new())))
        .clone()
}

/// Process-global receiver draining the bridge event bus for `pollEvent`.
/// Created lazily (synchronously — never inside the async runtime).
fn event_rx() -> &'static Mutex<broadcast::Receiver<BridgeEvent>> {
    static RX: OnceLock<Mutex<broadcast::Receiver<BridgeEvent>>> = OnceLock::new();
    RX.get_or_init(|| Mutex::new(state().blocking_read().subscribe_events()))
}

/// Run a fallible JSON-producing body, catching every error *and* any unwinding
/// panic so nothing crosses the `extern "C"` boundary (UB) — the Architecture
/// Quality Gate. The result is always a serialised JSON string.
fn guard_json<F>(f: F) -> String
where
    F: FnOnce() -> Result<Value, String>,
{
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(v)) => v.to_string(),
        Ok(Err(e)) => json!({ "error": e }).to_string(),
        Err(_) => json!({ "error": "internal panic captured at JNI boundary" }).to_string(),
    }
}

// ── Version ───────────────────────────────────────────────────────────────────

/// Returns the comrade_jni crate version string (e.g. "0.1.0").
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_getVersion<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    to_jstring(&mut env, env!("CARGO_PKG_VERSION"))
}

// ── Key management ────────────────────────────────────────────────────────────

/// Generate a new secp256k1 keypair.
///
/// Returns a JSON object:
/// ```json
/// {"npub":"npub1...","nsec":"nsec1..."}
/// ```
/// On error returns `{"error":"<message>"}`.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_generateKeypair<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let json = match KeyProfile::generate() {
        Ok(p) => format!(r#"{{"npub":"{}","nsec":"{}"}}"#, p.npub, p.nsec),
        Err(e) => format!(r#"{{"error":"{}"}}"#, e),
    };
    to_jstring(&mut env, &json)
}

/// Derive the npub from an nsec Bech32 string.
///
/// Returns the npub string on success, or `null` if the nsec is invalid.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_getNpubFromNsec<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    nsec: JString<'local>,
) -> jstring {
    let nsec_str: String = match env.get_string(&nsec) {
        Ok(s) => s.into(),
        Err(_) => return std::ptr::null_mut(),
    };
    match KeyProfile::from_nsec(&nsec_str) {
        Ok(p) => to_jstring(&mut env, &p.npub),
        Err(_) => std::ptr::null_mut(),
    }
}

// ── Workspace state machine ───────────────────────────────────────────────────

/// Returns the label for a workspace discriminant string.
///
/// `workspace` should be one of: "Base", "OffGridTravel", "CoupleSandboxSakha",
/// "CoupleSandboxSakhi".  Returns `null` for unknown values.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_workspaceLabel<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    workspace: JString<'local>,
) -> jstring {
    let ws_str: String = match env.get_string(&workspace) {
        Ok(s) => s.into(),
        Err(_) => return std::ptr::null_mut(),
    };
    let Some(ws) = AppWorkspace::from_key(&ws_str) else {
        return std::ptr::null_mut();
    };
    to_jstring(&mut env, ws.label())
}

/// Returns a JSON array of all workspace discriminants and their labels.
///
/// ```json
/// [
///   {"key":"Base","label":"Base — Sabha (Public Feed) + Vault (E2E DMs)"},
///   ...
/// ]
/// ```
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_allWorkspaces<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let entries: Vec<String> = AppWorkspace::all()
        .iter()
        .map(|ws| format!(r#"{{"key":"{}","label":"{}"}}"#, ws.key(), ws.label()))
        .collect();
    let json = format!("[{}]", entries.join(","));
    to_jstring(&mut env, &json)
}

// ── IPC bridge: vault, timeline, broadcast, workspace, events (M4) ────────────

/// Unlock the encrypted vault at `path` with `passphrase`, then start the
/// background relay/DM loops. Mirrors the desktop `unlock_comrade_vault` command.
///
/// Returns JSON `{"npub":"npub1…","has_secret":true}` on success or
/// `{"error":"…"}` on failure. Never panics across the boundary.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_unlockVault<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    path: JString<'local>,
    passphrase: JString<'local>,
) -> jstring {
    let Some(path) = jni_string(&mut env, &path) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid path argument"}).to_string(),
        );
    };
    let Some(passphrase) = jni_string(&mut env, &passphrase) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid passphrase argument"}).to_string(),
        );
    };

    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let mut guard = state.write().await;
            let id = guard
                .unlock_vault(&path, &passphrase)
                .await
                .map_err(|e| e.to_string())?;
            guard.spawn_event_loops();
            Ok(json!({ "npub": id.npub, "has_secret": id.has_secret }))
        })
    });
    // Arm the event receiver NOW: the loops just spawned, and a lazily
    // created receiver (first pollEvent) would silently drop every event
    // emitted before that first poll.
    let _ = event_rx();
    to_jstring(&mut env, &out)
}

/// Broadcast a Chitthi (optionally a reply). `replyTo` may be null/empty for a
/// top-level post. Returns `{"event_id":"…"}` or `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_broadcastChitthi<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    content: JString<'local>,
    reply_to: JString<'local>,
) -> jstring {
    let Some(content) = jni_string(&mut env, &content) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid content argument"}).to_string(),
        );
    };
    // A null/empty reply_to means a top-level Chitthi.
    let reply_to = jni_string(&mut env, &reply_to).filter(|s| !s.is_empty());

    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            // Hold the shared runtime lock only while BUILDING the task —
            // holding it across the multi-second relay send would stall every
            // other native call behind the fair RwLock.
            let task = {
                let guard = state.read().await;
                guard
                    .broadcast_chitthi_task(content, reply_to)
                    .map_err(|e| e.to_string())?
            };
            let id = task.await.map_err(|e| e.to_string())?;
            Ok(json!({ "event_id": id }))
        })
    });
    to_jstring(&mut env, &out)
}

/// Load the Sabha timeline from the encrypted offline cache. Returns a JSON
/// array of Chitthis or `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_fetchSabhaTimeline<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let out = guard_json(move || {
        let state = state();
        // A blocking read is safe here: this JNI call is synchronous and not
        // executing inside the async runtime.
        let guard = state.blocking_read();
        let feed = guard.fetch_sabha_timeline().map_err(|e| e.to_string())?;
        serde_json::to_value(feed).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// Toggle the active workspace, enforcing the transition state machine.
/// Returns the new workspace JSON or a typed `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_toggleWorkspace<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    target: JString<'local>,
) -> jstring {
    let Some(target) = jni_string(&mut env, &target) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid target argument"}).to_string(),
        );
    };

    let out = guard_json(move || {
        let state = state();
        let mut guard = state.blocking_write();
        let ws = guard.toggle_workspace(&target).map_err(|e| e.to_string())?;
        serde_json::to_value(ws).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

// ── Pukar: audio/video calls ──────────────────────────────────────────────────

/// Start an audio/video call. `peer` is an npub or hex pubkey; `sdpOffer`
/// comes from the platform WebRTC stack. Returns the ringing session JSON
/// (with `call_id`) or `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_placeCall<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    peer: JString<'local>,
    video: jboolean,
    sdp_offer: JString<'local>,
) -> jstring {
    let Some(peer) = jni_string(&mut env, &peer) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid peer argument"}).to_string(),
        );
    };
    let Some(sdp_offer) = jni_string(&mut env, &sdp_offer) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid sdp argument"}).to_string(),
        );
    };
    let video = video != 0;

    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let guard = state.read().await;
            let session = guard
                .place_call(&peer, video, &sdp_offer)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(session).map_err(|e| e.to_string())
        })
    });
    to_jstring(&mut env, &out)
}

/// Accept the ringing incoming call with the platform's SDP answer.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_answerCall<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    call_id: JString<'local>,
    sdp_answer: JString<'local>,
) -> jstring {
    let (Some(call_id), Some(sdp_answer)) = (
        jni_string(&mut env, &call_id),
        jni_string(&mut env, &sdp_answer),
    ) else {
        return to_jstring(&mut env, &json!({"error":"invalid argument"}).to_string());
    };
    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let guard = state.read().await;
            guard
                .answer_call(&call_id, &sdp_answer)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        })
    });
    to_jstring(&mut env, &out)
}

/// Decline the ringing incoming call.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_declineCall<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    call_id: JString<'local>,
) -> jstring {
    let Some(call_id) = jni_string(&mut env, &call_id) else {
        return to_jstring(&mut env, &json!({"error":"invalid argument"}).to_string());
    };
    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let task = {
                let guard = state.read().await;
                guard
                    .decline_call_task(&call_id)
                    .map_err(|e| e.to_string())?
            };
            task.await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        })
    });
    to_jstring(&mut env, &out)
}

/// Hang up the active call (or cancel an outgoing ring).
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_endCall<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    call_id: JString<'local>,
) -> jstring {
    let Some(call_id) = jni_string(&mut env, &call_id) else {
        return to_jstring(&mut env, &json!({"error":"invalid argument"}).to_string());
    };
    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let task = {
                let guard = state.read().await;
                guard.end_call_task(&call_id).map_err(|e| e.to_string())?
            };
            task.await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        })
    });
    to_jstring(&mut env, &out)
}

/// Forward a locally-gathered ICE candidate. `sdpMlineIndex < 0` means unset;
/// an empty `sdpMid` means unset.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_sendCallIce<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    call_id: JString<'local>,
    candidate: JString<'local>,
    sdp_mid: JString<'local>,
    sdp_mline_index: jint,
) -> jstring {
    let (Some(call_id), Some(candidate)) = (
        jni_string(&mut env, &call_id),
        jni_string(&mut env, &candidate),
    ) else {
        return to_jstring(&mut env, &json!({"error":"invalid argument"}).to_string());
    };
    let sdp_mid = jni_string(&mut env, &sdp_mid).filter(|s| !s.is_empty());
    let mline = u32::try_from(sdp_mline_index).ok();

    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let task = {
                let guard = state.read().await;
                guard
                    .send_call_ice_task(&call_id, candidate, sdp_mid, mline)
                    .map_err(|e| e.to_string())?
            };
            task.await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        })
    });
    to_jstring(&mut env, &out)
}

/// Platform WebRTC reports the media path is up. Returns the active session.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_callConnected<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    call_id: JString<'local>,
) -> jstring {
    let Some(call_id) = jni_string(&mut env, &call_id) else {
        return to_jstring(&mut env, &json!({"error":"invalid argument"}).to_string());
    };
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let session = guard.call_connected(&call_id).map_err(|e| e.to_string())?;
        serde_json::to_value(session).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// The live call as JSON, or `{"none":true}` when idle.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_activeCall<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        match guard.active_call().map_err(|e| e.to_string())? {
            Some(session) => serde_json::to_value(session).map_err(|e| e.to_string()),
            None => Ok(json!({ "none": true })),
        }
    });
    to_jstring(&mut env, &out)
}

/// Ended calls (newest first) as a JSON array.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_fetchCallLog<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let log = guard.call_log().map_err(|e| e.to_string())?;
        serde_json::to_value(log).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

// ── Companion: private, anonymous journal ─────────────────────────────────────

/// Sentinel passed from Kotlin for "no mood recorded" (see `ComradeCore.kt`).
const NO_MOOD: jint = jint::MIN;

/// Write an anonymous journal entry (typed or voice-dictated) into the encrypted
/// store. `mode` is one of "journal"/"vent"/"brainstorm"/"reflect"; `voice` marks
/// a dictated entry; `mood` uses [`NO_MOOD`] to mean unset. Returns the stored
/// entry plus an offline safety assessment and next prompt, or `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_journalEntry<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    mode: JString<'local>,
    voice: jboolean,
    body: JString<'local>,
    mood: jint,
) -> jstring {
    let Some(mode) = jni_string(&mut env, &mode) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid mode argument"}).to_string(),
        );
    };
    let Some(body) = jni_string(&mut env, &body) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid body argument"}).to_string(),
        );
    };
    let mood_opt: Option<i8> = if mood == NO_MOOD {
        None
    } else {
        Some(mood.clamp(-2, 2) as i8)
    };
    let is_voice = voice != 0;

    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let response = guard
            .write_journal_entry(&mode, is_voice, &body, mood_opt)
            .map_err(|e| e.to_string())?;
        serde_json::to_value(response).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// Load the private journal (newest first). Returns a JSON array of entries or
/// `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_fetchJournal<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let entries = guard.list_journal_entries().map_err(|e| e.to_string())?;
        serde_json::to_value(entries).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// On-device journaling insights (streak, momentum, mood trend, top tags).
/// `tz_offset_secs` is the device's UTC offset (Kotlin:
/// `TimeZone.getDefault().getOffset(now) / 1000`) so streaks roll at the
/// user's midnight rather than UTC's.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_journalInsights<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    tz_offset_secs: jint,
) -> jstring {
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let insights = guard
            .journal_insights(tz_offset_secs)
            .map_err(|e| e.to_string())?;
        serde_json::to_value(insights).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// A supportive companion prompt for `mode`. Returns `{"prompt":"…"}` or an error.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_companionPrompt<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    mode: JString<'local>,
) -> jstring {
    let Some(mode) = jni_string(&mut env, &mode) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid mode argument"}).to_string(),
        );
    };
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let prompt = guard.companion_prompt(&mode).map_err(|e| e.to_string())?;
        Ok(json!({ "prompt": prompt }))
    });
    to_jstring(&mut env, &out)
}

/// Non-blocking drain of the next bridge event (incoming Chitthi / DM / call).
/// Returns the event JSON, `{"empty":true}` when none is queued,
/// `{"lagged":n}` when the consumer fell `n` events behind (recover missed
/// data from the encrypted caches: timeline, DM history, call log),
/// `{"closed":true}` when the bus is gone, or `{"error":"…"}`.
///
/// Prefer [`pollEvents`](Java_global_auros_comrade_ComradeCore_pollEvents):
/// one event per JNI crossing cannot keep up with the public feed.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_pollEvent<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let out = guard_json(move || {
        let mut rx = event_rx()
            .lock()
            .map_err(|_| "event receiver lock poisoned".to_string())?;
        match rx.try_recv() {
            Ok(ev) => serde_json::to_value(ev).map_err(|e| e.to_string()),
            Err(broadcast::error::TryRecvError::Empty) => Ok(json!({ "empty": true })),
            Err(broadcast::error::TryRecvError::Lagged(n)) => Ok(json!({ "lagged": n })),
            Err(broadcast::error::TryRecvError::Closed) => Ok(json!({ "closed": true })),
        }
    });
    to_jstring(&mut env, &out)
}

/// Non-blocking batch drain of up to `max` bridge events in one JNI crossing.
///
/// Returns `{"events":[…], "lagged":n}` — `events` may be empty, and `lagged`
/// (present only when > 0) is how many events were evicted because the
/// consumer fell behind; recover missed data from the encrypted caches
/// (timeline, DM history, call log). `{"closed":true}` when the bus is gone.
#[no_mangle]
pub extern "C" fn Java_global_auros_comrade_ComradeCore_pollEvents<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    max: jint,
) -> jstring {
    let out = guard_json(move || {
        let max = max.clamp(1, 512) as usize;
        let mut rx = event_rx()
            .lock()
            .map_err(|_| "event receiver lock poisoned".to_string())?;
        let mut events = Vec::new();
        let mut lagged: u64 = 0;
        while events.len() < max {
            match rx.try_recv() {
                Ok(ev) => events.push(serde_json::to_value(ev).map_err(|e| e.to_string())?),
                // Lagged: count the eviction and keep draining what's left.
                Err(broadcast::error::TryRecvError::Lagged(n)) => lagged += n,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => {
                    if events.is_empty() {
                        return Ok(json!({ "closed": true }));
                    }
                    break;
                }
            }
        }
        let mut out = json!({ "events": events });
        if lagged > 0 {
            out["lagged"] = json!(lagged);
        }
        Ok(out)
    });
    to_jstring(&mut env, &out)
}
