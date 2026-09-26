//! The look: Raven Glass, the stylesheet shared with Raven Settings, Store
//! and Viewer (`data/raven-glass.css`), plus the classes only EagleEye
//! draws. Accent and light/dark come from desktop.toml.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
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

/// EagleEye's own light overrides, laid over `raven-glass-light.css`: the
/// pill and the canvas are drawn for a dark ground, so dark text on the dark
/// pill would vanish in Light.
const LIGHT_CSS: &str = r#"
.canvas { background-color: alpha(#000000, 0.04); }
.osd-bar {
  background-color: alpha(#ffffff, 0.82);
  border-color: alpha(#000000, 0.08);
  box-shadow: 0 10px 30px alpha(#000000, 0.18), inset 0 1px 0 alpha(#ffffff, 0.60);
}
"#;

/// How long desktop.toml has to be quiet before it is re-read: one save is
/// a burst of events (create, write, rename).
const DESKTOP_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

thread_local! {
    static OVERLAY: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static DESKTOP_MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
}

/// Load the stylesheet once, apply the desktop's look, and follow it.
pub fn apply() {
    let display = gtk::gdk::Display::default().expect("no display");
    let base = gtk::CssProvider::new();
    base.load_from_string(BASE_CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &base,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    refresh();
    watch_desktop();
}

/// Re-read desktop.toml and apply it: light/dark, the accent, and glass on
/// the open windows. The overlay provider is replaced, never stacked, so
/// this can run on every change.
fn refresh() {
    let appearance = Desktop::load().appearance;
    adw::StyleManager::default().set_color_scheme(match appearance.theme_mode {
        ThemeMode::Dark => adw::ColorScheme::ForceDark,
        ThemeMode::Light => adw::ColorScheme::ForceLight,
        ThemeMode::Auto => adw::ColorScheme::PreferDark,
    });
    let accent = if is_hex(&appearance.accent) {
        appearance.accent.as_str()
    } else {
        DEFAULT_ACCENT
    };
    let light = appearance.theme_mode == ThemeMode::Light;
    let css = format!(
        "@define-color accent_bg_color {accent};\n@define-color accent_color {accent};\n{}{}",
        if light {
            include_str!("../data/raven-glass-light.css")
        } else {
            ""
        },
        if light { LIGHT_CSS } else { "" }
    );
    if let Some(display) = gtk::gdk::Display::default() {
        OVERLAY.with(|slot| {
            if let Some(old) = slot.borrow_mut().take() {
                gtk::style_context_remove_provider_for_display(&display, &old);
            }
            let overlay = gtk::CssProvider::new();
            overlay.load_from_string(&css);
            gtk::style_context_add_provider_for_display(
                &display,
                &overlay,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            *slot.borrow_mut() = Some(overlay);
        });
    }

    // Glass is the material of EagleEye's own windows; dialogs stay opaque.
    let toplevels = gtk::Window::toplevels();
    for i in 0..toplevels.n_items() {
        let Some(window) = toplevels.item(i).and_downcast::<gtk::Window>() else {
            continue;
        };
        if !window.has_css_class("raven") {
            continue;
        }
        if appearance.transparency && window.transient_for().is_none() {
            window.add_css_class("glass");
        } else {
            window.remove_css_class("glass");
        }
    }
}

/// Follow desktop.toml, so a change made in Raven Settings shows here at
/// once. The directory is watched rather than the file: the file may not
/// exist yet, and is replaced by renaming a new one over it.
fn watch_desktop() {
    let path = Desktop::path();
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let name = name.to_os_string();
    let Ok(monitor) = gio::File::for_path(dir)
        .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
    else {
        return;
    };
    let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    monitor.connect_changed(move |_, file, other, event| {
        if matches!(
            event,
            gio::FileMonitorEvent::AttributeChanged
                | gio::FileMonitorEvent::PreUnmount
                | gio::FileMonitorEvent::Unmounted
        ) {
            return;
        }
        let names_desktop = |f: Option<&gio::File>| {
            f.and_then(|f| f.basename())
                .is_some_and(|b| b.as_os_str() == name.as_os_str())
        };
        if !names_desktop(Some(file)) && !names_desktop(other) {
            return;
        }
        if let Some(id) = pending.borrow_mut().take() {
            id.remove();
        }
        let fired = pending.clone();
        let id = glib::timeout_add_local_once(DESKTOP_SETTLE, move || {
            fired.borrow_mut().take();
            refresh();
        });
        *pending.borrow_mut() = Some(id);
    });
    DESKTOP_MONITOR.with(|m| *m.borrow_mut() = Some(monitor));
}

/// Whether the desktop asked for translucent windows.
pub fn glass() -> bool {
    Desktop::load().appearance.transparency
}

fn is_hex(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}
