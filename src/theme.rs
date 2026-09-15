//! The look: Raven Glass, the stylesheet shared with Raven Settings, Store
//! and Viewer (`data/raven-glass.css`), plus the classes only EagleEye
//! draws. Accent and light/dark come from desktop.toml.

use gtk4 as gtk;
use libadwaita as adw;

use crate::config::{DEFAULT_ACCENT, Desktop, ThemeMode};

const BASE_CSS: &str = concat!(
    include_str!("../data/raven-glass.css"),
    r#"
/* ── EagleEye-only ───────────────────────────────────────────────────── */
.canvas { background-color: alpha(#000000, 0.18); }
window.raven.fullscreen .canvas { background-color: #000000; }

/* The floating controls: one glass pill over the image. */
.osd-bar {
  background-color: alpha(#1c1c23, 0.78);
  border: 1px solid alpha(#ffffff, 0.10);
  border-radius: 999px;
  padding: 4px;
  margin: 18px;
  box-shadow: 0 10px 30px alpha(#000000, 0.40), inset 0 1px 0 alpha(#ffffff, 0.08);
}
.osd-bar button { min-height: 32px; min-width: 32px; border-radius: 999px; padding: 0 8px; }
.osd-bar separator { margin: 6px 4px; }
.zoom-label { font-size: 12px; font-weight: 600; font-feature-settings: "tnum"; min-width: 56px; }
.welcome .title-1 { font-size: 30px; font-weight: 800; letter-spacing: -0.8px; }
"#
);

pub fn apply() {
    let display = gtk::gdk::Display::default().expect("no display");
    let base = gtk::CssProvider::new();
    base.load_from_string(BASE_CSS);
    gtk::style_context_add_provider_for_display(&display, &base, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let appearance = Desktop::load().appearance;
    adw::StyleManager::default().set_color_scheme(match appearance.theme_mode {
        ThemeMode::Dark => adw::ColorScheme::ForceDark,
        ThemeMode::Light => adw::ColorScheme::ForceLight,
        ThemeMode::Auto => adw::ColorScheme::PreferDark,
    });
    let accent = if is_hex(&appearance.accent) { appearance.accent.as_str() } else { DEFAULT_ACCENT };
    let css = format!(
        "@define-color accent_bg_color {accent};\n@define-color accent_color {accent};\n{}",
        if appearance.theme_mode == ThemeMode::Light { include_str!("../data/raven-glass-light.css") } else { "" }
    );
    let overlay = gtk::CssProvider::new();
    overlay.load_from_string(&css);
    gtk::style_context_add_provider_for_display(&display, &overlay, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
}

/// Whether the desktop asked for translucent windows.
pub fn glass() -> bool {
    Desktop::load().appearance.transparency
}

fn is_hex(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}
