//! Event records: what the shell hands the host, `#[repr(C)]` twins of `NhShellEvent` in `include/nhshell.h`.
//!
//! One record type for every backend.  `kind` says which member of the union `u` is valid; kinds without a member
//! (`CLOSE`, `DISCONNECTED`, `POINTER_LEAVE`, `FRAME_READY`) leave it zeroed.  `MODIFIERS` uses `key.mods`; `POINTER_ENTER`,
//! `POINTER_MOTION` and `POINTER_LEAVE` use `pointer.x`/`y`; `BUTTON` uses `pointer.button`/`state`; `OUTPUT_REMOVE`
//! uses `output.id`.  Window sizes are logical pixels; pointer positions are device pixels (logical x scale);
//! `time_ns` is the compositor's clock in nanoseconds.

pub const NHSHELL_EV_KEY: u32 = 1;
pub const NHSHELL_EV_MODIFIERS: u32 = 2;
pub const NHSHELL_EV_POINTER_ENTER: u32 = 3;
pub const NHSHELL_EV_POINTER_LEAVE: u32 = 4;
pub const NHSHELL_EV_POINTER_MOTION: u32 = 5;
pub const NHSHELL_EV_BUTTON: u32 = 6;
pub const NHSHELL_EV_SCROLL: u32 = 7;
pub const NHSHELL_EV_CONFIGURE: u32 = 8;
pub const NHSHELL_EV_CLOSE: u32 = 9;
pub const NHSHELL_EV_SCALE: u32 = 10;
pub const NHSHELL_EV_FOCUS: u32 = 11;
pub const NHSHELL_EV_FRAME_DONE: u32 = 12;
pub const NHSHELL_EV_OUTPUT_ADD: u32 = 13;
pub const NHSHELL_EV_OUTPUT_REMOVE: u32 = 14;
pub const NHSHELL_EV_DISCONNECTED: u32 = 15;
/// The window's pacer lets the host present what it drew (after `window_dirty`, an ack, a frame callback or a release).
/// It may come twice for one frame: `window_ready` says whether to present.  16–20 belong to popups and children.
pub const NHSHELL_EV_FRAME_READY: u32 = 21;
/// The compositor dismissed a popup (no payload); the window is still there until the host destroys it.
pub const NHSHELL_EV_POPUP_DONE: u32 = 16;
/// The clipboard or the primary selection changed (ours or another client's); `u.selection.kind` says which.
pub const NHSHELL_EV_SELECTION: u32 = 17;
// text-input-v3 (`u.text`); the text is fetched with `nhshell_take_text`
/// The preedit changed: `u.text.id`/`len` its text (len 0: cleared), `cursor_begin`/`cursor_end` bytes (-1: hidden).
pub const NHSHELL_EV_PREEDIT: u32 = 18;
/// Text to insert: `u.text.id`, `u.text.len`.
pub const NHSHELL_EV_COMMIT_TEXT: u32 = 19;
/// Delete `u.text.before` bytes before and `u.text.after` bytes after the cursor.
pub const NHSHELL_EV_DELETE_SURROUNDING: u32 = 20;
/// An output announced by `OUTPUT_ADD` changed (same payload; sent only when a field differs).
pub const NHSHELL_EV_OUTPUT_CHANGE: u32 = 22;
/// The window is now on output `u.output.id` (other members zero).
pub const NHSHELL_EV_ENTER_OUTPUT: u32 = 23;
/// The window left output `u.output.id`.
pub const NHSHELL_EV_LEAVE_OUTPUT: u32 = 24;
/// A touch point went down, moved, lifted or was cancelled (`u.touch`), on the window it went down on.  The records
/// of one touch frame are posted together when the frame ends; a cancel posts `NHSHELL_TOUCH_CANCEL` for every live
/// point at once.
pub const NHSHELL_EV_TOUCH: u32 = 39;
/// A touchpad pinch began, changed or ended (`u.pinch`), on the window under the pointer.
pub const NHSHELL_EV_PINCH: u32 = 40;

pub const NHSHELL_MOD_SHIFT: u32 = 1 << 0;
pub const NHSHELL_MOD_CTRL: u32 = 1 << 1;
pub const NHSHELL_MOD_ALT: u32 = 1 << 2;
pub const NHSHELL_MOD_SUPER: u32 = 1 << 3;
pub const NHSHELL_MOD_HYPER: u32 = 1 << 4;
pub const NHSHELL_MOD_META: u32 = 1 << 5;
pub const NHSHELL_MOD_CAPS: u32 = 1 << 6;

pub const NHSHELL_STATE_MAXIMIZED: u32 = 1 << 0;
pub const NHSHELL_STATE_FULLSCREEN: u32 = 1 << 1;
pub const NHSHELL_STATE_ACTIVATED: u32 = 1 << 2;
pub const NHSHELL_STATE_RESIZING: u32 = 1 << 3;
pub const NHSHELL_STATE_TILED_L: u32 = 1 << 4;
pub const NHSHELL_STATE_TILED_R: u32 = 1 << 5;
pub const NHSHELL_STATE_TILED_T: u32 = 1 << 6;
pub const NHSHELL_STATE_TILED_B: u32 = 1 << 7;
pub const NHSHELL_STATE_SUSPENDED: u32 = 1 << 8;

