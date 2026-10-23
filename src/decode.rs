use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, mpsc};

use image::metadata::Orientation;
use image::{ColorType, DynamicImage, ImageDecoder, ImageReader};
use resvg::usvg;
use resvg::usvg::fontdb;
use slint::{Rgb8Pixel, Rgba8Pixel, SharedPixelBuffer};

use crate::render::{Pixels, Quality, Source, Vector, rasterize, resample};
use crate::viewport::{ImageRect, Size, Viewport};

#[derive(Clone, Copy, Debug)]
pub enum Target {
    /// Downscale to what a fitted view in a window of this many physical pixels shows.
    /// `scale` is the window's scale factor, which sets the size of an SVG unit.
    Fit { window: Size, scale: f64 },
    Full,
}

#[derive(Debug)]
pub enum DecodeError {
    NotFound,
    Failed(String),
}

pub struct Request {
    pub generation: u64,
    pub path: PathBuf,
    pub target: Target,
}

/// Starts the decode thread. `done` runs on that thread with each result.
///
/// Requests queued behind a newer one are skipped: holding an arrow key only decodes
/// the image the user stops on.
pub fn spawn_worker(
    done: impl Fn(u64, Result<Source, DecodeError>) + Send + 'static,
) -> mpsc::Sender<Request> {
    let (tx, rx) = mpsc::channel::<Request>();
    std::thread::Builder::new()
        .name("decode".into())
        .spawn(move || {
            while let Ok(mut request) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    request = newer;
                }
                done(request.generation, decode(&request.path, request.target));
            }
        })
        .expect("spawn decode thread");
    tx
}

pub fn is_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
}

pub fn decode(path: &Path, target: Target) -> Result<Source, DecodeError> {
    if is_svg(path) {
        return decode_svg(path, target);
    }
    let pixels = decode_full(path)?;
    let full = Size {
        w: pixels.width() as f64,
        h: pixels.height() as f64,
    };
    let source = |pixels| Source {
        pixels,
        full,
        vector: None,
    };
    let Target::Fit { window, .. } = target else {
        return Ok(source(pixels));
    };
    let Some((_, screen)) = Viewport::fit(full, window).visible() else {
        return Ok(source(pixels));
    };
    if screen.w >= pixels.width() && screen.h >= pixels.height() {
        return Ok(source(pixels));
    }
    // The full-size buffer is dropped on return; only the window-sized copy stays.
    let pixels = resample(&pixels, (0.0, 0.0, full.w, full.h), screen.w, screen.h, Quality::Best);
    Ok(source(pixels))
}

fn decode_svg(path: &Path, target: Target) -> Result<Source, DecodeError> {
    let data = std::fs::read(path).map_err(io_error)?;
    let tree = usvg::Tree::from_data(&data, svg_options())
        .map_err(|e| DecodeError::Failed(e.to_string()))?;
    let (window, units) = match target {
        Target::Fit { window, scale } => (Some(window), scale),
        Target::Full => (None, 1.0),
    };
    let size = tree.size();
    let full = Size {
        w: size.width() as f64 * units,
        h: size.height() as f64 * units,
    };
    let (w, h) = window
        .and_then(|window| Viewport::fit(full, window).visible())
        .map_or((full.w.ceil() as u32, full.h.ceil() as u32), |(_, s)| (s.w, s.h));
    let vector = Vector {
        tree: Arc::new(tree),
        units,
    };
    let whole = ImageRect {
        x: 0.0,
        y: 0.0,
        w: full.w,
        h: full.h,
    };
    Ok(Source {
        pixels: rasterize(&vector, whole, w.max(1), h.max(1)),
        full,
        vector: Some(vector),
    })
}

/// Text whose fonts aren't installed uses the system's Traditional Chinese UI font.
/// usvg would otherwise fall back to Times New Roman, and take missing glyphs from
/// whichever font it scans first (MingLiU on Windows).
#[cfg(windows)]
const FALLBACK_FAMILY: &str = "Microsoft JhengHei";
#[cfg(not(windows))]
const FALLBACK_FAMILY: &str = "PingFang TC";

/// System fonts are loaded once, on first use: few SVGs contain text, and scanning
/// the font directories takes tens of milliseconds.
fn svg_options() -> &'static usvg::Options<'static> {
    static OPTIONS: OnceLock<usvg::Options<'static>> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        let mut options = usvg::Options::default();
        options.fontdb_mut().load_system_fonts();
        options.font_family = FALLBACK_FAMILY.to_owned();
        options.font_resolver = usvg::FontResolver {
            select_font: Box::new(select_font),
            select_fallback: select_fallback(),
        };
        options
    })
}

