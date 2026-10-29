# nhglue

The glue between Emacs and Nidhogg's native shell, in Rust: the shell's state and every decision that needs no live
Emacs state. The C side (`src/nhwl*.c`) keeps what touches Emacs (Lisp objects, Emacs structs, the keyboard queue,
the hook tables, `DEFUN`s) and all Cairo drawing, and calls in here through `include/nhglue.h`.

- **One library.** Under `--with-nidhogg-wayland` configure links `libnhglue.a` instead of `libglitnir.a`. It is the
  one Rust static library in the binary (two would collide on std's symbols), so `src/lib.rs` re-exports Glitnir's
  C ABI: `nidhogg-glitnir.c` keeps linking. `nhshell` is an rlib with no C ABI; C reaches the shell only through
  `nhg_*`.
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
  `nhglue.h` and `glitnir.h`. `--android` runs `cargo check --no-default-features --target
  aarch64-linux-android` (no NDK, no link) and exits 3 when the toolchain has no std for that target.
- `check.sh --c-fields BUILD_DIR`: the members of `struct nhwl_output` (`src/nhwlterm.h`) are exactly the ones the
  comment above it lists (`@fields` for the shared set, `@extra NAME: reason` for each other member). `check.sh
  --provenance`: every file with an "adapted from" note names the upstream commit (`Upstream commit:` line, or a
  40-digit hash on the note). Both are text checks that need neither cargo nor a container.
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
`nhg_next`, so the calls C makes while applying them cannot free them. Kinds 1–31 are the original set, 32–63 the later ones (DnD, touch, session, reconnect, layers).

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
  keeps the window that has the keyboard. A popup frame's focus events give none, and neither does a top-level losing
  the keyboard while a popup frame of its own grabs (`src/popup.rs`). Emacs's focus frame and highlighting stay C.
- Every event kind is ported: one C did nothing with gives no action. An event naming a forgotten window gives no
  action, except keys and the input method's, which C sends to the focus frame.
- **A tip's dismissal** (`POPUP_DONE` of a tip's window) gives `DELETE` with `NHG_DEL_TIP`: C calls `nhg_tip_hide` and
  hides the frame, which Lisp deletes later. A popup frame's gives `DELETE` with `NHG_DEL_POPUP`: C stores a
  `delete-frame` event, and the frame stays until Lisp deletes or hides it.

## The title bar

The client-side title bar (`src/csd.rs`, from `nhwlcsd.c`) is laid out, hit-tested and kept here; C paints it.

- **Layout:** C calls `nhg_csd_layout` when it applies a configure or a scale and when `undecorated` changes, with the
  scale and the flag (pulled). The glue asks the shell whether the compositor decorates the window, reads the
  toplevel's states from its own configure state, and sets the frame's CSD fact itself (C no longer pushes it). C keeps
  a copy of the offsets for its drawing (`struct nhwl_csd`). A child frame's border is its shadow margin
  (`src/child.rs`).
