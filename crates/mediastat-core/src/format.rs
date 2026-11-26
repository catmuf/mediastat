use chrono::{DateTime, Local, TimeZone, Utc};

use crate::categories::FileCategory;
use crate::settings::{TreemapOptions, VizColorPalette, VolumeColorOverrides};

/// WinDirStat canonical palette brightness (`CColorSpace::GraphPaletteBrightness = 0.6`).
pub const GRAPH_PALETTE_BRIGHTNESS: f64 = 0.6;

/// Exact 18-color default palette from upstream `windirstat/windirstat` (`Controls/TreeMap.h:151-170`).
/// Assigned by descending extension byte rank via `make_bright_color(c, 0.6)`.
pub const WINDIRSTAT_PALETTE_18: [[u8; 3]; 18] = [
    [0, 0, 255],     // #0  Blue
    [255, 0, 0],     // #1  Red
    [0, 255, 0],     // #2  Green
    [255, 255, 0],   // #3  Yellow
    [0, 255, 255],   // #4  Cyan
    [255, 0, 255],   // #5  Magenta
    [255, 170, 0],   // #6  Orange
    [0, 85, 255],    // #7  Dodger Blue
    [255, 0, 85],    // #8  Hot Pink
    [85, 255, 0],    // #9  Lime Green
    [170, 0, 255],   // #10 Violet
    [0, 255, 85],    // #11 Spring Green
    [255, 0, 170],   // #12 Deep Pink
    [0, 170, 255],   // #13 Sky Blue
    [255, 85, 0],    // #14 Orange Red
    [0, 255, 170],   // #15 Aquamarine
    [85, 0, 255],    // #16 Indigo
    [255, 255, 255], // #17 White (fallback for rank >= 17)
];

/// Exact 8-color branch palette from upstream `CFolderColors::GetColor` (`Controls/TreeMap.cpp:104-107`)
/// used by Sunburst and Flame Graph for hierarchical folder coloring.
pub const WINDIRSTAT_FOLDER_BRANCH_8: [[u8; 3]; 8] = [
    [79, 157, 232],
    [71, 173, 128],
    [230, 159, 0],
    [213, 94, 0],
    [168, 135, 224],
    [86, 180, 233],
    [204, 121, 167],
    [230, 205, 70],
];

/// Exact 8-level FileTree percentage bar colors from upstream `Options.h:160-170` (`FileTreeColor0..7`).
pub const WINDIRSTAT_TREE_DEPTH_8: [[u8; 3]; 8] = [
    [64, 64, 140],
    [140, 64, 64],
    [64, 140, 64],
    [140, 140, 64],
    [0, 0, 255],
    [255, 0, 0],
    [0, 255, 0],
    [255, 255, 0],
];

/// Exact WinDirStat `CColorSpace::NormalizeColor` — distributes overflow > 255 across the other two
/// channels to produce authentic bright Cushion specular highlights without harsh flat clipping.
#[inline]
pub fn normalize_color(mut r: i32, mut g: i32, mut b: i32) -> [u8; 3] {
    fn distribute(first: &mut i32, second: &mut i32, third: &mut i32) {
        let h = (*first - 255) / 2;
        *first = 255;
        *second += h;
        *third += h;
        if *second > 255 {
            let j = *second - 255;
            *second = 255;
            *third += j;
        } else if *third > 255 {
            let j = *third - 255;
            *third = 255;
            *second += j;
        }
    }
    if r > 255 {
        distribute(&mut r, &mut g, &mut b);
    } else if g > 255 {
        distribute(&mut g, &mut r, &mut b);
    } else if b > 255 {
        distribute(&mut b, &mut r, &mut g);
    }
    [
        r.clamp(0, 255) as u8,
        g.clamp(0, 255) as u8,
        b.clamp(0, 255) as u8,
    ]
}

