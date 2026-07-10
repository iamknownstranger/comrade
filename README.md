# Comrade

A privacy-first, cross-platform social client built entirely in Rust, with a shared
view-model layer driving an Android (Kotlin/Compose), desktop (Tauri), or CLI frontend.

## What it does

| Engine | Protocol | Feature | Status |
|--------|----------|---------|--------|
| **Sabha** | Nostr Kind-1 + NIP-10 | Public microblogging — the **Chitthi Feed**, with nested `ChitthiThread` reply trees | ✅ Wired (desktop: broadcast + live feed; reply threading in live feed pending) |
| **Vault** | Nostr Kind-4 + NIP-04 | End-to-end encrypted direct messages; `/pay` UPI intent detection | ⚠️ Receive wired (desktop); sending not yet exposed in any UI; NIP-44 migration planned |
| **Saathi** | libp2p mDNS + Gossipsub | Off-grid local mesh — works without internet | 🧪 Experimental — engine + tests only, not started by any frontend |
| **Sakha/Sakhi** | Yrs CRDT + AES-256-GCM | Cryptographically isolated shared ledger for couples | 🧪 Engine built; pairing handshake not yet reachable from any UI |
| **Relay gossip** | NIP-65 | Dynamic relay discovery + outbox-model routing | 🧪 Experimental — routing library + CLI demo only |
| **Media** | NIP-94 / NIP-96 | Encrypted file staging + pluggable decentralized upload | 🧪 Experimental — library + CLI demo only |
| **Storage** | sled + Argon2id + AES-256-GCM | Encrypted-at-rest persistence (identity, ChitthiCache, VaultCache, LedgerState) unlocked by a passphrase | ✅ Wired (identity + own posts; incoming-message persistence planned) |
| **Voice** | Vosk (offline) + Android TTS | "Hey Comrade" wake word, tap-to-talk, and assist-app role — all on-device, no cloud | ⚠️ Recognition/dispatch work; `post`/`read timeline` need a vault-unlock screen the Android UI doesn't have yet |

> **Status honesty.** 🧪 rows are working, unit-tested library code that no
> frontend invokes yet — they describe the architecture's direction, not
> shipped behavior. The full gap analysis lives in [`AUDIT.md`](AUDIT.md).

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

The bridge logic lives once in **`comrade_ui`** and backs the Android app (via
`comrade_jni`) and the desktop app (via `#[tauri::command]` wrappers in `desktop/`).
This keeps the bridged UI contract unit-testable without a display server.
The CLI harness currently drives the core crates directly rather than going
through `comrade_ui`; unifying it is tracked in `AUDIT.md`.

## Building

### Prerequisites

- Rust stable — the committed `Cargo.lock` currently requires **rustc ≥ 1.88** — [rustup.rs](https://rustup.rs)
- Android NDK r27c — via Android Studio or `sdkmanager "ndk;27.2.12479018"`
- `cargo-ndk` — `cargo install cargo-ndk --locked`
- JDK 17 (for the Android build; Gradle comes from the committed wrapper)

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

# 2. Build the APK (the committed Gradle wrapper fetches Gradle 8.5)
cd android
./gradlew assembleRelease
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
| **CI** | Every push | Rust lane (`cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace --locked`) · Desktop lane (`clippy` on `desktop/src-tauri`) · Android lane (`./gradlew test`) · Supply-chain lane (`cargo-deny`: advisories, bans, sources, licenses) |
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

## Voice — "Hey Comrade" (Android)

The Android app can respond to a spoken **"Hey Comrade"** wake word, take a
tap-to-talk command, and register as the device's assist app. Everything runs
**on-device** with the offline [Vosk](https://alphacephei.com/vosk/) recogniser
and Android's built-in text-to-speech — no audio ever leaves the phone, keeping
with Comrade's privacy-first design.

| Layer | What it is | Entry point |
|-------|-----------|-------------|
| **Wake word** | A foreground `Service` keeps the mic open, listens for "Hey Comrade", then routes the following utterance as a command. Shows a persistent notification; costs battery. | `voice/WakeWordService` |
| **Tap-to-talk** | One-shot capture from a mic button in the app UI, no always-on service. | `voice/OneShotRecognizer` + `MainActivity` |
| **Assist app** | Register Comrade as the default digital assistant so the assist gesture (long-press power) opens it. | `voice/ComradeInteractionService` |

Recognised commands (see `voice/VoiceCommand`): **post** _&lt;message&gt;_ ·
**read my timeline** · **switch to** _base / off-grid / sakha / sakhi_ ·
**new identity** · **help**. Parsing and command dispatch are Android-free and
unit-tested (`VoiceCommandTest`, `CommandDispatcherTest`).

> **Current limitation.** `post` and `read my timeline` require an unlocked
> vault, and the Android UI does not yet expose an unlock screen — those two
> commands currently answer with a "vault is locked" error. The unlock flow is
> tracked as task M2-1 in [`AUDIT.md`](AUDIT.md).

> **Honest scope.** This is an *app-scoped* wake word, not the OS-level
> "Hey Google" hotword. Stock (non-rooted) Pixels reserve the always-on,
> screen-off DSP hotword pipeline for Google's own keyphrases — a third-party
> app cannot inject a custom phrase there. So "Hey Comrade" works only while the
> foreground service is running (persistent notification, mic open), and the
> assist-app role responds to the assist *gesture*, not a spoken phrase.

### Voice setup

The Vosk model (~40 MB) is **not** committed. Fetch it before building an APK
with voice support:

```sh
./scripts/fetch-vosk-model.sh   # → android/app/src/main/assets/model-en-us/
```

Without the model the app still builds and runs; voice features report
"Voice model missing" and stay inert. On first use the app requests the
`RECORD_AUDIO` (and, on Android 13+, `POST_NOTIFICATIONS`) runtime permissions.

## Architecture notes

- **Zero `.unwrap()`/`.expect()` in network or parsing paths** — all fallible I/O returns `Result<T, E>` with `thiserror`-derived domain errors.
- **Thread safety** — shared state uses `Arc<RwLock<T>>` or `Arc<Mutex<T>>` across async tasks.
- **DH key agreement** — secp256k1 ECDH with x-coordinate-only SHA-256 hashing for parity-independence; HKDF-SHA256 for label-scoped key derivation.
- **Off-line resilience** — Saathi caches up to 256 outbound messages and drains them automatically on mDNS peer discovery. (Engine-level behavior; Saathi is not yet started by any frontend.)
- **CRDT convergence** — Sakha/Sakhi use Yrs (Yjs port) so concurrent edits on either device merge deterministically; relay sees only AES-256-GCM ciphertext.
- **Encrypted-at-rest persistence** — every stored value is sealed with AES-256-GCM under an Argon2id-derived key (zeroized in memory, never written to disk); a wrong passphrase fails closed with `StorageError::InvalidPin`. On startup the CLI detects an existing store and prompts for the passphrase to restore the profile rather than minting a throwaway keypair. Durability is verified by `comrade_storage`'s `tests/durability.rs` reboot suite.