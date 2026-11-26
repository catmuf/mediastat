use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use ahash::AHashMap;
use md5::Digest as _;
use mediastat_core::{DuplicateGroup, NodeFlags, NodeId, ScanTree, WinAttrs};
use rayon::prelude::*;

const PARTIAL_WINDOW: usize = 16 * 1024;
const SPARSE_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024; // 64 MiB (WinDirStat 2.9+ sparse hash threshold)
const SPARSE_SAMPLE_WINDOW: usize = 64 * 1024;
const STREAM_BUF: usize = 64 * 1024;

/// Multi-algorithm cryptographic digest summary for the Inspector ("Compute Hashes").
#[derive(Debug, Clone)]
pub struct FileHashes {
    pub md5: String,
    pub sha256: String,
    pub blake3: String,
}

/// Compute MD5, SHA-256, and BLAKE3 in a single sequential streaming pass.
pub fn compute_file_hashes(path: &Path) -> std::io::Result<FileHashes> {
    let mut f = File::open(path)?;
    let mut md5_h = md5::Md5::new();
    let mut sha_h = sha2::Sha256::new();
    let mut b3_h = blake3::Hasher::new();
    let mut buf = vec![0u8; STREAM_BUF];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        md5_h.update(chunk);
        sha_h.update(chunk);
        b3_h.update(chunk);
    }
    Ok(FileHashes {
        md5: format!("{:x}", md5_h.finalize()),
        sha256: format!("{:x}", sha_h.finalize()),
        blake3: b3_h.finalize().to_hex().to_string(),
    })
}

/// 3-stage parallel duplicate detector using SIMD-accelerated `BLAKE3` with Anti-Hydration Cloud
/// safeguard and optional WinDirStat 2.9+ Sparse Hashing for files >= 64 MiB.
pub fn find_exact_duplicates(
    tree: &mut ScanTree,
    min_size_bytes: u64,
    sparse_large_files: bool,
    progress_done: &Arc<AtomicU64>,
    progress_total: &Arc<AtomicU64>,
    cancel: &Arc<AtomicBool>,
) -> Vec<DuplicateGroup> {
    // Stage 1: Group non-deleted, non-cloud candidate files by exact byte size
    let mut by_size: AHashMap<u64, Vec<NodeId>> = AHashMap::new();
    for i in 1..tree.len() {
        let f = tree.flags[i];
        if f.contains(NodeFlags::IS_DIR)
            || f.contains(NodeFlags::IS_SYMLINK)
            || f.contains(NodeFlags::IS_DELETED)
            || tree.win_attrs[i].contains(WinAttrs::OFFLINE_CLOUD)
        {
            continue;
        }
        let sz = tree.logical_sizes[i];
        if sz >= min_size_bytes {
            by_size.entry(sz).or_default().push(NodeId(i as u32));
        }
    }

    let size_candidates: Vec<(NodeId, u64, PathBuf)> = by_size
        .into_iter()
        .filter(|(_, nodes)| nodes.len() > 1)
        .flat_map(|(sz, nodes)| {
            nodes
                .into_iter()
                .map(|id| (id, sz, tree.resolve_path(id)))
                .collect::<Vec<_>>()
        })
        .collect();

    if size_candidates.is_empty() || cancel.load(Ordering::Relaxed) {
        return Vec::new();
    }

    progress_total.store(size_candidates.len() as u64, Ordering::Relaxed);
    progress_done.store(0, Ordering::Relaxed);

    // Stage 2: Partial hash (first 16 KB + last 16 KB) in parallel
    let partial_results: Vec<(NodeId, u64, PathBuf, [u8; 32])> = size_candidates
        .into_par_iter()
        .filter_map(|(id, sz, path)| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let h = partial_blake3(&path, sz).ok()?;
            progress_done.fetch_add(1, Ordering::Relaxed);
            Some((id, sz, path, h))
        })
        .collect();

    type PartialGroupMap = AHashMap<(u64, [u8; 32]), Vec<(NodeId, PathBuf)>>;
    let mut by_partial: PartialGroupMap = AHashMap::new();
    for (id, sz, path, ph) in partial_results {
        by_partial.entry((sz, ph)).or_default().push((id, path));
    }

    let full_candidates: Vec<(NodeId, u64, PathBuf)> = by_partial
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .flat_map(|((sz, _), v)| v.into_iter().map(move |(id, p)| (id, sz, p)))
        .collect();

    if full_candidates.is_empty() || cancel.load(Ordering::Relaxed) {
        return Vec::new();
    }

    // Stage 3: Full-file BLAKE3 (or fast Sparse 3-point sample for files >= 64 MiB when enabled)
    let full_results: Vec<(NodeId, u64, [u8; 32], bool)> = full_candidates
        .into_par_iter()
        .filter_map(|(id, sz, path)| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let use_sparse = sparse_large_files && sz >= SPARSE_THRESHOLD_BYTES;
            let h = if use_sparse {
                sparse_blake3(&path, sz).ok()?
            } else {
                full_blake3(&path).ok()?
            };
            Some((id, sz, h, use_sparse))
        })
        .collect();

    let mut by_full: AHashMap<(u64, [u8; 32], bool), Vec<NodeId>> = AHashMap::new();
    for (id, sz, fh, is_sparse) in full_results {
        by_full.entry((sz, fh, is_sparse)).or_default().push(id);
    }

    let mut groups: Vec<DuplicateGroup> = by_full
        .into_iter()
        .filter(|(_, nodes)| nodes.len() > 1)
        .map(|((sz, hash, is_probable_sparse), nodes)| {
            for &nid in &nodes {
                tree.flags[nid.index()].insert(NodeFlags::IS_DUPLICATE);
            }
            DuplicateGroup {
                blake3_hex: blake3::Hash::from(hash).to_hex().to_string(),
                file_size: sz,
                nodes,
                is_probable_sparse,
            }
        })
        .collect();

    groups.sort_unstable_by_key(|g| std::cmp::Reverse(g.reclaimable_bytes()));
    groups
}