/// Exact WinDirStat `CColorSpace::MakeBrightColor(rgb, brightness)` (`Controls/TreeMap.h:531-552`).
/// Scales any RGB color to uniform arithmetic brightness `brightness` (`0.0..=1.0`, default `0.6`).
#[inline]
pub fn make_bright_color(rgb: [u8; 3], brightness: f64) -> [u8; 3] {
    let b_clamped = brightness.clamp(0.0, 1.0);
    let mut dr = rgb[0] as f64 / 255.0;
    let mut dg = rgb[1] as f64 / 255.0;
    let mut db = rgb[2] as f64 / 255.0;
    let sum = dr + dg + db;
    if sum <= 1e-6 {
        let v = (b_clamped * 255.0).clamp(0.0, 255.0) as u8;
        return [v, v, v];
    }
    let f = 3.0 * b_clamped / sum;
    dr *= f;
    dg *= f;
    db *= f;
    normalize_color((dr * 255.0) as i32, (dg * 255.0) as i32, (db * 255.0) as i32)
}

/// Exact WinDirStat `CColorSpace::BlendColor(color, background, amount)`.
#[inline]
pub fn blend_rgb(color: [u8; 3], bg: [u8; 3], amount: f32) -> [u8; 3] {
    let a = amount.clamp(0.0, 1.0);
    [
        (color[0] as f32 * (1.0 - a) + bg[0] as f32 * a).clamp(0.0, 255.0) as u8,
        (color[1] as f32 * (1.0 - a) + bg[1] as f32 * a).clamp(0.0, 255.0) as u8,
        (color[2] as f32 * (1.0 - a) + bg[2] as f32 * a).clamp(0.0, 255.0) as u8,
    ]
}

/// Exact WinDirStat `PrepareRenderColor` saturation desaturation (`Controls/TreeMap.cpp:742-745`).
#[inline]
pub fn apply_saturation(rgb: [u8; 3], saturation: f32) -> [u8; 3] {
    let sat = saturation.clamp(0.0, 1.0);
    if (sat - 1.0).abs() < 1e-4 {
        return rgb;
    }
    let gray = ((rgb[0] as u16 + rgb[1] as u16 + rgb[2] as u16) / 3) as u8;
    blend_rgb(rgb, [gray, gray, gray], 1.0 - sat)
}

/// Exact WinDirStat WCAG 2.1 relative luminance (`CColorSpace::GetRelativeLuminance`, `TreeMap.h:515-528`).
#[inline]
pub fn wcag_relative_luminance(rgb: [u8; 3]) -> f64 {
    #[inline]
    fn to_linear(c: u8) -> f64 {
        let s = c as f64 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * to_linear(rgb[0]) + 0.7152 * to_linear(rgb[1]) + 0.0722 * to_linear(rgb[2])
}

/// Returns true if an RGB background is bright under WCAG (`relative_luminance > 0.179`, exact WinDirStat threshold)
/// so black/dark text provides higher contrast than white text.
#[inline]
pub fn is_light_rgb(rgb: [u8; 3]) -> bool {
    wcag_relative_luminance(rgb) > 0.179
}

/// WinDirStat 18-color rank palette entry normalized to `GraphPaletteBrightness = 0.6`.
#[inline]
pub fn windirstat_rank_color(rank: usize) -> [u8; 3] {
    let raw = WINDIRSTAT_PALETTE_18[rank.min(WINDIRSTAT_PALETTE_18.len() - 1)];
    make_bright_color(raw, GRAPH_PALETTE_BRIGHTNESS)
}

/// Format a byte count into a concise human-readable binary string (B, KB, MB, GB, TB, PB).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut val = bytes as f64;
    let mut unit = 0usize;
    while val >= 1024.0 && unit + 1 < UNITS.len() {
        val /= 1024.0;
        unit += 1;
    }
    if val >= 100.0 {
        format!("{val:.1} {}", UNITS[unit])
    } else {
        format!("{val:.2} {}", UNITS[unit])
    }
}

/// Format large integer counts with thousands separators (e.g., `12.450.120`).
pub fn format_count(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let len = bytes.len();
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(b as char);
    }
    out
}

/// Format Unix timestamp (seconds) to local `YYYY-MM-DD HH:MM`.
pub fn format_timestamp(mtime_secs: i64) -> String {
    format_timestamp_opts(mtime_secs, false)
}

/// Format Unix timestamp with optional seconds (`YYYY-MM-DD HH:MM:SS`).
pub fn format_timestamp_opts(mtime_secs: i64, with_seconds: bool) -> String {
    if mtime_secs <= 0 {
        return "—".to_string();
    }
    match Utc.timestamp_opt(mtime_secs, 0).single() {
        Some(dt_utc) => {
            let local: DateTime<Local> = dt_utc.with_timezone(&Local);
            if with_seconds {
                local.format("%Y-%m-%d %H:%M:%S").to_string()
            } else {
                local.format("%Y-%m-%d %H:%M").to_string()
            }
        }
        None => "—".to_string(),
    }
}

