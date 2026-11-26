# MediaStat v0.3

High-speed disk space analyzer inspired by **WinDirStat (`master` / 2.10)** with **Advanced Search** and **SysAdmin tools**, written **100% in Rust** (native performance, memory-safe, GPU-accelerated).

## Workspace Architecture

MediaStat is organized as a Cargo workspace with five crates:

- `crates/mediastat-core` — Arena-backed scan tree, file categories, filtering, formatting/palettes, i18n, and settings.
- `crates/mediastat-scanner` — High-speed filesystem walker, NTFS `$MFT` reader, hardlink deduplication, duplicate hashing, DACL permissions, and live file watcher.
- `crates/mediastat-storage` — SQLite WAL `.mstat` snapshots, snapshot diffing, exports, and self-contained HTML reports.
- `crates/mediastat-cli` — Headless CLI for file servers and automation.
- `crates/mediastat-gui` — GPU-accelerated desktop application (`egui`/`eframe` + `wgpu`).

## Build & Test

```powershell
# Build release binaries (with LTO)
cargo build --release --workspace

# Run strict Clippy checks and workspace tests
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License

MIT
