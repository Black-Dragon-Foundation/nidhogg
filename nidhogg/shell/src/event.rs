//! Event records: what the shell hands the host.
//!
//! One record type for every backend: the window it names, the compositor's time and the event with its payload.
//! Window sizes are logical pixels; pointer positions are device pixels (logical x scale); `time_ns` is the
//! compositor's clock in nanoseconds.

/// One event and its payload.
#[derive(Clone, Copy, Debug)]
pub enum ShellEvent {
    Key(KeyEv),
    /// The modifier mask changed: `NHSHELL_MOD_*` bits.
    Modifiers {
        mods: u32,
    },
    /// The pointer entered the window at `x`, `y`.
    PointerEnter(PointerEv),
    /// The pointer left the window (`x`, `y` 0).
    PointerLeave(PointerEv),
    /// The pointer moved to `x`, `y`.
    PointerMotion(PointerEv),
    /// A button was pressed or released: `button`, `state`.
    Button(PointerEv),
    Scroll(ScrollEv),
    Configure(ConfigureEv),
    Close,
    Scale(ScaleEv),
    Focus(FocusEv),
    FrameDone(FrameDoneEv),
    OutputAdd(OutputEv),
    /// Output `id` went (other fields zero).
    OutputRemove(OutputEv),
    /// After the connection was lost: the last record.
    Disconnected,
    /// The compositor dismissed a popup; the window is still there until the host destroys it.
    PopupDone,
    /// The clipboard or the primary selection changed (ours or another client's).
    Selection(SelectionEv),
    // text-input-v3; the text is fetched with `ShellBackend::take_text`
    /// The preedit changed: `id`/`len` its text (len 0: cleared), `cursor_begin`/`cursor_end` bytes (-1: hidden).
    Preedit(TextEv),
    /// Text to insert: `id`, `len`.
    CommitText(TextEv),
    /// Delete `before` bytes before and `after` bytes after the cursor.
    DeleteSurrounding(TextEv),
    /// The window's pacer lets the host present what it drew (after `window_dirty`, an ack, a frame callback or a
    /// release).  It may come twice for one frame: `window_ready` says whether to present.
    FrameReady,
    /// An output announced by `OutputAdd` changed (same payload; sent only when a field differs).
    OutputChange(OutputEv),
    /// The window is now on output `id` (other fields zero).
    EnterOutput(OutputEv),
    /// The window left output `id`.
    LeaveOutput(OutputEv),
    // Drag and drop.  The drop target's records name the window under the pointer (a child or popup too); the drag
    // source's name the window the drag started from.
    /// A drag entered one of our windows; its offer is `NHSHELL_SEL_DND` from now on.
    DndEnter(DndEv),
    /// The drag moved over the window.
    DndMotion(DndEv),
    /// The drag left the window (no drop): the offer is gone.
    DndLeave(DndEv),
    /// The drag was dropped on the window with `action`: read the offer, then `dnd_finish`.
    DndDrop(DndEv),
    /// Our drag: the action the compositor chose with the target changed (`action`, 0: none).
    DragAction(DndEv),
    /// Our drag: the user dropped it on a target that accepted it; `DragDone` comes when the target is finished.
    DragDropped(DndEv),
    /// Our drag ended: `ok` 1 when the target finished with `action`, 0 when it was cancelled (dropped on nothing,
    /// refused, or the display was lost: then before `Disconnected`).  Comes once per drag.
    DragDone(DndEv),
    /// A touch point went down, moved, lifted or was cancelled, on the window it went down on.  The records of one
    /// touch frame are posted together when the frame ends; a cancel posts `NHSHELL_TOUCH_CANCEL` for every live point
    /// at once.
    Touch(TouchEv),
    /// A touchpad pinch began, changed or ended, on the window under the pointer.
    Pinch(PinchEv),
    /// The compositor answered `session_open` (or took the session over).
    Session(SessionEv),
    /// The display went with reconnecting on (`ShellBackend::set_reconnect`): from the moment it is taken the shell is
    /// reconnecting, and the host drives `reconnect`.  The releases of held buttons, an empty `Preedit` and each
    /// window's `LeaveOutput` come before it.
    Reconnecting,
    /// `reconnect` succeeded: the shell is connected again.
    Reconnected,
}

/// A record's kind without its payload, for filters (`ShellEvent::kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Key,
    Modifiers,
    PointerEnter,
    PointerLeave,
    PointerMotion,
    Button,
    Scroll,
    Configure,
    Close,
    Scale,
    Focus,
    FrameDone,
    OutputAdd,
    OutputRemove,
    Disconnected,
    PopupDone,
    Selection,
    Preedit,
    CommitText,
    DeleteSurrounding,
    FrameReady,
    OutputChange,
    EnterOutput,
    LeaveOutput,
    DndEnter,
    DndMotion,
    DndLeave,
    DndDrop,
    DragAction,
    DragDropped,
    DragDone,
    Touch,
    Pinch,
    Session,
    Reconnecting,
    Reconnected,
}

