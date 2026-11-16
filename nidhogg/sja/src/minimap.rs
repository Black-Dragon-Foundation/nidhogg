//! The minimap's hub layers (`sja::minimap`): what the hub knows of a file that the editor's line model does not —
//! labels, fold ranges for buffers no parser folds, heat, a huge file's density buckets — kept here by file, read by
//! the line model and the minimap layer in Rust, so none of it becomes a Lisp string.  A layer comes as the hub's
//! `view-model` line (the framed reader's raw path, `SJA_PUSH`):
//!
//! ```text
//! {"event": "view-model", "data": {"minimap": {"file": F, "layer": L, "version": N, "items": I}}}
//! L = "labels"   I = [[line, depth, kind, text]]    depth 0 outermost
//!   | "folds"    I = [[first, last, kind]]
//!   | "heat:" S  I = [[first, last, value]]          value 0..=1000
//!   | "density"  I = {"buckets": [[bytes, errors, warnings]], "total": n}
//! I = null removes the layer.
//! ```
//!
//! Lines are from 1.  The newest version of each (file, layer) is kept; an older or equal one is dropped.  At most
//! [`FILES_MAX`] files are kept, the one touched longest ago going first, and at most [`ITEMS_MAX`] items a layer.

use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// The most files whose layers are kept at once.
pub const FILES_MAX: usize = 64;
/// The most items a layer keeps; the rest are dropped.
pub const ITEMS_MAX: usize = 1 << 20;
/// The longest label text kept, in bytes (cut at a character boundary).
pub const LABEL_MAX: usize = 256;

/// A layer's items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Data {
    /// (line, depth, kind, text), in the order of their lines.
    Labels(Vec<(u32, u32, Box<str>, Box<str>)>),
    /// (first line, last line, kind).
    Folds(Vec<(u32, u32, Box<str>)>),
    /// (first line, last line, value 0..=1000).
    Heat(Vec<(u32, u32, u16)>),
    /// The buckets built, (bytes, errors, warnings) each, and how many there are in all.
    Density {
        buckets: Vec<(u64, u32, u32)>,
        total: u32,
    },
}

impl Data {
    /// How many items the layer holds.
    pub fn len(&self) -> usize {
        match self {
            Data::Labels(v) => v.len(),
            Data::Folds(v) => v.len(),
            Data::Heat(v) => v.len(),
            Data::Density { buckets, .. } => buckets.len(),
        }
    }

    /// Whether the layer holds no item.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One pushed layer, read: the file, the layer's name, its version and its items (`None`: remove it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Push {
    pub file: String,
    pub layer: String,
    pub version: u64,
    pub data: Option<Data>,
}

/// What a push did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Took {
    /// The layer is now this version (or gone, for a removal).
    Kept,
    /// The layer held this version or a newer one: nothing changed.
    Stale,
}

#[derive(Default)]
struct Store {
    files: BTreeMap<String, File>,
    tick: u64,
}

#[derive(Default)]
struct File {
    layers: BTreeMap<String, (u64, Data)>,
    /// The versions of removed layers, so an older layer arriving late stays out.
    removed: BTreeMap<String, u64>,
    touched: u64,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut g = STORE.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(Store::default))
}

fn u32_of(v: &Value) -> Option<u32> {
    u32::try_from(v.as_u64()?).ok()
}

fn text(v: &Value, max: usize) -> Option<Box<str>> {
    let s = v.as_str()?;
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    Some(s[..end].into())
}

/// The items of an array, each read by F; `None` when it is not an array or an item does not read.
fn items<T>(v: &Value, f: impl Fn(&[Value]) -> Option<T>) -> Option<Vec<T>> {
    v.as_array()?
        .iter()
        .take(ITEMS_MAX)
        .map(|i| f(i.as_array()?))
        .collect()
}

