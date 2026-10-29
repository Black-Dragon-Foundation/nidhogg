//! T0 tests of the recorder's timing additions: held-buffer writes, commit times, regions, the 120 Hz clock.
mod common;
use common::Env;
use serde_json::{json, Value};
use std::os::unix::fs::FileExt;
use std::time::{Duration, Instant};
use wayland_client::protocol::{wl_compositor::WlCompositor, wl_surface::WlSurface};
use wayland_client::Proxy;

/// Show a fresh 16 x 16 buffer on `s`; returns the file behind it.
fn show(env: &mut Env, s: &WlSurface) -> std::path::PathBuf {
    let b = env.buffer(16, 16);
    let path = env.pool_path();
    s.attach(Some(&b), 0, 0);
    s.damage_buffer(0, 0, 16, 16);
    s.commit();
    env.roundtrip();
    path
}

fn scribble(path: &std::path::Path) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.write_all_at(&[0xff; 64], 0).unwrap();
}

fn writes(env: &Env) -> Vec<Value> {
    let r = env
        .rec
        .control(json!({"cmd": "held_buffer_writes", "since": 0}));
    assert_eq!(r["ok"], true, "{r}");
    r["writes"].as_array().unwrap().clone()
}

#[test]
fn held_buffer_write_is_logged() {
    let mut env = Env::new("sway", &[]);
    let s = env.surface();
    let a = show(&mut env, &s);
    scribble(&a); // A is attached, not released
    show(&mut env, &s); // B replaces A: A's release finds the change
    let w = writes(&env);
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0]["attached_seq"], 1);
    assert_eq!(w[0]["released_seq"], 2);
    assert_eq!(w[0]["surface"], s.id().protocol_id());
}

#[test]
fn release_then_write_is_not_logged() {
    let mut env = Env::new("sway", &[]);
    let s = env.surface();
    let a = show(&mut env, &s);
    show(&mut env, &s); // A is released, unchanged
    scribble(&a); // the client owns A again
    show(&mut env, &s);
    assert!(writes(&env).is_empty());
}

#[test]
fn a_held_buffer_is_checked_when_it_leaves_the_queue() {
    let mut env = Env::new("kwin", &["--clock", "manual"]);
    let s = env.surface();
    let a = show(&mut env, &s);
    show(&mut env, &s); // A is replaced but held
    scribble(&a);
    assert!(writes(&env).is_empty());
    env.rec.control(json!({"cmd": "release"}));
    assert_eq!(writes(&env).len(), 1);
}

#[test]
fn commit_has_a_timestamp() {
    let mut env = Env::new("sway", &[]);
    let s = env.surface();
    show(&mut env, &s);
    std::thread::sleep(Duration::from_millis(5));
    show(&mut env, &s);
    let c = env.rec.commits();
    let (t0, t1) = (
        c[0]["t_ns"].as_u64().unwrap(),
        c[1]["t_ns"].as_u64().unwrap(),
    );
    assert!(t0 > 0 && t1 >= t0 + 5_000_000, "{t0} {t1}");
}

#[test]
fn opaque_and_input_regions_logged() {
    let mut env = Env::new("sway", &[]);
    let compositor: WlCompositor = env.bind("wl_compositor", 6);
    let (s, plain) = (env.surface(), env.surface());
    let opaque = compositor.create_region(&env.qh, ());
    opaque.add(0, 0, 10, 8);
    let input = compositor.create_region(&env.qh, ());
    s.set_opaque_region(Some(&opaque));
    s.set_input_region(Some(&input));
    show(&mut env, &s);
    show(&mut env, &plain);
    let c = env.rec.commits();
    assert_eq!(c[0]["opaque"], json!([[0, 0, 10, 8]]));
    assert_eq!(c[0]["input"], json!([]));
    assert_eq!(c[1]["opaque"], Value::Null);
    assert_eq!(c[1]["input"], Value::Null);
}

#[test]
fn refresh_120_paces_callbacks() {
    let mut env = Env::new("sway", &["--refresh-mhz", "120000"]);
    let s = env.surface();
    let b = env.buffer(16, 16);
    s.attach(Some(&b), 0, 0);
    let mut stamps = Vec::new();
    for n in 1..=10 {
        s.frame(&env.qh, ());
        s.damage_buffer(0, 0, 16, 16);
        s.commit();
        let end = Instant::now() + Duration::from_secs(2);
        while env.client.frames < n {
            env.roundtrip();
            assert!(Instant::now() < end, "callback {n} never came");
            std::thread::sleep(Duration::from_micros(300));
        }
        stamps.push(Instant::now());
    }
    let span = stamps[9] - stamps[0];
    assert!(
        span >= Duration::from_millis(75) && span <= Duration::from_millis(85),
        "{span:?}"
    );
}
