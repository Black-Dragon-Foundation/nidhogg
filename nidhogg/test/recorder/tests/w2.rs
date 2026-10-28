//! T0 tests of the W2 recorder additions: drag and drop both ways, popup grabs and their order, touch, pinch.
mod common;
use common::{send, Env};
use serde_json::{json, Value};
use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};
use wayland_client::protocol::{
    wl_data_device::WlDataDevice,
    wl_data_device_manager::{DndAction, WlDataDeviceManager},
    wl_data_source::WlDataSource,
    wl_pointer::WlPointer,
    wl_seat::WlSeat,
    wl_surface::WlSurface,
    wl_touch::WlTouch,
};
use wayland_client::Proxy;
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gestures_v1::ZwpPointerGesturesV1;
use wayland_protocols::xdg::shell::client::{
    xdg_popup::XdgPopup, xdg_surface::XdgSurface, xdg_wm_base::XdgWmBase,
};

/// A client with a mapped toplevel (keyboard focus on it), a seat pointer and a data device; the pointer is over it.
struct Rig {
    env: Env,
    seat: WlSeat,
    device: WlDataDevice,
    surface: WlSurface,
    xs: XdgSurface,
}

fn rig(profile: &str) -> Rig {
    let mut env = Env::new(profile, &[]);
    let seat: WlSeat = env.bind("wl_seat", 9);
    let _pointer: WlPointer = seat.get_pointer(&env.qh, ());
    let manager: WlDataDeviceManager = env.bind("wl_data_device_manager", 3);
    let device = manager.get_data_device(&seat, &env.qh, ());
    let (surface, xs, _top) = env.toplevel(200, 200);
    let id = surface.id().protocol_id();
    assert_eq!(
        env.rec.control(json!({"cmd": "focus", "surface": id}))["ok"],
        true
    );
    assert_eq!(
        env.rec.control(json!({"cmd": "pointer", "pos": [3, 3]}))["ok"],
        true
    );
    env.roundtrip();
    Rig {
        env,
        seat,
        device,
        surface,
        xs,
    }
}

