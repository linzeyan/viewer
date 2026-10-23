//! Places a full-resolution image in the window. All window values are physical pixels.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

/// Where the visible part of the image lands, in whole window pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// A region of the full-resolution image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 3200%.
pub const MAX_ZOOM: f64 = 32.0;

#[derive(Clone, Debug)]
pub struct Viewport {
    image: Size,
    window: Size,
    /// Window pixels per image pixel.
    zoom: f64,
    /// The image point shown at the window center.
    center: (f64, f64),
    fitted: bool,
}

impl Viewport {
    pub fn fit(image: Size, window: Size) -> Self {
        let mut viewport = Self {
            image,
            window,
            zoom: 1.0,
            center: (0.0, 0.0),
            fitted: true,
        };
        viewport.reset_fit();
        viewport
    }

    /// Small images stay at 100%: enlarging them only adds blur.
    pub fn fit_zoom(&self) -> f64 {
        (self.window.w / self.image.w)
            .min(self.window.h / self.image.h)
            .min(1.0)
    }

    pub fn is_fitted(&self) -> bool {
        self.fitted
    }

    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    /// True when the image is larger than the window on some axis, so dragging moves it.
    pub fn can_pan(&self) -> bool {
        self.image.w * self.zoom > self.window.w + 0.5 || self.image.h * self.zoom > self.window.h + 0.5
    }

    pub fn set_window(&mut self, window: Size) {
        self.window = window;
        if self.fitted {
            self.reset_fit();
        } else {
            self.clamp();
        }
    }

    pub fn reset_fit(&mut self) {
        self.fitted = true;
        self.zoom = self.fit_zoom();
        self.center = (self.image.w / 2.0, self.image.h / 2.0);
    }

    /// Zooms so the image point under `anchor` (window pixels) stays under it.
    pub fn zoom_to(&mut self, zoom: f64, anchor: (f64, f64)) {
        let min = self.fit_zoom();
        let zoom = zoom.clamp(min, MAX_ZOOM);
        if zoom <= min * (1.0 + 1e-9) {
            self.reset_fit();
            return;
        }
        let offset = (anchor.0 - self.window.w / 2.0, anchor.1 - self.window.h / 2.0);
        let under = (self.center.0 + offset.0 / self.zoom, self.center.1 + offset.1 / self.zoom);
        self.zoom = zoom;
        self.center = (under.0 - offset.0 / zoom, under.1 - offset.1 / zoom);
        self.fitted = false;
        self.clamp();
    }

