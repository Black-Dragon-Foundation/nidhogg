//! Naglfar's one unit rule, and the one place that converts.
//!
//! The host's pixels (Lisp's, the frame's, a surface's, an event's position) are **device pixels**.  AppKit's sizes
//! and positions are **points**.  A window's scale is the one the host was told (`ShellEvent::Scale`), read from the
//! window once it is placed on its screen, never before: AppKit decides a window's backing scale only once the window
//! is on a screen, and a scale read earlier is the one of wherever AppKit first put it (the reparent bug).  Every
//! conversion between the two goes through [`Scale`]; the macOS backend reads AppKit's factor only in
//! `macos/scale.rs`.  The test `the_macos_backend_converts_only_here` holds both rules.

/// A backing scale, in 120ths (the host's unit for it: 120 is 1x, 240 is 2x).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scale(u32);

impl Scale {
    pub const ONE: Scale = Scale(120);

    /// The scale of AppKit's factor (`backingScaleFactor`), rounded to 120ths.
    pub fn of_factor(factor: f64) -> Scale {
        Scale(((factor * 120.0).round() as u32).max(1))
    }

    /// The scale of `s120` 120ths (0 is taken as 1x: a window not placed yet has no scale).
    pub fn of120(s120: u32) -> Scale {
        if s120 == 0 { Scale::ONE } else { Scale(s120) }
    }

    pub fn s120(self) -> u32 {
        self.0
    }

    /// The factor AppKit's APIs take (`contentsScale`, a layer's scale).
    pub fn factor(self) -> f64 {
        f64::from(self.0) / 120.0
    }

    /// Points to device pixels.
    pub fn px(self, points: f64) -> f64 {
        points * self.factor()
    }

    /// Device pixels to points.
    pub fn pt(self, px: f64) -> f64 {
        px / self.factor()
    }

    /// A size in points as device pixels, rounded (the host's size of a window of that many points).
    pub fn px_i(self, points: i32) -> i32 {
        self.px(f64::from(points)).round() as i32
    }

    /// A size in device pixels as points, rounded as the host rounds (`nhwl_to_logical`).
    pub fn pt_i(self, px: i32) -> i32 {
        self.pt(f64::from(px)).round() as i32
    }

    /// A size in points as device pixels, at least one.
    pub fn px_min1(self, points: i32) -> u32 {
        self.px(f64::from(points)).round().max(1.0) as u32
    }

    /// A size in device pixels as the points that hold all of it.
    pub fn pt_ceil(self, px: u32) -> i32 {
        self.pt(f64::from(px)).ceil() as i32
    }

    /// A point in a view (points, y up from its bottom left; the view `view_height` points tall) as the host's
    /// position: device pixels from its top left.
    pub fn device_point(self, x: f64, y: f64, view_height: f64) -> (f64, f64) {
        (self.px(x), self.px(view_height - y))
    }

    /// The host's position (device pixels from the view's top left) as a point in the view (points from its bottom
    /// left).
    pub fn view_point(self, x: f64, y: f64, view_height: f64) -> (f64, f64) {
        (self.pt(x), view_height - self.pt(y))
    }

    /// A rectangle in device pixels from a view's top left (x, y, w, h) as points from its bottom left, the view
    /// `view_height` points tall.
    pub fn view_rect(
        self,
        (x, y, w, h): (f64, f64, f64, f64),
        view_height: f64,
    ) -> (f64, f64, f64, f64) {
        (
            self.pt(x),
            view_height - self.pt(y + h),
            self.pt(w),
            self.pt(h),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The host's `nhwl_to_logical` (nhwlterm.c): DEV device pixels at S120/120 as logical pixels, rounded.
    fn host_logical(dev: i32, s120: u32) -> i32 {
        let s = i64::from(s120.max(1));
        ((i64::from(dev) * 120 + s / 2) / s) as i32
    }

    const SCALES: [u32; 6] = [120, 150, 180, 210, 240, 270];

    #[test]
    fn lisp_px_are_device_px_and_points_are_appkits() {
        // a frame 1600 x 1000 Lisp pixels at 2x is a window of 800 x 500 points, and that window is the frame's
        // 1600 x 1000 pixels again
        let two = Scale::of120(240);
        assert_eq!((two.pt_i(1600), two.pt_i(1000)), (800, 500));
        assert_eq!((two.px_i(800), two.px_i(500)), (1600, 1000));
        // at 1x the two are one
        assert_eq!((Scale::ONE.pt_i(1601), Scale::ONE.px_i(1601)), (1601, 1601));
        // a point and a device pixel at 1.5x: 3 device pixels are 2 points
        let s = Scale::of_factor(1.5);
        assert_eq!(s.s120(), 180);
        assert_eq!((s.pt_i(3), s.px_i(2)), (2, 3));
    }

    #[test]
    fn device_px_as_points_round_as_the_host_does() {
        for &s120 in &SCALES {
            let s = Scale::of120(s120);
            for dev in 0..4000 {
                assert_eq!(
                    s.pt_i(dev),
                    host_logical(dev, s120),
                    "dev {dev} at {s120}/120"
                );
            }
        }
    }

    #[test]
    fn a_window_of_the_hosts_points_is_the_hosts_pixels_within_a_point() {
        for &s120 in &SCALES {
            let s = Scale::of120(s120);
            for dev in 1..4000 {
                let back = s.px_i(s.pt_i(dev));
                assert!(
                    (back - dev).abs() <= s.px_i(1) / 2 + 1,
                    "dev {dev} at {s120}/120 came back {back}"
                );
            }
        }
    }

    #[test]
    fn positions_flip_and_scale_and_come_back() {
        let s = Scale::of120(240);
        // a view 500 points tall: its point (10, 490) is 20 device pixels right, 20 down
        assert_eq!(s.device_point(10.0, 490.0, 500.0), (20.0, 20.0));
        // a view 100 points tall at 2x: its point (10, 30) is (20, 140) device pixels from the top left
        assert_eq!(s.device_point(10.0, 30.0, 100.0), (20.0, 140.0));
        assert_eq!(s.view_point(20.0, 20.0, 500.0), (10.0, 490.0));
        // a rectangle 40 x 20 device pixels at (20, 20): 20 x 10 points, its bottom at 500 - 10 - 10
        assert_eq!(
            s.view_rect((20.0, 20.0, 40.0, 20.0), 500.0),
            (10.0, 480.0, 20.0, 10.0)
        );
        // a window with no scale yet is taken as 1x, never 0 (no division by zero)
        assert_eq!(Scale::of120(0), Scale::ONE);
        assert_eq!(Scale::of_factor(0.0).s120(), 1);
    }

    /// The macOS backend reads AppKit's backing scale only in `macos/scale.rs`, and converts between points and
    /// device pixels only through [`Scale`]: no other file of it has `backingScaleFactor`, or a raw `/ scale` or
    /// `* scale`.
    #[test]
    fn the_macos_backend_converts_only_here() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/macos");
        let mut bad = Vec::new();
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") || name == "scale.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (n, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if code.contains("backingScaleFactor")
                    || code.contains("/ scale")
                    || code.contains("* scale")
                {
                    bad.push(format!("{name}:{}: {}", n + 1, line.trim()));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "scale reads or conversions outside the one layer:\n{}",
            bad.join("\n")
        );
    }
}
