//! Supercharged Advanced File Search Tab (WinDirStat `SearchDlg`/`FileSearchControl` parity
//! + Multi-Category Chips + Multi-Extension Include/Exclude + Logical/Physical Size Units + MediaStat Diagnostics).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use ahash::AHashSet;
use egui::{Color32, RichText, Stroke, Ui};
use egui_extras::{Column, TableBuilder};
use mediastat_core::{
    format_bytes, format_count, format_timestamp_opts, t, AppSettings, FileCategory, NodeFlags,
    NodeId, ScanTree, WinAttrs,
};
use regex::RegexBuilder;

use crate::selection::{show_node_actions_menu, MenuScope, NodeAction, SelectionState};
use crate::shell_actions::reveal_in_explorer;
use crate::views::sort_header_button;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeUnit {
    Bytes,
    Kb,
    Mb,
    Gb,
    Tb,
}

impl SizeUnit {
    pub const ALL: [Self; 5] = [Self::Bytes, Self::Kb, Self::Mb, Self::Gb, Self::Tb];

    pub fn label(self) -> &'static str {
        match self {
            Self::Bytes => "B",
            Self::Kb => "KB",
            Self::Mb => "MB",
            Self::Gb => "GB",
            Self::Tb => "TB",
        }
    }

    pub fn to_bytes(self, val: f64) -> u64 {
        if !val.is_finite() || val <= 0.0 {
            return 0;
        }
        let mult: f64 = match self {
            Self::Bytes => 1.0,
            Self::Kb => 1024.0,
            Self::Mb => 1024.0 * 1024.0,
            Self::Gb => 1024.0 * 1024.0 * 1024.0,
            Self::Tb => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        };
        (val * mult).min(u64::MAX as f64) as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchSortColumn {
    Name,
    CategoryExt,
    LogicalSize,
    AllocatedSize,
    PctTotal,
    Flags,
    Modified,
    Owner,
    Attributes,
    FullPath,
}

pub struct SearchViewState {
    // Query & matching flags
    pub query: String,
    pub use_regex: bool,
    pub whole_phrase: bool,
    pub case_sensitive: bool,
    pub match_full_path: bool,
    pub include_files: bool,
    pub include_dirs: bool,
    pub scope_folder: Option<NodeId>,

    // Advanced panel toggle
    pub advanced_open: bool,

    // Categories (multi-select)
    pub selected_categories: Vec<FileCategory>,

    // Extensions include/exclude
    pub include_exts_input: String,
    pub exclude_exts_input: String,

    // Logical & Physical size bounds with units
    pub min_logical_val: f64,
    pub min_logical_unit: SizeUnit,
    pub max_logical_val: f64,
    pub max_logical_unit: SizeUnit,
    pub min_alloc_val: f64,
    pub min_alloc_unit: SizeUnit,
    pub max_alloc_val: f64,
    pub max_alloc_unit: SizeUnit,

    // Age & Owner
    pub older_than_days: u32,
    pub newer_than_days: u32,
    pub owner_query: String,

    // Diagnostic & Win32 attribute filters
    pub only_duplicates: bool,
    pub only_hardlinks: bool,
    pub only_compressed: bool,
    pub only_offline_cloud: bool,
    pub only_hidden: bool,
    pub only_system: bool,
    pub only_readonly: bool,
    pub only_long_path: bool,

    // Results & exact aggregates
    pub results: Vec<NodeId>,
    pub total_matches_found: u64,
    pub total_files_matched: u64,
    pub total_dirs_matched: u64,
    pub total_logical_bytes: u64,
    pub total_allocated_bytes: u64,
    pub sort_col: SearchSortColumn,
    pub sort_desc: bool,
    pub regex_error: Option<String>,
    pub focus_input_requested: bool,
}

impl Default for SearchViewState {
    fn default() -> Self {
        Self {
            query: String::new(),
            use_regex: false,
            whole_phrase: false,
            case_sensitive: false,
            match_full_path: false,
            include_files: true,
            include_dirs: false,
            scope_folder: None,
            advanced_open: true,
            selected_categories: Vec::new(),
            include_exts_input: String::new(),
            exclude_exts_input: String::new(),
            min_logical_val: 0.0,
            min_logical_unit: SizeUnit::Mb,
            max_logical_val: 0.0,
            max_logical_unit: SizeUnit::Mb,
            min_alloc_val: 0.0,
            min_alloc_unit: SizeUnit::Mb,
            max_alloc_val: 0.0,
            max_alloc_unit: SizeUnit::Mb,
            older_than_days: 0,
            newer_than_days: 0,
            owner_query: String::new(),
            only_duplicates: false,
            only_hardlinks: false,
            only_compressed: false,
            only_offline_cloud: false,
            only_hidden: false,
            only_system: false,
            only_readonly: false,
            only_long_path: false,
            results: Vec::new(),
            total_matches_found: 0,
            total_files_matched: 0,
            total_dirs_matched: 0,
            total_logical_bytes: 0,
            total_allocated_bytes: 0,
            sort_col: SearchSortColumn::LogicalSize,
            sort_desc: true,
            regex_error: None,
            focus_input_requested: false,
        }
    }
}

impl SearchViewState {
    pub fn reset_filters(&mut self) {
        let adv = self.advanced_open;
        *self = Self::default();
        self.advanced_open = adv;
    }

    /// Jump directly to searching all files of a specific extension (from Extension List context menu).
    pub fn search_for_extension(&mut self, ext: &str, tree: &ScanTree, settings: &mut AppSettings) {
        self.reset_filters();
        self.include_exts_input = ext.trim_start_matches('.').to_ascii_lowercase();
        self.execute_search(tree, settings);
    }

    /// Scope search inside a specific directory (from Tree context menu).
    pub fn search_in_folder(&mut self, folder: NodeId, tree: &ScanTree, settings: &mut AppSettings) {
        self.scope_folder = Some(folder);
        self.focus_input_requested = true;
        self.execute_search(tree, settings);
    }

    pub fn execute_search(&mut self, tree: &ScanTree, settings: &mut AppSettings) {
        self.results.clear();
        self.total_matches_found = 0;
        self.total_files_matched = 0;
        self.total_dirs_matched = 0;
        self.total_logical_bytes = 0;
        self.total_allocated_bytes = 0;
        self.regex_error = None;

        if tree.is_empty() || (!self.include_files && !self.include_dirs) {
            return;
        }

        let trimmed = self.query.trim();
        let compiled_re = if trimmed.is_empty() {
            None
        } else {
            let mut pat = if self.use_regex {
                trimmed.to_string()
            } else if trimmed.contains('*') || trimmed.contains('?') {
                mediastat_core::filtering::glob_to_regex(trimmed)
            } else {
                regex::escape(trimmed)
            };
            if self.whole_phrase {
                pat = format!("^(?:{pat})$");
            }
            match RegexBuilder::new(&pat)
                .case_insensitive(!self.case_sensitive)
                .build()
            {
                Ok(re) => Some(re),
                Err(e) => {
                    self.regex_error = Some(format!("{}: {e}", t("Regex inválido", "Invalid Regex")));
                    return;
                }
            }
        };

        if !trimmed.is_empty() {
            settings.push_search_history(trimmed);
        }

        let inc_exts = parse_ext_set(&self.include_exts_input);
        let exc_exts = parse_ext_set(&self.exclude_exts_input);
        let has_cat_filter = !self.selected_categories.is_empty();

        let min_log = self.min_logical_unit.to_bytes(self.min_logical_val);
        let max_log = if self.max_logical_val > 0.0 {
            Some(self.max_logical_unit.to_bytes(self.max_logical_val))
        } else {
            None
        };
        let min_alloc = self.min_alloc_unit.to_bytes(self.min_alloc_val);
        let max_alloc = if self.max_alloc_val > 0.0 {
            Some(self.max_alloc_unit.to_bytes(self.max_alloc_val))
        } else {
            None
        };

        let now = chrono::Utc::now().timestamp();
        let older_cut = if self.older_than_days > 0 {
            Some(now - (self.older_than_days as i64 * 86_400))
        } else {
            None
        };
        let newer_cut = if self.newer_than_days > 0 {
            Some(now - (self.newer_than_days as i64 * 86_400))
        } else {
            None
        };
        let own_q = self.owner_query.trim().to_ascii_lowercase();

        // Build candidate list: scoped subtree OR entire tree
        let candidates: Vec<NodeId> = match self
            .scope_folder
            .filter(|&f| f != NodeId::ROOT && f.index() < tree.len() && !tree.is_deleted(f))
        {
            Some(scope_root) => {
                let mut out = Vec::new();
                let mut stack = vec![scope_root];
                while let Some(cur) = stack.pop() {
                    for c in tree.children(cur) {
                        if tree.is_deleted(c) {
                            continue;
                        }
                        out.push(c);
                        if tree.flags[c.index()].contains(NodeFlags::IS_DIR) {
                            stack.push(c);
                        }
                    }
                }
                out
            }
            None => (1..tree.len()).map(|i| NodeId(i as u32)).collect(),
        };

        // Precompute hardlinked FileIds set if `only_hardlinks` is active
        let hardlink_nodes: AHashSet<NodeId> = if self.only_hardlinks {
            tree.hardlinks
                .iter()
                .flat_map(|g| g.nodes.iter().copied())
                .collect()
        } else {
            AHashSet::new()
        };

        let cap = settings.search_max_results.clamp(100, 200_000);
        let mut heap: BinaryHeap<Reverse<(u64, NodeId)>> = BinaryHeap::with_capacity(cap.min(4096));

        for nid in candidates {
            let idx = nid.index();
            let f = tree.flags[idx];
            if f.contains(NodeFlags::IS_DELETED) {
                continue;
            }
            let is_dir = f.contains(NodeFlags::IS_DIR);
            if is_dir && !self.include_dirs {
                continue;
            }
            if !is_dir && !self.include_files {
                continue;
            }

            let log_sz = tree.logical_sizes[idx];
            let alloc_sz = if !is_dir && f.contains(NodeFlags::IS_HARDLINK_DUP) {
                0
            } else {
                tree.allocated_sizes[idx]
            };

            if log_sz < min_log {
                continue;
            }
            if let Some(mx) = max_log {
                if log_sz > mx {
                    continue;
                }
            }
            if alloc_sz < min_alloc {
                continue;
            }
            if let Some(mx) = max_alloc {
                if alloc_sz > mx {
                    continue;
                }
            }

            let mtime = if is_dir {
                tree.dir_stats
                    .get(&nid)
                    .map(|d| d.max_mtime)
                    .unwrap_or(tree.mtimes[idx])
            } else {
                tree.mtimes[idx]
            };
            if let Some(cut) = older_cut {
                if mtime <= 0 || mtime > cut {
                    continue;
                }
            }
            if let Some(cut) = newer_cut {
                if mtime <= 0 || mtime < cut {
                    continue;
                }
            }

            // Category & extension filters apply to files (dirs excluded when file-only filters are active)
            let ext = if is_dir { "" } else { tree.ext(nid) };
            if has_cat_filter {
                if is_dir {
                    continue;
                }
                let cat = FileCategory::from_extension(ext);
                if !self.selected_categories.contains(&cat) {
                    continue;
                }
            }
            if !inc_exts.is_empty() && (is_dir || !inc_exts.contains(ext)) {
                continue;
            }
            if !exc_exts.is_empty() && !is_dir && exc_exts.contains(ext) {
                continue;
            }

            // Diagnostic & Win32 attribute filters
            if self.only_duplicates && !f.contains(NodeFlags::IS_DUPLICATE) {
                continue;
            }
            if self.only_hardlinks && !hardlink_nodes.contains(&nid) {
                continue;
            }
            let attrs = tree.win_attrs[idx];
            if self.only_compressed && !attrs.contains(WinAttrs::COMPRESSED) {
                continue;
            }
            if self.only_offline_cloud && !attrs.contains(WinAttrs::OFFLINE_CLOUD) {
                continue;
            }
            if self.only_hidden && !attrs.contains(WinAttrs::HIDDEN) {
                continue;
            }
            if self.only_system && !attrs.contains(WinAttrs::SYSTEM) {
                continue;
            }
            if self.only_readonly && !attrs.contains(WinAttrs::READONLY) {
                continue;
            }
            if self.only_long_path && !f.contains(NodeFlags::IS_LONG_PATH) {
                continue;
            }

            if !own_q.is_empty() && !tree.owner(nid).to_ascii_lowercase().contains(&own_q) {
                continue;
            }

            if let Some(re) = &compiled_re {
                let matched = if self.match_full_path {
                    let p = tree.resolve_path(nid).to_string_lossy().into_owned();
                    re.is_match(&p)
                } else {
                    re.is_match(tree.name(nid))
                };
                if !matched {
                    continue;
                }
            }

            // Matched! Update true totals
            self.total_matches_found += 1;
            if is_dir {
                self.total_dirs_matched += 1;
            } else {
                self.total_files_matched += 1;
                self.total_logical_bytes = self.total_logical_bytes.saturating_add(log_sz);
                self.total_allocated_bytes = self.total_allocated_bytes.saturating_add(alloc_sz);
            }

            // Min-heap keeping the top-N largest matches
            if heap.len() < cap {
                heap.push(Reverse((log_sz, nid)));
            } else if let Some(&Reverse((smallest, _))) = heap.peek() {
                if log_sz > smallest {
                    heap.pop();
                    heap.push(Reverse((log_sz, nid)));
                }
            }
        }

        self.results = heap.into_iter().map(|Reverse((_, id))| id).collect();
        self.resort_results(tree);
    }

    pub fn resort_results(&mut self, tree: &ScanTree) {
        let col = self.sort_col;
        let desc = self.sort_desc;
        self.results.sort_unstable_by(|&a, &b| {
            let ia = a.index();
            let ib = b.index();
            let ord = match col {
                SearchSortColumn::Name => tree
                    .name(a)
                    .to_ascii_lowercase()
                    .cmp(&tree.name(b).to_ascii_lowercase()),
                SearchSortColumn::CategoryExt => tree.ext(a).cmp(tree.ext(b)),
                SearchSortColumn::LogicalSize | SearchSortColumn::PctTotal => {
                    tree.logical_sizes[ia].cmp(&tree.logical_sizes[ib])
                }
                SearchSortColumn::AllocatedSize => {
                    tree.allocated_sizes[ia].cmp(&tree.allocated_sizes[ib])
                }
                SearchSortColumn::Flags => search_node_flags_rank(tree, a)
                    .cmp(&search_node_flags_rank(tree, b)),
                SearchSortColumn::Modified => tree.mtimes[ia].cmp(&tree.mtimes[ib]),
                SearchSortColumn::Owner => tree.owner(a).cmp(tree.owner(b)),
                SearchSortColumn::Attributes => tree.win_attrs[ia]
                    .bits()
                    .cmp(&tree.win_attrs[ib].bits()),
                SearchSortColumn::FullPath => tree.resolve_path(a).cmp(&tree.resolve_path(b)),
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
        selection: &mut SelectionState,
        settings: &mut AppSettings,
        read_only_locked: bool,
    ) -> Option<NodeAction> {
        let mut action: Option<NodeAction> = None;
        let mut trigger_search = false;
        let mut sort_clicked: Option<SearchSortColumn> = None;
        let mut pending_click: Option<(NodeId, egui::Modifiers)> = None;
        let mut pending_check: Option<(NodeId, bool)> = None;
        let mut pending_rclick: Option<NodeId> = None;
        let mut master_toggle: Option<bool> = None;

        // 1. Primary Search Bar + History Dropdown
        ui.horizontal_wrapped(|ui| {
            ui.label(t("🔎 Patrón:", "🔎 Pattern:"));
            let te = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text(t(
                        "Nombre, *.bak, IMG_*, ^202[0-5].*\\.mov$",
                        "Name, *.bak, IMG_*, ^202[0-5].*\\.mov$",
                    ))
                    .desired_width(240.0),
            );
            if self.focus_input_requested {
                te.request_focus();
                self.focus_input_requested = false;
            }
            if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                trigger_search = true;
            }

            if !settings.search_history.is_empty() {
                ui.menu_button("🕘", |ui| {
                    ui.label(RichText::new(t("Historial reciente:", "Recent searches:")).small().weak());
                    let hist = settings.search_history.clone();
                    for h in hist {
                        if ui.button(&h).clicked() {
                            self.query = h;
                            trigger_search = true;
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    if ui.button(t("🗑 Limpiar historial", "🗑 Clear history")).clicked() {
                        settings.search_history.clear();
                        ui.close_menu();
                    }
                });
            }

            ui.checkbox(&mut self.use_regex, "Regex");
            ui.checkbox(&mut self.whole_phrase, t("Exacto", "Exact"));
            ui.checkbox(&mut self.case_sensitive, "Aa");
            ui.checkbox(&mut self.match_full_path, t("Ruta", "Path"));
            ui.separator();
            ui.checkbox(&mut self.include_files, t("Ficheros", "Files"));
            ui.checkbox(&mut self.include_dirs, t("Carpetas", "Folders"));

            if ui
                .button(
                    RichText::new(t("🔍 Buscar", "🔍 Search"))
                        .strong()
                        .color(Color32::from_rgb(100, 185, 255)),
                )
                .clicked()
            {
                trigger_search = true;
            }
            if ui
                .button(if self.advanced_open {
                    t("▾ Filtros", "▾ Filters")
                } else {
                    t("▸ Filtros", "▸ Filters")
                })
                .clicked()
            {
                self.advanced_open = !self.advanced_open;
            }
            if ui
                .small_button(t("↺ Limpiar", "↺ Reset"))
                .on_hover_text(t(
                    "Restablecer todos los filtros de búsqueda",
                    "Reset all search filters",
                ))
                .clicked()
            {
                self.reset_filters();
            }
        });

        // Scope indicator if narrowed to a folder
        if let Some(sf) = self.scope_folder {
            if sf != NodeId::ROOT && sf.index() < tree.len() {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "📂 {}: {}",
                            t("Ámbito", "Scope"),
                            tree.resolve_path(sf).to_string_lossy()
                        ))
                        .small()
                        .color(Color32::from_rgb(130, 195, 240)),
                    );
                    if ui.small_button(t("✕ Buscar en todo", "✕ Search all")).clicked() {
                        self.scope_folder = None;
                        trigger_search = true;
                    }
                });
            }
        }

        // 2. Advanced Search Filter Panel
        if self.advanced_open {
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    // Row A: 9 File Categories
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(t("Categorías:", "Categories:"));
                        for cat in FileCategory::ALL {
                            let active = self.selected_categories.contains(&cat);
                            let [r, g, b] = cat.accent_rgb();
                            let lbl = format!("{} {}", cat.icon(), cat.short_label());
                            let btn = if active {
                                egui::Button::new(RichText::new(lbl).strong().color(Color32::WHITE))
                                    .fill(Color32::from_rgba_unmultiplied(r, g, b, 155))
                                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(r, g, b)))
                            } else {
                                egui::Button::new(RichText::new(lbl).small())
                            };
                            if ui.add(btn).clicked() {
                                if active {
                                    self.selected_categories.retain(|&c| c != cat);
                                } else {
                                    self.selected_categories.push(cat);
                                }
                                trigger_search = true;
                            }
                        }
                    });

                    // Row B: Multi-extension include/exclude + Quick-Picker from Top 25 extensions in scan
                    ui.horizontal_wrapped(|ui| {
                        ui.label(t("Incluir ext.:", "Include ext.:"));
                        let inc_te = ui.add(
                            egui::TextEdit::singleline(&mut self.include_exts_input)
                                .hint_text("bmp, tif, cr2, mov, dwg")
                                .desired_width(180.0),
                        );
                        if inc_te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            trigger_search = true;
                        }

                        ui.menu_button(t("+ Del escaneo ▾", "+ From scan ▾"), |ui| {
                            let mut exts: Vec<_> = tree
                                .extensions
                                .entries()
                                .iter()
                                .filter(|e| e.file_count > 0 && !e.ext.is_empty())
                                .collect();
                            exts.sort_unstable_by_key(|e| Reverse(e.total_logical_bytes));
                            exts.truncate(25);
                            let current_set = parse_ext_set(&self.include_exts_input);
                            egui::ScrollArea::vertical()
                                .max_height(260.0)
                                .show(ui, |ui| {
                                    for e in exts {
                                        let checked = current_set.contains(&e.ext);
                                        let text = format!(
                                            ".{}  ({}, {} {})",
                                            e.ext,
                                            format_bytes(e.total_logical_bytes),
                                            format_count(e.file_count),
                                            t("fich.", "files")
                                        );
                                        if ui.selectable_label(checked, text).clicked() {
                                            toggle_ext_in_csv(&mut self.include_exts_input, &e.ext);
                                            trigger_search = true;
                                        }
                                    }
                                });
                        });

                        ui.separator();
                        ui.label(t("Excluir ext.:", "Exclude ext.:"));
                        let exc_te = ui.add(
                            egui::TextEdit::singleline(&mut self.exclude_exts_input)
                                .hint_text("tmp, log, dll")
                                .desired_width(135.0),
                        );
                        if exc_te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            trigger_search = true;
                        }

                        ui.separator();
                        if let Some(sel) = selection.primary().filter(|&n| n.index() < tree.len()) {
                            let dir_target = if tree.is_dir(sel) {
                                sel
                            } else {
                                tree.parents[sel.index()]
                            };
                            if dir_target != NodeId::ROOT
                                && dir_target.is_some()
                                && ui
                                    .small_button(t(
                                        "📂 Limitar a carpeta sel.",
                                        "📂 Scope to sel. folder",
                                    ))
                                    .clicked()
                            {
                                self.scope_folder = Some(dir_target);
                                trigger_search = true;
                            }
                        }
                    });

                    // Row C: Logical & Physical Size with Units (B / KB / MB / GB / TB)
                    ui.horizontal_wrapped(|ui| {
                        ui.label(t("Lógico mín:", "Logical min:"));
                        ui.add(
                            egui::DragValue::new(&mut self.min_logical_val)
                                .speed(1.0)
                                .range(0.0..=1_000_000.0),
                        );
                        size_unit_combo(ui, "s_min_log_u", &mut self.min_logical_unit);
                        ui.label(t("máx:", "max:"));
                        ui.add(
                            egui::DragValue::new(&mut self.max_logical_val)
                                .speed(1.0)
                                .range(0.0..=1_000_000.0),
                        );
                        size_unit_combo(ui, "s_max_log_u", &mut self.max_logical_unit);

                        ui.separator();
                        ui.label(t("En Disco mín:", "On Disk min:"));
                        ui.add(
                            egui::DragValue::new(&mut self.min_alloc_val)
                                .speed(1.0)
                                .range(0.0..=1_000_000.0),
                        );
                        size_unit_combo(ui, "s_min_al_u", &mut self.min_alloc_unit);
                        ui.label(t("máx:", "max:"));
                        ui.add(
                            egui::DragValue::new(&mut self.max_alloc_val)
                                .speed(1.0)
                                .range(0.0..=1_000_000.0),
                        );
                        size_unit_combo(ui, "s_max_al_u", &mut self.max_alloc_unit);
                    });

                    // Row D: Age, Owner & Diagnostic / Attribute Flags
                    ui.horizontal_wrapped(|ui| {
                        ui.label(t("Más viejo >", "Older than >"));
                        ui.add(
                            egui::DragValue::new(&mut self.older_than_days)
                                .speed(5)
                                .range(0..=20_000)
                                .suffix(" d"),
                        );
                        ui.label(t("Más nuevo <", "Newer than <"));
                        ui.add(
                            egui::DragValue::new(&mut self.newer_than_days)
                                .speed(5)
                                .range(0..=20_000)
                                .suffix(" d"),
                        );
                        ui.label(t("Propietario:", "Owner:"));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.owner_query)
                                .hint_text(t("DOMINIO\\user", "DOMAIN\\user"))
                                .desired_width(110.0),
                        );
                        ui.separator();
                        ui.checkbox(&mut self.only_duplicates, t("🧬 Duplicados", "🧬 Duplicates"));
                        ui.checkbox(&mut self.only_hardlinks, "🔗 Hardlinks");
                        ui.checkbox(&mut self.only_compressed, "🗜 NTFS");
                        ui.checkbox(&mut self.only_offline_cloud, "☁ Cloud");
                        ui.checkbox(&mut self.only_hidden, t("Ocultos", "Hidden"));
                        ui.checkbox(&mut self.only_system, t("Sistema", "System"));
                        ui.checkbox(&mut self.only_readonly, t("Solo Lect.", "Read-Only"));
                        ui.checkbox(&mut self.only_long_path, ">260");
                    });
                });
        }

        if trigger_search {
            self.execute_search(tree, settings);
        }

        if let Some(err) = &self.regex_error {
            ui.label(RichText::new(err).color(Color32::from_rgb(240, 80, 80)));
        }

        // 3. Rich Summary Totals Bar
        let scan_total_logical = tree.logical_sizes.first().copied().unwrap_or(1).max(1);
        let pct_of_scan = (self.total_logical_bytes as f64 / scan_total_logical as f64) * 100.0;
        ui.horizontal_wrapped(|ui| {
            let cap_note = if self.total_matches_found as usize > self.results.len() {
                format!(
                    " ({} {})",
                    t("mostrando top", "showing top"),
                    format_count(self.results.len() as u64)
                )
            } else {
                String::new()
            };
            ui.label(
                RichText::new(format!(
                    "{}: {}{} ({} {}, {} {})  ·  {}: {} ({pct_of_scan:.2}%)  ·  {}: {}",
                    t("Coincidencias", "Matches"),
                    format_count(self.total_matches_found),
                    cap_note,
                    format_count(self.total_files_matched),
                    t("fich.", "files"),
                    format_count(self.total_dirs_matched),
                    t("carp.", "dirs"),
                    t("Lógico", "Logical"),
                    format_bytes(self.total_logical_bytes),
                    t("En Disco", "On Disk"),
                    format_bytes(self.total_allocated_bytes),
                ))
                .small()
                .strong()
                .color(Color32::from_rgb(155, 205, 245)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !self.results.is_empty(),
                        egui::Button::new(t("📤 Exportar resultados...", "📤 Export results...")),
                    )
                    .clicked()
                {
                    action = Some(NodeAction::ExportNodes(self.results.clone()));
                }
                if !self.results.is_empty()
                    && ui
                        .small_button(t("☑ Selec. todo", "☑ Select all"))
                        .on_hover_text(t(
                            "Seleccionar todos los resultados visibles (Ctrl+A)",
                            "Select all visible search results (Ctrl+A)",
                        ))
                        .clicked()
                {
                    master_toggle = Some(true);
                }
            });
        });
        ui.separator();

        // 4. 10-Column Sortable Results Table + Multi-Select Checkbox Column
        let total_rows = self.results.len();
        let all_results_selected =
            !self.results.is_empty() && self.results.iter().all(|&n| selection.contains(n));
        let show_seconds = settings.show_timestamp_seconds;

        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(24.0)) // Select checkbox
            .column(Column::initial(175.0).at_least(100.0).clip(true))
            .column(Column::initial(76.0).at_least(56.0))
            .column(Column::initial(76.0).at_least(58.0))
            .column(Column::initial(76.0).at_least(58.0))
            .column(Column::initial(54.0).at_least(42.0))
            .column(Column::initial(120.0).at_least(78.0).clip(true))
            .column(Column::initial(104.0).at_least(80.0))
            .column(Column::initial(95.0).at_least(60.0).clip(true))
            .column(Column::initial(50.0).at_least(38.0))
            .column(Column::remainder().at_least(180.0).clip(true))
            .header(22.0, |mut h| {
                h.col(|ui| {
                    let mut all_chk = all_results_selected;
                    if ui
                        .checkbox(&mut all_chk, "")
                        .on_hover_text(t(
                            "Seleccionar / deseleccionar todos los resultados (Ctrl+A)",
                            "Select / deselect all results (Ctrl+A)",
                        ))
                        .changed()
                    {
                        master_toggle = Some(all_chk);
                    }
                });
                let mut hdr =
                    |row: &mut egui_extras::TableRow<'_, '_>, label: &str, col: SearchSortColumn| {
                        row.col(|ui| {
                            if sort_header_button(ui, label, self.sort_col == col, self.sort_desc) {
                                sort_clicked = Some(col);
                            }
                        });
                    };
                hdr(&mut h, t("Nombre", "Name"), SearchSortColumn::Name);
                hdr(&mut h, t("Tipo/Ext", "Type/Ext"), SearchSortColumn::CategoryExt);
                hdr(&mut h, t("Lógico", "Logical"), SearchSortColumn::LogicalSize);
                hdr(&mut h, t("En Disco", "On Disk"), SearchSortColumn::AllocatedSize);
                hdr(&mut h, "%", SearchSortColumn::PctTotal);
                hdr(&mut h, t("Etiquetas", "Flags"), SearchSortColumn::Flags);
                hdr(&mut h, t("Modificado", "Modified"), SearchSortColumn::Modified);
                hdr(&mut h, t("Propietario", "Owner"), SearchSortColumn::Owner);
                hdr(&mut h, t("Atrib.", "Attrs"), SearchSortColumn::Attributes);
                hdr(&mut h, t("Ruta completa", "Full Path"), SearchSortColumn::FullPath);
            })
            .body(|body| {
                body.rows(22.0, total_rows, |mut row| {
                    let nid = self.results[row.index()];
                    let idx = nid.index();
                    let is_dir = tree.is_dir(nid);
                    let is_sel = selection.contains(nid);
                    row.set_selected(is_sel);
                    let path = tree.resolve_path(nid);

                    // Checkbox column
                    row.col(|ui| {
                        let mut chk = is_sel;
                        if ui.checkbox(&mut chk, "").changed() {
                            pending_check = Some((nid, chk));
                        }
                    });

                    // 0: Name + unified single/multi context menu
                    row.col(|ui| {
                        let icon = if is_dir { "📁" } else { "📄" };
                        let r = ui.selectable_label(is_sel, format!("{icon} {}", tree.name(nid)));
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
                                    visible_nodes: &self.results,
                                    allow_tree_ops: false,
                                }),
                                settings,
                                read_only_locked,
                            ) {
                                action = Some(act);
                            }
                        });
                    });

                    // 1: Category + Ext
                    row.col(|ui| {
                        if is_dir {
                            ui.label(RichText::new(t("📁 Carpeta", "📁 Folder")).small().weak());
                        } else {
                            let ext = tree.ext(nid);
                            let cat = FileCategory::from_extension(ext);
                            let [r, g, b] = cat.accent_rgb();
                            let txt = if ext.is_empty() {
                                format!("{} —", cat.icon())
                            } else {
                                format!("{} .{ext}", cat.icon())
                            };
                            ui.label(RichText::new(txt).small().color(Color32::from_rgb(r, g, b)))
                                .on_hover_text(cat.label());
                        }
                    });

                    // 2: Logical Size
                    row.col(|ui| {
                        ui.label(format_bytes(tree.logical_sizes[idx]));
                    });

                    // 3: Allocated Size
                    row.col(|ui| {
                        if !is_dir && tree.flags[idx].contains(NodeFlags::IS_HARDLINK_DUP) {
                            ui.label(
                                RichText::new("0 B (hl)")
                                    .small()
                                    .color(Color32::from_rgb(200, 160, 110)),
                            );
                        } else {
                            ui.label(format_bytes(tree.allocated_sizes[idx]));
                        }
                    });

                    // 4: % of total scan
                    row.col(|ui| {
                        let pct = (tree.logical_sizes[idx] as f64 / scan_total_logical as f64)
                            * 100.0;
                        ui.label(RichText::new(format!("{pct:.2}%")).small());
                    });

                    // 5: Flags / Diagnostic badges
                    row.col(|ui| {
                        let f = tree.flags[idx];
                        let a = tree.win_attrs[idx];
                        let mut tags: Vec<&'static str> = Vec::new();
                        if f.contains(NodeFlags::IS_DUPLICATE) {
                            tags.push(t("🧬 Dup", "🧬 Dup"));
                        }
                        if f.contains(NodeFlags::IS_HARDLINK_DUP) {
                            tags.push("🔗 HL");
                        }
                        if a.contains(WinAttrs::COMPRESSED) {
                            tags.push("🗜 NTFS");
                        }
                        if a.contains(WinAttrs::OFFLINE_CLOUD) {
                            tags.push("☁ Cloud");
                        }
                        if f.contains(NodeFlags::IS_LONG_PATH) {
                            tags.push(">260");
                        }
                        if tags.is_empty() {
                            ui.label(RichText::new("—").weak());
                        } else {
                            ui.label(
                                RichText::new(tags.join(" "))
                                    .small()
                                    .color(Color32::from_rgb(165, 205, 245)),
                            );
                        }
                    });

                    // 6: Modified
                    row.col(|ui| {
                        ui.label(format_timestamp_opts(tree.mtimes[idx], show_seconds));
                    });

                    // 7: Owner
                    row.col(|ui| {
                        let o = tree.owner(nid);
                        if o.is_empty() {
                            ui.label(RichText::new("—").weak());
                        } else {
                            ui.label(RichText::new(o).small());
                        }
                    });

                    // 8: Attributes
                    row.col(|ui| {
                        ui.label(
                            RichText::new(tree.win_attrs[idx].to_compact_string())
                                .small()
                                .monospace(),
                        );
                    });

                    // 9: Full Path
                    row.col(|ui| {
                        let pr = ui.selectable_label(
                            is_sel,
                            RichText::new(path.to_string_lossy().as_ref())
                                .small()
                                .monospace(),
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
                                    visible_nodes: &self.results,
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
                selection.select_all(&self.results);
            } else {
                selection.clear();
            }
        }
        if let Some((nid, chk)) = pending_check {
            selection.set_checked(nid, chk);
        }
        if let Some((nid, mods)) = pending_click {
            selection.handle_click(nid, mods, &self.results);
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
                    SearchSortColumn::Name
                        | SearchSortColumn::CategoryExt
                        | SearchSortColumn::Owner
                        | SearchSortColumn::Attributes
                        | SearchSortColumn::FullPath
                );
            }
            self.resort_results(tree);
        }

        action
    }
}