fn partial_blake3(path: &PathBuf, file_size: u64) -> std::io::Result<[u8; 32]> {
    let mut f = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; PARTIAL_WINDOW];

    let head_n = f.read(&mut buf)?;
    hasher.update(&buf[..head_n]);

    if file_size > (PARTIAL_WINDOW as u64) * 2 {
        f.seek(SeekFrom::End(-(PARTIAL_WINDOW as i64)))?;
        let tail_n = f.read(&mut buf)?;
        hasher.update(&buf[..tail_n]);
    } else if file_size > head_n as u64 {
        let mut rest = Vec::new();
        f.read_to_end(&mut rest)?;
        hasher.update(&rest);
    }

    Ok(*hasher.finalize().as_bytes())
}

fn sparse_blake3(path: &PathBuf, file_size: u64) -> std::io::Result<[u8; 32]> {
    let mut f = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; SPARSE_SAMPLE_WINDOW];

    // Sample head, 1/3, 2/3, and tail (fast O(1) I/O for multi-GB videos/VHDs)
    let offsets = [
        0u64,
        file_size / 3,
        (file_size * 2) / 3,
        file_size.saturating_sub(SPARSE_SAMPLE_WINDOW as u64),
    ];
    for off in offsets {
        f.seek(SeekFrom::Start(off))?;
        let n = f.read(&mut buf)?;
        hasher.update(&buf[..n]);
    }
    hasher.update(&file_size.to_le_bytes());
    Ok(*hasher.finalize().as_bytes())
}

fn full_blake3(path: &PathBuf) -> std::io::Result<[u8; 32]> {
    let mut f = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; STREAM_BUF];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(*hasher.finalize().as_bytes())
}
