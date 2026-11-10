//! The wire: one model patch, `{"ops": [...]}`, decoded from JSON into typed ops before anything is applied.  A
//! malformed patch is an error here and changes nothing.
//!
//! ```text
//! op = {"reset": {"descriptor": D, "nodes": [N], "edges": [E], "lists": [L]}}
//!    | {"nodes": [N]} | {"remove": [id]} | {"edges": {"add": [E], "remove": [E]}}
//!    | {"list": L} | {"splice": {"name", "at", "remove", "ids"}}
//! D  = {"scheme": "links"|"list"|"tree", "chain_kinds": [k], "match_fields": [f], "lists": [name],
//!       "classes": [name], "flags": [name], "forms": [{"name", "list"?}], "start": name?}
//! N  = {"id", "kind", "parent"?, "flags"?: u32, "nav"?: {link: id|null}, "navs"?: {form: {link: id|null}},
//!       "fields"?: [[name, string|int]], "rows"?: [key], "then"?: id|null}
//! E  = [from, to, kind]
//! L  = {"name", "ids": [id], "total"?: n, "from"?: n}
//! ```

use serde_json::Value;

/// A field value: a string or an integer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Val {
    Str(Box<str>),
    Int(i64),
}

/// The longest field value kept, in bytes; longer ones are cut (at a character boundary).
pub const MAX_VALUE: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Links,
    List,
    Tree,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormIn {
    pub name: String,
    /// For a list form: the list `up` and `down` walk.
    pub list: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DescIn {
    pub scheme: Scheme,
    pub chain_kinds: Vec<String>,
    pub match_fields: Vec<String>,
    pub lists: Vec<String>,
    pub classes: Vec<String>,
    pub flags: Vec<String>,
    pub forms: Vec<FormIn>,
    pub start: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeIn {
    pub id: String,
    pub kind: String,
    pub parent: Option<String>,
    pub flags: u32,
    pub nav: Vec<(String, String)>,
    pub navs: Vec<(String, Vec<(String, String)>)>,
    pub fields: Vec<(String, Val)>,
    pub rows: Vec<String>,
    pub then: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeIn {
    pub from: String,
    pub to: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListIn {
    pub name: String,
    pub ids: Vec<String>,
    pub total: Option<usize>,
    pub from: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResetIn {
    pub desc: DescIn,
    pub nodes: Vec<NodeIn>,
    pub edges: Vec<EdgeIn>,
    pub lists: Vec<ListIn>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Reset(Box<ResetIn>),
    Nodes(Vec<NodeIn>),
    Remove(Vec<String>),
    Edges {
        add: Vec<EdgeIn>,
        remove: Vec<EdgeIn>,
    },
    List(ListIn),
    Splice {
        name: String,
        at: usize,
        remove: usize,
        ids: Vec<String>,
    },
}

/// Why a patch was refused at decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bad(pub String);

type R<T> = Result<T, Bad>;

fn bad<T>(what: impl Into<String>) -> R<T> {
    Err(Bad(what.into()))
}

fn obj<'a>(v: &'a Value, what: &str) -> R<&'a serde_json::Map<String, Value>> {
    v.as_object()
        .ok_or_else(|| Bad(format!("{what}: not an object")))
}

fn arr<'a>(v: &'a Value, what: &str) -> R<&'a Vec<Value>> {
    v.as_array()
        .ok_or_else(|| Bad(format!("{what}: not an array")))
}

fn string(v: &Value, what: &str) -> R<String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        // row keys and ids may come as numbers
        Value::Number(n) if n.is_i64() || n.is_u64() => Ok(n.to_string()),
        _ => bad(format!("{what}: not a string")),
    }
}

fn opt_string(v: Option<&Value>, what: &str) -> R<Option<String>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(v) => string(v, what).map(Some),
    }
}

fn strings(v: Option<&Value>, what: &str) -> R<Vec<String>> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(v) => arr(v, what)?.iter().map(|x| string(x, what)).collect(),
    }
}

fn count(v: Option<&Value>, what: &str) -> R<Option<usize>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| Bad(format!("{what}: not a count"))),
    }
}

fn links(v: Option<&Value>, what: &str) -> R<Vec<(String, String)>> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (k, t) in obj(v, what)? {
        if let Some(t) = opt_string(Some(t), what)? {
            out.push((k.clone(), t));
        }
    }
    Ok(out)
}

/// Cut `s` to at most `MAX_VALUE` bytes at a character boundary.
fn cut(mut s: String) -> String {
    if s.len() > MAX_VALUE {
        let mut at = MAX_VALUE;
        while !s.is_char_boundary(at) {
            at -= 1;
        }
        s.truncate(at);
    }
    s
}

