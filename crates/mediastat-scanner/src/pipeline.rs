use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

use mediastat_core::{t, CompiledFilters, NodeId, ScanConfig, ScanTree};

use crate::drives::query_volume_space;
use crate::duplicates::find_exact_duplicates;
use crate::ntfs_mft::try_scan_ntfs_mft;
use crate::permissions::resolve_owners_for_nodes;
use crate::walker::{
    apply_process_priority, enable_backup_restore_privileges, scan_into_existing_dir,
    scan_target_parallel, WalkerCounters, WalkerPolicy,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ScanPhase {
    Idle = 0,
    ScanningFilesystem = 1,
    FindingDuplicates = 2,
    ResolvingOwners = 3,
    Aggregating = 4,
    Completed = 5,
    Cancelled = 6,
}

impl ScanPhase {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::ScanningFilesystem,
            2 => Self::FindingDuplicates,
            3 => Self::ResolvingOwners,
            4 => Self::Aggregating,
            5 => Self::Completed,
            6 => Self::Cancelled,
            _ => Self::Idle,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => t("Inactivo", "Idle"),
            Self::ScanningFilesystem => t(
                "Fase 1: Escaneando directorios y atributos...",
                "Phase 1: Scanning directories and attributes...",
            ),
            Self::FindingDuplicates => t(
                "Fase 2: Detectando duplicados (BLAKE3)...",
                "Phase 2: Detecting duplicates (BLAKE3)...",
            ),
            Self::ResolvingOwners => t(
                "Fase 3: Resolviendo propietarios (DOMINIO\\usuario)...",
                "Phase 3: Resolving file owners (DOMAIN\\user)...",
            ),
            Self::Aggregating => t(
                "Calculando totales, extensiones y mapa...",
                "Computing totals, extensions, and map...",
            ),
            Self::Completed => t("Escaneo completado", "Scan completed"),
            Self::Cancelled => t("Escaneo cancelado", "Scan cancelled"),
        }
    }
}

#[derive(Debug)]
pub struct ScanProgress {
    pub phase: AtomicU8,
    pub walker: Arc<WalkerCounters>,
    pub dup_total: Arc<AtomicU64>,
    pub dup_done: Arc<AtomicU64>,
    pub cancel: Arc<AtomicBool>,
    pub pause: Arc<AtomicBool>,
}

