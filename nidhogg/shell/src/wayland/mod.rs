//! The Wayland backend of the shell API.
//!
//! Queue ownership is fixed, so that one thread reads the socket and no handler runs on a thread it does not expect:
//! - the reader queue (`reader.rs`, the reader thread): `wl_seat`, `wl_keyboard`, `xdg_wm_base` (pings), the
//!   pointer and its pinch gesture, touch, key repeat and frame callbacks;
//! - the main queue (`MainState`, the host's thread): `wl_registry`, `wl_compositor`, `wl_subcompositor`, `wl_shm`
//!   and the surfaces, and later configures, outputs and scale.
//!
//! After `open` returns, the host's thread only sends requests, flushes and dispatches the main queue; it never reads
//! the socket (no roundtrip, no `blocking_dispatch`).

pub mod activation;
pub mod alpha;
pub mod bell;
pub mod child;
pub mod clipboard;
pub mod conn;
pub mod cursor;
pub mod cursor_theme;
pub mod decor;
pub mod foreign;
pub mod gesture;
pub mod idle;
pub mod keyboard;
pub mod output;
pub mod pointer;
pub mod popup;
pub mod present;
pub mod reader;
pub mod registry;
pub mod scale;
pub mod selection;
pub mod serial;
pub mod session;
pub mod shm;
pub mod text_input;
pub mod toplevel;
pub mod touch;

use crate::api::*;
use conn::Shared;
use reader::ReaderState;
use registry::Globals;
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::JoinHandle;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1;
use wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase;

/// What the main queue's handlers own; they run on the host's thread.
#[derive(Default)]
pub struct MainState {
    pub globals: Globals,
    registry: Option<WlRegistry>,
    compositor: Option<WlCompositor>,
    subcompositor: Option<WlSubcompositor>,
    shm: Option<WlShm>,
    surfaces: HashMap<u64, WlSurface>,
    shared: Option<Arc<Shared>>,
    fractional: Option<wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    viewporter: Option<wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter>,
    outputs: HashMap<u32, output::Output>,
    scales: HashMap<u64, scale::SurfaceScale>,
    wm_base: Option<XdgWmBase>,
    seat: Option<WlSeat>,
    decoration_manager: Option<ZxdgDecorationManagerV1>,
    presentation: Option<wayland_protocols::wp::presentation_time::client::wp_presentation::WpPresentation>,
    presents: HashMap<u64, Arc<std::sync::Mutex<present::Present>>>,
    reader_qh: Option<QueueHandle<ReaderState>>,
    windows: HashMap<u64, toplevel::Window>,
    children: HashMap<u64, child::Child>,
    popups: HashMap<u64, popup::Popup>,
    /// The popups that sent a grab, bottom to top, dismissed or not, until they are destroyed (`popup.rs`).
    grabs: Vec<u64>,
    activation: Option<wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1>,
    alpha_modifier: Option<wayland_protocols::wp::alpha_modifier::v1::client::wp_alpha_modifier_v1::WpAlphaModifierV1>,
    /// Each window's `wp_alpha_modifier_surface_v1`, made at its first `set_alpha`.
    alphas: HashMap<
        u64,
        wayland_protocols::wp::alpha_modifier::v1::client::wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1,
    >,
    /// `NIDHOGG_SHELL_FORCE_CSD` was set when the shell opened.
    force_csd: bool,
    /// The latest decoration mode the compositor granted a window.
    ssd: bool,
    /// The Xcursor theme fallback and the latest shape request.
    cursor: cursor_theme::ThemeCursor,
    bell: bell::Bell,
    idle: idle::Idle,
    foreign: foreign::Foreign,
    session: session::Session,
    tokens: activation::Tokens,
}

/// The main queue's waker: an event routed into the main queue (by the reader's read) marks it pending and wakes the
/// host through the same eventfd as the records.
struct MainWaker(Arc<Shared>);

impl Wake for MainWaker {
    fn wake(self: Arc<Self>) {
        self.0.main_pending.store(true, Ordering::Release);
        self.0.wake.signal();
    }
}

pub struct WaylandShell {
    conn: Connection,
    shared: Arc<Shared>,
    main_queue: EventQueue<MainState>,
    main: MainState,
    main_waker: Waker,
    reader: Option<JoinHandle<()>>,
    /// text-input-v3's state, shared with the reader's handlers.
    text_input: Arc<text_input::TextInput>,
}

