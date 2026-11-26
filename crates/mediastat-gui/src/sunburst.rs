//! Interactive Radial Sunburst Visualization (WinDirStat 2.10 `SunburstView` + `CFolderColors` parity).

use std::f32::consts::TAU;

use egui::{Color32, Pos2, RichText, Shape, Stroke, Ui};
use mediastat_core::{
    apply_saturation, blend_rgb, format_bytes, free_space_color, is_light_rgb, t,
    unknown_space_color, FileCategory, NodeFlags, NodeId, ScanTree, SizeMetric, TreemapOptions,
    VizColorMode, WINDIRSTAT_FOLDER_BRANCH_8,
};

use crate::treemap::{pick_node_color, SpecialTile};

#[derive(Debug, Clone, Copy)]
pub struct SunburstArc {
    pub depth: u8,
    pub angle0: f32,
    pub angle1: f32,
    pub item: SpecialTile,
    pub bytes: u64,
    pub rgb: [u8; 3],
}

pub enum VizAction {
    Select(NodeId),
    ZoomInto(NodeId),
    ZoomOut,
    HoverText(String),
}

const MAX_SUNBURST_RINGS: u8 = 5;

/// Exact WinDirStat `CFolderColors::GetColor` branch color with depth fade and saturation.
pub fn folder_branch_color(
    branch_idx: usize,
    depth: u8,
    bg_rgb: [u8; 3],
    saturation: f32,
) -> [u8; 3] {
    let base = WINDIRSTAT_FOLDER_BRANCH_8[branch_idx % WINDIRSTAT_FOLDER_BRANCH_8.len()];
    let blend = (0.22 + (depth.min(6) as f32) * 0.06).clamp(0.0, 0.72);
    apply_saturation(blend_rgb(base, bg_rgb, blend), saturation)
}

