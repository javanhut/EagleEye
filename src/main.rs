//! EagleEye. `eagleeye [open] FILE…` opens images; a second invocation hands
//! its files to the running instance instead of starting another process, so
//! opening from the file manager is instant.

mod canvas;
mod config;
mod loader;
mod resample;
mod theme;
mod window;

use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;

use window::{APP_ID, MIME_TYPES, Window};

const USAGE: &str = "\
EagleEye — view images

Usage:
  eagleeye [FILE…]          open an image (or an empty window)
  eagleeye open FILE…       same as above
  eagleeye set-default      make EagleEye the default for every image type
  eagleeye --help | --version
";

fn main() -> glib::ExitCode {
    let mut args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            return glib::ExitCode::SUCCESS;
        }
        Some("-V" | "--version") => {
            println!("eagleeye {}", env!("CARGO_PKG_VERSION"));
            return glib::ExitCode::SUCCESS;
        }
        Some("set-default") => return set_default(),
        // `open` is sugar; GApplication takes the files as plain arguments.
        Some("open") => {
            args.remove(1);
        }
        _ => {}
    }

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_startup(|_| theme::apply());
    app.connect_activate(|app| {
        if let Some(win) = app.active_window() {
            win.present();
        } else {
            Window::new(app).present();
        }
    });
    // One viewer: a file opened while EagleEye is running replaces what the
    // front window shows, the way a photo viewer is expected to behave.
    app.connect_open(|app, files, _hint| {
        let Some(file) = files.first() else { return };
        let win = Window::for_app(app);
        win.open(file.clone());
        win.present();
    });
    app.run_with_args(&args)
}

fn set_default() -> glib::ExitCode {
    match window::set_default_for_images() {
        Ok(()) => {
            println!("{} image types → {APP_ID}.desktop", MIME_TYPES.len());
            glib::ExitCode::SUCCESS
        }
        Err(mime) => {
            eprintln!("eagleeye: could not set the default for {mime} (is xdg-mime installed?)");
            glib::ExitCode::FAILURE
        }
    }
}
