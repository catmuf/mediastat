use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::filtering::FilterConfig;
use crate::settings::ProcessPriority;

/// Full configuration for launching a high-speed WinDirStat-style scan job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanConfig {
    pub targets: Vec<PathBuf>,
    /// Exclude folder names (case-insensitive exact match, e.g., `$RECYCLE.BIN`, `.git`).
    pub excluded_dir_names: Vec<String>,
    /// Rich WinDirStat 2.10 filtering rules (Regex/Glob, size, age, cloud protection).
    pub filter_config: FilterConfig,
    /// Attempt direct NTFS MFT scanning for whole local NTFS volumes (Windows Admin only, automatic fallback).
    pub use_ntfs_mft: bool,
    /// Run optional 3-stage BLAKE3 duplicate detection.
    pub find_duplicates: bool,
    /// Use fast sparse sampling for files > 64 MiB (WinDirStat 2.9+ "Probable duplicates").
    pub sparse_duplicate_hash: bool,
    /// Minimum file size for duplicate scanning (default 64 KB).
    pub duplicate_min_size_bytes: u64,
    /// Resolve NTFS/SMB file owner (`DOMAIN\user`) during scan (off by default for speed; also available on demand).
    pub resolve_owners_during_scan: bool,
    /// Max worker threads for filesystem traversal (0 = auto based on storage medium).
    pub scan_threads: usize,
    /// Deduplicate NTFS hardlinks by `FileId` (WinDirStat `ProcessHardlinks`, default true).
    #[serde(default = "default_true_scan")]
    pub process_hardlinks: bool,
    /// Enable `SE_BACKUP_NAME` / `SE_RESTORE_NAME` when elevated (WinDirStat `UseBackupRestore`).
    #[serde(default = "default_true_scan")]
    pub use_backup_restore: bool,
    /// Scan priority class (WinDirStat `PROCESSPRIORITY`).
    #[serde(default)]
    pub process_priority: ProcessPriority,
    /// WinDirStat reparse/attribute exclusions during traversal.
    #[serde(default = "default_true_scan")]
    pub exclude_junctions: bool,
    #[serde(default = "default_true_scan")]
    pub exclude_symlinks_dir: bool,
    #[serde(default = "default_true_scan")]
    pub exclude_symlinks_file: bool,
    #[serde(default = "default_true_scan")]
    pub exclude_mount_points: bool,
    #[serde(default)]
    pub exclude_hidden: bool,
    #[serde(default)]
    pub exclude_protected_system: bool,
}

fn default_true_scan() -> bool {
    true
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            excluded_dir_names: vec![
                "$RECYCLE.BIN".to_string(),
                "System Volume Information".to_string(),
            ],
            filter_config: FilterConfig::default(),
            use_ntfs_mft: true,
            find_duplicates: false,
            sparse_duplicate_hash: false,
            duplicate_min_size_bytes: 64 * 1024,
            resolve_owners_during_scan: false,
            scan_threads: 0,
            process_hardlinks: true,
            use_backup_restore: true,
            process_priority: ProcessPriority::Normal,
            exclude_junctions: true,
            exclude_symlinks_dir: true,
            exclude_symlinks_file: true,
            exclude_mount_points: true,
            exclude_hidden: false,
            exclude_protected_system: false,
        }
    }
}
