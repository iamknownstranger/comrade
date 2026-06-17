# Comrade Desktop (Tauri 2)

Cross-platform desktop shell for Comrade, built with **Tauri 2.0**: a native
Rust backend driving a lightweight HTML/Tailwind webview frontend.

## Architecture

```
desktop/
  ui/                 Static frontend (no build step — Tailwind via CDN)
    index.html
    main.js           Calls the backend via window.__TAURI__ IPC
  src-tauri/          Rust backend
    src/lib.rs        #[tauri::command] wrappers over comrade_ui::UiService
    src/main.rs       Entry point
    tauri.conf.json   Window + bundle configuration
```

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

The frontend and `#[tauri::command]` layer are complete and the underlying
`comrade_ui` service is unit-tested in CI. The Tauri build itself was **not**
compiled in the development sandbox (no webview system libraries available), so
verify `cargo tauri dev` on a machine with the prerequisites above before
release.
