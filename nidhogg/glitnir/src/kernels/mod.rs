//! Compose kernels on premultiplied ARGB32 pixels (Cairo's native-endian `0xAARRGGBB`).
//!
//! Copy and OVER do not depend on the colour channels' order; only the alpha must be the top byte. Every kernel
//! works on rows of a strided view, so it writes into any sub-rectangle of a buffer in place. Each row kernel has
//! a baseline build and, on x86-64, an AVX2 build of the same body, chosen once at start-up; on aarch64 NEON is part
//! of the baseline, so the baseline build is the vector build there. OVER is pixman's exact `UN8x4_MUL_UN8`
//! arithmetic (x·a/255 rounded), bit for bit equal to `pixman_image_composite32` for premultiplied input.

mod copy;
mod corner;
mod over;
mod ring;
#[cfg(test)]
mod tests;

pub use corner::corner_coverage;
pub use ring::{RingCoverage, clamp_radius, decor_tile, ring_coverage};

use std::sync::{Mutex, MutexGuard, OnceLock};

/// Pixels to read: row `r` starts at `data[r * stride]`.
pub struct Pixels<'a> {
    pub data: &'a [u32],
    pub stride: usize,
}

/// Pixels to write: row `r` starts at `data[r * stride]`.
pub struct PixelsMut<'a> {
    pub data: &'a mut [u32],
    pub stride: usize,
}

/// 8-bit coverage: row `r` starts at `data[r * stride]`.
pub struct Mask<'a> {
    pub data: &'a [u8],
    pub stride: usize,
}

/// One build of the row kernels. Get the machine's with [`kernels`].
pub struct Kernels {
    /// "baseline" or "avx2".
    pub name: &'static str,
    copy_row: fn(&mut [u32], &[u32]),
    over_const_row: fn(&mut [u32], &[u32], u8),
    over_row: fn(&mut [u32], &[u32]),
    mask_over_row: fn(&mut [u32], &[u32], &[u8], u8),
    solid_over_row: fn(&mut [u32], u32, &[u8]),
}

static BASELINE: Kernels = Kernels {
    name: "baseline",
    copy_row: copy::copy_base,
    over_const_row: over::over_const_base,
    over_row: over::over_base,
    mask_over_row: over::mask_over_base,
    solid_over_row: over::solid_over_base,
};

#[cfg(target_arch = "x86_64")]
static AVX2: Kernels = Kernels {
    name: "avx2",
    copy_row: copy::copy_avx2,
    over_const_row: over::over_const_avx2,
    over_row: over::over_avx2,
    mask_over_row: over::mask_over_avx2,
    solid_over_row: over::solid_over_avx2,
};

/// The kernels for this machine, chosen once: AVX2 where the CPU has it, else the baseline.
pub fn kernels() -> &'static Kernels {
    static CHOSEN: OnceLock<&'static Kernels> = OnceLock::new();
    CHOSEN.get_or_init(|| avx2().unwrap_or(&BASELINE))
}

/// The baseline build (every machine).
pub fn baseline() -> &'static Kernels {
    &BASELINE
}

/// The AVX2 build, when this is an x86-64 CPU that has AVX2.
pub fn avx2() -> Option<&'static Kernels> {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") {
        return Some(&AVX2);
    }
    None
}

/// Two entry points for one row kernel's body: the baseline build and, on x86-64, an AVX2 build of the same code.
macro_rules! row_kernel {
    ($base:ident, $avx2:ident, $body:path, ($($a:ident: $t:ty),*)) => {
        pub(super) fn $base($($a: $t),*) {
            $body($($a),*)
        }

        #[cfg(target_arch = "x86_64")]
        pub(super) fn $avx2($($a: $t),*) {
            #[target_feature(enable = "avx2")]
            fn inner($($a: $t),*) {
                $body($($a),*)
            }
            // SAFETY: the AVX2 table is handed out only after `is_x86_feature_detected!("avx2")` (`avx2`).
            unsafe { inner($($a),*) }
        }
    };
}
pub(crate) use row_kernel;

impl Pixels<'_> {
    fn row(&self, r: usize, w: usize) -> &[u32] {
        &self.data[r * self.stride..][..w]
    }
}

impl PixelsMut<'_> {
    fn row(&mut self, r: usize, w: usize) -> &mut [u32] {
        &mut self.data[r * self.stride..][..w]
    }
}

impl Mask<'_> {
    fn row(&self, r: usize, w: usize) -> &[u8] {
        &self.data[r * self.stride..][..w]
    }
}

impl Kernels {
    /// `dst` = `src` over `w`×`h` pixels.
    pub fn copy(&self, dst: &mut PixelsMut, src: &Pixels, w: usize, h: usize) {
        if w == 0 {
            return;
        }
        for r in 0..h {
            (self.copy_row)(dst.row(r, w), src.row(r, w));
        }
    }

    /// `dst` = `src`·alpha OVER `dst` (a layer's opacity).
    pub fn over_const(&self, dst: &mut PixelsMut, src: &Pixels, w: usize, h: usize, alpha: u8) {
        match alpha {
            0 => {}
            // x·255/255 is exact, so this is the same arithmetic, cheaper
            255 => self.over(dst, src, w, h),
            _ if w == 0 => {}
            _ => {
                for r in 0..h {
                    (self.over_const_row)(dst.row(r, w), src.row(r, w), alpha);
                }
            }
        }
    }

    /// `dst` = `src` OVER `dst` (a translucent source).
    pub fn over(&self, dst: &mut PixelsMut, src: &Pixels, w: usize, h: usize) {
        if w == 0 {
            return;
        }
        for r in 0..h {
            (self.over_row)(dst.row(r, w), src.row(r, w));
        }
    }

    /// `dst` = `src`·(mask·alpha) OVER `dst` (a layer's corners: coverage times opacity).
    pub fn mask_over(
        &self,
        dst: &mut PixelsMut,
        src: &Pixels,
        mask: &Mask,
        w: usize,
        h: usize,
        alpha: u8,
    ) {
        if w == 0 || alpha == 0 {
            return;
        }
        for r in 0..h {
            (self.mask_over_row)(dst.row(r, w), src.row(r, w), mask.row(r, w), alpha);
        }
    }

    /// `dst` = `colour`·mask OVER `dst`; `colour` is premultiplied (shadows, the border ring).
    pub fn solid_over(&self, dst: &mut PixelsMut, colour: u32, mask: &Mask, w: usize, h: usize) {
        if w == 0 || colour == 0 {
            return;
        }
        for r in 0..h {
            (self.solid_over_row)(dst.row(r, w), colour, mask.row(r, w));
        }
    }
}

/// A small cache that keeps the `cap` most recently used values.
pub(crate) struct Lru<K, V> {
    cap: usize,
    items: Vec<(K, V)>,
}

impl<K: PartialEq, V: Clone> Lru<K, V> {
    pub(crate) const fn new(cap: usize) -> Lru<K, V> {
        Lru {
            cap,
            items: Vec::new(),
        }
    }

    /// The value for `key`, made with `make` on a miss; the least recently used entry goes when full.
    pub(crate) fn get_or_insert_with(&mut self, key: K, make: impl FnOnce() -> V) -> V {
        if let Some(i) = self.items.iter().position(|(k, _)| *k == key) {
            let hit = self.items.remove(i);
            let v = hit.1.clone();
            self.items.push(hit);
            return v;
        }
        if self.items.len() >= self.cap {
            self.items.remove(0);
        }
        let v = make();
        self.items.push((key, v.clone()));
        v
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }
}

/// Lock a cache; a panic while it was held leaves nothing half-written (values are inserted whole), so go on.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
