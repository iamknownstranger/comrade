# Comrade

A privacy-first companion for your (mental) wellbeing — journal your
thoughts (typed or spoken, on-device), share what you choose anonymously,
and stay end-to-end-encrypted-close to the people who hold you up. Built
entirely in Rust, with a shared view-model layer driving an Android
(Kotlin/Compose), desktop (Tauri), or CLI frontend.

> **Vision status.** The wellbeing pillars (journal, anonymous thoughts,
> loved-one space, reflective companion) are the product's north star — the
> gap map and build order live in [`AUDIT.md` §8](AUDIT.md). What is shipped
> and working today is the table below; no pillar gets a checkmark before
> it's wired end-to-end.

## What it does

| Engine | Protocol | Feature | Status |
|--------|----------|---------|--------|
| **Sabha** | Nostr Kind-1 + NIP-10 | Public microblogging — the **Chitthi Feed**, with nested `ChitthiThread` reply trees | ✅ Wired (desktop + Android: broadcast + live feed; reply threading in live feed pending) |
| **Vault** | NIP-44 + NIP-17/NIP-59 (gift-wrapped); legacy NIP-04 read-only | End-to-end encrypted direct messages; replies (NIP-10 `e` tag), delivered/read receipts, `/pay` UPI intent detection | ✅ Send + receive wired (desktop + Android), offline chat history persisted. New DMs are NIP-44-encrypted and gift-wrapped (no sender/content metadata leaks to relays); a peer's older NIP-04 DMs still decrypt |
| **Message requests** | Conversation gate + Kind-4 profile share | A stranger's DM lands in a *requests* bucket, not your chat list; **your @handle is shared only when you accept**; accept / block; blocked keys are dropped | ✅ Engine + bridges tested; UI wired (desktop + Android) |
| **Profiles** | Nostr Kind-0 + NIP-50 | @username display handles: published with retry **and republished on every launch**, searched on dedicated NIP-50 relays; peers' handles cached locally so chats are titled by name; per-contact aliases | ✅ Wired (Android onboarding + settings + chat UI; desktop backend commands) |
| **Saathi** | libp2p mDNS + Gossipsub | Off-grid local mesh — works without internet | 🧪 Experimental — engine + tests only, not started by any frontend |
| **Sakha/Sakhi** | Yrs CRDT + AES-256-GCM | Cryptographically isolated shared ledger for couples | 🧪 Engine built; pairing handshake not yet reachable from any UI |
| **Relay gossip** | NIP-65 | Dynamic relay discovery + outbox-model routing | 🧪 Experimental — routing library + CLI demo only |
| **Media** | NIP-94 / NIP-96 | Encrypted file staging + pluggable decentralized upload | ⚠️ Send + download-and-decrypt over Blossom (`media-http` feature) — desktop **and Android** (attach button); rich in-thread rendering of received media is Android follow-up; not on CLI |
| **Calls** | WebRTC + signaling over the Vault DM channel (NIP-100 direction) | 1:1 **voice & video** — SDP/ICE ride encrypted DMs; STUN-only first attempt, automatic fallback to a configurable TURN relay if STUN can't connect (e.g. strict CGNAT); call log | ⚠️ 1:1 voice & video wired on BOTH desktop (webview WebRTC) and Android native (`org.webrtc` via `io.github.webrtc-sdk`): ringing/lock-screen call UI, foreground service, Bluetooth/wired audio routing, mid-call camera toggle + tap-to-swap self-view, encryption-emoji (SAS) verification, call history + missed-call notifications, connection-quality indicator. STUN-only first attempt with caller-driven TURN fallback (ICE restart) — wired on Android; desktop does not yet ICE-restart on failure — see [`AUDIT.md` §8.1](AUDIT.md). Calls need the app process alive (no push wakeup, by design) |
| **Storage** | redb + Argon2id + AES-256-GCM | Encrypted-at-rest persistence (identity, ChitthiCache, VaultCache, LedgerState) unlocked by a passphrase | ✅ Wired (identity, own posts, full DM history — incoming + outgoing — with offline backfill watermark, media refs, call log) |
| **Voice** | Vosk (offline) + Android TTS | "Hey Comrade" wake word, tap-to-talk, and assist-app role — all on-device, no cloud | ⚠️ Recognition/dispatch work; `post`/`read timeline` need a vault-unlock screen the Android UI doesn't have yet |

> **Status honesty.** 🧪 rows are working, unit-tested library code that no
> frontend invokes yet — they describe the architecture's direction, not
> shipped behavior. The full gap analysis lives in [`AUDIT.md`](AUDIT.md).

> **Nomenclature.** A public post is a **Chitthi** (Hindi for *letter*) throughout
> the application layer — `ChitthiNode`/`ChitthiThread`, `broadcast_chitthi`,
> `subscribe_chitthi_feed`, the `chitthi_cache`. Nostr protocol constants
> (`Kind::TextNote`, NIP-44) are kept intact at the wire level.

