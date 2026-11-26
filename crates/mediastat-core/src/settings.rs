use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::filtering::FilterConfig;
use crate::format::WINDIRSTAT_TREE_DEPTH_8;
use crate::i18n::{active_language, set_active_language, t, t_for, Language};

/// Global metric used for sizing bars, Treemap, Sunburst, and Flame Graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SizeMetric {
    #[default]
    Logical,
    AllocatedOnDisk,
}

impl SizeMetric {
    pub fn label(self) -> &'static str {
        self.label_for(active_language())
    }

    pub fn label_for(self, lang: Language) -> &'static str {
        match self {
            Self::Logical => t_for(lang, "Tamaño Lógico", "Logical Size"),
            Self::AllocatedOnDisk => t_for(lang, "Tamaño en Disco (Físico)", "Size on Disk (Allocated)"),
        }
    }
}

/// Process priority during scanning (exact WinDirStat `PROCESSPRIORITY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProcessPriority {
    Low,
    #[default]
    Normal,
    High,
}

impl ProcessPriority {
    pub const ALL: [Self; 3] = [Self::Low, Self::Normal, Self::High];

    pub fn label(self) -> &'static str {
        self.label_for(active_language())
    }

    pub fn label_for(self, lang: Language) -> &'static str {
        match self {
            Self::Low => t_for(lang, "Baja (fondo)", "Low (Background)"),
            Self::Normal => "Normal",
            Self::High => t_for(lang, "Alta", "High"),
        }
    }
}

/// Treemap layout algorithms (full WinDirStat 2.10 parity).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TreemapLayoutStyle {
    #[default]
    Squarified,
    KDirStat,
    Hilbert,
    Moore,
}

impl TreemapLayoutStyle {
    pub fn label(self) -> &'static str {
        self.label_for(active_language())
    }

    pub fn label_for(self, lang: Language) -> &'static str {
        match self {
            Self::Squarified => "Squarified (SequoiaView)",
            Self::KDirStat => t_for(lang, "KDirStat (Filas)", "KDirStat (Rows)"),
            Self::Hilbert => t_for(lang, "Curva de Hilbert", "Hilbert Curve"),
            Self::Moore => t_for(lang, "Curva de Moore", "Moore Curve"),
        }
    }
}

/// Visual shading presets for the Cushion Treemap (exact WinDirStat 2.10 `CTreeMap::Preset`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TreemapPreset {
    #[default]
    Classic,
    Calm,
    Flat,
    Pastel,
    HighContrast,
    Custom,
}

impl TreemapPreset {
    pub fn label(self) -> &'static str {
        self.label_for(active_language())
    }

    pub fn label_for(self, lang: Language) -> &'static str {
        match self {
            Self::Classic => t_for(lang, "Clásico (WinDirStat)", "Classic (WinDirStat)"),
            Self::Calm => t_for(lang, "Suave (Calm)", "Calm"),
            Self::Flat => t_for(lang, "Plano (Flat)", "Flat"),
            Self::Pastel => "Pastel",
            Self::HighContrast => t_for(lang, "Alto Contraste", "High Contrast"),
            Self::Custom => t_for(lang, "Personalizado", "Custom"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VisualizationKind {
    #[default]
    Treemap,
    Sunburst,
    FlameGraph,
}

impl VisualizationKind {
    pub fn label(self) -> &'static str {
        self.label_for(active_language())
    }

    pub fn label_for(self, lang: Language) -> &'static str {
        match self {
            Self::Treemap => t_for(lang, "Treemap Cushion", "Cushion Treemap"),
            Self::Sunburst => t_for(lang, "Sunburst Radial", "Radial Sunburst"),
            Self::FlameGraph => "Flame Graph",
        }
    }
}

/// Color mode shared & persisted across Treemap, Sunburst, and Flame Graph (zero I/O).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VizColorMode {
    #[default]
    #[serde(alias = "CompressionHeatmap", alias = "EstimatedSavings")]
    ByExtension,
    ByCategory,
    FileAge,
}

impl VizColorMode {
    pub const ALL: [Self; 3] = [Self::ByExtension, Self::ByCategory, Self::FileAge];

    pub fn label(self) -> &'static str {
        self.label_for(active_language())
    }

    pub fn label_for(self, lang: Language) -> &'static str {
        match self {
            Self::ByExtension => t_for(lang, "Por Extensión (WinDirStat)", "By Extension (WinDirStat)"),
            Self::ByCategory => t_for(lang, "Por Categoría", "By Category"),
            Self::FileAge => t_for(lang, "Antigüedad (Datos Fríos)", "File Age (Cold Data)"),
        }
    }
}

/// Selectable visual color palettes applied consistently across Treemap, Sunburst, Flame Graph & badges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VizColorPalette {
    #[default]
    WinDirStatClassic,
    MediaStatModern,
    ColorblindOkabeIto,
    SysAdminThermal,
}

impl VizColorPalette {
    pub const ALL: [Self; 4] = [
        Self::WinDirStatClassic,
        Self::MediaStatModern,
        Self::ColorblindOkabeIto,
        Self::SysAdminThermal,
    ];

