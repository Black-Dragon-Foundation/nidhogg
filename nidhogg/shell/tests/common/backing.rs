//! A host-side surface for the publishing tests: a backing (premultiplied ARGB32, as Cairo keeps it) and, unless
//! `bare`, a Glitnir scene with one frame layer over it, drawn on and damaged the way the glue does.
#![allow(dead_code)]

use glitnir::geom::Rect;
use glitnir::scene::LayerDesc;
use nhshell::api::*;
use nhshell::wayland::WaylandShell;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Backing {
    pub g: u64,
    pub w: i32,
    pub h: i32,
    pub px: Vec<u8>,
    pub src: (i32, i32),
    pub flags: u32,
    pub alpha_cut: u32,
    scene: bool,
}

/// A Glitnir id no other test of this process uses.
pub fn glitnir_id() -> u64 {
    static N: AtomicU64 = AtomicU64::new(0x7100_0000);
    N.fetch_add(1, Ordering::SeqCst)
}

impl Backing {
    /// A `w` x `h` backing with a scene: one frame layer over all of it.
    pub fn new(w: i32, h: i32) -> Backing {
        let mut b = Backing::bare(w, h);
        b.scene = true;
        glitnir::scene::root_new(b.g);
        b.layer();
        b
    }

    /// A backing without a scene: publishing commits its damage alone, and the presenter copies it.
    pub fn bare(w: i32, h: i32) -> Backing {
        Backing {
            g: glitnir_id(),
            w,
            h,
            px: vec![0; (w * h * 4) as usize],
            src: (0, 0),
            flags: NHSHELL_SF_DRAWABLE,
            alpha_cut: 0,
            scene: false,
        }
    }

    fn layer(&self) {
        let d = LayerDesc {
            id: self.g,
            parent: 0,
            kind: 1,
            rect: Rect::new(0, 0, self.w, self.h),
            scale: 1.0,
            visible: 1,
        };
        glitnir::scene::with(self.g, |s| s.upsert(&d)).expect("the frame layer");
    }

    /// Paint `r` with `argb` and damage it.
    pub fn fill(&mut self, r: Rect, argb: u32) {
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                let at = ((y * self.w + x) * 4) as usize;
                self.px[at..at + 4].copy_from_slice(&argb.to_ne_bytes());
            }
        }
        glitnir::damage::with(self.g, |d| d.add(r));
    }

    /// Paint all of it.
    pub fn fill_all(&mut self, argb: u32) {
        self.fill(Rect::new(0, 0, self.w, self.h), argb);
    }

    pub fn pixel(&self, x: i32, y: i32) -> u32 {
        let at = ((y * self.w + x) * 4) as usize;
        u32::from_ne_bytes(self.px[at..at + 4].try_into().unwrap())
    }

    /// A new size: the pixels are undefined (zero) until drawn, and the frame layer follows.
    pub fn resize(&mut self, w: i32, h: i32) {
        (self.w, self.h) = (w, h);
        self.px = vec![0; (w * h * 4) as usize];
        if self.scene {
            self.layer();
        }
    }

    pub fn frame(&self, window: u64) -> SurfaceFrame<'_> {
        SurfaceFrame {
            window,
            glitnir_root: self.g,
            pixels: &self.px,
            w: self.w,
            h: self.h,
            stride: self.w * 4,
            src_x: self.src.0,
            src_y: self.src.1,
            flags: self.flags,
            alpha_cut: self.alpha_cut,
        }
    }
}

/// Publish a tree: `(backing, window)` pairs, the root first.
pub fn publish(
    shell: &mut WaylandShell,
    root: u64,
    parts: &[(&Backing, u64)],
    t_key_ns: u64,
) -> bool {
    let frames: Vec<SurfaceFrame> = parts.iter().map(|(b, w)| b.frame(*w)).collect();
    shell.publish(root, &frames, t_key_ns).unwrap()
}