fn node(v: &Value) -> R<NodeIn> {
    let o = obj(v, "node")?;
    let id = string(o.get("id").unwrap_or(&Value::Null), "node id")?;
    let kind = string(o.get("kind").unwrap_or(&Value::Null), "node kind")?;
    let flags = match o.get("flags") {
        None | Some(Value::Null) => 0,
        Some(f) => f
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| Bad("node flags: not a u32".into()))?,
    };
    let mut navs = Vec::new();
    if let Some(n) = o.get("navs").filter(|v| !v.is_null()) {
        for (form, set) in obj(n, "navs")? {
            navs.push((form.clone(), links(Some(set), "navs")?));
        }
    }
    let mut fields = Vec::new();
    if let Some(fs) = o.get("fields").filter(|v| !v.is_null()) {
        for f in arr(fs, "fields")? {
            let pair = arr(f, "field")?;
            let [name, value] = pair.as_slice() else {
                return bad("field: not a pair");
            };
            let name = string(name, "field name")?;
            let value = match value {
                Value::String(s) => Val::Str(cut(s.clone()).into()),
                Value::Number(n) => match n.as_i64() {
                    Some(i) => Val::Int(i),
                    None => return bad("field value: not an integer"),
                },
                Value::Bool(b) => Val::Int(i64::from(*b)),
                Value::Null => continue,
                _ => return bad("field value: not a string or integer"),
            };
            fields.push((name, value));
        }
    }
    Ok(NodeIn {
        id,
        kind,
        parent: opt_string(o.get("parent"), "node parent")?,
        flags,
        nav: links(o.get("nav"), "nav")?,
        navs,
        fields,
        rows: strings(o.get("rows"), "rows")?,
        then: opt_string(o.get("then"), "then")?,
    })
}

fn edge(v: &Value) -> R<EdgeIn> {
    let a = arr(v, "edge")?;
    let [from, to, kind] = a.as_slice() else {
        return bad("edge: not [from, to, kind]");
    };
    Ok(EdgeIn {
        from: string(from, "edge from")?,
        to: string(to, "edge to")?,
        kind: string(kind, "edge kind")?,
    })
}

fn edges(v: Option<&Value>) -> R<Vec<EdgeIn>> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(v) => arr(v, "edges")?.iter().map(edge).collect(),
    }
}

fn list(v: &Value) -> R<ListIn> {
    let o = obj(v, "list")?;
    Ok(ListIn {
        name: string(o.get("name").unwrap_or(&Value::Null), "list name")?,
        ids: strings(o.get("ids"), "list ids")?,
        total: count(o.get("total"), "list total")?,
        from: count(o.get("from"), "list from")?.unwrap_or(0),
    })
}

fn desc(v: &Value) -> R<DescIn> {
    let o = obj(v, "descriptor")?;
    let scheme = match o.get("scheme").and_then(Value::as_str) {
        Some("links") => Scheme::Links,
        Some("list") => Scheme::List,
        Some("tree") => Scheme::Tree,
        _ => return bad("descriptor scheme: not links, list or tree"),
    };
    let mut forms = Vec::new();
    if let Some(fs) = o.get("forms").filter(|v| !v.is_null()) {
        for f in arr(fs, "forms")? {
            let fo = obj(f, "form")?;
            forms.push(FormIn {
                name: string(fo.get("name").unwrap_or(&Value::Null), "form name")?,
                list: opt_string(fo.get("list"), "form list")?,
            });
        }
    }
    Ok(DescIn {
        scheme,
        chain_kinds: strings(o.get("chain_kinds"), "chain_kinds")?,
        match_fields: strings(o.get("match_fields"), "match_fields")?,
        lists: strings(o.get("lists"), "lists")?,
        classes: strings(o.get("classes"), "classes")?,
        flags: strings(o.get("flags"), "flags")?,
        forms,
        start: opt_string(o.get("start"), "start")?,
    })
}

fn op(v: &Value) -> R<Op> {
    let o = obj(v, "op")?;
    let mut it = o.iter();
    let (Some((name, body)), None) = (it.next(), it.next()) else {
        return bad("op: not one key");
    };
    Ok(match name.as_str() {
        "reset" => {
            let r = obj(body, "reset")?;
            let lists = match r.get("lists") {
                None | Some(Value::Null) => Vec::new(),
                Some(l) => arr(l, "lists")?.iter().map(list).collect::<R<_>>()?,
            };
            let nodes = match r.get("nodes") {
                None | Some(Value::Null) => Vec::new(),
                Some(n) => arr(n, "nodes")?.iter().map(node).collect::<R<_>>()?,
            };
            Op::Reset(Box::new(ResetIn {
                desc: desc(r.get("descriptor").unwrap_or(&Value::Null))?,
                nodes,
                edges: edges(r.get("edges"))?,
                lists,
            }))
        }
        "nodes" => Op::Nodes(arr(body, "nodes")?.iter().map(node).collect::<R<_>>()?),
        "remove" => Op::Remove(strings(Some(body), "remove")?),
        "edges" => {
            let e = obj(body, "edges")?;
            Op::Edges {
                add: edges(e.get("add"))?,
                remove: edges(e.get("remove"))?,
            }
        }
        "list" => Op::List(list(body)?),
        "splice" => {
            let s = obj(body, "splice")?;
            Op::Splice {
                name: string(s.get("name").unwrap_or(&Value::Null), "splice name")?,
                at: count(s.get("at"), "splice at")?.unwrap_or(0),
                remove: count(s.get("remove"), "splice remove")?.unwrap_or(0),
                ids: strings(s.get("ids"), "splice ids")?,
            }
        }
        other => return bad(format!("op: unknown {other:?}")),
    })
}

/// Decode one model patch.
pub fn decode(text: &str) -> R<Vec<Op>> {
    let v: Value = serde_json::from_str(text).map_err(|e| Bad(format!("json: {e}")))?;
    decode_value(&v)
}

/// Decode one model patch already parsed (a `view-model` line's model object).
pub fn decode_value(v: &Value) -> R<Vec<Op>> {
    let o = obj(v, "patch")?;
    let ops = o.get("ops").ok_or_else(|| Bad("patch: no ops".into()))?;
    arr(ops, "ops")?.iter().map(op).collect()
}
