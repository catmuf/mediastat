use egui::{Color32, ColorImage, Context, Rect, TextureHandle, TextureOptions};
use mediastat_core::{
    age_color_for_palette, apply_saturation, category_color_for_palette,
    extension_color_for_palette, free_space_color, make_bright_color, normalize_color,
    unknown_space_color, windirstat_rank_color, FileCategory, NodeFlags, NodeId, ScanTree,
    SizeMetric, TreemapLayoutStyle, TreemapOptions, VizColorMode, VizColorPalette,
    GRAPH_PALETTE_BRIGHTNESS,
};
use rayon::prelude::*;

pub type TreemapColorMode = VizColorMode;

const HIT_GRID_DIM: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialTile {
    Normal(NodeId),
    FreeSpace(NodeId, u64),
    UnknownSpace(NodeId, u64),
}

/// Parabolic surface coefficients for Van Wijk & Van de Wetering Cushion Treemap shading
/// matching upstream `windirstat/windirstat` `Controls/TreeMap.cpp::AddRidge`.
#[derive(Debug, Clone, Copy, Default)]
struct CushionSurface {
    x2: f32,
    x1: f32,
    y2: f32,
    y1: f32,
}

impl CushionSurface {
    /// Exact WinDirStat `AddRidge` — scale-invariant edge slope `4 * h` regardless of tile width/height.
    #[inline]
    fn add_ridge(&mut self, x0: f32, x1: f32, y0: f32, y1: f32, height: f32) {
        if height <= 0.0005 {
            return;
        }
        let dx = (x1 - x0).max(1.0);
        let dy = (y1 - y0).max(1.0);
        let h4 = 4.0 * height;
        let wf = h4 / dx;
        let hf = h4 / dy;
        self.x2 -= wf;
        self.x1 += wf * (x0 + x1);
        self.y2 -= hf;
        self.y1 += hf * (y0 + y1);
    }
}

/// A laid-out leaf rectangle in the current Treemap viewport.
#[derive(Debug, Clone, Copy)]
pub struct TreemapLeaf {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub item: SpecialTile,
    pub parent_dir: NodeId,
    pub ext_id: u16,
    pub rgb: [u8; 3],
    surface: CushionSurface,
}

impl TreemapLeaf {
    pub fn node_id(&self) -> Option<NodeId> {
        match self.item {
            SpecialTile::Normal(id) => Some(id),
            _ => None,
        }
    }
}

/// Cached GPU-backed Multi-Algorithm Cushion Treemap renderer (Squarified, KDirStat, Hilbert, Moore)
/// with exact WinDirStat `CColorSpace::NormalizeColor` Cushion math & 16×16 O(1) spatial hit-test grid.
pub struct TreemapCache {
    pub root_node: NodeId,
    pub color_mode: TreemapColorMode,
    pub width: usize,
    pub height: usize,
    pub leaves: Vec<TreemapLeaf>,
    pub folder_frames: Vec<(f32, f32, f32, f32, NodeId)>,
    hit_grid: Vec<Vec<usize>>,
    pub texture: Option<TextureHandle>,
    pub dirty: bool,
    pub tree_generation: u64,
}

impl Default for TreemapCache {
    fn default() -> Self {
        Self {
            root_node: NodeId::ROOT,
            color_mode: TreemapColorMode::ByExtension,
            width: 0,
            height: 0,
            leaves: Vec::new(),
            folder_frames: Vec::new(),
            hit_grid: Vec::new(),
            texture: None,
            dirty: true,
            tree_generation: 0,
        }
    }
}

impl TreemapCache {
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ensure_rendered(
        &mut self,
        ctx: &Context,
        tree: &ScanTree,
        tree_gen: u64,
        width: usize,
        height: usize,
        opts: &TreemapOptions,
        metric: SizeMetric,
        show_free: bool,
        show_unknown: bool,
    ) {
        let w = width.clamp(64, 2400);
        let h = height.clamp(64, 1400);
        if !self.dirty
            && self.width == w
            && self.height == h
            && self.tree_generation == tree_gen
            && self.texture.is_some()
        {
            return;
        }

        self.width = w;
        self.height = h;
        self.color_mode = opts.color_mode;
        self.tree_generation = tree_gen;
        self.dirty = false;
        self.leaves.clear();
        self.folder_frames.clear();

        if tree.is_empty() || self.root_node.index() >= tree.len() {
            self.hit_grid.clear();
            self.texture = None;
            return;
        }

        let now_unix = chrono::Utc::now().timestamp();

        let initial_surface = CushionSurface::default();
        layout_recursive(
            tree,
            self.root_node,
            0.0,
            0.0,
            w as f32,
            h as f32,
            initial_surface,
            opts.cushion_height,
            0,
            opts,
            metric,
            show_free,
            show_unknown,
            now_unix,
            &mut self.leaves,
            &mut self.folder_frames,
        );

        self.rebuild_hit_grid(w, h);

        let pixels = rasterize_cushions(&self.leaves, w, h, opts);
        let color_img = ColorImage::from_rgba_unmultiplied([w, h], &pixels);
        self.texture = Some(ctx.load_texture(
            "mediastat_cushion_treemap",
            color_img,
            TextureOptions::NEAREST,
        ));
    }

