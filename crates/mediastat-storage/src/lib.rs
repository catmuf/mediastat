pub mod diff;
pub mod export;
pub mod html_report;
pub mod snapshot;

pub use diff::{compute_diff, FolderDelta, SnapshotDiff};
pub use export::{export_nodes, export_nodes_with_cost, ExportFormat};
pub use html_report::write_executive_html_report;
pub use snapshot::{load_csv_snapshot, load_snapshot, save_snapshot};

#[cfg(test)]
mod tests {
    use super::*;
    use mediastat_core::{NodeFlags, ScanTree, SizeMetric, WinAttrs};
    use rusqlite::Connection;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn test_snapshot_v3_legacy_v2_compat_csv_html_and_diff() {
        let dir = tempdir().unwrap();

        let mut tree1 = ScanTree::new();
        let root = tree1.add_root_target(Path::new(r"D:\Filestorage"));
        let f1 = tree1.add_child_with_attrs(
            root,
            "plano.dwg",
            NodeFlags::empty(),
            WinAttrs::ARCHIVE | WinAttrs::COMPRESSED,
            5_000_000,
            4_200_000,
            1_700_000_000,
        );
        tree1.set_owner(f1, r"CORP\admin");
        tree1.recompute_aggregates();

        let snap1 = dir.path().join("before.mstat");
        save_snapshot(&tree1, &snap1).unwrap();
        let loaded = load_snapshot(&snap1).unwrap();
        assert_eq!(loaded.summary().total_files, 1);
        assert_eq!(loaded.owner(f1), r"CORP\admin");
        assert_eq!(loaded.win_attrs[f1.index()].to_compact_string(), "AC");

        // Verify legacy v2 `.mstat` snapshots (with extra status/media_kind & media_reports) still load cleanly
        let legacy_v2 = dir.path().join("legacy_v2.mstat");
        {
            let conn = Connection::open(&legacy_v2).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                INSERT INTO meta VALUES ('schema_version', '2');
                INSERT INTO meta VALUES ('engine_used', 'Legacy v2');
                CREATE TABLE owners (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
                INSERT INTO owners VALUES (0, ''), (1, 'CORP\\legacy');
                CREATE TABLE nodes (
                    id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL, name TEXT NOT NULL,
                    ext_id INTEGER NOT NULL, flags INTEGER NOT NULL, win_attrs INTEGER NOT NULL,
                    owner_id INTEGER NOT NULL, logical_size INTEGER NOT NULL,
                    allocated_size INTEGER NOT NULL, mtime INTEGER NOT NULL,
                    status INTEGER NOT NULL, media_kind INTEGER NOT NULL
                );
                INSERT INTO nodes VALUES (1, 0, 'D:\\Old', 0, 3, 0, 0, 0, 0, 0, 0, 0);
                INSERT INTO nodes VALUES (2, 1, 'archive.zip', 1, 0, 8, 1, 123456, 123456, 1700000000, 1, 1);
                CREATE TABLE media_reports (node_id INTEGER PRIMARY KEY, estimated_savings INTEGER NOT NULL, report_json TEXT NOT NULL);
                CREATE TABLE duplicate_groups (group_id INTEGER PRIMARY KEY, blake3_hex TEXT NOT NULL, file_size INTEGER NOT NULL, is_probable_sparse INTEGER NOT NULL, node_ids_json TEXT NOT NULL);
                CREATE TABLE extras (key TEXT PRIMARY KEY, value_json TEXT NOT NULL);
                ",
            )
            .unwrap();
        }
        let legacy_loaded = load_snapshot(&legacy_v2).unwrap();
        assert_eq!(legacy_loaded.summary().total_files, 1);
        assert_eq!(legacy_loaded.summary().total_logical_bytes, 123_456);

        // Diff before vs after
        let mut tree2 = tree1.clone();
        let root2 = tree2.children(mediastat_core::NodeId::ROOT).next().unwrap();
        tree2.add_child(root2, "backup.vhdx", NodeFlags::empty(), 20_000_000, 20_000_000, 1_710_000_000);
        tree2.recompute_aggregates();
        let diff = compute_diff(&loaded, &tree2);
        assert_eq!(diff.delta_bytes, 20_000_000);
        assert_eq!(diff.newly_added_files.len(), 1);
        assert!(diff.newly_added_files[0].0.to_string_lossy().contains("backup.vhdx"));

        // Export CSV & reload back into a ScanTree
        let all_files = loaded.all_files_sorted(SizeMetric::Logical);
        let csv_path = dir.path().join("out.csv");
        export_nodes(&loaded, &all_files, ExportFormat::Csv, &csv_path).unwrap();

        let from_csv = load_csv_snapshot(&csv_path).unwrap();
        assert_eq!(from_csv.summary().total_files, 1);
        assert_eq!(from_csv.summary().total_logical_bytes, 5_000_000);

        // Generate self-contained Executive HTML Report
        let html_path = dir.path().join("exec_report.html");
        export_nodes(&loaded, &all_files, ExportFormat::ExecutiveHtml, &html_path).unwrap();
        let html_txt = std::fs::read_to_string(&html_path).unwrap();
        assert!(html_txt.contains("<!DOCTYPE html>"));
        assert!(html_txt.contains("plano.dwg"));
        assert!(html_txt.contains(r"CORP\admin"));
    }
}
