//! Parallel NTFS / SMB Owner & DACL Permission Inspector (WinDirStat 2.10 `FilePermsControl` parity).
//!
//! Uses a thread-safe `SID -> "DOMAIN\\user"` cache so millions of files owned by the same
//! Active Directory accounts query the Domain Controller only once per unique SID.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use ahash::AHashMap;
use mediastat_core::{NodeFlags, NodeId, PermissionEntry, ScanTree};
use parking_lot::RwLock;
use rayon::prelude::*;

pub type SidCache = RwLock<AHashMap<Vec<u8>, String>>;

/// Resolve file/folder owner (`DOMAIN\user`) for a single path (ideal for Inspector on demand).
pub fn resolve_owner_single(path: &Path) -> Option<String> {
    let cache = SidCache::default();
    resolve_owner_cached(path, &cache)
}

/// Resolve owners in parallel for the specified list of `NodeId`s and store them in `tree.owners`.
pub fn resolve_owners_for_nodes(
    tree: &mut ScanTree,
    nodes: &[NodeId],
    progress_done: Option<&Arc<AtomicU64>>,
    cancel: &Arc<AtomicBool>,
) -> usize {
    let items: Vec<(NodeId, PathBuf)> = nodes
        .iter()
        .copied()
        .filter(|&id| id.index() < tree.len() && !tree.is_deleted(id))
        .map(|id| (id, tree.resolve_path(id)))
        .collect();

    let sid_cache = SidCache::default();
    let results: Vec<(NodeId, String)> = items
        .into_par_iter()
        .filter_map(|(nid, path)| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let owner = resolve_owner_cached(&path, &sid_cache);
            if let Some(p) = progress_done {
                p.fetch_add(1, Ordering::Relaxed);
            }
            owner.map(|o| (nid, o))
        })
        .collect();

    let count = results.len();
    for (nid, owner) in results {
        tree.set_owner(nid, &owner);
    }
    count
}

/// Scan NTFS/SMB DACL permission entries across directories (and explicit non-inherited ACEs),
/// matching WinDirStat 2.10 `FilePermsControl::ScanTree`.
pub fn scan_permissions_for_scope(
    tree: &ScanTree,
    scope_root: NodeId,
    include_inherited_on_children: bool,
    max_items_to_scan: usize,
    cancel: &Arc<AtomicBool>,
) -> Vec<PermissionEntry> {
    let mut targets: Vec<(NodeId, PathBuf, bool, bool)> = Vec::new();
    let mut stack = vec![scope_root];
    let mut visited = 0usize;

    while let Some(nid) = stack.pop() {
        if visited >= max_items_to_scan || cancel.load(Ordering::Relaxed) {
            break;
        }
        if nid.index() >= tree.len() || tree.is_deleted(nid) {
            continue;
        }
        let is_dir = tree.flags[nid.index()].contains(NodeFlags::IS_DIR);
        let is_root = nid == NodeId::ROOT || tree.flags[nid.index()].contains(NodeFlags::IS_ROOT_TARGET);

        if nid != NodeId::ROOT {
            let include_inh = is_root || nid == scope_root || include_inherited_on_children;
            targets.push((nid, tree.resolve_path(nid), is_dir, include_inh));
            visited += 1;
        }

        if is_dir {
            // Prioritize scanning directories first (matching SysAdmin ACL audit workflows)
            let mut subdirs = Vec::new();
            for c in tree.children(nid) {
                if tree.flags[c.index()].contains(NodeFlags::IS_DIR) && !tree.is_deleted(c) {
                    subdirs.push(c);
                }
            }
            for s in subdirs.into_iter().rev() {
                stack.push(s);
            }
        }
    }

    let sid_cache = SidCache::default();
    targets
        .into_par_iter()
        .flat_map(|(nid, path, is_dir, include_inh)| {
            if cancel.load(Ordering::Relaxed) {
                return Vec::new();
            }
            read_dacl_entries(nid, &path, is_dir, include_inh, &sid_cache)
        })
        .collect()
}

