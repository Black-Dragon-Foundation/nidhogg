//! T0 tests of the W1 recorder additions: selections, primary selection, runtime outputs, xdg-activation, the surface
//! tree, popup dismissal and the alpha multiplier.
mod common;
use common::{send, Env};
use serde_json::{json, Value};
use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};
use wayland_client::protocol::{
    wl_data_device::WlDataDevice, wl_data_device_manager::WlDataDeviceManager, wl_output::WlOutput,
    wl_seat::WlSeat, wl_subcompositor::WlSubcompositor,
};
use wayland_client::Proxy;
use wayland_protocols::wp::alpha_modifier::v1::client::wp_alpha_modifier_v1::WpAlphaModifierV1;
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;
use wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase;

/// A client with a mapped toplevel, keyboard focus on it and a data device.
fn focused(
    profile: &str,
) -> (
    Env,
    WlDataDevice,
    wayland_client::protocol::wl_surface::WlSurface,
) {
    let mut env = Env::new(profile, &[]);
    let seat: WlSeat = env.bind("wl_seat", 9);
    let manager: WlDataDeviceManager = env.bind("wl_data_device_manager", 3);
    let device = manager.get_data_device(&seat, &env.qh, ());
    let (surface, _xs, _top) = env.toplevel(100, 100);
    let id = surface.id().protocol_id();
    assert_eq!(
        env.rec.control(json!({"cmd": "focus", "surface": id}))["ok"],
        true
    );
    env.roundtrip();
    (env, device, surface)
}

