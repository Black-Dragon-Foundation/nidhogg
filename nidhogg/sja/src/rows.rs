//! A view's rows, held where they are drawn: the hub's row patches (`view-patch`) for one (view, class) are decided
//! and applied here, and the draw asks for the edits that turn what the buffer holds into what the rows say.  Lisp
//! parses no patch and dresses no row: it inserts the edits' text and puts the runs, properties and display specs
//! the answer names (its final draw call), and keeps the windows' places.
//!
//! The rules are the editor's (they were `drgnx-view-patch--decide`/`--accept` in Lisp):
//! - a patch that is not a reset, from a hub other than the last reset's (or before any reset): dropped;
//! - a reset not newer than the version drawn, from the same hub: dropped;
//! - a patch built for an older env epoch than the editor sent last: held (dimmed) until a reset;
//! - a diff of an older base: dropped; of a newer base (a gap): resync;
//! - a diff that names an image or a style the store has not got and does not bring: resync;
//! - else applied: a reset takes its hub, drops the held patches and answers the outstanding resync.
//!
//! A resync is asked for at most once while outstanding.  Three resyncs (or caps) within 10 s mark the store broken;
//! between resyncs there is a back-off of 1 s, then 4 s: a resync asked for sooner is counted but not sent.
//!
//! **The draw.**  The drawn state is the list of rows the buffer holds, in order, each with its length in characters
//! (its text plus the newline) and its content hash.  `draw` compares it with the rows and answers edits in buffer
//! order, each `(OFFSET DELETE TEXT RUNS PROPS DISPLAY)`, offsets in characters from the region's start as the buffer
//! stands when that edit is applied.  The new drawn state is kept as pending until Lisp confirms the batch (`drawn`);
//! a batch never confirmed (a quit in the middle, a panic) is rolled back: the next draw erases the region and draws
//! it whole.  So does a region whose length is not the drawn state's (the length check).

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;

/// The most rows a store holds.
pub const MAX_ROWS: usize = 200_000;
/// The longest row text kept, in bytes; longer ones are cut at a character boundary.
pub const MAX_TEXT: usize = 64 << 10;
/// The most runs (faces) a row keeps.
pub const MAX_RUNS: usize = 4096;
/// The most images a store keeps; past it the ones no row names go.
pub const MAX_IMAGES: usize = 256;
/// The most held patches.
pub const MAX_HELD: usize = 64;
/// The breaker: this many resyncs within `BREAK_MS` mark the store broken.
pub const BREAK_COUNT: usize = 3;
pub const BREAK_MS: u64 = 10_000;
/// The back-off between resyncs, in milliseconds: after the first, then after each later one.
pub const BACKOFF_MS: [u64; 2] = [1_000, 4_000];

/// What a push decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Apply = 0,
    Drop = 1,
    Hold = 2,
    /// Ask the hub for a reset (Lisp sends `view/resync`).
    Resync = 3,
    /// A resync is needed but one is outstanding, or the back-off holds it: nothing is sent.
    Quiet = 4,
    /// The store is broken: Lisp says the view stopped.
    Broken = 5,
    /// The view is paused: the patch is kept.
    Paused = 6,
}

/// A run's face: a face name, or a terminal style by id (`s:ID` on the wire).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    Named(Box<str>),
    Style(u64),
}

/// One row as the store holds it.  Positions are character indices into `text`.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: Box<str>,
    pub text: Box<str>,
    pub runs: Vec<(u32, u32, Face)>,
    /// `(start, end, name, value)`: a string value as a string, an integer as an integer, anything else as its JSON.
    pub props: Vec<(u32, u32, Box<str>, PropVal)>,
    pub stops: Vec<(u32, u32)>,
    pub images: Vec<(u32, Box<str>, Box<str>)>,
    pub spaces: Vec<(u32, u32, u32)>,
    pub height: Option<(u32, u32)>,
    /// The row's length in characters, newline included, and its content hash: what the drawn state compares.
    pub chars: usize,
    pub hash: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PropVal {
    Str(Box<str>),
    Int(i64),
    Json(Box<str>),
}

/// One op of a patch.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Reset(Vec<Row>),
    Insert {
        after: Option<Box<str>>,
        rows: Vec<Row>,
    },
    Replace(Vec<Row>),
    Remove(Vec<Box<str>>),
}

/// A row patch, decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    pub view: String,
    pub class: String,
    pub hub: u64,
    pub base: u64,
    pub version: u64,
    pub epoch: u64,
    pub ops: Vec<Op>,
    pub images: Vec<(Box<str>, Box<str>)>,
    /// `(id, style)`: the style kept as its JSON (Lisp turns it into a face once per id).
    pub styles: Vec<(u64, Box<str>)>,
    /// A terminal's metadata (cursor, screen, title), as its JSON.
    pub term: Option<Box<str>>,
    pub model: Option<u64>,
}

