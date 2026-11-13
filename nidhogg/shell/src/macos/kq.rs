//! The main wait's primitives on macOS (`crate::wait`): one kqueue for Emacs's fds, watched through a
//! `CFFileDescriptor` run-loop source; a kick source any thread signals; the sleep, which runs the run loop until an
//! `NSEvent`; and the helper thread that `select`s on the fds kqueue refuses (`EINVAL`: some `/dev` files).
//!
//! Every wake reason reaches the sleep as an `NSEvent`: the kqueue's callback and the kick's perform (both on the main
//! thread, inside the run loop) post an application-defined marker, so `nextEventMatchingMask:` returns for them as
//! for a key.  A marker carries the sleep's generation; one left over from an earlier sleep is dropped unread.

use crate::wait::{self, Change, FILT_READ, FILT_WRITE, Reg, Woke};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint};
use std::cell::Cell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

type CFRef = *mut c_void;

#[repr(C)]
struct CFFileDescriptorContext {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

#[repr(C)]
struct CFRunLoopSourceContext {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
    equal: *const c_void,
    hash: *const c_void,
    schedule: *const c_void,
    cancel: *const c_void,
    perform: Option<extern "C" fn(*const c_void)>,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: CFRef;
    fn CFRunLoopGetMain() -> CFRef;
    fn CFRunLoopAddSource(rl: CFRef, source: CFRef, mode: CFRef);
    fn CFRunLoopWakeUp(rl: CFRef);
    fn CFRunLoopSourceCreate(alloc: CFRef, order: isize, ctx: *mut CFRunLoopSourceContext)
    -> CFRef;
    fn CFRunLoopSourceSignal(source: CFRef);
    fn CFFileDescriptorCreate(
        alloc: CFRef,
        fd: i32,
        close_on_invalidate: u8,
        callout: extern "C" fn(CFRef, usize, *mut c_void),
        ctx: *const CFFileDescriptorContext,
    ) -> CFRef;
    fn CFFileDescriptorEnableCallBacks(f: CFRef, types: usize);
    fn CFFileDescriptorCreateRunLoopSource(alloc: CFRef, f: CFRef, order: isize) -> CFRef;
}

/// `kCFFileDescriptorReadCallBack`.
const CFFD_READ: usize = 1;
/// The markers' `subtype` ('nh'), and their kinds (`data1`).
const MARK: i16 = 0x6e68;
const MARK_KQ: isize = 1;
const MARK_KICK: isize = 2;

/// The kick source and the main run loop, for [`kick`] from any thread.
static KICK_SRC: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static MAIN_RL: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

thread_local! {
    /// The current sleep's generation (main thread).
    static GEN: Cell<isize> = const { Cell::new(0) };
}

/// Post a marker of `kind` for the current sleep (main thread: the callbacks run inside the run loop).
fn post(kind: isize) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let ev = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags(0),
        0.0,
        0,
        None,
        MARK,
        kind,
        GEN.with(Cell::get),
    );
    if let Some(ev) = ev {
        NSApplication::sharedApplication(mtm).postEvent_atStart(&ev, true);
    }
}

extern "C" fn kq_ready(_f: CFRef, _types: usize, _info: *mut c_void) {
    post(MARK_KQ);
}

extern "C" fn kick_perform(_info: *const c_void) {
    post(MARK_KICK);
}

/// Any thread: end the main thread's sleep (a signalled source stays signalled until the run loop serves it).
pub fn kick() {
    let (src, rl) = (
        KICK_SRC.load(Ordering::Acquire),
        MAIN_RL.load(Ordering::Acquire),
    );
    if !src.is_null() && !rl.is_null() {
        // SAFETY: both were made at open and are never released; CFRunLoopSourceSignal and CFRunLoopWakeUp are
        // thread-safe.
        unsafe {
            CFRunLoopSourceSignal(src);
            CFRunLoopWakeUp(rl);
        }
    }
}

/// Whether `ev` is one of the wait's markers (a pump drops them).
pub fn is_marker(ev: &NSEvent) -> bool {
    ev.r#type() == NSEventType::ApplicationDefined && ev.subtype().0 == MARK
}

