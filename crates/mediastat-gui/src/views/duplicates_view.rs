use egui::{Color32, RichText, Ui};
use mediastat_core::{format_bytes, format_count, t, AppSettings, NodeId, ScanTree};

use crate::selection::{show_node_actions_menu, MenuScope, NodeAction, SelectionState};
use crate::shell_actions::reveal_in_explorer;

pub fn show_duplicates_view(
    ui: &mut Ui,
    tree: &ScanTree,
    selection: &mut SelectionState,
    settings: &AppSettings,
    read_only_locked: bool,
) -> Option<NodeAction> {
    if tree.duplicates.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(
                RichText::new(t(
                    "No se encontraron grupos de duplicados (BLAKE3) en este análisis.",
                    "No duplicate groups (BLAKE3) found in this scan.",
                ))
                .weak(),
            );
        });
        return None;
    }

    let mut visible_nodes: Vec<NodeId> = Vec::new();
    let mut redundant_copies: Vec<NodeId> = Vec::new();
    for grp in tree.duplicates.iter().take(500) {
        let mut first_live = true;
        for &nid in &grp.nodes {
            if tree.is_deleted(nid) {
                continue;
            }
            visible_nodes.push(nid);
            if first_live {
                first_live = false;
            } else {
                redundant_copies.push(nid);
            }
        }
    }

    let mut action: Option<NodeAction> = None;
    let mut pending_click: Option<(NodeId, egui::Modifiers)> = None;
    let mut pending_check: Option<(NodeId, bool)> = None;
    let mut pending_rclick: Option<NodeId> = None;

    let total_reclaim: u64 = tree.duplicates.iter().map(|g| g.reclaimable_bytes()).sum();
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!(
            "{} {}",
            format_count(tree.duplicates.len() as u64),
            t("grupos duplicados", "duplicate groups")
        ));
        ui.separator();
        ui.label(
            RichText::new(format!(
                "{}: {}",
                t("Espacio recuperable", "Reclaimable space"),
                format_bytes(total_reclaim)
            ))
            .strong()
            .color(Color32::from_rgb(76, 210, 128)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !redundant_copies.is_empty()
                && ui
                    .small_button(t(
                        "☑ Seleccionar copias redundantes (conservar 1º)",
                        "☑ Select redundant copies (keep 1st)",
                    ))
                    .on_hover_text(t(
                        "Selecciona todas las copias duplicadas manteniendo intacto el primer archivo de cada grupo",
                        "Selects all duplicate copies while keeping the first file in each group untouched",
                    ))
                    .clicked()
            {
                selection.select_many(redundant_copies.iter().copied());
            }
        });
    });
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (idx, grp) in tree.duplicates.iter().take(500).enumerate() {
            let kind_tag = if grp.is_probable_sparse {
                t("PROBABLE (Sparse >64 MiB)", "LIKELY (Sparse >64 MiB)")
            } else {
                t("EXACTO (BLAKE3)", "EXACT (BLAKE3)")
            };
            let header = format!(
                "#{} · [{}] {} {} × {} = {} {}  [{}…]",
                idx + 1,
                kind_tag,
                grp.nodes.len(),
                t("copias", "copies"),
                format_bytes(grp.file_size),
                format_bytes(grp.reclaimable_bytes()),
                t("recuperables", "reclaimable"),
                &grp.blake3_hex[..grp.blake3_hex.len().min(12)]
            );
            egui::CollapsingHeader::new(RichText::new(header).strong())
                .default_open(idx < 5)
                .show(ui, |ui| {
                    for &nid in &grp.nodes {
                        if tree.is_deleted(nid) {
                            continue;
                        }
                        let is_sel = selection.contains(nid);
                        let p = tree.resolve_path(nid);
                        ui.horizontal(|ui| {
                            let mut chk = is_sel;
                            if ui.checkbox(&mut chk, "").changed() {
                                pending_check = Some((nid, chk));
                            }
                            if ui
                                .small_button("📂")
                                .on_hover_text(t("Abrir en Explorador", "Reveal in Explorer"))
                                .clicked()
                            {
                                reveal_in_explorer(&p);
                            }
                            if ui
                                .small_button("📋")
                                .on_hover_text(t("Copiar ruta", "Copy path"))
                                .clicked()
                            {
                                ui.ctx().copy_text(p.to_string_lossy().into_owned());
                            }
                            let resp = ui.selectable_label(
                                is_sel,
                                RichText::new(p.to_string_lossy().as_ref()).monospace(),
                            );
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
                                    action = Some(act);
                                }
                            });
                        });
                    }
                });
        }
    });

    if let Some((nid, chk)) = pending_check {
        selection.set_checked(nid, chk);
    }
    if let Some((nid, mods)) = pending_click {
        selection.handle_click(nid, mods, &visible_nodes);
    }
    if let Some(nid) = pending_rclick {
        selection.ensure_for_context_menu(nid);
    }

    action
}
