use std::path::{Path, PathBuf};
use std::process::Command;

use crossbeam_channel::{bounded, Receiver, Sender};
use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use mediastat_core::{t, UserCleanupAction};
use mediastat_scanner::{compute_file_hashes, FileHashes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NtfsCompressionAlg {
    None,
    Lznt1,
    Xpress4k,
    Xpress8k,
    Xpress16k,
    Lzx,
}

impl NtfsCompressionAlg {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => t("Descomprimir (Ninguna)", "Uncompress (None)"),
            Self::Lznt1 => t("NTFS Clásica (LZNT1)", "Classic NTFS (LZNT1)"),
            Self::Xpress4k => t("CompactOS XPRESS4K (rápida)", "CompactOS XPRESS4K (fast)"),
            Self::Xpress8k => t("CompactOS XPRESS8K (equilibrada)", "CompactOS XPRESS8K (balanced)"),
            Self::Xpress16k => "CompactOS XPRESS16K",
            Self::Lzx => t("CompactOS LZX (máxima compresión)", "CompactOS LZX (max compression)"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsSysTool {
    DiskCleanup,
    StorageSense,
    ProgramsAndFeatures,
    DiskManagement,
    DefragOptimize,
    DismAnalyze,
    DismCleanup,
    DismResetBase,
    DisableHibernation,
}

/// Open the OS file manager with the given file or folder selected/highlighted.
pub fn reveal_in_explorer(path: &Path) {
    #[cfg(windows)]
    {
        let arg = format!("/select,\"{}\"", path.display());
        use std::os::windows::process::CommandExt;
        let _ = Command::new("explorer.exe").raw_arg(&arg).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = path.parent().unwrap_or(path);
        let _ = Command::new("xdg-open").arg(dir).spawn();
    }
}

/// Reveal multiple files/folders in Explorer safely: groups by parent directory and caps
/// opened Explorer windows at `max_windows` (e.g. 5) so users never accidentally spawn dozens.
/// Returns `(windows_opened, skipped_parent_folders)`.
pub fn reveal_multiple_in_explorer(paths: &[PathBuf], max_windows: usize) -> (usize, usize) {
    if paths.is_empty() {
        return (0, 0);
    }
    if paths.len() == 1 {
        reveal_in_explorer(&paths[0]);
        return (1, 0);
    }

    let mut seen_parents: Vec<PathBuf> = Vec::new();
    let mut representatives: Vec<&Path> = Vec::new();
    for p in paths {
        let parent = p.parent().unwrap_or(p.as_path()).to_path_buf();
        if !seen_parents.contains(&parent) {
            seen_parents.push(parent);
            representatives.push(p.as_path());
        }
    }

    let cap = max_windows.max(1);
    let to_open = representatives.len().min(cap);
    for p in &representatives[..to_open] {
        reveal_in_explorer(p);
    }
    (to_open, representatives.len().saturating_sub(to_open))
}

/// Open `%APPDATA%` (Roaming) or `%LOCALAPPDATA%` (Local) directly in Explorer (WinDirStat `CleanUp` parity).
pub fn open_appdata_dir(roaming: bool) {
    let var = if roaming { "APPDATA" } else { "LOCALAPPDATA" };
    if let Some(p) = std::env::var_os(var) {
        #[cfg(windows)]
        {
            let _ = Command::new("explorer.exe").arg(PathBuf::from(p)).spawn();
        }
        #[cfg(not(windows))]
        {
            let _ = p;
        }
    }
}

/// Open a file or folder with the operating system's default application.
pub fn open_with_default_app(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }
}

/// Open PowerShell or CMD directly inside `target` (or its parent directory if file).
pub fn open_terminal_at(target: &Path, powershell: bool, as_admin: bool) {
    let dir = if target.is_dir() {
        target
    } else {
        target.parent().unwrap_or(target)
    };
    #[cfg(windows)]
    {
        if as_admin {
            let exe = if powershell { "powershell.exe" } else { "cmd.exe" };
            let params = if powershell {
                format!(
                    "-NoExit -Command \"Set-Location -LiteralPath '{}'\"",
                    dir.to_string_lossy().replace('\'', "''")
                )
            } else {
                format!("/K cd /D \"{}\"", dir.display())
            };
            shell_execute_verb("runas", exe, &params);
        } else if powershell {
            let _ = Command::new("powershell.exe")
                .args([
                    "-NoExit",
                    "-Command",
                    &format!(
                        "Set-Location -LiteralPath '{}'",
                        dir.to_string_lossy().replace('\'', "''")
                    ),
                ])
                .spawn();
        } else {
            let _ = Command::new("cmd.exe")
                .args(["/K", "cd", "/D"])
                .arg(dir)
                .spawn();
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (dir, powershell, as_admin);
    }
}

/// Show native Windows File/Folder Properties dialog (`Alt+Enter` parity with WinDirStat).
pub fn show_windows_properties(path: &Path) {
    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::{
            ShellExecuteExW, SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;

        let verb: Vec<u16> = OsStr::new("properties")
            .encode_wide()
            .chain(Some(0))
            .collect();
        let file: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_INVOKEIDLIST;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.nShow = SW_SHOW;
        unsafe {
            ShellExecuteExW(&mut info);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

/// Safely move one or more files/folders into the Windows Recycle Bin in a single native
/// `SHFileOperationW` batch call (double-null-terminated `pFrom` buffer).
pub fn move_multiple_to_recycle_bin(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::{
            SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
            FO_DELETE, SHFILEOPSTRUCTW,
        };
        let mut wide: Vec<u16> = Vec::with_capacity(paths.len() * 64);
        for p in paths {
            wide.extend(p.as_os_str().encode_wide());
            wide.push(0);
        }
        wide.push(0);

        let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
        op.wFunc = FO_DELETE;
        op.pFrom = wide.as_ptr();
        op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16;

        let rc = unsafe { SHFileOperationW(&mut op) };
        if rc == 0 && op.fAnyOperationsAborted == 0 {
            Ok(())
        } else {
            Err(format!("SHFileOperationW code {rc}"))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
        Err(t(
            "Enviar a Papelera solo implementado en Windows",
            "Move to Recycle Bin is only implemented on Windows",
        )
        .into())
    }
}

/// Permanently delete a file or directory tree (handling read-only attributes cleanly).
#[allow(clippy::permissions_set_readonly_false)]
pub fn delete_permanently(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        clear_readonly_recursive(path);
        std::fs::remove_dir_all(path).map_err(|e| e.to_string())
    } else {
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            if perms.readonly() {
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }
}

#[allow(clippy::permissions_set_readonly_false)]
fn clear_readonly_recursive(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if let Ok(meta) = entry.metadata() {
                let mut perms = meta.permissions();
                if perms.readonly() {
                    perms.set_readonly(false);
                    let _ = std::fs::set_permissions(&p, perms);
                }
                if meta.is_dir() {
                    clear_readonly_recursive(&p);
                }
            }
        }
    }
}

/// Empty Windows Recycle Bin (`SHEmptyRecycleBinW`).
pub fn empty_recycle_bin() -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::SHEmptyRecycleBinW;
        let hr = unsafe { SHEmptyRecycleBinW(std::ptr::null_mut(), std::ptr::null(), 0) };
        if hr == 0 {
            Ok(())
        } else {
            Err(format!("SHEmptyRecycleBinW devolvió 0x{hr:08X}"))
        }
    }
    #[cfg(not(windows))]
    {
        Err("Solo disponible en Windows".into())
    }
}

/// Remove empty subdirectories bottom-up under `dir`. Returns count removed.
pub fn remove_empty_subfolders(dir: &Path) -> usize {
    let mut removed = 0usize;
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            removed += remove_empty_subfolders(&p);
            if std::fs::read_dir(&p).is_ok_and(|mut it| it.next().is_none())
                && std::fs::remove_dir(&p).is_ok()
            {
                removed += 1;
            }
        }
    }
    removed
}

