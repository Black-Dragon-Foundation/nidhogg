//! T0 tests of scale scripting: preferred scale events, scale changes between attach and commit, viewports.
mod common;
use common::Env;
use serde_json::json;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

#[test]
fn scale_preferred_scale_events() {
    let mut env = Env::new("sway", &[]);
    let manager: WpFractionalScaleManagerV1 = env.bind("wp_fractional_scale_manager_v1", 1);
    let _output: WlOutput = env.bind("wl_output", 4);
    let surface = env.surface();
    let _fractional = manager.get_fractional_scale(&surface, &env.qh, ());
    env.roundtrip();
    assert_eq!(env.client.fractional.last(), Some(&120));
    let reply = env.rec.control(json!({"cmd": "scale", "value120": 150}));
    assert_eq!(reply["ok"], true, "{reply}");
    env.roundtrip();
    assert_eq!(env.client.fractional.last(), Some(&150));
    assert_eq!(env.client.buffer_scales.last(), Some(&2));
    assert_eq!(env.client.output_scales.last(), Some(&2));
}

#[test]
fn scale_scale_change_between_attach_and_commit() {
    let mut env = Env::new("sway", &[]);
    let surface = env.surface();
    let buffer = env.buffer(600, 450);
    surface.attach(Some(&buffer), 0, 0);
    env.rec.control(json!({"cmd": "scale", "value120": 150}));
    surface.commit();
    env.roundtrip();
    let commits = env.rec.commits();
    assert_eq!(commits.last().unwrap()["scale120_at_commit"], 150);
}

#[test]
fn scale_viewport_is_logged() {
    let mut env = Env::new("sway", &[]);
    let viewporter: WpViewporter = env.bind("wp_viewporter", 1);
    let surface = env.surface();
    let viewport = viewporter.get_viewport(&surface, &env.qh, ());
    let buffer = env.buffer(600, 450);
    viewport.set_destination(400, 300);
    viewport.set_source(0.0, 0.0, 600.0, 450.0);
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    env.roundtrip();
    let commits = env.rec.commits();
    let v = &commits.last().unwrap()["viewport"];
    assert_eq!(v["dst"], json!([400, 300]));
    assert_eq!(v["src"], json!([0.0, 0.0, 600.0, 450.0]));
}
