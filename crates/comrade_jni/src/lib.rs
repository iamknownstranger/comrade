/*!
 * comrade_jni — Android JNI bridge
 *
 * Exposes Comrade core functions to the Android/Kotlin layer via JNI.
 * Each function maps to a `native` method in `ComradeCore.kt`.
 *
 * Naming convention: Java_<package_underscored>_<ClassName>_<methodName>
 * Package: global.auros.comrade  →  global_auros_comrade
 */

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;

use comrade_core::crypto::KeyProfile;
use comrade_state::AppWorkspace;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Convert a Rust `&str` to a Java `jstring`, returning null on failure.
fn to_jstring<'a>(env: &mut JNIEnv<'a>, s: &str) -> jstring {
    match env.new_string(s) {
        Ok(js) => js.into_raw(),
        Err(_) => std::ptr::null_mut(),
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
