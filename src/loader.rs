//! Decoding. Runs on a worker thread and hands back GPU-ready textures.
//!
//! The bytes are sniffed, not the extension trusted: a PNG saved as `.jpg`
//! still opens. `image` decodes the raster formats, resvg draws SVG, and
//! anything neither knows (HEIC, AVIF, JPEG XL…) goes to GTK's own loaders.

use std::cmp::Ordering;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use image::codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder};
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader};

/// Extensions EagleEye lists when walking a folder and offers in Open.
pub const EXTENSIONS: &[&str] = &[
    "png", "apng", "jpg", "jpeg", "jpe", "jfif", "gif", "webp", "bmp", "dib", "tif", "tiff", "ico", "cur", "tga",
    "pbm", "pgm", "ppm", "pnm", "pam", "qoi", "hdr", "exr", "dds", "ff", "svg", "svgz", "avif", "heic", "heif", "jxl",
];

/// SVGs are rasterized so the long side is at least this many pixels, which
/// keeps them sharp when zoomed well past their intrinsic size.
const SVG_MIN_SIDE: f32 = 2048.0;
const SVG_MAX_SIDE: f32 = 8192.0;

#[derive(Clone)]
pub struct Frame {
    pub texture: gdk::Texture,
    pub delay: Duration,
}

#[derive(Clone)]
pub struct Image {
    /// One frame for a still image, several for GIF / WebP / APNG.
    pub frames: Vec<Frame>,
    /// Intrinsic size after EXIF orientation. For SVG this is the document
    /// size, not the size of the (larger) rasterized texture.
    pub width: u32,
    pub height: u32,
    pub format: String,
    /// Vector art may be fitted larger than its intrinsic size.
    pub scalable: bool,
    pub file_size: u64,
}

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

pub fn load(path: &Path) -> anyhow::Result<Image> {
    let data = fs::read(path).with_context(|| format!("couldn’t read {}", path.display()))?;
    let file_size = data.len() as u64;

    if is_svg(path, &data) {
        return svg(&data, file_size);
    }

    let format = image::guess_format(&data).ok().or_else(|| ImageFormat::from_path(path).ok());
    let decoded = match format {
        Some(format) => raster(&data, format).map_err(anyhow::Error::from),
        None => Err(anyhow::anyhow!("not an image EagleEye recognizes")),
    };
    match decoded {
        Ok(frames) => {
            let (width, height) = (frames[0].texture.width() as u32, frames[0].texture.height() as u32);
            let name = format.map_or_else(String::new, |f| f.extensions_str()[0].to_uppercase());
            Ok(Image { frames, width, height, format: name, scalable: false, file_size })
        }
        // HEIC, AVIF, JPEG XL and friends: GTK's glycin loaders know them.
        Err(err) => fallback(path, file_size).map_err(|_| err),
    }
}

fn raster(data: &[u8], format: ImageFormat) -> image::ImageResult<Vec<Frame>> {
    match format {
        ImageFormat::Gif => return animation(GifDecoder::new(Cursor::new(data))?.into_frames()),
        ImageFormat::Png => {
            let decoder = PngDecoder::new(Cursor::new(data))?;
            if decoder.is_apng()? {
                return animation(decoder.apng()?.into_frames());
            }
        }
        ImageFormat::WebP => {
            let decoder = WebPDecoder::new(Cursor::new(data))?;
            if decoder.has_animation() {
                return animation(decoder.into_frames());
            }
        }
        _ => {}
    }

    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    // The default 512 MiB cap refuses big panoramas; this is a viewer.
    reader.no_limits();
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(vec![Frame { texture: still(img), delay: Duration::ZERO }])
}

fn animation(frames: image::Frames<'_>) -> image::ImageResult<Vec<Frame>> {
    let mut out = Vec::new();
    for frame in frames {
        let frame = frame?;
        let (numer, denom) = frame.delay().numer_denom_ms();
        let ms = u64::from(numer) / u64::from(denom.max(1));
        // Browsers play 0–10 ms frames at 100 ms; files rely on it.
        let delay = Duration::from_millis(if ms <= 10 { 100 } else { ms });
        let buffer = frame.into_buffer();
        let (w, h) = buffer.dimensions();
        out.push(Frame { texture: texture(w, h, gdk::MemoryFormat::R8g8b8a8, buffer.into_raw(), 4), delay });
    }
    if out.is_empty() {
        return Err(image::ImageError::IoError(std::io::Error::other("animation has no frames")));
    }
    Ok(out)
}

fn still(img: DynamicImage) -> gdk::Texture {
    let (w, h) = (img.width(), img.height());
    match img {
        DynamicImage::ImageRgb8(rgb) => texture(w, h, gdk::MemoryFormat::R8g8b8, rgb.into_raw(), 3),
        other => texture(w, h, gdk::MemoryFormat::R8g8b8a8, other.into_rgba8().into_raw(), 4),
    }
}

fn texture(w: u32, h: u32, format: gdk::MemoryFormat, pixels: Vec<u8>, channels: usize) -> gdk::Texture {
    let bytes = glib::Bytes::from_owned(pixels);
    gdk::MemoryTexture::new(w as i32, h as i32, format, &bytes, w as usize * channels).upcast()
}

