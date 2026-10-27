//! T0 tests of the frame clock: frame callbacks and presentation feedback wait for `present` in manual mode.
mod common;
use common::Env;
use serde_json::json;
use std::time::Duration;
use wayland_protocols::wp::presentation_time::client::wp_presentation::WpPresentation;

#[test]
fn clock_manual_present_fires_callbacks() {
    let mut env = Env::new("sway", &["--clock", "manual"]);
    let presentation: WpPresentation = env.bind("wp_presentation", 2);
    let surface = env.surface();
    let buffer = env.buffer(64, 64);
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 64, 64);
    surface.frame(&env.qh, ());
    presentation.feedback(&surface, &env.qh, ());
    surface.commit();
    env.roundtrip();
    assert_eq!(env.client.frames, 0);
    assert!(env.client.presented.is_empty());
    assert_eq!(env.rec.commits().last().unwrap()["presentation"], true);

    let reply = env
        .rec
        .control(json!({"cmd": "present", "advance_ns": 5_000_000}));
    assert_eq!(reply["ok"], true, "{reply}");
    env.roundtrip();
    assert_eq!(env.client.frames, 1);
    assert_eq!(env.client.presented.len(), 1);

    // the second present carries the clock 16 ms later
    surface.damage_buffer(0, 0, 64, 64);
    surface.frame(&env.qh, ());
    presentation.feedback(&surface, &env.qh, ());
    surface.commit();
    env.roundtrip();
    assert_eq!(env.client.frames, 1);
    env.rec
        .control(json!({"cmd": "present", "advance_ns": 16_000_000}));
    env.roundtrip();
    assert_eq!(env.client.frames, 2);
    let (a, b) = (env.client.presented[0], env.client.presented[1]);
    assert_eq!(b - a, Duration::from_millis(16));
}
