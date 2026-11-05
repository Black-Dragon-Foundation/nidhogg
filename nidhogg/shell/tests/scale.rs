//! T1 tests of outputs and scale against nh-recorder.
//! The harness is `common/mod.rs`.

mod common;

use common::*;
use nhshell::api::ShellBackend;
use nhshell::event::{Kind, Record, ShellEvent};

mod scale {
    use super::*;

    fn is_scale(id: u64, v: u32) -> impl Fn(&Record) -> bool {
        move |e| matches!(e.event, ShellEvent::Scale(s) if e.window == id && s.scale120 == v)
    }

    /// A window made on an output at 1.25, where `wl_output.scale` and `preferred_buffer_scale` round up to 2, takes the
    /// fractional scale alone: no `SCALE` of the integer hint comes before it.  (A child frame given that hint first
    /// opened its fonts at 2 and then at 1.25 again, and lost a size asked for in between.)
    #[test]
    fn new_window_on_a_fractional_output_never_takes_the_integer_hint() {
        use nhshell::api::{NHSHELL_ACK_CONTENT, WindowDesc};
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        assert!(
            rec.control(r#"{"cmd":"scale","value120":150}"#)
                .contains("\"ok\":true")
        );
        // every scale each window is given, from its making on, kept while waiting for something; a deadline bounds a
        // failure only
        type Seen = std::collections::BTreeMap<u64, Vec<u32>>;
        fn pump(
            shell: &mut nhshell::wayland::WaylandShell,
            seen: &mut Seen,
            done: &dyn Fn(&Seen, Option<&Record>) -> bool,
        ) -> Option<Record> {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !done(seen, None) {
                while let Some(e) = shell.next_event().unwrap() {
                    if let ShellEvent::Scale(s) = e.event {
                        seen.entry(e.window).or_default().push(s.scale120);
                    }
                    if done(seen, Some(&e)) {
                        return Some(e);
                    }
                }
                assert!(std::time::Instant::now() < deadline, "never came: {seen:?}");
                readable_within(shell.wake_fd(), 50);
            }
            None
        }
        let mut seen = Seen::new();
        let id = shell
            .window_new(&WindowDesc {
                w: 200,
                h: 100,
                title: "t".into(),
                app_id: "nidhogg.test".into(),
                flags: 0,
            })
            .unwrap();
        let ev = pump(&mut shell, &mut seen, &|_, e| {
            e.is_some_and(|e| e.kind() == Kind::Configure && e.window == id)
        })
        .unwrap();
        shell
            .ack_configure(id, configure_of(&ev).serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        // the window maps with its first buffer; a child frame comes under it
        let draw = |shell: &mut nhshell::wayland::WaylandShell, w: u64, (dw, dh): (i32, i32)| {
            let b = shell.present_begin(w, dw, dh).unwrap();
            unsafe { std::ptr::write_bytes(b.pixels, 0x80, (b.stride * b.h) as usize) };
            shell
                .present_commit(w, &[glitnir::geom::Rect::new(0, 0, dw, dh)])
                .unwrap();
            shell.flush().unwrap();
        };
        draw(&mut shell, id, (250, 125));
        let kid = shell
            .child_new(&nhshell::api::ChildDesc {
                kind: nhshell::api::NHSHELL_KIND_CHILD,
                parent: id,
                x: 20,
                y: 20,
                w: 100,
                h: 60,
                anchor: nhshell::api::Anchor::default(),
                flags: 0,
            })
            .unwrap();
        draw(&mut shell, kid, (125, 75));
        draw(&mut shell, id, (250, 125));
        pump(&mut shell, &mut seen, &|seen, _| {
            seen.get(&id).is_some_and(|v| v.contains(&150))
        });
        // the child starts at its parent's scale: 150 already, with no record, when the window had it at the child's
        // making, else 120 and then one record of 150
        let drain = |shell: &mut nhshell::wayland::WaylandShell, seen: &mut Seen| {
            while let Some(e) = shell.next_event().unwrap() {
                if let ShellEvent::Scale(s) = e.event {
                    seen.entry(e.window).or_default().push(s.scale120);
                }
            }
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while shell.window_scale(kid).unwrap() != 150 {
            drain(&mut shell, &mut seen);
            assert!(
                std::time::Instant::now() < deadline,
                "the child never took 150: {seen:?}"
            );
            readable_within(shell.wake_fd(), 50);
        }
        drain(&mut shell, &mut seen);
        assert_eq!(
            seen.get(&id).map(Vec::as_slice),
            Some(&[150][..]),
            "the window's: {seen:?}"
        );
        assert!(
            matches!(seen.get(&kid).map(Vec::as_slice), None | Some([150])),
            "the child's: {seen:?}"
        );
    }

    /// A child frame starts at its parent's scale, as C makes it: the compositor tells a subsurface its scale only once
    /// it shows, so its first buffer went out at scale 1 (twice its size at 2), and a blurred child had no backdrop until
    /// its next picture (the two scales differed).  Pattern C: each step waits for the recorder's answer.
    #[test]
    fn child_starts_at_its_parents_scale() {
        use nhshell::api::{NHSHELL_ACK_CONTENT, WindowDesc};
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        assert!(
            rec.control(r#"{"cmd":"scale","value120":240}"#)
                .contains("\"ok\":true")
        );
        let id = shell
            .window_new(&WindowDesc {
                w: 200,
                h: 100,
                title: "t".into(),
                app_id: "nidhogg.test".into(),
                flags: 0,
            })
            .unwrap();
        let ev = wait_for(&mut shell, |e| {
            e.kind() == Kind::Configure && e.window == id
        });
        shell
            .ack_configure(id, configure_of(&ev).serial, NHSHELL_ACK_CONTENT)
            .unwrap();
        let draw = |shell: &mut nhshell::wayland::WaylandShell, w: u64, (dw, dh): (i32, i32)| {
            let b = shell.present_begin(w, dw, dh).unwrap();
            unsafe { std::ptr::write_bytes(b.pixels, 0x80, (b.stride * b.h) as usize) };
            shell
                .present_commit(w, &[glitnir::geom::Rect::new(0, 0, dw, dh)])
                .unwrap();
            shell.flush().unwrap();
        };
        draw(&mut shell, id, (400, 200));
        // its SCALE may have come with the configure: the scale itself is what counts
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while shell.window_scale(id).unwrap() != 240 {
            while shell.next_event().unwrap().is_some() {}
            assert!(
                std::time::Instant::now() < deadline,
                "the window never took 240"
            );
            readable_within(shell.wake_fd(), 50);
        }
        let kid = shell
            .child_new(&nhshell::api::ChildDesc {
                kind: nhshell::api::NHSHELL_KIND_CHILD,
                parent: id,
                x: 20,
                y: 20,
                w: 100,
                h: 60,
                anchor: nhshell::api::Anchor::default(),
                flags: 0,
            })
            .unwrap();
        assert_eq!(shell.window_scale(kid).unwrap(), 240, "before any event");
        draw(&mut shell, kid, (200, 120));
        draw(&mut shell, id, (400, 200));
        rec.wait_node(kid, |n| n.contains("\"buffer\":{\"h\":120,\"w\":200}"));
        let log = rec.control(r#"{"cmd":"log","since":1}"#);
        let first = log
            .split("{\"ack_serial\":")
            .find(|c| c.contains(&format!("\"surface\":{kid},")) && c.contains("\"buffer\":{"))
            .expect("the child's first buffer");
        assert!(first.contains("\"dst\":[100,60]"), "{first}");
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
        let ev = wait_for(&mut shell, |e| e.kind() == Kind::OutputAdd);
        let o = output_of(&ev);
        let name = std::ffi::CStr::from_bytes_until_nul(&o.name).unwrap();
        assert_eq!(name.to_str().unwrap(), "nh-0");
        assert_eq!((o.w, o.h, o.scale120), (1920, 1080, 120));
    }
}

mod monitors {
    use super::*;

    fn out_id(ev: &Record) -> u64 {
        output_of(ev).id
    }

    #[test]
    fn scale_change_is_one_change_record() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        wait_for(&mut shell, |e| e.kind() == Kind::OutputAdd);
        assert!(
            rec.control(r#"{"cmd":"scale","value120":240}"#)
                .contains("\"ok\":true")
        );
        let ev = wait_for(&mut shell, |e| e.kind() == Kind::OutputChange);
        assert_eq!(output_of(&ev).scale120, 240);
    }

    #[test]
    fn scale_enter_and_leave_name_the_output() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        let add = wait_for(&mut shell, |e| e.kind() == Kind::OutputAdd);
        let first = out_id(&add);
        let id = shell.surface_new().unwrap();
        shell.scale_attach(id).unwrap();
        shell.flush().unwrap();
        let cmd =
            |op: &str| format!(r#"{{"cmd":"output","op":"{op}","name":"nh-0","surface":{id}}}"#);
        assert!(rec.control_ok(&cmd("enter")).contains("\"ok\":true"));
        let ev = wait_for(&mut shell, |e| e.kind() == Kind::EnterOutput);
        assert_eq!((ev.window, out_id(&ev)), (id, first));
        assert!(rec.control_ok(&cmd("leave")).contains("\"ok\":true"));
        let ev = wait_for(&mut shell, |e| e.kind() == Kind::LeaveOutput);
        assert_eq!((ev.window, out_id(&ev)), (id, first));
    }
}

mod monitors_added {
    use super::*;

    #[test]
    fn scale_added_output_is_one_add_record() {
        let rec = Recorder::start("sway");
        let mut shell = rec.open();
        wait_for(&mut shell, |e| e.kind() == Kind::OutputAdd);
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
                .map(|e| e.kind())
                .filter(|&k| k == Kind::OutputAdd || k == Kind::OutputChange),
        );
        assert_eq!(kinds, vec![Kind::OutputAdd], "{kinds:?}");
    }
}