impl Patch {
    fn reset(&self) -> bool {
        matches!(self.ops.as_slice(), [Op::Reset(_)])
    }

    fn rows(&self) -> impl Iterator<Item = &Row> {
        self.ops.iter().flat_map(|op| match op {
            Op::Reset(r) | Op::Replace(r) | Op::Insert { rows: r, .. } => r.as_slice(),
            Op::Remove(_) => &[],
        })
    }
}

/// Why a patch did not decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bad(pub String);

fn bad<T>(what: &str) -> Result<T, Bad> {
    Err(Bad(what.into()))
}

fn u(v: Option<&Value>, what: &str) -> Result<u64, Bad> {
    v.and_then(Value::as_u64)
        .ok_or_else(|| Bad(format!("{what}: not a count")))
}

fn u32_at(a: &[Value], i: usize) -> Result<u32, Bad> {
    a.get(i)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| Bad("a position is not a count".into()))
}

fn str_at(a: &[Value], i: usize) -> Result<Box<str>, Bad> {
    a.get(i)
        .and_then(Value::as_str)
        .map(Into::into)
        .ok_or_else(|| Bad("not a string".into()))
}

fn tuples(v: Option<&Value>) -> Result<&[Value], Bad> {
    match v {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(a)) => Ok(a.as_slice()),
        _ => bad("not a list"),
    }
}

fn tuple(v: &Value) -> Result<&[Value], Bad> {
    v.as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| Bad("not a tuple".into()))
}

/// The longest prefix of `s` within `max` bytes, at a character boundary.
fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// FNV-1a over the row's parts, so a row that changes in anything drawn redraws.
fn hash_row(r: &Row) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: &[u8]| {
        for x in b {
            h ^= u64::from(*x);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    eat(r.text.as_bytes());
    for (a, b, f) in &r.runs {
        eat(&a.to_le_bytes());
        eat(&b.to_le_bytes());
        match f {
            Face::Named(n) => eat(n.as_bytes()),
            Face::Style(id) => eat(&id.to_le_bytes()),
        }
    }
    for (a, b, n, v) in &r.props {
        eat(&a.to_le_bytes());
        eat(&b.to_le_bytes());
        eat(n.as_bytes());
        match v {
            PropVal::Str(s) | PropVal::Json(s) => eat(s.as_bytes()),
            PropVal::Int(i) => eat(&i.to_le_bytes()),
        }
    }
    for (a, b) in &r.stops {
        eat(&a.to_le_bytes());
        eat(&b.to_le_bytes());
    }
    for (a, id, alt) in &r.images {
        eat(&a.to_le_bytes());
        eat(id.as_bytes());
        eat(alt.as_bytes());
    }
    for (a, w, ht) in &r.spaces {
        eat(&a.to_le_bytes());
        eat(&w.to_le_bytes());
        eat(&ht.to_le_bytes());
    }
    if let Some((l, un)) = r.height {
        eat(&l.to_le_bytes());
        eat(&un.to_le_bytes());
    }
    h
}

/// Decode one row.
pub fn row(v: &Value) -> Result<Row, Bad> {
    let o = v
        .as_object()
        .ok_or_else(|| Bad("a row is not an object".into()))?;
    let key: Box<str> = o
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| Bad("a row has no key".into()))?
        .into();
    let text: Box<str> = cut(
        o.get("text").and_then(Value::as_str).unwrap_or(""),
        MAX_TEXT,
    )
    .into();
    let n = text.chars().count() as u32;
    let clamp = |x: u32| x.min(n);
    let mut runs = Vec::new();
    for t in tuples(o.get("runs"))?.iter().take(MAX_RUNS) {
        let t = tuple(t)?;
        let name = str_at(t, 2)?;
        let face = match name
            .strip_prefix("s:")
            .and_then(|id| id.parse::<u64>().ok())
        {
            Some(id) => Face::Style(id),
            None => Face::Named(name),
        };
        runs.push((clamp(u32_at(t, 0)?), clamp(u32_at(t, 1)?), face));
    }
    let mut props = Vec::new();
    for t in tuples(o.get("props"))? {
        let t = tuple(t)?;
        let val = match t.get(3) {
            Some(Value::String(s)) => PropVal::Str(s.as_str().into()),
            Some(Value::Number(x)) if x.as_i64().is_some() => PropVal::Int(x.as_i64().unwrap_or(0)),
            Some(other) => PropVal::Json(other.to_string().into()),
            None => return bad("a prop has no value"),
        };
        props.push((
            clamp(u32_at(t, 0)?),
            clamp(u32_at(t, 1)?),
            str_at(t, 2)?,
            val,
        ));
    }
    let mut stops = Vec::new();
    for t in tuples(o.get("stops"))? {
        let t = tuple(t)?;
        stops.push((clamp(u32_at(t, 0)?), u32_at(t, 1)?));
    }
    let mut images = Vec::new();
    for t in tuples(o.get("images"))? {
        let t = tuple(t)?;
        images.push((clamp(u32_at(t, 0)?), str_at(t, 1)?, str_at(t, 2)?));
    }
    let mut spaces = Vec::new();
    for t in tuples(o.get("spaces"))? {
        let t = tuple(t)?;
        spaces.push((clamp(u32_at(t, 0)?), u32_at(t, 1)?, u32_at(t, 2)?));
    }
    let height = match o.get("height") {
        Some(Value::Object(h)) => Some((
            u(h.get("lines"), "height")? as u32,
            u(h.get("unit"), "height")? as u32,
        )),
        _ => None,
    };
    let mut r = Row {
        key,
        text,
        runs,
        props,
        stops,
        images,
        spaces,
        height,
        chars: n as usize + 1,
        hash: 0,
    };
    r.hash = hash_row(&r);
    Ok(r)
}

