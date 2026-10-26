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
    assert_eq!(
        env.client.deco_modes.last(),
        Some(&(Mode::ServerSide as u32))
    );
    surface.commit();
    env.roundtrip();
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