    pub fn label(self) -> &'static str {
        self.label_for(active_language())
    }

    pub fn label_for(self, lang: Language) -> &'static str {
        match self {
            Self::WinDirStatClassic => t_for(lang, "WinDirStat Oficial (18 Colores)", "Official WinDirStat (18 Colors)"),
            Self::MediaStatModern => t_for(lang, "MediaStat Semántica (CAD/Media)", "MediaStat Semantic (CAD/Media)"),
            Self::ColorblindOkabeIto => t_for(lang, "Daltonismo Seguro (Okabe-Ito)", "Colorblind Safe (Okabe-Ito)"),
            Self::SysAdminThermal => t_for(lang, "Térmico SysAdmin (Alto Contraste)", "SysAdmin Thermal (High Contrast)"),
        }
    }

    pub fn short_label(self) -> &'static str {
        self.short_label_for(active_language())
    }

    pub fn short_label_for(self, lang: Language) -> &'static str {
        match self {
            Self::WinDirStatClassic => "WinDirStat",
            Self::MediaStatModern => t_for(lang, "Semántica", "Semantic"),
            Self::ColorblindOkabeIto => "Okabe-Ito",
            Self::SysAdminThermal => t_for(lang, "Térmico", "Thermal"),
        }
    }

    pub fn description(self) -> &'static str {
        self.description_for(active_language())
    }

    pub fn description_for(self, lang: Language) -> &'static str {
        match self {
            Self::WinDirStatClassic => t_for(
                lang,
                "Imitación exacta de WinDirStat: 18 colores oficiales asignados por ranking de bytes con brillo normalizado 0.6 y Cushion 3D.",
                "Exact WinDirStat imitation: 18 official colors assigned by descending byte rank with 0.6 normalized brightness and 3D Cushions.",
            ),
            Self::MediaStatModern => t_for(
                lang,
                "Colores semánticos por tipo de archivo (vídeo, RAW, imágenes, CAD/BIM .dwg/.rvt/.ifc, Office, backups).",
                "Semantic colors by file type (video, RAW, images, CAD/BIM .dwg/.rvt/.ifc, Office, backups).",
            ),
            Self::ColorblindOkabeIto => t_for(
                lang,
                "Paleta científicamente accesible (Okabe-Ito) sin ambigüedad entre rojo y verde.",
                "Scientifically colorblind-safe palette (Okabe-Ito) with zero red/green ambiguity.",
            ),
            Self::SysAdminThermal => t_for(
                lang,
                "Atenúa archivos normales en gris pizarra para que RAW, multimedia sin comprimir y datos fríos salten a la vista.",
                "Mutes ordinary files in slate gray so RAW, uncompressed media, and cold data stand out immediately.",
            ),
        }
    }
}

fn default_free_space_rgb() -> [u8; 3] {
    [100, 100, 100]
}
fn default_unknown_space_rgb() -> [u8; 3] {
    [255, 255, 0]
}

/// Customizable RGB swatches for special WinDirStat volume tiles (`<Free Space>` and `<Unknown Space>`).
/// Extra legacy fields from older `StatusColorOverrides` JSON are safely ignored by serde.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeColorOverrides {
    #[serde(default = "default_free_space_rgb")]
    pub free_space: [u8; 3],
    #[serde(default = "default_unknown_space_rgb")]
    pub unknown_space: [u8; 3],
}

impl Default for VolumeColorOverrides {
    fn default() -> Self {
        Self::from_palette(VizColorPalette::WinDirStatClassic)
    }
}

