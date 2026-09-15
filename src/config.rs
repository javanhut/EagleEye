//! `~/.config/raven/desktop.toml` is owned by Raven Settings and read here
//! only for the look, so EagleEye matches the rest of the desktop.

use std::path::PathBuf;

use serde::Deserialize;

pub const DEFAULT_ACCENT: &str = "#7AA2F7";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Light,
    #[default]
    Dark,
    Auto,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme_mode: ThemeMode,
    pub accent: String,
    pub transparency: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme_mode: ThemeMode::Dark,
            accent: DEFAULT_ACCENT.into(),
            transparency: true,
        }
    }
}

/// The slice of desktop.toml EagleEye cares about. Unknown keys are ignored
/// so Settings can grow without breaking us.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Desktop {
    pub appearance: Appearance,
}

impl Desktop {
    pub fn load() -> Desktop {
        std::fs::read_to_string(config_dir().join("desktop.toml"))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("raven")
}
