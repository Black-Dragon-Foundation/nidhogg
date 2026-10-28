# nhglue

The glue between Emacs and Nidhogg's native shell, in Rust: the shell's state and every decision that needs no live
Emacs state. The C side (`src/nhwl*.c`) keeps what touches Emacs (Lisp objects, Emacs structs, the keyboard queue,
the hook tables, `DEFUN`s) and all Cairo drawing, and calls in here through `include/nhglue.h`.

- **One library.** Under `--with-nidhogg-wayland` configure links `libnhglue.a` instead of `libglitnir.a`. It is the
  one Rust static library in the binary (two would collide on std's symbols), so `src/lib.rs` re-exports Glitnir's
  and the shell's C ABIs: `nidhogg-glitnir.c` and the C calls not yet ported keep linking. `nhshell` is an rlib.
- **Calls go one way.** C calls `nhg_*`; the glue answers with plain records (ids, rectangles, ARGB colours, UTF-8
  text, pixel pointers into Rust-owned backings) and never calls Emacs. It never sees a `Lisp_Object` or an Emacs
  struct, and C passes it no function pointers.
- **Threads.** The glue runs on the Lisp thread that holds Emacs's global lock, never on the shell's reader or the
  presenter thread.
- **Dependencies:** `nhshell`, `glitnir` and `libc`, built offline and locked from the shell's `vendor/`
  (`.cargo/config.toml`). `Cargo.lock` is the shell's plus the `nhglue` package; after the shell revendors, copy its
  lock here and add the package back (`check.sh` fails until they agree).
- **Features:** `wayland` (default) turns on the shell's Wayland backend; `test-hooks` adds entry points that exist
  only for tests. Configure passes `--features test-hooks` (through `GLITNIR_CARGO_FLAGS`) and turns on Rust's debug
  assertions (`CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS`) under `--enable-checking`.

## Checks

- `check.sh [--container DISTRO] [--test NAME]`: the lock against the shell's, fmt, clippy `-D warnings` (also with
  `test-hooks`), tests and cargo-deny. `--no-run` builds the tests without running them. `--full` adds the release archive and checks that it exports every function of
  `nhglue.h`, `nhshell.h` and `glitnir.h`. `--android` runs `cargo check --no-default-features --target
  aarch64-linux-android` (no NDK, no link) and exits 3 when the toolchain has no std for that target.
- `tests/abi.rs` (`abi::header_matches_records`) compiles `nhglue.h` with `cc` and compares its constants and
  prototypes with the Rust side.
- `nm-check.sh BUILD_DIR`, on a `--nidhogg-wayland` build: the archive's undefined symbols minus the ones it defines
  itself must not include any symbol Emacs's own `src/*.o` define (Rust never calls Emacs), and every `nhg_*` call in
  `src/nhwl*.c` goes through the `NHG (…)` macro. Calls through function pointers are invisible to it; there are none.

## Debugging

The release profile keeps full debug info (`debug = 2`), so gdb shows the glue's locals as well as the C side's.
`rust-gdb` is gdb with Rust's pretty-printers loaded; point it at the dumped binary:

```sh
rust-gdb --args src/emacs -Q            # in the build tree; or: rust-gdb -p PID
(gdb) break rust_panic                  # stops at any panic, before the boundary catches it
(gdb) set print pretty on
```

Without `rust-gdb`, load the printers by hand: `gdb -iex "add-auto-load-safe-path $(rustc --print sysroot)/lib/rustlib/etc"
-x $(rustc --print sysroot)/lib/rustlib/etc/gdb_load_rust_pretty_printers.py`. Emacs's own `src/.gdbinit` still
applies to the C frames.

A panic inside the glue is caught at the entry point and reported as `NHG_E_PANIC`; the display it happened on is
dropped and Emacs keeps running. `(nhwl-glue-panics)` returns the reports of the dropped displays, newest first: each
panic's message, location and backtrace, which also go to stderr when the display is dropped.

