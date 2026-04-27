# Little Britain Mod Tool - Mod Workspace Notes

This archive contains the Rust desktop modding tool. It does **not** include the C++ runtime modloader DLL source or a ready-made example mod package.

## What is included

- Rust/egui desktop tool for browsing Little Britain game assets.
- GEO, SCN, DDS, BNK, BIK, ANM inspection paths.
- GPU-backed GEO/SCN viewport via `eframe`/`egui-wgpu`/WGPU.
- A Lua mod workspace under the selected game folder's `Mods/` directory.
- Basic mod creation and script editing through `src/mod_workspace.rs`.

## External dependencies expected by the project

The project expects a local FFmpeg distribution at:

```text
third_party/ffmpeg-8.1-dist/
```

The committed `.cargo/config.toml` points `FFMPEG_DIR` and `PKG_CONFIG_PATH` at that folder. If `pkgconf`/`pkg-config` is not on your `PATH`, set `PKG_CONFIG` in your shell or in a user-level Cargo config rather than hard-coding a machine-specific path in the project.

## Runtime modloader status

The GUI scans the game folder for these runtime loader files when present:

```text
modloader.dll
binkw32.dll
binkw32_real.dll
```

Those files are **not built by this archive**. If you add a C++/DLL modloader later, document its source folder, build command, and install flow here.

## Mod structure used by the GUI

```text
Mods/
└── YourMod/
    ├── mod.json
    └── scripts/
        └── main.lua
```

The current manifest reader/writer is intentionally simple and is intended for manifests produced by the tool. If the runtime loader will consume user-edited manifests directly, switch the manifest code to `serde`/`serde_json` and use typed fields such as a boolean `enabled`.

## Build

```batch
cargo run
```

On Windows, keep your FFmpeg bundle in `third_party/ffmpeg-8.1-dist` and ensure `pkgconf.exe` or `pkg-config` is available on `PATH`, or set `PKG_CONFIG` externally.
