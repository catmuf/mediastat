//! Lightweight, zero-allocation Spanish / English (`ES` / `EN`) internationalization layer.
//!
//! Designed around an explicit [`Language`] enum persisted in `AppSettings.language` and mirrored
//! into a lock-free `AtomicU8` for instant live UI switching without restarting the application.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

/// Supported UI & report languages (extensible to additional languages in future versions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[repr(u8)]
pub enum Language {
    #[default]
    #[serde(alias = "es", alias = "ES", alias = "Spanish", alias = "Español")]
    Es = 0,
    #[serde(alias = "en", alias = "EN", alias = "English", alias = "Inglés")]
    En = 1,
}

impl Language {
    pub const ALL: [Self; 2] = [Self::Es, Self::En];

    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::En,
            _ => Self::Es,
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code.trim().to_ascii_lowercase().as_str() {
            "es" | "es-es" | "spanish" | "español" | "espanol" => Some(Self::Es),
            "en" | "en-us" | "en-gb" | "english" | "inglés" | "ingles" => Some(Self::En),
            _ => None,
        }
    }

    #[inline]
    pub fn native_name(self) -> &'static str {
        match self {
            Self::Es => "Español",
            Self::En => "English",
        }
    }

    #[inline]
    pub fn code(self) -> &'static str {
        match self {
            Self::Es => "es",
            Self::En => "en",
        }
    }
}

static ACTIVE_LANGUAGE: AtomicU8 = AtomicU8::new(Language::Es as u8);

/// Set the process-wide active UI language (takes effect immediately on the next frame).
#[inline]
pub fn set_active_language(lang: Language) {
    ACTIVE_LANGUAGE.store(lang as u8, Ordering::Relaxed);
}

/// Read the currently active UI language.
#[inline]
pub fn active_language() -> Language {
    Language::from_u8(ACTIVE_LANGUAGE.load(Ordering::Relaxed))
}

/// Pick the localized static string for a specific [`Language`].
#[inline]
pub fn t_for(lang: Language, es: &'static str, en: &'static str) -> &'static str {
    match lang {
        Language::Es => es,
        Language::En => en,
    }
}

/// Pick the localized static string for the currently active UI language.
#[inline]
pub fn t(es: &'static str, en: &'static str) -> &'static str {
    t_for(active_language(), es, en)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_codes_and_t_for() {
        assert_eq!(Language::from_code("es"), Some(Language::Es));
        assert_eq!(Language::from_code("EN-US"), Some(Language::En));
        assert_eq!(Language::from_code("english"), Some(Language::En));
        assert_eq!(Language::from_code("fr"), None);

        assert_eq!(t_for(Language::Es, "Archivo", "File"), "Archivo");
        assert_eq!(t_for(Language::En, "Archivo", "File"), "File");
    }
}