/// The kqueue and its source.
pub struct Kq {
    fd: i32,
    cffd: CFRef,
    helper: Option<Helper>,
}

impl Kq {
    /// Make the kqueue and the sources, on the main thread.  `None` when kqueue is refused.
    pub fn open(_mtm: MainThreadMarker) -> Option<Kq> {
        // SAFETY: plain syscalls; the CF objects made here live for the process (the backend is opened once).
        unsafe {
            let fd = libc::kqueue();
            if fd < 0 {
                return None;
            }
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            let ctx = CFFileDescriptorContext {
                version: 0,
                info: std::ptr::null_mut(),
                retain: std::ptr::null(),
                release: std::ptr::null(),
                copy_description: std::ptr::null(),
            };
            let cffd = CFFileDescriptorCreate(std::ptr::null_mut(), fd, 0, kq_ready, &ctx);
            if cffd.is_null() {
                libc::close(fd);
                return None;
            }
            let rl = CFRunLoopGetMain();
            let src = CFFileDescriptorCreateRunLoopSource(std::ptr::null_mut(), cffd, 0);
            CFRunLoopAddSource(rl, src, kCFRunLoopDefaultMode);
            let mut kctx = CFRunLoopSourceContext {
                version: 0,
                info: std::ptr::null_mut(),
                retain: std::ptr::null(),
                release: std::ptr::null(),
                copy_description: std::ptr::null(),
                equal: std::ptr::null(),
                hash: std::ptr::null(),
                schedule: std::ptr::null(),
                cancel: std::ptr::null(),
                perform: Some(kick_perform),
            };
            let ksrc = CFRunLoopSourceCreate(std::ptr::null_mut(), 0, &mut kctx);
            CFRunLoopAddSource(rl, ksrc, kCFRunLoopDefaultMode);
            KICK_SRC.store(ksrc, Ordering::Release);
            MAIN_RL.store(rl, Ordering::Release);
            wait::set_kick(kick);
            Some(Kq {
                fd,
                cffd,
                helper: None,
            })
        }
    }

    /// `crate::wait::WaitOps::kq_apply`: one `kevent` call, a receipt per change.
    pub fn apply(&mut self, changes: &[Change], errs: &mut Vec<i32>) {
        errs.clear();
        // SAFETY: kevent is plain data.
        let zero: libc::kevent = unsafe { std::mem::zeroed() };
        let list: Vec<libc::kevent> = changes
            .iter()
            .map(|c| libc::kevent {
                ident: c.fd as usize,
                filter: if c.filt == FILT_WRITE {
                    libc::EVFILT_WRITE
                } else {
                    libc::EVFILT_READ
                },
                flags: libc::EV_RECEIPT | if c.add { libc::EV_ADD } else { libc::EV_DELETE },
                ..zero
            })
            .collect();
        let mut got = vec![zero; list.len()];
        let ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: both lists hold `len` entries and live for the call.
        let n = unsafe {
            libc::kevent(
                self.fd,
                list.as_ptr(),
                list.len() as i32,
                got.as_mut_ptr(),
                got.len() as i32,
                &ts,
            )
        };
        let n = usize::try_from(n).unwrap_or(0);
        errs.resize(changes.len(), 0);
        // receipts come in the changes' order; match them by ident and filter all the same
        for r in &got[..n] {
            if r.flags & libc::EV_ERROR != 0
                && let Some(i) = list
                    .iter()
                    .position(|c| c.ident == r.ident && c.filter == r.filter)
            {
                errs[i] = r.data as i32;
            }
        }
    }

    /// `crate::wait::WaitOps::kq_poll`.
    pub fn poll(&mut self, out: &mut Vec<Reg>) {
        // SAFETY: kevent is plain data.
        let mut got = vec![unsafe { std::mem::zeroed::<libc::kevent>() }; 256];
        let ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `got` holds its length's entries.
        let n = unsafe {
            libc::kevent(
                self.fd,
                std::ptr::null(),
                0,
                got.as_mut_ptr(),
                got.len() as i32,
                &ts,
            )
        };
        for e in &got[..usize::try_from(n).unwrap_or(0)] {
            let filt = if e.filter == libc::EVFILT_WRITE {
                FILT_WRITE
            } else {
                FILT_READ
            };
            out.push((e.ident as i32, filt));
        }
    }