fn rows_of(v: &Value) -> Result<Vec<Row>, Bad> {
    v.as_array()
        .ok_or_else(|| Bad("rows are not a list".into()))?
        .iter()
        .map(row)
        .collect()
}

fn op(v: &Value) -> Result<Op, Bad> {
    let o = v
        .as_object()
        .ok_or_else(|| Bad("an op is not an object".into()))?;
    if let Some(r) = o.get("reset") {
        return Ok(Op::Reset(rows_of(r)?));
    }
    if let Some(r) = o.get("replace") {
        return Ok(Op::Replace(rows_of(r)?));
    }
    if let Some(i) = o.get("insert") {
        let after = i.get("after").and_then(Value::as_str).map(Into::into);
        return Ok(Op::Insert {
            after,
            rows: rows_of(i.get("rows").unwrap_or(&Value::Null))?,
        });
    }
    if let Some(r) = o.get("remove") {
        let keys = r
            .as_array()
            .ok_or_else(|| Bad("remove is not a list".into()))?
            .iter()
            .map(|k| {
                k.as_str()
                    .map(Into::into)
                    .ok_or_else(|| Bad("a key is not a string".into()))
            })
            .collect::<Result<_, _>>()?;
        return Ok(Op::Remove(keys));
    }
    bad("an unknown op")
}

/// Decode a patch (a `view-patch` event's data).
pub fn patch(v: &Value) -> Result<Patch, Bad> {
    let o = v
        .as_object()
        .ok_or_else(|| Bad("a patch is not an object".into()))?;
    let ops = tuples(o.get("ops"))?
        .iter()
        .map(op)
        .collect::<Result<_, _>>()?;
    let mut images = Vec::new();
    for t in tuples(o.get("images"))? {
        let t = tuple(t)?;
        images.push((str_at(t, 0)?, str_at(t, 1)?));
    }
    let mut styles = Vec::new();
    for t in tuples(o.get("styles"))? {
        let t = tuple(t)?;
        let id = t
            .first()
            .and_then(Value::as_u64)
            .ok_or_else(|| Bad("a style id".into()))?;
        styles.push((
            id,
            t.get(1).map(Value::to_string).unwrap_or_default().into(),
        ));
    }
    Ok(Patch {
        view: o
            .get("view")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        class: o
            .get("class")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        hub: u(o.get("hub"), "hub")?,
        base: u(o.get("base"), "base")?,
        version: u(o.get("version"), "version")?,
        epoch: o.get("env_epoch").and_then(Value::as_u64).unwrap_or(0),
        ops,
        images,
        styles,
        term: o
            .get("term")
            .filter(|t| !t.is_null())
            .map(|t| t.to_string().into()),
        model: o.get("model").and_then(Value::as_u64),
    })
}

/// The styles (id, JSON) and images (id, SVG) Lisp has not been given.
pub type Fresh = (Vec<(u64, Box<str>)>, Vec<(Box<str>, Box<str>)>);

/// Read a rows line: `{"event": "view-model", "data": {"view": V, "rows": PATCH}}` (the hub's `view-patch` data, sent
/// on the model's raw path so Lisp never parses it).  `None` when the line is not one.
pub fn read_line(line: &str) -> Option<(String, Result<Patch, Bad>)> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("event")?.as_str()? != "view-model" {
        return None;
    }
    let d = v.get("data")?;
    let rows = d.get("rows")?;
    let view = d.get("view")?.as_str()?.to_string();
    Some((view, patch(rows)))
}

/// One edit of a draw: at `offset` (characters from the region's start, the buffer as it stands then), `delete`
/// characters go and `rows` are inserted, their texts joined (each with its newline).
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub offset: usize,
    pub delete: usize,
    pub rows: Vec<Box<str>>,
}

/// A drawn row: its key, its length in characters and its hash.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Drawn {
    key: Box<str>,
    chars: usize,
    hash: u64,
}

