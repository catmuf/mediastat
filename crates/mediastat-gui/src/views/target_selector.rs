use std::path::PathBuf;

use egui::{Color32, RichText, Ui};
use mediastat_core::{format_bytes, t, AppSettings, Language, ScanConfig};
use mediastat_scanner::{is_process_elevated, DriveInfo};

use crate::shell_actions::relaunch_as_administrator;
use crate::views::{show_native_window, NativeWindowConfig};

pub struct TargetSelectorResult {
    pub start_config: Option<ScanConfig>,
    pub language_changed: bool,
}

pub struct TargetSelectorState {
    pub open: bool,
    pub drives: Vec<DriveInfo>,
    pub selected_drives: Vec<bool>,
    pub custom_paths: Vec<PathBuf>,
    pub unc_input: String,
    pub excluded_input: String,
    pub use_ntfs_mft: bool,
    pub find_duplicates: bool,
    pub sparse_duplicates: bool,
    pub resolve_owners: bool,
    pub scan_threads: usize,
    pub is_elevated: bool,
}

impl TargetSelectorState {
    pub fn new(drives: Vec<DriveInfo>, _settings: &AppSettings) -> Self {
        let len = drives.len();
        let is_elevated = is_process_elevated();
        Self {
            open: true,
            drives,
            selected_drives: vec![false; len],
            custom_paths: Vec::new(),
            unc_input: String::new(),
            excluded_input: "$RECYCLE.BIN, System Volume Information, .git, node_modules".into(),
            use_ntfs_mft: is_elevated,
            find_duplicates: false,
            sparse_duplicates: false,
            resolve_owners: false,
            scan_threads: 0,
            is_elevated,
        }
    }

    pub fn refresh_drives(&mut self, drives: Vec<DriveInfo>) {
        self.selected_drives = vec![false; drives.len()];
        self.drives = drives;
        self.is_elevated = is_process_elevated();
    }

    pub fn build_scan_config(&self, settings: &AppSettings) -> Option<ScanConfig> {
        let mut targets = Vec::new();
        for (i, d) in self.drives.iter().enumerate() {
            if self.selected_drives.get(i).copied().unwrap_or(false) {
                targets.push(d.path.clone());
            }
        }
        for p in &self.custom_paths {
            if !targets.contains(p) {
                targets.push(p.clone());
            }
        }
        if targets.is_empty() {
            return None;
        }
        let excluded_dir_names = self
            .excluded_input
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Some(ScanConfig {
            targets,
            excluded_dir_names,
            filter_config: settings.filter_config.clone(),
            use_ntfs_mft: self.use_ntfs_mft,
            find_duplicates: self.find_duplicates,
            sparse_duplicate_hash: self.sparse_duplicates,
            duplicate_min_size_bytes: 64 * 1024,
            resolve_owners_during_scan: self.resolve_owners,
            scan_threads: self.scan_threads,
            process_hardlinks: settings.process_hardlinks,
            use_backup_restore: settings.use_backup_restore_privilege,
            process_priority: settings.process_priority,
            exclude_junctions: settings.exclude_junctions,
            exclude_symlinks_dir: settings.exclude_symlinks_dir,
            exclude_symlinks_file: settings.exclude_symlinks_file,
            exclude_mount_points: settings.exclude_mount_points,
            exclude_hidden: settings.exclude_hidden,
            exclude_protected_system: settings.exclude_protected_system,
        })
    }

