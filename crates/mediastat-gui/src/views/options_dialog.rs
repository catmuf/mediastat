//! WinDirStat-style Multi-Tab Configuration Dialog:
//! General & Engine (Language ES/EN, Theme, Scale, ProcessPriority, SeBackupPrivilege, Hardlinks, Reparse Exclusions, Tree Bars/Columns),
//! Filtering, Visualization/Colors (2D XYSlider + Live Cushion BuildDemoTree Preview),
//! and 10 SMB Quick Actions (Ctrl+0..9).

use egui::{Color32, Pos2, RichText, Stroke, TextureHandle, Ui};
use mediastat_core::{
    t, AgeComparison, AppSettings, CleanupRefreshPolicy, CompiledFilters, Language,
    ProcessPriority, SizeComparison, SizeMetric, TreemapLayoutStyle, TreemapPreset,
    UserCleanupAction, VizColorMode, VizColorPalette, VolumeColorOverrides,
    WINDIRSTAT_TREE_DEPTH_8,
};

use crate::treemap::render_demo_treemap_preview;
use crate::views::{show_native_window, NativeWindowConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionsTab {
    General,
    Filtering,
    Treemap,
    Cleanups,
}

pub struct OptionsDialogState {
    pub open: bool,
    pub tab: OptionsTab,
    pub selected_cleanup_slot: usize,
    pub filter_validation_msg: Option<Result<String, String>>,
    demo_texture: Option<TextureHandle>,
    demo_dirty: bool,
}

impl Default for OptionsDialogState {
    fn default() -> Self {
        Self {
            open: false,
            tab: OptionsTab::Treemap,
            selected_cleanup_slot: 0,
            filter_validation_msg: None,
            demo_texture: None,
            demo_dirty: true,
        }
    }
}

pub struct OptionsResult {
    pub treemap_changed: bool,
    pub theme_or_scale_changed: bool,
    pub language_changed: bool,
    pub saved: bool,
}

impl OptionsDialogState {
    pub fn show(&mut self, ctx: &egui::Context, settings: &mut AppSettings) -> OptionsResult {
        let mut res = OptionsResult {
            treemap_changed: false,
            theme_or_scale_changed: false,
            language_changed: false,
            saved: false,
        };
        if !self.open {
            return res;
        }

        let mut keep = self.open;

        show_native_window(
            ctx,
            NativeWindowConfig {
                id: "options_dialog_window",
                title: t(
                    "MediaStat — Configuración (Paridad WinDirStat)",
                    "MediaStat — Settings (WinDirStat Parity)",
                ),
                default_size: [820.0, 610.0],
                min_size: [560.0, 400.0],
                resizable: true,
                always_on_top: false,
            },
            &mut keep,
            |vp_ctx, ui, close_now| {
                egui::TopBottomPanel::top("options_dialog_tabs")
                    .frame(egui::Frame::NONE)
                    .show_separator_line(false)
                    .show_inside(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.selectable_value(
                                &mut self.tab,
                                OptionsTab::General,
                                t("⚙ General y Motor", "⚙ General & Engine"),
                            );
                            ui.selectable_value(
                                &mut self.tab,
                                OptionsTab::Filtering,
                                t("🔎 Filtros de Escaneo", "🔎 Scan Filters"),
                            );
                            ui.selectable_value(
                                &mut self.tab,
                                OptionsTab::Treemap,
                                t("🎨 Visualización y Cushion 3D", "🎨 Visualization & 3D Cushion"),
                            );
                            ui.selectable_value(
                                &mut self.tab,
                                OptionsTab::Cleanups,
                                t("⚡ Acciones SMB (Ctrl+0..9)", "⚡ SMB Actions (Ctrl+0..9)"),
                            );
                        });
                        ui.separator();
                    });

                egui::TopBottomPanel::bottom("options_dialog_footer")
                    .frame(egui::Frame::NONE)
                    .show_separator_line(false)
                    .show_inside(ui, |ui| {
                        ui.separator();
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            if ui
                                .button(
                                    RichText::new(t("💾 Guardar configuración", "💾 Save settings"))
                                        .strong()
                                        .color(Color32::from_rgb(76, 210, 128)),
                                )
                                .clicked()
                            {
                                let _ = settings.save();
                                res.saved = true;
                                *close_now = true;
                            }
                            if ui.button(t("Cerrar", "Close")).clicked() {
                                *close_now = true;
                            }
                        });
                    });

                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show_inside(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| match self.tab {
                                OptionsTab::General => self.show_general(ui, settings, &mut res),
                                OptionsTab::Filtering => self.show_filtering(ui, settings),
                                OptionsTab::Treemap => {
                                    self.show_treemap(vp_ctx, ui, settings, &mut res)
                                }
                                OptionsTab::Cleanups => self.show_cleanups(ui, settings),
                            });
                    });
            },
        );

        self.open = keep;
        res
    }

    fn show_general(&mut self, ui: &mut Ui, s: &mut AppSettings, res: &mut OptionsResult) {
        ui.heading(t("Idioma, Seguridad y Apariencia", "Language, Safety & Appearance"));
        ui.horizontal(|ui| {
            ui.label(t("🌐 Idioma / Language:", "🌐 Language / Idioma:"));
            for lang in Language::ALL {
                if ui
                    .selectable_label(s.language == lang, lang.native_name())
                    .clicked()
                    && s.language != lang
                {
                    s.apply_language(lang);
                    s.sync_active_language();
                    res.language_changed = true;
                }
            }
        });
        ui.add_space(4.0);

        if ui
            .checkbox(&mut s.dark_mode, t("Tema Oscuro (Dark Mode)", "Dark Mode"))
            .changed()
        {
            res.theme_or_scale_changed = true;
        }
        ui.horizontal(|ui| {
            ui.label(t("Escala de fuente / interfaz:", "Font / UI scale:"));
            for pct in [100u16, 125, 150, 175, 200] {
                if ui
                    .selectable_label(s.ui_scale_percent == pct, format!("{pct}%"))
                    .clicked()
                {
                    s.ui_scale_percent = pct;
                    res.theme_or_scale_changed = true;
                }
            }
        });
        ui.checkbox(
            &mut s.read_only_lock_by_default,
            t(
                "Arrancar siempre con 🔒 Modo Solo Lectura (protege servidores SMB de producción)",
                "Always start in 🔒 Read-Only Mode (protects production SMB servers)",
            ),
        );
        ui.checkbox(
            &mut s.use_pacman_animation,
            t(
                "Mostrar animación clásica Pac-Man de WinDirStat durante el escaneo",
                "Show classic WinDirStat Pac-Man animation while scanning",
            ),
        );

        ui.add_space(8.0);
        ui.heading(t(
            "Motor de Escaneo NtQueryDirectoryFile & Exclusiones (WinDirStat)",
            "NtQueryDirectoryFile Scan Engine & Exclusions (WinDirStat)",
        ));
        ui.horizontal(|ui| {
            ui.label(t(
                "Prioridad del proceso al escanear:",
                "Process priority while scanning:",
            ));
            for pr in ProcessPriority::ALL {
                ui.selectable_value(&mut s.process_priority, pr, pr.label());
            }
        });
        ui.checkbox(
            &mut s.use_backup_restore_privilege,
            t(
                "Activar privilegios SeBackupPrivilege / SeRestorePrivilege (acceso Admin a carpetas protegidas)",
                "Enable SeBackupPrivilege / SeRestorePrivilege (Admin access to protected folders)",
            ),
        );
        ui.checkbox(
            &mut s.process_hardlinks,
            t(
                "Deduplicar enlaces físicos NTFS (Hardlinks) por FileId y mostrar contenedor <Hardlinks>",
                "Deduplicate NTFS Hardlinks by FileId and show <Hardlinks> container",
            ),
        );

        egui::Grid::new("reparse_exclusions_grid")
            .num_columns(2)
            .spacing([18.0, 3.0])
            .show(ui, |ui| {
                ui.checkbox(
                    &mut s.exclude_junctions,
                    t("Excluir Junctions NTFS", "Exclude NTFS Junctions"),
                );
                ui.checkbox(
                    &mut s.exclude_mount_points,
                    t("Excluir Puntos de Montaje", "Exclude Volume Mount Points"),
                );
                ui.end_row();
                ui.checkbox(
                    &mut s.exclude_symlinks_dir,
                    t("Excluir Symlinks de carpetas", "Exclude Directory Symlinks"),
                );
                ui.checkbox(
                    &mut s.exclude_symlinks_file,
                    t("Excluir Symlinks de ficheros", "Exclude File Symlinks"),
                );
                ui.end_row();
                ui.checkbox(
                    &mut s.exclude_hidden,
                    t("Excluir elementos ocultos (H)", "Exclude Hidden items (H)"),
                );
                ui.checkbox(
                    &mut s.exclude_protected_system,
                    t(
                        "Excluir archivos protegidos del sistema (H+S)",
                        "Exclude Protected OS System files (H+S)",
                    ),
                );
                ui.end_row();
            });

        ui.add_space(8.0);
        ui.heading(t(
            "Paneles Desacoplables (Ventanas Libres del SO)",
            "Detachable Panes (Free OS Windows)",
        ));
        ui.checkbox(
            &mut s.float_visualization,
            t(
                "🗗 Desacoplar Visualización (Treemap / Sunburst / Flame Graph) en ventana libre",
                "🗗 Detach Visualization (Treemap / Sunburst / Flame Graph) into a free window",
            ),
        );
        ui.checkbox(
            &mut s.float_extensions_panel,
            t(
                "🗗 Desacoplar Extensiones / Inspector en ventana libre",
                "🗗 Detach Extensions / Inspector into a free window",
            ),
        );

        ui.add_space(8.0);
        ui.heading(t(
            "Árbol de Directorios, Barras y Columnas",
            "Directory Tree, Bars & Columns",
        ));
        if ui
            .checkbox(
                &mut s.show_free_space,
                t("Mostrar nodo <Espacio Libre> (F6)", "Show <Free Space> item (F6)"),
            )
            .changed()
            || ui
                .checkbox(
                    &mut s.show_unknown_space,
                    t(
                        "Mostrar nodo <Espacio Desconocido> (F7)",
                        "Show <Unknown Space> item (F7)",
                    ),
                )
                .changed()
        {
            res.treemap_changed = true;
        }
        ui.checkbox(
            &mut s.group_files_node,
            t(
                "Agrupar archivos directos en nodo virtual <Archivos> dentro de carpetas mixtas",
                "Group direct files under virtual <Files> node inside mixed folders",
            ),
        );
        ui.checkbox(
            &mut s.absolute_percentages,
            t(
                "Mostrar porcentajes absolutos respecto al escaneo total (en lugar de % del padre)",
                "Show absolute percentages of total scan (instead of parent %)",
            ),
        );
        ui.checkbox(
            &mut s.show_timestamp_seconds,
            t(
                "Mostrar segundos en fechas de modificación (HH:MM:SS)",
                "Show seconds in modified timestamps (HH:MM:SS)",
            ),
        );
        ui.checkbox(
            &mut s.group_minor_extensions,
            t(
                "Agrupar extensiones minoritarias (<0.1%) en el panel de Extensiones",
                "Group minor extensions (<0.1%) in the Extensions list",
            ),
        );

        ui.checkbox(
            &mut s.tree_bar_classic_depth_colors,
            t(
                "Usar barras de 8 colores por profundidad estilo WinDirStat clásico",
                "Use classic WinDirStat 8-level depth bar colors",
            ),
        );
        if s.tree_bar_classic_depth_colors {
            ui.horizontal_wrapped(|ui| {
                ui.label(t("Colores nivel 0–7:", "Depth colors 0–7:"));
                for col in &mut s.tree_depth_colors {
                    ui.color_edit_button_srgb(col);
                }
                if ui.small_button(t("↺ Clásicos", "↺ Classic")).clicked() {
                    s.tree_depth_colors = WINDIRSTAT_TREE_DEPTH_8;
                }
            });
        }

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(t("Columnas visibles del árbol:", "Visible tree columns:"));
            let col_names: [(usize, &str); 7] = [
                (1, t("% Barra", "% Bar")),
                (2, t("Tamaño Lógico", "Logical Size")),
                (3, t("En Disco", "On Disk")),
                (4, t("Ficheros", "Files")),
                (5, t("Atrib.", "Attrs")),
                (6, t("Propietario", "Owner")),
                (7, t("Modificado", "Modified")),
            ];
            for (idx, lbl) in col_names {
                ui.checkbox(&mut s.tree_columns_visible[idx], lbl);
            }
        });

        ui.horizontal(|ui| {
            ui.label(t("Métrica de tamaño por defecto:", "Default size metric:"));
            if ui
                .radio_value(
                    &mut s.size_metric,
                    SizeMetric::Logical,
                    SizeMetric::Logical.label(),
                )
                .changed()
                || ui
                    .radio_value(
                        &mut s.size_metric,
                        SizeMetric::AllocatedOnDisk,
                        SizeMetric::AllocatedOnDisk.label(),
                    )
                    .changed()
            {
                res.treemap_changed = true;
            }
        });

        ui.horizontal(|ui| {
            ui.label(t(
                "Máximo de resultados en Búsqueda Avanzada:",
                "Max results in Advanced Search:",
            ));
            ui.add(
                egui::DragValue::new(&mut s.search_max_results)
                    .range(500..=200_000)
                    .speed(500),
            );
        });
    }

    fn show_filtering(&mut self, ui: &mut Ui, s: &mut AppSettings) {
        let f = &mut s.filter_config;
        ui.heading(t(
            "Filtros de Escaneo (Paridad WinDirStat 2.10 Filtering)",
            "Scan Filters (WinDirStat 2.10 Filtering Parity)",
        ));
        ui.checkbox(
            &mut f.protect_cloud_offline_files,
            t(
                "☁ Protección Anti-Hydration: omitir hashing de duplicados en ficheros OneDrive/Cloud Offline",
                "☁ Anti-Hydration Protection: skip duplicate hashing on OneDrive/Cloud Offline placeholders",
            ),
        );
        ui.checkbox(
            &mut f.use_regex,
            t(
                "Usar Expresiones Regulares (Regex) en lugar de comodines simples (* y ?)",
                "Use Regular Expressions (Regex) instead of wildcards (* and ?)",
            ),
        );

        ui.add_space(6.0);
        ui.label(t(
            "Excluir carpetas (una por línea o separadas por ';'):",
            "Exclude folders (one per line or separated by ';'):",
        ));
        ui.add(
            egui::TextEdit::multiline(&mut f.exclude_dirs)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        ui.label(t(
            "Excluir ficheros (ej. *.tmp;*.lnk;desktop.ini):",
            "Exclude files (e.g. *.tmp;*.lnk;desktop.ini):",
        ));
        ui.add(
            egui::TextEdit::multiline(&mut f.exclude_files)
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        ui.label(t(
            "Incluir SOLO carpetas (vacío = todas):",
            "Include ONLY folders (empty = all):",
        ));
        ui.add(egui::TextEdit::singleline(&mut f.include_dirs).desired_width(f32::INFINITY));
        ui.label(t(
            "Incluir SOLO ficheros (vacío = todos):",
            "Include ONLY files (empty = all):",
        ));
        ui.add(egui::TextEdit::singleline(&mut f.include_files).desired_width(f32::INFINITY));

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(t("Filtro por tamaño:", "Size filter:"));
            egui::ComboBox::from_id_salt("flt_sz_cmp")
                .selected_text(match f.size_cmp {
                    SizeComparison::Ignore => t("Ignorar", "Ignore"),
                    SizeComparison::GreaterThan => t("Solo mayores que (>)", "Larger than (>)"),
                    SizeComparison::LessThan => t("Solo menores que (<)", "Smaller than (<)"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.size_cmp, SizeComparison::Ignore, t("Ignorar", "Ignore"));
                    ui.selectable_value(
                        &mut f.size_cmp,
                        SizeComparison::GreaterThan,
                        t("Mayores que (>)", "Larger than (>)"),
                    );
                    ui.selectable_value(
                        &mut f.size_cmp,
                        SizeComparison::LessThan,
                        t("Menores que (<)", "Smaller than (<)"),
                    );
                });
            let mut kb = f.size_bytes / 1024;
            if ui.add(egui::DragValue::new(&mut kb).suffix(" KB")).changed() {
                f.size_bytes = kb * 1024;
            }
        });

        ui.horizontal(|ui| {
            ui.label(t("Filtro por antigüedad:", "Age filter:"));
            egui::ComboBox::from_id_salt("flt_age_cmp")
                .selected_text(match f.age_cmp {
                    AgeComparison::Ignore => t("Ignorar", "Ignore"),
                    AgeComparison::OlderThanDays => t("Más antiguos que (días)", "Older than (days)"),
                    AgeComparison::NewerThanDays => t("Más recientes que (días)", "Newer than (days)"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.age_cmp, AgeComparison::Ignore, t("Ignorar", "Ignore"));
                    ui.selectable_value(
                        &mut f.age_cmp,
                        AgeComparison::OlderThanDays,
                        t("Más antiguos que (días)", "Older than (days)"),
                    );
                    ui.selectable_value(
                        &mut f.age_cmp,
                        AgeComparison::NewerThanDays,
                        t("Más recientes que (días)", "Newer than (days)"),
                    );
                });
            ui.add(egui::DragValue::new(&mut f.age_days).suffix(t(" días", " days")));
        });

        ui.add_space(6.0);
        if ui
            .button(t("✓ Validar patrones de filtro", "✓ Validate filter patterns"))
            .clicked()
        {
            self.filter_validation_msg = Some(CompiledFilters::compile(f, &[]).map(|_| {
                t(
                    "Patrones válidos y listos para el próximo escaneo.",
                    "Patterns valid and ready for the next scan.",
                )
                .into()
            }));
        }
        if let Some(msg) = &self.filter_validation_msg {
            match msg {
                Ok(ok) => ui.label(RichText::new(ok).color(Color32::from_rgb(76, 210, 128))),
                Err(e) => ui.label(RichText::new(e).color(Color32::from_rgb(240, 80, 80))),
            };
        }
    }

    fn show_treemap(
        &mut self,
        vp_ctx: &egui::Context,
        ui: &mut Ui,
        s: &mut AppSettings,
        res: &mut OptionsResult,
    ) {
        let tm = &mut s.treemap;
        ui.heading(t(
            "Paleta de Colores (WinDirStat 18 Colores + Heatmaps)",
            "Color Palette (WinDirStat 18 Colors + Heatmaps)",
        ));
        ui.horizontal_wrapped(|ui| {
            for pal in VizColorPalette::ALL {
                if ui
                    .selectable_label(tm.color_palette == pal, pal.label())
                    .clicked()
                {
                    tm.color_palette = pal;
                    tm.custom_volume_colors = None;
                    res.treemap_changed = true;
                    self.demo_dirty = true;
                }
            }
        });
        ui.label(RichText::new(tm.color_palette.description()).small().weak());

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(t("Modo de color por defecto:", "Default color mode:"));
            for mode in VizColorMode::ALL {
                if ui
                    .selectable_label(tm.color_mode == mode, mode.label())
                    .clicked()
                {
                    tm.color_mode = mode;
                    res.treemap_changed = true;
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .checkbox(
                    &mut tm.use_rank_extension_colors,
                    t(
                        "Asignar 18 colores WinDirStat por ranking de bytes (extensión mayor = Azul #0, Rojo #1...)",
                        "Assign official 18 WinDirStat colors by byte rank (largest ext = Blue #0, Red #1...)",
                    ),
                )
                .changed()
            {
                res.treemap_changed = true;
                self.demo_dirty = true;
            }
            ui.checkbox(
                &mut tm.show_color_legend,
                t("Mostrar barra de leyenda", "Show color legend bar"),
            );
        });

        ui.add_space(4.0);
        egui::CollapsingHeader::new(t(
            "🎨 Personalizar colores de Volumen (<Libre> / <Desconocido>)",
            "🎨 Customize Volume Colors (<Free Space> / <Unknown Space>)",
        ))
        .default_open(tm.custom_volume_colors.is_some())
        .show(ui, |ui| {
            let mut colors = tm
                .custom_volume_colors
                .clone()
                .unwrap_or_else(|| VolumeColorOverrides::from_palette(tm.color_palette));
            let mut edited = false;

            ui.horizontal_wrapped(|ui| {
                ui.label(t("<Espacio Libre>:", "<Free Space>:"));
                edited |= ui.color_edit_button_srgb(&mut colors.free_space).changed();
                ui.separator();
                ui.label(t("<Espacio Desconocido>:", "<Unknown Space>:"));
                edited |= ui.color_edit_button_srgb(&mut colors.unknown_space).changed();
            });

            if edited {
                tm.custom_volume_colors = Some(colors);
                res.treemap_changed = true;
            }
            if ui
                .small_button(t(
                    "↺ Restaurar colores de la paleta activa",
                    "↺ Reset active palette colors",
                ))
                .clicked()
            {
                tm.custom_volume_colors = None;
                res.treemap_changed = true;
            }
        });

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.strong("Layout:");
            for st in [
                TreemapLayoutStyle::Squarified,
                TreemapLayoutStyle::KDirStat,
                TreemapLayoutStyle::Hilbert,
                TreemapLayoutStyle::Moore,
            ] {
                if ui.selectable_label(tm.style == st, st.label()).clicked() {
                    tm.style = st;
                    res.treemap_changed = true;
                    self.demo_dirty = true;
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.strong(t("Preset Cushion:", "Cushion Preset:"));
            for pr in [
                TreemapPreset::Classic,
                TreemapPreset::Calm,
                TreemapPreset::Flat,
                TreemapPreset::Pastel,
                TreemapPreset::HighContrast,
                TreemapPreset::Custom,
            ] {
                if ui.selectable_label(tm.preset == pr, pr.label()).clicked() {
                    tm.apply_shading_preset(pr);
                    res.treemap_changed = true;
                    self.demo_dirty = true;
                }
            }
        });

        ui.add_space(6.0);
        ui.columns(2, |cols| {
            let mut slider_changed = false;
            cols[0].strong(t(
                "Parámetros Cushion 3D (fórmula WinDirStat)",
                "3D Cushion Parameters (WinDirStat formula)",
            ));
            slider_changed |= cols[0]
                .add(egui::Slider::new(&mut tm.brightness, 0.10..=1.20).text(t("Brillo", "Brightness")))
                .changed();
            slider_changed |= cols[0]
                .add(egui::Slider::new(&mut tm.saturation, 0.0..=1.0).text(t("Saturación", "Saturation")))
                .changed();
            slider_changed |= cols[0]
                .add(egui::Slider::new(&mut tm.cushion_height, 0.0..=1.5).text(t("Altura Cushion", "Cushion Height")))
                .changed();
            slider_changed |= cols[0]
                .add(egui::Slider::new(&mut tm.scale_factor, 0.0..=1.0).text(t("Factor escala", "Scale Factor")))
                .changed();
            slider_changed |= cols[0]
                .add(egui::Slider::new(&mut tm.ambient_light, 0.0..=1.0).text(t("Luz ambiente", "Ambient Light")))
                .changed();

            cols[0].add_space(4.0);
            let mut grid_or_opt = false;
            grid_or_opt |= cols[0]
                .checkbox(&mut tm.show_grid, t("Mostrar rejilla 1px (Grid)", "Show 1px Grid"))
                .changed();
            if tm.show_grid {
                cols[0].horizontal(|ui| {
                    let mut auto_grid = tm.grid_color.is_none();
                    if ui.checkbox(&mut auto_grid, t("Rejilla Auto", "Auto Grid")).changed() {
                        tm.grid_color = if auto_grid { None } else { Some([0, 0, 0]) };
                        grid_or_opt = true;
                    }
                    if let Some(ref mut gcol) = tm.grid_color {
                        ui.label(t("Color rejilla:", "Grid color:"));
                        grid_or_opt |= ui.color_edit_button_srgb(gcol).changed();
                    }
                });
            }
            cols[0].horizontal(|ui| {
                ui.label(t("Color resaltado selección:", "Selection highlight color:"));
                grid_or_opt |= ui.color_edit_button_srgb(&mut tm.highlight_color).changed();
            });
            grid_or_opt |= cols[0]
                .checkbox(
                    &mut tm.show_extensions_overlay,
                    t(
                        "Etiquetas de nombre/extensión sobre bloques",
                        "Show filename/extension labels on large tiles",
                    ),
                )
                .changed();
            grid_or_opt |= cols[0]
                .checkbox(
                    &mut tm.contrast_labels,
                    t(
                        "Contraste automático WCAG en etiquetas",
                        "Automatic WCAG contrast on labels",
                    ),
                )
                .changed();
            cols[0].horizontal(|ui| {
                grid_or_opt |= ui
                    .checkbox(&mut tm.show_folder_frames, t("Marcos de carpeta", "Folder Frames"))
                    .changed();
                if tm.show_folder_frames {
                    ui.label(t("Umbral:", "Threshold:"));
                    grid_or_opt |= ui
                        .add(
                            egui::DragValue::new(&mut tm.folder_frames_draw_threshold)
                                .range(2..=40)
                                .suffix(" px"),
                        )
                        .changed();
                }
            });

            if slider_changed {
                tm.preset = TreemapPreset::Custom;
                res.treemap_changed = true;
                self.demo_dirty = true;
            }
            if grid_or_opt {
                res.treemap_changed = true;
                self.demo_dirty = true;
            }

            cols[1].horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.strong(t("Fuente de luz 2D (XYSlider)", "2D Light Source (XYSlider)"));
                    if draw_xy_light_slider(ui, &mut tm.light_x, &mut tm.light_y) {
                        tm.preset = TreemapPreset::Custom;
                        res.treemap_changed = true;
                        self.demo_dirty = true;
                    }
                    ui.label(
                        RichText::new(format!("X: {:.2}  Y: {:.2}", tm.light_x, tm.light_y))
                            .small()
                            .monospace()
                            .weak(),
                    );
                });
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.strong(t("Vista previa Cushion en vivo", "Live Cushion Preview"));
                    if self.demo_dirty || self.demo_texture.is_none() {
                        self.demo_texture =
                            Some(render_demo_treemap_preview(vp_ctx, tm, 210, 125));
                        self.demo_dirty = false;
                    }
                    if let Some(tex) = &self.demo_texture {
                        ui.add(
                            egui::Image::from_texture(tex)
                                .fit_to_exact_size(egui::vec2(210.0, 125.0))
                                .corner_radius(4.0),
                        );
                    }
                });
            });
        });
    }

    fn show_cleanups(&mut self, ui: &mut Ui, s: &mut AppSettings) {
        ui.horizontal(|ui| {
            ui.heading(t(
                "Acciones Rápidas y Limpiezas SMB (Ctrl+0 a Ctrl+9)",
                "Quick Actions & SMB Cleanups (Ctrl+0 to Ctrl+9)",
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button(t("↺ Restaurar 10 presets SMB", "↺ Restore 10 SMB Presets"))
                    .on_hover_text(t(
                        "Recarga los 10 atajos SysAdmin preconfigurados para Windows + SMB (Ctrl+0..6 solo lectura, Ctrl+7..9 escritura)",
                        "Reloads the 10 built-in Windows + SMB SysAdmin shortcuts (Ctrl+0..6 read-only, Ctrl+7..9 write)",
                    ))
                    .clicked()
                {
                    s.user_cleanups = UserCleanupAction::default_presets_for(s.language);
                }
            });
        });
        ui.label(
            RichText::new(t(
                "Variables: %p (ruta), %u (ruta UNC real \\\\servidor\\share\\...), %n (nombre), %d (carpeta), %e (extensión)",
                "Placeholders: %p (path), %u (real UNC path \\\\server\\share\\...), %n (name), %d (folder), %e (extension)",
            ))
            .small()
            .weak(),
        );
        ui.add_space(4.0);

        while s.user_cleanups.len() < 10 {
            s.user_cleanups
                .push(UserCleanupAction::empty_slot(s.user_cleanups.len()));
        }

        ui.horizontal_wrapped(|ui| {
            for (i, slot) in s.user_cleanups.iter().enumerate().take(10) {
                let badge = if !slot.enabled {
                    ""
                } else if slot.read_only_safe {
                    " 👁"
                } else {
                    " ⚠"
                };
                let lbl = format!("Ctrl+{i}{badge}");
                if ui
                    .selectable_label(self.selected_cleanup_slot == i, lbl)
                    .on_hover_text(&slot.title)
                    .clicked()
                {
                    self.selected_cleanup_slot = i;
                }
            }
        });
        ui.separator();

        let act = &mut s.user_cleanups[self.selected_cleanup_slot];
        ui.horizontal(|ui| {
            ui.checkbox(&mut act.enabled, t("Habilitada", "Enabled"));
            ui.checkbox(
                &mut act.read_only_safe,
                t(
                    "👁 Segura en Modo Solo Lectura (permitida con candado 🔒 activo)",
                    "👁 Read-Only Safe (allowed while 🔒 Read-Only lock is active)",
                ),
            );
        });
        ui.horizontal(|ui| {
            ui.label(t("Título:", "Title:"));
            ui.add(egui::TextEdit::singleline(&mut act.title).desired_width(380.0));
        });
        ui.label(t("Línea de comandos:", "Command line:"));
        ui.add(egui::TextEdit::singleline(&mut act.command_line).desired_width(f32::INFINITY));
        ui.horizontal_wrapped(|ui| {
            ui.label(t("Insertar variable:", "Insert variable:"));
            for (tok, desc) in [
                ("\"%p\"", t("%p Ruta", "%p Path")),
                ("\"%u\"", t("%u Ruta UNC", "%u UNC Path")),
                ("%n", t("%n Nombre", "%n Name")),
                ("\"%d\"", t("%d Carpeta", "%d Folder")),
                ("%e", "%e Ext"),
            ] {
                if ui.small_button(desc).clicked() {
                    if !act.command_line.is_empty() && !act.command_line.ends_with(' ') {
                        act.command_line.push(' ');
                    }
                    act.command_line.push_str(tok);
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut act.works_for_files, t("Aplicable a ficheros", "Works for files"));
            ui.checkbox(&mut act.works_for_dirs, t("Aplicable a carpetas", "Works for folders"));
            ui.checkbox(&mut act.ask_confirmation, t("Pedir confirmación", "Ask confirmation"));
            ui.checkbox(&mut act.run_minimized, t("Ocultar ventana consola", "Hide console window"));
        });
        ui.horizontal(|ui| {
            ui.label(t("Tras ejecutar:", "After running:"));
            for pol in [
                CleanupRefreshPolicy::None,
                CleanupRefreshPolicy::RefreshSelected,
                CleanupRefreshPolicy::RefreshParent,
            ] {
                ui.radio_value(&mut act.refresh_policy, pol, pol.label());
            }
        });
    }
}

