//! Unified Multi-Selection State, Deduplicated Hierarchy Stats, Clipboard Formatters,
//! and Shared Context / Batch Action Menu for MediaStat GUI.

use ahash::AHashSet;
use egui::{Color32, Modifiers, RichText, Ui};
use mediastat_core::{
    format_bytes, format_count, format_timestamp_opts, t, AppSettings, FileCategory, NodeFlags,
    NodeId, ScanTree,
};
use mediastat_scanner::resolve_to_unc_path;

use crate::shell_actions::NtfsCompressionAlg;

/// Unified selection state shared across Tree, Search, Uncompressed Media, Largest Files,
/// Duplicates, Permissions, Treemap, Inspector, and Top Menus/Toolbar.
#[derive(Debug, Default, Clone)]
pub struct SelectionState {
    selected: AHashSet<NodeId>,
    order: Vec<NodeId>,
    primary: Option<NodeId>,
    anchor: Option<NodeId>,
}

/// Accurate aggregated statistics for the current selection (ancestor-deduplicated to avoid
/// double-counting when both a folder and files inside it are selected).
#[derive(Debug, Default, Clone, Copy)]
pub struct SelectionStats {
    pub total_items: usize,
    pub file_count: usize,
    pub dir_count: usize,
    pub logical_bytes: u64,
    pub allocated_bytes: u64,
    pub single_node: Option<NodeId>,
}

impl SelectionStats {
    pub fn has_files(&self) -> bool {
        self.file_count > 0
    }

    pub fn has_dirs(&self) -> bool {
        self.dir_count > 0
    }
}

/// Unified action emitted by any view, context menu, selection bar, or Inspector panel.
#[derive(Debug, Clone)]
pub enum NodeAction {
    // Selection management
    SelectAllVisible(Vec<NodeId>),
    InvertVisible(Vec<NodeId>),
    ClearSelection,

    // Clipboard
    CopyPaths(Vec<NodeId>),
    CopyNames(Vec<NodeId>),
    CopyUncPaths(Vec<NodeId>),
    CopyTsv(Vec<NodeId>),

    // Explore / inspect / navigate
    RevealInExplorer(Vec<NodeId>),
    OpenFiles(Vec<NodeId>),
    OpenPowerShell(NodeId, bool),
    OpenCmd(NodeId),
    ShowProperties(NodeId),
    ComputeHashes(NodeId),
    ZoomTreemap(NodeId),
    SearchInFolder(NodeId),
    RefreshSelected(NodeId),
    ExportNodes(Vec<NodeId>),
    ExcludeNodes(Vec<NodeId>),

    // SysAdmin / Cleanup (guarded by Read-Only safety lock)
    MoveNodes(Vec<NodeId>),
    NtfsCompress(Vec<NodeId>, NtfsCompressionAlg),
    RemoveMotw(Vec<NodeId>),
    SparsifyFiles(Vec<NodeId>),
    SyncFolderDates(Vec<NodeId>),
    RemoveEmptySubfolders(Vec<NodeId>),
    RunUserCleanup(usize, Vec<NodeId>),
    TrashNodes(Vec<NodeId>),
    DeleteNodesPermanently(Vec<NodeId>),
}

impl SelectionState {
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    pub fn len(&self) -> usize {
        self.selected.len()
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.selected.contains(&id)
    }

    pub fn primary(&self) -> Option<NodeId> {
        self.primary
    }

    pub fn items(&self) -> &[NodeId] {
        &self.order
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.order.clear();
        self.primary = None;
        self.anchor = None;
    }

    pub fn select_single(&mut self, id: NodeId) {
        if id.is_none() || id == NodeId::ROOT {
            self.clear();
            return;
        }
        self.selected.clear();
        self.order.clear();
        self.selected.insert(id);
        self.order.push(id);
        self.primary = Some(id);
        self.anchor = Some(id);
    }

    pub fn toggle(&mut self, id: NodeId) {
        if id.is_none() || id == NodeId::ROOT {
            return;
        }
        if self.selected.remove(&id) {
            self.order.retain(|&n| n != id);
            if self.primary == Some(id) {
                self.primary = self.order.last().copied();
            }
            if self.anchor == Some(id) {
                self.anchor = self.primary;
            }
        } else {
            self.selected.insert(id);
            self.order.push(id);
            self.primary = Some(id);
            self.anchor = Some(id);
        }
    }

