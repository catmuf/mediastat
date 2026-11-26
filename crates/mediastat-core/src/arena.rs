use std::path::{Path, PathBuf};

use ahash::AHashMap;
use bitflags::bitflags;
use serde::{Deserialize, Serialize};

use crate::format::{extension_color, windirstat_rank_color};
use crate::i18n::{active_language, t_for};
use crate::settings::SizeMetric;

/// Compact 32-bit node handle supporting up to 4.29 billion nodes in memory/index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
pub struct NodeId(pub u32);

impl NodeId {
    pub const ROOT: Self = Self(0);
    pub const NONE: Self = Self(u32::MAX);

    #[inline]
    pub fn is_none(self) -> bool {
        self.0 == u32::MAX
    }

    #[inline]
    pub fn is_some(self) -> bool {
        self.0 != u32::MAX
    }

    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
    pub struct NodeFlags: u16 {
        const IS_DIR             = 1 << 0;
        const IS_ROOT_TARGET     = 1 << 1;
        const IS_SYMLINK         = 1 << 2;
        const IS_HIDDEN          = 1 << 3;
        // Bits 4..=6 reserved (legacy v1/v2 media flags ignored on load)
        const IS_DUPLICATE       = 1 << 7;
        const IS_HARDLINK_DUP    = 1 << 8;
        const ACCESS_DENIED      = 1 << 9;
        const IS_LONG_PATH       = 1 << 10;
        const IS_DELETED         = 1 << 11;
        const IS_JUNCTION        = 1 << 12;
        const IS_MOUNT_POINT     = 1 << 13;
    }
}

bitflags! {
    /// Win32 / NTFS file attributes displayed in WinDirStat Attributes column (`RHSACEPOL`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
    pub struct WinAttrs: u16 {
        const READONLY      = 1 << 0; // R
        const HIDDEN        = 1 << 1; // H
        const SYSTEM        = 1 << 2; // S
        const ARCHIVE       = 1 << 3; // A
        const COMPRESSED    = 1 << 4; // C (NTFS compressed)
        const ENCRYPTED     = 1 << 5; // E (EFS)
        const SPARSE        = 1 << 6; // P (Sparse file)
        const OFFLINE_CLOUD = 1 << 7; // O (OneDrive/Cloud placeholder — never hydrate!)
        const REPARSE       = 1 << 8; // L (Symlink / Junction / Reparse Point)
    }
}

impl WinAttrs {
    pub fn to_compact_string(self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut s = String::with_capacity(9);
        if self.contains(Self::READONLY) {
            s.push('R');
        }
        if self.contains(Self::HIDDEN) {
            s.push('H');
        }
        if self.contains(Self::SYSTEM) {
            s.push('S');
        }
        if self.contains(Self::ARCHIVE) {
            s.push('A');
        }
        if self.contains(Self::COMPRESSED) {
            s.push('C');
        }
        if self.contains(Self::ENCRYPTED) {
            s.push('E');
        }
        if self.contains(Self::SPARSE) {
            s.push('P');
        }
        if self.contains(Self::OFFLINE_CLOUD) {
            s.push('O');
        }
        if self.contains(Self::REPARSE) {
            s.push('L');
        }
        s
    }
}

/// Contiguous byte slab storing all filenames without per-node heap allocations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StringArena {
    bytes: Vec<u8>,
}

impl StringArena {
    pub fn with_capacity(bytes: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(bytes),
        }
    }

    #[inline]
    pub fn push(&mut self, s: &str) -> (u64, u16) {
        let offset = self.bytes.len() as u64;
        let raw = s.as_bytes();
        let len = raw.len().min(u16::MAX as usize);
        self.bytes.extend_from_slice(&raw[..len]);
        (offset, len as u16)
    }

    #[inline]
    pub fn get(&self, offset: u64, len: u16) -> &str {
        let start = offset as usize;
        let end = start + len as usize;
        std::str::from_utf8(&self.bytes[start..end]).unwrap_or("")
    }

    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionInfo {
    pub id: u16,
    pub ext: String,
    pub color: [u8; 3],
    pub file_count: u64,
    pub total_logical_bytes: u64,
    pub total_allocated_bytes: u64,
}

/// Interned file-extension dictionary (u16 IDs, up to 65,535 distinct extensions).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtensionTable {
    entries: Vec<ExtensionInfo>,
    lookup: AHashMap<String, u16>,
}

impl ExtensionTable {
    pub fn new() -> Self {
        let mut table = Self::default();
        table.intern("");
        table
    }

