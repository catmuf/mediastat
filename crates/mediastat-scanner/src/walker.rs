use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use crossbeam_channel::{bounded, unbounded};
use mediastat_core::{
    CompiledFilters, IssueKind, NodeFlags, NodeId, ProcessPriority, ScanConfig, ScanIssue,
    ScanTree, WinAttrs,
};

use crate::drives::{detect_path_media_type, query_volume_space, DriveMediaType};

/// Shared atomic counters updated continuously by scan workers with zero lock contention.
#[derive(Debug, Default)]
pub struct WalkerCounters {
    pub files_scanned: AtomicU64,
    pub dirs_scanned: AtomicU64,
    pub bytes_scanned: AtomicU64,
    pub errors: AtomicU64,
}

/// Traversal exclusion flags mirroring upstream WinDirStat `Options.h`.
#[derive(Debug, Clone, Copy)]
pub struct WalkerPolicy {
    pub exclude_junctions: bool,
    pub exclude_symlinks_dir: bool,
    pub exclude_symlinks_file: bool,
    pub exclude_mount_points: bool,
    pub exclude_hidden: bool,
    pub exclude_protected_system: bool,
}

impl From<&ScanConfig> for WalkerPolicy {
    fn from(c: &ScanConfig) -> Self {
        Self {
            exclude_junctions: c.exclude_junctions,
            exclude_symlinks_dir: c.exclude_symlinks_dir,
            exclude_symlinks_file: c.exclude_symlinks_file,
            exclude_mount_points: c.exclude_mount_points,
            exclude_hidden: c.exclude_hidden,
            exclude_protected_system: c.exclude_protected_system,
        }
    }
}

struct DirBatch {
    parent_id: NodeId,
    parent_path: PathBuf,
    access_denied: bool,
    entries: Vec<RawEntry>,
}

struct RawEntry {
    name: String,
    is_dir: bool,
    is_symlink: bool,
    is_junction: bool,
    is_mount_point: bool,
    is_hidden: bool,
    win_attrs: WinAttrs,
    logical_size: u64,
    allocated_size: u64,
    file_id: u64,
    mtime_secs: i64,
}

/// Enable `SE_BACKUP_NAME` and `SE_RESTORE_NAME` on the current process token (exact WinDirStat
/// `UseBackupRestore` behavior so elevated scans can traverse protected directories without Access Denied).
pub fn enable_backup_restore_privileges() {
    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LUID};
        use windows_sys::Win32::Security::{
            AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES,
            SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
        };
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        let mut token: HANDLE = std::ptr::null_mut();
        let ok = unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            ) != 0
        };
        if !ok || token.is_null() {
            return;
        }
        for priv_name in ["SeBackupPrivilege", "SeRestorePrivilege"] {
            let wide: Vec<u16> = OsStr::new(priv_name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let mut luid = LUID {
                LowPart: 0,
                HighPart: 0,
            };
            if unsafe { LookupPrivilegeValueW(std::ptr::null(), wide.as_ptr(), &mut luid) != 0 } {
                let tp = TOKEN_PRIVILEGES {
                    PrivilegeCount: 1,
                    Privileges: [LUID_AND_ATTRIBUTES {
                        Luid: luid,
                        Attributes: SE_PRIVILEGE_ENABLED,
                    }],
                };
                unsafe {
                    AdjustTokenPrivileges(
                        token,
                        0,
                        &tp,
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    );
                }
            }
        }
        unsafe {
            CloseHandle(token);
        }
    }
}

/// Apply WinDirStat process priority (`Low` / `Normal` / `High`) for scanning.
pub fn apply_process_priority(priority: ProcessPriority) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, SetPriorityClass, ABOVE_NORMAL_PRIORITY_CLASS,
            BELOW_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
        };
        let cls = match priority {
            ProcessPriority::Low => BELOW_NORMAL_PRIORITY_CLASS,
            ProcessPriority::Normal => NORMAL_PRIORITY_CLASS,
            ProcessPriority::High => ABOVE_NORMAL_PRIORITY_CLASS,
        };
        unsafe {
            SetPriorityClass(GetCurrentProcess(), cls);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = priority;
    }
}

/// Engine A — High-throughput multithreaded parallel directory walker using `ntdll!NtQueryDirectoryFile`
/// (4 MiB local buffer / 64 KiB SMB buffer, `FileIdFullDirectoryInformation`, automatic fallback).
#[allow(clippy::too_many_arguments)]
pub fn scan_target_parallel(
    tree: &mut ScanTree,
    target_root: &Path,
    filters: &Arc<CompiledFilters>,
    policy: WalkerPolicy,
    requested_threads: usize,
    counters: &Arc<WalkerCounters>,
    cancel: &Arc<AtomicBool>,
    pause: &Arc<AtomicBool>,
) -> NodeId {
    let root_id = tree.add_root_target(target_root);
    if let Some(vs) = query_volume_space(target_root) {
        tree.volume_space.insert(root_id, vs);
    }
    scan_into_existing_dir(
        tree,
        root_id,
        target_root,
        filters,
        policy,
        requested_threads,
        counters,
        cancel,
        pause,
    );
    root_id
}

