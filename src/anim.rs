use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, Frames};
use slint::{Rgba8Pixel, SharedPixelBuffer};

pub struct Frame {
    pub pixels: SharedPixelBuffer<Rgba8Pixel>,
    /// How long to show this frame.
    pub delay: Duration,
}

/// Browsers show GIF frames with a delay under 20 ms for 100 ms; files rely on it.
const MIN_DELAY: Duration = Duration::from_millis(20);
const DEFAULT_DELAY: Duration = Duration::from_millis(100);

pub fn may_animate(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gif") || e.eq_ignore_ascii_case("webp"))
}

/// Decodes frames on a background thread, looping forever.
///
/// The channel holds one frame, so at most three frames exist at once: the one on screen,
/// the queued one and the one being decoded. Dropping the receiver ends the thread.
/// Files with a single frame send nothing.
pub fn play(path: PathBuf) -> mpsc::Receiver<Frame> {
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("animation".into())
        .spawn(move || {
            loop {
                let Some(frames) = open_frames(&path) else { return };
                let mut frames = frames.peekable();
                let mut count = 0;
                while let Some(Ok(frame)) = frames.next() {
                    count += 1;
                    if count == 1 && frames.peek().is_none() {
                        return; // A still image: the regular decode already shows it.
                    }
                    let (numer, denom) = frame.delay().numer_denom_ms();
                    let delay = Duration::from_secs_f64(numer as f64 / denom.max(1) as f64 / 1000.0);
                    let buffer = frame.into_buffer();
                    let pixels = SharedPixelBuffer::clone_from_slice(
                        buffer.as_raw(),
                        buffer.width(),
                        buffer.height(),
                    );
                    let delay = if delay < MIN_DELAY { DEFAULT_DELAY } else { delay };
                    if tx.send(Frame { pixels, delay }).is_err() {
                        return;
                    }
                }
                if count == 0 {
                    return;
                }
            }
        })
        .expect("spawn animation thread");
    rx
}

fn open_frames(path: &Path) -> Option<Frames<'static>> {
    let reader = BufReader::new(File::open(path).ok()?);
    let is_gif = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gif"));
    if is_gif {
        Some(GifDecoder::new(reader).ok()?.into_frames())
    } else {
        let decoder = WebPDecoder::new(reader).ok()?;
        // Checked before decoding anything: still WebP photos are common and large.
        decoder.has_animation().then(|| decoder.into_frames())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Rgba, RgbaImage};

    fn write_gif(path: &Path, colors: &[[u8; 4]], delay_ms: u32) {
        let mut encoder = GifEncoder::new(File::create(path).unwrap());
        encoder.set_repeat(Repeat::Infinite).unwrap();
        for c in colors {
            let frame = image::Frame::from_parts(
                RgbaImage::from_pixel(4, 4, Rgba(*c)),
                0,
                0,
                Delay::from_numer_denom_ms(delay_ms, 1),
            );
            encoder.encode_frame(frame).unwrap();
        }
    }

    #[test]
    fn animation_loops_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.gif");
        write_gif(&path, &[[255, 0, 0, 255], [0, 0, 255, 255]], 50);
        let rx = play(path);
        let reds: Vec<u8> = (0..4).map(|_| rx.recv().unwrap().pixels.as_slice()[0].r).collect();
        assert_eq!(reds, [255, 0, 255, 0]);
    }

    #[test]
    fn zero_delay_frames_get_the_browser_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fast.gif");
        write_gif(&path, &[[1, 1, 1, 255], [2, 2, 2, 255]], 0);
        assert_eq!(play(path).recv().unwrap().delay, DEFAULT_DELAY);
    }

    #[test]
    fn still_gif_sends_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("still.gif");
        write_gif(&path, &[[1, 1, 1, 255]], 0);
        assert!(play(path).recv().is_err());
    }
}
