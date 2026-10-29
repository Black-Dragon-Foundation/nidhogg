//! A display: the shell connection, the frame registry, and what outlives a panic (the poisoned flag, the first
//! panic's report, the connection's socket and the shell's threads, all outside the state's lock).

use crate::action::Actions;
use crate::backing::Backings;
use crate::frame::{Frame, NHG_FRAME_KINDS};
use crate::guard::{GlueError, PanicReport};
use crate::keys::Keyboard;
use crate::present::Presenting;
use nhshell::api::{BackendThread, ShellBackend};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a poisoned close waits for each of the shell's threads to finish before it leaves it running.
pub const THREAD_WAIT: Duration = Duration::from_millis(100);

/// The display behind `NhgDisplay *`.
pub struct NhgDisplay {
    poisoned: AtomicBool,
    /// The first panic's report, and its text once read (the backtrace resolved).
    panic: OnceLock<(PanicReport, OnceLock<String>)>,
    /// The connection's socket, for the poisoned close (-1: none).
    conn_fd: AtomicI32,
    /// The shell's threads, for the poisoned close.
    threads: Vec<Arc<dyn BackendThread>>,
    pub(crate) state: Mutex<Display>,
}

/// The state behind the display's lock.
pub struct Display {
    /// The shell's backend (`nhshell::open`), owned here (a `Box` taken apart, released in `Drop`, leaked by
    /// `NhgDisplay::abandon`).
    shell: NonNull<dyn ShellBackend>,
    frames: BTreeMap<u64, Frame>,
    /// Windows forgotten since they were last registered: calls naming them are refused.
    forgotten: BTreeSet<u64>,
    /// Each frame's backing and layer, keyed by its Glitnir root id (`backing.rs`).
    pub(crate) backings: Backings,
    /// The last `nhg_next`'s actions, kept until the next one (`action.rs`).
    pub(crate) actions: Actions,
    /// Windows whose configure ack waits for C to apply the configure (`configure.rs`).
    pub(crate) acks_due: Vec<u64>,
    /// The presenting mode (`present.rs`).
    pub(crate) presenting: Presenting,
    /// The modifier mask and the window with the keyboard (`keys.rs`, `focus.rs`).
    pub(crate) keyboard: Keyboard,
    /// Which selections we own (`select.rs`).
    pub(crate) select: crate::select::Select,
    /// The outputs C pushed (`query.rs`).
    pub(crate) monitors: crate::query::Monitors,
    /// The title bars' style (`csd.rs`).
    pub(crate) csd_style: crate::csd::Style,
    /// The wheel's accumulator (`scroll.rs`).
    pub(crate) scroll: crate::scroll::Scroll,
    /// The drop target (`dnd.rs`).
    pub(crate) dnd: crate::dnd::Dnd,
    /// The live touch points (`touch.rs`).
    pub(crate) touch: crate::touch::Touch,

    /// Reconnecting in place (`reconnect.rs`).
    pub(crate) reconnect: crate::reconnect::Reconnect,
}

impl Drop for Display {
    fn drop(&mut self) {
        let shell = self.shell.as_ptr();
        // SAFETY: the backend came from `Box::into_raw` in `new` and is released only here.  A panic in its drop is
        // caught, as the close always was.
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(shell) })));
        // A backing C never forgot leaves no Glitnir root behind.  After the close, which stops the shell's threads,
        // so that no presenter composes a root while it goes.
        let keys: Vec<u64> = self.backings.frames_mut().map(|(k, _)| k).collect();
        for key in keys {
            self.backings.forget(key);
        }
    }
}