impl VolumeColorOverrides {
    pub fn from_palette(palette: VizColorPalette) -> Self {
        match palette {
            VizColorPalette::MediaStatModern => Self {
                free_space: [52, 146, 98],
                unknown_space: [192, 166, 58],
            },
            VizColorPalette::WinDirStatClassic => Self {
                free_space: [100, 100, 100],  // Classic WinDirStat dark gray <Free Space>
                unknown_space: [255, 255, 0], // Classic WinDirStat yellow <Unknown>
            },
            VizColorPalette::ColorblindOkabeIto => Self {
                free_space: [0, 158, 115],
                unknown_space: [240, 228, 66],
            },
            VizColorPalette::SysAdminThermal => Self {
                free_space: [40, 120, 96],
                unknown_space: [170, 145, 52],
            },
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_saturation() -> f32 {
    1.0
}
fn default_highlight_color() -> [u8; 3] {
    [255, 255, 255]
}
fn default_folder_threshold() -> u16 {
    5
}
fn default_tree_depth_colors() -> [[u8; 3]; 8] {
    WINDIRSTAT_TREE_DEPTH_8
}
fn default_tree_cols() -> [bool; 12] {
    [true, true, true, true, true, true, true, true, true, true, true, true]
}
fn default_search_max() -> usize {
    25_000
}

/// Configurable Cushion Treemap & Visualization options (exact parity with `CTreeMap::Options`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreemapOptions {
    pub style: TreemapLayoutStyle,
    pub preset: TreemapPreset,
    pub brightness: f32,     // 0.0 ..= 1.0 (Classic = 0.88)
    #[serde(default = "default_saturation")]
    pub saturation: f32,     // 0.0 ..= 1.0 (Classic = 1.0)
    pub cushion_height: f32, // 0.0 ..= 2.0 (Classic = 0.38)
    pub scale_factor: f32,   // 0.0 ..= 1.0 (Classic = 0.91)
    pub ambient_light: f32,  // 0.0 ..= 1.0 (Classic = 0.13)
    pub light_x: f32,        // -4.0 ..= 4.0 (Classic = -1.0)
    pub light_y: f32,        // -4.0 ..= 4.0 (Classic = -1.0)
    pub show_grid: bool,
    #[serde(default)]
    pub grid_color: Option<[u8; 3]>, // None = Auto (CLR_DEFAULT)
    #[serde(default = "default_highlight_color")]
    pub highlight_color: [u8; 3],
    pub show_extensions_overlay: bool,
    pub show_folder_frames: bool,
    #[serde(default = "default_folder_threshold")]
    pub folder_frames_draw_threshold: u16,
    #[serde(default = "default_true")]
    pub contrast_labels: bool,
    #[serde(default)]
    pub color_mode: VizColorMode,
    #[serde(default)]
    pub color_palette: VizColorPalette,
    #[serde(default = "default_true")]
    pub use_rank_extension_colors: bool,
    #[serde(default = "default_true")]
    pub show_color_legend: bool,
    #[serde(default, alias = "custom_status_colors")]
    pub custom_volume_colors: Option<VolumeColorOverrides>,
}

impl Default for TreemapOptions {
    fn default() -> Self {
        Self::from_preset(TreemapPreset::Classic)
    }
}

impl TreemapOptions {
    /// Exact presets from upstream `windirstat/windirstat` `Controls/TreeMap.cpp::GetPreset`.
    pub fn from_preset(preset: TreemapPreset) -> Self {
        match preset {
            TreemapPreset::Classic | TreemapPreset::Custom => Self {
                style: TreemapLayoutStyle::Squarified,
                preset,
                brightness: 0.88,
                saturation: 1.0,
                cushion_height: 0.38,
                scale_factor: 0.91,
                ambient_light: 0.13,
                light_x: -1.0,
                light_y: -1.0,
                show_grid: false,
                grid_color: Some([0, 0, 0]),
                highlight_color: [255, 255, 255],
                show_extensions_overlay: true,
                show_folder_frames: false,
                folder_frames_draw_threshold: 5,
                contrast_labels: true,
                color_mode: VizColorMode::ByExtension,
                color_palette: VizColorPalette::WinDirStatClassic,
                use_rank_extension_colors: true,
                show_color_legend: true,
                custom_volume_colors: None,
            },
            TreemapPreset::Calm => Self {
                preset,
                brightness: 0.72,
                saturation: 0.65,
                cushion_height: 0.15,
                scale_factor: 0.80,
                ambient_light: 0.70,
                show_grid: true,
                grid_color: None,
                contrast_labels: true,
                ..Self::from_preset(TreemapPreset::Classic)
            },
            TreemapPreset::Flat => Self {
                preset,
                brightness: 0.72,
                saturation: 0.85,
                cushion_height: 0.0,
                scale_factor: 0.80,
                ambient_light: 1.0,
                show_grid: true,
                grid_color: None,
                contrast_labels: true,
                ..Self::from_preset(TreemapPreset::Classic)
            },
            TreemapPreset::Pastel => Self {
                preset,
                brightness: 0.82,
                saturation: 0.40,
                cushion_height: 0.10,
                scale_factor: 0.80,
                ambient_light: 0.85,
                show_grid: true,
                grid_color: None,
                contrast_labels: true,
                ..Self::from_preset(TreemapPreset::Classic)
            },
            TreemapPreset::HighContrast => Self {
                preset,
                brightness: 0.62,
                saturation: 1.0,
                cushion_height: 0.0,
                scale_factor: 0.80,
                ambient_light: 1.0,
                show_grid: true,
                grid_color: Some([0, 0, 0]),
                contrast_labels: true,
                ..Self::from_preset(TreemapPreset::Classic)
            },
        }
    }

    /// Apply only the exact shading/appearance fields from `preset`, keeping style, color mode, palette & overrides intact.
    pub fn apply_shading_preset(&mut self, preset: TreemapPreset) {
        if preset == TreemapPreset::Custom {
            self.preset = TreemapPreset::Custom;
            return;
        }
        let p = Self::from_preset(preset);
        self.preset = preset;
        self.brightness = p.brightness;
        self.saturation = p.saturation;
        self.cushion_height = p.cushion_height;
        self.scale_factor = p.scale_factor;
        self.ambient_light = p.ambient_light;
        self.light_x = p.light_x;
        self.light_y = p.light_y;
        self.show_grid = p.show_grid;
        self.grid_color = p.grid_color;
        self.contrast_labels = p.contrast_labels;
    }
}

/// Pane type in WinDirStat's 12 official 3-pane layouts (`LAYOUT_VIEW_TYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutPaneKind {
    AllFiles,
    FileTypes,
    Visualization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutAxis {
    /// Split horizontally: top / bottom.
    Horizontal,
    /// Split vertically: left / right.
    Vertical,
}

/// Exact structural definition of one of WinDirStat 2.10's 12 layouts (`CLayoutPopup::LayoutDef`).
#[derive(Debug, Clone, Copy)]
pub struct LayoutDef {
    pub orientation: LayoutAxis,
    pub sub_orientation: LayoutAxis,
    /// `0` = first half contains the nested split, `1` = second half contains the nested split.
    pub sub_pane: usize,
    /// `[standalone_pane, nested_first, nested_second]`.
    pub views: [LayoutPaneKind; 3],
    pub ratio: f32,
    pub sub_ratio: f32,
    pub preview_ratio: f32,
    pub preview_sub_ratio: f32,
}

impl LayoutDef {
    /// Compute normalized `(pane_kind, x, y, w, h)` preview rectangles in `[0..1]²`,
    /// identical to `CLayoutPopup::LayoutDef::PreviewPanes()`.
    pub fn preview_panes(&self) -> [(LayoutPaneKind, f32, f32, f32, f32); 3] {
        let split = |b: (f32, f32, f32, f32), axis: LayoutAxis, r: f32| {
            let mut a = b;
            let mut c = b;
            match axis {
                LayoutAxis::Vertical => {
                    a.2 *= r;
                    c.0 += a.2;
                    c.2 -= a.2;
                }
                LayoutAxis::Horizontal => {
                    a.3 *= r;
                    c.1 += a.3;
                    c.3 -= a.3;
                }
            }
            [a, c]
        };
        let main = split((0.0, 0.0, 1.0, 1.0), self.orientation, self.preview_ratio);
        let sub = split(main[self.sub_pane], self.sub_orientation, self.preview_sub_ratio);
        let stand = main[1 - self.sub_pane];
        [
            (self.views[0], stand.0, stand.1, stand.2, stand.3),
            (self.views[1], sub[0].0, sub[0].1, sub[0].2, sub[0].3),
            (self.views[2], sub[1].0, sub[1].1, sub[1].2, sub[1].3),
        ]
    }
}

/// All 12 official window layouts from upstream WinDirStat 2.10 `LayoutPopup.cpp` (3×4 grid).
/// Legacy v0.2 aliases deserialize seamlessly into their corresponding official layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WindowLayoutPreset {
    /// 0: `[AF | FT]` above `Viz` (Classic WinDirStat)
    #[default]
    #[serde(alias = "AuditFocus")]
    ClassicWinDirStat,
    /// 1: `AF` | `[FT | Viz]`
    AllFilesLeftSplitRight,
    /// 2: `[Viz | AF]` | `FT`
    VizAllFilesLeftExtRight,
    /// 3: `Viz` above `[AF | FT]`
    #[serde(alias = "ExtensionsLeft")]
    VisualizationTop,
    /// 4: `[AF | Viz]` | `FT` (Ultrawide 3-Col)
    #[serde(alias = "Ultrawide3Col")]
    AllFilesVizExt3Col,
    /// 5: `Viz` | `[FT | AF]`
    VizLeftExtAllFilesRight,
    /// 6: `[AF / Viz]` | `FT`
    AllFilesOverVizExtRight,
    /// 7: `[AF / FT]` | `Viz`
    #[serde(alias = "SideBySide")]
    AllFilesOverExtVizRight,
    /// 8: `Viz` | `[AF / FT]`
    VizLeftAllFilesOverExt,
    /// 9: `[Viz / AF]` | `FT`
    VizOverAllFilesExtRight,
    /// 10: `[FT / AF]` | `Viz`
    ExtOverAllFilesVizRight,
    /// 11: `Viz` | `[FT / AF]`
    VizLeftExtOverAllFiles,
}

impl WindowLayoutPreset {
    pub const ALL: [Self; 12] = [
        Self::ClassicWinDirStat,
        Self::AllFilesLeftSplitRight,
        Self::VizAllFilesLeftExtRight,
        Self::VisualizationTop,
        Self::AllFilesVizExt3Col,
        Self::VizLeftExtAllFilesRight,
        Self::AllFilesOverVizExtRight,
        Self::AllFilesOverExtVizRight,
        Self::VizLeftAllFilesOverExt,
        Self::VizOverAllFilesExtRight,
        Self::ExtOverAllFilesVizRight,
        Self::VizLeftExtOverAllFiles,
    ];