    fn rebuild_hit_grid(&mut self, w: usize, h: usize) {
        let cells = HIT_GRID_DIM * HIT_GRID_DIM;
        if self.hit_grid.len() != cells {
            self.hit_grid = vec![Vec::new(); cells];
        } else {
            for cell in &mut self.hit_grid {
                cell.clear();
            }
        }
        let fw = w.max(1) as f32;
        let fh = h.max(1) as f32;
        let dim_f = HIT_GRID_DIM as f32;

        for (idx, leaf) in self.leaves.iter().enumerate() {
            let gx0 = ((leaf.x0 / fw) * dim_f).floor().clamp(0.0, dim_f - 1.0) as usize;
            let gx1 = ((leaf.x1 / fw) * dim_f).floor().clamp(0.0, dim_f - 1.0) as usize;
            let gy0 = ((leaf.y0 / fh) * dim_f).floor().clamp(0.0, dim_f - 1.0) as usize;
            let gy1 = ((leaf.y1 / fh) * dim_f).floor().clamp(0.0, dim_f - 1.0) as usize;
            for gy in gy0..=gy1 {
                let row = gy * HIT_GRID_DIM;
                for gx in gx0..=gx1 {
                    self.hit_grid[row + gx].push(idx);
                }
            }
        }
    }

    /// O(1) spatial grid hit-test (falls back to linear scan if grid is empty).
    pub fn hit_test(&self, px: f32, py: f32) -> Option<&TreemapLeaf> {
        if self.width == 0 || self.height == 0 || self.leaves.is_empty() {
            return None;
        }
        if self.hit_grid.len() == HIT_GRID_DIM * HIT_GRID_DIM {
            let dim_f = HIT_GRID_DIM as f32;
            let gx = ((px / self.width as f32) * dim_f)
                .floor()
                .clamp(0.0, dim_f - 1.0) as usize;
            let gy = ((py / self.height as f32) * dim_f)
                .floor()
                .clamp(0.0, dim_f - 1.0) as usize;
            let bucket = &self.hit_grid[gy * HIT_GRID_DIM + gx];
            for &idx in bucket.iter().rev() {
                if let Some(l) = self.leaves.get(idx) {
                    if px >= l.x0 && px <= l.x1 && py >= l.y0 && py <= l.y1 {
                        return Some(l);
                    }
                }
            }
            return None;
        }
        self.leaves
            .iter()
            .rev()
            .find(|l| px >= l.x0 && px <= l.x1 && py >= l.y0 && py <= l.y1)
    }

    pub fn bounds_of_node(&self, tree: &ScanTree, target: NodeId) -> Option<(f32, f32, f32, f32)> {
        if target == NodeId::ROOT || target.is_none() {
            return None;
        }
        let mut x0 = f32::MAX;
        let mut y0 = f32::MAX;
        let mut x1 = f32::MIN;
        let mut y1 = f32::MIN;
        let mut found = false;

        let is_dir = tree.is_dir(target);
        for leaf in &self.leaves {
            let Some(nid) = leaf.node_id() else {
                continue;
            };
            let matches = if !is_dir {
                nid == target
            } else {
                is_descendant_of(tree, nid, target)
            };
            if matches {
                x0 = x0.min(leaf.x0);
                y0 = y0.min(leaf.y0);
                x1 = x1.max(leaf.x1);
                y1 = y1.max(leaf.y1);
                found = true;
            }
        }
        if found {
            Some((x0, y0, x1, y1))
        } else {
            None
        }
    }
}

