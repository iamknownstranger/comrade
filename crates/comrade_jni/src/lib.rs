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
use jni::sys::jstring;
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
/// spawned relay/feed loops survive across JNI calls. Returns `None` (rather
/// than panicking) if the runtime cannot be built.
fn runtime() -> Option<&'static Runtime> {
    static RT: OnceLock<Runtime> = OnceLock::new();
    if RT.get().is_none() {
        if let Ok(rt) = Builder::new_multi_thread().enable_all().build() {
            let _ = RT.set(rt);
        }
    }
    RT.get()
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
            let guard = state.read().await;
            let id = guard
                .broadcast_chitthi(&content, reply_to)
                .await
                .map_err(|e| e.to_string())?;
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

/// Non-blocking drain of the next bridge event (incoming Chitthi / DM). Returns
/// the event JSON, `{"empty":true}` when none is queued, or `{"error":"…"}`.
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
