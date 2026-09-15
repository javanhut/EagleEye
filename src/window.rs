//! The window: a header bar, the image, and a glass pill of controls
//! floating over it. The arrow keys walk the folder the image came from;
//! the images either side are decoded ahead so stepping is instant.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gtk::{gdk, gio, glib};
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::canvas::Canvas;
use crate::loader::{self, Image, file_name};
use crate::theme;

pub const APP_ID: &str = "com.eagleeye.Raven";

/// Every type EagleEye claims in `set-default`. Keep in step with the
/// MimeType= line of the desktop entry.
pub const MIME_TYPES: &[&str] = &[
    "image/png",
    "image/apng",
    "image/jpeg",
    "image/pjpeg",
    "image/gif",
    "image/webp",
    "image/bmp",
    "image/x-bmp",
    "image/tiff",
    "image/x-icon",
    "image/vnd.microsoft.icon",
    "image/x-tga",
    "image/x-portable-anymap",
    "image/x-portable-bitmap",
    "image/x-portable-graymap",
    "image/x-portable-pixmap",
    "image/qoi",
    "image/x-qoi",
    "image/vnd.radiance",
    "image/x-exr",
    "image/x-dds",
    "image/svg+xml",
    "image/svg+xml-compressed",
    "image/avif",
    "image/heif",
    "image/heic",
    "image/jxl",
];

const OSD_HIDE_AFTER: Duration = Duration::from_millis(1800);
const SPINNER_AFTER: Duration = Duration::from_millis(150);

thread_local! {
    static WINDOWS: RefCell<Vec<Window>> = const { RefCell::new(Vec::new()) };
}

struct Inner {
    window: adw::ApplicationWindow,
    title: adw::WindowTitle,
    toolbar: adw::ToolbarView,
    menu: gtk::MenuButton,
    stack: gtk::Stack,
    canvas: Canvas,
    error: adw::StatusPage,
    spinner: gtk::Spinner,
    osd: gtk::Revealer,
    toasts: adw::ToastOverlay,
    files: RefCell<Vec<PathBuf>>,
    index: Cell<usize>,
    /// Decoded images for the current file and its two neighbours.
    cache: RefCell<HashMap<PathBuf, Image>>,
    pending: RefCell<HashSet<PathBuf>>,
    /// What decode threads should still bother with; a thread whose file
    /// scrolled out of reach before it started skips the work.
    wanted: Arc<Mutex<Vec<PathBuf>>>,
    hide_osd: RefCell<Option<glib::SourceId>>,
}

#[derive(Clone)]
pub struct Window(Rc<Inner>);

impl Window {
    pub fn new(app: &adw::Application) -> Window {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("EagleEye")
            .default_width(1100)
            .default_height(760)
            .build();
        window.add_css_class("raven");
        if theme::glass() {
            window.add_css_class("glass");
        }

        // ── Header ──────────────────────────────────────────────────────
        let title = adw::WindowTitle::new("EagleEye", "");
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        header.pack_start(&icon_button(
            "document-open-symbolic",
            "Open… (Ctrl+O)",
            "win.open",
            false,
        ));

        let file_section = gio::Menu::new();
        file_section.append(Some("Open…"), Some("win.open"));
        file_section.append(Some("Show in Folder"), Some("win.show-in-folder"));
        file_section.append(Some("Copy Image"), Some("win.copy"));
        file_section.append(Some("Move to Trash"), Some("win.trash"));
        let app_section = gio::Menu::new();
        app_section.append(
            Some("Make EagleEye the Default Viewer"),
            Some("win.set-default"),
        );
        app_section.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
        app_section.append(Some("About EagleEye"), Some("win.about"));
        let model = gio::Menu::new();
        model.append_section(None, &file_section);
        model.append_section(None, &app_section);
        let menu = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&model)
            .tooltip_text("Menu")
            .build();
        header.pack_end(&menu);
        header.pack_end(&icon_button(
            "view-fullscreen-symbolic",
            "Fullscreen (F)",
            "win.fullscreen",
            false,
        ));

