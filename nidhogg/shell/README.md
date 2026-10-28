# nhshell

Nidhogg's native shell layer: a Rust crate with a C ABI (`include/nhshell.h`), built as one static
library, `libnhshell.a`, that also carries every `glitnir_*` symbol. Under `--with-nidhogg-wayland` the Makefile links
it instead of `libglitnir.a`; two Rust static libraries in one binary would collide on std's symbols.

- Dependencies are vendored in `vendor/` (`cargo vendor-filterer`, Linux and macOS, x86_64 and aarch64) and the build
  runs offline and locked (`.cargo/config.toml`). Third-party crates are limited to wayland-client, wayland-backend,
  wayland-protocols, xkbcommon, rustix and their dependencies.
- `check.sh [--container DISTRO] [--test NAME]` runs fmt, clippy `-D warnings`, tests, cargo-deny, the vendor-drift
  check and an `nm` check that every Glitnir export and `nhshell_abi_version` are in the archive.
  `check.sh --container DISTRO --revendor` regenerates `Cargo.lock` and `vendor/` (needs the network).

## The shell API

`src/api.rs` (the `ShellBackend` trait), `src/caps.rs` (`NhShellCaps`), `src/event.rs` (`NhShellEvent`) and `src/ffi.rs`
(the C ABI, `include/nhshell.h`) name no windowing backend: `ffi.rs` holds a `Box<dyn ShellBackend>`. The tree check
(`nidhogg/test/tree-test.sh`) and the test `api::no_wayland_names_in_ffi` keep `wayland_` out of everything but
`src/wayland/`. `abi::event_layout` compiles `nhshell.h` with `cc` and compares every size, alignment and offset with
the Rust records. `caps::profile_names_map` checks that each global of the recorder's `mutter` profile has a field in
`NhShellCaps` (`caps::GLOBAL_FIELDS` is the name table; `wl_shm` and `wl_output` have no field).

A request that has no counterpart on a backend (`move`, `resize`, `show_menu`, `set_app_id`, `ack_configure`) is
accepted and does nothing; so is a capability the backend does not have (its field is 0). That is the same rule as for
a missing Wayland protocol, so it is "fits", not "needs change".

### API check (macOS and X11)

macOS with AppKit (CoreAnimation composites, so the CPU compose step is not used, and child frames are separate child `NSWindow`s, which are outside this API so far). X11 with x11rb, with no fractional
scale: `scale120` = 120 x (Xft DPI / 96), and server-side decorations always (`ssd` true).