    pub fn label(self) -> &'static str {
        self.label_for(active_language())
    }

    pub fn label_for(self, lang: Language) -> &'static str {
        match self {
            Self::ClassicWinDirStat => t_for(lang, "Clásico [Árbol|Ext] / Mapa", "Classic [Tree|Ext] / Map"),
            Self::AllFilesLeftSplitRight => t_for(lang, "Árbol | [Ext|Mapa]", "Tree | [Ext|Map]"),
            Self::VizAllFilesLeftExtRight => t_for(lang, "[Mapa|Árbol] | Ext", "[Map|Tree] | Ext"),
            Self::VisualizationTop => t_for(lang, "Mapa / [Árbol|Ext]", "Map / [Tree|Ext]"),
            Self::AllFilesVizExt3Col => t_for(lang, "3 Cols [Árbol|Mapa|Ext]", "3-Col [Tree|Map|Ext]"),
            Self::VizLeftExtAllFilesRight => t_for(lang, "Mapa | [Ext|Árbol]", "Map | [Ext|Tree]"),
            Self::AllFilesOverVizExtRight => t_for(lang, "[Árbol/Mapa] | Ext", "[Tree/Map] | Ext"),
            Self::AllFilesOverExtVizRight => t_for(lang, "[Árbol/Ext] | Mapa", "[Tree/Ext] | Map"),
            Self::VizLeftAllFilesOverExt => t_for(lang, "Mapa | [Árbol/Ext]", "Map | [Tree/Ext]"),
            Self::VizOverAllFilesExtRight => t_for(lang, "[Mapa/Árbol] | Ext", "[Map/Tree] | Ext"),
            Self::ExtOverAllFilesVizRight => t_for(lang, "[Ext/Árbol] | Mapa", "[Ext/Tree] | Map"),
            Self::VizLeftExtOverAllFiles => t_for(lang, "Mapa | [Ext/Árbol]", "Map | [Ext/Tree]"),
        }
    }

    /// Exact structural definition matching `CLayoutPopup::LAYOUTS` in upstream `LayoutPopup.cpp`.
    pub fn def(self) -> LayoutDef {
        use LayoutAxis::{Horizontal as H, Vertical as V};
        use LayoutPaneKind::{AllFiles as AF, FileTypes as FT, Visualization as VZ};
        match self {
            Self::ClassicWinDirStat => LayoutDef {
                orientation: H,
                sub_orientation: V,
                sub_pane: 0,
                views: [VZ, AF, FT],
                ratio: 0.56,
                sub_ratio: 0.74,
                preview_ratio: 0.50,
                preview_sub_ratio: 0.58,
            },
            Self::AllFilesLeftSplitRight => LayoutDef {
                orientation: V,
                sub_orientation: V,
                sub_pane: 1,
                views: [AF, FT, VZ],
                ratio: 0.40,
                sub_ratio: 0.34,
                preview_ratio: 0.40,
                preview_sub_ratio: 0.34,
            },
            Self::VizAllFilesLeftExtRight => LayoutDef {
                orientation: V,
                sub_orientation: V,
                sub_pane: 0,
                views: [FT, VZ, AF],
                ratio: 0.78,
                sub_ratio: 0.50,
                preview_ratio: 0.80,
                preview_sub_ratio: 0.50,
            },
            Self::VisualizationTop => LayoutDef {
                orientation: H,
                sub_orientation: V,
                sub_pane: 1,
                views: [VZ, AF, FT],
                ratio: 0.45,
                sub_ratio: 0.74,
                preview_ratio: 0.50,
                preview_sub_ratio: 0.58,
            },
            Self::AllFilesVizExt3Col => LayoutDef {
                orientation: V,
                sub_orientation: V,
                sub_pane: 0,
                views: [FT, AF, VZ],
                ratio: 0.78,
                sub_ratio: 0.48,
                preview_ratio: 0.80,
                preview_sub_ratio: 0.50,
            },
            Self::VizLeftExtAllFilesRight => LayoutDef {
                orientation: V,
                sub_orientation: V,
                sub_pane: 1,
                views: [VZ, FT, AF],
                ratio: 0.42,
                sub_ratio: 0.36,
                preview_ratio: 0.40,
                preview_sub_ratio: 0.34,
            },
            Self::AllFilesOverVizExtRight => LayoutDef {
                orientation: V,
                sub_orientation: H,
                sub_pane: 0,
                views: [FT, AF, VZ],
                ratio: 0.76,
                sub_ratio: 0.52,
                preview_ratio: 0.60,
                preview_sub_ratio: 0.50,
            },
            Self::AllFilesOverExtVizRight => LayoutDef {
                orientation: V,
                sub_orientation: H,
                sub_pane: 0,
                views: [VZ, AF, FT],
                ratio: 0.50,
                sub_ratio: 0.62,
                preview_ratio: 0.50,
                preview_sub_ratio: 0.50,
            },
            Self::VizLeftAllFilesOverExt => LayoutDef {
                orientation: V,
                sub_orientation: H,
                sub_pane: 1,
                views: [VZ, AF, FT],
                ratio: 0.50,
                sub_ratio: 0.62,
                preview_ratio: 0.50,
                preview_sub_ratio: 0.50,
            },
            Self::VizOverAllFilesExtRight => LayoutDef {
                orientation: V,
                sub_orientation: H,
                sub_pane: 0,
                views: [FT, VZ, AF],
                ratio: 0.76,
                sub_ratio: 0.48,
                preview_ratio: 0.60,
                preview_sub_ratio: 0.50,
            },
            Self::ExtOverAllFilesVizRight => LayoutDef {
                orientation: V,
                sub_orientation: H,
                sub_pane: 0,
                views: [VZ, FT, AF],
                ratio: 0.50,
                sub_ratio: 0.38,
                preview_ratio: 0.50,
                preview_sub_ratio: 0.50,
            },
            Self::VizLeftExtOverAllFiles => LayoutDef {
                orientation: V,
                sub_orientation: H,
                sub_pane: 1,
                views: [VZ, FT, AF],
                ratio: 0.50,
                sub_ratio: 0.38,
                preview_ratio: 0.50,
                preview_sub_ratio: 0.50,
            },
        }
    }
}

