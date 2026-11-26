use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::{bounded, Receiver};
use egui::{Color32, Pos2, RichText, Stroke, Ui};
use mediastat_core::{
    age_color_for_palette, category_color_for_palette, extension_color_for_palette, format_bytes,
    format_count, format_timestamp_opts, free_space_color, is_light_rgb, t, unknown_space_color,
    AppSettings, CleanupRefreshPolicy, FileCategory, Language, LayoutAxis, LayoutPaneKind,
    NodeFlags, NodeId, PermissionEntry, ScanConfig, ScanTree, SizeMetric, TreemapLayoutStyle,
    TreemapPreset, VisualizationKind, VizColorMode, VizColorPalette,
};
use mediastat_scanner::{
    enumerate_drives, is_process_elevated, query_recycle_bin, rescan_directory_node,
    resolve_owners_for_nodes, resolve_to_unc_path, run_scan_pipeline, scan_permissions_for_scope,
    ScanProgress,
};
use mediastat_storage::{
    compute_diff, export_nodes_with_cost, load_csv_snapshot, load_snapshot, save_snapshot,
    ExportFormat,
};

use crate::flamegraph::show_flame_graph;
use crate::selection::{
    deduplicate_roots_slice, format_names_for_clipboard, format_paths_for_clipboard,
    format_tsv_summary_for_clipboard, format_unc_paths_for_clipboard, show_node_actions_menu,
    MenuScope, NodeAction, SelectionState,
};
use crate::shell_actions::{
    apply_ntfs_compression, delete_permanently, empty_recycle_bin, execute_user_cleanup,
    launch_windows_tool, move_item_to_folder, move_multiple_to_recycle_bin, open_appdata_dir,
    open_terminal_at, open_with_default_app, relaunch_as_administrator, remove_empty_subfolders,
    remove_motw, reveal_multiple_in_explorer, show_windows_properties, sparsify_file,
    sync_folder_dates_to_newest, NtfsCompressionAlg, ThumbnailLoader, WindowsSysTool,
};
use crate::sunburst::{show_sunburst, VizAction};
use crate::treemap::{draw_mini_cushion_rect, extension_display_color, SpecialTile, TreemapCache};
use crate::views::{bring_native_window_to_front, show_native_window, NativeWindowConfig};
use crate::views::diff_view::DiffModalState;
use crate::views::duplicates_view::show_duplicates_view;
use crate::views::inspector_panel::{RightPanelAction, RightPanelState, RightTab};
use crate::views::largest_files::LargestFilesState;
use crate::views::layout_popup::LayoutPopupState;
use crate::views::options_dialog::{OptionsDialogState, OptionsTab};
use crate::views::permissions_view::{OwnerResolveScope, PermissionsViewState, PermsAction};
use crate::views::search_view::SearchViewState;
use crate::views::storage_analytics::show_storage_analytics;
use crate::views::sysadmin_audit::SysAdminAuditState;
use crate::views::target_selector::TargetSelectorState;
use crate::views::tree_view::TreeViewState;
use crate::views::watcher_view::WatcherViewState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainTab {
    DirectoryTree,
    LargestFiles,
    Duplicates,
    Search,
    Watcher,
    Permissions,
    StorageAnalytics,
    SysAdminAudit,
}

enum BgMessage {
    ScanCompleted(ScanTree),
    RescanCompleted(ScanTree, String),
    SnapshotLoaded(ScanTree, String),
    OwnersResolved(ScanTree, usize),
    PermissionsScanned(Vec<PermissionEntry>),
    Toast(String, bool),
}

enum PendingConfirm {
    Trash(Vec<(NodeId, PathBuf)>, u64),
    DeletePermanent(Vec<(NodeId, PathBuf, bool)>, u64),
    EmptyRecycleBin(u64, u64),
    UserCleanup(usize, Vec<(NodeId, PathBuf)>),
    Sparsify(Vec<(NodeId, PathBuf)>),
    SyncFolderDates(Vec<(NodeId, PathBuf)>),
    DisableHibernation,
}

pub struct MediaStatApp {
    settings: AppSettings,
    read_only_locked: bool,
    is_elevated: bool,
    recycle_bin_stats: (u64, u64),

    tree: ScanTree,
    tree_gen: u64,
    last_scan_config: Option<ScanConfig>,
    selection: SelectionState,
    selection_child_history: Vec<NodeId>,
    hover_status_text: String,
    main_tab: MainTab,

    // Subviews & Dialogs
    target_selector: TargetSelectorState,
    startup_selector_countdown: u8,
    tree_view: TreeViewState,
    largest_files: LargestFilesState,
    search_view: SearchViewState,
    watcher_view: WatcherViewState,
    permissions_view: PermissionsViewState,
    audit_view: SysAdminAuditState,
    right_panel: RightPanelState,
    treemap: TreemapCache,
    diff_modal: DiffModalState,
    options_dialog: OptionsDialogState,
    layout_popup: LayoutPopupState,
    thumb_loader: ThumbnailLoader,
    shortcuts_open: bool,
    about_open: bool,

    // Export modal
    export_open: bool,
    export_format: ExportFormat,
    export_top_5000: bool,
    export_custom_nodes: Option<Vec<NodeId>>,

    // Confirmation modal for destructive cleanup
    pending_confirm: Option<PendingConfirm>,

    // Background job state
    scanning: bool,
    scan_started: Option<Instant>,
    progress: Arc<ScanProgress>,
    bg_rx: Option<Receiver<BgMessage>>,

    // Toast notification
    toast: Option<(String, bool, Instant)>,
}

pub fn main_window_title() -> &'static str {
    t(
        "MediaStat v0.3 — Analizador de Espacio en Disco estilo WinDirStat · Búsqueda Avanzada & SysAdmin",
        "MediaStat v0.3 — WinDirStat-Style Disk Space Analyzer · Advanced Search & SysAdmin",
    )
}

