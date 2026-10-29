//! Selections (W1 Task 6): the clipboard and the primary selection against nh-recorder (`NH_RECORDER`).  Each test
//! starts its own recorder with a temporary `XDG_RUNTIME_DIR` and connects by absolute socket path.  The recorder's
//! `offer` stands in for another client that owns a selection, its `read` for another client that pastes.

use nhshell::api::*;
use nhshell::event::{Record, ShellEvent};
use nhshell::wayland::WaylandShell;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fd::BorrowedFd;
use rustix::process::{Pid, Signal, kill_process};
use std::ffi::CString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Recorder {
    child: Child,
    dir: PathBuf,
}

impl Recorder {
    fn start() -> Recorder {
        let bin = std::env::var_os("NH_RECORDER")
            .expect("NH_RECORDER is not set (the path of the nh-recorder binary)");
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("nhshell-selection-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let profile =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../test/recorder/profiles/sway.json");
        let mut child = Command::new(bin)
            .args(["--socket", "wl", "--control"])
            .arg(dir.join("control"))
            .arg("--profile")
            .arg(&profile)
            .env("XDG_RUNTIME_DIR", &dir)
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn nh-recorder");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line, "READY\n");
        Recorder { child, dir }
    }

    fn display(&self) -> CString {
        CString::new(self.dir.join("wl").as_os_str().as_bytes()).unwrap()
    }

    fn control(&self, req: &str) -> String {
        let mut s = UnixStream::connect(self.dir.join("control")).unwrap();
        writeln!(s, "{req}").unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        line
    }

    /// Retry `req` until it succeeds (a request the shell just flushed may not be processed yet), 3 s at most.
    fn control_ok(&self, req: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let reply = self.control(req);
            if reply.contains("\"ok\":true") || Instant::now() > deadline {
                return reply;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn signal(&self, sig: Signal) {
        kill_process(Pid::from_raw(self.child.id() as i32).unwrap(), sig).unwrap();
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn readable_within(fd: i32, ms: i64) -> bool {
    // SAFETY: the shell's wake descriptor stays open while the shell lives.
    let fd = unsafe { BorrowedFd::borrow_raw(fd) };
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    let t = Timespec {
        tv_sec: ms / 1000,
        tv_nsec: (ms % 1000) * 1_000_000,
    };
    matches!(poll(&mut fds, Some(&t)), Ok(n) if n > 0)
}

/// Take records until one satisfies `want` (`ms` at most).
fn wait_event(shell: &mut WaylandShell, ms: u64, want: impl Fn(&Record) -> bool) -> Option<Record> {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        readable_within(shell.wake_fd(), 20);
        while let Some(ev) = shell.next_event().unwrap() {
            if want(&ev) {
                return Some(ev);
            }
        }
    }
    None
}

fn is_selection(ev: &Record, kind: u32) -> bool {
    matches!(ev.event, ShellEvent::Selection(s) if s.kind == kind)
}

fn is_focus(ev: &Record, on: u8) -> bool {
    matches!(ev.event, ShellEvent::Focus(f) if f.r#in == on)
}

/// Take records until selection `kind` has the MIME types `want` (sorted) and a SELECTION record for it came (2 s at
/// most).  Types, not the record alone: focus brings `selection(null)` records of its own.
fn wait_types(shell: &mut WaylandShell, kind: u32, want: &[&str]) {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut seen = false;
    loop {
        let mut types = shell.selection_types(kind).unwrap();
        types.sort();
        if seen && types == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "types {types:?}, want {want:?}, SELECTION seen: {seen}"
        );
        readable_within(shell.wake_fd(), 20);
        while let Some(ev) = shell.next_event().unwrap() {
            seen |= is_selection(&ev, kind);
        }
    }
}

/// A shell with one surface that has keyboard focus.
fn focused(rec: &Recorder) -> (WaylandShell, u64) {
    let mut shell = WaylandShell::open(Some(&rec.display())).expect("open the shell");
    let surface = shell.surface_new().unwrap();
    shell.flush().unwrap();
    let reply = rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{surface}}}"#));
    assert!(reply.contains("\"ok\":true"), "{reply}");
    assert!(
        wait_event(&mut shell, 2000, |e| is_focus(e, 1)).is_some(),
        "keyboard focus"
    );
    (shell, surface)
}

/// The string after `"data":` in a recorder reply (plain text without escapes).
fn data_of(reply: &str) -> Option<&str> {
    let start = reply.find("\"data\":\"")? + 8;
    let len = reply[start..].find('"')?;
    Some(&reply[start..start + len])
}

#[allow(clippy::module_inception)]
mod selection {
    use super::*;

