# MediaStat v0.3

High-speed disk space analyzer inspired by **WinDirStat (`master` / 2.10)** with **Advanced Search** and **SysAdmin tools**, written **100% in Rust** (native performance, memory-safe, GPU-accelerated).

---

## Key Features

### Core Engine, Palettes & Internationalization (`mediastat-core`)
- **Arena-Backed Scan Tree (`ScanTree`)**: Compact, flat-indexed node arena with string interning (`StringArena`), extension tables, owner tables, hardlink groups, duplicate groups, and volume space tracking.
- **9 File Categories & Rich Filtering**: Categorizes files across *Images*, *Videos*, *Audio*, *Documents*, *CAD / BIM / 3D*, *Archives / Disk Images*, *Executables / Code*, *Databases / Mail*, and *Other*, with compiled filters for size, age, owner, regex/glob, and Windows attributes.
- **Exact WinDirStat Color Space & Palettes**:
  - **Official 18-color WinDirStat palette** (`WINDIRSTAT_PALETTE_18`) normalized via `make_bright_color(c, 0.6)` and assigned by descending byte rank per extension.
  - **3 Color Modes**: *By Extension (WinDirStat)*, *By Category*, and *File Age (Cold Data)*.
  - **4 Color Palettes**: *Official WinDirStat (18 Colors)*, *MediaStat Semantic (CAD/Media)*, *Colorblind Safe (Okabe-Ito)*, and *SysAdmin Thermal (High Contrast)*.
  - **WCAG 2.1 Contrast** (`luminance > 0.179`) and 8-color directory depth/branch palettes (`FileTreeColor0..7`, `CFolderColors`).
- **Live Bilingual UI (`ES` / `EN`)**: Lock-free `AtomicU8` language switcher persisted in `%APPDATA%\MediaStat\settings.json`, allowing instant switching between English and Spanish without restarting.

### Native Scanning Engine (`mediastat-scanner`)
- **Multithreaded `ntdll!NtQueryDirectoryFile` Walker**:
  - Queries **`FileIdFullDirectoryInformation` (class 38)** with automatic fallback to `FileFullDirectoryInformation` (class 2) and `FindFirstFileExW`, retrieving **logical size, allocated on-disk size (`AllocationSize`), and NTFS `FileId` in a single syscall** without extra `GetCompressedFileSizeW` calls.
  - WinDirStat-matched thread-local buffers: **4 MiB on local drives** and **64 KiB on SMB/UNC network shares** (`FILE_OPEN_FOR_BACKUP_INTENT`).
  - Accurate detection of **`IO_REPARSE_TAG_WOF`** (CompactOS), **NTFS Junctions vs. Mount Points** (`FSCTL_GET_REPARSE_POINT`), **OneDrive/Cloud Offline** anti-hydration protection, **`SeBackupPrivilege` / `SeRestorePrivilege`** activation, and configurable process priority (*Low*, *Normal*, *High*).
- **Direct NTFS `$MFT` Reader**: Reads the Master File Table directly when running elevated as Administrator.
- **NTFS Hardlink Deduplication**: Deduplicates physical disk allocation by `(volume, FileId)` to prevent double-counting (e.g., WinSxS) and groups linked paths under the virtual `<Hardlinks>` container.
- **Duplicate Detection & Multi-Hashing**: Fast BLAKE3 and Sparse Hash sampling for large files (> 64 MiB), plus on-demand MD5 / SHA-256 / BLAKE3 hashing.
- **Permissions, Owners & Live Watcher**: Win32 NTFS/SMB owner (`DOMAIN\user`) and DACL security auditing, drive/UNC enumeration, and real-time directory change monitoring (`ReadDirectoryChangesW`).

### Snapshots, Diffing & Reporting (`mediastat-storage`)
- **SQLite WAL `.mstat` Snapshots**: Fast persistence of full scan trees (with backward compatibility for legacy v2 `.mstat` snapshots) plus CSV snapshot import.
- **Before vs. After Diff Engine**: Compares two snapshots to pinpoint folder-level growth and space reclamation.
- **Multi-Format Exports**: Export scan results or selections to CSV, JSON, JSONL, Permissions CSV, Duplicates CSV, PowerShell Quarantine scripts, and **self-contained Executive HTML Reports**.

---

## Workspace Architecture

MediaStat is organized as a Cargo workspace:

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

---

## Headless CLI Usage (File Servers & Automation)

```powershell
# 1. List local drives, resolved UNC targets, and NTFS $MFT support (supports --lang en|es)
.\target\release\mediastat-cli.exe drives
.\target\release\mediastat-cli.exe --lang en drives

# 2. Scan with NTFS owners, DACL security audit, Sparse Hash duplicates, CSV, and Executive HTML report
.\target\release\mediastat-cli.exe scan P:\Projects \\fileserver\media `
  --out audit.mstat `
  --scan-owners --scan-perms --sparse-dupes `
  --export-csv largest_files.csv `
  --export-html executive_report.html

# 3. Export Top N largest files or generate a PowerShell quarantine script from a snapshot
.\target\release\mediastat-cli.exe export audit.mstat --format csv --top 500 --out top500.csv
.\target\release\mediastat-cli.exe export audit.mstat --format ps1-quarantine --out quarantine.ps1

# 4. Print a snapshot summary or diff two snapshots (Before vs. After)
.\target\release\mediastat-cli.exe summary audit.mstat
.\target\release\mediastat-cli.exe diff audit_before.mstat audit_after.mstat
```

## License

MIT
