# App icons

Tauri's bundler expects icons referenced in `tauri.conf.json`
(`32x32.png`, `128x128.png`, `icon.png`, `icon.ico`).

Generate them from a single source image with the Tauri CLI:

```sh
cargo tauri icon path/to/logo.png
```

This populates this directory automatically. The placeholder is intentionally
empty in version control so a real logo can be dropped in at packaging time.