fn search_node_flags_rank(tree: &ScanTree, nid: NodeId) -> u8 {
    let idx = nid.index();
    let f = tree.flags[idx];
    let a = tree.win_attrs[idx];
    let mut bits = 0u8;
    if f.contains(NodeFlags::IS_DUPLICATE) {
        bits |= 1 << 4;
    }
    if f.contains(NodeFlags::IS_HARDLINK_DUP) {
        bits |= 1 << 3;
    }
    if a.contains(WinAttrs::COMPRESSED) {
        bits |= 1 << 2;
    }
    if a.contains(WinAttrs::OFFLINE_CLOUD) {
        bits |= 1 << 1;
    }
    if f.contains(NodeFlags::IS_LONG_PATH) {
        bits |= 1;
    }
    bits
}

fn size_unit_combo(ui: &mut Ui, id: &str, unit: &mut SizeUnit) {
    egui::ComboBox::from_id_salt(id)
        .width(42.0)
        .selected_text(unit.label())
        .show_ui(ui, |ui| {
            for u in SizeUnit::ALL {
                ui.selectable_value(unit, u, u.label());
            }
        });
}

fn parse_ext_set(raw: &str) -> AHashSet<String> {
    raw.split([',', ';', ' ', '\t'])
        .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

fn toggle_ext_in_csv(csv: &mut String, ext: &str) {
    let ext_low = ext.trim_start_matches('.').to_ascii_lowercase();
    let mut parts: Vec<String> = csv
        .split([',', ';', ' '])
        .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(pos) = parts.iter().position(|e| e == &ext_low) {
        parts.remove(pos);
    } else {
        parts.push(ext_low);
    }
    *csv = parts.join(", ");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn build_sample_tree() -> (ScanTree, NodeId, NodeId, NodeId, NodeId, NodeId) {
        let mut tree = ScanTree::new();
        let root = tree.add_root_target(Path::new(r"P:\Proyectos"));
        let fotos = tree.add_child(root, "Fotos", NodeFlags::IS_DIR, 0, 0, 1_700_000_000);
        let cad = tree.add_child(root, "CAD", NodeFlags::IS_DIR, 0, 0, 1_700_000_000);

        let bmp = tree.add_child_full(
            fotos,
            "RAW_SCAN.bmp",
            NodeFlags::IS_DUPLICATE,
            WinAttrs::ARCHIVE,
            50_000_000,
            50_000_000,
            0x1001,
            1_650_000_000,
        );
        let mp4 = tree.add_child_full(
            fotos,
            "drone_4k.mp4",
            NodeFlags::empty(),
            WinAttrs::ARCHIVE,
            200_000_000,
            200_000_000,
            0x1002,
            1_720_000_000,
        );
        let dwg = tree.add_child_full(
            cad,
            "planta_v1.dwg",
            NodeFlags::empty(),
            WinAttrs::READONLY | WinAttrs::COMPRESSED,
            15_000_000,
            8_000_000,
            0x1003,
            1_700_000_000,
        );
        // Hardlink twin of dwg
        let dwg_hl = tree.add_child_full(
            cad,
            "planta_link.dwg",
            NodeFlags::empty(),
            WinAttrs::ARCHIVE,
            15_000_000,
            8_000_000,
            0x1003,
            1_700_000_000,
        );
        tree.deduplicate_hardlinks();
        (tree, fotos, bmp, mp4, dwg, dwg_hl)
    }

    #[test]
    fn test_advanced_search_categories_extensions_units_and_flags() {
        let (tree, fotos, bmp, mp4, dwg, dwg_hl) = build_sample_tree();
        let mut settings = AppSettings::default();

        // 1. Category filter: CAD/BIM/3D only
        let mut s = SearchViewState {
            selected_categories: vec![FileCategory::CadBim3d],
            ..Default::default()
        };
        s.execute_search(&tree, &mut settings);
        assert_eq!(s.total_files_matched, 2);
        assert!(s.results.contains(&dwg) && s.results.contains(&dwg_hl));

        // 2. Multi-extension include + exclude
        let mut s2 = SearchViewState {
            include_exts_input: ".bmp, mp4, dwg".into(),
            exclude_exts_input: "dwg".into(),
            ..Default::default()
        };
        s2.execute_search(&tree, &mut settings);
        assert_eq!(s2.total_files_matched, 2);
        assert_eq!(s2.results, vec![mp4, bmp]); // sorted descending by logical size
        assert_eq!(s2.total_logical_bytes, 250_000_000);

        // 3. Size units (MB / GB) & physical size deduplication
        let mut s3 = SearchViewState {
            min_logical_val: 100.0,
            min_logical_unit: SizeUnit::Mb,
            ..Default::default()
        };
        s3.execute_search(&tree, &mut settings);
        assert_eq!(s3.results, vec![mp4]);

        // 4. Hardlinks + Duplicates + Scope folder
        let mut s4 = SearchViewState {
            only_hardlinks: true,
            ..Default::default()
        };
        s4.execute_search(&tree, &mut settings);
        assert_eq!(s4.results.len(), 2);

        let mut s5 = SearchViewState {
            only_duplicates: true,
            scope_folder: Some(fotos),
            ..Default::default()
        };
        s5.execute_search(&tree, &mut settings);
        assert_eq!(s5.results, vec![bmp]);

        // 5. Case-sensitive + whole phrase + history persistence
        let mut s6 = SearchViewState {
            query: "RAW_SCAN.bmp".into(),
            whole_phrase: true,
            case_sensitive: true,
            ..Default::default()
        };
        s6.execute_search(&tree, &mut settings);
        assert_eq!(s6.results, vec![bmp]);
        assert_eq!(settings.search_history.first().map(String::as_str), Some("RAW_SCAN.bmp"));
    }
}
