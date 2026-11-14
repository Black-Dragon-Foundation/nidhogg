# sja

Sjá ("sight") is Nidhogg's native view model: a small Rust library that holds, inside the editor's process, the model
of each hub-fed view as it is drawn, plus each instance's user state. A key that moves, opens, filters or searches
answers from it in microseconds, with no round trip to the hub and no Lisp data built from the model.

The hub stays the only source of data and of navigation rules. It sends each view's model as a versioned JSON patch,
including every node's navigation links and every order as a list. Sjá follows the links and walks the lists; the one
rule it computes is generic: the chain class (reachability over the descriptor's `chain_kinds`) and search matches.
Sjá names no view, no kind and no field: every word in a model is the hub's.

## Crate rules

- Standard library, plus three locked dependencies (`Cargo.lock`; builds run `--locked`), vendored at release with
  Glitnir's crates and licence-checked (`deny.toml`):
  - `serde_json =1.0.154` (no default features but `std`): the wire is JSON, which the hub writes. It brings
    `serde_core`, `itoa`, `memchr` and `zmij`; `serde`, `serde_derive`, `syn`, `proc-macro2`, `quote` and
    `unicode-ident` are locked (and vendored) as optional dependencies but not built.
  - `serde_core =1.0.229` (`std`), the one serde_json brings: its traits, for the typed decode's hand-written visitors
    (`src/typed.rs`), so there is no `serde_derive` and no proc-macro build. A `view-model` line is read straight into
    ops; what the typed read refuses (a malformed model, a duplicate key, a float where a count goes) is read again
    through `serde_json::Value` (`src/wire.rs`, the reference), whose answer and error stand. The typed read takes
    6.2 ms where the `Value` read takes 16.9 ms, on a 10k-node model of 3.9 MB (2.7×; `tests/decode_bench.rs`).
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
`splice {name, at, remove, ids, from?}`. A splice grows or shrinks a window's total by what it adds less what it
removes; one that names `from` moves the window over the same list instead (its first position becomes `from`, its
total stays), so a hub moves a long window by the ids that change, not the window whole. Removing a node leaves its
edges: the hub removes them (an edge to a missing node is never followed).

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
  leaves the selection. With no selection, the first entry of the primary list (`start`, else the first list). A
  follow that leaves the selection answers `more` when its link names a direction (`up`, `down`, also as
  `up:KIND`/`down:KIND`; `left` and `right` too outside the tree scheme) and the primary list is a window that goes on
  that way: the row it wants may be in the next window.
- `walk LIST STEP` (wrapping) and `walk-to LIST N`; a windowed list stops at its window's edge and answers `more`
  with a direction.
- `select`, `search`, `form`, `lit`, `fold`, `tick`; reads of the selection, a node's kind, fields, flags, parent and
  rows, a list, a list's `window` (`(FROM LENGTH TOTAL)` when the model holds a window of it: where the editor asks
  the hub for the next one), the descriptor's class names.
- **The tree scheme:** the primary list holds the whole tree, parents before their children; a node is open when the
  model's flag `expanded` is set, flipped by the instance's expanded set (`fold` toggles it; `open` reads it). The
  list `visible` is the instance's own: the primary list's entries whose ancestors are all open. `follow` `up`/`down`
  steps along it (no wrap), `up:KIND`/`down:KIND` goes to the nearest visible node of that kind, `left` to the parent,
  `right` to the first child when open. `prefix TEXT` (type-to-navigate) selects the visible sibling of the
  selection (none: the top level) whose `label` starts with TEXT, case folded, from the one after the selection,
  wrapping (a longer TEXT from the selection itself); no match leaves it.
- **History:** `follow back` and `follow forward` are reserved links (a model's own links of those names are never
  read): they retrace the instance's moves, like a browser's. A jump (`select`, a link that is no direction) is one
  move; a run of steps (the directions, `walk`, `walk-to`, `prefix`) is one move too, recorded where the run began.
  A new move drops what was ahead; `back` skips ids the model no longer holds; each ring keeps `HISTORY` (50) moves.
  Each instance has its own history, also in a group.
- **Removal:** when the selected node goes, the selection moves to the removed node's `then` if the model holds it,
  else (list scheme) to the primary list's entry at its old index, else to none; `finished` records the id that went.
  A reset re-resolves selections by id, with the `then` of the model before it.

## Classes