/// One (view, class)'s rows and books.
#[derive(Debug, Default)]
pub struct Store {
    pub hub: Option<u64>,
    pub version: Option<u64>,
    /// The env epoch the editor sent last, and whether an older one is held.
    pub epoch: u64,
    pub epoch_reset: bool,
    pub held: Vec<Patch>,
    pub resync: bool,
    pub resyncs: u64,
    resync_times: VecDeque<u64>,
    last_resync: Option<u64>,
    pub broken: bool,
    pub paused: Option<(usize, Vec<Patch>)>,
    order: Vec<Box<str>>,
    rows: HashMap<Box<str>, Row>,
    pub styles: HashMap<u64, Box<str>>,
    /// Image id → its SVG: the images the rows name, sent to Lisp once each (`fresh`).
    pub images: HashMap<Box<str>, Box<str>>,
    /// Style and image ids Lisp has not been given yet.
    fresh_styles: Vec<u64>,
    fresh_images: Vec<Box<str>>,
    pub term: Option<Box<str>>,
    pub model: Option<u64>,
    drawn: Vec<Drawn>,
    pending: Option<(u64, Vec<Drawn>)>,
    batch: u64,
    /// Set by `redraw`: the next draw redraws every row (properties resolved again).
    redraw_all: bool,
}

impl Store {
    pub fn new(epoch_reset: bool) -> Self {
        Store {
            epoch_reset,
            ..Store::default()
        }
    }

    /// The rows in order.
    pub fn keys(&self) -> &[Box<str>] {
        &self.order
    }

    pub fn row(&self, key: &str) -> Option<&Row> {
        self.rows.get(key)
    }

    fn decide(&self, p: &Patch) -> Decision {
        let reset = p.reset();
        let same = self.hub == Some(p.hub);
        if !reset && !same {
            return Decision::Drop;
        }
        if reset && same && self.version.is_some_and(|v| p.version <= v) {
            return Decision::Drop;
        }
        if self.epoch_reset && p.epoch < self.epoch {
            return Decision::Hold;
        }
        let v = self.version.unwrap_or(0);
        if !reset && p.base < v {
            return Decision::Drop;
        }
        if !reset && p.base > v {
            return Decision::Resync;
        }
        if !reset {
            let new_img: HashSet<&str> = p.images.iter().map(|(i, _)| &**i).collect();
            let new_sty: HashSet<u64> = p.styles.iter().map(|(i, _)| *i).collect();
            for r in p.rows() {
                if r.images
                    .iter()
                    .any(|(_, id, _)| !self.images.contains_key(id) && !new_img.contains(&**id))
                {
                    return Decision::Resync;
                }
                if r.runs.iter().any(|(_, _, f)| matches!(f, Face::Style(id) if !self.styles.contains_key(id) && !new_sty.contains(id))) {
                    return Decision::Resync;
                }
            }
        }
        Decision::Apply
    }

    /// Count a resync at `now`: `Resync` when Lisp should send it, `Quiet` when one is outstanding or the back-off
    /// holds it, `Broken` when it trips the breaker.
    fn resync_at(&mut self, now: u64) -> Decision {
        self.resyncs += 1;
        self.resync_times
            .retain(|t| now.saturating_sub(*t) < BREAK_MS);
        self.resync_times.push_back(now);
        if self.resync_times.len() >= BREAK_COUNT {
            self.broken = true;
            return Decision::Broken;
        }
        if self.resync {
            return Decision::Quiet;
        }
        let wait = BACKOFF_MS[(self.resync_times.len() - 1).min(BACKOFF_MS.len() - 1)];
        if self
            .last_resync
            .is_some_and(|t| self.resync_times.len() > 1 && now.saturating_sub(t) < wait)
        {
            return Decision::Quiet;
        }
        self.resync = true;
        self.last_resync = Some(now);
        Decision::Resync
    }

    /// A patch that did not decode: a resync, by the books.
    pub fn resync_now(&mut self, now: u64) -> Decision {
        if self.broken {
            Decision::Broken
        } else {
            self.resync_at(now)
        }
    }

    /// Decide `p` and keep the books; an applied patch's rows are stored (the draw does the rest).
    pub fn push(&mut self, p: Patch, now: u64) -> Decision {
        if self.broken {
            return Decision::Broken;
        }
        if let Some((n, kept)) = &mut self.paused {
            *n += 1;
            if *n <= 256 {
                kept.push(p);
            } else {
                kept.clear();
            }
            return Decision::Paused;
        }
        match self.decide(&p) {
            Decision::Hold => {
                self.held.push(p);
                if self.held.len() > MAX_HELD {
                    self.held.remove(0);
                }
                Decision::Hold
            }
            Decision::Resync => self.resync_at(now),
            Decision::Apply => match self.apply(p) {
                Ok(()) => Decision::Apply,
                Err(()) => self.resync_at(now),
            },
            d => d,
        }
    }

    /// Resume a paused store: the patches kept, in order, or a resync when too many came.
    pub fn resume(&mut self, now: u64) -> Vec<Decision> {
        let Some((n, kept)) = self.paused.take() else {
            return Vec::new();
        };
        if n > 256 {
            return vec![self.resync_at(now)];
        }
        kept.into_iter().map(|p| self.push(p, now)).collect()
    }

