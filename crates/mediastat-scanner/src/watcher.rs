//! Live real-time File System Watcher (WinDirStat 2.10 `FileWatcherControl` parity).
//!
//! Uses Win32 `ReadDirectoryChangesW` on dedicated background threads per watched directory/drive,
//! streaming real-time file creation, modification, deletion, and rename events over a channel.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::{bounded, Receiver, Sender};
use mediastat_core::t;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchAction {
    Created,
    Deleted,
    Modified,
    RenamedOld,
    RenamedNew,
}

impl WatchAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Created => t("Creado", "Created"),
            Self::Deleted => t("Eliminado", "Deleted"),
            Self::Modified => t("Modificado", "Modified"),
            Self::RenamedOld => t("Renombrado (ant.)", "Renamed (old)"),
            Self::RenamedNew => t("Renombrado (nuevo)", "Renamed (new)"),
        }
    }

    pub fn color_rgb(self) -> [u8; 3] {
        match self {
            Self::Created => [76, 210, 128],
            Self::Deleted => [235, 87, 87],
            Self::Modified => [242, 180, 74],
            Self::RenamedOld | Self::RenamedNew => [110, 175, 245],
        }
    }
}

#[derive(Debug, Clone)]
pub struct WatchEvent {
    pub timestamp_unix: i64,
    pub action: WatchAction,
    pub path: PathBuf,
    pub file_size: u64,
}

/// Active multi-root real-time filesystem watcher handle.
pub struct FileWatcher {
    stop_flag: Arc<AtomicBool>,
    pub rx: Receiver<WatchEvent>,
    handles: Vec<JoinHandle<()>>,
    pub watched_paths: Vec<PathBuf>,
}

impl FileWatcher {
    pub fn start(paths: Vec<PathBuf>) -> Self {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let (tx, rx) = bounded::<WatchEvent>(4096);
        let mut handles = Vec::new();

        for root in &paths {
            let dir = root.clone();
            let stop = Arc::clone(&stop_flag);
            let event_tx = tx.clone();
            handles.push(std::thread::spawn(move || {
                watch_loop(&dir, &stop, &event_tx);
            }));
        }

        Self {
            stop_flag,
            rx,
            handles,
            watched_paths: paths,
        }
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        // Threads unblock via async overlapped poll timeout (250ms) and exit cleanly
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }

    pub fn is_running(&self) -> bool {
        !self.stop_flag.load(Ordering::Relaxed) && !self.handles.is_empty()
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

#[cfg(windows)]
fn watch_loop(root: &PathBuf, stop: &Arc<AtomicBool>, tx: &Sender<WatchEvent>) {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_IO_PENDING, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadDirectoryChangesW, FILE_ACTION_ADDED, FILE_ACTION_MODIFIED,
        FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
        FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
        FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

    extern "system" {
        fn CreateEventW(
            attrs: *const std::ffi::c_void,
            manual_reset: i32,
            initial_state: i32,
            name: *const u16,
        ) -> windows_sys::Win32::Foundation::HANDLE;
        fn WaitForSingleObject(
            handle: windows_sys::Win32::Foundation::HANDLE,
            ms: u32,
        ) -> u32;
        fn ResetEvent(handle: windows_sys::Win32::Foundation::HANDLE) -> i32;
    }

    let wide: Vec<u16> = OsStr::new(root)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let dir_handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            std::ptr::null_mut(),
        )
    };
    if dir_handle == INVALID_HANDLE_VALUE {
        return;
    }

    let event_handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event_handle.is_null() {
        unsafe {
            CloseHandle(dir_handle);
        }
        return;
    }

    let mut buffer = vec![0u32; 16 * 1024]; // 64 KB 4-byte aligned buffer
    let notify_filter = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_DIR_NAME
        | FILE_NOTIFY_CHANGE_SIZE
        | FILE_NOTIFY_CHANGE_LAST_WRITE;

    while !stop.load(Ordering::Relaxed) {
        unsafe {
            ResetEvent(event_handle);
        }
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event_handle;
        let mut bytes_returned = 0u32;

        let ok = unsafe {
            ReadDirectoryChangesW(
                dir_handle,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 4) as u32,
                1, // bWatchSubtree = true
                notify_filter,
                &mut bytes_returned,
                &mut overlapped,
                None,
            ) != 0
        };
        if !ok && unsafe { GetLastError() } != ERROR_IO_PENDING {
            break;
        }

        // Wait in 200ms slices so Stop is instant & non-blocking
        let mut completed = false;
        while !stop.load(Ordering::Relaxed) {
            let wait = unsafe { WaitForSingleObject(event_handle, 200) };
            if wait == WAIT_OBJECT_0 {
                completed = true;
                break;
            }
        }

        if !completed {
            unsafe {
                CancelIoEx(dir_handle, &overlapped);
            }
            break;
        }

        let mut transferred = 0u32;
        if unsafe { GetOverlappedResult(dir_handle, &overlapped, &mut transferred, 0) == 0 }
            || transferred == 0
        {
            continue;
        }

        let byte_slice =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr() as *const u8, transferred as usize) };
        let now = chrono::Utc::now().timestamp();
        let mut offset = 0usize;

        while offset + std::mem::size_of::<FILE_NOTIFY_INFORMATION>() <= byte_slice.len() {
            let info = unsafe { &*(byte_slice.as_ptr().add(offset) as *const FILE_NOTIFY_INFORMATION) };
            let name_bytes = info.FileNameLength as usize;
            let name_start = offset + 12;
            if name_start + name_bytes <= byte_slice.len() {
                let utf16: Vec<u16> = byte_slice[name_start..name_start + name_bytes]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let rel = OsString::from_wide(&utf16);
                let full_path = root.join(rel);
                let action = match info.Action {
                    FILE_ACTION_ADDED => WatchAction::Created,
                    FILE_ACTION_REMOVED => WatchAction::Deleted,
                    FILE_ACTION_MODIFIED => WatchAction::Modified,
                    FILE_ACTION_RENAMED_OLD_NAME => WatchAction::RenamedOld,
                    FILE_ACTION_RENAMED_NEW_NAME => WatchAction::RenamedNew,
                    _ => WatchAction::Modified,
                };
                let file_size = if action != WatchAction::Deleted {
                    std::fs::metadata(&full_path).map(|m| m.len()).unwrap_or(0)
                } else {
                    0
                };
                let _ = tx.try_send(WatchEvent {
                    timestamp_unix: now,
                    action,
                    path: full_path,
                    file_size,
                });
            }
            if info.NextEntryOffset == 0 {
                break;
            }
            offset += info.NextEntryOffset as usize;
        }
    }

    unsafe {
        CloseHandle(event_handle);
        CloseHandle(dir_handle);
    }
}

#[cfg(not(windows))]
fn watch_loop(_root: &PathBuf, stop: &Arc<AtomicBool>, _tx: &Sender<WatchEvent>) {
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}
