//! T1 tests of outputs and scale against nh-recorder.
//! The harness is `common/mod.rs`.

mod common;

use common::*;
use nhshell::api::ShellBackend;
use nhshell::event::{NHSHELL_EV_OUTPUT_ADD, NHSHELL_EV_SCALE, NhShellEvent};

mod scale {
    use super::*;

    fn is_scale(id: u64, v: u32) -> impl Fn(&NhShellEvent) -> bool {
        // SAFETY: SCALE records carry `scale`.
        move |e| e.kind == NHSHELL_EV_SCALE && e.window == id && unsafe { e.u.scale.scale120 } == v
    }

    #[test]
    fn fractional_event_reaches_window() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let id = shell.surface_new().unwrap();
        shell.scale_attach(id).unwrap();
        shell.flush().unwrap();
        assert_eq!(shell.window_scale(id).unwrap(), 120);
        assert!(
            rec.control(r#"{"cmd":"scale","value120":150}"#)
                .contains("\"ok\":true")
        );
        wait_for(&mut shell, is_scale(id, 150));
        assert_eq!(shell.window_scale(id).unwrap(), 150);
    }

    #[test]
    fn fallback_to_buffer_scale_without_fractional() {
        let rec = Recorder::start("no-fractional");
        let mut shell = rec.open();
        assert_eq!(shell.caps().fractional_scale, 0);
        let id = shell.surface_new().unwrap();
        shell.scale_attach(id).unwrap();
        shell.flush().unwrap();
        assert!(
            rec.control(r#"{"cmd":"scale","value120":240}"#)
                .contains("\"ok\":true")
        );
        wait_for(&mut shell, is_scale(id, 240));
        assert_eq!(shell.window_scale(id).unwrap(), 240);
    }

    #[test]
    fn outputs_listed_with_names() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let ev = wait_for(&mut shell, |e| e.kind == NHSHELL_EV_OUTPUT_ADD);
        // SAFETY: OUTPUT_ADD records carry `output`.
        let o = unsafe { ev.u.output };
        let name = std::ffi::CStr::from_bytes_until_nul(&o.name).unwrap();
        assert_eq!(name.to_str().unwrap(), "nh-0");
        assert_eq!((o.w, o.h, o.scale120), (1920, 1080, 120));
    }
}
