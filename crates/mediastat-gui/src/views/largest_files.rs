//! Dedicated "Top Largest Files" tab (WinDirStat 2.10 `FileTopControl` parity).

use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mediastat_core::{
    format_bytes, format_count, format_timestamp_opts, t, AppSettings, FileCategory, NodeId,
    ScanTree, SizeMetric,
};

use crate::selection::{show_node_actions_menu, MenuScope, NodeAction, SelectionState};
use crate::shell_actions::reveal_in_explorer;
use crate::views::sort_header_button;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LargestSortColumn {
    Rank,
    Size,
    Allocated,
    Percent,
    Name,
    Category,
    Attrs,
    Owner,
    Modified,
    FullPath,
}

pub struct LargestFilesState {
    pub limit: usize,
    pub ext_filter: String,
    pub sort_col: LargestSortColumn,
    pub sort_desc: bool,
    pub cached_nodes: Vec<NodeId>,
    pub dirty: bool,
    pub last_tree_gen: u64,
    pub last_metric: SizeMetric,
}

impl Default for LargestFilesState {
    fn default() -> Self {
        Self {
            limit: 1_000,
            ext_filter: String::new(),
            sort_col: LargestSortColumn::Size,
            sort_desc: true,
            cached_nodes: Vec::new(),
            dirty: true,
            last_tree_gen: 0,
            last_metric: SizeMetric::Logical,
        }
    }
}

