//! The C ABI (`include/sja.h`): one entry point, `sja_call`, written with `sja_export!` (the only way to write one).
//!
//! C passes an op number and arguments (nil, integers, UTF-8 strings) and gets back an answer: a flat list of tokens
//! (nil, integer, string, and list, dotted list or vector headers with their element counts, prefix order) in a buffer
//! Sjá owns until the next call on the thread.  C builds the Lisp value from it; Sjá never sees a Lisp object and
//! never calls Emacs.  Statuses and directions are small integers; Lisp's `defsubst`s turn them into symbols.

use std::cell::RefCell;

use crate::guard::{Ran, Refused, Slot, on_view, with_table};
use crate::view::{Status, Step, View};
use crate::wire::Val;

pub const SJA_OK: i32 = 0;
pub const SJA_E_PANIC: i32 = -1;
pub const SJA_E_ARG: i32 = -2;
pub const SJA_E_NOMEM: i32 = -3;
pub const SJA_E_STATE: i32 = -4;

/// Argument and token tags.
pub const SJA_NIL: i32 = 0;
pub const SJA_INT: i32 = 1;
pub const SJA_STR: i32 = 2;
pub const SJA_LIST: i32 = 3;
pub const SJA_DOTTED: i32 = 4;
pub const SJA_VECTOR: i32 = 5;

/// The ops.
pub const SJA_APPLY: i32 = 1;
pub const SJA_OPEN: i32 = 2;
pub const SJA_CLOSE: i32 = 3;
pub const SJA_FOLLOW: i32 = 4;
pub const SJA_WALK: i32 = 5;
pub const SJA_WALK_TO: i32 = 6;
pub const SJA_SET: i32 = 7;
pub const SJA_GET: i32 = 8;
pub const SJA_HUB_LOST: i32 = 9;
pub const SJA_STATS: i32 = 10;
/// Feature `test-hooks` only: panic inside the named view's guard.
pub const SJA_TEST_PANIC: i32 = 99;

/// The ABI version the header describes.
pub const SJA_ABI_VERSION: i32 = 1;

/// Every exported C symbol; a test checks it against `include/sja.h`.
pub const EXPORTS: &[&str] = &["sja_call"];

/// One argument: `tag` `SJA_NIL`, `SJA_INT` (`int`) or `SJA_STR` (`ptr`, `len` bytes of UTF-8).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SjaArg {
    pub tag: i32,
    pub int: i64,
    pub ptr: *const u8,
    pub len: usize,
}

/// One answer token: a nil, an integer, a string (`ptr`, `len`), or the header of a list, dotted list or vector whose
/// `int` elements follow (a dotted list's last element is its tail).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SjaTok {
    pub tag: i32,
    pub int: i64,
    pub ptr: *const u8,
    pub len: usize,
}

/// An answer before it is flattened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ans {
    Nil,
    Int(i64),
    Str(String),
    List(Vec<Ans>),
    Dotted(Vec<Ans>),
    Vector(Vec<Ans>),
}

impl Ans {
    fn id(id: Option<crate::Id>) -> Ans {
        id.map_or(Ans::Nil, |i| Ans::Str(i.to_string()))
    }

    fn step(s: Step) -> Ans {
        match s.more {
            Some(d) => Ans::Dotted(vec![Ans::id(s.id), Ans::Int(i64::from(d))]),
            None => Ans::id(s.id),
        }
    }

    fn val(v: Option<&Val>) -> Ans {
        match v {
            Some(Val::Str(s)) => Ans::Str(s.to_string()),
            Some(Val::Int(i)) => Ans::Int(*i),
            None => Ans::Nil,
        }
    }

    fn ints(xs: &[usize]) -> Ans {
        Ans::List(xs.iter().map(|&x| Ans::Int(x as i64)).collect())
    }
}

/// An answer's tokens and the strings they point into.
type Buffer = (Vec<SjaTok>, Vec<Box<[u8]>>);

thread_local! {
    /// The last answer, valid until the next call on this thread.
    static ANSWER: RefCell<Buffer> = const { RefCell::new((Vec::new(), Vec::new())) };
}

