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
