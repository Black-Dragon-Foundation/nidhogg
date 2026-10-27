//! T0 tests of the pacing modes: damage-only frames, discard on supersede, occlusion, held buffers, outstanding.
mod common;
use common::Env;
use serde_json::json;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_protocols::wp::presentation_time::client::wp_presentation::WpPresentation;

fn manual(profile: &str) -> (Env, WpPresentation, WlSurface) {
    let mut env = Env::new(profile, &["--clock", "manual"]);
    let presentation: WpPresentation = env.bind("wp_presentation", 2);
    let surface = env.surface();
    (env, presentation, surface)
}

fn present(env: &mut Env) {
    env.rec
        .control(json!({"cmd": "present", "advance_ns": 16_000_000}));
    env.roundtrip();
}

/// One commit with a frame callback and feedback, with or without damage and a new buffer.
fn commit(env: &mut Env, p: &WpPresentation, s: &WlSurface, damage: bool) {
    if damage {
        let b = env.buffer(8, 8);
        s.attach(Some(&b), 0, 0);
        s.damage_buffer(0, 0, 8, 8);
    }
    s.frame(&env.qh, ());
    p.feedback(s, &env.qh, ());
    s.commit();
    env.roundtrip();
}

fn outstanding(env: &Env) -> serde_json::Value {
    env.rec.control(json!({"cmd": "outstanding"}))
}

#[test]
fn damage_only_answers_callbacks_but_not_feedback_of_an_empty_commit() {
    let (mut env, p, s) = manual("sway");
    commit(&mut env, &p, &s, false);
    assert_eq!(outstanding(&env)["callbacks"], 1);
    present(&mut env);
    assert_eq!(env.client.frames, 1);
    assert!(env.client.presented.is_empty());
    let o = outstanding(&env);
    assert_eq!(
        (o["callbacks"].as_u64(), o["feedback"].as_u64()),
        (Some(0), Some(1)),
        "{o}"
    );
    // a commit with damage is answered
    commit(&mut env, &p, &s, true);
    present(&mut env);
    assert_eq!(env.client.presented.len(), 1);
}

#[test]
fn all_frames_answers_the_feedback_of_an_empty_commit() {
    let (mut env, p, s) = manual("sway");
    env.rec.control(json!({"cmd": "mode", "frames": "all"}));
    commit(&mut env, &p, &s, false);
    present(&mut env);
    assert_eq!(env.client.presented.len(), 1);
}

#[test]
fn a_later_commit_discards_waiting_feedback() {
    let (mut env, p, s) = manual("sway");
    commit(&mut env, &p, &s, false);
    present(&mut env);
    assert_eq!(env.client.discarded, 0);
    commit(&mut env, &p, &s, true);
    assert_eq!(env.client.discarded, 1);
    assert_eq!(outstanding(&env)["feedback"], 1);
    // without the mode the old feedback just waits
    env.rec.control(json!({"cmd": "mode", "discard": false}));
    commit(&mut env, &p, &s, false);
    commit(&mut env, &p, &s, true);
    assert_eq!(env.client.discarded, 1);
}

#[test]
fn occlude_holds_callbacks_and_feedback_until_unocclude() {
    let (mut env, p, s) = manual("mutter");
    commit(&mut env, &p, &s, true);
    assert_eq!(env.rec.control(json!({"cmd": "occlude"}))["ok"], true);
    present(&mut env);
    assert_eq!((env.client.frames, env.client.presented.len()), (0, 0));
    let o = outstanding(&env);
    assert_eq!(
        (
            o["callbacks"].as_u64(),
            o["feedback"].as_u64(),
            o["occluded"].as_bool()
        ),
        (Some(1), Some(1), Some(true))
    );
    env.rec.control(json!({"cmd": "unocclude"}));
    present(&mut env);
    assert_eq!((env.client.frames, env.client.presented.len()), (1, 1));
    let o = outstanding(&env);
    assert_eq!(
        (o["callbacks"].as_u64(), o["feedback"].as_u64()),
        (Some(0), Some(0))
    );
}

#[test]
fn occlude_needs_a_profile_that_has_it() {
    let (env, _p, _s) = manual("sway");
    assert_eq!(env.rec.control(json!({"cmd": "occlude"}))["ok"], false);
}

#[test]
fn held_buffers_stay_unreleased_until_release() {
    let (mut env, p, s) = manual("kwin");
    commit(&mut env, &p, &s, true); // A shown
    commit(&mut env, &p, &s, true); // B shown, A held
    assert_eq!(env.client.released, 0);
    assert_eq!(outstanding(&env)["buffers"], 1);
    commit(&mut env, &p, &s, true); // C shown, B held, A pushed out and released
    assert_eq!(env.client.released, 1);
    assert_eq!(outstanding(&env)["buffers"], 1);
    let r = env.rec.control(json!({"cmd": "release"}));
    assert_eq!(r["released"], 1, "{r}");
    env.roundtrip();
    assert_eq!(env.client.released, 2);
    assert_eq!(outstanding(&env)["buffers"], 0);
}

#[test]
fn profiles_without_hold_release_replaced_buffers_at_once() {
    let (mut env, p, s) = manual("sway");
    commit(&mut env, &p, &s, true);
    commit(&mut env, &p, &s, true);
    assert_eq!(env.client.released, 1);
    assert_eq!(outstanding(&env)["buffers"], 0);
}

#[test]
fn hold_buffers_mode_command_sets_and_lowers_the_hold() {
    let (mut env, p, s) = manual("sway");
    env.rec.control(json!({"cmd": "mode", "hold_buffers": 2}));
    for _ in 0..3 {
        commit(&mut env, &p, &s, true);
    }
    assert_eq!(
        (env.client.released, outstanding(&env)["buffers"].as_u64()),
        (0, Some(2))
    );
    env.rec.control(json!({"cmd": "mode", "hold_buffers": 0}));
    env.roundtrip();
    assert_eq!(env.client.released, 2);
}
