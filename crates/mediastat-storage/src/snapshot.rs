use std::path::{Path, PathBuf};

use ahash::AHashMap;
use anyhow::{Context, Result};
use mediastat_core::{
    DuplicateGroup, NodeFlags, NodeId, PermissionEntry, ScanIssue, ScanTree, VolumeSpaceInfo,
    WinAttrs,
};
use rusqlite::{params, Connection};

const SCHEMA_VERSION: u32 = 3;

/// Persist a complete `ScanTree` into a compact SQLite WAL `.mstat` snapshot file (v3 lean schema).
pub fn save_snapshot(tree: &ScanTree, path: &Path) -> Result<()> {
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    let mut conn = Connection::open(path)
        .with_context(|| format!("No se pudo crear el snapshot {}", path.display()))?;

    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA temp_store = MEMORY;
        PRAGMA cache_size = -64000;

        CREATE TABLE meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE extensions (
            id INTEGER PRIMARY KEY,
            ext TEXT NOT NULL
        );

        CREATE TABLE owners (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL
        );

        CREATE TABLE nodes (
            id INTEGER PRIMARY KEY,
            parent_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            ext_id INTEGER NOT NULL,
            flags INTEGER NOT NULL,
            win_attrs INTEGER NOT NULL DEFAULT 0,
            owner_id INTEGER NOT NULL DEFAULT 0,
            logical_size INTEGER NOT NULL,
            allocated_size INTEGER NOT NULL,
            mtime INTEGER NOT NULL
        );

        CREATE TABLE duplicate_groups (
            group_id INTEGER PRIMARY KEY,
            blake3_hex TEXT NOT NULL,
            file_size INTEGER NOT NULL,
            is_probable_sparse INTEGER NOT NULL DEFAULT 0,
            node_ids_json TEXT NOT NULL
        );

        CREATE TABLE extras (
            key TEXT PRIMARY KEY,
            value_json TEXT NOT NULL
        );
        ",
    )?;

    let tx = conn.transaction()?;
    {
        let mut stmt_meta = tx.prepare("INSERT INTO meta(key, value) VALUES (?1, ?2)")?;
        stmt_meta.execute(params!["schema_version", SCHEMA_VERSION.to_string()])?;
        stmt_meta.execute(params!["scanned_at_unix", tree.scanned_at_unix.to_string()])?;
        stmt_meta.execute(params!["engine_used", &tree.engine_used])?;
        stmt_meta.execute(params!["scan_duration_secs", tree.scan_duration_secs.to_string()])?;
        stmt_meta.execute(params![
            "root_targets_json",
            serde_json::to_string(&tree.root_targets)?
        ])?;
    }

    {
        let mut stmt_ext = tx.prepare("INSERT INTO extensions(id, ext) VALUES (?1, ?2)")?;
        for e in tree.extensions.entries() {
            stmt_ext.execute(params![e.id as i64, &e.ext])?;
        }
    }

    {
        let mut stmt_own = tx.prepare("INSERT INTO owners(id, name) VALUES (?1, ?2)")?;
        for (idx, name) in tree.owners.all().iter().enumerate() {
            stmt_own.execute(params![idx as i64, name])?;
        }
    }

    {
        let mut stmt_node = tx.prepare(
            "INSERT INTO nodes(id, parent_id, name, ext_id, flags, win_attrs, owner_id, logical_size, allocated_size, mtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        for i in 1..tree.len() {
            let nid = NodeId(i as u32);
            let parent = tree.parents[i].0 as i64;
            stmt_node.execute(params![
                i as i64,
                parent,
                tree.name(nid),
                tree.ext_ids[i] as i64,
                tree.flags[i].bits() as i64,
                tree.win_attrs[i].bits() as i64,
                tree.owner_ids[i] as i64,
                tree.logical_sizes[i] as i64,
                tree.allocated_sizes[i] as i64,
                tree.mtimes[i],
            ])?;
        }
    }

    {
        let mut stmt_dup = tx.prepare(
            "INSERT INTO duplicate_groups(group_id, blake3_hex, file_size, is_probable_sparse, node_ids_json) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for (idx, grp) in tree.duplicates.iter().enumerate() {
            let ids: Vec<u32> = grp.nodes.iter().map(|n| n.0).collect();
            stmt_dup.execute(params![
                idx as i64,
                &grp.blake3_hex,
                grp.file_size as i64,
                if grp.is_probable_sparse { 1i64 } else { 0i64 },
                serde_json::to_string(&ids)?
            ])?;
        }
    }

    {
        let mut stmt_ex = tx.prepare("INSERT INTO extras(key, value_json) VALUES (?1, ?2)")?;
        let vol_vec: Vec<(u32, &VolumeSpaceInfo)> = tree
            .volume_space
            .iter()
            .map(|(k, v)| (k.0, v))
            .collect();
        stmt_ex.execute(params!["volume_space", serde_json::to_string(&vol_vec)?])?;
        stmt_ex.execute(params!["issues", serde_json::to_string(&tree.issues)?])?;
        stmt_ex.execute(params![
            "permissions",
            serde_json::to_string(&tree.permissions)?
        ])?;
    }

    tx.commit()?;

    conn.execute_batch(
        "
        CREATE INDEX idx_nodes_parent ON nodes(parent_id);
        CREATE INDEX idx_nodes_size ON nodes(logical_size DESC);
        PRAGMA wal_checkpoint(TRUNCATE);
        ",
    )?;

    Ok(())
}

