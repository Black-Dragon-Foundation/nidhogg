//! The keyboard: xkb keymaps, compose, modifiers and key repeat (W0 Task 7).  Runs on the reader thread, so it never
//! blocks and never calls into the host.
//!
//! - `Xkb` turns evdev scancodes into keysyms, UTF-8 and modifier bits.  Modifier bits come from xkb's named modifiers
//!   (`Shift`, `Control`, `Mod1` as ALT, and the virtual `Super`, `Hyper`, `Meta`); a keymap without a virtual `Super`
//!   falls back to `Mod4`.  Compose sequences swallow the keys that start and continue them: the composed keysym and
//!   UTF-8 arrive with the key that ends the sequence.
//! - `RepeatState` is the repeat state machine, clock-free: the caller passes the time.  A repeat is dropped while the
//!   previous repeat of the same key is still unread in the host's queue, so a stalled Lisp finds at most one.
//! - `KeyboardState` joins them with the seat's events, the timerfd the reader polls, and the focus.

use crate::event::*;
use crate::wayland::conn::Shared;
use std::ffi::{OsStr, OsString};
use std::os::fd::{AsRawFd, OwnedFd};
use xkbcommon::xkb;

const NONE: xkb::ModIndex = xkb::MOD_INVALID;

/// The indices of the modifiers the shell reports, `MOD_INVALID` when the keymap has none of that name.
struct ModIdx {
    shift: xkb::ModIndex,
    ctrl: xkb::ModIndex,
    alt: xkb::ModIndex,
    sup: xkb::ModIndex,
    hyper: xkb::ModIndex,
    meta: xkb::ModIndex,
    caps: xkb::ModIndex,
}

impl ModIdx {
    fn new(keymap: &xkb::Keymap) -> ModIdx {
        let idx = |n: &str| keymap.mod_get_index(n);
        let mut sup = idx("Super");
        if sup == NONE {
            sup = idx(xkb::MOD_NAME_LOGO);
        }
        ModIdx {
            shift: idx(xkb::MOD_NAME_SHIFT),
            ctrl: idx(xkb::MOD_NAME_CTRL),
            alt: idx(xkb::MOD_NAME_ALT),
            sup,
            hyper: idx("Hyper"),
            meta: idx("Meta"),
            caps: idx(xkb::MOD_NAME_CAPS),
        }
    }
}

/// What a key press or release produced.
#[derive(Clone, Copy)]
pub struct Keyed {
    pub keysym: u32,
    pub utf8: [u8; 16],
}

fn utf8_buf(s: &str) -> [u8; 16] {
    let mut end = s.len().min(15);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = [0u8; 16];
    out[..end].copy_from_slice(&s.as_bytes()[..end]);
    out
}

/// The locale compose reads: `LC_ALL`, then `LC_CTYPE`, then `LANG`, else `C`.
pub fn locale_from_env() -> OsString {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(std::env::var_os)
        .find(|v| !v.is_empty())
        .unwrap_or_else(|| "C".into())
}

pub struct Xkb {
    keymap: xkb::Keymap,
    state: xkb::State,
    compose: Option<xkb::compose::State>,
    mods: ModIdx,
    /// Keys whose press compose swallowed; their release is swallowed too.
    swallowed: Vec<u32>,
}

impl Xkb {
    pub fn new(keymap: xkb::Keymap, locale: &OsStr) -> Xkb {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let compose =
            xkb::compose::Table::new_from_locale(&ctx, locale, xkb::compose::COMPILE_NO_FLAGS)
                .ok()
                .map(|t| xkb::compose::State::new(&t, xkb::compose::STATE_NO_FLAGS));
        Xkb {
            state: xkb::State::new(&keymap),
            mods: ModIdx::new(&keymap),
            keymap,
            compose,
            swallowed: Vec::new(),
        }
    }