fn flatten(a: &Ans, toks: &mut Vec<SjaTok>, strs: &mut Vec<Box<[u8]>>) {
    let tok = |tag, int| SjaTok {
        tag,
        int,
        ptr: std::ptr::null(),
        len: 0,
    };
    match a {
        Ans::Nil => toks.push(tok(SJA_NIL, 0)),
        Ans::Int(i) => toks.push(tok(SJA_INT, *i)),
        Ans::Str(s) => {
            let b: Box<[u8]> = s.as_bytes().into();
            toks.push(SjaTok {
                tag: SJA_STR,
                int: 0,
                ptr: b.as_ptr(),
                len: b.len(),
            });
            strs.push(b);
        }
        Ans::List(xs) | Ans::Dotted(xs) | Ans::Vector(xs) => {
            let tag = match a {
                Ans::List(_) => SJA_LIST,
                Ans::Dotted(_) => SJA_DOTTED,
                _ => SJA_VECTOR,
            };
            toks.push(tok(tag, xs.len() as i64));
            for x in xs {
                flatten(x, toks, strs);
            }
        }
    }
}

/// An argument, read.
enum Arg<'a> {
    Nil,
    Int(i64),
    Str(&'a str),
}

impl<'a> Arg<'a> {
    fn str(&self) -> Result<&'a str, Refused> {
        match self {
            Arg::Str(s) => Ok(s),
            _ => Err(Refused::Arg),
        }
    }
    fn opt_str(&self) -> Result<Option<&'a str>, Refused> {
        match self {
            Arg::Nil => Ok(None),
            Arg::Str(s) => Ok(Some(s)),
            Arg::Int(_) => Err(Refused::Arg),
        }
    }
    fn int(&self) -> Result<i64, Refused> {
        match self {
            Arg::Int(i) => Ok(*i),
            _ => Err(Refused::Arg),
        }
    }
    fn handle(&self) -> Result<u32, Refused> {
        u32::try_from(self.int()?).map_err(|_| Refused::Arg)
    }
}

/// Read C's arguments.
///
/// # Safety
/// `args` points to `n` arguments whose strings are `len` readable bytes.
unsafe fn read_args<'a>(args: *const SjaArg, n: usize) -> Result<Vec<Arg<'a>>, Refused> {
    if n == 0 {
        return Ok(Vec::new());
    }
    if args.is_null() || n > 16 {
        return Err(Refused::Arg);
    }
    // SAFETY: by the caller's promise.
    let raw = unsafe { std::slice::from_raw_parts(args, n) };
    raw.iter()
        .map(|a| match a.tag {
            SJA_NIL => Ok(Arg::Nil),
            SJA_INT => Ok(Arg::Int(a.int)),
            SJA_STR if a.len == 0 => Ok(Arg::Str("")),
            SJA_STR if !a.ptr.is_null() => {
                // SAFETY: by the caller's promise.
                let b = unsafe { std::slice::from_raw_parts(a.ptr, a.len) };
                std::str::from_utf8(b)
                    .map(Arg::Str)
                    .map_err(|_| Refused::Arg)
            }
            _ => Err(Refused::Arg),
        })
        .collect()
}

fn arg<'a, 'b>(args: &'b [Arg<'a>], i: usize) -> &'b Arg<'a> {
    args.get(i).unwrap_or(&Arg::Nil)
}

/// The answer of a guarded call on a view: its body's, or `on_resync`/`on_broken` (an apply answers its status, a
/// verb nil: Lisp reads the view's health when a verb answers nil).
fn ran(r: Ran<Ans>, on_resync: Ans, on_broken: Ans) -> Ans {
    match r {
        Ran::Done(a) => a,
        Ran::Resync => on_resync,
        Ran::Broken => on_broken,
    }
}

/// Run a verb on handle `h`'s view.
fn verb(h: u32, body: impl FnOnce(&mut View) -> Ans) -> Result<Ans, Refused> {
    let slot = with_table(|t| t.of(h))?.ok_or(Refused::Arg)?;
    Ok(ran(on_view(&slot, body), Ans::Nil, Ans::Nil))
}