        // ── Content ─────────────────────────────────────────────────────
        let open_button = gtk::Button::builder()
            .label("Open Image…")
            .halign(gtk::Align::Center)
            .css_classes(["suggested-action", "pill"])
            .action_name("win.open")
            .build();
        let welcome = adw::StatusPage::builder()
            .icon_name("image-x-generic-symbolic")
            .title("EagleEye")
            .description("Open an image, or drop one here.")
            .child(&open_button)
            .css_classes(["welcome"])
            .build();
        let error = adw::StatusPage::builder()
            .icon_name("image-missing-symbolic")
            .title("Can’t Open This Image")
            .build();
        let canvas = Canvas::new();

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(120)
            .css_classes(["canvas"])
            .build();
        stack.add_named(&welcome, Some("welcome"));
        stack.add_named(&canvas, Some("image"));
        stack.add_named(&error, Some("error"));

        // ── The floating pill ───────────────────────────────────────────
        let zoom_label = gtk::Button::builder()
            .label("100%")
            .tooltip_text("Fit to Window (0)")
            .action_name("win.fit")
            .css_classes(["flat", "zoom-label"])
            .build();
        let bar = gtk::Box::builder()
            .spacing(2)
            .css_classes(["osd-bar"])
            .build();
        bar.append(&icon_button(
            "go-previous-symbolic",
            "Previous (←)",
            "win.prev",
            true,
        ));
        bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        bar.append(&icon_button(
            "zoom-out-symbolic",
            "Zoom Out (−)",
            "win.zoom-out",
            true,
        ));
        bar.append(&zoom_label);
        bar.append(&icon_button(
            "zoom-in-symbolic",
            "Zoom In (+)",
            "win.zoom-in",
            true,
        ));
        bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        bar.append(&icon_button(
            "object-rotate-left-symbolic",
            "Rotate Left (Shift+R)",
            "win.rotate-left",
            true,
        ));
        bar.append(&icon_button(
            "object-rotate-right-symbolic",
            "Rotate Right (R)",
            "win.rotate-right",
            true,
        ));
        bar.append(&icon_button(
            "object-flip-horizontal-symbolic",
            "Flip (H)",
            "win.flip",
            true,
        ));
        bar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        bar.append(&icon_button(
            "go-next-symbolic",
            "Next (→)",
            "win.next",
            true,
        ));
        let osd = gtk::Revealer::builder()
            .child(&bar)
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .build();

        let spinner = gtk::Spinner::builder()
            .width_request(32)
            .height_request(32)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .can_target(false)
            .visible(false)
            .build();

