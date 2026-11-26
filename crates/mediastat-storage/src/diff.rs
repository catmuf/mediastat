use std::path::PathBuf;

use ahash::AHashMap;
use mediastat_core::{NodeFlags, NodeId, ScanTree};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderDelta {
    pub path: PathBuf,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub delta_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotDiff {
    pub files_before: u64,
    pub files_after: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub delta_bytes: i64,
    pub newly_added_files: Vec<(PathBuf, u64)>,
    pub removed_files: Vec<(PathBuf, u64)>,
    pub top_growing_folders: Vec<FolderDelta>,
    pub top_shrinking_folders: Vec<FolderDelta>,
}

/// Compare two `ScanTree` snapshots ("Before" vs "After") to audit storage growth and cleanup impact.
pub fn compute_diff(before: &ScanTree, after: &ScanTree) -> SnapshotDiff {
    let sb = before.summary();
    let sa = after.summary();

    // Map all live files by path
    let mut before_files: AHashMap<PathBuf, u64> = AHashMap::new();
    for i in 1..before.len() {
        let f = before.flags[i];
        if !f.contains(NodeFlags::IS_DIR) && !f.contains(NodeFlags::IS_DELETED) {
            before_files.insert(
                before.resolve_path(NodeId(i as u32)),
                before.logical_sizes[i],
            );
        }
    }

    let mut after_files: AHashMap<PathBuf, u64> = AHashMap::new();
    for i in 1..after.len() {
        let f = after.flags[i];
        if !f.contains(NodeFlags::IS_DIR) && !f.contains(NodeFlags::IS_DELETED) {
            after_files.insert(after.resolve_path(NodeId(i as u32)), after.logical_sizes[i]);
        }
    }

    let mut newly_added_files: Vec<(PathBuf, u64)> = after_files
        .iter()
        .filter(|(p, _)| !before_files.contains_key(*p))
        .map(|(p, &sz)| (p.clone(), sz))
        .collect();
    newly_added_files.sort_unstable_by_key(|a| std::cmp::Reverse(a.1));
    newly_added_files.truncate(500);

    let mut removed_files: Vec<(PathBuf, u64)> = before_files
        .iter()
        .filter(|(p, _)| !after_files.contains_key(*p))
        .map(|(p, &sz)| (p.clone(), sz))
        .collect();
    removed_files.sort_unstable_by_key(|a| std::cmp::Reverse(a.1));
    removed_files.truncate(500);

    // Compare directory sizes
    let mut before_dirs: AHashMap<PathBuf, u64> = AHashMap::new();
    for i in 1..before.len() {
        if before.flags[i].contains(NodeFlags::IS_DIR)
            && !before.flags[i].contains(NodeFlags::IS_DELETED)
        {
            let nid = NodeId(i as u32);
            before_dirs.insert(before.resolve_path(nid), before.logical_sizes[i]);
        }
    }

    let mut after_dirs: AHashMap<PathBuf, u64> = AHashMap::new();
    for i in 1..after.len() {
        if after.flags[i].contains(NodeFlags::IS_DIR)
            && !after.flags[i].contains(NodeFlags::IS_DELETED)
        {
            let nid = NodeId(i as u32);
            after_dirs.insert(after.resolve_path(nid), after.logical_sizes[i]);
        }
    }

    let mut deltas = Vec::new();
    for (path, &sz_after) in &after_dirs {
        let sz_before = before_dirs.get(path).copied().unwrap_or(0);
        let delta = sz_after as i64 - sz_before as i64;
        if delta != 0 {
            deltas.push(FolderDelta {
                path: path.clone(),
                bytes_before: sz_before,
                bytes_after: sz_after,
                delta_bytes: delta,
            });
        }
    }
    for (path, &sz_before) in &before_dirs {
        if !after_dirs.contains_key(path) && sz_before > 0 {
            deltas.push(FolderDelta {
                path: path.clone(),
                bytes_before: sz_before,
                bytes_after: 0,
                delta_bytes: -(sz_before as i64),
            });
        }
    }

    let mut top_growing: Vec<FolderDelta> = deltas
        .iter()
        .filter(|d| d.delta_bytes > 0)
        .cloned()
        .collect();
    top_growing.sort_unstable_by_key(|a| std::cmp::Reverse(a.delta_bytes));
    top_growing.truncate(50);

    let mut top_shrinking: Vec<FolderDelta> = deltas
        .into_iter()
        .filter(|d| d.delta_bytes < 0)
        .collect();
    top_shrinking.sort_unstable_by_key(|a| a.delta_bytes);
    top_shrinking.truncate(50);

    SnapshotDiff {
        files_before: sb.total_files,
        files_after: sa.total_files,
        bytes_before: sb.total_logical_bytes,
        bytes_after: sa.total_logical_bytes,
        delta_bytes: sa.total_logical_bytes as i64 - sb.total_logical_bytes as i64,
        newly_added_files,
        removed_files,
        top_growing_folders: top_growing,
        top_shrinking_folders: top_shrinking,
    }
}
