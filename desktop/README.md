# Comrade Desktop (Tauri 2)

Cross-platform desktop shell for Comrade, built with **Tauri 2.0**: a native
Rust backend driving a lightweight HTML/Tailwind webview frontend.

## Architecture

```
desktop/
  ui/                 Static frontend (no build step, no CDN)
    index.html        SPA shell: vault door · base workspace · modality overlays
    styles.css        Dark-mode-first theme; modality re-skins via <body> class
    main.js           IPC (window.__TAURI__) + live event stream + toasts
  src-tauri/          Rust backend
    src/lib.rs        Builder, event-forwarder setup hook, command registry
    src/commands.rs   async #[tauri::command] wrappers over comrade_ui::ComradeRuntime
    src/main.rs       Entry point
    tauri.conf.json   Window + bundle configuration
```

## Frontend (Progressive Disclosure SPA)

`ui/` is a dependency-free vanilla-JS single-page app implementing the three-tier
workspace model:

1. **Vault Door** — passphrase lock screen → `invoke('unlock_comrade_vault')`.
   Any passphrase forges/loads the encrypted vault and seeds an identity.
2. **Base Workspace** — a fixed left sidebar (brand + workspace badge,
   section navigation, mode controls, network status, identity chip) beside
   the content area; it folds into a compact top bar below ~840 px. Sections:
   **Sabha** (compose + broadcast Chitthis, live feed) and **Vault** (contacts
   + encrypted chat populated by the live DM stream, with on-device UPI `/pay`
   detection and a 📎 **encrypted media** attachment).
3. **Modality overlays** — a **Travel / Off-Grid** switch and a **Partner
   Portal** button in the sidebar's Modes group (cryptographic pairing →
   Couple Sandbox, which also offers encrypted media sharing) that re-theme
   the whole app by swapping a `<body>` class.

### Encrypted media pipeline (NIP-94/96 · Blossom)

Attaching a photo/audio reads the file (≤ 10 MB, enforced on both sides),
encrypts it on-device with AES-256-GCM under a key derived from the **ECDH
shared secret** (`derive_media_key`), uploads only the opaque ciphertext to a
Blossom server (`send_media_bytes` → `BlossomUploader`), and delivers a
zero-knowledge NIP-94 reference privately over the E2E DM channel. The public
event and the host blob carry **no key** — the recipient re-derives it from
their own private key. Incoming media (`incoming_media` events) render a
**Download & view** control that calls `download_and_decrypt_media(event_id)`,
rebuilds a `Blob`, and shows the `<img>`/`<audio>` inline. The real HTTP path is
behind the `media-http` cargo feature (enabled for this desktop build).

Real-time updates arrive over the single `comrade://event` window channel
(internally-tagged `incoming_chitthi` / `incoming_direct_message` /
`incoming_media`) and are
prepended to the UI without a refresh. Every `invoke` is funnelled through a
`safeInvoke` wrapper that renders backend errors as toasts. Running the page
outside Tauri activates a built-in mock backend for design preview.

All application logic lives in the **`comrade_ui`** crate in the main workspace
(`crates/comrade_ui`), which is fully unit-tested. This `src-tauri` crate is a
thin IPC marshalling layer, so the same logic also backs the Android (JNI) build
and any future native (Slint/Iced) frontend.

## Why it's outside the Cargo workspace

Tauri requires the Tauri CLI plus **system webview libraries** that are not in
the headless CI image:

- **Linux**: `libwebkit2gtk-4.1-dev`, `libsoup-3.0-dev`, `build-essential`, etc.
- **macOS**: Xcode command-line tools (WKWebView is built in)
- **Windows**: WebView2 runtime (preinstalled on Windows 11)

To keep `cargo test --workspace` and CI lean, `desktop/src-tauri` is listed under
`exclude` in the root `Cargo.toml`. Build it explicitly from this directory.

## Building locally

1. Install prerequisites (Linux example):

   ```sh
   sudo apt install libwebkit2gtk-4.1-dev libsoup-3.0-dev \
       build-essential curl wget file libssl-dev libayatana-appindicator3-dev librsvg2-dev
   cargo install tauri-cli --version "^2.0.0"
   ```

2. Generate app icons (one-time):

   ```sh
   cargo tauri icon path/to/logo.png   # run from desktop/src-tauri
   ```

3. Run in development:

   ```sh
   cd desktop/src-tauri
   cargo tauri dev
   ```

4. Build a distributable bundle (`.deb`/`.AppImage`/`.dmg`/`.msi`):

   ```sh
   cargo tauri build
   ```

## Mobile

Tauri 2 can also target Android/iOS from this same crate (`cargo tauri android
init` / `cargo tauri ios init`). The existing `android/` directory uses a direct
JNI bridge (`comrade_jni`); the Tauri mobile path is an alternative that reuses
`comrade_ui` unchanged.

## Status

The SPA frontend and the async `#[tauri::command]` layer are complete, and the
underlying `comrade_ui::ComradeRuntime` is unit-tested in CI. The Tauri build
itself was **not** compiled in the development sandbox (no webview system
libraries available), so verify `cargo tauri dev` on a machine with the
prerequisites above before release.

Known frontend gaps (await backend commands, not yet exposed): outbound *text*
DM send (`send_dm`), persistent contact list, and server-side QR-pairing
validation. The Vault tab therefore renders live *incoming* DMs and validates
pairing payloads client-side before committing the workspace transition.
(Encrypted *media* send is implemented — it rides the existing NIP-04 channel.)

The desktop crate's webview build needs system GTK/webkit libs absent from the
CI sandbox, so the media commands compile-check via the workspace crates
(`comrade_core`/`comrade_ui` with `--features media-http`) but the end-to-end
Blossom upload was not exercised here — verify with `cargo tauri dev` against a
live Blossom server before release.
