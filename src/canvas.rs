//! The image surface. Draws the current frame fitted to the window or at a
//! chosen zoom; drag pans, the wheel and pinch zoom around the pointer,
//! double-click toggles fit / 100%, and animations play on a timer.
//!
//! Zoom is in device pixels: 100% puts one image pixel on one screen pixel,
//! whatever the desktop's fractional scale.

use std::cell::{Cell, RefCell};

use gtk4 as gtk;
use gtk::{gdk, glib, graphene, gsk, prelude::*, subclass::prelude::*};

use crate::loader::Image;

const MIN_ZOOM: f64 = 0.02;
const MAX_ZOOM: f64 = 40.0;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Canvas {
        pub image: RefCell<Option<Image>>,
        pub frame: Cell<usize>,
        pub timer: RefCell<Option<glib::SourceId>>,
        pub fit: Cell<bool>,
        pub zoom: Cell<f64>,
        /// Offset of the image centre from the viewport centre, logical px.
        pub pan: Cell<(f64, f64)>,
        pub turns: Cell<u8>,
        pub flipped: Cell<bool>,
        pub pointer: Cell<(f64, f64)>,
        pub drag_origin: Cell<(f64, f64)>,
        pub pinch_origin: Cell<f64>,
        pub last_zoom: Cell<f64>,
        pub on_zoom: RefCell<Option<Box<dyn Fn(f64)>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "EagleEyeCanvas";
        type Type = super::Canvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Canvas {
        fn constructed(&self) {
            self.parent_constructed();
            self.fit.set(true);
            self.zoom.set(1.0);
            self.last_zoom.set(-1.0);
            let obj = self.obj();
            obj.set_hexpand(true);
            obj.set_vexpand(true);
            obj.set_overflow(gtk::Overflow::Hidden);
            obj.install_gestures();
        }

        fn dispose(&self) {
            if let Some(id) = self.timer.take() {
                id.remove();
            }
        }
    }

    impl WidgetImpl for Canvas {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            let obj = self.obj();
            obj.clamp_pan();
            obj.notify_zoom();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().draw(snapshot);
        }
    }
}

