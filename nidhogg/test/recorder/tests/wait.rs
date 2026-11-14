//! T0 tests of `wait`: its reply comes once a popup maps or an input-log entry is there, and at its deadline it fails
//! naming what never came.
mod common;
use common::{send, Env};
use serde_json::json;
use wayland_client::Proxy;
use wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase;

#[test]
fn wait_for_a_popup_answers_with_its_id() {
    let mut env = Env::new("sway", &[]);
    let (_top, xs, _t) = env.toplevel(200, 200);
    let path = env.rec.control_path();
    let asker = std::thread::spawn(move || {
        send(
            &path,
            json!({"cmd": "wait", "for": "popup", "before": [], "timeout_ms": 5000}),
        )
    });
    let wm: XdgWmBase = env.bind("xdg_wm_base", 6);
    let positioner = wm.create_positioner(&env.qh, ());
    positioner.set_size(50, 40);
    positioner.set_anchor_rect(5, 6, 10, 10);
    let surface = env.surface();
    let pxs = wm.get_xdg_surface(&surface, &env.qh, ());
    let _popup = pxs.get_popup(Some(&xs), &positioner, &env.qh, ());
    surface.commit();
    env.roundtrip();
    let reply = asker.join().unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(
        reply["id"].as_u64(),
        Some(surface.id().protocol_id() as u64),
        "{reply}"
    );
    // a popup named in `before` is not the one waited for
    let reply = env.rec.control(
        json!({"cmd": "wait", "for": "popup", "before": [reply["id"]], "timeout_ms": 100}),
    );
    assert_eq!(reply["ok"], false, "{reply}");
}

#[test]
fn wait_fails_at_its_deadline_naming_what_never_came() {
    let env = Env::new("sway", &[]);
    let reply = env
        .rec
        .control(json!({"cmd": "wait", "for": "popup", "before": [], "timeout_ms": 100}));
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(
        reply["error"].as_str().unwrap().contains("no popup"),
        "{reply}"
    );
    let reply = env
        .rec
        .control(json!({"cmd": "wait", "for": "input", "req": "xdg_toplevel.move", "since": 1, "timeout_ms": 100}));
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("xdg_toplevel.move"),
        "{reply}"
    );
    let reply = env.rec.control(json!({"cmd": "wait", "for": "nothing"}));
    assert_eq!(reply["ok"], false, "{reply}");
}
