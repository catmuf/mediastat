//! Storage Tier Analytics & Cloud/NAS Cost Estimator (WinDirStat 2.10 `StorageAnalyticsView` parity).

use egui::{Color32, RichText, Stroke, Ui};
use mediastat_core::{format_bytes, format_count, t, AppSettings, NodeFlags, ScanTree};

const TIB: f64 = 1_099_511_627_776.0;

pub fn show_storage_analytics(ui: &mut Ui, tree: &ScanTree, settings: &mut AppSettings) {
    let summary = tree.summary();
    let now = chrono::Utc::now().timestamp();

    let tier_count = settings.storage_tiers.len();
    let mut tier_files = vec![0u64; tier_count];
    let mut tier_bytes = vec![0u64; tier_count];

    let mut order: Vec<usize> = (0..tier_count).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(settings.storage_tiers[i].min_age_days));

    for i in 1..tree.len() {
        let f = tree.flags[i];
        if f.contains(NodeFlags::IS_DIR) || f.contains(NodeFlags::IS_DELETED) {
            continue;
        }
        let age_days = if tree.mtimes[i] > 0 {
            ((now - tree.mtimes[i]).max(0) / 86_400) as u32
        } else {
            0
        };
        if let Some(&ti) = order
            .iter()
            .find(|&&ti| age_days >= settings.storage_tiers[ti].min_age_days)
        {
            tier_files[ti] += 1;
            tier_bytes[ti] = tier_bytes[ti].saturating_add(tree.logical_sizes[i]);
        }
    }

    let total_bytes = summary.total_logical_bytes.max(1);
    let total_monthly_cost: f64 = settings
        .storage_tiers
        .iter()
        .enumerate()
        .map(|(i, t)| (tier_bytes[i] as f64 / TIB) * t.cost_per_tib_monthly)
        .sum();

    let reclaim_dupes = summary.duplicate_reclaimable_bytes;
    let monthly_savings = (reclaim_dupes as f64 / TIB) * settings.default_cost_per_tib_monthly;
    let cur = settings.currency_symbol.clone();

    egui::ScrollArea::vertical().show(ui, |ui| {
        let mo = t("mes", "mo");
        let yr = t("año", "yr");
        ui.horizontal_wrapped(|ui| {
            ui.heading(t(
                "📊 Analítica de Tiers de Almacenamiento y Costes",
                "📊 Storage Tier & Cost Analytics",
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(t("Moneda:", "Currency:"));
                ui.add(egui::TextEdit::singleline(&mut settings.currency_symbol).desired_width(32.0));
                ui.label(t("Coste medio/TiB/mes:", "Avg cost/TiB/mo:"));
                ui.add(
                    egui::DragValue::new(&mut settings.default_cost_per_tib_monthly)
                        .speed(0.5)
                        .range(0.0..=1000.0)
                        .suffix(format!(" {cur}")),
                );
            });
        });
        ui.add_space(6.0);

        // Executive financial summary strip
        ui.horizontal_wrapped(|ui| {
            finance_card(
                ui,
                t("Coste Mensual Actual (Tiers)", "Current Monthly Cost (Tiers)"),
                &format!("{total_monthly_cost:.2} {cur}/{mo}"),
                &format!(
                    "{:.2} {cur}/{yr} · {}",
                    total_monthly_cost * 12.0,
                    format_bytes(summary.total_logical_bytes)
                ),
                Color32::from_rgb(76, 166, 255),
            );
            finance_card(
                ui,
                t("Ahorro por Limpiar Duplicados", "Duplicate Cleanup Savings"),
                &format!("{monthly_savings:.2} {cur}/{mo}"),
                &format!(
                    "{} {} {} {}",
                    format_bytes(reclaim_dupes),
                    t("en", "across"),
                    format_count(summary.duplicate_groups),
                    t("grupos", "groups")
                ),
                Color32::from_rgb(185, 130, 245),
            );
            finance_card(
                ui,
                t("Ahorro Anual Proyectado", "Projected Annual Savings"),
                &format!("{:.2} {cur}/{yr}", monthly_savings * 12.0),
                &format!(
                    "{} {}",
                    format_bytes(reclaim_dupes),
                    t("recuperables en disco", "reclaimable on disk")
                ),
                Color32::from_rgb(76, 210, 128),
            );
        });

        ui.add_space(12.0);
        ui.strong(t(
            "Distribución por Antigüedad de Última Modificación (Hot → Archive):",
            "Distribution by Last Modified Age (Hot → Archive):",
        ));
        ui.add_space(6.0);

        // Stacked visual bar across all tiers
        let (bar_rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().max(100.0), 26.0),
            egui::Sense::hover(),
        );
        let painter = ui.painter();
        painter.rect_filled(bar_rect, 4.0, Color32::from_gray(38));
        let mut x_cur = bar_rect.min.x;
        for (i, t) in settings.storage_tiers.iter().enumerate() {
            let frac = (tier_bytes[i] as f64 / total_bytes as f64) as f32;
            let seg_w = bar_rect.width() * frac;
            if seg_w > 1.0 {
                let [r, g, b] = t.accent_rgb;
                let sr = egui::Rect::from_min_size(
                    egui::pos2(x_cur, bar_rect.min.y),
                    egui::vec2(seg_w, bar_rect.height()),
                );
                painter.rect_filled(sr, 2.0, Color32::from_rgb(r, g, b));
                if seg_w > 54.0 {
                    painter.text(
                        sr.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("{:.0}%", frac * 100.0),
                        egui::FontId::proportional(11.0),
                        Color32::WHITE,
                    );
                }
                x_cur += seg_w;
            }
        }

        ui.add_space(10.0);

        // Editable Tier cards
        for (i, tier) in settings.storage_tiers.iter_mut().enumerate() {
            let [r, g, b] = tier.accent_rgb;
            let accent = Color32::from_rgb(r, g, b);
            let bytes = tier_bytes[i];
            let pct = (bytes as f64 / total_bytes as f64) * 100.0;
            let cost_mo = (bytes as f64 / TIB) * tier.cost_per_tib_monthly;

            egui::Frame::NONE
                .fill(Color32::from_rgba_unmultiplied(r, g, b, 20))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(r, g, b, 110)))
                .corner_radius(6.0)
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&tier.name).strong().size(14.0).color(accent));
                        ui.separator();
                        ui.label(t("Desde días:", "Min age (days):"));
                        ui.add(egui::DragValue::new(&mut tier.min_age_days).range(0..=10_000));
                        ui.label(t("Coste TiB/mes:", "Cost/TiB/mo:"));
                        ui.add(
                            egui::DragValue::new(&mut tier.cost_per_tib_monthly)
                                .speed(0.5)
                                .range(0.0..=500.0)
                                .suffix(format!(" {cur}")),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{cost_mo:.2} {cur}/{mo}"))
                                    .strong()
                                    .color(accent),
                            );
                            ui.separator();
                            ui.label(format!(
                                "{} ({pct:.1}%) · {} {}",
                                format_bytes(bytes),
                                format_count(tier_files[i]),
                                t("archivos", "files")
                            ));
                        });
                    });
                });
            ui.add_space(4.0);
        }
    });
}

fn finance_card(ui: &mut Ui, title: &str, big_val: &str, subtitle: &str, accent: Color32) {
    egui::Frame::NONE
        .fill(Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 24))
        .stroke(Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 120),
        ))
        .corner_radius(8.0)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_min_width(185.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(title).small().weak());
                ui.label(RichText::new(big_val).size(18.0).strong().color(accent));
                ui.label(RichText::new(subtitle).small());
            });
        });
}
