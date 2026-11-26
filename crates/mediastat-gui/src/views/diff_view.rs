use egui::{Color32, RichText};
use mediastat_core::{format_bytes, format_count, t};
use mediastat_storage::SnapshotDiff;

use crate::views::{show_native_window, NativeWindowConfig};

#[derive(Default)]
pub struct DiffModalState {
    pub open: bool,
    pub before_label: String,
    pub after_label: String,
    pub diff: Option<SnapshotDiff>,
}

impl DiffModalState {
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let Some(diff) = &self.diff else {
            self.open = false;
            return;
        };

        let mut keep_open = self.open;
        show_native_window(
            ctx,
            NativeWindowConfig {
                id: "diff_modal_window",
                title: t(
                    "MediaStat — Comparador de Snapshots (Antes vs Después)",
                    "MediaStat — Snapshot Comparison (Before vs After)",
                ),
                default_size: [740.0, 520.0],
                min_size: [500.0, 340.0],
                resizable: true,
                always_on_top: false,
            },
            &mut keep_open,
            |_vp_ctx, ui, _close_now| {
                ui.label(
                    RichText::new(format!(
                        "{}: {}   →   {}: {}",
                        t("Antes", "Before"),
                        self.before_label,
                        t("Después", "After"),
                        self.after_label
                    ))
                    .small()
                    .weak(),
                );
                ui.separator();

                let delta_color = if diff.delta_bytes <= 0 {
                    Color32::from_rgb(76, 210, 128)
                } else {
                    Color32::from_rgb(245, 110, 80)
                };
                let sign = if diff.delta_bytes >= 0 { "+" } else { "-" };

                ui.horizontal_wrapped(|ui| {
                    ui.strong(t("Tamaño total:", "Total size:"));
                    ui.label(format!(
                        "{} → {}",
                        format_bytes(diff.bytes_before),
                        format_bytes(diff.bytes_after)
                    ));
                    ui.label(
                        RichText::new(format!(
                            "({sign}{})",
                            format_bytes(diff.delta_bytes.unsigned_abs())
                        ))
                        .strong()
                        .color(delta_color),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    ui.strong(t("Archivos:", "Files:"));
                    let fdelta = diff.files_after as i64 - diff.files_before as i64;
                    let fsign = if fdelta >= 0 { "+" } else { "-" };
                    ui.label(format!(
                        "{} → {} ({fsign}{})",
                        format_count(diff.files_before),
                        format_count(diff.files_after),
                        format_count(fdelta.unsigned_abs())
                    ));
                });

                ui.add_space(8.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::CollapsingHeader::new(format!(
                        "{} ({})",
                        t(
                            "📈 Carpetas con mayor crecimiento",
                            "📈 Fastest-growing folders"
                        ),
                        diff.top_growing_folders.len()
                    ))
                    .default_open(true)
                    .show(ui, |ui| {
                        for f in &diff.top_growing_folders {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("+{}", format_bytes(f.delta_bytes as u64)))
                                        .color(Color32::from_rgb(245, 110, 80)),
                                );
                                ui.monospace(f.path.to_string_lossy().as_ref());
                            });
                        }
                    });

                    egui::CollapsingHeader::new(format!(
                        "{} ({})",
                        t(
                            "📉 Carpetas donde se liberó más espacio",
                            "📉 Folders with most freed space"
                        ),
                        diff.top_shrinking_folders.len()
                    ))
                    .default_open(true)
                    .show(ui, |ui| {
                        for f in &diff.top_shrinking_folders {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "-{}",
                                        format_bytes(f.delta_bytes.unsigned_abs())
                                    ))
                                    .color(Color32::from_rgb(76, 210, 128)),
                                );
                                ui.monospace(f.path.to_string_lossy().as_ref());
                            });
                        }
                    });

                    egui::CollapsingHeader::new(format!(
                        "{} ({})",
                        t(
                            "🆕 Nuevos archivos más grandes",
                            "🆕 Largest newly added files"
                        ),
                        diff.newly_added_files.len()
                    ))
                    .show(ui, |ui| {
                        for (p, sz) in diff.newly_added_files.iter().take(100) {
                            ui.horizontal(|ui| {
                                ui.label(format_bytes(*sz));
                                ui.monospace(p.to_string_lossy().as_ref());
                            });
                        }
                    });

                    egui::CollapsingHeader::new(format!(
                        "{} ({})",
                        t(
                            "🗑 Archivos eliminados más grandes",
                            "🗑 Largest removed files"
                        ),
                        diff.removed_files.len()
                    ))
                    .show(ui, |ui| {
                        for (p, sz) in diff.removed_files.iter().take(100) {
                            ui.horizontal(|ui| {
                                ui.label(format_bytes(*sz));
                                ui.monospace(p.to_string_lossy().as_ref());
                            });
                        }
                    });
                });
            },
        );
        self.open = keep_open;
    }
}
