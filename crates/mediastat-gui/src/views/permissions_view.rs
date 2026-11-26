//! Permissions / DACL Audit & Owner Quota Breakdown Tab (WinDirStat 2.10 `FilePermsControl` + SysAdmin).

use ahash::AHashMap;
use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mediastat_core::{format_bytes, format_count, t, AppSettings, NodeFlags, NodeId, ScanTree};

use crate::selection::{show_node_actions_menu, MenuScope, NodeAction, SelectionState};
use crate::shell_actions::reveal_in_explorer;
use crate::views::sort_header_button;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermsSubTab {
    OwnerQuotas,
    DaclEntries,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerSortColumn {
    Owner,
    Files,
    Logical,
    Allocated,
    Percent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaclSortColumn {
    AccessType,
    Identity,
    Rights,
    Inherited,
    Owner,
    Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerResolveScope {
    TopLargest1000,
    SelectedFolder,
    EntireTree,
}

pub enum PermsAction {
    ResolveOwners(OwnerResolveScope),
    ScanDacl { include_inherited: bool },
    ExportPermissionsCsv,
    Node(NodeAction),
}

pub struct PermissionsViewState {
    pub sub_tab: PermsSubTab,
    pub filter_query: String,
    pub only_deny: bool,
    pub only_explicit: bool,
    pub include_inherited_scan: bool,
    pub running_task: bool,
    pub owner_sort_col: OwnerSortColumn,
    pub owner_sort_desc: bool,
    pub dacl_sort_col: DaclSortColumn,
    pub dacl_sort_desc: bool,
}

impl Default for PermissionsViewState {
    fn default() -> Self {
        Self {
            sub_tab: PermsSubTab::OwnerQuotas,
            filter_query: String::new(),
            only_deny: false,
            only_explicit: false,
            include_inherited_scan: false,
            running_task: false,
            owner_sort_col: OwnerSortColumn::Logical,
            owner_sort_desc: true,
            dacl_sort_col: DaclSortColumn::Path,
            dacl_sort_desc: false,
        }
    }
}

impl PermissionsViewState {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        tree: &ScanTree,
        selection: &mut SelectionState,
        settings: &AppSettings,
        read_only_locked: bool,
    ) -> Option<PermsAction> {
        let mut action = None;

        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.sub_tab,
                PermsSubTab::OwnerQuotas,
                t(
                    "👤 Cuotas por Propietario (DOMINIO\\usuario)",
                    "👤 Owner Quotas (DOMAIN\\user)",
                ),
            );
            ui.selectable_value(
                &mut self.sub_tab,
                PermsSubTab::DaclEntries,
                format!(
                    "{} (DACL: {})",
                    t("🔐 Permisos NTFS/SMB", "🔐 NTFS/SMB Permissions"),
                    format_count(tree.permissions.len() as u64)
                ),
            );
            if self.running_task {
                ui.spinner();
                ui.label(
                    RichText::new(t(
                        "Consultando ACLs/SIDs en segundo plano...",
                        "Querying ACLs/SIDs in background...",
                    ))
                    .small(),
                );
            }
        });
        ui.separator();

        match self.sub_tab {
            PermsSubTab::OwnerQuotas => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(t(
                        "Calcular propietarios bajo demanda:",
                        "Resolve owners on demand:",
                    ));
                    if ui
                        .add_enabled(
                            !self.running_task,
                            egui::Button::new(t(
                                "🏆 En Top 1.000 grandes",
                                "🏆 Top 1,000 largest",
                            )),
                        )
                        .clicked()
                    {
                        action = Some(PermsAction::ResolveOwners(OwnerResolveScope::TopLargest1000));
                    }
                    if ui
                        .add_enabled(
                            !self.running_task,
                            egui::Button::new(t(
                                "📁 En carpeta seleccionada",
                                "📁 Selected folder",
                            )),
                        )
                        .clicked()
                    {
                        action = Some(PermsAction::ResolveOwners(OwnerResolveScope::SelectedFolder));
                    }
                    if ui
                        .add_enabled(
                            !self.running_task,
                            egui::Button::new(t("🌐 Todo el escaneo", "🌐 Entire scan")),
                        )
                        .clicked()
                    {
                        action = Some(PermsAction::ResolveOwners(OwnerResolveScope::EntireTree));
                    }
                });
                ui.add_space(4.0);

                // Aggregate resolved owners
                let total_scan_bytes = tree.logical_sizes.first().copied().unwrap_or(1).max(1);
                let mut map: AHashMap<&str, (u64, u64, u64)> = AHashMap::new();
                for i in 1..tree.len() {
                    let f = tree.flags[i];
                    if f.contains(NodeFlags::IS_DIR) || f.contains(NodeFlags::IS_DELETED) {
                        continue;
                    }
                    let own = tree.owner(NodeId(i as u32));
                    if own.is_empty() {
                        continue;
                    }
                    let e = map.entry(own).or_default();
                    e.0 += 1;
                    e.1 = e.1.saturating_add(tree.logical_sizes[i]);
                    e.2 = e.2.saturating_add(tree.allocated_sizes[i]);
                }

                if map.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(36.0);
                        ui.label(
                            RichText::new(t(
                                "Aún no se han resuelto propietarios en este escaneo.\nPulsa cualquiera de los botones superiores para identificar qué usuarios (DOMINIO\\usuario) ocupan más espacio.",
                                "No owners have been resolved in this scan yet.\nClick any button above to identify which users (DOMAIN\\user) consume the most disk space.",
                            ))
                            .weak(),
                        );
                    });
                    return action;
                }

                let mut rows: Vec<(&str, u64, u64, u64)> = map
                    .into_iter()
                    .map(|(k, (c, l, a))| (k, c, l, a))
                    .collect();
                let o_col = self.owner_sort_col;
                let o_desc = self.owner_sort_desc;
                rows.sort_unstable_by(|a, b| {
                    let ord = match o_col {
                        OwnerSortColumn::Owner => {
                            a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase())
                        }
                        OwnerSortColumn::Files => a.1.cmp(&b.1),
                        OwnerSortColumn::Logical | OwnerSortColumn::Percent => a.2.cmp(&b.2),
                        OwnerSortColumn::Allocated => a.3.cmp(&b.3),
                    };
                    if o_desc {
                        ord.reverse()
                    } else {
                        ord
                    }
                });

                let mut owner_sort_clicked: Option<OwnerSortColumn> = None;
                TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::initial(260.0).at_least(150.0).clip(true))
                    .column(Column::initial(95.0).at_least(65.0))
                    .column(Column::initial(115.0).at_least(80.0))
                    .column(Column::initial(115.0).at_least(80.0))
                    .column(Column::remainder().at_least(80.0))
                    .header(22.0, |mut h| {
                        let mut hdr = |row: &mut egui_extras::TableRow<'_, '_>,
                                       label: &str,
                                       col: OwnerSortColumn| {
                            row.col(|ui| {
                                if sort_header_button(ui, label, self.owner_sort_col == col, self.owner_sort_desc) {
                                    owner_sort_clicked = Some(col);
                                }
                            });
                        };
                        hdr(&mut h, t("Propietario (DOMINIO\\usuario)", "Owner (DOMAIN\\user)"), OwnerSortColumn::Owner);
                        hdr(&mut h, t("Archivos", "Files"), OwnerSortColumn::Files);
                        hdr(&mut h, t("Tamaño Lógico", "Logical Size"), OwnerSortColumn::Logical);
                        hdr(&mut h, t("En Disco", "On Disk"), OwnerSortColumn::Allocated);
                        hdr(&mut h, t("% Total", "% Total"), OwnerSortColumn::Percent);
                    })
                    .body(|body| {
                        body.rows(22.0, rows.len(), |mut row| {
                            let (own, cnt, log, alloc) = rows[row.index()];
                            let pct = (log as f64 / total_scan_bytes as f64) * 100.0;
                            row.col(|ui| { ui.strong(own); });
                            row.col(|ui| { ui.label(format_count(cnt)); });
                            row.col(|ui| { ui.label(format_bytes(log)); });
                            row.col(|ui| { ui.label(format_bytes(alloc)); });
                            row.col(|ui| { ui.label(format!("{pct:.2}%")); });
                        });
                    });

                if let Some(col) = owner_sort_clicked {
                    if self.owner_sort_col == col {
                        self.owner_sort_desc = !self.owner_sort_desc;
                    } else {
                        self.owner_sort_col = col;
                        self.owner_sort_desc = !matches!(col, OwnerSortColumn::Owner);
                    }
                }
            }

            PermsSubTab::DaclEntries => {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !self.running_task,
                            egui::Button::new(t(
                                "🔐 Escanear Permisos DACL ahora",
                                "🔐 Scan DACL Permissions now",
                            )),
                        )
                        .clicked()
                    {
                        action = Some(PermsAction::ScanDacl {
                            include_inherited: self.include_inherited_scan,
                        });
                    }
                    ui.checkbox(
                        &mut self.include_inherited_scan,
                        t(
                            "Incluir heredados en subcarpetas",
                            "Include inherited in subfolders",
                        ),
                    );
                    ui.separator();
                    ui.label(t("Filtrar:", "Filter:"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter_query)
                            .hint_text(t(
                                "Usuario, grupo, ruta o permiso...",
                                "User, group, path, or right...",
                            ))
                            .desired_width(180.0),
                    );
                    ui.checkbox(&mut self.only_deny, t("Solo DENY", "DENY only"));
                    ui.checkbox(&mut self.only_explicit, t("Solo Explícitos", "Explicit only"));
                    if ui
                        .add_enabled(
                            !tree.permissions.is_empty(),
                            egui::Button::new(t(
                                "📤 Exportar CSV Permisos",
                                "📤 Export Permissions CSV",
                            )),
                        )
                        .clicked()
                    {
                        action = Some(PermsAction::ExportPermissionsCsv);
                    }
                });
                ui.separator();

                if tree.permissions.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(36.0);
                        ui.label(
                            RichText::new(t(
                                "Pulsa 'Escanear Permisos DACL ahora' para auditar las reglas de seguridad (ACEs Allow/Deny, grupos AD y permisos explícitos) en tus carpetas.",
                                "Click 'Scan DACL Permissions now' to audit security rules (Allow/Deny ACEs, AD groups, and explicit permissions) across your folders.",
                            ))
                            .weak(),
                        );
                    });
                    return action;
                }

                let q = self.filter_query.trim().to_ascii_lowercase();
                let mut filtered: Vec<_> = tree
                    .permissions
                    .iter()
                    .filter(|p| {
                        if self.only_deny && p.access_type != "Deny" {
                            return false;
                        }
                        if self.only_explicit && p.inherited {
                            return false;
                        }
                        if !q.is_empty() {
                            let hit = p.identity.to_ascii_lowercase().contains(&q)
                                || p.owner.to_ascii_lowercase().contains(&q)
                                || p.rights.to_ascii_lowercase().contains(&q)
                                || p.path.to_string_lossy().to_ascii_lowercase().contains(&q);
                            if !hit {
                                return false;
                            }
                        }
                        true
                    })
                    .collect();

                let d_col = self.dacl_sort_col;
                let d_desc = self.dacl_sort_desc;
                filtered.sort_unstable_by(|a, b| {
                    let ord = match d_col {
                        DaclSortColumn::AccessType => a.access_type.cmp(&b.access_type),
                        DaclSortColumn::Identity => a
                            .identity
                            .to_ascii_lowercase()
                            .cmp(&b.identity.to_ascii_lowercase()),
                        DaclSortColumn::Rights => a
                            .rights
                            .to_ascii_lowercase()
                            .cmp(&b.rights.to_ascii_lowercase()),
                        DaclSortColumn::Inherited => a.inherited.cmp(&b.inherited),
                        DaclSortColumn::Owner => a
                            .owner
                            .to_ascii_lowercase()
                            .cmp(&b.owner.to_ascii_lowercase()),
                        DaclSortColumn::Path => a.path.cmp(&b.path),
                    };
                    if d_desc {
                        ord.reverse()
                    } else {
                        ord
                    }
                });

                let visible_nodes: Vec<NodeId> = filtered
                    .iter()
                    .map(|p| p.node_id)
                    .filter(|&n| n.is_some() && n != NodeId::ROOT && n.index() < tree.len())
                    .collect();
                let all_selected = !visible_nodes.is_empty()
                    && visible_nodes.iter().all(|&n| selection.contains(n));
                let mut pending_click: Option<(NodeId, egui::Modifiers)> = None;
                let mut pending_check: Option<(NodeId, bool)> = None;
                let mut pending_rclick: Option<NodeId> = None;
                let mut master_toggle: Option<bool> = None;
                let mut dacl_sort_clicked: Option<DaclSortColumn> = None;

                TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::exact(24.0)) // Checkbox
                    .column(Column::initial(68.0).at_least(54.0)) // Allow/Deny
                    .column(Column::initial(160.0).at_least(100.0).clip(true)) // Identity
                    .column(Column::initial(110.0).at_least(80.0)) // Rights
                    .column(Column::initial(78.0).at_least(60.0)) // Inherited
                    .column(Column::initial(130.0).at_least(80.0).clip(true)) // Owner
                    .column(Column::remainder().at_least(200.0).clip(true)) // Path
                    .header(22.0, |mut h| {
                        h.col(|ui| {
                            let mut all_chk = all_selected;
                            if ui.checkbox(&mut all_chk, "").changed() {
                                master_toggle = Some(all_chk);
                            }
                        });
                        let mut hdr = |row: &mut egui_extras::TableRow<'_, '_>,
                                       label: &str,
                                       col: DaclSortColumn| {
                            row.col(|ui| {
                                if sort_header_button(ui, label, self.dacl_sort_col == col, self.dacl_sort_desc) {
                                    dacl_sort_clicked = Some(col);
                                }
                            });
                        };
                        hdr(&mut h, t("Tipo", "Type"), DaclSortColumn::AccessType);
                        hdr(&mut h, t("Identidad / Grupo", "Identity / Group"), DaclSortColumn::Identity);
                        hdr(&mut h, t("Permisos", "Rights"), DaclSortColumn::Rights);
                        hdr(&mut h, t("Origen", "Source"), DaclSortColumn::Inherited);
                        hdr(&mut h, t("Propietario", "Owner"), DaclSortColumn::Owner);
                        hdr(&mut h, t("Ruta", "Path"), DaclSortColumn::Path);
                    })
                    .body(|body| {
                        body.rows(21.0, filtered.len(), |mut row| {
                            let p = filtered[row.index()];
                            let nid = p.node_id;
                            let valid_nid = nid.is_some() && nid != NodeId::ROOT && nid.index() < tree.len();
                            let is_sel = valid_nid && selection.contains(nid);
                            row.set_selected(is_sel);

                            row.col(|ui| {
                                if valid_nid {
                                    let mut chk = is_sel;
                                    if ui.checkbox(&mut chk, "").changed() {
                                        pending_check = Some((nid, chk));
                                    }
                                }
                            });
                            row.col(|ui| {
                                let col = if p.access_type == "Deny" {
                                    Color32::from_rgb(235, 70, 70)
                                } else {
                                    Color32::from_rgb(76, 200, 128)
                                };
                                ui.label(RichText::new(&p.access_type).strong().color(col));
                            });
                            row.col(|ui| { ui.label(&p.identity); });
                            row.col(|ui| { ui.label(&p.rights); });
                            row.col(|ui| {
                                ui.label(if p.inherited {
                                    RichText::new(t("Heredado", "Inherited")).weak()
                                } else {
                                    RichText::new(t("Explícito", "Explicit"))
                                        .strong()
                                        .color(Color32::from_rgb(245, 180, 70))
                                });
                            });
                            row.col(|ui| { ui.label(RichText::new(&p.owner).small()); });
                            row.col(|ui| {
                                let resp = ui.selectable_label(
                                    is_sel,
                                    RichText::new(p.path.to_string_lossy().as_ref()).small().monospace(),
                                );
                                if valid_nid {
                                    if resp.clicked() {
                                        pending_click = Some((nid, ui.input(|i| i.modifiers)));
                                    }
                                    if resp.secondary_clicked() {
                                        pending_rclick = Some(nid);
                                    }
                                    resp.context_menu(|ui| {
                                        let targets = selection.context_targets_for(nid);
                                        if let Some(act) = show_node_actions_menu(
                                            ui,
                                            tree,
                                            &targets,
                                            Some(MenuScope {
                                                visible_nodes: &visible_nodes,
                                                allow_tree_ops: false,
                                            }),
                                            settings,
                                            read_only_locked,
                                        ) {
                                            action = Some(PermsAction::Node(act));
                                        }
                                    });
                                }
                                if resp.double_clicked() {
                                    reveal_in_explorer(&p.path);
                                }
                            });
                        });
                    });

                if let Some(chk_all) = master_toggle {
                    if chk_all {
                        selection.select_all(&visible_nodes);
                    } else {
                        selection.clear();
                    }
                }
                if let Some((nid, chk)) = pending_check {
                    selection.set_checked(nid, chk);
                }
                if let Some((nid, mods)) = pending_click {
                    selection.handle_click(nid, mods, &visible_nodes);
                }
                if let Some(nid) = pending_rclick {
                    selection.ensure_for_context_menu(nid);
                }
                if let Some(col) = dacl_sort_clicked {
                    if self.dacl_sort_col == col {
                        self.dacl_sort_desc = !self.dacl_sort_desc;
                    } else {
                        self.dacl_sort_col = col;
                        self.dacl_sort_desc = false;
                    }
                }
            }
        }

        action
    }
}