/// Refresh behavior after executing a User-Defined Cleanup action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CleanupRefreshPolicy {
    None,
    #[default]
    RefreshSelected,
    RefreshParent,
}

impl CleanupRefreshPolicy {
    pub fn label(self) -> &'static str {
        let lang = active_language();
        match self {
            Self::None => t_for(lang, "No refrescar", "Do not refresh"),
            Self::RefreshSelected => t_for(lang, "Refrescar selección", "Refresh selection"),
            Self::RefreshParent => t_for(lang, "Refrescar carpeta padre", "Refresh parent folder"),
        }
    }
}

/// WinDirStat-compatible User-Defined Cleanup / Quick Action slot (`Ctrl+0..9`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserCleanupAction {
    pub enabled: bool,
    pub title: String,
    pub command_line: String,
    pub works_for_files: bool,
    pub works_for_dirs: bool,
    pub ask_confirmation: bool,
    pub run_minimized: bool,
    #[serde(default)]
    pub read_only_safe: bool,
    pub refresh_policy: CleanupRefreshPolicy,
}

impl UserCleanupAction {
    pub fn empty_slot(index: usize) -> Self {
        Self {
            enabled: false,
            title: format!("{} #{index}", t("Acción personalizada", "Custom Action")),
            command_line: String::new(),
            works_for_files: true,
            works_for_dirs: true,
            ask_confirmation: true,
            run_minimized: false,
            read_only_safe: false,
            refresh_policy: CleanupRefreshPolicy::RefreshSelected,
        }
    }

    pub fn default_presets() -> Vec<Self> {
        Self::default_presets_for(active_language())
    }

