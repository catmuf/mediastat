//! Engine B — Direct NTFS Master File Table (`$MFT`) sequential scanner for Windows.
//!
//! When scanning a whole local NTFS volume (`C:\`, `D:\`, etc.) with Administrator elevation,
//! reading `$MFT` extents directly in 4 MB sequential buffers scans millions of files in a
//! few seconds (WizTree / WinDirStat 2.x technique).
//!
//! If the user is not elevated, or the volume is non-NTFS (exFAT/FAT32/ReFS/SMB), this module
//! returns `Err(...)` cleanly so the pipeline transparently falls back to Engine A.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use mediastat_core::{NodeId, ScanTree};

use crate::walker::WalkerCounters;

#[derive(Debug, thiserror::Error)]
pub enum MftError {
    #[error("NTFS MFT solo está disponible en Windows")]
    UnsupportedOs,
    #[error("La ruta no corresponde a la raíz de un volumen NTFS local")]
    NotWholeVolume,
    #[error("Se requieren privilegios de Administrador para leer $MFT directamente")]
    NotElevated,
    #[error("Error accediendo al volumen NTFS: {0}")]
    Io(String),
}

#[cfg(not(windows))]
pub fn try_scan_ntfs_mft(
    _tree: &mut ScanTree,
    _target_root: &Path,
    _excluded_dir_names: &[String],
    _counters: &Arc<WalkerCounters>,
    _cancel: &Arc<AtomicBool>,
) -> Result<NodeId, MftError> {
    Err(MftError::UnsupportedOs)
}