glib::wrapper! {
    pub struct Canvas(ObjectSubclass<imp::Canvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Canvas {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Show `image` fitted and upright; `None` clears the surface.
    pub fn set_image(&self, image: Option<Image>) {
        let imp = self.imp();
        if let Some(id) = imp.timer.take() {
            id.remove();
        }
        let animated = image.as_ref().is_some_and(|i| i.frames.len() > 1);
        imp.image.replace(image);
        imp.frame.set(0);
        imp.turns.set(0);
        imp.flipped.set(false);
        imp.fit.set(true);
        imp.zoom.set(1.0);
        imp.pan.set((0.0, 0.0));
        if animated {
            self.schedule_frame();
        }
        self.queue_draw();
        self.notify_zoom();
    }

    /// The frame on screen, for copying to the clipboard.
    pub fn texture(&self) -> Option<gdk::Texture> {
        let imp = self.imp();
        let image = imp.image.borrow();
        image.as_ref().and_then(|i| i.frames.get(imp.frame.get())).map(|f| f.texture.clone())
    }

    /// Called with the zoom (1.0 = 100%) whenever it changes.
    pub fn connect_zoom_changed(&self, f: impl Fn(f64) + 'static) {
        self.imp().on_zoom.replace(Some(Box::new(f)));
    }

    pub fn fit(&self) {
        let imp = self.imp();
        imp.fit.set(true);
        imp.pan.set((0.0, 0.0));
        self.queue_draw();
        self.notify_zoom();
    }

    pub fn zoom_by(&self, factor: f64, anchor: Option<(f64, f64)>) {
        self.zoom_to(self.scale() * factor, anchor);
    }

    /// Zoom so the image point under `anchor` (widget coordinates; the
    /// centre when `None`) stays where it is. Zooming out past "fit" snaps
    /// back to fit, so the image never shrinks to a speck by accident.
    pub fn zoom_to(&self, zoom: f64, anchor: Option<(f64, f64)>) {
        let imp = self.imp();
        if imp.image.borrow().is_none() {
            return;
        }
        let old = self.scale();
        let new = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if new < self.fit_scale() && new < old {
            self.fit();
            return;
        }
        let (vw, vh) = (self.width() as f64, self.height() as f64);
        let (ax, ay) = anchor.map_or((0.0, 0.0), |(x, y)| (x - vw / 2.0, y - vh / 2.0));
        let (px, py) = imp.pan.get();
        let k = new / old;
        imp.pan.set((ax - (ax - px) * k, ay - (ay - py) * k));
        imp.fit.set(false);
        imp.zoom.set(new);
        self.clamp_pan();
        self.queue_draw();
        self.notify_zoom();
    }

    pub fn rotate(&self, quarter_turns: i8) {
        let imp = self.imp();
        imp.turns.set((imp.turns.get() as i8 + quarter_turns).rem_euclid(4) as u8);
        imp.pan.set((0.0, 0.0));
        self.clamp_pan();
        self.queue_draw();
        self.notify_zoom();
    }

    pub fn flip(&self) {
        let imp = self.imp();
        imp.flipped.set(!imp.flipped.get());
        self.queue_draw();
    }

    /// Zoom in device pixels per image pixel.
    fn scale(&self) -> f64 {
        if self.imp().fit.get() { self.fit_scale() } else { self.imp().zoom.get() }
    }

    fn fit_scale(&self) -> f64 {
        let imp = self.imp();
        let image = imp.image.borrow();
        let Some(image) = image.as_ref() else { return 1.0 };
        let (vw, vh) = (self.width() as f64, self.height() as f64);
        if vw <= 0.0 || vh <= 0.0 {
            return 1.0;
        }
        let (w, h) = self.rotated(image.width as f64, image.height as f64);
        let s = (vw / w).min(vh / h) * self.device_scale();
        // Photos are never blown up past 100% to fill the window; SVGs are.
        if image.scalable { s } else { s.min(1.0) }
    }

    fn device_scale(&self) -> f64 {
        self.native().and_then(|n| n.surface()).map_or(1.0, |s| s.scale()).max(0.1)
    }

    fn rotated(&self, w: f64, h: f64) -> (f64, f64) {
        if self.imp().turns.get() % 2 == 1 { (h, w) } else { (w, h) }
    }

    /// The image's on-screen size in logical pixels, rotation applied.
    fn display_size(&self) -> Option<(f64, f64)> {
        let image = self.imp().image.borrow();
        let image = image.as_ref()?;
        let s = self.scale() / self.device_scale();
        Some(self.rotated(image.width as f64 * s, image.height as f64 * s))
    }

    fn clamp_pan(&self) {
        let imp = self.imp();
        let Some((w, h)) = self.display_size() else { return };
        let over_x = ((w - self.width() as f64) / 2.0).max(0.0);
        let over_y = ((h - self.height() as f64) / 2.0).max(0.0);
        let (px, py) = imp.pan.get();
        imp.pan.set((px.clamp(-over_x, over_x), py.clamp(-over_y, over_y)));
    }

    fn pan_by(&self, dx: f64, dy: f64) {
        let (px, py) = self.imp().pan.get();
        self.imp().pan.set((px + dx, py + dy));
        self.clamp_pan();
        self.queue_draw();
    }

    fn pannable(&self) -> bool {
        self.display_size().is_some_and(|(w, h)| w > self.width() as f64 || h > self.height() as f64)
    }

    /// Report the zoom on idle: this runs inside size_allocate, where
    /// touching another widget's label would queue a resize mid-layout.
    fn notify_zoom(&self) {
        let imp = self.imp();
        let s = self.scale();
        if (imp.last_zoom.get() - s).abs() < 1e-9 {
            return;
        }
        imp.last_zoom.set(s);
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(canvas) = weak.upgrade()
                && let Some(f) = canvas.imp().on_zoom.borrow().as_ref()
            {
                f(canvas.scale());
            }
        });
    }

    fn schedule_frame(&self) {
        let imp = self.imp();
        let delay = {
            let image = imp.image.borrow();
            let Some(image) = image.as_ref() else { return };
            image.frames[imp.frame.get()].delay
        };
        let weak = self.downgrade();
        let id = glib::timeout_add_local_once(delay, move || {
            let Some(canvas) = weak.upgrade() else { return };
            let imp = canvas.imp();
            // This source has fired; forget it so set_image won't remove it.
            imp.timer.take();
            let count = imp.image.borrow().as_ref().map_or(1, |i| i.frames.len());
            imp.frame.set((imp.frame.get() + 1) % count);
            canvas.queue_draw();
            canvas.schedule_frame();
        });
        imp.timer.replace(Some(id));
    }

    fn draw(&self, snapshot: &gtk::Snapshot) {
        let imp = self.imp();
        let image = imp.image.borrow();
        let Some(image) = image.as_ref() else { return };
        let Some(frame) = image.frames.get(imp.frame.get()) else { return };

        let ds = self.device_scale();
        let scale = self.scale();
        let (w, h) = (image.width as f64 * scale / ds, image.height as f64 * scale / ds);
        let (rw, rh) = self.rotated(w, h);
        let (px, py) = imp.pan.get();
        let (vw, vh) = (self.width() as f64, self.height() as f64);
        // Snap the top-left corner to a device pixel so 100% is pixel-exact.
        let snap = |v: f64| (v * ds).round() / ds;
        let cx = snap((vw - rw) / 2.0 + px) + rw / 2.0;
        let cy = snap((vh - rh) / 2.0 + py) + rh / 2.0;

        let filter = if scale >= 3.0 {
            gsk::ScalingFilter::Nearest
        } else if scale < 1.0 {
            gsk::ScalingFilter::Trilinear
        } else {
            gsk::ScalingFilter::Linear
        };

        snapshot.save();
        snapshot.translate(&graphene::Point::new(cx as f32, cy as f32));
        if imp.flipped.get() {
            snapshot.scale(-1.0, 1.0);
        }
        snapshot.rotate(90.0 * imp.turns.get() as f32);
        let rect = graphene::Rect::new((-w / 2.0) as f32, (-h / 2.0) as f32, w as f32, h as f32);
        snapshot.append_scaled_texture(&frame.texture, filter, &rect);
        snapshot.restore();
    }

    fn install_gestures(&self) {
        let motion = gtk::EventControllerMotion::new();
        let weak = self.downgrade();
        motion.connect_motion(move |_, x, y| {
            if let Some(canvas) = weak.upgrade() {
                canvas.imp().pointer.set((x, y));
            }
        });
        self.add_controller(motion);

        let drag = gtk::GestureDrag::new();
        let weak = self.downgrade();
        drag.connect_drag_begin(move |_, _, _| {
            let Some(canvas) = weak.upgrade() else { return };
            canvas.imp().drag_origin.set(canvas.imp().pan.get());
            if canvas.pannable() {
                canvas.set_cursor_from_name(Some("grabbing"));
            }
        });
        let weak = self.downgrade();
        drag.connect_drag_update(move |_, dx, dy| {
            let Some(canvas) = weak.upgrade() else { return };
            let (ox, oy) = canvas.imp().drag_origin.get();
            canvas.imp().pan.set((ox + dx, oy + dy));
            canvas.clamp_pan();
            canvas.queue_draw();
        });
        let weak = self.downgrade();
        drag.connect_drag_end(move |_, _, _| {
            if let Some(canvas) = weak.upgrade() {
                canvas.set_cursor_from_name(None);
            }
        });
        self.add_controller(drag);

        let click = gtk::GestureClick::new();
        let weak = self.downgrade();
        click.connect_pressed(move |_, presses, x, y| {
            let Some(canvas) = weak.upgrade() else { return };
            if presses == 2 {
                if canvas.imp().fit.get() || (canvas.scale() - 1.0).abs() > 1e-6 {
                    canvas.zoom_to(1.0, Some((x, y)));
                } else {
                    canvas.fit();
                }
            }
        });
        self.add_controller(click);

        // A mouse wheel zooms; a touchpad pans, and zooms with Ctrl held.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        let weak = self.downgrade();
        scroll.connect_scroll(move |controller, dx, dy| {
            let Some(canvas) = weak.upgrade() else { return glib::Propagation::Proceed };
            let anchor = Some(canvas.imp().pointer.get());
            let ctrl = controller.current_event_state().contains(gdk::ModifierType::CONTROL_MASK);
            if controller.unit() == gdk::ScrollUnit::Wheel {
                if dy != 0.0 {
                    canvas.zoom_by(1.2f64.powf(-dy), anchor);
                }
            } else if ctrl {
                canvas.zoom_by((-dy * 0.01).exp(), anchor);
            } else {
                canvas.pan_by(-dx, -dy);
            }
            glib::Propagation::Stop
        });
        self.add_controller(scroll);

        let pinch = gtk::GestureZoom::new();
        let weak = self.downgrade();
        pinch.connect_begin(move |_, _| {
            if let Some(canvas) = weak.upgrade() {
                canvas.imp().pinch_origin.set(canvas.scale());
            }
        });
        let weak = self.downgrade();
        pinch.connect_scale_changed(move |gesture, factor| {
            if let Some(canvas) = weak.upgrade() {
                canvas.zoom_to(canvas.imp().pinch_origin.get() * factor, gesture.bounding_box_center());
            }
        });
        self.add_controller(pinch);
    }
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}