    pub fn show(&mut self, ctx: &egui::Context, settings: &mut AppSettings) -> TargetSelectorResult {
        let mut result = TargetSelectorResult {
            start_config: None,
            language_changed: false,
        };
        if !self.open {
            return result;
        }

        let mut keep_open = self.open;
        let mut pick_folders_requested = false;

        show_native_window(
            ctx,
            NativeWindowConfig {
                id: "target_selector_window",
                title: t(
                    "MediaStat — Seleccionar Unidades y Carpetas",
                    "MediaStat — Select Drives & Folders",
                ),
                default_size: [760.0, 520.0],
                min_size: [540.0, 360.0],
                resizable: true,
                always_on_top: false,
            },
            &mut keep_open,
            |_vp_ctx, ui, close_now| {
                egui::TopBottomPanel::bottom("target_selector_footer")
                    .frame(egui::Frame::NONE)
                    .show_separator_line(false)
                    .show_inside(ui, |ui| {
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(4.0);

                        let ready_count = self.selected_drives.iter().filter(|&&b| b).count()
                            + self.custom_paths.len();
                        ui.horizontal(|ui| {
                            ui.label(format!(
                                "{}: {ready_count}",
                                t("Orígenes seleccionados", "Selected targets")
                            ));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let start_btn = ui.add_enabled(
                                    ready_count > 0,
                                    egui::Button::new(
                                        RichText::new(t("▶  Iniciar Escaneo", "▶  Start Scan"))
                                            .strong()
                                            .size(15.0)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(Color32::from_rgb(38, 124, 214))
                                    .min_size(egui::vec2(160.0, 34.0)),
                                );
                                if start_btn.clicked() {
                                    result.start_config = self.build_scan_config(settings);
                                }
                                if ui.button(t("Cancelar", "Cancel")).clicked() {
                                    *close_now = true;
                                }
                            });
                        });
                    });

                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show_inside(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                self.render_body(
                                    ui,
                                    settings,
                                    &mut result.language_changed,
                                    &mut pick_folders_requested,
                                );
                            });
                    });
            },
        );

        if pick_folders_requested {
            if let Some(folders) = rfd::FileDialog::new()
                .set_title(t(
                    "Seleccionar una o varias carpetas",
                    "Select one or more folders",
                ))
                .pick_folders()
            {
                for f in folders {
                    if !self.custom_paths.contains(&f) {
                        self.custom_paths.push(f);
                    }
                }
            }
        }

        self.open = keep_open && result.start_config.is_none();
        result
    }

    fn render_body(
        &mut self,
        ui: &mut Ui,
        settings: &mut AppSettings,
        language_changed: &mut bool,
        pick_folders_requested: &mut bool,
    ) {
        ui.horizontal(|ui| {
            ui.heading(t(
                "1. Unidades / Particiones locales y de red",
                "1. Local & Network Drives / Shares",
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for lang in [Language::En, Language::Es] {
                    if ui
                        .selectable_label(
                            settings.language == lang,
                            lang.code().to_ascii_uppercase(),
                        )
                        .on_hover_text(lang.native_name())
                        .clicked()
                        && settings.language != lang
                    {
                        settings.apply_language(lang);
                        settings.sync_active_language();
                        *language_changed = true;
                    }
                }
                ui.label(RichText::new("🌐").small());
                if !self.is_elevated
                    && ui
                        .small_button(t(
                            "🛡 Reiniciar como Admin (NTFS $MFT)",
                            "🛡 Restart as Admin (NTFS $MFT)",
                        ))
                        .clicked()
                {
                    relaunch_as_administrator();
                }
            });
        });
        ui.add_space(4.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            if self.drives.is_empty() {
                ui.label(t(
                    "No se detectaron unidades montadas.",
                    "No mounted drives detected.",
                ));
            }
            for (idx, d) in self.drives.iter().enumerate() {
                ui.horizontal(|ui| {
                    if let Some(sel) = self.selected_drives.get_mut(idx) {
                        ui.checkbox(sel, "");
                    }
                    ui.strong(d.path.to_string_lossy().as_ref());
                    ui.label(&d.label);
                    if let Some(unc) = &d.unc_remote_path {
                        ui.label(
                            RichText::new(format!("→ {unc}"))
                                .small()
                                .monospace()
                                .color(Color32::from_rgb(130, 195, 225)),
                        );
                    }
                    ui.label(
                        RichText::new(format!("[{} · {}]", d.filesystem, d.media_type.label()))
                            .small()
                            .color(Color32::from_rgb(140, 175, 220)),
                    );
                    if d.supports_ntfs_mft && self.is_elevated {
                        ui.label(
                            RichText::new("MFT")
                                .small()
                                .strong()
                                .color(Color32::from_rgb(80, 200, 120)),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let frac = d.used_fraction();
                        let bar = egui::ProgressBar::new(frac)
                            .desired_width(130.0)
                            .text(format!(
                                "{} / {}",
                                format_bytes(d.used_bytes()),
                                format_bytes(d.total_bytes)
                            ));
                        ui.add(bar);
                    });
                });
            }
        });

        ui.add_space(10.0);
        ui.heading(t(
            "2. Múltiples carpetas (de distintas unidades) o rutas SMB/UNC",
            "2. Multiple Folders (across drives) or SMB/UNC Paths",
        ));
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            if ui
                .button(t("+ Añadir carpeta(s)...", "+ Add folder(s)..."))
                .clicked()
            {
                *pick_folders_requested = true;
            }
            ui.separator();
            ui.label(t("Ruta / UNC:", "Path / UNC:"));
            let te = ui.add(
                egui::TextEdit::singleline(&mut self.unc_input)
                    .hint_text(t(
                        r"\\fileserver\datos\2025 o P:\Proyectos",
                        r"\\fileserver\data\2025 or P:\Projects",
                    ))
                    .desired_width(260.0),
            );
            let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button(t("Añadir", "Add")).clicked() || enter)
                && !self.unc_input.trim().is_empty()
            {
                let p = PathBuf::from(self.unc_input.trim());
                if !self.custom_paths.contains(&p) {
                    self.custom_paths.push(p);
                }
                self.unc_input.clear();
            }
        });

        if !self.custom_paths.is_empty() {
            ui.add_space(4.0);
            let mut remove_idx = None;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                for (i, p) in self.custom_paths.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("✕").clicked() {
                            remove_idx = Some(i);
                        }
                        ui.monospace(p.to_string_lossy().as_ref());
                    });
                }
            });
            if let Some(i) = remove_idx {
                self.custom_paths.remove(i);
            }
        }

        ui.add_space(10.0);
        ui.heading(t(
            "3. Motor de Escaneo y Opciones",
            "3. Scan Engine & Options",
        ));
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut self.find_duplicates,
                t(
                    "Detectar duplicados (BLAKE3)",
                    "Detect duplicates (BLAKE3)",
                ),
            );
            if self.find_duplicates {
                ui.checkbox(
                    &mut self.sparse_duplicates,
                    t("Sparse Hash (>64 MiB rápido)", "Sparse Hash (>64 MiB fast)"),
                );
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut self.resolve_owners,
                t(
                    "Resolver propietario (DOMINIO\\usuario) durante el escaneo (más lento en SMB; también disponible bajo demanda)",
                    "Resolve owner (DOMAIN\\user) during scan (slower on SMB; also available on demand)",
                ),
            );
        });
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut self.use_ntfs_mft,
                if self.is_elevated {
                    t(
                        "Lectura directa NTFS $MFT en particiones locales (Admin activo)",
                        "Direct NTFS $MFT read on local volumes (Admin active)",
                    )
                } else {
                    t(
                        "Intentar NTFS $MFT (requiere Admin; fallback automático a NtQueryDirectoryFile)",
                        "Try NTFS $MFT (requires Admin; auto-fallback to NtQueryDirectoryFile)",
                    )
                },
            );
        });
        ui.horizontal(|ui| {
            ui.label(t("Excluir carpetas:", "Exclude folders:"));
            ui.add(egui::TextEdit::singleline(&mut self.excluded_input).desired_width(420.0));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::options_dialog::OptionsDialogState;
    use mediastat_core::set_active_language;
    use mediastat_scanner::DriveMediaType;

    #[test]
    fn test_target_selector_and_options_windows_do_not_autogrow_vertically() {
        for lang in Language::ALL {
            set_active_language(lang);
            let ctx = egui::Context::default();
            let mut settings = AppSettings::default();
            settings.apply_language(lang);
            let drives: Vec<DriveInfo> = (0..7)
                .map(|i| DriveInfo {
                    path: PathBuf::from(format!("{}:\\", (b'C' + i as u8) as char)),
                    label: format!("Share {i}"),
                    filesystem: "NTFS".into(),
                    total_bytes: 1_000_000_000_000,
                    free_bytes: 400_000_000_000,
                    media_type: if i == 0 {
                        DriveMediaType::SsdOrNvme
                    } else {
                        DriveMediaType::NetworkShare
                    },
                    supports_ntfs_mft: i == 0,
                    unc_remote_path: if i == 0 {
                        None
                    } else {
                        Some(format!(r"\\vs-qnap-01\Share{i}"))
                    },
                })
                .collect();

            let mut ts = TargetSelectorState::new(drives, &settings);
            let mut opt = OptionsDialogState::default();
            opt.open = true;

            let mut ts_heights = Vec::new();
            let mut opt_heights = Vec::new();

            for _ in 0..25 {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1440.0, 900.0),
                    )),
                    ..Default::default()
                };
                let _ = ctx.run(raw, |ctx| {
                    ts.show(ctx, &mut settings);
                    opt.show(ctx, &mut settings);
                });
                let ts_h = ctx.memory(|m| {
                    m.area_rect(egui::Id::new("target_selector_window"))
                        .map(|r| r.height())
                });
                let opt_h = ctx.memory(|m| {
                    m.area_rect(egui::Id::new("options_dialog_window"))
                        .map(|r| r.height())
                });
                if let Some(h) = ts_h {
                    ts_heights.push(h);
                }
                if let Some(h) = opt_h {
                    opt_heights.push(h);
                }
            }

            assert!(ts_heights.len() >= 20, "TargetSelector rect not recorded ({lang:?})");
            assert!(opt_heights.len() >= 20, "OptionsDialog rect not recorded ({lang:?})");

            let ts_delta = (ts_heights[ts_heights.len() - 1] - ts_heights[3]).abs();
            let opt_delta = (opt_heights[opt_heights.len() - 1] - opt_heights[3]).abs();
            assert!(
                ts_delta < 1.0,
                "TargetSelector auto-grew vertically ({lang:?})! heights={ts_heights:?}"
            );
            assert!(
                opt_delta < 1.0,
                "OptionsDialog auto-grew vertically ({lang:?})! heights={opt_heights:?}"
            );
        }
        set_active_language(Language::Es);
    }
}