/// Parallel walk populating children directly under an existing `root_id` (used by initial scan AND `F5` Rescan).
#[allow(clippy::too_many_arguments)]
pub fn scan_into_existing_dir(
    tree: &mut ScanTree,
    root_id: NodeId,
    root_path: &Path,
    filters: &Arc<CompiledFilters>,
    policy: WalkerPolicy,
    requested_threads: usize,
    counters: &Arc<WalkerCounters>,
    cancel: &Arc<AtomicBool>,
    pause: &Arc<AtomicBool>,
) {
    let media_type = detect_path_media_type(root_path);
    let is_remote = matches!(media_type, DriveMediaType::NetworkShare);
    let thread_count = if requested_threads > 0 {
        requested_threads.clamp(1, 64)
    } else {
        media_type.recommended_threads()
    };

    let cluster_size = query_cluster_size(root_path);
    let supports_file_id = Arc::new(AtomicBool::new(!is_remote));

    let (work_tx, work_rx) = unbounded::<(NodeId, PathBuf)>();
    let (batch_tx, batch_rx) = bounded::<DirBatch>(thread_count * 4);
    let in_flight = Arc::new(AtomicUsize::new(1));

    let _ = work_tx.send((root_id, root_path.to_path_buf()));

    let mut handles = Vec::with_capacity(thread_count);
    for _ in 0..thread_count {
        let rx = work_rx.clone();
        let btx = batch_tx.clone();
        let flt = Arc::clone(filters);
        let cnt = Arc::clone(counters);
        let sup_fid = Arc::clone(&supports_file_id);
        let cancel_flag = Arc::clone(cancel);
        let pause_flag = Arc::clone(pause);

        handles.push(std::thread::spawn(move || {
            while let Ok((parent_id, dir_path)) = rx.recv() {
                if cancel_flag.load(Ordering::Relaxed) {
                    let _ = btx.send(DirBatch {
                        parent_id,
                        parent_path: dir_path,
                        access_denied: false,
                        entries: Vec::new(),
                    });
                    continue;
                }
                while pause_flag.load(Ordering::Relaxed) && !cancel_flag.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }

                let (entries, access_denied) = read_dir_fast(
                    &dir_path,
                    &flt,
                    policy,
                    cluster_size,
                    is_remote,
                    &sup_fid,
                    &cnt,
                );
                if btx
                    .send(DirBatch {
                        parent_id,
                        parent_path: dir_path,
                        access_denied,
                        entries,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    drop(batch_tx);

    while in_flight.load(Ordering::Acquire) > 0 {
        let Ok(batch) = batch_rx.recv() else {
            break;
        };
        in_flight.fetch_sub(1, Ordering::AcqRel);

        if cancel.load(Ordering::Relaxed) {
            continue;
        }

        if batch.access_denied {
            tree.flags[batch.parent_id.index()].insert(NodeFlags::ACCESS_DENIED);
            if tree.issues.len() < 100_000 {
                tree.issues.push(ScanIssue {
                    kind: IssueKind::AccessDenied,
                    path: batch.parent_path.clone(),
                    detail: "Permiso denegado al enumerar carpeta (prueba con Modo Administrador)".into(),
                });
            }
        }

        for entry in batch.entries {
            let mut flags = NodeFlags::empty();
            if entry.is_dir {
                flags.insert(NodeFlags::IS_DIR);
            }
            if entry.is_symlink {
                flags.insert(NodeFlags::IS_SYMLINK);
            }
            if entry.is_junction {
                flags.insert(NodeFlags::IS_JUNCTION);
            }
            if entry.is_mount_point {
                flags.insert(NodeFlags::IS_MOUNT_POINT);
            }
            if entry.is_hidden {
                flags.insert(NodeFlags::IS_HIDDEN);
            }

            let child_path = batch.parent_path.join(&entry.name);
            let path_len = child_path.as_os_str().len();
            if path_len > 260 {
                flags.insert(NodeFlags::IS_LONG_PATH);
                if tree.issues.len() < 100_000 {
                    tree.issues.push(ScanIssue {
                        kind: IssueKind::LongPath,
                        path: child_path.clone(),
                        detail: format!("Longitud de ruta: {path_len} caracteres (>260 MAX_PATH)"),
                    });
                }
            }

            let child_id = tree.add_child_full(
                batch.parent_id,
                &entry.name,
                flags,
                entry.win_attrs,
                entry.logical_size,
                entry.allocated_size,
                entry.file_id,
                entry.mtime_secs,
            );

            // Traverse into subdirectories unless excluded by WinDirStat reparse rules
            let skip_descend = (entry.is_symlink && policy.exclude_symlinks_dir)
                || (entry.is_junction && policy.exclude_junctions)
                || (entry.is_mount_point && policy.exclude_mount_points);

            if entry.is_dir && !skip_descend {
                in_flight.fetch_add(1, Ordering::AcqRel);
                let _ = work_tx.send((child_id, child_path));
            }
        }
    }

    drop(work_tx);
    for h in handles {
        let _ = h.join();
    }
}

#[inline]
fn align_to_cluster(size: u64, cluster_size: u64) -> u64 {
    if size == 0 {
        return 0;
    }
    let cs = cluster_size.max(512);
    size.div_ceil(cs) * cs
}

fn query_cluster_size(path: &Path) -> u64 {
    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceW;
        let s = path.to_string_lossy();
        let root_str = if s.len() >= 2 && s.as_bytes()[1] == b':' {
            format!("{}:\\", s.chars().next().unwrap())
        } else {
            return 4096;
        };
        let wide: Vec<u16> = OsStr::new(&root_str)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut spc = 0u32;
        let mut bps = 0u32;
        let mut free_c = 0u32;
        let mut tot_c = 0u32;
        let ok = unsafe {
            GetDiskFreeSpaceW(
                wide.as_ptr(),
                &mut spc,
                &mut bps,
                &mut free_c,
                &mut tot_c,
            ) != 0
        };
        if ok && spc > 0 && bps > 0 {
            return (spc as u64) * (bps as u64);
        }
        4096
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        4096
    }
}

#[cfg(windows)]
mod nt_query {
    use std::cell::RefCell;
    use std::ffi::{c_void, OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    use mediastat_core::{CompiledFilters, WinAttrs};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_ENCRYPTED, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_OFFLINE,
        FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SPARSE_FILE,
        FILE_ATTRIBUTE_SYSTEM, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING, SYNCHRONIZE,
    };
    use windows_sys::Win32::System::Ioctl::FSCTL_GET_REPARSE_POINT;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows_sys::Win32::System::IO::DeviceIoControl;

    use super::{align_to_cluster, filetime_to_unix_secs, RawEntry, WalkerCounters, WalkerPolicy};

    // Exact WinDirStat FinderBasic.cpp constants
    const LOCAL_BUFFER_BYTES: usize = 4 * 1024 * 1024; // 4 MiB on local NTFS/FAT
    const REMOTE_BUFFER_BYTES: usize = 64 * 1024; // 64 KiB on SMB/UNC (fixes issue #631)
    const FILE_FULL_DIRECTORY_INFORMATION: u32 = 2;
    const FILE_ID_FULL_DIRECTORY_INFORMATION: u32 = 38;

    const STATUS_NO_MORE_FILES: i32 = 0x8000_0006u32 as i32;
    const STATUS_INVALID_INFO_CLASS: i32 = 0xC000_0003u32 as i32;
    const STATUS_INVALID_PARAMETER: i32 = 0xC000_000Du32 as i32;
    const STATUS_NOT_SUPPORTED: i32 = 0xC000_00BBu32 as i32;
    const STATUS_ACCESS_DENIED: i32 = 0xC000_0022u32 as i32;

    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
    const IO_REPARSE_TAG_WOF: u32 = 0x8000_0017;

    #[repr(C)]
    struct IoStatusBlock {
        status: isize,
        information: usize,
    }

    type NtQueryDirectoryFileFn = unsafe extern "system" fn(
        file_handle: HANDLE,
        event: HANDLE,
        apc_routine: *const c_void,
        apc_context: *const c_void,
        io_status_block: *mut IoStatusBlock,
        file_information: *mut c_void,
        length: u32,
        file_information_class: u32,
        return_single_entry: u8,
        file_name: *const c_void,
        restart_scan: u8,
    ) -> i32;

    static NT_QUERY_DIR_FN: OnceLock<Option<NtQueryDirectoryFileFn>> = OnceLock::new();

    fn get_nt_query_directory_file() -> Option<NtQueryDirectoryFileFn> {
        *NT_QUERY_DIR_FN.get_or_init(|| {
            let ntdll: Vec<u16> = OsStr::new("ntdll.dll")
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let hmod = unsafe { GetModuleHandleW(ntdll.as_ptr()) };
            if hmod.is_null() {
                return None;
            }
            let proc = unsafe { GetProcAddress(hmod, c"NtQueryDirectoryFile".as_ptr().cast()) };
            proc.map(|p| unsafe { std::mem::transmute::<_, NtQueryDirectoryFileFn>(p) })
        })
    }

    // 8-byte aligned thread-local reusable buffer (zero heap churn across millions of directories!)
    thread_local! {
        static DIR_BUF: RefCell<Vec<u64>> = RefCell::new(vec![0u64; LOCAL_BUFFER_BYTES / 8]);
    }

    /// Check whether an `IO_REPARSE_TAG_MOUNT_POINT` directory is a NTFS Junction (vs Volume Mount Point),
    /// matching upstream `FinderBasic.cpp:170-186`.
    fn is_reparse_junction(full_ext_path: &str) -> bool {
        let wide: Vec<u16> = OsStr::new(full_ext_path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return true; // conservative default: treat mount point as junction to avoid cycles
        }
        let mut buf = [0u8; 2048];
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                h,
                FSCTL_GET_REPARSE_POINT,
                std::ptr::null(),
                0,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            ) != 0
        };
        unsafe {
            CloseHandle(h);
        }
        if !ok || (returned as usize) < 16 {
            return true;
        }
        // REPARSE_DATA_BUFFER for MountPoint:
        // [0..4] ReparseTag, [4..6] ReparseDataLength, [6..8] Reserved,
        // [8..10] SubstituteNameOffset, [10..12] SubstituteNameLength, [12..14] PrintNameOffset,
        // [14..16] PrintNameLength, [16..] PathBuffer (UTF-16)
        let sub_off = u16::from_le_bytes([buf[8], buf[9]]) as usize;
        let sub_len = u16::from_le_bytes([buf[10], buf[11]]) as usize;
        let start = 16 + sub_off;
        let end = (start + sub_len).min(returned as usize);
        if end <= start || (end - start) < 2 {
            return true;
        }
        let u16_slice: Vec<u16> = buf[start..end]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        let target = OsString::from_wide(&u16_slice).to_string_lossy().to_ascii_lowercase();
        // Volume mount points point to `\??\volume{guid}\`; junctions point to `\??\x:\...`
        !target.starts_with(r"\??\volume{")
    }

    #[inline]
    fn is_cloud_reparse_tag(tag: u32) -> bool {
        // Microsoft Cloud Files API reparse tags (`IO_REPARSE_TAG_CLOUD*`, 0x9000_001A .. 0x9000_F01A)
        (tag & 0xFFFF_0FFF) == 0x9000_001A
    }

    /// High-speed `ntdll!NtQueryDirectoryFile` directory enumerator (returns `None` only if
    /// `NtQueryDirectoryFile` is unavailable and caller should fall back to `FindFirstFileExW`).
    #[allow(clippy::too_many_arguments)]
    pub fn try_read_dir_ntquery(
        dir: &Path,
        filters: &CompiledFilters,
        policy: WalkerPolicy,
        cluster_size: u64,
        is_remote: bool,
        supports_file_id: &AtomicBool,
        counters: &WalkerCounters,
    ) -> Option<(Vec<RawEntry>, bool)> {
        let nt_query = get_nt_query_directory_file()?;

        let raw = dir.to_string_lossy();
        let trimmed = raw.trim_end_matches(['\\', '/']);
        let ext_prefix = if trimmed.starts_with(r"\\?\") {
            trimmed.to_string()
        } else if let Some(unc_rest) = trimmed.strip_prefix(r"\\") {
            format!(r"\\?\UNC\{unc_rest}")
        } else {
            format!(r"\\?\{trimmed}")
        };
        // Drive roots like `\\?\C:` MUST end with `\` for CreateFileW directory handle!
        let open_path = if ext_prefix.ends_with(':') {
            format!(r"{ext_prefix}\")
        } else {
            ext_prefix.clone()
        };

        let wide_dir: Vec<u16> = OsStr::new(&open_path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let handle = unsafe {
            CreateFileW(
                wide_dir.as_ptr(),
                FILE_LIST_DIRECTORY | SYNCHRONIZE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let err = unsafe { GetLastError() };
            counters.errors.fetch_add(1, Ordering::Relaxed);
            return Some((Vec::new(), err == ERROR_ACCESS_DENIED));
        }

        let buf_bytes = if is_remote {
            REMOTE_BUFFER_BYTES
        } else {
            LOCAL_BUFFER_BYTES
        };
        let mut out = Vec::with_capacity(64);
        let mut local_files = 0u64;
        let mut local_dirs = 0u64;
        let mut local_bytes = 0u64;
        let mut access_denied = false;
        let mut first_call = true;

        DIR_BUF.with(|cell| {
            let mut buf = cell.borrow_mut();
            let req_u64 = buf_bytes / 8;
            if buf.len() < req_u64 {
                buf.resize(req_u64, 0);
            }

            loop {
                let mut iosb = IoStatusBlock {
                    status: 0,
                    information: 0,
                };
                let mut use_file_id = supports_file_id.load(Ordering::Relaxed);
                let info_class = if use_file_id {
                    FILE_ID_FULL_DIRECTORY_INFORMATION
                } else {
                    FILE_FULL_DIRECTORY_INFORMATION
                };

                let mut status = unsafe {
                    nt_query(
                        handle,
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        std::ptr::null(),
                        &mut iosb,
                        buf.as_mut_ptr().cast(),
                        buf_bytes as u32,
                        info_class,
                        0,
                        std::ptr::null(),
                        u8::from(first_call),
                    )
                };

                if use_file_id
                    && (status == STATUS_INVALID_INFO_CLASS
                        || status == STATUS_INVALID_PARAMETER
                        || status == STATUS_NOT_SUPPORTED)
                {
                    supports_file_id.store(false, Ordering::Relaxed);
                    use_file_id = false;
                    iosb.information = 0;
                    status = unsafe {
                        nt_query(
                            handle,
                            std::ptr::null_mut(),
                            std::ptr::null(),
                            std::ptr::null(),
                            &mut iosb,
                            buf.as_mut_ptr().cast(),
                            buf_bytes as u32,
                            FILE_FULL_DIRECTORY_INFORMATION,
                            0,
                            std::ptr::null(),
                            u8::from(first_call),
                        )
                    };
                }
                first_call = false;

                if status == STATUS_ACCESS_DENIED {
                    access_denied = true;
                    break;
                }
                if status == STATUS_NO_MORE_FILES || status != 0 || iosb.information == 0 {
                    break;
                }

                let raw_bytes: &[u8] = unsafe {
                    std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), iosb.information)
                };

                let mut off = 0usize;
                let min_hdr = if use_file_id { 0x50 } else { 0x44 };
                while off + min_hdr <= raw_bytes.len() {
                    let b = &raw_bytes[off..];
                    let next_off = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
                    let last_write = i64::from_le_bytes([
                        b[0x18], b[0x19], b[0x1A], b[0x1B], b[0x1C], b[0x1D], b[0x1E], b[0x1F],
                    ]) as u64;
                    let end_of_file = i64::from_le_bytes([
                        b[0x28], b[0x29], b[0x2A], b[0x2B], b[0x2C], b[0x2D], b[0x2E], b[0x2F],
                    ])
                    .max(0) as u64;
                    let alloc_raw = i64::from_le_bytes([
                        b[0x30], b[0x31], b[0x32], b[0x33], b[0x34], b[0x35], b[0x36], b[0x37],
                    ])
                    .max(0) as u64;
                    let mut attrs = u32::from_le_bytes([b[0x38], b[0x39], b[0x3A], b[0x3B]]);
                    let name_bytes =
                        u32::from_le_bytes([b[0x3C], b[0x3D], b[0x3E], b[0x3F]]) as usize;
                    let ea_or_reparse = u32::from_le_bytes([b[0x40], b[0x41], b[0x42], b[0x43]]);
                    let file_id = if use_file_id {
                        u64::from_le_bytes([
                            b[0x48], b[0x49], b[0x4A], b[0x4B], b[0x4C], b[0x4D], b[0x4E], b[0x4F],
                        ])
                    } else {
                        0
                    };

                    let name_start = min_hdr;
                    let name_end = (name_start + name_bytes).min(b.len());
                    let u16_count = (name_end - name_start) / 2;

                    if u16_count > 0 {
                        let name_u16: Vec<u16> = b[name_start..name_start + u16_count * 2]
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|&c| u16::from_le_bytes(c))
                            .collect();

                        let is_dot = (u16_count == 1 && name_u16[0] == b'.' as u16)
                            || (u16_count == 2
                                && name_u16[0] == b'.' as u16
                                && name_u16[1] == b'.' as u16);

                        if !is_dot {
                            let name = OsString::from_wide(&name_u16)
                                .to_string_lossy()
                                .into_owned();
                            let is_dir = (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0;
                            let is_reparse = (attrs & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
                            let reparse_tag = if is_reparse { ea_or_reparse } else { 0 };

                            // Exact WinDirStat FinderBasic.cpp: mark WOF-compressed files as COMPRESSED!
                            if reparse_tag == IO_REPARSE_TAG_WOF {
                                attrs |= FILE_ATTRIBUTE_COMPRESSED;
                            }

                            let is_hidden = (attrs & FILE_ATTRIBUTE_HIDDEN) != 0;
                            let is_system = (attrs & FILE_ATTRIBUTE_SYSTEM) != 0;
                            let is_cloud = (attrs
                                & (FILE_ATTRIBUTE_OFFLINE
                                    | FILE_ATTRIBUTE_RECALL_ON_OPEN
                                    | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS))
                                != 0
                                || is_cloud_reparse_tag(reparse_tag);

                            let mut is_symlink = reparse_tag == IO_REPARSE_TAG_SYMLINK;
                            let mut is_junction = false;
                            let mut is_mount_point = false;

                            if is_reparse && is_dir && reparse_tag == IO_REPARSE_TAG_MOUNT_POINT {
                                let full_ext = format!(r"{ext_prefix}\{name}");
                                if is_reparse_junction(&full_ext) {
                                    is_junction = true;
                                } else {
                                    is_mount_point = true;
                                }
                            } else if is_reparse
                                && !is_cloud
                                && reparse_tag != IO_REPARSE_TAG_WOF
                                && reparse_tag != IO_REPARSE_TAG_MOUNT_POINT
                            {
                                // Unknown non-cloud/non-WOF reparse point on a directory: treat conservatively as symlink
                                is_symlink = true;
                            }

                            // Apply WinDirStat attribute/reparse exclusions
                            let policy_excluded = (policy.exclude_hidden && is_hidden)
                                || (policy.exclude_protected_system && is_hidden && is_system)
                                || (!is_dir && is_symlink && policy.exclude_symlinks_file);

                            if !policy_excluded {
                                let mut win_attrs = WinAttrs::empty();
                                if (attrs & FILE_ATTRIBUTE_READONLY) != 0 {
                                    win_attrs.insert(WinAttrs::READONLY);
                                }
                                if is_hidden {
                                    win_attrs.insert(WinAttrs::HIDDEN);
                                }
                                if is_system {
                                    win_attrs.insert(WinAttrs::SYSTEM);
                                }
                                if (attrs & FILE_ATTRIBUTE_ARCHIVE) != 0 {
                                    win_attrs.insert(WinAttrs::ARCHIVE);
                                }
                                if (attrs & FILE_ATTRIBUTE_COMPRESSED) != 0 {
                                    win_attrs.insert(WinAttrs::COMPRESSED);
                                }
                                if (attrs & FILE_ATTRIBUTE_ENCRYPTED) != 0 {
                                    win_attrs.insert(WinAttrs::ENCRYPTED);
                                }
                                if (attrs & FILE_ATTRIBUTE_SPARSE_FILE) != 0 {
                                    win_attrs.insert(WinAttrs::SPARSE);
                                }
                                if is_cloud {
                                    win_attrs.insert(WinAttrs::OFFLINE_CLOUD);
                                }
                                if is_reparse {
                                    win_attrs.insert(WinAttrs::REPARSE);
                                }

                                let logical_size = if is_dir { 0 } else { end_of_file };
                                let mtime_secs = filetime_to_unix_secs(last_write);
                                let full_str = format!(r"{trimmed}\{name}");
                                let excluded = if is_dir {
                                    filters.is_dir_excluded(&name, &full_str)
                                } else {
                                    filters.is_file_excluded(
                                        &name,
                                        &full_str,
                                        logical_size,
                                        mtime_secs,
                                    )
                                };

                                if !excluded {
                                    // NtQueryDirectoryFile already returns exact physical AllocationSize!
                                    let allocated_size = if is_dir || is_cloud {
                                        0
                                    } else if alloc_raw > 0
                                        || logical_size == 0
                                        || win_attrs.intersects(WinAttrs::COMPRESSED | WinAttrs::SPARSE)
                                    {
                                        alloc_raw
                                    } else {
                                        align_to_cluster(logical_size, cluster_size)
                                    };

                                    if is_dir {
                                        local_dirs += 1;
                                    } else {
                                        local_files += 1;
                                        local_bytes += logical_size;
                                    }

                                    out.push(RawEntry {
                                        name,
                                        is_dir,
                                        is_symlink,
                                        is_junction,
                                        is_mount_point,
                                        is_hidden,
                                        win_attrs,
                                        logical_size,
                                        allocated_size,
                                        file_id: if is_dir { 0 } else { file_id },
                                        mtime_secs,
                                    });
                                }
                            }
                        }
                    }

                    if next_off == 0 {
                        break;
                    }
                    off += next_off;
                }
            }
        });

        unsafe {
            CloseHandle(handle);
        }

        if local_files > 0 {
            counters.files_scanned.fetch_add(local_files, Ordering::Relaxed);
        }
        if local_dirs > 0 {
            counters.dirs_scanned.fetch_add(local_dirs, Ordering::Relaxed);
        }
        if local_bytes > 0 {
            counters.bytes_scanned.fetch_add(local_bytes, Ordering::Relaxed);
        }

        Some((out, access_denied))
    }
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn read_dir_fast(
    dir: &Path,
    filters: &CompiledFilters,
    policy: WalkerPolicy,
    cluster_size: u64,
    is_remote: bool,
    supports_file_id: &AtomicBool,
    counters: &WalkerCounters,
) -> (Vec<RawEntry>, bool) {
    if let Some(res) = nt_query::try_read_dir_ntquery(
        dir,
        filters,
        policy,
        cluster_size,
        is_remote,
        supports_file_id,
        counters,
    ) {
        return res;
    }

    // Fallback to FindFirstFileExW if ntdll!NtQueryDirectoryFile is unavailable
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ACCESS_DENIED, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
        FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_ENCRYPTED, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_OFFLINE,
        FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SPARSE_FILE,
        FILE_ATTRIBUTE_SYSTEM, FIND_FIRST_EX_LARGE_FETCH, WIN32_FIND_DATAW,
    };

    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

    let raw = dir.to_string_lossy();
    let trimmed = raw.trim_end_matches(['\\', '/']);
    let ext_prefix = if trimmed.starts_with(r"\\?\") {
        trimmed.to_string()
    } else if let Some(unc_rest) = trimmed.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{unc_rest}")
    } else {
        format!(r"\\?\{trimmed}")
    };
    let pattern = format!(r"{ext_prefix}\*");

    let wide_pattern: Vec<u16> = OsStr::new(&pattern)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let handle = unsafe {
        FindFirstFileExW(
            wide_pattern.as_ptr(),
            FindExInfoBasic,
            (&mut data as *mut WIN32_FIND_DATAW).cast(),
            FindExSearchNameMatch,
            std::ptr::null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        let err = unsafe { GetLastError() };
        counters.errors.fetch_add(1, Ordering::Relaxed);
        return (Vec::new(), err == ERROR_ACCESS_DENIED);
    }

    let mut out = Vec::with_capacity(32);
    let mut local_files = 0u64;
    let mut local_dirs = 0u64;
    let mut local_bytes = 0u64;

    loop {
        let name_len = data
            .cFileName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(data.cFileName.len());
        let name_slice = &data.cFileName[..name_len];
        let is_dot = (name_len == 1 && name_slice[0] == b'.' as u16)
            || (name_len == 2 && name_slice[0] == b'.' as u16 && name_slice[1] == b'.' as u16);

        if !is_dot && name_len > 0 {
            let name = OsString::from_wide(name_slice)
                .to_string_lossy()
                .into_owned();
            let attrs = data.dwFileAttributes;
            let is_dir = (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0;
            let is_symlink = (attrs & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
            let is_hidden = (attrs & FILE_ATTRIBUTE_HIDDEN) != 0;
            let is_system = (attrs & FILE_ATTRIBUTE_SYSTEM) != 0;

            let policy_excluded = (policy.exclude_hidden && is_hidden)
                || (policy.exclude_protected_system && is_hidden && is_system)
                || (!is_dir && is_symlink && policy.exclude_symlinks_file);

            if !policy_excluded {
                let mut win_attrs = WinAttrs::empty();
                if (attrs & FILE_ATTRIBUTE_READONLY) != 0 {
                    win_attrs.insert(WinAttrs::READONLY);
                }
                if is_hidden {
                    win_attrs.insert(WinAttrs::HIDDEN);
                }
                if is_system {
                    win_attrs.insert(WinAttrs::SYSTEM);
                }
                if (attrs & FILE_ATTRIBUTE_ARCHIVE) != 0 {
                    win_attrs.insert(WinAttrs::ARCHIVE);
                }
                if (attrs & FILE_ATTRIBUTE_COMPRESSED) != 0 {
                    win_attrs.insert(WinAttrs::COMPRESSED);
                }
                if (attrs & FILE_ATTRIBUTE_ENCRYPTED) != 0 {
                    win_attrs.insert(WinAttrs::ENCRYPTED);
                }
                if (attrs & FILE_ATTRIBUTE_SPARSE_FILE) != 0 {
                    win_attrs.insert(WinAttrs::SPARSE);
                }
                if (attrs
                    & (FILE_ATTRIBUTE_OFFLINE
                        | FILE_ATTRIBUTE_RECALL_ON_OPEN
                        | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS))
                    != 0
                {
                    win_attrs.insert(WinAttrs::OFFLINE_CLOUD);
                }
                if is_symlink {
                    win_attrs.insert(WinAttrs::REPARSE);
                }

                let logical_size = if is_dir {
                    0
                } else {
                    ((data.nFileSizeHigh as u64) << 32) | (data.nFileSizeLow as u64)
                };
                let ft = ((data.ftLastWriteTime.dwHighDateTime as u64) << 32)
                    | (data.ftLastWriteTime.dwLowDateTime as u64);
                let mtime_secs = filetime_to_unix_secs(ft);

                let full_str = format!(r"{trimmed}\{name}");
                let excluded = if is_dir {
                    filters.is_dir_excluded(&name, &full_str)
                } else {
                    filters.is_file_excluded(&name, &full_str, logical_size, mtime_secs)
                };

                if !excluded {
                    let allocated_size = if is_dir || win_attrs.contains(WinAttrs::OFFLINE_CLOUD) {
                        0
                    } else {
                        align_to_cluster(logical_size, cluster_size)
                    };
                    if is_dir {
                        local_dirs += 1;
                    } else {
                        local_files += 1;
                        local_bytes += logical_size;
                    }
                    out.push(RawEntry {
                        name,
                        is_dir,
                        is_symlink,
                        is_junction: false,
                        is_mount_point: false,
                        is_hidden,
                        win_attrs,
                        logical_size,
                        allocated_size,
                        file_id: 0,
                        mtime_secs,
                    });
                }
            }
        }

        if unsafe { FindNextFileW(handle, &mut data) == 0 } {
            break;
        }
    }

    unsafe {
        FindClose(handle);
    }

    if local_files > 0 {
        counters.files_scanned.fetch_add(local_files, Ordering::Relaxed);
    }
    if local_dirs > 0 {
        counters.dirs_scanned.fetch_add(local_dirs, Ordering::Relaxed);
    }
    if local_bytes > 0 {
        counters.bytes_scanned.fetch_add(local_bytes, Ordering::Relaxed);
    }
    (out, false)
}

#[cfg(windows)]
#[inline]
pub(crate) fn filetime_to_unix_secs(ft: u64) -> i64 {
    const WINDOWS_TICK: u64 = 10_000_000;
    const SEC_TO_UNIX_EPOCH: u64 = 11_644_473_600;
    let secs = ft / WINDOWS_TICK;
    if secs >= SEC_TO_UNIX_EPOCH {
        (secs - SEC_TO_UNIX_EPOCH) as i64
    } else {
        0
    }
}

#[cfg(not(windows))]
#[allow(clippy::too_many_arguments)]
fn read_dir_fast(
    dir: &Path,
    filters: &CompiledFilters,
    policy: WalkerPolicy,
    cluster_size: u64,
    _is_remote: bool,
    _supports_file_id: &AtomicBool,
    counters: &WalkerCounters,
) -> (Vec<RawEntry>, bool) {
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            counters.errors.fetch_add(1, Ordering::Relaxed);
            return (Vec::new(), e.kind() == std::io::ErrorKind::PermissionDenied);
        }
    };
    let mut out = Vec::new();
    let mut local_files = 0u64;
    let mut local_dirs = 0u64;
    let mut local_bytes = 0u64;

    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else {
            counters.errors.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let is_symlink = entry
            .file_type()
            .map(|ft| ft.is_symlink())
            .unwrap_or(false);
        let is_dir = meta.is_dir();
        let is_hidden = name.starts_with('.');
        if (policy.exclude_hidden && is_hidden)
            || (!is_dir && is_symlink && policy.exclude_symlinks_file)
        {
            continue;
        }
        let logical_size = if is_dir { 0 } else { meta.len() };
        let mtime_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let full_str = dir.join(&name).to_string_lossy().into_owned();

        let excluded = if is_dir {
            filters.is_dir_excluded(&name, &full_str)
        } else {
            filters.is_file_excluded(&name, &full_str, logical_size, mtime_secs)
        };
        if excluded {
            continue;
        }
        let allocated_size = if is_dir {
            0
        } else {
            align_to_cluster(logical_size, cluster_size)
        };

        let mut win_attrs = WinAttrs::empty();
        if is_hidden {
            win_attrs.insert(WinAttrs::HIDDEN);
        }
        if is_symlink {
            win_attrs.insert(WinAttrs::REPARSE);
        }

        if is_dir {
            local_dirs += 1;
        } else {
            local_files += 1;
            local_bytes += logical_size;
        }
        out.push(RawEntry {
            name,
            is_dir,
            is_symlink,
            is_junction: false,
            is_mount_point: false,
            is_hidden,
            win_attrs,
            logical_size,
            allocated_size,
            file_id: 0,
            mtime_secs,
        });
    }

    counters.files_scanned.fetch_add(local_files, Ordering::Relaxed);
    counters.dirs_scanned.fetch_add(local_dirs, Ordering::Relaxed);
    counters.bytes_scanned.fetch_add(local_bytes, Ordering::Relaxed);
    (out, false)
}
