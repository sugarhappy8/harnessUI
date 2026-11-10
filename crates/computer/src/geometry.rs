//! Coordinates (ARCHITECTURE.md 9.3 "Coordinates"): where a screenshot came from, and how
//! the model's answer maps back to the point the input goes to.
//!
//! Three coordinate spaces are involved:
//!
//! 1. **Global input coordinates** ([`GlobalPoint`], [`Rect`]): what the OS input APIs
//!    take. On Windows these are physical pixels of the virtual desktop (the app is
//!    per-monitor DPI aware, so no call sees scaled values). On macOS they are points of
//!    the global display space, origin at the top-left of the main display, y growing
//!    downwards. On both, a monitor left of or above the main one has a negative origin.
//! 2. **Captured pixels**: the screenshot as captured, `pixels_per_unit` captured pixels per
//!    input unit (1 on Windows, the backing scale factor on macOS, e.g. 2 on Retina).
//! 3. **Model coordinates**: the screenshot resized to what llama.cpp would feed the model
//!    (see [`crate::profile`]), read in the model's [`Convention`].
//!
//! The mapping only needs the monitor's rectangle in input units and the size of the image
//! the model saw: a fraction of the image is the same fraction of the monitor.

use serde::{Deserialize, Serialize};

use crate::profile::Convention;

/// A point in global input coordinates (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct GlobalPoint {
    pub x: f64,
    pub y: f64,
}

impl GlobalPoint {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// The integer device position containing this point (floor, so a point never moves
    /// onto the next pixel or point to its right or below).
    pub fn device(self) -> (i32, i32) {
        (self.x.floor() as i32, self.y.floor() as i32)
    }
}

/// A rectangle in global input coordinates. `contains` is half-open: the right and bottom
/// edges belong to the neighbour.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(&self, p: GlobalPoint) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.x + self.width && p.y < self.y + self.height
    }

    pub fn centre(&self) -> GlobalPoint {
        GlobalPoint::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// Same rectangle within a quarter unit (monitor layouts are compared with this).
    pub fn approx_eq(&self, other: &Rect) -> bool {
        (self.x - other.x).abs() < 0.25
            && (self.y - other.y).abs() < 0.25
            && (self.width - other.width).abs() < 0.25
            && (self.height - other.height).abs() < 0.25
    }

    /// Area shared with `other` (0 when they do not overlap).
    pub fn overlap_area(&self, other: &Rect) -> f64 {
        let w = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let h = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
    }
}

/// One monitor as the platform reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// Platform id: the `HMONITOR` value on Windows, the `CGDirectDisplayID` on macOS.
    pub id: u32,
    /// Display name for the card and the model (untrusted: from the OS / EDID).
    pub name: String,
    /// Position and size in global input coordinates.
    pub bounds: Rect,
    /// Captured pixels per input unit: 1 on Windows, the backing scale factor on macOS.
    pub pixels_per_unit: f64,
    /// The display scaling the user chose (Windows DPI / 96, e.g. 1.5; macOS backing
    /// scale). Shown to the model and on the card; not used for mapping.
    pub ui_scale: f64,
    pub primary: bool,
}

impl MonitorInfo {
    /// The capture size in pixels this monitor should produce.
    pub fn pixel_size(&self) -> (u32, u32) {
        (
            (self.bounds.width * self.pixels_per_unit).round().max(1.0) as u32,
            (self.bounds.height * self.pixels_per_unit).round().max(1.0) as u32,
        )
    }
}

/// Orders monitors as the model sees them: primary first, then left to right, top to
/// bottom. Tool arguments name monitors by their 1-based position in this order.
pub fn order_monitors(monitors: &mut [MonitorInfo]) {
    monitors.sort_by(|a, b| {
        b.primary
            .cmp(&a.primary)
            .then(a.bounds.x.total_cmp(&b.bounds.x))
            .then(a.bounds.y.total_cmp(&b.bounds.y))
            .then(a.id.cmp(&b.id))
    });
}