## The guard

Every entry point is written with `nhg_export!` (`src/guard.rs`), which runs its body in `guard` (or `unlocked`, for
the calls that take no display lock); `tests/ffi.rs` (`ffi::every_extern_comes_from_nhg_export`) fails on any
`extern "C"` function written by hand. The guard, in order:

1. **Stack headroom.** With less than 256 KiB of stack left (`pthread_getattr_np`, read once per thread) the call
   returns `NHG_E_NOMEM` and nothing is poisoned: a stack probe near the end of the stack would land in Emacs's
   SIGSEGV handler, which `siglongjmp`s.
2. **`try_lock`** on the display's state. A held lock means a call re-entered, or a `longjmp` abandoned one: the
   display is poisoned instead of hanging Emacs.
3. **`catch_unwind`.** A panic poisons the display and keeps its first report; the backtrace is captured at once and
   resolved when the report is read, so the call returns at once.

A poisoned display answers `NHG_E_PANIC` to everything but `nhg_panic_message` and `nhg_close`. C sets
`glue_broken`, and the next `read_socket_hook` drops the display as if the compositor had gone. `nhg_close` then
touches nothing behind the lock: it shuts the connection's socket down (the compositor destroys our windows), stops
the shell's threads (`ShellBackend::abandon`), waits at most 100 ms for each and leaks the rest.

C makes every call through `NHG (…)` (`src/nhwlterm.h`), which clears `waiting_for_input` around it, so that SIGINT's
`longjmp` to `read_char` never crosses a Rust frame; `nm-check.sh` fails on a call outside it. The tests and the
`--full` archive build with the feature `test-hooks`.

## The event step

`read_socket_hook` (`nhwl_drain_events`) loops on `nhg_next`, which takes exactly one shell event and answers with its
actions (`src/action.rs`, `src/drain.rs`). C applies them (the Emacs calls) before it asks again, so every decision
sees what the earlier events changed, as the C drain did; before each call it samples `NHG_IN_*` again
(`kbd_buffer_events_waiting ()`, `mwheel-coalesce-scroll-events`). The actions and their texts live until the next
`nhg_next`, so the calls C makes while applying them cannot free them. Kinds 1–31 are the port's, 32–63 W2's and W3's.

- **Configure** (`src/configure.rs`): a toplevel's configure gives a `RESIZE` action and an ack. The ack is a content
  ack for the window's first configure, for a change of activated, maximized or tiled under a client-side title bar,
  and when applying `RESIZE` changes the frame's size: C pushes the new size (`nhg_frame_resized`) and the ack is
  recorded then. Otherwise it is recorded at the start of the next `nhg_next`.
- **Scale:** a `SCALE` action when the window's preferred scale differs from the one C last applied.
- **Pushed facts** are keyed by the shell window, as the events are, and C seeds them when it makes a window: the
  native size, the scale and whether the frame draws its own title bar.
- **Keys** (`src/keys.rs`): a press or a repeat gives a `KEY` action with the keysym, its text and the modifier mask;
  the shell has already run it through xkb and compose. It names the window with the keyboard, and C sends it to the
  frame of that window's tree that Emacs focused, after running the keysym cascade. A repeat taken by a call with
  `NHG_IN_INPUT_WAITING` set is dropped, so a key stored earlier in the same drain counts. The glue keeps the modifier
  mask (`MODIFIERS` events give no action).
- **Focus** (`src/focus.rs`): a keyboard focus event gives `FOCUS_IN` or `FOCUS_OUT` for its window, and the glue
  keeps the window that has the keyboard (`nhg_keyboard_window`). Emacs's focus frame and highlighting stay C.
