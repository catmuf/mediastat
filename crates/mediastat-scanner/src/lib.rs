pub mod drives;
pub mod duplicates;
pub mod ntfs_mft;
pub mod permissions;
pub mod pipeline;
pub mod walker;
pub mod watcher;

pub use drives::{
    detect_path_media_type, enumerate_drives, is_process_elevated, query_recycle_bin,
    query_volume_space, resolve_to_unc_path, whole_volume_drive_letter, DriveInfo, DriveMediaType,
};
pub use duplicates::{compute_file_hashes, FileHashes};
pub use permissions::{resolve_owner_single, resolve_owners_for_nodes, scan_permissions_for_scope};
pub use pipeline::{rescan_directory_node, run_scan_pipeline, ScanPhase, ScanProgress};
pub use watcher::{FileWatcher, WatchAction, WatchEvent};

#[cfg(test)]
mod tests {
    use super::*;
    use mediastat_core::{NodeId, ScanConfig};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn test_full_scan_pipeline_f5_rescan_owners_perms_hashes_and_watcher() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();

        let sub = dir1.path().join("Proyectos");
        std::fs::create_dir_all(&sub).unwrap();

        let file1 = sub.join("plano_v1.dwg");
        std::fs::write(&file1, vec![77u8; 120_000]).unwrap();
        // Create a real NTFS hardlink to verify NtQueryDirectoryFile FileId + Hardlink deduplication!
        let hl_path = dir1.path().join("plano_hardlink.dwg");
        let created_hardlink = std::fs::hard_link(&file1, &hl_path).is_ok();
        std::fs::write(dir2.path().join("copia_plano.dwg"), vec![77u8; 120_000]).unwrap();
        std::fs::write(dir2.path().join("otro_plano.dwg"), vec![99u8; 120_000]).unwrap();
        std::fs::write(dir1.path().join("notas.txt"), vec![b'a'; 80_000]).unwrap();

        let config = ScanConfig {
            targets: vec![dir1.path().to_path_buf(), dir2.path().to_path_buf()],
            use_ntfs_mft: false,
            find_duplicates: true,
            duplicate_min_size_bytes: 1024,
            ..Default::default()
        };

        let prog = Arc::new(ScanProgress::new());
        let mut tree = run_scan_pipeline(&config, &prog);

        let sum = tree.summary();
        let expected_files = if created_hardlink { 5 } else { 4 };
        assert_eq!(sum.total_files, expected_files);
        assert_eq!(sum.duplicate_groups, 1);

        #[cfg(windows)]
        if created_hardlink {
            assert_eq!(
                tree.hardlinks.len(),
                1,
                "Expected NtQueryDirectoryFile to extract matching FileId and deduplicate hardlink!"
            );
            assert_eq!(tree.hardlinks[0].nodes.len(), 2);
            assert!(
                sum.total_allocated_bytes < sum.total_logical_bytes,
                "Hardlink deduplication must avoid double-counting physical allocated bytes"
            );
        }

        // Test single-file multi-hashes (MD5 / SHA-256 / BLAKE3)
        let hashes = compute_file_hashes(&file1).unwrap();
        assert_eq!(hashes.md5.len(), 32);
        assert_eq!(hashes.sha256.len(), 64);
        assert_eq!(hashes.blake3.len(), 64);

        // Test Win32 owner & DACL permission scanner on Windows
        #[cfg(windows)]
        {
            let owner = resolve_owner_single(&file1);
            assert!(owner.is_some(), "Expected Windows owner for temp file");
            let cancel = Arc::new(AtomicBool::new(false));
            let perms = scan_permissions_for_scope(&tree, NodeId::ROOT, true, 50, &cancel);
            assert!(!perms.is_empty(), "Expected DACL ACE entries on Windows");
        }

        // Test F5 selective subtree rescan after adding a new file in `sub`
        let sub_id = tree
            .children(NodeId::ROOT)
            .flat_map(|r| tree.children(r))
            .find(|&n| tree.name(n) == "Proyectos")
            .unwrap();
        std::fs::write(sub.join("nuevo_plano.dwg"), vec![55u8; 90_000]).unwrap();
        let prog_f5 = Arc::new(ScanProgress::new());
        rescan_directory_node(&mut tree, sub_id, &config, &prog_f5);
        assert_eq!(tree.summary().total_files, expected_files + 1);

        // Verify live FileWatcher picks up newly written file on Windows
        #[cfg(windows)]
        {
            let mut watcher = FileWatcher::start(vec![dir2.path().to_path_buf()]);
            std::thread::sleep(std::time::Duration::from_millis(120));
            std::fs::write(dir2.path().join("live_watch.txt"), b"hello watcher").unwrap();
            let ev = watcher.rx.recv_timeout(std::time::Duration::from_secs(2));
            watcher.stop();
            assert!(ev.is_ok(), "FileWatcher should receive real-time event");
        }
    }
}