/// Interactive 2D XYSlider matching upstream WinDirStat `Controls/XYSlider.cpp` (`-4.0..=4.0`).
fn draw_xy_light_slider(ui: &mut Ui, lx: &mut f32, ly: &mut f32) -> bool {
    let size = egui::vec2(120.0, 120.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let mut changed = false;

    if (resp.clicked() || resp.dragged()) && resp.interact_pointer_pos().is_some() {
        let p = resp.interact_pointer_pos().unwrap();
        let nx = ((p.x - rect.min.x) / rect.width()).clamp(0.0, 1.0) * 8.0 - 4.0;
        let ny = ((p.y - rect.min.y) / rect.height()).clamp(0.0, 1.0) * 8.0 - 4.0;
        if (*lx - nx).abs() > 0.01 || (*ly - ny).abs() > 0.01 {
            *lx = nx;
            *ly = ny;
            changed = true;
        }
    }

    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, Color32::from_rgb(24, 28, 36));
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0_f32, Color32::from_gray(85)),
        egui::StrokeKind::Inside,
    );

    let c = rect.center();
    let axis = Stroke::new(1.0_f32, Color32::from_gray(55));
    painter.line_segment(
        [Pos2::new(rect.min.x + 4.0, c.y), Pos2::new(rect.max.x - 4.0, c.y)],
        axis,
    );
    painter.line_segment(
        [Pos2::new(c.x, rect.min.y + 4.0), Pos2::new(c.x, rect.max.y - 4.0)],
        axis,
    );

    let px = rect.min.x + ((*lx + 4.0) / 8.0).clamp(0.0, 1.0) * rect.width();
    let py = rect.min.y + ((*ly + 4.0) / 8.0).clamp(0.0, 1.0) * rect.height();
    let handle = Pos2::new(px, py);

    painter.circle_filled(handle, 14.0, Color32::from_rgba_unmultiplied(255, 235, 130, 35));
    painter.circle_filled(handle, 6.0, Color32::from_rgb(255, 224, 92));
    painter.circle_stroke(handle, 6.0, Stroke::new(1.5_f32, Color32::WHITE));

    let guide = Stroke::new(0.8_f32, Color32::from_rgba_unmultiplied(255, 224, 92, 90));
    painter.line_segment([Pos2::new(rect.min.x, py), Pos2::new(rect.max.x, py)], guide);
    painter.line_segment([Pos2::new(px, rect.min.y), Pos2::new(px, rect.max.y)], guide);

    let _ = resp.on_hover_text(t(
        "Arrastra para mover la dirección de la luz Cushion 3D (X, Y)",
        "Drag to adjust 3D Cushion light direction (X, Y)",
    ));
    changed
}