/// Exact WinDirStat `Controls/TreeMap.cpp::DrawCushion` parallel software rasterizer.
fn rasterize_cushions(
    leaves: &[TreemapLeaf],
    w: usize,
    h: usize,
    opts: &TreemapOptions,
) -> Vec<u8> {
    let mut pixels = vec![0u8; w * h * 4];
    for px in pixels.as_chunks_mut::<4>().0 {
        px[0] = 22;
        px[1] = 25;
        px[2] = 31;
        px[3] = 255;
    }

    // Exact upstream WinDirStat light vector normalization (`lz = 10.0`)
    let lx_raw = opts.light_x as f64;
    let ly_raw = opts.light_y as f64;
    let lz_raw = 10.0_f64;
    let norm = (lx_raw * lx_raw + ly_raw * ly_raw + lz_raw * lz_raw).sqrt();
    let lx = (lx_raw / norm) as f32;
    let ly = (ly_raw / norm) as f32;
    let lz = (lz_raw / norm) as f32;

    let ia = opts.ambient_light.clamp(0.0, 1.0);
    let is = (1.0 - ia).max(0.0);
    let brightness_factor = opts.brightness.clamp(0.05, 1.5) / (GRAPH_PALETTE_BRIGHTNESS as f32);
    let saturation = opts.saturation.clamp(0.0, 1.0);
    let show_grid = opts.show_grid;
    let grid_color = opts.grid_color;

    let num_bands = 32usize.min(h.max(1));
    let band_h = h.div_ceil(num_bands);

    let mut band_leaves: Vec<Vec<usize>> = vec![Vec::new(); num_bands];
    for (idx, leaf) in leaves.iter().enumerate() {
        let y_start = (leaf.y0.floor() as isize).clamp(0, h as isize - 1) as usize;
        let y_end = (leaf.y1.ceil() as isize).clamp(0, h as isize) as usize;
        if y_end <= y_start {
            continue;
        }
        let b0 = y_start / band_h;
        let b1 = (y_end - 1) / band_h;
        let b_end = b1.min(num_bands - 1);
        for bucket in &mut band_leaves[b0..=b_end] {
            bucket.push(idx);
        }
    }

    pixels
        .par_chunks_mut(band_h * w * 4)
        .enumerate()
        .for_each(|(band_idx, band_buf)| {
            let band_y0 = band_idx * band_h;
            let band_rows = band_buf.len() / (w * 4);
            let band_y1 = band_y0 + band_rows;

            for &leaf_idx in &band_leaves[band_idx] {
                let leaf = &leaves[leaf_idx];
                let ix0 = (leaf.x0.floor() as isize).clamp(0, w as isize) as usize;
                let ix1 = (leaf.x1.ceil() as isize).clamp(0, w as isize) as usize;
                let iy0 =
                    (leaf.y0.floor() as isize).clamp(band_y0 as isize, band_y1 as isize) as usize;
                let iy1 =
                    (leaf.y1.ceil() as isize).clamp(band_y0 as isize, band_y1 as isize) as usize;

                if ix1 <= ix0 || iy1 <= iy0 {
                    continue;
                }

                let sat_rgb = apply_saturation(leaf.rgb, saturation);
                let col_r = sat_rgb[0] as f32 * brightness_factor;
                let col_g = sat_rgb[1] as f32 * brightness_factor;
                let col_b = sat_rgb[2] as f32 * brightness_factor;
                let surf = leaf.surface;
                let flat_tile = surf.x2.abs() < 1e-6 && surf.y2.abs() < 1e-6;
                let top_border_y = leaf.y0.floor() as usize;

                for y in iy0..iy1 {
                    let fy = y as f32 + 0.5;
                    let ny = -(2.0 * surf.y2 * fy + surf.y1);
                    let row_off = (y - band_y0) * w * 4;
                    let is_border_y = show_grid && y == top_border_y;

                    for x in ix0..ix1 {
                        let p = row_off + x * 4;
                        let is_grid_px = is_border_y || (show_grid && x == ix0);

                        if is_grid_px {
                            if let Some([gr, gg, gb]) = grid_color {
                                band_buf[p] = gr;
                                band_buf[p + 1] = gg;
                                band_buf[p + 2] = gb;
                                band_buf[p + 3] = 255;
                                continue;
                            }
                        }

                        let mut pixel_factor = if flat_tile {
                            ia + is * lz
                        } else {
                            let fx = x as f32 + 0.5;
                            let nx = -(2.0 * surf.x2 * fx + surf.x1);
                            let inv_len = 1.0 / (nx * nx + ny * ny + 1.0).sqrt();
                            let cosa = ((nx * lx + ny * ly + lz) * inv_len).max(0.0);
                            ia + is * cosa
                        };

                        if is_grid_px {
                            // Auto grid (`CLR_DEFAULT`): subtle darkened border
                            pixel_factor *= 0.62;
                        }

                        let [r, g, b] = normalize_color(
                            (col_r * pixel_factor) as i32,
                            (col_g * pixel_factor) as i32,
                            (col_b * pixel_factor) as i32,
                        );
                        band_buf[p] = r;
                        band_buf[p + 1] = g;
                        band_buf[p + 2] = b;
                        band_buf[p + 3] = 255;
                    }
                }
            }
        });

    pixels
}