/// Mark a file as NTFS Sparse (`fsutil sparse setflag`) matching WinDirStat `ID_CLEANUP_SPARSIFY`.
pub fn sparsify_file(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        return Err("Solo se puede sparsificar un archivo individual".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = Command::new("fsutil.exe")
            .args(["sparse", "setflag"])
            .arg(path)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let msg2 = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Err(if !msg.is_empty() { msg } else { msg2 })
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("Sparsificar solo disponible en Windows NTFS".into())
    }
}

/// Recursively synchronize folder modified timestamps to match the newest file/subfolder inside each folder
/// (exact WinDirStat `ID_TOOLS_SET_DATES` parity). Returns number of folders updated.
pub fn sync_folder_dates_to_newest(dir: &Path) -> Result<usize, String> {
    if !dir.is_dir() {
        return Err("La ruta seleccionada no es una carpeta".into());
    }
    #[cfg(windows)]
    {
        let mut updated = 0usize;
        let _ = sync_folder_dates_rec_win(dir, &mut updated);
        Ok(updated)
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        Err("Sincronización de fechas de carpeta solo disponible en Windows".into())
    }
}

#[cfg(windows)]
fn sync_folder_dates_rec_win(dir: &Path, updated: &mut usize) -> Option<u64> {
    use std::os::windows::fs::MetadataExt;
    let rd = std::fs::read_dir(dir).ok()?;
    let mut newest_ft: u64 = 0;

    for entry in rd.flatten() {
        let p = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let ft = if meta.is_dir() {
            sync_folder_dates_rec_win(&p, updated).unwrap_or_else(|| meta.last_write_time())
        } else {
            meta.last_write_time()
        };
        if ft > newest_ft {
            newest_ft = ft;
        }
    }

    if newest_ft > 0 && set_dir_mtime_filetime(dir, newest_ft) {
        *updated += 1;
        Some(newest_ft)
    } else {
        std::fs::metadata(dir).ok().map(|m| m.last_write_time())
    }
}