impl Default for ScanProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanProgress {
    pub fn new() -> Self {
        Self {
            phase: AtomicU8::new(ScanPhase::Idle as u8),
            walker: Arc::new(WalkerCounters::default()),
            dup_total: Arc::new(AtomicU64::new(0)),
            dup_done: Arc::new(AtomicU64::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
            pause: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn current_phase(&self) -> ScanPhase {
        ScanPhase::from_u8(self.phase.load(Ordering::Relaxed))
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn set_paused(&self, paused: bool) {
        self.pause.store(paused, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.pause.load(Ordering::Relaxed)
    }
}

/// Run the full multi-target WinDirStat-style scan pipeline synchronously (typically on a worker thread).
pub fn run_scan_pipeline(config: &ScanConfig, progress: &Arc<ScanProgress>) -> ScanTree {
    let start = Instant::now();
    if config.use_backup_restore {
        enable_backup_restore_privileges();
    }
    apply_process_priority(config.process_priority);

    let mut tree = ScanTree::with_capacity(65_536);
    let mut engines_used: Vec<&'static str> = Vec::new();
    let policy = WalkerPolicy::from(config);

    let filters = Arc::new(
        CompiledFilters::compile(&config.filter_config, &config.excluded_dir_names)
            .unwrap_or_default(),
    );

    // =========================================================================
    // PHASE 1: Filesystem Discovery (NTFS $MFT or High-Speed Parallel Walker)
    // =========================================================================
    progress
        .phase
        .store(ScanPhase::ScanningFilesystem as u8, Ordering::Relaxed);

    for target in &config.targets {
        if progress.cancel.load(Ordering::Relaxed) {
            break;
        }
        let mut used_mft = false;
        if config.use_ntfs_mft {
            match try_scan_ntfs_mft(
                &mut tree,
                target,
                &config.excluded_dir_names,
                &progress.walker,
                &progress.cancel,
            ) {
                Ok(root_id) => {
                    if let Some(vs) = query_volume_space(target) {
                        tree.volume_space.insert(root_id, vs);
                    }
                    used_mft = true;
                    if !engines_used.contains(&"NTFS $MFT") {
                        engines_used.push("NTFS $MFT");
                    }
                }
                Err(reason) => {
                    tracing::info!("Fallback a Walker paralelo para {:?}: {}", target, reason);
                }
            }
        }
        if !used_mft {
            let eng_lbl = if cfg!(windows) {
                t(
                    "NtQueryDirectoryFile (Multihilo)",
                    "NtQueryDirectoryFile (Multi-threaded)",
                )
            } else {
                t("Paralelo Multihilo", "Parallel Multi-threaded")
            };
            if !engines_used.contains(&eng_lbl) {
                engines_used.push(eng_lbl);
            }
            scan_target_parallel(
                &mut tree,
                target,
                &filters,
                policy,
                config.scan_threads,
                &progress.walker,
                &progress.cancel,
                &progress.pause,
            );
        }
    }

    // =========================================================================
    // PHASE 2 (Optional): Duplicate Detection (3-Stage BLAKE3 / Sparse >64 MiB)
    // =========================================================================
    if config.find_duplicates && !progress.cancel.load(Ordering::Relaxed) {
        progress
            .phase
            .store(ScanPhase::FindingDuplicates as u8, Ordering::Relaxed);
        tree.duplicates = find_exact_duplicates(
            &mut tree,
            config.duplicate_min_size_bytes,
            config.sparse_duplicate_hash,
            &progress.dup_done,
            &progress.dup_total,
            &progress.cancel,
        );
    }

    // =========================================================================
    // PHASE 3 (Optional): Resolve NTFS/SMB File Owners (DOMAIN\user)
    // =========================================================================
    if config.resolve_owners_during_scan && !progress.cancel.load(Ordering::Relaxed) {
        progress
            .phase
            .store(ScanPhase::ResolvingOwners as u8, Ordering::Relaxed);
        let all_nodes: Vec<NodeId> = (1..tree.len()).map(|i| NodeId(i as u32)).collect();
        resolve_owners_for_nodes(&mut tree, &all_nodes, None, &progress.cancel);
    }

    // =========================================================================
    // FINAL: NTFS Hardlink Deduplication + Bottom-up Aggregation + Rank Colors
    // =========================================================================
    progress
        .phase
        .store(ScanPhase::Aggregating as u8, Ordering::Relaxed);
    if config.process_hardlinks {
        tree.deduplicate_hardlinks();
    } else {
        tree.recompute_aggregates();
    }
    tree.scan_duration_secs = start.elapsed().as_secs_f64();
    tree.engine_used = if engines_used.is_empty() {
        "Paralelo Multihilo".into()
    } else {
        engines_used.join(" + ")
    };

    let final_phase = if progress.cancel.load(Ordering::Relaxed) {
        ScanPhase::Cancelled
    } else {
        ScanPhase::Completed
    };
    progress.phase.store(final_phase as u8, Ordering::Relaxed);
    tree
}

/// Selective `F5` Rescan of a single directory node without re-scanning the entire drive!
/// Detaches stale children, re-walks only `dir_id`, updates volume free space if root,
/// and recomputes aggregates.
pub fn rescan_directory_node(
    tree: &mut ScanTree,
    dir_id: NodeId,
    config: &ScanConfig,
    progress: &Arc<ScanProgress>,
) {
    if dir_id == NodeId::ROOT || dir_id.index() >= tree.len() || !tree.is_dir(dir_id) {
        return;
    }
    let dir_path = tree.resolve_path(dir_id);
    if !dir_path.exists() {
        tree.mark_node_deleted(dir_id);
        return;
    }

    let filters = Arc::new(
        CompiledFilters::compile(&config.filter_config, &config.excluded_dir_names)
            .unwrap_or_default(),
    );

    progress
        .phase
        .store(ScanPhase::ScanningFilesystem as u8, Ordering::Relaxed);

    tree.detach_children(dir_id);

    let policy = WalkerPolicy::from(config);
    scan_into_existing_dir(
        tree,
        dir_id,
        &dir_path,
        &filters,
        policy,
        config.scan_threads,
        &progress.walker,
        &progress.cancel,
        &progress.pause,
    );

    if let Some(vs) = query_volume_space(&dir_path) {
        tree.volume_space.insert(dir_id, vs);
    }

    progress
        .phase
        .store(ScanPhase::Aggregating as u8, Ordering::Relaxed);
    if config.process_hardlinks {
        tree.deduplicate_hardlinks();
    } else {
        tree.recompute_aggregates();
    }
    progress
        .phase
        .store(ScanPhase::Completed as u8, Ordering::Relaxed);
}