/// Render an interactive concentric radial Sunburst chart for `root_node`.
#[allow(clippy::too_many_arguments)]
pub fn show_sunburst(
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

    let center = rect.center();
    let max_radius = (rect.width().min(rect.height()) * 0.47).max(20.0);
    let inner_hub = max_radius * 0.20;
    let ring_w = (max_radius - inner_hub) / MAX_SUNBURST_RINGS as f32;

    let now_unix = chrono::Utc::now().timestamp();
    let bg = ui.visuals().panel_fill;
    let bg_rgb = [bg.r(), bg.g(), bg.b()];

    let mut arcs = Vec::with_capacity(512);
    collect_arcs(
        tree,
        root_node,
        0,
        None,
        0.0,
        TAU,
        opts,
        metric,
        show_free,
        show_unknown,
        now_unix,
        bg_rgb,
        &mut arcs,
    );

    let painter = ui.painter_at(rect);
    let [hr, hg, hb] = opts.highlight_color;
    let sel_stroke = Stroke::new(2.0_f32, Color32::from_rgb(hr, hg, hb));

    // Hit-test pointer in polar coordinates
    let mut hovered_arc: Option<SunburstArc> = None;
    let mut hover_hub = false;
    if let Some(pos) = response.hover_pos() {
        let dx = pos.x - center.x;
        let dy = pos.y - center.y;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < inner_hub {
            hover_hub = true;
        } else if dist <= max_radius {
            let depth = ((dist - inner_hub) / ring_w).floor() as u8;
            let mut angle = dy.atan2(dx);
            if angle < 0.0 {
                angle += TAU;
            }
            hovered_arc = arcs
                .iter()
                .find(|a| a.depth == depth && angle >= a.angle0 && angle <= a.angle1)
                .copied();
        }
    }

    // Draw concentric wedges
    for arc in &arcs {
        let r0 = inner_hub + arc.depth as f32 * ring_w + 1.0;
        let r1 = r0 + ring_w - 1.5;
        let is_sel = matches!(arc.item, SpecialTile::Normal(id) if Some(id) == selected);
        let is_hov = hovered_arc
            .is_some_and(|h| h.depth == arc.depth && (h.angle0 - arc.angle0).abs() < 1e-4);

        let boost = if is_hov { 1.22 } else { 1.0 };
        let fill = Color32::from_rgb(
            (arc.rgb[0] as f32 * boost).min(255.0) as u8,
            (arc.rgb[1] as f32 * boost).min(255.0) as u8,
            (arc.rgb[2] as f32 * boost).min(255.0) as u8,
        );
        let stroke = if is_sel {
            sel_stroke
        } else if is_hov {
            Stroke::new(1.5_f32, Color32::from_rgb(255, 240, 120))
        } else {
            Stroke::new(0.6_f32, Color32::from_rgb(20, 24, 30))
        };

        draw_ring_sector(&painter, center, r0, r1, arc.angle0, arc.angle1, fill, stroke);

        // Draw WCAG contrast label on large wedges (including Free/Unknown labels)
        let span = arc.angle1 - arc.angle0;
        if span > 0.24 && ring_w > 18.0 {
            let mid_a = (arc.angle0 + arc.angle1) * 0.5;
            let mid_r = (r0 + r1) * 0.5;
            let lp = Pos2::new(center.x + mid_a.cos() * mid_r, center.y + mid_a.sin() * mid_r);
            let lbl = match arc.item {
                SpecialTile::Normal(id) => truncate_label(tree.name(id), 14),
                SpecialTile::FreeSpace(_, _) => t("<Libre>", "<Free>").into(),
                SpecialTile::UnknownSpace(_, _) => t("<Descon.>", "<Unknown>").into(),
            };
            let light = opts.contrast_labels && is_light_rgb([fill.r(), fill.g(), fill.b()]);
            let fg = if light {
                Color32::from_rgb(18, 22, 30)
            } else {
                Color32::WHITE
            };
            if !light {
                painter.text(
                    lp + egui::vec2(1.0, 1.0),
                    egui::Align2::CENTER_CENTER,
                    &lbl,
                    egui::FontId::proportional(10.5),
                    Color32::from_black_alpha(150),
                );
            }
            painter.text(
                lp,
                egui::Align2::CENTER_CENTER,
                lbl,
                egui::FontId::proportional(10.5),
                fg,
            );
        }
    }

    // Center hub (click to Zoom Out)
    let hub_fill = if hover_hub && root_node != NodeId::ROOT {
        Color32::from_rgb(56, 86, 128)
    } else {
        Color32::from_rgb(30, 35, 44)
    };
    painter.circle_filled(center, inner_hub - 1.0, hub_fill);
    painter.circle_stroke(
        center,
        inner_hub - 1.0,
        Stroke::new(1.2_f32, Color32::from_gray(90)),
    );
    let root_sz = tree.size_for_metric(root_node, metric);
    painter.text(
        center - egui::vec2(0.0, 7.0),
        egui::Align2::CENTER_CENTER,
        truncate_label(tree.name(root_node), 14),
        egui::FontId::proportional(11.5),
        Color32::WHITE,
    );
    painter.text(
        center + egui::vec2(0.0, 8.0),
        egui::Align2::CENTER_CENTER,
        format_bytes(root_sz),
        egui::FontId::proportional(10.5),
        Color32::from_rgb(160, 200, 240),
    );

    let mut action = None;

    if hover_hub && response.clicked() && root_node != NodeId::ROOT {
        return Some(VizAction::ZoomOut);
    }

    if let Some(arc) = hovered_arc {
        let total_root = root_sz.max(1) as f64;
        let pct = (arc.bytes as f64 / total_root) * 100.0;
        let (title, path_str, extra) = match arc.item {
            SpecialTile::Normal(id) => {
                let extra = if tree.is_dir(id) {
                    let fcnt = tree.dir_stats.get(&id).map(|d| d.total_files).unwrap_or(0);
                    format!("📁 {} ({} {})", t("Carpeta", "Folder"), mediastat_core::format_count(fcnt), t("fich.", "files"))
                } else {
                    let cat = FileCategory::from_extension(tree.ext(id));
                    format!("{} {}", cat.icon(), cat.label())
                };
                (
                    tree.name(id).to_string(),
                    tree.resolve_path(id).to_string_lossy().into_owned(),
                    extra,
                )
            }
            SpecialTile::FreeSpace(_, b) => (
                t("<Espacio Libre>", "<Free Space>").into(),
                format!("{} {}", t("Volumen", "Volume"), tree.name(root_node)),
                format!("{}: {}", t("Libre disponible", "Available free"), format_bytes(b)),
            ),
            SpecialTile::UnknownSpace(_, b) => (
                t("<Espacio Desconocido>", "<Unknown Space>").into(),
                format!("{} {}", t("Volumen", "Volume"), tree.name(root_node)),
                format!(
                    "{}: {}",
                    t("VSS / $MFT / Sistema / Sin permisos", "VSS / $MFT / System / Access Denied"),
                    format_bytes(b)
                ),
            ),
        };

        response.clone().on_hover_ui_at_pointer(|ui| {
            ui.strong(&title);
            ui.label(RichText::new(&path_str).small().monospace());
            ui.label(format!("{} ({pct:.2}%)", format_bytes(arc.bytes)));
            ui.label(RichText::new(&extra).small().weak());
        });

        action = Some(VizAction::HoverText(format!(
            "{path_str} ({} · {pct:.2}%)",
            format_bytes(arc.bytes)
        )));

        if let SpecialTile::Normal(id) = arc.item {
            if response.double_clicked() && tree.is_dir(id) {
                action = Some(VizAction::ZoomInto(id));
            } else if response.clicked() {
                action = Some(VizAction::Select(id));
            }
        }
    }

    action
}

