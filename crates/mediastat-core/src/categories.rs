use serde::{Deserialize, Serialize};

use crate::i18n::{active_language, t_for, Language};

/// Known image file extensions (lowercase, without leading dot).
pub const KNOWN_IMAGE_EXTS: &[&str] = &[
    // Raster & Raw
    "bmp", "dib", "tga", "icb", "vda", "vst", "ppm", "pgm", "pbm", "pnm", "pam", "pfm", "qoi",
    "hdr", "exr", "psd", "psb", "pcx", "sgi", "rgb", "rgba",
    // TIFF & Camera RAW
    "tif", "tiff", "dng", "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "orf", "rw2", "raf",
    "pef", "srw", "raw", "rwl", "x3f", "erf", "mrw", "kdc", "dcr", "mos", "iiq", "3fr", "fff",
    "mef", "mdc",
    // Web & mobile
    "png", "apng", "jpg", "jpeg", "jpe", "jfif", "jif", "mpo", "webp", "gif", "heic", "heif",
    "hif", "heics", "avif", "avifs", "jp2", "j2k", "jpf", "jpx", "jxl", "ico",
];

/// Known video file extensions (lowercase, without leading dot).
pub const KNOWN_VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "mov", "qt", "mkv", "webm", "avi", "divx", "wmv", "asf", "flv", "f4v", "3gp",
    "3g2", "ts", "mts", "m2ts", "tp", "trp", "mpg", "mpeg", "mpe", "vob", "m2v", "m1v", "mod",
    "ogv", "ogm", "mxf", "rm", "rmvb", "vfw", "y4m", "yuv", "braw", "r3d", "crm", "dv", "dif",
    "gxf", "lxf", "cin", "dpx", "mkv3d", "mk3d", "ismv", "wtv", "dvr-ms",
];

/// Image formats directly decodable on demand by the GUI Inspector thumbnail previewer.
const PREVIEWABLE_IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "bmp", "dib", "gif", "webp", "tif", "tiff", "tga",
];

pub fn is_known_image_ext(ext_lower: &str) -> bool {
    KNOWN_IMAGE_EXTS.contains(&ext_lower)
}

pub fn is_known_video_ext(ext_lower: &str) -> bool {
    KNOWN_VIDEO_EXTS.contains(&ext_lower)
}

pub fn is_previewable_image_ext(ext_lower: &str) -> bool {
    PREVIEWABLE_IMAGE_EXTS.contains(&ext_lower)
}

/// High-level file category for Advanced Search filtering, Extension badges, and analytics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileCategory {
    Image,
    Video,
    Audio,
    Document,
    CadBim3d,
    ArchiveDisk,
    ExecutableCode,
    DatabaseMail,
    Other,
}

impl FileCategory {
    pub const ALL: [Self; 9] = [
        Self::Image,
        Self::Video,
        Self::Audio,
        Self::Document,
        Self::CadBim3d,
        Self::ArchiveDisk,
        Self::ExecutableCode,
        Self::DatabaseMail,
        Self::Other,
    ];

