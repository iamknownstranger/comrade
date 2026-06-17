# Comrade

A privacy-first, cross-platform social client built entirely in Rust, with a shared
view-model layer driving an Android (Kotlin/Compose), desktop (Tauri), or CLI frontend.

## What it does

| Engine | Protocol | Feature |
|--------|----------|---------|
| **Sabha** | Nostr Kind-1 + NIP-10 | Public microblogging — the **Chitthi Feed**, with nested `ChitthiThread` reply trees |
| **Vault** | Nostr Kind-4 + NIP-04 | End-to-end encrypted direct messages; `/pay` UPI intent detection |
| **Saathi** | libp2p mDNS + Gossipsub | Off-grid local mesh — works without internet |
| **Sakha/Sakhi** | Yrs CRDT + AES-256-GCM | Cryptographically isolated shared ledger for couples |
| **Relay gossip** | NIP-65 | Dynamic relay discovery + outbox-model routing |
| **Media** | NIP-94 / NIP-96 | Encrypted file staging + pluggable decentralized upload |
| **Storage** | sled + Argon2id + AES-256-GCM | Encrypted-at-rest persistence (identity, ChitthiCache, VaultCache, LedgerState) unlocked by a passphrase |

> **Nomenclature.** A public post is a **Chitthi** (Hindi for *letter*) throughout
> the application layer — `ChitthiNode`/`ChitthiThread`, `broadcast_chitthi`,
> `subscribe_chitthi_feed`, the `chitthi_cache`. Nostr protocol constants
> (`Kind::TextNote`, NIP-04) are kept intact at the wire level.

The **Progressive-Disclosure state machine** (`comrade_state`) gates which engines are active:

```
Base ──────── Sabha (public feed) + Vault (E2E DMs)
  └─ OffGridTravel ─── Saathi mesh replaces Nostr relays
  └─ CoupleSandbox ─── Sakha or Sakhi view of the shared ledger
```

## Repository layout

```
crates/
  comrade_state/   State machine (no I/O dependencies)
  comrade_core/    Protocol engines: crypto, sabha, vault, saathi, sakha, relay, media
  comrade_storage/ Encrypted-at-rest persistence (sled + Argon2id + AES-256-GCM)
  comrade_ui/      Framework-agnostic view-model / service layer (UiService + DTOs)
  comrade_jni/     JNI bridge — compiled to libcomrade_jni.so for Android
src/main.rs        Interactive CLI harness (development / testing)
android/           Kotlin + Jetpack Compose Android app
desktop/           Tauri 2 desktop shell (excluded from the workspace — see desktop/README.md)
.github/workflows/ CI (test + lint) and manual APK release
```

The UI logic lives once in **`comrade_ui`** and is reused by every frontend: the Android
app (via `comrade_jni`), the desktop app (via `#[tauri::command]` wrappers in `desktop/`),
and the CLI. This keeps the entire UI contract unit-testable without a display server.

## Building

### Prerequisites

- Rust stable (≥ 1.75) — [rustup.rs](https://rustup.rs)
- Android NDK r27c — via Android Studio or `sdkmanager "ndk;27.2.12479018"`
- `cargo-ndk` — `cargo install cargo-ndk --locked`
- JDK 17 and Gradle 8.5 (for the Android build)

### Run the CLI harness

```sh
cargo run
```

### Run all tests

```sh
cargo test --workspace
```

### Build the Android APK locally

```sh
# 1. Cross-compile the Rust JNI library for your target ABI
cargo ndk \
  --target aarch64-linux-android \
  --output-dir android/app/src/main/jniLibs \
  -- build --release -p comrade_jni

# 2. Build the APK (requires Gradle 8.5 on PATH)
cd android
gradle assembleRelease
# APK → android/app/build/outputs/apk/release/app-release.apk
```

### Build the desktop app (Tauri 2)

The desktop shell lives in `desktop/` and is **excluded from the Cargo workspace**
because it needs the Tauri CLI and system webview libraries. See
[`desktop/README.md`](desktop/README.md) for full prerequisites. In short:

```sh
cargo install tauri-cli --version "^2.0.0"
cd desktop/src-tauri
cargo tauri dev      # run; or `cargo tauri build` for a distributable bundle
```

## CI / Releases

| Workflow | Trigger | What it does |
|----------|---------|--------------|
| **CI** | Every push / PR | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace` |
| **Release APK** | Manual — Actions → "Release APK" → Run workflow | Builds `.so` libs for arm64-v8a / armeabi-v7a / x86_64, assembles APK, creates GitHub Release |

### Creating a release from the GitHub UI

1. Go to **Actions** → **Release APK**
2. Click **Run workflow**
3. Enter a version string (e.g. `1.0.0`) and optionally mark as pre-release
4. The workflow runs tests → cross-compiles Rust → assembles APK → creates a Release with the APK attached

### APK signing (optional)

Add these repository secrets (**Settings → Secrets → Actions**) for a production-signed build:

| Secret | Description |
|--------|-------------|
| `SIGNING_STORE_B64` | Base64-encoded `.jks` keystore |
| `SIGNING_KEY_ALIAS` | Key alias inside the keystore |
| `SIGNING_KEY_PASSWORD` | Key password |
| `SIGNING_STORE_PASSWORD` | Keystore password |

Without signing secrets the APK is signed with the Android debug key and can be sideloaded for testing.

## Architecture notes

- **Zero `.unwrap()`/`.expect()` in network or parsing paths** — all fallible I/O returns `Result<T, E>` with `thiserror`-derived domain errors.
- **Thread safety** — shared state uses `Arc<RwLock<T>>` or `Arc<Mutex<T>>` across async tasks.
- **DH key agreement** — secp256k1 ECDH with x-coordinate-only SHA-256 hashing for parity-independence; HKDF-SHA256 for label-scoped key derivation.
- **Off-line resilience** — Saathi caches up to 256 outbound messages and drains them automatically on mDNS peer discovery.
- **CRDT convergence** — Sakha/Sakhi use Yrs (Yjs port) so concurrent edits on either device merge deterministically; relay sees only AES-256-GCM ciphertext.
- **Encrypted-at-rest persistence** — every stored value is sealed with AES-256-GCM under an Argon2id-derived key (zeroized in memory, never written to disk); a wrong passphrase fails closed with `StorageError::InvalidPin`. On startup the CLI detects an existing store and prompts for the passphrase to restore the profile rather than minting a throwaway keypair. Durability is verified by `comrade_storage`'s `tests/durability.rs` reboot suite.