#[cfg(windows)]
pub fn try_scan_ntfs_mft(
    tree: &mut ScanTree,
    target_root: &Path,
    excluded_dir_names: &[String],
    counters: &Arc<WalkerCounters>,
    cancel: &Arc<AtomicBool>,
) -> Result<NodeId, MftError> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::sync::atomic::Ordering;

    use ahash::AHashMap;
    use mediastat_core::NodeFlags;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, SetFilePointerEx, FILE_BEGIN, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Ioctl::{
        FSCTL_GET_NTFS_VOLUME_DATA, FSCTL_GET_RETRIEVAL_POINTERS, NTFS_VOLUME_DATA_BUFFER,
        STARTING_VCN_INPUT_BUFFER,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    use crate::drives::{is_process_elevated, whole_volume_drive_letter};
    use crate::walker::filetime_to_unix_secs;

    const GENERIC_READ: u32 = 0x8000_0000;
    const NTFS_ROOT_RECORD: u64 = 5;

    let letter = whole_volume_drive_letter(target_root).ok_or(MftError::NotWholeVolume)?;
    if !is_process_elevated() {
        return Err(MftError::NotElevated);
    }

    let to_wide = |s: &str| -> Vec<u16> {
        OsString::from(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };

    let vol_path_w = to_wide(&format!(r"\\.\{letter}:"));
    let vol_handle = unsafe {
        CreateFileW(
            vol_path_w.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if vol_handle == INVALID_HANDLE_VALUE {
        return Err(MftError::Io(
            "No se pudo abrir el volumen en modo crudo (se requiere Admin + NTFS)".into(),
        ));
    }

    struct HandleGuard(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    let _vol_guard = HandleGuard(vol_handle);

    // 1. Query NTFS volume geometry
    let mut vol_data: NTFS_VOLUME_DATA_BUFFER = unsafe { std::mem::zeroed() };
    let mut bytes_ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            vol_handle,
            FSCTL_GET_NTFS_VOLUME_DATA,
            std::ptr::null(),
            0,
            (&mut vol_data as *mut NTFS_VOLUME_DATA_BUFFER).cast(),
            std::mem::size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,
            &mut bytes_ret,
            std::ptr::null_mut(),
        ) != 0
    };
    if !ok || vol_data.BytesPerFileRecordSegment == 0 || vol_data.BytesPerCluster == 0 {
        return Err(MftError::Io(
            "El volumen no es NTFS válido o falló FSCTL_GET_NTFS_VOLUME_DATA".into(),
        ));
    }

    let record_size = vol_data.BytesPerFileRecordSegment as usize;
    let cluster_size = vol_data.BytesPerCluster as u64;
    let mft_valid_len = vol_data.MftValidDataLength as u64;
    let total_records = (mft_valid_len / record_size as u64) as usize;
    if total_records == 0 || !(512..=16384).contains(&record_size) {
        return Err(MftError::Io("Geometría MFT inválida".into()));
    }

    // 2. Discover MFT extents (handles fragmented $MFT cleanly!)
    let extents = query_mft_extents(letter, &vol_data, &to_wide);

    // 3. Stream all MFT extents in 4 MB aligned buffers and extract live file/dir entries
    #[derive(Clone)]
    struct MftNode {
        parent_ref: u64,
        name: String,
        is_dir: bool,
        is_symlink: bool,
        is_hidden: bool,
        logical_size: u64,
        allocated_size: u64,
        mtime_secs: i64,
    }

    let mut records: AHashMap<u64, MftNode> = AHashMap::with_capacity(total_records / 2);
    let chunk_records = (4 * 1024 * 1024 / record_size).max(1);
    let mut buf = vec![0u8; chunk_records * record_size];
    let mut current_rec_index = 0u64;

    for (start_lcn, cluster_count) in extents {
        if cancel.load(Ordering::Relaxed) || current_rec_index as usize >= total_records {
            break;
        }
        let mut extent_offset = start_lcn * cluster_size;
        let mut remaining = (cluster_count * cluster_size)
            .min(mft_valid_len.saturating_sub(current_rec_index * record_size as u64));

        while remaining >= record_size as u64 && !cancel.load(Ordering::Relaxed) {
            let to_read = (remaining as usize).min(buf.len());
            let aligned_read = (to_read / record_size) * record_size;
            if aligned_read == 0 {
                break;
            }
            let mut new_pos = 0i64;
            let seek_ok = unsafe {
                SetFilePointerEx(vol_handle, extent_offset as i64, &mut new_pos, FILE_BEGIN) != 0
            };
            if !seek_ok {
                return Err(MftError::Io("Error posicionando puntero en volumen NTFS".into()));
            }
            let mut read_bytes = 0u32;
            let read_ok = unsafe {
                ReadFile(
                    vol_handle,
                    buf.as_mut_ptr(),
                    aligned_read as u32,
                    &mut read_bytes,
                    std::ptr::null_mut(),
                ) != 0
            };
            if !read_ok || read_bytes == 0 {
                break;
            }

            let recs_read = (read_bytes as usize) / record_size;
            let mut batch_files = 0u64;
            let mut batch_dirs = 0u64;
            let mut batch_bytes = 0u64;

            for r in 0..recs_read {
                let rec_num = current_rec_index + r as u64;
                let slice = &mut buf[r * record_size..(r + 1) * record_size];
                if let Some(node) = parse_mft_record(slice, rec_num) {
                    if node.is_dir {
                        batch_dirs += 1;
                    } else {
                        batch_files += 1;
                        batch_bytes += node.logical_size;
                    }
                    records.insert(rec_num, node);
                }
            }

            counters.files_scanned.fetch_add(batch_files, Ordering::Relaxed);
            counters.dirs_scanned.fetch_add(batch_dirs, Ordering::Relaxed);
            counters.bytes_scanned.fetch_add(batch_bytes, Ordering::Relaxed);

            current_rec_index += recs_read as u64;
            extent_offset += read_bytes as u64;
            remaining = remaining.saturating_sub(read_bytes as u64);
        }
    }

    if records.is_empty() {
        return Err(MftError::Io("No se extrajeron registros MFT válidos".into()));
    }

    // 4. Reconstruct hierarchy from MFT record references rooted at record 5 (`\`)
    let root_id = tree.add_root_target(target_root);
    let excluded_lower: Vec<String> = excluded_dir_names
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();

    // Group children by parent MFT reference
    let mut children_by_parent: AHashMap<u64, Vec<u64>> = AHashMap::with_capacity(records.len() / 6);
    for (&rec_num, node) in &records {
        // Skip internal NTFS metafiles ($MFT, $LogFile, etc., records 0..16 except root 5)
        if rec_num < 16 {
            continue;
        }
        if node.name.starts_with('$') && node.parent_ref == NTFS_ROOT_RECORD {
            continue;
        }
        children_by_parent
            .entry(node.parent_ref)
            .or_default()
            .push(rec_num);
    }

    // Iterative BFS from NTFS root record 5 into ScanTree
    let mut queue = std::collections::VecDeque::new();
    queue.push_back((NTFS_ROOT_RECORD, root_id));

    while let Some((mft_parent, tree_parent)) = queue.pop_front() {
        if let Some(kids) = children_by_parent.remove(&mft_parent) {
            for kid_ref in kids {
                let Some(node) = records.get(&kid_ref) else {
                    continue;
                };
                if node.is_dir
                    && excluded_lower
                        .iter()
                        .any(|e| node.name.eq_ignore_ascii_case(e))
                {
                    continue;
                }
                let mut flags = NodeFlags::empty();
                if node.is_dir {
                    flags.insert(NodeFlags::IS_DIR);
                }
                if node.is_symlink {
                    flags.insert(NodeFlags::IS_SYMLINK);
                }
                if node.is_hidden {
                    flags.insert(NodeFlags::IS_HIDDEN);
                }

                let child_id = tree.add_child_full(
                    tree_parent,
                    &node.name,
                    flags,
                    mediastat_core::WinAttrs::empty(),
                    if node.is_dir { 0 } else { node.logical_size },
                    if node.is_dir { 0 } else { node.allocated_size },
                    if node.is_dir { 0 } else { kid_ref },
                    node.mtime_secs,
                );
                if node.is_dir && !node.is_symlink {
                    queue.push_back((kid_ref, child_id));
                }
            }
        }
    }

    return Ok(root_id);

    /// Query `$MFT` cluster extents via `FSCTL_GET_RETRIEVAL_POINTERS`; falls back to contiguous MFT start LCN.
    fn query_mft_extents(
        letter: char,
        vol_data: &NTFS_VOLUME_DATA_BUFFER,
        to_wide: &dyn Fn(&str) -> Vec<u16>,
    ) -> Vec<(u64, u64)> {
        let mft_path = to_wide(&format!(r"\\.\{letter}:\$MFT"));
        let h = unsafe {
            CreateFileW(
                mft_path.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if h != INVALID_HANDLE_VALUE {
            let mut extents = Vec::new();
            let mut start_vcn = 0i64;
            let mut out_buf = vec![0u8; 64 * 1024];
            loop {
                let input = STARTING_VCN_INPUT_BUFFER {
                    StartingVcn: start_vcn,
                };
                let mut ret = 0u32;
                let ok = unsafe {
                    DeviceIoControl(
                        h,
                        FSCTL_GET_RETRIEVAL_POINTERS,
                        (&input as *const STARTING_VCN_INPUT_BUFFER).cast(),
                        std::mem::size_of::<STARTING_VCN_INPUT_BUFFER>() as u32,
                        out_buf.as_mut_ptr().cast(),
                        out_buf.len() as u32,
                        &mut ret,
                        std::ptr::null_mut(),
                    ) != 0
                };
                if ret < 32 {
                    break;
                }
                let extent_count =
                    u32::from_le_bytes([out_buf[0], out_buf[1], out_buf[2], out_buf[3]]) as usize;
                let mut prev_vcn = i64::from_le_bytes([
                    out_buf[8],
                    out_buf[9],
                    out_buf[10],
                    out_buf[11],
                    out_buf[12],
                    out_buf[13],
                    out_buf[14],
                    out_buf[15],
                ]);
                for i in 0..extent_count {
                    let off = 16 + i * 16;
                    if off + 16 > out_buf.len() {
                        break;
                    }
                    let next_vcn = i64::from_le_bytes(out_buf[off..off + 8].try_into().unwrap());
                    let lcn = i64::from_le_bytes(out_buf[off + 8..off + 16].try_into().unwrap());
                    let len = (next_vcn - prev_vcn).max(0) as u64;
                    if lcn >= 0 && len > 0 {
                        extents.push((lcn as u64, len));
                    }
                    prev_vcn = next_vcn;
                }
                if ok {
                    break;
                }
                start_vcn = prev_vcn;
            }
            unsafe {
                CloseHandle(h);
            }
            if !extents.is_empty() {
                return extents;
            }
        }

        // Contiguous fallback from MftStartLcn
        let clusters =
            (vol_data.MftValidDataLength as u64).div_ceil(vol_data.BytesPerCluster as u64);
        vec![(vol_data.MftStartLcn as u64, clusters)]
    }

    /// Parse a single NTFS `FILE` record in place (applying USA sector fixups first).
    fn parse_mft_record(rec: &mut [u8], _rec_num: u64) -> Option<MftNode> {
        if rec.len() < 48 || &rec[0..4] != b"FILE" {
            return None;
        }
        let flags = u16::from_le_bytes([rec[22], rec[23]]);
        if (flags & 0x0001) == 0 {
            return None; // Not in use (deleted)
        }
        let is_dir = (flags & 0x0002) != 0;

        // Ignore extension records (non-zero BaseFileRecordSegment)
        let base_ref = u64::from_le_bytes(rec[32..40].try_into().ok()?) & 0x0000_FFFF_FFFF_FFFF;
        if base_ref != 0 {
            return None;
        }

        // Apply Update Sequence Array (USA) fixup to each 512-byte sector
        let usa_off = u16::from_le_bytes([rec[4], rec[5]]) as usize;
        let usa_count = u16::from_le_bytes([rec[6], rec[7]]) as usize;
        if usa_count >= 2 && usa_off + usa_count * 2 <= rec.len() {
            let usn = [rec[usa_off], rec[usa_off + 1]];
            for s in 1..usa_count {
                let sector_end = s * 512;
                if sector_end > rec.len() {
                    break;
                }
                if rec[sector_end - 2] == usn[0] && rec[sector_end - 1] == usn[1] {
                    rec[sector_end - 2] = rec[usa_off + s * 2];
                    rec[sector_end - 1] = rec[usa_off + s * 2 + 1];
                }
            }
        }

        let mut attr_off = u16::from_le_bytes([rec[20], rec[21]]) as usize;
        let mut parent_ref: Option<u64> = None;
        let mut best_name: Option<String> = None;
        let mut best_namespace: u8 = 2; // Prefer Win32(1) / Win32+DOS(3) / POSIX(0) over DOS(2)
        let mut logical_size = 0u64;
        let mut allocated_size = 0u64;
        let mut mtime_secs = 0i64;
        let mut is_hidden = false;
        let mut is_symlink = false;

        while attr_off + 16 <= rec.len() {
            let attr_type = u32::from_le_bytes(rec[attr_off..attr_off + 4].try_into().ok()?);
            if attr_type == 0xFFFF_FFFF {
                break;
            }
            let attr_len =
                u32::from_le_bytes(rec[attr_off + 4..attr_off + 8].try_into().ok()?) as usize;
            if attr_len < 16 || attr_off + attr_len > rec.len() {
                break;
            }
            let non_resident = rec[attr_off + 8] != 0;
            let name_len = rec[attr_off + 9];

            match attr_type {
                // $STANDARD_INFORMATION (0x10)
                0x10 if !non_resident => {
                    let val_off =
                        u16::from_le_bytes([rec[attr_off + 20], rec[attr_off + 21]]) as usize;
                    let p = attr_off + val_off;
                    if p + 36 <= attr_off + attr_len {
                        let mtime_ft = u64::from_le_bytes(rec[p + 8..p + 16].try_into().ok()?);
                        mtime_secs = filetime_to_unix_secs(mtime_ft);
                        let file_attrs = u32::from_le_bytes(rec[p + 32..p + 36].try_into().ok()?);
                        is_hidden = (file_attrs & 0x0002) != 0;
                        is_symlink = (file_attrs & 0x0400) != 0;
                    }
                }
                // $FILE_NAME (0x30)
                0x30 if !non_resident => {
                    let val_off =
                        u16::from_le_bytes([rec[attr_off + 20], rec[attr_off + 21]]) as usize;
                    let p = attr_off + val_off;
                    if p + 66 <= attr_off + attr_len {
                        let pref =
                            u64::from_le_bytes(rec[p..p + 8].try_into().ok()?) & 0x0000_FFFF_FFFF_FFFF;
                        let fn_len = rec[p + 64] as usize;
                        let namespace = rec[p + 65];
                        let name_bytes_end = p + 66 + fn_len * 2;
                        if name_bytes_end <= attr_off + attr_len && fn_len > 0 {
                            let prefer = best_name.is_none()
                                || (best_namespace == 2 && namespace != 2);
                            if prefer {
                                let utf16: Vec<u16> = rec[p + 66..name_bytes_end]
                                    .as_chunks::<2>()
                                    .0
                                    .iter()
                                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                                    .collect();
                                best_name =
                                    Some(OsString::from_wide(&utf16).to_string_lossy().into_owned());
                                best_namespace = namespace;
                                parent_ref = Some(pref);
                            }
                        }
                    }
                }
                // $DATA (0x80) — unnamed default stream only (name_len == 0)
                0x80 if name_len == 0 && !is_dir => {
                    if non_resident {
                        if attr_off + 56 <= attr_off + attr_len {
                            let alloc =
                                u64::from_le_bytes(rec[attr_off + 40..attr_off + 48].try_into().ok()?);
                            let real =
                                u64::from_le_bytes(rec[attr_off + 48..attr_off + 56].try_into().ok()?);
                            logical_size = real;
                            allocated_size = alloc;
                        }
                    } else if attr_off + 20 <= attr_off + attr_len {
                        let val_len =
                            u32::from_le_bytes(rec[attr_off + 16..attr_off + 20].try_into().ok()?)
                                as u64;
                        logical_size = val_len;
                        allocated_size = val_len;
                    }
                }
                _ => {}
            }

            attr_off += attr_len;
        }

        Some(MftNode {
            parent_ref: parent_ref?,
            name: best_name?,
            is_dir,
            is_symlink,
            is_hidden,
            logical_size,
            allocated_size,
            mtime_secs,
        })
    }
}