    pub fn intern(&mut self, ext_raw: &str) -> u16 {
        let ext_lower = ext_raw.to_ascii_lowercase();
        if let Some(&id) = self.lookup.get(&ext_lower) {
            return id;
        }
        if self.entries.len() >= u16::MAX as usize {
            return 0;
        }
        let id = self.entries.len() as u16;
        let color = extension_color(&ext_lower);
        self.entries.push(ExtensionInfo {
            id,
            ext: ext_lower.clone(),
            color,
            file_count: 0,
            total_logical_bytes: 0,
            total_allocated_bytes: 0,
        });
        self.lookup.insert(ext_lower, id);
        id
    }

    #[inline]
    pub fn get(&self, id: u16) -> Option<&ExtensionInfo> {
        self.entries.get(id as usize)
    }

    #[inline]
    pub fn ext_str(&self, id: u16) -> &str {
        self.entries
            .get(id as usize)
            .map(|e| e.ext.as_str())
            .unwrap_or("")
    }

    pub fn entries(&self) -> &[ExtensionInfo] {
        &self.entries
    }

    pub fn entries_mut(&mut self) -> &mut [ExtensionInfo] {
        &mut self.entries
    }

    pub fn reset_counters(&mut self) {
        for e in &mut self.entries {
            e.file_count = 0;
            e.total_logical_bytes = 0;
            e.total_allocated_bytes = 0;
        }
    }
}

/// Interned NTFS/AD Owner dictionary (`u16` ID -> `DOMAIN\user`). ID `0` = unresolved.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OwnerTable {
    names: Vec<String>,
    lookup: AHashMap<String, u16>,
}

impl OwnerTable {
    pub fn new() -> Self {
        let mut t = Self::default();
        t.names.push(String::new()); // 0 = unresolved
        t
    }

    pub fn intern(&mut self, owner: &str) -> u16 {
        if owner.is_empty() {
            return 0;
        }
        if let Some(&id) = self.lookup.get(owner) {
            return id;
        }
        if self.names.len() >= u16::MAX as usize {
            return 0;
        }
        let id = self.names.len() as u16;
        let owned = owner.to_string();
        self.names.push(owned.clone());
        self.lookup.insert(owned, id);
        id
    }

    #[inline]
    pub fn get(&self, id: u16) -> &str {
        self.names.get(id as usize).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn all(&self) -> &[String] {
        &self.names
    }
}

/// Real volume space telemetry attached to whole-volume root targets (for WinDirStat F6 Free & F7 Unknown).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VolumeSpaceInfo {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub unc_remote_path: Option<String>,
}

impl VolumeSpaceInfo {
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }

    /// Space consumed on the volume that was NOT accounted for by scanned files (VSS, $MFT, Access Denied, System).
    pub fn unknown_bytes(&self, scanned_allocated: u64) -> u64 {
        self.used_bytes().saturating_sub(scanned_allocated)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueKind {
    AccessDenied,
    LongPath,
    CloudSkipped,
}

impl IssueKind {
    pub fn label(self) -> &'static str {
        let lang = active_language();
        match self {
            Self::AccessDenied => t_for(lang, "Acceso Denegado", "Access Denied"),
            Self::LongPath => t_for(lang, "Ruta > 260 caracteres", "Path > 260 characters"),
            Self::CloudSkipped => "Cloud/OneDrive Offline",
        }
    }
}

/// Actionable SysAdmin audit issue recorded during scanning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanIssue {
    pub kind: IssueKind,
    pub path: PathBuf,
    pub detail: String,
}

/// Single DACL Access Control Entry inspected by `Permissions` tab (WinDirStat 2.10 `ItemPerm` parity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionEntry {
    pub node_id: NodeId,
    pub path: PathBuf,
    pub is_dir: bool,
    pub owner: String,
    pub identity: String,
    pub access_type: String, // "Allow" / "Deny"
    pub rights: String,      // "Full Control", "Modify", "Read & Execute", etc.
    pub inherited: bool,
}

/// Aggregated recursive metrics computed for directory nodes (stored sparsely only for dirs).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct DirStats {
    pub total_files: u64,
    pub total_dirs: u64,
    pub max_mtime: i64,
}

/// Group of duplicate files detected by BLAKE3 (full exact or fast sparse >64 MiB).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub blake3_hex: String,
    pub file_size: u64,
    pub nodes: Vec<NodeId>,
    #[serde(default)]
    pub is_probable_sparse: bool,
}

impl DuplicateGroup {
    pub fn reclaimable_bytes(&self) -> u64 {
        if self.nodes.len() > 1 {
            (self.nodes.len() as u64 - 1) * self.file_size
        } else {
            0
        }
    }
}