fn read_all(mut s: UnixStream) -> String {
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn selection_offer_reaches_a_client() {
    let (mut env, _device, _surface) = focused("sway");
    let reply = env.rec.control(json!({"cmd": "offer", "kind": "clipboard",
        "types": {"text/plain;charset=utf-8": "hello", "text/html": "<b>hi</b>"}}));
    assert_eq!(reply["ok"], true, "{reply}");
    env.roundtrip();
    let (offer, mut types) = env.client.ev.clipboard.clone().expect("a selection offer");
    types.sort();
    assert_eq!(types, ["text/html", "text/plain;charset=utf-8"]);
    let (ours, theirs) = UnixStream::pair().unwrap();
    offer.receive("text/html".into(), theirs.as_fd());
    drop(theirs);
    env.roundtrip();
    assert_eq!(read_all(ours), "<b>hi</b>");
}

#[test]
fn selection_read_returns_client_data() {
    let (mut env, device, _surface) = focused("sway");
    env.client.ev.source_text = "from the client".into();
    let manager: WlDataDeviceManager = env.bind("wl_data_device_manager", 3);
    let source = manager.create_data_source(&env.qh, ());
    source.offer("text/plain".into());
    device.set_selection(Some(&source), 1);
    env.roundtrip();
    let path = env.rec.control_path();
    let reader = std::thread::spawn(move || {
        send(
            &path,
            json!({"cmd": "read", "kind": "clipboard", "type": "text/plain"}),
        )
    });
    wait_for(&mut env, |e| !e.client.ev.sends.is_empty());
    let reply = reader.join().unwrap();
    assert_eq!(reply, json!({"ok": true, "data": "from the client"}));
    let none = env
        .rec
        .control(json!({"cmd": "read", "kind": "clipboard", "type": "image/png"}));
    assert_eq!(
        none["ok"], false,
        "a type the client does not offer: {none}"
    );
}

/// Dispatch the client until `done` holds (5 s at most).
fn wait_for(env: &mut Env, done: impl Fn(&Env) -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !done(env) {
        assert!(Instant::now() < end, "timed out waiting for the client");
        env.roundtrip();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn silent_offer_times_out_cleanly() {
    let (mut env, device, _surface) = focused("sway");
    // a silent offer takes the receive and never writes: the reader sees neither data nor an end
    env.rec.control(
        json!({"cmd": "offer", "kind": "clipboard", "types": {"text/plain": "x"}, "silent": true}),
    );
    env.roundtrip();
    let (offer, _) = env.client.ev.clipboard.clone().unwrap();
    let (ours, theirs) = UnixStream::pair().unwrap();
    offer.receive("text/plain".into(), theirs.as_fd());
    drop(theirs);
    env.roundtrip();
    ours.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let mut byte = [0u8; 1];
    let err = (&ours)
        .read(&mut byte)
        .expect_err("no data and no end of file");
    assert!(matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    // a client that owns the selection and stays silent: `read` answers {"ok":false} after 1 s, and the loop stays live
    env.client.ev.source_mute = true;
    let manager: WlDataDeviceManager = env.bind("wl_data_device_manager", 3);
    let source = manager.create_data_source(&env.qh, ());
    source.offer("text/plain".into());
    device.set_selection(Some(&source), 2);
    env.roundtrip();
    let path = env.rec.control_path();
    let started = Instant::now();
    let reader = std::thread::spawn(move || {
        send(
            &path,
            json!({"cmd": "read", "kind": "clipboard", "type": "text/plain"}),
        )
    });
    wait_for(&mut env, |e| !e.client.ev.sends.is_empty());
    let globals = env.rec.control(json!({"cmd": "globals"}));
    assert_eq!(globals["ok"], true, "the loop answers while a read waits");
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "answered promptly, not after the deadline"
    );
    assert_eq!(reader.join().unwrap(), json!({"ok": false}));
    assert!(started.elapsed() >= Duration::from_millis(900));
}

#[test]
fn primary_selection_served() {
    for profile in ["sway", "kwin", "mutter", "no-fractional", "no-cursor-shape"] {
        let env = Env::new(profile, &[]);
        assert_eq!(
            env.client.versions["zwp_primary_selection_device_manager_v1"], 1,
            "{profile}"
        );
        assert_eq!(env.client.versions["xdg_activation_v1"], 1, "{profile}");
        // the profile without a wl_data_device_manager (no-fractional) keeps its list
        if profile != "no-fractional" {
            assert_eq!(
                env.client.versions["wl_data_device_manager"], 3,
                "{profile}"
            );
        }
    }
    let no_shape = Env::new("no-cursor-shape", &[]);
    assert!(!no_shape
        .client
        .versions
        .contains_key("wp_cursor_shape_manager_v1"));
    let mut env = Env::new("sway", &[]);
    let seat: WlSeat = env.bind("wl_seat", 9);
    let manager: ZwpPrimarySelectionDeviceManagerV1 =
        env.bind("zwp_primary_selection_device_manager_v1", 1);
    let _device = manager.get_device(&seat, &env.qh, ());
    let (surface, _xs, _top) = env.toplevel(50, 50);
    env.rec
        .control(json!({"cmd": "focus", "surface": surface.id().protocol_id()}));
    let reply = env
        .rec
        .control(json!({"cmd": "offer", "kind": "primary", "types": {"text/plain": "sel"}}));
    assert_eq!(reply["ok"], true, "{reply}");
    env.roundtrip();
    let (offer, types) = env.client.ev.primary.clone().expect("a primary offer");
    assert_eq!(types, ["text/plain"]);
    let (ours, theirs) = UnixStream::pair().unwrap();
    offer.receive("text/plain".into(), theirs.as_fd());
    drop(theirs);
    env.roundtrip();
    assert_eq!(read_all(ours), "sel");
}

#[test]
fn output_add_and_remove_at_runtime() {
    let mut env = Env::new("sway", &[]);
    assert_eq!(env.client.ev.outputs.len(), 1);
    let reply = env.rec.control(
        json!({"cmd": "output", "op": "add", "name": "nh-1", "size": [800, 600], "scale120": 240}),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    env.roundtrip();
    assert_eq!(env.client.ev.outputs.len(), 2);
    let second = env.client.ev.outputs[1];
    let _out: WlOutput = env.registry.bind(second, 4, &env.qh, ());
    env.roundtrip();
    assert_eq!(env.client.output_scales.last(), Some(&2));
    env.rec
        .control(json!({"cmd": "output", "op": "scale", "name": "nh-1", "scale120": 360}));
    env.roundtrip();
    assert_eq!(env.client.output_scales.last(), Some(&3));
    let reply = env
        .rec
        .control(json!({"cmd": "output", "op": "add", "name": "nh-1", "size": [1, 1]}));
    assert_eq!(reply["ok"], false, "a name is unique");
    let reply = env
        .rec
        .control(json!({"cmd": "output", "op": "remove", "name": "nh-1"}));
    assert_eq!(reply["ok"], true, "{reply}");
    env.roundtrip();
    assert_eq!(env.client.ev.outputs.len(), 1);
}

#[test]
fn activation_logged() {
    let mut env = Env::new("sway", &[]);
    let activation: XdgActivationV1 = env.bind("xdg_activation_v1", 1);
    let (surface, _xs, _top) = env.toplevel(50, 50);
    let token = activation.get_activation_token(&env.qh, ());
    token.set_app_id("org.example.app".into());
    token.commit();
    env.roundtrip();
    let token_text = env.client.ev.activation_token.clone().expect("a token");
    activation.activate(token_text.clone(), &surface);
    env.roundtrip();
    let log = env.rec.control(json!({"cmd": "activation_log"}));
    assert_eq!(
        log["activations"],
        json!([{"token": token_text, "surface": surface.id().protocol_id()}])
    );
}

fn node(tree: &Value, id: u32) -> &Value {
    tree["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
        .expect("a node")
}

#[test]
fn tree_reports_subsurface_position_and_stack() {
    let mut env = Env::new("sway", &[]);
    let (top, _xs, _t) = env.toplevel(200, 200);
    let sub: WlSubcompositor = env.bind("wl_subcompositor", 1);
    let compositor: wayland_client::protocol::wl_compositor::WlCompositor =
        env.bind("wl_compositor", 6);
    let (a, b) = (env.surface(), env.surface());
    let sa = sub.get_subsurface(&a, &top, &env.qh, ());
    let sb = sub.get_subsurface(&b, &top, &env.qh, ());
    sa.set_position(10, 20);
    sb.set_position(30, 40);
    let region = compositor.create_region(&env.qh, ());
    region.add(1, 2, 3, 4);
    a.set_input_region(Some(&region));
    for s in [&a, &b] {
        let buffer = env.buffer(16, 8);
        s.attach(Some(&buffer), 0, 0);
        s.commit();
    }
    top.commit();
    env.roundtrip();
    let (ia, ib, it) = (
        a.id().protocol_id(),
        b.id().protocol_id(),
        top.id().protocol_id(),
    );
    let tree = env.rec.control(json!({"cmd": "tree"}));
    let (na, nb, nt) = (node(&tree, ia), node(&tree, ib), node(&tree, it));
    assert_eq!(na["role"], "subsurface");
    assert_eq!(
        (na["parent"].clone(), na["position"].clone()),
        (json!(it), json!([10, 20]))
    );
    assert_eq!(nb["position"], json!([30, 40]));
    assert_eq!(
        na["above"],
        json!([ib]),
        "b was created later, so it is above a"
    );
    assert_eq!(nb["above"], json!([]));
    assert_eq!(nt["children"], json!([ia, ib]));
    assert_eq!(
        (na["mapped"].clone(), na["buffer"].clone()),
        (json!(true), json!({"w": 16, "h": 8}))
    );
    assert_eq!(na["input_region"], json!([[1, 2, 3, 4]]));
    assert_eq!(nb["input_region"], Value::Null);
    assert_eq!(nt["role"], "toplevel");
    // restack: a above b, applied by the parent's commit
    sa.place_above(&b);
    top.commit();
    env.roundtrip();
    let tree = env.rec.control(json!({"cmd": "tree"}));
    assert_eq!(node(&tree, ia)["above"], json!([]));
    assert_eq!(node(&tree, ib)["above"], json!([ia]));
    let roles: Vec<String> = env
        .rec
        .commits()
        .iter()
        .map(|c| c["role"].as_str().unwrap().to_string())
        .collect();
    assert!(roles.iter().any(|r| r == "subsurface"), "{roles:?}");
}

#[test]
fn popup_done_sent() {
    let mut env = Env::new("sway", &[]);
    let (_top, xs, _t) = env.toplevel(200, 200);
    let wm: XdgWmBase = env.bind("xdg_wm_base", 6);
    let positioner = wm.create_positioner(&env.qh, ());
    positioner.set_size(50, 40);
    positioner.set_anchor_rect(5, 6, 10, 10);
    let surface = env.surface();
    let pxs = wm.get_xdg_surface(&surface, &env.qh, ());
    let _popup = pxs.get_popup(Some(&xs), &positioner, &env.qh, ());
    surface.commit();
    env.roundtrip();
    assert_eq!(env.client.ev.popup_done, 0);
    let tree = env.rec.control(json!({"cmd": "tree"}));
    assert_eq!(node(&tree, surface.id().protocol_id())["role"], "popup");
    assert_eq!(
        env.rec
            .control(json!({"cmd": "popup_done", "surface": "newest"}))["ok"],
        true
    );
    env.roundtrip();
    assert_eq!(env.client.ev.popup_done, 1);
    assert!(env.rec.commits().iter().any(|c| c["role"] == "popup"));
    assert_eq!(
        env.rec
            .control(json!({"cmd": "popup_done", "surface": 9999}))["ok"],
        false
    );
}

#[test]
fn alpha_multiplier_in_commit_log() {
    let mut env = Env::new("sway", &[]);
    let alpha: WpAlphaModifierV1 = env.bind("wp_alpha_modifier_v1", 1);
    let (surface, _xs, _t) = env.toplevel(20, 20);
    alpha
        .get_surface(&surface, &env.qh, ())
        .set_multiplier(0x8000_0000);
    surface.commit();
    env.roundtrip();
    let commits = env.rec.commits();
    let last = commits.last().unwrap();
    let a = last["alpha"].as_f64().expect("an alpha");
    assert!((a - 0.5).abs() < 0.001, "{a}");
    assert!(commits[0].get("alpha").is_none(), "absent when unset");
}
