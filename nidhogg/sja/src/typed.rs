//! The typed decode: a `view-model` line, or a model, read straight into ops by `serde_json`'s deserializer and
//! hand-written visitors, with no `Value` between (no serde_derive).
//!
//! It reads only what the `Value` decode (`wire.rs`, `queue::read_line_value`) reads the same way.  Anything else (a
//! malformed model, a duplicate key, a float where a count goes, a key out of the usual shape) is refused here, and the
//! caller falls back to the `Value` decode, whose answer and error stand.  Maps the `Value` decode iterates (`nav`,
//! `navs`) come out in its order, sorted by key.

use std::fmt;

use serde_core::de::{
    self, Deserialize, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor,
};

use crate::queue::Envelope;
use crate::wire::{DescIn, EdgeIn, FormIn, ListIn, NodeIn, Op, ResetIn, Scheme, Val, cut};

/// Read a model, `{"ops": [...]}`; `None`: fall back.
pub(crate) fn patch(text: &str) -> Option<Vec<Op>> {
    run::<Patch>(text).map(|p| p.0)
}

/// Read a `view-model` line; `None`: fall back.
pub(crate) fn line(text: &str) -> Option<(Envelope, Vec<Op>)> {
    run::<Line>(text).map(|l| l.0)
}

fn run<T: for<'de> Deserialize<'de>>(text: &str) -> Option<T> {
    let mut d = serde_json::Deserializer::from_str(text);
    let v = T::deserialize(&mut d).ok()?;
    d.end().ok()?;
    Some(v)
}

/// The refusal: the caller falls back, so its words are never read.
fn no<E: de::Error>() -> E {
    E::custom("typed: fall back")
}

/// A map key: its index in the known keys, or their count for any other.
struct Key(&'static [&'static str]);

impl<'de> DeserializeSeed<'de> for Key {
    type Value = usize;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<usize, D::Error> {
        d.deserialize_str(self)
    }
}

impl<'de> Visitor<'de> for Key {
    type Value = usize;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a key")
    }
    fn visit_str<E: de::Error>(self, s: &str) -> Result<usize, E> {
        Ok(self.0.iter().position(|k| *k == s).unwrap_or(self.0.len()))
    }
}

/// Read a map's entries: `f` reads each known key's value; other keys are skipped; a known key twice is refused (the
/// `Value` decode keeps the last).
fn entries<'de, A: MapAccess<'de>>(
    map: &mut A,
    keys: &'static [&'static str],
    mut f: impl FnMut(usize, &mut A) -> Result<(), A::Error>,
) -> Result<(), A::Error> {
    let mut seen = 0u32;
    while let Some(i) = map.next_key_seed(Key(keys))? {
        if i == keys.len() {
            map.next_value::<IgnoredAny>()?;
            continue;
        }
        if seen & (1 << i) != 0 {
            return Err(no());
        }
        seen |= 1 << i;
        f(i, map)?;
    }
    Ok(())
}

/// A sequence that must end here.
fn end<'de, A: SeqAccess<'de>>(seq: &mut A) -> Result<(), A::Error> {
    match seq.next_element::<IgnoredAny>()? {
        None => Ok(()),
        Some(_) => Err(no()),
    }
}

fn req<T, E: de::Error>(v: Option<T>) -> Result<T, E> {
    v.ok_or_else(no)
}

/// One wrapper read by one visitor: `typed!(Name, Out, deserialize_any, { fn visit_…(self, …) -> Result<Name, E> })`.
macro_rules! typed {
    ($w:ident, $out:ty, $how:ident, { $($body:tt)* }) => {
        struct $w($out);
        impl<'de> Deserialize<'de> for $w {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct V;
                impl<'de> Visitor<'de> for V {
                    type Value = $w;
                    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                        f.write_str(stringify!($w))
                    }
                    $($body)*
                }
                d.$how(V)
            }
        }
    };
}