    /// The hub went away: forget its hub, version, held patches and resync.
    pub fn forget(&mut self) {
        self.hub = None;
        self.version = None;
        self.resync = false;
        self.held.clear();
    }

    fn apply(&mut self, p: Patch) -> Result<(), ()> {
        let reset = p.reset();
        if reset {
            self.images.clear();
            self.styles.clear();
            self.held.clear();
            self.resync = false;
            self.hub = Some(p.hub);
        }
        for (id, s) in p.styles {
            self.fresh_styles.push(id);
            self.styles.insert(id, s);
        }
        for (id, svg) in p.images {
            self.fresh_images.push(id.clone());
            self.images.insert(id, svg);
        }
        for op in p.ops {
            match op {
                Op::Reset(rows) => {
                    if rows.len() > MAX_ROWS {
                        return Err(());
                    }
                    self.order = rows.iter().map(|r| r.key.clone()).collect();
                    self.rows = rows.into_iter().map(|r| (r.key.clone(), r)).collect();
                }
                Op::Insert { after, rows } => {
                    if self.order.len() + rows.len() > MAX_ROWS {
                        return Err(());
                    }
                    let at = match &after {
                        Some(a) => self
                            .order
                            .iter()
                            .position(|k| k == a)
                            .map_or(self.order.len(), |i| i + 1),
                        None => 0,
                    };
                    let keys: Vec<Box<str>> = rows.iter().map(|r| r.key.clone()).collect();
                    for r in rows {
                        if self.rows.insert(r.key.clone(), r).is_some() {
                            return Err(()); // a key inserted twice: the hub and the editor disagree
                        }
                    }
                    self.order.splice(at..at, keys);
                }
                Op::Replace(rows) => {
                    for r in rows {
                        if !self.rows.contains_key(&r.key) {
                            return Err(());
                        }
                        self.rows.insert(r.key.clone(), r);
                    }
                }
                Op::Remove(keys) => {
                    let gone: HashSet<&str> = keys.iter().map(|k| &**k).collect();
                    self.order.retain(|k| !gone.contains(&**k));
                    for k in &keys {
                        self.rows.remove(k);
                    }
                }
            }
        }
        self.version = Some(p.version);
        if p.term.is_some() {
            self.term = p.term;
        }
        if p.model.is_some() {
            self.model = p.model;
        }
        self.evict_images();
        Ok(())
    }

    fn evict_images(&mut self) {
        if self.images.len() <= MAX_IMAGES {
            return;
        }
        let live: HashSet<&str> = self
            .rows
            .values()
            .flat_map(|r| r.images.iter().map(|(_, id, _)| &**id))
            .collect();
        let gone: Vec<Box<str>> = self
            .images
            .keys()
            .filter(|k| !live.contains(&***k))
            .cloned()
            .collect();
        for k in gone {
            self.images.remove(&k);
        }
    }

    /// The styles and images Lisp has not been given, taken.
    pub fn take_fresh(&mut self) -> Fresh {
        let s = std::mem::take(&mut self.fresh_styles)
            .into_iter()
            .filter_map(|id| self.styles.get(&id).map(|s| (id, s.clone())))
            .collect();
        let i = std::mem::take(&mut self.fresh_images)
            .into_iter()
            .filter_map(|id| self.images.get(&id).map(|s| (id, s.clone())))
            .collect();
        (s, i)
    }

    /// Every style and image again (a GUI/tty switch, a theme change: Lisp makes its faces and images afresh).
    pub fn refresh_all(&mut self) {
        self.fresh_styles = self.styles.keys().copied().collect();
        self.fresh_images = self.images.keys().cloned().collect();
        self.redraw_all = true;
    }

    /// The drawn state's length in characters.
    pub fn drawn_chars(&self) -> usize {
        self.drawn.iter().map(|d| d.chars).sum()
    }

