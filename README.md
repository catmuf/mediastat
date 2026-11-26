# MediaStat v0.3

High-speed disk space analyzer inspired by **WinDirStat (`master` / 2.10)** with **Advanced Search** and **SysAdmin tools**, written **100% in Rust** (native performance, memory-safe, GPU-accelerated with `egui`/`eframe` + `wgpu` Vulkan/DX12).

---

## Key Features

### 1. Faithful WinDirStat Experience & Interactive Visualizations
- **Native Multithreaded `ntdll!NtQueryDirectoryFile` Engine**:
  - Queries **`FileIdFullDirectoryInformation` (class 38)** with automatic fallback to `FileFullDirectoryInformation` (class 2) and `FindFirstFileExW`, retrieving **logical size, allocated on-disk size (`AllocationSize`), and NTFS `FileId` in a single syscall** without extra `GetCompressedFileSizeW` calls.
  - WinDirStat-matched thread-local buffers: **4 MiB on local drives** and **64 KiB on SMB/UNC network shares** (`FILE_OPEN_FOR_BACKUP_INTENT`).
  - Accurate detection of **`IO_REPARSE_TAG_WOF`** (CompactOS), **NTFS Junctions vs. Mount Points** (`FSCTL_GET_REPARSE_POINT`), **OneDrive/Cloud Offline** anti-hydration protection, **`SeBackupPrivilege` / `SeRestorePrivilege`** activation, and configurable process priority (*Low*, *Normal*, *High*).
  - Direct **NTFS `$MFT`** reader when running elevated as Administrator.
- **NTFS Hardlink Deduplication by `(volume, FileId)`**:
  - Prevents double-counting physical disk space (e.g., WinSxS, hardlinked backups) and exposes an expandable virtual **`<Hardlinks>`** container under each volume.
- **Exact 3D Cushion Treemap (`CColorSpace::NormalizeColor` + 18 Official Colors)**:
  - **Official 18-color WinDirStat palette** (`WINDIRSTAT_PALETTE_18`) normalized via `make_bright_color(c, 0.6)` and assigned by descending byte rank per extension, with **3D mini-cushions** in the Extension List.
  - Exact `AddRidge` (scale-invariant slope) + `DrawCushion` (`lz = 10.0`, brightness `/ 0.6`, saturation `0–100%`, 1px Auto/RGB grid, configurable highlight color, 5 presets: `Classic`, `Calm`, `Flat`, `Pastel`, `HighContrast`), 4 layout styles (`Squarified`, `KDirStat`, `Hilbert`, `Moore`), and a **16×16 spatial grid** for O(1) hit-testing.
  - **3 Color Modes**: *By Extension (WinDirStat)*, *By Category*, and *File Age (Cold Data)*, paired with **4 Palettes**: *Official WinDirStat (18 Colors)*, *MediaStat Semantic (CAD/Media)*, *Colorblind Safe (Okabe-Ito)*, and *SysAdmin Thermal (High Contrast)*.
  - **Radial Sunburst** and **Flame Graph** views with hierarchical branch coloring (`CFolderColors` 8-color palette) and **WCAG 2.1 contrast** (`luminance > 0.179`).
  - **2D `XYSlider`** light-direction control and live 3D Cushion preview (`BuildDemoTree`) in *Options → Visualization*.
- **12 Official Window Layouts (3×4 Grid in `Layout`) + Independent OS Windows**:
  - All 12 WinDirStat layouts (`LayoutPopup.cpp`) plus **independent native OS subwindows** (`HWND` multi-viewport) for dialogs and detachable Visualization / Extensions & Inspector panels across monitors.
- **Directory Tree & Integrated Windows Utilities**:
  - Virtual `<Files>`, `<Hardlinks>`, `<Free Space>` (`F6`), and `<Unknown Space>` (`F7`) nodes, classic 8-color depth bars (`FileTreeColor0..7`), configurable columns, and `Ctrl+↑/↓` / `Ctrl+←/→` keyboard navigation.
  - Built-in Windows tools: *Sparsify file (`fsutil`)*, *Sync folder timestamps to newest file*, *NTFS/CompactOS compression*, *Remove MOTW (`Unblock-File`)*, *Delete empty subfolders*, *Open AppData (Roaming/Local)*, `appwiz.cpl`, `cleanmgr`, *Storage Sense*, `dfrgui`, `diskmgmt.msc`, `DISM /AnalyzeComponentStore`, `/StartComponentCleanup`, `/ResetBase`, and *Disable Hibernation (`powercfg /h off`)*.

