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
    pub damage: Vec<[i32; 4]>,
    pub frame_cb: bool,
    pub presentation: bool,
    pub ack_serial: Option<u32>,
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