    /// The edits that turn the region (of `region` characters) into the rows, and the batch id Lisp confirms with
    /// `drawn`.  `whole` is true when the region is erased and drawn whole (the first edit then deletes it all).
    pub fn draw(&mut self, region: usize) -> (u64, bool, Vec<Edit>) {
        let rolled_back = self.pending.take().is_some();
        let whole = rolled_back || self.redraw_all || region != self.drawn_chars();
        if whole {
            self.drawn.clear();
            self.redraw_all = false;
        }
        let want: Vec<Drawn> = self
            .order
            .iter()
            .filter_map(|k| self.rows.get(k))
            .map(|r| Drawn {
                key: r.key.clone(),
                chars: r.chars,
                hash: r.hash,
            })
            .collect();
        let mut edits: Vec<Edit> = Vec::new();
        if whole {
            edits.push(Edit {
                offset: 0,
                delete: region,
                rows: want.iter().map(|d| d.key.clone()).collect(),
            });
        } else {
            // the drawn rows kept: in both lists, in the same order (a greedy increasing run over the wanted index)
            let at: HashMap<&str, usize> =
                want.iter().enumerate().map(|(i, d)| (&*d.key, i)).collect();
            let mut keep = vec![false; self.drawn.len()];
            let mut last: Option<usize> = None;
            for (j, d) in self.drawn.iter().enumerate() {
                if let Some(&i) = at.get(&*d.key)
                    && last.is_none_or(|l| i > l)
                    && want[i].hash == d.hash
                {
                    keep[j] = true;
                    last = Some(i);
                }
            }
            let kept: HashSet<&str> = self
                .drawn
                .iter()
                .zip(&keep)
                .filter(|(_, k)| **k)
                .map(|(d, _)| &*d.key)
                .collect();
            // walk both in order: between two kept rows, the drawn rows go and the wanted rows come
            let (mut i, mut j, mut pos) = (0usize, 0usize, 0usize);
            loop {
                let mut delete = 0;
                while j < self.drawn.len() && !keep[j] {
                    delete += self.drawn[j].chars;
                    j += 1;
                }
                let mut rows = Vec::new();
                while i < want.len() && !kept.contains(&*want[i].key) {
                    rows.push(want[i].key.clone());
                    i += 1;
                }
                // (a kept row lies between two edits, so edits never touch)
                if delete > 0 || !rows.is_empty() {
                    let added: usize = rows
                        .iter()
                        .filter_map(|k| self.rows.get(k))
                        .map(|r| r.chars)
                        .sum();
                    edits.push(Edit {
                        offset: pos,
                        delete,
                        rows,
                    });
                    pos += added;
                }
                if j >= self.drawn.len() && i >= want.len() {
                    break;
                }
                // the kept row at both heads
                pos += self.drawn[j].chars;
                i += 1;
                j += 1;
            }
        }
        self.batch += 1;
        self.pending = Some((self.batch, want));
        (self.batch, whole, edits)
    }

    /// Lisp drew batch `id`: its rows are the drawn state.  A stale id changes nothing.
    pub fn drawn(&mut self, id: u64) -> bool {
        match self.pending.take() {
            Some((b, want)) if b == id => {
                self.drawn = want;
                true
            }
            other => {
                self.pending = other;
                false
            }
        }
    }

    /// Text kept, properties resolved again: the next draw redraws every row.
    pub fn redraw(&mut self) {
        self.redraw_all = true;
    }

    /// Where `pos` (characters from the region's start) is in the drawn rows: the row's key and the offset in it.
    pub fn place(&self, pos: usize) -> Option<(&str, usize)> {
        let mut at = 0;
        for d in &self.drawn {
            if pos < at + d.chars {
                return Some((&d.key, pos - at));
            }
            at += d.chars;
        }
        self.drawn.last().map(|d| (&*d.key, d.chars))
    }

    /// The position of `key` plus `off` (clamped to the row) in the drawn rows; `None` when the row is not drawn.
    pub fn pos(&self, key: &str, off: usize) -> Option<usize> {
        let mut at = 0;
        for d in &self.drawn {
            if &*d.key == key {
                return Some(at + off.min(d.chars.saturating_sub(1)));
            }
            at += d.chars;
        }
        None
    }

