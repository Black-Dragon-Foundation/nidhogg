# sja

Sjá ("sight") is Nidhogg's native view model: a small Rust library that holds, inside the editor's process, the model
of each hub-fed view as it is drawn, plus each instance's user state. A key that moves, opens, filters or searches
answers from it in microseconds, with no round trip to the hub and no Lisp data built from the model.

The hub stays the only source of data and of navigation rules. It sends each view's model as a versioned JSON patch,
including every node's navigation links and every order as a list. Sjá follows the links and walks the lists; the one
rule it computes is generic: the chain class (reachability over the descriptor's `chain_kinds`) and search matches.
Sjá names no view, no kind and no field: every word in a model is the hub's.

## Crate rules

- Standard library, plus two locked dependencies (`Cargo.lock`; builds run `--locked`), vendored at release with
  Glitnir's crates and licence-checked (`deny.toml`):
  - `serde_json =1.0.154` (no default features but `std`): the wire is JSON, which the hub writes. It is decoded
    through `serde_json::Value` by hand (`src/wire.rs`), so there is no `serde_derive` and no proc-macro build. It
    brings `serde_core`, `itoa`, `memchr` and `zmij`; `serde`, `serde_derive`, `syn`, `proc-macro2`, `quote` and
    `unicode-ident` are locked (and vendored) as optional dependencies but not built.
  - `libc 0.2`: the guard's stack headroom check (`pthread_getattr_np`).
- No threads, no I/O, no clock of its own: it runs only on the Lisp thread that holds Emacs's global lock.
- Builds on Rust 1.93 (the glue's floor), Linux, macOS and Android (`check.sh --android`).

## The model

A view's model is a **descriptor**, **nodes**, **edges** and **lists**:

```text
Descriptor = {scheme: "links" | "list" | "tree", chain_kinds, match_fields, lists, classes, flags,
              forms: [{name, list?}], start?}
Node       = {id, kind, parent?, flags: u32, nav: {link: id}, navs: {form: {link: id}},
              fields: [[name, string | int]], rows: [row key], then?}
Edge       = [from, to, kind]
List       = {name, ids, total?, from?}          ; windowed when total is set
```

A patch is `{"ops": [...]}` with ops `reset`, `nodes` (insert or replace), `remove`, `edges {add, remove}`, `list` and
`splice`. Removing a node leaves its edges: the hub removes them (an edge to a missing node is never followed).

**Apply** checks a whole patch before changing anything: it decodes every op, projects the caps over the batch and
refuses a splice outside its list. A refused patch leaves the model, the selections and the classes as they were.
It answers `applied`, `dropped` (not newer than what is drawn, from the same hub), `resync` (malformed, before the first
reset, a new hub without a reset), `too-large` or `broken`.

**Caps** per view: 200,000 nodes, 400,000 edges, 64 lists, 20,000 entries in an unwindowed list, field values cut at
4 KiB, 65,536 vocabulary words; and a byte budget (the process-wide 32 MiB less the other views'), counted by an
estimate of every node, edge and list entry.

## Instances and verbs

An **instance** is one buffer's view of one view: a selection (an id, plus its index in the primary list as a
fallback), a form, a search, a lit list, expanded and ticked sets, and the classes it last reported. Instances in a
**group** share one selection.

- `follow LINK`: the link of the instance's form (the default set when the node has none for it). In a list form
  (`{name, list}`) `up` and `down` step along the form's list. A null link, or one to a node the model doesn't hold,
  leaves the selection. With no selection, the first entry of the primary list (`start`, else the first list).
- `walk LIST STEP` (wrapping) and `walk-to LIST N`; a windowed list stops at its window's edge and answers `more`
  with a direction.
- `select`, `search`, `form`, `lit`, `fold`, `tick`; reads of the selection, a node's kind, fields, flags, parent and
  rows, a list, the descriptor's class names.
- **The tree scheme:** the primary list holds the whole tree, parents before their children; a node is open when the
  model's flag `expanded` is set, flipped by the instance's expanded set (`fold` toggles it; `open` reads it). The
  list `visible` is the instance's own: the primary list's entries whose ancestors are all open. `follow` `up`/`down`
  steps along it (no wrap), `up:KIND`/`down:KIND` goes to the nearest visible node of that kind, `left` to the parent,
  `right` to the first child when open. `prefix TEXT` (type-to-navigate) selects the visible sibling of the
  selection (none: the top level) whose `label` starts with TEXT, case folded, from the one after the selection,
  wrapping (a longer TEXT from the selection itself); no match leaves it.
- **Removal:** when the selected node goes, the selection moves to the removed node's `then` if the model holds it,
  else (list scheme) to the primary list's entry at its old index, else to none; `finished` records the id that went.
  A reset re-resolves selections by id, with the `then` of the model before it.

## Classes

For an instance, in order: `selected`; while a search is set, `match` or `dim`; else `lit` for the lit list's nodes;
else, while there is a selection, `chain` or `dim`; else none. A class is its position in the descriptor's `classes`,
which is the index into the face vector Lisp keeps. `classes` answers only the nodes whose class changed since the
last call.

## The C door

One entry point, `sja_call(op, args, n, out, nout)` (`include/sja.h`), written with `sja_export!` (`src/abi.rs`), the
only way to write one. Arguments are nil, integers or UTF-8 strings; the answer is a flat token list (nil, integer,
string, and list, dotted list or vector headers with their counts) that Sjá owns until the next call on the thread.
C (`src/nidhogg-views.c`) builds the Lisp value; Sjá never sees a Lisp object, never calls Emacs and interns nothing.

| Op | Arguments | Answer |
|---|---|---|
| `SJA_APPLY` | view, hub, version, patch | status: 0 applied, 1 dropped, 2 resync, 3 too-large, 4 broken |
| `SJA_OPEN` | view, group or nil | a handle |
| `SJA_CLOSE` | handle | nil |
| `SJA_FOLLOW` | handle, link | the selection or nil |
| `SJA_WALK`, `SJA_WALK_TO` | handle, list, step or position | the selection or nil; `(ID . DIRECTION)` when a window must move |
| `SJA_SET` | handle, `select`/`search`/`form`/`lit`/`fold`/`tick`/`prefix`, value | the selection (select, prefix), 0 or 1 (fold, tick), else nil |
| `SJA_GET` | handle, what, arg, arg | `selection`, `kind`, `field`, `flag`, `parent`, `rows`, `list` (a tree's `visible` is the instance's), `finished`, `class`, `classes` (a vector `[ID CLASS …]` of changes), `reclass`, `class-names`, `expanded`, `open`, `ticked`, `version`, `stats`, `health` (0 ok, 1 no model, 2 broken, 3 stale) |
| `SJA_HUB_LOST` | | nil |
| `SJA_STATS` | | views, instances, bytes, panics, broken views |

Codes: `SJA_E_ARG` (an unknown op, handle, tag or type, bad UTF-8), `SJA_E_NOMEM` (too little stack), `SJA_E_STATE`
(the table was held), `SJA_E_PANIC`.

## The guard and the breaker

Every call checks the stack headroom (256 KiB, the glue's figure), takes the process-wide table (view name → view,
handle → view) with `try_lock`, then the one view it touches with `try_lock`, and runs in `catch_unwind`. A held view
lock (a call a `longjmp` abandoned) answers as a resync; nothing blocks. A panic drops only that view's model, clears
the poison, keeps its instances with their handles and selection ids (the next reset re-resolves them) and answers
`resync`. Two panics on one view within 60 s trip its **breaker**: every call on it answers `broken` until its last
instance closes. `SJA_STATS` counts panics and broken views; drgnx's doctor fails while a view is broken.

Feature `test-hooks` adds `SJA_TEST_PANIC` (a panic inside a view's guard) and the breaker's test clock.

`../sja-fuzz` is a `cargo-fuzz` target (`apply_bytes`: arbitrary bytes as a patch, then the verbs), nightly only:
`cargo +nightly fuzz run apply_bytes` there. It is not part of `check.sh`.

## Checks

`check.sh [--container fedora-45] [--test NAME] [--android]`: fmt, clippy `-D warnings` (with and without
`test-hooks`), the tests, the licence policy. The MSRV: the same checks under a 1.93 toolchain.