/// Load a `.mstat` SQLite snapshot (seamlessly supports v1, v2, and v3 schemas).
pub fn load_snapshot(path: &Path) -> Result<ScanTree> {
    let conn = Connection::open(path)
        .with_context(|| format!("No se pudo abrir el snapshot {}", path.display()))?;

    let node_count: usize = conn.query_row("SELECT COUNT(*) FROM nodes", [], |r| {
        Ok(r.get::<_, i64>(0)? as usize)
    })?;

    let mut tree = ScanTree::with_capacity(node_count + 2);
    let mut schema_ver: u32 = 1;

    // Load metadata
    {
        let mut stmt = conn.prepare("SELECT key, value FROM meta")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (k, v) = row?;
            match k.as_str() {
                "schema_version" => schema_ver = v.parse().unwrap_or(1),
                "scanned_at_unix" => {
                    if let Ok(ts) = v.parse::<i64>() {
                        tree.scanned_at_unix = ts;
                    }
                }
                "engine_used" => tree.engine_used = v,
                "scan_duration_secs" => {
                    if let Ok(d) = v.parse::<f64>() {
                        tree.scan_duration_secs = d;
                    }
                }
                "root_targets_json" => {
                    if let Ok(targets) = serde_json::from_str::<Vec<PathBuf>>(&v) {
                        tree.root_targets = targets;
                    }
                }
                _ => {}
            }
        }
    }

    // Load owner dictionary if v2+
    let mut owner_names: Vec<String> = vec![String::new()];
    if schema_ver >= 2 {
        if let Ok(mut stmt) = conn.prepare("SELECT id, name FROM owners ORDER BY id ASC") {
            if let Ok(rows) = stmt.query_map([], |r| {
                Ok((r.get::<_, i64>(0)? as usize, r.get::<_, String>(1)?))
            }) {
                for (id, name) in rows.flatten() {
                    if id >= owner_names.len() {
                        owner_names.resize(id + 1, String::new());
                    }
                    owner_names[id] = name;
                }
            }
        }
    }

    // Load nodes (retrocompatible with v1, v2, and v3 schemas)
    if schema_ver >= 2 {
        let mut stmt = conn.prepare(
            "SELECT id, parent_id, name, flags, win_attrs, owner_id, logical_size, allocated_size, mtime
             FROM nodes ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)? as u32,
                r.get::<_, i64>(1)? as u32,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)? as u16,
                r.get::<_, i64>(4)? as u16,
                r.get::<_, i64>(5)? as usize,
                r.get::<_, i64>(6)? as u64,
                r.get::<_, i64>(7)? as u64,
                r.get::<_, i64>(8)?,
            ))
        })?;
        for row in rows {
            let (_id, parent_raw, name, flags_bits, attr_bits, owner_idx, logical, alloc, mtime) =
                row?;
            let flags = NodeFlags::from_bits_truncate(flags_bits);
            let attrs = WinAttrs::from_bits_truncate(attr_bits);
            let nid = tree.add_child_with_attrs(
                NodeId(parent_raw),
                &name,
                flags,
                attrs,
                logical,
                alloc,
                mtime,
            );
            if let Some(oname) = owner_names.get(owner_idx) {
                if !oname.is_empty() {
                    tree.set_owner(nid, oname);
                }
            }
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, parent_id, name, flags, logical_size, allocated_size, mtime
             FROM nodes ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)? as u32,
                r.get::<_, i64>(1)? as u32,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)? as u16,
                r.get::<_, i64>(4)? as u64,
                r.get::<_, i64>(5)? as u64,
                r.get::<_, i64>(6)?,
            ))
        })?;
        for row in rows {
            let (_id, parent_raw, name, flags_bits, logical, alloc, mtime) = row?;
            let flags = NodeFlags::from_bits_truncate(flags_bits);
            tree.add_child(NodeId(parent_raw), &name, flags, logical, alloc, mtime);
        }
    }

    // Load duplicate groups
    {
        let sql = if schema_ver >= 2 {
            "SELECT blake3_hex, file_size, is_probable_sparse, node_ids_json FROM duplicate_groups ORDER BY group_id ASC"
        } else {
            "SELECT blake3_hex, file_size, 0, node_ids_json FROM duplicate_groups ORDER BY group_id ASC"
        };
        if let Ok(mut stmt) = conn.prepare(sql) {
            if let Ok(rows) = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, i64>(2)? != 0,
                    r.get::<_, String>(3)?,
                ))
            }) {
                for row in rows.flatten() {
                    let (hex, file_size, is_probable_sparse, ids_json) = row;
                    if let Ok(ids) = serde_json::from_str::<Vec<u32>>(&ids_json) {
                        let nodes: Vec<NodeId> = ids.into_iter().map(NodeId).collect();
                        tree.duplicates.push(DuplicateGroup {
                            blake3_hex: hex,
                            file_size,
                            nodes,
                            is_probable_sparse,
                        });
                    }
                }
            }
        }
    }

    // Load v2+ extras (volume_space, issues, permissions)
    if schema_ver >= 2 {
        if let Ok(mut stmt) = conn.prepare("SELECT key, value_json FROM extras") {
            if let Ok(rows) =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            {
                for (k, json) in rows.flatten() {
                    match k.as_str() {
                        "volume_space" => {
                            if let Ok(vec) = serde_json::from_str::<Vec<(u32, VolumeSpaceInfo)>>(&json) {
                                for (id, info) in vec {
                                    tree.volume_space.insert(NodeId(id), info);
                                }
                            }
                        }
                        "issues" => {
                            if let Ok(iss) = serde_json::from_str::<Vec<ScanIssue>>(&json) {
                                tree.issues = iss;
                            }
                        }
                        "permissions" => {
                            if let Ok(perms) = serde_json::from_str::<Vec<PermissionEntry>>(&json) {
                                tree.permissions = perms;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    tree.recompute_aggregates();
    Ok(tree)
}

/// Reconstruct a `ScanTree` from an exported CSV report (WinDirStat `CsvLoader` parity).
pub fn load_csv_snapshot(path: &Path) -> Result<ScanTree> {
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .with_context(|| format!("No se pudo abrir CSV {}", path.display()))?;

    let headers: Vec<String> = rdr
        .headers()?
        .iter()
        .map(|h| h.trim_start_matches('\u{feff}').to_ascii_lowercase())
        .collect();

    let col_idx = |candidates: &[&str]| -> Option<usize> {
        candidates
            .iter()
            .find_map(|c| headers.iter().position(|h| h == c))
    };

    let idx_path = col_idx(&["full_path", "name", "path"])
        .context("El CSV no contiene columna 'full_path' ni 'name'")?;
    let idx_size = col_idx(&["logical_size_bytes", "size", "size_logical"]);
    let idx_alloc = col_idx(&["allocated_size_bytes", "size_physical", "allocated"]);
    let idx_owner = col_idx(&["owner"]);

    let mut tree = ScanTree::new();
    tree.engine_used = format!(
        "Importado desde CSV ({})",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    let mut dir_map: AHashMap<PathBuf, NodeId> = AHashMap::new();

    for rec in rdr.records().flatten() {
        let raw_path = rec.get(idx_path).unwrap_or("").trim();
        if raw_path.is_empty() {
            continue;
        }
        let pb = PathBuf::from(raw_path);
        let logical = idx_size
            .and_then(|i| rec.get(i))
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let alloc = idx_alloc
            .and_then(|i| rec.get(i))
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(logical);

        let mut comps: Vec<String> = Vec::new();
        for c in pb.components() {
            let s = c.as_os_str().to_string_lossy().into_owned();
            if !s.is_empty() && s != "\\" && s != "/" {
                comps.push(s);
            }
        }
        if comps.is_empty() {
            continue;
        }

        let mut cur_parent = NodeId::ROOT;
        let mut cur_path = PathBuf::new();
        for (idx, comp) in comps.iter().enumerate() {
            let is_last = idx + 1 == comps.len();
            if idx == 0 && comp.ends_with(':') {
                cur_path.push(format!(r"{comp}\"));
            } else {
                cur_path.push(comp);
            }
            if !is_last {
                if let Some(&existing) = dir_map.get(&cur_path) {
                    cur_parent = existing;
                } else {
                    let nid = if cur_parent == NodeId::ROOT {
                        tree.add_root_target(&cur_path)
                    } else {
                        tree.add_child(cur_parent, comp, NodeFlags::IS_DIR, 0, 0, 0)
                    };
                    dir_map.insert(cur_path.clone(), nid);
                    cur_parent = nid;
                }
            } else {
                let fid = tree.add_child(cur_parent, comp, NodeFlags::empty(), logical, alloc, 0);
                if let Some(o) = idx_owner.and_then(|i| rec.get(i)).filter(|s| !s.is_empty()) {
                    tree.set_owner(fid, o);
                }
            }
        }
    }

    tree.recompute_aggregates();
    Ok(tree)
}