        let overlay = gtk::Overlay::builder().child(&stack).build();
        overlay.add_overlay(&osd);
        overlay.add_overlay(&spinner);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&overlay));
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&toasts));
        window.set_content(Some(&toolbar));

        let label = zoom_label.clone();
        canvas
            .connect_zoom_changed(move |scale| label.set_label(&format!("{:.0}%", scale * 100.0)));

        let this = Window(Rc::new(Inner {
            window,
            title,
            toolbar,
            menu,
            stack,
            canvas,
            error,
            spinner,
            osd,
            toasts,
            files: RefCell::default(),
            index: Cell::new(0),
            cache: RefCell::default(),
            pending: RefCell::default(),
            wanted: Arc::default(),
            hide_osd: RefCell::default(),
        }));
        this.install_actions();
        this.install_keys();
        this.install_drop();
        this.install_osd_autohide(&overlay);
        this.connect_signals();
        WINDOWS.with(|w| w.borrow_mut().push(this.clone()));
        this
    }

    /// The window a newly opened file should go to: the front one, or a new
    /// one if EagleEye has none yet.
    pub fn for_app(app: &adw::Application) -> Window {
        let active = app.active_window();
        WINDOWS
            .with(|w| {
                w.borrow()
                    .iter()
                    .find(|w| active.as_ref() == Some(w.0.window.upcast_ref()))
                    .cloned()
            })
            .unwrap_or_else(|| Window::new(app))
    }

    pub fn present(&self) {
        self.0.window.present();
    }

    /// Show `file` and make its folder the set the arrow keys walk. A
    /// folder opens at its first image.
    pub fn open(&self, file: gio::File) {
        let Some(path) = file.path() else {
            self.toast("EagleEye opens local files only");
            return;
        };
        let files = loader::siblings(&path);
        if files.is_empty() {
            self.clear();
            self.show_error(&path, "There are no images here.");
            return;
        }
        let path = std::path::absolute(&path).unwrap_or(path);
        let index = files.iter().position(|p| *p == path).unwrap_or(0);
        *self.0.files.borrow_mut() = files;
        self.show(index);
    }

    fn current_path(&self) -> Option<PathBuf> {
        self.0.files.borrow().get(self.0.index.get()).cloned()
    }

    fn step(&self, delta: isize) {
        let len = self.0.files.borrow().len();
        if len > 1 {
            self.show((self.0.index.get() as isize + delta).rem_euclid(len as isize) as usize);
        }
    }

    fn show(&self, index: usize) {
        let (path, neighbours) = {
            let files = self.0.files.borrow();
            let Some(path) = files.get(index).cloned() else {
                return;
            };
            let len = files.len();
            let mut near = vec![
                files[(index + 1) % len].clone(),
                files[(index + len - 1) % len].clone(),
            ];
            near.dedup();
            near.retain(|p| *p != path);
            (path, near)
        };
        self.0.index.set(index);

        let mut keep = neighbours.clone();
        keep.push(path.clone());
        self.0.cache.borrow_mut().retain(|p, _| keep.contains(p));
        *self.0.wanted.lock().unwrap() = keep;

        let name = file_name(&path);
        self.0.title.set_title(&name);
        self.0.window.set_title(Some(&format!("{name} — EagleEye")));
        self.sync_osd();

        let cached = self.0.cache.borrow().get(&path).cloned();
        match cached {
            Some(image) => self.display(&path, image),
            None => {
                self.set_subtitle(&path, None);
                self.decode(path.clone());
                self.spinner_later(path);
            }
        }
        for neighbour in neighbours {
            self.decode(neighbour);
        }
    }

    fn decode(&self, path: PathBuf) {
        if self.0.cache.borrow().contains_key(&path)
            || !self.0.pending.borrow_mut().insert(path.clone())
        {
            return;
        }
        let (tx, rx) = async_channel::bounded(1);
        let wanted = self.0.wanted.clone();
        let job = path.clone();
        std::thread::spawn(move || {
            let result = wanted
                .lock()
                .unwrap()
                .contains(&job)
                .then(|| loader::load(&job));
            let _ = tx.send_blocking(result);
        });
        let this = self.clone();
        glib::spawn_future_local(async move {
            let Ok(result) = rx.recv().await else { return };
            this.0.pending.borrow_mut().remove(&path);
            this.decoded(path, result);
        });
    }

    /// `None` means the thread skipped a file nobody wanted any more.
    fn decoded(&self, path: PathBuf, result: Option<anyhow::Result<Image>>) {
        let is_current = self.current_path().as_ref() == Some(&path);
        match result {
            Some(Ok(image)) => {
                if is_current {
                    self.display(&path, image.clone());
                }
                if self.0.wanted.lock().unwrap().contains(&path) {
                    self.0.cache.borrow_mut().insert(path, image);
                }
            }
            Some(Err(err)) if is_current => self.show_error(&path, &format!("{err:#}")),
            // Skipped, but the person came back to it in the meantime.
            None if is_current => self.decode(path),
            _ => {}
        }
    }

    fn spinner_later(&self, path: PathBuf) {
        let this = self.clone();
        glib::timeout_add_local_once(SPINNER_AFTER, move || {
            if this.current_path() == Some(path.clone()) && this.0.pending.borrow().contains(&path)
            {
                this.0.spinner.set_visible(true);
                this.0.spinner.start();
            }
        });
    }

    fn display(&self, path: &Path, image: Image) {
        self.stop_spinner();
        self.set_subtitle(path, Some(&image));
        self.0.canvas.set_image(Some(image));
        self.0.stack.set_visible_child_name("image");
    }

    fn show_error(&self, path: &Path, message: &str) {
        self.stop_spinner();
        self.set_subtitle(path, None);
        self.0.canvas.set_image(None);
        self.0
            .error
            .set_description(Some(&glib::markup_escape_text(message)));
        self.0.stack.set_visible_child_name("error");
    }

    fn clear(&self) {
        self.0.files.borrow_mut().clear();
        self.0.cache.borrow_mut().clear();
        self.0.wanted.lock().unwrap().clear();
        self.0.canvas.set_image(None);
        self.0.title.set_title("EagleEye");
        self.0.title.set_subtitle("");
        self.0.window.set_title(Some("EagleEye"));
        self.0.stack.set_visible_child_name("welcome");
        self.sync_osd();
    }

    fn stop_spinner(&self) {
        self.0.spinner.stop();
        self.0.spinner.set_visible(false);
    }

    /// "3 of 12 · PNG · 1672 × 941 · 1.2 MB"
    fn set_subtitle(&self, path: &Path, image: Option<&Image>) {
        let mut parts = Vec::new();
        let len = self.0.files.borrow().len();
        if len > 1 {
            parts.push(format!("{} of {len}", self.0.index.get() + 1));
        }
        match image {
            Some(image) => {
                let animated = if image.frames.len() > 1 {
                    " (animated)"
                } else {
                    ""
                };
                parts.push(format!("{}{animated}", image.format));
                parts.push(format!("{} × {}", image.width, image.height));
                parts.push(human_size(image.file_size));
            }
            None => {
                if let Ok(meta) = std::fs::metadata(path) {
                    parts.push(human_size(meta.len()));
                }
            }
        }
        self.0.title.set_subtitle(&parts.join(" · "));
    }

    fn toast(&self, message: &str) {
        self.0.toasts.add_toast(adw::Toast::new(message));
    }

    // ── The pill: always there in a window, on mouse movement in fullscreen.
    fn sync_osd(&self) {
        let has_files = !self.0.files.borrow().is_empty();
        self.0
            .osd
            .set_reveal_child(has_files && !self.0.window.is_fullscreen());
    }

    fn install_osd_autohide(&self, overlay: &gtk::Overlay) {
        let motion = gtk::EventControllerMotion::new();
        let this = self.clone();
        motion.connect_motion(move |_, _, _| {
            if !this.0.window.is_fullscreen() || this.0.files.borrow().is_empty() {
                return;
            }
            this.0.osd.set_reveal_child(true);
            if let Some(id) = this.0.hide_osd.take() {
                id.remove();
            }
            let later = this.clone();
            let id = glib::timeout_add_local_once(OSD_HIDE_AFTER, move || {
                later.0.hide_osd.take();
                if later.0.window.is_fullscreen() {
                    later.0.osd.set_reveal_child(false);
                }
            });
            this.0.hide_osd.replace(Some(id));
        });
        overlay.add_controller(motion);
    }

    fn connect_signals(&self) {
        let this = self.clone();
        self.0.window.connect_fullscreened_notify(move |w| {
            let full = w.is_fullscreen();
            this.0.toolbar.set_reveal_top_bars(!full);
            if full {
                w.add_css_class("fullscreen");
            } else {
                w.remove_css_class("fullscreen");
            }
            this.sync_osd();
        });

        let this = self.clone();
        self.0.window.connect_close_request(move |_| {
            if let Some(id) = this.0.hide_osd.take() {
                id.remove();
            }
            WINDOWS.with(|w| w.borrow_mut().retain(|w| !Rc::ptr_eq(&w.0, &this.0)));
            glib::Propagation::Proceed
        });
    }

    fn install_actions(&self) {
        let win = &self.0.window;
        let add = |name: &str, f: Box<dyn Fn(&Window)>| {
            let action = gio::SimpleAction::new(name, None);
            let this = self.clone();
            action.connect_activate(move |_, _| f(&this));
            win.add_action(&action);
        };

        add("open", Box::new(|w| w.choose_file()));
        add("prev", Box::new(|w| w.step(-1)));
        add("next", Box::new(|w| w.step(1)));
        add("zoom-in", Box::new(|w| w.0.canvas.zoom_by(1.25, None)));
        add("zoom-out", Box::new(|w| w.0.canvas.zoom_by(0.8, None)));
        add("fit", Box::new(|w| w.0.canvas.fit()));
        add("actual-size", Box::new(|w| w.0.canvas.zoom_to(1.0, None)));
        add("rotate-left", Box::new(|w| w.0.canvas.rotate(-1)));
        add("rotate-right", Box::new(|w| w.0.canvas.rotate(1)));
        add("flip", Box::new(|w| w.0.canvas.flip()));
        add(
            "fullscreen",
            Box::new(|w| {
                if w.0.window.is_fullscreen() {
                    w.0.window.unfullscreen();
                } else {
                    w.0.window.fullscreen();
                }
            }),
        );
        add(
            "copy",
            Box::new(|w| {
                if let Some(texture) = w.0.canvas.texture() {
                    w.0.window.clipboard().set_texture(&texture);
                    w.toast("Image copied");
                }
            }),
        );
        add("trash", Box::new(|w| w.trash()));
        add("show-in-folder", Box::new(|w| w.show_in_folder()));
        add(
            "set-default",
            Box::new(|w| {
                w.toast(match set_default_for_images() {
                    Ok(()) => "EagleEye now opens your images",
                    Err(_) => "Couldn’t set the default (is xdg-mime installed?)",
                });
            }),
        );
        add("shortcuts", Box::new(|w| w.show_shortcuts()));
        add(
            "about",
            Box::new(|w| {
                adw::AboutDialog::builder()
                    .application_name("EagleEye")
                    .application_icon(APP_ID)
                    .version(env!("CARGO_PKG_VERSION"))
                    .comments("Every kind of image, on Raven Linux.")
                    .license_type(gtk::License::MitX11)
                    .build()
                    .present(Some(&w.0.window));
            }),
        );
        add("close", Box::new(|w| w.0.window.close()));

        let app = win.application().expect("window has an application");
        for (action, keys) in [
            ("win.open", &["<Ctrl>o"][..]),
            (
                "win.zoom-in",
                &["plus", "equal", "KP_Add", "<Ctrl>plus", "<Ctrl>equal"],
            ),
            ("win.zoom-out", &["minus", "KP_Subtract", "<Ctrl>minus"]),
            ("win.fit", &["0", "KP_0", "<Ctrl>0"]),
            ("win.actual-size", &["1", "KP_1"]),
            ("win.rotate-right", &["r"]),
            ("win.rotate-left", &["<Shift>r"]),
            ("win.flip", &["h"]),
            ("win.fullscreen", &["f", "F11"]),
            ("win.copy", &["<Ctrl>c"]),
            ("win.trash", &["Delete"]),
            ("win.shortcuts", &["<Ctrl>question"]),
            ("win.close", &["<Ctrl>w"]),
        ] {
            app.set_accels_for_action(action, keys);
        }
    }

    /// Navigation keys are caught before the focused button sees them, so
    /// Space and the arrows move through images instead of focus.
    fn install_keys(&self) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let this = self.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            let window = &this.0.window;
            if window.visible_dialog().is_some()
                || this.0.menu.is_active()
                || state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK)
            {
                return glib::Propagation::Proceed;
            }
            match key {
                gdk::Key::Left | gdk::Key::Page_Up | gdk::Key::BackSpace => this.step(-1),
                gdk::Key::Right | gdk::Key::Page_Down | gdk::Key::space => this.step(1),
                gdk::Key::Home if !this.0.files.borrow().is_empty() => this.show(0),
                gdk::Key::End if !this.0.files.borrow().is_empty() => {
                    let last = this.0.files.borrow().len() - 1;
                    this.show(last);
                }
                gdk::Key::Escape if window.is_fullscreen() => window.unfullscreen(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.0.window.add_controller(keys);
    }

    fn install_drop(&self) {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let this = self.clone();
        target.connect_drop(move |_, value, _, _| {
            match value
                .get::<gdk::FileList>()
                .ok()
                .and_then(|l| l.files().into_iter().next())
            {
                Some(file) => {
                    this.open(file);
                    true
                }
                None => false,
            }
        });
        self.0.window.add_controller(target);
    }

    fn choose_file(&self) {
        let images = gtk::FileFilter::new();
        images.set_name(Some("Images"));
        for mime in MIME_TYPES {
            images.add_mime_type(mime);
        }
        for ext in loader::EXTENSIONS {
            images.add_suffix(ext);
        }
        let everything = gtk::FileFilter::new();
        everything.set_name(Some("All Files"));
        everything.add_pattern("*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&images);
        filters.append(&everything);

        let dialog = gtk::FileDialog::builder()
            .title("Open Image")
            .filters(&filters)
            .modal(true)
            .build();
        if let Some(dir) = self
            .current_path()
            .and_then(|p| p.parent().map(gio::File::for_path))
        {
            dialog.set_initial_folder(Some(&dir));
        }
        let this = self.clone();
        dialog.open(Some(&self.0.window), gio::Cancellable::NONE, move |res| {
            if let Ok(file) = res {
                this.open(file);
            }
        });
    }

    fn trash(&self) {
        let Some(path) = self.current_path() else {
            return;
        };
        let file = gio::File::for_path(&path);
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Err(err) = file.trash_future(glib::Priority::DEFAULT).await {
                this.toast(&format!("Couldn’t move to Trash: {}", err.message()));
                return;
            }
            this.toast(&format!("Moved “{}” to Trash", file_name(&path)));
            let (removed_at, len) = {
                let mut files = this.0.files.borrow_mut();
                let at = files.iter().position(|p| *p == path);
                files.retain(|p| *p != path);
                (at, files.len())
            };
            this.0.cache.borrow_mut().remove(&path);
            if len == 0 {
                this.clear();
                return;
            }
            // Stay on the same spot in the folder: the next image slides in.
            let mut index = this.0.index.get();
            if removed_at.is_some_and(|at| at < index) {
                index -= 1;
            }
            this.show(index.min(len - 1));
        });
    }

    fn show_in_folder(&self) {
        let Some(path) = self.current_path() else {
            return;
        };
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&path)));
        let this = self.clone();
        launcher.open_containing_folder(Some(&self.0.window), gio::Cancellable::NONE, move |res| {
            if res.is_err() {
                this.toast("Couldn’t open the folder");
            }
        });
    }

    fn show_shortcuts(&self) {
        let rows = [
            ("Open", "Ctrl+O"),
            ("Next / previous image", "→ / ← · Space / Backspace"),
            ("First / last image", "Home / End"),
            ("Zoom in / out", "+ / − · scroll · pinch"),
            ("Fit to window / 100%", "0 / 1 · double-click"),
            ("Rotate right / left", "R / Shift+R"),
            ("Flip horizontally", "H"),
            ("Fullscreen", "F / F11 · Esc to leave"),
            ("Copy image", "Ctrl+C"),
            ("Move to Trash", "Delete"),
            ("Close window", "Ctrl+W"),
        ];
        let list = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            .build();
        for (what, keys) in rows {
            let row = adw::ActionRow::builder().title(what).build();
            row.add_suffix(
                &gtk::Label::builder()
                    .label(keys)
                    .css_classes(["kbd"])
                    .valign(gtk::Align::Center)
                    .build(),
            );
            list.append(&row);
        }
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        page.append(&list);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&page));
        adw::Dialog::builder()
            .title("Keyboard Shortcuts")
            .content_width(480)
            .child(&view)
            .build()
            .present(Some(&self.0.window));
    }
}

fn icon_button(icon: &str, tooltip: &str, action: &str, flat: bool) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .action_name(action)
        .build();
    if flat {
        button.add_css_class("flat");
    }
    button
}

/// Point every image type at EagleEye for the current user.
pub fn set_default_for_images() -> Result<(), &'static str> {
    let desktop = format!("{APP_ID}.desktop");
    let ok = std::process::Command::new("xdg-mime")
        .arg("default")
        .arg(&desktop)
        .args(MIME_TYPES)
        .status()
        .is_ok_and(|s| s.success());
    if ok { Ok(()) } else { Err("image files") }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}
