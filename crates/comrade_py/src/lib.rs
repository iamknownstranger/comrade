/*!
 * comrade_py — Python bindings (PyO3) for Comrade's core engines
 *
 * Exposes the two production-ready engines (see the root README's feature
 * table) to external Python tooling as plain functions/dicts so a script can
 * `pip install` the built wheel and use it for local data analysis or custom
 * integrations, with no Android/desktop UI involved:
 *
 *   • Sabha (public feed)  — `fetch_sabha_timeline` (fetch), `broadcast_chitthi` (publish)
 *   • Vault (E2E DMs)      — `messages_with`/`conversations` (fetch), `send_dm` (publish)
 *
 * Design mirrors `comrade_jni`: a single [`ComradeRuntime`] behind a
 * `tokio::sync::RwLock`, driven by an owned multi-thread Tokio runtime per
 * `ComradeClient` instance. Unlike the JNI bridge (which serialises everything
 * to JSON strings across the FFI boundary), DTOs are converted straight to
 * native Python objects with `pythonize`, and failures raise a catchable
 * `comrade_py.ComradeError` instead of an `{"error": …}` envelope — the shape
 * a Python caller actually expects.
 *
 * The blocking Tokio/network work in every method runs inside
 * `Python::detach` so the GIL is released for the duration, keeping a
 * multi-threaded Python caller (or async framework driving this from a
 * worker thread) responsive.
 */

use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::{create_exception, wrap_pyfunction};
use serde::Serialize;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::RwLock;

use comrade_core::crypto::KeyProfile;
use comrade_ui::{ComradeRuntime, UiError};

create_exception!(
    comrade_py,
    ComradeError,
    PyException,
    "An error raised by the Comrade core engines (crypto, relay, storage)."
);

/// Map a core [`UiError`] to the Python-visible [`ComradeError`].
fn to_py_err(e: UiError) -> PyErr {
    ComradeError::new_err(e.to_string())
}

/// Convert any serialisable DTO into a native Python object (dict/list/etc.)
/// via `pythonize`, so callers get plain Python data instead of opaque
/// wrapper classes.
fn to_py_obj<T: Serialize>(py: Python<'_>, value: &T) -> PyResult<Py<PyAny>> {
    pythonize::pythonize(py, value)
        .map(Bound::unbind)
        .map_err(|e| ComradeError::new_err(format!("failed to convert result to Python: {e}")))
}

/// A freshly generated secp256k1 identity, never persisted anywhere.
#[derive(Serialize)]
struct KeypairOut {
    npub: String,
    nsec: String,
}

/// Generate a brand-new Comrade identity (secp256k1 keypair), entirely
/// on-device. Returns `{"npub": "npub1…", "nsec": "nsec1…"}`.
///
/// The `nsec` is the private key — treat it like a password. It never
/// leaves this process; nothing here transmits it anywhere.
#[pyfunction]
fn generate_keypair(py: Python<'_>) -> PyResult<Py<PyAny>> {
    let profile = KeyProfile::generate().map_err(|e| ComradeError::new_err(e.to_string()))?;
    to_py_obj(
        py,
        &KeypairOut {
            npub: profile.npub,
            nsec: profile.nsec,
        },
    )
}

/// Derive the public `npub` from an existing `nsec` Bech32 secret key.
#[pyfunction]
fn npub_from_nsec(nsec: &str) -> PyResult<String> {
    KeyProfile::from_nsec(nsec)
        .map(|p| p.npub)
        .map_err(|e| ComradeError::new_err(e.to_string()))
}

/// A local Comrade node: one identity, its encrypted offline store, and the
/// Sabha/Vault relay engines once [`ComradeClient::unlock_vault`] has run.
///
/// Thread safety: methods take `&self` and serialise access to the
/// underlying engines through an async `RwLock`, so one `ComradeClient` may
/// safely be driven from multiple Python threads.
#[pyclass(module = "comrade_py")]
struct ComradeClient {
    state: RwLock<ComradeRuntime>,
    rt: Runtime,
}

