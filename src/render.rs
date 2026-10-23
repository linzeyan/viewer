use std::sync::Arc;

use fast_image_resize as fr;
use resvg::{tiny_skia, usvg};
use slint::{Rgb8Pixel, Rgba8Pixel, SharedPixelBuffer};

use crate::viewport::{ImageRect, Size};

/// Decoded pixels. Opaque images stay RGB: a quarter less memory than RGBA.
///
/// `SharedPixelBuffer` is reference counted, so handing it to Slint doesn't copy it.
#[derive(Clone)]
pub enum Pixels {
    Rgb(SharedPixelBuffer<Rgb8Pixel>),
    Rgba(SharedPixelBuffer<Rgba8Pixel>),
}

impl Pixels {
    pub fn width(&self) -> u32 {
        match self {
            Pixels::Rgb(b) => b.width(),
            Pixels::Rgba(b) => b.width(),
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            Pixels::Rgb(b) => b.height(),
            Pixels::Rgba(b) => b.height(),
        }
    }

    pub fn to_image(&self) -> slint::Image {
        match self {
            Pixels::Rgb(b) => slint::Image::from_rgb8(b.clone()),
            Pixels::Rgba(b) => slint::Image::from_rgba8(b.clone()),
        }
    }

    fn view(&self) -> fr::images::ImageRef<'_> {
        let (bytes, pixel_type) = match self {
            Pixels::Rgb(b) => (b.as_bytes(), fr::PixelType::U8x3),
            Pixels::Rgba(b) => (b.as_bytes(), fr::PixelType::U8x4),
        };
        fr::images::ImageRef::new(self.width(), self.height(), bytes, pixel_type)
            .expect("buffer matches its own size")
    }
}

/// A parsed SVG. `units` is full-resolution pixels per SVG unit: the display scale factor at
/// load time, so 100% shows the SVG at its CSS size the way a browser does.
#[derive(Clone)]
pub struct Vector {
    pub tree: Arc<usvg::Tree>,
    pub units: f64,
}

/// Pixels of an image together with the full-resolution size they stand for.
/// The pixels may be a downscaled copy.
#[derive(Clone)]
pub struct Source {
    pub pixels: Pixels,
    pub full: Size,
    /// For SVG, `pixels` is only the stand-in shown while the view moves.
    pub vector: Option<Vector>,
}

impl Source {
    /// Vectors redraw sharply at any zoom, so they never need a larger decode.
    pub fn is_full_resolution(&self) -> bool {
        self.vector.is_some()
            || (self.pixels.width() as f64 >= self.full.w
                && self.pixels.height() as f64 >= self.full.h)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// While the view moves: about 1-2 ms for a full Retina window.
    Fast,
    /// Once the view settles.
    Smooth,
    /// One-off downscale on the decode thread.
    Best,
}

/// Resamples `region` (full-resolution coordinates) of `source` into a `w`×`h` buffer.
pub fn render(source: &Source, region: ImageRect, w: u32, h: u32, quality: Quality) -> Pixels {
    let sx = source.pixels.width() as f64 / source.full.w;
    let sy = source.pixels.height() as f64 / source.full.h;
    let crop = (region.x * sx, region.y * sy, region.w * sx, region.h * sy);
    // Past 100% of the real image the spec wants visible pixel squares. Judged against the
    // full-resolution size: a fitted copy stretched while the original decodes stays smooth.
    let quality = if w as f64 > region.w * 1.001 {
        Quality::Fast
    } else {
        quality
    };
    resample(&source.pixels, crop, w, h, quality)
}

/// Draws `region` (full-resolution coordinates) of an SVG straight into a `w`×`h` buffer.
pub fn rasterize(vector: &Vector, region: ImageRect, w: u32, h: u32) -> Pixels {
    let scale = w as f64 / region.w;
    let k = (vector.units * scale) as f32;
    let transform = tiny_skia::Transform::from_row(
        k,
        0.0,
        0.0,
        k,
        (-region.x * scale) as f32,
        (-region.y * scale) as f32,
    );
    let mut out = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    let mut pixmap = tiny_skia::PixmapMut::from_bytes(out.make_mut_bytes(), w, h)
        .expect("buffer matches its own size");
    resvg::render(&vector.tree, transform, &mut pixmap);
    // tiny-skia writes premultiplied alpha; everything else here is straight alpha.
    for px in out.make_mut_bytes().chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Pixels::Rgba(out)
}

pub fn resample(
    pixels: &Pixels,
    (left, top, width, height): (f64, f64, f64, f64),
    w: u32,
    h: u32,
    quality: Quality,
) -> Pixels {
    // Float rounding can push the crop a hair past the edge, which the resizer rejects.
    let (pw, ph) = (pixels.width() as f64, pixels.height() as f64);
    let (left, top) = (left.clamp(0.0, pw - 1.0), top.clamp(0.0, ph - 1.0));
    let (width, height) = (width.min(pw - left), height.min(ph - top));
    let alg = match quality {
        Quality::Fast => fr::ResizeAlg::Nearest,
        Quality::Smooth => fr::ResizeAlg::Convolution(fr::FilterType::Bilinear),
        Quality::Best => fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3),
    };
    let options = fr::ResizeOptions::new()
        .resize_alg(alg)
        .crop(left, top, width, height);
    let src = pixels.view();
    let mut resizer = fr::Resizer::new();
    match pixels {
        Pixels::Rgb(_) => {
            let mut out = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
            let mut dst =
                fr::images::Image::from_slice_u8(w, h, out.make_mut_bytes(), fr::PixelType::U8x3)
                    .expect("buffer matches its own size");
            resizer
                .resize(&src, &mut dst, &options)
                .expect("crop lies inside the source");
            Pixels::Rgb(out)
        }
        Pixels::Rgba(_) => {
            let mut out = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
            let mut dst =
                fr::images::Image::from_slice_u8(w, h, out.make_mut_bytes(), fr::PixelType::U8x4)
                    .expect("buffer matches its own size");
            resizer
                .resize(&src, &mut dst, &options)
                .expect("crop lies inside the source");
            Pixels::Rgba(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×1 fitted copy (black, white) standing in for a 4×2 original.
    fn half_size_source() -> Source {
        let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(2, 1);
        buf.make_mut_slice()[1] = Rgb8Pixel { r: 255, g: 255, b: 255 };
        Source {
            pixels: Pixels::Rgb(buf),
            full: Size { w: 4.0, h: 2.0 },
            vector: None,
        }
    }

    fn has_gray(pixels: &Pixels) -> bool {
        let Pixels::Rgb(b) = pixels else { unreachable!() };
        b.as_slice().iter().any(|p| p.r != 0 && p.r != 255)
    }

    #[test]
    fn stretched_fitted_copy_stays_smooth_below_actual_size() {
        // 100% of the original, while only the half-size copy is decoded.
        let whole = ImageRect { x: 0.0, y: 0.0, w: 4.0, h: 2.0 };
        assert!(has_gray(&render(&half_size_source(), whole, 4, 2, Quality::Smooth)));
    }

    #[test]
    fn beyond_actual_size_shows_pixel_squares() {
        // 200% of the original.
        let left_half = ImageRect { x: 0.0, y: 0.0, w: 2.0, h: 1.0 };
        assert!(!has_gray(&render(&half_size_source(), left_half, 4, 2, Quality::Smooth)));
    }
}