/// Draw a miniature 3D WinDirStat Cushion swatch (for Extension List & legends).
pub fn draw_mini_cushion_rect(
    painter: &egui::Painter,
    rect: Rect,
    rgb: [u8; 3],
    opts: &TreemapOptions,
) {
    let sat = apply_saturation(rgb, opts.saturation);
    let bf = (opts.brightness.clamp(0.2, 1.4) / (GRAPH_PALETTE_BRIGHTNESS as f32)).clamp(0.5, 1.8);
    let ia = opts.ambient_light.clamp(0.0, 1.0);
    let is = (1.0 - ia).max(0.0);

    // Outer shadow bevel
    let dark_f = (ia + is * 0.35) * bf;
    let dark = normalize_color(
        (sat[0] as f32 * dark_f) as i32,
        (sat[1] as f32 * dark_f) as i32,
        (sat[2] as f32 * dark_f) as i32,
    );
    painter.rect_filled(rect, 2.0, Color32::from_rgb(dark[0], dark[1], dark[2]));

    // Mid body
    let mid_f = (ia + is * 0.80) * bf;
    let mid = normalize_color(
        (sat[0] as f32 * mid_f) as i32,
        (sat[1] as f32 * mid_f) as i32,
        (sat[2] as f32 * mid_f) as i32,
    );
    let r1 = rect.shrink(1.0);
    if r1.width() > 2.0 && r1.height() > 2.0 {
        painter.rect_filled(r1, 1.5, Color32::from_rgb(mid[0], mid[1], mid[2]));
    }

    // Specular Cushion peak offset towards light direction
    if opts.cushion_height > 0.04 && r1.width() > 4.0 && r1.height() > 4.0 {
        let hi_f = (ia + is * 1.08) * bf;
        let hi = normalize_color(
            (sat[0] as f32 * hi_f) as i32,
            (sat[1] as f32 * hi_f) as i32,
            (sat[2] as f32 * hi_f) as i32,
        );
        let dx = (opts.light_x.clamp(-2.0, 2.0) / 2.0) * (r1.width() * 0.18);
        let dy = (opts.light_y.clamp(-2.0, 2.0) / 2.0) * (r1.height() * 0.18);
        let spec_center = r1.center() + egui::vec2(dx, dy);
        let spec_rect = Rect::from_center_size(spec_center, r1.size() * 0.48).intersect(r1);
        painter.rect_filled(spec_rect, 2.0, Color32::from_rgb(hi[0], hi[1], hi[2]));
    }
}

/// Build and render a live WinDirStat-style `BuildDemoTree` Cushion Treemap preview for Options Dialog.
pub fn render_demo_treemap_preview(
    ctx: &Context,
    opts: &TreemapOptions,
    width: usize,
    height: usize,
) -> TextureHandle {
    let w = width.clamp(80, 420);
    let h = height.clamp(60, 280);

    // Deterministic hierarchical demo tree matching WinDirStat `CTreeMap::BuildDemoTree`
    let mut tree = ScanTree::new();
    let root = tree.add_root_target(std::path::Path::new("Demo"));
    let folders: [(&str, &[(&str, u64)]); 5] = [
        (
            "Video",
            &[
                ("a.mp4", 420),
                ("b.mp4", 310),
                ("c.mkv", 260),
                ("d.mov", 190),
                ("e.avi", 110),
            ],
        ),
        (
            "RawPhotos",
            &[
                ("p1.cr2", 290),
                ("p2.cr2", 240),
                ("p3.bmp", 195),
                ("p4.tif", 150),
                ("p5.png", 120),
                ("p6.jpg", 95),
            ],
        ),
        (
            "CAD_BIM",
            &[
                ("plan.dwg", 240),
                ("model.rvt", 200),
                ("mesh.ifc", 140),
                ("tex.psd", 110),
                ("doc.pdf", 85),
            ],
        ),
        (
            "Backups",
            &[
                ("db.bak", 210),
                ("mail.pst", 170),
                ("arc.zip", 130),
                ("img.iso", 105),
                ("log.txt", 65),
            ],
        ),
        (
            "System",
            &[
                ("app.exe", 120),
                ("core.dll", 100),
                ("snd.wav", 85),
                ("mus.mp3", 70),
                ("cfg.json", 45),
                ("note.docx", 40),
            ],
        ),
    ];

    for (dir_name, files) in folders {
        let d = tree.add_child(root, dir_name, NodeFlags::IS_DIR, 0, 0, 1_700_000_000);
        for &(fname, kb) in files {
            let bytes = kb * 1024;
            tree.add_child(d, fname, NodeFlags::empty(), bytes, bytes, 1_700_000_000);
        }
    }
    tree.recompute_aggregates();

    // Render using a ByExtension copy so the Cushion geometry & palette preview is always vivid
    let mut demo_opts = opts.clone();
    demo_opts.color_mode = VizColorMode::ByExtension;

    let mut leaves = Vec::with_capacity(64);
    let mut frames = Vec::new();
    layout_recursive(
        &tree,
        root,
        0.0,
        0.0,
        w as f32,
        h as f32,
        CushionSurface::default(),
        demo_opts.cushion_height,
        0,
        &demo_opts,
        SizeMetric::Logical,
        false,
        false,
        1_700_000_000,
        &mut leaves,
        &mut frames,
    );

    let pixels = rasterize_cushions(&leaves, w, h, &demo_opts);
    let color_img = ColorImage::from_rgba_unmultiplied([w, h], &pixels);
    ctx.load_texture(
        "mediastat_cushion_demo_preview",
        color_img,
        TextureOptions::NEAREST,
    )
}

