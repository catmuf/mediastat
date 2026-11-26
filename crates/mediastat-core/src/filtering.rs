use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SizeComparison {
    #[default]
    Ignore,
    GreaterThan,
    LessThan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AgeComparison {
    #[default]
    Ignore,
    OlderThanDays,
    NewerThanDays,
}

/// WinDirStat 2.10-compatible pre-scan & live filtering configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterConfig {
    /// Use regular expressions instead of simple wildcard globs (`*`, `?`).
    pub use_regex: bool,
    /// Directory patterns to exclude (one per line or semicolon-separated).
    pub exclude_dirs: String,
    /// File patterns to exclude.
    pub exclude_files: String,
    /// Optional include-only directory patterns (empty = include all).
    pub include_dirs: String,
    /// Optional include-only file patterns (empty = include all).
    pub include_files: String,
    /// Optional file size filter.
    pub size_cmp: SizeComparison,
    pub size_bytes: u64,
    /// Optional file modification age filter.
    pub age_cmp: AgeComparison,
    pub age_days: u32,
    /// Skip reading content of OneDrive/Azure File Sync cloud-only offline files (Anti-Hydration).
    pub protect_cloud_offline_files: bool,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            use_regex: false,
            exclude_dirs: "$RECYCLE.BIN\nSystem Volume Information".to_string(),
            exclude_files: String::new(),
            include_dirs: String::new(),
            include_files: String::new(),
            size_cmp: SizeComparison::Ignore,
            size_bytes: 0,
            age_cmp: AgeComparison::Ignore,
            age_days: 0,
            protect_cloud_offline_files: true,
        }
    }
}

/// Pre-compiled, thread-safe filter matcher used during multithreaded scanning.
#[derive(Debug, Clone, Default)]
pub struct CompiledFilters {
    pub exclude_dirs: Vec<Regex>,
    pub exclude_files: Vec<Regex>,
    pub include_dirs: Vec<Regex>,
    pub include_files: Vec<Regex>,
    pub size_cmp: SizeComparison,
    pub size_bytes: u64,
    pub age_cmp: AgeComparison,
    pub age_cutoff_unix: Option<i64>,
    pub protect_cloud_offline_files: bool,
}

impl CompiledFilters {
    pub fn compile(cfg: &FilterConfig, extra_excluded_dirs: &[String]) -> Result<Self, String> {
        let mut exclude_dirs = compile_list(&cfg.exclude_dirs, cfg.use_regex)?;
        for extra in extra_excluded_dirs {
            let trimmed = extra.trim();
            if !trimmed.is_empty() {
                exclude_dirs.push(compile_single(trimmed, false)?);
            }
        }
        let exclude_files = compile_list(&cfg.exclude_files, cfg.use_regex)?;
        let include_dirs = compile_list(&cfg.include_dirs, cfg.use_regex)?;
        let include_files = compile_list(&cfg.include_files, cfg.use_regex)?;

        let age_cutoff_unix = match cfg.age_cmp {
            AgeComparison::Ignore => None,
            AgeComparison::OlderThanDays | AgeComparison::NewerThanDays => {
                let now = chrono::Utc::now().timestamp();
                Some(now.saturating_sub(cfg.age_days as i64 * 86_400))
            }
        };

        Ok(Self {
            exclude_dirs,
            exclude_files,
            include_dirs,
            include_files,
            size_cmp: cfg.size_cmp,
            size_bytes: cfg.size_bytes,
            age_cmp: cfg.age_cmp,
            age_cutoff_unix,
            protect_cloud_offline_files: cfg.protect_cloud_offline_files,
        })
    }

    /// Returns `true` if a directory should be skipped during traversal.
    pub fn is_dir_excluded(&self, dir_name: &str, full_path: &str) -> bool {
        self.exclude_dirs
            .iter()
            .any(|re| re.is_match(dir_name) || re.is_match(full_path))
    }

    /// Returns `true` if a file should be excluded from the scan tree.
    pub fn is_file_excluded(
        &self,
        file_name: &str,
        full_path: &str,
        logical_size: u64,
        mtime_secs: i64,
    ) -> bool {
        if self
            .exclude_files
            .iter()
            .any(|re| re.is_match(file_name) || re.is_match(full_path))
        {
            return true;
        }
        if !self.include_files.is_empty()
            && !self
                .include_files
                .iter()
                .any(|re| re.is_match(file_name) || re.is_match(full_path))
        {
            return true;
        }
        if !self.include_dirs.is_empty()
            && !self.include_dirs.iter().any(|re| re.is_match(full_path))
        {
            return true;
        }
        match self.size_cmp {
            SizeComparison::Ignore => {}
            SizeComparison::GreaterThan => {
                if logical_size <= self.size_bytes {
                    return true;
                }
            }
            SizeComparison::LessThan => {
                if logical_size >= self.size_bytes {
                    return true;
                }
            }
        }
        if let Some(cutoff) = self.age_cutoff_unix {
            if mtime_secs > 0 {
                match self.age_cmp {
                    AgeComparison::OlderThanDays if mtime_secs > cutoff => return true,
                    AgeComparison::NewerThanDays if mtime_secs < cutoff => return true,
                    _ => {}
                }
            }
        }
        false
    }
}

fn compile_list(raw: &str, use_regex: bool) -> Result<Vec<Regex>, String> {
    let mut out = Vec::new();
    for line in raw.split(['\n', ';']) {
        let pat = line.trim();
        if pat.is_empty() {
            continue;
        }
        out.push(compile_single(pat, use_regex)?);
    }
    Ok(out)
}

pub fn compile_single(pattern: &str, use_regex: bool) -> Result<Regex, String> {
    let regex_str = if use_regex {
        pattern.to_string()
    } else {
        glob_to_regex(pattern)
    };
    RegexBuilder::new(&regex_str)
        .case_insensitive(true)
        .build()
        .map_err(|e| format!("Patrón inválido '{pattern}': {e}"))
}

/// Convert a simple wildcard glob (`*`, `?`) into an anchored regex.
pub fn glob_to_regex(glob: &str) -> String {
    let mut out = String::with_capacity(glob.len() + 8);
    out.push('^');
    for ch in glob.chars() {
        match ch {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '[' | ']' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            c => out.push(c),
        }
    }
    out.push('$');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_and_regex_filtering() {
        let cfg = FilterConfig {
            use_regex: false,
            exclude_dirs: "$RECYCLE.BIN\nnode_modules".into(),
            exclude_files: "*.tmp;*.bak".into(),
            size_cmp: SizeComparison::GreaterThan,
            size_bytes: 1000,
            ..Default::default()
        };
        let f = CompiledFilters::compile(&cfg, &[]).unwrap();
        assert!(f.is_dir_excluded("node_modules", r"C:\proj\node_modules"));
        assert!(!f.is_dir_excluded("src", r"C:\proj\src"));
        assert!(f.is_file_excluded("cache.tmp", r"C:\cache.tmp", 5000, 0));
        assert!(f.is_file_excluded("small.jpg", r"C:\small.jpg", 500, 0));
        assert!(!f.is_file_excluded("big.jpg", r"C:\big.jpg", 5000, 0));
    }
}