### 2. Supercharged Advanced Search (`Ctrl+F`) & Universal Multi-Selection
- **9 File Category Filters** (multi-select chips): *Images*, *Videos*, *Audio*, *Documents*, *CAD / BIM / 3D*, *Archives / Disk Images*, *Executables / Code*, *Databases / Mail*, and *Other*.
- **Extension Filters**: Multi-value **Include** and **Exclude** fields (`bmp, tif, cr2, dwg`) plus a **`+ From scan`** picker listing the **Top 25 extensions** from the current scan.
- **Size, Age, Owner & Diagnostic Filters**: Logical and on-disk size filters with units (`B` / `KB` / `MB` / `GB` / `TB`), file age in days, owner (`DOMAIN\user`), scope (full scan or selected folder), **Regex / Glob / Whole Word / Case-Sensitive / Full Path**, persistent search history, and diagnostic flags (*Duplicates*, *Hardlinks*, *NTFS Compressed*, *Cloud Offline*, *Hidden*, *System*, *Read-Only*, `>260 chars`).
- **Universal Multi-Selection & Batch Actions** (across *Tree*, *Advanced Search*, *Largest Files*, *Duplicates*, *DACL Permissions*, *SysAdmin Audit*, and *Treemap*):
  - Multi-select via **`Ctrl+Click`**, **`Shift+Click`** (range), **`Ctrl+A`** (all in active view), or row/header checkboxes, plus **`Select redundant copies (keep 1st)`** in Duplicates.
  - Contextual selection bar and unified **Actions** menu with hierarchy-deduplicated totals: *Copy full paths (`Ctrl+C`)*, *Copy names*, *Copy resolved UNC paths*, *Copy TSV summary for Excel*, *Reveal in Explorer*, *Export selected items (CSV/JSON/JSONL/HTML/Scripts)*, *Exclude in filter*, *Move to folder*, *NTFS/CompactOS compression*, *Remove MOTW*, *Sparsify*, *Sync timestamps*, *Quick SMB Actions (`Ctrl+0..9`)*, and *Batch Recycle Bin / Permanent Delete* with confirmation (protected by **Read-Only Mode**).
  - **Multi-Selection Inspector** showing file/folder breakdown, deduplicated logical/physical totals, and removable selected items.

### 3. SysAdmin Safety, Snapshots & Live Bilingual UI
- **Read-Only Mode Enabled by Default**: All write/delete operations require unlocking the safety lock in the top bar and confirming in a modal dialog; read-only SMB quick actions (`Ctrl+0..6`, with real UNC `%u` resolution) work while locked.
- **Snapshots, Diff & Reports**: SQLite WAL `.mstat` snapshots (with legacy v2 compatibility), CSV snapshot import, **Before vs. After Diff** viewer, real-time **File Watcher**, **Storage Analytics**, **DACL Permissions**, **SysAdmin Audit**, and **self-contained Executive HTML Reports**.
- **Live Bilingual Interface (`EN` / `ES`)**: Switch instantly between English and Spanish without restarting via *Options → Language*, the start screen selector (`ES | EN`), or `--lang en|es` in the CLI; persisted in `%APPDATA%\MediaStat\settings.json`.

---

## Keyboard Shortcuts (GUI)

| Shortcut | Action |
|---|---|
| `Ctrl+O` / `Ctrl+S` | Select drives/folders/UNC targets · Save `.mstat` snapshot |
| `Ctrl+F` | Open **Advanced Search** |
| `Ctrl+A` / `Ctrl+Click` / `Shift+Click` | Select all in active view · Toggle item / range multi-selection |
| `F5` / `Shift+F5` | Refresh selected folder / Re-scan all targets |
| `F6` / `F7` / `F8` / `F9` | Toggle `<Free Space>` / `<Unknown Space>` / Extensions / Visualization |
| `Ctrl+E` / `Ctrl+C` / `Alt+Enter` | Reveal in Explorer · Copy full path(s) · Windows Properties |
| `Ctrl+↑` / `Ctrl+↓` | Select parent / Reselect child |
| `Ctrl+←` / `Ctrl+→` | Jump to first subfolder / largest file |
| `Del` / `Shift+Del` | Send to Recycle Bin / Permanent delete *(requires unlocking Read-Only lock)* |
| `Ctrl+0` .. `Ctrl+6` | Read-only quick SMB actions (`icacls`, PowerShell, SHA-256, copy UNC `%u`, `compact /q`, `robocopy /L`) |
| `Ctrl+7` .. `Ctrl+9` | Write SMB actions (`compact /c`, `Unblock-File`, Quarantine; *requires unlocking*) |
| `Backspace` / `Esc` | Zoom out one level in visualization / Reset zoom |

---

## Workspace Architecture

- `crates/mediastat-core` — Arena-backed scan tree, file categories, filtering, formatting/palettes, i18n, and settings.
- `crates/mediastat-scanner` — High-speed `NtQueryDirectoryFile` walker, NTFS `$MFT` reader, hardlink deduplication, BLAKE3/Sparse duplicate hashing, DACL permissions, and live file watcher.
- `crates/mediastat-storage` — SQLite WAL `.mstat` snapshots, snapshot diffing, exports, and self-contained HTML reports.
- `crates/mediastat-cli` — Headless CLI for file servers and automation.
- `crates/mediastat-gui` — GPU-accelerated desktop application (`egui`/`eframe` + `wgpu`).

---

## Build & Run

```powershell
# Build release binaries (maximum performance with LTO)
cargo build --release --workspace

# Launch the desktop GUI application
.\target\release\mediastat-gui.exe

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