#[allow(clippy::too_many_arguments)]
fn collect_arcs(
    tree: &ScanTree,
    node: NodeId,
    depth: u8,
    branch_idx: Option<usize>,
    a0: f32,
    a1: f32,
    opts: &TreemapOptions,
    metric: SizeMetric,
    show_free: bool,
    show_unknown: bool,
    now_unix: i64,
    bg_rgb: [u8; 3],
    out: &mut Vec<SunburstArc>,
) {
    if depth >= MAX_SUNBURST_RINGS || (a1 - a0) < 0.012 {
        return;
    }

    let mut kids: Vec<(SpecialTile, u64)> = tree
        .children(node)
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

    if let Some(vs) = tree.volume_space.get(&node) {
        if show_free && vs.free_bytes > 0 {
            kids.push((SpecialTile::FreeSpace(node, vs.free_bytes), vs.free_bytes));
        }
        if show_unknown {
            let unk = vs.unknown_bytes(tree.allocated_sizes[node.index()]);
            if unk > 0 {
                kids.push((SpecialTile::UnknownSpace(node, unk), unk));
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

    let span = a1 - a0;
    let mut cur = a0;
    for (sibling_rank, (item, sz)) in kids.into_iter().enumerate() {
        let frac = (sz as f64 / total as f64) as f32;
        let next = cur + span * frac;
        if (next - cur) < 0.008 {
            break;
        }
        let child_branch = branch_idx.unwrap_or(sibling_rank);
        let rgb = match item {
            SpecialTile::Normal(id) => {
                if opts.color_mode == VizColorMode::ByExtension
                    && tree.flags[id.index()].contains(NodeFlags::IS_DIR)
                {
                    folder_branch_color(child_branch, depth, bg_rgb, opts.saturation)
                } else {
                    apply_saturation(pick_node_color(tree, id, opts, now_unix), opts.saturation)
                }
            }
            SpecialTile::FreeSpace(_, _) => apply_saturation(free_space_color(opts), opts.saturation),
            SpecialTile::UnknownSpace(_, _) => {
                apply_saturation(unknown_space_color(opts), opts.saturation)
            }
        };
        out.push(SunburstArc {
            depth,
            angle0: cur,
            angle1: next,
            item,
            bytes: sz,
            rgb,
        });
        if let SpecialTile::Normal(cid) = item {
            if tree.flags[cid.index()].contains(NodeFlags::IS_DIR) {
                collect_arcs(
                    tree,
                    cid,
                    depth + 1,
                    Some(child_branch),
                    cur,
                    next,
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
        cur = next;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_ring_sector(
    painter: &egui::Painter,
    center: Pos2,
    r0: f32,
    r1: f32,
    a0: f32,
    a1: f32,
    fill: Color32,
    stroke: Stroke,
) {
    let span = (a1 - a0).max(0.001);
    let steps = ((span / 0.10).ceil() as usize).clamp(2, 48);
    let mut pts = Vec::with_capacity((steps + 1) * 2);

    for i in 0..=steps {
        let a = a0 + span * (i as f32 / steps as f32);
        pts.push(Pos2::new(center.x + a.cos() * r1, center.y + a.sin() * r1));
    }
    for i in (0..=steps).rev() {
        let a = a0 + span * (i as f32 / steps as f32);
        pts.push(Pos2::new(center.x + a.cos() * r0, center.y + a.sin() * r0));
    }

    painter.add(Shape::convex_polygon(pts.clone(), fill, Stroke::NONE));
    painter.add(Shape::closed_line(pts, stroke));
}

fn truncate_label(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let t: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{t}…")
    }
}
