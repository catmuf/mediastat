use std::path::{Path, PathBuf};

use mediastat_core::{t, VolumeSpaceInfo};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriveMediaType {
    SsdOrNvme,
    HddRotational,
    NetworkShare,
    Removable,
    Unknown,
}

impl DriveMediaType {
    pub fn label(self) -> &'static str {
        match self {
            Self::SsdOrNvme => "SSD/NVMe",
            Self::HddRotational => "HDD",
            Self::NetworkShare => t("Red (SMB)", "Network (SMB)"),
            Self::Removable => t("Extraíble", "Removable"),
            Self::Unknown => t("Disco", "Drive"),
        }
    }

    pub fn recommended_threads(self) -> usize {
        let cores = num_cpus::get().max(2);
        match self {
            Self::SsdOrNvme => (cores * 2).clamp(4, 32),
            Self::HddRotational => 2,
            Self::NetworkShare => 6,
            Self::Removable => 4,
            Self::Unknown => cores.clamp(4, 16),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveInfo {
    pub path: PathBuf,
    pub label: String,
    pub filesystem: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub media_type: DriveMediaType,
    pub supports_ntfs_mft: bool,
    /// Real UNC remote path for mapped network drives (e.g. `\\fileserver\Proyectos` for `P:\`).
    pub unc_remote_path: Option<String>,
}

impl DriveInfo {
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }

    pub fn used_fraction(&self) -> f32 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.used_bytes() as f64 / self.total_bytes as f64) as f32
        }
    }
}