fn health(slot: &Slot) -> Ans {
    if slot.broken() {
        return Ans::Int(2);
    }
    match slot.view.try_lock() {
        Ok(v) if v.model.is_none() => Ans::Int(1),
        Ok(v) if v.stale => Ans::Int(3),
        Ok(_) => Ans::Int(0),
        Err(_) => Ans::Int(1),
    }
}

fn get(h: u32, what: &str, a: &Arg, b: &Arg) -> Result<Ans, Refused> {
    if what == "health" {
        let slot = with_table(|t| t.of(h))?.ok_or(Refused::Arg)?;
        return Ok(health(&slot));
    }
    let (a, b) = (a.opt_str()?, b.opt_str()?);
    let what = what.to_string();
    verb(h, move |v| match what.as_str() {
        "selection" => Ans::id(v.selection(h)),
        "kind" => v.kind(h, a).map_or(Ans::Nil, |k| Ans::Str(k.into())),
        "field" => Ans::val(a.and_then(|f| v.field(h, f, b))),
        "flag" => Ans::Int(i64::from(a.is_some_and(|f| v.flag(h, f, b)))),
        "parent" => Ans::id(v.parent(h, a)),
        "rows" => Ans::List(
            v.rows(h, a)
                .into_iter()
                .map(|r| Ans::Str(r.into()))
                .collect(),
        ),
        "list" => Ans::List(
            a.map(|l| v.list(l))
                .unwrap_or_default()
                .into_iter()
                .map(|i| Ans::Str(i.to_string()))
                .collect(),
        ),
        "finished" => Ans::id(v.take_finished(h)),
        "class" => a
            .and_then(|id| v.class_of(h, id))
            .map_or(Ans::Nil, |k| Ans::Int(i64::from(k))),
        "classes" => Ans::Vector(
            v.classes(h)
                .into_iter()
                .flat_map(|(id, k)| {
                    [
                        Ans::Str(id.to_string()),
                        k.map_or(Ans::Nil, |k| Ans::Int(i64::from(k))),
                    ]
                })
                .collect(),
        ),
        "reclass" => {
            v.reclass(h);
            Ans::Nil
        }
        "class-names" => Ans::List(v.class_names().into_iter().map(Ans::Str).collect()),
        "expanded" => {
            Ans::Int(i64::from(a.is_some_and(|id| {
                v.inst(h).is_some_and(|i| i.expanded.contains(id))
            })))
        }
        "ticked" => {
            let mut t: Vec<String> = v
                .inst(h)
                .map(|i| i.ticked.iter().cloned().collect())
                .unwrap_or_default();
            t.sort();
            Ans::List(t.into_iter().map(Ans::Str).collect())
        }
        "version" => Ans::Int(v.version as i64),
        "stats" => {
            let c = v.counts();
            Ans::ints(&[
                c[0],
                c[1],
                c[2],
                c[3],
                c[4],
                v.version as usize,
                usize::from(v.stale),
            ])
        }
        _ => Ans::Nil,
    })
}

fn set(h: u32, what: &str, value: &Arg) -> Result<Ans, Refused> {
    let value = value.opt_str()?.map(str::to_string);
    let what = what.to_string();
    let known = ["select", "search", "form", "lit", "fold", "tick"];
    if !known.contains(&what.as_str()) {
        return Err(Refused::Arg);
    }
    verb(h, move |v| {
        let value = value.as_deref();
        match what.as_str() {
            "select" => Ans::id(v.select(h, value)),
            "search" => {
                v.set_search(h, value);
                Ans::Nil
            }
            "form" => {
                v.set_form(h, value);
                Ans::Nil
            }
            "lit" => {
                v.set_lit(h, value);
                Ans::Nil
            }
            _ => Ans::Int(i64::from(
                value.is_some_and(|id| v.toggle(h, id, what == "tick")),
            )),
        }
    })
}