fn is_svg(path: &Path, data: &[u8]) -> bool {
    let by_name = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz")
    });
    by_name || String::from_utf8_lossy(&data[..data.len().min(1024)]).contains("<svg")
}

fn svg(data: &[u8], file_size: u64) -> anyhow::Result<Image> {
    use resvg::{tiny_skia, usvg};

    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(data, &options).context("not a valid SVG")?;
    let size = tree.size();
    let long = size.width().max(size.height()).max(1.0);
    let scale = (SVG_MIN_SIDE / long).max(1.0).min(SVG_MAX_SIDE / long);
    let (w, h) = ((size.width() * scale).ceil().max(1.0) as u32, (size.height() * scale).ceil().max(1.0) as u32);
    let mut pixmap = tiny_skia::Pixmap::new(w, h).context("SVG is too large to draw")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let frame = Frame {
        texture: texture(w, h, gdk::MemoryFormat::R8g8b8a8Premultiplied, pixmap.take(), 4),
        delay: Duration::ZERO,
    };
    Ok(Image {
        frames: vec![frame],
        width: size.width().round().max(1.0) as u32,
        height: size.height().round().max(1.0) as u32,
        format: "SVG".into(),
        scalable: true,
        file_size,
    })
}

fn fallback(path: &Path, file_size: u64) -> anyhow::Result<Image> {
    let texture = gdk::Texture::from_filename(path)?;
    let format = path.extension().and_then(|e| e.to_str()).unwrap_or("image").to_uppercase();
    Ok(Image {
        width: texture.width() as u32,
        height: texture.height() as u32,
        frames: vec![Frame { texture, delay: Duration::ZERO }],
        format,
        scalable: false,
        file_size,
    })
}

/// The images next to `path` (or inside it, for a folder), in the order a
/// person expects: `img2` before `img10`, case ignored.
pub fn siblings(path: &Path) -> Vec<PathBuf> {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = if path.is_dir() { path.clone() } else { path.parent().map(Path::to_path_buf).unwrap_or_default() };
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file() && is_image(p))
                .collect()
        })
        .unwrap_or_default();
    // A file opened by name is shown even if its extension is unusual.
    if path.is_file() && !files.contains(&path) {
        files.push(path);
    }
    files.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
    files
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let n: String = std::iter::from_fn(|| x.next_if(char::is_ascii_digit)).collect();
                let m: String = std::iter::from_fn(|| y.next_if(char::is_ascii_digit)).collect();
                let (n, m) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let ord = n.len().cmp(&m.len()).then_with(|| n.cmp(m));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(c), Some(d)) => {
                let ord = c.to_lowercase().cmp(d.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                x.next();
                y.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Delay, Rgb, RgbImage, Rgba, RgbaImage};

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-images");
        fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn png_decodes() {
        let path = scratch("still.png");
        RgbImage::from_pixel(7, 3, Rgb([1, 2, 3])).save(&path).unwrap();
        let img = load(&path).unwrap();
        assert_eq!((img.width, img.height), (7, 3));
        assert_eq!(img.frames.len(), 1);
        assert_eq!(img.format, "PNG");
    }

    #[test]
    fn content_wins_over_extension() {
        let path = scratch("really-a-png.jpg");
        RgbaImage::from_pixel(5, 5, Rgba([9, 9, 9, 128])).save_with_format(&path, ImageFormat::Png).unwrap();
        let img = load(&path).unwrap();
        assert_eq!(img.format, "PNG");
        assert_eq!((img.width, img.height), (5, 5));
    }

    #[test]
    fn animated_gif_keeps_every_frame() {
        use image::codecs::gif::GifEncoder;
        let path = scratch("anim.gif");
        {
            let mut encoder = GifEncoder::new(fs::File::create(&path).unwrap());
            let frames = (0..3u8).map(|i| {
                let buffer = RgbaImage::from_pixel(4, 4, Rgba([i * 80, 0, 0, 255]));
                image::Frame::from_parts(buffer, 0, 0, Delay::from_numer_denom_ms(50, 1))
            });
            encoder.encode_frames(frames).unwrap();
        }
        let img = load(&path).unwrap();
        assert_eq!(img.frames.len(), 3);
        assert_eq!(img.frames[1].delay, Duration::from_millis(50));
    }

    #[test]
    fn svg_keeps_intrinsic_size_and_renders_sharp() {
        let path = scratch("shape.svg");
        fs::write(
            &path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="red"/></svg>"#,
        )
        .unwrap();
        let img = load(&path).unwrap();
        assert_eq!((img.width, img.height), (40, 20));
        assert!(img.scalable);
        assert_eq!(img.frames[0].texture.width(), 2048);
    }

    #[test]
    fn garbage_is_an_error() {
        let path = scratch("junk.png");
        fs::write(&path, b"definitely not an image").unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn natural_order() {
        let mut names = vec!["img10.png", "img2.png", "IMG1.png", "img02b.png"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["IMG1.png", "img2.png", "img02b.png", "img10.png"]);
    }

    #[test]
    fn siblings_lists_only_images() {
        let dir = scratch("folder");
        fs::create_dir_all(&dir).unwrap();
        for name in ["b10.png", "b9.JPG", "notes.txt"] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let names: Vec<String> = siblings(&dir.join("b10.png")).iter().map(|p| file_name(p)).collect();
        assert_eq!(names, ["b9.JPG", "b10.png"]);
    }
}