impl ShellEvent {
    pub fn kind(&self) -> Kind {
        match self {
            ShellEvent::Key(_) => Kind::Key,
            ShellEvent::Modifiers { .. } => Kind::Modifiers,
            ShellEvent::PointerEnter(_) => Kind::PointerEnter,
            ShellEvent::PointerLeave(_) => Kind::PointerLeave,
            ShellEvent::PointerMotion(_) => Kind::PointerMotion,
            ShellEvent::Button(_) => Kind::Button,
            ShellEvent::Scroll(_) => Kind::Scroll,
            ShellEvent::Configure(_) => Kind::Configure,
            ShellEvent::Close => Kind::Close,
            ShellEvent::Scale(_) => Kind::Scale,
            ShellEvent::Focus(_) => Kind::Focus,
            ShellEvent::FrameDone(_) => Kind::FrameDone,
            ShellEvent::OutputAdd(_) => Kind::OutputAdd,
            ShellEvent::OutputRemove(_) => Kind::OutputRemove,
            ShellEvent::Disconnected => Kind::Disconnected,
            ShellEvent::PopupDone => Kind::PopupDone,
            ShellEvent::Selection(_) => Kind::Selection,
            ShellEvent::Preedit(_) => Kind::Preedit,
            ShellEvent::CommitText(_) => Kind::CommitText,
            ShellEvent::DeleteSurrounding(_) => Kind::DeleteSurrounding,
            ShellEvent::FrameReady => Kind::FrameReady,
            ShellEvent::OutputChange(_) => Kind::OutputChange,
            ShellEvent::EnterOutput(_) => Kind::EnterOutput,
            ShellEvent::LeaveOutput(_) => Kind::LeaveOutput,
            ShellEvent::DndEnter(_) => Kind::DndEnter,
            ShellEvent::DndMotion(_) => Kind::DndMotion,
            ShellEvent::DndLeave(_) => Kind::DndLeave,
            ShellEvent::DndDrop(_) => Kind::DndDrop,
            ShellEvent::DragAction(_) => Kind::DragAction,
            ShellEvent::DragDropped(_) => Kind::DragDropped,
            ShellEvent::DragDone(_) => Kind::DragDone,
            ShellEvent::Touch(_) => Kind::Touch,
            ShellEvent::Pinch(_) => Kind::Pinch,
            ShellEvent::Session(_) => Kind::Session,
            ShellEvent::Reconnecting => Kind::Reconnecting,
            ShellEvent::Reconnected => Kind::Reconnected,
        }
    }
}

/// `session.state`.
pub const NHSHELL_SESSION_CREATED: u32 = 0;
pub const NHSHELL_SESSION_RESTORED: u32 = 1;
pub const NHSHELL_SESSION_REPLACED: u32 = 2;

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

#[derive(Clone, Copy, Debug, Default)]
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

#[derive(Clone, Copy, Debug, Default)]
pub struct PointerEv {
    pub x: f64,
    pub y: f64,
    /// An evdev button code (`BTN_LEFT` = 0x110).
    pub button: u32,
    pub state: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollEv {
    pub dx: f64,
    pub dy: f64,
    /// High-resolution wheel steps (120 per notch), 0 when the source reports none.
    pub v120x: i32,
    pub v120y: i32,
    pub source: u32,
    pub stop: u8,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ConfigureEv {
    /// The size the compositor asks for, in logical pixels; 0 means "your choice".
    pub w: i32,
    pub h: i32,
    pub states: u32,
    /// Hand it to `ShellBackend::ack_configure` once a buffer of that size is committed.
    pub serial: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScaleEv {
    /// The preferred scale times 120.
    pub scale120: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FocusEv {
    pub r#in: u8,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameDoneEv {
    /// 0 when the compositor sent no presentation feedback.
    pub presented_ns: u64,
    pub refresh_ns: u32,
}

#[derive(Clone, Copy, Debug, Default)]
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
#[derive(Clone, Copy, Debug, Default)]
pub struct TextEv {
    /// The text for `ShellBackend::take_text`; 0 for none.
    pub id: u32,
    pub len: u32,
    pub cursor_begin: i32,
    pub cursor_end: i32,
    pub before: u32,
    pub after: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SelectionEv {
    /// `NHSHELL_SEL_CLIPBOARD` or `NHSHELL_SEL_PRIMARY`.
    pub kind: u32,
}

/// One touch point.  Positions are device pixels in the window the point went down on; UP and CANCEL carry the
/// point's last position.
#[derive(Clone, Copy, Debug, Default)]
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
#[derive(Clone, Copy, Debug, Default)]
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

#[derive(Clone, Copy, Debug, Default)]
pub struct SessionEv {
    /// `NHSHELL_SESSION_*`.
    pub state: u32,
}

/// A drag-and-drop record (`ShellEvent::Dnd*`, `ShellEvent::Drag*`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DndEv {
    /// The pointer's position in the window, device pixels (target records).
    pub x: f64,
    pub y: f64,
    /// The actions the drag's source offers (`NHSHELL_DND_*`; target records).
    pub source_actions: u32,
    /// The action to suggest: the compositor's choice when it made one, else the first of the source's in copy, move,
    /// ask order (target records).
    pub preferred: u32,
    /// The action the compositor chose with the target (0: none yet).
    pub action: u32,
    /// The compositor's time of the latest motion, milliseconds (target records).
    pub time_ms: u32,
    /// 1 when the drag is our own (target records).
    pub own: u8,
    /// `DRAG_DONE`: 1 finished, 0 cancelled.
    pub ok: u8,
}

/// One record: `event` for `window` (0: none) at `time_ns` (0: unknown).
#[derive(Clone, Copy, Debug)]
pub struct Record {
    pub window: u64,
    pub time_ns: u64,
    pub event: ShellEvent,
}

impl Record {
    pub fn new(event: ShellEvent, window: u64, time_ns: u64) -> Self {
        Record {
            window,
            time_ns,
            event,
        }
    }

    pub fn kind(&self) -> Kind {
        self.event.kind()
    }
}
