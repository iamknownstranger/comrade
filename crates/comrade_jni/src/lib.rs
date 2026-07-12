/*!
 * comrade_jni — Android JNI bridge
 *
 * Exposes Comrade core functions to the Android/Kotlin layer via JNI.
 * Each function maps to a `native` method in `ComradeCore.kt`.
 *
 * Naming convention: Java_<package_underscored>_<ClassName>_<methodName>
 * Package: mullu.comrade  →  mullu_comrade
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
pub extern "C" fn Java_mullu_comrade_ComradeCore_getVersion<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    to_jstring(&mut env, env!("CARGO_PKG_VERSION"))
}

// ── Key management ────────────────────────────────────────────────────────────

/// Serialised keypair payload. Uses serde_json (not hand-rolled `format!`) so
/// an error message containing quotes/backslashes still yields valid JSON that
/// the Kotlin `JSONObject` parser can read.
fn keypair_json() -> String {
    match KeyProfile::generate() {
        Ok(p) => json!({ "npub": p.npub, "nsec": p.nsec }).to_string(),
        Err(e) => json!({ "error": e.to_string() }).to_string(),
    }
}

/// Generate a new secp256k1 keypair.
///
/// Returns a JSON object:
/// ```json
/// {"npub":"npub1...","nsec":"nsec1..."}
/// ```
/// On error returns `{"error":"<message>"}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_generateKeypair<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    to_jstring(&mut env, &keypair_json())
}

/// Derive the npub from an nsec Bech32 string.
///
/// Returns the npub string on success, or `null` if the nsec is invalid.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_getNpubFromNsec<'local>(
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
pub extern "C" fn Java_mullu_comrade_ComradeCore_workspaceLabel<'local>(
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
pub extern "C" fn Java_mullu_comrade_ComradeCore_allWorkspaces<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    to_jstring(&mut env, &workspaces_json())
}

/// Serialised workspace list, escaping-safe for any future label text.
fn workspaces_json() -> String {
    let entries: Vec<Value> = AppWorkspace::all()
        .iter()
        .map(|ws| json!({ "key": ws.key(), "label": ws.label() }))
        .collect();
    Value::Array(entries).to_string()
}

// ── IPC bridge: vault, timeline, broadcast, workspace, events (M4) ────────────

/// Unlock the encrypted vault at `path` with `passphrase`, then start the
/// background relay/DM loops. Mirrors the desktop `unlock_comrade_vault` command.
///
/// Returns JSON `{"npub":"npub1…","has_secret":true}` on success or
/// `{"error":"…"}` on failure. Never panics across the boundary.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_unlockVault<'local>(
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
pub extern "C" fn Java_mullu_comrade_ComradeCore_broadcastChitthi<'local>(
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
pub extern "C" fn Java_mullu_comrade_ComradeCore_fetchSabhaTimeline<'local>(
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
pub extern "C" fn Java_mullu_comrade_ComradeCore_toggleWorkspace<'local>(
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
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let mut guard = state.write().await;
            let ws = guard
                .toggle_workspace(&target)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(ws).map_err(|e| e.to_string())
        })
    });
    to_jstring(&mut env, &out)
}

/// Snapshot of the off-grid mesh's live status: whether it is running, and how
/// many peers are currently reachable via mDNS. Returns
/// `{"active":bool,"peer_count":n}` — never `{"error":…}`, since there is no
/// failure mode (the mesh being off is a valid, common state).
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_meshStatus<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        serde_json::to_value(guard.mesh_status()).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

// ── Chat, profile & contacts (Telegram-like flow) ────────────────────────────

/// Send an E2E-encrypted DM to `target` (npub or hex). Persists to the offline
/// history. Returns the stored message JSON or `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_sendDm<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    target: JString<'local>,
    content: JString<'local>,
) -> jstring {
    let (Some(target), Some(content)) = (
        jni_string(&mut env, &target),
        jni_string(&mut env, &content),
    ) else {
        return to_jstring(&mut env, &json!({"error":"invalid arguments"}).to_string());
    };
    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let guard = state.read().await;
            let msg = guard
                .send_dm(&target, &content)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(msg).map_err(|e| e.to_string())
        })
    });
    to_jstring(&mut env, &out)
}

/// Claim a display @handle for this identity (persist locally, publish Kind-0
/// best-effort). Returns the profile JSON `{"npub":…,"username":…}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_setUsername<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
) -> jstring {
    let Some(name) = jni_string(&mut env, &name) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid name argument"}).to_string(),
        );
    };
    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let mut guard = state.write().await;
            let profile = guard.set_username(&name).await.map_err(|e| e.to_string())?;
            serde_json::to_value(profile).map_err(|e| e.to_string())
        })
    });
    to_jstring(&mut env, &out)
}

/// The local profile (npub + optional username), or `{"error":…}` pre-unlock.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_currentProfile<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let profile = guard.profile().map_err(|e| e.to_string())?;
        serde_json::to_value(profile).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// Best-effort people search by handle (NIP-50 relays). Returns a JSON array
/// of `{"npub","name","about"}`; empty when no search relay knew the name.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_searchProfiles<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    query: JString<'local>,
) -> jstring {
    let Some(query) = jni_string(&mut env, &query) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid query argument"}).to_string(),
        );
    };
    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let guard = state.read().await;
            let found = guard
                .search_profiles(&query)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(found).map_err(|e| e.to_string())
        })
    });
    to_jstring(&mut env, &out)
}

/// Save (or re-alias) a contact pinned by npub. Returns the contact JSON.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_addContact<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    npub: JString<'local>,
    alias: JString<'local>,
) -> jstring {
    let (Some(npub), Some(alias)) = (jni_string(&mut env, &npub), jni_string(&mut env, &alias))
    else {
        return to_jstring(&mut env, &json!({"error":"invalid arguments"}).to_string());
    };
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let contact = guard
            .add_contact(&npub, &alias)
            .map_err(|e| e.to_string())?;
        serde_json::to_value(contact).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// Set (non-empty) or clear (empty) the user-chosen alias for a contact.
/// Returns the contact JSON or `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_setContactAlias<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    npub: JString<'local>,
    alias: JString<'local>,
) -> jstring {
    let (Some(npub), Some(alias)) = (jni_string(&mut env, &npub), jni_string(&mut env, &alias))
    else {
        return to_jstring(&mut env, &json!({"error":"invalid arguments"}).to_string());
    };
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let contact = guard
            .set_contact_alias(&npub, &alias)
            .map_err(|e| e.to_string())?;
        serde_json::to_value(contact).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// Remove a saved contact (the message history stays). Returns
/// `{"removed":true|false}` or `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_removeContact<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    npub: JString<'local>,
) -> jstring {
    let Some(npub) = jni_string(&mut env, &npub) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid npub argument"}).to_string(),
        );
    };
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let removed = guard.remove_contact(&npub).map_err(|e| e.to_string())?;
        Ok(json!({ "removed": removed }))
    });
    to_jstring(&mut env, &out)
}

/// Refresh the cached Kind-0 profiles of conversation peers and contacts
/// (bounded, TTL-gated). Returns `{"changed":n}` — reload the chat list when
/// n > 0 — or `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_refreshPeerProfiles<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            // Detach the refresher under a briefly-held guard, then do the
            // slow relay work guard-free — holding the shared lock across
            // network awaits would stall every other bridge call (AUDIT P2).
            let refresher =
                { state.read().await.profile_refresher() }.map_err(|e| e.to_string())?;
            let changed = refresher.run().await.map_err(|e| e.to_string())?;
            Ok(json!({ "changed": changed }))
        })
    });
    to_jstring(&mut env, &out)
}

/// All saved contacts as a JSON array of `{"npub","alias","name"}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_listContacts<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let contacts = guard.list_contacts().map_err(|e| e.to_string())?;
        serde_json::to_value(contacts).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// The chat list (one entry per peer, newest first) as a JSON array.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_listConversations<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let convos = guard.conversations().map_err(|e| e.to_string())?;
        serde_json::to_value(convos).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// The offline message history with `peer`, oldest first, as a JSON array.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_messagesWith<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    peer: JString<'local>,
) -> jstring {
    let Some(peer) = jni_string(&mut env, &peer) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid peer argument"}).to_string(),
        );
    };
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let msgs = guard.messages_with(&peer).map_err(|e| e.to_string())?;
        serde_json::to_value(msgs).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// Full encrypted-media history with `peer`, oldest first, as a JSON array —
/// the media counterpart of [`Java_mullu_comrade_ComradeCore_messagesWith`],
/// for rendering past attachments inline after a restart.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_mediaWith<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    peer: JString<'local>,
) -> jstring {
    let Some(peer) = jni_string(&mut env, &peer) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid peer argument"}).to_string(),
        );
    };
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let media = guard.media_with(&peer).map_err(|e| e.to_string())?;
        serde_json::to_value(media).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

// ── Journal (strictly local, never networked) ─────────────────────────────────

/// Save a journal entry (`mood` may be empty for none). Returns the stored
/// entry JSON or `{"error":"…"}`. The entry never leaves the device.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_addJournalEntry<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    text: JString<'local>,
    mood: JString<'local>,
) -> jstring {
    let (Some(text), Some(mood)) = (jni_string(&mut env, &text), jni_string(&mut env, &mood))
    else {
        return to_jstring(&mut env, &json!({"error":"invalid arguments"}).to_string());
    };
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let mood = (!mood.trim().is_empty()).then_some(mood.as_str());
        let entry = guard
            .add_journal_entry(&text, mood)
            .map_err(|e| e.to_string())?;
        serde_json::to_value(entry).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// All journal entries, newest first, as a JSON array or `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_listJournal<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let entries = guard.journal_entries().map_err(|e| e.to_string())?;
        serde_json::to_value(entries).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// Delete a journal entry by id. Returns `{"removed":true|false}` or
/// `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_deleteJournalEntry<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: JString<'local>,
) -> jstring {
    let Some(id) = jni_string(&mut env, &id) else {
        return to_jstring(
            &mut env,
            &json!({"error":"invalid id argument"}).to_string(),
        );
    };
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let removed = guard.delete_journal_entry(&id).map_err(|e| e.to_string())?;
        Ok(json!({ "removed": removed }))
    });
    to_jstring(&mut env, &out)
}

/// Non-blocking drain of the next bridge event (incoming Chitthi / DM). Returns
/// the event JSON, `{"empty":true}` when none is queued, or `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_pollEvent<'local>(
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

// ── Replies, message requests & receipts (Session-parity messaging) ──────────

/// Send an E2E DM as a reply to a prior message. `replyTo` is the replied
/// message's event id (hex), or empty for a normal message.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_sendDmReply<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    target: JString<'local>,
    content: JString<'local>,
    reply_to: JString<'local>,
) -> jstring {
    let (Some(target), Some(content)) = (
        jni_string(&mut env, &target),
        jni_string(&mut env, &content),
    ) else {
        return to_jstring(&mut env, &json!({"error":"invalid arguments"}).to_string());
    };
    let reply_to = jni_string(&mut env, &reply_to).filter(|s| !s.is_empty());
    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let guard = state.read().await;
            let msg = guard
                .send_dm_reply(&target, &content, reply_to.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(msg).map_err(|e| e.to_string())
        })
    });
    to_jstring(&mut env, &out)
}

/// Pending message requests as a JSON array of `{"peer","last_message","last_at"}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_messageRequests<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let reqs = guard.message_requests().map_err(|e| e.to_string())?;
        serde_json::to_value(reqs).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// Accept a message request: move it into the chat list, share our handle with
/// the peer, and acknowledge their messages. Returns `{"accepted":true}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_acceptRequest<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    peer: JString<'local>,
) -> jstring {
    let Some(peer) = jni_string(&mut env, &peer) else {
        return to_jstring(&mut env, &json!({"error":"invalid peer"}).to_string());
    };
    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        // Run inside the runtime so the accept's background profile/receipt
        // sends have a reactor to spawn onto.
        rt.block_on(async move {
            let guard = state.read().await;
            guard.accept_request(&peer).map_err(|e| e.to_string())?;
            Ok(json!({ "accepted": true }))
        })
    });
    to_jstring(&mut env, &out)
}

/// Block a peer (hide + drop future DMs). Returns `{"blocked":true}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_blockConversation<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    peer: JString<'local>,
) -> jstring {
    let Some(peer) = jni_string(&mut env, &peer) else {
        return to_jstring(&mut env, &json!({"error":"invalid peer"}).to_string());
    };
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        guard.block_conversation(&peer).map_err(|e| e.to_string())?;
        Ok(json!({ "blocked": true }))
    });
    to_jstring(&mut env, &out)
}

/// Send a read receipt for a conversation (call when the thread is opened).
/// Returns `{"ok":true}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_markConversationRead<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    peer: JString<'local>,
) -> jstring {
    let Some(peer) = jni_string(&mut env, &peer) else {
        return to_jstring(&mut env, &json!({"error":"invalid peer"}).to_string());
    };
    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let guard = state.read().await;
            guard
                .mark_conversation_read(&peer)
                .map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        })
    });
    to_jstring(&mut env, &out)
}

// ── Encrypted media (send/receive on Android) ────────────────────────────────

/// Encrypt + upload media (base64 bytes) and deliver the NIP-94 reference to
/// `targetPubkey` over the DM channel. Returns the media message JSON.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_sendMediaBytes<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    target: JString<'local>,
    mime_type: JString<'local>,
    caption: JString<'local>,
    base64: JString<'local>,
) -> jstring {
    let (Some(target), Some(mime_type), Some(caption), Some(b64)) = (
        jni_string(&mut env, &target),
        jni_string(&mut env, &mime_type),
        jni_string(&mut env, &caption),
        jni_string(&mut env, &base64),
    ) else {
        return to_jstring(&mut env, &json!({"error":"invalid arguments"}).to_string());
    };
    let out = guard_json(move || {
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        let bytes = B64
            .decode(b64.as_bytes())
            .map_err(|e| format!("invalid base64: {e}"))?;
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let guard = state.read().await;
            let dto = guard
                .upload_and_send_media(&target, bytes, &mime_type, &caption)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(dto).map_err(|e| e.to_string())
        })
    });
    to_jstring(&mut env, &out)
}

/// Resolve a NIP-94 reference by event id and decrypt the blob. Returns
/// `{"mime_type","base64"}` or `{"error":"…"}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_downloadMedia<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    event_id: JString<'local>,
) -> jstring {
    let Some(event_id) = jni_string(&mut env, &event_id) else {
        return to_jstring(&mut env, &json!({"error":"invalid event id"}).to_string());
    };
    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let guard = state.read().await;
            let dto = guard
                .download_and_decrypt_media(&event_id)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(dto).map_err(|e| e.to_string())
        })
    });
    to_jstring(&mut env, &out)
}

// ── Calls (voice/video signaling) ────────────────────────────────────────────

/// The ICE servers for the WebRTC layer as a JSON array.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_callIceServers<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        serde_json::to_value(guard.call_ice_servers()).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// Configure (or clear, with an empty url) the TURN relay. Returns `{"ok":true}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_setTurnServer<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    url: JString<'local>,
    username: JString<'local>,
    credential: JString<'local>,
) -> jstring {
    let (Some(url), Some(username), Some(credential)) = (
        jni_string(&mut env, &url),
        jni_string(&mut env, &username),
        jni_string(&mut env, &credential),
    ) else {
        return to_jstring(&mut env, &json!({"error":"invalid arguments"}).to_string());
    };
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        guard
            .set_turn_server(&url, &username, &credential)
            .map_err(|e| e.to_string())?;
        Ok(json!({ "ok": true }))
    });
    to_jstring(&mut env, &out)
}

/// Begin a call to `peer` (`media` = "audio"/"video"). Returns the call session
/// JSON `{"call_id","peer","media","ice_servers":[…]}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_placeCall<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    peer: JString<'local>,
    media: JString<'local>,
) -> jstring {
    let (Some(peer), Some(media)) = (jni_string(&mut env, &peer), jni_string(&mut env, &media))
    else {
        return to_jstring(&mut env, &json!({"error":"invalid arguments"}).to_string());
    };
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let session = guard.place_call(&peer, &media).map_err(|e| e.to_string())?;
        serde_json::to_value(session).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// Send one call-signaling payload (`signalJson` = a CallSignal, e.g.
/// `{"kind":"offer","sdp":"…"}`) to `peer`. Returns `{"ok":true}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_sendCallSignal<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    peer: JString<'local>,
    call_id: JString<'local>,
    media: JString<'local>,
    signal_json: JString<'local>,
) -> jstring {
    let (Some(peer), Some(call_id), Some(media), Some(signal_json)) = (
        jni_string(&mut env, &peer),
        jni_string(&mut env, &call_id),
        jni_string(&mut env, &media),
        jni_string(&mut env, &signal_json),
    ) else {
        return to_jstring(&mut env, &json!({"error":"invalid arguments"}).to_string());
    };
    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let guard = state.read().await;
            guard
                .send_call_signal(&peer, &call_id, &media, &signal_json)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        })
    });
    to_jstring(&mut env, &out)
}

/// Send a call `Hangup` with `reason` and end negotiation. Returns `{"ok":true}`.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_hangupCall<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    peer: JString<'local>,
    call_id: JString<'local>,
    media: JString<'local>,
    reason: JString<'local>,
) -> jstring {
    let (Some(peer), Some(call_id), Some(media), Some(reason)) = (
        jni_string(&mut env, &peer),
        jni_string(&mut env, &call_id),
        jni_string(&mut env, &media),
        jni_string(&mut env, &reason),
    ) else {
        return to_jstring(&mut env, &json!({"error":"invalid arguments"}).to_string());
    };
    let out = guard_json(move || {
        let rt = runtime().ok_or_else(|| "failed to initialise async runtime".to_string())?;
        let state = state();
        rt.block_on(async move {
            let guard = state.read().await;
            guard
                .hangup_call(&peer, &call_id, &media, &reason)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        })
    });
    to_jstring(&mut env, &out)
}

/// Persist a finished call to the call log. Returns the call record JSON.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_logCall<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    peer: JString<'local>,
    call_id: JString<'local>,
    media: JString<'local>,
    incoming: jni::sys::jboolean,
    outcome: JString<'local>,
    started_at: jni::sys::jlong,
    duration_secs: jni::sys::jlong,
) -> jstring {
    let (Some(peer), Some(call_id), Some(media), Some(outcome)) = (
        jni_string(&mut env, &peer),
        jni_string(&mut env, &call_id),
        jni_string(&mut env, &media),
        jni_string(&mut env, &outcome),
    ) else {
        return to_jstring(&mut env, &json!({"error":"invalid arguments"}).to_string());
    };
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let rec = guard
            .log_call(
                &peer,
                &call_id,
                &media,
                incoming != 0,
                &outcome,
                started_at.max(0) as u64,
                duration_secs.max(0) as u64,
            )
            .map_err(|e| e.to_string())?;
        serde_json::to_value(rec).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

/// The call log as a JSON array. `peer` empty ⇒ all peers; else that peer only.
#[no_mangle]
pub extern "C" fn Java_mullu_comrade_ComradeCore_callHistory<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    peer: JString<'local>,
) -> jstring {
    let peer = jni_string(&mut env, &peer).filter(|s| !s.is_empty());
    let out = guard_json(move || {
        let state = state();
        let guard = state.blocking_read();
        let calls = guard
            .call_history(peer.as_deref())
            .map_err(|e| e.to_string())?;
        serde_json::to_value(calls).map_err(|e| e.to_string())
    });
    to_jstring(&mut env, &out)
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// The `extern "C"` wrappers need a live JVM to exercise, but the payloads they
// marshal are plain Rust — test those directly so a malformed JSON contract
// (the bug class serde_json replaced `format!` to fix) fails in `cargo test`
// rather than as a Kotlin JSONException on device.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_json_is_valid_and_typed() {
        let parsed: Value = serde_json::from_str(&keypair_json()).expect("valid JSON");
        let obj = parsed.as_object().expect("JSON object");
        assert!(
            obj["npub"].as_str().unwrap().starts_with("npub1"),
            "npub must be bech32: {parsed}"
        );
        assert!(obj["nsec"].as_str().unwrap().starts_with("nsec1"));
        assert!(!obj.contains_key("error"));
    }

    #[test]
    fn workspaces_json_lists_every_workspace_with_labels() {
        let parsed: Value = serde_json::from_str(&workspaces_json()).expect("valid JSON");
        let arr = parsed.as_array().expect("JSON array");
        assert_eq!(arr.len(), AppWorkspace::all().len());
        assert!(arr.iter().any(|w| w["key"] == "Base"));
        for ws in arr {
            assert!(!ws["key"].as_str().unwrap().is_empty());
            assert!(!ws["label"].as_str().unwrap().is_empty());
        }
    }

    #[test]
    fn guard_json_serialises_errors_with_special_characters() {
        // Regression for the hand-rolled `format!` JSON: an error containing
        // quotes must still round-trip as parseable JSON.
        let out = guard_json(|| Err(r#"boom: "quoted" \path"#.to_string()));
        let parsed: Value = serde_json::from_str(&out).expect("valid JSON despite quotes");
        assert_eq!(parsed["error"], r#"boom: "quoted" \path"#);
    }

    #[test]
    fn guard_json_captures_panics_at_the_boundary() {
        // The FFI safety net `unlock_vault`/`broadcastChitthi` rely on — this
        // is also why the release profile must NOT set `panic = "abort"`.
        let out = guard_json(|| panic!("do not cross the FFI boundary"));
        let parsed: Value = serde_json::from_str(&out).expect("valid JSON");
        assert!(parsed["error"].as_str().unwrap().contains("panic"));
    }
}