/// usvg's default selector, with the fallback family tried before its last resort (serif).
fn select_font(font: &usvg::Font, db: &mut Arc<fontdb::Database>) -> Option<fontdb::ID> {
    let mut families: Vec<fontdb::Family> = font
        .families()
        .iter()
        .map(|family| match family {
            usvg::FontFamily::Serif => fontdb::Family::Serif,
            usvg::FontFamily::SansSerif => fontdb::Family::SansSerif,
            usvg::FontFamily::Cursive => fontdb::Family::Cursive,
            usvg::FontFamily::Fantasy => fontdb::Family::Fantasy,
            usvg::FontFamily::Monospace => fontdb::Family::Monospace,
            usvg::FontFamily::Named(name) => fontdb::Family::Name(name),
        })
        .collect();
    families.extend([fontdb::Family::Name(FALLBACK_FAMILY), fontdb::Family::Serif]);
    let stretch = match font.stretch() {
        usvg::FontStretch::UltraCondensed => fontdb::Stretch::UltraCondensed,
        usvg::FontStretch::ExtraCondensed => fontdb::Stretch::ExtraCondensed,
        usvg::FontStretch::Condensed => fontdb::Stretch::Condensed,
        usvg::FontStretch::SemiCondensed => fontdb::Stretch::SemiCondensed,
        usvg::FontStretch::Normal => fontdb::Stretch::Normal,
        usvg::FontStretch::SemiExpanded => fontdb::Stretch::SemiExpanded,
        usvg::FontStretch::Expanded => fontdb::Stretch::Expanded,
        usvg::FontStretch::ExtraExpanded => fontdb::Stretch::ExtraExpanded,
        usvg::FontStretch::UltraExpanded => fontdb::Stretch::UltraExpanded,
    };
    let style = match font.style() {
        usvg::FontStyle::Normal => fontdb::Style::Normal,
        usvg::FontStyle::Italic => fontdb::Style::Italic,
        usvg::FontStyle::Oblique => fontdb::Style::Oblique,
    };
    db.query(&fontdb::Query {
        families: &families,
        weight: fontdb::Weight(font.weight()),
        stretch,
        style,
    })
}

/// For a character the chosen font lacks, prefers the fallback family in the same weight;
/// usvg's default picks the first font that has it.
fn select_fallback() -> usvg::FallbackSelectionFn<'static> {
    let default = usvg::FontResolver::default_fallback_selector();
    Box::new(move |c, used, db| {
        let weight = used.first().and_then(|id| db.face(*id)).map(|face| face.weight);
        let preferred = db.query(&fontdb::Query {
            families: &[fontdb::Family::Name(FALLBACK_FAMILY)],
            weight: weight.unwrap_or_default(),
            ..Default::default()
        });
        match preferred {
            Some(id) if !used.contains(&id) && has_char(db, id, c) => Some(id),
            _ => default(c, used, db),
        }
    })
}

fn has_char(db: &fontdb::Database, id: fontdb::ID, c: char) -> bool {
    db.with_face_data(id, |data, index| {
        ttf_parser::Face::parse(data, index).is_ok_and(|face| face.glyph_index(c).is_some())
    }) == Some(true)
}

fn decode_full(path: &Path) -> Result<Pixels, DecodeError> {
    let reader = ImageReader::open(path)
        .map_err(io_error)?
        .with_guessed_format()
        .map_err(io_error)?;
    let mut decoder = reader.into_decoder().map_err(failed)?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let (w, h) = decoder.dimensions();
    // Decode straight into the buffer Slint will use when no conversion is needed,
    // so a 48MP photo doesn't briefly exist twice.
    if orientation == Orientation::NoTransforms {
        match decoder.color_type() {
            ColorType::Rgb8 => {
                let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
                decoder.read_image(buf.make_mut_bytes()).map_err(failed)?;
                return Ok(Pixels::Rgb(buf));
            }
            ColorType::Rgba8 => {
                let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
                decoder.read_image(buf.make_mut_bytes()).map_err(failed)?;
                return Ok(Pixels::Rgba(buf));
            }
            _ => {}
        }
    }
    let mut image = DynamicImage::from_decoder(decoder).map_err(failed)?;
    image.apply_orientation(orientation);
    let (w, h) = (image.width(), image.height());
    Ok(if image.color().has_alpha() {
        Pixels::Rgba(SharedPixelBuffer::clone_from_slice(
            image.into_rgba8().as_raw(),
            w,
            h,
        ))
    } else {
        Pixels::Rgb(SharedPixelBuffer::clone_from_slice(
            image.into_rgb8().as_raw(),
            w,
            h,
        ))
    })
}

fn io_error(e: std::io::Error) -> DecodeError {
    if e.kind() == std::io::ErrorKind::NotFound {
        DecodeError::NotFound
    } else {
        DecodeError::Failed(e.to_string())
    }
}