impl LargestFilesState {
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn resort_cached(&mut self, tree: &ScanTree, metric: SizeMetric) {
        let col = self.sort_col;
        let desc = self.sort_desc;
        self.cached_nodes.sort_unstable_by(|&a, &b| {
            let ia = a.index();
            let ib = b.index();
            let ord = match col {
                LargestSortColumn::Rank => tree
                    .size_for_metric(b, metric)
                    .cmp(&tree.size_for_metric(a, metric)),
                LargestSortColumn::Size => tree.logical_sizes[ia].cmp(&tree.logical_sizes[ib]),
                LargestSortColumn::Allocated => {
                    tree.allocated_sizes[ia].cmp(&tree.allocated_sizes[ib])
                }
                LargestSortColumn::Percent => tree
                    .size_for_metric(a, metric)
                    .cmp(&tree.size_for_metric(b, metric)),
                LargestSortColumn::Name => tree
                    .name(a)
                    .to_ascii_lowercase()
                    .cmp(&tree.name(b).to_ascii_lowercase()),
                LargestSortColumn::Category => FileCategory::from_extension(tree.ext(a))
                    .short_label()
                    .cmp(FileCategory::from_extension(tree.ext(b)).short_label())
                    .then_with(|| tree.ext(a).cmp(tree.ext(b))),
                LargestSortColumn::Attrs => tree.win_attrs[ia]
                    .bits()
                    .cmp(&tree.win_attrs[ib].bits()),
                LargestSortColumn::Owner => tree
                    .owner(a)
                    .to_ascii_lowercase()
                    .cmp(&tree.owner(b).to_ascii_lowercase()),
                LargestSortColumn::Modified => tree.mtimes[ia].cmp(&tree.mtimes[ib]),
                LargestSortColumn::FullPath => tree.resolve_path(a).cmp(&tree.resolve_path(b)),
            };
            if desc {
                ord.reverse()
            } else {
                ord
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        tree: &ScanTree,
        tree_gen: u64,
        selection: &mut SelectionState,
        settings: &AppSettings,
        read_only_locked: bool,
    ) -> Option<NodeAction> {
        let metric = settings.size_metric;
        let show_seconds = settings.show_timestamp_seconds;
        let mut action: Option<NodeAction> = None;
        let mut pending_click: Option<(NodeId, egui::Modifiers)> = None;
        let mut pending_check: Option<(NodeId, bool)> = None;
        let mut pending_rclick: Option<NodeId> = None;
        let mut master_toggle: Option<bool> = None;

        ui.horizontal_wrapped(|ui| {
            ui.strong(t("🏆 Top Archivos Más Grandes", "🏆 Top Largest Files"));
            ui.separator();
            ui.label(t("Mostrar Top:", "Show Top:"));
            for n in [100usize, 500, 1_000, 5_000, 10_000] {
                if ui.selectable_label(self.limit == n, format_count(n as u64)).clicked() {
                    self.limit = n;
                    self.dirty = true;
                }
            }
            ui.separator();
            ui.label(t("Filtrar ext:", "Filter ext:"));
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.ext_filter)
                        .hint_text(t("ej. vhdx, mp4, bak", "e.g. vhdx, mp4, bak"))
                        .desired_width(130.0),
                )
                .changed()
            {
                self.dirty = true;
            }
        });

        if self.dirty || self.last_tree_gen != tree_gen || self.last_metric != metric {
            let mut list = tree.largest_files(self.limit * 2, metric);
            let ext_q = self.ext_filter.trim().trim_start_matches('.').to_ascii_lowercase();
            if !ext_q.is_empty() {
                list.retain(|&id| tree.ext(id).contains(&ext_q));
            }
            list.truncate(self.limit);
            self.cached_nodes = list;
            self.resort_cached(tree, metric);
            self.dirty = false;
            self.last_tree_gen = tree_gen;
            self.last_metric = metric;
        }

        let sum_bytes: u64 = self
            .cached_nodes
            .iter()
            .map(|&id| tree.size_for_metric(id, metric))
            .sum();
        let total_root = tree.size_for_metric(NodeId::ROOT, metric).max(1);
        ui.label(
            RichText::new(format!(
                "Top {} {} {} ({:.1}% {})",
                format_count(self.cached_nodes.len() as u64),
                t("ficheros suman", "files total"),
                format_bytes(sum_bytes),
                (sum_bytes as f64 / total_root as f64) * 100.0,
                t("del escaneo total", "of full scan")
            ))
            .small()
            .color(Color32::from_rgb(160, 200, 235)),
        );
        ui.separator();

        let count = self.cached_nodes.len();
        let all_selected =
            !self.cached_nodes.is_empty() && self.cached_nodes.iter().all(|&n| selection.contains(n));
        let mut sort_clicked: Option<LargestSortColumn> = None;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(24.0)) // Select checkbox
            .column(Column::initial(46.0).at_least(36.0)) // #
            .column(Column::initial(85.0).at_least(65.0)) // Logical
            .column(Column::initial(85.0).at_least(65.0)) // Allocated
            .column(Column::initial(60.0).at_least(48.0)) // % Total
            .column(Column::initial(175.0).at_least(100.0).clip(true)) // Name
            .column(Column::initial(85.0).at_least(65.0)) // Status
            .column(Column::initial(60.0).at_least(46.0)) // Attrs
            .column(Column::initial(100.0).at_least(65.0).clip(true)) // Owner
            .column(Column::initial(105.0).at_least(85.0)) // Modified
            .column(Column::remainder().at_least(180.0).clip(true)) // Full path
            .header(22.0, |mut h| {
                h.col(|ui| {
                    let mut all_chk = all_selected;
                    if ui
                        .checkbox(&mut all_chk, "")
                        .on_hover_text(t(
                            "Seleccionar / deseleccionar todo el Top (Ctrl+A)",
                            "Select / deselect all Top files (Ctrl+A)",
                        ))
                        .changed()
                    {
                        master_toggle = Some(all_chk);
                    }
                });
                let mut hdr =
                    |row: &mut egui_extras::TableRow<'_, '_>, label: &str, col: LargestSortColumn| {
                        row.col(|ui| {
                            if sort_header_button(ui, label, self.sort_col == col, self.sort_desc) {
                                sort_clicked = Some(col);
                            }
                        });
                    };
                hdr(&mut h, "#", LargestSortColumn::Rank);
                hdr(&mut h, t("Tamaño", "Size"), LargestSortColumn::Size);
                hdr(&mut h, t("En Disco", "On Disk"), LargestSortColumn::Allocated);
                hdr(&mut h, "% Tot", LargestSortColumn::Percent);
                hdr(&mut h, t("Nombre", "Name"), LargestSortColumn::Name);
                hdr(&mut h, t("Tipo", "Type"), LargestSortColumn::Category);
                hdr(&mut h, t("Atrib.", "Attrs"), LargestSortColumn::Attrs);
                hdr(&mut h, t("Propietario", "Owner"), LargestSortColumn::Owner);
                hdr(&mut h, t("Modificado", "Modified"), LargestSortColumn::Modified);
                hdr(&mut h, t("Ruta completa", "Full path"), LargestSortColumn::FullPath);
            })
            .body(|body| {
                body.rows(22.0, count, |mut row| {
                    let rank = row.index() + 1;
                    let nid = self.cached_nodes[row.index()];
                    let idx = nid.index();
                    let is_sel = selection.contains(nid);
                    row.set_selected(is_sel);

                    let path = tree.resolve_path(nid);
                    let pct = (tree.size_for_metric(nid, metric) as f64 / total_root as f64) * 100.0;

                    row.col(|ui| {
                        let mut chk = is_sel;
                        if ui.checkbox(&mut chk, "").changed() {
                            pending_check = Some((nid, chk));
                        }
                    });
                    row.col(|ui| { ui.label(RichText::new(format!("{rank}")).weak()); });
                    row.col(|ui| { ui.strong(format_bytes(tree.logical_sizes[idx])); });
                    row.col(|ui| { ui.label(format_bytes(tree.allocated_sizes[idx])); });
                    row.col(|ui| { ui.label(format!("{pct:.2}%")); });
                    row.col(|ui| {
                        let r = ui.selectable_label(is_sel, tree.name(nid));
                        if r.clicked() {
                            pending_click = Some((nid, ui.input(|i| i.modifiers)));
                        }
                        if r.secondary_clicked() {
                            pending_rclick = Some(nid);
                        }
                        if r.double_clicked() {
                            reveal_in_explorer(&path);
                        }
                        r.context_menu(|ui| {
                            let targets = selection.context_targets_for(nid);
                            if let Some(act) = show_node_actions_menu(
                                ui,
                                tree,
                                &targets,
                                Some(MenuScope {
                                    visible_nodes: &self.cached_nodes,
                                    allow_tree_ops: false,
                                }),
                                settings,
                                read_only_locked,
                            ) {
                                action = Some(act);
                            }
                        });
                    });
                    row.col(|ui| {
                        let cat = FileCategory::from_extension(tree.ext(nid));
                        let [r, g, b] = cat.accent_rgb();
                        ui.label(
                            RichText::new(format!("{} {}", cat.icon(), cat.short_label()))
                                .small()
                                .color(Color32::from_rgb(r, g, b)),
                        );
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(tree.win_attrs[idx].to_compact_string()).small().monospace());
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(tree.owner(nid)).small());
                    });
                    row.col(|ui| {
                        ui.label(format_timestamp_opts(tree.mtimes[idx], show_seconds));
                    });
                    row.col(|ui| {
                        let pr = ui.selectable_label(
                            is_sel,
                            RichText::new(path.to_string_lossy().as_ref()).small().monospace(),
                        );
                        if pr.clicked() {
                            pending_click = Some((nid, ui.input(|i| i.modifiers)));
                        }
                        if pr.secondary_clicked() {
                            pending_rclick = Some(nid);
                        }
                        pr.context_menu(|ui| {
                            let targets = selection.context_targets_for(nid);
                            if let Some(act) = show_node_actions_menu(
                                ui,
                                tree,
                                &targets,
                                Some(MenuScope {
                                    visible_nodes: &self.cached_nodes,
                                    allow_tree_ops: false,
                                }),
                                settings,
                                read_only_locked,
                            ) {
                                action = Some(act);
                            }
                        });
                    });
                });
            });

        if let Some(chk_all) = master_toggle {
            if chk_all {
                selection.select_all(&self.cached_nodes);
            } else {
                selection.clear();
            }
        }
        if let Some((nid, chk)) = pending_check {
            selection.set_checked(nid, chk);
        }
        if let Some((nid, mods)) = pending_click {
            selection.handle_click(nid, mods, &self.cached_nodes);
        }
        if let Some(nid) = pending_rclick {
            selection.ensure_for_context_menu(nid);
        }
        if let Some(col) = sort_clicked {
            if self.sort_col == col {
                self.sort_desc = !self.sort_desc;
            } else {
                self.sort_col = col;
                self.sort_desc = !matches!(
                    col,
                    LargestSortColumn::Rank
                        | LargestSortColumn::Name
                        | LargestSortColumn::Category
                        | LargestSortColumn::Attrs
                        | LargestSortColumn::Owner
                        | LargestSortColumn::FullPath
                );
            }
            self.resort_cached(tree, metric);
        }

        action
    }
}