For an instance, in order: `selected`; `ticked` for the nodes of the instance's ticked set, when the descriptor names
that class (the Explorer's marks; Search's ticks name none); while a search is set, `match` or `dim`; else `lit` for the
lit list's nodes; else, while there is a selection in a view with `chain_kinds` or a lit list, `chain` or `dim`; else
none. A view with neither (the Run and Debug view, the Explorer) has no chain to dim off: dimming every node but the
selection only cost the editor a face on every row. A class is its position in the descriptor's `classes`, which is the
index into the face vector Lisp keeps. `classes` answers only the nodes whose class changed since the last call; when
only the selection and the ticked nodes can have a class (no search, no lit list, no chain kinds) it looks at those,
not at every node, and a call with the model, the selection, the search, the lit list and the ticks as they were at the
last one runs no pass at all (the editor asks twice per key: the key's step and the draw's).

## The C door

One entry point, `sja_call(op, args, n, out, nout)` (`include/sja.h`), written with `sja_export!` (`src/abi.rs`), the
only way to write one. Arguments are nil, integers or UTF-8 strings; the answer is a flat token list (nil, integer,
string, and list, dotted list or vector headers with their counts) that Sjá owns until the next call on the thread.
C (`src/nidhogg-views.c`) builds the Lisp value; Sjá never sees a Lisp object, never calls Emacs and interns nothing.

| Op | Arguments | Answer |
|---|---|---|
| `SJA_OPEN` | view, group or nil | a handle |
| `SJA_CLOSE` | handle | nil |
| `SJA_FOLLOW` | handle, link | the selection or nil |
| `SJA_WALK`, `SJA_WALK_TO` | handle, list, step or position | the selection or nil; `(ID . DIRECTION)` when a window must move |
| `SJA_SET` | handle, `select`/`search`/`form`/`lit`/`fold`/`tick`/`prefix`, value | the selection (select, prefix), 0 or 1 (fold, tick), else nil |
| `SJA_GET` | handle, what, arg, arg | `selection`, `kind`, `field`, `flag`, `parent`, `rows`, `list` (a tree's `visible` is the instance's), `finished`, `class`, `classes` (a vector `[ID CLASS …]` of changes), `reclass`, `class-names`, `expanded`, `open`, `ticked`, `version`, `stats`, `health` (0 ok, 1 no model, 2 broken, 3 stale) |
| `SJA_HUB_LOST` | | nil |
| `SJA_STATS` | | views, instances, bytes, panics, broken views |
| `SJA_PUSH` | a whole `view-model` line | `(VIEW VERSION DECISION)`: 0 queue, 1 reset, 2 drop, 3 resync, 4 none (no instance of the view); nil when the line is not a `view-model` |
|  | a `view-model` line whose data holds `rows` | `(VIEW VERSION DECISION CLASS)`: 0 apply, 1 drop, 2 hold, 3 resync, 4 quiet (a resync outstanding), 5 broken, 6 paused; nil when no instance holds rows |
| `SJA_DRAIN` | view, version or nil | `(APPLIED LAST VERSION)`: the models applied (up to the version, else all), the last status (0 applied, 1 dropped, 2 resync, 3 too-large, 4 broken), the stream's newest version or nil |
| `SJA_NAMED` | view, version | nil: a row patch named that model version |
| `SJA_DUE` | view | as `SJA_DRAIN` (all), when the oldest queued model is one no row patch named; else nil |
| `SJA_ROWS` | handle, verb, int or nil, string or nil | the view's row store: `on`, `draw` (the edits, see Rows), `drawn`, `epoch`, `place`, `pos`, `span`, `prop`, `pause`, `redraw`, `refresh`, `forget`, `state` (`nidhogg-view-rows` says each) |
| `SJA_BAR` | a bar's spec (a line of integers: the room, the pads, the separators, the ellipsis and each segment's drop rank and width, the cuttable one's character widths) | `(LEFT RIGHT CUT RIGHT-WIDTH)`: the segments that fit and where the cuttable one is cut (`bar.rs`, `nidhogg-view-bar`); no view, no state |
| `SJA_PICK` | `"load"` and a list's text (a `KIND RECENT LABEL` line per candidate); `"top"`, handle, input, N; `"close"`, handle | a local list (`pick.rs`, `nidhogg-view-pick`, the palette): `load` answers a handle (at most 16 kept, the oldest goes), `top` the indices of the first N candidates for the input (filtered, scored, sorted, grouped here), `close` nil |

