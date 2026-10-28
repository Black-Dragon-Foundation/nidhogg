//! The ABI version and the return codes of `include/nhglue.h` (the layout test is `tests/abi.rs`).

use nhshell::api::{
    NHSHELL_E_ARG, NHSHELL_E_DISCONNECTED, NHSHELL_E_NOMEM, NHSHELL_E_STATE, NHSHELL_E_TIMEOUT,
    NHSHELL_E_TOO_SMALL, NHSHELL_E_UNSUPPORTED,
};

/// The ABI version the header describes.
pub const NHG_ABI_VERSION: u32 = 1;

pub const NHG_OK: i32 = 0;
/// A panic inside the glue, caught at the boundary.
pub const NHG_E_PANIC: i32 = -1;
// The codes below equal the shell's where they mean the same.
pub const NHG_E_ARG: i32 = NHSHELL_E_ARG;
pub const NHG_E_STATE: i32 = NHSHELL_E_STATE;
pub const NHG_E_UNSUPPORTED: i32 = NHSHELL_E_UNSUPPORTED;
pub const NHG_E_DISCONNECTED: i32 = NHSHELL_E_DISCONNECTED;
pub const NHG_E_NOMEM: i32 = NHSHELL_E_NOMEM;
pub const NHG_E_TIMEOUT: i32 = NHSHELL_E_TIMEOUT;
pub const NHG_E_TOO_SMALL: i32 = NHSHELL_E_TOO_SMALL;
/// Called from a thread the backend refuses (a backend may require the main thread).
pub const NHG_E_THREAD: i32 = -9;
/// The display is reconnecting: the call sent nothing (the shell's `NHSHELL_E_RECONNECTING`, whose -9 is taken here).
pub const NHG_E_RECONNECTING: i32 = -10;