/// The monitor holding most of `window` (the foreground window), or the one holding its
/// centre, or the primary, or the first.
pub fn monitor_for_window(monitors: &[MonitorInfo], window: Option<Rect>) -> Option<usize> {
    if let Some(w) = window {
        let best = monitors
            .iter()
            .enumerate()
            .map(|(i, m)| (i, m.bounds.overlap_area(&w)))
            .filter(|(_, a)| *a > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _)) = best {
            return Some(i);
        }
        if let Some(i) = monitors.iter().position(|m| m.bounds.contains(w.centre())) {
            return Some(i);
        }
    }
    monitors
        .iter()
        .position(|m| m.primary)
        .or(if monitors.is_empty() { None } else { Some(0) })
}

/// Why a model coordinate cannot be used.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CoordError {
    #[error("{0}")]
    Shape(String),
    #[error("{0}")]
    OutOfRange(String),
}

/// Everything needed to map an answer about one screenshot back to the screen. Stored per
/// chat for the latest screenshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenshotGeometry {
    /// Counts screenshots taken by this server.
    pub id: u64,
    pub monitor: MonitorInfo,
    /// Pixel size of the capture.
    pub captured: (u32, u32),
    /// Pixel size of the image the model saw.
    pub image: (u32, u32),
    pub convention: Convention,
}

impl ScreenshotGeometry {
    /// Model coordinates → global input coordinates.
    ///
    /// `coord` is `[x, y]` (a point) or four numbers (a box, whose centre is used; the axis
    /// order is the convention's). Values outside the screenshot are refused, not clamped,
    /// so a confused answer never clicks somewhere unexpected.
    pub fn to_global(&self, coord: &[f64]) -> Result<GlobalPoint, CoordError> {
        let (fx, fy) = model_to_fraction(coord, self.convention, self.image)?;
        Ok(fraction_to_global(fx, fy, &self.monitor))
    }

    /// Global input coordinates → continuous pixel position in the image the model saw
    /// (for the cursor marker). `None` when the point is not on this monitor.
    pub fn global_to_image(&self, p: GlobalPoint) -> Option<(f64, f64)> {
        let b = &self.monitor.bounds;
        if !b.contains(p) {
            return None;
        }
        Some((
            (p.x - b.x) / b.width * f64::from(self.image.0),
            (p.y - b.y) / b.height * f64::from(self.image.1),
        ))
    }

    /// Global input coordinates → the model's own coordinates (as integers), so a text line
    /// can tell the model where something is in its convention.
    pub fn global_to_model(&self, p: GlobalPoint) -> Option<(i64, i64)> {
        let (ix, iy) = self.global_to_image(p)?;
        Some(match self.convention {
            Convention::Pixels => (ix.floor() as i64, iy.floor() as i64),
            Convention::Grid1000 | Convention::Box1000 | Convention::Box1000Yx => (
                (ix / f64::from(self.image.0) * 1000.0).round() as i64,
                (iy / f64::from(self.image.1) * 1000.0).round() as i64,
            ),
        })
    }
}