pub fn is_process_elevated() -> bool {
    #[cfg(windows)]
    {
        windows_impl::is_elevated()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn enumerate_drives() -> Vec<DriveInfo> {
    #[cfg(windows)]
    {
        windows_impl::enumerate_drives()
    }
    #[cfg(not(windows))]
    {
        vec![DriveInfo {
            path: PathBuf::from("/"),
            label: "Root".into(),
            filesystem: "Unix".into(),
            total_bytes: 0,
            free_bytes: 0,
            media_type: DriveMediaType::Unknown,
            supports_ntfs_mft: false,
            unc_remote_path: None,
        }]
    }
}

/// Query total & free space for a target path if it is a drive/volume root or network share root.
pub fn query_volume_space(path: &Path) -> Option<VolumeSpaceInfo> {
    #[cfg(windows)]
    {
        windows_impl::query_volume_space(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

/// Query Windows Recycle Bin total item count & bytes (for WinDirStat Cleanup menu indicator).
pub fn query_recycle_bin() -> (u64, u64) {
    #[cfg(windows)]
    {
        windows_impl::query_recycle_bin()
    }
    #[cfg(not(windows))]
    {
        (0, 0)
    }
}

/// Resolve a mapped network drive path (e.g. `P:\Obra\foto.bmp`) to its real UNC path (`\\server\share\Obra\foto.bmp`).
/// Returns the original path string unchanged for local drives or paths that are already UNC.
pub fn resolve_to_unc_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    if stripped.starts_with(r"\\") {
        return stripped.to_string();
    }
    #[cfg(windows)]
    {
        let bytes = stripped.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            let letter = (bytes[0] as char).to_ascii_uppercase();
            if let Some(remote) = windows_impl::resolve_mapped_drive_unc(letter) {
                let remote_clean = remote.trim_end_matches(['\\', '/']);
                let rest = stripped[2..].trim_start_matches(['\\', '/']);
                if rest.is_empty() {
                    return remote_clean.to_string();
                }
                return format!("{remote_clean}\\{rest}");
            }
        }
    }
    stripped.to_string()
}

pub fn detect_path_media_type(path: &Path) -> DriveMediaType {
    let s = path.to_string_lossy();
    if s.starts_with(r"\\") {
        return DriveMediaType::NetworkShare;
    }
    #[cfg(windows)]
    {
        if let Some(letter) = extract_drive_letter(path) {
            return windows_impl::detect_drive_media_type(letter);
        }
    }
    DriveMediaType::Unknown
}

pub fn whole_volume_drive_letter(path: &Path) -> Option<char> {
    let s = path.to_string_lossy();
    let trimmed = s.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Some((bytes[0] as char).to_ascii_uppercase());
    }
    if bytes.len() == 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return Some((bytes[0] as char).to_ascii_uppercase());
    }
    None
}

#[cfg(windows)]
fn extract_drive_letter(path: &Path) -> Option<char> {
    let s = path.to_string_lossy();
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        Some((bytes[0] as char).to_ascii_uppercase())
    } else {
        None
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::NetworkManagement::WNet::WNetGetConnectionW;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Ioctl::{
        PropertyStandardQuery, StorageDeviceSeekPenaltyProperty, DEVICE_SEEK_PENALTY_DESCRIPTOR,
        IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_PROPERTY_QUERY,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::UI::Shell::{SHQueryRecycleBinW, SHQUERYRBINFO};

    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;

    extern "system" {
        fn GetCurrentProcess() -> HANDLE;
        fn OpenProcessToken(
            process_handle: HANDLE,
            desired_access: u32,
            token_handle: *mut HANDLE,
        ) -> i32;
    }

    pub fn is_elevated() -> bool {
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut elev = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut ret_len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                (&mut elev as *mut TOKEN_ELEVATION).cast(),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut ret_len,
            );
            CloseHandle(token);
            ok != 0 && elev.TokenIsElevated != 0
        }
    }

    fn to_wide(s: &str) -> Vec<u16> {
        OsString::from(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn from_wide_nul(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        OsString::from_wide(&buf[..len])
            .to_string_lossy()
            .into_owned()
    }

    pub fn resolve_mapped_drive_unc(letter: char) -> Option<String> {
        let local_w = to_wide(&format!("{letter}:"));
        let mut remote_buf = vec![0u16; 512];
        let mut len = remote_buf.len() as u32;
        let rc = unsafe { WNetGetConnectionW(local_w.as_ptr(), remote_buf.as_mut_ptr(), &mut len) };
        if rc == 0 {
            let s = from_wide_nul(&remote_buf);
            if !s.is_empty() {
                return Some(s);
            }
        }
        None
    }

    pub fn query_volume_space(path: &Path) -> Option<VolumeSpaceInfo> {
        let is_whole_drive = whole_volume_drive_letter(path);
        let s = path.to_string_lossy();
        let is_unc_root = s.starts_with(r"\\") && s.trim_end_matches('\\').matches('\\').count() == 3;
        if is_whole_drive.is_none() && !is_unc_root {
            return None;
        }
        let wide = to_wide(&s);
        let mut free_avail = 0u64;
        let mut total_bytes = 0u64;
        let mut total_free = 0u64;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_avail,
                &mut total_bytes,
                &mut total_free,
            ) != 0
        };
        if !ok || total_bytes == 0 {
            return None;
        }
        let unc_remote_path = is_whole_drive.and_then(resolve_mapped_drive_unc);
        Some(VolumeSpaceInfo {
            total_bytes,
            free_bytes: total_free,
            unc_remote_path,
        })
    }

    pub fn query_recycle_bin() -> (u64, u64) {
        let mut info = SHQUERYRBINFO {
            cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
            i64Size: 0,
            i64NumItems: 0,
        };
        let hr = unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut info) };
        if hr == 0 {
            (info.i64NumItems.max(0) as u64, info.i64Size.max(0) as u64)
        } else {
            (0, 0)
        }
    }

    pub fn detect_drive_media_type(letter: char) -> DriveMediaType {
        let root_w = to_wide(&format!("{letter}:\\"));
        let dtype = unsafe { GetDriveTypeW(root_w.as_ptr()) };
        match dtype {
            DRIVE_REMOTE => DriveMediaType::NetworkShare,
            DRIVE_REMOVABLE => DriveMediaType::Removable,
            DRIVE_FIXED => query_seek_penalty(letter),
            _ => DriveMediaType::Unknown,
        }
    }

    fn query_seek_penalty(letter: char) -> DriveMediaType {
        let dev_w = to_wide(&format!(r"\\.\{letter}:"));
        unsafe {
            let h = CreateFileW(
                dev_w.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if h == INVALID_HANDLE_VALUE {
                return DriveMediaType::SsdOrNvme;
            }
            let query = STORAGE_PROPERTY_QUERY {
                PropertyId: StorageDeviceSeekPenaltyProperty,
                QueryType: PropertyStandardQuery,
                AdditionalParameters: [0],
            };
            let mut desc: DEVICE_SEEK_PENALTY_DESCRIPTOR = std::mem::zeroed();
            let mut returned = 0u32;
            let ok = DeviceIoControl(
                h,
                IOCTL_STORAGE_QUERY_PROPERTY,
                (&query as *const STORAGE_PROPERTY_QUERY).cast(),
                std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                (&mut desc as *mut DEVICE_SEEK_PENALTY_DESCRIPTOR).cast(),
                std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32,
                &mut returned,
                std::ptr::null_mut(),
            );
            CloseHandle(h);
            if ok != 0 {
                if desc.IncursSeekPenalty != 0 {
                    DriveMediaType::HddRotational
                } else {
                    DriveMediaType::SsdOrNvme
                }
            } else {
                DriveMediaType::SsdOrNvme
            }
        }
    }

    pub fn enumerate_drives() -> Vec<DriveInfo> {
        let mask = unsafe { GetLogicalDrives() };
        let mut drives = Vec::new();
        for bit in 0..26u32 {
            if (mask & (1 << bit)) == 0 {
                continue;
            }
            let letter = (b'A' + bit as u8) as char;
            let root = format!("{letter}:\\");
            let root_w = to_wide(&root);
            let dtype = unsafe { GetDriveTypeW(root_w.as_ptr()) };
            if !matches!(dtype, DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_REMOTE) {
                continue;
            }

            let mut free_avail = 0u64;
            let mut total_bytes = 0u64;
            let mut total_free = 0u64;
            let space_ok = unsafe {
                GetDiskFreeSpaceExW(
                    root_w.as_ptr(),
                    &mut free_avail,
                    &mut total_bytes,
                    &mut total_free,
                ) != 0
            };
            if !space_ok && dtype == DRIVE_REMOVABLE {
                continue;
            }

            let mut vol_name = [0u16; 261];
            let mut fs_name = [0u16; 64];
            unsafe {
                GetVolumeInformationW(
                    root_w.as_ptr(),
                    vol_name.as_mut_ptr(),
                    vol_name.len() as u32,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    fs_name.as_mut_ptr(),
                    fs_name.len() as u32,
                );
            }
            let label = from_wide_nul(&vol_name);
            let filesystem = from_wide_nul(&fs_name);
            let media_type = match dtype {
                DRIVE_REMOTE => DriveMediaType::NetworkShare,
                DRIVE_REMOVABLE => DriveMediaType::Removable,
                DRIVE_FIXED => query_seek_penalty(letter),
                _ => DriveMediaType::Unknown,
            };
            let supports_ntfs_mft =
                dtype == DRIVE_FIXED && filesystem.eq_ignore_ascii_case("NTFS");
            let unc_remote_path = if dtype == DRIVE_REMOTE {
                resolve_mapped_drive_unc(letter)
            } else {
                None
            };

            drives.push(DriveInfo {
                path: PathBuf::from(&root),
                label: if label.is_empty() {
                    format!("Disco local ({letter}:)")
                } else {
                    format!("{label} ({letter}:)")
                },
                filesystem: if filesystem.is_empty() {
                    "Desconocido".into()
                } else {
                    filesystem
                },
                total_bytes,
                free_bytes: total_free,
                media_type,
                supports_ntfs_mft,
                unc_remote_path,
            });
        }
        drives
    }
}
