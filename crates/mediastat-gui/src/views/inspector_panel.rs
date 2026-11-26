use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mediastat_core::{
    format_bytes, format_count, format_timestamp_opts, is_previewable_image_ext, t, AppSettings,
    FileCategory, NodeFlags, NodeId, ScanTree, SizeMetric, TreemapOptions,
};
use mediastat_scanner::{resolve_owner_single, resolve_to_unc_path};

use crate::selection::{show_node_actions_menu, NodeAction, SelectionState};
use crate::shell_actions::{
    open_terminal_at, open_with_default_app, reveal_in_explorer, show_windows_properties,
    AsyncHashState, ThumbnailLoader,
};
use crate::treemap::{draw_mini_cushion_rect, extension_display_color};
use crate::views::sort_header_button;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightTab {
    Extensions,
    Inspector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtSortColumn {
    Ext,
    Size,
    Percent,
    Files,
}

pub enum RightPanelAction {
    SearchExtension(String),
    Node(NodeAction),
}

pub struct RightPanelState {
    pub active_tab: RightTab,
    pub highlighted_ext_id: Option<u16>,
    pub ext_sort_col: ExtSortColumn,
    pub ext_sort_desc: bool,
    pub hash_state: AsyncHashState,
    cached_owner_node: Option<NodeId>,
    cached_owner_str: Option<String>,
}

impl Default for RightPanelState {
    fn default() -> Self {
        Self {
            active_tab: RightTab::Extensions,
            highlighted_ext_id: None,
            ext_sort_col: ExtSortColumn::Size,
            ext_sort_desc: true,
            hash_state: AsyncHashState::default(),
            cached_owner_node: None,
            cached_owner_str: None,
        }
    }
}

impl RightPanelState {
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        tree: &ScanTree,
        selection: &mut SelectionState,
        thumb_loader: &mut ThumbnailLoader,
        settings: &AppSettings,
        read_only_locked: bool,
        float_ext: &mut bool,
    ) -> Option<RightPanelAction> {
        self.hash_state.poll();

        let inspector_lbl = if selection.len() > 1 {
            format!("🔬 Inspector ({})", selection.len())
        } else {
            "🔬 Inspector".to_string()
        };

        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.active_tab,
                RightTab::Extensions,
                t("🎨 Extensiones", "🎨 Extensions"),
            );
            ui.selectable_value(&mut self.active_tab, RightTab::Inspector, inspector_lbl);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (icon, tip) = if *float_ext {
                    (
                        "📌",
                        t(
                            "Acoplar panel de Extensiones/Inspector al layout principal",
                            "Dock Extensions/Inspector pane back into main layout",
                        ),
                    )
                } else {
                    (
                        "🗗",
                        t(
                            "Desacoplar como ventana libre independiente del SO",
                            "Detach as a free OS window",
                        ),
                    )
                };
                if ui.small_button(icon).on_hover_text(tip).clicked() {
                    *float_ext = !*float_ext;
                }
            });
        });
        ui.separator();

        match self.active_tab {
            RightTab::Extensions => self.show_extensions(
                ui,
                tree,
                settings.size_metric,
                &settings.treemap,
                settings.group_minor_extensions,
            ),
            RightTab::Inspector => self
                .show_inspector(
                    ui,
                    ctx,
                    tree,
                    selection,
                    thumb_loader,
                    settings,
                    read_only_locked,
                )
                .map(RightPanelAction::Node),
        }
    }

    fn show_extensions(
        &mut self,
        ui: &mut Ui,
        tree: &ScanTree,
        metric: SizeMetric,
        opts: &TreemapOptions,
        group_minor: bool,
    ) -> Option<RightPanelAction> {
        let mut action = None;
        let total = tree.size_for_metric(NodeId::ROOT, metric).max(1);
        let mut exts: Vec<_> = tree
            .extensions
            .entries()
            .iter()
            .filter(|e| e.file_count > 0)
            .collect();

        sort_extensions(&mut exts, metric, self.ext_sort_col, self.ext_sort_desc);

        let mut minor_count = 0u64;
        let mut minor_bytes = 0u64;
        if group_minor {
            let threshold = total / 1000; // <0.1%
            exts.retain(|e| {
                let b = ext_bytes_for_metric(e, metric);
                if b < threshold {
                    minor_count += e.file_count;
                    minor_bytes = minor_bytes.saturating_add(b);
                    false
                } else {
                    true
                }
            });
        }

        if self.highlighted_ext_id.is_some() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(t(
                        "Resaltando extensión en Mapa",
                        "Highlighting extension on Map",
                    ))
                    .small()
                    .color(Color32::from_rgb(255, 220, 90)),
                );
                if ui.small_button(t("Quitar", "Clear")).clicked() {
                    self.highlighted_ext_id = None;
                }
            });
        }

        let extra_minor_row = usize::from(minor_count > 0);
        let total_rows = exts.len() + extra_minor_row;
        let mut sort_clicked: Option<ExtSortColumn> = None;

        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(104.0).at_least(72.0))
            .column(Column::initial(84.0).at_least(66.0))
            .column(Column::initial(56.0).at_least(44.0))
            .column(Column::remainder().at_least(68.0))
            .header(20.0, |mut h| {
                h.col(|ui| {
                    if sort_header_button(ui, "Ext", self.ext_sort_col == ExtSortColumn::Ext, self.ext_sort_desc) {
                        sort_clicked = Some(ExtSortColumn::Ext);
                    }
                });
                h.col(|ui| {
                    if sort_header_button(ui, t("Tamaño", "Size"), self.ext_sort_col == ExtSortColumn::Size, self.ext_sort_desc) {
                        sort_clicked = Some(ExtSortColumn::Size);
                    }
                });
                h.col(|ui| {
                    if sort_header_button(ui, "%", self.ext_sort_col == ExtSortColumn::Percent, self.ext_sort_desc) {
                        sort_clicked = Some(ExtSortColumn::Percent);
                    }
                });
                h.col(|ui| {
                    if sort_header_button(ui, t("Ficheros", "Files"), self.ext_sort_col == ExtSortColumn::Files, self.ext_sort_desc) {
                        sort_clicked = Some(ExtSortColumn::Files);
                    }
                });
            })
            .body(|body| {
                body.rows(20.0, total_rows, |mut row| {
                    let ri = row.index();
                    if ri >= exts.len() {
                        row.col(|ui| {
                            ui.label(RichText::new(t("<Otros>", "<Others>")).italics().weak());
                        });
                        row.col(|ui| {
                            ui.label(format_bytes(minor_bytes));
                        });
                        row.col(|ui| {
                            ui.label(format!("{:.1}%", (minor_bytes as f64 / total as f64) * 100.0));
                        });
                        row.col(|ui| {
                            ui.label(format_count(minor_count));
                        });
                        return;
                    }
                    let info = exts[ri];
                    let bytes = match metric {
                        SizeMetric::Logical => info.total_logical_bytes,
                        SizeMetric::AllocatedOnDisk => info.total_allocated_bytes,
                    };
                    let is_hl = self.highlighted_ext_id == Some(info.id);
                    row.set_selected(is_hl);

                    row.col(|ui| {
                        let rgb = extension_display_color(tree, info.id, opts);
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(14.0, 12.0), egui::Sense::hover());
                        draw_mini_cushion_rect(ui.painter(), rect, rgb, opts);

                        let cat = FileCategory::from_extension(&info.ext);
                        let name = if info.ext.is_empty() {
                            t("(sin ext)", "(no ext)").to_string()
                        } else {
                            format!(".{}", info.ext)
                        };
                        let resp = ui
                            .selectable_label(is_hl, &name)
                            .on_hover_text(format!(
                                "{} {} — {}",
                                cat.icon(),
                                cat.label(),
                                t(
                                    "Clic para resaltar · Clic derecho para buscar",
                                    "Click to highlight · Right-click to search"
                                )
                            ));
                        if resp.clicked() {
                            self.highlighted_ext_id = if is_hl { None } else { Some(info.id) };
                        }
                        resp.context_menu(|ui| {
                            if ui
                                .button(format!(
                                    "🔎 {} {name}",
                                    t("Buscar todos los", "Search all")
                                ))
                                .clicked()
                            {
                                action = Some(RightPanelAction::SearchExtension(info.ext.clone()));
                                ui.close_menu();
                            }
                            if ui
                                .button(t("🎨 Resaltar en el Mapa", "🎨 Highlight on Map"))
                                .clicked()
                            {
                                self.highlighted_ext_id = Some(info.id);
                                ui.close_menu();
                            }
                        });
                    });
                    row.col(|ui| {
                        ui.label(format_bytes(bytes));
                    });
                    row.col(|ui| {
                        let pct = (bytes as f64 / total as f64) * 100.0;
                        ui.label(format!("{pct:.1}%"));
                    });
                    row.col(|ui| {
                        ui.label(format_count(info.file_count));
                    });
                });
            });

        if let Some(col) = sort_clicked {
            if self.ext_sort_col == col {
                self.ext_sort_desc = !self.ext_sort_desc;
            } else {
                self.ext_sort_col = col;
                self.ext_sort_desc = !matches!(col, ExtSortColumn::Ext);
            }
        }

        action
    }

    #[allow(clippy::too_many_arguments)]
    fn show_inspector(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        tree: &ScanTree,
        selection: &mut SelectionState,
        thumb_loader: &mut ThumbnailLoader,
        settings: &AppSettings,
        read_only_locked: bool,
    ) -> Option<NodeAction> {
        let show_seconds = settings.show_timestamp_seconds;
        let mut action: Option<NodeAction> = None;

        if selection.is_empty() {
            thumb_loader.clear();
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    RichText::new(t(
                        "Selecciona uno o varios ficheros/carpetas (Ctrl+Clic, Shift+Clic o casillas) para ver su ficha técnica, totales acumulados y acciones.",
                        "Select one or more files/folders (Ctrl+Click, Shift+Click, or checkboxes) to view technical details, combined totals, and batch actions.",
                    ))
                    .weak(),
                );
            });
            return None;
        }

        // Multi-Selection Summary Inspector
        if selection.len() > 1 {
            thumb_loader.clear();
            let stats = selection.compute_stats(tree);
            let targets = selection.items().to_vec();
            let total_root = tree.logical_sizes.first().copied().unwrap_or(1).max(1);
            let pct = (stats.logical_bytes as f64 / total_root as f64) * 100.0;
            let mut remove_node: Option<NodeId> = None;

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.strong(
                    RichText::new(format!(
                        "✓ {} {}",
                        format_count(stats.total_items as u64),
                        t("elementos seleccionados", "items selected")
                    ))
                    .size(15.0)
                    .color(Color32::from_rgb(120, 200, 250)),
                );
                ui.add_space(4.0);

                egui::Grid::new("multi_inspect_grid")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        ui.strong(t("Desglose:", "Breakdown:"));
                        ui.label(format!(
                            "{} {} · {} {}",
                            format_count(stats.file_count as u64),
                            t("archivos", "files"),
                            format_count(stats.dir_count as u64),
                            t("carpetas", "folders"),
                        ));
                        ui.end_row();

                        ui.strong(t("Tamaño lógico:", "Logical size:"));
                        ui.label(format!("{} ({pct:.2}%)", format_bytes(stats.logical_bytes)));
                        ui.end_row();

                        ui.strong(t("En disco (físico):", "Size on disk:"));
                        ui.label(format_bytes(stats.allocated_bytes));
                        ui.end_row();
                    });

                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .small_button(t("📋 Copiar rutas", "📋 Copy paths"))
                        .clicked()
                    {
                        action = Some(NodeAction::CopyPaths(targets.clone()));
                    }
                    if ui.small_button(t("📄 Nombres", "📄 Names")).clicked() {
                        action = Some(NodeAction::CopyNames(targets.clone()));
                    }
                    if ui.small_button("📊 TSV").clicked() {
                        action = Some(NodeAction::CopyTsv(targets.clone()));
                    }
                    if ui.small_button(t("📤 Exportar", "📤 Export")).clicked() {
                        action = Some(NodeAction::ExportNodes(targets.clone()));
                    }
                    ui.menu_button(t("⚡ Acciones ▾", "⚡ Actions ▾"), |ui| {
                        if let Some(act) = show_node_actions_menu(
                            ui,
                            tree,
                            &targets,
                            None,
                            settings,
                            read_only_locked,
                        ) {
                            action = Some(act);
                        }
                    });
                    if ui.small_button(t("✕ Limpiar", "✕ Clear")).clicked() {
                        action = Some(NodeAction::ClearSelection);
                    }
                });

                ui.add_space(6.0);
                ui.separator();
                ui.label(
                    RichText::new(t("Elementos seleccionados:", "Selected items:"))
                        .small()
                        .strong(),
                );
                for &nid in targets.iter().take(250) {
                    if nid.index() >= tree.len() {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("✕")
                            .on_hover_text(t("Quitar de la selección", "Remove from selection"))
                            .clicked()
                        {
                            remove_node = Some(nid);
                        }
                        let icon = if tree.is_dir(nid) { "📁" } else { "📄" };
                        let lbl = format!(
                            "{icon} {} ({})",
                            tree.name(nid),
                            format_bytes(tree.logical_sizes[nid.index()])
                        );
                        ui.label(RichText::new(lbl).small())
                            .on_hover_text(tree.resolve_path(nid).to_string_lossy().as_ref());
                    });
                }
                if targets.len() > 250 {
                    ui.label(
                        RichText::new(format!(
                            "… {} {} {}",
                            t("y", "and"),
                            format_count((targets.len() - 250) as u64),
                            t("elementos más", "more items")
                        ))
                        .small()
                        .weak(),
                    );
                }
            });

            if let Some(rm) = remove_node {
                selection.set_checked(rm, false);
            }
            return action;
        }

        let nid = selection.primary()?;

        if nid.index() >= tree.len() {
            return None;
        }

        let path = tree.resolve_path(nid);
        let is_dir = tree.flags[nid.index()].contains(NodeFlags::IS_DIR);
        self.hash_state.clear_if_different(&path);

        let owner_display = if !tree.owner(nid).is_empty() {
            tree.owner(nid).to_string()
        } else {
            if self.cached_owner_node != Some(nid) {
                self.cached_owner_node = Some(nid);
                self.cached_owner_str = resolve_owner_single(&path);
            }
            self.cached_owner_str
                .clone()
                .unwrap_or_else(|| "—".into())
        };

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.strong(RichText::new(tree.name(nid)).size(15.0));
            ui.add_space(2.0);
            let path_str = path.to_string_lossy();
            ui.label(
                RichText::new(path_str.as_ref())
                    .small()
                    .monospace()
                    .color(Color32::from_rgb(150, 185, 225)),
            );
            let unc_str = resolve_to_unc_path(&path);
            let has_distinct_unc = !unc_str.eq_ignore_ascii_case(&path_str);
            if has_distinct_unc {
                ui.label(
                    RichText::new(format!("UNC: {unc_str}"))
                        .small()
                        .monospace()
                        .color(Color32::from_rgb(110, 210, 190)),
                );
            }

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui.small_button(t("📂 Explorador", "📂 Explorer")).clicked() {
                    reveal_in_explorer(&path);
                }
                if ui.small_button(t("📋 Copiar", "📋 Copy")).clicked() {
                    ui.ctx().copy_text(path_str.to_string());
                }
                if has_distinct_unc && ui.small_button(t("🌐 Copiar UNC", "🌐 Copy UNC")).clicked() {
                    ui.ctx().copy_text(unc_str);
                }
                if ui
                    .small_button(">_ PS")
                    .on_hover_text(t("Abrir PowerShell aquí", "Open PowerShell here"))
                    .clicked()
                {
                    open_terminal_at(&path, true, false);
                }
                if ui
                    .small_button(t("ℹ Prop.", "ℹ Props"))
                    .on_hover_text(t("Propiedades de Windows (Alt+Enter)", "Windows Properties (Alt+Enter)"))
                    .clicked()
                {
                    show_windows_properties(&path);
                }
                if !is_dir && ui.small_button(t("▶ Abrir", "▶ Open")).clicked() {
                    open_with_default_app(&path);
                }
                ui.menu_button(t("⚡ Acciones ▾", "⚡ Actions ▾"), |ui| {
                    if let Some(act) = show_node_actions_menu(
                        ui,
                        tree,
                        &[nid],
                        None,
                        settings,
                        read_only_locked,
                    ) {
                        action = Some(act);
                    }
                });
            });

            ui.add_space(6.0);
            ui.separator();

            let attrs_str = tree.win_attrs[nid.index()].to_compact_string();

            if is_dir {
                thumb_loader.clear();
                let ds = tree.dir_stats.get(&nid).copied().unwrap_or_default();
                egui::Grid::new("dir_inspect_grid")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        ui.strong(t("Tamaño lógico:", "Logical size:"));
                        ui.label(format_bytes(tree.logical_sizes[nid.index()]));
                        ui.end_row();
                        ui.strong(t("En disco (físico):", "Size on disk:"));
                        ui.label(format_bytes(tree.allocated_sizes[nid.index()]));
                        ui.end_row();
                        ui.strong(t("Propietario:", "Owner:"));
                        ui.label(&owner_display);
                        ui.end_row();
                        ui.strong(t("Atributos Win32:", "Win32 attributes:"));
                        ui.label(if attrs_str.is_empty() {
                            "—".into()
                        } else {
                            attrs_str
                        });
                        ui.end_row();
                        ui.strong(t("Archivos / Subcarpetas:", "Files / Subfolders:"));
                        ui.label(format!(
                            "{} / {}",
                            format_count(ds.total_files),
                            format_count(ds.total_dirs)
                        ));
                        ui.end_row();
                        ui.strong(t("Modificado:", "Modified:"));
                        ui.label(format_timestamp_opts(
                            ds.max_mtime.max(tree.mtimes[nid.index()]),
                            show_seconds,
                        ));
                        ui.end_row();
                    });
                return;
            }

            // On-demand image thumbnail preview (zero scan-time I/O; decodes only when user clicks an image)
            let ext = tree.ext(nid);
            if is_previewable_image_ext(ext)
                && !tree.win_attrs[nid.index()].contains(mediastat_core::WinAttrs::OFFLINE_CLOUD)
            {
                let (tex, loading) = thumb_loader.request_for(&path, ctx);
                if let Some(t_handle) = tex {
                    ui.add(
                        egui::Image::from_texture(t_handle)
                            .max_size(egui::vec2(ui.available_width().min(260.0), 170.0))
                            .corner_radius(4.0),
                    );
                    ui.add_space(6.0);
                } else if loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new(t("Cargando vista previa...", "Loading preview..."))
                                .small()
                                .weak(),
                        );
                    });
                }
            } else {
                thumb_loader.clear();
            }

            let cat = FileCategory::from_extension(ext);
            let fid = tree.file_ids.get(nid.index()).copied().unwrap_or(0);
            let hl_group = if fid != 0 {
                tree.hardlinks
                    .iter()
                    .find(|g| g.file_id == fid && g.nodes.contains(&nid))
            } else {
                None
            };

            egui::Grid::new("file_inspect_grid")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    ui.strong(t("Categoría:", "Category:"));
                    ui.label(format!("{} {}", cat.icon(), cat.label()));
                    ui.end_row();
                    ui.strong(t("Tamaño lógico / disco:", "Logical / on disk:"));
                    ui.label(format!(
                        "{} / {}",
                        format_bytes(tree.logical_sizes[nid.index()]),
                        format_bytes(tree.allocated_sizes[nid.index()])
                    ));
                    ui.end_row();
                    if let Some(hg) = hl_group {
                        ui.strong(t("Hardlink NTFS:", "NTFS Hardlink:"));
                        ui.label(
                            RichText::new(format!(
                                "🔗 {} {} (FileId 0x{:X})",
                                hg.nodes.len(),
                                t("enlaces", "links"),
                                hg.file_id
                            ))
                            .color(Color32::from_rgb(220, 175, 120)),
                        );
                        ui.end_row();
                    }
                    ui.strong(t("Propietario:", "Owner:"));
                    ui.label(&owner_display);
                    ui.end_row();
                    ui.strong(t("Atributos Win32:", "Win32 attributes:"));
                    ui.label(if attrs_str.is_empty() {
                        "—".into()
                    } else {
                        attrs_str
                    });
                    ui.end_row();
                    ui.strong(t("Modificado:", "Modified:"));
                    ui.label(format_timestamp_opts(
                        tree.mtimes[nid.index()],
                        show_seconds,
                    ));
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.separator();
            if self.hash_state.loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(t(
                        "Calculando MD5 / SHA-256 / BLAKE3...",
                        "Computing MD5 / SHA-256 / BLAKE3...",
                    ));
                });
            } else if let Some(res) = &self.hash_state.result {
                match res {
                    Ok(h) => {
                        ui.strong(t("Hashes Criptográficos:", "Cryptographic Hashes:"));
                        for (lbl, val) in
                            [("MD5", &h.md5), ("SHA-256", &h.sha256), ("BLAKE3", &h.blake3)]
                        {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(lbl).small().strong());
                                if ui.small_button("📋").clicked() {
                                    ui.ctx().copy_text(val.clone());
                                }
                                ui.label(RichText::new(val).small().monospace());
                            });
                        }
                    }
                    Err(e) => {
                        ui.label(
                            RichText::new(format!("{}: {e}", t("Error hash", "Hash error")))
                                .color(Color32::from_rgb(240, 80, 80)),
                        );
                    }
                }
            } else if ui
                .button(t(
                    "🔑 Calcular Hashes (MD5 / SHA-256 / BLAKE3)",
                    "🔑 Compute Hashes (MD5 / SHA-256 / BLAKE3)",
                ))
                .clicked()
            {
                self.hash_state.start(path, ctx.clone());
            }
        });

        action
    }
}