| Call | macOS (AppKit, Core Animation) | X11 (x11rb) | Verdict |
|---|---|---|---|
| `open`, `close` | `NSApplication` set-up; the display name is ignored | `connect(DISPLAY)` | fits |
| `wake_fd` | a pipe the main-thread event hook writes when it queues an event | the connection's fd | fits |
| `caps` | fixed record: no optional protocols (fields 0), `ssd` true | fixed record: `ssd` true, no Wayland-only fields | fits |
| `next_event`, `dispatch_main` | `NSEvent`s from `sendEvent:` become records; dispatch pumps the run loop once | `poll_for_event` and the XInput2 and RandR events become records; dispatch is a no-op | fits |
| `window_new` | `NSWindow initWithContentRect:`, style mask from the flags | `CreateWindow` plus `WM_PROTOCOLS`, `_NET_WM_STATE` | fits |
| `window_destroy` | `close` | `DestroyWindow` | fits |
| `set_title` | `setTitle:` | `_NET_WM_NAME` | fits |
| `set_app_id` | none per window (the bundle id is per app): no-op | `WM_CLASS` | fits |
| `set_min_size` | `contentMinSize` | `WM_NORMAL_HINTS` min size | fits |
| `set_fullscreen` | `toggleFullScreen:` | `_NET_WM_STATE_FULLSCREEN` | fits |
| `set_maximized` | `zoom:` | `_NET_WM_STATE_MAXIMIZED_*` | fits |
| `minimize` | `miniaturize:` | `WM_CHANGE_STATE` iconic | fits |
| `move` | `performWindowDragWithEvent:` with the last mouse event | `_NET_WM_MOVERESIZE` | fits |
| `resize` | no client-started edge resize: no-op (the system border resizes) | `_NET_WM_MOVERESIZE` with the edge | fits |
| `show_menu` | no window menu: no-op | `_GTK_SHOW_WINDOW_MENU`, or no-op | fits |
| `ack_configure` (serial, flags: `ACK_CONTENT` for an ack that changes what the window shows), `present_bare` | no configure handshake: no-op (`CONFIGURE` events come from `windowDidResize:`) | answers `_NET_WM_SYNC_REQUEST` with that serial, or no-op | fits |
| `set_cursor` | `NSCursor` set | cursor font via `ChangeWindowAttributes` | fits |
| `window_activate` | `activateIgnoringOtherApps:` (the token is ignored) | `_NET_ACTIVE_WINDOW` (the token is the startup id) | fits |
| `window_scale` | `backingScaleFactor` x 120 (120 or 240) | 120 x Xft DPI ratio | fits |
| `window_ssd` | always true (the system draws the title bar) | true | fits |
| `window_new` kinds child / popup, `set_position`, `stack` | a child `NSWindow` (`addChildWindow:ordered:`, `setFrameOrigin:`); a popup is a borderless child panel placed by the host from the anchor | an override-redirect window (`XMoveWindow`, `XConfigureWindow` stack mode); `POPUP_DONE` never sent | fits |
| `set_input_region`, `set_mapped` | `ignoresMouseEvents` for an empty region (a hit-test override otherwise); `orderOut:` / `orderFront:` | `XShapeCombineRectangles` (input); `XUnmapWindow` / `XMapWindow` | fits |
| `present_begin`, `present_commit` | a bitmap the layer shows (`CALayer.contents`); `age` 0 or 1; damage as `setNeedsDisplayInRect:` | MIT-SHM `XImage`, `XShmPutImage` per damage rectangle; `age` 0 or 2 | fits |
| events: `KEY`, `MODIFIERS`, `BUTTON`, `POINTER_*`, `SCROLL` | `keyDown:` (`keyCode`, `characters`), `flagsChanged:`, mouse and `scrollWheel:` (`v120` 0, the precise deltas in `dx`, `dy`) | XInput2 key and button events through xkb (`v120` from wheel buttons) | fits |
| events: `CONFIGURE`, `CLOSE`, `SCALE`, `FOCUS` | `windowDidResize:`, `windowShouldClose:`, `windowDidChangeBackingProperties:`, `windowDidBecomeKey:` | `ConfigureNotify`, `WM_DELETE_WINDOW`, an Xft DPI change, `FocusIn` | fits |
| events: `FRAME_DONE` | the display link (`presented_ns` 0 unless known) | after `XShmPutImage` completes (`presented_ns` 0) | fits |
| events: `FRAME_READY` (21, roots only: a child presents with its tree, deepest first), `window_dirty`, `window_ready` (bits `READY_CONTENT`, `READY_OWED`, `READY_ACK`) | the display link: ready when the layer may show the next bitmap (always, once drawn); child windows show in the root's walk | idle after `XShmCompletion` of the last put; children's puts in the root's walk | fits |
| events: `OUTPUT_ADD`, `OUTPUT_CHANGE`, `OUTPUT_REMOVE`, `ENTER_OUTPUT`, `LEAVE_OUTPUT` | `NSScreen`, `didChangeScreenParameters`, `windowDidChangeScreen:` | RandR monitors (a window's monitor from its position) | fits |
| events: `DISCONNECTED` | never sent | connection error | fits |
| `set_decorated` | `styleMask` with or without the title bar | `_MOTIF_WM_HINTS` decorations | fits |
| `set_alpha` | `alphaValue` | `_NET_WM_WINDOW_OPACITY` | fits |
| `selection_own`, `selection_disown`, `selection_owned` | `NSPasteboard` `declareTypes:owner:` with the copied bytes per type; no system primary selection, so `PRIMARY` stays inside the process | `SetSelectionOwner` on `CLIPBOARD` or `PRIMARY`, answering `SelectionRequest` from the copy (INCR for large data) | fits |
| `selection_types`, `selection_read` | the pasteboard's `types` and `dataForType:` (synchronous, the timeout never runs out) | `ConvertSelection` to `TARGETS` or the type, waiting for `SelectionNotify` up to the timeout | fits |
| events: `SELECTION` | the pasteboard's `changeCount`, checked when the app becomes active | `XFixesSelectionNotify` | fits |
| `text_input_enable`, `text_input_cursor`, `text_input_surrounding`, `take_text`; events `PREEDIT`, `COMMIT_TEXT`, `DELETE_SURROUNDING` | `NSTextInputClient` (`setMarkedText:`, `insertText:`, `firstRectForCharacterRange:`); no delete record (macOS replaces ranges in `insertText:`) | XIM or ibus over D-Bus (preedit callbacks, `XIM_SET_IC_VALUES` for the spot) | fits |

Result: every row fits; the API needed no change.

## Frame parameters

Every frame parameter of Emacs's table either has its effect on a frame of the shell or is accepted, stored and
listed here as a no-op with its reason (`frame-parameter` returns what was set either way).

| Parameter | On the shell |
|---|---|
| `alpha` | `wp_alpha_modifier_v1` on the window's surface (a child's too): the multiplier is surface state, so the window owes a commit and a bare one carries it. Without the protocol the glue multiplies the composed pixels. The active value while the frame is the highlighted one, the inactive one otherwise, never below `frame-alpha-lower-limit` (as pgtk) |
| `alpha-background` | the backing is drawn with it |
| `undecorated` | at creation the window asks for no decorations (client-side mode, and our title bar is not drawn); changed later, the window asks the compositor again and our title bar and margins come or go at once. A maximized, fullscreen or tiled window keeps the compositor's size; a floating one keeps its native size (as X11) |
| `fullscreen` | `fullboth` and `maximized` as asked; `fullwidth` and `fullheight` maximize (a client cannot fill one direction) |
| `width`, `height`, `min-width`, `min-height` | the size; the compositor is told the smallest size (one column and one line, the bars and our title bar: `xdg_toplevel.set_min_size`), as GTK's size hints |
| `border-width`, `internal-border-width`, `child-frame-border-width`, `right-divider-width`, `bottom-divider-width`, `left-fringe`, `right-fringe`, `line-spacing`, `tab-bar-lines`, `menu-bar-lines`, `tool-bar-lines` | Emacs's own layout, redrawn |
| `font`, `font-backend`, colours (`foreground-color`, `background-color`, `cursor-color`, `border-color`), `cursor-type`, `screen-gamma`, `no-special-glyphs`, `borders-respect-alpha-background` | Emacs's drawing |
| `name`, `title` | the window's title (`xdg_toplevel.set_title`) and our title bar |
| `visibility` | shown or hidden; `icon` minimizes |
| `parent-frame`, `z-group` (child frames), `no-accept-focus` | child frames (sync subsurfaces) and their stacking; a `no-accept-focus` child never takes its tree's keys |
| `auto-raise`, `auto-lower`, `unsplittable` | generic |
| `mouse-color` | no-op: a compositor's cursor (`wp_cursor_shape_v1`) has no colour |
| `icon-name` | no-op: Wayland has no icon title |
| `icon-type` | no-op: the icon comes from the desktop file |
| `left`, `top` on a top-level | no-op: Wayland gives a top-level no position (a child frame's are its position) |
| `z-group` on a top-level, `sticky`, `skip-taskbar`, `shaded` | no-op: no protocol for them |
| `no-focus-on-map` | no-op: the compositor decides whether a new window takes the focus |
| `override-redirect` | no-op: child frames are subsurfaces already |
| `inhibit-double-buffering` | no-op: the backing is single, and the shell's buffers are its own |
| `wait-for-wm` | no-op: the first configure is always awaited |
| `tool-bar-position` | no-op: the tool bar is Emacs-drawn, on top |
| `scroll-bar-width`, `scroll-bar-height`, `vertical-scroll-bars`, `horizontal-scroll-bars`, `scroll-bar-foreground`, `scroll-bar-background` | no-op: the shell has no toolkit scroll bars |
