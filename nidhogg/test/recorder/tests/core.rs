//! T0 tests of the recorder core: sockets, globals from a profile, the commit log, the control channel.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle};

struct Recorder {
    child: Child,
    dir: PathBuf,
    socket: String,
}

impl Recorder {
    fn start(profile: &Value) -> Recorder {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let socket = format!("nh-test-{}-{n}", std::process::id());
        let dir = std::env::temp_dir().join(&socket);
        std::fs::create_dir_all(&dir).unwrap();
        let profile_path = dir.join("profile.json");
        std::fs::write(&profile_path, profile.to_string()).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_nh-recorder"))
            .args(["--socket", &socket, "--control"])
            .arg(dir.join("control"))
            .arg("--profile")
            .arg(&profile_path)
            .env("XDG_RUNTIME_DIR", &dir)
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn nh-recorder");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line, "READY\n");
        Recorder { child, dir, socket }
    }

    fn control(&self, req: Value) -> Value {
        let mut s = UnixStream::connect(self.dir.join("control")).unwrap();
        writeln!(s, "{req}").unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn connect(&self) -> Connection {
        Connection::from_socket(UnixStream::connect(self.dir.join(&self.socket)).unwrap()).unwrap()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn six() -> Value {
    json!({"name": "t", "globals": {"wl_compositor": 5, "wl_shm": 1, "wl_subcompositor": 1, "wl_seat": 7,
                                    "wl_output": 4, "xdg_wm_base": 5}})
}

#[derive(Default)]
struct Client {
    globals: BTreeMap<String, u32>,
    names: BTreeMap<String, u32>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_registry::WlRegistry,
        e: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = e
        {
            s.names.insert(interface.clone(), name);
            s.globals.insert(interface, version);
        }
    }
}

macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for Client {
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection,
                     _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface
);

#[test]
fn core_ready_line_then_both_sockets() {
    let r = Recorder::start(&six());
    assert!(r.dir.join(&r.socket).exists());
    assert!(r.dir.join("control").exists());
}

#[test]
fn core_globals_follow_profile() {
    let r = Recorder::start(&six());
    let conn = r.connect();
    let mut queue = conn.new_event_queue::<Client>();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    let mut client = Client::default();
    queue.roundtrip(&mut client).unwrap();
    let want: BTreeMap<String, u32> = [
        ("wl_compositor", 5),
        ("wl_shm", 1),
        ("wl_subcompositor", 1),
        ("wl_seat", 7),
        ("wl_output", 4),
        ("xdg_wm_base", 5),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    assert_eq!(client.globals, want);
    let reply = r.control(json!({"cmd": "globals"}));
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["globals"], serde_json::to_value(&want).unwrap());
}

#[test]
fn core_profiles_serve_a_data_device_manager() {
    // GTK 3 adds no seat until a wl_data_device_manager is bound, and then dies on its first seat lookup
    for p in ["sway", "mutter", "kwin"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("profiles/{p}.json"));
        let profile: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let r = Recorder::start(&profile);
        let conn = r.connect();
        let mut queue = conn.new_event_queue::<Client>();
        let _registry = conn.display().get_registry(&queue.handle(), ());
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(
            client.globals.get("wl_data_device_manager"),
            Some(&3),
            "{p}"
        );
    }
}

#[test]
fn core_commit_is_logged() {
    let r = Recorder::start(&six());
    let conn = r.connect();
    let mut queue = conn.new_event_queue::<Client>();
    let qh = queue.handle();
    let registry = conn.display().get_registry(&qh, ());
    let mut client = Client::default();
    queue.roundtrip(&mut client).unwrap();
    let compositor: wl_compositor::WlCompositor =
        registry.bind(client.names["wl_compositor"], 5, &qh, ());
    let shm: wl_shm::WlShm = registry.bind(client.names["wl_shm"], 1, &qh, ());
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(r.dir.join("pool"))
        .unwrap();
    file.set_len(64 * 32 * 4).unwrap();
    let pool = shm.create_pool(file.as_fd(), 64 * 32 * 4, &qh, ());
    let buffer = pool.create_buffer(0, 64, 32, 256, wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage(0, 0, 64, 32);
    surface.commit();
    queue.roundtrip(&mut client).unwrap();
    let reply = r.control(json!({"cmd": "log", "since": 0}));
    assert_eq!(reply["ok"], true);
    let commits = reply["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 1, "{reply}");
    let c = &commits[0];
    assert_eq!(c["buffer"]["w"], 64);
    assert_eq!(c["buffer"]["h"], 32);
    assert_eq!(c["buffer"]["stride"], 256);
    assert_eq!(c["damage"], json!([[0, 0, 64, 32]]));
    assert_eq!(c["role"], "none");
    assert_eq!(c["seq"], 1);
    assert_eq!(reply["next"], 2);
    let later = r.control(json!({"cmd": "log", "since": 2}));
    assert_eq!(later["commits"], json!([]));
}

#[test]
fn core_quit_exits_zero() {
    let mut r = Recorder::start(&six());
    let reply = r.control(json!({"cmd": "quit"}));
    assert_eq!(reply["ok"], true);
    let status = r.child.wait().unwrap();
    assert_eq!(status.code(), Some(0));
    assert!(!r.dir.join(&r.socket).exists());
    assert!(!r.dir.join("control").exists());
}
