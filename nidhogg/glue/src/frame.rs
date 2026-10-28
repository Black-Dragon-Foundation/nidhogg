//! One frame as the glue sees it: its shell window's kind and the Emacs facts C pushed for it.  Later tasks add the
//! decision state (sizes, scale, configure, CSD, child, timing).

/// `nhg_frame_register` flags: what kind of window the frame has (none of them: a top-level).
pub const NHG_FRAME_CHILD: u32 = 1;
pub const NHG_FRAME_TIP: u32 = 2;
pub const NHG_FRAME_POPUP: u32 = 4;
pub const NHG_FRAME_KINDS: u32 = NHG_FRAME_CHILD | NHG_FRAME_TIP | NHG_FRAME_POPUP;

/// `NhgFrameFacts` flags.
pub const NHG_FACT_VISIBLE: u32 = 1;
pub const NHG_FACT_ICONIFIED: u32 = 2;

/// The Emacs facts a frame's decisions read, as C pushes them (design §3.3); `nhg_debug_check_frame` compares them with
/// Emacs's own values in a checking build.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NhgFrameFacts {
    /// The frame's native size as Emacs has it (device pixels).
    pub native_w: i32,
    pub native_h: i32,
    /// `FRAME_COLUMN_WIDTH`, `FRAME_LINE_HEIGHT`.
    pub col_w: i32,
    pub line_h: i32,
    /// The parent frame's window (0: none).
    pub parent: u64,
    /// `NHG_FACT_*`.
    pub flags: u32,
}

/// A registered frame.
#[derive(Debug, Default)]
pub struct Frame {
    /// `NHG_FRAME_*`.
    pub flags: u32,
    pub facts: NhgFrameFacts,
}

impl Frame {
    pub fn new(flags: u32) -> Frame {
        Frame {
            flags,
            facts: NhgFrameFacts::default(),
        }
    }
}