/// NTFS Hardlink group deduplicated by `(volume_root, file_id)` (exact WinDirStat `<Hardlinks>` parity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardlinkGroup {
    pub volume_root: NodeId,
    pub file_id: u64,
    pub physical_size: u64,
    pub nodes: Vec<NodeId>,
}

/// High-level KPI summary of a completed or in-progress scan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanSummary {
    pub total_files: u64,
    pub total_dirs: u64,
    pub total_logical_bytes: u64,
    pub total_allocated_bytes: u64,
    pub duplicate_groups: u64,
    pub duplicate_reclaimable_bytes: u64,
    pub access_denied_count: u64,
    pub long_path_count: u64,
    pub cloud_offline_count: u64,
    pub scan_duration_secs: f64,
    pub engine_used: String,
}

/// Compact Structure-of-Arrays (SoA) disk tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanTree {
    // Structure-of-Arrays columns indexed by NodeId(u32)
    pub parents: Vec<NodeId>,
    pub first_child: Vec<NodeId>,
    pub last_child: Vec<NodeId>,
    pub next_sibling: Vec<NodeId>,
    pub name_offsets: Vec<u64>,
    pub name_lens: Vec<u16>,
    pub ext_ids: Vec<u16>,
    pub flags: Vec<NodeFlags>,
    pub win_attrs: Vec<WinAttrs>,
    pub owner_ids: Vec<u16>,
    pub logical_sizes: Vec<u64>,
    pub allocated_sizes: Vec<u64>,
    /// NTFS 64-bit FileId / MFT segment number for hardlink deduplication (`0` if unavailable).
    #[serde(default)]
    pub file_ids: Vec<u64>,
    pub mtimes: Vec<i64>,

    /// Contiguous UTF-8 slab holding all node basenames (and full root paths for root targets).
    pub names: StringArena,
    /// Interned file extensions + WinDirStat palette & totals.
    pub extensions: ExtensionTable,
    /// Interned owner strings (`DOMAIN\user`).
    pub owners: OwnerTable,
    /// Directory-only recursive statistics (sparse).
    pub dir_stats: AHashMap<NodeId, DirStats>,
    /// Volume free/total disk space for root targets (enables WinDirStat F6/F7 Free/Unknown).
    pub volume_space: AHashMap<NodeId, VolumeSpaceInfo>,
    /// Exact / sparse duplicate groups discovered via BLAKE3.
    pub duplicates: Vec<DuplicateGroup>,
    /// NTFS Hardlink groups deduplicated by `FileId` per volume (powers `<Hardlinks>` node).
    #[serde(default)]
    pub hardlinks: Vec<HardlinkGroup>,
    /// SysAdmin audit issues (Access Denied, Long Paths >260, Cloud Offline).
    pub issues: Vec<ScanIssue>,
    /// Inspected NTFS/SMB DACL permission entries.
    pub permissions: Vec<PermissionEntry>,
    /// Scan target roots (children of virtual NodeId::ROOT).
    pub root_targets: Vec<PathBuf>,
    /// Unix timestamp when this scan started.
    pub scanned_at_unix: i64,
    /// Scan engine description.
    pub engine_used: String,
    /// Total elapsed scan time in seconds.
    pub scan_duration_secs: f64,
}

impl Default for ScanTree {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanTree {
    pub fn new() -> Self {
        Self::with_capacity(1024)
    }

    pub fn with_capacity(estimated_nodes: usize) -> Self {
        let mut tree = Self {
            parents: Vec::with_capacity(estimated_nodes),
            first_child: Vec::with_capacity(estimated_nodes),
            last_child: Vec::with_capacity(estimated_nodes),
            next_sibling: Vec::with_capacity(estimated_nodes),
            name_offsets: Vec::with_capacity(estimated_nodes),
            name_lens: Vec::with_capacity(estimated_nodes),
            ext_ids: Vec::with_capacity(estimated_nodes),
            flags: Vec::with_capacity(estimated_nodes),
            win_attrs: Vec::with_capacity(estimated_nodes),
            owner_ids: Vec::with_capacity(estimated_nodes),
            logical_sizes: Vec::with_capacity(estimated_nodes),
            allocated_sizes: Vec::with_capacity(estimated_nodes),
            file_ids: Vec::with_capacity(estimated_nodes),
            mtimes: Vec::with_capacity(estimated_nodes),
            names: StringArena::with_capacity(estimated_nodes.saturating_mul(20)),
            extensions: ExtensionTable::new(),
            owners: OwnerTable::new(),
            dir_stats: AHashMap::with_capacity(estimated_nodes / 8),
            volume_space: AHashMap::new(),
            duplicates: Vec::new(),
            hardlinks: Vec::new(),
            issues: Vec::new(),
            permissions: Vec::new(),
            root_targets: Vec::new(),
            scanned_at_unix: chrono::Utc::now().timestamp(),
            engine_used: String::new(),
            scan_duration_secs: 0.0,
        };
        tree.push_raw_node(
            NodeId::NONE,
            "Espacio de Trabajo",
            0,
            NodeFlags::IS_DIR,
            WinAttrs::empty(),
            0,
            0,
            0,
            0,
        );
        tree
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.parents.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.parents.len() <= 1
    }

