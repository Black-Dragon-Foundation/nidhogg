//! Shared harness for the scripting tests: start nh-recorder with a profile from `profiles/`, a recording
//! Wayland client, and helpers for the usual surfaces.
#![allow(dead_code)]
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

pub struct Recorder {
    child: Child,
    pub dir: PathBuf,
    socket: String,
}

impl Recorder {
    /// Start with `profiles/<profile>.json` and extra command-line arguments (e.g. `--clock manual`).
    pub fn start(profile: &str, extra: &[&str]) -> Recorder {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let socket = format!("nh-test-{}-{n}", std::process::id());
        let dir = std::env::temp_dir().join(&socket);
        std::fs::create_dir_all(&dir).unwrap();
        let profile_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("profiles")
            .join(format!("{profile}.json"));
        let mut child = Command::new(env!("CARGO_BIN_EXE_nh-recorder"))
            .args(["--socket", &socket, "--control"])
            .arg(dir.join("control"))
            .arg("--profile")
            .arg(&profile_path)
            .args(extra)
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

    pub fn control_path(&self) -> PathBuf {
        self.dir.join("control")
    }

    pub fn control(&self, req: Value) -> Value {
        send(&self.control_path(), req)
    }

    /// The commits logged so far.
    pub fn commits(&self) -> Vec<Value> {
        self.control(json!({"cmd": "log", "since": 0}))["commits"]
            .as_array()
            .unwrap()
            .clone()
    }

    pub fn connect(&self) -> Connection {
        Connection::from_socket(UnixStream::connect(self.dir.join(&self.socket)).unwrap()).unwrap()
    }
}

pub fn send(path: &Path, req: Value) -> Value {
    let mut s = UnixStream::connect(path).unwrap();
    writeln!(s, "{req}").unwrap();
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Everything the test client hears.
#[derive(Default)]
pub struct Client {
    pub names: std::collections::BTreeMap<String, u32>,
    pub versions: std::collections::BTreeMap<String, u32>,
    pub xdg_serial: u32,
    /// (width, height, state numbers) of every xdg_toplevel.configure
    pub configures: Vec<(i32, i32, Vec<u32>)>,
    pub deco_modes: Vec<u32>,
    pub fractional: Vec<u32>,
    pub buffer_scales: Vec<i32>,
    pub output_scales: Vec<i32>,
    pub frames: u32,
    pub presented: Vec<Duration>,
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
            s.versions.insert(interface, version);
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_surface::WlSurface,
        e: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::PreferredBufferScale { factor } = e {
            s.buffer_scales.push(factor);
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_output::WlOutput,
        e: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Scale { factor } = e {
            s.output_scales.push(factor);
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
    fn event(
        _: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        e: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = e {
            base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(
        s: &mut Self,
        _: &xdg_surface::XdgSurface,
        e: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = e {
            s.xdg_serial = serial;
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Client {
    fn event(
        s: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        e: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure {
            width,
            height,
            states,
        } = e
        {
            let states = states
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_ne_bytes(*c))
                .collect();
            s.configures.push((width, height, states));
        }
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ()> for Client {
    fn event(
        s: &mut Self,
        _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        e: zxdg_toplevel_decoration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = e {
            s.deco_modes.push(match mode {
                wayland_client::WEnum::Value(m) => m as u32,
                wayland_client::WEnum::Unknown(u) => u,
            });
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        e: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = e {
            s.fractional.push(scale);
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_callback::WlCallback,
        e: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = e {
            s.frames += 1;
        }
    }
}

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        e: wp_presentation_feedback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation_feedback::Event::Presented {
            tv_sec_hi,
            tv_sec_lo,
            tv_nsec,
            ..
        } = e
        {
            let secs = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
            s.presented.push(Duration::new(secs, tv_nsec));
        }
    }
}

macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for Client {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection,
                     _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_viewporter::WpViewporter,
    wp_viewport::WpViewport,
    wp_presentation::WpPresentation
);

/// A connected client with the registry listed.
pub struct Env {
    pub rec: Recorder,
    pub queue: EventQueue<Client>,
    pub qh: QueueHandle<Client>,
    pub client: Client,
    pub registry: wl_registry::WlRegistry,
    _conn: Connection,
    pools: u32,
}

impl Env {
    pub fn new(profile: &str, extra: &[&str]) -> Env {
        let rec = Recorder::start(profile, extra);
        let conn = rec.connect();
        let queue = conn.new_event_queue::<Client>();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut env = Env {
            rec,
            queue,
            qh,
            client: Client::default(),
            registry,
            _conn: conn,
            pools: 0,
        };
        env.roundtrip();
        env
    }

    pub fn roundtrip(&mut self) {
        self.queue.roundtrip(&mut self.client).unwrap();
    }

    /// Bind a global at `min(offered, max)`.
    pub fn bind<I: Proxy + 'static>(&mut self, name: &str, max: u32) -> I
    where
        Client: Dispatch<I, ()>,
    {
        let offered = self.client.versions[name];
        self.registry
            .bind(self.client.names[name], offered.min(max), &self.qh, ())
    }

    /// A shm buffer of `w` x `h` ARGB pixels.
    pub fn buffer(&mut self, w: i32, h: i32) -> wl_buffer::WlBuffer {
        let shm: wl_shm::WlShm = self.bind("wl_shm", 1);
        self.pools += 1;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(self.rec.dir.join(format!("pool{}", self.pools)))
            .unwrap();
        let size = (w * h * 4) as u64;
        file.set_len(size).unwrap();
        let pool = shm.create_pool(file.as_fd(), size as i32, &self.qh, ());
        pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, &self.qh, ())
    }

    pub fn surface(&mut self) -> wl_surface::WlSurface {
        let compositor: wl_compositor::WlCompositor = self.bind("wl_compositor", 6);
        compositor.create_surface(&self.qh, ())
    }

    /// A mapped xdg_toplevel of `w` x `h`: initial commit, configure, ack, buffer, commit.
    pub fn toplevel(
        &mut self,
        w: i32,
        h: i32,
    ) -> (
        wl_surface::WlSurface,
        xdg_surface::XdgSurface,
        xdg_toplevel::XdgToplevel,
    ) {
        let wm: xdg_wm_base::XdgWmBase = self.bind("xdg_wm_base", 6);
        let surface = self.surface();
        let xs = wm.get_xdg_surface(&surface, &self.qh, ());
        let top = xs.get_toplevel(&self.qh, ());
        surface.commit();
        self.roundtrip();
        xs.ack_configure(self.client.xdg_serial);
        let buffer = self.buffer(w, h);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        self.roundtrip();
        (surface, xs, top)
    }
}