#[cfg(windows)]
fn set_dir_mtime_filetime(dir: &Path, filetime_u64: u64) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, SetFileTime, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, OPEN_EXISTING,
    };

    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_WRITE_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return false;
    }
    let ft = FILETIME {
        dwLowDateTime: (filetime_u64 & 0xFFFF_FFFF) as u32,
        dwHighDateTime: (filetime_u64 >> 32) as u32,
    };
    let ok = unsafe { SetFileTime(h, std::ptr::null(), std::ptr::null(), &ft) } != 0;
    unsafe {
        CloseHandle(h);
    }
    ok
}

/// Apply or remove native Windows NTFS / CompactOS compression on a file or folder.
pub fn apply_ntfs_compression(path: &Path, alg: NtfsCompressionAlg) -> Result<(), String> {
    #[cfg(windows)]
    {
        let mut args: Vec<String> = Vec::new();
        match alg {
            NtfsCompressionAlg::None => args.push("/U".into()),
            NtfsCompressionAlg::Lznt1 => args.push("/C".into()),
            NtfsCompressionAlg::Xpress4k => {
                args.push("/C".into());
                args.push("/EXE:XPRESS4K".into());
            }
            NtfsCompressionAlg::Xpress8k => {
                args.push("/C".into());
                args.push("/EXE:XPRESS8K".into());
            }
            NtfsCompressionAlg::Xpress16k => {
                args.push("/C".into());
                args.push("/EXE:XPRESS16K".into());
            }
            NtfsCompressionAlg::Lzx => {
                args.push("/C".into());
                args.push("/EXE:LZX".into());
            }
        }
        if path.is_dir() {
            args.push(format!("/S:{}", path.display()));
            args.push("/I".into());
        } else {
            args.push("/I".into());
            args.push(path.to_string_lossy().into_owned());
        }
        Command::new("compact.exe")
            .args(&args)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = (path, alg);
        Err("Compresión NTFS solo disponible en Windows".into())
    }
}