    pub fn set_checked(&mut self, id: NodeId, checked: bool) {
        if id.is_none() || id == NodeId::ROOT {
            return;
        }
        if checked {
            if self.selected.insert(id) {
                self.order.push(id);
            }
            self.primary = Some(id);
            self.anchor = Some(id);
        } else if self.selected.remove(&id) {
            self.order.retain(|&n| n != id);
            if self.primary == Some(id) {
                self.primary = self.order.last().copied();
            }
            if self.anchor == Some(id) {
                self.anchor = self.primary;
            }
        }
    }

    /// Standard Windows Explorer `Shift+Click` (and `Ctrl+Shift+Click`) range selection.
    pub fn select_range(&mut self, target: NodeId, ordered_visible: &[NodeId], additive: bool) {
        if target.is_none() || target == NodeId::ROOT {
            return;
        }
        let Some(target_pos) = ordered_visible.iter().position(|&n| n == target) else {
            if additive {
                self.set_checked(target, true);
            } else {
                self.select_single(target);
            }
            return;
        };

        let anchor_id = self.anchor.or(self.primary).unwrap_or(target);
        let anchor_pos = ordered_visible
            .iter()
            .position(|&n| n == anchor_id)
            .unwrap_or(target_pos);

        let (lo, hi) = if anchor_pos <= target_pos {
            (anchor_pos, target_pos)
        } else {
            (target_pos, anchor_pos)
        };

        if !additive {
            self.selected.clear();
            self.order.clear();
        }
        for &nid in &ordered_visible[lo..=hi] {
            if nid.is_some() && nid != NodeId::ROOT && self.selected.insert(nid) {
                self.order.push(nid);
            }
        }
        self.primary = Some(target);
        if self.anchor.is_none() {
            self.anchor = Some(ordered_visible[anchor_pos]);
        }
    }

    /// Standard click handler: plain click = single, `Ctrl+Click` = toggle, `Shift+Click` = range.
    pub fn handle_click(&mut self, id: NodeId, modifiers: Modifiers, ordered_visible: &[NodeId]) {
        if modifiers.shift {
            self.select_range(id, ordered_visible, modifiers.ctrl || modifiers.command);
        } else if modifiers.ctrl || modifiers.command {
            self.toggle(id);
        } else {
            self.select_single(id);
        }
    }

    /// Windows Explorer right-click semantics: if `id` is already part of a multi-selection,
    /// keep the entire selection intact; otherwise replace selection with `[id]`.
    pub fn ensure_for_context_menu(&mut self, id: NodeId) {
        if id.is_none() || id == NodeId::ROOT {
            return;
        }
        if self.selected.contains(&id) {
            self.primary = Some(id);
        } else {
            self.select_single(id);
        }
    }

    /// Compute the effective target list for a right-click context menu on `nid` (even on the
    /// exact frame the menu opens before deferred `ensure_for_context_menu` runs).
    pub fn context_targets_for(&self, nid: NodeId) -> Vec<NodeId> {
        if self.selected.contains(&nid) && !self.order.is_empty() {
            self.order.clone()
        } else if nid.is_some() && nid != NodeId::ROOT {
            vec![nid]
        } else {
            Vec::new()
        }
    }

    pub fn select_all(&mut self, ordered_visible: &[NodeId]) {
        self.selected.clear();
        self.order.clear();
        for &nid in ordered_visible {
            if nid.is_some() && nid != NodeId::ROOT && self.selected.insert(nid) {
                self.order.push(nid);
            }
        }
        self.primary = self.order.first().copied();
        self.anchor = self.primary;
    }

    pub fn select_many(&mut self, nodes: impl IntoIterator<Item = NodeId>) {
        self.selected.clear();
        self.order.clear();
        for nid in nodes {
            if nid.is_some() && nid != NodeId::ROOT && self.selected.insert(nid) {
                self.order.push(nid);
            }
        }
        self.primary = self.order.first().copied();
        self.anchor = self.primary;
    }

    pub fn invert_selection(&mut self, ordered_visible: &[NodeId]) {
        let prev = std::mem::take(&mut self.selected);
        self.order.clear();
        for &nid in ordered_visible {
            if nid.is_some() && nid != NodeId::ROOT && !prev.contains(&nid) && self.selected.insert(nid) {
                self.order.push(nid);
            }
        }
        self.primary = self.order.first().copied();
        self.anchor = self.primary;
    }