fn failed(e: image::ImageError) -> DecodeError {
    DecodeError::Failed(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    const WINDOW: Size = Size { w: 400.0, h: 300.0 };
    const FIT: Target = Target::Fit {
        window: WINDOW,
        scale: 1.0,
    };

    #[test]
    fn opaque_images_are_kept_as_rgb_to_save_memory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opaque.png");
        RgbImage::from_pixel(10, 10, Rgb([1, 2, 3])).save(&path).unwrap();
        assert!(matches!(decode(&path, Target::Full).unwrap().pixels, Pixels::Rgb(_)));
    }

    #[test]
    fn transparency_survives_decoding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alpha.png");
        RgbaImage::from_pixel(10, 10, Rgba([1, 2, 3, 4])).save(&path).unwrap();
        assert!(matches!(decode(&path, Target::Full).unwrap().pixels, Pixels::Rgba(_)));
    }

    #[test]
    fn fit_target_keeps_only_a_window_sized_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.png");
        RgbImage::new(4000, 3000).save(&path).unwrap();
        let source = decode(&path, FIT).unwrap();
        assert_eq!((source.pixels.width(), source.pixels.height()), (400, 300));
        assert_eq!(source.full, Size { w: 4000.0, h: 3000.0 });
        assert!(!source.is_full_resolution());
    }

    #[test]
    fn small_images_are_not_resampled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.png");
        RgbImage::new(40, 30).save(&path).unwrap();
        let source = decode(&path, FIT).unwrap();
        assert_eq!((source.pixels.width(), source.pixels.height()), (40, 30));
        assert!(source.is_full_resolution());
    }

    #[test]
    fn content_wins_over_a_wrong_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("really-a-png.jpg");
        RgbImage::new(4, 4).save_with_format(&path, image::ImageFormat::Png).unwrap();
        assert!(decode(&path, Target::Full).is_ok());
    }

    const HALF_RED_HALF_BLUE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
        <rect width="5" height="10" fill="red"/><rect x="5" width="5" height="10" fill="blue"/></svg>"#;

    #[test]
    fn svg_is_sized_like_a_browser_shows_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.svg");
        std::fs::write(&path, HALF_RED_HALF_BLUE).unwrap();
        // 10 CSS px on a 2× display are 20 physical pixels.
        let retina = Target::Fit {
            window: WINDOW,
            scale: 2.0,
        };
        let source = decode(&path, retina).unwrap();
        assert_eq!(source.full, Size { w: 20.0, h: 20.0 });
        assert_eq!((source.pixels.width(), source.pixels.height()), (20, 20));
    }

    #[test]
    fn svg_stays_sharp_when_zoomed_far_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.svg");
        std::fs::write(&path, HALF_RED_HALF_BLUE).unwrap();
        let source = decode(&path, FIT).unwrap();
        // 100× around the red/blue edge at x = 5.
        let region = ImageRect {
            x: 3.0,
            y: 0.0,
            w: 4.0,
            h: 4.0,
        };
        let Pixels::Rgba(out) = rasterize(source.vector.as_ref().unwrap(), region, 400, 400) else {
            unreachable!()
        };
        let row = &out.as_slice()[..400];
        let blended = row.iter().filter(|p| p.r != 0 && p.b != 0).count();
        assert!(blended <= 1, "edge spans {blended} pixels");
        assert_eq!((row[150].r, row[150].b), (255, 0));
        assert_eq!((row[250].r, row[250].b), (0, 255));
    }

    /// Each glyph's text and whether it was drawn with the fallback family.
    fn glyphs_in_fallback_font(svg: &str) -> Vec<(String, bool)> {
        let tree = usvg::Tree::from_str(svg, svg_options()).unwrap();
        let db = tree.fontdb();
        let mut glyphs = Vec::new();
        for node in tree.root().children() {
            let usvg::Node::Text(text) = node else { continue };
            for glyph in text.layouted().iter().flat_map(|span| &span.positioned_glyphs) {
                let face = db.face(glyph.font).unwrap();
                let fallback = face.families.iter().any(|(name, _)| name == FALLBACK_FAMILY);
                glyphs.push((glyph.text.clone(), fallback));
            }
        }
        assert!(!glyphs.is_empty(), "no text was laid out; is {FALLBACK_FAMILY} installed?");
        glyphs
    }

    #[test]
    fn text_in_an_uninstalled_font_uses_the_fallback_family() {
        let glyphs = glyphs_in_fallback_font(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50">
            <text y="40" font-family="No Such Font" font-size="30">Ab中文</text></svg>"#,
        );
        assert!(glyphs.iter().all(|(_, fallback)| *fallback), "{glyphs:?}");
    }

    #[test]
    fn characters_missing_from_an_installed_font_come_from_the_fallback_family() {
        // Arial is on both platforms and has no Chinese. usvg reshapes the whole run with
        // the fallback font when that font covers all of it, so only the Chinese is checked.
        let glyphs = glyphs_in_fallback_font(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50">
            <text y="40" font-family="Arial" font-size="30">Ab中文</text></svg>"#,
        );
        let chinese: Vec<_> = glyphs.iter().filter(|(t, _)| !t.is_ascii()).collect();
        assert_eq!(chinese.len(), 2, "{glyphs:?}");
        assert!(chinese.iter().all(|(_, fallback)| *fallback), "{glyphs:?}");
    }

    #[test]
    fn missing_file_is_reported_as_not_found() {
        let result = decode(Path::new("/nonexistent/x.jpg"), Target::Full);
        assert!(matches!(result, Err(DecodeError::NotFound)));
    }
}