    pub fn default_presets_for(lang: Language) -> Vec<Self> {
        vec![
            Self {
                enabled: true,
                title: t_for(
                    lang,
                    "Inspeccionar permisos NTFS/SMB (icacls)",
                    "Inspect NTFS/SMB permissions (icacls)",
                )
                .into(),
                command_line: "cmd /K icacls \"%p\"".into(),
                works_for_files: true,
                works_for_dirs: true,
                ask_confirmation: false,
                run_minimized: false,
                read_only_safe: true,
                refresh_policy: CleanupRefreshPolicy::None,
            },
            Self {
                enabled: true,
                title: t_for(
                    lang,
                    "Listar atributos y flujos NTFS (dir /R /A)",
                    "List NTFS attributes & streams (dir /R /A)",
                )
                .into(),
                command_line: "cmd /K dir /R /A \"%p\"".into(),
                works_for_files: true,
                works_for_dirs: true,
                ask_confirmation: false,
                run_minimized: false,
                read_only_safe: true,
                refresh_policy: CleanupRefreshPolicy::None,
            },
            Self {
                enabled: true,
                title: t_for(
                    lang,
                    "Abrir PowerShell en carpeta (soporta UNC)",
                    "Open PowerShell in folder (UNC aware)",
                )
                .into(),
                command_line: "powershell -NoExit -Command \"Set-Location -LiteralPath '%d'\"".into(),
                works_for_files: true,
                works_for_dirs: true,
                ask_confirmation: false,
                run_minimized: false,
                read_only_safe: true,
                refresh_policy: CleanupRefreshPolicy::None,
            },
            Self {
                enabled: true,
                title: t_for(
                    lang,
                    "Calcular hash SHA-256 en consola",
                    "Compute SHA-256 hash in console",
                )
                .into(),
                command_line: "powershell -NoExit -Command \"Get-FileHash -LiteralPath '%p' -Algorithm SHA256 | Format-List\"".into(),
                works_for_files: true,
                works_for_dirs: false,
                ask_confirmation: false,
                run_minimized: false,
                read_only_safe: true,
                refresh_policy: CleanupRefreshPolicy::None,
            },
            Self {
                enabled: true,
                title: t_for(
                    lang,
                    "Copiar ruta UNC real (\\\\servidor\\share\\...)",
                    "Copy real UNC path (\\\\server\\share\\...)",
                )
                .into(),
                command_line: "powershell -NoProfile -Command \"Set-Clipboard -Value '%u'\"".into(),
                works_for_files: true,
                works_for_dirs: true,
                ask_confirmation: false,
                run_minimized: true,
                read_only_safe: true,
                refresh_policy: CleanupRefreshPolicy::None,
            },
            Self {
                enabled: true,
                title: t_for(
                    lang,
                    "Consultar compresión NTFS (compact /q)",
                    "Query NTFS compression (compact /q)",
                )
                .into(),
                command_line: "cmd /K compact /q \"%p\"".into(),
                works_for_files: true,
                works_for_dirs: true,
                ask_confirmation: false,
                run_minimized: false,
                read_only_safe: true,
                refresh_policy: CleanupRefreshPolicy::None,
            },
            Self {
                enabled: true,
                title: t_for(
                    lang,
                    "Simular inventario carpeta (robocopy /L dry-run)",
                    "Dry-run folder inventory (robocopy /L)",
                )
                .into(),
                command_line: "cmd /K robocopy \"%d\" \"NUL\" /L /S /BYTES /NP /NDL /NFL".into(),
                works_for_files: true,
                works_for_dirs: true,
                ask_confirmation: false,
                run_minimized: false,
                read_only_safe: true,
                refresh_policy: CleanupRefreshPolicy::None,
            },
            Self {
                enabled: true,
                title: t_for(
                    lang,
                    "Comprimir NTFS (compact /c /i)",
                    "Compress NTFS (compact /c /i)",
                )
                .into(),
                command_line: "cmd /K compact /c /i \"%p\"".into(),
                works_for_files: true,
                works_for_dirs: true,
                ask_confirmation: true,
                run_minimized: false,
                read_only_safe: false,
                refresh_policy: CleanupRefreshPolicy::RefreshSelected,
            },
            Self {
                enabled: true,
                title: t_for(
                    lang,
                    "Quitar bloqueo Internet / MOTW (Unblock-File)",
                    "Remove Internet Mark-of-the-Web (Unblock-File)",
                )
                .into(),
                command_line: "powershell -NoExit -Command \"Unblock-File -LiteralPath '%p' -Verbose\"".into(),
                works_for_files: true,
                works_for_dirs: false,
                ask_confirmation: true,
                run_minimized: false,
                read_only_safe: false,
                refresh_policy: CleanupRefreshPolicy::None,
            },
            Self {
                enabled: true,
                title: t_for(
                    lang,
                    "Mover a _CUARENTENA_MEDIASTAT (mismo volumen)",
                    "Move to _CUARENTENA_MEDIASTAT (same volume)",
                )
                .into(),
                command_line: "powershell -NoExit -Command \"$d=Join-Path (Split-Path '%p' -Qualifier) '_CUARENTENA_MEDIASTAT'; New-Item -ItemType Directory -Force -Path $d | Out-Null; Move-Item -LiteralPath '%p' -Destination $d -Verbose\"".into(),
                works_for_files: true,
                works_for_dirs: true,
                ask_confirmation: true,
                run_minimized: false,
                read_only_safe: false,
                refresh_policy: CleanupRefreshPolicy::RefreshParent,
            },
        ]
    }

    pub fn expand_command(&self, target: &Path) -> String {
        self.expand_command_with_unc(target, None)
    }

    pub fn expand_command_with_unc(&self, target: &Path, unc_resolved: Option<&str>) -> String {
        let full = target.to_string_lossy();
        let unc = unc_resolved.filter(|s| !s.is_empty()).unwrap_or(full.as_ref());
        let name = target
            .file_name()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let parent = target
            .parent()
            .map(|p| p.to_string_lossy())
            .unwrap_or_else(|| full.clone());
        let ext = target
            .extension()
            .map(|e| e.to_string_lossy())
            .unwrap_or_default();

        self.command_line
            .replace("%p", &full)
            .replace("%u", unc)
            .replace("%n", &name)
            .replace("%d", &parent)
            .replace("%e", &ext)
    }
}

/// Configurable age-based storage tier for `StorageAnalyticsView` (WinDirStat 2.10 parity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageTierConfig {
    pub name: String,
    pub min_age_days: u32,
    pub cost_per_tib_monthly: f64,
    pub accent_rgb: [u8; 3],
}

impl StorageTierConfig {
    pub fn default_tiers() -> Vec<Self> {
        Self::default_tiers_for(active_language())
    }

    pub fn default_tiers_for(lang: Language) -> Vec<Self> {
        vec![
            Self {
                name: t_for(lang, "Hot (< 30 días)", "Hot (< 30 days)").into(),
                min_age_days: 0,
                cost_per_tib_monthly: 24.0,
                accent_rgb: [239, 68, 68],
            },
            Self {
                name: t_for(lang, "Warm (30–180 días)", "Warm (30–180 days)").into(),
                min_age_days: 30,
                cost_per_tib_monthly: 18.0,
                accent_rgb: [59, 130, 246],
            },
            Self {
                name: t_for(lang, "Cool (6–12 meses)", "Cool (6–12 months)").into(),
                min_age_days: 180,
                cost_per_tib_monthly: 12.0,
                accent_rgb: [139, 92, 246],
            },
            Self {
                name: t_for(lang, "Cold (1–3 años)", "Cold (1–3 years)").into(),
                min_age_days: 365,
                cost_per_tib_monthly: 6.0,
                accent_rgb: [13, 148, 136],
            },
            Self {
                name: t_for(lang, "Archive (> 3 años)", "Archive (> 3 years)").into(),
                min_age_days: 1095,
                cost_per_tib_monthly: 2.0,
                accent_rgb: [155, 81, 224],
            },
        ]
    }
}