Codes: `SJA_E_ARG` (an unknown op, handle, tag or type, bad UTF-8), `SJA_E_NOMEM` (too little stack), `SJA_E_STATE`
(the table was held), `SJA_E_PANIC`.

## The queue

The hub's `view-model` line reaches Sjá whole (`SJA_PUSH`), so Lisp never parses a model: `{"event": "view-model",
"data": {view, hub, base, version, reset?, model: {"ops": [...]}}}`. Sjá reads the envelope and decides by it, per view:

- a reset from another hub, or with base 0 (a new subscription), starts a new stream: the queue goes and the stream
  gets a new generation, so the model takes its version 1 as new (the model is told the hub as `HUB/GEN`);
- a reset not newer than the stream is dropped, a newer one replaces the queue;
- a diff from another hub (or before any reset), or of an older base, is dropped; of a base past the stream's version it
  is a gap: `resync`, and the editor asks the hub for a reset; of the stream's version it is queued.

A queued model waits for the draw: the editor drains up to the version its drawn rows name (`SJA_DRAIN`), and a timer
drains models no row patch names (`SJA_NAMED`, `SJA_DUE`). Past 64 models or 16 MiB of lines a view's queue and stream
are dropped and the push answers `resync`; a model that does not apply at the drain does the same. A line for a view no
instance shows makes no slot. The hub lost, a panic and the view's last instance closing drop the queue.

## Rows (`src/rows.rs`)

A hub-fed view's rows can be held here too, where they are drawn (drgnx's rows-in-Sjá design §6, the terminal first).
A **store** takes the hub's row patches (`{view, class, hub, base, version, env_epoch, ops, images?, styles?, term?,
model?}`, ops `reset`, `insert {after, rows}`, `replace`, `remove`) and decides each by the editor's rules: another
hub's diff, or one before any reset, is dropped; so is an old reset; a patch of an older env epoch is held until a reset;
a diff of an older base is dropped, of a newer one (a gap) or naming an image or style the store lacks, it asks for a
resync. A resync is asked once while outstanding, with a back-off of 1 s then 4 s; three within 10 s break the store.
A paused store keeps up to 256 patches and applies them on resume (past that, a resync). The patches come on the
hub's `view-model` line, the patch whole under `data.rows`, through `SJA_PUSH`; an instance gives its view a store with
`SJA_ROWS on`, and the store's verbs are `SJA_ROWS`'s (`nidhogg-view-rows` in Lisp).

**The draw.** The store keeps the drawn state (each drawn row's key, length in characters and content hash). `draw`
answers the edits, in buffer order, that turn the region into the rows: an offset (characters from the region's
start, the buffer as it stands at that edit), a count to delete and the rows to insert. Rows kept in the same order
are untouched; a reorder redraws the rows that moved. The new drawn state is pending until the editor confirms the
batch (`drawn`). A batch never confirmed (a quit, a panic) rolls back: the next draw erases the region and draws it
whole, as does a region whose length is not the drawn state's. `place POS` and `pos KEY OFF` turn a window's start and
point into a row and an offset and back, so windows keep their place across a draw.

Caps: 200,000 rows, 64 KiB of text and 4,096 runs a row, 256 images (the ones no row names are evicted first), 64 held
patches.

## The guard and the breaker

Every call checks the stack headroom (256 KiB, the glue's figure), takes the process-wide table (view name → view,
handle → view) with `try_lock`, then the one view it touches with `try_lock`, and runs in `catch_unwind`. A held view
lock (a call a `longjmp` abandoned) answers as a resync; nothing blocks. A panic drops only that view's model, clears
the poison, keeps its instances with their handles and selection ids (the next reset re-resolves them) and answers
`resync`. Two panics on one view within 60 s trip its **breaker**: every call on it answers `broken` until its last
instance closes. `SJA_STATS` counts panics and broken views; drgnx's doctor fails while a view is broken.

Feature `test-hooks` adds `SJA_TEST_PANIC` (a panic inside a view's guard) and the breaker's test clock.

`../sja-fuzz` has `cargo-fuzz` targets (`push_bytes`: arbitrary bytes as a `view-model` line's model, pushed and drained,
then the verbs; `typed_bytes`: whatever the typed read reads, the `Value` read reads the same), nightly only:
`cargo +nightly fuzz run push_bytes` there. It is not part of `check.sh`.

## Checks

`check.sh [--container fedora-45] [--test NAME] [--android]`: fmt, clippy `-D warnings` (with and without
`test-hooks`), the tests, the licence policy. The MSRV: the same checks under a 1.93 toolchain.