    /// The drawn row's start, and its length in characters.
    pub fn row_span(&self, key: &str) -> Option<(usize, usize)> {
        let mut at = 0;
        for d in &self.drawn {
            if &*d.key == key {
                return Some((at, d.chars));
            }
            at += d.chars;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn r(key: &str, text: &str) -> Value {
        json!({"key": key, "text": text})
    }

    fn p(hub: u64, base: u64, version: u64, ops: Value) -> Patch {
        patch(&json!({"view": "v", "class": "c", "hub": hub, "base": base, "version": version, "env_epoch": 0,
                      "ops": ops}))
        .unwrap()
    }

    /// The region as Lisp would hold it after applying `edits` to `buf` (rows by key from `s`).
    fn apply(s: &Store, buf: &mut String, edits: &[Edit]) {
        for e in edits {
            let start: usize = buf
                .char_indices()
                .nth(e.offset)
                .map_or(buf.len(), |(i, _)| i);
            let end: usize = buf
                .char_indices()
                .nth(e.offset + e.delete)
                .map_or(buf.len(), |(i, _)| i);
            let text: String = e
                .rows
                .iter()
                .map(|k| format!("{}\n", s.row(k).unwrap().text))
                .collect();
            buf.replace_range(start..end, &text);
        }
    }

    fn expect(s: &Store) -> String {
        s.keys()
            .iter()
            .map(|k| format!("{}\n", s.row(k).unwrap().text))
            .collect()
    }

    fn draw(s: &mut Store, buf: &mut String) -> Vec<Edit> {
        let (id, _, edits) = s.draw(buf.chars().count());
        apply(s, buf, &edits);
        assert!(s.drawn(id));
        edits
    }

    #[test]
    fn a_reset_draws_every_row_and_a_replace_one() {
        let mut s = Store::new(true);
        let mut buf = String::new();
        assert_eq!(
            s.push(
                p(
                    7,
                    0,
                    1,
                    json!([{"reset": [r("a", "one"), r("b", "two"), r("c", "three")]}])
                ),
                0
            ),
            Decision::Apply
        );
        draw(&mut s, &mut buf);
        assert_eq!(buf, "one\ntwo\nthree\n");
        assert_eq!(
            s.push(p(7, 1, 2, json!([{"replace": [r("b", "TWO!")]}])), 0),
            Decision::Apply
        );
        let edits = draw(&mut s, &mut buf);
        assert_eq!(buf, expect(&s));
        assert_eq!(
            edits,
            vec![Edit {
                offset: 4,
                delete: 4,
                rows: vec!["b".into()]
            }]
        );
    }

    #[test]
    fn inserts_and_removes_touch_only_their_rows() {
        let mut s = Store::new(true);
        let mut buf = String::new();
        s.push(
            p(
                1,
                0,
                1,
                json!([{"reset": [r("a", "a"), r("b", "b"), r("c", "c")]}]),
            ),
            0,
        );
        draw(&mut s, &mut buf);
        s.push(
            p(
                1,
                1,
                2,
                json!([{"insert": {"after": "a", "rows": [r("x", "xx")]}}, {"remove": ["c"]}]),
            ),
            0,
        );
        let edits = draw(&mut s, &mut buf);
        assert_eq!(buf, "a\nxx\nb\n");
        assert_eq!(edits.len(), 2);
        s.push(
            p(
                1,
                2,
                3,
                json!([{"insert": {"after": null, "rows": [r("top", "t")]}}]),
            ),
            0,
        );
        draw(&mut s, &mut buf);
        assert_eq!(buf, "t\na\nxx\nb\n");
    }

    #[test]
    fn a_reorder_is_drawn_right() {
        let mut s = Store::new(true);
        let mut buf = String::new();
        s.push(
            p(
                1,
                0,
                1,
                json!([{"reset": [r("a", "a"), r("b", "b"), r("c", "c")]}]),
            ),
            0,
        );
        draw(&mut s, &mut buf);
        s.push(
            p(
                1,
                1,
                2,
                json!([{"reset": [r("c", "c"), r("a", "a"), r("b", "B")]}]),
            ),
            0,
        );
        draw(&mut s, &mut buf);
        assert_eq!(buf, "c\na\nB\n");
    }

    #[test]
    fn the_rules() {
        let mut s = Store::new(true);
        // a diff before any reset
        assert_eq!(
            s.push(p(1, 0, 1, json!([{"replace": [r("a", "a")]}])), 0),
            Decision::Drop
        );
        s.push(p(1, 0, 1, json!([{"reset": [r("a", "a")]}])), 0);
        // an old reset of the same hub
        assert_eq!(
            s.push(p(1, 0, 1, json!([{"reset": []}])), 0),
            Decision::Drop
        );
        // another hub's diff
        assert_eq!(
            s.push(p(2, 1, 2, json!([{"replace": [r("a", "b")]}])), 0),
            Decision::Drop
        );
        // an older base
        s.push(p(1, 1, 2, json!([{"replace": [r("a", "b")]}])), 0);
        assert_eq!(
            s.push(p(1, 1, 3, json!([{"replace": [r("a", "c")]}])), 0),
            Decision::Drop
        );
        // a gap: one resync, then quiet while it is outstanding
        assert_eq!(
            s.push(p(1, 5, 6, json!([{"replace": [r("a", "c")]}])), 0),
            Decision::Resync
        );
        assert_eq!(
            s.push(p(1, 6, 7, json!([{"replace": [r("a", "c")]}])), 100),
            Decision::Quiet
        );
        // a reset answers it
        assert_eq!(
            s.push(p(1, 0, 9, json!([{"reset": [r("a", "z")]}])), 200),
            Decision::Apply
        );
        assert!(!s.resync);
        // a style the store has not got
        assert_eq!(
            s.push(
                p(
                    1,
                    9,
                    10,
                    json!([{"replace": [{"key": "a", "text": "z", "runs": [[0, 1, "s:4"]]}]}])
                ),
                20_000
            ),
            Decision::Resync
        );
    }

    #[test]
    fn an_older_epoch_is_held_until_a_reset() {
        let mut s = Store::new(true);
        s.epoch = 3;
        let mut q = p(1, 0, 1, json!([{"reset": [r("a", "a")]}]));
        q.epoch = 2;
        assert_eq!(s.push(q, 0), Decision::Hold);
        assert_eq!(s.held.len(), 1);
        let mut q = p(1, 0, 2, json!([{"reset": [r("a", "a")]}]));
        q.epoch = 3;
        assert_eq!(s.push(q, 0), Decision::Apply);
        assert!(s.held.is_empty());
    }

    #[test]
    fn three_resyncs_in_ten_seconds_break_the_store() {
        let mut s = Store::new(true);
        s.push(p(1, 0, 1, json!([{"reset": []}])), 0);
        assert_eq!(s.push(p(1, 5, 6, json!([])), 0), Decision::Resync);
        s.resync = false; // as if the hub answered without a reset
        assert_eq!(s.push(p(1, 5, 6, json!([])), 500), Decision::Quiet); // inside the 1 s back-off
        assert_eq!(s.push(p(1, 5, 6, json!([])), 2_000), Decision::Broken);
        assert_eq!(
            s.push(p(1, 0, 9, json!([{"reset": []}])), 3_000),
            Decision::Broken
        );
    }

    #[test]
    fn an_unconfirmed_batch_is_rolled_back_and_a_wrong_length_redraws() {
        let mut s = Store::new(true);
        let mut buf = String::new();
        s.push(
            p(1, 0, 1, json!([{"reset": [r("a", "a"), r("b", "b")]}])),
            0,
        );
        draw(&mut s, &mut buf);
        s.push(p(1, 1, 2, json!([{"replace": [r("b", "bb")]}])), 0);
        let (_, whole, _) = s.draw(buf.chars().count()); // a quit: never applied, never confirmed
        assert!(!whole);
        let (id, whole, edits) = s.draw(buf.chars().count());
        assert!(whole);
        apply(&s, &mut buf, &edits);
        assert!(s.drawn(id));
        assert_eq!(buf, "a\nbb\n");
        // a region edited behind the store's back
        buf.push('x');
        let (id, whole, edits) = s.draw(buf.chars().count());
        assert!(whole);
        apply(&s, &mut buf, &edits);
        s.drawn(id);
        assert_eq!(buf, "a\nbb\n");
    }

    #[test]
    fn places_follow_the_rows() {
        let mut s = Store::new(true);
        let mut buf = String::new();
        s.push(
            p(1, 0, 1, json!([{"reset": [r("a", "ab"), r("b", "cde")]}])),
            0,
        );
        draw(&mut s, &mut buf);
        assert_eq!(s.place(4), Some(("b", 1)));
        s.push(
            p(
                1,
                1,
                2,
                json!([{"insert": {"after": null, "rows": [r("t", "0123")]}}]),
            ),
            0,
        );
        draw(&mut s, &mut buf);
        assert_eq!(s.pos("b", 1), Some(9));
        assert_eq!(s.row_span("a"), Some((5, 3)));
    }

    #[test]
    fn a_paused_store_keeps_its_patches() {
        let mut s = Store::new(true);
        s.push(p(1, 0, 1, json!([{"reset": [r("a", "a")]}])), 0);
        s.paused = Some((0, Vec::new()));
        assert_eq!(
            s.push(p(1, 1, 2, json!([{"replace": [r("a", "b")]}])), 0),
            Decision::Paused
        );
        assert_eq!(s.resume(0), vec![Decision::Apply]);
        assert_eq!(s.row("a").unwrap().text.as_ref(), "b");
    }

    #[test]
    fn caps_and_decoding() {
        let long = "é".repeat(MAX_TEXT);
        let row = row(
            &json!({"key": "k", "text": long, "runs": [[0, 999_999, "s:3"], [1, 2, "drgnx-x"]]}),
        )
        .unwrap();
        assert!(row.text.len() <= MAX_TEXT);
        assert_eq!(row.runs[0], (0, row.chars as u32 - 1, Face::Style(3)));
        assert_eq!(row.runs[1].2, Face::Named("drgnx-x".into()));
        assert!(patch(&json!({"hub": 1})).is_err());
    }

    /// Every arrival order of three diffs after a reset ends in the same rows or a resync, never in other rows.
    #[test]
    fn arrival_orders_never_draw_wrong_rows() {
        let diffs = [
            p(1, 1, 2, json!([{"replace": [r("a", "2")]}])),
            p(
                1,
                2,
                3,
                json!([{"insert": {"after": "a", "rows": [r("b", "3")]}}]),
            ),
            p(1, 3, 4, json!([{"remove": ["a"]}])),
        ];
        let orders = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];
        for o in orders {
            let mut s = Store::new(true);
            s.push(p(1, 0, 1, json!([{"reset": [r("a", "1")]}])), 0);
            let mut resynced = false;
            for (t, i) in o.iter().enumerate() {
                resynced |= s.push(diffs[*i].clone(), t as u64 * 5_000) == Decision::Resync;
            }
            let mut buf = String::new();
            draw(&mut s, &mut buf);
            if o == [0, 1, 2] {
                assert_eq!(buf, "3\n");
            } else {
                assert!(resynced || buf == "3\n", "order {o:?} drew {buf:?}");
            }
        }
    }
}
