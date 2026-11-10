//! A window's surfaces: the IOSurfaces of its ring (`crate::ring`), BGRA and premultiplied as the glue's backing is,
//! tagged with the colour space `ns-use-srgb-colorspace` names (sRGB, else Generic RGB, as `nsterm.m:11130-11138`).
//!
//! A publish copies into the picked surface only what it lacks ([`crate::ring::Fill`]), from the frame's backing,
//! between `IOSurfaceLock` and `IOSurfaceUnlock`; `macos::layers` then sets it as the layer's contents.  A surface of
//! another size than the backing is never shown: a new size starts the ring over.  The counters are
//! `(nhmac-stats)`'s.
//!
//! IOSurface, Core Foundation and Core Graphics are reached through their C functions, declared here as `kq.rs`
//! declares its own, so the shell takes no further crates.

use crate::api::SurfaceFrame;
use crate::ring::{Fill, Paced, Pacer, Pick, Ring};
use glitnir::geom::Rect;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};

/// The ring's size: S2's fallback, so that a surface the window server still holds is not drawn into while typing.
pub const RING: usize = 3;

/// `'BGRA'`: the backing's premultiplied ARGB32 words as they lie in memory.
const BGRA: i32 = 0x4247_5241;

type CFRef = *const c_void;

/// `kCFNumberSInt32Type`.
const CF_SINT32: isize = 3;
/// `kIOSurfaceLockReadOnly`.
const LOCK_READ_ONLY: u32 = 1;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    fn CFDictionaryCreate(
        alloc: CFRef,
        keys: *const CFRef,
        values: *const CFRef,
        n: isize,
        key_cb: *const c_void,
        value_cb: *const c_void,
    ) -> CFRef;
    fn CFNumberCreate(alloc: CFRef, kind: isize, value: *const c_void) -> CFRef;
    fn CFEqual(a: CFRef, b: CFRef) -> u8;
    fn CFRelease(r: CFRef);
}

