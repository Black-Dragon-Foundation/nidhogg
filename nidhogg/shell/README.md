# nhshell

Nidhogg's native shell layer: a Rust crate with a C ABI (`include/nhshell.h`), built as an rlib. Under
`--with-nidhogg-wayland` the Makefile links `nidhogg/glue`'s `libnhglue.a` instead of `libglitnir.a`: the one Rust
static library in the binary (two would collide on std's symbols), carrying this crate's `nhshell_*` and every
`glitnir_*` symbol too. The Wayland backend (`src/wayland/` and its dependencies) sits behind the default feature
`wayland`. New shell calls go into `trait ShellBackend` and this README only: no new entries in `nhshell.h` or
`src/ffi.rs`, which retire once the glue makes every call in Rust.

- Dependencies are vendored in `vendor/` (`cargo vendor-filterer`, Linux and macOS, x86_64 and aarch64) and the build
  runs offline and locked (`.cargo/config.toml`). Third-party crates are limited to wayland-client, wayland-backend,
  wayland-protocols, xkbcommon, rustix and their dependencies.
- `check.sh [--container DISTRO] [--test NAME]` runs fmt, clippy `-D warnings`, tests and cargo-deny; `--full` adds
  the vendor-drift check and the release build. The archive's exports are checked by `nidhogg/glue/check.sh --full`.
  The glue builds from this `vendor/`; after a revendor, its `Cargo.lock` takes this one's packages (its check says
  so).
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
| after `DISCONNECTED`: every other call returns `DISCONNECTED`, a waiting read gives up, `close` frees everything; `last_error` (a protocol error's text, empty for a hangup) | the window server does not go while the app runs: never `DISCONNECTED`, `last_error` empty | the I/O error handler (`XSetIOErrorExitHandler`) posts `DISCONNECTED` instead of exiting; `last_error` is the `XGetErrorText` of a fatal X error, empty for an I/O error | fits |
| `set_reconnect`, `reconnect`, `connection_state`; events `RECONNECTING`, `RECONNECTED`; error `RECONNECTING` (−9): with it on, a hangup posts what the loss ends (held buttons released, a preedit cleared, outputs left) and `RECONNECTING`; calls that send return `Reconnecting`; bounded `reconnect` steps connect to the same socket again, rebind the globals, the input and the buffers (same memfds), and post `RECONNECTED` | the window server does not go while the app runs: `set_reconnect` is `Unsupported`, the state stays up | the X server going ends the session: `set_reconnect` is `Unsupported` (X has no restart in place) | fits |
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
| `ack_configure` (serial, flags: `ACK_CONTENT` for an ack that changes what the window shows; sent with the tree's next commit) | no configure handshake: no-op (`CONFIGURE` events come from `windowDidResize:`) | answers `_NET_WM_SYNC_REQUEST` with that serial, or no-op | fits |
| `set_cursor` | `NSCursor` set | cursor font via `ChangeWindowAttributes` | fits |
| `window_activate` | `activateIgnoringOtherApps:` (the token is ignored) | `_NET_ACTIVE_WINDOW` (the token is the startup id) | fits |
| `window_scale` | `backingScaleFactor` x 120 (120 or 240) | 120 x Xft DPI ratio | fits |
| `window_ssd` | always true (the system draws the title bar) | true | fits |
| `window_new` kinds child / popup, `set_position`, `stack` | a child `NSWindow` (`addChildWindow:ordered:`, `setFrameOrigin:`); a popup is a borderless child panel placed by the host from the anchor | an override-redirect window (`XMoveWindow`, `XConfigureWindow` stack mode); `POPUP_DONE` never sent | fits |
| popup flag `WIN_GRAB`, `window_grabbed`, popup destroy order (descendants and grabbing popups above, topmost first) | a menu panel run modally (`NSMenu`-style tracking: the app takes every event until it closes); destroy order is free | `XGrabPointer` and `XGrabKeyboard` on the override-redirect window (a failed grab is "not grabbed"); destroy order is free | fits |
| `set_size` (popups) | `setFrame:display:` on the panel, placed again from the anchor; never "redraw all" | `XMoveResizeWindow`, placed again from the anchor; never "redraw all" | fits |
| events: `TOUCH` (39; down, motion, up, cancel per point, posted per touch frame), `PINCH` (40; begin, update, end; cumulative scale) | no touch screens (`TOUCH` never sent); `magnifyWithEvent:` (`magnification` summed into the scale) | XInput2 2.2 `XI_TouchBegin`/`Update`/`End` (no frames: one record each); XInput2 2.4 `XI_GesturePinchBegin`/`Update`/`End` | fits |
| `set_input_region`, `set_mapped` | `ignoresMouseEvents` for an empty region (a hit-test override otherwise); `orderOut:` / `orderFront:` | `XShapeCombineRectangles` (input); `XUnmapWindow` / `XMapWindow` | fits |
| `publish` (per tree: each drawable surface's damage, its copied pixels, the scene snapshot, the key time) | the same packets; fronts per surface, composed per tree | the same packets and fronts | fits |
| `present_tree` (the present path's rule; inline mode), `set_presenter_thread`, `presenter_stats` | the presenter sets each tree's `CALayer.contents` from a display-link thread (`CVDisplayLink` callback), inside one `CATransaction` per tree; inline mode on the main thread at the display link's tick | MIT-SHM puts of the composed tree with the Present extension (`PresentPixmap`, `PresentCompleteNotify` paces); the thread owns its own connection to the server | fits |
| the presenter thread (`set_presenter_thread` on: one per display composes and commits; pacers wake it instead of posting `FRAME_READY`; the host's thread waits only for a tree's lock or the mailbox, `main_wait_max_ns`), `presenter_timing` (frames committed more than a refresh after they were ready) | the display-link thread is the presenter; late frames are display-link ticks it missed | the presenter thread over its own connection; late frames are `PresentCompleteNotify` MSCs past the target | fits |
| content layers (no call: a window the host makes its own Glitnir output is shown by presenting on a sync subsurface of its root, over ring buffers whose `wp_viewport` source moves on a full-window scroll; it unmaps with the root commit that draws it in the frame again) | a `CALayer` per window whose `contentsRect` moves over an IOSurface ring, in the tree's `CATransaction` | none: the window stays in the frame's damage (no subwindow scrolling over shared memory) | fits |
| the tree owns its commit-time state: `set_position`, `stack`, `set_input_region`, `set_mapped` (off), `set_alpha`, `set_geometry` and the re-arm are recorded and sent with the tree's next commit; destroy holds the tree's lock | the recorded layer properties are set inside the tree's `CATransaction`; `NSWindow` calls stay on the main thread | the recorded requests go out right before the tree's `PresentPixmap` | fits |
| events: `KEY`, `MODIFIERS`, `BUTTON`, `POINTER_*`, `SCROLL` | `keyDown:` (`keyCode`, `characters`), `flagsChanged:`, mouse and `scrollWheel:` (`v120` 0, the precise deltas in `dx`, `dy`) | XInput2 key and button events through xkb (`v120` from wheel buttons) | fits |
| events: `CONFIGURE`, `CLOSE`, `SCALE`, `FOCUS` | `windowDidResize:`, `windowShouldClose:`, `windowDidChangeBackingProperties:`, `windowDidBecomeKey:` | `ConfigureNotify`, `WM_DELETE_WINDOW`, an Xft DPI change, `FocusIn` | fits |
| events: `FRAME_DONE` | the display link (`presented_ns` 0 unless known) | after `XShmPutImage` completes (`presented_ns` 0) | fits |
| events: `FRAME_READY` (21, roots only, inline mode: the tree's pacer lets it present; a child presents with its tree, deepest first) | the display link: ready when the layer may show the next bitmap (always, once drawn); child windows show in the root's walk | idle after `XShmCompletion` of the last put; children's puts in the root's walk | fits |
| events: `OUTPUT_ADD`, `OUTPUT_CHANGE`, `OUTPUT_REMOVE`, `ENTER_OUTPUT`, `LEAVE_OUTPUT` | `NSScreen`, `didChangeScreenParameters`, `windowDidChangeScreen:` | RandR monitors (a window's monitor from its position) | fits |
| events: `DISCONNECTED` | never sent | connection error | fits |
| `set_decorated` | `styleMask` with or without the title bar | `_MOTIF_WM_HINTS` decorations | fits |
| `bell` | `NSBeep` | `XkbBell` | fits |
| `inhibit_idle` | an `IOPMAssertion` (`PreventUserIdleDisplaySleep`) | the XScreenSaver `Suspend`/DPMS request | fits |
| `window_export` | none (sandbox handles do not exist) | the window's XID as text | fits |
| `activation_token` | none: an empty token | a `_NET_STARTUP_ID` for the window | fits |
| `session_open`, `window_session_remove` | state restoration (`NSWindowRestoration`) | `XSMP` session management | fits |
| `set_alpha` | `alphaValue` | `_NET_WM_WINDOW_OPACITY` | fits |
| `selection_own`, `selection_disown`, `selection_owned` | `NSPasteboard` `declareTypes:owner:` with the copied bytes per type; no system primary selection, so `PRIMARY` stays inside the process | `SetSelectionOwner` on `CLIPBOARD` or `PRIMARY`, answering `SelectionRequest` from the copy (INCR for large data) | fits |
| `selection_types`, `selection_read` | the pasteboard's `types` and `dataForType:` (synchronous, the timeout never runs out) | `ConvertSelection` to `TARGETS` or the type, waiting for `SelectionNotify` up to the timeout | fits |
| events: `SELECTION` | the pasteboard's `changeCount`, checked when the app becomes active | `XFixesSelectionNotify` | fits |
| drop target (Rust only): `selection_types`, `selection_read` with `SEL_DND` (our own drag's bytes without a round trip), `dnd_status`, `dnd_finish`; events `DND_ENTER`, `DND_MOTION`, `DND_LEAVE`, `DND_DROP` (32–35) | `NSDraggingDestination`: `draggingEntered:`, `draggingUpdated:` (its return value is the status), `draggingExited:`, `performDragOperation:` reading the dragging pasteboard | XDND target: `XdndEnter`, `XdndPosition` answered by `XdndStatus`, `XdndLeave`, `XdndDrop`; `ConvertSelection` of `XdndSelection`; `XdndFinished` | fits |
| drag source (Rust only): `drag_start` (with the press still held), `drag_cancel`; events `DRAG_ACTION`, `DRAG_DROPPED`, `DRAG_DONE` (36–38; `DRAG_DONE` comes before `DISCONNECTED`) | `beginDraggingSessionWithItems:event:source:` with the held mouse-down event; `draggingSession:endedAtPoint:operation:` is `DRAG_DROPPED` and `DRAG_DONE` together (operation none: cancelled) | XDND source: own `XdndSelection` and grab the pointer from the held press; `XdndStatus` gives the action, `XdndDrop` sent is dropped, `XdndFinished` is done | fits |
| `abandon` (the connection's socket and every thread's stop and done report, for a host that drops a backend whose state is in doubt: shut down, bounded join, leak) | a pipe the host can close, and the display-link thread's stop | the connection's fd, and the event thread's stop | fits |
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
| `inhibit-idle` | non-nil holds an idle inhibitor (`zwp_idle_inhibit_manager_v1`) on the frame's surface, nil releases it; a compositor without the protocol ignores it |
| `fullscreen` | `fullboth` and `maximized` as asked; `fullwidth` and `fullheight` maximize (a client cannot fill one direction) |
| `width`, `height`, `min-width`, `min-height` | the size; the compositor is told the smallest size (one column and one line, the bars and our title bar: `xdg_toplevel.set_min_size`), as GTK's size hints |
| `border-width`, `internal-border-width`, `child-frame-border-width`, `right-divider-width`, `bottom-divider-width`, `left-fringe`, `right-fringe`, `line-spacing`, `tab-bar-lines`, `menu-bar-lines`, `tool-bar-lines` | Emacs's own layout, redrawn |
| `font`, `font-backend`, colours (`foreground-color`, `background-color`, `cursor-color`, `border-color`), `cursor-type`, `screen-gamma`, `no-special-glyphs`, `borders-respect-alpha-background` | Emacs's drawing |
| `name`, `title` | the window's title (`xdg_toplevel.set_title`) and our title bar |
| `visibility` | shown or hidden; `icon` minimizes |
| `parent-frame`, `z-group` (child frames), `no-accept-focus` | child frames (sync subsurfaces) and their stacking; a `no-accept-focus` child never takes its tree's keys |
| `nhwl-popup` (Nidhogg's own, with `parent-frame`, at creation) | the frame is a popup frame, the window of a menu level: an `xdg_popup` of the parent's window, made when the frame is shown and destroyed when it is hidden (its popup frames first). The value is `(X Y W H . PLIST)`, the rectangle it sits beside in device pixels of the parent's native area; PLIST has `:gravity` (`bottom-right` by default, or `top`, `bottom`, `left`, `right`, `top-left`, `bottom-left`, `top-right`), `:adjust` (a list of `flip-x`, `flip-y`, `slide-x`, `slide-y`; all by default) and `:grab` (it takes the keyboard and pointer, so a click elsewhere dismisses it). A new X or Y moves the shown popup; the other fields apply when it is shown next. A resize gives the popup the new size. Keys and pointer events on it go to its frame; the keyboard moving between a top-level and its own popups is no focus change for Emacs. The compositor dismissing it is a `delete-frame` event for the frame |
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