#[pymethods]
impl ComradeClient {
    /// Create a client with no identity loaded yet — call `unlock_vault`
    /// before any fetch/publish method.
    #[new]
    fn new() -> PyResult<Self> {
        let rt = Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| ComradeError::new_err(format!("failed to start async runtime: {e}")))?;
        Ok(Self {
            state: RwLock::new(ComradeRuntime::new()),
            rt,
        })
    }

    /// Open (or create) the encrypted store at `path` with `passphrase`,
    /// restore or seed the identity, build the Sabha + Vault engines, and
    /// connect their background relay loops. Required before any other
    /// method. Idempotent — calling it again on an already-unlocked client
    /// just returns the loaded identity.
    ///
    /// Returns `{"npub": "npub1…", "has_secret": true}`.
    fn unlock_vault(
        &self,
        py: Python<'_>,
        path: String,
        passphrase: String,
    ) -> PyResult<Py<PyAny>> {
        let identity = py.detach(|| {
            self.rt.block_on(async {
                let mut guard = self.state.write().await;
                let identity = guard.unlock_vault(&path, &passphrase).await?;
                guard.spawn_event_loops();
                Ok::<_, UiError>(identity)
            })
        });
        to_py_obj(py, &identity.map_err(to_py_err)?)
    }

    /// Whether `unlock_vault` has completed and the engines are live.
    #[getter]
    fn is_unlocked(&self, py: Python<'_>) -> bool {
        py.detach(|| self.state.blocking_read().is_vault_unlocked())
    }

    /// The local profile: `{"npub": "npub1…", "username": "handle-or-None"}`.
    fn profile(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let profile = py.detach(|| self.state.blocking_read().profile());
        to_py_obj(py, &profile.map_err(to_py_err)?)
    }

    // ── Sabha: public feed (fetch + publish) ─────────────────────────────

    /// The Sabha (public feed) timeline from the encrypted offline cache, as
    /// a list of `{"id", "author", "content", "created_at", "reply_to"}`.
    fn fetch_sabha_timeline(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let feed = py.detach(|| self.state.blocking_read().fetch_sabha_timeline());
        to_py_obj(py, &feed.map_err(to_py_err)?)
    }

    /// Broadcast a Chitthi (public post) to the relay set, optionally as a
    /// reply to the Chitthi with event id `reply_to`. Returns the new event
    /// id (hex).
    #[pyo3(signature = (content, reply_to=None))]
    fn broadcast_chitthi(
        &self,
        py: Python<'_>,
        content: String,
        reply_to: Option<String>,
    ) -> PyResult<String> {
        py.detach(|| {
            self.rt.block_on(async {
                let guard = self.state.read().await;
                guard.broadcast_chitthi(&content, reply_to).await
            })
        })
        .map_err(to_py_err)
    }

    // ── Vault: end-to-end encrypted DMs (fetch + publish) ────────────────

    /// Send an end-to-end encrypted DM to `target` (npub or hex pubkey),
    /// persisting it to the offline history. Returns the stored message as
    /// `{"id", "peer", "content", "created_at", "outgoing", "status", "reply_to"}`.
    fn send_dm(&self, py: Python<'_>, target: String, content: String) -> PyResult<Py<PyAny>> {
        let msg = py.detach(|| {
            self.rt.block_on(async {
                let guard = self.state.read().await;
                guard.send_dm(&target, &content).await
            })
        });
        to_py_obj(py, &msg.map_err(to_py_err)?)
    }

    /// The chat list — one entry per accepted peer, newest thread first — as
    /// a list of `{"peer", "alias", "peer_name", "last_message", "last_at", "last_outgoing"}`.
    fn conversations(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let list = py.detach(|| self.state.blocking_read().conversations());
        to_py_obj(py, &list.map_err(to_py_err)?)
    }

    /// The full offline message history with `peer` (npub or hex), oldest
    /// first, as a list of message dicts (see [`Self::send_dm`] for shape).
    fn messages_with(&self, py: Python<'_>, peer: String) -> PyResult<Py<PyAny>> {
        let list = py.detach(|| self.state.blocking_read().messages_with(&peer));
        to_py_obj(py, &list.map_err(to_py_err)?)
    }
}

#[pymodule]
fn comrade_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("ComradeError", m.py().get_type::<ComradeError>())?;
    m.add_class::<ComradeClient>()?;
    m.add_function(wrap_pyfunction!(generate_keypair, m)?)?;
    m.add_function(wrap_pyfunction!(npub_from_nsec, m)?)?;
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────
//
// Exercising the `#[pyfunction]`/`#[pymethods]` wrappers themselves needs a
// real Python process to import the built module (a `Python::attach` unit
// test would only prove pyo3's own embedding works, not that the compiled
// wheel loads) — that happens in CI's dedicated maturin build + smoke-test
// job. So — exactly like `comrade_jni`'s tests — these cover the plain-Rust
// logic behind the boundary instead.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_keypair_round_trips_through_npub_from_nsec() {
        let profile = KeyProfile::generate().expect("keygen");
        assert!(profile.npub.starts_with("npub1"));
        assert!(profile.nsec.starts_with("nsec1"));
        let derived = npub_from_nsec(&profile.nsec).expect("valid nsec");
        assert_eq!(derived, profile.npub);
    }

    #[test]
    fn npub_from_nsec_rejects_garbage() {
        assert!(npub_from_nsec("not-a-valid-nsec").is_err());
    }

    #[test]
    fn keypair_out_serialises_both_fields() {
        let out = KeypairOut {
            npub: "npub1x".into(),
            nsec: "nsec1y".into(),
        };
        let json = serde_json::to_value(&out).expect("serialisable");
        assert_eq!(json["npub"], "npub1x");
        assert_eq!(json["nsec"], "nsec1y");
    }
}