/// Model coordinates → fraction of the image, each axis in `[0, 1]`.
///
/// - [`Convention::Pixels`]: pixel `x` of an image `w` wide covers `[x, x+1)`; its centre
///   is used. Accepted range `0..=w` (the far edge is read as the last pixel).
/// - [`Convention::Grid1000`]: `0..=1000` across the image.
/// - [`Convention::Box1000`]: `[x1, y1, x2, y2]` on the 0–1000 grid, centre used; a plain
///   `[x, y]` point is also accepted.
/// - [`Convention::Box1000Yx`]: as `Box1000` with `[y1, x1, y2, x2]` (and `[y, x]`).
///
/// Four numbers under `Pixels` or `Grid1000` are read as an `[x1, y1, x2, y2]` box too.
pub fn model_to_fraction(
    coord: &[f64],
    convention: Convention,
    image: (u32, u32),
) -> Result<(f64, f64), CoordError> {
    if coord.iter().any(|v| !v.is_finite()) {
        return Err(CoordError::Shape(
            "coordinates must be plain numbers".to_string(),
        ));
    }
    let yx = convention == Convention::Box1000Yx;
    let (x, y) = match (coord.len(), yx) {
        (2, false) => (coord[0], coord[1]),
        (2, true) => (coord[1], coord[0]),
        (4, false) => box_centre(coord[0], coord[1], coord[2], coord[3])?,
        (4, true) => box_centre(coord[1], coord[0], coord[3], coord[2])?,
        (n, _) => {
            return Err(CoordError::Shape(format!(
                "a coordinate is [x, y] or a box [x1, y1, x2, y2]; got {n} numbers"
            )));
        }
    };
    match convention {
        Convention::Pixels => {
            let (w, h) = (f64::from(image.0.max(1)), f64::from(image.1.max(1)));
            if !(0.0..=w).contains(&x) || !(0.0..=h).contains(&y) {
                return Err(CoordError::OutOfRange(format!(
                    "({x}, {y}) is outside the screenshot: x must be 0 to {}, y 0 to {}",
                    image.0.saturating_sub(1),
                    image.1.saturating_sub(1)
                )));
            }
            // Centre of the pixel, kept inside the image.
            let fx = ((x + 0.5) / w).clamp(0.5 / w, (w - 0.5) / w);
            let fy = ((y + 0.5) / h).clamp(0.5 / h, (h - 0.5) / h);
            Ok((fx, fy))
        }
        Convention::Grid1000 | Convention::Box1000 | Convention::Box1000Yx => {
            if !(0.0..=1000.0).contains(&x) || !(0.0..=1000.0).contains(&y) {
                return Err(CoordError::OutOfRange(format!(
                    "({x}, {y}) is outside the screenshot: both values must be 0 to 1000"
                )));
            }
            Ok((x / 1000.0, y / 1000.0))
        }
    }
}

fn box_centre(x1: f64, y1: f64, x2: f64, y2: f64) -> Result<(f64, f64), CoordError> {
    if x2 < x1 || y2 < y1 {
        return Err(CoordError::Shape(format!(
            "the box [{x1}, {y1}, {x2}, {y2}] has its corners swapped"
        )));
    }
    Ok(((x1 + x2) / 2.0, (y1 + y2) / 2.0))
}

/// Fraction of the monitor → global input coordinates. The result stays on the monitor:
/// at most the start of its last physical pixel on the right and bottom.
pub fn fraction_to_global(fx: f64, fy: f64, monitor: &MonitorInfo) -> GlobalPoint {
    let b = &monitor.bounds;
    let last = 1.0 / monitor.pixels_per_unit.max(1.0);
    let gx = (b.x + fx.clamp(0.0, 1.0) * b.width).min(b.x + b.width - last);
    let gy = (b.y + fy.clamp(0.0, 1.0) * b.height).min(b.y + b.height - last);
    GlobalPoint::new(gx.max(b.x), gy.max(b.y))
}