#[cfg(not(windows))]
fn resolve_owner_cached(_path: &Path, _cache: &SidCache) -> Option<String> {
    None
}

#[cfg(not(windows))]
fn read_dacl_entries(
    _nid: NodeId,
    _path: &Path,
    _is_dir: bool,
    _include_inherited: bool,
    _cache: &SidCache,
) -> Vec<PermissionEntry> {
    Vec::new()
}

#[cfg(windows)]
fn resolve_owner_cached(path: &Path, cache: &SidCache) -> Option<String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{OWNER_SECURITY_INFORMATION, PSID};
    use windows_sys::Win32::Foundation::LocalFree;

    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut owner_sid: PSID = std::ptr::null_mut();
    let mut sd: *mut std::ffi::c_void = std::ptr::null_mut();

    let rc = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner_sid,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sd,
        )
    };

    if rc != 0 || owner_sid.is_null() {
        if !sd.is_null() {
            unsafe {
                LocalFree(sd);
            }
        }
        return None;
    }

    let name = sid_to_account_name(owner_sid, cache);
    if !sd.is_null() {
        unsafe {
            LocalFree(sd);
        }
    }
    name
}

#[cfg(windows)]
fn read_dacl_entries(
    nid: NodeId,
    path: &Path,
    is_dir: bool,
    include_inherited: bool,
    cache: &SidCache,
) -> Vec<PermissionEntry> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        AclSizeInformation, GetAce, GetAclInformation, ACCESS_ALLOWED_ACE, ACCESS_DENIED_ACE,
        ACE_HEADER, ACL, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
        OWNER_SECURITY_INFORMATION, PSID,
    };
    use windows_sys::Win32::Foundation::LocalFree;

    const ACCESS_ALLOWED_ACE_TYPE_U8: u8 = 0;
    const ACCESS_DENIED_ACE_TYPE_U8: u8 = 1;
    const INHERITED_ACE_FLAG: u8 = 0x10;

    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut owner_sid: PSID = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd: *mut std::ffi::c_void = std::ptr::null_mut();

    let rc = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner_sid,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut sd,
        )
    };

    if rc != 0 {
        if !sd.is_null() {
            unsafe {
                LocalFree(sd);
            }
        }
        return Vec::new();
    }

    let owner = if !owner_sid.is_null() {
        sid_to_account_name(owner_sid, cache).unwrap_or_default()
    } else {
        String::new()
    };

    let mut out = Vec::new();
    if !dacl.is_null() {
        let mut size_info = ACL_SIZE_INFORMATION {
            AceCount: 0,
            AclBytesInUse: 0,
            AclBytesFree: 0,
        };
        let ok = unsafe {
            GetAclInformation(
                dacl,
                (&mut size_info as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            ) != 0
        };
        if ok {
            for idx in 0..size_info.AceCount {
                let mut ace_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
                if unsafe { GetAce(dacl, idx, &mut ace_ptr) == 0 } || ace_ptr.is_null() {
                    continue;
                }
                let hdr = unsafe { &*(ace_ptr as *const ACE_HEADER) };
                let inherited = (hdr.AceFlags & INHERITED_ACE_FLAG) != 0;
                if inherited && !include_inherited {
                    continue;
                }

                let (access_type, mask, sid_ptr) = match hdr.AceType {
                    ACCESS_ALLOWED_ACE_TYPE_U8 => {
                        let ace = unsafe { &*(ace_ptr as *const ACCESS_ALLOWED_ACE) };
                        ("Allow", ace.Mask, (&ace.SidStart as *const u32) as PSID)
                    }
                    ACCESS_DENIED_ACE_TYPE_U8 => {
                        let ace = unsafe { &*(ace_ptr as *const ACCESS_DENIED_ACE) };
                        ("Deny", ace.Mask, (&ace.SidStart as *const u32) as PSID)
                    }
                    _ => continue,
                };

                let identity =
                    sid_to_account_name(sid_ptr, cache).unwrap_or_else(|| "(SID desconocido)".into());
                out.push(PermissionEntry {
                    node_id: nid,
                    path: path.to_path_buf(),
                    is_dir,
                    owner: owner.clone(),
                    identity,
                    access_type: access_type.to_string(),
                    rights: format_access_mask(mask),
                    inherited,
                });
            }
        }
    }

    if !sd.is_null() {
        unsafe {
            LocalFree(sd);
        }
    }
    out
}