fn wait_for(env: &mut Env, done: impl Fn(&Env) -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !done(env) {
        assert!(Instant::now() < end, "timed out waiting for the client");
        env.roundtrip();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn ok(v: Value) -> Value {
    assert_eq!(v["ok"], true, "{v}");
    v
}

// --- drops onto the client ---------------------------------------------------------------------------------

fn offer_cmd(rig: &Rig, silent: bool) -> Value {
    rig.env.rec.control(json!({"cmd": "dnd_offer", "types": {"text/plain": "dropped text"},
        "actions": ["copy", "move"], "preferred": "copy", "silent": silent, "surface": "toplevel", "at": [10, 12]}))
}

#[test]
fn dnd_offer_reaches_the_client() {
    let mut r = rig("sway");
    ok(offer_cmd(&r, false));
    r.env.roundtrip();
    let ev = &r.env.client.ev;
    assert!(ev.dnd.contains(&"enter 10 12".to_string()), "{:?}", ev.dnd);
    assert!(
        ev.dnd.iter().any(|l| l.starts_with("source_actions")),
        "{:?}",
        ev.dnd
    );
    assert_eq!(ev.clipboard.as_ref().unwrap().1, ["text/plain"]);
    ok(r.env
        .rec
        .control(json!({"cmd": "dnd_motion", "at": [20, 21]})));
    r.env.roundtrip();
    assert!(r.env.client.ev.dnd.contains(&"motion 20 21".to_string()));
    ok(r.env.rec.control(json!({"cmd": "dnd_leave"})));
    r.env.roundtrip();
    assert_eq!(r.env.client.ev.dnd.last().unwrap(), "leave");
}

#[test]
fn dnd_silent_source_never_writes() {
    let mut r = rig("sway");
    ok(offer_cmd(&r, true));
    r.env.roundtrip();
    let (offer, _) = r.env.client.ev.clipboard.clone().unwrap();
    offer.set_actions(DndAction::Copy, DndAction::Copy);
    offer.accept(0, Some("text/plain".into()));
    let (ours, theirs) = UnixStream::pair().unwrap();
    offer.receive("text/plain".into(), theirs.as_fd());
    drop(theirs);
    r.env.roundtrip();
    ours.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let err = (&ours)
        .read(&mut [0u8; 1])
        .expect_err("no data and no end of file");
    assert!(matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    let log = ok(r.env.rec.control(json!({"cmd": "dnd_log"})));
    let o = &log["offers"][0];
    assert_eq!(o["received"], json!(["text/plain"]));
    assert_eq!(o["accepted"], json!(["text/plain"]));
    assert_eq!(o["chosen"], "copy");
}

#[test]
fn dnd_finish_without_action_is_an_error() {
    let mut r = rig("sway");
    ok(offer_cmd(&r, false));
    r.env.roundtrip();
    let (offer, _) = r.env.client.ev.clipboard.clone().unwrap();
    offer.accept(0, Some("text/plain".into()));
    r.env.roundtrip();
    ok(r.env.rec.control(json!({"cmd": "dnd_drop"})));
    r.env.roundtrip();
    assert!(
        !r.env.client.ev.dnd.contains(&"drop".to_string()),
        "no action: no drop"
    );
    offer.finish();
    assert!(
        r.env.queue.roundtrip(&mut r.env.client).is_err(),
        "a finish without an action is a protocol error"
    );
}

// --- drags out of the client -------------------------------------------------------------------------------

/// The client presses a button over its surface and starts a drag of `text/plain`.
fn start_drag(r: &mut Rig) -> WlDataSource {
    r.env.client.ev.source_text = "dragged text".into();
    let manager: WlDataDeviceManager = r.env.bind("wl_data_device_manager", 3);
    let source = manager.create_data_source(&r.env.qh, ());
    source.offer("text/plain".into());
    source.set_actions(DndAction::Copy);
    ok(r.env
        .rec
        .control(json!({"cmd": "button", "button": 272, "state": "press"})));
    r.env.roundtrip();
    r.device.start_drag(
        Some(&source),
        &r.surface,
        None,
        r.env.client.ev.button_serial,
    );
    r.env.roundtrip();
    source
}

#[test]
fn client_drag_logged_with_its_serial() {
    let mut r = rig("sway");
    let _source = start_drag(&mut r);
    let log = ok(r.env.rec.control(json!({"cmd": "drag_log"})));
    let d = &log["drags"][0];
    assert_eq!(d["serial_kind"], "button");
    assert_eq!(d["origin"], r.surface.id().protocol_id());
    assert_eq!(d["mimes"], json!(["text/plain"]));
    assert_eq!(d["actions"], json!(["copy"]));
    assert!(d["icon"].is_null());
    assert_eq!(d["result"], "started");
}

#[test]
fn foreign_target_reads_and_finishes() {
    let mut r = rig("sway");
    let _source = start_drag(&mut r);
    ok(r.env
        .rec
        .control(json!({"cmd": "drag_target", "accept": "text/plain", "action": "copy"})));
    let path = r.env.rec.control_path();
    let drop = std::thread::spawn(move || send(&path, json!({"cmd": "drag_drop", "finish": true})));
    wait_for(&mut r.env, |e| {
        e.client.ev.source_events.contains(&"finished".to_string())
    });
    let reply = drop.join().unwrap();
    assert_eq!(reply["result"], "finished", "{reply}");
    assert_eq!(reply["data"], "dragged text");
    let events = &r.env.client.ev.source_events;
    let at = |s: &str| {
        events
            .iter()
            .position(|e| e == s)
            .unwrap_or_else(|| panic!("{s} in {events:?}"))
    };
    assert!(
        at("target Some(\"text/plain\")") < at("drop") && at("drop") < at("finished"),
        "{events:?}"
    );
    let log = ok(r.env.rec.control(json!({"cmd": "drag_log"})));
    assert_eq!(log["drags"][0]["sends"], json!(["text/plain"]));
    assert_eq!(log["drags"][0]["result"], "finished");
}

#[test]
fn drag_cancel_sends_cancelled() {
    let mut r = rig("sway");
    let _source = start_drag(&mut r);
    ok(r.env.rec.control(json!({"cmd": "drag_cancel"})));
    r.env.roundtrip();
    assert!(r
        .env
        .client
        .ev
        .source_events
        .contains(&"cancelled".to_string()));
    let log = ok(r.env.rec.control(json!({"cmd": "drag_log"})));
    assert_eq!(log["drags"][0]["result"], "cancelled");
}

// --- popup grabs -------------------------------------------------------------------------------------------

/// A popup of `parent`, grabbing with `serial`; returns its surface, xdg_surface and xdg_popup.
fn grabbing_popup(
    r: &mut Rig,
    parent: &XdgSurface,
    serial: u32,
) -> (WlSurface, XdgSurface, XdgPopup) {
    let wm: XdgWmBase = r.env.bind("xdg_wm_base", 6);
    let positioner = wm.create_positioner(&r.env.qh, ());
    positioner.set_size(50, 40);
    positioner.set_anchor_rect(5, 6, 10, 10);
    let surface = r.env.surface();
    let xs = wm.get_xdg_surface(&surface, &r.env.qh, ());
    let popup = xs.get_popup(Some(parent), &positioner, &r.env.qh, ());
    popup.grab(&r.seat, serial);
    surface.commit();
    r.env.roundtrip();
    (surface, xs, popup)
}

/// A rig whose client pressed a button: the serial a grab may use.
fn pressed() -> (Rig, u32) {
    let mut r = rig("sway");
    ok(r.env
        .rec
        .control(json!({"cmd": "button", "button": 272, "state": "press"})));
    r.env.roundtrip();
    let serial = r.env.client.ev.button_serial;
    (r, serial)
}

#[test]
fn grab_under_non_topmost_parent_is_an_error() {
    let (mut r, serial) = pressed();
    let top = r.xs.clone();
    let (sa, xa, _pa) = grabbing_popup(&mut r, &top, serial);
    let (_sb, _xb, _pb) = grabbing_popup(&mut r, &xa, serial);
    let tree = ok(r.env.rec.control(json!({"cmd": "tree"})));
    let grab = tree["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == sa.id().protocol_id())
        .unwrap()["grab"]
        .clone();
    assert_eq!(grab, json!({"serial_kind": "button"}));
    // a second popup of the toplevel's popup A, while B is the topmost grab
    let (_sc, _xc, _pc) = {
        let wm: XdgWmBase = r.env.bind("xdg_wm_base", 6);
        let positioner = wm.create_positioner(&r.env.qh, ());
        positioner.set_size(20, 20);
        positioner.set_anchor_rect(1, 1, 5, 5);
        let surface = r.env.surface();
        let xs = wm.get_xdg_surface(&surface, &r.env.qh, ());
        let popup = xs.get_popup(Some(&xa), &positioner, &r.env.qh, ());
        popup.grab(&r.seat, serial);
        surface.commit();
        (surface, xs, popup)
    };
    assert!(
        r.env.queue.roundtrip(&mut r.env.client).is_err(),
        "invalid_grab is a protocol error"
    );
}

#[test]
fn destroying_a_non_topmost_popup_is_an_error() {
    let (mut r, serial) = pressed();
    let top = r.xs.clone();
    let (_sa, xa, pa) = grabbing_popup(&mut r, &top, serial);
    let (_sb, _xb, _pb) = grabbing_popup(&mut r, &xa, serial);
    pa.destroy();
    assert!(
        r.env.queue.roundtrip(&mut r.env.client).is_err(),
        "not_the_topmost_popup is a protocol error"
    );
}

#[test]
fn click_outside_dismisses_topmost_first() {
    let (mut r, serial) = pressed();
    let top = r.xs.clone();
    let (_sa, xa, pa) = grabbing_popup(&mut r, &top, serial);
    let (_sb, _xb, pb) = grabbing_popup(&mut r, &xa, serial);
    ok(r.env.rec.control(json!({"cmd": "click_outside"})));
    r.env.roundtrip();
    assert_eq!(
        r.env.client.ev.popup_done_ids,
        [pb.id().protocol_id(), pa.id().protocol_id()]
    );
}

#[test]
fn stale_grab_serial_is_dismissed_at_once() {
    let (mut r, serial) = pressed();
    let top = r.xs.clone();
    let (_s, _x, _p) = grabbing_popup(&mut r, &top, serial + 1000);
    assert_eq!(r.env.client.ev.popup_done, 1);
}

// --- touch and pinch ---------------------------------------------------------------------------------------

#[test]
fn touch_points_delivered_with_frame() {
    let mut env = Env::new("sway", &[]);
    let seat: WlSeat = env.bind("wl_seat", 9);
    let _touch: WlTouch = seat.get_touch(&env.qh, ());
    let (_surface, _xs, _top) = env.toplevel(100, 100);
    env.roundtrip();
    let down = ok(env
        .rec
        .control(json!({"cmd": "touch", "op": "down", "id": 0, "at": [5, 6]})));
    ok(env
        .rec
        .control(json!({"cmd": "touch", "op": "motion", "id": 0, "at": [7, 8]})));
    ok(env
        .rec
        .control(json!({"cmd": "touch", "op": "up", "id": 0})));
    env.roundtrip();
    assert_eq!(
        env.client.ev.touch,
        [
            "down 0 5 6",
            "frame",
            "motion 0 7 8",
            "frame",
            "up 0",
            "frame"
        ]
    );
    assert_eq!(
        env.client.ev.touch_serial as u64,
        down["serial"].as_u64().unwrap()
    );
}

#[test]
fn pinch_begin_update_end() {
    let mut env = Env::new("sway", &[]);
    let seat: WlSeat = env.bind("wl_seat", 9);
    let pointer: WlPointer = seat.get_pointer(&env.qh, ());
    let gestures: ZwpPointerGesturesV1 = env.bind("zwp_pointer_gestures_v1", 3);
    let _pinch = gestures.get_pinch_gesture(&pointer, &env.qh, ());
    let (surface, _xs, _top) = env.toplevel(100, 100);
    let id = surface.id().protocol_id();
    ok(env.rec.control(json!({"cmd": "focus", "surface": id})));
    ok(env.rec.control(json!({"cmd": "pointer", "pos": [4, 4]})));
    ok(env
        .rec
        .control(json!({"cmd": "pinch", "op": "begin", "fingers": 2})));
    ok(env.rec.control(
        json!({"cmd": "pinch", "op": "update", "dx": 1, "dy": 2, "scale": 1.5, "rotation": 10}),
    ));
    ok(env.rec.control(json!({"cmd": "pinch", "op": "end"})));
    env.roundtrip();
    assert_eq!(
        env.client.ev.pinch,
        ["begin 2", "update 1 2 1.5 10", "end 0"]
    );
}

// --- bell, idle inhibit, export, sessions, restart -------------------------------------------------------------

/// The client side of xdg session management, generated from the XML the recorder serves.
#[allow(
    clippy::all,
    dead_code,
    unused_imports,
    non_camel_case_types,
    missing_docs
)]
mod session_client {
    use wayland_client;
    use wayland_client::protocol::*;
    use wayland_protocols::xdg::shell::client::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        use wayland_protocols::xdg::shell::client::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocols/xdg-session-management-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("./protocols/xdg-session-management-v1.xml");
}
use session_client::xdg_session_manager_v1::{Reason, XdgSessionManagerV1};
use session_client::xdg_session_v1::{self, XdgSessionV1};
use session_client::xdg_toplevel_session_v1::{self, XdgToplevelSessionV1};
use std::sync::{Arc, Mutex};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::idle_inhibit::zv1::client::{
    zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1, zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1,
};
use wayland_protocols::xdg::foreign::zv2::client::{
    zxdg_exported_v2::{self, ZxdgExportedV2},
    zxdg_exporter_v2::ZxdgExporterV2,
};
use wayland_protocols::xdg::system_bell::v1::client::xdg_system_bell_v1::XdgSystemBellV1;

/// What a session object or a toplevel's session object heard, as text.
type Heard = Arc<Mutex<Vec<String>>>;

macro_rules! no_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for common::Client {
            fn event(
                _: &mut Self,
                _: &$t,
                _: <$t as wayland_client::Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}
no_events!(
    XdgSystemBellV1,
    ZwpIdleInhibitManagerV1,
    ZwpIdleInhibitorV1,
    ZxdgExporterV2,
    XdgSessionManagerV1
);

impl Dispatch<ZxdgExportedV2, Arc<Mutex<Option<String>>>> for common::Client {
    fn event(
        _: &mut Self,
        _: &ZxdgExportedV2,
        e: zxdg_exported_v2::Event,
        handle: &Arc<Mutex<Option<String>>>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let zxdg_exported_v2::Event::Handle { handle: h } = e else {
            return;
        };
        *handle.lock().unwrap() = Some(h);
    }
}

impl Dispatch<XdgSessionV1, Heard> for common::Client {
    fn event(
        _: &mut Self,
        _: &XdgSessionV1,
        e: xdg_session_v1::Event,
        heard: &Heard,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let text = match e {
            xdg_session_v1::Event::Created { session_id } => format!("created {session_id}"),
            xdg_session_v1::Event::Restored => "restored".to_string(),
            xdg_session_v1::Event::Replaced => "replaced".to_string(),
        };
        heard.lock().unwrap().push(text);
    }
}

impl Dispatch<XdgToplevelSessionV1, Heard> for common::Client {
    fn event(
        _: &mut Self,
        _: &XdgToplevelSessionV1,
        e: xdg_toplevel_session_v1::Event,
        heard: &Heard,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let xdg_toplevel_session_v1::Event::Restored = e;
        heard.lock().unwrap().push("restored".to_string());
    }
}

/// A session of `reason` (`id` None: a new one) on a client; what it hears lands in the returned list.
fn get_session(env: &mut Env, reason: Reason, id: Option<&str>) -> (XdgSessionV1, Heard) {
    let manager: XdgSessionManagerV1 = env.bind("xdg_session_manager_v1", 1);
    let heard = Heard::default();
    let session = manager.get_session(reason, id.map(String::from), &env.qh, heard.clone());
    env.roundtrip();
    (session, heard)
}

/// The id a new session was created with.
fn created_id(heard: &Heard) -> String {
    let all = heard.lock().unwrap();
    all.iter()
        .find_map(|l| l.strip_prefix("created ").map(String::from))
        .unwrap_or_else(|| panic!("no created event: {all:?}"))
}

/// An xdg_toplevel that is added to or restored in `session` as `name` before its first commit, then mapped at
/// `w` x `h`.  Returns what the toplevel's session object heard.
fn session_toplevel(
    env: &mut Env,
    session: &XdgSessionV1,
    name: &str,
    restore: bool,
    w: i32,
    h: i32,
) -> Heard {
    let wm: XdgWmBase = env.bind("xdg_wm_base", 6);
    let surface = env.surface();
    let xs = wm.get_xdg_surface(&surface, &env.qh, ());
    let top = xs.get_toplevel(&env.qh, ());
    let heard = Heard::default();
    if restore {
        session.restore_toplevel(&top, name.into(), &env.qh, heard.clone());
    } else {
        session.add_toplevel(&top, name.into(), &env.qh, heard.clone());
    }
    surface.commit();
    env.roundtrip();
    xs.ack_configure(env.client.xdg_serial);
    let buffer = env.buffer(w, h);
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    env.roundtrip();
    heard
}

#[test]
fn bell_logged() {
    let mut env = Env::new("mutter", &[]);
    let bell: XdgSystemBellV1 = env.bind("xdg_system_bell_v1", 1);
    let (surface, _xs, _top) = env.toplevel(100, 100);
    bell.ring(Some(&surface));
    bell.ring(None);
    env.roundtrip();
    let id = surface.id().protocol_id();
    let log = ok(env.rec.control(json!({"cmd": "bell_log"})));
    assert_eq!(log["bells"], json!([{"surface": id}, {"surface": null}]));
}

#[test]
fn idle_inhibitor_tracked() {
    let mut env = Env::new("sway", &[]);
    let manager: ZwpIdleInhibitManagerV1 = env.bind("zwp_idle_inhibit_manager_v1", 1);
    let (surface, _xs, _top) = env.toplevel(100, 100);
    let inhibitor = manager.create_inhibitor(&surface, &env.qh, ());
    env.roundtrip();
    let id = surface.id().protocol_id();
    let list = ok(env.rec.control(json!({"cmd": "idle_inhibitors"})));
    assert_eq!(list["inhibitors"], json!([{"surface": id, "alive": true}]));
    inhibitor.destroy();
    env.roundtrip();
    let list = ok(env.rec.control(json!({"cmd": "idle_inhibitors"})));
    assert_eq!(list["inhibitors"], json!([{"surface": id, "alive": false}]));
}

#[test]
fn export_returns_a_handle() {
    let mut env = Env::new("sway", &[]);
    let exporter: ZxdgExporterV2 = env.bind("zxdg_exporter_v2", 1);
    let (surface, _xs, _top) = env.toplevel(100, 100);
    let handle = Arc::new(Mutex::new(None));
    let _exported = exporter.export_toplevel(&surface, &env.qh, handle.clone());
    env.roundtrip();
    assert_eq!(handle.lock().unwrap().as_deref(), Some("rec-1"));
    let id = surface.id().protocol_id();
    let list = ok(env.rec.control(json!({"cmd": "exports"})));
    assert_eq!(list["exports"], json!([{"surface": id, "handle": "rec-1"}]));
}

#[test]
fn session_restore_before_first_commit() {
    let dir = std::env::temp_dir().join(format!("nh-session-{}-a", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("store.json");
    let mut first = Env::new("mutter", &["--session-store", store.to_str().unwrap()]);
    let (old, old_heard) = get_session(&mut first, Reason::Launch, None);
    let id = created_id(&old_heard);
    session_toplevel(&mut first, &old, "main", false, 300, 200);
    // another client takes the session over: the first one is told, and the toplevel comes back at its size
    let mut second = first.join();
    let (taken, new_heard) = get_session(&mut second, Reason::Recover, Some(&id));
    assert_eq!(*new_heard.lock().unwrap(), ["restored"]);
    second.client.configures.clear();
    let ts = session_toplevel(&mut second, &taken, "main", true, 300, 200);
    first.roundtrip();
    assert_eq!(old_heard.lock().unwrap().last().unwrap(), "replaced");
    assert_eq!(*ts.lock().unwrap(), ["restored"]);
    let (w, h, _) = second.client.configures.first().cloned().unwrap();
    assert_eq!((w, h), (300, 200));
    let log = ok(second.rec.control(json!({"cmd": "session_log"})));
    let last = log["session_log"].as_array().unwrap().last().unwrap();
    assert_eq!(last["op"], "restore");
    assert_eq!(last["name"], "main");
    assert_eq!(last["before_first_commit"], true);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_restore_after_commit_is_already_mapped() {
    let mut env = Env::new("mutter", &[]);
    let (session, _heard) = get_session(&mut env, Reason::Launch, None);
    let (_surface, _xs, top) = env.toplevel(100, 100);
    session.restore_toplevel(&top, "late".into(), &env.qh, Heard::default());
    let result = env.queue.roundtrip(&mut env.client);
    assert!(result.is_err(), "already_mapped is a protocol error");
    let log = ok(env.rec.control(json!({"cmd": "session_log"})));
    let last = log["session_log"].as_array().unwrap().last().unwrap();
    assert_eq!(last["op"], "restore");
    assert_eq!(last["before_first_commit"], false);
}

/// Wait for a recorder's control socket to go: it has quit and cleaned up.
fn wait_until_gone(path: &std::path::Path) {
    let end = Instant::now() + Duration::from_secs(5);
    while path.exists() {
        assert!(Instant::now() < end, "the recorder did not quit");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn session_persists_across_instances() {
    let dir = std::env::temp_dir().join(format!("nh-session-{}-b", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("store.json");
    let args = ["--session-store", store.to_str().unwrap()];
    let id = {
        let mut env = Env::new("mutter", &args);
        let (session, heard) = get_session(&mut env, Reason::Launch, None);
        session_toplevel(&mut env, &session, "main", false, 320, 240);
        let control = env.rec.control_path();
        ok(env.rec.control(json!({"cmd": "quit"})));
        wait_until_gone(&control);
        created_id(&heard)
    };
    let mut env = Env::new("mutter", &args);
    let (session, heard) = get_session(&mut env, Reason::Recover, Some(&id));
    assert_eq!(*heard.lock().unwrap(), ["restored"]);
    env.client.configures.clear();
    let ts = session_toplevel(&mut env, &session, "main", true, 320, 240);
    assert_eq!(*ts.lock().unwrap(), ["restored"]);
    let (w, h, _) = env.client.configures.first().cloned().unwrap();
    assert_eq!((w, h), (320, 240));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn quit_frees_the_socket_name() {
    let dir = std::env::temp_dir().join(format!("nh-quit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let profile = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/sway.json");
    let start = || {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_nh-recorder"))
            .args(["--socket", "nh-quit-sock", "--control"])
            .arg(dir.join("control"))
            .arg("--profile")
            .arg(&profile)
            .env("XDG_RUNTIME_DIR", &dir)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(child.stdout.take().unwrap()),
            &mut line,
        )
        .unwrap();
        assert_eq!(line, "READY\n");
        child
    };
    let mut a = start();
    ok(send(&dir.join("control"), json!({"cmd": "quit"})));
    assert!(a.wait().unwrap().success());
    assert!(!dir.join("nh-quit-sock").exists());
    assert!(!dir.join("nh-quit-sock.lock").exists());
    let mut b = start();
    assert!(UnixStream::connect(dir.join("nh-quit-sock")).is_ok());
    let _ = b.kill();
    let _ = b.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

// --- restart in place ---------------------------------------------------------------------------------------

/// The path of a recorder's Wayland socket (the harness names it like its directory).
fn socket_path(env: &Env) -> std::path::PathBuf {
    env.rec.dir.join(env.rec.dir.file_name().unwrap())
}

/// A new client's registry listing, read with a roundtrip (which waits until the recorder serves it).
fn list_globals(env: &Env) -> common::Client {
    let conn = env.rec.connect();
    let mut queue = conn.new_event_queue::<common::Client>();
    let _registry = conn.display().get_registry(&queue.handle(), ());
    let mut client = common::Client::default();
    queue.roundtrip(&mut client).unwrap();
    client
}

fn restart(env: &Env, mode: &str, down_ms: u64) -> Value {
    ok(env
        .rec
        .control(json!({"cmd": "restart", "mode": mode, "down_ms": down_ms})))
}

#[test]
fn connections_counted() {
    let env = Env::new("sway", &[]);
    let _other = env.join();
    let n = ok(env.rec.control(json!({"cmd": "connections"})));
    assert_eq!(
        (n["accepted"].as_u64(), n["live"].as_u64()),
        (Some(2), Some(2))
    );
    restart(&env, "keep-socket", 0);
    let n = ok(env.rec.control(json!({"cmd": "connections"})));
    assert_eq!(
        (n["accepted"].as_u64(), n["live"].as_u64()),
        (Some(2), Some(0))
    );
    let _again = list_globals(&env);
    let n = ok(env.rec.control(json!({"cmd": "connections"})));
    assert_eq!(n["accepted"], 3);
}

#[test]
fn restart_keep_socket_queues_connections() {
    let mut env = Env::new("sway", &[]);
    let _ = env.toplevel(100, 100);
    let started = Instant::now();
    restart(&env, "keep-socket", 400);
    // the connect succeeds at once; the registry answer comes after the recorder listens again
    assert!(UnixStream::connect(socket_path(&env)).is_ok());
    let client = list_globals(&env);
    assert!(
        started.elapsed() >= Duration::from_millis(350),
        "{:?}",
        started.elapsed()
    );
    assert!(client.names.contains_key("xdg_wm_base"));
    // the old client is gone
    assert!(env.queue.roundtrip(&mut env.client).is_err());
}

#[test]
fn restart_rebind_frees_and_reuses_the_name() {
    let env = Env::new("sway", &[]);
    restart(&env, "rebind", 300);
    assert!(!socket_path(&env).exists());
    assert!(UnixStream::connect(socket_path(&env)).is_err());
    let end = Instant::now() + Duration::from_secs(5);
    while !socket_path(&env).exists() {
        assert!(Instant::now() < end, "the name was not bound again");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(list_globals(&env).names.contains_key("wl_compositor"));
}

#[test]
fn restart_gone_refuses_until_up() {
    let env = Env::new("sway", &[]);
    restart(&env, "gone", 0);
    std::thread::sleep(Duration::from_millis(200));
    assert!(UnixStream::connect(socket_path(&env)).is_err());
    ok(env.rec.control(json!({"cmd": "restart_up"})));
    assert!(list_globals(&env).names.contains_key("wl_compositor"));
}

#[test]
fn restart_resets_state_but_keeps_the_session_store() {
    let dir = std::env::temp_dir().join(format!("nh-session-{}-c", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("store.json");
    let mut env = Env::new("mutter", &["--session-store", store.to_str().unwrap()]);
    let (session, heard) = get_session(&mut env, Reason::Launch, None);
    let id = created_id(&heard);
    session_toplevel(&mut env, &session, "main", false, 300, 200);
    assert!(!ok(env.rec.control(json!({"cmd": "tree"})))["surfaces"]
        .as_array()
        .unwrap()
        .is_empty());
    let marker = ok(env.rec.control(json!({"cmd": "log", "since": 0})))["next"].clone();
    restart(&env, "keep-socket", 0);
    // compositor state is fresh: no surfaces, no session log, no clients
    let tree = ok(env.rec.control(json!({"cmd": "tree"})));
    assert!(tree["surfaces"].as_array().unwrap().is_empty(), "{tree}");
    assert_eq!(
        ok(env.rec.control(json!({"cmd": "session_log"})))["session_log"],
        json!([])
    );
    assert_eq!(
        ok(env.rec.control(json!({"cmd": "clients"})))["clients"],
        json!([])
    );
    let log = ok(env.rec.control(json!({"cmd": "log", "since": 0})));
    assert_eq!(log["restarts"], json!([{"restart": 1, "seq": marker}]));
    // the store survived: a new client recovers the session and gets its toplevel back at its size
    let mut again = env.join();
    let (taken, heard) = get_session(&mut again, Reason::Recover, Some(&id));
    assert_eq!(*heard.lock().unwrap(), ["restored"]);
    again.client.configures.clear();
    let ts = session_toplevel(&mut again, &taken, "main", true, 300, 200);
    assert_eq!(*ts.lock().unwrap(), ["restored"]);
    assert_eq!(
        again.client.configures.first().map(|c| (c.0, c.1)),
        Some((300, 200))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn restart_with_another_profile() {
    let mut env = Env::new("mutter", &[]);
    assert_eq!(env.client.versions["xdg_wm_base"], 6);
    ok(env
        .rec
        .control(json!({"cmd": "restart", "mode": "keep-socket", "profile": "sway"})));
    let client = list_globals(&env);
    assert_eq!(client.versions["xdg_wm_base"], 5);
    assert!(!client.names.contains_key("xdg_session_manager_v1"));
    assert!(client.names.contains_key("wp_alpha_modifier_v1"));
    assert_eq!(
        ok(env.rec.control(json!({"cmd": "globals"})))["globals"]["xdg_wm_base"],
        5
    );
    // and a later restart without a profile keeps the new one
    restart(&env, "keep-socket", 0);
    assert_eq!(list_globals(&env).versions["xdg_wm_base"], 5);
    assert!(env.queue.roundtrip(&mut env.client).is_err());
    let bad = env
        .rec
        .control(json!({"cmd": "restart", "profile": "nope"}));
    assert_eq!(bad["ok"], false);
}
