//! Interactive Hierarchical Flame Graph Visualization (WinDirStat 2.10 `FlameGraphView` + `CFolderColors` parity).

use egui::{Color32, Pos2, Rect, RichText, Stroke, Ui};
use mediastat_core::{
    apply_saturation, format_bytes, free_space_color, is_light_rgb, t, unknown_space_color,
    NodeFlags, NodeId, ScanTree, SizeMetric, TreemapOptions, VizColorMode,
};

use crate::sunburst::{folder_branch_color, VizAction};
use crate::treemap::{pick_node_color, SpecialTile};

#[derive(Debug, Clone, Copy)]
struct FlameBar {
    rect: Rect,
    item: SpecialTile,
    bytes: u64,
    rgb: [u8; 3],
}

const MAX_FLAME_DEPTH: usize = 12;

/// Render an interactive bottom-up Flame Graph of disk usage by directory depth.
#[allow(clippy::too_many_arguments)]
pub fn show_flame_graph(
    ui: &mut Ui,
    tree: &ScanTree,
    root_node: NodeId,
    selected: Option<NodeId>,
    opts: &TreemapOptions,
    metric: SizeMetric,
    show_free: bool,
    show_unknown: bool,
) -> Option<VizAction> {
    let avail = ui.available_size();
    let (rect, response) = ui.allocate_exact_size(avail, egui::Sense::click());
    if rect.width() < 40.0 || rect.height() < 40.0 || tree.is_empty() {
        return None;
    }

    let row_h = (rect.height() / MAX_FLAME_DEPTH as f32).clamp(16.0, 26.0);
    let max_levels = ((rect.height() / row_h).floor() as usize).clamp(1, MAX_FLAME_DEPTH);

    let now_unix = chrono::Utc::now().timestamp();
    let bg = ui.visuals().panel_fill;
    let bg_rgb = [bg.r(), bg.g(), bg.b()];

    let mut bars = Vec::with_capacity(512);
    let root_bytes = compute_node_plus_special(tree, root_node, metric, show_free, show_unknown);
    if root_bytes == 0 {
        return None;
    }

    // Level 0 = bottom bar representing zoom root
    let y0 = rect.max.y - row_h;
    bars.push(FlameBar {
        rect: Rect::from_min_max(
            Pos2::new(rect.min.x, y0),
            Pos2::new(rect.max.x, rect.max.y - 1.0),
        ),
        item: SpecialTile::Normal(root_node),
        bytes: root_bytes,
        rgb: [52, 84, 126],
    });

    collect_flame_bars(
        tree,
        root_node,
        1,
        max_levels,
        None,
        rect.min.x,
        rect.max.x,
        rect.max.y,
        row_h,
        opts,
        metric,
        show_free,
        show_unknown,
        now_unix,
        bg_rgb,
        &mut bars,
    );

    let painter = ui.painter_at(rect);
    let [hr, hg, hb] = opts.highlight_color;
    let sel_stroke = Stroke::new(2.0_f32, Color32::from_rgb(hr, hg, hb));

    let hovered = response
        .hover_pos()
        .and_then(|p| bars.iter().rev().find(|b| b.rect.contains(p)).copied());

    for bar in &bars {
        let is_sel = matches!(bar.item, SpecialTile::Normal(id) if Some(id) == selected);
        let is_hov = hovered.is_some_and(|h| h.rect == bar.rect);
        let boost = if is_hov { 1.20 } else { 1.0 };
        let fill = Color32::from_rgb(
            (bar.rgb[0] as f32 * boost).min(255.0) as u8,
            (bar.rgb[1] as f32 * boost).min(255.0) as u8,
            (bar.rgb[2] as f32 * boost).min(255.0) as u8,
        );
        let stroke = if is_sel {
            sel_stroke
        } else if is_hov {
            Stroke::new(1.5_f32, Color32::from_rgb(255, 240, 120))
        } else {
            Stroke::new(0.7_f32, Color32::from_rgb(18, 22, 28))
        };

        painter.rect_filled(bar.rect, 2.0, fill);
        painter.rect_stroke(bar.rect, 2.0, stroke, egui::StrokeKind::Inside);

        let w = bar.rect.width();
        if w > 36.0 {
            let max_ch = ((w - 8.0) / 6.5).floor() as usize;
            let raw_name = match bar.item {
                SpecialTile::Normal(id) => tree.name(id),
                SpecialTile::FreeSpace(_, _) => t("<Espacio Libre>", "<Free Space>"),
                SpecialTile::UnknownSpace(_, _) => t("<Espacio Desconocido>", "<Unknown Space>"),
            };
            let label = if raw_name.chars().count() <= max_ch {
                raw_name.to_string()
            } else {
                let s: String = raw_name.chars().take(max_ch.saturating_sub(1)).collect();
                format!("{s}…")
            };
            let light = opts.contrast_labels && is_light_rgb([fill.r(), fill.g(), fill.b()]);
            let fg = if light {
                Color32::from_rgb(18, 22, 30)
            } else {
                Color32::WHITE
            };
            let tp = Pos2::new(bar.rect.min.x + 4.0, bar.rect.center().y);
            if !light {
                painter.text(
                    tp + egui::vec2(1.0, 1.0),
                    egui::Align2::LEFT_CENTER,
                    &label,
                    egui::FontId::proportional(11.0),
                    Color32::from_black_alpha(150),
                );
            }
            painter.text(
                tp,
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(11.0),
                fg,
            );
        }
    }

    let mut action = None;
    if let Some(bar) = hovered {
        let pct = (bar.bytes as f64 / root_bytes.max(1) as f64) * 100.0;
        let (title, path_str) = match bar.item {
            SpecialTile::Normal(id) => (
                tree.name(id).to_string(),
                tree.resolve_path(id).to_string_lossy().into_owned(),
            ),
            SpecialTile::FreeSpace(_, _) => (
                t("<Espacio Libre>", "<Free Space>").into(),
                tree.name(root_node).into(),
            ),
            SpecialTile::UnknownSpace(_, _) => (
                t("<Espacio Desconocido>", "<Unknown Space>").into(),
                tree.name(root_node).into(),
            ),
        };
        response.clone().on_hover_ui_at_pointer(|ui| {
            ui.strong(&title);
            ui.label(RichText::new(&path_str).small().monospace());
            ui.label(format!("{} ({pct:.2}%)", format_bytes(bar.bytes)));
        });
        action = Some(VizAction::HoverText(format!(
            "{path_str} ({} · {pct:.2}%)",
            format_bytes(bar.bytes)
        )));

        if let SpecialTile::Normal(id) = bar.item {
            if response.double_clicked() {
                if id == root_node && root_node != NodeId::ROOT {
                    action = Some(VizAction::ZoomOut);
                } else if tree.is_dir(id) {
                    action = Some(VizAction::ZoomInto(id));
                }
            } else if response.clicked() {
                action = Some(VizAction::Select(id));
            }
        }
    }
    action
}

