//! T0 tests of xdg-shell scripting: configures with states, stale acks, decoration mode per profile, ping.
mod common;
use common::{send, Env};
use serde_json::json;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::Mode;
use wayland_protocols::xdg::shell::client::xdg_toplevel::State;
use wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase;

#[test]
fn shell_configure_states_reach_the_client() {
    let mut env = Env::new("sway", &[]);
    let _top = env.toplevel(200, 100);
    env.client.configures.clear();
    let reply = env.rec.control(
        json!({"cmd": "configure", "surface": "focused", "size": [400, 300], "states": ["maximized", "activated"]}),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    env.roundtrip();
    let (w, h, mut states) = env.client.configures.last().cloned().expect("a configure");
    states.sort();
    assert_eq!((w, h), (400, 300));
    assert_eq!(
        states,
        vec![State::Maximized as u32, State::Activated as u32]
    );
}

#[test]
fn shell_close_reaches_the_client() {
    let mut env = Env::new("sway", &[]);
    let _top = env.toplevel(200, 100);
    let reply = env
        .rec
        .control(json!({"cmd": "close", "surface": "focused"}));
    assert_eq!(reply["ok"], true, "{reply}");
    env.roundtrip();
    assert_eq!(env.client.closes, 1);
    let reply = env.rec.control(json!({"cmd": "close", "surface": 9999}));
    assert_eq!(reply["ok"], false, "{reply}");
}

#[test]
fn shell_configure_stale_ack_is_logged_as_stale() {
    let mut env = Env::new("sway", &[]);
    let (surface, xs, _top) = env.toplevel(200, 100);
    let first = env
        .rec
        .control(json!({"cmd": "configure", "surface": "focused", "size": [300, 200]}));
    let second = env
        .rec
        .control(json!({"cmd": "configure", "surface": "focused", "size": [400, 300]}));
    let (s1, s2) = (
        first["serial"].as_u64().unwrap(),
        second["serial"].as_u64().unwrap(),
    );
    assert!(s1 < s2);
    env.roundtrip();
    xs.ack_configure(s1 as u32);
    surface.commit();
    env.roundtrip();
    let commits = env.rec.commits();
    let last = commits.last().unwrap();
    assert_eq!(last["ack_serial"], s1, "{last}");
    assert_eq!(last["ack_stale"], true, "{last}");
    // acking the newest is not stale
    xs.ack_configure(s2 as u32);
    surface.commit();
    env.roundtrip();
    let commits = env.rec.commits();
    assert_eq!(commits.last().unwrap()["ack_stale"], false);
}

#[test]
fn shell_commit_logs_window_geometry() {
    let mut env = Env::new("sway", &[]);
    let (surface, xs, _top) = env.toplevel(200, 100);
    assert_eq!(env.rec.commits().last().unwrap()["geometry"], json!(null));
    xs.set_window_geometry(10, 20, 180, 70);
    surface.commit();
    env.roundtrip();
    assert_eq!(
        env.rec.commits().last().unwrap()["geometry"],
        json!([10, 20, 180, 70])
    );
}

#[test]
fn shell_decoration_mode_per_profile() {
    // sway: server-side is sent whatever the client asks
    let mut env = Env::new("sway", &[]);
    let manager: ZxdgDecorationManagerV1 = env.bind("zxdg_decoration_manager_v1", 1);
    let wm: XdgWmBase = env.bind("xdg_wm_base", 5);
    let surface = env.surface();
    let xs = wm.get_xdg_surface(&surface, &env.qh, ());
    let top = xs.get_toplevel(&env.qh, ());
    let deco = manager.get_toplevel_decoration(&top, &env.qh, ());
    deco.set_mode(Mode::ClientSide);
    env.roundtrip();
    // nothing is sent before the first commit; the initial configure after it carries the mode
    assert!(env.client.deco_modes.is_empty());
    surface.commit();
    env.roundtrip();
    assert_eq!(
        env.client.deco_modes.last(),
        Some(&(Mode::ServerSide as u32))
    );
    assert_eq!(env.rec.commits().last().unwrap()["decoration"], "server");
    // the command switches what is sent next
    assert_eq!(
        env.rec
            .control(json!({"cmd": "decoration", "mode": "client"}))["ok"],
        true
    );
    deco.set_mode(Mode::ServerSide);
    env.roundtrip();
    assert_eq!(
        env.client.deco_modes.last(),
        Some(&(Mode::ClientSide as u32))
    );

    // mutter: no decoration global at all
    let env = Env::new("mutter", &[]);
    assert!(!env.client.names.contains_key("zxdg_decoration_manager_v1"));
}

#[test]
fn shell_ping_reports_pong() {
    let mut env = Env::new("sway", &[]);
    let _top = env.toplevel(200, 100);
    let path = env.rec.control_path();
    let asker = std::thread::spawn(move || send(&path, json!({"cmd": "ping"})));
    env.queue.blocking_dispatch(&mut env.client).unwrap(); // the ping; the handler answers with the pong
    env.roundtrip();
    let reply = asker.join().unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(reply["pong_ms"].is_f64(), "{reply}");
}

#[test]
fn shell_decoration_request_before_first_commit_sends_no_configure() {
    let mut env = Env::new("sway", &[]);
    let manager: ZxdgDecorationManagerV1 = env.bind("zxdg_decoration_manager_v1", 1);
    let wm: XdgWmBase = env.bind("xdg_wm_base", 5);
    let surface = env.surface();
    let xs = wm.get_xdg_surface(&surface, &env.qh, ());
    let top = xs.get_toplevel(&env.qh, ());
    let _deco = manager.get_toplevel_decoration(&top, &env.qh, ());
    env.roundtrip();
    assert!(env.client.configures.is_empty());
    assert_eq!(env.client.xdg_serial, 0);
    surface.commit();
    env.roundtrip();
    assert_eq!(env.client.configures.len(), 1);
    assert_eq!(env.client.deco_modes, vec![Mode::ServerSide as u32]);
    assert_ne!(env.client.xdg_serial, 0);
}

#[test]
fn shell_acks_are_stale_per_surface_not_per_protocol_id() {
    // two clients whose first surfaces have the same protocol id: a configure for the first must not make the
    // second's ack stale
    let mut a = Env::new("sway", &[]);
    let (_s, _xs, _top) = a.toplevel(200, 100);
    let a_surface = a.rec.commits().last().unwrap()["surface"].as_u64().unwrap();
    let mut b = a.join();
    let wm: XdgWmBase = b.bind("xdg_wm_base", 5);
    let bs = b.surface();
    let bxs = wm.get_xdg_surface(&bs, &b.qh, ());
    let _btop = bxs.get_toplevel(&b.qh, ());
    bs.commit();
    b.roundtrip();
    let b_serial = b.client.xdg_serial;
    let first = b.rec.commits().last().cloned().unwrap();
    assert_eq!(first["client"], 2, "{first}");
    assert_eq!(
        first["surface"].as_u64().unwrap(),
        a_surface,
        "ids differ: the test needs the same id"
    );
    // a newer configure goes to the first client's surface (the same protocol id) before the second acks its own
    let reply = a
        .rec
        .control(json!({"cmd": "configure", "surface": a_surface, "size": [300, 200]}));
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(reply["serial"].as_u64().unwrap() > u64::from(b_serial));
    bxs.ack_configure(b_serial);
    bs.commit();
    b.roundtrip();
    let last = b.rec.commits().last().cloned().unwrap();
    assert_eq!(last["client"], 2, "{last}");
    assert_eq!(last["ack_serial"], b_serial, "{last}");
    assert_eq!(last["ack_stale"], false, "{last}");
}

#[test]
fn shell_a_recreated_toplevel_starts_clean() {
    let mut env = Env::new("sway", &[]);
    let (surface, xs, top) = env.toplevel(200, 100);
    env.rec
        .control(json!({"cmd": "configure", "surface": "focused", "size": [300, 200]}));
    env.roundtrip();
    top.destroy();
    xs.destroy();
    surface.destroy();
    env.roundtrip();
    let _again = env.toplevel(200, 100);
    let last = env.rec.commits().last().cloned().unwrap();
    assert_eq!(last["ack_stale"], false, "{last}");
    assert!(last["ack_serial"].is_u64(), "{last}");
}

#[test]
fn shell_maximize_request_configure_counts_for_stale_acks() {
    let mut env = Env::new("sway", &[]);
    let wm: XdgWmBase = env.bind("xdg_wm_base", 5);
    let surface = env.surface();
    let xs = wm.get_xdg_surface(&surface, &env.qh, ());
    let top = xs.get_toplevel(&env.qh, ());
    surface.commit();
    env.roundtrip();
    let initial = env.client.xdg_serial;
    top.set_maximized();
    env.roundtrip();
    assert_ne!(
        env.client.xdg_serial, initial,
        "the maximize request got no configure"
    );
    // the client acks only the initial configure: older than the maximize configure the recorder sent
    xs.ack_configure(initial);
    surface.commit();
    env.roundtrip();
    let last = env.rec.commits().last().cloned().unwrap();
    assert_eq!(last["ack_serial"], initial, "{last}");
    assert_eq!(last["ack_stale"], true, "{last}");
}

#[test]
fn commit_logs_only_its_own_damage() {
    let mut env = Env::new("sway", &[]);
    let surface = env.surface();
    surface.damage(0, 0, 10, 10);
    surface.commit();
    env.roundtrip();
    let buffer = env.buffer(64, 32);
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(1, 2, 3, 4);
    surface.commit();
    env.roundtrip();
    let commits = env.rec.commits();
    assert_eq!(
        commits[0]["damage"],
        json!([{"kind": "surface", "rect": [0, 0, 10, 10]}])
    );
    assert_eq!(
        commits[1]["damage"],
        json!([{"kind": "buffer", "rect": [1, 2, 3, 4]}])
    );
}

#[test]
fn commit_logs_frame_cb_only_when_it_asked_for_one() {
    let mut env = Env::new("sway", &[]);
    let surface = env.surface();
    surface.frame(&env.qh, ());
    surface.commit();
    env.roundtrip();
    surface.commit();
    env.roundtrip();
    let commits = env.rec.commits();
    assert_eq!(commits[0]["frame_cb"], true);
    assert_eq!(commits[1]["frame_cb"], false);
}

#[test]
fn clients_lists_each_clients_pid() {
    let env = Env::new("sway", &[]);
    let reply = env.rec.control(json!({"cmd": "clients"}));
    assert_eq!(reply["clients"][0]["pid"], std::process::id(), "{reply}");
}