impl MainState {
    /// Whether `id` is a window of any kind (top-level, child or popup).
    fn is_window(&self, id: u64) -> bool {
        self.windows.contains_key(&id)
            || self.children.contains_key(&id)
            || self.popups.contains_key(&id)
    }
}

impl WaylandShell {
    /// Connect, round-trip the registry, bind what exists at `min(advertised, supported)`, fill the capabilities and
    /// start the reader.  `NHSHELL_E_UNSUPPORTED` when no compositor answers or a required global is missing.
    pub fn open(display: Option<&CStr>) -> ShellResult<WaylandShell> {
        fn lost<E>(_: E) -> ShellError {
            ShellError::Unsupported
        }
        let conn = conn::connect(display)?;
        let shared = Arc::new(Shared::new()?);
        let mut main_queue = conn.new_event_queue::<MainState>();
        let mqh = main_queue.handle();
        let mut main = MainState {
            registry: Some(conn.display().get_registry(&mqh, ())),
            shared: Some(shared.clone()),
            force_csd: decor::force_csd_from_env(),
            ..MainState::default()
        };
        main_queue.roundtrip(&mut main).map_err(lost)?;
        if !main.globals.required() {
            return Err(ShellError::Unsupported);
        }

        let mut reader_queue = conn.new_event_queue::<ReaderState>();
        let rqh = reader_queue.handle();
        let mut rstate = ReaderState::new(shared.clone());
        {
            let g = &main.globals;
            let reg = main.registry.as_ref().ok_or(ShellError::Unsupported)?;
            main.compositor = g.bind(reg, &mqh, ());
            main.subcompositor = g.bind(reg, &mqh, ());
            main.shm = g.bind(reg, &mqh, ());
            main.fractional = g.bind(reg, &mqh, ());
            main.viewporter = g.bind(reg, &mqh, ());
            rstate.seat = g.bind(reg, &rqh, ());
            rstate.wm_base = g.bind(reg, &rqh, ());
            rstate.cursor_manager = g.bind(reg, &rqh, ());
            rstate.gestures = g.bind(reg, &rqh, ());
            main.decoration_manager = g.bind(reg, &mqh, ());
            main.presentation = g.bind(reg, &mqh, ());
            main.activation = g.bind(reg, &mqh, ());
            main.alpha_modifier = g.bind(reg, &mqh, ());
            main.bell.manager = g.bind(reg, &mqh, ());
            main.idle.manager = g.bind(reg, &mqh, ());
            main.foreign.exporter = g.bind(reg, &mqh, ());
            main.session.manager = g.bind(reg, &mqh, ());
            if let (Some(seat), Some(manager)) = (
                &rstate.seat,
                g.bind::<ZwpTextInputManagerV3, _, _>(reg, &rqh, ()),
            ) {
                rstate
                    .text_input
                    .set_object(manager.get_text_input(seat, &rqh, ()));
            }
            main.reader_qh = Some(rqh.clone());
            main.seat = rstate.seat.clone();
            main.wm_base = rstate.wm_base.clone();
            selection::bind(g, reg, &rqh, &shared, rstate.seat.as_ref());
        }
        // Before the reader owns its queue, this thread may still read: two round trips let the seat announce its
        // capabilities and the keyboard bind, so the shell opens with its input objects in place.
        for _ in 0..2 {
            conn.roundtrip().map_err(lost)?;
            reader_queue.dispatch_pending(&mut rstate).map_err(lost)?;
        }
        let text_input = rstate.text_input.clone();
        let main_waker = Waker::from(Arc::new(MainWaker(shared.clone())));
        let reader =
            reader::spawn(conn.clone(), reader_queue, rstate).map_err(|_| ShellError::NoMem)?;
        let mut shell = WaylandShell {
            conn,
            shared,
            main_queue,
            main,
            main_waker,
            reader: Some(reader),
            text_input,
        };
        // dispatches the setup's main-queue events and registers the waker
        shell.dispatch_main().map_err(|_| ShellError::Unsupported)?;
        Ok(shell)
    }

