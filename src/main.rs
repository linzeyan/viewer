// Release builds on Windows shouldn't open a console window next to the viewer.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
mod alloc;
mod anim;
mod decode;
mod folder;
#[cfg(target_os = "macos")]
mod open_files;
#[cfg(windows)]
mod register;
mod render;
mod viewport;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use slint::ComponentHandle;
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};

use decode::{DecodeError, Request, Target};
use folder::Folder;
use render::{Pixels, Quality, Source};
use viewport::{Size, Viewport};

slint::include_modules!();

#[cfg(target_os = "macos")]
#[global_allocator]
static ALLOCATOR: alloc::Allocator = alloc::Allocator;

/// How long the view must sit still before the fast preview is redrawn smoothly.
const REFINE_DELAY: Duration = Duration::from_millis(150);
/// How long a window resize must settle before the image is decoded again at the new size.
const REDECODE_DELAY: Duration = Duration::from_millis(250);
/// Slint reports one mouse-wheel notch as 60 logical pixels; each notch zooms by 1.1×.
const WHEEL_NOTCH: f64 = 60.0;

struct Shown {
    generation: u64,
    source: Source,
    viewport: Viewport,
    full_requested: bool,
}

struct App {
    ui: MainWindow,
    decoder: mpsc::Sender<Request>,
    folder: Option<Folder>,
    /// Bumped on every navigation; decode results from older generations are dropped.
    generation: u64,
    direction: isize,
    shown: Option<Shown>,
    /// A file asked for before the window had its real size and scale factor.
    pending: Option<PathBuf>,
    window_ready: bool,
    /// Zoom when the current pinch began; the gesture reports scale relative to it.
    pinch_base: f64,
    refine: slint::Timer,
    redecode: slint::Timer,
    animation: Option<mpsc::Receiver<anim::Frame>>,
    next_frame: slint::Timer,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app(f: impl FnOnce(&mut App)) {
    APP.with_borrow_mut(|app| {
        if let Some(app) = app {
            f(app);
        }
    });
}

impl App {
    fn open(&mut self, path: &Path) {
        match Folder::open(path) {
            Ok(folder) => {
                self.folder = Some(folder);
                self.direction = 1;
                self.load_current();
            }
            Err(e) => self.show_message(&format!("無法開啟 {}\n{e}", path.display())),
        }
    }

    /// A file opened from the OS while running, or at launch before the window was ready.
    #[cfg_attr(not(target_os = "macos"), expect(dead_code))]
    fn open_requested(&mut self, path: PathBuf) {
        if self.window_ready {
            self.open(&path);
        } else {
            self.pending = Some(path);
        }
    }

    fn navigate(&mut self, delta: isize) {
        if let Some(folder) = &mut self.folder
            && folder.step(delta)
        {
            self.direction = delta;
            self.load_current();
        }
    }

    /// Starts decoding the current file. The previous image stays up until the new one
    /// arrives, so flipping through a folder doesn't flash an empty window.
    fn load_current(&mut self) {
        self.generation += 1;
        // Dropping the receiver ends the decoding thread.
        self.animation = None;
        self.next_frame.stop();
        let Some(folder) = &self.folder else { return };
        let Some(path) = folder.current() else {
            self.folder = None;
            self.shown = None;
            self.ui.set_window_title("Viewer".into());
            self.show_message("用 Viewer 開啟圖檔");
            return;
        };
        let (index, total) = folder.position();
        self.ui
            .set_window_title(format!("{} — {index} / {total}", display_name(path)).into());
        self.request(self.fit_target());
    }

    fn request(&self, target: Target) {
        let Some(path) = self.folder.as_ref().and_then(Folder::current) else { return };
        let _ = self.decoder.send(Request {
            generation: self.generation,
            path: path.to_path_buf(),
            target,
        });
    }