/// A list of `T` read by `$item` (null: empty).
macro_rules! list_of {
    ($w:ident, $item:ident, $t:ty) => {
        typed!($w, Vec<$t>, deserialize_any, {
            fn visit_unit<E: de::Error>(self) -> Result<$w, E> {
                Ok($w(Vec::new()))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<$w, A::Error> {
                let mut out = Vec::new();
                while let Some(x) = seq.next_element::<$item>()? {
                    out.push(x.0);
                }
                Ok($w(out))
            }
        });
    };
}

// An id, name or key: a string, or an integer as its digits.
typed!(Str, String, deserialize_any, {
    fn visit_str<E: de::Error>(self, s: &str) -> Result<Str, E> {
        Ok(Str(s.to_owned()))
    }
    fn visit_u64<E: de::Error>(self, n: u64) -> Result<Str, E> {
        Ok(Str(n.to_string()))
    }
    fn visit_i64<E: de::Error>(self, n: i64) -> Result<Str, E> {
        Ok(Str(n.to_string()))
    }
});

typed!(OptStr, Option<String>, deserialize_any, {
    fn visit_unit<E: de::Error>(self) -> Result<OptStr, E> {
        Ok(OptStr(None))
    }
    fn visit_str<E: de::Error>(self, s: &str) -> Result<OptStr, E> {
        Ok(OptStr(Some(s.to_owned())))
    }
    fn visit_u64<E: de::Error>(self, n: u64) -> Result<OptStr, E> {
        Ok(OptStr(Some(n.to_string())))
    }
    fn visit_i64<E: de::Error>(self, n: i64) -> Result<OptStr, E> {
        Ok(OptStr(Some(n.to_string())))
    }
});

list_of!(Strs, Str, String);

// A count (null: none).
typed!(Count, Option<usize>, deserialize_any, {
    fn visit_unit<E: de::Error>(self) -> Result<Count, E> {
        Ok(Count(None))
    }
    fn visit_u64<E: de::Error>(self, n: u64) -> Result<Count, E> {
        usize::try_from(n).map(|n| Count(Some(n))).map_err(|_| no())
    }
});

typed!(Flags, u32, deserialize_any, {
    fn visit_unit<E: de::Error>(self) -> Result<Flags, E> {
        Ok(Flags(0))
    }
    fn visit_u64<E: de::Error>(self, n: u64) -> Result<Flags, E> {
        u32::try_from(n).map(Flags).map_err(|_| no())
    }
});

/// A map's entries sorted by key, as the `Value` decode's map iterates them; a key twice is refused.
fn sorted<T, E: de::Error>(mut v: Vec<(String, T)>) -> Result<Vec<(String, T)>, E> {
    v.sort_by(|a, b| a.0.cmp(&b.0));
    if v.windows(2).any(|w| w[0].0 == w[1].0) {
        return Err(no());
    }
    Ok(v)
}

// A node's links, `{link: id|null}` (null: none; a null target: no link).
typed!(Links, Vec<(String, String)>, deserialize_any, {
    fn visit_unit<E: de::Error>(self) -> Result<Links, E> {
        Ok(Links(Vec::new()))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Links, A::Error> {
        let mut out = Vec::new();
        while let Some((k, t)) = map.next_entry::<Str, OptStr>()? {
            out.push((k.0, t.0));
        }
        let out = sorted(out)?;
        Ok(Links(
            out.into_iter().filter_map(|(k, t)| Some((k, t?))).collect(),
        ))
    }
});

// A node's links per form, `{form: {link: id|null}}`.
typed!(
    Navs,
    Vec<(String, Vec<(String, String)>)>,
    deserialize_any,
    {
        fn visit_unit<E: de::Error>(self) -> Result<Navs, E> {
            Ok(Navs(Vec::new()))
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Navs, A::Error> {
            let mut out = Vec::new();
            while let Some((k, l)) = map.next_entry::<Str, Links>()? {
                out.push((k.0, l.0));
            }
            Ok(Navs(sorted(out)?))
        }
    }
);

// A field's value: a string (cut), an integer, a boolean (0 or 1); null: no field.
typed!(FieldVal, Option<Val>, deserialize_any, {
    fn visit_unit<E: de::Error>(self) -> Result<FieldVal, E> {
        Ok(FieldVal(None))
    }
    fn visit_str<E: de::Error>(self, s: &str) -> Result<FieldVal, E> {
        Ok(FieldVal(Some(Val::Str(cut(s.to_owned()).into()))))
    }
    fn visit_u64<E: de::Error>(self, n: u64) -> Result<FieldVal, E> {
        i64::try_from(n)
            .map(|n| FieldVal(Some(Val::Int(n))))
            .map_err(|_| no())
    }
    fn visit_i64<E: de::Error>(self, n: i64) -> Result<FieldVal, E> {
        Ok(FieldVal(Some(Val::Int(n))))
    }
    fn visit_bool<E: de::Error>(self, b: bool) -> Result<FieldVal, E> {
        Ok(FieldVal(Some(Val::Int(i64::from(b)))))
    }
});

// `[name, value]`.
typed!(Field, Option<(String, Val)>, deserialize_seq, {
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Field, A::Error> {
        let name = req(seq.next_element::<Str>()?)?.0;
        let value = req(seq.next_element::<FieldVal>()?)?.0;
        end(&mut seq)?;
        Ok(Field(value.map(|v| (name, v))))
    }
});

typed!(Fields, Vec<(String, Val)>, deserialize_any, {
    fn visit_unit<E: de::Error>(self) -> Result<Fields, E> {
        Ok(Fields(Vec::new()))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Fields, A::Error> {
        let mut out = Vec::new();
        while let Some(f) = seq.next_element::<Field>()? {
            out.extend(f.0);
        }
        Ok(Fields(out))
    }
});

typed!(Node, NodeIn, deserialize_map, {
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let (mut id, mut kind) = (None, None);
        let mut n = NodeIn {
            id: String::new(),
            kind: String::new(),
            parent: None,
            flags: 0,
            nav: Vec::new(),
            navs: Vec::new(),
            fields: Vec::new(),
            rows: Vec::new(),
            then: None,
        };
        const KEYS: &[&str] = &[
            "id", "kind", "parent", "flags", "nav", "navs", "fields", "rows", "then",
        ];
        entries(&mut map, KEYS, |i, m| {
            match i {
                0 => id = Some(m.next_value::<Str>()?.0),
                1 => kind = Some(m.next_value::<Str>()?.0),
                2 => n.parent = m.next_value::<OptStr>()?.0,
                3 => n.flags = m.next_value::<Flags>()?.0,
                4 => n.nav = m.next_value::<Links>()?.0,
                5 => n.navs = m.next_value::<Navs>()?.0,
                6 => n.fields = m.next_value::<Fields>()?.0,
                7 => n.rows = m.next_value::<Strs>()?.0,
                _ => n.then = m.next_value::<OptStr>()?.0,
            }
            Ok(())
        })?;
        n.id = req(id)?;
        n.kind = req(kind)?;
        Ok(Node(n))
    }
});

// A node list: null is `None`, for the op that refuses it.
typed!(Nodes, Option<Vec<NodeIn>>, deserialize_any, {
    fn visit_unit<E: de::Error>(self) -> Result<Nodes, E> {
        Ok(Nodes(None))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Nodes, A::Error> {
        let mut out = Vec::new();
        while let Some(x) = seq.next_element::<Node>()? {
            out.push(x.0);
        }
        Ok(Nodes(Some(out)))
    }
});

// `[from, to, kind]`.
typed!(Edge, EdgeIn, deserialize_seq, {
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Edge, A::Error> {
        let from = req(seq.next_element::<Str>()?)?.0;
        let to = req(seq.next_element::<Str>()?)?.0;
        let kind = req(seq.next_element::<Str>()?)?.0;
        end(&mut seq)?;
        Ok(Edge(EdgeIn { from, to, kind }))
    }
});

list_of!(Edges, Edge, EdgeIn);

typed!(EdgesOp, (Vec<EdgeIn>, Vec<EdgeIn>), deserialize_map, {
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<EdgesOp, A::Error> {
        let (mut add, mut remove) = (Vec::new(), Vec::new());
        entries(&mut map, &["add", "remove"], |i, m| {
            let e = m.next_value::<Edges>()?.0;
            if i == 0 {
                add = e
            } else {
                remove = e
            }
            Ok(())
        })?;
        Ok(EdgesOp((add, remove)))
    }
});

typed!(List, ListIn, deserialize_map, {
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<List, A::Error> {
        let (mut name, mut ids, mut total, mut from) = (None, Vec::new(), None, None);
        entries(&mut map, &["name", "ids", "total", "from"], |i, m| {
            match i {
                0 => name = Some(m.next_value::<Str>()?.0),
                1 => ids = m.next_value::<Strs>()?.0,
                2 => total = m.next_value::<Count>()?.0,
                _ => from = m.next_value::<Count>()?.0,
            }
            Ok(())
        })?;
        Ok(List(ListIn {
            name: req(name)?,
            ids,
            total,
            from: from.unwrap_or(0),
        }))
    }
});

list_of!(Lists, List, ListIn);

typed!(Form, FormIn, deserialize_map, {
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Form, A::Error> {
        let (mut name, mut list) = (None, None);
        entries(&mut map, &["name", "list"], |i, m| {
            match i {
                0 => name = Some(m.next_value::<Str>()?.0),
                _ => list = m.next_value::<OptStr>()?.0,
            }
            Ok(())
        })?;
        Ok(Form(FormIn {
            name: req(name)?,
            list,
        }))
    }
});

list_of!(Forms, Form, FormIn);

typed!(SchemeW, Scheme, deserialize_str, {
    fn visit_str<E: de::Error>(self, s: &str) -> Result<SchemeW, E> {
        match s {
            "links" => Ok(SchemeW(Scheme::Links)),
            "list" => Ok(SchemeW(Scheme::List)),
            "tree" => Ok(SchemeW(Scheme::Tree)),
            _ => Err(no()),
        }
    }
});

typed!(Desc, DescIn, deserialize_map, {
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Desc, A::Error> {
        let mut scheme = None;
        let mut d = DescIn {
            scheme: Scheme::List,
            chain_kinds: Vec::new(),
            match_fields: Vec::new(),
            lists: Vec::new(),
            classes: Vec::new(),
            flags: Vec::new(),
            forms: Vec::new(),
            start: None,
        };
        const KEYS: &[&str] = &[
            "scheme",
            "chain_kinds",
            "match_fields",
            "lists",
            "classes",
            "flags",
            "forms",
            "start",
        ];
        entries(&mut map, KEYS, |i, m| {
            match i {
                0 => scheme = Some(m.next_value::<SchemeW>()?.0),
                1 => d.chain_kinds = m.next_value::<Strs>()?.0,
                2 => d.match_fields = m.next_value::<Strs>()?.0,
                3 => d.lists = m.next_value::<Strs>()?.0,
                4 => d.classes = m.next_value::<Strs>()?.0,
                5 => d.flags = m.next_value::<Strs>()?.0,
                6 => d.forms = m.next_value::<Forms>()?.0,
                _ => d.start = m.next_value::<OptStr>()?.0,
            }
            Ok(())
        })?;
        d.scheme = req(scheme)?;
        Ok(Desc(d))
    }
});

typed!(Reset, ResetIn, deserialize_map, {
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Reset, A::Error> {
        let (mut desc, mut nodes, mut edges, mut lists) =
            (None, Vec::new(), Vec::new(), Vec::new());
        entries(
            &mut map,
            &["descriptor", "nodes", "edges", "lists"],
            |i, m| {
                match i {
                    0 => desc = Some(m.next_value::<Desc>()?.0),
                    1 => nodes = m.next_value::<Nodes>()?.0.unwrap_or_default(),
                    2 => edges = m.next_value::<Edges>()?.0,
                    _ => lists = m.next_value::<Lists>()?.0,
                }
                Ok(())
            },
        )?;
        Ok(Reset(ResetIn {
            desc: req(desc)?,
            nodes,
            edges,
            lists,
        }))
    }
});

typed!(Splice, Op, deserialize_map, {
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Splice, A::Error> {
        let (mut name, mut at, mut remove, mut ids, mut from) =
            (None, None, None, Vec::new(), None);
        entries(
            &mut map,
            &["name", "at", "remove", "ids", "from"],
            |i, m| {
                match i {
                    0 => name = Some(m.next_value::<Str>()?.0),
                    1 => at = m.next_value::<Count>()?.0,
                    2 => remove = m.next_value::<Count>()?.0,
                    3 => ids = m.next_value::<Strs>()?.0,
                    _ => from = m.next_value::<Count>()?.0,
                }
                Ok(())
            },
        )?;
        Ok(Splice(Op::Splice {
            name: req(name)?,
            at: at.unwrap_or(0),
            remove: remove.unwrap_or(0),
            ids,
            from,
        }))
    }
});

// `{name: body}`, one key.
typed!(OpW, Op, deserialize_map, {
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<OpW, A::Error> {
        const KEYS: &[&str] = &["reset", "nodes", "remove", "edges", "list", "splice"];
        let op = match req(map.next_key_seed(Key(KEYS))?)? {
            0 => Op::Reset(Box::new(map.next_value::<Reset>()?.0)),
            1 => Op::Nodes(req(map.next_value::<Nodes>()?.0)?),
            2 => Op::Remove(map.next_value::<Strs>()?.0),
            3 => {
                let (add, remove) = map.next_value::<EdgesOp>()?.0;
                Op::Edges { add, remove }
            }
            4 => Op::List(map.next_value::<List>()?.0),
            5 => map.next_value::<Splice>()?.0,
            _ => return Err(no()),
        };
        match map.next_key::<IgnoredAny>()? {
            None => Ok(OpW(op)),
            Some(_) => Err(no()),
        }
    }
});

typed!(Ops, Vec<Op>, deserialize_seq, {
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Ops, A::Error> {
        let mut out = Vec::new();
        while let Some(x) = seq.next_element::<OpW>()? {
            out.push(x.0);
        }
        Ok(Ops(out))
    }
});

typed!(Patch, Vec<Op>, deserialize_map, {
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Patch, A::Error> {
        let mut ops = None;
        entries(&mut map, &["ops"], |_, m| {
            ops = Some(m.next_value::<Ops>()?.0);
            Ok(())
        })?;
        Ok(Patch(req(ops)?))
    }
});

// The envelope's words: a plain string, a count, a flag (null: false).
typed!(Word, String, deserialize_str, {
    fn visit_str<E: de::Error>(self, s: &str) -> Result<Word, E> {
        Ok(Word(s.to_owned()))
    }
});

typed!(U64, u64, deserialize_u64, {
    fn visit_u64<E: de::Error>(self, n: u64) -> Result<U64, E> {
        Ok(U64(n))
    }
});

typed!(Flag, bool, deserialize_any, {
    fn visit_unit<E: de::Error>(self) -> Result<Flag, E> {
        Ok(Flag(false))
    }
    fn visit_bool<E: de::Error>(self, b: bool) -> Result<Flag, E> {
        Ok(Flag(b))
    }
});

typed!(Data, (Envelope, Vec<Op>), deserialize_map, {
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Data, A::Error> {
        let (mut base, mut hub, mut model, mut reset, mut version, mut view) =
            (None, None, None, false, None, None);
        entries(
            &mut map,
            &["base", "hub", "model", "reset", "version", "view"],
            |i, m| {
                match i {
                    0 => base = Some(m.next_value::<U64>()?.0),
                    1 => hub = Some(m.next_value::<U64>()?.0),
                    2 => model = Some(m.next_value::<Patch>()?.0),
                    3 => reset = m.next_value::<Flag>()?.0,
                    4 => version = Some(m.next_value::<U64>()?.0),
                    _ => view = Some(m.next_value::<Word>()?.0),
                }
                Ok(())
            },
        )?;
        let e = Envelope {
            view: req(view)?,
            hub: req(hub)?,
            base: req(base)?,
            version: req(version)?,
            reset,
        };
        Ok(Data((e, req(model)?)))
    }
});

typed!(Line, (Envelope, Vec<Op>), deserialize_map, {
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Line, A::Error> {
        let (mut event, mut data) = (false, None);
        entries(&mut map, &["event", "data"], |i, m| {
            match i {
                0 => event = m.next_value::<Word>()?.0 == "view-model",
                _ => data = Some(m.next_value::<Data>()?.0),
            }
            Ok(())
        })?;
        if !event {
            return Err(no());
        }
        Ok(Line(req(data)?))
    }
});