    /// Dispatch the main queue and register its waker; does not flush.
    fn dispatch_main_queue(&mut self) -> ShellResult {
        let mut cx = Context::from_waker(&self.main_waker);
        match self
            .main_queue
            .poll_dispatch_pending(&mut cx, &mut self.main)
        {
            Poll::Pending => Ok(()),
            Poll::Ready(Err(_)) => {
                // the reader's next dispatch meets the same error and posts DISCONNECTED
                self.shared.reader_wake.signal();
                Err(ShellError::Disconnected)
            }
        }
    }

    /// Send the queued requests; a full socket is retried by the reader when it becomes writable.
    pub fn flush(&self) -> ShellResult {
        self.shared.host_flush(&self.conn)
    }

    /// A bare surface on the main queue, for tests that need no window; its id is the protocol id.
    pub fn surface_new(&mut self) -> ShellResult<u64> {
        let compositor = self.main.compositor.as_ref().ok_or(ShellError::State)?;
        let surface = compositor.create_surface(&self.main_queue.handle(), ());
        let id = u64::from(surface.id().protocol_id());
        self.main.surfaces.insert(id, surface);
        Ok(id)
    }

    /// Give surface `id` its scale state (a fractional-scale object and a viewport when the compositor has them).
    /// A window owns this for its surface's lifetime; `window_scale` and `SCALE` events use the surface id.
    pub fn scale_attach(&mut self, id: u64) -> ShellResult {
        let surface = self
            .main
            .surfaces
            .get(&id)
            .ok_or(ShellError::State)?
            .clone();
        self.main.attach_scale(&surface, &self.main_queue.handle());
        Ok(())
    }

    /// Drop surface `id`'s scale state.
    pub fn scale_detach(&mut self, id: u64) {
        self.main.detach_scale(id);
    }

    /// The viewport destination of surface `id` (its logical size).
    pub fn scale_set_destination(
        &mut self,
        id: u64,
        logical_w: i32,
        logical_h: i32,
    ) -> ShellResult {
        self.main
            .scales
            .get(&id)
            .ok_or(ShellError::State)?
            .set_destination(logical_w, logical_h);
        Ok(())
    }

    /// Commit surface `id` (not flushed).
    pub fn surface_commit(&mut self, id: u64) -> ShellResult {
        self.main
            .surfaces
            .get(&id)
            .ok_or(ShellError::State)?
            .commit();
        Ok(())
    }

    /// Test hook: a flush hit EAGAIN and waits for the reader to retry it.
    #[doc(hidden)]
    pub fn flush_pending(&self) -> bool {
        self.shared.want_flush.load(Ordering::Acquire)
    }

    /// Test hook: a drag is in progress on the seat (popups take no grab).
    #[doc(hidden)]
    pub fn dragging(&self) -> bool {
        self.shared.dragging.load(Ordering::Acquire)
    }

    /// Test hook: shrink the socket's send buffer (`SO_SNDBUF`), so that a flush meets EAGAIN early.
    #[doc(hidden)]
    pub fn set_send_buffer(&self, bytes: usize) -> ShellResult {
        let backend = self.conn.backend();
        rustix::net::sockopt::set_socket_send_buffer_size(backend.poll_fd(), bytes)
            .map_err(|_| ShellError::Arg)
    }
}

impl Drop for WaylandShell {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.reader_wake.signal();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

delegate_noop!(MainState: WlCompositor);
delegate_noop!(MainState: WlSubcompositor);
delegate_noop!(MainState: wayland_client::protocol::wl_subsurface::WlSubsurface);
delegate_noop!(MainState: wayland_client::protocol::wl_region::WlRegion);
delegate_noop!(MainState: wayland_protocols::xdg::shell::client::xdg_positioner::XdgPositioner);
delegate_noop!(MainState: ignore WlShm);

impl Dispatch<WlSurface, ()> for MainState {
    fn event(
        state: &mut Self,
        surface: &WlSurface,
        event: wl_surface::Event,
        _: &(),
        conn: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // the surface may be shown again, or its scale changed: a waiting pacer asks again
        if matches!(
            event,
            wl_surface::Event::Enter { .. } | wl_surface::Event::PreferredBufferScale { .. }
        ) {
            state.present_rearm(u64::from(surface.id().protocol_id()), conn);
        }
        scale::surface_event(state, surface, event);
    }
}
