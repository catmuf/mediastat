//! SysAdmin Audit Tab: Access Denied folders, Long Paths (>260 chars), Cloud Offline files, and NTFS Compression stats.

use ahash::AHashSet;
use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mediastat_core::{
    format_bytes, format_count, t, IssueKind, NodeFlags, ScanTree, WinAttrs,
};
use mediastat_scanner::is_process_elevated;

use crate::shell_actions::{
    relaunch_as_administrator, reveal_in_explorer, reveal_multiple_in_explorer,
};
use crate::views::sort_header_button;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditFilter {
    All,
    AccessDenied,
    LongPaths,
    CloudOffline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditSortColumn {
    Kind,
    Detail,
    Path,
}

pub struct SysAdminAuditState {
    pub filter: AuditFilter,
    pub selected_issues: AHashSet<usize>,
    anchor_issue: Option<usize>,
    pub sort_col: AuditSortColumn,
    pub sort_desc: bool,
}

impl Default for SysAdminAuditState {
    fn default() -> Self {
        Self {
            filter: AuditFilter::All,
            selected_issues: AHashSet::new(),
            anchor_issue: None,
            sort_col: AuditSortColumn::Kind,
            sort_desc: false,
        }
    }
}

impl SysAdminAuditState {
    pub fn show(&mut self, ui: &mut Ui, tree: &ScanTree) {
        let sum = tree.summary();

        // Compute NTFS-compressed savings summary across the tree
        let mut ntfs_comp_files = 0u64;
        let mut ntfs_comp_logical = 0u64;
        let mut ntfs_comp_alloc = 0u64;
        for i in 1..tree.len() {
            if !tree.flags[i].contains(NodeFlags::IS_DIR)
                && !tree.flags[i].contains(NodeFlags::IS_DELETED)
                && tree.win_attrs[i].contains(WinAttrs::COMPRESSED)
            {
                ntfs_comp_files += 1;
                ntfs_comp_logical = ntfs_comp_logical.saturating_add(tree.logical_sizes[i]);
                ntfs_comp_alloc = ntfs_comp_alloc.saturating_add(tree.allocated_sizes[i]);
            }
        }
        let ntfs_saved = ntfs_comp_logical.saturating_sub(ntfs_comp_alloc);

        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.filter,
                AuditFilter::All,
                format!(
                    "{} ({})",
                    t("Todas", "All"),
                    format_count(tree.issues.len() as u64)
                ),
            );
            ui.selectable_value(
                &mut self.filter,
                AuditFilter::AccessDenied,
                format!(
                    "{} ({})",
                    t("⛔ Acceso Denegado", "⛔ Access Denied"),
                    format_count(sum.access_denied_count)
                ),
            );
            ui.selectable_value(
                &mut self.filter,
                AuditFilter::LongPaths,
                format!(
                    "{} ({})",
                    t("📏 Rutas > 260 chars", "📏 Paths > 260 chars"),
                    format_count(sum.long_path_count)
                ),
            );
            ui.selectable_value(
                &mut self.filter,
                AuditFilter::CloudOffline,
                format!("☁ Cloud/OneDrive Offline ({})", format_count(sum.cloud_offline_count)),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !is_process_elevated()
                    && sum.access_denied_count > 0
                    && ui
                        .button(
                            RichText::new(t(
                                "🛡 Reiniciar como Administrador",
                                "🛡 Restart as Administrator",
                            ))
                            .strong()
                            .color(Color32::from_rgb(245, 195, 75)),
                        )
                        .clicked()
                {
                    relaunch_as_administrator();
                }
            });
        });

        ui.label(
            RichText::new(format!(
                "{}: {} {} ({} {} → {} {}, {} {})",
                t(
                    "Compresión NTFS activa detectada",
                    "Active NTFS compression detected"
                ),
                format_count(ntfs_comp_files),
                t("archivos", "files"),
                format_bytes(ntfs_comp_logical),
                t("lógico", "logical"),
                format_bytes(ntfs_comp_alloc),
                t("en disco", "on disk"),
                t("ahorrando", "saving"),
                format_bytes(ntfs_saved),
            ))
            .small()
            .color(Color32::from_rgb(150, 200, 235)),
        );
        ui.separator();

        let mut filtered: Vec<(usize, _)> = tree
            .issues
            .iter()
            .enumerate()
            .filter(|(_, iss)| match self.filter {
                AuditFilter::All => true,
                AuditFilter::AccessDenied => iss.kind == IssueKind::AccessDenied,
                AuditFilter::LongPaths => iss.kind == IssueKind::LongPath,
                AuditFilter::CloudOffline => iss.kind == IssueKind::CloudSkipped,
            })
            .collect();

        let a_col = self.sort_col;
        let a_desc = self.sort_desc;
        filtered.sort_unstable_by(|(_, a), (_, b)| {
            let ord = match a_col {
                AuditSortColumn::Kind => a.kind.label().cmp(b.kind.label()),
                AuditSortColumn::Detail => a
                    .detail
                    .to_ascii_lowercase()
                    .cmp(&b.detail.to_ascii_lowercase()),
                AuditSortColumn::Path => a.path.cmp(&b.path),
            };
            if a_desc {
                ord.reverse()
            } else {
                ord
            }
        });

        self.selected_issues.retain(|&idx| idx < tree.issues.len());

        if filtered.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    RichText::new(t(
                        "✅ Sin incidencias registradas en esta categoría.",
                        "✅ No issues recorded in this category.",
                    ))
                    .color(Color32::from_rgb(76, 210, 128)),
                );
            });
            return;
        }

        if !self.selected_issues.is_empty() {
            let sel_paths: Vec<_> = filtered
                .iter()
                .filter(|(idx, _)| self.selected_issues.contains(idx))
                .map(|(_, iss)| iss.path.clone())
                .collect();
            if !sel_paths.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "✓ {} {}",
                            format_count(sel_paths.len() as u64),
                            t("incidencias seleccionadas", "issues selected")
                        ))
                        .small()
                        .strong()
                        .color(Color32::from_rgb(125, 200, 250)),
                    );
                    if ui
                        .small_button(t("📋 Copiar rutas", "📋 Copy paths"))
                        .clicked()
                    {
                        let joined = sel_paths
                            .iter()
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join("\r\n");
                        ui.ctx().copy_text(joined);
                    }
                    if ui
                        .small_button(t("📂 Explorador", "📂 Explorer"))
                        .clicked()
                    {
                        reveal_multiple_in_explorer(&sel_paths, 5);
                    }
                    if ui.small_button(t("✕ Limpiar", "✕ Clear")).clicked() {
                        self.selected_issues.clear();
                    }
                });
                ui.separator();
            }
        }

        let all_filtered_sel = filtered.iter().all(|(idx, _)| self.selected_issues.contains(idx));
        let mut master_toggle: Option<bool> = None;
        let mut pending_check: Option<(usize, bool)> = None;
        let mut pending_click: Option<(usize, egui::Modifiers)> = None;
        let mut pending_rclick: Option<usize> = None;
        let mut sort_clicked: Option<AuditSortColumn> = None;

        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(24.0))
            .column(Column::initial(160.0).at_least(125.0))
            .column(Column::initial(260.0).at_least(160.0).clip(true))
            .column(Column::remainder().at_least(220.0).clip(true))
            .header(22.0, |mut h| {
                h.col(|ui| {
                    let mut all_chk = all_filtered_sel;
                    if ui.checkbox(&mut all_chk, "").changed() {
                        master_toggle = Some(all_chk);
                    }
                });
                h.col(|ui| {
                    if sort_header_button(ui, t("Tipo de Incidencia", "Issue Type"), self.sort_col == AuditSortColumn::Kind, self.sort_desc) {
                        sort_clicked = Some(AuditSortColumn::Kind);
                    }
                });
                h.col(|ui| {
                    if sort_header_button(ui, t("Detalle", "Detail"), self.sort_col == AuditSortColumn::Detail, self.sort_desc) {
                        sort_clicked = Some(AuditSortColumn::Detail);
                    }
                });
                h.col(|ui| {
                    if sort_header_button(ui, t("Ruta Afectada", "Affected Path"), self.sort_col == AuditSortColumn::Path, self.sort_desc) {
                        sort_clicked = Some(AuditSortColumn::Path);
                    }
                });
            })
            .body(|body| {
                body.rows(22.0, filtered.len(), |mut row| {
                    let (orig_idx, iss) = filtered[row.index()];
                    let is_sel = self.selected_issues.contains(&orig_idx);
                    row.set_selected(is_sel);
                    let col = match iss.kind {
                        IssueKind::AccessDenied => Color32::from_rgb(235, 75, 75),
                        IssueKind::LongPath => Color32::from_rgb(245, 165, 65),
                        IssueKind::CloudSkipped => Color32::from_rgb(95, 185, 245),
                    };
                    row.col(|ui| {
                        let mut chk = is_sel;
                        if ui.checkbox(&mut chk, "").changed() {
                            pending_check = Some((orig_idx, chk));
                        }
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(iss.kind.label()).strong().color(col));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&iss.detail).small());
                    });
                    row.col(|ui| {
                        let resp = ui.selectable_label(
                            is_sel,
                            RichText::new(iss.path.to_string_lossy().as_ref()).small().monospace(),
                        );
                        if resp.clicked() {
                            pending_click = Some((orig_idx, ui.input(|i| i.modifiers)));
                        }
                        if resp.secondary_clicked() {
                            pending_rclick = Some(orig_idx);
                        }
                        if resp.double_clicked() {
                            reveal_in_explorer(&iss.path);
                        }
                        resp.context_menu(|ui| {
                            let target_paths: Vec<_> = if self.selected_issues.contains(&orig_idx) {
                                filtered
                                    .iter()
                                    .filter(|(i, _)| self.selected_issues.contains(i))
                                    .map(|(_, it)| it.path.clone())
                                    .collect()
                            } else {
                                vec![iss.path.clone()]
                            };
                            let n = target_paths.len();
                            if ui
                                .button(if n > 1 {
                                    format!("📋 {} ({n})", t("Copiar rutas", "Copy paths"))
                                } else {
                                    t("📋 Copiar ruta", "📋 Copy path").to_string()
                                })
                                .clicked()
                            {
                                let txt = target_paths
                                    .iter()
                                    .map(|p| p.to_string_lossy().into_owned())
                                    .collect::<Vec<_>>()
                                    .join("\r\n");
                                ui.ctx().copy_text(txt);
                                ui.close_menu();
                            }
                            if ui
                                .button(if n > 1 {
                                    format!(
                                        "📂 {} ({n})",
                                        t("Abrir ubicaciones en Explorador", "Reveal locations in Explorer")
                                    )
                                } else {
                                    t(
                                        "📂 Abrir ubicación en Explorador",
                                        "📂 Reveal location in Explorer",
                                    )
                                    .to_string()
                                })
                                .clicked()
                            {
                                reveal_multiple_in_explorer(&target_paths, 5);
                                ui.close_menu();
                            }
                        });
                    });
                });
            });

        if let Some(chk_all) = master_toggle {
            self.selected_issues.clear();
            if chk_all {
                for (orig_idx, _) in &filtered {
                    self.selected_issues.insert(*orig_idx);
                }
            }
        }
        if let Some((idx, chk)) = pending_check {
            if chk {
                self.selected_issues.insert(idx);
                self.anchor_issue = Some(idx);
            } else {
                self.selected_issues.remove(&idx);
            }
        }
        if let Some((idx, mods)) = pending_click {
            if mods.shift {
                let target_pos = filtered.iter().position(|(i, _)| *i == idx);
                let anchor_pos = self
                    .anchor_issue
                    .and_then(|a| filtered.iter().position(|(i, _)| *i == a))
                    .or(target_pos);
                if let (Some(a), Some(b)) = (anchor_pos, target_pos) {
                    if !mods.ctrl && !mods.command {
                        self.selected_issues.clear();
                    }
                    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                    for (oi, _) in &filtered[lo..=hi] {
                        self.selected_issues.insert(*oi);
                    }
                }
            } else if mods.ctrl || mods.command {
                if !self.selected_issues.remove(&idx) {
                    self.selected_issues.insert(idx);
                    self.anchor_issue = Some(idx);
                }
            } else {
                self.selected_issues.clear();
                self.selected_issues.insert(idx);
                self.anchor_issue = Some(idx);
            }
        }
        if let Some(idx) = pending_rclick {
            if !self.selected_issues.contains(&idx) {
                self.selected_issues.clear();
                self.selected_issues.insert(idx);
                self.anchor_issue = Some(idx);
            }
        }
        if let Some(col) = sort_clicked {
            if self.sort_col == col {
                self.sort_desc = !self.sort_desc;
            } else {
                self.sort_col = col;
                self.sort_desc = false;
            }
        }
    }
}