/// The bounding rectangle of all monitors (the Windows virtual desktop).
pub fn virtual_desktop(monitors: &[MonitorInfo]) -> Option<Rect> {
    let first = monitors.first()?;
    let (mut x0, mut y0) = (first.bounds.x, first.bounds.y);
    let (mut x1, mut y1) = (x0 + first.bounds.width, y0 + first.bounds.height);
    for m in &monitors[1..] {
        x0 = x0.min(m.bounds.x);
        y0 = y0.min(m.bounds.y);
        x1 = x1.max(m.bounds.x + m.bounds.width);
        y1 = y1.max(m.bounds.y + m.bounds.height);
    }
    Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// Windows `SendInput` absolute coordinates with `MOUSEEVENTF_VIRTUALDESK`: the virtual
/// desktop (`SM_XVIRTUALSCREEN`, `SM_YVIRTUALSCREEN`, `SM_CXVIRTUALSCREEN`,
/// `SM_CYVIRTUALSCREEN`) is normalised to `0..=65535`.
///
/// Windows maps a normalised value back with truncation (`px = dx * width / 65536`, as
/// Wine and ReactOS do and as AutoHotkey's `+1` compensation implies). Aiming at the
/// middle of the pixel's interval, `floor((px + 0.5) * 65536 / width)`, lands on `px` under
/// truncation and is at most one pixel off under the linear reading of Microsoft's docs
/// (`0..65535` ↔ first..last pixel). The injector also reads the cursor back and corrects
/// any remaining difference.
pub fn virtual_desk_absolute(px: i32, py: i32, desk: (i32, i32, i32, i32)) -> (i32, i32) {
    let (vx, vy, vw, vh) = desk;
    let norm = |p: i32, origin: i32, size: i32| -> i32 {
        let size = size.max(1);
        let rel = i64::from(p) - i64::from(origin);
        let v = ((rel as f64 + 0.5) * 65536.0 / f64::from(size)).floor();
        v.clamp(0.0, 65535.0) as i32
    };
    (norm(px, vx, vw), norm(py, vy, vh))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(x: f64, y: f64, w: f64, h: f64, ppu: f64) -> MonitorInfo {
        MonitorInfo {
            id: 1,
            name: "m".into(),
            bounds: Rect::new(x, y, w, h),
            pixels_per_unit: ppu,
            ui_scale: ppu,
            primary: x == 0.0 && y == 0.0,
        }
    }

    fn geo(m: MonitorInfo, image: (u32, u32), convention: Convention) -> ScreenshotGeometry {
        let captured = m.pixel_size();
        ScreenshotGeometry {
            id: 1,
            monitor: m,
            captured,
            image,
            convention,
        }
    }

    /// Windows truncation as Wine/ReactOS implement it.
    fn windows_denormalise(dx: i32, origin: i32, size: i32) -> i32 {
        origin + ((i64::from(dx) * i64::from(size)) >> 16) as i32
    }

    #[test]
    fn pixels_at_full_size_hit_the_same_pixel() {
        // Windows, 1920x1080 primary, image not resized.
        let g = geo(
            monitor(0.0, 0.0, 1920.0, 1080.0, 1.0),
            (1920, 1080),
            Convention::Pixels,
        );
        assert_eq!(g.to_global(&[0.0, 0.0]).unwrap().device(), (0, 0));
        assert_eq!(g.to_global(&[10.0, 20.0]).unwrap().device(), (10, 20));
        assert_eq!(
            g.to_global(&[1919.0, 1079.0]).unwrap().device(),
            (1919, 1079)
        );
        // The far edge is read as the last pixel, never the next monitor.
        assert_eq!(
            g.to_global(&[1920.0, 1080.0]).unwrap().device(),
            (1919, 1079)
        );
    }

    #[test]
    fn pixels_of_a_downscaled_image_map_to_the_centre_of_their_region() {
        // 3840x2160 monitor shown to the model at 1920x1080: pixel 10 covers 20..22.
        let g = geo(
            monitor(0.0, 0.0, 3840.0, 2160.0, 1.0),
            (1920, 1080),
            Convention::Pixels,
        );
        assert_eq!(g.to_global(&[10.0, 10.0]).unwrap().device(), (21, 21));
        // Qwen2.5-VL size of a 1440x900 screen (factor 28): 1428x896.
        let g = geo(
            monitor(0.0, 0.0, 1440.0, 900.0, 1.0),
            (1428, 896),
            Convention::Pixels,
        );
        let p = g.to_global(&[714.0, 448.0]).unwrap();
        assert!(
            (p.x - 720.5).abs() < 1.0 && (p.y - 450.5).abs() < 1.0,
            "{p:?}"
        );
    }

    #[test]
    fn negative_origins_on_windows() {
        // A secondary monitor left of and above the primary.
        let g = geo(
            monitor(-2560.0, -300.0, 2560.0, 1440.0, 1.0),
            (1280, 720),
            Convention::Pixels,
        );
        assert_eq!(g.to_global(&[0.0, 0.0]).unwrap().device(), (-2559, -299));
        assert_eq!(
            g.to_global(&[1279.0, 719.0]).unwrap().device(),
            (-2560 + 2559, -300 + 1439)
        );
        let mid = g.to_global(&[640.0, 360.0]).unwrap().device();
        assert_eq!(mid, (-2560 + 1281, -300 + 721));
        // Grid: 0 is the top-left pixel, 1000 the bottom-right one.
        let g = geo(
            monitor(-1920.0, 0.0, 1920.0, 1080.0, 1.0),
            (1932, 1092),
            Convention::Grid1000,
        );
        assert_eq!(g.to_global(&[0.0, 0.0]).unwrap().device(), (-1920, 0));
        assert_eq!(g.to_global(&[1000.0, 1000.0]).unwrap().device(), (-1, 1079));
        assert_eq!(g.to_global(&[500.0, 500.0]).unwrap().device(), (-960, 540));
    }

    #[test]
    fn hidpi_macos_points() {
        // Retina main display: 1512x982 points, 3024x1964 pixels, image 1428x924.
        let g = geo(
            monitor(0.0, 0.0, 1512.0, 982.0, 2.0),
            (1428, 924),
            Convention::Grid1000,
        );
        let p = g.to_global(&[500.0, 500.0]).unwrap();
        assert_eq!((p.x, p.y), (756.0, 491.0));
        // The far corner stays on the last physical pixel (half a point in).
        let p = g.to_global(&[1000.0, 1000.0]).unwrap();
        assert_eq!((p.x, p.y), (1511.5, 981.5));
        assert_eq!(p.device(), (1511, 981));
        // An external monitor above the main one, at 1x, with a negative origin.
        let g = geo(
            monitor(-200.0, -1080.0, 1920.0, 1080.0, 1.0),
            (1920, 1088),
            Convention::Grid1000,
        );
        let p = g.to_global(&[250.0, 250.0]).unwrap();
        assert_eq!((p.x, p.y), (-200.0 + 480.0, -1080.0 + 270.0));
    }

    #[test]
    fn fractional_windows_scales_use_physical_pixels() {
        // 150 % scaling on Windows: input is still physical pixels (pixels_per_unit 1).
        let mut m = monitor(1920.0, 0.0, 2560.0, 1440.0, 1.0);
        m.ui_scale = 1.5;
        let g = geo(m, (2380, 1344), Convention::Pixels);
        let p = g.to_global(&[1190.0, 672.0]).unwrap().device();
        assert_eq!(p, (1920 + 1280, 720));
    }

    #[test]
    fn boxes_use_their_centre_in_either_axis_order() {
        let g = geo(
            monitor(0.0, 0.0, 2000.0, 1000.0, 1.0),
            (1000, 500),
            Convention::Box1000,
        );
        let p = g.to_global(&[100.0, 200.0, 300.0, 400.0]).unwrap();
        assert_eq!((p.x, p.y), (400.0, 300.0));
        // A plain point is also accepted.
        let p = g.to_global(&[500.0, 500.0]).unwrap();
        assert_eq!((p.x, p.y), (1000.0, 500.0));
        // Gemma order: [y1, x1, y2, x2].
        let g = geo(
            monitor(0.0, 0.0, 2000.0, 1000.0, 1.0),
            (1000, 500),
            Convention::Box1000Yx,
        );
        let p = g.to_global(&[200.0, 100.0, 400.0, 300.0]).unwrap();
        assert_eq!((p.x, p.y), (400.0, 300.0));
        // Swapped corners are refused.
        assert!(matches!(
            g.to_global(&[400.0, 300.0, 200.0, 100.0]),
            Err(CoordError::Shape(_))
        ));
        // A box in pixel convention: centre of the pixel box.
        let g = geo(
            monitor(0.0, 0.0, 1000.0, 500.0, 1.0),
            (1000, 500),
            Convention::Pixels,
        );
        assert_eq!(
            g.to_global(&[10.0, 10.0, 20.0, 30.0]).unwrap().device(),
            (15, 20)
        );
    }

    #[test]
    fn out_of_range_and_bad_shapes_are_refused() {
        let g = geo(
            monitor(0.0, 0.0, 100.0, 100.0, 1.0),
            (50, 50),
            Convention::Pixels,
        );
        assert!(matches!(
            g.to_global(&[-1.0, 0.0]),
            Err(CoordError::OutOfRange(_))
        ));
        assert!(matches!(
            g.to_global(&[51.0, 0.0]),
            Err(CoordError::OutOfRange(_))
        ));
        assert!(matches!(
            g.to_global(&[1.0, 2.0, 3.0]),
            Err(CoordError::Shape(_))
        ));
        assert!(matches!(
            g.to_global(&[f64::NAN, 2.0]),
            Err(CoordError::Shape(_))
        ));
        let g = geo(
            monitor(0.0, 0.0, 100.0, 100.0, 1.0),
            (50, 50),
            Convention::Grid1000,
        );
        assert!(matches!(
            g.to_global(&[1001.0, 0.0]),
            Err(CoordError::OutOfRange(_))
        ));
        let msg = g.to_global(&[1001.0, 0.0]).unwrap_err().to_string();
        assert!(msg.contains("0 to 1000"), "{msg}");
    }

    #[test]
    fn inverse_mapping_for_the_cursor_marker() {
        let g = geo(
            monitor(-1000.0, 0.0, 1000.0, 500.0, 2.0),
            (500, 250),
            Convention::Grid1000,
        );
        assert_eq!(
            g.global_to_image(GlobalPoint::new(-500.0, 250.0)),
            Some((250.0, 125.0))
        );
        assert_eq!(g.global_to_image(GlobalPoint::new(10.0, 10.0)), None);
        assert_eq!(
            g.global_to_model(GlobalPoint::new(-500.0, 250.0)),
            Some((500, 500))
        );
        // Round trip through the model's convention lands within a pixel.
        let p = g.to_global(&[123.0, 456.0]).unwrap();
        let (mx, my) = g.global_to_model(p).unwrap();
        assert!((mx - 123).abs() <= 1 && (my - 456).abs() <= 1);
    }

    #[test]
    fn monitor_order_and_default_choice() {
        let mut ms = vec![
            MonitorInfo {
                id: 3,
                primary: false,
                ..monitor(1920.0, 0.0, 1920.0, 1080.0, 1.0)
            },
            MonitorInfo {
                id: 2,
                primary: false,
                ..monitor(-1280.0, 0.0, 1280.0, 1024.0, 1.0)
            },
            MonitorInfo {
                id: 1,
                primary: true,
                ..monitor(0.0, 0.0, 1920.0, 1080.0, 1.0)
            },
        ];
        order_monitors(&mut ms);
        assert_eq!(ms.iter().map(|m| m.id).collect::<Vec<_>>(), vec![1, 2, 3]);
        // A window mostly on the right-hand monitor.
        let w = Rect::new(1800.0, 100.0, 800.0, 600.0);
        assert_eq!(monitor_for_window(&ms, Some(w)), Some(2));
        // No window: the primary.
        assert_eq!(monitor_for_window(&ms, None), Some(0));
        // A window off every monitor: the primary.
        let off = Rect::new(9000.0, 9000.0, 10.0, 10.0);
        assert_eq!(monitor_for_window(&ms, Some(off)), Some(0));
        assert_eq!(monitor_for_window(&[], None), None);
        assert_eq!(
            virtual_desktop(&ms),
            Some(Rect::new(-1280.0, 0.0, 1280.0 + 3840.0, 1080.0))
        );
    }

    #[test]
    fn virtual_desk_normalisation_round_trips_under_truncation() {
        // Two monitors: 1920x1080 primary and 2560x1440 to its left, 360 px higher.
        let desk = (-2560, -360, 2560 + 1920, 1440);
        for &(px, py) in &[
            (-2560, -360),
            (-2559, -359),
            (-1, 0),
            (0, 0),
            (1, 1),
            (960, 540),
            (1919, 1079),
            (-1280, 700),
        ] {
            let (dx, dy) = virtual_desk_absolute(px, py, desk);
            assert!((0..=65535).contains(&dx) && (0..=65535).contains(&dy));
            assert_eq!(windows_denormalise(dx, desk.0, desk.2), px, "x for {px}");
            assert_eq!(windows_denormalise(dy, desk.1, desk.3), py, "y for {py}");
        }
        // Every pixel of a single 1920-wide screen round-trips.
        for px in 0..1920 {
            let (dx, _) = virtual_desk_absolute(px, 0, (0, 0, 1920, 1080));
            assert_eq!(windows_denormalise(dx, 0, 1920), px);
        }
        // Outside the desktop is clamped into range.
        assert_eq!(virtual_desk_absolute(-5000, 99999, desk), (0, 65535));
    }
}