fn is_descendant_of(tree: &ScanTree, mut node: NodeId, ancestor: NodeId) -> bool {
    while node.is_some() {
        if node == ancestor {
            return true;
        }
        node = tree.parents[node.index()];
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn layout_recursive(
    tree: &ScanTree,
    node: NodeId,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    mut surface: CushionSurface,
    ridge_h: f32,
    depth: u32,
    opts: &TreemapOptions,
    metric: SizeMetric,
    show_free: bool,
    show_unknown: bool,
    now_unix: i64,
    out: &mut Vec<TreemapLeaf>,
    frames: &mut Vec<(f32, f32, f32, f32, NodeId)>,
) {
    let w = x1 - x0;
    let h = y1 - y0;
    if w < 1.5 || h < 1.5 {
        return;
    }

    if depth > 0 {
        surface.add_ridge(x0, x1, y0, y1, ridge_h);
    }

    let is_dir = tree.flags[node.index()].contains(NodeFlags::IS_DIR);
    if !is_dir || depth >= 14 || (w < 4.0 && h < 4.0) {
        let rgb = pick_node_color(tree, node, opts, now_unix);
        let parent_dir = if is_dir {
            node
        } else {
            tree.parents[node.index()]
        };
        out.push(TreemapLeaf {
            x0,
            y0,
            x1,
            y1,
            item: SpecialTile::Normal(node),
            parent_dir,
            ext_id: tree.ext_ids[node.index()],
            rgb,
            surface,
        });
        return;
    }

    let frame_thresh = (opts.folder_frames_draw_threshold as f32).max(2.0) * 3.0;
    if depth > 0 && depth <= 4 && w >= frame_thresh && h >= frame_thresh {
        frames.push((x0, y0, x1, y1, node));
    }

    // Collect non-empty children (+ optional WinDirStat FreeSpace / UnknownSpace for volume roots)
    let mut children: Vec<(SpecialTile, u64)> = tree
        .children(node)
        .filter_map(|c| {
            if tree.is_deleted(c) {
                return None;
            }
            // Avoid double-drawing secondary NTFS hardlinks when visualizing physical disk usage
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
            children.push((SpecialTile::FreeSpace(node, vs.free_bytes), vs.free_bytes));
        }
        if show_unknown {
            let unk = vs.unknown_bytes(tree.allocated_sizes[node.index()]);
            if unk > 0 {
                children.push((SpecialTile::UnknownSpace(node, unk), unk));
            }
        }
    }

    if children.is_empty() {
        return;
    }
    children.sort_unstable_by_key(|a| std::cmp::Reverse(a.1));

    let total_bytes: u64 = children.iter().map(|(_, s)| *s).sum();
    if total_bytes == 0 {
        return;
    }

    let next_ridge = ridge_h * opts.scale_factor.clamp(0.0, 1.0);

    match opts.style {
        TreemapLayoutStyle::KDirStat => {
            layout_kdirstat(
                tree,
                node,
                &children,
                total_bytes,
                x0,
                y0,
                x1,
                y1,
                surface,
                next_ridge,
                depth,
                opts,
                metric,
                show_free,
                show_unknown,
                now_unix,
                out,
                frames,
            );
        }
        TreemapLayoutStyle::Hilbert | TreemapLayoutStyle::Moore => {
            let is_moore = opts.style == TreemapLayoutStyle::Moore;
            layout_space_filling(
                tree,
                node,
                &children,
                total_bytes,
                x0,
                y0,
                x1,
                y1,
                surface,
                next_ridge,
                depth,
                0,
                is_moore,
                opts,
                metric,
                show_free,
                show_unknown,
                now_unix,
                out,
                frames,
            );
        }
        TreemapLayoutStyle::Squarified => {
            layout_squarified(
                tree,
                node,
                &children,
                total_bytes,
                x0,
                y0,
                x1,
                y1,
                surface,
                next_ridge,
                depth,
                opts,
                metric,
                show_free,
                show_unknown,
                now_unix,
                out,
                frames,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_child(
    tree: &ScanTree,
    parent_dir: NodeId,
    item: SpecialTile,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    mut surface: CushionSurface,
    next_ridge: f32,
    depth: u32,
    opts: &TreemapOptions,
    metric: SizeMetric,
    show_free: bool,
    show_unknown: bool,
    now_unix: i64,
    out: &mut Vec<TreemapLeaf>,
    frames: &mut Vec<(f32, f32, f32, f32, NodeId)>,
) {
    match item {
        SpecialTile::Normal(cid) => {
            layout_recursive(
                tree,
                cid,
                x0,
                y0,
                x1,
                y1,
                surface,
                next_ridge,
                depth + 1,
                opts,
                metric,
                show_free,
                show_unknown,
                now_unix,
                out,
                frames,
            );
        }
        SpecialTile::FreeSpace(_, _) => {
            surface.add_ridge(x0, x1, y0, y1, next_ridge);
            out.push(TreemapLeaf {
                x0,
                y0,
                x1,
                y1,
                item,
                parent_dir,
                ext_id: 0,
                rgb: free_space_color(opts),
                surface,
            });
        }
        SpecialTile::UnknownSpace(_, _) => {
            surface.add_ridge(x0, x1, y0, y1, next_ridge);
            out.push(TreemapLeaf {
                x0,
                y0,
                x1,
                y1,
                item,
                parent_dir,
                ext_id: 0,
                rgb: make_bright_color(unknown_space_color(opts), GRAPH_PALETTE_BRIGHTNESS),
                surface,
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_kdirstat(
    tree: &ScanTree,
    parent_dir: NodeId,
    children: &[(SpecialTile, u64)],
    total_bytes: u64,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    surface: CushionSurface,
    next_ridge: f32,
    depth: u32,
    opts: &TreemapOptions,
    metric: SizeMetric,
    show_free: bool,
    show_unknown: bool,
    now_unix: i64,
    out: &mut Vec<TreemapLeaf>,
    frames: &mut Vec<(f32, f32, f32, f32, NodeId)>,
) {
    let horizontal = (x1 - x0) >= (y1 - y0);
    let mut cursor = if horizontal { x0 } else { y0 };
    let span = if horizontal { x1 - x0 } else { y1 - y0 };

    for &(item, sz) in children {
        let frac = sz as f64 / total_bytes as f64;
        let len = (span as f64 * frac) as f32;
        if len < 1.2 {
            break;
        }
        let (cx0, cy0, cx1, cy1) = if horizontal {
            (cursor, y0, (cursor + len).min(x1), y1)
        } else {
            (x0, cursor, x1, (cursor + len).min(y1))
        };
        emit_child(
            tree,
            parent_dir,
            item,
            cx0,
            cy0,
            cx1,
            cy1,
            surface,
            next_ridge,
            depth,
            opts,
            metric,
            show_free,
            show_unknown,
            now_unix,
            out,
            frames,
        );
        cursor += len;
    }
}

/// Recursive balanced binary space-filling layout following Hilbert / Moore quadrant ordering.
#[allow(clippy::too_many_arguments)]
fn layout_space_filling(
    tree: &ScanTree,
    parent_dir: NodeId,
    slice: &[(SpecialTile, u64)],
    total: u64,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    surface: CushionSurface,
    next_ridge: f32,
    depth: u32,
    orient: u8,
    is_moore: bool,
    opts: &TreemapOptions,
    metric: SizeMetric,
    show_free: bool,
    show_unknown: bool,
    now_unix: i64,
    out: &mut Vec<TreemapLeaf>,
    frames: &mut Vec<(f32, f32, f32, f32, NodeId)>,
) {
    if slice.is_empty() || total == 0 || (x1 - x0) < 1.5 || (y1 - y0) < 1.5 {
        return;
    }
    if slice.len() == 1 {
        emit_child(
            tree,
            parent_dir,
            slice[0].0,
            x0,
            y0,
            x1,
            y1,
            surface,
            next_ridge,
            depth,
            opts,
            metric,
            show_free,
            show_unknown,
            now_unix,
            out,
            frames,
        );
        return;
    }

    let half_target = total / 2;
    let mut acc = 0u64;
    let mut split = 1usize;
    for (i, &(_, sz)) in slice.iter().enumerate() {
        acc += sz;
        if acc >= half_target && i + 1 < slice.len() {
            split = i + 1;
            break;
        }
    }
    let left_sum: u64 = slice[..split].iter().map(|(_, s)| *s).sum();
    let right_sum = total.saturating_sub(left_sum);
    let ratio = (left_sum as f64 / total as f64).clamp(0.02, 0.98) as f32;

    let split_vertical = (x1 - x0) >= (y1 - y0);
    let reverse_second = is_moore && orient.is_multiple_of(2);

    if split_vertical {
        let mid_x = x0 + (x1 - x0) * ratio;
        let (a_x0, a_x1, b_x0, b_x1) = if orient % 4 >= 2 {
            (mid_x, x1, x0, mid_x)
        } else {
            (x0, mid_x, mid_x, x1)
        };
        layout_space_filling(
            tree,
            parent_dir,
            &slice[..split],
            left_sum,
            a_x0,
            y0,
            a_x1,
            y1,
            surface,
            next_ridge,
            depth,
            orient.wrapping_add(1),
            is_moore,
            opts,
            metric,
            show_free,
            show_unknown,
            now_unix,
            out,
            frames,
        );
        layout_space_filling(
            tree,
            parent_dir,
            &slice[split..],
            right_sum,
            b_x0,
            y0,
            b_x1,
            y1,
            surface,
            next_ridge,
            depth,
            if reverse_second {
                orient.wrapping_add(3)
            } else {
                orient.wrapping_add(1)
            },
            is_moore,
            opts,
            metric,
            show_free,
            show_unknown,
            now_unix,
            out,
            frames,
        );
    } else {
        let mid_y = y0 + (y1 - y0) * ratio;
        let (a_y0, a_y1, b_y0, b_y1) = if orient % 4 >= 2 {
            (mid_y, y1, y0, mid_y)
        } else {
            (y0, mid_y, mid_y, y1)
        };
        layout_space_filling(
            tree,
            parent_dir,
            &slice[..split],
            left_sum,
            x0,
            a_y0,
            x1,
            a_y1,
            surface,
            next_ridge,
            depth,
            orient.wrapping_add(1),
            is_moore,
            opts,
            metric,
            show_free,
            show_unknown,
            now_unix,
            out,
            frames,
        );
        layout_space_filling(
            tree,
            parent_dir,
            &slice[split..],
            right_sum,
            x0,
            b_y0,
            x1,
            b_y1,
            surface,
            next_ridge,
            depth,
            if reverse_second {
                orient.wrapping_add(3)
            } else {
                orient.wrapping_add(1)
            },
            is_moore,
            opts,
            metric,
            show_free,
            show_unknown,
            now_unix,
            out,
            frames,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_squarified(
    tree: &ScanTree,
    parent_dir: NodeId,
    children: &[(SpecialTile, u64)],
    total_bytes: u64,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    surface: CushionSurface,
    next_ridge: f32,
    depth: u32,
    opts: &TreemapOptions,
    metric: SizeMetric,
    show_free: bool,
    show_unknown: bool,
    now_unix: i64,
    out: &mut Vec<TreemapLeaf>,
    frames: &mut Vec<(f32, f32, f32, f32, NodeId)>,
) {
    let mut rx0 = x0;
    let mut ry0 = y0;
    let rx1 = x1;
    let ry1 = y1;
    let mut rem_bytes = total_bytes as f64;
    let mut idx = 0usize;

    while idx < children.len() {
        let rw = (rx1 - rx0).max(0.0) as f64;
        let rh = (ry1 - ry0).max(0.0) as f64;
        if rw < 1.5 || rh < 1.5 || rem_bytes <= 0.0 {
            break;
        }

        let horizontal_strip = rw >= rh;
        let side = if horizontal_strip { rh } else { rw };
        let total_area = rw * rh;

        let next_area = (children[idx].1 as f64 / rem_bytes) * total_area;
        if next_area < 3.0 {
            let rgb = pick_node_color(tree, parent_dir, opts, now_unix);
            out.push(TreemapLeaf {
                x0: rx0,
                y0: ry0,
                x1: rx1,
                y1: ry1,
                item: SpecialTile::Normal(parent_dir),
                parent_dir,
                ext_id: 0,
                rgb,
                surface,
            });
            break;
        }

        let mut row_end = idx + 1;
        let mut row_bytes = children[idx].1 as f64;
        let mut best_worst = worst_aspect(
            row_bytes,
            children[idx].1 as f64,
            children[idx].1 as f64,
            rem_bytes,
            total_area,
            side,
        );

        while row_end < children.len() {
            let cand_b = children[row_end].1 as f64;
            let new_sum = row_bytes + cand_b;
            let cand_worst = worst_aspect(
                new_sum,
                children[idx].1 as f64,
                cand_b,
                rem_bytes,
                total_area,
                side,
            );
            if cand_worst <= best_worst {
                row_bytes = new_sum;
                best_worst = cand_worst;
                row_end += 1;
            } else {
                break;
            }
        }

        let strip_frac = (row_bytes / rem_bytes).clamp(0.0, 1.0);
        if horizontal_strip {
            let strip_w = (rw * strip_frac) as f32;
            let mut cy = ry0;
            for &(item, cbytes) in &children[idx..row_end] {
                let item_h = ((cbytes as f64 / row_bytes) * rh) as f32;
                let ny = (cy + item_h).min(ry1);
                emit_child(
                    tree,
                    parent_dir,
                    item,
                    rx0,
                    cy,
                    (rx0 + strip_w).min(rx1),
                    ny,
                    surface,
                    next_ridge,
                    depth,
                    opts,
                    metric,
                    show_free,
                    show_unknown,
                    now_unix,
                    out,
                    frames,
                );
                cy = ny;
            }
            rx0 = (rx0 + strip_w).min(rx1);
        } else {
            let strip_h = (rh * strip_frac) as f32;
            let mut cx = rx0;
            for &(item, cbytes) in &children[idx..row_end] {
                let item_w = ((cbytes as f64 / row_bytes) * rw) as f32;
                let nx = (cx + item_w).min(rx1);
                emit_child(
                    tree,
                    parent_dir,
                    item,
                    cx,
                    ry0,
                    nx,
                    (ry0 + strip_h).min(ry1),
                    surface,
                    next_ridge,
                    depth,
                    opts,
                    metric,
                    show_free,
                    show_unknown,
                    now_unix,
                    out,
                    frames,
                );
                cx = nx;
            }
            ry0 = (ry0 + strip_h).min(ry1);
        }

        rem_bytes -= row_bytes;
        idx = row_end;
    }
}

#[inline]
fn worst_aspect(
    row_bytes: f64,
    max_b: f64,
    min_b: f64,
    rem_bytes: f64,
    total_area: f64,
    side: f64,
) -> f64 {
    if min_b <= 0.0 || side <= 0.0 || rem_bytes <= 0.0 {
        return f64::MAX;
    }
    let s_area = (row_bytes / rem_bytes) * total_area;
    let max_a = (max_b / rem_bytes) * total_area;
    let min_a = (min_b / rem_bytes) * total_area;
    let s2 = side * side;
    let r1 = (s2 * max_a) / (s_area * s_area);
    let r2 = (s_area * s_area) / (s2 * min_a);
    r1.max(r2)
}

/// Resolve the display RGB for an extension ID honoring WinDirStat's exact 18-color rank palette.
#[inline]
pub fn extension_display_color(tree: &ScanTree, ext_id: u16, opts: &TreemapOptions) -> [u8; 3] {
    if opts.use_rank_extension_colors && opts.color_palette == VizColorPalette::WinDirStatClassic {
        if let Some(info) = tree.extensions.get(ext_id) {
            return info.color;
        }
        return windirstat_rank_color(17);
    }
    let ext = tree.extensions.ext_str(ext_id);
    extension_color_for_palette(ext, opts.color_palette)
}

pub fn pick_node_color(
    tree: &ScanTree,
    node: NodeId,
    opts: &TreemapOptions,
    now_unix: i64,
) -> [u8; 3] {
    let idx = node.index();
    let is_dir = tree.flags[idx].contains(NodeFlags::IS_DIR);
    let palette = opts.color_palette;

    match opts.color_mode {
        VizColorMode::ByExtension => {
            if is_dir {
                [88, 98, 114]
            } else {
                extension_display_color(tree, tree.ext_ids[idx], opts)
            }
        }
        VizColorMode::ByCategory => {
            if is_dir {
                [84, 94, 110]
            } else {
                let cat = FileCategory::from_extension(tree.ext(node));
                let raw = category_color_for_palette(cat, palette);
                if cat == FileCategory::Other && palette == VizColorPalette::SysAdminThermal {
                    raw
                } else {
                    make_bright_color(raw, GRAPH_PALETTE_BRIGHTNESS)
                }
            }
        }
        VizColorMode::FileAge => {
            let mtime = if is_dir {
                tree.dir_stats
                    .get(&node)
                    .map(|d| d.max_mtime)
                    .unwrap_or(tree.mtimes[idx])
            } else {
                tree.mtimes[idx]
            };
            if mtime <= 0 {
                return [78, 84, 94];
            }
            let age_days = ((now_unix - mtime).max(0) as f32) / 86400.0;
            make_bright_color(
                age_color_for_palette(age_days, palette),
                GRAPH_PALETTE_BRIGHTNESS,
            )
        }
    }
}