    /// Compile a keymap from names (rules, model, layout, variant, options).
    pub fn from_rmlvo(rmlvo: [&str; 5], locale: &OsStr) -> Option<Xkb> {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let [rules, model, layout, variant, options] = rmlvo;
        let opts = (!options.is_empty()).then(|| options.to_string());
        xkb::Keymap::new_from_names(
            &ctx,
            rules,
            model,
            layout,
            variant,
            opts,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .map(|k| Xkb::new(k, locale))
    }

    /// Compile the keymap text the compositor sent.
    pub fn from_text(text: String, locale: &OsStr) -> Option<Xkb> {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        xkb::Keymap::new_from_string(
            &ctx,
            text,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .map(|k| Xkb::new(k, locale))
    }

    /// The `NHSHELL_MOD_*` bits of the effective modifiers.
    pub fn mods(&self) -> u32 {
        let m = &self.mods;
        [
            (m.shift, NHSHELL_MOD_SHIFT),
            (m.ctrl, NHSHELL_MOD_CTRL),
            (m.alt, NHSHELL_MOD_ALT),
            (m.sup, NHSHELL_MOD_SUPER),
            (m.hyper, NHSHELL_MOD_HYPER),
            (m.meta, NHSHELL_MOD_META),
            (m.caps, NHSHELL_MOD_CAPS),
        ]
        .into_iter()
        .filter(|&(i, _)| i != NONE && self.state.mod_index_is_active(i, xkb::STATE_MODS_EFFECTIVE))
        .fold(0, |acc, (_, bit)| acc | bit)
    }

    /// `wl_keyboard.modifiers`.
    pub fn update_mask(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        self.state
            .update_mask(depressed, latched, locked, 0, 0, group);
    }

    /// Track a key in the modifier state, as the compositor would.  The shell gets modifiers from `update_mask`; this
    /// is for tests and tools that have no compositor.
    pub fn track_key(&mut self, evdev: u32, pressed: bool) {
        let dir = if pressed {
            xkb::KeyDirection::Down
        } else {
            xkb::KeyDirection::Up
        };
        self.state.update_key(xkb::Keycode::new(evdev + 8), dir);
    }

    /// Whether holding `evdev` repeats.
    pub fn repeats(&self, evdev: u32) -> bool {
        self.keymap.key_repeats(xkb::Keycode::new(evdev + 8))
    }

    /// A press or release of `evdev` (a scancode).  `None` while compose swallows the key.
    pub fn key(&mut self, evdev: u32, pressed: bool) -> Option<Keyed> {
        let code = xkb::Keycode::new(evdev + 8);
        let plain = |s: &xkb::State| Keyed {
            keysym: s.key_get_one_sym(code).raw(),
            utf8: utf8_buf(&s.key_get_utf8(code)),
        };
        if !pressed {
            if let Some(i) = self.swallowed.iter().position(|&k| k == evdev) {
                self.swallowed.swap_remove(i);
                return None;
            }
            return Some(plain(&self.state));
        }
        let mut out = plain(&self.state);
        if let Some(c) = self.compose.as_mut()
            && c.feed(self.state.key_get_one_sym(code)) == xkb::compose::FeedResult::Accepted
        {
            match c.status() {
                xkb::compose::Status::Composing | xkb::compose::Status::Cancelled => {
                    if c.status() == xkb::compose::Status::Cancelled {
                        c.reset();
                    }
                    self.swallowed.push(evdev);
                    return None;
                }
                xkb::compose::Status::Composed => {
                    if let Some(sym) = c.keysym() {
                        out.keysym = sym.raw();
                    }
                    out.utf8 = utf8_buf(&c.utf8().unwrap_or_default());
                    c.reset();
                }
                xkb::compose::Status::Nothing => {}
            }
        }
        Some(out)
    }

    /// Drop a half-typed compose sequence (focus left).
    pub fn reset_compose(&mut self) {
        if let Some(c) = self.compose.as_mut() {
            c.reset();
        }
        self.swallowed.clear();
    }
}

/// The repeat state machine.  Times are milliseconds on any monotonic clock the caller keeps.
#[derive(Default)]
pub struct RepeatState {
    /// Keys per second; 0 disables repeat.
    rate: u32,
    delay_ms: u32,
    held: Option<(u32, u64)>,
}

impl RepeatState {
    pub fn set_info(&mut self, rate: i32, delay_ms: i32) {
        self.rate = rate.max(0) as u32;
        self.delay_ms = delay_ms.max(0) as u32;
    }

    /// `key` went down at `now`; the last key pressed is the one that repeats.
    pub fn press(&mut self, key: u32, now: u64) {
        self.held = (self.rate > 0).then(|| (key, now + u64::from(self.delay_ms)));
    }

    pub fn release(&mut self, key: u32) {
        if self.held.is_some_and(|(k, _)| k == key) {
            self.held = None;
        }
    }

    pub fn stop(&mut self) {
        self.held = None;
    }

    pub fn held_key(&self) -> Option<u32> {
        self.held.map(|(k, _)| k)
    }

    /// When the next repeat is due.
    pub fn deadline(&self) -> Option<u64> {
        self.held.map(|(_, t)| t)
    }

    /// The repeat due at `now`, if any.  `unread` says the previous repeat of the key still sits in the host's queue:
    /// the schedule moves on, and nothing is returned (coalescing).  Missed periods are skipped, not replayed.
    pub fn tick(&mut self, now: u64, unread: bool) -> Option<u32> {
        let (key, next) = self.held.as_mut()?;
        if now < *next {
            return None;
        }
        let period = u64::from(1000 / self.rate.max(1)).max(1);
        *next += ((now - *next) / period + 1) * period;
        (!unread).then_some(*key)
    }
}

fn now_ms() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid out-pointer for the call.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000
}

/// The keyboard of one seat, owned by the reader's state.
pub struct KeyboardState {
    xkb: Option<Xkb>,
    repeat: RepeatState,
    timer: Option<OwnedFd>,
    locale: OsString,
    /// The surface that has keyboard focus (the window id), 0 for none.
    focus: u64,
    /// The press the compositor-less repeat replays.
    held: Option<KeyEv>,
    last_mods: u32,
    /// The seat is version 10 or newer: the compositor sends `repeated` key states itself.
    compositor_repeats: bool,
}

// SAFETY: the xkb objects are plain heap data with no thread affinity and are never shared.  `ReaderState` (with this
// inside) moves to the reader thread once, before any keymap arrives on it, and only that thread touches them after.
unsafe impl Send for KeyboardState {}

impl KeyboardState {
    pub fn new() -> KeyboardState {
        // SAFETY: plain syscall; the descriptor is owned right away.
        let fd = unsafe {
            libc::timerfd_create(
                libc::CLOCK_MONOTONIC,
                libc::TFD_NONBLOCK | libc::TFD_CLOEXEC,
            )
        };
        let timer = (fd >= 0).then(|| {
            // SAFETY: `fd` is a fresh descriptor nothing else owns.
            unsafe { std::os::fd::FromRawFd::from_raw_fd(fd) }
        });
        KeyboardState {
            xkb: None,
            repeat: RepeatState::default(),
            timer,
            locale: locale_from_env(),
            focus: 0,
            held: None,
            last_mods: 0,
            compositor_repeats: false,
        }
    }

