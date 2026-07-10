# App icons

Tauri references these icons in `tauri.conf.json` (`32x32.png`, `128x128.png`,
`icon.png`, `icon.ico`). They are needed at **compile time** — the
`tauri::generate_context!()` macro embeds the window icon, so the crate does
not build without them (not just the bundler).

The committed files are deliberate **placeholders** (indigo squares) so the
shell compiles and CI can lint it. Replace them with the real logo via the
Tauri CLI, which regenerates every size from one source image:

```sh
cargo tauri icon path/to/logo.png
```