/// Semantic RGB color for a file extension (includes media, CAD/BIM, DB/Mail, archives).
pub fn extension_color(ext_lower: &str) -> [u8; 3] {
    match ext_lower {
        // Videos — rich blues / purples / teals
        "mp4" | "m4v" => [65, 131, 215],
        "mov" | "qt" => [108, 92, 231],
        "mkv" => [52, 152, 219],
        "avi" => [142, 68, 173],
        "webm" => [0, 184, 148],
        "wmv" | "asf" | "flv" => [155, 89, 182],
        "mts" | "m2ts" | "ts" | "mxf" => [41, 128, 185],
        "braw" | "r3d" | "y4m" | "yuv" => [192, 57, 43],
        // Uncompressed / RAW images — vivid reds & magentas
        "bmp" | "tga" | "ppm" | "pgm" | "pbm" => [231, 76, 60],
        "tif" | "tiff" => [211, 84, 0],
        "cr2" | "cr3" | "nef" | "arw" | "dng" | "raf" | "orf" | "rw2" => [232, 67, 147],
        "psd" | "psb" | "exr" => [181, 52, 113],
        // Common images — warm greens, yellows, oranges
        "jpg" | "jpeg" | "jpe" => [46, 204, 113],
        "png" => [26, 188, 156],
        "heic" | "heif" => [243, 156, 18],
        "webp" | "avif" => [22, 160, 133],
        "gif" => [241, 196, 15],
        // CAD / BIM / 3D Engineering
        "dwg" | "dxf" => [0, 168, 232],
        "rvt" | "rfa" | "ifc" | "nwd" | "nwc" => [56, 189, 248],
        "skp" | "3ds" | "fbx" | "obj" | "blend" | "step" | "stp" | "iges" => [168, 85, 247],
        // Databases, Mail & Backups
        "bak" | "sql" | "mdb" | "accdb" => [217, 119, 6],
        "pst" | "ost" | "eml" | "msg" => [234, 179, 8],
        // Archives & binaries
        "zip" | "7z" | "rar" | "tar" | "gz" | "zst" => [230, 126, 34],
        "iso" | "img" | "vhd" | "vhdx" | "vmdk" => [148, 163, 184],
        "exe" | "dll" | "sys" | "msi" => [99, 110, 114],
        "pdf" => [220, 62, 66],
        "doc" | "docx" => [59, 130, 246],
        "xls" | "xlsx" | "csv" => [34, 197, 94],
        "ppt" | "pptx" => [249, 115, 22],
        "mp3" | "flac" | "wav" | "aac" | "ogg" | "m4a" => [253, 203, 110],
        "" => [127, 140, 141],
        other => hash_extension_color(other, 0.58, 0.45),
    }
}