#[cfg(windows)]
fn sid_to_account_name(sid: windows_sys::Win32::Security::PSID, cache: &SidCache) -> Option<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Security::{GetLengthSid, IsValidSid, LookupAccountSidW, SID_NAME_USE};

    if sid.is_null() || unsafe { IsValidSid(sid) == 0 } {
        return None;
    }
    let sid_len = unsafe { GetLengthSid(sid) } as usize;
    if sid_len == 0 || sid_len > 128 {
        return None;
    }
    let sid_bytes = unsafe { std::slice::from_raw_parts(sid as *const u8, sid_len) };

    // Fast path: cache hit (read lock)
    if let Some(cached) = cache.read().get(sid_bytes) {
        return Some(cached.clone());
    }

    let mut name_buf = [0u16; 256];
    let mut dom_buf = [0u16; 256];
    let mut name_len = name_buf.len() as u32;
    let mut dom_len = dom_buf.len() as u32;
    let mut sid_use: SID_NAME_USE = 0;

    let ok = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid,
            name_buf.as_mut_ptr(),
            &mut name_len,
            dom_buf.as_mut_ptr(),
            &mut dom_len,
            &mut sid_use,
        ) != 0
    };

    let resolved = if ok {
        let name = OsString::from_wide(&name_buf[..name_len as usize])
            .to_string_lossy()
            .into_owned();
        let dom = OsString::from_wide(&dom_buf[..dom_len as usize])
            .to_string_lossy()
            .into_owned();
        if dom.is_empty() {
            name
        } else {
            format!(r"{dom}\{name}")
        }
    } else {
        // Fallback: hex representation of unresolvable/orphaned AD SID
        format!("SID-Orphan({}B)", sid_bytes.len())
    };

    cache.write().insert(sid_bytes.to_vec(), resolved.clone());
    Some(resolved)
}

#[cfg(windows)]
fn format_access_mask(mask: u32) -> String {
    const FILE_FULL_CONTROL: u32 = 0x001F_01FF;
    const FILE_MODIFY: u32 = 0x0013_01BF;
    const FILE_READ_EXEC: u32 = 0x0012_00A9;
    const FILE_READ: u32 = 0x0012_0089;
    const GENERIC_ALL: u32 = 0x1000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const GENERIC_READ: u32 = 0x8000_0000;
    const DELETE_BIT: u32 = 0x0001_0000;

    if (mask & GENERIC_ALL) != 0 || (mask & FILE_FULL_CONTROL) == FILE_FULL_CONTROL {
        return "Full Control".into();
    }
    if (mask & FILE_MODIFY) == FILE_MODIFY {
        return "Modify".into();
    }
    if (mask & FILE_READ_EXEC) == FILE_READ_EXEC {
        if (mask & 0x0000_0002) != 0 || (mask & GENERIC_WRITE) != 0 {
            return "Read & Write".into();
        }
        return "Read & Execute".into();
    }
    if (mask & FILE_READ) == FILE_READ || (mask & GENERIC_READ) != 0 {
        return "Read".into();
    }
    let mut parts = Vec::new();
    if (mask & (0x0000_0001 | GENERIC_READ)) != 0 {
        parts.push("Read");
    }
    if (mask & (0x0000_0002 | 0x0000_0004 | GENERIC_WRITE)) != 0 {
        parts.push("Write");
    }
    if (mask & DELETE_BIT) != 0 {
        parts.push("Delete");
    }
    if parts.is_empty() {
        format!("Special (0x{mask:08X})")
    } else {
        parts.join(" + ")
    }
}
