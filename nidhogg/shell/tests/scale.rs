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

mod monitors {
    use super::*;
    use nhshell::event::{
        NHSHELL_EV_ENTER_OUTPUT, NHSHELL_EV_LEAVE_OUTPUT, NHSHELL_EV_OUTPUT_CHANGE,
    };

    fn out_id(ev: &NhShellEvent) -> u64 {
        // SAFETY: these records carry `output`.
        unsafe { ev.u.output.id }
    }

    #[test]
    fn scale_change_is_one_change_record() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        wait_for(&mut shell, |e| e.kind == NHSHELL_EV_OUTPUT_ADD);
        assert!(
            rec.control(r#"{"cmd":"scale","value120":240}"#)
                .contains("\"ok\":true")
        );
        let ev = wait_for(&mut shell, |e| e.kind == NHSHELL_EV_OUTPUT_CHANGE);
        // SAFETY: OUTPUT_CHANGE records carry `output`.
        assert_eq!(unsafe { ev.u.output.scale120 }, 240);
    }

    #[test]
    fn scale_enter_and_leave_name_the_output() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let add = wait_for(&mut shell, |e| e.kind == NHSHELL_EV_OUTPUT_ADD);
        let first = out_id(&add);
        let id = shell.surface_new().unwrap();
        shell.scale_attach(id).unwrap();
        shell.flush().unwrap();
        let cmd =
            |op: &str| format!(r#"{{"cmd":"output","op":"{op}","name":"nh-0","surface":{id}}}"#);
        assert!(rec.control_ok(&cmd("enter")).contains("\"ok\":true"));
        let ev = wait_for(&mut shell, |e| e.kind == NHSHELL_EV_ENTER_OUTPUT);
        assert_eq!((ev.window, out_id(&ev)), (id, first));
        assert!(rec.control_ok(&cmd("leave")).contains("\"ok\":true"));
        let ev = wait_for(&mut shell, |e| e.kind == NHSHELL_EV_LEAVE_OUTPUT);
        assert_eq!((ev.window, out_id(&ev)), (id, first));
    }
}

mod monitors_added {
    use super::*;
    use nhshell::event::NHSHELL_EV_OUTPUT_CHANGE;

    #[test]
    fn scale_added_output_is_one_add_record() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        wait_for(&mut shell, |e| e.kind == NHSHELL_EV_OUTPUT_ADD);
        rec.control_ok(
            r#"{"cmd":"output","op":"add","name":"nh-1","size":[800,600],"scale120":240}"#,
        );
        // outputs that come and go are the host's to dispatch (the main queue)
        let mut evs = Vec::new();
        for _ in 0..10 {
            shell.dispatch_main().unwrap();
            while let Some(ev) = shell.next_event().unwrap() {
                evs.push(ev);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let mut kinds = Vec::new();
        kinds.extend(
            evs.iter()
                .map(|e| e.kind)
                .filter(|&k| k == NHSHELL_EV_OUTPUT_ADD || k == NHSHELL_EV_OUTPUT_CHANGE),
        );
        assert_eq!(kinds, vec![NHSHELL_EV_OUTPUT_ADD], "{kinds:?}");
    }
}
