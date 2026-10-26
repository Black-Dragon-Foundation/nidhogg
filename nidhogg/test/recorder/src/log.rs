//! The commit log: one `Commit` per surface commit, serialised as the JSON the control channel returns.
//! Later tasks add fields; they never rename these.
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct BufferInfo {
    pub w: i32,
    pub h: i32,
    pub stride: i32,
    pub format: String,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Viewport {
    pub src: Option<[f64; 4]>,
    pub dst: Option<[i32; 2]>,
}

#[derive(Serialize, Clone, Debug)]
pub struct DamageRect {
    pub kind: &'static str,
    pub rect: [i32; 4],
}

impl DamageRect {
    pub fn new(kind: &'static str, x: i32, y: i32, w: i32, h: i32) -> Self {
        DamageRect {
            kind,
            rect: [x, y, w, h],
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Commit {
    pub seq: u64,
    pub client: u32,
    pub surface: u32,
    pub role: String,
    pub parent: Option<u32>,
    pub buffer: Option<BufferInfo>,
    pub buffer_scale: i32,
    pub viewport: Viewport,
    pub pos: Option<[i32; 2]>,
    /// Each rectangle this commit damaged (not the accumulation of earlier commits), by `surface` or `buffer`
    /// coordinates.
    pub damage: Vec<DamageRect>,
    pub frame_cb: bool,
    pub presentation: bool,
    /// The newest configure serial the client acked since its previous commit.
    pub ack_serial: Option<u32>,
    /// The acked serial is older than the newest configure the recorder sent for the surface.
    pub ack_stale: bool,
    /// The scale (value120) the recorder had announced when it processed the commit.
    pub scale120_at_commit: u32,
    /// The decoration mode the recorder sent for a toplevel: `server`, `client` or null.
    pub decoration: Option<String>,
    pub title: Option<String>,
    pub app_id: Option<String>,
    /// A toplevel's or popup's window geometry (`xdg_surface.set_window_geometry`, surface-local logical
    /// `[x, y, w, h]`): where the window sits inside its buffer (GTK's client-side shadow lies outside it); null when
    /// the client never set one.
    pub geometry: Option<[i32; 4]>,
    pub t_ns: u64,
}

#[derive(Default)]
pub struct Log {
    commits: Vec<Commit>,
    next: u64,
}

impl Log {
    pub fn new() -> Self {
        Log {
            commits: Vec::new(),
            next: 1,
        }
    }

    /// The seq the next commit gets.
    pub fn next_seq(&self) -> u64 {
        self.next
    }

    pub fn push(&mut self, c: Commit) {
        self.next = c.seq + 1;
        self.commits.push(c);
    }

    pub fn since(&self, seq: u64) -> impl Iterator<Item = &Commit> {
        self.commits.iter().filter(move |c| c.seq >= seq)
    }
}