/// One call, decoded (see `sja_call`).
fn call(op: i32, args: &[Arg]) -> Result<Ans, Refused> {
    match op {
        SJA_APPLY => {
            let (view, hub, version, text) = (
                arg(args, 0).str()?,
                arg(args, 1).str()?,
                arg(args, 2).int()?,
                arg(args, 3).str()?,
            );
            let version = u64::try_from(version).map_err(|_| Refused::Arg)?;
            let (slot, others) = with_table(|t| (t.slot(view), t.others(view)))?;
            let budget = crate::guard::TOTAL_BYTES.saturating_sub(others);
            let r = on_view(&slot, |v| {
                Ans::Int(v.apply(hub, version, text, budget) as i64)
            });
            Ok(ran(
                r,
                Ans::Int(Status::Resync as i64),
                Ans::Int(Status::Broken as i64),
            ))
        }
        SJA_OPEN => {
            let (view, group) = (arg(args, 0).str()?, arg(args, 1).opt_str()?);
            let (h, slot) = with_table(|t| t.open(view))?;
            // a broken or busy view still gets its handle: the instance opens when the view is whole again
            let _ = on_view(&slot, |v| v.open(h, group));
            Ok(Ans::Int(i64::from(h)))
        }
        SJA_CLOSE => {
            let h = arg(args, 0).handle()?;
            let (slot, name) = with_table(|t| (t.of(h), t.handles.remove(&h)))?;
            let (Some(slot), Some(name)) = (slot, name) else {
                return Err(Refused::Arg);
            };
            let empty = match slot.view.try_lock() {
                Ok(mut v) => v.close(h),
                Err(_) => false,
            };
            // the last instance gone: the view goes, and with it a tripped breaker
            let last = with_table(|t| !t.handles.values().any(|n| *n == name))?;
            if empty || last {
                with_table(|t| t.views.remove(&name))?;
            }
            Ok(Ans::Nil)
        }
        SJA_FOLLOW => {
            let (h, link) = (arg(args, 0).handle()?, arg(args, 1).str()?.to_string());
            verb(h, move |v| Ans::id(v.follow(h, &link)))
        }
        SJA_WALK | SJA_WALK_TO => {
            let (h, list, n) = (
                arg(args, 0).handle()?,
                arg(args, 1).str()?.to_string(),
                arg(args, 2).int()?,
            );
            verb(h, move |v| {
                Ans::step(if op == SJA_WALK {
                    v.walk(h, &list, n)
                } else {
                    match usize::try_from(n) {
                        Ok(n) => v.walk_to(h, &list, n),
                        Err(_) => Step {
                            id: v.selection(h),
                            more: None,
                        },
                    }
                })
            })
        }
        SJA_SET => set(arg(args, 0).handle()?, arg(args, 1).str()?, arg(args, 2)),
        SJA_GET => get(
            arg(args, 0).handle()?,
            arg(args, 1).str()?,
            arg(args, 2),
            arg(args, 3),
        ),
        SJA_HUB_LOST => {
            let slots: Vec<_> = with_table(|t| t.views.values().cloned().collect())?;
            for s in slots {
                let _ = on_view(&s, View::hub_lost);
            }
            Ok(Ans::Nil)
        }
        SJA_STATS => Ok(Ans::ints(&with_table(|t| t.stats())?)),
        #[cfg(feature = "test-hooks")]
        SJA_TEST_PANIC => {
            let view = arg(args, 0).str()?;
            let slot = with_table(|t| t.slot(view))?;
            let r: Ran<Ans> = on_view(&slot, |_| panic!("a test panic"));
            Ok(ran(
                r,
                Ans::Int(Status::Resync as i64),
                Ans::Int(Status::Broken as i64),
            ))
        }
        _ => Err(Refused::Arg),
    }
}

/// Run `op` on the decoded arguments and keep the flattened answer for C.
///
/// # Safety
/// As `sja_call`.
unsafe fn call_raw(
    op: i32,
    args: *const SjaArg,
    n: usize,
    out: *mut *const SjaTok,
    nout: *mut usize,
) -> i32 {
    if out.is_null() || nout.is_null() {
        return SJA_E_ARG;
    }
    // SAFETY: by `sja_call`'s contract.
    let answer = unsafe { read_args(args, n) }.and_then(|a| call(op, &a));
    match answer {
        Ok(a) => ANSWER.with(|b| {
            let mut b = b.borrow_mut();
            let (toks, strs) = &mut *b;
            toks.clear();
            strs.clear();
            flatten(&a, toks, strs);
            // SAFETY: `out` and `nout` are writable, by the contract; the tokens live until the next call.
            unsafe {
                *out = toks.as_ptr();
                *nout = toks.len();
            }
            SJA_OK
        }),
        Err(Refused::Arg) => SJA_E_ARG,
        Err(Refused::NoMem) => SJA_E_NOMEM,
        Err(Refused::State) => SJA_E_STATE,
    }
}