fn ext_bytes_for_metric(e: &mediastat_core::ExtensionInfo, metric: SizeMetric) -> u64 {
    match metric {
        SizeMetric::Logical => e.total_logical_bytes,
        SizeMetric::AllocatedOnDisk => e.total_allocated_bytes,
    }
}

pub(crate) fn sort_extensions(
    exts: &mut [&mediastat_core::ExtensionInfo],
    metric: SizeMetric,
    col: ExtSortColumn,
    desc: bool,
) {
    exts.sort_unstable_by(|a, b| {
        let ord = match col {
            ExtSortColumn::Ext => a.ext.cmp(&b.ext),
            ExtSortColumn::Size | ExtSortColumn::Percent => ext_bytes_for_metric(a, metric)
                .cmp(&ext_bytes_for_metric(b, metric))
                .then_with(|| a.ext.cmp(&b.ext)),
            ExtSortColumn::Files => a
                .file_count
                .cmp(&b.file_count)
                .then_with(|| ext_bytes_for_metric(a, metric).cmp(&ext_bytes_for_metric(b, metric))),
        };
        if desc {
            ord.reverse()
        } else {
            ord
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediastat_core::ExtensionInfo;

    fn make_ext(id: u16, ext: &str, bytes: u64, files: u64) -> ExtensionInfo {
        ExtensionInfo {
            id,
            ext: ext.to_string(),
            color: [0, 0, 0],
            file_count: files,
            total_logical_bytes: bytes,
            total_allocated_bytes: bytes / 2,
        }
    }

    #[test]
    fn test_extensions_column_sorting_asc_and_desc() {
        let data = [
            make_ext(0, "zip", 1_000, 8),
            make_ext(1, "bmp", 500, 2),
            make_ext(2, "iso", 9_000, 1),
            make_ext(3, "mp4", 600, 30),
        ];

        // Size descending / ascending
        let mut refs: Vec<&ExtensionInfo> = data.iter().collect();
        sort_extensions(&mut refs, SizeMetric::Logical, ExtSortColumn::Size, true);
        assert_eq!(refs.iter().map(|e| e.ext.as_str()).collect::<Vec<_>>(), ["iso", "zip", "mp4", "bmp"]);

        sort_extensions(&mut refs, SizeMetric::Logical, ExtSortColumn::Size, false);
        assert_eq!(refs.iter().map(|e| e.ext.as_str()).collect::<Vec<_>>(), ["bmp", "mp4", "zip", "iso"]);

        // Ext name ascending / descending
        sort_extensions(&mut refs, SizeMetric::Logical, ExtSortColumn::Ext, false);
        assert_eq!(refs.iter().map(|e| e.ext.as_str()).collect::<Vec<_>>(), ["bmp", "iso", "mp4", "zip"]);

        sort_extensions(&mut refs, SizeMetric::Logical, ExtSortColumn::Ext, true);
        assert_eq!(refs.iter().map(|e| e.ext.as_str()).collect::<Vec<_>>(), ["zip", "mp4", "iso", "bmp"]);

        // Files descending / ascending
        sort_extensions(&mut refs, SizeMetric::Logical, ExtSortColumn::Files, true);
        assert_eq!(refs.iter().map(|e| e.ext.as_str()).collect::<Vec<_>>(), ["mp4", "zip", "bmp", "iso"]);

        sort_extensions(&mut refs, SizeMetric::Logical, ExtSortColumn::Files, false);
        assert_eq!(refs.iter().map(|e| e.ext.as_str()).collect::<Vec<_>>(), ["iso", "bmp", "zip", "mp4"]);
    }
}
