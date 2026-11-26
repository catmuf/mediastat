//! Live Real-Time File Watcher Tab (WinDirStat 2.10 `FileWatcherControl` parity).

use std::collections::VecDeque;
use std::path::PathBuf;

use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mediastat_core::{format_bytes, format_count, format_timestamp_opts, t, ScanTree};
use mediastat_scanner::{FileWatcher, WatchEvent};
use regex::RegexBuilder;

use crate::shell_actions::reveal_in_explorer;
use crate::views::sort_header_button;

const MAX_EVENTS_HISTORY: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatcherSortColumn {
    Time,
    Action,
    Size,
    Path,
}

pub struct WatcherViewState {
    watcher: Option<FileWatcher>,
    pub custom_path: String,
    pub filter_pattern: String,
    pub filter_regex: bool,
    pub paused: bool,
    pub events: VecDeque<WatchEvent>,
    pub total_seen: u64,
    pub sort_col: WatcherSortColumn,
    pub sort_desc: bool,
}

impl Default for WatcherViewState {
    fn default() -> Self {
        Self {
            watcher: None,
            custom_path: String::new(),
            filter_pattern: String::new(),
            filter_regex: false,
            paused: false,
            events: VecDeque::with_capacity(2048),
            total_seen: 0,
            sort_col: WatcherSortColumn::Time,
            sort_desc: true,
        }
    }
}

impl WatcherViewState {
    pub fn is_running(&self) -> bool {
        self.watcher.as_ref().is_some_and(|w| w.is_running())
    }