/// Palette-aware semantic extension color (used when `use_rank_extension_colors == false`
/// or for non-rank palettes like Okabe-Ito / SysAdminThermal).
pub fn extension_color_for_palette(ext_lower: &str, palette: VizColorPalette) -> [u8; 3] {
    match palette {
        VizColorPalette::MediaStatModern => extension_color(ext_lower),
        VizColorPalette::WinDirStatClassic => {
            // Classic primary hues normalized to WinDirStat's 0.6 brightness
            let raw = match ext_lower {
                "mp4" | "m4v" | "mkv" | "mov" | "avi" | "wmv" | "webm" => [0, 0, 255],
                "bmp" | "tif" | "tiff" | "tga" | "raw" | "dng" | "cr2" | "cr3" | "nef" | "arw" => {
                    [255, 0, 0]
                }
                "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" => [0, 255, 0],
                "dwg" | "dxf" | "rvt" | "ifc" | "skp" => [0, 255, 255],
                "zip" | "7z" | "rar" | "tar" | "gz" => [255, 255, 0],
                "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" => [255, 0, 255],
                "mp3" | "wav" | "flac" | "aac" => [255, 170, 0],
                "exe" | "dll" | "sys" | "msi" => [180, 180, 180],
                "" => [160, 160, 160],
                other => {
                    let h = fnv1a(other.as_bytes()) as usize;
                    WINDIRSTAT_PALETTE_18[h % WINDIRSTAT_PALETTE_18.len()]
                }
            };
            make_bright_color(raw, GRAPH_PALETTE_BRIGHTNESS)
        }
        VizColorPalette::ColorblindOkabeIto => {
            const OKABE: &[[u8; 3]] = &[
                [0, 114, 178],
                [230, 159, 0],
                [0, 158, 115],
                [86, 180, 233],
                [213, 94, 0],
                [204, 121, 167],
                [240, 228, 66],
                [120, 130, 145],
            ];
            match ext_lower {
                "mp4" | "m4v" | "mov" | "mkv" | "avi" | "webm" => OKABE[0],
                "bmp" | "tif" | "tiff" | "tga" | "y4m" | "braw" | "r3d" => OKABE[4],
                "cr2" | "cr3" | "nef" | "arw" | "dng" | "raf" | "psd" | "exr" => OKABE[5],
                "jpg" | "jpeg" | "png" | "webp" | "heic" | "avif" => OKABE[2],
                "dwg" | "dxf" | "rvt" | "ifc" | "skp" => OKABE[3],
                "zip" | "7z" | "rar" | "bak" | "pst" | "ost" => OKABE[1],
                "pdf" | "doc" | "docx" | "xls" | "xlsx" => OKABE[6],
                "" => OKABE[7],
                other => {
                    let h = fnv1a(other.as_bytes()) as usize;
                    OKABE[h % OKABE.len()]
                }
            }
        }
        VizColorPalette::SysAdminThermal => match ext_lower {
            "bmp" | "tga" | "tif" | "tiff" | "y4m" | "braw" | "r3d" => [255, 42, 85],
            "cr2" | "cr3" | "nef" | "arw" | "dng" | "raf" | "orf" | "rw2" | "psd" | "exr" => {
                [255, 0, 170]
            }
            "mp4" | "mov" | "mkv" | "avi" | "mts" | "mxf" => [255, 138, 0],
            "jpg" | "jpeg" | "png" | "heic" => [255, 204, 0],
            "dwg" | "dxf" | "rvt" | "ifc" | "skp" => [56, 189, 248],
            "zip" | "7z" | "rar" | "iso" | "vmdk" | "vhdx" | "bak" | "pst" => [168, 85, 247],
            "" => [68, 76, 88],
            other => hash_extension_color(other, 0.24, 0.34),
        },
    }
}

/// Palette-aware semantic color for each of the 9 high-level file categories (0 I/O).
pub fn category_color_for_palette(cat: FileCategory, palette: VizColorPalette) -> [u8; 3] {
    match palette {
        VizColorPalette::WinDirStatClassic => {
            let raw = match cat {
                FileCategory::Video => [0, 0, 255],
                FileCategory::Image => [0, 255, 0],
                FileCategory::Audio => [255, 170, 0],
                FileCategory::Document => [255, 0, 255],
                FileCategory::CadBim3d => [0, 255, 255],
                FileCategory::ArchiveDisk => [255, 255, 0],
                FileCategory::ExecutableCode => [170, 0, 255],
                FileCategory::DatabaseMail => [255, 0, 0],
                FileCategory::Other => [170, 170, 170],
            };
            make_bright_color(raw, GRAPH_PALETTE_BRIGHTNESS)
        }
        VizColorPalette::MediaStatModern => cat.accent_rgb(),
        VizColorPalette::ColorblindOkabeIto => match cat {
            FileCategory::Video => [0, 114, 178],
            FileCategory::Image => [0, 158, 115],
            FileCategory::Audio => [240, 228, 66],
            FileCategory::Document => [204, 121, 167],
            FileCategory::CadBim3d => [86, 180, 233],
            FileCategory::ArchiveDisk => [230, 159, 0],
            FileCategory::ExecutableCode => [120, 134, 152],
            FileCategory::DatabaseMail => [213, 94, 0],
            FileCategory::Other => [92, 100, 112],
        },
        VizColorPalette::SysAdminThermal => match cat {
            FileCategory::Video => [255, 138, 0],
            FileCategory::Image => [255, 42, 85],
            FileCategory::Audio => [255, 204, 0],
            FileCategory::Document => [76, 166, 255],
            FileCategory::CadBim3d => [56, 210, 248],
            FileCategory::ArchiveDisk => [168, 85, 247],
            FileCategory::ExecutableCode => [88, 98, 112],
            FileCategory::DatabaseMail => [255, 0, 170],
            FileCategory::Other => [64, 72, 84],
        },
    }
}

