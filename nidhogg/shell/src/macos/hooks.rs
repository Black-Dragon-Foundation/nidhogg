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
use objc2_app_kit::{
    NSAlert, NSAppearanceCustomization, NSModalResponseContinue, NSWindowOcclusionState,
};
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

thread_local! {
    /// The last `test-real-drag`'s points and the windows under them (`test-drag-log`).
    static DRAG_LOG: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
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

/// [`numbers`] as whole numbers: a card's id is a u64 an f64 does not hold (round 8: every `test-card-row` was STATE).
fn whole_numbers(arg: &str) -> ShellResult<Vec<u64>> {
    arg.trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split_whitespace()
        .map(|n| n.parse().map_err(|_| ShellError::Arg))
        .collect()
}

/// `(FAMILY SIZE GLYPH...)`, as `nhmac--call` prints its arguments (strings unquoted, so FAMILY is the words before
/// the first number): Core Text's own advances of the glyphs in the open font of FAMILY at SIZE pixels
/// (`CTFontGetAdvancesForGlyphs`, unrounded), a list; nil when no such font is open.
fn ct_advances(arg: &str) -> ShellResult<String> {
    let Some((handle, glyphs)) = open_font_and_numbers(arg)? else {
        return Ok("nil".into());
    };
    let glyphs: Vec<u16> = glyphs.iter().map(|&g| g as u16).collect();
    let mut b = vec![[0.0f64; 5]; glyphs.len()];
    if !super::super::fonts::bounds_for(handle, &glyphs, &mut b) {
        return Ok("nil".into());
    }
    let xs: Vec<String> = b.iter().map(|x| format!("{:?}", x[0])).collect();
    Ok(format!("({})", xs.join(" ")))
}

/// `(FAMILY SIZE N...)`: the open font of FAMILY at SIZE pixels (FAMILY is the words before the first number) and the
/// numbers after SIZE; `None` when no such font is open.
fn open_font_and_numbers(arg: &str) -> ShellResult<Option<(u64, Vec<f64>)>> {
    let words: Vec<&str> = arg
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split_whitespace()
        .collect();
    let at = words
        .iter()
        .position(|w| w.parse::<f64>().is_ok())
        .ok_or(ShellError::Arg)?;
    let family = words[..at].join(" ");
    let n = numbers(&words[at..].join(" "))?;
    let (&size, rest) = n.split_first().ok_or(ShellError::Arg)?;
    Ok(super::super::fonts::find(&family, size).map(|h| (h, rest.to_vec())))
}

/// Each character's UTF-16 units, and where each starts.
fn utf16(chars: &[f64]) -> ShellResult<(Vec<u16>, Vec<usize>)> {
    let (mut units, mut starts) = (Vec::new(), Vec::new());
    for &c in chars {
        let c = char::from_u32(c as u32).ok_or(ShellError::Arg)?;
        starts.push(units.len());
        let mut b = [0u16; 2];
        units.extend_from_slice(c.encode_utf16(&mut b));
    }
    Ok((units, starts))
}

/// `(FAMILY SIZE CHAR...)`: Core Text's own answers for the open font of FAMILY at SIZE pixels, unrounded, device
/// pixels, y up: `(:ascent A :descent D :leading L :underline-position P :underline-thickness T :glyphs ((CHAR GLYPH
/// ADVANCE LEFT RIGHT TOP BOTTOM) ...))` after its `:size` and `:family`, GLYPH 0 for a character the font lacks; nil
/// when no such font is open.  The
/// GUI rows' oracle: they apply the driver's stated rounding rules to these numbers themselves.
fn ct_font(arg: &str) -> ShellResult<String> {
    use super::super::fonts;
    let Some((handle, chars)) = open_font_and_numbers(arg)? else {
        return Ok("nil".into());
    };
    let Some(m) = fonts::raw(handle) else {
        return Ok("nil".into());
    };
    let (units, starts) = utf16(&chars)?;
    let mut all = vec![0u16; units.len()];
    if !fonts::glyphs_for(handle, &units, &mut all) {
        return Ok("nil".into());
    }
    let glyphs: Vec<u16> = starts.iter().map(|&s| all[s]).collect();
    let mut b = vec![[0.0f64; 5]; glyphs.len()];
    if !fonts::bounds_for(handle, &glyphs, &mut b) {
        return Ok("nil".into());
    }
    let rows: Vec<String> = chars
        .iter()
        .zip(&glyphs)
        .zip(&b)
        .map(|((c, g), x)| {
            format!(
                "({} {g} {:?} {:?} {:?} {:?} {:?})",
                *c as u32, x[0], x[1], x[2], x[3], x[4]
            )
        })
        .collect();
    Ok(format!(
        "(:size {:?} :family {} :mono {} :ascent {:?} :descent {:?} :leading {:?} :underline-position {:?} \
         :underline-thickness {:?} :glyphs ({}))",
        m.size,
        crate::pasteboard::lisp_string(&m.family),
        // kCTFontMonoSpaceTrait
        if m.symbolic & (1 << 10) != 0 {
            "t"
        } else {
            "nil"
        },
        m.ascent,
        m.descent,
        m.leading,
        m.underline_position,
        m.underline_thickness,
        rows.join(" ")
    ))
}

/// `(FILE)`: ImageIO's own decode of FILE's first frame in sRGB, premultiplied ARGB words (`images::decode`, the
/// GUI rows' oracle for what an image puts on the surface): `(W H [PIXELS...])`, rows top down; nil when ImageIO
/// cannot read it.  FILE is the rest of the argument list (a path without blanks).
fn imageio_pixels(arg: &str) -> ShellResult<String> {
    let path = arg
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    let bytes = std::fs::read(path).map_err(|_| ShellError::Arg)?;
    Ok(match super::super::images::decode(&bytes, 0) {
        Some(d) => {
            let px: Vec<String> = d.argb.iter().map(u32::to_string).collect();
            format!("({} {} [{}])", d.width, d.height, px.join(" "))
        }
        None => "nil".into(),
    })
}

/// `(FAMILY SIZE CHAR...)`: for each character, the family of the font Core Text's cascade picks for it from the open
/// font of FAMILY at SIZE (`CTFontCreateForString`), a string, or nil; nil when no such font is open.
fn ct_cascade(arg: &str) -> ShellResult<String> {
    let Some((handle, chars)) = open_font_and_numbers(arg)? else {
        return Ok("nil".into());
    };
    let names: Vec<String> = chars
        .iter()
        .map(|&c| {
            super::super::text::cascade_family(handle, c as u32)
                .map_or_else(|| "nil".into(), |f| crate::pasteboard::lisp_string(&f))
        })
        .collect();
    Ok(format!("({})", names.join(" ")))
}

/// `(FAMILY SIZE CHAR...)`: the characters laid out by Core Text in the open font of FAMILY at SIZE, kerning off (the
/// driver's line), its runs in storage order: `((RTL (INDEX GLYPH X Y WIDTH) ...) ...)`, INDEX the glyph's character
/// (not its UTF-16 unit), X and Y its position from the line's origin, WIDTH its typographic width, unrounded; nil when
/// no such font is open or Core Text needed another font.
fn ct_shape(arg: &str) -> ShellResult<String> {
    let Some((handle, chars)) = open_font_and_numbers(arg)? else {
        return Ok("nil".into());
    };
    let (units, starts) = utf16(&chars)?;
    let Some(line) = super::super::text::shape(handle, &units) else {
        return Ok("nil".into());
    };
    let char_of = |u: usize| starts.iter().rposition(|&s| s <= u).unwrap_or(0);
    let runs: Vec<String> = line
        .runs
        .iter()
        .map(|r| {
            let gs: Vec<String> = (0..r.glyphs.len())
                .map(|i| {
                    format!(
                        "({} {} {:?} {:?} {:?})",
                        char_of(r.indices[i]),
                        r.glyphs[i],
                        r.positions[i].0,
                        r.positions[i].1,
                        r.widths[i]
                    )
                })
                .collect();
            format!("({} {})", if r.rtl { "t" } else { "nil" }, gs.join(" "))
        })
        .collect();
    Ok(format!("({})", runs.join(" ")))
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
            // (SIGNAL): raised by the next pump, as a signal that lands inside a glue call
            "test-signal-in-pump" => Some(numbers(arg).and_then(|n| match n[..] {
                [s] if (1.0..32.0).contains(&s) => {
                    self.signal_in_pump = s as i32;
                    Ok(())
                }
                _ => Err(ShellError::Arg),
            })),
            // -1: the system's setting again; 0 or 1 stand for it
            "test-reduced-motion" => Some(numbers(arg).and_then(|n| match n[..] {
                [v] => {
                    super::motion::REDUCED_OVERRIDE.store(
                        v.clamp(-1.0, 1.0) as i32,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    Ok(())
                }
                _ => Err(ShellError::Arg),
            })),
            "test-signal-helper" => Some(numbers(arg).and_then(|n| match n[..] {
                [k] if k >= 0.0 => {
                    if super::kq::signal_helper(k as u32) {
                        Ok(())
                    } else {
                        Err(ShellError::State)
                    }
                }
                _ => Err(ShellError::Arg),
            })),
            "test-helper-next" => {
                self.force_helper += 1;
                Some(Ok(()))
            }
            "test-force-in-use" => Some(numbers(arg).and_then(|n| match n[..] {
                [w, on] => {
                    let s = self.surfaces_mut(w as WindowId).ok_or(ShellError::State)?;
                    s.force_in_use = on != 0.0;
                    Ok(())
                }
                _ => Err(ShellError::Arg),
            })),
            "test-capture-switch" => Some(numbers(arg).and_then(|n| match n[..] {
                [r, g, b] => {
                    self.capture = Capture {
                        armed: Some([r as u8, g as u8, b as u8]),
                        counts: Vec::new(),
                    };
                    Ok(())
                }
                _ => Err(ShellError::Arg),
            })),
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
            // "(WINDOW SECS)": post `f34` to WINDOW after SECS; a negative SECS disarms it
            "test-wake-after" => Some(numbers(arg).and_then(|n| match n[..] {
                [win, secs] => {
                    let window = self.win(win as WindowId)?.window.windowNumber();
                    super::super::menus::test_wake_after(self.mtm, window, secs);
                    Ok(())
                }
                _ => Err(ShellError::Arg),
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
            // "(WINDOW FILE PAYLOAD)": a drop on WINDOW's view, of the file at PAYLOAD (FILE 1) or the text PAYLOAD
            "test-drop" => Some(
                crate::menu::split_numbers(arg, 2)
                    .ok_or(ShellError::Arg)
                    .and_then(|(n, payload)| {
                        let payload = payload.strip_suffix(')').unwrap_or(payload);
                        let view = self.win(n[0] as WindowId)?.view.clone();
                        super::super::dnd::test_drop(self.mtm, &view, n[1] == 1, payload).and_then(
                            |taken| {
                                if taken {
                                    Ok(())
                                } else {
                                    Err(ShellError::State)
                                }
                            },
                        )
                    }),
            ),
            // "(NAME TEXT)": another application's Services request, as AppKit hands it to the provider
            "test-service-request" => Some(
                crate::pasteboard::word_and_text(arg)
                    .ok_or(ShellError::Arg)
                    .map(|(name, text)| {
                        super::super::services::test_request(&self.app, name, text)
                    }),
            ),
            // "(DARK)": the application's appearance, dark (1) or the system's (0), as a system switch shows it
            "test-app-appearance" => Some(numbers(arg).map(|n| {
                super::super::windowing::test_appearance(&self.app, n.first() == Some(&1.0));
                // what the switch's notification waking the run loop leads to
                self.check_appearance();
            })),
            // "(WINDOW OP TEXT)": what an input method sends WINDOW's view: OP 0 marked text, 1 inserted text, 2 the
            // end of the composition, 3 the command TEXT (a selector)
            "test-ime" => Some(
                crate::menu::split_numbers(arg, 2)
                    .ok_or(ShellError::Arg)
                    .and_then(|(n, text)| {
                        let text = text.strip_suffix(')').unwrap_or(text);
                        let view = self.win(n[0] as WindowId)?.view.clone();
                        super::super::ime::test_send(&view, n[1], text)
                    }),
            ),
            // "(ON)": count Zoom as on (1) or ask the system again (0)
            "test-zoom-force" => {
                Some(first_number(arg).map(|on| super::super::ax::test_force_zoom(on == 1)))
            }
            "test-menu-update" => {
                super::super::menus::test_update(&self.app);
                Some(Ok(()))
            }
            // "(CODE FLAGS)": a real key press and release through the window server (`inject::real_key`)
            "test-real-key" => Some(whole_numbers(arg).and_then(|n| match n[..] {
                [code, flags] => inject::real_key(u16::try_from(code).map_err(|_| ShellError::Arg)?, flags),
                _ => Err(ShellError::Arg),
            })),
            // "(WINDOW)": the green button's zoom, as AppKit's `zoom:` runs it
            "test-zoom" => Some(first_number(arg).and_then(|w| {
                self.win(w)?.window.zoom(None);
                Ok(())
            })),
            // "(FROM X Y TO X Y DELAY-MS)": a real left drag through the window server from device pixel X, Y of window
            // FROM to X, Y of window TO (from each frame's top left), the moves after DELAY-MS (`inject::real_drag`)
            "test-real-drag" => Some(numbers(arg).and_then(|n| match n[..] {
                [a, ax, ay, b, bx, by, ms] => {
                    let at = |w: &Win, x: f64, y: f64| {
                        let (px, py) = w.scale().view_point(x, y, w.view.bounds().size.height);
                        let p = w
                            .view
                            .convertPoint_toView(objc2_foundation::NSPoint::new(px, py), None);
                        let s = w.window.convertPointToScreen(p);
                        (s.x, s.y)
                    };
                    let from = at(self.win(a as WindowId)?, ax, ay);
                    let to = at(self.win(b as WindowId)?, bx, by);
                    // which windows the press and the release land on (round 4: no press reached the view)
                    let under = |(x, y): (f64, f64)| {
                        objc2_app_kit::NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(
                            objc2_foundation::NSPoint::new(x, y),
                            0,
                            self.mtm,
                        )
                    };
                    let log = format!(
                        "(:from ({} {}) :to ({} {}) :under-from {} :under-to {} :a {} :b {} :key {})",
                        from.0,
                        from.1,
                        to.0,
                        to.1,
                        under(from),
                        under(to),
                        self.win(a as WindowId)?.window.windowNumber(),
                        self.win(b as WindowId)?.window.windowNumber(),
                        self.app.keyWindow().map_or(0, |k| k.windowNumber()),
                    );
                    DRAG_LOG.with(|l| *l.borrow_mut() = log);
                    inject::real_drag(self.mtm, from, to, Duration::from_millis(ms as u64), 15)
                }
                _ => Err(ShellError::Arg),
            })),
            // "(WINDOW KIND BOTTOM OUT DX DY)": a real left press (KIND 0), release (1) or drag (2) through the window
            // server, OUT points outside WINDOW's right edge (and its bottom edge with BOTTOM 1), moved by DX, DY
            // points (y up)
            "test-post-real" => Some(numbers(arg).and_then(|n| match n[..] {
                [w, kind, bottom, out, dx, dy] => {
                    let f = self.win(w as WindowId)?.window.frame();
                    let x = f.origin.x + f.size.width + out + dx;
                    let y = if bottom == 1.0 {
                        f.origin.y - out
                    } else {
                        f.origin.y + f.size.height / 2.0
                    } + dy;
                    inject::real_mouse(self.mtm, kind as i64, (x, y))
                }
                _ => Err(ShellError::Arg),
            })),
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
                0,
            ),
            ("test-post-key", &[win, c, ignoring, flags, code]) => inject::key(
                &self.app,
                self.win(win as WindowId)?,
                c as i64,
                ignoring as i64,
                flags as i64,
                code as u16,
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
            "modal-alert" | "modal-resize" => first_number(arg)
                .map(|ms| self.modal_alert(Duration::from_millis(ms), kind == "modal-resize")),
            "test-reentry" => Ok(self.reentry()),
            "startup-log" => Ok(crate::trace::startup_text()),
            "image-draws" => Ok(crate::trace::images_text()),
            "pump-trace" => Ok(crate::trace::pumps_text()),
            "test-entries" => first_number(arg).map(|on| crate::trace::entries_text(on != 0)),
            "shell-view" => Ok(self.shell_view()),
            "test-window-state" => first_number(arg).and_then(|w| {
                let w = self.win(w)?;
                Ok(format!(
                    "(:fullscreen {} :zoomed {} :miniaturized {} :tabbing-disallowed {} :titlebar-transparent {} :dark {} :fs-busy {} :fs-want {} :app-running {} :style-full {} :fs-notes ({}) :fs-log ({}))",
                    if super::super::windowing::is_fullscreen(w) { "t" } else { "nil" },
                    if w.window.isZoomed() { "t" } else { "nil" },
                    if w.window.isMiniaturized() { "t" } else { "nil" },
                    if w.window.tabbingMode() == objc2_app_kit::NSWindowTabbingMode::Disallowed { "t" } else { "nil" },
                    if w.window.titlebarAppearsTransparent() { "t" } else { "nil" },
                    if w.window.effectiveAppearance().name().to_string().contains("Dark") { "t" } else { "nil" },
                    if w.fs_busy { "t" } else { "nil" },
                    match w.fs_want { Some(true) => "on", Some(false) => "off", None => "nil" },
                    // whether AppKit counts the application as running (`[NSApp run]`, which the pumps never call)
                    if self.app.isRunning() { "t" } else { "nil" },
                    if w.window.styleMask().contains(objc2_app_kit::NSWindowStyleMask::FullScreen) { "t" } else { "nil" },
                    // a Lisp list: will-enter will-exit did-enter did-exit fail-enter fail-exit applied
                    super::super::runloop::FS_NOTES
                        .iter()
                        .map(|n| n.load(std::sync::atomic::Ordering::Relaxed).to_string())
                        .collect::<Vec<_>>()
                        .join(" "),
                    // the newest notifications as (INDEX MS): how long each transition took
                    super::super::runloop::FS_LOG.with(|l| l
                        .borrow()
                        .1
                        .iter()
                        .map(|(i, ms)| format!("({i} {ms})"))
                        .collect::<Vec<_>>()
                        .join(" ")),
                ))
            }),
            "test-menu-dump" => Ok(format!("{:?}", super::super::menus::test_dump(&self.app))),
            // "(INDEX)": the Dock's menu as the delegate serves it, choosing item INDEX first (-1: none)
            "test-dock-menu" => numbers(arg).map(|n| {
                let choose = n.first().map(|i| *i as isize).filter(|i| *i >= 0);
                format!("{:?}", super::super::menus::test_dock(&self.app, choose))
            }),
            "test-kq-probe" => Ok(kq_probe(arg)),
            "test-kq-layers" => kq_layers(arg),
            "test-child" => first_number(arg).and_then(|w| self.child_info(w)),
            "test-cards" => first_number(arg).map(|w| {
                let c = self.cards.get(&w);
                format!(
                    "(:images {} :cards ({}))",
                    c.map_or(0, |c| c.images),
                    c.map_or_else(Vec::new, |c| c.shown())
                        .iter()
                        .map(|(id, (x, y, w, h))| format!("({id} {x} {y} {w} {h})"))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            }),
            // "(W ID Y X0 N)": card ID's image row Y from column X0, N pixels (round 6: its ring's left side blended)
            "test-card-row" => whole_numbers(arg).and_then(|n| match n[..] {
                [w, id, y, x0, k] => self
                    .cards
                    .get(&w)
                    .and_then(|c| c.row(id, y as u32, x0 as u32, k as u32))
                    .map(|r| format!("({})", r.iter().map(|p| format!("\"{p:08x}\"")).collect::<Vec<_>>().join(" ")))
                    .ok_or(ShellError::State),
                _ => Err(ShellError::Arg),
            }),
            "test-windows" => Ok(self.window_numbers()),
            "test-captures" => Ok(format!(
                "({})",
                self.capture
                    .counts
                    .iter()
                    .map(|c| c.map_or_else(|| "nil".to_string(), |n| n.to_string()))
                    .collect::<Vec<_>>()
                    .join(" ")
            )),
            "test-ime-state" => numbers(arg).and_then(|n| match n[..] {
                [w, loc, len] => {
                    let w = self.win(w as WindowId)?;
                    Ok(super::super::ime::test_state(&w.view, &w.window, loc as usize, len as usize))
                }
                _ => Err(ShellError::Arg),
            }),
            "test-ax" => first_number(arg).and_then(|w| {
                let w = self.win(w)?;
                Ok(super::super::ax::test_state(&w.view, &w.window))
            }),
            "test-presentation" =>Ok(self.app.presentationOptions().0.to_string()),
            "test-resize" => first_number(arg).and_then(|w| self.resize_state(w)),
            // "(WINDOW EDGES)": whether the current cursor is the resize cursor for EDGES
            "test-resize-cursor" => numbers(arg).and_then(|n| match n[..] {
                [_, edges] => Ok(lisp_bool(super::super::resize::test_cursor_is(edges as u8))),
                _ => Err(ShellError::Arg),
            }),
            // whether this process may post events through the window server (`CGPreflightPostEventAccess`)
            "test-post-access" => Ok(lisp_bool(inject::post_access())),
            "test-posted" => Ok(inject::posted_pending().to_string()),
            // "(W)": whether the application is active and W key, who is frontmost, the AX grant
            "test-app-state" => first_number(arg).and_then(|w| Ok(inject::app_state(&self.app, self.win(w)?))),
            // "(W)": activate through Accessibility, then W key and front; the AX error
            // the window's focus and transition notifications since the start, oldest first
            "test-key-log" => Ok(format!(
                "({})",
                super::super::runloop::KEY_LOG
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .join(" ")
            )),
            "test-ax-activate" => {
                first_number(arg).and_then(|w| Ok(inject::ax_activate(&self.app, self.win(w)?).to_string()))
            }
            "test-surface" => numbers(arg).and_then(|n| match n[..] {
                [w, x, y] => self.surface_info(w as WindowId, x as i32, y as i32),
                _ => Err(ShellError::Arg),
            }),
            "test-surface-region" => numbers(arg).and_then(|n| match n[..] {
                [w, x, y, rw, rh] => {
                    let px = self
                        .win(w as WindowId)?
                        .surfaces
                        .region(x as i32, y as i32, rw as i32, rh as i32)
                        .ok_or(ShellError::Arg)?;
                    let items: Vec<String> = px.iter().map(u32::to_string).collect();
                    Ok(format!("[{}]", items.join(" ")))
                }
                _ => Err(ShellError::Arg),
            }),
            "test-ct-advances" => ct_advances(arg),
            "test-ct-font" => ct_font(arg),
            "test-ct-cascade" => ct_cascade(arg),
            "test-ct-shape" => ct_shape(arg),
            "test-imageio-pixels" => imageio_pixels(arg),
            // "(ID)": select the input source ID (an empty ID only reports)
            "test-input-source" => super::super::ime::test_input_source(
                super::super::nsapp::text_arg(arg.trim()).trim(),
            ),
            "test-font-probe" => super::super::font_probe::query(arg).ok_or(ShellError::Arg),
            "test-zoom-log" => Ok(super::super::ax::test_zoom_log()),
            "test-drag-log" => Ok(DRAG_LOG.with(|l| l.borrow().clone())),
            // its own hook: Lisp reads one object from an answer (rounds 9 and 10 read the drag log only)
            "test-drag-start-log" => Ok({
                let l = super::super::dnd::START_LOG
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                if l.is_empty() { "nil".to_owned() } else { l }
            }),
            "test-chrome-log" => Ok(format!(
                "({})",
                super::child::CHROME_LOG.with(|l| l.borrow().join(" "))
            )),
            "test-legs" => first_number(arg).map(|w| {
                self.legs.get(&w).map_or_else(|| "nil".to_owned(), |l| l.text())
            }),
            _ => return None,
        })
    }

    /// The `test-raise-in-pump` and `test-signal-in-pump` hooks, in the pump.
    pub(super) fn maybe_raise(&mut self) {
        let sig = std::mem::take(&mut self.signal_in_pump);
        if sig != 0 {
            // SAFETY: raise sends a signal to this thread; Emacs's handler runs here, inside the glue call
            unsafe { libc::raise(sig) };
        }
        if std::mem::take(&mut self.raise_in_pump) {
            let name = NSString::from_str("NhmacTestException");
            let reason = NSString::from_str("a test exception raised inside the pump");
            if let Some(e) = NSException::new(&name, Some(&reason), None) {
                e.raise();
            }
        }
    }

    /// A modal alert for `length` from the session's start (making the alert and the test window, slow on a cold
    /// runner, does not use it up), run as a modal session with the backend lent and ended then: the pumps' test of a
    /// call that runs a modal.  With `resize`, a test window is resized from inside the session after
    /// its first round, so its delegate is called inside the modal.  An exception inside it skips the session's end,
    /// on a display the guard then poisons anyway.
    fn modal_alert(&mut self, length: Duration, resize: bool) -> String {
        let (mtm, app) = (self.mtm, self.app.clone());
        // what callbacks outside a lend queued before this (a reparent's resize and backing changes) is applied
        // first, so the counts are the session's own
        self.lent(|_| ());
        let before = scope_stats();
        let test =
            resize.then(|| Win::open(mtm, TEST_WINDOW, 200, 100, false, "Nidhogg test window"));
        let rounds = self.lent(|_| {
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str("Nidhogg test alert"));
            let panel = alert.window();
            let session = app.beginModalSessionForWindow(&panel);
            // the session runs `length` after its first round, which shows the panel: round 4 (s1) spent 335 ms in
            // that round alone (AppKit's first alert of the process), and a deadline from the start left no second
            // round.  A length past the clock's range ends the session after that round.
            let mut deadline = None;
            let mut rounds = 0u64;
            // why a session ran few rounds (round 2, s1: one): each round's length and the last response
            let (mut longest, mut first, mut response) =
                (Duration::ZERO, Duration::ZERO, NSModalResponseContinue);
            while deadline.is_none_or(|d| Instant::now() < d) {
                let t0 = Instant::now();
                // SAFETY: the session is the one begun above, and not ended yet.
                let r = unsafe { app.runModalSession(session) };
                if deadline.is_none() {
                    first = t0.elapsed();
                    deadline = Some(
                        Instant::now()
                            .checked_add(length)
                            .unwrap_or_else(Instant::now),
                    );
                } else {
                    longest = longest.max(t0.elapsed());
                }
                response = r;
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
            (rounds, first, longest, response)
        });
        let (rounds, first, longest, response) = rounds;
        // counted before the test window goes: its closing events wait for the next pump
        let r = format!(
            "(:rounds {rounds} :first-round-ms {} :longest-round-ms {} :last-response {response} {})",
            first.as_millis(),
            longest.as_millis(),
            stats_delta(before, scope_stats())
        );
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

    /// The shell's own view, as a Lisp plist: its windows (id, content size in points, scale, AppKit's visibility and
    /// occlusion, and whether it is the key window), the records queued for the glue (window and event), oldest first,
    /// and whether the wake pipe holds its byte.
    fn shell_view(&self) -> String {
        let mut ids: Vec<_> = self.windows.keys().copied().collect();
        ids.sort_unstable();
        let windows: Vec<String> = ids
            .iter()
            .map(|id| {
                let w = &self.windows[id];
                let (cw, ch) = w.content_size();
                let occluded = !w
                    .window
                    .occlusionState()
                    .contains(NSWindowOcclusionState::Visible);
                let t = |b: bool| if b { "t" } else { "nil" };
                format!(
                    "(:id {id} :points ({cw} {ch}) :scale {} :visible {} :occluded {} :key {})",
                    crate::macos::scale::of_window(&w.window).factor(),
                    t(w.window.isVisible()),
                    t(occluded),
                    t(w.window.isKeyWindow())
                )
            })
            .collect();
        let queued: Vec<String> = self
            .queue
            .iter()
            .map(|r| format!("({} {:?})", r.window, format!("{:?}", r.event)))
            .collect();
        let mut p = libc::pollfd {
            fd: self.wake[0],
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd, for the call; no wait.
        let woken = unsafe { libc::poll(&mut p, 1, 0) } > 0;
        format!(
            "(:windows ({}) :queued ({}) :wake-readable {})",
            windows.join(" "),
            queued.join(" "),
            if woken { "t" } else { "nil" }
        )
    }

    /// The window's layer facts, as a Lisp plist: whether the root view has our layer, wants it, and has no subviews;
    /// the window's visibility and its content size in device pixels.
    fn window_info(&self, window: WindowId) -> ShellResult<String> {
        let w = self.win(window)?;
        let layer_ours = w
            .view
            .layer()
            .is_some_and(|l| std::ptr::eq(Retained::as_ptr(&l), Retained::as_ptr(&w.layer)));
        let scale = crate::macos::scale::of_window(&w.window);
        let (cw, ch) = w.content_size();
        let screen = screens::outputs(self.mtm).first().map_or(0, |o| o.w);
        // where AppKit has the window: its frame (points, y up from the main screen's bottom), the screen it is on
        // (its frame and scale), and whether any of it is visible
        let fr = w.window.frame();
        let on = w.window.screen().map_or("nil".to_string(), |sc| {
            let f = sc.frame();
            format!(
                "({} {} {} {} {})",
                f.origin.x,
                f.origin.y,
                f.size.width,
                f.size.height,
                crate::macos::scale::of_screen(&sc).factor()
            )
        });
        let occluded = !w
            .window
            .occlusionState()
            .contains(NSWindowOcclusionState::Visible);
        Ok(format!(
            "(:visible {} :layer {} :wants-layer {} :subviews {} :device-width {} :device-height {} \
             :screen-device-width {screen} :frame ({} {} {} {}) :on-screen {on} :occluded {} :has-shadow {} \
             :sublayers {})",
            if w.window.isVisible() { "t" } else { "nil" },
            if layer_ours { "t" } else { "nil" },
            if w.view.wantsLayer() { "t" } else { "nil" },
            w.view.subviews().len(),
            i64::from(scale.px_i(cw)),
            i64::from(scale.px_i(ch)),
            fr.origin.x,
            fr.origin.y,
            fr.size.width,
            fr.size.height,
            if occluded { "t" } else { "nil" },
            if w.window.hasShadow() { "t" } else { "nil" },
            // SAFETY: the sublayers array is read, not changed.
            unsafe { w.layer.sublayers() }.map_or(0, |a| a.len()),
        ))
    }
}

fn lisp_bool(b: bool) -> String {
    (if b { "t" } else { "nil" }).to_string()
}

impl MacShell {
    /// `test-resize` (WINDOW): our own resize's facts as a Lisp plist: the frame (screen points, origin bottom left),
    /// the content size and its minimum (points), the scale times 120, `inLiveResize`, whether a resize waits for the
    /// host, and the band presses `sendEvent:` took.
    fn resize_state(&self, window: WindowId) -> ShellResult<String> {
        let w = self.win(window)?;
        let f = w.window.frame();
        let (cw, ch) = w.content_size();
        let min = w.window.contentMinSize();
        Ok(format!(
            "(:frame ({} {} {} {}) :content ({cw} {ch}) :min ({} {}) :scale120 {} :live {} :pending {} \
             :band-presses {})",
            f.origin.x.round(),
            f.origin.y.round(),
            f.size.width.round(),
            f.size.height.round(),
            min.width.round(),
            min.height.round(),
            w.scale_now(),
            lisp_bool(w.window.inLiveResize()),
            lisp_bool(w.resize.inflight.is_some()),
            super::super::resize::BAND_PRESSES.load(std::sync::atomic::Ordering::Relaxed),
        ))
    }

    /// `test-surface` (WINDOW X Y): the window's newest surface as a Lisp plist: its size in device pixels, the
    /// content size in points, the layer's `contentsScale` and gravity, whether the layer shows that surface, its
    /// colour space (`srgb`, `generic` or nil), its pixel at X, Y (premultiplied ARGB; nil outside), how many
    /// surfaces the ring has made, whether the pacer holds a commit, and whether the next commit would find a slot the
    /// window server does not hold.
    fn surface_info(&self, window: WindowId, x: i32, y: i32) -> ShellResult<String> {
        let w = self.win(window)?;
        let s = &w.surfaces;
        let (sw, sh) = s.size();
        let (cw, ch) = w.content_size();
        // SAFETY: `contents` is an object or nil; we only compare its address.
        let contents = unsafe { w.layer.contents() };
        let shown = match (contents, s.newest()) {
            (Some(c), Some(n)) => {
                std::ptr::eq(Retained::as_ptr(&c).cast::<std::ffi::c_void>(), n.as_ptr())
            }
            _ => false,
        };
        let srgb = match s.tagged_srgb() {
            Some(true) => "srgb",
            Some(false) => "generic",
            None => "nil",
        };
        let pixel = s
            .pixel(x, y)
            .map_or_else(|| "nil".to_string(), |p| p.to_string());
        Ok(format!(
            "(:w {sw} :h {sh} :points ({cw} {ch}) :scale {} :gravity \"{}\" :contents-is-newest {} :srgb {srgb} \
             :pixel {pixel} :slots {} :held {} :free {})",
            w.layer.contentsScale(),
            w.layer.contentsGravity(),
            if shown { "t" } else { "nil" },
            s.slots(),
            if s.owes() { "t" } else { "nil" },
            if s.free() { "t" } else { "nil" },
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

/// `test-kq-layers` (N): which layer of the wait's kqueue loses a readiness, below the run loop.  For a pipe and a
/// pty, a writer thread writes N bytes at random 0-3 ms intervals while this thread waits as the main wait does: a
/// poll of the kqueue first, then a block, either on that kqueue (`direct`) or on a second kqueue watching the first's
/// descriptor (`nested`, the shape the run loop's source has).  A block that times out (300 ms) while `select` calls
/// the descriptor readable is a miss: `(:pipe-direct M :pipe-nested M :pty-direct M :pty-nested M)`.
fn kq_layers(arg: &str) -> ShellResult<String> {
    let n = first_number(arg)?;
    let mut out = String::from("(");
    for pty in [false, true] {
        for nested in [false, true] {
            let m = kq_layer_misses(pty, nested, n).ok_or(ShellError::Arg)?;
            out.push_str(&format!(
                ":{}-{} {m} ",
                if pty { "pty" } else { "pipe" },
                if nested { "nested" } else { "direct" }
            ));
        }
    }
    out.pop();
    out.push(')');
    Ok(out)
}

/// One `test-kq-layers` run: the misses, `None` when a descriptor could not be made.
fn kq_layer_misses(pty: bool, nested: bool, n: u64) -> Option<u64> {
    let ms = |m: i64| libc::timespec {
        tv_sec: 0,
        tv_nsec: m * 1_000_000,
    };
    let readable = |fd: i32| {
        // SAFETY: a one-entry pollfd array on the stack.
        let mut p = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe { libc::poll(&mut p, 1, 0) > 0 && p.revents & libc::POLLIN != 0 }
    };
    let drain = |fd: i32| {
        let mut got = 0u64;
        let mut buf = [0u8; 256];
        // SAFETY: our non-blocking read end; `buf` is writable.
        loop {
            let r = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if r <= 0 {
                return got;
            }
            got += r as u64;
        }
    };
    // SAFETY: plain syscalls on descriptors made, used and closed here; kevents are plain data.
    unsafe {
        let mut fds = [-1i32; 2];
        if pty {
            // [read, write] = [master, slave]; raw, so a byte reaches the master as written
            let (mut m, mut s) = (-1, -1);
            if libc::openpty(
                &mut m,
                &mut s,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) < 0
            {
                return None;
            }
            let mut t: libc::termios = std::mem::zeroed();
            libc::tcgetattr(s, &mut t);
            libc::cfmakeraw(&mut t);
            libc::tcsetattr(s, libc::TCSANOW, &t);
            fds = [m, s];
        } else if libc::pipe(fds.as_mut_ptr()) < 0 {
            return None;
        }
        let (rd, wr) = (fds[0], fds[1]);
        libc::fcntl(rd, libc::F_SETFL, libc::O_NONBLOCK);
        let reg = |kq: i32, fd: i32| {
            let mut ev: libc::kevent = std::mem::zeroed();
            ev.ident = fd as usize;
            ev.filter = libc::EVFILT_READ;
            ev.flags = libc::EV_ADD;
            libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null())
        };
        let inner = libc::kqueue();
        let outer = libc::kqueue();
        reg(inner, rd);
        reg(outer, inner);
        let writer = std::thread::spawn(move || {
            let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
            for _ in 0..n {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                std::thread::sleep(Duration::from_micros(x % 3000));
                libc::write(wr, b"x".as_ptr().cast(), 1);
            }
        });
        let (mut got, mut misses) = (0u64, 0u64);
        let end = Instant::now() + Duration::from_secs(30);
        let mut evs = [std::mem::zeroed::<libc::kevent>(); 8];
        while got < n && Instant::now() < end {
            if libc::kevent(inner, std::ptr::null(), 0, evs.as_mut_ptr(), 8, &ms(0)) > 0 {
                got += drain(rd);
                continue;
            }
            let on = if nested { outer } else { inner };
            if libc::kevent(on, std::ptr::null(), 0, evs.as_mut_ptr(), 8, &ms(300)) == 0
                && readable(rd)
            {
                misses += 1;
            }
            got += drain(rd);
        }
        let _ = writer.join();
        for fd in [inner, outer, rd, wr] {
            libc::close(fd);
        }
        Some(misses)
    }
}

/// The overhang switch's captures (`test-capture-switch` R G B): armed with a colour, the next switch captures the
/// screen around its root right after the panel is ordered in and after the transaction that moves the container, and
/// counts the pixels of that colour (`None`: no capture).
#[derive(Debug, Default)]
pub struct Capture {
    pub armed: Option<[u8; 3]>,
    pub counts: Vec<Option<u64>>,
}

type CFRef = *const std::ffi::c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGImageGetWidth(i: CFRef) -> usize;
    fn CGImageGetHeight(i: CFRef) -> usize;
    fn CGImageGetBytesPerRow(i: CFRef) -> usize;
    fn CGImageGetBitsPerPixel(i: CFRef) -> usize;
    fn CGImageGetDataProvider(i: CFRef) -> CFRef;
    fn CGDataProviderCopyData(p: CFRef) -> CFRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataGetBytePtr(d: CFRef) -> *const u8;
    fn CFDataGetLength(d: CFRef) -> isize;
    fn CFRelease(r: CFRef);
}

/// `CGWindowListCreateImage`, unavailable in the macOS 15 SDK but still in the library: looked up at run time.
type CreateImage = unsafe extern "C" fn(NSRect, u32, u32, u32) -> CFRef;

/// The pixels of `rgb` (each channel within 60) in a capture of `rect` (Core Graphics' global points, from the main
/// display's top left), counted in device pixels; `None` when the capture is refused.
fn count_colour(rect: NSRect, rgb: [u8; 3]) -> Option<u64> {
    // SAFETY: a symbol lookup in the loaded images; the name is a C string.
    let f = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"CGWindowListCreateImage".as_ptr()) };
    if f.is_null() {
        return None;
    }
    // SAFETY: the symbol is CGWindowListCreateImage, of this signature.
    let create: CreateImage = unsafe { std::mem::transmute(f) };
    // on-screen windows (kCGWindowListOptionOnScreenOnly), no window id, the default image
    // SAFETY: a +1 image or null; released below.
    let img = unsafe { create(rect, 1, 0, 0) };
    if img.is_null() {
        return None;
    }
    // SAFETY: a live image and its provider's data copy, released once each; the reads stay inside the data.
    unsafe {
        let (w, h, row) = (
            CGImageGetWidth(img),
            CGImageGetHeight(img),
            CGImageGetBytesPerRow(img),
        );
        let ok = CGImageGetBitsPerPixel(img) == 32;
        let data = CGDataProviderCopyData(CGImageGetDataProvider(img));
        let mut n = 0u64;
        if ok && !data.is_null() {
            let len = CFDataGetLength(data) as usize;
            let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(data), len);
            let near = |a: u8, b: u8| a.abs_diff(b) <= 60;
            for y in 0..h {
                for x in 0..w {
                    let i = y * row + x * 4;
                    // BGRA, the window server's order
                    if i + 3 < len
                        && near(bytes[i + 2], rgb[0])
                        && near(bytes[i + 1], rgb[1])
                        && near(bytes[i], rgb[2])
                    {
                        n += 1;
                    }
                }
            }
        }
        if !data.is_null() {
            CFRelease(data);
        }
        CFRelease(img);
        ok.then_some(n)
    }
}

impl MacShell {
    /// A capture point of an armed `test-capture-switch` (`step` 0: the panel is in, 1: the container moved): the
    /// screen around root `root`'s window, grown by its own size on each side.  The second disarms it.
    pub(super) fn capture_point(&mut self, root: WindowId, step: u32) {
        let Some(rgb) = self.capture.armed else {
            return;
        };
        let Some(w) = self.windows.get(&root) else {
            return;
        };
        let c = w.window.contentRectForFrameRect(w.window.frame());
        let main_h = objc2_app_kit::NSScreen::screens(self.mtm)
            .firstObject()
            .map_or(0.0, |s| s.frame().size.height);
        let (gw, gh) = (c.size.width, c.size.height);
        let rect = NSRect::new(
            objc2_foundation::NSPoint::new(c.origin.x - gw, main_h - (c.origin.y + gh) - gh),
            NSSize::new(gw * 3.0, gh * 3.0),
        );
        self.capture.counts.push(count_colour(rect, rgb));
        if step == 1 {
            self.capture.armed = None;
        }
    }

    /// `test-windows`: the window numbers the window server lists for this application's visible windows.
    fn window_numbers(&self) -> String {
        let n = objc2_app_kit::NSWindow::windowNumbersWithOptions(
            objc2_app_kit::NSWindowNumberListOptions(0),
            self.mtm,
        );
        let list: Vec<String> = n
            .map(|a| a.iter().map(|x| x.integerValue().to_string()).collect())
            .unwrap_or_default();
        format!("({})", list.join(" "))
    }
}