    pub fn prune_invalid(&mut self, tree: &ScanTree) {
        self.order.retain(|&nid| {
            let ok = nid.is_some()
                && nid != NodeId::ROOT
                && nid.index() < tree.len()
                && !tree.is_deleted(nid);
            if !ok {
                self.selected.remove(&nid);
            }
            ok
        });
        if let Some(p) = self.primary {
            if !self.selected.contains(&p) {
                self.primary = self.order.last().copied();
            }
        }
        if let Some(a) = self.anchor {
            if !self.selected.contains(&a) {
                self.anchor = self.primary;
            }
        }
    }

    /// Filter out any selected node that has a strict ancestor also in `self.selected`.
    /// Prevents double-counting sizes and redundant nested deletes/moves.
    #[cfg(test)]
    pub fn deduplicated_roots(&self, tree: &ScanTree) -> Vec<NodeId> {
        deduplicate_roots_slice(&self.order, tree)
    }

    pub fn compute_stats(&self, tree: &ScanTree) -> SelectionStats {
        compute_stats_for_nodes(&self.order, tree)
    }
}

/// Remove any node in `nodes` whose strict ancestor is also present in `nodes`.
pub fn deduplicate_roots_slice(nodes: &[NodeId], tree: &ScanTree) -> Vec<NodeId> {
    let set: AHashSet<NodeId> = nodes
        .iter()
        .copied()
        .filter(|&n| n.is_some() && n != NodeId::ROOT && n.index() < tree.len() && !tree.is_deleted(n))
        .collect();

    let mut out = Vec::with_capacity(set.len());
    let mut emitted = AHashSet::with_capacity(set.len());
    for &nid in nodes {
        if !set.contains(&nid) || !emitted.insert(nid) {
            continue;
        }
        let mut cur = tree.parents[nid.index()];
        let mut has_selected_ancestor = false;
        while cur.is_some() && cur != NodeId::ROOT && cur.index() < tree.len() {
            if set.contains(&cur) {
                has_selected_ancestor = true;
                break;
            }
            cur = tree.parents[cur.index()];
        }
        if !has_selected_ancestor {
            out.push(nid);
        }
    }
    out
}

pub fn compute_stats_for_nodes(nodes: &[NodeId], tree: &ScanTree) -> SelectionStats {
    let valid: Vec<NodeId> = nodes
        .iter()
        .copied()
        .filter(|&n| n.is_some() && n != NodeId::ROOT && n.index() < tree.len() && !tree.is_deleted(n))
        .collect();
    if valid.is_empty() {
        return SelectionStats::default();
    }

    let mut file_count = 0usize;
    let mut dir_count = 0usize;
    for &nid in &valid {
        if tree.is_dir(nid) {
            dir_count += 1;
        } else {
            file_count += 1;
        }
    }

    let roots = deduplicate_roots_slice(&valid, tree);
    let mut logical_bytes = 0u64;
    let mut allocated_bytes = 0u64;

    for &nid in &roots {
        let idx = nid.index();
        logical_bytes = logical_bytes.saturating_add(tree.logical_sizes[idx]);
        let alloc = if !tree.is_dir(nid) && tree.flags[idx].contains(NodeFlags::IS_HARDLINK_DUP) {
            0
        } else {
            tree.allocated_sizes[idx]
        };
        allocated_bytes = allocated_bytes.saturating_add(alloc);
    }

    SelectionStats {
        total_items: valid.len(),
        file_count,
        dir_count,
        logical_bytes,
        allocated_bytes,
        single_node: if valid.len() == 1 {
            Some(valid[0])
        } else {
            None
        },
    }
}