    pub fn approx_memory_bytes(&self) -> usize {
        let n = self.parents.len();
        let per_node = std::mem::size_of::<NodeId>() * 4
            + std::mem::size_of::<u64>() * 4
            + std::mem::size_of::<i64>()
            + std::mem::size_of::<u16>() * 5;
        n * per_node
            + self.names.byte_len()
            + self.dir_stats.len()
                * (std::mem::size_of::<NodeId>() + std::mem::size_of::<DirStats>() + 8)
            + self.permissions.len() * 140
    }

    pub fn add_root_target(&mut self, path: &Path) -> NodeId {
        let display = path.to_string_lossy();
        let id = self.push_raw_node(
            NodeId::ROOT,
            &display,
            0,
            NodeFlags::IS_DIR | NodeFlags::IS_ROOT_TARGET,
            WinAttrs::empty(),
            0,
            0,
            0,
            0,
        );
        self.root_targets.push(path.to_path_buf());
        id
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_child(
        &mut self,
        parent: NodeId,
        name: &str,
        flags: NodeFlags,
        logical_size: u64,
        allocated_size: u64,
        mtime: i64,
    ) -> NodeId {
        self.add_child_full(
            parent,
            name,
            flags,
            WinAttrs::empty(),
            logical_size,
            allocated_size,
            0,
            mtime,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_child_with_attrs(
        &mut self,
        parent: NodeId,
        name: &str,
        flags: NodeFlags,
        win_attrs: WinAttrs,
        logical_size: u64,
        allocated_size: u64,
        mtime: i64,
    ) -> NodeId {
        self.add_child_full(
            parent,
            name,
            flags,
            win_attrs,
            logical_size,
            allocated_size,
            0,
            mtime,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_child_full(
        &mut self,
        parent: NodeId,
        name: &str,
        flags: NodeFlags,
        win_attrs: WinAttrs,
        logical_size: u64,
        allocated_size: u64,
        file_id: u64,
        mtime: i64,
    ) -> NodeId {
        let ext_id = if flags.contains(NodeFlags::IS_DIR) {
            0
        } else {
            let ext = extract_extension(name);
            self.extensions.intern(ext)
        };
        self.push_raw_node(
            parent,
            name,
            ext_id,
            flags,
            win_attrs,
            logical_size,
            allocated_size,
            file_id,
            mtime,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn push_raw_node(
        &mut self,
        parent: NodeId,
        name: &str,
        ext_id: u16,
        flags: NodeFlags,
        win_attrs: WinAttrs,
        logical_size: u64,
        allocated_size: u64,
        file_id: u64,
        mtime: i64,
    ) -> NodeId {
        let id = NodeId(self.parents.len() as u32);
        let (off, len) = self.names.push(name);
        self.parents.push(parent);
        self.first_child.push(NodeId::NONE);
        self.last_child.push(NodeId::NONE);
        self.next_sibling.push(NodeId::NONE);
        self.name_offsets.push(off);
        self.name_lens.push(len);
        self.ext_ids.push(ext_id);
        self.flags.push(flags);
        self.win_attrs.push(win_attrs);
        self.owner_ids.push(0);
        self.logical_sizes.push(logical_size);
        self.allocated_sizes.push(allocated_size);
        self.file_ids.push(file_id);
        self.mtimes.push(mtime);

        if parent.is_some() {
            let p_idx = parent.index();
            let prev_last = self.last_child[p_idx];
            if prev_last.is_none() {
                self.first_child[p_idx] = id;
                self.last_child[p_idx] = id;
            } else {
                self.next_sibling[prev_last.index()] = id;
                self.last_child[p_idx] = id;
            }
        }
        id
    }

    pub fn set_owner(&mut self, id: NodeId, owner_name: &str) {
        if id.index() < self.owner_ids.len() {
            let oid = self.owners.intern(owner_name);
            self.owner_ids[id.index()] = oid;
        }
    }

    #[inline]
    pub fn owner(&self, id: NodeId) -> &str {
        self.owners.get(self.owner_ids[id.index()])
    }

    #[inline]
    pub fn name(&self, id: NodeId) -> &str {
        let idx = id.index();
        self.names.get(self.name_offsets[idx], self.name_lens[idx])
    }

    #[inline]
    pub fn is_dir(&self, id: NodeId) -> bool {
        self.flags[id.index()].contains(NodeFlags::IS_DIR)
    }

    #[inline]
    pub fn is_deleted(&self, id: NodeId) -> bool {
        self.flags[id.index()].contains(NodeFlags::IS_DELETED)
    }

    #[inline]
    pub fn ext(&self, id: NodeId) -> &str {
        self.extensions.ext_str(self.ext_ids[id.index()])
    }

    #[inline]
    pub fn size_for_metric(&self, id: NodeId, metric: SizeMetric) -> u64 {
        let idx = id.index();
        match metric {
            SizeMetric::Logical => self.logical_sizes[idx],
            SizeMetric::AllocatedOnDisk => self.allocated_sizes[idx],
        }
    }

    pub fn children(&self, parent: NodeId) -> ChildIter<'_> {
        ChildIter {
            tree: self,
            current: self.first_child[parent.index()],
        }
    }

    /// Unlink a node (and mark its entire subtree deleted) in O(subtree) without reallocating NodeIds,
    /// then immediately recompute bottom-up totals (used after Trash/Delete cleanup actions).
    pub fn mark_node_deleted(&mut self, target: NodeId) {
        if target == NodeId::ROOT || target.is_none() || target.index() >= self.len() {
            return;
        }
        let parent = self.parents[target.index()];
        if parent.is_some() {
            let p_idx = parent.index();
            let mut prev = NodeId::NONE;
            let mut cur = self.first_child[p_idx];
            while cur.is_some() {
                let next = self.next_sibling[cur.index()];
                if cur == target {
                    if prev.is_none() {
                        self.first_child[p_idx] = next;
                    } else {
                        self.next_sibling[prev.index()] = next;
                    }
                    if self.last_child[p_idx] == target {
                        self.last_child[p_idx] = prev;
                    }
                    self.next_sibling[target.index()] = NodeId::NONE;
                    break;
                }
                prev = cur;
                cur = next;
            }
        }
        self.mark_subtree_deleted_flags(target);
        self.recompute_aggregates();
    }

    /// Detach all direct children of `dir_id` (marking old subtree deleted) prior to an `F5` partial rescan.
    pub fn detach_children(&mut self, dir_id: NodeId) {
        if dir_id.is_none() || dir_id.index() >= self.len() {
            return;
        }
        let mut child = self.first_child[dir_id.index()];
        self.first_child[dir_id.index()] = NodeId::NONE;
        self.last_child[dir_id.index()] = NodeId::NONE;
        while child.is_some() {
            let next = self.next_sibling[child.index()];
            self.mark_subtree_deleted_flags(child);
            child = next;
        }
    }

    fn mark_subtree_deleted_flags(&mut self, root: NodeId) {
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            let idx = n.index();
            self.flags[idx].insert(NodeFlags::IS_DELETED);
            let mut c = self.first_child[idx];
            while c.is_some() {
                stack.push(c);
                c = self.next_sibling[c.index()];
            }
        }
    }

    pub fn resolve_path(&self, id: NodeId) -> PathBuf {
        if id == NodeId::ROOT || id.is_none() {
            return PathBuf::new();
        }
        let mut chain = Vec::with_capacity(12);
        let mut cur = id;
        while cur.is_some() && cur != NodeId::ROOT {
            chain.push(cur);
            if self.flags[cur.index()].contains(NodeFlags::IS_ROOT_TARGET) {
                break;
            }
            cur = self.parents[cur.index()];
        }
        chain.reverse();
        let mut path = PathBuf::new();
        for node in chain {
            path.push(self.name(node));
        }
        path
    }

    /// Recompute bottom-up recursive directory totals, extension stats, and media savings in O(N).
    pub fn recompute_aggregates(&mut self) {
        self.dir_stats.clear();
        self.extensions.reset_counters();

        let n = self.len();
        if n == 0 {
            return;
        }

        for i in 0..n {
            if self.flags[i].contains(NodeFlags::IS_DIR) {
                self.logical_sizes[i] = 0;
                self.allocated_sizes[i] = 0;
            }
        }

        let mut order = Vec::with_capacity(n);
        let mut stack = Vec::with_capacity(1024);
        stack.push(NodeId::ROOT);
        while let Some(node) = stack.pop() {
            if self.flags[node.index()].contains(NodeFlags::IS_DELETED) {
                continue;
            }
            order.push(node);
            let mut child = self.first_child[node.index()];
            while child.is_some() {
                stack.push(child);
                child = self.next_sibling[child.index()];
            }
        }

        for &node in order.iter().rev() {
            let idx = node.index();
            let parent = self.parents[idx];
            if self.flags[idx].contains(NodeFlags::IS_DIR) {
                let stats = self.dir_stats.entry(node).or_default();
                let my_stats = *stats;
                let my_logical = self.logical_sizes[idx];
                let my_alloc = self.allocated_sizes[idx];
                let my_mtime = self.mtimes[idx].max(my_stats.max_mtime);

                if parent.is_some() {
                    let p_idx = parent.index();
                    self.logical_sizes[p_idx] = self.logical_sizes[p_idx].saturating_add(my_logical);
                    self.allocated_sizes[p_idx] =
                        self.allocated_sizes[p_idx].saturating_add(my_alloc);
                    let p_stats = self.dir_stats.entry(parent).or_default();
                    p_stats.total_files += my_stats.total_files;
                    p_stats.total_dirs += my_stats.total_dirs + 1;
                    p_stats.max_mtime = p_stats.max_mtime.max(my_mtime);
                }
            } else {
                let logical = self.logical_sizes[idx];
                let alloc = if self.flags[idx].contains(NodeFlags::IS_HARDLINK_DUP) {
                    0
                } else {
                    self.allocated_sizes[idx]
                };

                let ext_id = self.ext_ids[idx] as usize;
                if let Some(ext_info) = self.extensions.entries_mut().get_mut(ext_id) {
                    ext_info.file_count += 1;
                    ext_info.total_logical_bytes =
                        ext_info.total_logical_bytes.saturating_add(logical);
                    ext_info.total_allocated_bytes =
                        ext_info.total_allocated_bytes.saturating_add(alloc);
                }

                if parent.is_some() {
                    let p_idx = parent.index();
                    self.logical_sizes[p_idx] = self.logical_sizes[p_idx].saturating_add(logical);
                    self.allocated_sizes[p_idx] = self.allocated_sizes[p_idx].saturating_add(alloc);
                    let p_stats = self.dir_stats.entry(parent).or_default();
                    p_stats.total_files += 1;
                    p_stats.max_mtime = p_stats.max_mtime.max(self.mtimes[idx]);
                }
            }
        }

        // Assign exact WinDirStat 18-color palette by descending byte rank (`WinDirStatModel.cpp`)
        self.recompute_extension_rank_colors();
    }

    /// Assign WinDirStat's exact 18-color normalized palette to extensions ordered by descending bytes.
    pub fn recompute_extension_rank_colors(&mut self) {
        let mut ranked: Vec<(usize, u64)> = self
            .extensions
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| e.file_count > 0)
            .map(|(idx, e)| (idx, e.total_logical_bytes.max(e.total_allocated_bytes)))
            .collect();
        ranked.sort_unstable_by_key(|&(_, bytes)| std::cmp::Reverse(bytes));
        let entries = self.extensions.entries_mut();
        for (rank, (idx, _)) in ranked.into_iter().enumerate() {
            entries[idx].color = windirstat_rank_color(rank);
        }
    }

    /// Deduplicate NTFS hardlinks per root volume using `file_ids` (WinDirStat `Item.Extended.cpp` parity).
    /// Marks secondary links with `NodeFlags::IS_HARDLINK_DUP` so physical sizes are never double-counted,
    /// populates `self.hardlinks`, and recomputes aggregates.
    pub fn deduplicate_hardlinks(&mut self) {
        self.hardlinks.clear();
        if self.file_ids.len() < self.len() {
            self.file_ids.resize(self.len(), 0);
        }
        for flag in self.flags.iter_mut().skip(1) {
            flag.remove(NodeFlags::IS_HARDLINK_DUP);
        }

        let roots: Vec<NodeId> = self.children(NodeId::ROOT).collect();
        for vol_root in roots {
            let mut seen: AHashMap<u64, Vec<NodeId>> = AHashMap::new();
            let mut stack = vec![vol_root];
            while let Some(n) = stack.pop() {
                let idx = n.index();
                if self.flags[idx].contains(NodeFlags::IS_DELETED) {
                    continue;
                }
                if self.flags[idx].contains(NodeFlags::IS_DIR) {
                    let mut c = self.first_child[idx];
                    while c.is_some() {
                        stack.push(c);
                        c = self.next_sibling[c.index()];
                    }
                } else {
                    let fid = self.file_ids[idx];
                    if fid != 0 {
                        seen.entry(fid).or_default().push(n);
                    }
                }
            }

            for (file_id, mut nodes) in seen {
                if nodes.len() > 1 {
                    nodes.sort_unstable();
                    let mut max_phys = 0u64;
                    for (pos, &nid) in nodes.iter().enumerate() {
                        max_phys = max_phys.max(self.allocated_sizes[nid.index()]);
                        if pos > 0 {
                            self.flags[nid.index()].insert(NodeFlags::IS_HARDLINK_DUP);
                        }
                    }
                    self.hardlinks.push(HardlinkGroup {
                        volume_root: vol_root,
                        file_id,
                        physical_size: max_phys,
                        nodes,
                    });
                }
            }
        }

        self.hardlinks
            .sort_unstable_by_key(|g| std::cmp::Reverse(g.physical_size));
        self.recompute_aggregates();
    }

    pub fn hardlinks_for_volume(&self, volume_root: NodeId) -> Vec<&HardlinkGroup> {
        self.hardlinks
            .iter()
            .filter(|g| g.volume_root == volume_root)
            .collect()
    }

    pub fn hardlinks_total_bytes_for_volume(&self, volume_root: NodeId) -> u64 {
        self.hardlinks
            .iter()
            .filter(|g| g.volume_root == volume_root)
            .map(|g| g.physical_size)
            .sum()
    }

    pub fn summary(&self) -> ScanSummary {
        let root_stats = self
            .dir_stats
            .get(&NodeId::ROOT)
            .copied()
            .unwrap_or_default();
        let mut long_paths = 0u64;
        let mut cloud_offline = 0u64;
        for i in 1..self.len() {
            let f = self.flags[i];
            if f.contains(NodeFlags::IS_DELETED) {
                continue;
            }
            if f.contains(NodeFlags::IS_LONG_PATH) {
                long_paths += 1;
            }
            if self.win_attrs[i].contains(WinAttrs::OFFLINE_CLOUD) {
                cloud_offline += 1;
            }
        }
        let access_denied = self
            .issues
            .iter()
            .filter(|is| is.kind == IssueKind::AccessDenied)
            .count() as u64;
        let dup_reclaim: u64 = self.duplicates.iter().map(|g| g.reclaimable_bytes()).sum();
        ScanSummary {
            total_files: root_stats.total_files,
            total_dirs: root_stats.total_dirs,
            total_logical_bytes: self.logical_sizes[0],
            total_allocated_bytes: self.allocated_sizes[0],
            duplicate_groups: self.duplicates.len() as u64,
            duplicate_reclaimable_bytes: dup_reclaim,
            access_denied_count: access_denied,
            long_path_count: long_paths,
            cloud_offline_count: cloud_offline,
            scan_duration_secs: self.scan_duration_secs,
            engine_used: self.engine_used.clone(),
        }
    }

    /// Return all non-deleted file NodeIds sorted descending by the chosen metric.
    pub fn all_files_sorted(&self, metric: SizeMetric) -> Vec<NodeId> {
        let mut files: Vec<NodeId> = (1..self.len())
            .filter(|&i| {
                let f = self.flags[i];
                !f.contains(NodeFlags::IS_DIR) && !f.contains(NodeFlags::IS_DELETED)
            })
            .map(|i| NodeId(i as u32))
            .collect();
        files.sort_unstable_by_key(|&id| std::cmp::Reverse(self.size_for_metric(id, metric)));
        files
    }

    /// Return the top `limit` largest non-deleted files sorted descending by the chosen metric.
    pub fn largest_files(&self, limit: usize, metric: SizeMetric) -> Vec<NodeId> {
        let mut files = self.all_files_sorted(metric);
        files.truncate(limit);
        files
    }
}

pub struct ChildIter<'a> {
    tree: &'a ScanTree,
    current: NodeId,
}

impl<'a> Iterator for ChildIter<'a> {
    type Item = NodeId;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.current.is_none() {
            return None;
        }
        let id = self.current;
        self.current = self.tree.next_sibling[id.index()];
        Some(id)
    }
}

#[inline]
pub fn extract_extension(filename: &str) -> &str {
    match filename.rfind('.') {
        Some(pos) if pos > 0 && pos + 1 < filename.len() => &filename[pos + 1..],
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arena_multi_root_aggregation_and_live_delete() {
        let mut tree = ScanTree::new();
        let root_c = tree.add_root_target(Path::new(r"C:\Fotos"));
        let root_d = tree.add_root_target(Path::new(r"\\server\media"));

        let sub = tree.add_child(root_c, "Vacaciones", NodeFlags::IS_DIR, 0, 0, 1000);
        let f1 = tree.add_child_with_attrs(
            sub,
            "IMG_0001.BMP",
            NodeFlags::empty(),
            WinAttrs::ARCHIVE | WinAttrs::READONLY,
            10_000_000,
            10_002_432,
            1700000000,
        );
        let f2 = tree.add_child(
            root_d,
            "clip.mov",
            NodeFlags::empty(),
            50_000_000,
            50_003_968,
            1710000000,
        );
        tree.set_owner(f1, r"CORP\jdoe");

        tree.recompute_aggregates();
        assert_eq!(tree.owner(f1), r"CORP\jdoe");
        assert_eq!(tree.win_attrs[f1.index()].to_compact_string(), "RA");
        assert_eq!(tree.largest_files(10, SizeMetric::Logical), vec![f2, f1]);

        // Live delete f1 -> totals update immediately!
        tree.mark_node_deleted(f1);
        let sum = tree.summary();
        assert_eq!(sum.total_files, 1);
        assert_eq!(sum.total_logical_bytes, 50_000_000);
        assert_eq!(sum.total_allocated_bytes, 50_003_968);
    }

    #[test]
    fn test_compact_memory_footprint() {
        let mut tree = ScanTree::with_capacity(10_000);
        let root = tree.add_root_target(Path::new("/data"));
        for i in 0..10_000u32 {
            tree.add_child(
                root,
                &format!("IMG_{i:06}.JPG"),
                NodeFlags::empty(),
                1024,
                4096,
                1700000000,
            );
        }
        let bytes_per_node = tree.approx_memory_bytes() as f64 / tree.len() as f64;
        assert!(
            bytes_per_node < 95.0,
            "Expected < 95 bytes/node, got {bytes_per_node:.1}"
        );
    }

    #[test]
    fn test_hardlink_deduplication_and_rank_palette() {
        let mut tree = ScanTree::new();
        let root = tree.add_root_target(Path::new(r"C:\"));
        let d1 = tree.add_child(root, "WinSxS", NodeFlags::IS_DIR, 0, 0, 100);
        let d2 = tree.add_child(root, "System32", NodeFlags::IS_DIR, 0, 0, 100);

        // 3 hardlinks sharing the exact same NTFS FileId 0xABCD_1234 (10 MB each)
        let h1 = tree.add_child_full(d1, "kernel.dll", NodeFlags::empty(), WinAttrs::empty(), 10_000_000, 10_002_432, 0xABCD_1234, 100);
        let h2 = tree.add_child_full(d2, "kernel.dll", NodeFlags::empty(), WinAttrs::empty(), 10_000_000, 10_002_432, 0xABCD_1234, 100);
        let h3 = tree.add_child_full(d2, "kernel_copy.dll", NodeFlags::empty(), WinAttrs::empty(), 10_000_000, 10_002_432, 0xABCD_1234, 100);
        // Smaller file of another extension (.mp4 = 5 MB)
        let v1 = tree.add_child_full(root, "clip.mp4", NodeFlags::empty(), WinAttrs::empty(), 5_000_000, 5_000_000, 0x9999, 100);

        tree.deduplicate_hardlinks();
        assert_eq!(tree.hardlinks.len(), 1);
        assert_eq!(tree.hardlinks[0].nodes.len(), 3);
        assert!(!tree.flags[h1.index()].contains(NodeFlags::IS_HARDLINK_DUP));
        assert!(tree.flags[h2.index()].contains(NodeFlags::IS_HARDLINK_DUP));
        assert!(tree.flags[h3.index()].contains(NodeFlags::IS_HARDLINK_DUP));

        // Total logical = 35 MB, total physical = 10_002_432 + 5_000_000 (no triple-counting!)
        let sum = tree.summary();
        assert_eq!(sum.total_logical_bytes, 35_000_000);
        assert_eq!(sum.total_allocated_bytes, 15_002_432);

        // Rank 0 (.dll, 30 MB) gets WinDirStat palette #0 (Blue), Rank 1 (.mp4, 5 MB) gets #1 (Red)
        let dll_col = tree.extensions.get(tree.ext_ids[h1.index()]).unwrap().color;
        let mp4_col = tree.extensions.get(tree.ext_ids[v1.index()]).unwrap().color;
        assert_eq!(dll_col, windirstat_rank_color(0));
        assert_eq!(mp4_col, windirstat_rank_color(1));
    }
}