    /// `crate::wait::WaitOps::sleep`: run the run loop until an event (sent), a marker of this sleep, or `timeout`.
    /// The caller has lent the backend to the scope.
    pub fn sleep(
        &mut self,
        app: &NSApplication,
        helper: &[Reg],
        timeout: Option<Duration>,
    ) -> Woke {
        let this = GEN.with(|g| {
            g.set(g.get().wrapping_add(1));
            g.get()
        });
        // SAFETY: our descriptor's CF wrapper; its callback is one-shot, so it is enabled for each sleep.
        unsafe { CFFileDescriptorEnableCallBacks(self.cffd, CFFD_READ) };
        if !helper.is_empty() {
            self.helper.get_or_insert_with(Helper::start).arm(helper);
        }
        let until = match timeout {
            Some(t) => NSDate::dateWithTimeIntervalSinceNow(t.as_secs_f64()),
            None => NSDate::distantFuture(),
        };
        // SAFETY: an immutable string constant of AppKit's.
        let mode = unsafe { NSDefaultRunLoopMode };
        let woke = loop {
            match app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                mode,
                true,
            ) {
                None => break Woke::Timeout,
                Some(ev) if is_marker(&ev) => {
                    if ev.data2() != this {
                        continue;
                    }
                    break if ev.data1() == MARK_KQ {
                        Woke::Kq
                    } else {
                        Woke::Kick
                    };
                }
                Some(ev) => {
                    #[cfg(feature = "test-hooks")]
                    if super::inject::send_posted(app, &ev) {
                        break Woke::Event;
                    }
                    app.sendEvent(&ev);
                    break Woke::Event;
                }
            }
        };
        if let Some(h) = self.helper.as_ref() {
            h.disarm();
        }
        woke
    }
}

/// The helper's state, shared with its thread.
#[derive(Default)]
struct HState {
    set: Vec<Reg>,
    armed: bool,
    in_select: bool,
}

/// The helper thread: armed for one sleep with the refused registrations, it `select`s on them and on its control
/// pipe, reports a ready one with a wake ([`wait::wake`]), and is disarmed (out of its `select`) before the sleep
/// returns, so it never watches an fd the host may close.
struct Helper {
    shared: Arc<(Mutex<HState>, Condvar)>,
    /// The control pipe: [read, write]; a byte takes the thread out of its `select`.
    ctl: [i32; 2],
}