impl NhgDisplay {
    /// A display over `shell`, which it takes over.
    pub fn new(shell: Box<dyn ShellBackend>) -> NhgDisplay {
        let abandon = shell.abandon();
        let shell = NonNull::from(Box::leak(shell));
        NhgDisplay {
            poisoned: AtomicBool::new(false),
            panic: OnceLock::new(),
            conn_fd: AtomicI32::new(abandon.conn_fd),
            threads: abandon.threads,
            state: Mutex::new(Display {
                shell,
                frames: BTreeMap::new(),
                forgotten: BTreeSet::new(),
                backings: Backings::default(),
                actions: Actions::default(),
                acks_due: Vec::new(),
                presenting: Presenting::default(),
                keyboard: Keyboard::default(),
                select: Default::default(),
                monitors: Default::default(),
                csd_style: Default::default(),
                scroll: Default::default(),
                dnd: Default::default(),
                touch: Default::default(),

                reconnect: Default::default(),
            }),
        }
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    /// Poison the display.  The first report is kept, and its message and location go to stderr; later ones are
    /// dropped.
    pub(crate) fn poison(&self, report: PanicReport) {
        self.poisoned.store(true, Ordering::Release);
        if self.panic.set((report, OnceLock::new())).is_ok()
            && let Some((r, _)) = self.panic.get()
        {
            let _ = writeln!(std::io::stderr(), "nhglue: {}", r.head);
        }
    }

    /// The first panic's whole report (message, location, resolved backtrace), if the display is poisoned.
    pub fn panic_report(&self) -> Option<&str> {
        self.panic
            .get()
            .map(|(r, text)| text.get_or_init(|| r.text()).as_str())
    }

    /// Give up a poisoned display without touching anything behind its lock: shut the connection down (the
    /// compositor then destroys every object of ours, so the windows leave the screen), stop the shell's threads and
    /// wait at most `THREAD_WAIT` for each, then leak the rest, the descriptor included, so that a thread still
    /// running never meets a reused descriptor number.
    pub fn abandon(self: Box<NhgDisplay>) {
        let fd = self.conn_fd.swap(-1, Ordering::AcqRel);
        if fd >= 0 {
            // SAFETY: a plain syscall on a descriptor the leaked connection keeps open.
            unsafe { libc::shutdown(fd, libc::SHUT_RDWR) };
        }
        for t in &self.threads {
            t.stop();
        }
        for t in &self.threads {
            t.wait_done(Instant::now() + THREAD_WAIT);
        }
        std::mem::forget(self);
    }

    /// The shell's backend, for the Rust tests, which stand in for the shell's other users (C has no way to it).
    /// Valid until the display is closed; use it only between glue calls.
    #[doc(hidden)]
    pub fn backend_ptr(&self) -> *mut dyn ShellBackend {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).shell()
    }

    /// Test hook: hold the state's lock, as an abandoned call would.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn lock_for_test(&self) -> std::sync::MutexGuard<'_, Display> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Display {
    /// The shell's backend (valid until the display is dropped).
    pub fn shell(&mut self) -> *mut dyn ShellBackend {
        self.shell.as_ptr()
    }

    /// The shell's backend, the backings and the presenting mode, borrowed at once.
    pub(crate) fn presenting(&mut self) -> (&mut dyn ShellBackend, &Backings, &mut Presenting) {
        // SAFETY: the backend lives as long as the display, and this is its one borrow while `self` is borrowed.
        let backend = unsafe { self.shell.as_mut() };
        (backend, &self.backings, &mut self.presenting)
    }

    /// Register `window` (a shell window id) as a frame of kind `flags`.  A window forgotten earlier may be
    /// registered again (the shell's ids are protocol object ids, which the client library reuses); it starts as a
    /// new frame.
    pub fn register(&mut self, window: u64, flags: u32) -> Result<(), GlueError> {
        if window == 0 || flags & !NHG_FRAME_KINDS != 0 {
            return Err(GlueError::Arg);
        }
        if self.frames.contains_key(&window) {
            return Err(GlueError::State);
        }
        self.forgotten.remove(&window);
        self.frames.insert(window, Frame::new(flags));
        Ok(())
    }

