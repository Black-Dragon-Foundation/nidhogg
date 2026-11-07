//! The backend's test hooks (`--enable-checking` builds: the glue's and the shell's `test-hooks` feature): an exception
//! raised inside the pump, a modal and the re-entry test for the scope, the window's layer facts, and posted AppKit key
//! and mouse events.  Lisp reaches them through `nhmac--call` and `nhmac--query`; without the feature those kinds are
//! `Arg` errors.

use super::super::inject;
use super::super::window::Win;
use super::{MacShell, drain_events, scope_stats, screens};
use crate::api::{ShellError, ShellResult, WindowId};
use crate::scope::ScopeStats;
use objc2::rc::Retained;
use objc2_app_kit::{NSAlert, NSAppearanceCustomization, NSModalResponseContinue};
use objc2_foundation::{NSException, NSRect, NSSize, NSString};
use std::time::{Duration, Instant};

/// The id the test windows' delegates report as: no host window has it, so their events change nothing.
const TEST_WINDOW: WindowId = WindowId::MAX;

/// `after - before`, as the test queries return it.
fn stats_delta(before: ScopeStats, after: ScopeStats) -> String {
    format!(
        ":takes {} :overlapping-takes {} :queued {} :applied {}",
        after.takes - before.takes,
        after.overlapping_takes - before.overlapping_takes,
        after.queued - before.queued,
        after.applied - before.applied
    )
}

/// Move `window`'s frame `by` points wider, through `setFrame:display:` (AppKit calls `windowDidResize:` inside).
fn grow(window: &objc2_app_kit::NSWindow, by: f64) {
    let f = window.frame();
    window.setFrame_display(
        NSRect::new(f.origin, NSSize::new(f.size.width + by, f.size.height)),
        false,
    );
}

/// The first element of a printed argument list such as `(5)`, as a number (`Arg` otherwise).
fn first_number(arg: &str) -> ShellResult<u64> {
    let inner = arg.trim().trim_start_matches('(').trim_end_matches(')');
    inner
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or(ShellError::Arg)
}

/// A printed argument list of numbers, such as `(5 97 97 0)`, as numbers (`Arg` for anything else).
fn numbers(arg: &str) -> ShellResult<Vec<f64>> {
    arg.trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split_whitespace()
        .map(|n| n.parse().map_err(|_| ShellError::Arg))
        .collect()
}

impl MacShell {
    /// A test `nhmac--call` kind: `None` when `kind` is not one.
    pub(super) fn test_call(&mut self, kind: &str, arg: &str) -> Option<ShellResult> {
        match kind {
            "test-post-key" | "test-post-mouse" => Some(self.post(kind, arg)),
            "test-raise-in-pump" => {
                self.raise_in_pump = true;
                Some(Ok(()))
            }
            "test-helper-next" => {
                self.force_helper += 1;
                Some(Ok(()))
            }
            // "(SECS)" or "(SECS INDEX)": end the popup being tracked after SECS, choosing item INDEX first
            "test-popup-end" => Some(numbers(arg).and_then(|n| {
                let secs = *n.first().ok_or(ShellError::Arg)?;
                super::super::menus::test_end_popup_after(
                    self.mtm,
                    secs,
                    n.get(1).map(|i| *i as isize),
                );
                Ok(())
            })),
            // "(GENERATION INDEX)": a choice from the menu bar, as AppKit's action reports it
            "test-menu-choose" => Some(numbers(arg).and_then(|n| match n[..] {
                [generation, index] => {
                    self.apply(super::super::runloop::MacEv::Menu(
                        generation as u32,
                        index as u32,
                    ));
                    Ok(())
                }
                _ => Err(ShellError::Arg),
            })),
            // "(SECS PATH)": answer the next file dialog with PATH after SECS
            "test-dialog-ok" => Some(
                crate::menu::split_numbers(arg, 1)
                    .ok_or(ShellError::Arg)
                    .map(|(n, path)| {
                        let path = path.strip_suffix(')').unwrap_or(path);
                        super::super::dialogs::schedule_ok(self.mtm, n[0] as f64, path);
                    }),
            ),
            // "(PATH)": put the file on the clipboard as a file URL
            "test-pasteboard-file" => Some(super::super::pasteboard::test_own_file(
                arg.trim().trim_start_matches('(').trim_end_matches(')'),
            )),
            // "(KIND)": an application event, as the delegate reports it (1 open files, 2 quit, 3 reopen)
            "test-app-event" => Some(numbers(arg).and_then(|n| match n[..] {
                [kind] => {
                    self.apply(super::super::runloop::MacEv::Menu(0, kind as u32));
                    Ok(())
                }
                _ => Err(ShellError::Arg),
            })),
            // "(PATH...)": files as `application:openFiles:` delivers them
            "test-open-files" => {
                let inner = arg.trim().trim_start_matches('(').trim_end_matches(')');
                super::super::appdelegate::open_files(
                    inner.split_whitespace().map(String::from).collect(),
                );
                Some(Ok(()))
            }
            "test-menu-update" => {
                super::super::menus::test_update(&self.app);
                Some(Ok(()))
            }
            _ => None,
        }
    }