## Identity & usernames

Comrade is serverless, so it deliberately does **not** have Telegram-style
globally unique usernames — no central registry exists that could enforce
them, and a first-come claim on public relays would be squattable and
unreliable. The model instead:

- **Identity = the keypair.** Every account is a secp256k1 keypair created
  on-device; the public half (`npub…`) is the address peers actually message.
  It is better than a UUID for device-to-device interaction: globally unique
  *and* unforgeable — using it requires the private key.
- **Username = a display alias.** At first launch the app asks for a
  `@handle`, stores it with the identity, and publishes it as Kind-0 metadata
  so people can find you by name (NIP-50 relay search, best-effort).
  Publication retries with backoff and is **re-published on every launch** —
  a claim made offline (or during the onboarding connect race) heals itself
  the next time a relay is reachable.
- **Discovery is honest about its limits.** Search queries go to dedicated
  NIP-50 relays (`relay.nostr.band`, `search.nos.today`) and results are
  filtered against the query client-side; new profiles can take a little
  while to be indexed by the search relays. Pasting an `npub` key looks that
  identity up directly. The reliable path is always swapping keys.
- **Contacts pin the key, not the name (trust-on-first-use).** Once you add
  `@abc_user`, the contact is stored under their npub. If `@abc_user`
  disconnects and a stranger later claims the same handle, that stranger is a
  *different npub*: they show up as a separate, unverified entry, and they can
  never read or receive the encrypted messages bound to the original key.
- **Chats are titled by name, in trust order.** A conversation shows (1) the
  alias *you* set for that contact, else (2) the `@handle` *they* published
  (fetched and cached locally, refreshed in the background), else (3) the
  shortened key. The open-conversation header always shows the npub tail —
  handles are claims, keys are identity. Aliases are edited from the pencil
  icon in the conversation header; an empty alias falls back to the handle.
- The opt-in path to *verified* unique names (NIP-05 DNS mapping) is future
  work.

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
  comrade_storage/ Encrypted-at-rest persistence (redb + Argon2id + AES-256-GCM)
  comrade_ui/      Framework-agnostic view-model / service layer (UiService + DTOs)
  comrade_jni/     JNI bridge — compiled to libcomrade_jni.so for Android
  comrade_py/      PyO3 bridge — Python bindings (pip/maturin) for local scripting
src/main.rs        Interactive CLI harness (development / testing)
android/           Kotlin + Jetpack Compose app — Telegram-like shell:
                   onboarding (@username + passcode → encrypted vault),
                   Chats (E2E DMs), Feed (public Chitthis), Settings
desktop/           Tauri 2 desktop shell (excluded from the workspace — see desktop/README.md)
.github/workflows/ CI (test + lint) and manual APK release
```

The bridge logic lives once in **`comrade_ui`** and backs the Android app (via
`comrade_jni`), the desktop app (via `#[tauri::command]` wrappers in `desktop/`),
and external Python scripts (via `comrade_py`, covering the Sabha/Vault subset —
see [`crates/comrade_py/README.md`](crates/comrade_py/README.md)).
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

### Startup performance

App startup is dominated by loading `libcomrade_jni.so` (the entire statically
linked Rust core). Three things keep it fast — please don't regress them:

- **Release profile** — the root `Cargo.toml` sets thin LTO, one codegen unit,
  and debuginfo stripping for `[profile.release]`, which cuts the shipped `.so`
  by ~20%. `panic = "abort"` must stay **off**: uniffi's generated scaffolding
  catches panics at the FFI boundary, which needs unwinding.
- **Off-main-thread load** — `ComradeApplication` warms the library on a
  background thread at process start, and `ComradeApp` resolves core facts on
  `Dispatchers.IO` inside a `LaunchedEffect`, so the first Compose frame never
  blocks on the native call. `MainActivityUiTest` guards this on the CI device
  lanes.
- **mmap-from-APK packaging** — `useLegacyPackaging = false` stores the `.so`
  uncompressed so the linker maps it straight from the APK.

Observability: logcat shows `ComradeApplication: comrade_jni v… warmed in N ms`
and the framework's `Fully drawn` line (via `reportFullyDrawn()`); the Rust
side traces `vault unlocked: store opened and engines built` with `kdf_ms` /
`total_ms` fields on every unlock.

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

Step 2 also builds `comrade_jni` for your *host* (a separate, fast, non-NDK
build — not the cross-compiled `.so` from step 1) and runs it through
`comrade_uniffi_bindgen` to generate the Kotlin bindings, so `cargo` needs to
be on `PATH` even though step 1 already ran. This happens automatically as a
Gradle task dependency (`android/app/build.gradle.kts`); there's no separate
bindgen command to run by hand, and nothing to keep in sync as
`crates/comrade_jni`'s interface grows.

