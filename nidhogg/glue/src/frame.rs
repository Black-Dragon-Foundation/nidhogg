//! One shell window as the glue sees it: its kind, the Emacs facts C pushed for its frame, and the decision state of
//! its configures and scale.  Later tasks add the rest (CSD, child, timing).
//!
//! This state is keyed by the shell window, as the events that drive it are: a window's configure state starts with
//! the window (a new toplevel is unconfigured), and C seeds the frame's facts each time it makes a window for the frame
//! (`nhg_window_new`'s descriptor, `nhg_tip_show`).  A frame's pixel state is keyed by its Glitnir root id
//! instead (`backing.rs`), which a frame gets only at its first draw, after its first configure.

/// `nhg_frame_register` flags: what kind of window the frame has (none of them: a top-level).
pub const NHG_FRAME_CHILD: u32 = 1;
pub const NHG_FRAME_TIP: u32 = 2;
pub const NHG_FRAME_POPUP: u32 = 4;
pub const NHG_FRAME_KINDS: u32 = NHG_FRAME_CHILD | NHG_FRAME_TIP | NHG_FRAME_POPUP;

/// `NhgFrameFacts` flags.
pub const NHG_FACT_VISIBLE: u32 = 1;
pub const NHG_FACT_ICONIFIED: u32 = 2;
/// The frame draws its own title bar (the glue's title-bar layout sets it, `nhg_csd_layout`).
pub const NHG_FACT_CSD: u32 = 4;

/// The Emacs facts a frame's decisions read, as C pushes them (design §3.3); `nhg_debug_check_frame` compares them with
/// Emacs's own values in a checking build.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NhgFrameFacts {
    /// The frame's native size as Emacs has it (device pixels): the size C last asked for.
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

/// A toplevel's configure state (moved from `struct nhwl_output`'s `configured`, `logical_w`, `logical_h`, `serial`
/// and `states`; C keeps copies of what its own code still reads, set when it applies `RESIZE`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Configure {
    pub configured: bool,
    /// The compositor's last suggestion in logical pixels (0: the frame's own size).
    pub logical_w: i32,
    pub logical_h: i32,
    pub serial: u32,
    /// `NHSHELL_STATE_*`.
    pub states: u32,
}

/// A configure's ack, held until C has applied the configure's `RESIZE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ack {
    pub serial: u32,
    /// Only a buffer carries it (`NHSHELL_ACK_CONTENT`).
    pub content: bool,
}

/// A registered window.
#[derive(Debug, Default)]
pub struct Frame {
    /// `NHG_FRAME_*`.
    pub flags: u32,
    pub facts: NhgFrameFacts,
    pub cfg: Configure,
    /// The window's scale times 120 as C last applied it (0: not pushed yet).
    pub scale120: u32,
    /// The ack of the configure C is applying.
    pub ack: Option<Ack>,
    /// The last ack recorded with the shell: serial and `NHSHELL_ACK_*` flags (for the tests).
    pub last_ack: Option<(u32, u32)>,
    /// The title bar (`csd.rs`).
    pub csd: crate::csd::Csd,
    /// The outputs the window is on, as the compositor's enter and leave events said (`monitors.rs`).
    pub outputs: Vec<u64>,
    /// What the window's alpha takes off opaque (`u32::MAX` minus the multiplier; 0: opaque), and whether the shell
    /// multiplies it because the compositor cannot (`window.rs`).
    pub alpha_cut: u32,
    pub alpha_host: bool,
    /// A child frame's layout, chrome and sync state (`child.rs`).
    pub child: crate::child::Child,
    /// A shown popup frame's place, owner and size (`popup.rs`).
    pub popup: Option<crate::popup::Popup>,
    /// A top-level: it lost the keyboard to a grabbing popup frame of its own, and keeps Emacs's focus meanwhile
    /// (`popup.rs`).
    pub focus_lent: bool,
}

impl Frame {
    pub fn new(flags: u32) -> Frame {
        Frame {
            flags,
            ..Frame::default()
        }
    }
}