    #[test]
    fn own_then_client_reads_it() {
        let rec = Recorder::start();
        let (mut shell, _) = focused(&rec);
        let text: &[u8] = b"hello from nidhogg";
        shell
            .selection_own(
                NHSHELL_SEL_CLIPBOARD,
                &[("text/plain;charset=utf-8", text), ("UTF8_STRING", text)],
            )
            .expect("own");
        assert!(shell.selection_owned(NHSHELL_SEL_CLIPBOARD).unwrap());
        // the shell's reader answers the recorder's `send` while this thread waits in the control call
        let reply = rec
            .control_ok(r#"{"cmd":"read","kind":"clipboard","type":"text/plain;charset=utf-8"}"#);
        assert_eq!(data_of(&reply), Some("hello from nidhogg"), "{reply}");
        let reply = rec.control_ok(r#"{"cmd":"read","kind":"clipboard","type":"UTF8_STRING"}"#);
        assert_eq!(data_of(&reply), Some("hello from nidhogg"), "{reply}");
        drop(shell);
    }

    #[test]
    fn client_offer_then_we_read_it() {
        let rec = Recorder::start();
        let (mut shell, _) = focused(&rec);
        let reply = rec.control(
            r#"{"cmd":"offer","kind":"clipboard","types":{"text/plain;charset=utf-8":"from the recorder","text/html":"<b>x</b>"}}"#,
        );
        assert!(reply.contains("\"ok\":true"), "{reply}");
        let want = ["text/html", "text/plain;charset=utf-8"];
        wait_types(&mut shell, NHSHELL_SEL_CLIPBOARD, &want);
        assert!(!shell.selection_owned(NHSHELL_SEL_CLIPBOARD).unwrap());
        let bytes = shell
            .selection_read(NHSHELL_SEL_CLIPBOARD, "text/plain;charset=utf-8", 1000)
            .expect("read");
        assert_eq!(bytes, b"from the recorder");
        let html = shell
            .selection_read(NHSHELL_SEL_CLIPBOARD, "text/html", 1000)
            .unwrap();
        assert_eq!(html, b"<b>x</b>");
        assert_eq!(
            shell.selection_read(NHSHELL_SEL_CLIPBOARD, "image/png", 1000),
            Err(ShellError::State),
            "a type the offer does not have"
        );
    }

    /// Own, then read our own bytes while the compositor is stopped (SIGSTOP), which a `receive` round trip could
    /// not survive.  (The recorder keeps no log of `receive` requests; a stopped compositor shows the same thing.)
    #[test]
    fn read_our_own_without_a_round_trip() {
        let rec = Recorder::start();
        let mut shell = WaylandShell::open(Some(&rec.display())).expect("open the shell");
        let win = shell
            .window_new(&WindowDesc {
                w: 200,
                h: 100,
                title: "selection".into(),
                app_id: String::new(),
                flags: 0,
            })
            .unwrap();
        assert_eq!(
            shell.selection_own(NHSHELL_SEL_CLIPBOARD, &[]),
            Err(ShellError::State),
            "no focus yet"
        );
        let reply = rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{win}}}"#));
        assert!(reply.contains("\"ok\":true"), "{reply}");
        assert!(
            wait_event(&mut shell, 2000, |e| is_focus(e, 1)).is_some(),
            "keyboard focus"
        );
        let (a, b): (&[u8], &[u8]) = (b"plain bytes", b"utf8 bytes");
        shell
            .selection_own(
                NHSHELL_SEL_CLIPBOARD,
                &[("text/plain", a), ("UTF8_STRING", b)],
            )
            .unwrap();
        assert!(shell.selection_owned(NHSHELL_SEL_CLIPBOARD).unwrap());
        rec.signal(Signal::STOP);
        let started = Instant::now();
        let r = shell.selection_read(NHSHELL_SEL_CLIPBOARD, "UTF8_STRING", 1000);
        let took = started.elapsed();
        rec.signal(Signal::CONT);
        assert!(took < Duration::from_millis(100), "{took:?}: a round trip");
        assert_eq!(r.as_deref(), Ok(&b"utf8 bytes"[..]));
        drop(shell);
    }