/// Persistent application settings saved in `%APPDATA%\MediaStat\settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub language: Language,
    pub dark_mode: bool,
    pub ui_scale_percent: u16,
    pub layout: WindowLayoutPreset,
    pub visualization_kind: VisualizationKind,
    pub treemap: TreemapOptions,
    pub size_metric: SizeMetric,
    pub show_free_space: bool,        // F6
    pub show_unknown_space: bool,     // F7
    pub show_extensions_panel: bool,  // F8
    pub show_visualization: bool,     // F9
    pub show_toolbar: bool,
    pub show_status_bar: bool,
    pub group_files_node: bool,
    pub absolute_percentages: bool,
    pub show_timestamp_seconds: bool,
    pub group_minor_extensions: bool,
    pub read_only_lock_by_default: bool,
    pub use_pacman_animation: bool,
    #[serde(default)]
    pub float_visualization: bool,
    #[serde(default)]
    pub float_extensions_panel: bool,
    /// Use WinDirStat's 8 depth-level colors (`FileTreeColor0..7`) on subtree bars (default true).
    #[serde(default = "default_true")]
    pub tree_bar_classic_depth_colors: bool,
    #[serde(default = "default_tree_depth_colors")]
    pub tree_depth_colors: [[u8; 3]; 8],
    #[serde(default = "default_tree_cols")]
    pub tree_columns_visible: [bool; 12],
    #[serde(default = "default_true")]
    pub process_hardlinks: bool,
    #[serde(default = "default_true")]
    pub use_backup_restore_privilege: bool,
    #[serde(default)]
    pub process_priority: ProcessPriority,
    #[serde(default = "default_true")]
    pub exclude_junctions: bool,
    #[serde(default = "default_true")]
    pub exclude_symlinks_dir: bool,
    #[serde(default = "default_true")]
    pub exclude_symlinks_file: bool,
    #[serde(default = "default_true")]
    pub exclude_mount_points: bool,
    #[serde(default)]
    pub exclude_hidden: bool,
    #[serde(default)]
    pub exclude_protected_system: bool,
    #[serde(default)]
    pub search_history: Vec<String>,
    #[serde(default = "default_search_max")]
    pub search_max_results: usize,
    pub currency_symbol: String,
    pub default_cost_per_tib_monthly: f64,
    pub storage_tiers: Vec<StorageTierConfig>,
    pub user_cleanups: Vec<UserCleanupAction>,
    pub filter_config: FilterConfig,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: Language::Es,
            dark_mode: true,
            ui_scale_percent: 100,
            layout: WindowLayoutPreset::ClassicWinDirStat,
            visualization_kind: VisualizationKind::Treemap,
            treemap: TreemapOptions::default(),
            size_metric: SizeMetric::Logical,
            show_free_space: false,
            show_unknown_space: false,
            show_extensions_panel: true,
            show_visualization: true,
            show_toolbar: true,
            show_status_bar: true,
            group_files_node: false,
            absolute_percentages: false,
            show_timestamp_seconds: false,
            group_minor_extensions: false,
            read_only_lock_by_default: true,
            use_pacman_animation: true,
            float_visualization: false,
            float_extensions_panel: false,
            tree_bar_classic_depth_colors: true,
            tree_depth_colors: default_tree_depth_colors(),
            tree_columns_visible: default_tree_cols(),
            process_hardlinks: true,
            use_backup_restore_privilege: true,
            process_priority: ProcessPriority::Normal,
            exclude_junctions: true,
            exclude_symlinks_dir: true,
            exclude_symlinks_file: true,
            exclude_mount_points: true,
            exclude_hidden: false,
            exclude_protected_system: false,
            search_history: Vec::new(),
            search_max_results: default_search_max(),
            currency_symbol: "€".into(),
            default_cost_per_tib_monthly: 18.0,
            storage_tiers: StorageTierConfig::default_tiers(),
            user_cleanups: UserCleanupAction::default_presets(),
            filter_config: FilterConfig::default(),
        }
    }
}