    /// Moves the image by `dx`, `dy` window pixels.
    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.center = (self.center.0 - dx / self.zoom, self.center.1 - dy / self.zoom);
        self.clamp();
    }

    /// Double-click: fitted ↔ 100% at the pointer. Images smaller than the window
    /// are already at 100% when fitted, so nothing changes for them.
    pub fn toggle_actual_size(&mut self, anchor: (f64, f64)) {
        if self.fitted {
            self.zoom_to(1.0, anchor);
        } else {
            self.reset_fit();
        }
    }

    /// Keeps the image centered on axes where it fits, and its edges at or beyond the window
    /// edges where it doesn't, so panning never reveals empty space next to a large image.
    fn clamp(&mut self) {
        let clamp_axis = |center: f64, image: f64, window: f64, zoom: f64| {
            let half = window / 2.0 / zoom;
            if image * zoom <= window {
                image / 2.0
            } else {
                center.clamp(half, image - half)
            }
        };
        self.center = (
            clamp_axis(self.center.0, self.image.w, self.window.w, self.zoom),
            clamp_axis(self.center.1, self.image.h, self.window.h, self.zoom),
        );
    }

    /// The visible part of the image and where it goes on screen.
    ///
    /// Screen edges are rounded to whole pixels so the renderer can blit 1:1;
    /// the image region is derived from those rounded edges so nothing shifts by a fraction.
    pub fn visible(&self) -> Option<(ImageRect, ScreenRect)> {
        let left = self.window.w / 2.0 - self.center.0 * self.zoom;
        let top = self.window.h / 2.0 - self.center.1 * self.zoom;
        let x0 = left.max(0.0).round();
        let y0 = top.max(0.0).round();
        let x1 = (left + self.image.w * self.zoom).min(self.window.w).round();
        let y1 = (top + self.image.h * self.zoom).min(self.window.h).round();
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let ix = ((x0 - left) / self.zoom).clamp(0.0, self.image.w);
        let iy = ((y0 - top) / self.zoom).clamp(0.0, self.image.h);
        let source = ImageRect {
            x: ix,
            y: iy,
            w: ((x1 - x0) / self.zoom).min(self.image.w - ix),
            h: ((y1 - y0) / self.zoom).min(self.image.h - iy),
        };
        let screen = ScreenRect {
            x: x0 as i32,
            y: y0 as i32,
            w: (x1 - x0) as u32,
            h: (y1 - y0) as u32,
        };
        Some((source, screen))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Size = Size { w: 2000.0, h: 1000.0 };

    #[test]
    fn large_image_fits_inside_window_and_is_centered() {
        let v = Viewport::fit(Size { w: 8000.0, h: 6000.0 }, WINDOW);
        let (source, screen) = v.visible().unwrap();
        // Height is the limiting side: 6000 → 1000, so 8000 → 1333.
        assert_eq!(screen, ScreenRect { x: 333, y: 0, w: 1334, h: 1000 });
        assert!(source.x.abs() < 1e-9 && source.y.abs() < 1e-9);
        assert!((source.w - 8000.0).abs() < 1.0 && (source.h - 6000.0).abs() < 1e-9);
    }

    #[test]
    fn small_image_is_not_enlarged() {
        let v = Viewport::fit(Size { w: 400.0, h: 300.0 }, WINDOW);
        assert_eq!(v.fit_zoom(), 1.0);
        let (_, screen) = v.visible().unwrap();
        assert_eq!(screen, ScreenRect { x: 800, y: 350, w: 400, h: 300 });
    }

    fn screen_of(v: &Viewport, image_point: (f64, f64)) -> (f64, f64) {
        (
            WINDOW.w / 2.0 + (image_point.0 - v.center.0) * v.zoom,
            WINDOW.h / 2.0 + (image_point.1 - v.center.1) * v.zoom,
        )
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor_still() {
        let mut v = Viewport::fit(Size { w: 8000.0, h: 6000.0 }, WINDOW);
        let anchor = (700.0, 300.0);
        let under = (
            v.center.0 + (anchor.0 - WINDOW.w / 2.0) / v.zoom,
            v.center.1 + (anchor.1 - WINDOW.h / 2.0) / v.zoom,
        );
        v.zoom_to(1.0, anchor);
        let (x, y) = screen_of(&v, under);
        assert!((x - anchor.0).abs() < 1e-6 && (y - anchor.1).abs() < 1e-6);
        assert!(!v.is_fitted());
    }

    #[test]
    fn zoom_is_limited_to_fit_and_3200_percent() {
        let mut v = Viewport::fit(Size { w: 8000.0, h: 6000.0 }, WINDOW);
        v.zoom_to(1000.0, (0.0, 0.0));
        assert_eq!(v.zoom(), MAX_ZOOM);
        v.zoom_to(0.0001, (0.0, 0.0));
        assert!(v.is_fitted());
        assert_eq!(v.zoom(), v.fit_zoom());
    }

    #[test]
    fn panning_stops_at_the_image_edges() {
        let mut v = Viewport::fit(Size { w: 8000.0, h: 6000.0 }, WINDOW);
        v.zoom_to(1.0, (1000.0, 500.0));
        v.pan(1e6, 1e6);
        // Dragged as far right/down as possible: the image's top-left corner meets the window's.
        let (source, screen) = v.visible().unwrap();
        assert_eq!((source.x, source.y), (0.0, 0.0));
        assert_eq!(screen, ScreenRect { x: 0, y: 0, w: 2000, h: 1000 });
    }

    #[test]
    fn an_axis_that_fits_stays_centered_while_panning() {
        // Wide panorama at 100%: taller window than image, so vertical drags do nothing.
        let mut v = Viewport::fit(Size { w: 6000.0, h: 500.0 }, WINDOW);
        v.zoom_to(1.0, (1000.0, 500.0));
        v.pan(0.0, 300.0);
        let (_, screen) = v.visible().unwrap();
        assert_eq!((screen.y, screen.h), (250, 500));
        assert!(v.can_pan());
    }

    #[test]
    fn double_click_toggles_between_fit_and_actual_size() {
        let mut v = Viewport::fit(Size { w: 8000.0, h: 6000.0 }, WINDOW);
        v.toggle_actual_size((1000.0, 500.0));
        assert_eq!(v.zoom(), 1.0);
        v.toggle_actual_size((1000.0, 500.0));
        assert!(v.is_fitted());

        let mut small = Viewport::fit(Size { w: 400.0, h: 300.0 }, WINDOW);
        small.toggle_actual_size((0.0, 0.0));
        assert!(small.is_fitted());
        assert!(!small.can_pan());
    }

    #[test]
    fn zoomed_view_keeps_its_zoom_when_the_window_changes() {
        let mut v = Viewport::fit(Size { w: 8000.0, h: 6000.0 }, WINDOW);
        v.zoom_to(2.0, (1000.0, 500.0));
        v.set_window(Size { w: 800.0, h: 600.0 });
        assert_eq!(v.zoom(), 2.0);
        let (_, screen) = v.visible().unwrap();
        assert_eq!(screen, ScreenRect { x: 0, y: 0, w: 800, h: 600 });
    }

    #[test]
    fn fitted_view_refits_when_the_window_changes() {
        let mut v = Viewport::fit(Size { w: 8000.0, h: 6000.0 }, WINDOW);
        v.set_window(Size { w: 800.0, h: 600.0 });
        let (_, screen) = v.visible().unwrap();
        assert_eq!(screen, ScreenRect { x: 0, y: 0, w: 800, h: 600 });
    }
}