    pub fn poll_events(&mut self) {
        let Some(w) = &self.watcher else {
            return;
        };
        let mut drained = 0;
        while drained < 256 {
            let Ok(ev) = w.rx.try_recv() else {
                break;
            };
            drained += 1;
            self.total_seen += 1;
            if !self.paused {
                if self.events.len() >= MAX_EVENTS_HISTORY {
                    self.events.pop_back();
                }
                self.events.push_front(ev);
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(mut w) = self.watcher.take() {
            w.stop();
        }
    }

    pub fn show(&mut self, ui: &mut Ui, ctx: &egui::Context, tree: &ScanTree) {
        self.poll_events();
        if self.is_running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        ui.horizontal_wrapped(|ui| {
            let running = self.is_running();
            if running {
                if ui
                    .button(
                        RichText::new(t("⏹ Detener Monitor", "⏹ Stop Monitor"))
                            .strong()
                            .color(Color32::from_rgb(240, 85, 85)),
                    )
                    .clicked()
                {
                    self.stop();
                }
                ui.checkbox(&mut self.paused, t("Pausar vista", "Pause view"));
            } else {
                ui.label(t("Ruta a vigilar:", "Path to watch:"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.custom_path)
                        .hint_text(t(
                            "Vacío = vigilar orígenes del escaneo actual",
                            "Empty = watch current scan targets",
                        ))
                        .desired_width(240.0),
                );
                if ui.button("📁...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.custom_path = p.to_string_lossy().into_owned();
                    }
                }
                let can_start = !self.custom_path.trim().is_empty() || !tree.root_targets.is_empty();
                if ui
                    .add_enabled(
                        can_start,
                        egui::Button::new(
                            RichText::new(t("▶ Iniciar Monitor en Vivo", "▶ Start Live Monitor"))
                                .strong()
                                .color(Color32::from_rgb(76, 210, 128)),
                        ),
                    )
                    .clicked()
                {
                    let targets = if !self.custom_path.trim().is_empty() {
                        vec![PathBuf::from(self.custom_path.trim())]
                    } else {
                        tree.root_targets.clone()
                    };
                    self.watcher = Some(FileWatcher::start(targets));
                }
            }

            ui.separator();
            ui.label(t("Filtro vivo:", "Live filter:"));
            ui.add(
                egui::TextEdit::singleline(&mut self.filter_pattern)
                    .hint_text(t("Filtrar ruta o Regex...", "Filter path or Regex..."))
                    .desired_width(170.0),
            );
            ui.checkbox(&mut self.filter_regex, "Regex");

            if ui.button(t("🧹 Limpiar", "🧹 Clear")).clicked() {
                self.events.clear();
            }
        });

        let watched_lbl = self
            .watcher
            .as_ref()
            .map(|w| {
                w.watched_paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .unwrap_or_else(|| t("Detenido", "Stopped").into());

        ui.label(
            RichText::new(format!(
                "{}: {} · {}: {} ({}: {})",
                t("Estado", "Status"),
                watched_lbl,
                t("Eventos capturados", "Captured events"),
                format_count(self.total_seen),
                t("en historial", "in history"),
                format_count(self.events.len() as u64)
            ))
            .small()
            .color(Color32::from_rgb(160, 200, 235)),
        );
        ui.separator();

        // Filter events
        let compiled_re = if self.filter_regex && !self.filter_pattern.trim().is_empty() {
            RegexBuilder::new(self.filter_pattern.trim())
                .case_insensitive(true)
                .build()
                .ok()
        } else {
            None
        };
        let q_lower = self.filter_pattern.trim().to_ascii_lowercase();

        let mut visible: Vec<&WatchEvent> = self
            .events
            .iter()
            .filter(|ev| {
                if q_lower.is_empty() {
                    return true;
                }
                let ps = ev.path.to_string_lossy();
                if let Some(re) = &compiled_re {
                    re.is_match(&ps)
                } else {
                    ps.to_ascii_lowercase().contains(&q_lower)
                }
            })
            .collect();

        let w_col = self.sort_col;
        let w_desc = self.sort_desc;
        visible.sort_unstable_by(|a, b| {
            let ord = match w_col {
                WatcherSortColumn::Time => a.timestamp_unix.cmp(&b.timestamp_unix),
                WatcherSortColumn::Action => a.action.label().cmp(b.action.label()),
                WatcherSortColumn::Size => a.file_size.cmp(&b.file_size),
                WatcherSortColumn::Path => a.path.cmp(&b.path),
            };
            if w_desc {
                ord.reverse()
            } else {
                ord
            }
        });

        let mut sort_clicked: Option<WatcherSortColumn> = None;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(135.0).at_least(110.0))
            .column(Column::initial(120.0).at_least(90.0))
            .column(Column::initial(90.0).at_least(65.0))
            .column(Column::remainder().at_least(220.0).clip(true))
            .header(22.0, |mut h| {
                h.col(|ui| {
                    if sort_header_button(ui, t("Hora", "Time"), self.sort_col == WatcherSortColumn::Time, self.sort_desc) {
                        sort_clicked = Some(WatcherSortColumn::Time);
                    }
                });
                h.col(|ui| {
                    if sort_header_button(ui, t("Acción", "Action"), self.sort_col == WatcherSortColumn::Action, self.sort_desc) {
                        sort_clicked = Some(WatcherSortColumn::Action);
                    }
                });
                h.col(|ui| {
                    if sort_header_button(ui, t("Tamaño", "Size"), self.sort_col == WatcherSortColumn::Size, self.sort_desc) {
                        sort_clicked = Some(WatcherSortColumn::Size);
                    }
                });
                h.col(|ui| {
                    if sort_header_button(ui, t("Ruta del archivo", "File path"), self.sort_col == WatcherSortColumn::Path, self.sort_desc) {
                        sort_clicked = Some(WatcherSortColumn::Path);
                    }
                });
            })
            .body(|body| {
                body.rows(21.0, visible.len(), |mut row| {
                    let ev = visible[row.index()];
                    let [r, g, b] = ev.action.color_rgb();
                    row.col(|ui| {
                        ui.label(format_timestamp_opts(ev.timestamp_unix, true));
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(ev.action.label())
                                .strong()
                                .color(Color32::from_rgb(r, g, b)),
                        );
                    });
                    row.col(|ui| {
                        ui.label(if ev.file_size > 0 {
                            format_bytes(ev.file_size)
                        } else {
                            "—".into()
                        });
                    });
                    row.col(|ui| {
                        let resp = ui.selectable_label(
                            false,
                            RichText::new(ev.path.to_string_lossy().as_ref()).small().monospace(),
                        );
                        if resp.double_clicked() {
                            reveal_in_explorer(&ev.path);
                        }
                    });
                });
            });

        if let Some(col) = sort_clicked {
            if self.sort_col == col {
                self.sort_desc = !self.sort_desc;
            } else {
                self.sort_col = col;
                self.sort_desc = matches!(col, WatcherSortColumn::Time | WatcherSortColumn::Size);
            }
        }
    }
}
