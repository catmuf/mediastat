use ahash::AHashSet;
use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mediastat_core::{
    format_bytes, format_count, format_timestamp_opts, hardlinks_color, is_light_rgb, t,
    AppSettings, FileCategory, NodeFlags, NodeId, ScanTree, SizeMetric,
};

use crate::selection::{show_node_actions_menu, MenuScope, NodeAction, SelectionState};
use crate::treemap::extension_display_color;
use crate::views::sort_header_button;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeSortColumn {
    Percent,
    Size,
    Allocated,
    Name,
    Files,
    Attributes,
    Owner,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeRowKind {
    Normal(NodeId),
    VirtualFilesGroup {
        parent: NodeId,
        count: u64,
        logical: u64,
        allocated: u64,
    },
    HardlinksContainer {
        volume_root: NodeId,
        group_count: u64,
        link_count: u64,
        physical_bytes: u64,
    },
    HardlinkGroupRow {
        volume_root: NodeId,
        file_id: u64,
        link_count: u64,
        physical_bytes: u64,
    },
    FreeSpace {
        root: NodeId,
        bytes: u64,
    },
    UnknownSpace {
        root: NodeId,
        bytes: u64,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct FlattenedRow {
    pub kind: TreeRowKind,
    pub depth: u16,
    pub is_dir: bool,
    pub expanded: bool,
}

pub struct TreeViewState {
    pub expanded: AHashSet<NodeId>,
    pub expanded_files_groups: AHashSet<NodeId>,
    pub expanded_hardlinks_vols: AHashSet<NodeId>,
    pub expanded_hardlink_groups: AHashSet<(NodeId, u64)>,
    pub sort_col: TreeSortColumn,
    pub sort_desc: bool,
    pub rows: Vec<FlattenedRow>,
    pub visible_nodes: Vec<NodeId>,
    pub dirty: bool,
    pub last_tree_gen: u64,
    pub pending_scroll_to: Option<NodeId>,
    last_group_files: bool,
    last_show_free: bool,
    last_show_unknown: bool,
    last_metric: SizeMetric,
}

impl Default for TreeViewState {
    fn default() -> Self {
        let mut expanded = AHashSet::new();
        expanded.insert(NodeId::ROOT);
        Self {
            expanded,
            expanded_files_groups: AHashSet::new(),
            expanded_hardlinks_vols: AHashSet::new(),
            expanded_hardlink_groups: AHashSet::new(),
            sort_col: TreeSortColumn::Size,
            sort_desc: true,
            rows: Vec::new(),
            visible_nodes: Vec::new(),
            dirty: true,
            last_tree_gen: 0,
            pending_scroll_to: None,
            last_group_files: false,
            last_show_free: false,
            last_show_unknown: false,
            last_metric: SizeMetric::Logical,
        }
    }
}

impl TreeViewState {
    pub fn reset_for_new_tree(&mut self, tree: &ScanTree, gen: u64) {
        self.expanded.clear();
        self.expanded_files_groups.clear();
        self.expanded_hardlinks_vols.clear();
        self.expanded_hardlink_groups.clear();
        self.expanded.insert(NodeId::ROOT);
        for root_target in tree.children(NodeId::ROOT) {
            if !tree.is_deleted(root_target) {
                self.expanded.insert(root_target);
            }
        }
        self.last_tree_gen = gen;
        self.dirty = true;
    }

    /// Expand all ancestors and queue auto-scroll so clicking Treemap/Sunburst/FlameGraph jumps right to the row.
    pub fn reveal_and_scroll(&mut self, tree: &ScanTree, node: NodeId) {
        if node.is_none() || node.index() >= tree.len() {
            return;
        }
        let mut cur = node;
        let parent_of_leaf = tree.parents[node.index()];
        if !tree.is_dir(node) && parent_of_leaf.is_some() {
            self.expanded_files_groups.insert(parent_of_leaf);
        }
        while cur.is_some() {
            let parent = tree.parents[cur.index()];
            if parent.is_some() {
                self.expanded.insert(parent);
            }
            cur = parent;
        }
        self.pending_scroll_to = Some(node);
        self.dirty = true;
    }

    fn rebuild_rows(&mut self, tree: &ScanTree, settings: &AppSettings) {
        self.rows.clear();
        self.visible_nodes.clear();
        if tree.is_empty() {
            return;
        }
        self.push_children_recursive(tree, NodeId::ROOT, 0, settings);
        for r in &self.rows {
            if let TreeRowKind::Normal(nid) = r.kind {
                self.visible_nodes.push(nid);
            }
        }
        self.dirty = false;
        self.last_group_files = settings.group_files_node;
        self.last_show_free = settings.show_free_space;
        self.last_show_unknown = settings.show_unknown_space;
        self.last_metric = settings.size_metric;
    }

    fn push_children_recursive(
        &mut self,
        tree: &ScanTree,
        parent: NodeId,
        depth: u16,
        settings: &AppSettings,
    ) {
        let mut dirs: Vec<NodeId> = Vec::new();
        let mut files: Vec<NodeId> = Vec::new();
        for c in tree.children(parent) {
            if tree.is_deleted(c) {
                continue;
            }
            if tree.flags[c.index()].contains(NodeFlags::IS_DIR) {
                dirs.push(c);
            } else {
                files.push(c);
            }
        }

        // Group direct files under virtual `<Archivos>` node when enabled & directory has subdirs
        let use_virtual_group = settings.group_files_node && !dirs.is_empty() && !files.is_empty();

        if use_virtual_group {
            self.sort_nodes(tree, &mut dirs, settings.size_metric);
            self.sort_nodes(tree, &mut files, settings.size_metric);

            for d in dirs {
                let is_exp = self.expanded.contains(&d);
                self.rows.push(FlattenedRow {
                    kind: TreeRowKind::Normal(d),
                    depth,
                    is_dir: true,
                    expanded: is_exp,
                });
                if is_exp {
                    self.push_children_recursive(tree, d, depth + 1, settings);
                }
            }

            let mut f_log = 0u64;
            let mut f_alloc = 0u64;
            for &f in &files {
                f_log = f_log.saturating_add(tree.logical_sizes[f.index()]);
                f_alloc = f_alloc.saturating_add(tree.allocated_sizes[f.index()]);
            }
            let grp_exp = self.expanded_files_groups.contains(&parent);
            self.rows.push(FlattenedRow {
                kind: TreeRowKind::VirtualFilesGroup {
                    parent,
                    count: files.len() as u64,
                    logical: f_log,
                    allocated: f_alloc,
                },
                depth,
                is_dir: true,
                expanded: grp_exp,
            });
            if grp_exp {
                for f in files {
                    self.rows.push(FlattenedRow {
                        kind: TreeRowKind::Normal(f),
                        depth: depth + 1,
                        is_dir: false,
                        expanded: false,
                    });
                }
            }
        } else {
            let mut all = dirs;
            all.extend(files);
            self.sort_nodes(tree, &mut all, settings.size_metric);
            for child in all {
                let is_dir = tree.flags[child.index()].contains(NodeFlags::IS_DIR);
                let is_exp = is_dir && self.expanded.contains(&child);
                self.rows.push(FlattenedRow {
                    kind: TreeRowKind::Normal(child),
                    depth,
                    is_dir,
                    expanded: is_exp,
                });
                if is_exp {
                    self.push_children_recursive(tree, child, depth + 1, settings);
                }
            }
        }

        // WinDirStat `<Hardlinks>` virtual container under volume roots that have deduplicated NTFS hardlinks
        if parent != NodeId::ROOT && tree.flags[parent.index()].contains(NodeFlags::IS_ROOT_TARGET) {
            let hl_groups = tree.hardlinks_for_volume(parent);
            if !hl_groups.is_empty() {
                let group_count = hl_groups.len() as u64;
                let link_count: u64 = hl_groups.iter().map(|g| g.nodes.len() as u64).sum();
                let physical_bytes: u64 = hl_groups.iter().map(|g| g.physical_size).sum();
                let hl_exp = self.expanded_hardlinks_vols.contains(&parent);
                self.rows.push(FlattenedRow {
                    kind: TreeRowKind::HardlinksContainer {
                        volume_root: parent,
                        group_count,
                        link_count,
                        physical_bytes,
                    },
                    depth,
                    is_dir: true,
                    expanded: hl_exp,
                });
                if hl_exp {
                    for grp in hl_groups {
                        let g_exp = self
                            .expanded_hardlink_groups
                            .contains(&(parent, grp.file_id));
                        self.rows.push(FlattenedRow {
                            kind: TreeRowKind::HardlinkGroupRow {
                                volume_root: parent,
                                file_id: grp.file_id,
                                link_count: grp.nodes.len() as u64,
                                physical_bytes: grp.physical_size,
                            },
                            depth: depth + 1,
                            is_dir: true,
                            expanded: g_exp,
                        });
                        if g_exp {
                            for &nid in &grp.nodes {
                                if !tree.is_deleted(nid) {
                                    self.rows.push(FlattenedRow {
                                        kind: TreeRowKind::Normal(nid),
                                        depth: depth + 2,
                                        is_dir: false,
                                        expanded: false,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        // WinDirStat F6 `<Espacio Libre>` & F7 `<Espacio Desconocido>` rows under volume roots
        if let Some(vs) = tree.volume_space.get(&parent) {
            if settings.show_free_space && vs.free_bytes > 0 {
                self.rows.push(FlattenedRow {
                    kind: TreeRowKind::FreeSpace {
                        root: parent,
                        bytes: vs.free_bytes,
                    },
                    depth,
                    is_dir: false,
                    expanded: false,
                });
            }
            if settings.show_unknown_space {
                let unk = vs.unknown_bytes(tree.allocated_sizes[parent.index()]);
                if unk > 0 {
                    self.rows.push(FlattenedRow {
                        kind: TreeRowKind::UnknownSpace {
                            root: parent,
                            bytes: unk,
                        },
                        depth,
                        is_dir: false,
                        expanded: false,
                    });
                }
            }
        }
    }

    fn sort_nodes(&self, tree: &ScanTree, nodes: &mut [NodeId], metric: SizeMetric) {
        let desc = self.sort_desc;
        let col = self.sort_col;
        nodes.sort_unstable_by(|&a, &b| {
            let ord = match col {
                TreeSortColumn::Size | TreeSortColumn::Percent => tree
                    .size_for_metric(a, metric)
                    .cmp(&tree.size_for_metric(b, metric)),
                TreeSortColumn::Allocated => {
                    tree.allocated_sizes[a.index()].cmp(&tree.allocated_sizes[b.index()])
                }
                TreeSortColumn::Name => tree
                    .name(a)
                    .to_ascii_lowercase()
                    .cmp(&tree.name(b).to_ascii_lowercase()),
                TreeSortColumn::Files => {
                    let fa = tree
                        .dir_stats
                        .get(&a)
                        .map(|d| d.total_files)
                        .unwrap_or(1);
                    let fb = tree
                        .dir_stats
                        .get(&b)
                        .map(|d| d.total_files)
                        .unwrap_or(1);
                    fa.cmp(&fb)
                }
                TreeSortColumn::Attributes => tree.win_attrs[a.index()]
                    .bits()
                    .cmp(&tree.win_attrs[b.index()].bits()),
                TreeSortColumn::Owner => tree
                    .owner(a)
                    .to_ascii_lowercase()
                    .cmp(&tree.owner(b).to_ascii_lowercase()),
                TreeSortColumn::Modified => tree.mtimes[a.index()].cmp(&tree.mtimes[b.index()]),
            };
            if desc {
                ord.reverse()
            } else {
                ord
            }
        });
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        tree: &ScanTree,
        tree_gen: u64,
        selection: &mut SelectionState,
        settings: &AppSettings,
        read_only_locked: bool,
    ) -> Option<NodeAction> {
        if self.last_tree_gen != tree_gen {
            self.reset_for_new_tree(tree, tree_gen);
        }
        if self.dirty
            || self.last_group_files != settings.group_files_node
            || self.last_show_free != settings.show_free_space
            || self.last_show_unknown != settings.show_unknown_space
            || self.last_metric != settings.size_metric
        {
            self.rebuild_rows(tree, settings);
        }

        let scroll_row_idx = self.pending_scroll_to.take().and_then(|target| {
            self.rows
                .iter()
                .position(|r| matches!(r.kind, TreeRowKind::Normal(id) if id == target))
        });

        let mut action: Option<NodeAction> = None;
        let mut pending_click: Option<(NodeId, egui::Modifiers)> = None;
        let mut pending_check: Option<(NodeId, bool)> = None;
        let mut pending_rclick: Option<NodeId> = None;
        let mut master_toggle: Option<bool> = None;
        let mut toggle_dir: Option<NodeId> = None;
        let mut toggle_vgroup: Option<NodeId> = None;
        let mut toggle_hl_vol: Option<NodeId> = None;
        let mut toggle_hl_group: Option<(NodeId, u64)> = None;
        let mut sort_clicked: Option<TreeSortColumn> = None;

        let total_rows = self.rows.len();
        let all_visible_selected = !self.visible_nodes.is_empty()
            && self.visible_nodes.iter().all(|&n| selection.contains(n));
        let total_scan_bytes = tree
            .size_for_metric(NodeId::ROOT, settings.size_metric)
            .max(1);
        let cols = settings.tree_columns_visible;

        let mut builder = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(24.0)) // Select checkbox
            .column(Column::initial(265.0).at_least(140.0).clip(true)); // Col 0: Name (always visible)

        if cols[1] {
            builder = builder.column(Column::initial(110.0).at_least(70.0)); // Subtree bar
        }
        if cols[2] {
            builder = builder.column(Column::initial(84.0).at_least(64.0)); // Logical
        }
        if cols[3] {
            builder = builder.column(Column::initial(84.0).at_least(64.0)); // Allocated on disk
        }
        if cols[4] {
            builder = builder.column(Column::initial(74.0).at_least(54.0)); // Files / Type
        }
        if cols[5] {
            builder = builder.column(Column::initial(60.0).at_least(44.0)); // Attributes
        }
        if cols[6] {
            builder = builder.column(Column::initial(115.0).at_least(65.0).clip(true)); // Owner
        }
        if cols[7] {
            builder = builder.column(Column::remainder().at_least(100.0)); // Modified
        }

        let extra_empty_cols_after_alloc: usize =
            usize::from(cols[4]) + usize::from(cols[5]) + usize::from(cols[6]) + usize::from(cols[7]);

        if let Some(row_idx) = scroll_row_idx {
            builder = builder.scroll_to_row(row_idx, Some(egui::Align::Center));
        }

        builder
            .header(22.0, |mut header| {
                header.col(|ui| {
                    let mut all_chk = all_visible_selected;
                    if ui
                        .checkbox(&mut all_chk, "")
                        .on_hover_text(t(
                            "Seleccionar / deseleccionar todo en el árbol visible (Ctrl+A)",
                            "Select / deselect all visible tree items (Ctrl+A)",
                        ))
                        .changed()
                    {
                        master_toggle = Some(all_chk);
                    }
                });
                let mut hdr_btn =
                    |col: &mut egui_extras::TableRow<'_, '_>, label: &str, sort: TreeSortColumn| {
                        col.col(|ui| {
                            if sort_header_button(ui, label, self.sort_col == sort, self.sort_desc) {
                                sort_clicked = Some(sort);
                            }
                        });
                    };
                hdr_btn(
                    &mut header,
                    t("Nombre / Carpeta", "Name / Folder"),
                    TreeSortColumn::Name,
                );
                if cols[1] {
                    hdr_btn(
                        &mut header,
                        if settings.absolute_percentages {
                            t("% Total", "% Total")
                        } else {
                            t("% Subárbol", "% Subtree")
                        },
                        TreeSortColumn::Percent,
                    );
                }
                if cols[2] {
                    hdr_btn(&mut header, t("Tamaño", "Size"), TreeSortColumn::Size);
                }
                if cols[3] {
                    hdr_btn(&mut header, t("En Disco", "On Disk"), TreeSortColumn::Allocated);
                }
                if cols[4] {
                    hdr_btn(&mut header, t("Ficheros", "Files"), TreeSortColumn::Files);
                }
                if cols[5] {
                    hdr_btn(&mut header, t("Atrib.", "Attrs"), TreeSortColumn::Attributes);
                }
                if cols[6] {
                    hdr_btn(&mut header, t("Propietario", "Owner"), TreeSortColumn::Owner);
                }
                if cols[7] {
                    hdr_btn(&mut header, t("Modificado", "Modified"), TreeSortColumn::Modified);
                }
            })
            .body(|body| {
                body.rows(22.0, total_rows, |mut row| {
                    let item = self.rows[row.index()];
                    match item.kind {
                        TreeRowKind::Normal(nid) => {
                            let idx = nid.index();
                            let is_sel = selection.contains(nid);
                            row.set_selected(is_sel);

                            // Checkbox column
                            row.col(|ui| {
                                let mut chk = is_sel;
                                if ui.checkbox(&mut chk, "").changed() {
                                    pending_check = Some((nid, chk));
                                }
                            });

                            let active_sz = tree.size_for_metric(nid, settings.size_metric);
                            let parent = tree.parents[idx];
                            let parent_sz = if parent.is_some() {
                                tree.size_for_metric(parent, settings.size_metric).max(1)
                            } else {
                                active_sz.max(1)
                            };
                            let pct_parent = (active_sz as f32 / parent_sz as f32).clamp(0.0, 1.0);
                            let pct_total =
                                (active_sz as f32 / total_scan_bytes as f32).clamp(0.0, 1.0);
                            let bar_pct = if settings.absolute_percentages {
                                pct_total
                            } else {
                                pct_parent
                            };

                            // Col 0: Name + chevron
                            row.col(|ui| {
                                ui.add_space((item.depth as f32) * 13.0);
                                if item.is_dir {
                                    let icon = if item.expanded { "▾" } else { "▸" };
                                    if ui.small_button(icon).clicked() {
                                        toggle_dir = Some(nid);
                                    }
                                } else {
                                    ui.add_space(18.0);
                                }

                                let flags = tree.flags[idx];
                                let is_denied = flags.contains(NodeFlags::ACCESS_DENIED);
                                let is_hl_dup = flags.contains(NodeFlags::IS_HARDLINK_DUP);
                                let badge_col = if is_denied {
                                    Color32::from_rgb(235, 75, 75)
                                } else if is_hl_dup {
                                    let [r, g, b] = hardlinks_color();
                                    Color32::from_rgb(r, g, b)
                                } else if item.is_dir {
                                    Color32::from_rgb(236, 193, 86)
                                } else {
                                    let rgb = extension_display_color(
                                        tree,
                                        tree.ext_ids[idx],
                                        &settings.treemap,
                                    );
                                    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
                                };
                                let glyph = if is_denied {
                                    "⛔"
                                } else if is_hl_dup {
                                    "🔗"
                                } else if flags.contains(NodeFlags::IS_MOUNT_POINT) {
                                    "💽"
                                } else if flags.contains(NodeFlags::IS_JUNCTION)
                                    || flags.contains(NodeFlags::IS_SYMLINK)
                                {
                                    "↪"
                                } else if item.is_dir {
                                    "📁"
                                } else {
                                    "●"
                                };
                                ui.label(RichText::new(glyph).color(badge_col));

                                let mut display = tree.name(nid).to_string();
                                if let Some(vs) = tree.volume_space.get(&nid) {
                                    if let Some(unc) = &vs.unc_remote_path {
                                        display = format!("{display} ({unc})");
                                    }
                                }

                                let label_resp = ui.selectable_label(is_sel, display);
                                if label_resp.clicked() {
                                    pending_click = Some((nid, ui.input(|i| i.modifiers)));
                                }
                                if label_resp.secondary_clicked() {
                                    pending_rclick = Some(nid);
                                }
                                if label_resp.double_clicked() && item.is_dir {
                                    action = Some(NodeAction::ZoomTreemap(nid));
                                }
                                label_resp.context_menu(|ui| {
                                    let targets = selection.context_targets_for(nid);
                                    if let Some(act) = show_node_actions_menu(
                                        ui,
                                        tree,
                                        &targets,
                                        Some(MenuScope {
                                            visible_nodes: &self.visible_nodes,
                                            allow_tree_ops: true,
                                        }),
                                        settings,
                                        read_only_locked,
                                    ) {
                                        action = Some(act);
                                    }
                                });
                            });

                            // Col 1: Classic 8-level WinDirStat depth bar OR MediaStat bicolor bar
                            if cols[1] {
                                row.col(|ui| {
                                    let (rect, resp) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width().clamp(40.0, 96.0), 14.0),
                                        egui::Sense::hover(),
                                    );
                                    let painter = ui.painter();
                                    painter.rect_filled(rect, 3.0, Color32::from_gray(42));
                                    let fill_w = rect.width() * bar_pct;
                                    let fill_rgb = if settings.tree_bar_classic_depth_colors {
                                        settings.tree_depth_colors[(item.depth as usize) % 8]
                                    } else {
                                        [58, 128, 206]
                                    };
                                    painter.rect_filled(
                                        egui::Rect::from_min_size(
                                            rect.min,
                                            egui::vec2(fill_w, rect.height()),
                                        ),
                                        3.0,
                                        Color32::from_rgb(fill_rgb[0], fill_rgb[1], fill_rgb[2]),
                                    );
                                    let txt_col = if bar_pct > 0.45 && is_light_rgb(fill_rgb) {
                                        Color32::from_rgb(16, 20, 28)
                                    } else {
                                        Color32::WHITE
                                    };
                                    painter.text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        format!("{:.1}%", bar_pct * 100.0),
                                        egui::FontId::proportional(10.5),
                                        txt_col,
                                    );
                                    resp.on_hover_text(format!(
                                        "{:.2}% {} · {:.2}% {}",
                                        pct_parent * 100.0,
                                        t("de la carpeta padre", "of parent folder"),
                                        pct_total * 100.0,
                                        t("del escaneo total", "of total scan"),
                                    ));
                                });
                            }

                            // Col 2: Logical Size
                            if cols[2] {
                                row.col(|ui| {
                                    ui.label(format_bytes(tree.logical_sizes[idx]));
                                });
                            }

                            // Col 3: Allocated Size on Disk
                            if cols[3] {
                                row.col(|ui| {
                                    if tree.flags[idx].contains(NodeFlags::IS_HARDLINK_DUP) {
                                        ui.label(
                                            RichText::new("0 B (hl)")
                                                .small()
                                                .color(Color32::from_rgb(200, 160, 110)),
                                        )
                                        .on_hover_text(t(
                                            "Enlace físico (Hardlink NTFS) ya contabilizado en el enlace primario",
                                            "NTFS Hardlink already counted on its primary link",
                                        ));
                                    } else {
                                        ui.label(format_bytes(tree.allocated_sizes[idx]));
                                    }
                                });
                            }

                            // Col 4: Files (for dirs) / Category (for files)
                            if cols[4] {
                                row.col(|ui| {
                                    if item.is_dir {
                                        let fcnt = tree
                                            .dir_stats
                                            .get(&nid)
                                            .map(|d| d.total_files)
                                            .unwrap_or(0);
                                        ui.label(format_count(fcnt));
                                    } else {
                                        let cat = FileCategory::from_extension(tree.ext(nid));
                                        let [r, g, b] = cat.accent_rgb();
                                        ui.label(
                                            RichText::new(cat.short_label())
                                                .small()
                                                .color(Color32::from_rgb(r, g, b)),
                                        );
                                    }
                                });
                            }

                            // Col 5: Win32 Attributes
                            if cols[5] {
                                row.col(|ui| {
                                    let attrs = tree.win_attrs[idx].to_compact_string();
                                    if attrs.is_empty() {
                                        ui.label(RichText::new("—").weak());
                                    } else {
                                        ui.label(RichText::new(attrs).small().monospace());
                                    }
                                });
                            }

                            // Col 6: Owner
                            if cols[6] {
                                row.col(|ui| {
                                    let own = tree.owner(nid);
                                    if own.is_empty() {
                                        ui.label(RichText::new("—").weak());
                                    } else {
                                        ui.label(RichText::new(own).small());
                                    }
                                });
                            }

                            // Col 7: Modified
                            if cols[7] {
                                row.col(|ui| {
                                    let ts = if item.is_dir {
                                        tree.dir_stats
                                            .get(&nid)
                                            .map(|d| d.max_mtime)
                                            .unwrap_or(tree.mtimes[idx])
                                    } else {
                                        tree.mtimes[idx]
                                    };
                                    ui.label(format_timestamp_opts(
                                        ts,
                                        settings.show_timestamp_seconds,
                                    ));
                                });
                            }
                        }

                        TreeRowKind::VirtualFilesGroup {
                            parent,
                            count,
                            logical,
                            allocated,
                        } => {
                            row.col(|_| {});
                            row.col(|ui| {
                                ui.add_space((item.depth as f32) * 13.0);
                                let icon = if item.expanded { "▾" } else { "▸" };
                                if ui.small_button(icon).clicked() {
                                    toggle_vgroup = Some(parent);
                                }
                                ui.label(
                                    RichText::new(format!(
                                        "📄 {} ({})",
                                        t("<Archivos>", "<Files>"),
                                        format_count(count)
                                    ))
                                    .italics()
                                    .color(Color32::from_rgb(175, 200, 235)),
                                );
                            });
                            if cols[1] {
                                row.col(|ui| {
                                    let psz = tree
                                        .size_for_metric(parent, settings.size_metric)
                                        .max(1);
                                    let pct = (logical as f32 / psz as f32) * 100.0;
                                    ui.label(format!("{pct:.1}%"));
                                });
                            }
                            if cols[2] {
                                row.col(|ui| {
                                    ui.label(format_bytes(logical));
                                });
                            }
                            if cols[3] {
                                row.col(|ui| {
                                    ui.label(format_bytes(allocated));
                                });
                            }
                            if cols[4] {
                                row.col(|ui| {
                                    ui.label(format_count(count));
                                });
                            }
                            if cols[5] {
                                row.col(|_| {});
                            }
                            if cols[6] {
                                row.col(|_| {});
                            }
                            if cols[7] {
                                row.col(|_| {});
                            }
                        }

                        TreeRowKind::HardlinksContainer {
                            volume_root,
                            group_count,
                            link_count,
                            physical_bytes,
                        } => {
                            let [hr, hg, hb] = hardlinks_color();
                            let hcol = Color32::from_rgb(hr, hg, hb);
                            row.col(|_| {});
                            row.col(|ui| {
                                ui.add_space((item.depth as f32) * 13.0);
                                let icon = if item.expanded { "▾" } else { "▸" };
                                if ui.small_button(icon).clicked() {
                                    toggle_hl_vol = Some(volume_root);
                                }
                                ui.label(
                                    RichText::new(format!(
                                        "🔗 <Hardlinks> ({} {} · {} {})",
                                        format_count(group_count),
                                        t("grupos", "groups"),
                                        format_count(link_count),
                                        t("enlaces", "links")
                                    ))
                                    .strong()
                                    .color(hcol),
                                )
                                .on_hover_text(t(
                                    "Grupos de enlaces físicos NTFS deduplicados por FileId (sin doble conteo en disco)",
                                    "NTFS Hardlink groups deduplicated by FileId (zero physical double-counting)",
                                ));
                            });
                            if cols[1] {
                                row.col(|ui| {
                                    let vsz = tree.allocated_sizes[volume_root.index()].max(1);
                                    let pct = (physical_bytes as f64 / vsz as f64) * 100.0;
                                    ui.label(format!("{pct:.1}%"));
                                });
                            }
                            if cols[2] {
                                row.col(|ui| {
                                    ui.label(RichText::new("dedup.").weak().small());
                                });
                            }
                            if cols[3] {
                                row.col(|ui| {
                                    ui.label(
                                        RichText::new(format_bytes(physical_bytes)).color(hcol),
                                    );
                                });
                            }
                            for _ in 0..extra_empty_cols_after_alloc {
                                row.col(|_| {});
                            }
                        }

                        TreeRowKind::HardlinkGroupRow {
                            volume_root,
                            file_id,
                            link_count,
                            physical_bytes,
                        } => {
                            let [hr, hg, hb] = hardlinks_color();
                            let hcol = Color32::from_rgb(hr, hg, hb);
                            row.col(|_| {});
                            row.col(|ui| {
                                ui.add_space((item.depth as f32) * 13.0);
                                let icon = if item.expanded { "▾" } else { "▸" };
                                if ui.small_button(icon).clicked() {
                                    toggle_hl_group = Some((volume_root, file_id));
                                }
                                ui.label(
                                    RichText::new(format!(
                                        "🔗 FileId 0x{file_id:012X} ({link_count} {})",
                                        t("enlaces", "links")
                                    ))
                                    .small()
                                    .monospace()
                                    .color(hcol),
                                );
                            });
                            if cols[1] {
                                row.col(|_| {});
                            }
                            if cols[2] {
                                row.col(|_| {});
                            }
                            if cols[3] {
                                row.col(|ui| {
                                    ui.label(format_bytes(physical_bytes));
                                });
                            }
                            for _ in 0..extra_empty_cols_after_alloc {
                                row.col(|_| {});
                            }
                        }

                        TreeRowKind::FreeSpace { root, bytes } => {
                            let total_v = tree
                                .volume_space
                                .get(&root)
                                .map(|v| v.total_bytes)
                                .unwrap_or(bytes)
                                .max(1);
                            let pct = (bytes as f64 / total_v as f64) * 100.0;
                            row.col(|_| {});
                            row.col(|ui| {
                                ui.add_space((item.depth as f32) * 13.0 + 18.0);
                                ui.label(
                                    RichText::new(t("🟢 <Espacio Libre>", "🟢 <Free Space>"))
                                        .strong()
                                        .color(Color32::from_rgb(76, 210, 128)),
                                );
                            });
                            if cols[1] {
                                row.col(|ui| {
                                    ui.label(format!("{pct:.1}% vol."));
                                });
                            }
                            if cols[2] {
                                row.col(|ui| {
                                    ui.label(format_bytes(bytes));
                                });
                            }
                            if cols[3] {
                                row.col(|ui| {
                                    ui.label(format_bytes(bytes));
                                });
                            }
                            for _ in 0..extra_empty_cols_after_alloc {
                                row.col(|_| {});
                            }
                        }

                        TreeRowKind::UnknownSpace { root, bytes } => {
                            let total_v = tree
                                .volume_space
                                .get(&root)
                                .map(|v| v.total_bytes)
                                .unwrap_or(bytes)
                                .max(1);
                            let pct = (bytes as f64 / total_v as f64) * 100.0;
                            row.col(|_| {});
                            row.col(|ui| {
                                ui.add_space((item.depth as f32) * 13.0 + 18.0);
                                ui.label(
                                    RichText::new(t(
                                        "🟡 <Espacio Desconocido>",
                                        "🟡 <Unknown Space>",
                                    ))
                                    .strong()
                                    .color(Color32::from_rgb(235, 196, 75)),
                                )
                                .on_hover_text(t(
                                    "Espacio ocupado en el volumen no visible en el escaneo: Shadow Copies (VSS), $MFT, USN Journal, System Volume Information o carpetas sin permisos.",
                                    "Volume space not accounted for by scanned files: Shadow Copies (VSS), $MFT, USN Journal, System Volume Information, or inaccessible folders.",
                                ));
                            });
                            if cols[1] {
                                row.col(|ui| {
                                    ui.label(format!("{pct:.1}% vol."));
                                });
                            }
                            if cols[2] {
                                row.col(|ui| {
                                    ui.label(format_bytes(bytes));
                                });
                            }
                            if cols[3] {
                                row.col(|ui| {
                                    ui.label(format_bytes(bytes));
                                });
                            }
                            for _ in 0..extra_empty_cols_after_alloc {
                                row.col(|_| {});
                            }
                        }
                    }
                });
            });

        if let Some(chk_all) = master_toggle {
            if chk_all {
                selection.select_all(&self.visible_nodes);
            } else {
                selection.clear();
            }
        }
        if let Some((nid, chk)) = pending_check {
            selection.set_checked(nid, chk);
        }
        if let Some((nid, mods)) = pending_click {
            selection.handle_click(nid, mods, &self.visible_nodes);
        }
        if let Some(nid) = pending_rclick {
            selection.ensure_for_context_menu(nid);
        }
        if let Some(dir) = toggle_dir {
            if !self.expanded.remove(&dir) {
                self.expanded.insert(dir);
            }
            self.dirty = true;
        }
        if let Some(vg) = toggle_vgroup {
            if !self.expanded_files_groups.remove(&vg) {
                self.expanded_files_groups.insert(vg);
            }
            self.dirty = true;
        }
        if let Some(vol) = toggle_hl_vol {
            if !self.expanded_hardlinks_vols.remove(&vol) {
                self.expanded_hardlinks_vols.insert(vol);
            }
            self.dirty = true;
        }
        if let Some(grp) = toggle_hl_group {
            if !self.expanded_hardlink_groups.remove(&grp) {
                self.expanded_hardlink_groups.insert(grp);
            }
            self.dirty = true;
        }
        if let Some(col) = sort_clicked {
            if self.sort_col == col {
                self.sort_desc = !self.sort_desc;
            } else {
                self.sort_col = col;
                self.sort_desc = !matches!(
                    col,
                    TreeSortColumn::Name | TreeSortColumn::Attributes | TreeSortColumn::Owner
                );
            }
            self.dirty = true;
        }

        action
    }
}