fn data(layer: &str, v: &Value) -> Option<Data> {
    Some(match layer {
        "labels" => Data::Labels({
            let mut v = items(v, |i| match i {
                [l, d, k, t] => Some((u32_of(l)?, u32_of(d)?, text(k, 64)?, text(t, LABEL_MAX)?)),
                _ => None,
            })?;
            // the minimap looks them up by line
            v.sort_by_key(|x| x.0);
            v
        }),
        "folds" => Data::Folds(items(v, |i| match i {
            [a, b, k] => Some((u32_of(a)?, u32_of(b)?, text(k, 64)?)),
            _ => None,
        })?),
        "density" => Data::Density {
            buckets: items(v.get("buckets")?, |i| match i {
                [b, e, w] => Some((b.as_u64()?, u32_of(e)?, u32_of(w)?)),
                _ => None,
            })?,
            total: u32_of(v.get("total")?)?,
        },
        l if l.starts_with("heat:") && l.len() > 5 => Data::Heat(items(v, |i| match i {
            [a, b, x] => Some((
                u32_of(a)?,
                u32_of(b)?,
                u16::try_from(x.as_u64()?).ok()?.min(1000),
            )),
            _ => None,
        })?),
        _ => return None,
    })
}

/// Read a minimap line.  `None` when the line is not one; `Some(Err(()))` when it is one that does not read (it
/// changes nothing).
pub fn read_line(line: &str) -> Option<Result<Push, ()>> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("event")?.as_str()? != "view-model" {
        return None;
    }
    let m = v.get("data")?.get("minimap")?;
    let read = || {
        let file = m.get("file")?.as_str()?.to_string();
        let layer = m.get("layer")?.as_str()?.to_string();
        let version = m.get("version")?.as_u64()?;
        let items = m.get("items")?;
        let data = if items.is_null() {
            None
        } else {
            Some(data(&layer, items)?)
        };
        (!file.is_empty()).then_some(Push {
            file,
            layer,
            version,
            data,
        })
    };
    Some(read().ok_or(()))
}

/// Keep P: its layer becomes this version, unless the file holds this version or a newer one.
pub fn push(p: Push) -> Took {
    with_store(|s| {
        s.tick += 1;
        let tick = s.tick;
        if !s.files.contains_key(&p.file)
            && s.files.len() >= FILES_MAX
            && let Some(oldest) = s
                .files
                .iter()
                .min_by_key(|(_, f)| f.touched)
                .map(|(k, _)| k.clone())
        {
            s.files.remove(&oldest);
        }
        let f = s.files.entry(p.file).or_default();
        f.touched = tick;
        let held = f
            .layers
            .get(&p.layer)
            .map(|(v, _)| *v)
            .max(f.removed.get(&p.layer).copied());
        if held.is_some_and(|h| h >= p.version) {
            return Took::Stale;
        }
        match p.data {
            Some(d) => {
                f.removed.remove(&p.layer);
                f.layers.insert(p.layer, (p.version, d));
            }
            None => {
                f.layers.remove(&p.layer);
                f.removed.insert(p.layer, p.version);
            }
        }
        Took::Kept
    })
}

/// F of FILE's LAYER (its version and items), or `None` when it holds no such layer.
pub fn with_layer<R>(file: &str, layer: &str, f: impl FnOnce(u64, &Data) -> R) -> Option<R> {
    with_store(|s| {
        let (v, d) = s.files.get(file)?.layers.get(layer)?;
        Some(f(*v, d))
    })
}

/// FILE's layers by name, with their versions.
pub fn layers(file: &str) -> Vec<(String, u64)> {
    with_store(|s| {
        s.files.get(file).map_or_else(Vec::new, |f| {
            f.layers.iter().map(|(k, (v, _))| (k.clone(), *v)).collect()
        })
    })
}

/// Forget FILE's layers (its last buffer was killed).
pub fn forget(file: &str) {
    with_store(|s| {
        s.files.remove(file);
    });
}
