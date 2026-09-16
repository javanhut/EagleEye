//! High-quality downscaling. The GPU's trilinear filter blends in a
//! half-resolution mipmap and looks soft; a Lanczos pass on the CPU to the
//! exact size on screen keeps edges and fine texture crisp.

use fast_image_resize::{PixelType, ResizeOptions, Resizer, images::Image};
use gtk4::prelude::*;
use gtk4::{gdk, glib};

/// `source` resized to `width` × `height` with Lanczos3. Runs on a worker
/// thread; `None` only if the texture could not be read.
pub fn lanczos(source: &gdk::Texture, width: u32, height: u32) -> Option<gdk::Texture> {
    let pixels = rgba(source);
    resize(
        source.width() as u32,
        source.height() as u32,
        pixels,
        width,
        height,
    )
}

/// The texture's pixels as tightly packed straight-alpha RGBA.
fn rgba(source: &gdk::Texture) -> Vec<u8> {
    let row = source.width() as usize * 4;
    let mut downloader = gdk::TextureDownloader::new(source);
    downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    if stride == row {
        bytes.to_vec()
    } else {
        bytes
            .chunks(stride)
            .flat_map(|r| &r[..row])
            .copied()
            .collect()
    }
}

fn resize(sw: u32, sh: u32, pixels: Vec<u8>, width: u32, height: u32) -> Option<gdk::Texture> {
    let src = Image::from_vec_u8(sw, sh, pixels, PixelType::U8x4).ok()?;
    let mut dst = Image::new(width.max(1), height.max(1), PixelType::U8x4);
    // The defaults are Lanczos3 with alpha premultiplied around the filter,
    // so transparent edges don't pick up dark fringes.
    Resizer::new()
        .resize(&src, &mut dst, &ResizeOptions::new())
        .ok()?;

    let (w, h) = (dst.width(), dst.height());
    let bytes = glib::Bytes::from_owned(dst.into_vec());
    Some(
        gdk::MemoryTexture::new(
            w as i32,
            h as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            w as usize * 4,
        )
        .upcast(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture(w: u32, h: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> gdk::Texture {
        let pixels: Vec<u8> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .flat_map(|(x, y)| pixel(x, y))
            .collect();
        let bytes = glib::Bytes::from_owned(pixels);
        gdk::MemoryTexture::new(
            w as i32,
            h as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            w as usize * 4,
        )
        .upcast()
    }

    fn pixels(t: &gdk::Texture) -> Vec<u8> {
        let mut d = gdk::TextureDownloader::new(t);
        d.set_format(gdk::MemoryFormat::R8g8b8a8);
        d.download_bytes().0.to_vec()
    }

    #[test]
    fn resizes_to_the_exact_size() {
        let out = lanczos(&texture(100, 50, |_, _| [10, 20, 30, 255]), 40, 20).unwrap();
        assert_eq!((out.width(), out.height()), (40, 20));
        // A flat colour stays that colour.
        assert!(pixels(&out).chunks(4).all(|p| p == [10, 20, 30, 255]));
    }

    #[test]
    fn rgb_textures_are_read_correctly() {
        let bytes = glib::Bytes::from_owned([200u8, 100, 50].repeat(9 * 9));
        let rgb: gdk::Texture =
            gdk::MemoryTexture::new(9, 9, gdk::MemoryFormat::R8g8b8, &bytes, 27).upcast();
        let out = lanczos(&rgb, 3, 3).unwrap();
        assert!(pixels(&out).chunks(4).all(|p| p == [200, 100, 50, 255]));
    }

    /// Time a real downscale, step by step:
    /// `EAGLEEYE_IMAGE=photo.jpg cargo test lanczos_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lanczos_timing() {
        use std::time::Instant;
        let path = std::env::var("EAGLEEYE_IMAGE").expect("set EAGLEEYE_IMAGE");
        let image = crate::loader::load(std::path::Path::new(&path)).unwrap();
        let source = &image.frames[0].texture;
        let (sw, sh) = (source.width() as u32, source.height() as u32);

        let start = Instant::now();
        let pixels = rgba(source);
        let read = start.elapsed();
        let start = Instant::now();
        let out = resize(sw, sh, pixels, sw / 6, sh / 6).unwrap();
        let resized = start.elapsed();
        println!(
            "{sw}x{sh} → {}x{}: read pixels {read:?}, lanczos {resized:?}",
            out.width(),
            out.height()
        );
    }

    #[test]
    fn keeps_a_hard_edge_sharper_than_averaging() {
        // Left half black, right half white, halved in size: Lanczos keeps
        // the columns either side of the edge at (nearly) pure black/white.
        let edge = texture(64, 8, |x, _| {
            if x < 32 {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        let out = lanczos(&edge, 32, 4).unwrap();
        let row = &pixels(&out)[..32 * 4];
        assert!(row[4 * 4] < 8, "far left should stay black");
        assert!(row[27 * 4] > 247, "far right should stay white");
    }
}