    /// `test-post-key` (WINDOW CHAR IGNORING FLAGS) and `test-post-mouse` (WINDOW KIND X Y BUTTON FLAGS).
    fn post(&mut self, kind: &str, arg: &str) -> ShellResult {
        match (kind, &numbers(arg)?[..]) {
            ("test-post-key", &[win, c, ignoring, flags]) => inject::key(
                &self.app,
                self.win(win as WindowId)?,
                c as i64,
                ignoring as i64,
                flags as i64,
            ),
            ("test-post-mouse", &[win, what, x, y, button, flags]) => inject::mouse(
                self.mtm,
                &self.app,
                self.win(win as WindowId)?,
                what as i64,
                (x, y),
                button as i64,
                flags as i64,
            ),
            _ => Err(ShellError::Arg),
        }
    }

    /// A test `nhmac--query` kind: `None` when `kind` is not one.
    pub(super) fn test_query(&mut self, kind: &str, arg: &str) -> Option<ShellResult<String>> {
        Some(match kind {
            "window-info" => first_number(arg).and_then(|w| self.window_info(w)),
            "modal-alert" | "modal-resize" => first_number(arg).and_then(|ms| {
                let deadline = Instant::now()
                    .checked_add(Duration::from_millis(ms))
                    .ok_or(ShellError::Arg)?;
                Ok(self.modal_alert(deadline, kind == "modal-resize"))
            }),
            "test-reentry" => Ok(self.reentry()),
            "test-window-state" => first_number(arg).and_then(|w| {
                let w = self.win(w)?;
                Ok(format!(
                    "(:fullscreen {} :zoomed {} :miniaturized {} :tabbing-disallowed {} :titlebar-transparent {} :dark {})",
                    if super::super::windowing::is_fullscreen(w) { "t" } else { "nil" },
                    if w.window.isZoomed() { "t" } else { "nil" },
                    if w.window.isMiniaturized() { "t" } else { "nil" },
                    if w.window.tabbingMode() == objc2_app_kit::NSWindowTabbingMode::Disallowed { "t" } else { "nil" },
                    if w.window.titlebarAppearsTransparent() { "t" } else { "nil" },
                    if w.window.effectiveAppearance().name().to_string().contains("Dark") { "t" } else { "nil" },
                ))
            }),
            "test-menu-dump" => Ok(format!("{:?}", super::super::menus::test_dump(&self.app))),
            "test-kq-probe" => Ok(kq_probe(arg)),
            _ => return None,
        })
    }

    /// The `test-raise-in-pump` hook, in the pump.
    pub(super) fn maybe_raise(&mut self) {
        if std::mem::take(&mut self.raise_in_pump) {
            let name = NSString::from_str("NhmacTestException");
            let reason = NSString::from_str("a test exception raised inside the pump");
            if let Some(e) = NSException::new(&name, Some(&reason), None) {
                e.raise();
            }
        }
    }

