//! Visual 3×4 Window Layout Switcher Popup (Exact 12-layout WinDirStat 2.10 `LayoutPopup` parity
//! + MediaStat free OS multi-viewport detach toggles).

use egui::{Color32, Pos2, Rect, RichText, Stroke, Ui};
use mediastat_core::{t, LayoutPaneKind, WindowLayoutPreset};

use crate::views::{show_native_window, NativeWindowConfig};

#[derive(Default)]
pub struct LayoutPopupState {
    pub open: bool,
}

impl LayoutPopupState {
    /// Show an independent OS window 3×4 card selector; returns `Some(new_layout)` when user clicks a card.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        current: WindowLayoutPreset,
        float_viz: &mut bool,
        float_ext: &mut bool,
    ) -> Option<WindowLayoutPreset> {
        if !self.open {
            return None;
        }
        let mut chosen = None;
        let mut keep = self.open;

        show_native_window(
            ctx,
            NativeWindowConfig {
                id: "layout_popup_window",
                title: t(
                    "MediaStat — 12 Diseños de Ventana (WinDirStat Layout)",
                    "MediaStat — 12 Window Layouts (WinDirStat Layout)",
                ),
                default_size: [535.0, 505.0],
                min_size: [510.0, 480.0],
                resizable: false,
                always_on_top: false,
            },
            &mut keep,
            |_vp_ctx, ui, _close_now| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(t(
                            "12 diseños oficiales de WinDirStat (3×4):",
                            "12 official WinDirStat layouts (3×4):",
                        ))
                        .small()
                        .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        legend_dot(ui, Color32::from_rgb(225, 115, 72), t("Mapa", "Map"));
                        legend_dot(ui, Color32::from_rgb(135, 105, 195), "Ext");
                        legend_dot(ui, Color32::from_rgb(70, 130, 195), t("Árbol", "Tree"));
                    });
                });
                ui.add_space(6.0);

                egui::Grid::new("layout_cards_grid_3x4")
                    .spacing([10.0, 10.0])
                    .show(ui, |ui| {
                        for (idx, layout) in WindowLayoutPreset::ALL.iter().copied().enumerate() {
                            if draw_layout_card(ui, layout, layout == current) {
                                chosen = Some(layout);
                            }
                            if idx % 3 == 2 {
                                ui.end_row();
                            }
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.checkbox(
                        float_viz,
                        t(
                            "🗗 Ventana libre del SO: Visualización",
                            "🗗 Free OS Window: Visualization",
                        ),
                    );
                    ui.checkbox(
                        float_ext,
                        t(
                            "🗗 Ventana libre del SO: Extensiones / Inspector",
                            "🗗 Free OS Window: Extensions / Inspector",
                        ),
                    );
                });
            },
        );

        self.open = keep && chosen.is_none();
        chosen
    }
}

fn legend_dot(ui: &mut Ui, col: Color32, text: &str) {
    ui.label(RichText::new(text).small().weak());
    let (r, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 2.0, col);
}

fn draw_layout_card(ui: &mut Ui, layout: WindowLayoutPreset, active: bool) -> bool {
    let card_size = egui::vec2(158.0, 96.0);
    let (rect, resp) = ui.allocate_exact_size(card_size, egui::Sense::click());
    let painter = ui.painter();

    let bg = if active {
        Color32::from_rgb(38, 72, 118)
    } else if resp.hovered() {
        Color32::from_rgb(44, 52, 66)
    } else {
        Color32::from_rgb(30, 35, 44)
    };
    let border = if active {
        Stroke::new(2.0_f32, Color32::from_rgb(86, 175, 255))
    } else {
        Stroke::new(1.0_f32, Color32::from_gray(75))
    };

    painter.rect_filled(rect, 6.0, bg);
    painter.rect_stroke(rect, 6.0, border, egui::StrokeKind::Inside);

    let inner = Rect::from_min_max(
        Pos2::new(rect.min.x + 8.0, rect.min.y + 7.0),
        Pos2::new(rect.max.x - 8.0, rect.max.y - 21.0),
    );
    let c_tree = Color32::from_rgb(70, 130, 195);
    let c_ext = Color32::from_rgb(135, 105, 195);
    let c_viz = Color32::from_rgb(225, 115, 72);

    for (kind, x, y, w, h) in layout.def().preview_panes() {
        let (col, tag) = match kind {
            LayoutPaneKind::AllFiles => (c_tree, t("Á", "T")),
            LayoutPaneKind::FileTypes => (c_ext, "E"),
            LayoutPaneKind::Visualization => (c_viz, "M"),
        };
        let r = Rect::from_min_max(
            Pos2::new(
                inner.min.x + inner.width() * x + 1.5,
                inner.min.y + inner.height() * y + 1.5,
            ),
            Pos2::new(
                inner.min.x + inner.width() * (x + w) - 1.5,
                inner.min.y + inner.height() * (y + h) - 1.5,
            ),
        );
        painter.rect_filled(r, 2.0, col);
        if r.width() > 14.0 && r.height() > 12.0 {
            painter.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                tag,
                egui::FontId::proportional(9.5),
                Color32::from_white_alpha(220),
            );
        }
    }

    painter.text(
        Pos2::new(rect.center().x, rect.max.y - 10.5),
        egui::Align2::CENTER_CENTER,
        layout.label(),
        egui::FontId::proportional(10.0),
        Color32::WHITE,
    );

    resp.clicked()
}