    /// The timerfd the reader polls (`None` when the system could not make one: no repeat then).
    pub fn timer_fd(&self) -> Option<&OwnedFd> {
        self.timer.as_ref()
    }

    pub fn set_compositor_repeats(&mut self, on: bool) {
        self.compositor_repeats = on;
    }

    pub fn set_keymap(&mut self, text: String) {
        self.stop_repeat();
        self.xkb = Xkb::from_text(text, &self.locale);
        self.last_mods = 0;
    }

    pub fn set_repeat_info(&mut self, rate: i32, delay: i32) {
        self.repeat.set_info(rate, delay);
        self.rearm();
    }

    pub fn enter(&mut self, surface: u64) {
        self.focus = surface;
    }

    pub fn leave(&mut self, surface: u64) {
        self.stop_repeat();
        if let Some(x) = self.xkb.as_mut() {
            x.reset_compose();
        }
        if self.focus == surface {
            self.focus = 0;
        }
    }

    pub fn modifiers(&mut self, d: u32, l: u32, k: u32, group: u32, out: &mut Vec<NhShellEvent>) {
        let Some(x) = self.xkb.as_mut() else { return };
        x.update_mask(d, l, k, group);
        let mods = x.mods();
        if mods != self.last_mods {
            self.last_mods = mods;
            let mut ev = NhShellEvent::new(NHSHELL_EV_MODIFIERS, self.focus, 0);
            ev.u.key = KeyEv {
                keycode: 0,
                keysym: 0,
                mods,
                pressed: 0,
                repeat: 0,
                utf8: [0; 16],
            };
            out.push(ev);
        }
    }