    /// A modal alert until `deadline`, run as a modal session with the backend lent and ended by the deadline: the
    /// pumps' test of a call that runs a modal.  With `resize`, a test window is resized from inside the session after
    /// its first round, so its delegate is called inside the modal.  An exception inside it skips the session's end,
    /// on a display the guard then poisons anyway.
    fn modal_alert(&mut self, deadline: Instant, resize: bool) -> String {
        let (mtm, app) = (self.mtm, self.app.clone());
        let before = scope_stats();
        let test =
            resize.then(|| Win::open(mtm, TEST_WINDOW, 200, 100, false, "Nidhogg test window"));
        let rounds = self.lent(|_| {
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str("Nidhogg test alert"));
            let panel = alert.window();
            let session = app.beginModalSessionForWindow(&panel);
            let mut rounds = 0u64;
            while Instant::now() < deadline {
                // SAFETY: the session is the one begun above, and not ended yet.
                let r = unsafe { app.runModalSession(session) };
                rounds += 1;
                if let (1, Some(t)) = (rounds, &test) {
                    grow(&t.window, 40.0);
                }
                if r != NSModalResponseContinue {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            // SAFETY: the session begun above, ended once.
            unsafe { app.endModalSession(session) };
            panel.orderOut(None);
            rounds
        });
        // counted before the test window goes: its closing events wait for the next pump
        let r = format!("(:rounds {rounds} {})", stats_delta(before, scope_stats()));
        drop(test);
        r
    }

    /// The re-entry test: AppKit calls that call a delegate back, made with the backend lent (the delegate takes it at
    /// once), while a callback holds the borrow (the event is queued, and applied when the lend ends), and as a
    /// borrow's deferred effect (made after the borrow is put back, so taken at once again).  Inside the borrow it
    /// also runs a modal session and a nested `nextEventMatchingMask:` loop.  Returns the scope's counters' change
    /// and how many events had queued after each step of the lend.
    fn reentry(&mut self) -> String {
        let (mtm, app) = (self.mtm, self.app.clone());
        let before = scope_stats();
        let a = Win::open(mtm, TEST_WINDOW, 200, 100, false, "Nidhogg test window A");
        let b = Win::open(mtm, TEST_WINDOW, 200, 100, false, "Nidhogg test window B");
        let (wa, wb) = (a.window.clone(), b.window.clone());
        let mut steps = Vec::new();
        self.lent(|scope| {
            // the windows' opening events, if any, queued before the lend
            let start = scope.stats().queued;
            // with the backend lent and no borrow live: the delegate takes it
            grow(&wa, 10.0);
            steps.push(format!(":sync-queued {}", scope.stats().queued - start));
            let mut t = scope.take().expect("the pump's scope is lent");
            // inside the borrow: each callback finds the scope empty and queues
            grow(&wa, 10.0);
            wb.makeKeyAndOrderFront(None);
            let alert = NSAlert::new(mtm);
            let panel = alert.window();
            let session = app.beginModalSessionForWindow(&panel);
            // SAFETY: the session begun above, run once and ended once.
            unsafe {
                app.runModalSession(session);
                app.endModalSession(session);
            }
            panel.orderOut(None);
            grow(&wb, 10.0);
            drain_events(&app);
            steps.push(format!(":inner-queued {}", scope.stats().queued - start));
            // a deferred effect: made after the borrow is put back
            let wa2 = wa.clone();
            t.defer(move || grow(&wa2, 10.0));
            drop(t);
            steps.push(format!(
                ":after-effect-queued {}",
                scope.stats().queued - start
            ));
        });
        // counted before the test windows go: their closing events wait for the next pump
        let r = format!(
            "({} {})",
            stats_delta(before, scope_stats()),
            steps.join(" ")
        );
        drop((a, b));
        r
    }

    /// The window's layer facts, as a Lisp plist: whether the root view has our layer, wants it, and has no subviews;
    /// the window's visibility and its content size in device pixels.
    fn window_info(&self, window: WindowId) -> ShellResult<String> {
        let w = self.win(window)?;
        let layer_ours = w
            .view
            .layer()
            .is_some_and(|l| std::ptr::eq(Retained::as_ptr(&l), Retained::as_ptr(&w.layer)));
        let scale = w.window.backingScaleFactor();
        let (cw, ch) = w.content_size();
        let screen = screens::outputs(self.mtm).first().map_or(0, |o| o.w);
        Ok(format!(
            "(:visible {} :layer {} :wants-layer {} :subviews {} :device-width {} :device-height {} \
             :screen-device-width {screen})",
            if w.window.isVisible() { "t" } else { "nil" },
            if layer_ours { "t" } else { "nil" },
            if w.view.wantsLayer() { "t" } else { "nil" },
            w.view.subviews().len(),
            (f64::from(cw) * scale).round() as i64,
            (f64::from(ch) * scale).round() as i64,
        ))
    }
}

/// `test-kq-probe` (PATH): open PATH read-only and non-blocking, and ask a fresh kqueue for its readability:
/// `(:open ERRNO :kq ERRNO)`, 0 for success; `:kq` is -1 when the open failed.
fn kq_probe(arg: &str) -> String {
    // the argument list, printed without quotes: "(/dev/null)"
    let path = arg.trim().trim_start_matches('(').trim_end_matches(')');
    let Ok(c) = std::ffi::CString::new(path) else {
        return "(:open 22 :kq -1)".into();
    };
    let err = || std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
    // SAFETY: plain syscalls on descriptors made and closed here; the kevent is plain data.
    unsafe {
        let fd = libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOCTTY,
        );
        if fd < 0 {
            return format!("(:open {} :kq -1)", err());
        }
        let kq = libc::kqueue();
        let mut ev: libc::kevent = std::mem::zeroed();
        ev.ident = fd as usize;
        ev.filter = libc::EVFILT_READ;
        ev.flags = libc::EV_ADD;
        let rc = libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null());
        let e = if rc < 0 { err() } else { 0 };
        libc::close(kq);
        libc::close(fd);
        format!("(:open 0 :kq {e})")
    }
}