impl Helper {
    fn start() -> Helper {
        let mut ctl = [-1; 2];
        // SAFETY: `ctl` holds two descriptors' room; the pipe is ours.
        unsafe {
            libc::pipe(ctl.as_mut_ptr());
            for fd in ctl {
                libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        let shared = Arc::new((Mutex::new(HState::default()), Condvar::new()));
        let s2 = shared.clone();
        let rd = ctl[0];
        let _ = std::thread::Builder::new()
            .name("nhmac-io".into())
            .spawn(move || {
                #[cfg(feature = "test-hooks")]
                // SAFETY: this thread's own handle.
                HELPER_THREAD.store(unsafe { libc::pthread_self() } as usize, Ordering::Release);
                helper_loop(&s2, rd)
            });
        Helper { shared, ctl }
    }

    fn arm(&self, set: &[Reg]) {
        let (m, cv) = &*self.shared;
        let mut s = m.lock().unwrap_or_else(|e| e.into_inner());
        s.set = set.to_vec();
        s.armed = true;
        cv.notify_all();
    }

    fn disarm(&self) {
        let (m, cv) = &*self.shared;
        let mut s = m.lock().unwrap_or_else(|e| e.into_inner());
        s.armed = false;
        if s.in_select {
            // SAFETY: our pipe's write end; one byte (non-blocking; a full pipe already wakes the thread).
            unsafe { libc::write(self.ctl[1], [1u8].as_ptr().cast(), 1) };
        }
        while s.in_select {
            s = cv.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// Test hook: the helper thread's `pthread_t` (0 before it starts).
#[cfg(feature = "test-hooks")]
static HELPER_THREAD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Test hook: interrupt the helper thread `n` times, 20 ms apart, from a thread of its own, with SIGURG caught by a
/// handler that does nothing and does not restart calls (a stand-in for SIGCHLD landing on the helper).  The signal's
/// previous action is put back 200 ms after the last one, so a later row finds SIGURG as it was (the watchdog's
/// sampling signal refuses a claimed one).  `false` when the helper has not started.
#[cfg(feature = "test-hooks")]
pub fn signal_helper(n: u32) -> bool {
    extern "C" fn nothing(_: i32) {}
    let t = HELPER_THREAD.load(Ordering::Acquire);
    if t == 0 {
        return false;
    }
    // SAFETY: a handler that touches nothing, installed with no SA_RESTART; the thread handle stays valid (the helper
    // never exits).
    let old = unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        let mut old: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = nothing as extern "C" fn(i32) as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGURG, &sa, &mut old);
        old
    };
    let _ = std::thread::Builder::new()
        .name("nhmac-test-signals".into())
        .spawn(move || {
            for _ in 0..n {
                std::thread::sleep(Duration::from_millis(20));
                // SAFETY: as above.
                unsafe { libc::pthread_kill(t as libc::pthread_t, libc::SIGURG) };
            }
            // the last signal delivered first (SIGURG's default action is to ignore it)
            std::thread::sleep(Duration::from_millis(200));
            // SAFETY: the action read above, put back.
            unsafe { libc::sigaction(libc::SIGURG, &old, std::ptr::null_mut()) };
        });
    true
}

fn helper_loop(shared: &(Mutex<HState>, Condvar), ctl: i32) {
    let (m, cv) = shared;
    loop {
        let set = {
            let mut s = m.lock().unwrap_or_else(|e| e.into_inner());
            while !s.armed {
                s = cv.wait(s).unwrap_or_else(|e| e.into_inner());
            }
            s.in_select = true;
            s.set.clone()
        };
        let mut regs = set.clone();
        regs.push((ctl, FILT_READ));
        let mut ready = Vec::new();
        blocking_select(&regs, &mut ready);
        let mut s = m.lock().unwrap_or_else(|e| e.into_inner());
        s.in_select = false;
        let mut buf = [0u8; 16];
        // SAFETY: our pipe's read end, non-blocking.
        while unsafe { libc::read(ctl, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
        let hit = ready.iter().any(|r| r.0 != ctl);
        let failed = ready.is_empty();
        let report = hit && s.armed;
        if hit || failed {
            // reported, or select failed (a bad fd): wait for the next arming instead of spinning
            s.armed = false;
        }
        drop(s);
        cv.notify_all();
        if report {
            wait::wake();
        }
    }
}

/// `select` on `regs` with no timeout; the ready ones into `out` (none on an error).  A signal caught on this thread
/// (Emacs's SIGCHLD can land on any thread that does not block it) is no error: the `select` starts again, so the
/// sleep it serves still ends on a ready fd.
fn blocking_select(regs: &[Reg], out: &mut Vec<Reg>) {
    let fits = |fd: i32| (0..libc::FD_SETSIZE as i32).contains(&fd);
    // SAFETY: fd_set is plain data, written only for fds below FD_SETSIZE.
    let (mut r, mut w): (libc::fd_set, libc::fd_set) =
        unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    let mut max = -1;
    for &(fd, filt) in regs.iter().filter(|r| fits(r.0)) {
        // SAFETY: below FD_SETSIZE.
        unsafe { libc::FD_SET(fd, if filt == FILT_WRITE { &mut w } else { &mut r }) };
        max = max.max(fd);
    }
    let (r0, w0) = (r, w);
    let n = loop {
        // SAFETY: the sets are ours; no timeout.
        let n = unsafe {
            libc::select(
                max + 1,
                &mut r,
                &mut w,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if n >= 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break n;
        }
        (r, w) = (r0, w0);
    };
    if n <= 0 {
        return;
    }
    for &(fd, filt) in regs.iter().filter(|r| fits(r.0)) {
        // SAFETY: as above.
        if unsafe { libc::FD_ISSET(fd, if filt == FILT_WRITE { &w } else { &r }) } {
            out.push((fd, filt));
        }
    }
}
