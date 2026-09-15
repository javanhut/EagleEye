# EagleEye

A fast image viewer for Raven Linux. Open any image and look at it — no
hunting for which viewer handles which format.

## What it does

| | |
|---|---|
| **Formats** | PNG, APNG, JPEG, GIF, WebP, BMP, TIFF, ICO, TGA, PNM, QOI, HDR, EXR, DDS, farbfeld, SVG — decoded in pure Rust ([image](https://github.com/image-rs/image), [resvg](https://github.com/linebender/resvg)); HEIC, AVIF and JPEG XL through GTK's own loaders |
| **Sniffing** | The bytes decide, not the extension: a PNG saved as `.jpg` still opens |
| **Animation** | Animated GIF, WebP and APNG play |
| **Folder** | Opens the folder the image is in; `←` / `→` step through it in natural order (`img2` before `img10`), neighbours decoded ahead so stepping is instant |
| **Zoom** | Fits the window; wheel or pinch zooms around the pointer, drag pans, double-click toggles fit / 100%. 100% is one image pixel per screen pixel, whatever the desktop scale |
| **Orientation** | EXIF rotation applied; rotate and flip by hand |
| **Look** | GTK 4 + libadwaita with Raven Glass; theme, accent and transparency follow `~/.config/raven/desktop.toml` |

## Use

```sh
eagleeye photo.png            # open an image
eagleeye ~/Pictures           # open a folder at its first image
eagleeye                      # empty window: Open… or drag an image in
eagleeye set-default          # make it the default for every image type
```

A second `eagleeye FILE` hands the file to the running window.

### Keyboard

| Action | Keys |
|---|---|
| Next / previous | `→` / `←`, `Space` / `Backspace` |
| First / last | `Home` / `End` |
| Zoom in / out | `+` / `−`, scroll, pinch |
| Fit / 100% | `0` / `1`, double-click |
| Rotate right / left | `R` / `Shift+R` |
| Flip | `H` |
| Fullscreen | `F` / `F11`, `Esc` to leave |
| Copy image | `Ctrl+C` |
| Move to Trash | `Delete` |
| Open | `Ctrl+O` |
| Shortcuts | `Ctrl+?` |

## Build

```sh
make            # release build
make run FILE=photo.png
sudo make install && eagleeye set-default
```

or with ImLazy: `imlazy build`, `imlazy dev -- photo.png`, `imlazy install`.

Dependencies are always optimized, even in debug builds, so `cargo run` is
as smooth as a release build.

## Tests

```sh
cargo test
```
