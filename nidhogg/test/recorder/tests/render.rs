//! T0 tests of snapshots: the surface tree composited by Smithay's pixman renderer.
mod common;
use common::Env;
use serde_json::json;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;

fn snapshot(env: &Env) -> (u32, u32, Vec<u32>) {
    let path = env.rec.dir.join("snap.bgra");
    let reply = env
        .rec
        .control(json!({"cmd": "snapshot", "path": path, "format": "bgra"}));
    assert_eq!(reply["ok"], true, "{reply}");
    let (w, h) = (
        reply["w"].as_u64().unwrap() as u32,
        reply["h"].as_u64().unwrap() as u32,
    );
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(bytes.len(), (w * h * 4) as usize);
    let px = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    (w, h, px)
}

#[test]
fn snapshot_bgra_matches_buffer() {
    let mut env = Env::new("sway", &[]);
    env.fill = 0xff33_6699;
    let (_surface, _xs, _top) = env.toplevel(64, 48);
    let (w, _h, px) = snapshot(&env);
    for y in 0..48 {
        for x in 0..64 {
            assert_eq!(px[(y * w + x) as usize], 0xff33_6699, "pixel {x},{y}");
        }
    }
    // outside the buffer the output is untouched
    assert_eq!(px[(100 * w + 100) as usize], 0);
}

#[test]
fn subsurface_composited_at_pos() {
    let mut env = Env::new("sway", &[]);
    env.fill = 0xff33_6699;
    let (parent, _xs, _top) = env.toplevel(100, 100);
    env.fill = 0xffcc_5500;
    let sub: WlSubcompositor = env.bind("wl_subcompositor", 1);
    let child = env.surface();
    let subsurface = sub.get_subsurface(&child, &parent, &env.qh, ());
    subsurface.set_position(10, 20);
    let buffer = env.buffer(10, 10);
    child.attach(Some(&buffer), 0, 0);
    child.commit();
    parent.commit();
    env.roundtrip();
    let (w, _h, px) = snapshot(&env);
    let at = |x: u32, y: u32| px[(y * w + x) as usize];
    assert_eq!(at(10, 20), 0xffcc_5500);
    assert_eq!(at(19, 29), 0xffcc_5500);
    assert_eq!(at(9, 20), 0xff33_6699);
    assert_eq!(at(20, 20), 0xff33_6699);
    assert_eq!(at(10, 30), 0xff33_6699);
}

/// The recorder's peak resident memory after snapshots of a 2880x1800 output stays small (the output is 20 MB of
/// pixels: one image, no copies of it).  NH_RECORDER_BIN=<release binary> with `--nocapture` prints the number.
#[test]
fn snapshot_peak_memory_stays_under_50_mb() {
    let mut env = Env::new("sway", &["--output", "2880x1800@120"]);
    let (_surface, _xs, _top) = env.toplevel(640, 480);
    let (w, h, _) = snapshot(&env);
    assert_eq!((w, h), (2880, 1800));
    let png = env.rec.dir.join("snap.png");
    let reply = env
        .rec
        .control(json!({"cmd": "snapshot", "path": png, "format": "png"}));
    assert_eq!(reply["ok"], true, "{reply}");
    let status = std::fs::read_to_string(format!("/proc/{}/status", env.rec.pid())).unwrap();
    let kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("VmHWM");
    eprintln!("nh-recorder VmHWM after two snapshots: {kb} kB");
    assert!(kb <= 50 * 1024, "VmHWM {kb} kB");
}