impl MediaStatApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = AppSettings::load_or_default();
        settings.sync_active_language();
        apply_visual_theme(&cc.egui_ctx, settings.dark_mode, settings.ui_scale_percent);
        let drives = enumerate_drives();
        let read_only_locked = settings.read_only_lock_by_default;
        let is_elevated = is_process_elevated();
        let recycle_bin_stats = query_recycle_bin();
        let mut target_selector = TargetSelectorState::new(drives, &settings);
        // Defer opening the OS viewport until after ROOT finishes its initial visible frame
        // so Windows never activates ROOT on top of the Target Selector.
        target_selector.open = false;

        Self {
            settings,
            read_only_locked,
            is_elevated,
            recycle_bin_stats,
            tree: ScanTree::new(),
            tree_gen: 0,
            last_scan_config: None,
            selection: SelectionState::default(),
            selection_child_history: Vec::new(),
            hover_status_text: String::new(),
            main_tab: MainTab::DirectoryTree,
            target_selector,
            startup_selector_countdown: 3,
            tree_view: TreeViewState::default(),
            largest_files: LargestFilesState::default(),
            search_view: SearchViewState::default(),
            watcher_view: WatcherViewState::default(),
            permissions_view: PermissionsViewState::default(),
            audit_view: SysAdminAuditState::default(),
            right_panel: RightPanelState::default(),
            treemap: TreemapCache::default(),
            diff_modal: DiffModalState::default(),
            options_dialog: OptionsDialogState::default(),
            layout_popup: LayoutPopupState::default(),
            thumb_loader: ThumbnailLoader::new(cc.egui_ctx.clone()),
            shortcuts_open: false,
            about_open: false,
            export_open: false,
            export_format: ExportFormat::Csv,
            export_top_5000: true,
            export_custom_nodes: None,
            pending_confirm: None,
            scanning: false,
            scan_started: None,
            progress: Arc::new(ScanProgress::new()),
            bg_rx: None,
            toast: None,
        }
    }

    fn set_toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some((msg.into(), is_error, Instant::now()));
    }

    fn on_language_changed(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(
            main_window_title().to_string(),
        ));
        let _ = self.settings.save();
        self.set_toast(
            t("Idioma cambiado a Español", "Language switched to English"),
            false,
        );
    }

    fn invalidate_all_views(&mut self) {
        self.tree_gen = self.tree_gen.wrapping_add(1);
        self.selection.prune_invalid(&self.tree);
        self.treemap.invalidate();
        self.tree_view.dirty = true;
        self.largest_files.invalidate();
    }

    fn install_new_tree(&mut self, tree: ScanTree) {
        self.tree = tree;
        self.tree_gen = self.tree_gen.wrapping_add(1);
        self.selection.clear();
        self.selection_child_history.clear();
        self.treemap.root_node = NodeId::ROOT;
        self.treemap.invalidate();
        self.tree_view.reset_for_new_tree(&self.tree, self.tree_gen);
        self.largest_files.invalidate();
        self.right_panel.highlighted_ext_id = None;
        self.thumb_loader.clear();
        self.recycle_bin_stats = query_recycle_bin();
    }

    fn select_node_and_sync(&mut self, nid: NodeId, open_inspector: bool) {
        self.selection.select_single(nid);
        self.tree_view.reveal_and_scroll(&self.tree, nid);
        if open_inspector {
            self.right_panel.active_tab = RightTab::Inspector;
        }
    }

    fn current_tab_visible_nodes(&self) -> Vec<NodeId> {
        match self.main_tab {
            MainTab::DirectoryTree => self.tree_view.visible_nodes.clone(),
            MainTab::LargestFiles => self.largest_files.cached_nodes.clone(),
            MainTab::Duplicates => self
                .tree
                .duplicates
                .iter()
                .take(500)
                .flat_map(|g| g.nodes.iter().copied())
                .filter(|&n| !self.tree.is_deleted(n))
                .collect(),
            MainTab::Search => self.search_view.results.clone(),
            MainTab::Permissions => self
                .tree
                .permissions
                .iter()
                .map(|p| p.node_id)
                .filter(|&n| n.is_some() && n != NodeId::ROOT && n.index() < self.tree.len())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// WinDirStat `View -> Select Parent` (`Ctrl+Up`).
    fn select_parent_node(&mut self) {
        let Some(cur) = self.selection.primary().filter(|&n| n.index() < self.tree.len()) else {
            return;
        };
        let parent = self.tree.parents[cur.index()];
        if parent.is_some() && parent != NodeId::ROOT {
            self.selection_child_history.push(cur);
            self.select_node_and_sync(parent, false);
        }
    }

    /// WinDirStat `View -> Reselect Child` (`Ctrl+Down`).
    fn reselect_child_node(&mut self) {
        while let Some(child) = self.selection_child_history.pop() {
            if child.index() < self.tree.len() && !self.tree.is_deleted(child) {
                self.select_node_and_sync(child, false);
                return;
            }
        }
    }

    /// WinDirStat `Ctrl+Left` (first subdir) / `Ctrl+Right` (first file) under selected directory.
    fn select_first_child_kind(&mut self, want_dir: bool) {
        let Some(cur) = self.selection.primary().filter(|&n| n.index() < self.tree.len()) else {
            return;
        };
        let dir = if self.tree.is_dir(cur) {
            cur
        } else {
            self.tree.parents[cur.index()]
        };
        if dir.is_none() || dir.index() >= self.tree.len() {
            return;
        }
        let best = self
            .tree
            .children(dir)
            .filter(|&c| !self.tree.is_deleted(c) && self.tree.is_dir(c) == want_dir)
            .max_by_key(|&c| self.tree.size_for_metric(c, self.settings.size_metric));
        if let Some(target) = best {
            self.select_node_and_sync(target, false);
        }
    }

    fn start_scan(&mut self, config: ScanConfig, ctx: egui::Context) {
        let progress = Arc::new(ScanProgress::new());
        let prog_clone = Arc::clone(&progress);
        let (tx, rx) = bounded::<BgMessage>(4);

        self.last_scan_config = Some(config.clone());
        self.progress = progress;
        self.bg_rx = Some(rx);
        self.scanning = true;
        self.scan_started = Some(Instant::now());
        self.selection.clear();

        std::thread::spawn(move || {
            let tree = run_scan_pipeline(&config, &prog_clone);
            let _ = tx.send(BgMessage::ScanCompleted(tree));
            ctx.request_repaint();
        });
    }

    /// WinDirStat `F5` Selective Rescan of a single folder in background without re-scanning whole volume!
    fn start_rescan_subtree(&mut self, dir_id: NodeId, ctx: egui::Context) {
        if self.scanning || dir_id == NodeId::ROOT || dir_id.index() >= self.tree.len() {
            return;
        }
        let target_dir = if self.tree.is_dir(dir_id) {
            dir_id
        } else {
            self.tree.parents[dir_id.index()]
        };
        if target_dir == NodeId::ROOT || target_dir.is_none() {
            return;
        }
        let dir_label = self.tree.resolve_path(target_dir).to_string_lossy().into_owned();
        let cfg = self.last_scan_config.clone().unwrap_or_else(|| ScanConfig {
            filter_config: self.settings.filter_config.clone(),
            ..Default::default()
        });
        let mut tree_clone = self.tree.clone();
        let progress = Arc::new(ScanProgress::new());
        let prog_clone = Arc::clone(&progress);
        let (tx, rx) = bounded::<BgMessage>(2);

        self.progress = progress;
        self.bg_rx = Some(rx);
        self.scanning = true;
        self.scan_started = Some(Instant::now());

        std::thread::spawn(move || {
            rescan_directory_node(&mut tree_clone, target_dir, &cfg, &prog_clone);
            let _ = tx.send(BgMessage::RescanCompleted(tree_clone, dir_label));
            ctx.request_repaint();
        });
    }

    fn start_resolve_owners(&mut self, scope: OwnerResolveScope, ctx: egui::Context) {
        if self.scanning || self.permissions_view.running_task || self.tree.is_empty() {
            return;
        }
        let nodes: Vec<NodeId> = match scope {
            OwnerResolveScope::TopLargest1000 => {
                self.tree.largest_files(1_000, self.settings.size_metric)
            }
            OwnerResolveScope::SelectedFolder => {
                let root = self
                    .selection
                    .primary()
                    .filter(|&n| n.index() < self.tree.len())
                    .map(|n| {
                        if self.tree.is_dir(n) {
                            n
                        } else {
                            self.tree.parents[n.index()]
                        }
                    })
                    .unwrap_or(NodeId::ROOT);
                collect_subtree_files(&self.tree, root)
            }
            OwnerResolveScope::EntireTree => (1..self.tree.len())
                .filter(|&i| !self.tree.flags[i].contains(NodeFlags::IS_DELETED))
                .map(|i| NodeId(i as u32))
                .collect(),
        };
        if nodes.is_empty() {
            self.set_toast(
                t(
                    "No hay archivos en ese ámbito para resolver propietarios.",
                    "No files in that scope to resolve owners for.",
                ),
                false,
            );
            return;
        }

        let mut tree_clone = self.tree.clone();
        let (tx, rx) = bounded::<BgMessage>(2);
        self.bg_rx = Some(rx);
        self.permissions_view.running_task = true;

        std::thread::spawn(move || {
            let cancel = Arc::new(AtomicBool::new(false));
            let resolved = resolve_owners_for_nodes(&mut tree_clone, &nodes, None, &cancel);
            let _ = tx.send(BgMessage::OwnersResolved(tree_clone, resolved));
            ctx.request_repaint();
        });
    }

    fn start_scan_permissions(&mut self, include_inherited: bool, ctx: egui::Context) {
        if self.scanning || self.permissions_view.running_task || self.tree.is_empty() {
            return;
        }
        let scope = self
            .selection
            .primary()
            .filter(|&n| n.index() < self.tree.len() && self.tree.is_dir(n))
            .unwrap_or(NodeId::ROOT);
        let tree_clone = self.tree.clone();
        let (tx, rx) = bounded::<BgMessage>(2);
        self.bg_rx = Some(rx);
        self.permissions_view.running_task = true;

        std::thread::spawn(move || {
            let cancel = Arc::new(AtomicBool::new(false));
            let perms =
                scan_permissions_for_scope(&tree_clone, scope, include_inherited, 20_000, &cancel);
            let _ = tx.send(BgMessage::PermissionsScanned(perms));
            ctx.request_repaint();
        });
    }

    fn load_snapshot_dialog(&mut self, ctx: egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title(t(
                "Abrir Snapshot MediaStat (.mstat) o CSV",
                "Open MediaStat Snapshot (.mstat) or CSV",
            ))
            .add_filter("MediaStat / CSV", &["mstat", "csv"])
            .pick_file()
        {
            let (tx, rx) = bounded::<BgMessage>(2);
            self.bg_rx = Some(rx);
            self.set_toast(
                format!("{} {}...", t("Cargando", "Loading"), path.display()),
                false,
            );
            let err_prefix = t("Error abriendo archivo", "Error opening file");
            std::thread::spawn(move || {
                let is_csv = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("csv"));
                let res = if is_csv {
                    load_csv_snapshot(&path)
                } else {
                    load_snapshot(&path)
                };
                match res {
                    Ok(tree) => {
                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        let _ = tx.send(BgMessage::SnapshotLoaded(tree, name));
                    }
                    Err(e) => {
                        let _ = tx.send(BgMessage::Toast(format!("{err_prefix}: {e}"), true));
                    }
                }
                ctx.request_repaint();
            });
        }
    }

    fn save_snapshot_dialog(&mut self) {
        if self.tree.is_empty() {
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .set_title(t(
                "Guardar Snapshot MediaStat (.mstat)",
                "Save MediaStat Snapshot (.mstat)",
            ))
            .set_file_name(t("auditoria_filestorage.mstat", "filestorage_audit.mstat"))
            .add_filter("MediaStat Snapshot", &["mstat"])
            .save_file()
        {
            match save_snapshot(&self.tree, &path) {
                Ok(()) => self.set_toast(
                    format!("{} {}", t("Snapshot guardado en", "Snapshot saved to"), path.display()),
                    false,
                ),
                Err(e) => self.set_toast(
                    format!("{}: {e}", t("Error guardando snapshot", "Error saving snapshot")),
                    true,
                ),
            }
        }
    }

    fn open_diff_dialog(&mut self) {
        let Some(before_path) = rfd::FileDialog::new()
            .set_title(t(
                "Seleccionar Snapshot ANTES (.mstat)",
                "Select BEFORE Snapshot (.mstat)",
            ))
            .add_filter("MediaStat Snapshot", &["mstat"])
            .pick_file()
        else {
            return;
        };
        let before_tree = match load_snapshot(&before_path) {
            Ok(t) => t,
            Err(e) => {
                self.set_toast(
                    format!("{}: {e}", t("Error cargando ANTES", "Error loading BEFORE")),
                    true,
                );
                return;
            }
        };
        if !self.tree.is_empty() {
            let diff = compute_diff(&before_tree, &self.tree);
            self.diff_modal.before_label =
                before_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            self.diff_modal.after_label =
                t("Análisis actual en pantalla", "Current on-screen scan").into();
            self.diff_modal.diff = Some(diff);
            self.diff_modal.open = true;
        } else if let Some(after_path) = rfd::FileDialog::new()
            .set_title(t(
                "Seleccionar Snapshot DESPUÉS (.mstat)",
                "Select AFTER Snapshot (.mstat)",
            ))
            .add_filter("MediaStat Snapshot", &["mstat"])
            .pick_file()
        {
            match load_snapshot(&after_path) {
                Ok(t) => {
                    let diff = compute_diff(&before_tree, &t);
                    self.diff_modal.before_label =
                        before_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    self.diff_modal.after_label =
                        after_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    self.diff_modal.diff = Some(diff);
                    self.diff_modal.open = true;
                }
                Err(e) => self.set_toast(
                    format!("{}: {e}", t("Error cargando DESPUÉS", "Error loading AFTER")),
                    true,
                ),
            }
        }
    }

    fn request_trash_nodes(&mut self, nodes: &[NodeId]) {
        if self.read_only_locked {
            self.set_toast(
                t(
                    "🔒 Desbloquea el Modo Solo Lectura en la barra superior para limpiar archivos.",
                    "🔒 Unlock Read-Only Mode in the top bar to clean up files.",
                ),
                true,
            );
            return;
        }
        let roots = deduplicate_roots_slice(nodes, &self.tree);
        if roots.is_empty() {
            return;
        }
        let total_sz: u64 = roots
            .iter()
            .map(|&n| self.tree.logical_sizes[n.index()])
            .fold(0u64, u64::saturating_add);
        let pairs: Vec<(NodeId, PathBuf)> = roots
            .into_iter()
            .map(|n| (n, self.tree.resolve_path(n)))
            .collect();
        self.pending_confirm = Some(PendingConfirm::Trash(pairs, total_sz));
    }

    fn request_delete_permanent_nodes(&mut self, nodes: &[NodeId]) {
        if self.read_only_locked {
            self.set_toast(
                t(
                    "🔒 Desbloquea el Modo Solo Lectura en la barra superior para eliminar archivos.",
                    "🔒 Unlock Read-Only Mode in the top bar to delete files.",
                ),
                true,
            );
            return;
        }
        let roots = deduplicate_roots_slice(nodes, &self.tree);
        if roots.is_empty() {
            return;
        }
        let total_sz: u64 = roots
            .iter()
            .map(|&n| self.tree.logical_sizes[n.index()])
            .fold(0u64, u64::saturating_add);
        let entries: Vec<(NodeId, PathBuf, bool)> = roots
            .into_iter()
            .map(|n| (n, self.tree.resolve_path(n), self.tree.is_dir(n)))
            .collect();
        self.pending_confirm = Some(PendingConfirm::DeletePermanent(entries, total_sz));
    }

    fn trigger_user_cleanup_batch(&mut self, slot: usize, nodes: &[NodeId], ctx: &egui::Context) {
        let Some(act) = self.settings.user_cleanups.get(slot).cloned() else {
            return;
        };
        if !act.enabled || act.command_line.trim().is_empty() {
            return;
        }
        if self.read_only_locked && !act.read_only_safe {
            self.set_toast(
                format!(
                    "🔒 '{}': {}",
                    act.title,
                    t(
                        "modifica archivos — desbloquea el Modo Solo Lectura en la barra superior.",
                        "modifies files — unlock Read-Only Mode in the top bar."
                    )
                ),
                true,
            );
            return;
        }
        let matching: Vec<(NodeId, PathBuf)> = nodes
            .iter()
            .copied()
            .filter(|&nid| {
                if nid.is_none() || nid == NodeId::ROOT || nid.index() >= self.tree.len() || self.tree.is_deleted(nid) {
                    return false;
                }
                let is_dir = self.tree.is_dir(nid);
                (is_dir && act.works_for_dirs) || (!is_dir && act.works_for_files)
            })
            .map(|nid| (nid, self.tree.resolve_path(nid)))
            .collect();
        if matching.is_empty() {
            self.set_toast(
                format!(
                    "'{}' {} {}.",
                    act.title,
                    t("solo aplica a", "applies only to"),
                    if act.works_for_dirs {
                        t("carpetas", "folders")
                    } else {
                        t("ficheros", "files")
                    }
                ),
                true,
            );
            return;
        }
        if act.ask_confirmation || matching.len() > 1 {
            self.pending_confirm = Some(PendingConfirm::UserCleanup(slot, matching));
        } else {
            let (nid, path) = &matching[0];
            match execute_user_cleanup(&act, path) {
                Ok(()) => {
                    self.set_toast(
                        format!("{} '{}'", t("Ejecutado", "Executed"), act.title),
                        false,
                    );
                    self.apply_cleanup_refresh(act.refresh_policy, *nid, ctx);
                }
                Err(e) => self.set_toast(
                    format!("{}: {e}", t("Error ejecutando acción", "Error executing action")),
                    true,
                ),
            }
        }
    }

    /// Unified dispatcher for ALL single-item and multi-selection actions across all views,
    /// menus, toolbar, Treemap, Selection Action Bar, and Inspector.
    fn handle_node_action(&mut self, action: NodeAction, ctx: &egui::Context) {
        match action {
            NodeAction::SelectAllVisible(nodes) => {
                self.selection.select_all(&nodes);
            }
            NodeAction::InvertVisible(nodes) => {
                self.selection.invert_selection(&nodes);
            }
            NodeAction::ClearSelection => {
                self.selection.clear();
            }
            NodeAction::CopyPaths(nodes) => {
                let txt = format_paths_for_clipboard(&nodes, &self.tree);
                if !txt.is_empty() {
                    ctx.copy_text(txt);
                    let n = nodes.len();
                    let msg = if n > 1 {
                        format!(
                            "{} {}",
                            format_count(n as u64),
                            t("rutas copiadas al portapapeles", "paths copied to clipboard")
                        )
                    } else {
                        t("Ruta copiada al portapapeles", "Path copied to clipboard").to_string()
                    };
                    self.set_toast(msg, false);
                }
            }
            NodeAction::CopyNames(nodes) => {
                let txt = format_names_for_clipboard(&nodes, &self.tree);
                if !txt.is_empty() {
                    ctx.copy_text(txt);
                    let n = nodes.len();
                    let msg = if n > 1 {
                        format!(
                            "{} {}",
                            format_count(n as u64),
                            t("nombres copiados al portapapeles", "names copied to clipboard")
                        )
                    } else {
                        t("Nombre copiado al portapapeles", "Name copied to clipboard").to_string()
                    };
                    self.set_toast(msg, false);
                }
            }
            NodeAction::CopyUncPaths(nodes) => {
                if let Some(txt) = format_unc_paths_for_clipboard(&nodes, &self.tree) {
                    ctx.copy_text(txt);
                    let n = nodes.len();
                    let msg = if n > 1 {
                        format!(
                            "{} {}",
                            format_count(n as u64),
                            t("rutas UNC copiadas", "UNC paths copied")
                        )
                    } else {
                        t("Ruta UNC copiada al portapapeles", "UNC path copied to clipboard")
                            .to_string()
                    };
                    self.set_toast(msg, false);
                }
            }
            NodeAction::CopyTsv(nodes) => {
                let txt = format_tsv_summary_for_clipboard(
                    &nodes,
                    &self.tree,
                    self.settings.show_timestamp_seconds,
                );
                ctx.copy_text(txt);
                self.set_toast(
                    format!(
                        "{} ({} {})",
                        t("Resumen TSV copiado (pega en Excel)", "TSV summary copied (paste into Excel)"),
                        format_count(nodes.len() as u64),
                        t("elem.", "items")
                    ),
                    false,
                );
            }
            NodeAction::RevealInExplorer(nodes) => {
                let paths: Vec<PathBuf> = nodes
                    .iter()
                    .copied()
                    .filter(|&n| n.index() < self.tree.len())
                    .map(|n| self.tree.resolve_path(n))
                    .collect();
                let (opened, skipped) = reveal_multiple_in_explorer(&paths, 5);
                if skipped > 0 {
                    self.set_toast(
                        format!(
                            "{} {opened} {} ({skipped} {})",
                            t("Abiertas", "Opened"),
                            t("ventanas del Explorador", "Explorer windows"),
                            t("carpetas adicionales omitidas", "additional folders skipped")
                        ),
                        false,
                    );
                }
            }
            NodeAction::OpenFiles(nodes) => {
                for nid in nodes.into_iter().take(10) {
                    if nid.index() < self.tree.len() && !self.tree.is_dir(nid) {
                        open_with_default_app(&self.tree.resolve_path(nid));
                    }
                }
            }
            NodeAction::OpenPowerShell(nid, as_admin) => {
                if nid.index() < self.tree.len() {
                    open_terminal_at(&self.tree.resolve_path(nid), true, as_admin);
                }
            }
            NodeAction::OpenCmd(nid) => {
                if nid.index() < self.tree.len() {
                    open_terminal_at(&self.tree.resolve_path(nid), false, false);
                }
            }
            NodeAction::ShowProperties(nid) => {
                if nid.index() < self.tree.len() {
                    show_windows_properties(&self.tree.resolve_path(nid));
                }
            }
            NodeAction::ComputeHashes(nid) => {
                if nid.index() < self.tree.len() && !self.tree.is_dir(nid) {
                    self.selection.select_single(nid);
                    self.right_panel.active_tab = RightTab::Inspector;
                    self.settings.show_extensions_panel = true;
                    self.right_panel
                        .hash_state
                        .start(self.tree.resolve_path(nid), ctx.clone());
                }
            }
            NodeAction::ZoomTreemap(nid) => {
                if nid.index() < self.tree.len() {
                    self.treemap.root_node = nid;
                    self.treemap.invalidate();
                    if self.selection.len() <= 1 {
                        self.selection.select_single(nid);
                    }
                }
            }
            NodeAction::SearchInFolder(nid) => {
                self.main_tab = MainTab::Search;
                self.search_view
                    .search_in_folder(nid, &self.tree, &mut self.settings);
            }
            NodeAction::RefreshSelected(nid) => {
                self.start_rescan_subtree(nid, ctx.clone());
            }
            NodeAction::ExportNodes(nodes) => {
                self.export_custom_nodes = Some(nodes);
                self.export_open = true;
            }
            NodeAction::ExcludeNodes(nodes) => {
                let mut added = 0usize;
                for nid in nodes {
                    if nid.index() >= self.tree.len() {
                        continue;
                    }
                    let name = self.tree.name(nid).to_string();
                    let target_str = if self.tree.is_dir(nid) {
                        &mut self.settings.filter_config.exclude_dirs
                    } else {
                        &mut self.settings.filter_config.exclude_files
                    };
                    if !target_str.lines().any(|l| l.trim().eq_ignore_ascii_case(&name)) {
                        if !target_str.is_empty() {
                            target_str.push('\n');
                        }
                        target_str.push_str(&name);
                        added += 1;
                    }
                }
                if added > 0 {
                    self.set_toast(
                        format!(
                            "{added} {}",
                            t(
                                "elemento(s) añadidos al filtro de exclusión (aplica en próximo escaneo/F5)",
                                "item(s) added to exclusion filter (applies on next scan/F5)"
                            )
                        ),
                        false,
                    );
                }
            }
            NodeAction::MoveNodes(nodes) => {
                if self.read_only_locked {
                    self.set_toast(
                        t(
                            "🔒 Desbloquea el Modo Solo Lectura en la barra superior para mover elementos.",
                            "🔒 Unlock Read-Only Mode in the top bar to move items.",
                        ),
                        true,
                    );
                    return;
                }
                let roots = deduplicate_roots_slice(&nodes, &self.tree);
                if roots.is_empty() {
                    return;
                }
                if let Some(dst_dir) = rfd::FileDialog::new().pick_folder() {
                    let mut moved = 0usize;
                    let mut last_err: Option<String> = None;
                    for nid in roots {
                        let src = self.tree.resolve_path(nid);
                        match move_item_to_folder(&src, &dst_dir) {
                            Ok(_) => {
                                self.tree.mark_node_deleted(nid);
                                moved += 1;
                            }
                            Err(e) => last_err = Some(e),
                        }
                    }
                    if moved > 0 {
                        self.invalidate_all_views();
                    }
                    match (moved, last_err) {
                        (m, None) if m > 0 => self.set_toast(
                            format!(
                                "{} {m} {} {}",
                                t("Movidos", "Moved"),
                                t("elemento(s) a", "item(s) to"),
                                dst_dir.display()
                            ),
                            false,
                        ),
                        (m, Some(err)) => self.set_toast(
                            format!(
                                "{} {m} — {}: {err}",
                                t("Movidos", "Moved"),
                                t("error en alguno", "some failed")
                            ),
                            true,
                        ),
                        _ => {}
                    }
                }
            }
            NodeAction::NtfsCompress(nodes, alg) => {
                if self.read_only_locked {
                    return;
                }
                let roots = deduplicate_roots_slice(&nodes, &self.tree);
                let mut ok = 0usize;
                let mut last_err = None;
                for nid in roots {
                    let p = self.tree.resolve_path(nid);
                    match apply_ntfs_compression(&p, alg) {
                        Ok(()) => ok += 1,
                        Err(e) => last_err = Some(e),
                    }
                }
                if let Some(e) = last_err {
                    self.set_toast(format!("{}: {e}", t("Error compact", "Compact error")), true);
                } else if ok > 0 {
                    self.set_toast(
                        format!(
                            "{} {} {} {ok} {}",
                            t("Aplicando", "Applying"),
                            alg.label(),
                            t("sobre", "on"),
                            t("elemento(s)", "item(s)")
                        ),
                        false,
                    );
                }
            }
            NodeAction::RemoveMotw(nodes) => {
                if self.read_only_locked {
                    return;
                }
                let mut ok = 0usize;
                let mut last_err = None;
                for nid in nodes {
                    if nid.index() < self.tree.len() && !self.tree.is_dir(nid) {
                        match remove_motw(&self.tree.resolve_path(nid)) {
                            Ok(()) => ok += 1,
                            Err(e) => last_err = Some(e),
                        }
                    }
                }
                if let Some(e) = last_err {
                    self.set_toast(format!("{}: {e}", t("Error MOTW", "MOTW error")), true);
                } else if ok > 0 {
                    self.set_toast(
                        format!(
                            "{} ({ok})",
                            t("Zone.Identifier (MOTW) eliminado", "Zone.Identifier (MOTW) removed")
                        ),
                        false,
                    );
                }
            }
            NodeAction::SparsifyFiles(nodes) => {
                if self.read_only_locked {
                    return;
                }
                let files: Vec<(NodeId, PathBuf)> = nodes
                    .into_iter()
                    .filter(|&n| n.index() < self.tree.len() && !self.tree.is_dir(n))
                    .map(|n| (n, self.tree.resolve_path(n)))
                    .collect();
                if !files.is_empty() {
                    self.pending_confirm = Some(PendingConfirm::Sparsify(files));
                }
            }
            NodeAction::SyncFolderDates(nodes) => {
                if self.read_only_locked {
                    return;
                }
                let dirs: Vec<(NodeId, PathBuf)> = deduplicate_roots_slice(&nodes, &self.tree)
                    .into_iter()
                    .filter(|&n| self.tree.is_dir(n))
                    .map(|n| (n, self.tree.resolve_path(n)))
                    .collect();
                if !dirs.is_empty() {
                    self.pending_confirm = Some(PendingConfirm::SyncFolderDates(dirs));
                }
            }
            NodeAction::RemoveEmptySubfolders(nodes) => {
                if self.read_only_locked {
                    return;
                }
                let dirs: Vec<NodeId> = deduplicate_roots_slice(&nodes, &self.tree)
                    .into_iter()
                    .filter(|&n| self.tree.is_dir(n))
                    .collect();
                let mut total_removed = 0usize;
                for &nid in &dirs {
                    total_removed += remove_empty_subfolders(&self.tree.resolve_path(nid));
                }
                self.set_toast(
                    format!(
                        "{} {total_removed} {}",
                        t("Eliminadas", "Removed"),
                        t("subcarpetas vacías", "empty subfolders")
                    ),
                    false,
                );
                if dirs.len() == 1 {
                    self.start_rescan_subtree(dirs[0], ctx.clone());
                }
            }
            NodeAction::RunUserCleanup(slot, nodes) => {
                self.trigger_user_cleanup_batch(slot, &nodes, ctx);
            }
            NodeAction::TrashNodes(nodes) => {
                self.request_trash_nodes(&nodes);
            }
            NodeAction::DeleteNodesPermanently(nodes) => {
                self.request_delete_permanent_nodes(&nodes);
            }
        }
    }

    fn apply_cleanup_refresh(&mut self, policy: CleanupRefreshPolicy, nid: NodeId, ctx: &egui::Context) {
        match policy {
            CleanupRefreshPolicy::None => {}
            CleanupRefreshPolicy::RefreshSelected => self.start_rescan_subtree(nid, ctx.clone()),
            CleanupRefreshPolicy::RefreshParent => {
                let p = self.tree.parents[nid.index()];
                if p.is_some() && p != NodeId::ROOT {
                    self.start_rescan_subtree(p, ctx.clone());
                }
            }
        }
    }

    fn poll_background(&mut self) {
        let mut msg = None;
        if let Some(rx) = &self.bg_rx {
            if let Ok(m) = rx.try_recv() {
                msg = Some(m);
            }
        }
        if let Some(m) = msg {
            match m {
                BgMessage::ScanCompleted(tree) => {
                    self.scanning = false;
                    self.bg_rx = None;
                    let s = tree.summary();
                    self.install_new_tree(tree);
                    self.set_toast(
                        format!(
                            "{} {:.2}s ({}) — {} {} ({})",
                            t("Escaneo listo en", "Scan finished in"),
                            s.scan_duration_secs,
                            s.engine_used,
                            format_count(s.total_files),
                            t("archivos", "files"),
                            format_bytes(s.total_logical_bytes)
                        ),
                        false,
                    );
                }
                BgMessage::RescanCompleted(tree, dir_label) => {
                    self.scanning = false;
                    self.bg_rx = None;
                    self.tree = tree;
                    self.invalidate_all_views();
                    self.set_toast(
                        format!(
                            "{} {dir_label}",
                            t("↻ Carpeta actualizada (F5):", "↻ Folder refreshed (F5):")
                        ),
                        false,
                    );
                }
                BgMessage::SnapshotLoaded(tree, name) => {
                    self.bg_rx = None;
                    let s = tree.summary();
                    self.install_new_tree(tree);
                    self.target_selector.open = false;
                    self.set_toast(
                        format!(
                            "{} '{name}' — {} {} ({})",
                            t("Cargado", "Loaded"),
                            format_count(s.total_files),
                            t("archivos", "files"),
                            format_bytes(s.total_logical_bytes)
                        ),
                        false,
                    );
                }
                BgMessage::OwnersResolved(tree, count) => {
                    self.permissions_view.running_task = false;
                    self.bg_rx = None;
                    self.tree = tree;
                    self.invalidate_all_views();
                    self.set_toast(
                        format!(
                            "{} {} {}",
                            t("👤 Propietarios resueltos para", "👤 Owners resolved for"),
                            format_count(count as u64),
                            t("archivos", "files")
                        ),
                        false,
                    );
                }
                BgMessage::PermissionsScanned(perms) => {
                    self.permissions_view.running_task = false;
                    self.bg_rx = None;
                    let cnt = perms.len();
                    self.tree.permissions = perms;
                    self.set_toast(
                        format!(
                            "{} {} {}",
                            t("🔐 Escaneo DACL completado:", "🔐 DACL scan complete:"),
                            format_count(cnt as u64),
                            t("entradas ACE", "ACE entries")
                        ),
                        false,
                    );
                }
                BgMessage::Toast(text, is_err) => {
                    self.bg_rx = None;
                    self.permissions_view.running_task = false;
                    self.set_toast(text, is_err);
                }
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let input = ctx.input(|i| i.clone());

        if input.key_pressed(egui::Key::F5) {
            if input.modifiers.shift {
                if let Some(cfg) = self.last_scan_config.clone() {
                    self.start_scan(cfg, ctx.clone());
                }
            } else if let Some(nid) = self.selection.primary() {
                self.start_rescan_subtree(nid, ctx.clone());
            }
        }
        if input.key_pressed(egui::Key::F6) {
            self.settings.show_free_space = !self.settings.show_free_space;
            self.treemap.invalidate();
        }
        if input.key_pressed(egui::Key::F7) {
            self.settings.show_unknown_space = !self.settings.show_unknown_space;
            self.treemap.invalidate();
        }
        if input.key_pressed(egui::Key::F8) {
            self.settings.show_extensions_panel = !self.settings.show_extensions_panel;
        }
        if input.key_pressed(egui::Key::F9) {
            self.settings.show_visualization = !self.settings.show_visualization;
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::F) {
            self.main_tab = MainTab::Search;
            self.search_view.focus_input_requested = true;
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::A) && !self.tree.is_empty() {
            let vis = self.current_tab_visible_nodes();
            if !vis.is_empty() {
                self.selection.select_all(&vis);
                self.set_toast(
                    format!(
                        "☑ {} {}",
                        format_count(self.selection.len() as u64),
                        t("elementos seleccionados", "items selected")
                    ),
                    false,
                );
            }
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::O) && !self.scanning {
            self.target_selector.refresh_drives(enumerate_drives());
            self.target_selector.open = true;
            bring_native_window_to_front(ctx, "target_selector_window");
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::S) && !self.tree.is_empty() {
            self.save_snapshot_dialog();
        }
        if !self.selection.is_empty() {
            let targets = self.selection.items().to_vec();
            let primary = self.selection.primary().filter(|&n| n.index() < self.tree.len());
            if input.modifiers.alt && input.key_pressed(egui::Key::Enter) {
                if let Some(nid) = primary {
                    show_windows_properties(&self.tree.resolve_path(nid));
                }
            }
            if input.modifiers.ctrl && input.key_pressed(egui::Key::E) {
                self.handle_node_action(NodeAction::RevealInExplorer(targets.clone()), ctx);
            }
            if input.modifiers.ctrl && input.key_pressed(egui::Key::C) {
                self.handle_node_action(NodeAction::CopyPaths(targets.clone()), ctx);
            }
            if input.key_pressed(egui::Key::Delete) {
                if input.modifiers.shift {
                    self.request_delete_permanent_nodes(&targets);
                } else {
                    self.request_trash_nodes(&targets);
                }
            }
            if input.modifiers.ctrl && input.key_pressed(egui::Key::ArrowUp) {
                self.select_parent_node();
            }
            if input.modifiers.ctrl && input.key_pressed(egui::Key::ArrowDown) {
                self.reselect_child_node();
            }
            if input.modifiers.ctrl && input.key_pressed(egui::Key::ArrowLeft) {
                self.select_first_child_kind(true);
            }
            if input.modifiers.ctrl && input.key_pressed(egui::Key::ArrowRight) {
                self.select_first_child_kind(false);
            }
            if input.modifiers.ctrl {
                let num_keys = [
                    egui::Key::Num0,
                    egui::Key::Num1,
                    egui::Key::Num2,
                    egui::Key::Num3,
                    egui::Key::Num4,
                    egui::Key::Num5,
                    egui::Key::Num6,
                    egui::Key::Num7,
                    egui::Key::Num8,
                    egui::Key::Num9,
                ];
                for (slot, key) in num_keys.into_iter().enumerate() {
                    if input.key_pressed(key) {
                        self.trigger_user_cleanup_batch(slot, &targets, ctx);
                    }
                }
            }
        }
        if input.key_pressed(egui::Key::Backspace) && self.treemap.root_node != NodeId::ROOT {
            let p = self.tree.parents[self.treemap.root_node.index()];
            self.treemap.root_node = if p.is_some() { p } else { NodeId::ROOT };
            self.treemap.invalidate();
        }
        if input.key_pressed(egui::Key::Escape) {
            if self.treemap.root_node != NodeId::ROOT {
                self.treemap.root_node = NodeId::ROOT;
                self.treemap.invalidate();
            } else if !self.selection.is_empty() {
                self.selection.clear();
            }
        }
    }

    /// Classic 7-Section WinDirStat Menu Bar (`File | Edit | Cleanup | View | Tools | Options | Help`)
    fn render_menu_and_toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("mediastat_menubar_and_toolbar").show(ctx, |ui| {
            // 1. Classic Menu Bar
            egui::menu::bar(ui, |ui| {
                ui.menu_button(t("Archivo", "File"), |ui| {
                    if ui
                        .add_enabled(
                            !self.scanning,
                            egui::Button::new(t(
                                "🖴 Seleccionar Unidades / Carpetas...  (Ctrl+O)",
                                "🖴 Select Drives / Folders...  (Ctrl+O)",
                            )),
                        )
                        .clicked()
                    {
                        self.target_selector.refresh_drives(enumerate_drives());
                        self.target_selector.open = true;
                        bring_native_window_to_front(ctx, "target_selector_window");
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            !self.scanning,
                            egui::Button::new(t(
                                "📂 Abrir Snapshot (.mstat / .csv)...",
                                "📂 Open Snapshot (.mstat / .csv)...",
                            )),
                        )
                        .clicked()
                    {
                        self.load_snapshot_dialog(ctx.clone());
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            !self.tree.is_empty(),
                            egui::Button::new(t(
                                "💾 Guardar Snapshot (.mstat)...  (Ctrl+S)",
                                "💾 Save Snapshot (.mstat)...  (Ctrl+S)",
                            )),
                        )
                        .clicked()
                    {
                        self.save_snapshot_dialog();
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            !self.tree.is_empty(),
                            egui::Button::new(t(
                                "📤 Exportar CSV / JSON / HTML / Scripts...",
                                "📤 Export CSV / JSON / HTML / Scripts...",
                            )),
                        )
                        .clicked()
                    {
                        self.export_custom_nodes = None;
                        self.export_open = true;
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            !self.tree.duplicates.is_empty(),
                            egui::Button::new(t(
                                "🧬 Exportar Duplicados (.csv)...",
                                "🧬 Export Duplicates (.csv)...",
                            )),
                        )
                        .clicked()
                    {
                        self.export_format = ExportFormat::DuplicatesCsv;
                        self.export_open = true;
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            !self.tree.permissions.is_empty(),
                            egui::Button::new(t(
                                "🔐 Exportar Permisos DACL (.csv)...",
                                "🔐 Export DACL Permissions (.csv)...",
                            )),
                        )
                        .clicked()
                    {
                        self.export_format = ExportFormat::PermissionsCsv;
                        self.export_open = true;
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            !self.scanning,
                            egui::Button::new(t(
                                "⚖ Comparar Snapshots (Diff)...",
                                "⚖ Compare Snapshots (Diff)...",
                            )),
                        )
                        .clicked()
                    {
                        self.open_diff_dialog();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            !self.scanning && self.last_scan_config.is_some(),
                            egui::Button::new(t(
                                "↻ Refrescar Todo  (Shift+F5)",
                                "↻ Refresh All  (Shift+F5)",
                            )),
                        )
                        .clicked()
                    {
                        if let Some(cfg) = self.last_scan_config.clone() {
                            self.start_scan(cfg, ctx.clone());
                        }
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            !self.scanning && self.selection.primary().is_some(),
                            egui::Button::new(t(
                                "↻ Refrescar Selección  (F5)",
                                "↻ Refresh Selected  (F5)",
                            )),
                        )
                        .clicked()
                    {
                        if let Some(n) = self.selection.primary() {
                            self.start_rescan_subtree(n, ctx.clone());
                        }
                        ui.close_menu();
                    }
                    ui.separator();
                    if !self.is_elevated
                        && ui
                            .button(t(
                                "🛡 Reiniciar como Administrador (NTFS $MFT)",
                                "🛡 Restart as Administrator (NTFS $MFT)",
                            ))
                            .clicked()
                    {
                        relaunch_as_administrator();
                        ui.close_menu();
                    }
                    if ui.button(t("Salir", "Exit")).clicked() {
                        let _ = self.settings.save();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.menu_button(t("Editar", "Edit"), |ui| {
                    let sel_cnt = self.selection.len();
                    let has_sel = sel_cnt > 0;
                    let targets = self.selection.items().to_vec();

                    let copy_lbl = if sel_cnt > 1 {
                        format!(
                            "📋 {} ({sel_cnt})  (Ctrl+C)",
                            t("Copiar rutas completas", "Copy full paths")
                        )
                    } else {
                        t(
                            "📋 Copiar ruta completa  (Ctrl+C)",
                            "📋 Copy full path  (Ctrl+C)",
                        )
                        .to_string()
                    };
                    if ui.add_enabled(has_sel, egui::Button::new(copy_lbl)).clicked() {
                        self.handle_node_action(NodeAction::CopyPaths(targets.clone()), ctx);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            has_sel,
                            egui::Button::new(if sel_cnt > 1 {
                                format!("📄 {} ({sel_cnt})", t("Copiar nombres", "Copy names"))
                            } else {
                                t("📄 Copiar nombre", "📄 Copy name").to_string()
                            }),
                        )
                        .clicked()
                    {
                        self.handle_node_action(NodeAction::CopyNames(targets.clone()), ctx);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            has_sel,
                            egui::Button::new(t(
                                "📊 Copiar resumen (TSV para Excel)",
                                "📊 Copy summary (TSV for Excel)",
                            )),
                        )
                        .clicked()
                    {
                        self.handle_node_action(NodeAction::CopyTsv(targets.clone()), ctx);
                        ui.close_menu();
                    }
                    ui.separator();
                    let vis_nodes = self.current_tab_visible_nodes();
                    if ui
                        .add_enabled(
                            !vis_nodes.is_empty(),
                            egui::Button::new(format!(
                                "{} ({})  (Ctrl+A)",
                                t("☑ Seleccionar todo en vista actual", "☑ Select all in current view"),
                                format_count(vis_nodes.len() as u64)
                            )),
                        )
                        .clicked()
                    {
                        self.selection.select_all(&vis_nodes);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            !vis_nodes.is_empty(),
                            egui::Button::new(t("⇄ Invertir selección", "⇄ Invert selection")),
                        )
                        .clicked()
                    {
                        self.selection.invert_selection(&vis_nodes);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            has_sel,
                            egui::Button::new(t("✕ Limpiar selección  (Esc)", "✕ Clear selection  (Esc)")),
                        )
                        .clicked()
                    {
                        self.selection.clear();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .button(t(
                            "🔍 Buscar en todo el escaneo...  (Ctrl+F)",
                            "🔍 Search entire scan...  (Ctrl+F)",
                        ))
                        .clicked()
                    {
                        self.main_tab = MainTab::Search;
                        self.search_view.focus_input_requested = true;
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            sel_cnt == 1
                                && self.selection.primary().is_some_and(|n| !self.tree.is_dir(n)),
                            egui::Button::new(t(
                                "🔑 Calcular Hashes (MD5 / SHA-256 / BLAKE3)",
                                "🔑 Compute Hashes (MD5 / SHA-256 / BLAKE3)",
                            )),
                        )
                        .clicked()
                    {
                        if let Some(nid) = self.selection.primary() {
                            self.handle_node_action(NodeAction::ComputeHashes(nid), ctx);
                        }
                        ui.close_menu();
                    }
                });

                ui.menu_button(t("Limpieza", "Cleanup"), |ui| {
                    let targets = self.selection.items().to_vec();
                    let stats = self.selection.compute_stats(&self.tree);
                    let has_sel = stats.total_items > 0;
                    let n_sel = stats.total_items;
                    let primary = self.selection.primary().filter(|&n| n.index() < self.tree.len());
                    let unlocked = !self.read_only_locked;

                    if ui
                        .add_enabled(
                            stats.has_files() && stats.file_count <= 10,
                            egui::Button::new(if stats.file_count > 1 {
                                format!("▶ {} ({})", t("Abrir ficheros", "Open files"), stats.file_count)
                            } else {
                                t("▶ Abrir fichero", "▶ Open file").to_string()
                            }),
                        )
                        .clicked()
                    {
                        self.handle_node_action(NodeAction::OpenFiles(targets.clone()), ctx);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            has_sel,
                            egui::Button::new(if n_sel > 1 {
                                format!(
                                    "📂 {} ({n_sel})  (Ctrl+E)",
                                    t("Mostrar en Explorador", "Reveal in Explorer")
                                )
                            } else {
                                t(
                                    "📂 Mostrar en Explorador  (Ctrl+E)",
                                    "📂 Reveal in Explorer  (Ctrl+E)",
                                )
                                .to_string()
                            }),
                        )
                        .clicked()
                    {
                        self.handle_node_action(NodeAction::RevealInExplorer(targets.clone()), ctx);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            primary.is_some(),
                            egui::Button::new(t(
                                ">_ Abrir PowerShell aquí",
                                ">_ Open PowerShell here",
                            )),
                        )
                        .clicked()
                    {
                        if let Some(nid) = primary {
                            self.handle_node_action(NodeAction::OpenPowerShell(nid, false), ctx);
                        }
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            primary.is_some(),
                            egui::Button::new(t(
                                "🛡 Abrir PowerShell Admin aquí",
                                "🛡 Open Admin PowerShell here",
                            )),
                        )
                        .clicked()
                    {
                        if let Some(nid) = primary {
                            self.handle_node_action(NodeAction::OpenPowerShell(nid, true), ctx);
                        }
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            primary.is_some(),
                            egui::Button::new(t("⌨ Abrir CMD aquí", "⌨ Open CMD here")),
                        )
                        .clicked()
                    {
                        if let Some(nid) = primary {
                            self.handle_node_action(NodeAction::OpenCmd(nid), ctx);
                        }
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            primary.is_some(),
                            egui::Button::new(t(
                                "ℹ Propiedades de Windows  (Alt+Enter)",
                                "ℹ Windows Properties  (Alt+Enter)",
                            )),
                        )
                        .clicked()
                    {
                        if let Some(nid) = primary {
                            self.handle_node_action(NodeAction::ShowProperties(nid), ctx);
                        }
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            has_sel && unlocked,
                            egui::Button::new(if n_sel > 1 {
                                format!("🗑 {} ({n_sel})  (Del)", t("Enviar a Papelera", "Move to Recycle Bin"))
                            } else {
                                t("🗑 Enviar a Papelera  (Del)", "🗑 Move to Recycle Bin  (Del)").to_string()
                            }),
                        )
                        .clicked()
                    {
                        self.request_trash_nodes(&targets);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            has_sel && unlocked,
                            egui::Button::new(if n_sel > 1 {
                                format!(
                                    "☠ {} ({n_sel})  (Shift+Del)",
                                    t("Eliminar permanentemente", "Delete permanently")
                                )
                            } else {
                                t(
                                    "☠ Eliminar permanentemente  (Shift+Del)",
                                    "☠ Delete permanently  (Shift+Del)",
                                )
                                .to_string()
                            }),
                        )
                        .clicked()
                    {
                        self.request_delete_permanent_nodes(&targets);
                        ui.close_menu();
                    }
                    let (rb_cnt, rb_bytes) = self.recycle_bin_stats;
                    if ui
                        .add_enabled(
                            unlocked && rb_cnt > 0,
                            egui::Button::new(format!(
                                "{} ({} {}, {})",
                                t("🧹 Vaciar Papelera", "🧹 Empty Recycle Bin"),
                                format_count(rb_cnt),
                                t("elem.", "items"),
                                format_bytes(rb_bytes)
                            )),
                        )
                        .clicked()
                    {
                        self.pending_confirm = Some(PendingConfirm::EmptyRecycleBin(rb_cnt, rb_bytes));
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            stats.has_dirs() && unlocked,
                            egui::Button::new(t(
                                "📁 Eliminar subcarpetas vacías",
                                "📁 Remove empty subfolders",
                            )),
                        )
                        .clicked()
                    {
                        self.handle_node_action(NodeAction::RemoveEmptySubfolders(targets.clone()), ctx);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            has_sel && unlocked,
                            egui::Button::new(if n_sel > 1 {
                                format!("📦 {} ({n_sel})...", t("Mover a otra carpeta", "Move to another folder"))
                            } else {
                                t("📦 Mover a otra carpeta...", "📦 Move to another folder...").to_string()
                            }),
                        )
                        .clicked()
                    {
                        self.handle_node_action(NodeAction::MoveNodes(targets.clone()), ctx);
                        ui.close_menu();
                    }
                    ui.separator();
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
                                    .add_enabled(has_sel && unlocked, egui::Button::new(alg.label()))
                                    .clicked()
                                {
                                    self.handle_node_action(
                                        NodeAction::NtfsCompress(targets.clone(), alg),
                                        ctx,
                                    );
                                    ui.close_menu();
                                }
                            }
                        },
                    );
                    if ui
                        .add_enabled(
                            stats.has_files() && unlocked,
                            egui::Button::new(t(
                                "🔓 Quitar marca de Internet (MOTW)",
                                "🔓 Remove Mark of the Web (MOTW)",
                            )),
                        )
                        .clicked()
                    {
                        self.handle_node_action(NodeAction::RemoveMotw(targets.clone()), ctx);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            stats.has_files() && unlocked,
                            egui::Button::new(t(
                                "🕳 Sparsificar fichero (fsutil)",
                                "🕳 Make file sparse (fsutil)",
                            )),
                        )
                        .clicked()
                    {
                        self.handle_node_action(NodeAction::SparsifyFiles(targets.clone()), ctx);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            stats.has_dirs() && unlocked,
                            egui::Button::new(t(
                                "🕒 Sincronizar fechas de carpetas (más reciente)",
                                "🕒 Sync folder dates (newest file)",
                            )),
                        )
                        .clicked()
                    {
                        self.handle_node_action(NodeAction::SyncFolderDates(targets.clone()), ctx);
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.menu_button(
                        t("🪟 Utilidades de Limpieza Windows", "🪟 Windows Cleanup Utilities"),
                        |ui| {
                            if ui
                                .button(t(
                                    "Liberador de espacio en disco (cleanmgr)",
                                    "Disk Cleanup (cleanmgr)",
                                ))
                                .clicked()
                            {
                                launch_windows_tool(WindowsSysTool::DiskCleanup);
                                ui.close_menu();
                            }
                            if ui
                                .button(t(
                                    "Sensor de almacenamiento (Storage Sense)",
                                    "Storage Sense",
                                ))
                                .clicked()
                            {
                                launch_windows_tool(WindowsSysTool::StorageSense);
                                ui.close_menu();
                            }
                            if ui
                                .button(t(
                                    "Programas y Características (appwiz.cpl)",
                                    "Programs and Features (appwiz.cpl)",
                                ))
                                .clicked()
                            {
                                launch_windows_tool(WindowsSysTool::ProgramsAndFeatures);
                                ui.close_menu();
                            }
                            if ui
                                .button(t("📂 Abrir AppData (Roaming)", "📂 Open AppData (Roaming)"))
                                .clicked()
                            {
                                open_appdata_dir(true);
                                ui.close_menu();
                            }
                            if ui
                                .button(t("📂 Abrir AppData (Local)", "📂 Open AppData (Local)"))
                                .clicked()
                            {
                                open_appdata_dir(false);
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("DISM /AnalyzeComponentStore (Admin)").clicked() {
                                launch_windows_tool(WindowsSysTool::DismAnalyze);
                                ui.close_menu();
                            }
                            if ui.button("DISM /StartComponentCleanup (Admin)").clicked() {
                                launch_windows_tool(WindowsSysTool::DismCleanup);
                                ui.close_menu();
                            }
                            if ui
                                .add_enabled(
                                    unlocked,
                                    egui::Button::new("DISM /StartComponentCleanup /ResetBase (Admin)"),
                                )
                                .clicked()
                            {
                                launch_windows_tool(WindowsSysTool::DismResetBase);
                                ui.close_menu();
                            }
                            if ui
                                .add_enabled(
                                    unlocked,
                                    egui::Button::new(t(
                                        "⚡ Deshabilitar hibernación (powercfg /h off)",
                                        "⚡ Disable Hibernation (powercfg /h off)",
                                    )),
                                )
                                .clicked()
                            {
                                self.pending_confirm = Some(PendingConfirm::DisableHibernation);
                                ui.close_menu();
                            }
                        },
                    );
                    ui.menu_button(
                        t("⚡ Acciones Rápidas SMB (Ctrl+0..9)", "⚡ SMB Quick Actions (Ctrl+0..9)"),
                        |ui| {
                            let cleanups = self.settings.user_cleanups.clone();
                            for (slot, act) in cleanups.iter().enumerate() {
                                if !act.enabled || act.command_line.trim().is_empty() {
                                    continue;
                                }
                                let tag = if act.read_only_safe { "👁" } else { "⚠" };
                                let can_run = has_sel && (act.read_only_safe || unlocked);
                                if ui
                                    .add_enabled(
                                        can_run,
                                        egui::Button::new(format!("{tag} Ctrl+{slot}: {}", act.title)),
                                    )
                                    .clicked()
                                {
                                    self.trigger_user_cleanup_batch(slot, &targets, ctx);
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            if ui
                                .button(t(
                                    "✏ Configurar acciones SMB...",
                                    "✏ Configure SMB actions...",
                                ))
                                .clicked()
                            {
                                self.options_dialog.tab = OptionsTab::Cleanups;
                                self.options_dialog.open = true;
                                ui.close_menu();
                            }
                        },
                    );
                });

                ui.menu_button(t("Ver", "View"), |ui| {
                    ui.menu_button(
                        t("🗺 Treemap — Algoritmo", "🗺 Treemap — Algorithm"),
                        |ui| {
                            for st in [
                                TreemapLayoutStyle::Squarified,
                                TreemapLayoutStyle::KDirStat,
                                TreemapLayoutStyle::Hilbert,
                                TreemapLayoutStyle::Moore,
                            ] {
                                if ui.selectable_label(self.settings.treemap.style == st, st.label()).clicked() {
                                    self.settings.visualization_kind = VisualizationKind::Treemap;
                                    self.settings.treemap.style = st;
                                    self.treemap.invalidate();
                                    ui.close_menu();
                                }
                            }
                        },
                    );
                    if ui
                        .selectable_label(
                            self.settings.visualization_kind == VisualizationKind::Sunburst,
                            t("☀ Sunburst Radial", "☀ Radial Sunburst"),
                        )
                        .clicked()
                    {
                        self.settings.visualization_kind = VisualizationKind::Sunburst;
                        ui.close_menu();
                    }
                    if ui
                        .selectable_label(
                            self.settings.visualization_kind == VisualizationKind::FlameGraph,
                            "🔥 Flame Graph",
                        )
                        .clicked()
                    {
                        self.settings.visualization_kind = VisualizationKind::FlameGraph;
                        ui.close_menu();
                    }
                    ui.menu_button(t("🎨 Paleta de Colores", "🎨 Color Palette"), |ui| {
                        for pal in VizColorPalette::ALL {
                            if ui
                                .selectable_label(self.settings.treemap.color_palette == pal, pal.label())
                                .clicked()
                            {
                                self.settings.treemap.color_palette = pal;
                                self.settings.treemap.custom_volume_colors = None;
                                self.treemap.invalidate();
                                let _ = self.settings.save();
                                ui.close_menu();
                            }
                        }
                    });
                    ui.menu_button(
                        t("✨ Presets Sombreado Cushion", "✨ Cushion Shading Presets"),
                        |ui| {
                            for pr in [
                                TreemapPreset::Classic,
                                TreemapPreset::Calm,
                                TreemapPreset::Flat,
                                TreemapPreset::Pastel,
                                TreemapPreset::HighContrast,
                            ] {
                                if ui.selectable_label(self.settings.treemap.preset == pr, pr.label()).clicked() {
                                    self.settings.treemap.apply_shading_preset(pr);
                                    self.treemap.invalidate();
                                    ui.close_menu();
                                }
                            }
                        },
                    );
                    ui.separator();
                    if ui
                        .radio_value(
                            &mut self.settings.size_metric,
                            SizeMetric::Logical,
                            t("Tamaño Lógico", "Logical Size"),
                        )
                        .changed()
                        || ui
                            .radio_value(
                                &mut self.settings.size_metric,
                                SizeMetric::AllocatedOnDisk,
                                t("Tamaño Físico en Disco", "Physical Size on Disk"),
                            )
                            .changed()
                    {
                        self.treemap.invalidate();
                        self.largest_files.invalidate();
                    }
                    ui.separator();
                    ui.checkbox(
                        &mut self.settings.absolute_percentages,
                        t("Porcentajes absolutos del escaneo", "Absolute scan percentages"),
                    );
                    ui.checkbox(
                        &mut self.settings.tree_bar_classic_depth_colors,
                        t(
                            "Barras 8 colores WinDirStat en árbol",
                            "WinDirStat 8-color depth bars in tree",
                        ),
                    );
                    ui.checkbox(
                        &mut self.settings.treemap.show_color_legend,
                        t("Mostrar leyenda de colores", "Show color legend"),
                    );
                    if ui
                        .checkbox(
                            &mut self.settings.treemap.show_extensions_overlay,
                            t("Mostrar etiquetas en Treemap", "Show labels on Treemap"),
                        )
                        .changed()
                        || ui
                            .checkbox(
                                &mut self.settings.treemap.show_folder_frames,
                                t("Mostrar marcos de carpeta", "Show folder frames"),
                            )
                            .changed()
                    {
                        self.treemap.invalidate();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            self.selection.primary().is_some(),
                            egui::Button::new(t(
                                "⬆ Seleccionar padre  (Ctrl+↑)",
                                "⬆ Select parent  (Ctrl+↑)",
                            )),
                        )
                        .clicked()
                    {
                        self.select_parent_node();
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            !self.selection_child_history.is_empty(),
                            egui::Button::new(t(
                                "⬇ Reseleccionar hijo  (Ctrl+↓)",
                                "⬇ Reselect child  (Ctrl+↓)",
                            )),
                        )
                        .clicked()
                    {
                        self.reselect_child_node();
                        ui.close_menu();
                    }
                    if ui
                        .button(t("🔍+ Zoom en carpeta seleccionada", "🔍+ Zoom into selected folder"))
                        .clicked()
                    {
                        if let Some(n) = self.selection.primary() {
                            let target = if self.tree.is_dir(n) { n } else { self.tree.parents[n.index()] };
                            if target.is_some() {
                                self.treemap.root_node = target;
                                self.treemap.invalidate();
                            }
                        }
                        ui.close_menu();
                    }
                    if ui
                        .button(t("🔍- Subir nivel  (Backspace)", "🔍- Zoom out one level  (Backspace)"))
                        .clicked()
                        && self.treemap.root_node != NodeId::ROOT
                    {
                        let p = self.tree.parents[self.treemap.root_node.index()];
                        self.treemap.root_node = if p.is_some() { p } else { NodeId::ROOT };
                        self.treemap.invalidate();
                        ui.close_menu();
                    }
                    if ui
                        .button(t("↺ Restablecer Zoom  (Esc)", "↺ Reset Zoom  (Esc)"))
                        .clicked()
                    {
                        self.treemap.root_node = NodeId::ROOT;
                        self.treemap.invalidate();
                        ui.close_menu();
                    }
                });

                ui.menu_button(t("Herramientas", "Tools"), |ui| {
                    if ui
                        .button(t(
                            "📡 Monitor en Tiempo Real (FileWatcher)",
                            "📡 Real-Time Monitor (FileWatcher)",
                        ))
                        .clicked()
                    {
                        self.main_tab = MainTab::Watcher;
                        ui.close_menu();
                    }
                    if ui
                        .button(t(
                            "🔐 Permisos NTFS/SMB y Propietarios",
                            "🔐 NTFS/SMB Permissions & Owners",
                        ))
                        .clicked()
                    {
                        self.main_tab = MainTab::Permissions;
                        ui.close_menu();
                    }
                    if ui
                        .button(t(
                            "📊 Analítica de Tiers y Costes",
                            "📊 Tier & Cost Analytics",
                        ))
                        .clicked()
                    {
                        self.main_tab = MainTab::StorageAnalytics;
                        ui.close_menu();
                    }
                    if ui
                        .button(t(
                            "⚠ Auditoría SysAdmin (Access Denied / >260 / Cloud)",
                            "⚠ SysAdmin Audit (Access Denied / >260 / Cloud)",
                        ))
                        .clicked()
                    {
                        self.main_tab = MainTab::SysAdminAudit;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .button(t(
                            "⚡ Optimizar / Desfragmentar Unidades (dfrgui)",
                            "⚡ Optimize / Defragment Drives (dfrgui)",
                        ))
                        .clicked()
                    {
                        launch_windows_tool(WindowsSysTool::DefragOptimize);
                        ui.close_menu();
                    }
                    if ui
                        .button(t(
                            "🖴 Administración de Discos (diskmgmt.msc)",
                            "🖴 Disk Management (diskmgmt.msc)",
                        ))
                        .clicked()
                    {
                        launch_windows_tool(WindowsSysTool::DiskManagement);
                        ui.close_menu();
                    }
                });

                ui.menu_button(t("Opciones", "Options"), |ui| {
                    if ui
                        .checkbox(
                            &mut self.settings.show_free_space,
                            t("Mostrar <Espacio Libre>  (F6)", "Show <Free Space>  (F6)"),
                        )
                        .changed()
                        || ui
                            .checkbox(
                                &mut self.settings.show_unknown_space,
                                t(
                                    "Mostrar <Espacio Desconocido>  (F7)",
                                    "Show <Unknown Space>  (F7)",
                                ),
                            )
                            .changed()
                    {
                        self.treemap.invalidate();
                    }
                    ui.checkbox(
                        &mut self.settings.group_files_node,
                        t("Agrupar ficheros en <Archivos>", "Group files under <Files>"),
                    );
                    ui.separator();
                    ui.checkbox(
                        &mut self.settings.show_extensions_panel,
                        t(
                            "Mostrar Panel Extensiones/Inspector  (F8)",
                            "Show Extensions/Inspector Panel  (F8)",
                        ),
                    );
                    ui.checkbox(
                        &mut self.settings.group_minor_extensions,
                        t(
                            "Agrupar extensiones minoritarias (<0.1%)",
                            "Group minor extensions (<0.1%)",
                        ),
                    );
                    ui.checkbox(
                        &mut self.settings.show_visualization,
                        t("Mostrar Visualización  (F9)", "Show Visualization  (F9)"),
                    );
                    ui.checkbox(
                        &mut self.settings.show_toolbar,
                        t("Mostrar Barra de Herramientas", "Show Toolbar"),
                    );
                    ui.checkbox(
                        &mut self.settings.show_status_bar,
                        t("Mostrar Barra de Estado", "Show Status Bar"),
                    );
                    ui.separator();
                    if ui
                        .button(t(
                            "⊞ Elegir Layout de Ventana...",
                            "⊞ Choose Window Layout...",
                        ))
                        .clicked()
                    {
                        self.layout_popup.open = true;
                        ui.close_menu();
                    }
                    ui.checkbox(
                        &mut self.settings.float_visualization,
                        t("🗗 Flotar Visualización", "🗗 Detach Visualization"),
                    );
                    ui.checkbox(
                        &mut self.settings.float_extensions_panel,
                        t(
                            "🗗 Flotar Extensiones/Inspector",
                            "🗗 Detach Extensions/Inspector",
                        ),
                    );
                    if ui
                        .button(t(
                            "↺ Restablecer posición de subventanas",
                            "↺ Reset subwindow positions",
                        ))
                        .clicked()
                    {
                        self.settings.float_visualization = false;
                        self.settings.float_extensions_panel = false;
                        ctx.memory_mut(|m| m.reset_areas());
                        self.set_toast(
                            t(
                                "Posiciones y tamaños de subventanas restablecidos",
                                "Subwindow positions and sizes reset",
                            ),
                            false,
                        );
                        ui.close_menu();
                    }
                    ui.menu_button(t("🔠 Escala de Fuente / UI", "🔠 Font / UI Scale"), |ui| {
                        for pct in [100u16, 125, 150, 175, 200] {
                            if ui.selectable_label(self.settings.ui_scale_percent == pct, format!("{pct}%")).clicked() {
                                self.settings.ui_scale_percent = pct;
                                apply_visual_theme(ctx, self.settings.dark_mode, pct);
                                ui.close_menu();
                            }
                        }
                    });
                    ui.menu_button(
                        t("🌐 Idioma / Language", "🌐 Language / Idioma"),
                        |ui| {
                            for lang in Language::ALL {
                                if ui
                                    .selectable_label(self.settings.language == lang, lang.native_name())
                                    .clicked()
                                {
                                    if self.settings.language != lang {
                                        self.settings.apply_language(lang);
                                        self.settings.sync_active_language();
                                        self.on_language_changed(ctx);
                                    }
                                    ui.close_menu();
                                }
                            }
                        },
                    );
                    ui.separator();
                    if ui
                        .button(t("⚙ Configuración completa...", "⚙ Full Settings..."))
                        .clicked()
                    {
                        self.options_dialog.open = true;
                        bring_native_window_to_front(ctx, "options_dialog_window");
                        ui.close_menu();
                    }
                });

                ui.menu_button(t("Ayuda", "Help"), |ui| {
                    if ui
                        .button(t(
                            "⌨ Referencia de Atajos de Teclado",
                            "⌨ Keyboard Shortcuts Reference",
                        ))
                        .clicked()
                    {
                        self.shortcuts_open = true;
                        ui.close_menu();
                    }
                    if ui
                        .button(t("ℹ Acerca de MediaStat v0.3", "ℹ About MediaStat v0.3"))
                        .clicked()
                    {
                        self.about_open = true;
                        ui.close_menu();
                    }
                });
            });

            // 2. Classic + Modern Toolbar
            if self.settings.show_toolbar {
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(!self.scanning, egui::Button::new(t("🖴 Abrir...", "🖴 Open...")))
                        .on_hover_text(t(
                            "Seleccionar unidades / carpetas (Ctrl+O)",
                            "Select drives / folders (Ctrl+O)",
                        ))
                        .clicked()
                    {
                        self.target_selector.refresh_drives(enumerate_drives());
                        self.target_selector.open = true;
                        bring_native_window_to_front(ctx, "target_selector_window");
                    }
                    if ui
                        .add_enabled(
                            !self.scanning && self.selection.primary().is_some(),
                            egui::Button::new("↻ F5"),
                        )
                        .on_hover_text(t(
                            "Refrescar carpeta seleccionada sin re-escanear todo",
                            "Refresh selected folder without re-scanning everything",
                        ))
                        .clicked()
                    {
                        if let Some(n) = self.selection.primary() {
                            self.start_rescan_subtree(n, ctx.clone());
                        }
                    }

                    if self.scanning {
                        let paused = self.progress.is_paused();
                        if ui
                            .button(if paused {
                                t("▶ Reanudar", "▶ Resume")
                            } else {
                                t("⏸ Pausar", "⏸ Pause")
                            })
                            .clicked()
                        {
                            self.progress.set_paused(!paused);
                        }
                        if ui
                            .button(
                                RichText::new(t("⏹ Detener", "⏹ Stop"))
                                    .color(Color32::from_rgb(240, 85, 85)),
                            )
                            .clicked()
                        {
                            self.progress.request_cancel();
                        }
                    }

                    ui.separator();
                    if ui
                        .add_enabled(!self.scanning, egui::Button::new(t("📂 Cargar", "📂 Load")))
                        .on_hover_text(t("Abrir .mstat o CSV", "Open .mstat or CSV"))
                        .clicked()
                    {
                        self.load_snapshot_dialog(ctx.clone());
                    }
                    if ui
                        .add_enabled(!self.tree.is_empty(), egui::Button::new(t("💾 Guardar", "💾 Save")))
                        .on_hover_text(t("Guardar snapshot .mstat", "Save .mstat snapshot"))
                        .clicked()
                    {
                        self.save_snapshot_dialog();
                    }
                    if ui
                        .add_enabled(!self.tree.is_empty(), egui::Button::new(t("📤 Exportar", "📤 Export")))
                        .on_hover_text(t(
                            "CSV / JSON / Reporte HTML / Scripts",
                            "CSV / JSON / HTML Report / Scripts",
                        ))
                        .clicked()
                    {
                        self.export_custom_nodes = None;
                        self.export_open = true;
                    }

                    ui.separator();
                    let has_sel = !self.selection.is_empty();
                    let sel_targets = self.selection.items().to_vec();
                    let primary = self.selection.primary();
                    if ui
                        .add_enabled(has_sel, egui::Button::new("📋"))
                        .on_hover_text(t(
                            "Copiar ruta(s) completa(s) (Ctrl+C)",
                            "Copy full path(s) (Ctrl+C)",
                        ))
                        .clicked()
                    {
                        self.handle_node_action(NodeAction::CopyPaths(sel_targets.clone()), ctx);
                    }
                    if ui
                        .add_enabled(has_sel, egui::Button::new("📂 Exp"))
                        .on_hover_text(t(
                            "Mostrar en Explorador (Ctrl+E)",
                            "Reveal in Explorer (Ctrl+E)",
                        ))
                        .clicked()
                    {
                        self.handle_node_action(NodeAction::RevealInExplorer(sel_targets.clone()), ctx);
                    }
                    if ui
                        .add_enabled(primary.is_some(), egui::Button::new(">_ PS"))
                        .on_hover_text(t(
                            "Abrir PowerShell en esta carpeta",
                            "Open PowerShell in this folder",
                        ))
                        .clicked()
                    {
                        if let Some(n) = primary {
                            open_terminal_at(&self.tree.resolve_path(n), true, false);
                        }
                    }
                    if ui
                        .add_enabled(primary.is_some(), egui::Button::new("ℹ Prop"))
                        .on_hover_text(t(
                            "Propiedades de Windows (Alt+Enter)",
                            "Windows Properties (Alt+Enter)",
                        ))
                        .clicked()
                    {
                        if let Some(n) = primary {
                            show_windows_properties(&self.tree.resolve_path(n));
                        }
                    }
                    if ui
                        .add_enabled(has_sel && !self.read_only_locked, egui::Button::new("🗑"))
                        .on_hover_text(t(
                            "Enviar seleccionado(s) a Papelera (Del)",
                            "Move selected item(s) to Recycle Bin (Del)",
                        ))
                        .clicked()
                    {
                        self.request_trash_nodes(&sel_targets);
                    }

                    ui.separator();
                    if ui
                        .button("⊞ Layout")
                        .on_hover_text(t(
                            "Cambiar disposición de paneles",
                            "Change panel layout",
                        ))
                        .clicked()
                    {
                        self.layout_popup.open = !self.layout_popup.open;
                    }
                    if ui.button(t("⚙ Opciones", "⚙ Settings")).clicked() {
                        self.options_dialog.open = true;
                        bring_native_window_to_front(ctx, "options_dialog_window");
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(if self.settings.dark_mode { "☀" } else { "🌙" })
                            .on_hover_text(t("Cambiar tema", "Toggle theme"))
                            .clicked()
                        {
                            self.settings.dark_mode = !self.settings.dark_mode;
                            apply_visual_theme(ctx, self.settings.dark_mode, self.settings.ui_scale_percent);
                        }

                        // Corporate Read-Only Safety Lock Toggle
                        let (lock_lbl, lock_col) = if self.read_only_locked {
                            (t("🔒 Solo Lectura", "🔒 Read-Only"), Color32::from_rgb(76, 195, 130))
                        } else {
                            (t("🔓 Limpieza Activa", "🔓 Cleanup Active"), Color32::from_rgb(245, 120, 65))
                        };
                        if ui
                            .button(RichText::new(lock_lbl).strong().color(lock_col))
                            .on_hover_text(t(
                                "Protege tus servidores SMB contra borrados accidentales. Haz clic para desbloquear/bloquear acciones de limpieza.",
                                "Protects your SMB servers from accidental deletion. Click to unlock/lock cleanup actions.",
                            ))
                            .clicked()
                        {
                            self.read_only_locked = !self.read_only_locked;
                            self.recycle_bin_stats = query_recycle_bin();
                        }

                        if !self.is_elevated {
                            if ui
                                .button(RichText::new("🛡 Admin").color(Color32::from_rgb(245, 195, 75)))
                                .on_hover_text(t(
                                    "Reiniciar elevado como Administrador para habilitar NTFS $MFT y carpetas restringidas",
                                    "Restart elevated as Administrator to enable NTFS $MFT and restricted folders",
                                ))
                                .clicked()
                            {
                                relaunch_as_administrator();
                            }
                        } else {
                            ui.label(RichText::new("🛡 ADMIN").small().strong().color(Color32::from_rgb(76, 210, 128)));
                        }

                        // Metric toggle
                        let m_lbl = match self.settings.size_metric {
                            SizeMetric::Logical => t("📐 Lógico", "📐 Logical"),
                            SizeMetric::AllocatedOnDisk => t("💽 En Disco", "💽 On Disk"),
                        };
                        if ui
                            .button(m_lbl)
                            .on_hover_text(t(
                                "Alternar entre Tamaño Lógico y Tamaño Real en Disco",
                                "Toggle between Logical Size and Allocated Size on Disk",
                            ))
                            .clicked()
                        {
                            self.settings.size_metric = match self.settings.size_metric {
                                SizeMetric::Logical => SizeMetric::AllocatedOnDisk,
                                SizeMetric::AllocatedOnDisk => SizeMetric::Logical,
                            };
                            self.treemap.invalidate();
                            self.largest_files.invalidate();
                        }
                    });
                });
            }

            // 3. Live Progress OR Executive KPI Strip
            ui.add_space(2.0);
            if self.scanning {
                self.render_live_progress(ui, ctx);
            } else if !self.tree.is_empty() {
                let s = self.tree.summary();
                ui.horizontal_wrapped(|ui| {
                    kpi_chip(
                        ui,
                        t("Archivos", "Files"),
                        &format_count(s.total_files),
                        Color32::from_rgb(130, 185, 240),
                    );
                    kpi_chip(
                        ui,
                        t("Carpetas", "Folders"),
                        &format_count(s.total_dirs),
                        Color32::from_rgb(155, 195, 235),
                    );
                    kpi_chip(
                        ui,
                        if self.settings.size_metric == SizeMetric::Logical {
                            t("Lógico", "Logical")
                        } else {
                            t("En Disco", "On Disk")
                        },
                        &format_bytes(self.tree.size_for_metric(NodeId::ROOT, self.settings.size_metric)),
                        Color32::from_rgb(115, 200, 240),
                    );
                    if s.duplicate_groups > 0 {
                        kpi_chip(
                            ui,
                            t("Duplicados", "Duplicates"),
                            &format!(
                                "{} {} ({})",
                                format_count(s.duplicate_groups),
                                t("grupos", "groups"),
                                format_bytes(s.duplicate_reclaimable_bytes)
                            ),
                            Color32::from_rgb(185, 130, 245),
                        );
                    }
                    if s.access_denied_count > 0 || s.long_path_count > 0 {
                        kpi_chip(
                            ui,
                            "SysAdmin",
                            &format!("{} Denied · {} >260", s.access_denied_count, s.long_path_count),
                            Color32::from_rgb(245, 185, 75),
                        );
                    }
                });
            }
            ui.add_space(2.0);
        });
    }

    fn render_live_progress(&self, ui: &mut Ui, ctx: &egui::Context) {
        let phase = self.progress.current_phase();
        let files = self.progress.walker.files_scanned.load(Ordering::Relaxed);
        let dirs = self.progress.walker.dirs_scanned.load(Ordering::Relaxed);
        let bytes = self.progress.walker.bytes_scanned.load(Ordering::Relaxed);
        let ddone = self.progress.dup_done.load(Ordering::Relaxed);
        let dtot = self.progress.dup_total.load(Ordering::Relaxed);
        let elapsed = self
            .scan_started
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.01)
            .max(0.01);
        let rate = (files as f64 / elapsed) as u64;

        ui.horizontal(|ui| {
            if self.settings.use_pacman_animation {
                draw_pacman_widget(ui, elapsed as f32);
            } else {
                ui.spinner();
            }
            ui.strong(phase.label());
            ui.separator();
            ui.label(format!(
                "{}: {} ({}/s) · {}: {} · {}",
                t("Archivos", "Files"),
                format_count(files),
                format_count(rate),
                t("Carpetas", "Folders"),
                format_count(dirs),
                format_bytes(bytes)
            ));
            if dtot > 0 {
                ui.separator();
                let frac = (ddone as f32 / dtot as f32).clamp(0.0, 1.0);
                ui.add(
                    egui::ProgressBar::new(frac)
                        .desired_width(140.0)
                        .text(format!("BLAKE3 {ddone}/{dtot}")),
                );
            }
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }

    /// Classic WinDirStat Bottom Status Bar
    fn render_status_bar(&self, ctx: &egui::Context) {
        if !self.settings.show_status_bar {
            return;
        }
        egui::TopBottomPanel::bottom("mediastat_statusbar")
            .exact_height(24.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let left_txt = if !self.hover_status_text.is_empty() {
                        self.hover_status_text.clone()
                    } else if self.selection.len() > 1 {
                        let st = self.selection.compute_stats(&self.tree);
                        let tot = self.tree.size_for_metric(NodeId::ROOT, self.settings.size_metric).max(1);
                        let metric_sz = match self.settings.size_metric {
                            SizeMetric::Logical => st.logical_bytes,
                            SizeMetric::AllocatedOnDisk => st.allocated_bytes,
                        };
                        format!(
                            "✓ {} {} ({} {}, {} {}) · {} ({:.2}% {})",
                            format_count(st.total_items as u64),
                            t("elementos seleccionados", "items selected"),
                            format_count(st.file_count as u64),
                            t("fich.", "files"),
                            format_count(st.dir_count as u64),
                            t("carp.", "dirs"),
                            format_bytes(metric_sz),
                            (metric_sz as f64 / tot as f64) * 100.0,
                            t("total", "total")
                        )
                    } else if let Some(nid) = self.selection.primary().filter(|&n| n.index() < self.tree.len()) {
                        let p = self.tree.resolve_path(nid);
                        let sz = self.tree.size_for_metric(nid, self.settings.size_metric);
                        let tot = self.tree.size_for_metric(NodeId::ROOT, self.settings.size_metric).max(1);
                        format!(
                            "{}  ({} · {:.2}% {})",
                            p.display(),
                            format_bytes(sz),
                            (sz as f64 / tot as f64) * 100.0,
                            t("total", "total")
                        )
                    } else {
                        t(
                            "Listo — Ctrl/Shift+Clic o casillas para multiselección · Ctrl+A todo · Ctrl+F buscar",
                            "Ready — Ctrl/Shift+Click or checkboxes for multi-select · Ctrl+A all · Ctrl+F search",
                        )
                        .into()
                    };
                    ui.label(RichText::new(left_txt).small().monospace());

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{}: {} · {}: {}",
                                t("RAM árbol", "Tree RAM"),
                                format_bytes(self.tree.approx_memory_bytes() as u64),
                                t("Motor", "Engine"),
                                if self.tree.engine_used.is_empty() { "—" } else { &self.tree.engine_used }
                            ))
                            .small()
                            .weak(),
                        );
                        ui.separator();
                        let lock_txt = if self.read_only_locked {
                            t("🔒 Solo Lectura", "🔒 Read-Only")
                        } else {
                            t("🔓 Limpieza", "🔓 Cleanup")
                        };
                        ui.label(RichText::new(lock_txt).small());
                    });
                });
            });
    }

    /// Render the Visualization Pane (Treemap Cushion / Sunburst / Flame Graph) inside any panel.
    fn render_visualization_contents(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        ui.horizontal_wrapped(|ui| {
            for vk in [
                VisualizationKind::Treemap,
                VisualizationKind::Sunburst,
                VisualizationKind::FlameGraph,
            ] {
                if ui
                    .selectable_label(self.settings.visualization_kind == vk, vk.label())
                    .clicked()
                {
                    self.settings.visualization_kind = vk;
                }
            }
            ui.separator();

            // Breadcrumbs
            let mut chain = Vec::new();
            let mut cur = self.treemap.root_node;
            while cur.is_some() && cur != NodeId::ROOT && cur.index() < self.tree.len() {
                chain.push(cur);
                cur = self.tree.parents[cur.index()];
            }
            chain.reverse();

            if ui
                .selectable_label(self.treemap.root_node == NodeId::ROOT, t("Raíz", "Root"))
                .clicked()
            {
                self.treemap.root_node = NodeId::ROOT;
                self.treemap.invalidate();
            }
            for &crumb in &chain {
                ui.label("›");
                if ui
                    .selectable_label(self.treemap.root_node == crumb, self.tree.name(crumb))
                    .clicked()
                {
                    self.treemap.root_node = crumb;
                    self.treemap.invalidate();
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (dock_icon, dock_tip) = if self.settings.float_visualization {
                    (
                        "📌",
                        t(
                            "Acoplar panel de Visualización al layout principal",
                            "Dock Visualization panel back to main layout",
                        ),
                    )
                } else {
                    (
                        "🗗",
                        t(
                            "Desacoplar Visualización como subventana flotante movible",
                            "Detach Visualization as a free-floating window",
                        ),
                    )
                };
                if ui.small_button(dock_icon).on_hover_text(dock_tip).clicked() {
                    self.settings.float_visualization = !self.settings.float_visualization;
                    self.treemap.invalidate();
                }

                egui::ComboBox::from_id_salt("viz_palette_quick")
                    .width(96.0)
                    .selected_text(format!("🎨 {}", self.settings.treemap.color_palette.short_label()))
                    .show_ui(ui, |ui| {
                        for pal in VizColorPalette::ALL {
                            if ui
                                .selectable_label(self.settings.treemap.color_palette == pal, pal.label())
                                .on_hover_text(pal.description())
                                .clicked()
                            {
                                self.settings.treemap.color_palette = pal;
                                self.settings.treemap.custom_volume_colors = None;
                                self.treemap.invalidate();
                            }
                        }
                    });

                for mode in [
                    VizColorMode::FileAge,
                    VizColorMode::ByCategory,
                    VizColorMode::ByExtension,
                ] {
                    if ui
                        .selectable_label(self.settings.treemap.color_mode == mode, mode.label())
                        .clicked()
                    {
                        self.settings.treemap.color_mode = mode;
                        self.treemap.invalidate();
                    }
                }
            });
        });

        if self.settings.treemap.show_color_legend {
            self.render_color_legend(ui);
        }
        ui.separator();

        if self.tree.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(t(
                        "Inicia un escaneo o abre un snapshot .mstat / CSV para ver el mapa de disco.",
                        "Start a scan or open a .mstat / CSV snapshot to view the disk map.",
                    ))
                    .weak(),
                );
            });
            return;
        }

        match self.settings.visualization_kind {
            VisualizationKind::Sunburst => {
                if let Some(act) = show_sunburst(
                    ui,
                    &self.tree,
                    self.treemap.root_node,
                    self.selection.primary(),
                    &self.settings.treemap,
                    self.settings.size_metric,
                    self.settings.show_free_space,
                    self.settings.show_unknown_space,
                ) {
                    self.handle_viz_action(act);
                }
            }
            VisualizationKind::FlameGraph => {
                if let Some(act) = show_flame_graph(
                    ui,
                    &self.tree,
                    self.treemap.root_node,
                    self.selection.primary(),
                    &self.settings.treemap,
                    self.settings.size_metric,
                    self.settings.show_free_space,
                    self.settings.show_unknown_space,
                ) {
                    self.handle_viz_action(act);
                }
            }
            VisualizationKind::Treemap => {
                self.render_treemap_canvas(ui, ctx);
            }
        }
    }

    fn render_color_legend(&self, ui: &mut Ui) {
        let opts = &self.settings.treemap;
        let pal = opts.color_palette;

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let mut swatch = |rgb: [u8; 3], label: &str| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 10.0), egui::Sense::hover());
                    draw_mini_cushion_rect(ui.painter(), r, rgb, opts);
                    ui.label(RichText::new(label).small().weak());
                });
            };

            match opts.color_mode {
                VizColorMode::ByCategory => {
                    for cat in FileCategory::ALL {
                        swatch(category_color_for_palette(cat, pal), cat.short_label());
                    }
                }
                VizColorMode::FileAge => {
                    for (days, lbl) in [
                        (5.0, "<30d"),
                        (90.0, "1–6m"),
                        (240.0, "6–12m"),
                        (500.0, t("1–2a", "1–2y")),
                        (1000.0, t("2–4a", "2–4y")),
                        (2000.0, t(">4a", ">4y")),
                    ] {
                        swatch(age_color_for_palette(days, pal), lbl);
                    }
                }
                VizColorMode::ByExtension => {
                    if opts.use_rank_extension_colors
                        && pal == VizColorPalette::WinDirStatClassic
                        && !self.tree.is_empty()
                    {
                        let mut top_exts: Vec<_> = self
                            .tree
                            .extensions
                            .entries()
                            .iter()
                            .filter(|e| e.file_count > 0)
                            .collect();
                        top_exts.sort_unstable_by_key(|e| {
                            std::cmp::Reverse(e.total_logical_bytes.max(e.total_allocated_bytes))
                        });
                        for e in top_exts.into_iter().take(8) {
                            let lbl = if e.ext.is_empty() {
                                t("(sin ext)", "(no ext)").to_string()
                            } else {
                                format!(".{}", e.ext)
                            };
                            swatch(extension_display_color(&self.tree, e.id, opts), &lbl);
                        }
                    } else {
                        for (ext, lbl) in [
                            ("mp4", t("Vídeo", "Video")),
                            ("jpg", t("Imagen", "Image")),
                            ("cr2", "RAW"),
                            ("dwg", "CAD/BIM"),
                            ("zip", t("Archivo", "Archive")),
                            ("pdf", "PDF"),
                            ("exe", t("Ejec.", "Exec")),
                        ] {
                            swatch(extension_color_for_palette(ext, pal), lbl);
                        }
                    }
                }
            }

            if self.settings.show_free_space {
                swatch(free_space_color(opts), t("<Libre>", "<Free>"));
            }
            if self.settings.show_unknown_space {
                swatch(unknown_space_color(opts), t("<Descon.>", "<Unknown>"));
            }
        });
    }

    fn handle_viz_action(&mut self, act: VizAction) {
        match act {
            VizAction::Select(nid) => self.select_node_and_sync(nid, true),
            VizAction::ZoomInto(nid) => {
                self.treemap.root_node = nid;
                self.treemap.invalidate();
            }
            VizAction::ZoomOut => {
                if self.treemap.root_node != NodeId::ROOT {
                    let p = self.tree.parents[self.treemap.root_node.index()];
                    self.treemap.root_node = if p.is_some() { p } else { NodeId::ROOT };
                    self.treemap.invalidate();
                }
            }
            VizAction::HoverText(txt) => self.hover_status_text = txt,
        }
    }

    fn render_treemap_canvas(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let avail = ui.available_size();
        let w = (avail.x.floor() as usize).max(64);
        let h = (avail.y.floor() as usize).max(64);

        self.treemap.ensure_rendered(
            ctx,
            &self.tree,
            self.tree_gen,
            w,
            h,
            &self.settings.treemap,
            self.settings.size_metric,
            self.settings.show_free_space,
            self.settings.show_unknown_space,
        );

        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(w as f32, h as f32), egui::Sense::click());

        if let Some(tex) = &self.treemap.texture {
            ui.painter().image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        // Optional Folder Frames overlay (WinDirStat 2.10 `ID_TREEMAP_SHOW_FOLDER_FRAMES`)
        if self.settings.treemap.show_folder_frames {
            let painter = ui.painter();
            for &(fx0, fy0, fx1, fy1, _) in &self.treemap.folder_frames {
                let r = egui::Rect::from_min_max(
                    rect.min + egui::vec2(fx0, fy0),
                    rect.min + egui::vec2(fx1, fy1),
                );
                painter.rect_stroke(
                    r,
                    0.0,
                    Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 80)),
                    egui::StrokeKind::Inside,
                );
            }
        }

        // Optional Extension / File Name Labels overlay on large tiles (WinDirStat 2.10 `ID_TREEMAP_SHOW_EXTENSIONS`)
        if self.settings.treemap.show_extensions_overlay {
            let painter = ui.painter_at(rect);
            for leaf in &self.treemap.leaves {
                let lw = leaf.x1 - leaf.x0;
                let lh = leaf.y1 - leaf.y0;
                if lw > 52.0 && lh > 18.0 {
                    let text = match leaf.item {
                        SpecialTile::Normal(nid) => {
                            let ext = self.tree.ext(nid);
                            if lw > 110.0 {
                                let name = self.tree.name(nid);
                                let max_c = ((lw - 8.0) / 6.5) as usize;
                                if name.chars().count() <= max_c {
                                    name.to_string()
                                } else {
                                    let t: String = name.chars().take(max_c.saturating_sub(1)).collect();
                                    format!("{t}…")
                                }
                            } else if !ext.is_empty() {
                                format!(".{ext}")
                            } else {
                                self.tree.name(nid).chars().take(8).collect()
                            }
                        }
                        SpecialTile::FreeSpace(_, _) => t("<Libre>", "<Free>").into(),
                        SpecialTile::UnknownSpace(_, _) => t("<Descon.>", "<Unknown>").into(),
                    };
                    let center = rect.min + egui::vec2((leaf.x0 + leaf.x1) * 0.5, (leaf.y0 + leaf.y1) * 0.5);
                    let light_bg = self.settings.treemap.contrast_labels && is_light_rgb(leaf.rgb);
                    let fg = if light_bg {
                        Color32::from_rgb(16, 20, 28)
                    } else {
                        Color32::WHITE
                    };
                    if !light_bg {
                        painter.text(
                            center + egui::vec2(1.0, 1.0),
                            egui::Align2::CENTER_CENTER,
                            &text,
                            egui::FontId::proportional(10.5),
                            Color32::from_black_alpha(175),
                        );
                    }
                    painter.text(
                        center,
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::proportional(10.5),
                        fg,
                    );
                }
            }
        }

        // Extension highlight from WinDirStat Extension List
        if let Some(hl_ext) = self.right_panel.highlighted_ext_id {
            let painter = ui.painter();
            for leaf in &self.treemap.leaves {
                if leaf.ext_id == hl_ext {
                    let r = egui::Rect::from_min_max(
                        rect.min + egui::vec2(leaf.x0, leaf.y0),
                        rect.min + egui::vec2(leaf.x1, leaf.y1),
                    );
                    painter.rect_stroke(
                        r,
                        0.0,
                        Stroke::new(1.5_f32, Color32::from_rgb(255, 240, 80)),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }

        // Selected node(s) highlight frames (supports multi-selection; capped at 500 frames)
        if !self.selection.is_empty() {
            let [hr, hg, hb] = self.settings.treemap.highlight_color;
            let stroke = Stroke::new(2.2_f32, Color32::from_rgb(hr, hg, hb));
            let painter = ui.painter();
            for &sel in self.selection.items().iter().take(500) {
                if let Some((bx0, by0, bx1, by1)) = self.treemap.bounds_of_node(&self.tree, sel) {
                    let r = egui::Rect::from_min_max(
                        rect.min + egui::vec2(bx0, by0),
                        rect.min + egui::vec2(bx1, by1),
                    );
                    painter.rect_stroke(r, 1.0, stroke, egui::StrokeKind::Inside);
                }
            }
        }

        // Hover hit-test & interactions
        let hovered_leaf = response.hover_pos().and_then(|pos| {
            let lx = pos.x - rect.min.x;
            let ly = pos.y - rect.min.y;
            self.treemap.hit_test(lx, ly).copied()
        });

        if let Some(leaf) = hovered_leaf {
            let hr = egui::Rect::from_min_max(
                rect.min + egui::vec2(leaf.x0, leaf.y0),
                rect.min + egui::vec2(leaf.x1, leaf.y1),
            );
            ui.painter().rect_stroke(
                hr,
                0.0,
                Stroke::new(1.5_f32, Color32::from_rgb(255, 255, 140)),
                egui::StrokeKind::Inside,
            );

            match leaf.item {
                SpecialTile::Normal(nid) => {
                    let full_p = self.tree.resolve_path(nid);
                    let sz = self.tree.size_for_metric(nid, self.settings.size_metric);
                    let tot = self.tree.size_for_metric(NodeId::ROOT, self.settings.size_metric).max(1);
                    let pct = (sz as f64 / tot as f64) * 100.0;
                    let cat_str = if self.tree.is_dir(nid) {
                        format!("📁 {}", t("Carpeta", "Folder"))
                    } else {
                        let cat = FileCategory::from_extension(self.tree.ext(nid));
                        format!("{} {}", cat.icon(), cat.label())
                    };
                    self.hover_status_text = format!(
                        "{}  ({} · {pct:.2}% total · {cat_str})",
                        full_p.display(),
                        format_bytes(sz),
                    );

                    let mtime_str = format_timestamp_opts(
                        self.tree.mtimes[nid.index()],
                        self.settings.show_timestamp_seconds,
                    );
                    response.clone().on_hover_ui_at_pointer(|ui| {
                        ui.strong(self.tree.name(nid));
                        ui.label(RichText::new(full_p.to_string_lossy().as_ref()).small().monospace());
                        ui.label(format!(
                            "{}: {} ({pct:.2}%) · {cat_str}",
                            t("Tamaño", "Size"),
                            format_bytes(sz),
                        ));
                        ui.label(
                            RichText::new(format!("{}: {mtime_str}", t("Modificado", "Modified")))
                                .small()
                                .weak(),
                        );
                    });

                    if response.clicked() {
                        let mods = ui.input(|i| i.modifiers);
                        if mods.ctrl || mods.command {
                            self.selection.toggle(nid);
                            self.right_panel.active_tab = RightTab::Inspector;
                        } else {
                            self.select_node_and_sync(nid, true);
                        }
                    }
                    if response.secondary_clicked() {
                        self.selection.ensure_for_context_menu(nid);
                    }
                    if response.double_clicked() {
                        let zoom_target = if self.tree.is_dir(nid) { nid } else { leaf.parent_dir };
                        if zoom_target.is_some() && zoom_target != self.treemap.root_node {
                            self.treemap.root_node = zoom_target;
                            self.treemap.invalidate();
                        }
                    }

                    let mut tm_action: Option<NodeAction> = None;
                    let targets = self.selection.context_targets_for(nid);
                    response.context_menu(|ui| {
                        tm_action = show_node_actions_menu(
                            ui,
                            &self.tree,
                            &targets,
                            None,
                            &self.settings,
                            self.read_only_locked,
                        );
                    });
                    if let Some(act) = tm_action {
                        self.handle_node_action(act, ctx);
                    }
                }
                SpecialTile::FreeSpace(_, b) => {
                    self.hover_status_text = format!(
                        "{}: {}",
                        t("<Espacio Libre>", "<Free Space>"),
                        format_bytes(b)
                    );
                    response.on_hover_text(format!(
                        "🟢 {}: {}",
                        t("<Espacio Libre>", "<Free Space>"),
                        format_bytes(b)
                    ));
                }
                SpecialTile::UnknownSpace(_, b) => {
                    self.hover_status_text = format!(
                        "{}: {}",
                        t("<Espacio Desconocido>", "<Unknown Space>"),
                        format_bytes(b)
                    );
                    response.on_hover_text(format!(
                        "🟡 {}: {}\n{}",
                        t("<Espacio Desconocido>", "<Unknown Space>"),
                        format_bytes(b),
                        t(
                            "(Shadow Copies VSS, $MFT, System Volume Information o carpetas sin permisos)",
                            "(VSS Shadow Copies, $MFT, System Volume Information, or restricted folders)"
                        )
                    ));
                }
            }
        }
    }

    /// Render the main 9-Tab Workspace (All 7 WinDirStat 2.10 Tabs + MediaStat Uncompressed + SysAdmin Audit).
    fn render_main_tabs_contents(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        if let Some((msg, is_err, ts)) = &self.toast {
            if ts.elapsed().as_secs() < 8 {
                let col = if *is_err {
                    Color32::from_rgb(230, 80, 80)
                } else {
                    Color32::from_rgb(76, 195, 130)
                };
                egui::Frame::NONE
                    .fill(Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 35))
                    .stroke(Stroke::new(1.0_f32, col))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        ui.label(RichText::new(msg).color(col));
                    });
                ui.add_space(3.0);
            }
        }

        let sum = self.tree.summary();
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.main_tab, MainTab::DirectoryTree, t("📁 Árbol", "📁 Tree"));
            ui.selectable_value(
                &mut self.main_tab,
                MainTab::LargestFiles,
                t("🏆 Más Grandes", "🏆 Largest"),
            );
            ui.selectable_value(
                &mut self.main_tab,
                MainTab::Duplicates,
                format!(
                    "{} ({})",
                    t("🧬 Duplicados", "🧬 Duplicates"),
                    format_count(sum.duplicate_groups)
                ),
            );
            ui.selectable_value(&mut self.main_tab, MainTab::Search, t("🔍 Buscar", "🔍 Search"));
            let watch_lbl = if self.watcher_view.is_running() {
                "📡 Watcher ●"
            } else {
                "📡 Watcher"
            };
            ui.selectable_value(&mut self.main_tab, MainTab::Watcher, watch_lbl);
            ui.selectable_value(
                &mut self.main_tab,
                MainTab::Permissions,
                t("🔐 Permisos", "🔐 Permissions"),
            );
            ui.selectable_value(
                &mut self.main_tab,
                MainTab::StorageAnalytics,
                t("📊 Costes", "📊 Costs"),
            );
            let issue_cnt = self.tree.issues.len();
            ui.selectable_value(
                &mut self.main_tab,
                MainTab::SysAdminAudit,
                format!(
                    "{} ({})",
                    t("⚠ Auditoría", "⚠ Audit"),
                    format_count(issue_cnt as u64)
                ),
            );
        });
        ui.separator();

        // Watcher works even before a scan is loaded!
        if self.main_tab == MainTab::Watcher {
            self.watcher_view.show(ui, ctx, &self.tree);
            return;
        }

        if self.tree.is_empty() && !self.scanning {
            ui.vertical_centered(|ui| {
                ui.add_space(55.0);
                ui.heading(t(
                    "MediaStat v0.3 — Analizador de Espacio en Disco estilo WinDirStat + Búsqueda Avanzada & SysAdmin",
                    "MediaStat v0.3 — WinDirStat-Style Disk Space Analyzer + Advanced Search & SysAdmin",
                ));
                ui.add_space(8.0);
                ui.label(t(
                    "Motor nativo NtQueryDirectoryFile / NTFS $MFT multihilo, deduplicación de Hardlinks NTFS, Cushion 3D exacto con 18 colores, 12 layouts, ventanas libres y buscador avanzado.",
                    "Multi-threaded native NtQueryDirectoryFile / NTFS $MFT engine, NTFS hardlink deduplication, exact 18-color 3D Cushion, 12 layouts, free OS windows, and advanced search.",
                ));
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(t(
                            "🖴 Seleccionar Unidades / Carpetas...",
                            "🖴 Select Drives / Folders...",
                        ))
                        .clicked()
                    {
                        self.target_selector.open = true;
                        bring_native_window_to_front(ctx, "target_selector_window");
                    }
                    if ui
                        .button(t(
                            "📂 Abrir Snapshot (.mstat / .csv)",
                            "📂 Open Snapshot (.mstat / .csv)",
                        ))
                        .clicked()
                    {
                        self.load_snapshot_dialog(ctx.clone());
                    }
                });
            });
            return;
        }

        // Contextual Selection Action Bar (shows live summary + Copy + Full Actions Menu for 1 or N items)
        if !self.selection.is_empty()
            && !matches!(
                self.main_tab,
                MainTab::Watcher | MainTab::StorageAnalytics | MainTab::SysAdminAudit
            )
        {
            let stats = self.selection.compute_stats(&self.tree);
            if stats.total_items > 0 {
                let targets = self.selection.items().to_vec();
                let visible_for_menu = self.current_tab_visible_nodes();
                let mut bar_action: Option<NodeAction> = None;
                let accent = if stats.total_items > 1 {
                    Color32::from_rgb(76, 166, 255)
                } else {
                    Color32::from_rgb(120, 155, 190)
                };
                egui::Frame::NONE
                    .fill(Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 22))
                    .stroke(Stroke::new(
                        1.0_f32,
                        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 110),
                    ))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if let Some(single_id) = stats.single_node {
                                let icon = if self.tree.is_dir(single_id) { "📁" } else { "📄" };
                                ui.label(
                                    RichText::new(format!(
                                        "{icon} {} ({})",
                                        self.tree.name(single_id),
                                        format_bytes(stats.logical_bytes)
                                    ))
                                    .small()
                                    .strong(),
                                );
                            } else {
                                ui.label(
                                    RichText::new(format!(
                                        "✓ {} {} ({} {}, {} {} · {} {} / {} {})",
                                        format_count(stats.total_items as u64),
                                        t("seleccionados", "selected"),
                                        format_count(stats.file_count as u64),
                                        t("fich.", "files"),
                                        format_count(stats.dir_count as u64),
                                        t("carp.", "dirs"),
                                        format_bytes(stats.logical_bytes),
                                        t("lógico", "logical"),
                                        format_bytes(stats.allocated_bytes),
                                        t("en disco", "on disk"),
                                    ))
                                    .small()
                                    .strong()
                                    .color(Color32::from_rgb(130, 205, 255)),
                                );
                            }

                            ui.separator();
                            if ui
                                .small_button(if stats.total_items > 1 {
                                    format!(
                                        "📋 {} ({})",
                                        t("Copiar rutas", "Copy paths"),
                                        stats.total_items
                                    )
                                } else {
                                    t("📋 Copiar ruta", "📋 Copy path").to_string()
                                })
                                .clicked()
                            {
                                bar_action = Some(NodeAction::CopyPaths(targets.clone()));
                            }

                            ui.menu_button(
                                RichText::new(t("⚡ Acciones ▾", "⚡ Actions ▾")).small().strong(),
                                |ui| {
                                    if let Some(act) = show_node_actions_menu(
                                        ui,
                                        &self.tree,
                                        &targets,
                                        Some(MenuScope {
                                            visible_nodes: &visible_for_menu,
                                            allow_tree_ops: self.main_tab == MainTab::DirectoryTree,
                                        }),
                                        &self.settings,
                                        self.read_only_locked,
                                    ) {
                                        bar_action = Some(act);
                                    }
                                },
                            );

                            if ui
                                .small_button(t("📤 Exportar", "📤 Export"))
                                .clicked()
                            {
                                bar_action = Some(NodeAction::ExportNodes(targets.clone()));
                            }

                            if stats.total_items > 1
                                && ui
                                    .add_enabled(
                                        !self.read_only_locked,
                                        egui::Button::new(
                                            RichText::new(t("🗑 Papelera", "🗑 Trash")).small(),
                                        ),
                                    )
                                    .on_disabled_hover_text(t(
                                        "Desbloquea 🔒 Solo Lectura en la barra superior",
                                        "Unlock 🔒 Read-Only in the top toolbar",
                                    ))
                                    .clicked()
                            {
                                bar_action = Some(NodeAction::TrashNodes(targets.clone()));
                            }

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui
                                    .small_button(t("✕ Limpiar", "✕ Clear"))
                                    .on_hover_text(t("Limpiar selección (Esc)", "Clear selection (Esc)"))
                                    .clicked()
                                {
                                    bar_action = Some(NodeAction::ClearSelection);
                                }
                            });
                        });
                    });
                ui.add_space(3.0);
                if let Some(act) = bar_action {
                    self.handle_node_action(act, ctx);
                }
            }
        }

        let prev_primary = self.selection.primary();
        let prev_len = self.selection.len();
        let mut pending_node_action: Option<NodeAction> = None;

        match self.main_tab {
            MainTab::DirectoryTree => {
                pending_node_action = self.tree_view.show(
                    ui,
                    &self.tree,
                    self.tree_gen,
                    &mut self.selection,
                    &self.settings,
                    self.read_only_locked,
                );
            }
            MainTab::LargestFiles => {
                pending_node_action = self.largest_files.show(
                    ui,
                    &self.tree,
                    self.tree_gen,
                    &mut self.selection,
                    &self.settings,
                    self.read_only_locked,
                );
            }
            MainTab::Duplicates => {
                pending_node_action = show_duplicates_view(
                    ui,
                    &self.tree,
                    &mut self.selection,
                    &self.settings,
                    self.read_only_locked,
                );
            }
            MainTab::Search => {
                pending_node_action = self.search_view.show(
                    ui,
                    &self.tree,
                    &mut self.selection,
                    &mut self.settings,
                    self.read_only_locked,
                );
            }
            MainTab::Watcher => {}
            MainTab::Permissions => {
                if let Some(act) = self.permissions_view.show(
                    ui,
                    &self.tree,
                    &mut self.selection,
                    &self.settings,
                    self.read_only_locked,
                ) {
                    match act {
                        PermsAction::ResolveOwners(scope) => {
                            self.start_resolve_owners(scope, ctx.clone());
                        }
                        PermsAction::ScanDacl { include_inherited } => {
                            self.start_scan_permissions(include_inherited, ctx.clone());
                        }
                        PermsAction::ExportPermissionsCsv => {
                            self.export_format = ExportFormat::PermissionsCsv;
                            self.export_open = true;
                        }
                        PermsAction::Node(na) => {
                            pending_node_action = Some(na);
                        }
                    }
                }
            }
            MainTab::StorageAnalytics => {
                show_storage_analytics(ui, &self.tree, &mut self.settings);
            }
            MainTab::SysAdminAudit => {
                self.audit_view.show(ui, &self.tree);
            }
        }

        if (self.selection.primary() != prev_primary || self.selection.len() != prev_len)
            && !self.selection.is_empty()
        {
            self.right_panel.active_tab = RightTab::Inspector;
            if self.main_tab != MainTab::DirectoryTree && self.selection.len() == 1 {
                if let Some(nid) = self.selection.primary() {
                    self.tree_view.reveal_and_scroll(&self.tree, nid);
                }
            }
        }
        if let Some(act) = pending_node_action {
            self.handle_node_action(act, ctx);
        }
    }

    fn render_export_modal(&mut self, ctx: &egui::Context) {
        if !self.export_open {
            return;
        }
        let mut keep = self.export_open;
        let mut trigger_export = false;

        show_native_window(
            ctx,
            NativeWindowConfig {
                id: "export_modal_window",
                title: t(
                    "MediaStat — Exportar Resultados (CSV, JSON, HTML o Scripts)",
                    "MediaStat — Export Results (CSV, JSON, HTML or Scripts)",
                ),
                default_size: [460.0, 390.0],
                min_size: [380.0, 320.0],
                resizable: true,
                always_on_top: false,
            },
            &mut keep,
            |_vp_ctx, ui, close_now| {
                ui.label(t("Formato de salida:", "Output format:"));
                ui.add_space(4.0);
                for fmt in [
                    ExportFormat::ExecutiveHtml,
                    ExportFormat::Csv,
                    ExportFormat::Json,
                    ExportFormat::Jsonl,
                    ExportFormat::PermissionsCsv,
                    ExportFormat::DuplicatesCsv,
                    ExportFormat::PowerShellQuarantine,
                ] {
                    ui.radio_value(&mut self.export_format, fmt, fmt.label());
                }
                ui.add_space(6.0);
                if self.export_custom_nodes.is_none() {
                    ui.checkbox(
                        &mut self.export_top_5000,
                        t(
                            "Limitar a los Top 5.000 archivos más grandes (desmarca para exportar todos)",
                            "Limit to Top 5,000 largest files (uncheck to export all files)",
                        ),
                    );
                } else {
                    ui.label(
                        RichText::new(format!(
                            "{} {} {}",
                            t("Exportando", "Exporting"),
                            self.export_custom_nodes.as_ref().map(|v| v.len()).unwrap_or(0),
                            t(
                                "elementos seleccionados / filtrados",
                                "selected / filtered items"
                            )
                        ))
                        .small()
                        .color(Color32::from_rgb(130, 195, 240)),
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(t(
                            "💾 Elegir destino y exportar...",
                            "💾 Choose destination & export...",
                        ))
                        .clicked()
                    {
                        trigger_export = true;
                    }
                    if ui.button(t("Cerrar", "Close")).clicked() {
                        *close_now = true;
                    }
                });
            },
        );

        // Run native OS save dialog safely outside the child viewport callback
        if trigger_export {
            let nodes = if let Some(custom) = &self.export_custom_nodes {
                custom.clone()
            } else if self.export_top_5000 {
                self.tree.largest_files(5_000, self.settings.size_metric)
            } else {
                self.tree.all_files_sorted(self.settings.size_metric)
            };
            let ext = self.export_format.extension();
            if let Some(out) = rfd::FileDialog::new()
                .set_file_name(format!(
                    "{}.{ext}",
                    t("mediastat_auditoria", "mediastat_audit")
                ))
                .add_filter(t("Exportación MediaStat", "MediaStat Export"), &[ext])
                .save_file()
            {
                match export_nodes_with_cost(
                    &self.tree,
                    &nodes,
                    self.export_format,
                    &self.settings.storage_tiers,
                    self.settings.default_cost_per_tib_monthly,
                    &self.settings.currency_symbol,
                    &out,
                ) {
                    Ok(n) => {
                        self.set_toast(
                            format!(
                                "{} ({} {}) {} {}",
                                t("Exportado", "Exported"),
                                format_count(n as u64),
                                t("registros", "records"),
                                t("en", "to"),
                                out.display()
                            ),
                            false,
                        );
                        if self.export_format == ExportFormat::ExecutiveHtml {
                            open_with_default_app(&out);
                        }
                        keep = false;
                    }
                    Err(e) => self.set_toast(
                        format!("{}: {e}", t("Error exportando", "Export error")),
                        true,
                    ),
                }
            }
        }

        self.export_open = keep;
    }

    fn render_confirm_modal(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_confirm.take() else {
            return;
        };
        let mut keep_pending = Some(pending);
        let mut open = true;
        let mut confirmed = false;
        let mut cancelled = false;

        show_native_window(
            ctx,
            NativeWindowConfig {
                id: "confirm_modal_window",
                title: t(
                    "⚠ Confirmación de Acción SysAdmin",
                    "⚠ SysAdmin Action Confirmation",
                ),
                default_size: [580.0, 290.0],
                min_size: [440.0, 200.0],
                resizable: true,
                always_on_top: true,
            },
            &mut open,
            |_vp_ctx, ui, close_now| {
                match keep_pending.as_ref().unwrap() {
                    PendingConfirm::Trash(items, total_sz) => {
                        if items.len() == 1 {
                            ui.label(t(
                                "¿Enviar este elemento a la Papelera de Reciclaje de Windows?",
                                "Move this item to the Windows Recycle Bin?",
                            ));
                        } else {
                            ui.label(
                                RichText::new(format!(
                                    "{} {} {}?",
                                    t("¿Enviar", "Move"),
                                    format_count(items.len() as u64),
                                    t(
                                        "elementos a la Papelera de Reciclaje de Windows",
                                        "items to the Windows Recycle Bin"
                                    )
                                ))
                                .strong(),
                            );
                        }
                        ui.add_space(4.0);
                        render_confirm_paths_preview(ui, items.iter().map(|(_, p)| p.as_path()));
                        ui.label(format!(
                            "{}: {}",
                            t("Tamaño total", "Total size"),
                            format_bytes(*total_sz)
                        ));
                    }
                    PendingConfirm::DeletePermanent(items, total_sz) => {
                        ui.label(
                            RichText::new(if items.len() == 1 {
                                t(
                                    "¡ATENCIÓN! Esta acción ELIMINARÁ PERMANENTEMENTE el elemento sin pasar por la Papelera.",
                                    "WARNING! This will PERMANENTLY DELETE the item bypassing the Recycle Bin.",
                                )
                                .to_string()
                            } else {
                                format!(
                                    "{} {} {}",
                                    t(
                                        "¡ATENCIÓN! Esta acción ELIMINARÁ PERMANENTEMENTE",
                                        "WARNING! This will PERMANENTLY DELETE"
                                    ),
                                    format_count(items.len() as u64),
                                    t(
                                        "elementos sin pasar por la Papelera.",
                                        "items bypassing the Recycle Bin."
                                    )
                                )
                            })
                            .strong()
                            .color(Color32::from_rgb(240, 80, 80)),
                        );
                        ui.add_space(4.0);
                        render_confirm_paths_preview(ui, items.iter().map(|(_, p, _)| p.as_path()));
                        let dirs_cnt = items.iter().filter(|(_, _, d)| *d).count();
                        let files_cnt = items.len() - dirs_cnt;
                        ui.label(format!(
                            "{} {}, {} {} · {}: {}",
                            format_count(files_cnt as u64),
                            t("archivo(s)", "file(s)"),
                            format_count(dirs_cnt as u64),
                            t("carpeta(s) recursiva(s)", "recursive folder(s)"),
                            t("Tamaño total", "Total size"),
                            format_bytes(*total_sz)
                        ));
                    }
                    PendingConfirm::EmptyRecycleBin(cnt, bytes) => {
                        ui.label(format!(
                            "{} ({} {}, {})?",
                            t(
                                "¿Vaciar permanentemente la Papelera de Reciclaje",
                                "Permanently empty the Recycle Bin"
                            ),
                            format_count(*cnt),
                            t("elementos", "items"),
                            format_bytes(*bytes)
                        ));
                    }
                    PendingConfirm::UserCleanup(slot, items) => {
                        if let Some(act) = self.settings.user_cleanups.get(*slot) {
                            ui.label(format!(
                                "{} '{}' ({} {})?",
                                t("¿Ejecutar acción", "Execute action"),
                                act.title,
                                format_count(items.len() as u64),
                                t("elemento(s)", "item(s)")
                            ));
                            if let Some((_, first_p)) = items.first() {
                                let unc = resolve_to_unc_path(first_p);
                                ui.monospace(act.expand_command_with_unc(first_p, Some(&unc)));
                            }
                            if items.len() > 1 {
                                render_confirm_paths_preview(
                                    ui,
                                    items.iter().map(|(_, p)| p.as_path()),
                                );
                            }
                        }
                    }
                    PendingConfirm::Sparsify(items) => {
                        ui.label(format!(
                            "{} ({} {})?",
                            t(
                                "¿Marcar como NTFS Sparse (fsutil sparse setflag)",
                                "Mark as NTFS Sparse (fsutil sparse setflag)"
                            ),
                            format_count(items.len() as u64),
                            t("fichero(s)", "file(s)")
                        ));
                        render_confirm_paths_preview(ui, items.iter().map(|(_, p)| p.as_path()));
                    }
                    PendingConfirm::SyncFolderDates(items) => {
                        ui.label(format!(
                            "{} ({} {})?",
                            t(
                                "¿Actualizar recursivamente las fechas de modificación de carpetas para que coincidan con su archivo más reciente",
                                "Recursively update folder modified dates to match their newest file"
                            ),
                            format_count(items.len() as u64),
                            t("carpeta(s)", "folder(s)")
                        ));
                        render_confirm_paths_preview(ui, items.iter().map(|(_, p)| p.as_path()));
                    }
                    PendingConfirm::DisableHibernation => {
                        ui.label(
                            RichText::new(t(
                                "¿Deshabilitar la hibernación de Windows (powercfg /h off)?",
                                "Disable Windows hibernation (powercfg /h off)?",
                            ))
                            .strong()
                            .color(Color32::from_rgb(245, 185, 75)),
                        );
                        ui.label(t(
                            "Esto eliminará C:\\hiberfil.sys y desactivará el Inicio Rápido (Fast Startup).",
                            "This will remove C:\\hiberfil.sys and disable Fast Startup.",
                        ));
                    }
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(
                            RichText::new(t("✓ Confirmar y Ejecutar", "✓ Confirm & Execute"))
                                .strong()
                                .color(Color32::from_rgb(245, 110, 80)),
                        )
                        .clicked()
                    {
                        confirmed = true;
                        *close_now = true;
                    }
                    if ui.button(t("Cancelar", "Cancel")).clicked() {
                        cancelled = true;
                        *close_now = true;
                    }
                });
            },
        );

        if confirmed {
            if let Some(act) = keep_pending.take() {
                match act {
                    PendingConfirm::Trash(items, _) => {
                        let paths: Vec<PathBuf> = items.iter().map(|(_, p)| p.clone()).collect();
                        let res = move_multiple_to_recycle_bin(&paths);
                        let mut removed = 0usize;
                        for (nid, p) in &items {
                            if res.is_ok() || !p.exists() {
                                self.tree.mark_node_deleted(*nid);
                                removed += 1;
                            }
                        }
                        if removed > 0 {
                            self.invalidate_all_views();
                        }
                        self.recycle_bin_stats = query_recycle_bin();
                        match res {
                            Ok(()) => {
                                let msg = if items.len() == 1 {
                                    format!(
                                        "{}: {}",
                                        t("Enviado a Papelera", "Moved to Recycle Bin"),
                                        items[0].1.display()
                                    )
                                } else {
                                    format!(
                                        "{}: {} {}",
                                        t("Enviados a Papelera", "Moved to Recycle Bin"),
                                        format_count(removed as u64),
                                        t("elementos", "items")
                                    )
                                };
                                self.set_toast(msg, false);
                            }
                            Err(e) => self.set_toast(
                                format!(
                                    "{}: {e} ({removed}/{})",
                                    t("Error Papelera", "Recycle Bin error"),
                                    items.len()
                                ),
                                true,
                            ),
                        }
                    }
                    PendingConfirm::DeletePermanent(items, _) => {
                        let mut deleted = 0usize;
                        let mut last_err = None;
                        for (nid, path, _) in &items {
                            match delete_permanently(path) {
                                Ok(()) => {
                                    self.tree.mark_node_deleted(*nid);
                                    deleted += 1;
                                }
                                Err(e) => last_err = Some(e),
                            }
                        }
                        if deleted > 0 {
                            self.invalidate_all_views();
                        }
                        if let Some(e) = last_err {
                            self.set_toast(
                                format!(
                                    "{}: {e} ({deleted}/{})",
                                    t("Error eliminando", "Delete error"),
                                    items.len()
                                ),
                                true,
                            );
                        } else if items.len() == 1 {
                            self.set_toast(
                                format!(
                                    "{}: {}",
                                    t("Eliminado permanentemente", "Permanently deleted"),
                                    items[0].1.display()
                                ),
                                false,
                            );
                        } else {
                            self.set_toast(
                                format!(
                                    "{}: {} {}",
                                    t("Eliminados permanentemente", "Permanently deleted"),
                                    format_count(deleted as u64),
                                    t("elementos", "items")
                                ),
                                false,
                            );
                        }
                    }
                    PendingConfirm::EmptyRecycleBin(_, _) => match empty_recycle_bin() {
                        Ok(()) => {
                            self.recycle_bin_stats = query_recycle_bin();
                            self.set_toast(
                                t(
                                    "Papelera vaciada correctamente",
                                    "Recycle Bin emptied successfully",
                                ),
                                false,
                            );
                        }
                        Err(e) => self.set_toast(
                            format!("{}: {e}", t("Error vaciando Papelera", "Error emptying Recycle Bin")),
                            true,
                        ),
                    },
                    PendingConfirm::UserCleanup(slot, items) => {
                        if let Some(uact) = self.settings.user_cleanups.get(slot).cloned() {
                            let mut ok = 0usize;
                            let mut last_err = None;
                            for (_, path) in &items {
                                match execute_user_cleanup(&uact, path) {
                                    Ok(()) => ok += 1,
                                    Err(e) => last_err = Some(e),
                                }
                            }
                            if let Some(e) = last_err {
                                self.set_toast(
                                    format!(
                                        "{}: {e} ({ok}/{})",
                                        t("Error limpieza", "Cleanup error"),
                                        items.len()
                                    ),
                                    true,
                                );
                            } else {
                                self.set_toast(
                                    format!(
                                        "{} '{}' ({ok})",
                                        t("Ejecutado", "Executed"),
                                        uact.title
                                    ),
                                    false,
                                );
                                if items.len() == 1 {
                                    self.apply_cleanup_refresh(uact.refresh_policy, items[0].0, ctx);
                                }
                            }
                        }
                    }
                    PendingConfirm::Sparsify(items) => {
                        let mut ok = 0usize;
                        let mut last_err = None;
                        for (_, path) in &items {
                            match sparsify_file(path) {
                                Ok(()) => ok += 1,
                                Err(e) => last_err = Some(e),
                            }
                        }
                        if let Some(e) = last_err {
                            self.set_toast(
                                format!("{}: {e} ({ok}/{})", t("Error sparsificando", "Sparsify error"), items.len()),
                                true,
                            );
                        } else {
                            self.set_toast(
                                format!(
                                    "{} ({ok})",
                                    t("Marcado como NTFS Sparse", "Marked as NTFS Sparse")
                                ),
                                false,
                            );
                            if items.len() == 1 {
                                self.start_rescan_subtree(items[0].0, ctx.clone());
                            }
                        }
                    }
                    PendingConfirm::SyncFolderDates(items) => {
                        let mut total_cnt = 0usize;
                        let mut last_err = None;
                        for (_, path) in &items {
                            match sync_folder_dates_to_newest(path) {
                                Ok(cnt) => total_cnt += cnt,
                                Err(e) => last_err = Some(e),
                            }
                        }
                        if let Some(e) = last_err {
                            self.set_toast(
                                format!("{}: {e}", t("Error sincronizando fechas", "Error syncing dates")),
                                true,
                            );
                        } else {
                            self.set_toast(
                                format!(
                                    "{} {total_cnt} {}",
                                    t("🕒 Fechas sincronizadas en", "🕒 Dates synced across"),
                                    t("carpetas", "folders")
                                ),
                                false,
                            );
                            if items.len() == 1 {
                                self.start_rescan_subtree(items[0].0, ctx.clone());
                            }
                        }
                    }
                    PendingConfirm::DisableHibernation => {
                        launch_windows_tool(WindowsSysTool::DisableHibernation);
                        self.set_toast(
                            t(
                                "Ejecutado powercfg /h off (elevado)",
                                "Executed powercfg /h off (elevated)",
                            ),
                            false,
                        );
                    }
                }
            }
        } else if cancelled || !open {
            keep_pending = None;
        }

        self.pending_confirm = keep_pending;
    }

    fn render_help_modals(&mut self, ctx: &egui::Context) {
        if self.shortcuts_open {
            let mut open = self.shortcuts_open;
            show_native_window(
                ctx,
                NativeWindowConfig {
                    id: "shortcuts_modal_window",
                    title: t(
                        "MediaStat — ⌨ Atajos de Teclado (Paridad WinDirStat)",
                        "MediaStat — ⌨ Keyboard Shortcuts (WinDirStat Parity)",
                    ),
                    default_size: [600.0, 440.0],
                    min_size: [460.0, 320.0],
                    resizable: true,
                    always_on_top: false,
                },
                &mut open,
                |_vp_ctx, ui, _close_now| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        egui::Grid::new("shortcuts_grid").spacing([18.0, 4.0]).show(ui, |ui| {
                            for (k, desc) in [
                                ("Ctrl + O", t("Seleccionar unidades / carpetas / UNC", "Select drives / folders / UNC")),
                                ("Ctrl + S", t("Guardar snapshot .mstat", "Save .mstat snapshot")),
                                ("F5", t("Refrescar solo la carpeta seleccionada", "Refresh selected folder only")),
                                ("Shift + F5", t("Refrescar todo el escaneo", "Refresh entire scan")),
                                ("F6", t("Mostrar / Ocultar <Espacio Libre>", "Show / Hide <Free Space>")),
                                ("F7", t("Mostrar / Ocultar <Espacio Desconocido>", "Show / Hide <Unknown Space>")),
                                ("F8", t("Mostrar / Ocultar Extensiones / Inspector", "Show / Hide Extensions / Inspector")),
                                ("F9", t("Mostrar / Ocultar Visualización (Treemap/Sunburst/Flame)", "Show / Hide Visualization (Treemap/Sunburst/Flame)")),
                                ("Ctrl + F", t("Abrir Buscador Global (Regex / Filtros)", "Open Global Search (Regex / Filters)")),
                                ("Ctrl + A", t("Seleccionar todo en la vista activa", "Select all in the active view")),
                                ("Ctrl + Clic / Shift + Clic", t("Añadir/quitar o seleccionar rango de ficheros/carpetas", "Toggle or range-select multiple files/folders")),
                                ("Ctrl + E", t("Mostrar seleccionado(s) en el Explorador", "Reveal selected item(s) in Explorer")),
                                ("Ctrl + C", t("Copiar ruta(s) completa(s) seleccionada(s)", "Copy selected full path(s)")),
                                ("Alt + Enter", t("Propiedades de Windows", "Windows Properties")),
                                ("Del / Shift+Del", t("Enviar seleccionado(s) a Papelera / Borrado permanente (requiere 🔓)", "Move selected item(s) to Recycle Bin / Permanent delete (requires 🔓)")),
                                ("Ctrl + 0..6", t("Acciones SMB rápidas de Solo Lectura (icacls, dir /R, PS, SHA-256, UNC, compact /q, robocopy /L)", "Read-Only SMB quick actions (icacls, dir /R, PS, SHA-256, UNC, compact /q, robocopy /L)")),
                                ("Ctrl + 7..9", t("Acciones SMB de escritura (compact /c, Unblock-File, Cuarentena; requieren 🔓)", "Write SMB actions (compact /c, Unblock-File, Quarantine; require 🔓)")),
                                ("Ctrl + ↑ / ↓", t("Seleccionar carpeta padre / Reseleccionar hijo", "Select parent folder / Reselect child")),
                                ("Ctrl + ← / →", t("Ir a primera subcarpeta / primer archivo más grande", "Jump to largest subfolder / largest file")),
                                ("Backspace / Esc", t("Subir un nivel / Restablecer Zoom en el Mapa", "Zoom out one level / Reset map zoom")),
                            ] {
                                ui.strong(k);
                                ui.label(desc);
                                ui.end_row();
                            }
                        });
                    });
                },
            );
            self.shortcuts_open = open;
        }

        if self.about_open {
            let mut open = self.about_open;
            show_native_window(
                ctx,
                NativeWindowConfig {
                    id: "about_modal_window",
                    title: t("Acerca de MediaStat v0.3", "About MediaStat v0.3"),
                    default_size: [520.0, 185.0],
                    min_size: [400.0, 150.0],
                    resizable: true,
                    always_on_top: false,
                },
                &mut open,
                |_vp_ctx, ui, _close_now| {
                    ui.heading("MediaStat v0.3");
                    ui.label(t(
                        "Analizador de espacio en disco estilo WinDirStat (NtQueryDirectoryFile / NTFS $MFT multihilo, Hardlinks NTFS, Cushion 3D exacto con 18 colores + XYSlider, 12 layouts, ventanas libres) + Búsqueda Avanzada por categorías/extensiones + SysAdmin.",
                        "WinDirStat-style disk space analyzer (multi-threaded NtQueryDirectoryFile / NTFS $MFT, NTFS Hardlinks, exact 18-color 3D Cushion + XYSlider, 12 layouts, free OS windows) + Advanced Search by categories/extensions + SysAdmin.",
                    ));
                    ui.add_space(6.0);
                    ui.label(t(
                        "100% Rust nativo · egui/eframe + wgpu (Vulkan/DX12) · NtQueryDirectoryFile / $MFT · BLAKE3 · SQLite WAL",
                        "100% native Rust · egui/eframe + wgpu (Vulkan/DX12) · NtQueryDirectoryFile / $MFT · BLAKE3 · SQLite WAL",
                    ));
                },
            );
            self.about_open = open;
        }
    }

    fn render_right_panel(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let mut float_ext = self.settings.float_extensions_panel;
        let act = self.right_panel.show(
            ui,
            ctx,
            &self.tree,
            &mut self.selection,
            &mut self.thumb_loader,
            &self.settings,
            self.read_only_locked,
            &mut float_ext,
        );
        self.settings.float_extensions_panel = float_ext;
        if let Some(act) = act {
            match act {
                RightPanelAction::SearchExtension(ext) => {
                    self.main_tab = MainTab::Search;
                    self.search_view
                        .search_for_extension(&ext, &self.tree, &mut self.settings);
                }
                RightPanelAction::Node(na) => {
                    self.handle_node_action(na, ctx);
                }
            }
        }
    }

    fn render_pane(&mut self, kind: LayoutPaneKind, ui: &mut Ui, ctx: &egui::Context) {
        match kind {
            LayoutPaneKind::AllFiles => self.render_main_tabs_contents(ui, ctx),
            LayoutPaneKind::FileTypes => self.render_right_panel(ui, ctx),
            LayoutPaneKind::Visualization => self.render_visualization_contents(ui, ctx),
        }
    }

    fn render_docked_layout(
        &mut self,
        ctx: &egui::Context,
        show_ext_docked: bool,
        show_viz_docked: bool,
    ) {
        let screen = ctx.screen_rect();
        let layout = self.settings.layout;
        let def = layout.def();
        let tag = format!("{layout:?}");

        let is_docked = |k: LayoutPaneKind| -> bool {
            match k {
                LayoutPaneKind::AllFiles => true,
                LayoutPaneKind::FileTypes => show_ext_docked,
                LayoutPaneKind::Visualization => show_viz_docked,
            }
        };

        let standalone = def.views[0];
        let n0 = def.views[1];
        let n1 = def.views[2];
        let any_nested = is_docked(n0) || is_docked(n1);

        // 1. Outer split: place `standalone` panel on side `1 - def.sub_pane` when both halves have docked panes
        if is_docked(standalone) && any_nested {
            let frac = if def.sub_pane == 1 {
                def.ratio
            } else {
                1.0 - def.ratio
            };
            let id = format!("l_{tag}_outer_{standalone:?}");
            match (def.orientation, def.sub_pane) {
                (LayoutAxis::Horizontal, 0) => {
                    egui::TopBottomPanel::bottom(id)
                        .resizable(true)
                        .default_height((screen.height() * frac).clamp(130.0, 520.0))
                        .min_height(120.0)
                        .show(ctx, |ui| self.render_pane(standalone, ui, ctx));
                }
                (LayoutAxis::Horizontal, _) => {
                    egui::TopBottomPanel::top(id)
                        .resizable(true)
                        .default_height((screen.height() * frac).clamp(130.0, 520.0))
                        .min_height(120.0)
                        .show(ctx, |ui| self.render_pane(standalone, ui, ctx));
                }
                (LayoutAxis::Vertical, 0) => {
                    egui::SidePanel::right(id)
                        .resizable(true)
                        .default_width((screen.width() * frac).clamp(220.0, 820.0))
                        .min_width(200.0)
                        .show(ctx, |ui| self.render_pane(standalone, ui, ctx));
                }
                (LayoutAxis::Vertical, _) => {
                    egui::SidePanel::left(id)
                        .resizable(true)
                        .default_width((screen.width() * frac).clamp(220.0, 820.0))
                        .min_width(200.0)
                        .show(ctx, |ui| self.render_pane(standalone, ui, ctx));
                }
            }
        }

        // 2. Inner split for nested pair `(n0, n1)`
        let avail_w = if def.orientation == LayoutAxis::Vertical && is_docked(standalone) {
            screen.width() * if def.sub_pane == 0 { def.ratio } else { 1.0 - def.ratio }
        } else {
            screen.width()
        };
        let avail_h = if def.orientation == LayoutAxis::Horizontal && is_docked(standalone) {
            screen.height() * if def.sub_pane == 0 { def.ratio } else { 1.0 - def.ratio }
        } else {
            screen.height()
        };

        match (is_docked(n0), is_docked(n1)) {
            (true, true) => {
                // Keep `AllFiles` in `CentralPanel` whenever possible so its table gets full flexible space
                if n0 == LayoutPaneKind::AllFiles {
                    let frac = 1.0 - def.sub_ratio;
                    let id = format!("l_{tag}_inner_{n1:?}");
                    match def.sub_orientation {
                        LayoutAxis::Vertical => {
                            egui::SidePanel::right(id)
                                .resizable(true)
                                .default_width((avail_w * frac).clamp(220.0, 760.0))
                                .min_width(180.0)
                                .show(ctx, |ui| self.render_pane(n1, ui, ctx));
                        }
                        LayoutAxis::Horizontal => {
                            egui::TopBottomPanel::bottom(id)
                                .resizable(true)
                                .default_height((avail_h * frac).clamp(140.0, 480.0))
                                .min_height(120.0)
                                .show(ctx, |ui| self.render_pane(n1, ui, ctx));
                        }
                    }
                    egui::CentralPanel::default().show(ctx, |ui| self.render_pane(n0, ui, ctx));
                } else {
                    let frac = def.sub_ratio;
                    let id = format!("l_{tag}_inner_{n0:?}");
                    match def.sub_orientation {
                        LayoutAxis::Vertical => {
                            egui::SidePanel::left(id)
                                .resizable(true)
                                .default_width((avail_w * frac).clamp(220.0, 760.0))
                                .min_width(180.0)
                                .show(ctx, |ui| self.render_pane(n0, ui, ctx));
                        }
                        LayoutAxis::Horizontal => {
                            egui::TopBottomPanel::top(id)
                                .resizable(true)
                                .default_height((avail_h * frac).clamp(140.0, 480.0))
                                .min_height(120.0)
                                .show(ctx, |ui| self.render_pane(n0, ui, ctx));
                        }
                    }
                    egui::CentralPanel::default().show(ctx, |ui| self.render_pane(n1, ui, ctx));
                }
            }
            (true, false) => {
                egui::CentralPanel::default().show(ctx, |ui| self.render_pane(n0, ui, ctx));
            }
            (false, true) => {
                egui::CentralPanel::default().show(ctx, |ui| self.render_pane(n1, ui, ctx));
            }
            (false, false) => {
                egui::CentralPanel::default()
                    .show(ctx, |ui| self.render_pane(standalone, ui, ctx));
            }
        }
    }
}

