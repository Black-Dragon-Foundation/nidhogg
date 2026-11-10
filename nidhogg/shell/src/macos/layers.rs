//! The root layer's commit, and the watch that proves redisplay spins no run loop.
//!
//! **The commit** (design §5.4): one explicit `CATransaction` per root tree with actions disabled, the tree's
//! surfaces set as their layers' contents (nil first when a layer gets the surface it already shows), then
//! `[CATransaction flush]`, so the frame shows without the run loop turning: without the flush an explicit
//! transaction nested in an implicit one waits for the run loop.  Gravity is top-left and `contentsScale` the
//! window's, so a picture is never stretched.  Every layer change the shell makes outside a commit marks an implicit
//! transaction open; the commit's flush ends it, and one still open when a publish ends is counted.
//!
//! **The watch:** a run-loop observer counts the passes the main run loop makes outside the two pumps
//! (`nhg_dispatch_main` and `nhg_wait`, which mark themselves through [`Pumping`]).  Redisplay runs outside them, so
//! every pass it causes is one; `:spins-in-redisplay` must stay 0.

use super::surface::{Cf, STATS};
use objc2::runtime::AnyObject;
use objc2_quartz_core::{CALayer, CATransaction, kCAGravityTopLeft};
use std::cell::Cell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};

thread_local! {
    /// How deep the main thread is in the pumps.
    static PUMPING: Cell<u32> = const { Cell::new(0) };
    /// A layer was changed outside a commit since the last flush.
    static IMPLICIT: Cell<bool> = const { Cell::new(false) };
}

/// Held while a pump runs: run-loop passes inside are the pump's own.
pub struct Pumping;

impl Pumping {
    pub fn enter() -> Pumping {
        PUMPING.with(|p| p.set(p.get() + 1));
        Pumping
    }
}

impl Drop for Pumping {
    fn drop(&mut self) {
        PUMPING.with(|p| p.set(p.get().saturating_sub(1)));
    }
}

/// A layer property changed outside a commit (an implicit transaction is open until the next flush).
pub fn changed_outside() {
    IMPLICIT.with(|i| i.set(true));
}

/// One root tree's commit: what `f` sets, in one flushed transaction with actions disabled.
pub fn transaction<R>(f: impl FnOnce() -> R) -> R {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    let r = f();
    {
        let _t = super::surface::Timed::start(&super::surface::STATS.commit_ns);
        CATransaction::commit();
        CATransaction::flush();
    }
    IMPLICIT.with(|i| i.set(false));
    if STATS.commits.fetch_add(1, Ordering::Relaxed) == 0 {
        crate::trace::note(|| "first commit".into());
    }
    r
}

/// Show `surface` on `layer` at `scale`, inside a [`transaction`]: nil first when the layer already shows it (`same`).
pub fn show(layer: &CALayer, surface: &Cf, same: bool, scale: f64) {
    // SAFETY: an IOSurfaceRef is toll-free bridged to IOSurface, an object `contents` takes.
    let obj = unsafe { &*surface.as_ptr().cast::<AnyObject>() };
    // SAFETY: `contents` takes nil or an IOSurface.
    unsafe {
        if same {
            layer.setContents(None);
        }
        layer.setContents(Some(obj));
        layer.setContentsGravity(kCAGravityTopLeft);
    }
    layer.setContentsScale(scale);
    PROPS.contents.fetch_add(1, Ordering::Relaxed);
}

/// The layers' property changes since start, by kind (`(nhmac-stats)`'s `:props-*`): a move must change the
/// position alone.
pub struct Props {
    pub contents: AtomicU64,
    pub position: AtomicU64,
    pub bounds: AtomicU64,
    pub hidden: AtomicU64,
    pub opacity: AtomicU64,
    pub order: AtomicU64,
    pub host: AtomicU64,
    /// Children moved into a panel or back, and the panels ordered in or out.
    pub switches: AtomicU64,
    pub panel_orders: AtomicU64,
    /// Chrome: the corner and border properties set, and the shadow images rendered.
    pub chrome: AtomicU64,
    pub shadow_renders: AtomicU64,
    /// Show and hide legs handed to Core Animation.
    pub legs: AtomicU64,
}

pub static PROPS: Props = Props {
    contents: AtomicU64::new(0),
    position: AtomicU64::new(0),
    bounds: AtomicU64::new(0),
    hidden: AtomicU64::new(0),
    opacity: AtomicU64::new(0),
    order: AtomicU64::new(0),
    host: AtomicU64::new(0),
    switches: AtomicU64::new(0),
    panel_orders: AtomicU64::new(0),
    chrome: AtomicU64::new(0),
    shadow_renders: AtomicU64::new(0),
    legs: AtomicU64::new(0),
};

impl Props {
    pub fn text(&self) -> String {
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        format!(
            ":props-contents {} :props-position {} :props-bounds {} :props-hidden {} :props-opacity {} :props-order {} \
             :props-host {} :overhang-switches {} :panel-orders {} :props-chrome {} :shadow-renders {} \
             :motion-legs {}",
            g(&self.contents),
            g(&self.position),
            g(&self.bounds),
            g(&self.hidden),
            g(&self.opacity),
            g(&self.order),
            g(&self.host),
            g(&self.switches),
            g(&self.panel_orders),
            g(&self.chrome),
            g(&self.shadow_renders),
            g(&self.legs),
        )
    }
}

/// Count one change of kind `c`.
pub fn count(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

/// A publish ended: count an implicit transaction the shell left open.
pub fn publish_end() {
    if IMPLICIT.with(|i| i.replace(false)) {
        STATS.implicit_open.fetch_add(1, Ordering::Relaxed);
        CATransaction::flush();
    }
}

type CFRef = *mut c_void;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: CFRef;
    fn CFRunLoopGetMain() -> CFRef;
    fn CFRunLoopObserverCreate(
        alloc: CFRef,
        activities: usize,
        repeats: u8,
        order: isize,
        callout: extern "C" fn(CFRef, usize, *mut c_void),
        ctx: *mut c_void,
    ) -> CFRef;
    fn CFRunLoopAddObserver(rl: CFRef, observer: CFRef, mode: CFRef);
}

/// `kCFRunLoopBeforeSources`: once per pass.
const BEFORE_SOURCES: usize = 1 << 2;

extern "C" fn pass(_o: CFRef, _activity: usize, _info: *mut c_void) {
    if PUMPING.with(Cell::get) == 0 {
        STATS.spins_in_redisplay.fetch_add(1, Ordering::Relaxed);
    }
}

/// Install the watch on the main run loop (main thread, once at open).
pub fn watch_spins() {
    // SAFETY: CF calls on the main run loop; the observer lives as long as the process.
    unsafe {
        let o = CFRunLoopObserverCreate(
            std::ptr::null_mut(),
            BEFORE_SOURCES,
            1,
            0,
            pass,
            std::ptr::null_mut(),
        );
        if !o.is_null() {
            CFRunLoopAddObserver(CFRunLoopGetMain(), o, kCFRunLoopCommonModes);
        }
    }
}