- **Painting:** at each publish C calls `nhg_csd_paint` with the native size and a hash of what it paints from that
  the glue does not see (the title, the colours, the font, the backing's generation). The glue sends the window
  geometry if it changed, and answers 1 with the draw list (`NhgCsdPaint`: the strip, the title's box, the buttons
  with their state) only when the strip shows something new; C paints it with Cairo and damages the border.
- **Pointer:** the event step routes pointer events through the title bar first. Over the border it consumes them and
  gives an `NHG_ACT_TITLE_BAR` (the pointer shape, a repaint); a press moves, resizes, maximizes (a double click on the
  strip too), minimizes or shows the window menu through the shell, and a click on close gives `NHG_ACT_DELETE`.
  Back in the native area the title bar's action comes first and the pointer's own action last. Hit tests use the native
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

## Child frames

A child frame's window is a sync subsurface of its parent's (`src/child.rs`, from `nhwlchild.c`); its position,
stacking, input region and mapping go out with its tree's commit.

- **Facts:** C pushes the child's parent, its position (the frame's left and top, device pixels of the parent's native
  area), whether it is shown and whether it is no-accept-focus (`nhg_child_set`) when its window is made, moved, shown
  or hidden, and the chrome `glitnir--resolve` gives it (`nhg_child_chrome`, a pull of the Lisp call's result) when its
  parameters change. A window made again (a reparent) gets both again; the debug check compares the parent.
- **Margin:** the shadow needs room around the native area (spread, three sigma of blur, the offset). It is the frame's
  border, laid out by `nhg_csd_layout` like a title bar, so `nhg_frame_offsets`, the backing, the damage and the
  pointer use it. `nhg_layer_ensure` puts a child's Glitnir layer inside it and hands the layer the chrome.
- **Publish:** `nhg_csd_paint` sends the child's position and its input region (the native area) when they changed;
  a top-level whose border changed places its children again at once.
- **Stacking:** C keeps the siblings' order (its tree walk lists children by it) and hands it to `nhg_child_stack` on
  raise and lower; the glue stacks the child against its neighbour. The keyboard focus redirect stays C.

## Menu popups

A popup frame (`src/popup.rs`, from `nhwlmenu.c`) is a frame whose `nhwl-popup` parameter, given at creation with
`parent-frame`, makes its window an xdg popup of the parent's window; the menus themselves are Lisp (`nhwl-menu.el`).
The frame is the root of its own tree (its pacer and acks are its own).

- **Parameter:** C decodes `(X Y W H . PLIST)` (the rectangle in device pixels of the parent's native area, `:gravity`,
  `:adjust`, `:grab`) and hands it to each call; the glue places it in the parent's surface after its border, at the
  parent's scale.
- **Show and hide:** `nhg_popup_show` makes the popup at the current parameter and the frame's size, registered as a
  visible popup frame, when Emacs makes the frame visible; `nhg_popup_hide` destroys it (C hides the popup frames
  hanging from it first: the compositor wants the topmost destroyed first). Size, gravity, adjust and grab apply at
  the next show; `nhg_popup_place` moves a shown popup when the parameter changes, and `nhg_popup_size` resizes it
  when C sets the frame's size (1 from either: the popup was made again below `xdg_wm_base` 3, C draws it whole).
- **Grab and focus:** `NHG_POPUP_GRAB` asks for a grab, which the shell sends only with a recent press's serial. While
  a popup of its own grabs, a top-level that loses the keyboard has lent it: no `FOCUS_OUT`, and Emacs's focus stays.
  `nhg_popup_hide` answers 1 when the loan ended with the keyboard on none of our windows (the compositor dismissed
  the menu as the focus went elsewhere), and C sends Emacs the focus-out then.
- **Dismissal:** `POPUP_DONE` gives `DELETE` with `NHG_DEL_POPUP`. Popups get no Glitnir shadow, and C refuses to
  reparent, raise or focus them.

## Window layers

An Emacs window that scrolls whole gets a surface of its own (`src/layers.rs`): the glue makes its Glitnir window
layer an own output, and the shell shows it as a content layer whose viewport source moves with each scroll, so the
compositor uploads only the exposed rows. The glue decides; C passes facts.

- **Scroll:** `nhwl_scroll_run` calls `nhg_scroll_window` with the window's box
  (`window_box (w, ANY_AREA)`) and `nhwl-scroll-layers` plus the frame's `alpha-background` as flags. A full-window
  run (it moves every row of the box but the exposed ones) on an opaque root promotes the window: a WINDOW layer with
  id `sequence_number | WINDOW_TAG` (the scroll's id too) at the box plus the title strip and margins, made an own
  output (Glitnir snaps it). At most four per frame. The backing still moves its rows: it is Emacs's picture, and the
  shell's front relies on it.
- **Opaque root:** a top-level window (not a child, tip or popup), alpha 1 applied by the compositor, `alpha-background`
  1, `wp_viewporter` and `wl_subcompositor`, and a scale `nhwl-scroll-layers` allows (`integer-scales`: whole ones).
- **Demotion:** at a publish, `nhg_layers_sync` gets the root's windows and their boxes (only while it has layers,
  `nhg_layers_count`) and demotes each layer whose window is gone or has another box, whose last full-window scroll is
  2 s old, or every one at a scale or border change or when the frame is no longer an opaque root; a scroll with a new
  box demotes first. Demotion is `set_output (…, 0)` and the layer's removal, in that publish: Glitnir damages the
  window on the frame and the shell unmaps the layer with the root's buffer. A quiet window with nothing published
  keeps its layer until the next publish (a redisplay), as no timer runs for it. `layers::drop_all` demotes every
  frame's (for a reconnect).

## The calls

Every `nhg_*` entry point of `include/nhglue.h`. **Pixels** says whether the call reads (R) or writes (W) a frame's
backing pixels; the rest touch none (-). **macOS** is what a macOS backend would do for the same call; "same" means
the glue's own logic carries over and only the shell's request changes. A call the platform has no counterpart for is
accepted and does nothing, as for a missing Wayland protocol.

| Call | Pixels | What it does | On macOS |
| --- | --- | --- | --- |
| `nhg_abi_version` | - | the ABI version | same |
| `nhg_open` / `nhg_close` | - | connect through the shell; close and free | open an `NSApplication` connection; close it |
| `nhg_fd` | - | descriptor readable when events are queued | a pipe the run loop writes to |
| `nhg_dispatch_main` | - | the shell's host-thread work, then flush | pump the main run loop once |
| `nhg_caps` | - | the shell's capabilities | fill what AppKit has (no fractional-scale protocol, scale from the screen) |
| `nhg_frame_register` / `nhg_frame_forget` | - | add or drop a frame in the glue's table | same |
| `nhg_frame_resized` / `nhg_frame_scale` / `nhg_frame_units` / `nhg_frame_csd` | - | push the facts a frame's decisions read | same |
| `nhg_next` | - | take one shell event as actions | same |
| `nhg_panic_message` / `nhg_test_panic` / `nhg_last_error` / `nhg_give_up` / `nhg_debug_check_frame` | - | panic, error and checking-build helpers | same |
| `nhg_layer_ensure` | - | make the frame's Glitnir layer match a geometry | same (Glitnir is the compositor-independent scene) |
| `nhg_backing` | R W | size the frame's backing and describe it | same; CoreAnimation layers could use an IOSurface instead |
| `nhg_damage` / `nhg_damage_all` | - | damage rectangles of the frame | same |
| `nhg_snapshot` / `nhg_snapshot_copy` | R | borrow or copy the backing | same |
| `nhg_backing_forget` | - | drop a frame's scene and backing | same |
| `nhg_present` | R | publish and present a tree | hand the backing to a `CALayer`; no CPU compose |
| `nhg_presenter_thread` / `nhg_presenter_stats` | - | presenter thread switch and counters | not used (CoreAnimation composites) |
| `nhg_bench_full_redraw` | R | time full redraws of a tree | same, with the layer commit as the end mark |
| `nhg_selection_set` / `nhg_selection_disown` / `nhg_selection_owned` / `nhg_selection_types` / `nhg_selection_pick` / `nhg_selection_read` | - | clipboard and primary selection | `NSPasteboard` general; no primary selection |
| `nhg_display_info` | - | what `x-display-*` reads | `NSScreen` values |
| `nhg_bell` | - | the system bell on a window's root surface | `NSBeep` |
| `nhg_pointer_visible` / `nhg_window_set_cursor` / `nhg_set_cursor_theme` | - | pointer shape, visibility, theme | `NSCursor`; the theme call does nothing |
| `nhg_csd_style` / `nhg_csd_layout` / `nhg_csd_paint` / `nhg_csd_title_bar` / `nhg_csd_mix` | - | the client-side title bar: style, layout, paint record, strip, colours | not used (native title bar); `nhg_csd_mix` same |
| `nhg_frame_offsets` | - | the backing's border around the native area | same (zero border without a title bar) |
| `nhg_im_enable` / `nhg_im_cursor` / `nhg_im_surrounding` | - | the input method's requests | `NSTextInputClient` |
| `nhg_monitors` / `nhg_frame_outputs` / `nhg_frame_monitors` / `nhg_monitor_origin` | - | outputs, the ones a frame is on, a monitor's device origin | `NSScreen` list |
| `nhg_window_new` / `nhg_window_destroy` | - | make or destroy a frame's shell window | `NSWindow` |
| `nhg_window_title` / `nhg_window_undecorated` / `nhg_window_alpha` / `nhg_window_min_size` / `nhg_window_size` | - | title, decoration, alpha, smallest size, pushed size | the matching `NSWindow` properties |
| `nhg_window_move` / `nhg_window_resize` / `nhg_window_show_menu` / `nhg_window_activate` | - | interactive move and resize, window menu, activation | `performWindowDrag`, `NSWindow` resize, no window menu, `makeKeyAndOrderFront` |
| `nhg_window_visible` / `nhg_window_iconify` / `nhg_window_fullscreen` | - | visibility, minimize, fullscreen | `orderFront`/`orderOut`, `miniaturize`, `toggleFullScreen` |
| `nhg_window_wait_configured` | - | one round of the wait for the first configure | returns configured at once |
| `nhg_frame_clamp` / `nhg_frame_fit` | - | size fitting and the surface clamp | same |
| `nhg_tip_show` / `nhg_tip_hide` | - | tooltip popups | a borderless `NSPanel` |
| `nhg_child_set` / `nhg_child_chrome` / `nhg_child_raise` | - | child frames: place, chrome, stacking | a child `NSWindow`, via `addChildWindow` |
| `nhg_popup_show` / `nhg_popup_place` / `nhg_popup_size` / `nhg_popup_hide` | - | popup frames for menus | a borderless `NSPanel` with a grab |
| `nhg_scroll_window` | W | scroll a window's rows; promote or demote its layer | same; a layer is a `CALayer` |
| `nhg_layers_sync` / `nhg_layers_count` | - | demote layers that are gone, resized or idle; count them | same |
| `nhg_bell_flash` / `nhg_bell_restore` | W | the visible bell's rows inverted and put back | same |
| `nhg_inhibit_idle` | - | hold or release an idle inhibitor | `IOPMAssertion` |
| `nhg_window_export` / `nhg_activation_token` | - | xdg-foreign handle; activation token | no counterpart (nothing, error) |
| `nhg_session_open` / `nhg_session_id` / `nhg_session_remove` | - | the compositor's session for frame restore | state restoration, or nothing |
| `nhg_dnd_register` / `nhg_dnd_status` / `nhg_dnd_finish` | - | drop target: types, acceptance, finish | `NSDraggingDestination` |
| `nhg_drag_start` / `nhg_drag_cancel` / `nhg_drag_state` | - | drag source | `NSDraggingSource` |
| `nhg_set_reconnect` / `nhg_reconnect_step` / `nhg_reconnect_settle` | - | reconnect in place after the compositor goes | not used (no compositor to lose) |
| `nhg_premultiply` | - | a colour as a backing stores it | same |