    /// `wl_keyboard.key`: `state` 0 released, 1 pressed, 2 repeated (seat 10).
    pub fn key(&mut self, time_ms: u32, evdev: u32, state: u32, out: &mut Vec<NhShellEvent>) {
        let Some(x) = self.xkb.as_mut() else { return };
        let pressed = state != 0;
        let Some(k) = x.key(evdev, pressed) else {
            return;
        };
        let key = KeyEv {
            keycode: evdev,
            keysym: k.keysym,
            mods: x.mods(),
            pressed: u8::from(pressed),
            repeat: u8::from(state == 2),
            utf8: k.utf8,
        };
        let mut ev = NhShellEvent::new(NHSHELL_EV_KEY, self.focus, u64::from(time_ms) * 1_000_000);
        ev.u.key = key;
        out.push(ev);
        if self.compositor_repeats {
            return;
        }
        if pressed && x.repeats(evdev) {
            self.repeat.press(evdev, now_ms());
            self.held = Some(key);
        } else if !pressed {
            self.repeat.release(evdev);
            if self.held.is_some_and(|h| h.keycode == evdev) {
                self.held = None;
            }
        }
        self.rearm();
    }

    /// The timerfd fired: post the repeat that is due, unless the previous one is still unread.
    pub fn on_timer(&mut self, shared: &Shared, out: &mut Vec<NhShellEvent>) {
        if let Some(fd) = &self.timer {
            let mut buf = [0u8; 8];
            // SAFETY: reads at most 8 bytes into `buf`; a nonblocking timerfd.
            unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), 8) };
        }
        let now = now_ms();
        let unread = self
            .repeat
            .held_key()
            .is_some_and(|k| shared.has_unread_repeat(k));
        if let (Some(key), Some(held)) = (self.repeat.tick(now, unread), self.held)
            && held.keycode == key
        {
            let mut ev = NhShellEvent::new(NHSHELL_EV_KEY, self.focus, now * 1_000_000);
            ev.u.key = KeyEv { repeat: 1, ..held };
            out.push(ev);
        }
        self.rearm();
    }

    fn stop_repeat(&mut self) {
        self.repeat.stop();
        self.held = None;
        self.rearm();
    }

    /// Point the timerfd at the next repeat, or disarm it.
    fn rearm(&self) {
        let Some(fd) = &self.timer else { return };
        let ns = self.repeat.deadline().map_or(0, |t| {
            // 0 would disarm; a due repeat fires at once
            (t.saturating_sub(now_ms()) * 1_000_000).max(1)
        });
        let spec = libc::itimerspec {
            it_interval: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            it_value: libc::timespec {
                tv_sec: (ns / 1_000_000_000) as _,
                tv_nsec: (ns % 1_000_000_000) as _,
            },
        };
        // SAFETY: `spec` is a valid itimerspec and the old-value pointer may be null.
        unsafe { libc::timerfd_settime(fd.as_raw_fd(), 0, &spec, std::ptr::null_mut()) };
    }
}

impl Default for KeyboardState {
    fn default() -> Self {
        Self::new()
    }
}