fn compute_node_plus_special(
    tree: &ScanTree,
    node: NodeId,
    metric: SizeMetric,
    show_free: bool,
    show_unknown: bool,
) -> u64 {
    let mut tot = tree.size_for_metric(node, metric);
    if let Some(vs) = tree.volume_space.get(&node) {
        if show_free {
            tot = tot.saturating_add(vs.free_bytes);
        }
        if show_unknown {
            tot = tot.saturating_add(vs.unknown_bytes(tree.allocated_sizes[node.index()]));
        }
    }
    tot
}

#[allow(clippy::too_many_arguments)]
fn collect_flame_bars(
    tree: &ScanTree,
    parent: NodeId,
    level: usize,
    max_levels: usize,
    branch_idx: Option<usize>,
    x0: f32,
    x1: f32,
    bottom_y: f32,
    row_h: f32,
    opts: &TreemapOptions,
    metric: SizeMetric,
    show_free: bool,
    show_unknown: bool,
    now_unix: i64,
    bg_rgb: [u8; 3],
    out: &mut Vec<FlameBar>,
) {
    if level >= max_levels || (x1 - x0) < 3.0 {
        return;
    }

    let mut kids: Vec<(SpecialTile, u64)> = tree
        .children(parent)
        .filter_map(|c| {
            if tree.is_deleted(c) {
                return None;
            }
            if metric == SizeMetric::AllocatedOnDisk
                && !tree.flags[c.index()].contains(NodeFlags::IS_DIR)
                && tree.flags[c.index()].contains(NodeFlags::IS_HARDLINK_DUP)
            {
                return None;
            }
            let sz = tree.size_for_metric(c, metric);
            if sz > 0 {
                Some((SpecialTile::Normal(c), sz))
            } else {
                None
            }
        })
        .collect();

    if let Some(vs) = tree.volume_space.get(&parent) {
        if show_free && vs.free_bytes > 0 {
            kids.push((SpecialTile::FreeSpace(parent, vs.free_bytes), vs.free_bytes));
        }
        if show_unknown {
            let unk = vs.unknown_bytes(tree.allocated_sizes[parent.index()]);
            if unk > 0 {
                kids.push((SpecialTile::UnknownSpace(parent, unk), unk));
            }
        }
    }

    if kids.is_empty() {
        return;
    }
    kids.sort_unstable_by_key(|k| std::cmp::Reverse(k.1));

    let total: u64 = kids.iter().map(|(_, s)| *s).sum();
    if total == 0 {
        return;
    }

    let span = x1 - x0;
    let bar_y1 = bottom_y - (level as f32) * row_h - 1.0;
    let bar_y0 = bar_y1 - row_h + 2.0;

    let mut cx = x0;
    for (sibling_rank, (item, sz)) in kids.into_iter().enumerate() {
        let w = (span as f64 * (sz as f64 / total as f64)) as f32;
        if w < 2.0 {
            break;
        }
        let nx = (cx + w).min(x1);
        let child_branch = branch_idx.unwrap_or(sibling_rank);
        let rgb = match item {
            SpecialTile::Normal(id) => {
                if opts.color_mode == VizColorMode::ByExtension
                    && tree.flags[id.index()].contains(NodeFlags::IS_DIR)
                {
                    folder_branch_color(child_branch, level as u8, bg_rgb, opts.saturation)
                } else {
                    apply_saturation(pick_node_color(tree, id, opts, now_unix), opts.saturation)
                }
            }
            SpecialTile::FreeSpace(_, _) => apply_saturation(free_space_color(opts), opts.saturation),
            SpecialTile::UnknownSpace(_, _) => {
                apply_saturation(unknown_space_color(opts), opts.saturation)
            }
        };
        out.push(FlameBar {
            rect: Rect::from_min_max(Pos2::new(cx, bar_y0), Pos2::new(nx, bar_y1)),
            item,
            bytes: sz,
            rgb,
        });

        if let SpecialTile::Normal(cid) = item {
            if tree.flags[cid.index()].contains(NodeFlags::IS_DIR) {
                collect_flame_bars(
                    tree,
                    cid,
                    level + 1,
                    max_levels,
                    Some(child_branch),
                    cx,
                    nx,
                    bottom_y,
                    row_h,
                    opts,
                    metric,
                    show_free,
                    show_unknown,
                    now_unix,
                    bg_rgb,
                    out,
                );
            }
        }
        cx = nx;
    }
}