/// Remove Mark-of-the-Web (`Zone.Identifier` NTFS Alternate Data Stream).
pub fn remove_motw(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let ads = format!("{}:Zone.Identifier", path.display());
        match std::fs::remove_file(&ads) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}

/// Move a file or folder into a destination folder selected by the user.
pub fn move_item_to_folder(src: &Path, dst_dir: &Path) -> Result<PathBuf, String> {
    let name = src
        .file_name()
        .ok_or_else(|| "Ruta de origen sin nombre de archivo".to_string())?;
    let dst = dst_dir.join(name);
    std::fs::rename(src, &dst)
        .map_err(|e| format!("No se pudo mover (¿distinta unidad?): {e}"))?;
    Ok(dst)
}

/// Relaunch the current `mediastat-gui.exe` process elevated as Administrator via UAC `runas`.
pub fn relaunch_as_administrator() -> bool {
    #[cfg(windows)]
    {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        shell_execute_verb("runas", &exe.to_string_lossy(), "")
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Launch native Windows SysAdmin disk maintenance tools (100% WinDirStat parity).
pub fn launch_windows_tool(tool: WindowsSysTool) {
    #[cfg(windows)]
    {
        match tool {
            WindowsSysTool::DiskCleanup => {
                let _ = Command::new("cleanmgr.exe").spawn();
            }
            WindowsSysTool::StorageSense => {
                let _ = Command::new("cmd.exe")
                    .args(["/C", "start", "ms-settings:storagesense"])
                    .spawn();
            }
            WindowsSysTool::ProgramsAndFeatures => {
                let _ = Command::new("cmd.exe")
                    .args(["/C", "start", "appwiz.cpl"])
                    .spawn();
            }
            WindowsSysTool::DiskManagement => {
                let _ = Command::new("cmd.exe")
                    .args(["/C", "start", "diskmgmt.msc"])
                    .spawn();
            }
            WindowsSysTool::DefragOptimize => {
                let _ = Command::new("dfrgui.exe").spawn();
            }
            WindowsSysTool::DismAnalyze => {
                shell_execute_verb(
                    "runas",
                    "cmd.exe",
                    "/K dism.exe /Online /Cleanup-Image /AnalyzeComponentStore",
                );
            }
            WindowsSysTool::DismCleanup => {
                shell_execute_verb(
                    "runas",
                    "cmd.exe",
                    "/K dism.exe /Online /Cleanup-Image /StartComponentCleanup",
                );
            }
            WindowsSysTool::DismResetBase => {
                shell_execute_verb(
                    "runas",
                    "cmd.exe",
                    "/K dism.exe /Online /Cleanup-Image /StartComponentCleanup /ResetBase",
                );
            }
            WindowsSysTool::DisableHibernation => {
                shell_execute_verb("runas", "cmd.exe", "/K powercfg.exe /h off");
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = tool;
    }
}

/// Execute a User-Defined Cleanup / Quick Action (`Ctrl+0..9`), resolving `%u` to real UNC on mapped SMB drives.
pub fn execute_user_cleanup(action: &UserCleanupAction, target: &Path) -> Result<(), String> {
    let unc = mediastat_scanner::resolve_to_unc_path(target);
    let expanded = action.expand_command_with_unc(target, Some(&unc));
    if expanded.trim().is_empty() {
        return Err("La línea de comando está vacía".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = Command::new("cmd.exe");
        cmd.raw_arg(format!("/C {expanded}"));
        if action.run_minimized {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        Command::new("sh")
            .args(["-c", &expanded])
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(windows)]
fn shell_execute_verb(verb: &str, file: &str, params: &str) -> bool {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb_w: Vec<u16> = OsStr::new(verb).encode_wide().chain(Some(0)).collect();
    let file_w: Vec<u16> = OsStr::new(file).encode_wide().chain(Some(0)).collect();
    let params_w: Vec<u16> = OsStr::new(params).encode_wide().chain(Some(0)).collect();

    let h = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb_w.as_ptr(),
            file_w.as_ptr(),
            if params.is_empty() {
                std::ptr::null()
            } else {
                params_w.as_ptr()
            },
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    (h as isize) > 32
}

// ============================================================================
// Asynchronous Image Thumbnail & Multi-Hash Background Loaders
// ============================================================================

struct ThumbResult {
    path: PathBuf,
    image: Option<ColorImage>,
}

pub struct ThumbnailLoader {
    req_tx: Sender<PathBuf>,
    res_rx: Receiver<ThumbResult>,
    current_path: Option<PathBuf>,
    current_texture: Option<TextureHandle>,
    loading: bool,
}

impl ThumbnailLoader {
    pub fn new(ctx: Context) -> Self {
        let (req_tx, req_rx) = bounded::<PathBuf>(8);
        let (res_tx, res_rx) = bounded::<ThumbResult>(8);

        std::thread::spawn(move || {
            while let Ok(mut path) = req_rx.recv() {
                while let Ok(newer) = req_rx.try_recv() {
                    path = newer;
                }
                let img = decode_thumbnail(&path);
                let _ = res_tx.send(ThumbResult { path, image: img });
                ctx.request_repaint();
            }
        });

        Self {
            req_tx,
            res_rx,
            current_path: None,
            current_texture: None,
            loading: false,
        }
    }

    pub fn request_for(&mut self, path: &Path, ctx: &Context) -> (Option<&TextureHandle>, bool) {
        while let Ok(res) = self.res_rx.try_recv() {
            if self.current_path.as_deref() == Some(res.path.as_path()) {
                self.loading = false;
                self.current_texture = res.image.map(|ci| {
                    ctx.load_texture(
                        format!("thumb:{}", res.path.display()),
                        ci,
                        TextureOptions::LINEAR,
                    )
                });
            }
        }

        if self.current_path.as_deref() != Some(path) {
            self.current_path = Some(path.to_path_buf());
            self.current_texture = None;
            self.loading = true;
            let _ = self.req_tx.try_send(path.to_path_buf());
        }

        (self.current_texture.as_ref(), self.loading)
    }

    pub fn clear(&mut self) {
        self.current_path = None;
        self.current_texture = None;
        self.loading = false;
    }
}

fn decode_thumbnail(path: &Path) -> Option<ColorImage> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 80 * 1024 * 1024 || meta.len() == 0 {
        return None;
    }
    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let dyn_img = reader.decode().ok()?;
    let thumb = dyn_img.thumbnail(260, 190).to_rgba8();
    let size = [thumb.width() as usize, thumb.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, thumb.as_raw()))
}

/// On-demand async MD5 / SHA-256 / BLAKE3 hash calculator for the Inspector panel.
#[derive(Default)]
pub struct AsyncHashState {
    pub path: Option<PathBuf>,
    pub loading: bool,
    pub result: Option<Result<FileHashes, String>>,
    rx: Option<Receiver<(PathBuf, Result<FileHashes, String>)>>,
}

impl AsyncHashState {
    pub fn start(&mut self, path: PathBuf, ctx: Context) {
        let (tx, rx) = bounded(1);
        self.path = Some(path.clone());
        self.loading = true;
        self.result = None;
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let res = compute_file_hashes(&path).map_err(|e| e.to_string());
            let _ = tx.send((path, res));
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self) {
        if let Some(rx) = &self.rx {
            if let Ok((p, res)) = rx.try_recv() {
                if self.path.as_deref() == Some(p.as_path()) {
                    self.loading = false;
                    self.result = Some(res);
                }
                self.rx = None;
            }
        }
    }

    pub fn clear_if_different(&mut self, current: &Path) {
        if self.path.as_deref() != Some(current) {
            self.path = None;
            self.loading = false;
            self.result = None;
            self.rx = None;
        }
    }
}