    /// Forget `window`: its frame goes, and calls naming it are refused until it is registered again.
    pub fn forget(&mut self, window: u64) -> Result<(), GlueError> {
        self.frames.remove(&window).ok_or(GlueError::State)?;
        self.touch.forget(window);
        self.forgotten.insert(window);
        self.dnd.forget(window);
        Ok(())
    }

    /// The frame of `window`.
    pub fn frame(&self, window: u64) -> Result<&Frame, GlueError> {
        self.frames.get(&window).ok_or(GlueError::State)
    }

    pub fn frame_mut(&mut self, window: u64) -> Result<&mut Frame, GlueError> {
        self.frames.get_mut(&window).ok_or(GlueError::State)
    }

    /// The shell's backend.
    pub fn backend(&mut self) -> &mut dyn ShellBackend {
        // SAFETY: the backend is live until the display is dropped, and the display's lock is held.
        unsafe { self.shell.as_mut() }
    }

    /// The last `nhg_next`'s actions.
    pub fn actions(&self) -> &Actions {
        &self.actions
    }

    pub fn is_forgotten(&self, window: u64) -> bool {
        self.forgotten.contains(&window)
    }

    /// The keyboard's modifier mask and the window that has it.
    pub fn keyboard(&self) -> Keyboard {
        self.keyboard
    }

    /// The number of registered frames.
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    // ---- the pointer, wheel, input method and outputs (`pointer.rs`, `scroll.rs`, `ime.rs`, `monitors.rs`) ----

    /// Every registered frame.
    pub(crate) fn frames_mut(&mut self) -> impl Iterator<Item = &mut Frame> {
        self.frames.values_mut()
    }

    /// The outputs the shell reported.
    pub fn monitors(&self) -> &crate::query::Monitors {
        &self.monitors
    }
}

impl Display {
    /// The shell's backend, the title bars' style and the frame of `window`, borrowed at once (`csd.rs`).
    pub(crate) fn csd_parts(
        &mut self,
        window: u64,
    ) -> Result<(&mut dyn ShellBackend, &crate::csd::Style, &mut Frame), GlueError> {
        let f = self.frames.get_mut(&window).ok_or(GlueError::State)?;
        // SAFETY: the backend is live until the display is dropped, and the display's lock is held; the frame and
        // the style are other fields.
        let backend = unsafe { self.shell.as_mut() };
        Ok((backend, &self.csd_style, f))
    }
}

impl Display {
    /// The child frames whose pushed facts name `parent` as their parent (`child.rs`).
    pub(crate) fn child_windows(&self, parent: u64) -> Vec<u64> {
        self.frames
            .iter()
            .filter(|(_, f)| {
                f.flags & crate::frame::NHG_FRAME_CHILD != 0 && f.child.parent() == parent
            })
            .map(|(&w, _)| w)
            .collect()
    }
}

impl Display {
    /// The display is given up (`nhg_give_up`): the compositor went and nothing waits for it to come back (reconnecting
    /// is off, failed or timed out), and C deletes the display's frames next.  What the glue held for the connection
    /// goes first, in this order:
    /// 1. reconnecting: no step or settle is due any more;
    /// 2. the drop target: a drag over us ends unfinished (Lisp drops the display's `pgtk-dnd-current-state` entry);
    /// 3. input: the touch points and the configure acks still owed.
    ///
    /// Each frame's deletion then forgets its window and frees its backing, and with it its Glitnir root and layers;
    /// the close frees whatever is left (`Drop`).  C itself cancels the bell's timers and dismisses the popup frames, so
    /// that a menu's loop ends, because it must do so on a poisoned display too, where this is never called.
    pub(crate) fn give_up(&mut self) {
        self.reconnect.give_up();
        self.dnd.lost();
        self.touch.clear();
        self.acks_due.clear();
    }
}

impl Display {
    /// Every registered frame with its window (`popup.rs`).
    pub(crate) fn frames(&self) -> impl Iterator<Item = (u64, &Frame)> {
        self.frames.iter().map(|(&w, f)| (w, f))
    }
}