impl AppSettings {
    pub fn config_path() -> Option<PathBuf> {
        #[cfg(windows)]
        let base = std::env::var_os("APPDATA").map(PathBuf::from);
        #[cfg(not(windows))]
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));

        base.map(|b| b.join("MediaStat").join("settings.json"))
    }

    pub fn load_or_default() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(path) = Self::config_path() else {
            return Err("No se encontró el directorio de configuración del usuario".into());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())
    }

    pub fn push_search_history(&mut self, query: &str) {
        let q = query.trim();
        if q.is_empty() {
            return;
        }
        self.search_history.retain(|h| !h.eq_ignore_ascii_case(q));
        self.search_history.insert(0, q.to_string());
        self.search_history.truncate(15);
    }

    /// Update `self.language` and automatically translate unmodified built-in preset titles
    /// (`user_cleanups` and `storage_tiers`), preserving any custom user-edited names.
    pub fn apply_language(&mut self, new_lang: Language) {
        self.language = new_lang;

        let es_cleanups = UserCleanupAction::default_presets_for(Language::Es);
        let en_cleanups = UserCleanupAction::default_presets_for(Language::En);
        let target_cleanups = UserCleanupAction::default_presets_for(new_lang);
        for (i, slot) in self.user_cleanups.iter_mut().enumerate() {
            if i < target_cleanups.len()
                && (slot.title == es_cleanups[i].title || slot.title == en_cleanups[i].title)
            {
                slot.title = target_cleanups[i].title.clone();
            }
        }

        let es_tiers = StorageTierConfig::default_tiers_for(Language::Es);
        let en_tiers = StorageTierConfig::default_tiers_for(Language::En);
        let target_tiers = StorageTierConfig::default_tiers_for(new_lang);
        for (i, tier) in self.storage_tiers.iter_mut().enumerate() {
            if i < target_tiers.len()
                && (tier.name == es_tiers[i].name || tier.name == en_tiers[i].name)
            {
                tier.name = target_tiers[i].name.clone();
            }
        }
    }

    /// Push `self.language` into the global active UI language atomic.
    pub fn sync_active_language(&self) {
        set_active_language(self.language);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cleanup_placeholder_expansion() {
        let act = UserCleanupAction {
            enabled: true,
            title: "Test".into(),
            command_line: "echo \"%p\" | \"%u\" | %n | %d | %e".into(),
            works_for_files: true,
            works_for_dirs: false,
            ask_confirmation: false,
            run_minimized: false,
            read_only_safe: true,
            refresh_policy: CleanupRefreshPolicy::None,
        };
        let out = act.expand_command_with_unc(
            Path::new(r"P:\Proyectos\RAW\IMG_001.CR2"),
            Some(r"\\vs-qnap-01\Proyectos\RAW\IMG_001.CR2"),
        );
        assert!(out.contains(r"P:\Proyectos\RAW\IMG_001.CR2"));
        assert!(out.contains(r"\\vs-qnap-01\Proyectos\RAW\IMG_001.CR2"));
        assert!(out.contains("IMG_001.CR2"));
        assert!(out.contains(r"P:\Proyectos\RAW"));
        assert!(out.contains("CR2"));
    }

    #[test]
    fn test_default_smb_cleanups_and_safety_split() {
        let presets = UserCleanupAction::default_presets();
        assert_eq!(presets.len(), 10);
        for slot in &presets[0..7] {
            assert!(slot.enabled);
            assert!(slot.read_only_safe);
            assert!(!slot.ask_confirmation);
        }
        for slot in &presets[7..10] {
            assert!(slot.enabled);
            assert!(!slot.read_only_safe);
            assert!(slot.ask_confirmation);
        }
        assert!(presets[4].command_line.contains("%u"));
    }

    #[test]
    fn test_all_12_layouts_and_v02_backward_compat() {
        assert_eq!(WindowLayoutPreset::ALL.len(), 12);
        for l in WindowLayoutPreset::ALL {
            let panes = l.def().preview_panes();
            let area: f32 = panes.iter().map(|p| p.3 * p.4).sum();
            assert!((area - 1.0).abs() < 0.01, "Layout {:?} area mismatch: {area}", l);
        }
        // Deserialize old v0.2 layout names via serde aliases
        let legacy: WindowLayoutPreset = serde_json::from_str("\"Ultrawide3Col\"").unwrap();
        assert_eq!(legacy, WindowLayoutPreset::AllFilesVizExt3Col);
        let legacy2: WindowLayoutPreset = serde_json::from_str("\"SideBySide\"").unwrap();
        assert_eq!(legacy2, WindowLayoutPreset::AllFilesOverExtVizRight);

        // Deserialize legacy settings containing removed Phase 2 fields & CompressionHeatmap
        let mut val = serde_json::to_value(AppSettings::default()).unwrap();
        let obj = val.as_object_mut().unwrap();
        obj.insert("detection_profile".into(), serde_json::json!({"preset": "Strict"}));
        for k in [
            "float_visualization",
            "tree_bar_classic_depth_colors",
            "tree_depth_colors",
            "tree_columns_visible",
            "process_hardlinks",
            "search_history",
            "search_max_results",
        ] {
            obj.remove(k);
        }
        let tm = obj.get_mut("treemap").unwrap().as_object_mut().unwrap();
        tm.remove("custom_volume_colors");
        tm.insert("color_mode".into(), serde_json::json!("CompressionHeatmap"));
        tm.insert(
            "custom_status_colors".into(),
            serde_json::json!({
                "strictly_uncompressed": [255, 0, 0],
                "free_space": [11, 22, 33],
                "unknown_space": [44, 55, 66]
            }),
        );
        for k in ["saturation", "grid_color", "highlight_color", "use_rank_extension_colors"] {
            tm.remove(k);
        }
        let parsed: AppSettings = serde_json::from_str(&serde_json::to_string(&val).unwrap()).unwrap();
        assert!(parsed.process_hardlinks);
        assert!(parsed.tree_bar_classic_depth_colors);
        assert_eq!(parsed.treemap.saturation, 1.0);
        assert!(parsed.treemap.use_rank_extension_colors);
        assert_eq!(parsed.treemap.color_mode, VizColorMode::ByExtension);
        assert_eq!(
            parsed.treemap.custom_volume_colors.as_ref().map(|v| v.free_space),
            Some([11, 22, 33])
        );
    }

    #[test]
    fn test_language_switch_preserves_custom_titles_and_backward_compat() {
        let mut s = AppSettings::default();
        assert_eq!(s.language, Language::Es);
        s.user_cleanups[0].title = "Mi script custom".into();
        s.storage_tiers[0].name = "Custom Hot Tier".into();

        s.apply_language(Language::En);
        assert_eq!(s.language, Language::En);
        assert_eq!(s.user_cleanups[0].title, "Mi script custom");
        assert!(s.user_cleanups[1].title.contains("NTFS attributes"));
        assert_eq!(s.storage_tiers[0].name, "Custom Hot Tier");
        assert!(s.storage_tiers[1].name.contains("days"));

        s.apply_language(Language::Es);
        assert_eq!(s.user_cleanups[0].title, "Mi script custom");
        assert!(s.user_cleanups[1].title.contains("atributos y flujos"));
        assert!(s.storage_tiers[1].name.contains("días"));

        // Missing `"language"` field in legacy settings.json defaults to Spanish
        let mut val = serde_json::to_value(&s).unwrap();
        val.as_object_mut().unwrap().remove("language");
        let parsed: AppSettings = serde_json::from_value(val).unwrap();
        assert_eq!(parsed.language, Language::Es);

        // Verify distinct ES vs EN labels on key enums
        assert_ne!(
            SizeMetric::Logical.label_for(Language::Es),
            SizeMetric::Logical.label_for(Language::En)
        );
        assert_ne!(
            WindowLayoutPreset::ClassicWinDirStat.label_for(Language::Es),
            WindowLayoutPreset::ClassicWinDirStat.label_for(Language::En)
        );
        assert_ne!(
            VizColorMode::ByCategory.label_for(Language::Es),
            VizColorMode::ByCategory.label_for(Language::En)
        );
    }
}