/// `pointer.state` and `key`-like press state for buttons.
pub const NHSHELL_BUTTON_RELEASED: u32 = 0;
pub const NHSHELL_BUTTON_PRESSED: u32 = 1;

/// `scroll.source`.
pub const NHSHELL_SCROLL_WHEEL: u32 = 0;
pub const NHSHELL_SCROLL_FINGER: u32 = 1;
pub const NHSHELL_SCROLL_CONTINUOUS: u32 = 2;

/// `touch.op`.
pub const NHSHELL_TOUCH_DOWN: u32 = 0;
pub const NHSHELL_TOUCH_MOTION: u32 = 1;
pub const NHSHELL_TOUCH_UP: u32 = 2;
pub const NHSHELL_TOUCH_CANCEL: u32 = 3;

/// `pinch.phase`.
pub const NHSHELL_PINCH_BEGIN: u32 = 0;
pub const NHSHELL_PINCH_UPDATE: u32 = 1;
pub const NHSHELL_PINCH_END: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KeyEv {
    /// The evdev scancode.
    pub keycode: u32,
    pub keysym: u32,
    /// `NHSHELL_MOD_*` bits after this event.
    pub mods: u32,
    pub pressed: u8,
    pub repeat: u8,
    /// The text the key produced, NUL-terminated (compose and dead keys applied); empty for none.
    pub utf8: [u8; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PointerEv {
    pub x: f64,
    pub y: f64,
    /// An evdev button code (`BTN_LEFT` = 0x110).
    pub button: u32,
    pub state: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScrollEv {
    pub dx: f64,
    pub dy: f64,
    /// High-resolution wheel steps (120 per notch), 0 when the source reports none.
    pub v120x: i32,
    pub v120y: i32,
    pub source: u32,
    pub stop: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ConfigureEv {
    /// The size the compositor asks for, in logical pixels; 0 means "your choice".
    pub w: i32,
    pub h: i32,
    pub states: u32,
    /// Hand it to `nhshell_window_ack_configure` once a buffer of that size is committed.
    pub serial: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScaleEv {
    /// The preferred scale times 120.
    pub scale120: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FocusEv {
    pub r#in: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FrameDoneEv {
    /// 0 when the compositor sent no presentation feedback.
    pub presented_ns: u64,
    pub refresh_ns: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OutputEv {
    pub id: u64,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub mm_w: i32,
    pub mm_h: i32,
    pub scale120: u32,
    /// NUL-terminated.
    pub name: [u8; 32],
}

/// text-input-v3 records; lengths and offsets are bytes of UTF-8.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TextEv {
    /// The text for `nhshell_take_text`; 0 for none.
    pub id: u32,
    pub len: u32,
    pub cursor_begin: i32,
    pub cursor_end: i32,
    pub before: u32,
    pub after: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SelectionEv {
    /// `NHSHELL_SEL_CLIPBOARD` or `NHSHELL_SEL_PRIMARY`.
    pub kind: u32,
}

/// One touch point.  Positions are device pixels in the window the point went down on; UP and CANCEL carry the
/// point's last position.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TouchEv {
    /// The point's id, unique while it is down.
    pub id: i32,
    /// `NHSHELL_TOUCH_*`.
    pub op: u32,
    pub x: f64,
    pub y: f64,
}

/// A touchpad pinch.  `dx`, `dy` are this update's motion of the fingers' centre (device pixels); `scale` is
/// cumulative since BEGIN (1.0 at BEGIN; END repeats the last); `rotation` is this update's angle in degrees,
/// clockwise; `x`, `y` are the pointer's position (device pixels).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PinchEv {
    /// `NHSHELL_PINCH_*`.
    pub phase: u32,
    pub fingers: u32,
    pub dx: f64,
    pub dy: f64,
    pub scale: f64,
    pub rotation: f64,
    pub x: f64,
    pub y: f64,
    /// END only: the gesture was cancelled.
    pub cancelled: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union NhShellEventData {
    pub key: KeyEv,
    pub pointer: PointerEv,
    pub scroll: ScrollEv,
    pub configure: ConfigureEv,
    pub scale: ScaleEv,
    pub focus: FocusEv,
    pub frame_done: FrameDoneEv,
    pub output: OutputEv,
    pub selection: SelectionEv,
    pub text: TextEv,
    pub touch: TouchEv,
    pub pinch: PinchEv,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NhShellEvent {
    pub kind: u32,
    pub window: u64,
    pub time_ns: u64,
    pub u: NhShellEventData,
}

impl NhShellEvent {
    /// An event of `kind` for `window` with a zeroed payload.
    pub fn new(kind: u32, window: u64, time_ns: u64) -> Self {
        NhShellEvent {
            kind,
            window,
            time_ns,
            // SAFETY: every member is plain integers and floats, for which all-zero is valid.
            u: unsafe { std::mem::zeroed() },
        }
    }
}