impl eframe::App for MediaStatApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.hover_status_text.clear();
        self.poll_background();
        self.handle_shortcuts(ctx);

        // Defer opening the Target Selector until ROOT has painted & become visible on Windows,
        // so the Target Selector always spawns centered and strictly IN FRONT of the main window.
        if self.startup_selector_countdown > 0 {
            self.startup_selector_countdown -= 1;
            if self.startup_selector_countdown == 0 {
                self.target_selector.open = true;
                bring_native_window_to_front(ctx, "target_selector_window");
            } else {
                ctx.request_repaint();
            }
        }

        self.render_menu_and_toolbar(ctx);
        self.render_status_bar(ctx);

        let show_ext = self.settings.show_extensions_panel;
        let show_viz = self.settings.show_visualization;
        let show_ext_docked = show_ext && !self.settings.float_extensions_panel;
        let show_viz_docked = show_viz && !self.settings.float_visualization;

        // Exact 12-Layout WinDirStat Window Layout Engine + Detachable OS Subwindows
        self.render_docked_layout(ctx, show_ext_docked, show_viz_docked);

        // Detached Independent Native OS Subwindows (freely draggable outside MediaStat / multi-monitor!)
        if show_viz && self.settings.float_visualization {
            let mut keep = true;
            show_native_window(
                ctx,
                NativeWindowConfig {
                    id: "floating_visualization_window",
                    title: t(
                        "MediaStat — 🗺 Visualización (Treemap / Sunburst / Flame Graph)",
                        "MediaStat — 🗺 Visualization (Treemap / Sunburst / Flame Graph)",
                    ),
                    default_size: [900.0, 520.0],
                    min_size: [380.0, 240.0],
                    resizable: true,
                    always_on_top: false,
                },
                &mut keep,
                |vp_ctx, ui, _close_now| self.render_visualization_contents(ui, vp_ctx),
            );
            if !keep {
                self.settings.float_visualization = false;
                self.treemap.invalidate();
            }
        }
        if show_ext && self.settings.float_extensions_panel {
            let mut keep = true;
            show_native_window(
                ctx,
                NativeWindowConfig {
                    id: "floating_extensions_window",
                    title: t(
                        "MediaStat — 🎨 Extensiones e Inspector",
                        "MediaStat — 🎨 Extensions & Inspector",
                    ),
                    default_size: [440.0, 620.0],
                    min_size: [280.0, 280.0],
                    resizable: true,
                    always_on_top: false,
                },
                &mut keep,
                |vp_ctx, ui, _close_now| self.render_right_panel(ui, vp_ctx),
            );
            if !keep {
                self.settings.float_extensions_panel = false;
            }
        }

        // Movable Modals & Popups
        let ts_res = self.target_selector.show(ctx, &mut self.settings);
        if ts_res.language_changed {
            self.on_language_changed(ctx);
        }
        if let Some(cfg) = ts_res.start_config {
            self.start_scan(cfg, ctx.clone());
        }

        let prev_float_viz = self.settings.float_visualization;
        if let Some(new_layout) = self.layout_popup.show(
            ctx,
            self.settings.layout,
            &mut self.settings.float_visualization,
            &mut self.settings.float_extensions_panel,
        ) {
            self.settings.layout = new_layout;
            self.treemap.invalidate();
            let _ = self.settings.save();
        }
        if self.settings.float_visualization != prev_float_viz {
            self.treemap.invalidate();
        }

        let opt_res = self.options_dialog.show(ctx, &mut self.settings);
        if opt_res.language_changed {
            self.on_language_changed(ctx);
        }
        if opt_res.treemap_changed {
            self.treemap.invalidate();
        }
        if opt_res.theme_or_scale_changed {
            apply_visual_theme(ctx, self.settings.dark_mode, self.settings.ui_scale_percent);
        }
        if opt_res.saved {
            self.set_toast(t("Configuración guardada", "Settings saved"), false);
        }

        self.diff_modal.show(ctx);
        self.render_export_modal(ctx);
        self.render_confirm_modal(ctx);
        self.render_help_modals(ctx);
    }

    fn on_exit(&mut self) {
        self.watcher_view.stop();
        let _ = self.settings.save();
    }
}