pub fn format_paths_for_clipboard(nodes: &[NodeId], tree: &ScanTree) -> String {
    nodes
        .iter()
        .filter(|&&n| n.is_some() && n != NodeId::ROOT && n.index() < tree.len())
        .map(|&n| tree.resolve_path(n).to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\r\n")
}

pub fn format_names_for_clipboard(nodes: &[NodeId], tree: &ScanTree) -> String {
    nodes
        .iter()
        .filter(|&&n| n.is_some() && n != NodeId::ROOT && n.index() < tree.len())
        .map(|&n| tree.name(n).to_string())
        .collect::<Vec<_>>()
        .join("\r\n")
}

pub fn format_unc_paths_for_clipboard(nodes: &[NodeId], tree: &ScanTree) -> Option<String> {
    let mut any_distinct = false;
    let mut lines = Vec::with_capacity(nodes.len());
    for &nid in nodes {
        if nid.is_none() || nid == NodeId::ROOT || nid.index() >= tree.len() {
            continue;
        }
        let p = tree.resolve_path(nid);
        let p_str = p.to_string_lossy();
        let unc = resolve_to_unc_path(&p);
        if !unc.eq_ignore_ascii_case(&p_str) {
            any_distinct = true;
        }
        lines.push(unc);
    }
    if any_distinct {
        Some(lines.join("\r\n"))
    } else {
        None
    }
}

pub fn format_tsv_summary_for_clipboard(
    nodes: &[NodeId],
    tree: &ScanTree,
    show_seconds: bool,
) -> String {
    let mut lines = Vec::with_capacity(nodes.len() + 1);
    lines.push(format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        t("Nombre", "Name"),
        t("Tipo", "Type"),
        t("Lógico", "Logical"),
        t("En Disco", "On Disk"),
        t("Categoría", "Category"),
        t("Modificado", "Modified"),
        t("Ruta completa", "Full Path"),
    ));
    for &nid in nodes {
        if nid.is_none() || nid == NodeId::ROOT || nid.index() >= tree.len() {
            continue;
        }
        let idx = nid.index();
        let is_dir = tree.is_dir(nid);
        let kind = if is_dir {
            t("Carpeta", "Folder")
        } else {
            t("Archivo", "File")
        };
        let category = if is_dir {
            "—"
        } else {
            FileCategory::from_extension(tree.ext(nid)).short_label()
        };
        let ts = format_timestamp_opts(tree.mtimes[idx], show_seconds);
        let path = tree.resolve_path(nid);
        lines.push(format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            tree.name(nid),
            kind,
            format_bytes(tree.logical_sizes[idx]),
            format_bytes(tree.allocated_sizes[idx]),
            category,
            ts,
            path.to_string_lossy()
        ));
    }
    lines.join("\r\n")
}

/// Optional view-specific context passed into the unified context/actions menu.
pub struct MenuScope<'a> {
    pub visible_nodes: &'a [NodeId],
    pub allow_tree_ops: bool,
}