#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    static kIOSurfaceWidth: CFRef;
    static kIOSurfaceHeight: CFRef;
    static kIOSurfaceBytesPerElement: CFRef;
    static kIOSurfaceBytesPerRow: CFRef;
    static kIOSurfacePixelFormat: CFRef;
    static kIOSurfaceColorSpace: CFRef;
    fn IOSurfaceCreate(props: CFRef) -> CFRef;
    fn IOSurfaceLock(s: CFRef, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceUnlock(s: CFRef, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceGetBaseAddress(s: CFRef) -> *mut c_void;
    fn IOSurfaceGetBytesPerRow(s: CFRef) -> usize;
    fn IOSurfaceGetAllocSize(s: CFRef) -> usize;
    fn IOSurfaceIsInUse(s: CFRef) -> u8;
    fn IOSurfaceSetValue(s: CFRef, key: CFRef, value: CFRef);
    fn IOSurfaceCopyValue(s: CFRef, key: CFRef) -> CFRef;
    fn IOSurfaceAlignProperty(key: CFRef, value: usize) -> usize;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: CFRef;
    static kCGColorSpaceGenericRGB: CFRef;
    fn CGColorSpaceCreateWithName(name: CFRef) -> CFRef;
    fn CGColorSpaceCopyPropertyList(cs: CFRef) -> CFRef;
}

/// An owned Core Foundation object (one retain), released on drop.
#[derive(Debug)]
pub struct Cf(CFRef);

impl Cf {
    /// Take a +1 reference; `None` for null.
    fn take(r: CFRef) -> Option<Cf> {
        (!r.is_null()).then_some(Cf(r))
    }

    pub fn as_ptr(&self) -> CFRef {
        self.0
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: the one reference this value owns.
        unsafe { CFRelease(self.0) };
    }
}

/// The surfaces' counters since start.
#[derive(Debug, Default)]
pub struct Stats {
    pub bytes_copied: AtomicU64,
    pub damage_bytes: AtomicU64,
    pub full_copies: AtomicU64,
    pub commits: AtomicU64,
    pub surface_busy: AtomicU64,
    pub ring_acquires: AtomicU64,
    pub implicit_open: AtomicU64,
    pub spins_in_redisplay: AtomicU64,
    pub pumps: AtomicU64,
    pub deferred_commits: AtomicU64,
    /// `nhg_dispatch_main` calls from a Lisp thread, handed to the main thread's wait.
    pub handed_pumps: AtomicU64,
    /// Publishes held by the pacer (every other slot held), and holds that outlasted its bound.
    pub held: AtomicU64,
    pub held_timeouts: AtomicU64,
    /// The pacer timer's fires, and how often it was armed.
    pub pacer_fires: AtomicU64,
    pub pacer_arms: AtomicU64,
    /// Where a redisplay's time goes on the shell's side, in nanoseconds since start: painting draw records,
    /// filling the surfaces from the backing (`publish`'s draws), and the layer transactions' commit and flush.
    pub paint_ns: AtomicU64,
    pub fill_ns: AtomicU64,
    pub commit_ns: AtomicU64,
    /// Glyphs whose advance and bounds were asked of Core Text (each open font asks once per glyph).
    pub glyph_asks: AtomicU64,
}

/// Adds the time from its making to its drop to a counter of nanoseconds.
pub struct Timed(&'static AtomicU64, std::time::Instant);

impl Timed {
    pub fn start(c: &'static AtomicU64) -> Timed {
        Timed(c, std::time::Instant::now())
    }
}

impl Drop for Timed {
    fn drop(&mut self) {
        let ns = u64::try_from(self.1.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.0.fetch_add(ns, Ordering::Relaxed);
    }
}

pub static STATS: Stats = Stats {
    bytes_copied: AtomicU64::new(0),
    damage_bytes: AtomicU64::new(0),
    full_copies: AtomicU64::new(0),
    commits: AtomicU64::new(0),
    surface_busy: AtomicU64::new(0),
    ring_acquires: AtomicU64::new(0),
    implicit_open: AtomicU64::new(0),
    spins_in_redisplay: AtomicU64::new(0),
    pumps: AtomicU64::new(0),
    deferred_commits: AtomicU64::new(0),
    handed_pumps: AtomicU64::new(0),
    held: AtomicU64::new(0),
    held_timeouts: AtomicU64::new(0),
    pacer_fires: AtomicU64::new(0),
    pacer_arms: AtomicU64::new(0),
    paint_ns: AtomicU64::new(0),
    fill_ns: AtomicU64::new(0),
    commit_ns: AtomicU64::new(0),
    glyph_asks: AtomicU64::new(0),
};

impl Stats {
    /// The counters as `(nhmac-stats)` lists them.
    pub fn text(&self) -> String {
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        format!(
            ":bytes-copied {} :damage-bytes {} :full-copies {} :commits {} :surface-busy {} :ring-acquires {} \
             :implicit-open-at-update-end {} :spins-in-redisplay {} :pumps {} :deferred-commits {} :handed-pumps {} :draws {} \
             :held {} :held-timeouts {} :pacer-fires {} :pacer-arms {} :paint-ns {} :fill-ns {} :commit-ns {} :glyph-asks {}",
            g(&self.bytes_copied),
            g(&self.damage_bytes),
            g(&self.full_copies),
            g(&self.commits),
            g(&self.surface_busy),
            g(&self.ring_acquires),
            g(&self.implicit_open),
            g(&self.spins_in_redisplay),
            g(&self.pumps),
            g(&self.deferred_commits),
            g(&self.handed_pumps),
            super::draw::DRAWS.load(Ordering::Relaxed),
            g(&self.held),
            g(&self.held_timeouts),
            g(&self.pacer_fires),
            g(&self.pacer_arms),
            g(&self.paint_ns),
            g(&self.fill_ns),
            g(&self.commit_ns),
            g(&self.glyph_asks),
        )
    }
}

/// The property list of the colour space a surface is tagged with.
fn colour_space(srgb: bool) -> Option<Cf> {
    // SAFETY: Core Graphics' name constants and calls; each result is a +1 reference we own.
    unsafe {
        let cs = Cf::take(CGColorSpaceCreateWithName(if srgb {
            kCGColorSpaceSRGB
        } else {
            kCGColorSpaceGenericRGB
        }))?;
        Cf::take(CGColorSpaceCopyPropertyList(cs.as_ptr()))
    }
}

/// A new `w` x `h` surface, tagged.  `None` when IOSurface refuses.
fn make(w: i32, h: i32, srgb: bool) -> Option<Cf> {
    // SAFETY: IOSurface's key constants; the numbers and the dictionary are +1 references we own, and the
    // dictionary retains what it holds.
    unsafe {
        let row = IOSurfaceAlignProperty(kIOSurfaceBytesPerRow, w as usize * 4);
        let row = i32::try_from(row).ok()?;
        let keys = [
            kIOSurfaceWidth,
            kIOSurfaceHeight,
            kIOSurfaceBytesPerElement,
            kIOSurfaceBytesPerRow,
            kIOSurfacePixelFormat,
        ];
        let nums: Vec<Cf> = [w, h, 4, row, BGRA]
            .iter()
            .map(|v| {
                Cf::take(CFNumberCreate(
                    std::ptr::null(),
                    CF_SINT32,
                    std::ptr::from_ref(v).cast(),
                ))
            })
            .collect::<Option<_>>()?;
        let vals: Vec<CFRef> = nums.iter().map(Cf::as_ptr).collect();
        let props = Cf::take(CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            vals.as_ptr(),
            keys.len() as isize,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        ))?;
        let s = Cf::take(IOSurfaceCreate(props.as_ptr()))?;
        if let Some(cs) = colour_space(srgb) {
            IOSurfaceSetValue(s.as_ptr(), kIOSurfaceColorSpace, cs.as_ptr());
        }
        Some(s)
    }
}

/// Copy `fill` of `f`'s layer from its backing into `s` (`w` x `h`).  Writes pixels.
fn copy(s: &Cf, fill: &Fill, f: &SurfaceFrame, w: i32, h: i32) {
    let all = [Rect::new(0, 0, w, h)];
    let rects: &[Rect] = match fill {
        Fill::All => &all,
        Fill::Rects(r) => r,
    };
    // SAFETY: a surface we made, locked for writing for the copy (a null seed is allowed); every row is checked
    // against both buffers before it is copied, and a surface and a backing never overlap.
    unsafe {
        if IOSurfaceLock(s.as_ptr(), 0, std::ptr::null_mut()) != 0 {
            return;
        }
        let base = IOSurfaceGetBaseAddress(s.as_ptr()).cast::<u8>();
        let (dst_stride, size) = (
            IOSurfaceGetBytesPerRow(s.as_ptr()),
            IOSurfaceGetAllocSize(s.as_ptr()),
        );
        let src_stride = f.stride as usize;
        for r in rects {
            let n = r.w as usize * 4;
            for y in r.y..r.y + r.h {
                let from = (y + f.src_y) as usize * src_stride + (r.x + f.src_x) as usize * 4;
                let to = y as usize * dst_stride + r.x as usize * 4;
                if from + n <= f.pixels.len() && to + n <= size {
                    std::ptr::copy_nonoverlapping(f.pixels.as_ptr().add(from), base.add(to), n);
                }
            }
        }
        IOSurfaceUnlock(s.as_ptr(), 0, std::ptr::null_mut());
    }
}

/// One window's ring of surfaces.
pub struct Surfaces {
    ring: Ring,
    slots: Vec<Cf>,
    w: i32,
    h: i32,
    srgb: bool,
    /// The last commit drew into the surface the layer already showed.
    same: bool,
    /// Commits held while the window server holds every other slot.
    pacer: Pacer,
    /// Test hook: every slot reads as held by the window server.
    #[cfg(feature = "test-hooks")]
    pub force_in_use: bool,
}

impl Default for Surfaces {
    fn default() -> Self {
        Surfaces {
            ring: Ring::new(RING),
            slots: Vec::new(),
            w: 0,
            h: 0,
            srgb: true,
            same: false,
            pacer: Pacer::default(),
            #[cfg(feature = "test-hooks")]
            force_in_use: false,
        }
    }
}

/// What [`Surfaces::draw`] did with a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drew {
    /// Drawn into a slot, which the layer is to show.
    Drawn,
    /// Held: every other slot is held by the window server; the publish is owed (`Surfaces::due`).
    Held,
}

impl Surfaces {
    /// Start over: the next publish makes new surfaces, tagged by `srgb`, and copies them whole.
    pub fn reset(&mut self, srgb: bool) {
        self.ring.clear();
        self.slots.clear();
        self.pacer.clear();
        self.srgb = srgb;
    }

    /// Whether the window server holds slot `i`.
    fn in_use(&self, i: usize) -> bool {
        #[cfg(feature = "test-hooks")]
        if self.force_in_use {
            return true;
        }
        // SAFETY: a surface we made.
        unsafe { IOSurfaceIsInUse(self.slots[i].as_ptr()) != 0 }
    }

    /// Whether a commit is held.
    pub fn owes(&self) -> bool {
        self.pacer.owes()
    }

    /// Whether the held commit should be published now (`now` in ns): a slot has freed, or the hold reached its bound.
    pub fn due(&self, now: u64) -> bool {
        self.pacer.owes() && self.pacer.due(self.ring.pick(|i| self.in_use(i)).busy, now)
    }

    /// Draw commit `f` (its own `damage`, in the layer's pixels; `now` in ns) into a slot the window server does not
    /// hold, with what the slot lacks copied from the backing; or hold it, its damage kept, while every other slot is
    /// held (the pacer), past whose bound the oldest is drawn into anyway.  `None` when IOSurface refuses a surface.
    pub fn draw(&mut self, f: &SurfaceFrame, damage: &[Rect], now: u64) -> Option<Drew> {
        let (w, h) = (f.w - f.src_x, f.h - f.src_y);
        if (w, h) != (self.w, self.h) {
            self.ring.clear();
            self.slots.clear();
            self.pacer.clear();
            self.w = w;
            self.h = h;
        }
        let damage = crate::ring::disjoint(damage, w, h);
        let pick = self.ring.pick(|i| self.in_use(i));
        let st = &STATS;
        let damage = match self.pacer.pace(pick, &damage, now) {
            Paced::Hold => {
                st.held.fetch_add(1, Ordering::Relaxed);
                return Some(Drew::Held);
            }
            Paced::Draw(d) => crate::ring::disjoint(&d, w, h),
            Paced::Reuse(d) => {
                st.held_timeouts.fetch_add(1, Ordering::Relaxed);
                crate::ring::disjoint(&d, w, h)
            }
        };
        let Pick { slot, busy } = pick;
        let fill = self.ring.fill(slot, &damage, w, h);
        if slot == self.slots.len() {
            self.slots.push(make(w, h, self.srgb)?);
        }
        let st = &STATS;
        st.ring_acquires.fetch_add(1, Ordering::Relaxed);
        st.bytes_copied
            .fetch_add(fill.bytes(w, h), Ordering::Relaxed);
        st.damage_bytes
            .fetch_add(Fill::Rects(damage.clone()).bytes(w, h), Ordering::Relaxed);
        if fill == Fill::All {
            st.full_copies.fetch_add(1, Ordering::Relaxed);
        }
        if busy {
            st.surface_busy.fetch_add(1, Ordering::Relaxed);
        }
        copy(&self.slots[slot], &fill, f, w, h);
        self.same = self.ring.commit(slot, &damage);
        Some(Drew::Drawn)
    }

    /// The surface committed last.
    pub fn newest(&self) -> Option<&Cf> {
        self.ring.newest().and_then(|i| self.slots.get(i))
    }

    /// Whether the last commit's surface was already the layer's contents (set it to nil first).
    pub fn was_same(&self) -> bool {
        self.same
    }

    pub fn size(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    /// Whether the next commit would find a slot the window server does not hold (else the pacer holds it).
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub fn free(&self) -> bool {
        !self.ring.pick(|i| self.in_use(i)).busy
    }

    /// The surfaces the ring has made so far (at most `RING`).
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub fn slots(&self) -> usize {
        self.slots.len()
    }

    /// The newest surface's colour space: `Some(true)` sRGB, `Some(false)` Generic RGB, `None` untagged or none.
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub fn tagged_srgb(&self) -> Option<bool> {
        let s = self.newest()?;
        // SAFETY: a surface we made; the copied value is a +1 reference we own.
        let v = Cf::take(unsafe { IOSurfaceCopyValue(s.as_ptr(), kIOSurfaceColorSpace) })?;
        // SAFETY: two live CF objects.
        let is = |srgb: bool| {
            colour_space(srgb).is_some_and(|c| unsafe { CFEqual(c.as_ptr(), v.as_ptr()) } != 0)
        };
        if is(true) {
            Some(true)
        } else if is(false) {
            Some(false)
        } else {
            None
        }
    }

    /// The newest surface's pixel at `x`, `y` as premultiplied ARGB (`None` outside it).  Reads pixels.
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub fn pixel(&self, x: i32, y: i32) -> Option<u32> {
        let s = self.newest()?;
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return None;
        }
        let mut px = [0u8; 4];
        // SAFETY: a surface we made, locked read-only for one read inside it (`x`, `y` were checked against its size).
        unsafe {
            if IOSurfaceLock(s.as_ptr(), LOCK_READ_ONLY, std::ptr::null_mut()) != 0 {
                return None;
            }
            let at = y as usize * IOSurfaceGetBytesPerRow(s.as_ptr()) + x as usize * 4;
            let base = IOSurfaceGetBaseAddress(s.as_ptr()).cast::<u8>();
            std::ptr::copy_nonoverlapping(base.add(at), px.as_mut_ptr(), 4);
            IOSurfaceUnlock(s.as_ptr(), LOCK_READ_ONLY, std::ptr::null_mut());
        }
        Some(u32::from_ne_bytes(px))
    }

    /// The newest surface's `w` x `h` pixels from `x`, `y`, row by row, as premultiplied ARGB (`None` when the
    /// rectangle is not inside it).  Reads pixels.
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub fn region(&self, x: i32, y: i32, w: i32, h: i32) -> Option<Vec<u32>> {
        let s = self.newest()?;
        if x < 0 || y < 0 || w < 0 || h < 0 || x + w > self.w || y + h > self.h {
            return None;
        }
        let mut out = Vec::with_capacity(w as usize * h as usize);
        // SAFETY: a surface we made, locked read-only for the reads, all inside it (checked above).
        unsafe {
            if IOSurfaceLock(s.as_ptr(), LOCK_READ_ONLY, std::ptr::null_mut()) != 0 {
                return None;
            }
            let stride = IOSurfaceGetBytesPerRow(s.as_ptr());
            let base = IOSurfaceGetBaseAddress(s.as_ptr()).cast::<u8>();
            for row in y..y + h {
                for col in x..x + w {
                    let mut px = [0u8; 4];
                    let at = row as usize * stride + col as usize * 4;
                    std::ptr::copy_nonoverlapping(base.add(at), px.as_mut_ptr(), 4);
                    out.push(u32::from_ne_bytes(px));
                }
            }
            IOSurfaceUnlock(s.as_ptr(), LOCK_READ_ONLY, std::ptr::null_mut());
        }
        Some(out)
    }
}