/// Write an entry point.  Every `extern "C"` function of Sjá comes from here (the glue's `tests/ffi.rs` checks the
/// source), so each one runs in `catching` past its own guards; each needs a doc comment with a `# Safety` section.
macro_rules! sja_export {
    ($(#[$attr:meta])+
     fn $name:ident($($arg:ident: $ty:ty),* $(,)?) -> i32 $body:block) => {
        $(#[$attr])+
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name($($arg: $ty),*) -> i32 {
            $crate::guard::catching(|| $body).unwrap_or(SJA_E_PANIC)
        }
    };
}

sja_export! {
    /// One call into Sjá: `op` (`SJA_APPLY` …) on `n` arguments; on `SJA_OK` the answer's `*nout` tokens are at `*out`,
    /// valid until the next call on this thread.  A negative `SJA_E_*` code otherwise, with no answer.
    ///
    /// # Safety
    /// `args` points to `n` readable arguments (null when `n` is 0), each string `len` readable bytes; `out` and `nout`
    /// are writable.  Called on the thread that holds Emacs's global lock.
    fn sja_call(op: i32, args: *const SjaArg, n: usize, out: *mut *const SjaTok, nout: *mut usize) -> i32 {
        // SAFETY: by this entry point's contract.
        unsafe { call_raw(op, args, n, out, nout) }
    }
}

/// Run one call from Rust (tests and the fuzz target): the answer, or the code.
pub fn call_for_test(op: i32, args: &[SjaArg]) -> Result<Ans, i32> {
    let mut out: *const SjaTok = std::ptr::null();
    let mut n = 0usize;
    // SAFETY: `args` is a live slice; `out` and `n` are writable locals.
    let rc = unsafe { sja_call(op, args.as_ptr(), args.len(), &mut out, &mut n) };
    if rc != SJA_OK {
        return Err(rc);
    }
    // SAFETY: on SJA_OK `out` holds `n` tokens until the next call.
    let toks = unsafe { std::slice::from_raw_parts(out, n) };
    let mut at = 0;
    Ok(unflatten(toks, &mut at))
}

fn unflatten(toks: &[SjaTok], at: &mut usize) -> Ans {
    let t = toks[*at];
    *at += 1;
    match t.tag {
        SJA_INT => Ans::Int(t.int),
        SJA_STR => {
            // SAFETY: a string token points to `len` bytes of the answer's buffer.
            let b = unsafe { std::slice::from_raw_parts(t.ptr, t.len) };
            Ans::Str(String::from_utf8_lossy(b).into_owned())
        }
        SJA_LIST | SJA_DOTTED | SJA_VECTOR => {
            let xs = (0..t.int).map(|_| unflatten(toks, at)).collect();
            match t.tag {
                SJA_LIST => Ans::List(xs),
                SJA_DOTTED => Ans::Dotted(xs),
                _ => Ans::Vector(xs),
            }
        }
        _ => Ans::Nil,
    }
}

/// An integer argument.
pub fn int(i: i64) -> SjaArg {
    SjaArg {
        tag: SJA_INT,
        int: i,
        ptr: std::ptr::null(),
        len: 0,
    }
}

/// A string argument borrowing `s` (keep `s` alive across the call).
pub fn text(s: &str) -> SjaArg {
    SjaArg {
        tag: SJA_STR,
        int: 0,
        ptr: s.as_ptr(),
        len: s.len(),
    }
}

/// A nil argument.
pub fn nil() -> SjaArg {
    SjaArg {
        tag: SJA_NIL,
        int: 0,
        ptr: std::ptr::null(),
        len: 0,
    }
}
