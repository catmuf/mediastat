pub mod arena;
pub mod categories;
pub mod filtering;
pub mod format;
pub mod i18n;
pub mod profile;
pub mod settings;

pub use arena::{
    DirStats, DuplicateGroup, ExtensionInfo, ExtensionTable, HardlinkGroup, IssueKind, NodeFlags,
    NodeId, OwnerTable, PermissionEntry, ScanIssue, ScanSummary, ScanTree, StringArena,
    VolumeSpaceInfo, WinAttrs,
};
pub use categories::{
    is_known_image_ext, is_known_video_ext, is_previewable_image_ext, FileCategory,
};
pub use filtering::{AgeComparison, CompiledFilters, FilterConfig, SizeComparison};
pub use format::{
    age_color_for_palette, apply_saturation, blend_rgb, category_color_for_palette,
    extension_color, extension_color_for_palette, format_bytes, format_count, format_timestamp,
    format_timestamp_opts, free_space_color, hardlinks_color, is_light_rgb, make_bright_color,
    normalize_color, unknown_space_color, wcag_relative_luminance, windirstat_rank_color,
    GRAPH_PALETTE_BRIGHTNESS, WINDIRSTAT_FOLDER_BRANCH_8, WINDIRSTAT_PALETTE_18,
    WINDIRSTAT_TREE_DEPTH_8,
};
pub use i18n::{active_language, set_active_language, t, t_for, Language};
pub use profile::ScanConfig;
pub use settings::{
    AppSettings, CleanupRefreshPolicy, LayoutAxis, LayoutDef, LayoutPaneKind, ProcessPriority,
    SizeMetric, StorageTierConfig, TreemapLayoutStyle, TreemapOptions, TreemapPreset,
    UserCleanupAction, VisualizationKind, VizColorMode, VizColorPalette, VolumeColorOverrides,
    WindowLayoutPreset,
};
