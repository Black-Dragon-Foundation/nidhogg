//! Geometry: device-pixel rectangles and the logical/device unit types.

/// A rectangle in device pixels; empty when `w <= 0` or `h <= 0`.  Layout equals `GlitnirRect`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// Area in pixels (0 when empty).
    pub fn area(&self) -> u64 {
        if self.is_empty() {
            0
        } else {
            self.w as u64 * self.h as u64
        }
    }

    /// The overlap of two rectangles (empty when they do not meet).
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x as i64 + self.w as i64).min(o.x as i64 + o.w as i64);
        let y1 = (self.y as i64 + self.h as i64).min(o.y as i64 + o.h as i64);
        if x1 <= x0 as i64 || y1 <= y0 as i64 {
            return Rect::default();
        }
        Rect::new(x0, y0, (x1 - x0 as i64) as i32, (y1 - y0 as i64) as i32)
    }

    /// The smallest rectangle holding both; an empty operand is ignored.
    pub fn bounds(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x as i64 + self.w as i64).max(o.x as i64 + o.w as i64);
        let y1 = (self.y as i64 + self.h as i64).max(o.y as i64 + o.h as i64);
        Rect::new(x0, y0, (x1 - x0 as i64) as i32, (y1 - y0 as i64) as i32)
    }

    pub fn translate(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        !self.is_empty()
            && px >= self.x
            && py >= self.y
            && (px as i64) < self.x as i64 + self.w as i64
            && (py as i64) < self.y as i64 + self.h as i64
    }
}

/// A length in logical (scale-independent) units.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Logical(pub f64);

/// A length in device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Device(pub i32);

impl Logical {
    /// Device pixels at `scale`, rounded to the nearest pixel (ties away from zero).
    pub fn to_device(self, scale: f64) -> Device {
        Device((self.0 * scale).round() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_to_device_rounds() {
        assert_eq!(Logical(10.0).to_device(1.5), Device(15));
        assert_eq!(Logical(3.0).to_device(1.25), Device(4));
        assert_eq!(Logical(0.4).to_device(1.0), Device(0));
    }
}
