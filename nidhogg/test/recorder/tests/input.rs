//! T0 tests of the seat: keymaps from RMLVO, pointer axis, text-input-v3, cursor requests, toplevel requests.
mod common;
use common::Env;
use serde_json::json;
use wayland_client::protocol::{wl_keyboard::WlKeyboard, wl_pointer::WlPointer, wl_seat::WlSeat};
use wayland_client::Proxy;
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1::Shape, wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3;
use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;
use xkbcommon::xkb;

fn entries(env: &Env) -> Vec<serde_json::Value> {
    env.rec.control(json!({"cmd": "input_log", "since": 0}))["entries"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn keymap_from_rmlvo_with_options() {
    let mut env = Env::new("sway", &[]);
    let reply = env.rec.control(json!({"cmd": "keymap", "rmlvo": {
        "rules": "evdev", "model": "pc105", "layout": "us", "variant": "", "options": "ctrl:swap_lalt_lctl_lwin"}}));
    assert_eq!(reply["ok"], true, "{reply}");
    let seat: WlSeat = env.bind("wl_seat", 9);
    let _kb: WlKeyboard = seat.get_keyboard(&env.qh, ());
    env.roundtrip();
    let text = env.client.ev.keymap.clone().expect("a keymap event");
    let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let keymap = xkb::Keymap::new_from_string(&ctx, text, xkb::KEYMAP_FORMAT_TEXT_V1, 0)
        .expect("the client's keymap compiles");
    // evdev KEY_LEFTALT is 56; xkb keycodes are evdev + 8
    let syms = keymap.key_get_syms_by_level(xkb::Keycode::new(56 + 8), 0, 0);
    assert_eq!(syms, &[xkb::Keysym::Control_L]);
}

#[test]
fn axis_value120_on_seat9() {
    let mut env = Env::new("sway", &[]);
    let (_surface, _xs, _top) = env.toplevel(200, 100);
    let seat: WlSeat = env.bind("wl_seat", 9);
    assert!(seat.version() >= 8);
    let _ptr: WlPointer = seat.get_pointer(&env.qh, ());
    env.roundtrip();
    env.rec
        .control(json!({"cmd": "focus", "surface": "toplevel"}));
    env.rec.control(json!({"cmd": "pointer", "pos": [5, 5]}));
    env.roundtrip();
    let reply = env
        .rec
        .control(json!({"cmd": "axis", "source": "wheel", "v120": 120, "stop": false}));
    assert_eq!(reply["ok"], true, "{reply}");
    env.roundtrip();
    let p = &env.client.ev.pointer;
    assert!(p.contains(&"axis_source Wheel".to_string()), "{p:?}");
    assert!(
        p.contains(&"axis_value120 VerticalScroll 120".to_string()),
        "{p:?}"
    );
    assert!(
        p.iter().any(|l| l.starts_with("axis VerticalScroll")),
        "{p:?}"
    );
}

#[test]
fn text_input_preedit_then_commit_order() {
    let mut env = Env::new("sway", &[]);
    let (_surface, _xs, _top) = env.toplevel(200, 100);
    let seat: WlSeat = env.bind("wl_seat", 9);
    let manager: ZwpTextInputManagerV3 = env.bind("zwp_text_input_manager_v3", 1);
    let ti = manager.get_text_input(&seat, &env.qh, ());
    env.rec
        .control(json!({"cmd": "focus", "surface": "toplevel"}));
    ti.enable();
    ti.set_cursor_rectangle(1, 2, 3, 4);
    ti.commit();
    env.roundtrip();
    let reply = env
        .rec
        .control(json!({"cmd": "text_input", "preedit": "ab", "cursor": [0, 1], "commit": "x"}));
    assert_eq!(reply["ok"], true, "{reply}");
    env.roundtrip();
    assert_eq!(
        env.client.ev.text,
        ["enter", "preedit \"ab\" 0 1", "commit \"x\"", "done 1"]
    );
    let log = entries(&env);
    let reqs: Vec<&str> = log.iter().map(|e| e["req"].as_str().unwrap()).collect();
    assert_eq!(
        reqs,
        [
            "text_input.enable",
            "text_input.set_cursor_rectangle",
            "text_input.commit"
        ]
    );
    assert_eq!(log[2]["serial"], 1);
    assert_eq!(log[1]["rect"], json!([1, 2, 3, 4]));
}

#[test]
fn text_input_delete_surrounding() {
    let mut env = Env::new("sway", &[]);
    let (_surface, _xs, _top) = env.toplevel(200, 100);
    let seat: WlSeat = env.bind("wl_seat", 9);
    let manager: ZwpTextInputManagerV3 = env.bind("zwp_text_input_manager_v3", 1);
    let ti = manager.get_text_input(&seat, &env.qh, ());
    env.rec
        .control(json!({"cmd": "focus", "surface": "toplevel"}));
    ti.enable();
    ti.commit();
    env.roundtrip();
    let reply = env
        .rec
        .control(json!({"cmd": "text_input", "delete": [2, 1]}));
    assert_eq!(reply["ok"], true, "{reply}");
    env.roundtrip();
    assert_eq!(env.client.ev.text, ["enter", "delete 2 1", "done 1"]);
}

#[test]
fn cursor_shape_lands_in_the_input_log() {
    let mut env = Env::new("sway", &[]);
    let (_surface, _xs, _top) = env.toplevel(200, 100);
    let seat: WlSeat = env.bind("wl_seat", 9);
    let ptr: WlPointer = seat.get_pointer(&env.qh, ());
    let manager: WpCursorShapeManagerV1 = env.bind("wp_cursor_shape_manager_v1", 2);
    let device = manager.get_pointer(&ptr, &env.qh, ());
    env.roundtrip();
    env.rec
        .control(json!({"cmd": "focus", "surface": "toplevel"}));
    env.rec.control(json!({"cmd": "pointer", "pos": [5, 5]}));
    env.roundtrip();
    device.set_shape(env.client.ev.enter_serial, Shape::Text);
    env.roundtrip();
    let log = entries(&env);
    let shape = log
        .iter()
        .find(|e| e["req"] == "set_shape")
        .expect("set_shape");
    assert_eq!(shape["shape"], "text");
}

#[test]
fn toplevel_requests_are_logged_with_serials() {
    let mut env = Env::new("sway", &[]);
    let (surface, _xs, top) = env.toplevel(200, 100);
    let seat: WlSeat = env.bind("wl_seat", 9);
    top._move(&seat, 41);
    top.resize(&seat, 42, ResizeEdge::BottomRight);
    top.show_window_menu(&seat, 43, 7, 8);
    top.set_fullscreen(None);
    top.unset_fullscreen();
    top.set_maximized();
    top.unset_maximized();
    top.set_minimized();
    env.roundtrip();
    let log = entries(&env);
    let got: Vec<(String, serde_json::Value)> = log
        .iter()
        .map(|e| (e["req"].as_str().unwrap().to_string(), e["serial"].clone()))
        .collect();
    let want = [
        ("xdg_toplevel.move", json!(41)),
        ("xdg_toplevel.resize", json!(42)),
        ("xdg_toplevel.show_window_menu", json!(43)),
        ("xdg_toplevel.set_fullscreen", json!(null)),
        ("xdg_toplevel.unset_fullscreen", json!(null)),
        ("xdg_toplevel.set_maximized", json!(null)),
        ("xdg_toplevel.unset_maximized", json!(null)),
        ("xdg_toplevel.set_minimized", json!(null)),
    ];
    assert_eq!(got, want.map(|(r, s)| (r.to_string(), s)), "{log:?}");
    assert_eq!(log[1]["edges"], "bottom_right");
    assert_eq!(log[2]["location"], json!([7, 8]));
    assert_eq!(log[0]["surface"], surface.id().protocol_id());
}