### Build the desktop app (Tauri 2)

The desktop shell lives in `desktop/` and is **excluded from the Cargo workspace**
because it needs the Tauri CLI and system webview libraries. See
[`desktop/README.md`](desktop/README.md) for full prerequisites. In short:

```sh
cargo install tauri-cli --version "^2.0.0"
cd desktop/src-tauri
cargo tauri dev      # run; or `cargo tauri build` for a distributable bundle
```

### Build the Python bindings

`crates/comrade_py` exposes the Sabha (public feed) and Vault (E2E DM) engines
to Python — fetch/publish only, see [its README](crates/comrade_py/README.md)
for the full scope and a usage example.

```sh
cd crates/comrade_py
python -m venv .venv && source .venv/bin/activate
pip install maturin
maturin develop            # build + install into the active venv
# or: maturin build --release   # wheel → target/wheels/
```

## CI / Releases

| Workflow | Trigger | What it does |
|----------|---------|--------------|
| **CI** | Every push | Rust lane (`cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace --locked`) · Desktop lane (`clippy` on `desktop/src-tauri`) · Android lane (`./gradlew test`) · Python lane (builds the `comrade_py` wheel with maturin and smoke-tests importing it) · Supply-chain lane (`cargo-deny`: advisories, bans, sources, licenses) |
| **Android APK** | Push touching `android/`, `crates/`, `Cargo.*` · manual | Cross-compiles `libcomrade_jni.so` (arm64-v8a for handsets, x86_64 for emulators), assembles a sideloadable debug APK artifact, and runs the on-device smoke suite (`connectedDebugAndroidTest`) on two emulator lanes — Pixel 9 and Pixel 9 Pro XL (Android 15 / API 35) |
| **Release APK** | Manual — Actions → "Release APK" → Run workflow (also reusable via `workflow_call`) | Builds `.so` libs for arm64-v8a / armeabi-v7a / x86_64, assembles APK + AAB, creates GitHub Release |
| **Auto Release** | Every push to `main` (each merged PR) | Bumps the patch version `X.Y.Z → X.Y.(Z+1)` from the newest `v*` tag (seeds at `0.0.2`), then calls **Release APK** to build + publish. Put `[skip release]` in the merge commit to skip |

### On-device APK testing

The **Android APK** workflow installs the built APK on KVM-accelerated Android
emulators and runs `android/app/src/androidTest/…/DeviceSmokeTest.kt`: the JNI
library loads for the device ABI, keypair generation round-trips through real
Rust crypto, workspaces come back labelled, and `MainActivity` reaches
`RESUMED`. Two lanes model the target hardware:

| Lane | AVD profile | Android |
|------|-------------|---------|
| Google Pixel 9 | `pixel_9` (falls back to `pixel_7` on older SDK tools) | 15 (API 35) |
| Google Pixel 9 Pro XL | `pixel_9_pro_xl` (falls back to `pixel_7_pro`) | 15 (API 35) |

Honest limits: GitHub-hosted runners have no physical phones — real-hardware
runs need a device farm such as Firebase Test Lab (paid, service-account
secrets). For manual testing, download the `comrade-debug-apk` artifact from
any run and sideload it (real handsets are arm64-v8a, which the APK
includes). **iOS is out of scope**: an APK is an Android package and cannot
run on an iPhone, and Comrade currently has no iOS frontend — the non-Android
frontends are the Tauri desktop shell and the CLI.

Debug artifacts are signed with the keystore committed at
`android/debug.keystore` (the standard, deliberately-public Android debug
credentials), so every CI run and every local `assembleDebug` share one
signature — a newer sideloaded artifact installs directly over an older one
and keeps app data instead of tripping Android's "App not installed"
signature-conflict error. **One-time caveat**: any APK you installed *before*
this change carries an old, throwaway per-runner signature and must be
uninstalled once to make room for a debug-keystore-signed build; today that
uninstall discards its local app data, since there is no identity
export/import yet. This committed debug key is public by design and cannot
be used to sign anything for Play — see [`RELEASING.md`](RELEASING.md) for
production signing.

### Creating a release from the GitHub UI

1. Go to **Actions** → **Release APK**
2. Click **Run workflow**
3. Enter a version string (e.g. `1.0.0`) and optionally mark as pre-release
4. The workflow runs tests → cross-compiles Rust → assembles APK → creates a Release with the APK attached

> **Shipping to real users?** See [`RELEASING.md`](RELEASING.md) for the
> keystore setup, the Play-ready App Bundle, distribution options
> (Play / F-Droid / Obtainium), and an honest account of what Play Protect
> will still say about sideloaded builds.

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

> **Note.** `post` and `read my timeline` require an unlocked vault. The app's
> onboarding flow (username + passcode) unlocks it at startup, so these work
> once you're past the door; before unlocking they answer with a "vault is
> locked" error.

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