    fn on_decoded(&mut self, generation: u64, result: Result<Source, DecodeError>) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(source) => {
                let window = self.window_size();
                match &mut self.shown {
                    // A re-decode of the image on screen: keep the user's zoom and position.
                    Some(shown) if shown.generation == generation => {
                        if shown.source.is_full_resolution() && !source.is_full_resolution() {
                            return;
                        }
                        shown.source = source;
                    }
                    _ => {
                        if let Some(path) = self.folder.as_ref().and_then(Folder::current)
                            && anim::may_animate(path)
                        {
                            self.animation = Some(anim::play(path.to_path_buf()));
                            self.next_frame.start(slint::TimerMode::SingleShot, Duration::ZERO, || {
                                with_app(App::show_next_frame)
                            });
                        }
                        let viewport = Viewport::fit(source.full, window);
                        self.shown = Some(Shown {
                            generation,
                            source,
                            viewport,
                            full_requested: false,
                        });
                    }
                }
                self.ui.set_message("".into());
                self.present(Quality::Smooth);
            }
            Err(DecodeError::NotFound) => {
                if let Some(folder) = &mut self.folder {
                    folder.remove_current(self.direction);
                }
                self.load_current();
            }
            Err(DecodeError::Failed(reason)) => {
                self.shown = None;
                let name = self.folder.as_ref().and_then(Folder::current).map(display_name);
                self.show_message(&format!("無法開啟 {}\n{reason}", name.unwrap_or_default()));
            }
        }
    }

    /// Until the native window exists Slint reports a placeholder size (10×15 at scale 1),
    /// so this is the earliest point a file can be decoded to fit.
    fn on_window_ready(&mut self) {
        self.fit_window_to_screen();
        self.window_ready = true;
        match self.pending.take() {
            Some(path) => self.open(&path),
            None => self.show_message("用 Viewer 開啟圖檔"),
        }
    }

    /// The default 1200×800 is larger than common screens at high scaling (1920×1080 at 150%
    /// is 1280×720 logical): shrink to 85% of the monitor and center there.
    fn fit_window_to_screen(&self) {
        let window = self.ui.window();
        let Some(monitor) = window.with_winit_window(|w| w.current_monitor()).flatten() else {
            return;
        };
        let (screen, origin, size) = (monitor.size(), monitor.position(), window.size());
        let limit = |screen: u32| screen as f64 * 0.85;
        let shrink = (limit(screen.width) / size.width as f64)
            .min(limit(screen.height) / size.height as f64);
        if shrink >= 1.0 {
            return;
        }
        let new = slint::PhysicalSize::new(
            (size.width as f64 * shrink) as u32,
            (size.height as f64 * shrink) as u32,
        );
        window.set_size(new);
        window.set_position(slint::PhysicalPosition::new(
            origin.x + (screen.width - new.width) as i32 / 2,
            origin.y + (screen.height - new.height) as i32 / 2,
        ));
    }

    fn on_resized(&mut self) {
        if !self.window_ready {
            return;
        }
        let window = self.window_size();
        let Some(shown) = &mut self.shown else { return };
        shown.viewport.set_window(window);
        self.present(Quality::Fast);
        self.redecode.start(slint::TimerMode::SingleShot, REDECODE_DELAY, || {
            with_app(App::redecode_if_needed)
        });
    }

    fn show_next_frame(&mut self) {
        let Some(frames) = &self.animation else { return };
        let wait = match frames.try_recv() {
            Ok(frame) => {
                if let Some(shown) = &mut self.shown {
                    let full = Size {
                        w: frame.pixels.width() as f64,
                        h: frame.pixels.height() as f64,
                    };
                    shown.source = Source {
                        pixels: Pixels::Rgba(frame.pixels),
                        full,
                        vector: None,
                    };
                }
                self.present(Quality::Smooth);
                frame.delay
            }
            // The decoder is behind; check again shortly rather than block the UI.
            Err(TryRecvError::Empty) => Duration::from_millis(10),
            Err(TryRecvError::Disconnected) => {
                self.animation = None;
                return;
            }
        };
        self.next_frame.start(slint::TimerMode::SingleShot, wait, || {
            with_app(App::show_next_frame)
        });
    }

    fn zoom_by(&mut self, wheel_delta: f32, x: f32, y: f32) {
        let factor = 1.1_f64.powf(wheel_delta as f64 / WHEEL_NOTCH);
        let anchor = self.to_physical(x, y);
        self.update_view(|v| v.zoom_to(v.zoom() * factor, anchor));
    }

    fn pan_by(&mut self, dx: f32, dy: f32) {
        let (dx, dy) = self.to_physical(dx, dy);
        self.update_view(|v| v.pan(dx, dy));
    }

    fn toggle_zoom(&mut self, x: f32, y: f32) {
        let anchor = self.to_physical(x, y);
        self.update_view(|v| v.toggle_actual_size(anchor));
    }

    fn pinch_started(&mut self) {
        if let Some(shown) = &self.shown {
            self.pinch_base = shown.viewport.zoom();
        }
    }

    fn pinch(&mut self, scale: f32, x: f32, y: f32) {
        let zoom = self.pinch_base * scale as f64;
        let anchor = self.to_physical(x, y);
        self.update_view(|v| v.zoom_to(zoom, anchor));
    }

    /// Applies a zoom or pan and draws it fast. Once the view needs more detail than the
    /// fitted copy has, the full-resolution image is decoded in the background.
    fn update_view(&mut self, change: impl FnOnce(&mut Viewport)) {
        let Some(shown) = &mut self.shown else { return };
        change(&mut shown.viewport);
        let detail = shown.source.pixels.width() as f64 / shown.source.full.w;
        let needs_full = !shown.source.is_full_resolution()
            && shown.viewport.zoom() > detail * 1.001
            && !shown.full_requested;
        if needs_full {
            shown.full_requested = true;
            self.request(Target::Full);
        }
        self.present(Quality::Fast);
    }

    /// After a resize the fitted copy may be too small (blurry) or too large (wasted memory).
    fn redecode_if_needed(&mut self) {
        let Some(shown) = &self.shown else { return };
        if shown.source.is_full_resolution() || !shown.viewport.is_fitted() || shown.full_requested {
            return;
        }
        let Some((_, screen)) = shown.viewport.visible() else { return };
        if (screen.w, screen.h) != (shown.source.pixels.width(), shown.source.pixels.height()) {
            self.request(self.fit_target());
        }
    }

    /// Draws the visible part of the image at window resolution.
    fn present(&mut self, quality: Quality) {
        let Some(shown) = &self.shown else { return };
        let Some((region, screen)) = shown.viewport.visible() else {
            self.ui.set_frame(slint::Image::default());
            return;
        };
        let source = &shown.source;
        let pixels = if (screen.w, screen.h) == (source.pixels.width(), source.pixels.height())
            && shown.viewport.is_fitted()
        {
            // The fitted copy is exactly what the window shows: share it instead of copying.
            source.pixels.clone()
        } else if let (Quality::Smooth, Some(vector)) = (quality, &source.vector) {
            render::rasterize(vector, region, screen.w, screen.h)
        } else {
            render::render(source, region, screen.w, screen.h, quality)
        };
        let scale = self.ui.window().scale_factor();
        self.ui.set_can_pan(shown.viewport.can_pan());
        self.ui.set_frame(pixels.to_image());
        self.ui.set_frame_x(screen.x as f32 / scale);
        self.ui.set_frame_y(screen.y as f32 / scale);
        self.ui.set_frame_width(screen.w as f32 / scale);
        self.ui.set_frame_height(screen.h as f32 / scale);
        if quality == Quality::Fast {
            self.refine.start(slint::TimerMode::SingleShot, REFINE_DELAY, || {
                with_app(|app| app.present(Quality::Smooth))
            });
        }
    }

    fn show_message(&self, text: &str) {
        self.ui.set_frame(slint::Image::default());
        self.ui.set_message(text.into());
    }

    fn fit_target(&self) -> Target {
        Target::Fit {
            window: self.window_size(),
            scale: self.ui.window().scale_factor() as f64,
        }
    }

    fn to_physical(&self, x: f32, y: f32) -> (f64, f64) {
        let scale = self.ui.window().scale_factor() as f64;
        (x as f64 * scale, y as f64 * scale)
    }

    fn window_size(&self) -> Size {
        let size = self.ui.window().size();
        Size {
            w: size.width as f64,
            h: size.height as f64,
        }
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn main() -> Result<(), slint::PlatformError> {
    #[cfg(windows)]
    if let Some(code) = register::run_cli() {
        std::process::exit(code);
    }

    // Pinned in code so an inherited SLINT_BACKEND can't switch to a GPU renderer.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()?;

    let ui = MainWindow::new()?;
    ui.on_navigate(|delta| with_app(|app| app.navigate(delta as isize)));
    ui.on_resized(|| with_app(App::on_resized));
    ui.on_zoom_by(|delta, x, y| with_app(|app| app.zoom_by(delta, x, y)));
    ui.on_pan_by(|dx, dy| with_app(|app| app.pan_by(dx, dy)));
    ui.on_toggle_zoom(|x, y| with_app(|app| app.toggle_zoom(x, y)));
    ui.on_pinch_started(|| with_app(App::pinch_started));
    ui.on_pinch(|scale, x, y| with_app(|app| app.pinch(scale, x, y)));

    let decoder = decode::spawn_worker(|generation, result| {
        let _ = slint::invoke_from_event_loop(move || {
            with_app(|app| app.on_decoded(generation, result))
        });
    });

    APP.with_borrow_mut(|app| {
        *app = Some(App {
            ui: ui.clone_strong(),
            decoder,
            folder: None,
            generation: 0,
            direction: 1,
            shown: None,
            pending: std::env::args_os().nth(1).map(PathBuf::from),
            window_ready: false,
            pinch_base: 1.0,
            refine: slint::Timer::default(),
            redecode: slint::Timer::default(),
            animation: None,
            next_frame: slint::Timer::default(),
        })
    });

    #[cfg(target_os = "macos")]
    open_files::install(|path| with_app(|app| app.open_requested(path)));

    let weak = ui.as_weak();
    slint::spawn_local(async move {
        let Some(ui) = weak.upgrade() else { return };
        if ui.window().winit_window().await.is_ok() {
            with_app(App::on_window_ready);
        }
    })
    .expect("event loop exists");
    // Moving the window to a display with another scale factor keeps its logical size,
    // so `changed width` doesn't fire, yet the image must be redrawn at the new density.
    ui.window().on_winit_window_event(|_, event| {
        if let WindowEvent::ScaleFactorChanged { .. } = event {
            // Slint applies the new factor after this filter returns.
            slint::Timer::single_shot(Duration::ZERO, || with_app(App::on_resized));
        }
        EventResult::Propagate
    });

    ui.run()?;
    APP.with_borrow_mut(|app| *app = None);
    Ok(())
}
