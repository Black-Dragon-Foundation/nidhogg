//! The actions `nhg_next` answers with: one shell event's, which C applies before it asks for the next event (design
//! §4, review B1).  The actions and their texts stay valid until the next `nhg_next` on the display: only that call
//! frees them, so no other call C makes while it applies them can.

use nhshell::event::NhShellEvent;
use std::ffi::c_char;

/// `NhgNextIn` flags, sampled by C before each `nhg_next`: `kbd_buffer_events_waiting ()` and
/// `mwheel-coalesce-scroll-events`.
pub const NHG_IN_INPUT_WAITING: u32 = 1;
pub const NHG_IN_COALESCE_WHEEL: u32 = 2;

/// Action kinds 1–31 are the port's; 32–63 belong to W2's and W3's glue.
pub const NHG_ACT_KEY: u32 = 1;
pub const NHG_ACT_CLICK: u32 = 2;
pub const NHG_ACT_WHEEL: u32 = 3;
pub const NHG_ACT_MOTION: u32 = 4;
pub const NHG_ACT_LEAVE: u32 = 5;
pub const NHG_ACT_RESIZE: u32 = 6;
pub const NHG_ACT_SCALE: u32 = 7;
pub const NHG_ACT_GARBAGE: u32 = 8;
pub const NHG_ACT_DELETE: u32 = 9;
pub const NHG_ACT_FOCUS_IN: u32 = 10;
pub const NHG_ACT_FOCUS_OUT: u32 = 11;
pub const NHG_ACT_VISIBLE: u32 = 12;
pub const NHG_ACT_ICONIFIED: u32 = 13;
pub const NHG_ACT_READY: u32 = 14;
pub const NHG_ACT_MONITORS: u32 = 15;
pub const NHG_ACT_LOST: u32 = 16;
pub const NHG_ACT_PREEDIT: u32 = 17;
pub const NHG_ACT_COMMIT: u32 = 18;
pub const NHG_ACT_IM_DELETE: u32 = 19;
pub const NHG_ACT_IM_SURROUNDING: u32 = 20;
pub const NHG_ACT_SELECTION_CLEAR: u32 = 21;
/// A pointer event went through the window's title bar or its margins (`csd.rs`): `flags` `NHG_TB_*`, `code` the
/// pointer shape (`NHSHELL_CURSOR_*`) with `NHG_TB_POINTER`.  A raw action of the same event, if any, comes after it.
pub const NHG_ACT_TITLE_BAR: u32 = 22;
/// Transitional: a shell event whose kind is not ported yet, which C handles on its old path (`nhg_raw_event` gives
/// it).  A `PREEDIT` or `COMMIT_TEXT` event's text comes with it as the action's text.  Each port task takes its kinds
/// out (the pointer, the input method, outputs, selections, popups); the kind goes when none is left.
pub const NHG_ACT_RAW: u32 = 31;

/// `RESIZE` flags: fullscreen or maximized changed (C stores the frame's `fullscreen` parameter).
pub const NHG_RESIZE_FULLSCREEN: u32 = 1;

/// What C passes to each `nhg_next`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NhgNextIn {
    /// `NHG_IN_*`.
    pub flags: u32,
}

/// One action.  The fields' meaning depends on the kind:
/// - `KEY` (a press or a repeat): `window` the window with the keyboard (C resolves the frame of its tree that Emacs
///   focused), `code` the keysym, `text` its UTF-8 (possibly empty), `mods` the `NHSHELL_MOD_*` mask, `flags`
///   `NHG_KEY_REPEAT` for a repeat;
/// - `FOCUS_IN`, `FOCUS_OUT`: the window whose keyboard focus changed (C resolves Emacs's focus frame);
/// - `RESIZE`: `w`, `h` the compositor's suggestion in logical pixels (0: the frame's own), `code` the toplevel's
///   `NHSHELL_STATE_*`, `flags` `NHG_RESIZE_*`;
/// - `SCALE`: `code` the scale times 120, `scale` the factor;
/// - `DELETE`, `READY`, `LOST`: the window only (`LOST`: none);
/// - `RAW`: the shell event (`nhg_raw_event`), with a `PREEDIT`'s or `COMMIT_TEXT`'s text in `text`, `len` (null
///   when it could not be taken), and the keyboard's `NHSHELL_MOD_*` mask in `mods`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NhgAction {
    /// `NHG_ACT_*`.
    pub kind: u32,
    /// The shell window the event belongs to (0: none).
    pub window: u64,
    /// The event's time in milliseconds.
    pub time_ms: u64,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub code: i32,
    pub mods: u32,
    pub flags: u32,
    pub dx: f64,
    pub dy: f64,
    pub scale: f64,
    /// UTF-8, NUL-terminated after `len` bytes; null for none.
    pub text: *const c_char,
    pub len: usize,
}

impl NhgAction {
    pub fn new(kind: u32, window: u64, time_ns: u64) -> NhgAction {
        NhgAction {
            kind,
            window,
            time_ms: time_ns / 1_000_000,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            code: 0,
            mods: 0,
            flags: 0,
            dx: 0.0,
            dy: 0.0,
            scale: 0.0,
            text: std::ptr::null(),
            len: 0,
        }
    }
}

/// The actions of one event, borrowed until the next `nhg_next`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NhgActionList {
    pub v: *const NhgAction,
    pub n: usize,
}

/// One event's actions, their texts and (for a `RAW` action) the event, kept until the next `nhg_next`.
#[derive(Default)]
pub struct Actions {
    list: Vec<NhgAction>,
    /// Each text's bytes and a NUL; a text's heap buffer does not move when this list grows.
    texts: Vec<Vec<u8>>,
    raw: Option<NhShellEvent>,
}

impl Actions {
    /// Drop the last event's actions (only `nhg_next` calls this).
    pub fn clear(&mut self) {
        self.list.clear();
        self.texts.clear();
        self.raw = None;
    }

    pub fn push(&mut self, a: NhgAction) {
        self.list.push(a);
    }

    /// Push `a` with `text` (`None`: a null text).
    pub fn push_text(&mut self, mut a: NhgAction, text: Option<Vec<u8>>) {
        if let Some(mut t) = text {
            a.len = t.len();
            t.push(0);
            a.text = t.as_ptr().cast();
            self.texts.push(t);
        }
        self.list.push(a);
    }

    /// Keep `ev` for `nhg_raw_event`.
    pub fn set_raw(&mut self, ev: NhShellEvent) {
        self.raw = Some(ev);
    }

    pub fn raw(&self) -> Option<&NhShellEvent> {
        self.raw.as_ref()
    }

    pub fn as_slice(&self) -> &[NhgAction] {
        &self.list
    }

    pub fn as_list(&self) -> NhgActionList {
        NhgActionList {
            v: if self.list.is_empty() {
                std::ptr::null()
            } else {
                self.list.as_ptr()
            },
            n: self.list.len(),
        }
    }
}