    pub fn label(self) -> &'static str {
        self.label_for(active_language())
    }

    pub fn label_for(self, lang: Language) -> &'static str {
        match self {
            Self::Image => t_for(lang, "Imágenes", "Images"),
            Self::Video => t_for(lang, "Vídeos", "Videos"),
            Self::Audio => "Audio",
            Self::Document => t_for(lang, "Documentos / Office", "Documents / Office"),
            Self::CadBim3d => "CAD / BIM / 3D",
            Self::ArchiveDisk => t_for(lang, "Comprimidos / ISO", "Archives / Disk Images"),
            Self::ExecutableCode => t_for(lang, "Ejecutables / Código", "Executables / Code"),
            Self::DatabaseMail => t_for(lang, "BD / Correo / Backup", "Databases / Mail / Backup"),
            Self::Other => t_for(lang, "Otros", "Other"),
        }
    }

    pub fn short_label(self) -> &'static str {
        self.short_label_for(active_language())
    }

    pub fn short_label_for(self, lang: Language) -> &'static str {
        match self {
            Self::Image => t_for(lang, "Imagen", "Image"),
            Self::Video => t_for(lang, "Vídeo", "Video"),
            Self::Audio => "Audio",
            Self::Document => "Docs",
            Self::CadBim3d => "CAD/3D",
            Self::ArchiveDisk => t_for(lang, "Archivo", "Archive"),
            Self::ExecutableCode => t_for(lang, "Ejec.", "Exec"),
            Self::DatabaseMail => t_for(lang, "BD/Mail", "DB/Mail"),
            Self::Other => t_for(lang, "Otro", "Other"),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Image => "🖼",
            Self::Video => "🎬",
            Self::Audio => "🎵",
            Self::Document => "📄",
            Self::CadBim3d => "📐",
            Self::ArchiveDisk => "📦",
            Self::ExecutableCode => "⚙",
            Self::DatabaseMail => "🗄",
            Self::Other => "📁",
        }
    }

    pub fn accent_rgb(self) -> [u8; 3] {
        match self {
            Self::Image => [46, 204, 113],
            Self::Video => [65, 131, 215],
            Self::Audio => [243, 180, 52],
            Self::Document => [220, 72, 76],
            Self::CadBim3d => [56, 189, 248],
            Self::ArchiveDisk => [230, 126, 34],
            Self::ExecutableCode => [130, 145, 165],
            Self::DatabaseMail => [168, 85, 247],
            Self::Other => [120, 130, 142],
        }
    }

    /// Classify a lowercase extension (without leading dot) into a `FileCategory`.
    pub fn from_extension(ext_lower: &str) -> Self {
        if ext_lower.is_empty() {
            return Self::Other;
        }
        if is_known_image_ext(ext_lower) || matches!(ext_lower, "svg" | "ai" | "eps" | "xcf") {
            return Self::Image;
        }
        if is_known_video_ext(ext_lower) {
            return Self::Video;
        }
        match ext_lower {
            "mp3" | "wav" | "flac" | "aac" | "ogg" | "oga" | "m4a" | "wma" | "opus" | "aiff"
            | "aif" | "alac" | "mid" | "midi" | "ac3" | "dts" => Self::Audio,

            "pdf" | "doc" | "docx" | "docm" | "dotx" | "xls" | "xlsx" | "xlsm" | "xlsb" | "ppt"
            | "pptx" | "pptm" | "odt" | "ods" | "odp" | "rtf" | "txt" | "md" | "csv" | "tsv"
            | "epub" | "pages" | "numbers" | "key" | "one" | "vsd" | "vsdx" | "mpp" => {
                Self::Document
            }

            "dwg" | "dxf" | "dwf" | "dwfx" | "dgn" | "rvt" | "rfa" | "rte" | "ifc" | "nwd"
            | "nwc" | "nwf" | "skp" | "skb" | "3ds" | "max" | "fbx" | "obj" | "mtl" | "stl"
            | "ply" | "blend" | "c4d" | "ma" | "mb" | "step" | "stp" | "iges" | "igs" | "sat"
            | "sldprt" | "sldasm" | "slddrw" | "ipt" | "iam" | "catpart" | "catproduct" | "las"
            | "laz" | "e57" | "rcp" | "rcs" | "shp" | "kml" | "kmz" | "gpx" => Self::CadBim3d,

            "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "lz4" | "cab"
            | "iso" | "img" | "vhd" | "vhdx" | "vmdk" | "vdi" | "qcow2" | "wim" | "esd" | "dmg"
            | "ova" | "ovf" => Self::ArchiveDisk,

            "exe" | "dll" | "sys" | "msi" | "msp" | "msix" | "appx" | "bat" | "cmd" | "ps1"
            | "psm1" | "vbs" | "wsf" | "sh" | "py" | "rs" | "c" | "cpp" | "h" | "hpp" | "cs"
            | "java" | "jar" | "js" | "ts" | "tsx" | "jsx" | "json" | "xml" | "yaml" | "yml"
            | "toml" | "ini" | "cfg" | "conf" | "reg" | "inf" | "html" | "htm" | "css" | "php"
            | "go" | "rb" | "pdb" | "lib" | "obj_bin" | "so" | "dylib" => Self::ExecutableCode,

            "sql" | "sqlite" | "sqlite3" | "db" | "db3" | "mdb" | "accdb" | "mdf" | "ldf"
            | "ndf" | "bak" | "bkp" | "dmp" | "dump" | "pst" | "ost" | "eml" | "msg" | "mbox"
            | "log" | "evtx" | "etl" | "parquet" | "avro" | "orc" => Self::DatabaseMail,

            _ => Self::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_category_classification() {
        assert_eq!(FileCategory::from_extension("cr2"), FileCategory::Image);
        assert_eq!(FileCategory::from_extension("mov"), FileCategory::Video);
        assert_eq!(FileCategory::from_extension("flac"), FileCategory::Audio);
        assert_eq!(FileCategory::from_extension("xlsx"), FileCategory::Document);
        assert_eq!(FileCategory::from_extension("dwg"), FileCategory::CadBim3d);
        assert_eq!(FileCategory::from_extension("rvt"), FileCategory::CadBim3d);
        assert_eq!(FileCategory::from_extension("vhdx"), FileCategory::ArchiveDisk);
        assert_eq!(FileCategory::from_extension("ps1"), FileCategory::ExecutableCode);
        assert_eq!(FileCategory::from_extension("pst"), FileCategory::DatabaseMail);
        assert_eq!(FileCategory::from_extension("xyz_unknown"), FileCategory::Other);
    }
}