    /// Review Focus 1: a source that takes the `receive` and never writes.  The read gives up after its timeout, and
    /// the next read of a normal offer works.
    #[test]
    fn read_times_out() {
        let rec = Recorder::start();
        let (mut shell, _) = focused(&rec);
        let reply = rec.control(
            r#"{"cmd":"offer","kind":"clipboard","types":{"text/plain":"never sent"},"silent":true}"#,
        );
        assert!(reply.contains("\"ok\":true"), "{reply}");
        wait_types(&mut shell, NHSHELL_SEL_CLIPBOARD, &["text/plain"]);
        let started = Instant::now();
        let r = shell.selection_read(NHSHELL_SEL_CLIPBOARD, "text/plain", 1000);
        let took = started.elapsed();
        assert_eq!(r, Err(ShellError::Timeout));
        assert!(
            took >= Duration::from_millis(1000) && took < Duration::from_millis(1200),
            "gave up after {took:?}"
        );
        // the shell goes on: a ping is answered, and a normal offer reads
        let pong = rec.control(r#"{"cmd":"ping"}"#);
        assert!(pong.contains("\"ok\":true"), "{pong}");
        let reply = rec.control(
            r#"{"cmd":"offer","kind":"clipboard","types":{"text/plain;charset=utf-8":"after the silence"}}"#,
        );
        assert!(reply.contains("\"ok\":true"), "{reply}");
        let want = ["text/plain;charset=utf-8"];
        wait_types(&mut shell, NHSHELL_SEL_CLIPBOARD, &want);
        let bytes = shell
            .selection_read(NHSHELL_SEL_CLIPBOARD, "text/plain;charset=utf-8", 1000)
            .expect("the next read works");
        assert_eq!(bytes, b"after the silence");
    }

    #[test]
    fn primary_selection_round_trip() {
        let rec = Recorder::start();
        let (mut shell, _) = focused(&rec);
        let reply = rec.control(r#"{"cmd":"offer","kind":"primary","types":{"text/plain":"sel"}}"#);
        assert!(reply.contains("\"ok\":true"), "{reply}");
        wait_types(&mut shell, NHSHELL_SEL_PRIMARY, &["text/plain"]);
        // the clipboard is untouched
        assert!(
            shell
                .selection_types(NHSHELL_SEL_CLIPBOARD)
                .unwrap()
                .is_empty()
        );
        let bytes = shell
            .selection_read(NHSHELL_SEL_PRIMARY, "text/plain", 1000)
            .unwrap();
        assert_eq!(bytes, b"sel");
        shell
            .selection_own(NHSHELL_SEL_PRIMARY, &[("text/plain", b"ours")])
            .unwrap();
        assert!(shell.selection_owned(NHSHELL_SEL_PRIMARY).unwrap());
        assert!(!shell.selection_owned(NHSHELL_SEL_CLIPBOARD).unwrap());
        let reply = rec.control_ok(r#"{"cmd":"read","kind":"primary","type":"text/plain"}"#);
        assert_eq!(data_of(&reply), Some("ours"), "{reply}");
        shell.selection_disown(NHSHELL_SEL_PRIMARY).unwrap();
        assert!(!shell.selection_owned(NHSHELL_SEL_PRIMARY).unwrap());
    }

    #[test]
    fn own_without_focus_is_refused() {
        let rec = Recorder::start();
        let mut shell = WaylandShell::open(Some(&rec.display())).unwrap();
        let surface = shell.surface_new().unwrap();
        shell.flush().unwrap();
        assert_eq!(
            shell.selection_own(NHSHELL_SEL_CLIPBOARD, &[("text/plain", b"x")]),
            Err(ShellError::State)
        );
        assert!(!shell.selection_owned(NHSHELL_SEL_CLIPBOARD).unwrap());
        rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{surface}}}"#));
        assert!(wait_event(&mut shell, 2000, |e| is_focus(e, 1)).is_some());
        shell
            .selection_own(NHSHELL_SEL_CLIPBOARD, &[("text/plain", b"x")])
            .unwrap();
        // focus goes away: owning is refused again (what we own stays ours)
        rec.control_ok(r#"{"cmd":"focus","surface":null}"#);
        assert!(wait_event(&mut shell, 2000, |e| is_focus(e, 0)).is_some());
        assert_eq!(
            shell.selection_own(NHSHELL_SEL_PRIMARY, &[("text/plain", b"y")]),
            Err(ShellError::State)
        );
        assert!(shell.selection_owned(NHSHELL_SEL_CLIPBOARD).unwrap());
        assert_eq!(
            shell.selection_own(99, &[("text/plain", b"y")]),
            Err(ShellError::Arg),
            "no such selection"
        );
    }

    #[test]
    fn selection_event_on_new_offer() {
        let rec = Recorder::start();
        let (mut shell, _) = focused(&rec);
        rec.control(r#"{"cmd":"offer","kind":"clipboard","types":{"text/plain":"one"}}"#);
        wait_types(&mut shell, NHSHELL_SEL_CLIPBOARD, &["text/plain"]);
        // our own selection is an offer change too
        shell
            .selection_own(
                NHSHELL_SEL_CLIPBOARD,
                &[("text/plain", b"two"), ("text/html", b"<i>two</i>")],
            )
            .unwrap();
        let want = ["text/html", "text/plain"];
        wait_types(&mut shell, NHSHELL_SEL_CLIPBOARD, &want);
        assert!(shell.selection_owned(NHSHELL_SEL_CLIPBOARD).unwrap());
        // another client takes it: an event, and we no longer own it
        rec.control(r#"{"cmd":"offer","kind":"clipboard","types":{"text/plain":"three"}}"#);
        wait_types(&mut shell, NHSHELL_SEL_CLIPBOARD, &["text/plain"]);
        assert!(!shell.selection_owned(NHSHELL_SEL_CLIPBOARD).unwrap());
        assert_eq!(
            shell
                .selection_read(NHSHELL_SEL_CLIPBOARD, "text/plain", 1000)
                .unwrap(),
            b"three"
        );
    }

    /// 4 MiB to a second client that does not read for a while: the reader writes without blocking, so a ping during
    /// the transfer is answered within 100 ms, and every byte arrives in order.
    #[test]
    fn large_selection_written_without_blocking_the_reader() {
        use std::io::Read;
        use wayland_client::protocol::wl_compositor::WlCompositor;
        use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
        use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
        use wayland_client::protocol::wl_data_offer::WlDataOffer;
        use wayland_client::protocol::wl_registry::{self, WlRegistry};
        use wayland_client::protocol::wl_seat::WlSeat;
        use wayland_client::protocol::wl_surface::WlSurface;
        use wayland_client::{
            Connection, Dispatch, Proxy, QueueHandle, delegate_noop, event_created_child,
        };

        #[derive(Default)]
        struct Paster {
            globals: Vec<(u32, String)>,
            offer: Option<WlDataOffer>,
        }
        impl Dispatch<WlRegistry, ()> for Paster {
            fn event(
                s: &mut Self,
                _: &WlRegistry,
                e: wl_registry::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                if let wl_registry::Event::Global {
                    name, interface, ..
                } = e
                {
                    s.globals.push((name, interface));
                }
            }
        }
        impl Dispatch<WlDataDevice, ()> for Paster {
            fn event(
                s: &mut Self,
                _: &WlDataDevice,
                e: wl_data_device::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                if let wl_data_device::Event::Selection { id } = e {
                    s.offer = id;
                }
            }
            event_created_child!(Paster, WlDataDevice, [
                wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
            ]);
        }
        delegate_noop!(Paster: WlCompositor);
        delegate_noop!(Paster: WlDataDeviceManager);
        delegate_noop!(Paster: ignore WlSurface);
        delegate_noop!(Paster: ignore WlSeat);
        delegate_noop!(Paster: ignore WlDataOffer);

        let rec = Recorder::start();
        let (mut shell, our_surface) = focused(&rec);
        let big: Vec<u8> = (0..4u32 << 20).map(|i| b'a' + (i % 26) as u8).collect();
        shell
            .selection_own(NHSHELL_SEL_CLIPBOARD, &[("text/plain", &big[..])])
            .unwrap();

        // the second client: a surface, a data device, and keyboard focus
        let conn =
            Connection::from_socket(UnixStream::connect(rec.dir.join("wl")).unwrap()).unwrap();
        let mut queue = conn.new_event_queue::<Paster>();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut p = Paster::default();
        queue.roundtrip(&mut p).unwrap();
        let name = |i: &str| p.globals.iter().find(|g| g.1 == i).unwrap().0;
        let compositor: WlCompositor = registry.bind(name("wl_compositor"), 4, &qh, ());
        let seat: WlSeat = registry.bind(name("wl_seat"), 1, &qh, ());
        let manager: WlDataDeviceManager =
            registry.bind(name("wl_data_device_manager"), 3, &qh, ());
        let _device = manager.get_data_device(&seat, &qh, ());
        let surface = compositor.create_surface(&qh, ());
        queue.roundtrip(&mut p).unwrap();
        let sid = surface.id().protocol_id();
        assert_ne!(
            u64::from(sid),
            our_surface,
            "the recorder finds surfaces by protocol id"
        );
        let reply = rec.control_ok(&format!(r#"{{"cmd":"focus","surface":{sid}}}"#));
        assert!(reply.contains("\"ok\":true"), "{reply}");
        let deadline = Instant::now() + Duration::from_secs(2);
        while p.offer.is_none() && Instant::now() < deadline {
            queue.roundtrip(&mut p).unwrap();
        }
        let offer = p
            .offer
            .clone()
            .expect("the second client sees our selection");
        let (read_end, write_end) = rustix::pipe::pipe().unwrap();
        offer.receive("text/plain".into(), rustix::fd::AsFd::as_fd(&write_end));
        conn.flush().unwrap();
        drop(write_end);
        // the pipe fills (64 KiB) and the shell's reader waits for it to drain; meanwhile it answers pings
        std::thread::sleep(Duration::from_millis(300));
        let pong = rec.control(r#"{"cmd":"ping"}"#);
        let ms: u64 = pong
            .split("\"pong_ms\":")
            .nth(1)
            .and_then(|r| r.trim_start().split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no pong: {pong}"));
        assert!(ms < 100, "pong after {ms} ms during the transfer");
        let mut got = Vec::new();
        std::fs::File::from(read_end).read_to_end(&mut got).unwrap();
        assert_eq!(got.len(), big.len());
        assert!(got == big, "the bytes arrive in order");
        drop(shell);
    }
}
