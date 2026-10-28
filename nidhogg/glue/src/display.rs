//! A display: the shell connection, the frame registry, and what outlives a panic (the poisoned flag, the first
//! panic's report, the connection's socket and the shell's threads, all outside the state's lock).

use crate::action::Actions;
use crate::backing::Backings;
use crate::frame::{Frame, NHG_FRAME_KINDS};
use crate::guard::{GlueError, PanicReport};
use crate::keys::Keyboard;
use crate::present::Presenting;
use nhshell::api::{BackendThread, ShellBackend};
use nhshell::ffi::{NhShell, nhshell_close};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
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
    /// The shell handle (`nhshell_open`), owned here; C reaches it through `nhg_shell` for the calls not yet ported.
    shell: NonNull<NhShell>,
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
}

impl Drop for Display {
    fn drop(&mut self) {
        // SAFETY: the handle came from `nhshell_open` and is released only here.
        unsafe { nhshell_close(self.shell.as_ptr()) };
    }
}

impl NhgDisplay {
    /// A display over `shell`, which it takes over.
    ///
    /// # Safety
    /// `shell` is a live handle from `nhshell_open`, owned by nothing else.
    pub unsafe fn new(mut shell: NonNull<NhShell>) -> NhgDisplay {
        // SAFETY: a live handle, not used anywhere else yet (the caller's promise).
        let abandon = unsafe { shell.as_mut() }.backend_mut().abandon();
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

    /// Test hook: hold the state's lock, as an abandoned call would.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn lock_for_test(&self) -> std::sync::MutexGuard<'_, Display> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Display {
    /// The shell handle (valid until the display is dropped).
    pub fn shell(&mut self) -> *mut NhShell {
        self.shell.as_ptr()
    }

    /// The shell's backend, the backings and the presenting mode, borrowed at once.
    pub(crate) fn presenting(&mut self) -> (&mut dyn ShellBackend, &Backings, &mut Presenting) {
        // SAFETY: the handle came from `nhshell_open` and lives as long as the display; C reaches it through
        // `nhg_shell` only between glue calls, so this is its one borrow while `self` is borrowed.
        let backend = unsafe { self.shell.as_mut() }.backend_mut();
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
        self.forgotten.insert(window);
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
        // SAFETY: the handle is live until the display is dropped, and the display's lock is held.
        unsafe { self.shell.as_mut() }.backend_mut()
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
}

impl Display {
    /// The shell's backend, the title bars' style and the frame of `window`, borrowed at once (`csd.rs`).
    pub(crate) fn csd_parts(
        &mut self,
        window: u64,
    ) -> Result<(&mut dyn ShellBackend, &crate::csd::Style, &mut Frame), GlueError> {
        let f = self.frames.get_mut(&window).ok_or(GlueError::State)?;
        // SAFETY: the handle is live until the display is dropped, and the display's lock is held; the frame and
        // the style are other fields.
        let backend = unsafe { self.shell.as_mut() }.backend_mut();
        Ok((backend, &self.csd_style, f))
    }
}