/// Render the unified single + multi-selection actions menu (used in ALL right-click context
/// menus, the floating Selection Action Bar, and the Multi-Selection Inspector!).
pub fn show_node_actions_menu(
    ui: &mut Ui,
    tree: &ScanTree,
    targets: &[NodeId],
    scope: Option<MenuScope<'_>>,
    settings: &AppSettings,
    read_only_locked: bool,
) -> Option<NodeAction> {
    let stats = compute_stats_for_nodes(targets, tree);
    if stats.total_items == 0 {
        return None;
    }

    let mut action: Option<NodeAction> = None;
    let is_multi = stats.total_items > 1;
    let n = stats.total_items;
    let single = stats.single_node;
    let primary = single.unwrap_or(targets[0]);
    let primary_is_dir = tree.is_dir(primary);

    // Multi-selection header summary banner
    if is_multi {
        ui.label(
            RichText::new(format!(
                "✓ {} {} ({} {}, {} {} · {})",
                format_count(n as u64),
                t("seleccionados", "selected"),
                format_count(stats.file_count as u64),
                t("fich.", "files"),
                format_count(stats.dir_count as u64),
                t("carp.", "dirs"),
                format_bytes(stats.logical_bytes),
            ))
            .small()
            .strong()
            .color(Color32::from_rgb(125, 200, 250)),
        );
        ui.separator();
    }

    // 1. Copy / Clipboard actions (always available, Read-Only safe!)
    let copy_path_lbl = if is_multi {
        format!(
            "📋 {} ({n})  (Ctrl+C)",
            t("Copiar rutas completas", "Copy full paths")
        )
    } else {
        t(
            "📋 Copiar ruta completa  (Ctrl+C)",
            "📋 Copy full path  (Ctrl+C)",
        )
        .to_string()
    };
    if ui.button(copy_path_lbl).clicked() {
        action = Some(NodeAction::CopyPaths(targets.to_vec()));
        ui.close_menu();
    }

    let copy_name_lbl = if is_multi {
        format!("📄 {} ({n})", t("Copiar nombres", "Copy names"))
    } else {
        t("📄 Copiar nombre", "📄 Copy name").to_string()
    };
    if ui.button(copy_name_lbl).clicked() {
        action = Some(NodeAction::CopyNames(targets.to_vec()));
        ui.close_menu();
    }

    // Check if primary or any target maps to a UNC path (fast check on first few targets)
    let has_unc = targets.iter().take(16).any(|&nid| {
        if nid.index() >= tree.len() {
            return false;
        }
        let p = tree.resolve_path(nid);
        !resolve_to_unc_path(&p).eq_ignore_ascii_case(&p.to_string_lossy())
    });
    if has_unc {
        let unc_lbl = if is_multi {
            format!("🌐 {} ({n})", t("Copiar rutas UNC reales", "Copy real UNC paths"))
        } else {
            t("🌐 Copiar ruta UNC real", "🌐 Copy real UNC path").to_string()
        };
        if ui.button(unc_lbl).clicked() {
            action = Some(NodeAction::CopyUncPaths(targets.to_vec()));
            ui.close_menu();
        }
    }

    if ui
        .button(t(
            "📊 Copiar resumen (TSV para Excel)",
            "📊 Copy summary (TSV for Excel)",
        ))
        .clicked()
    {
        action = Some(NodeAction::CopyTsv(targets.to_vec()));
        ui.close_menu();
    }

    ui.separator();

    // 2. Explore, Open, Terminal, Properties
    let exp_lbl = if is_multi {
        format!(
            "📂 {} ({n})",
            t("Mostrar en Explorador", "Reveal in Explorer")
        )
    } else {
        t(
            "📂 Mostrar en Explorador  (Ctrl+E)",
            "📂 Reveal in Explorer  (Ctrl+E)",
        )
        .to_string()
    };
    if ui.button(exp_lbl).clicked() {
        action = Some(NodeAction::RevealInExplorer(targets.to_vec()));
        ui.close_menu();
    }

    if stats.has_files() && stats.file_count <= 10 {
        let open_lbl = if stats.file_count > 1 {
            format!(
                "▶ {} ({})",
                t("Abrir ficheros", "Open files"),
                stats.file_count
            )
        } else {
            t("▶ Abrir fichero", "▶ Open file").to_string()
        };
        if ui.button(open_lbl).clicked() {
            let files: Vec<NodeId> = targets
                .iter()
                .copied()
                .filter(|&nid| nid.index() < tree.len() && !tree.is_dir(nid))
                .collect();
            action = Some(NodeAction::OpenFiles(files));
            ui.close_menu();
        }
    }

    if ui
        .button(t(">_ Abrir PowerShell aquí", ">_ Open PowerShell here"))
        .clicked()
    {
        action = Some(NodeAction::OpenPowerShell(primary, false));
        ui.close_menu();
    }
    if ui
        .button(t("⌨ Abrir CMD aquí", "⌨ Open CMD here"))
        .clicked()
    {
        action = Some(NodeAction::OpenCmd(primary));
        ui.close_menu();
    }
    if !is_multi
        && ui
            .button(t(
                "ℹ Propiedades de Windows  (Alt+Enter)",
                "ℹ Windows Properties  (Alt+Enter)",
            ))
            .clicked()
    {
        action = Some(NodeAction::ShowProperties(primary));
        ui.close_menu();
    }

    ui.separator();

    // 3. Navigation, Analysis & Export
    if let Some(nid) = single {
        if primary_is_dir {
            if scope.as_ref().is_none_or(|s| s.allow_tree_ops)
                && ui
                    .button(t(
                        "↻ Refrescar esta carpeta  (F5)",
                        "↻ Refresh this folder  (F5)",
                    ))
                    .clicked()
            {
                action = Some(NodeAction::RefreshSelected(nid));
                ui.close_menu();
            }
            if ui
                .button(t(
                    "🔎 Buscar dentro de esta carpeta...",
                    "🔎 Search inside this folder...",
                ))
                .clicked()
            {
                action = Some(NodeAction::SearchInFolder(nid));
                ui.close_menu();
            }
        }
        let zoom_dir = if primary_is_dir {
            nid
        } else {
            tree.parents[nid.index()]
        };
        if zoom_dir.is_some()
            && zoom_dir != NodeId::ROOT
            && ui
                .button(t("🔍 Zoom carpeta en Mapa", "🔍 Zoom folder on Map"))
                .clicked()
        {
            action = Some(NodeAction::ZoomTreemap(zoom_dir));
            ui.close_menu();
        }
        if !primary_is_dir
            && ui
                .button(t(
                    "🔑 Calcular Hashes (MD5 / SHA-256 / BLAKE3)",
                    "🔑 Compute Hashes (MD5 / SHA-256 / BLAKE3)",
                ))
                .clicked()
        {
            action = Some(NodeAction::ComputeHashes(nid));
            ui.close_menu();
        }
    }

    let exp_sel_lbl = if is_multi {
        format!(
            "📤 {} ({n})...",
            t("Exportar seleccionados", "Export selected")
        )
    } else {
        t("📤 Exportar elemento...", "📤 Export item...").to_string()
    };
    if ui.button(exp_sel_lbl).clicked() {
        action = Some(NodeAction::ExportNodes(targets.to_vec()));
        ui.close_menu();
    }

    let excl_lbl = if is_multi {
        format!(
            "🚫 {} ({n})",
            t("Excluir en filtro", "Exclude in filter")
        )
    } else {
        t("🚫 Excluir en filtro", "🚫 Exclude in filter").to_string()
    };
    if ui.button(excl_lbl).clicked() {
        action = Some(NodeAction::ExcludeNodes(targets.to_vec()));
        ui.close_menu();
    }

    // 4. Selection helpers (when view exposes its visible list)
    if let Some(sc) = scope {
        if !sc.visible_nodes.is_empty() {
            ui.separator();
            ui.menu_button(t("☑ Selección", "☑ Selection"), |ui| {
                if ui
                    .button(format!(
                        "{} ({})  (Ctrl+A)",
                        t("☑ Seleccionar todo en esta vista", "☑ Select all in this view"),
                        format_count(sc.visible_nodes.len() as u64)
                    ))
                    .clicked()
                {
                    action = Some(NodeAction::SelectAllVisible(sc.visible_nodes.to_vec()));
                    ui.close_menu();
                }
                if ui
                    .button(t("⇄ Invertir selección", "⇄ Invert selection"))
                    .clicked()
                {
                    action = Some(NodeAction::InvertVisible(sc.visible_nodes.to_vec()));
                    ui.close_menu();
                }
                if ui
                    .button(t("✕ Limpiar selección  (Esc)", "✕ Clear selection  (Esc)"))
                    .clicked()
                {
                    action = Some(NodeAction::ClearSelection);
                    ui.close_menu();
                }
            });
        }
    }

    ui.separator();

    // 5. SysAdmin / Write Tools (guarded by Read-Only Lock)
    let lock_hint = t(
        "Desbloquea el candado 🔒 Solo Lectura en la barra superior",
        "Unlock the 🔒 Read-Only lock in the top toolbar",
    );

    let move_lbl = if is_multi {
        format!(
            "📦 {} ({n})...",
            t("Mover a otra carpeta", "Move to another folder")
        )
    } else {
        t("📦 Mover a otra carpeta...", "📦 Move to another folder...").to_string()
    };
    if ui
        .add_enabled(!read_only_locked, egui::Button::new(move_lbl))
        .on_disabled_hover_text(lock_hint)
        .clicked()
    {
        action = Some(NodeAction::MoveNodes(targets.to_vec()));
        ui.close_menu();
    }

    ui.menu_button(
        t("🗜 Compresión NTFS / CompactOS", "🗜 NTFS / CompactOS Compression"),
        |ui| {
            for alg in [
                NtfsCompressionAlg::None,
                NtfsCompressionAlg::Lznt1,
                NtfsCompressionAlg::Xpress4k,
                NtfsCompressionAlg::Xpress8k,
                NtfsCompressionAlg::Xpress16k,
                NtfsCompressionAlg::Lzx,
            ] {
                if ui
                    .add_enabled(!read_only_locked, egui::Button::new(alg.label()))
                    .on_disabled_hover_text(lock_hint)
                    .clicked()
                {
                    action = Some(NodeAction::NtfsCompress(targets.to_vec(), alg));
                    ui.close_menu();
                }
            }
        },
    );

    if stats.has_files() {
        let motw_lbl = if stats.file_count > 1 {
            format!(
                "🔓 {} ({})",
                t("Quitar marca de Internet (MOTW)", "Remove Mark of the Web (MOTW)"),
                stats.file_count
            )
        } else {
            t(
                "🔓 Quitar marca de Internet (MOTW)",
                "🔓 Remove Mark of the Web (MOTW)",
            )
            .to_string()
        };
        if ui
            .add_enabled(!read_only_locked, egui::Button::new(motw_lbl))
            .on_disabled_hover_text(lock_hint)
            .clicked()
        {
            action = Some(NodeAction::RemoveMotw(targets.to_vec()));
            ui.close_menu();
        }

        let sparse_lbl = if stats.file_count > 1 {
            format!(
                "🕳 {} ({})",
                t("Sparsificar ficheros (fsutil)", "Sparsify files (fsutil)"),
                stats.file_count
            )
        } else {
            t(
                "🕳 Sparsificar fichero (fsutil)",
                "🕳 Sparsify file (fsutil)",
            )
            .to_string()
        };
        if ui
            .add_enabled(!read_only_locked, egui::Button::new(sparse_lbl))
            .on_disabled_hover_text(lock_hint)
            .clicked()
        {
            action = Some(NodeAction::SparsifyFiles(targets.to_vec()));
            ui.close_menu();
        }
    }

    if stats.has_dirs() {
        let sync_lbl = if stats.dir_count > 1 {
            format!(
                "🕒 {} ({})",
                t(
                    "Sincronizar fechas de carpetas (más reciente)",
                    "Sync folder dates (newest child)"
                ),
                stats.dir_count
            )
        } else {
            t(
                "🕒 Sincronizar fechas de carpetas (más reciente)",
                "🕒 Sync folder dates (newest child)",
            )
            .to_string()
        };
        if ui
            .add_enabled(!read_only_locked, egui::Button::new(sync_lbl))
            .on_disabled_hover_text(lock_hint)
            .clicked()
        {
            action = Some(NodeAction::SyncFolderDates(targets.to_vec()));
            ui.close_menu();
        }

        let empty_lbl = if stats.dir_count > 1 {
            format!(
                "📁 {} ({})",
                t("Eliminar subcarpetas vacías", "Remove empty subfolders"),
                stats.dir_count
            )
        } else {
            t(
                "📁 Eliminar subcarpetas vacías",
                "📁 Remove empty subfolders",
            )
            .to_string()
        };
        if ui
            .add_enabled(!read_only_locked, egui::Button::new(empty_lbl))
            .on_disabled_hover_text(lock_hint)
            .clicked()
        {
            action = Some(NodeAction::RemoveEmptySubfolders(targets.to_vec()));
            ui.close_menu();
        }
    }

    ui.menu_button(
        t("⚡ Acciones Rápidas SMB (Ctrl+0..9)", "⚡ SMB Quick Actions (Ctrl+0..9)"),
        |ui| {
            for (slot, act) in settings.user_cleanups.iter().enumerate() {
                if !act.enabled || act.command_line.trim().is_empty() {
                    continue;
                }
                let applicable = (stats.has_dirs() && act.works_for_dirs)
                    || (stats.has_files() && act.works_for_files);
                let permitted = act.read_only_safe || !read_only_locked;
                let tag = if act.read_only_safe { "👁" } else { "⚠" };
                if ui
                    .add_enabled(
                        applicable && permitted,
                        egui::Button::new(format!("{tag} Ctrl+{slot}: {}", act.title)),
                    )
                    .on_disabled_hover_text(if !permitted {
                        t(
                            "Acción de escritura: desbloquea 🔒 Solo Lectura en la barra superior",
                            "Write action: unlock 🔒 Read-Only lock in the top toolbar",
                        )
                    } else {
                        t(
                            "No aplicable a los elementos seleccionados",
                            "Not applicable to the selected items",
                        )
                    })
                    .clicked()
                {
                    action = Some(NodeAction::RunUserCleanup(slot, targets.to_vec()));
                    ui.close_menu();
                }
            }
        },
    );

    ui.separator();

    // 6. Destructive actions (Trash / Permanent Delete)
    let trash_lbl = if is_multi {
        format!(
            "🗑 {} ({n})  (Del)",
            t("Enviar a Papelera", "Move to Recycle Bin")
        )
    } else {
        t(
            "🗑 Enviar a Papelera  (Del)",
            "🗑 Move to Recycle Bin  (Del)",
        )
        .to_string()
    };
    if ui
        .add_enabled(!read_only_locked, egui::Button::new(trash_lbl))
        .on_disabled_hover_text(lock_hint)
        .clicked()
    {
        action = Some(NodeAction::TrashNodes(targets.to_vec()));
        ui.close_menu();
    }

    let del_lbl = if is_multi {
        format!(
            "☠ {} ({n})  (Shift+Del)",
            t("Eliminar permanentemente", "Delete permanently")
        )
    } else {
        t(
            "☠ Eliminar permanentemente  (Shift+Del)",
            "☠ Delete permanently  (Shift+Del)",
        )
        .to_string()
    };
    if ui
        .add_enabled(
            !read_only_locked,
            egui::Button::new(RichText::new(del_lbl).color(Color32::from_rgb(240, 85, 85))),
        )
        .on_disabled_hover_text(lock_hint)
        .clicked()
    {
        action = Some(NodeAction::DeleteNodesPermanently(targets.to_vec()));
        ui.close_menu();
    }

    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn sample_tree() -> (ScanTree, NodeId, NodeId, NodeId, NodeId) {
        let mut tree = ScanTree::new();
        let root = tree.add_root_target(Path::new(r"C:\Audit"));
        let folder = tree.add_child(root, "Media", NodeFlags::IS_DIR, 0, 0, 1_700_000_000);
        let f1 = tree.add_child(
            folder,
            "a.bmp",
            NodeFlags::empty(),
            10_000_000,
            10_000_000,
            1_700_000_001,
        );
        let f2 = tree.add_child(
            folder,
            "b.png",
            NodeFlags::empty(),
            5_000_000,
            5_000_000,
            1_700_000_002,
        );
        let f3 = tree.add_child(root, "notes.txt", NodeFlags::empty(), 2_000, 4_096, 1_700_000_003);
        tree.recompute_aggregates();
        (tree, folder, f1, f2, f3)
    }

    #[test]
    fn test_multi_selection_click_ctrl_shift_range_and_context_menu() {
        let (_tree, folder, f1, f2, f3) = sample_tree();
        let visible = vec![folder, f1, f2, f3];
        let mut sel = SelectionState::default();

        // Plain click selects single
        sel.handle_click(f1, Modifiers::NONE, &visible);
        assert_eq!(sel.items(), &[f1]);
        assert_eq!(sel.primary(), Some(f1));

        // Shift+Click selects contiguous range [f1, f2, f3]
        sel.handle_click(f3, Modifiers::SHIFT, &visible);
        assert_eq!(sel.items(), &[f1, f2, f3]);
        assert_eq!(sel.primary(), Some(f3));

        // Ctrl+Click toggles f2 off
        sel.handle_click(f2, Modifiers::CTRL, &visible);
        assert_eq!(sel.items(), &[f1, f3]);
        assert!(!sel.contains(f2));

        // Right-clicking an already-selected node preserves the entire multi-selection!
        sel.ensure_for_context_menu(f1);
        assert_eq!(sel.items(), &[f1, f3]);
        assert_eq!(sel.primary(), Some(f1));

        // Right-clicking an UNSELECTED node switches selection to just that node
        sel.ensure_for_context_menu(folder);
        assert_eq!(sel.items(), &[folder]);

        // Select All & Invert
        sel.select_all(&visible);
        assert_eq!(sel.len(), 4);
        sel.toggle(f2);
        sel.invert_selection(&visible);
        assert_eq!(sel.items(), &[f2]);
    }

    #[test]
    fn test_selection_ancestor_deduplication_and_clipboard_formatters() {
        let (tree, folder, f1, f2, f3) = sample_tree();
        let mut sel = SelectionState::default();
        // Select BOTH `folder` (15 MB) AND `f1` inside it (10 MB) AND sibling `f3` (2 KB)
        sel.select_many([folder, f1, f3]);

        let roots = sel.deduplicated_roots(&tree);
        assert_eq!(roots, vec![folder, f3]);

        // Size totals must NOT double-count f1 inside folder!
        let stats = sel.compute_stats(&tree);
        assert_eq!(stats.total_items, 3);
        assert_eq!(stats.dir_count, 1);
        assert_eq!(stats.file_count, 2);
        assert_eq!(stats.logical_bytes, 15_002_000);

        // Clipboard helpers
        let paths = format_paths_for_clipboard(sel.items(), &tree);
        assert!(paths.contains(r"C:\Audit\Media"));
        assert!(paths.contains(r"C:\Audit\Media\a.bmp"));
        assert!(paths.contains(r"C:\Audit\notes.txt"));
        assert_eq!(paths.lines().count(), 3);

        let names = format_names_for_clipboard(sel.items(), &tree);
        assert_eq!(names, "Media\r\na.bmp\r\nnotes.txt");

        let tsv = format_tsv_summary_for_clipboard(&[f1, f2], &tree, false);
        assert_eq!(tsv.lines().count(), 3); // header + 2 rows
        assert!(tsv.contains("a.bmp"));
        assert!(tsv.contains("b.png"));
    }
}