- **Transitional `NHG_ACT_RAW`** carries each event whose kind is not ported yet; C handles it on its old path
  (`nhg_raw_event` gives the event), a `PREEDIT`'s or `COMMIT_TEXT`'s text comes as the action's text, and `mods` is
  the keyboard's modifier mask. It is its event's last action (a title bar's may come before it). Each port task takes
  its kinds out.
- An event naming a forgotten window gives no action, except keys and the input method's, which C sends to the focus
  frame.
- **A tip's dismissal** (`POPUP_DONE` of a tip's window) gives `DELETE` with `NHG_DEL_TIP`: C calls `nhg_tip_hide` and
  hides the frame, which Lisp deletes later. A popup frame's `POPUP_DONE` is still raw.

## The title bar

The client-side title bar (`src/csd.rs`, from `nhwlcsd.c`) is laid out, hit-tested and kept here; C paints it.

- **Layout:** C calls `nhg_csd_layout` when it applies a configure or a scale and when `undecorated` changes, with the
  scale and the flag (pulled). The glue asks the shell whether the compositor decorates the window, reads the
  toplevel's states from its own configure state, and sets the frame's CSD fact itself (C no longer pushes it). C keeps
  a copy of the offsets for its drawing (`struct nhwl_csd`), and a child frame's shadow margin stays C's until child
  frames move.
- **Painting:** at each publish C calls `nhg_csd_paint` with the native size and a hash of what it paints from that
  the glue does not see (the title, the colours, the font, the backing's generation). The glue sends the window
  geometry if it changed, and answers 1 with the draw list (`NhgCsdPaint`: the strip, the title's box, the buttons
  with their state) only when the strip shows something new; C paints it with Cairo and damages the border.
- **Pointer:** the event step routes pointer events through the title bar first. Over the border it consumes them and
  gives an `NHG_ACT_TITLE_BAR` (the pointer shape, a repaint); a press moves, resizes, maximizes (a double click on the
  strip too), minimizes or shows the window menu through the shell, and a click on close gives `NHG_ACT_DELETE`.
  Back in the native area the title bar's action comes first and the event goes on raw, last. Hit tests use the native
  size of the last paint, which is what the surface shows.
- **Style:** `nhwl--set-title-bar-style` hands the buttons of each side to every display (`nhg_csd_style`), and to a
  display opened later; the colours and the font stay C's.

## Windows and tips

Every shell window C makes goes through `nhg_window_new` (`src/window.rs`), whose descriptor carries the facts the
frame starts from (native size, scale, visible, iconified, our own title bar): the glue registers the window seeded
with them. Tip, popup and child frames get a new window each time they are shown, so each new window starts from
Emacs's values, and its alpha starts over. `nhg_window_destroy` destroys and forgets (the frame is forgotten even when
a lost connection makes the shell refuse).

- **Operations:** title, `undecorated` (a child's window has nothing to ask), alpha, the size C sets itself (a push,
  as `nhg_frame_resized`), the smallest size (none for a child), visibility and iconify (checked pushes), fullscreen
  (`fullwidth`/`fullheight` ask for maximized) and one bounded round of the wait for the first configure
  (`nhg_window_wait_configured`: C drains between rounds). What reads Emacs stays C: the title's encoding, the
  `alpha` parameter with the highlight frame and `frame-alpha-lower-limit`, sizes from fonts, the `fullscreen`
  parameter.
- **Alpha:** C passes the effective alpha; the glue asks the compositor and falls back to the shell multiplying
  (`NHG_ALPHA_HOST`), and says when C must damage the whole frame (`NHG_ALPHA_DAMAGE`). C keeps copies of the cut and
  the host flag for its surface list.
- **Tips** (`src/tip.rs`): `nhg_tip_show` makes the tip's popup (below and right of a 1 x 1 point at the pointer plus
  the offsets, or at `left`/`top`, flipped and slid by the compositor) or moves it when only the offsets changed; 1
  asks C to draw the whole tip (a new tip, or a popup made again below `xdg_wm_base` 3). The tip frame itself and the
  frame it hangs on stay C.