fn render_confirm_paths_preview<'a>(ui: &mut Ui, paths: impl Iterator<Item = &'a std::path::Path>) {
    let all: Vec<&std::path::Path> = paths.collect();
    egui::ScrollArea::vertical().max_height(130.0).show(ui, |ui| {
        for p in all.iter().take(12) {
            ui.monospace(p.to_string_lossy().as_ref());
        }
        if all.len() > 12 {
            ui.label(
                RichText::new(format!(
                    "… {} {} {}",
                    t("y", "and"),
                    format_count((all.len() - 12) as u64),
                    t("elementos más", "more items")
                ))
                .small()
                .weak(),
            );
        }
    });
}

fn collect_subtree_files(tree: &ScanTree, root: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.index() >= tree.len() || tree.is_deleted(n) {
            continue;
        }
        if tree.is_dir(n) {
            for c in tree.children(n) {
                stack.push(c);
            }
        } else {
            out.push(n);
        }
    }
    out
}

/// Iconic animated WinDirStat Pac-Man progress indicator drawn vectorially in `egui`.
fn draw_pacman_widget(ui: &mut Ui, elapsed_secs: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(76.0, 18.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, Color32::from_rgb(18, 22, 28));

    let period = 2.4_f32;
    let t = (elapsed_secs % period) / period;
    let going_right = t < 0.5;
    let frac = if going_right { t * 2.0 } else { (1.0 - t) * 2.0 };
    let px = rect.min.x + 10.0 + frac * (rect.width() - 20.0);
    let cy = rect.center().y;

    // Pellets ahead of Pac-Man
    for i in 0..6 {
        let dot_x = rect.min.x + 10.0 + (i as f32 / 5.0) * (rect.width() - 20.0);
        let visible = if going_right { dot_x > px + 6.0 } else { dot_x < px - 6.0 };
        if visible {
            painter.circle_filled(Pos2::new(dot_x, cy), 1.8, Color32::from_rgb(245, 225, 130));
        }
    }

    // Chomp mouth angle
    let mouth = ((elapsed_secs * 10.0).sin().abs() * 0.65).max(0.08);
    let base_angle = if going_right { 0.0 } else { std::f32::consts::PI };
    let r = 6.5;
    let mut pts = Vec::with_capacity(18);
    pts.push(Pos2::new(px, cy));
    let steps = 14;
    let start_a = base_angle + mouth;
    let end_a = base_angle + std::f32::consts::TAU - mouth;
    for i in 0..=steps {
        let a = start_a + (end_a - start_a) * (i as f32 / steps as f32);
        pts.push(Pos2::new(px + a.cos() * r, cy + a.sin() * r));
    }
    painter.add(egui::Shape::convex_polygon(
        pts,
        Color32::from_rgb(255, 212, 48),
        Stroke::NONE,
    ));
}

fn kpi_chip(ui: &mut egui::Ui, title: &str, value: &str, accent: Color32) {
    egui::Frame::NONE
        .fill(Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 24))
        .stroke(Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 110),
        ))
        .corner_radius(5.0)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{title}:")).small().weak());
                ui.label(RichText::new(value).small().strong().color(accent));
            });
        });
}

fn apply_visual_theme(ctx: &egui::Context, dark: bool, scale_percent: u16) {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    if dark {
        visuals.panel_fill = Color32::from_rgb(22, 25, 31);
        visuals.window_fill = Color32::from_rgb(28, 32, 40);
        visuals.extreme_bg_color = Color32::from_rgb(16, 18, 23);
        visuals.selection.bg_fill = Color32::from_rgb(42, 108, 186);
    }
    ctx.set_visuals(visuals);
    let factor = (scale_percent as f32 / 100.0).clamp(0.85, 2.0);
    ctx.set_zoom_factor(factor);
}