/// Palette-aware file-age color scale (Hot <30d → Archive >4y).
pub fn age_color_for_palette(age_days: f32, palette: VizColorPalette) -> [u8; 3] {
    let stops: [[u8; 3]; 6] = match palette {
        VizColorPalette::ColorblindOkabeIto => [
            [0, 114, 178],
            [86, 180, 233],
            [0, 158, 115],
            [240, 228, 66],
            [230, 159, 0],
            [213, 94, 0],
        ],
        VizColorPalette::SysAdminThermal => [
            [42, 74, 112],
            [48, 128, 148],
            [110, 168, 72],
            [255, 200, 32],
            [255, 108, 24],
            [255, 36, 72],
        ],
        _ => [
            [56, 168, 232],
            [46, 196, 162],
            [134, 196, 78],
            [235, 180, 52],
            [235, 112, 52],
            [206, 56, 68],
        ],
    };
    if age_days < 30.0 {
        stops[0]
    } else if age_days < 180.0 {
        stops[1]
    } else if age_days < 365.0 {
        stops[2]
    } else if age_days < 730.0 {
        stops[3]
    } else if age_days < 1460.0 {
        stops[4]
    } else {
        stops[5]
    }
}

pub fn free_space_color(opts: &TreemapOptions) -> [u8; 3] {
    opts.custom_volume_colors
        .as_ref()
        .map(|o| o.free_space)
        .unwrap_or_else(|| VolumeColorOverrides::from_palette(opts.color_palette).free_space)
}

pub fn unknown_space_color(opts: &TreemapOptions) -> [u8; 3] {
    opts.custom_volume_colors
        .as_ref()
        .map(|o| o.unknown_space)
        .unwrap_or_else(|| VolumeColorOverrides::from_palette(opts.color_palette).unknown_space)
}

/// Exact WinDirStat `<Hardlinks>` container warm-tan color (`Item.Extended.cpp:1175`).
pub fn hardlinks_color() -> [u8; 3] {
    make_bright_color([200, 150, 100], GRAPH_PALETTE_BRIGHTNESS)
}

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 2166136261;
    for &b in bytes {
        hash ^= b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

fn hash_extension_color(other: &str, base_sat: f32, base_light: f32) -> [u8; 3] {
    let hash = fnv1a(other.as_bytes());
    let hue = (hash % 360) as f32;
    let sat = (base_sat + (((hash >> 8) % 22) as f32 / 100.0)).clamp(0.15, 0.92);
    let light = (base_light + (((hash >> 16) % 16) as f32 / 100.0)).clamp(0.24, 0.72);
    hsl_to_rgb(hue, sat, light)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [
        ((r + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((g + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((b + m) * 255.0).clamp(0.0, 255.0) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_windirstat_color_math_and_wcag_contrast() {
        // Pure blue (0,0,255) scaled to brightness 0.6 => (0+0+1)=1, f=1.8 => b=459 => normalized (102,102,255)
        let blue_bright = make_bright_color([0, 0, 255], GRAPH_PALETTE_BRIGHTNESS);
        assert_eq!(blue_bright[2], 255);
        assert!(blue_bright[0] > 90 && blue_bright[1] > 90);

        // WCAG relative luminance contrast check
        assert!(is_light_rgb([255, 255, 0]));
        assert!(is_light_rgb([240, 228, 66]));
        assert!(!is_light_rgb([30, 40, 60]));
        assert!(!is_light_rgb([0, 0, 200]));

        // Saturation 0.0 produces grayscale
        let gray = apply_saturation([255, 0, 0], 0.0);
        assert_eq!(gray[0], gray[1]);
        assert_eq!(gray[1], gray[2]);

        for pal in VizColorPalette::ALL {
            let dwg = extension_color_for_palette("dwg", pal);
            let bmp = extension_color_for_palette("bmp", pal);
            assert_ne!(dwg, [0, 0, 0]);
            assert_ne!(bmp, [0, 0, 0]);
            for cat in FileCategory::ALL {
                assert_ne!(category_color_for_palette(cat, pal), [0, 0, 0]);
            }
        }
    }
}
