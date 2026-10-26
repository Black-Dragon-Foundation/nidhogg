//! The Wayland backend of the shell API.
//!
//! Queue ownership is fixed (MP-NS11, spec §2.3):
//! - the reader queue (`reader.rs`, the reader thread): `wl_seat`, `wl_keyboard`, `xdg_wm_base` (pings), and later
//!   the pointer, key repeat and frame callbacks;
//! - the main queue (`MainState`, the host's thread): `wl_registry`, `wl_compositor`, `wl_subcompositor`, `wl_shm`
//!   and the surfaces, and later configures, outputs and scale.
//!
//! After `open` returns, the host's thread only sends requests, flushes and dispatches the main queue; it never reads
//! the socket (no roundtrip, no `blocking_dispatch`).

pub mod conn;
pub mod cursor;
pub mod decor;
pub mod keyboard;
pub mod output;
pub mod pointer;
pub mod present;
pub mod reader;
pub mod registry;
pub mod scale;
pub mod serial;
pub mod shm;
pub mod toplevel;

use crate::api::*;
use crate::caps::NhShellCaps;
use crate::event::NhShellEvent;
use conn::Shared;
use glitnir::geom::Rect;
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
    /// `NIDHOGG_SHELL_FORCE_CSD` was set when the shell opened.
    force_csd: bool,
    /// The latest decoration mode the compositor granted a window.
    ssd: bool,
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
            main.decoration_manager = g.bind(reg, &mqh, ());
            main.presentation = g.bind(reg, &mqh, ());
            main.reader_qh = Some(rqh.clone());
            main.seat = rstate.seat.clone();
            main.wm_base = rstate.wm_base.clone();
        }
        // Before the reader owns its queue, this thread may still read: two round trips let the seat announce its
        // capabilities and the keyboard bind, so the shell opens with its input objects in place.
        for _ in 0..2 {
            conn.roundtrip().map_err(lost)?;
            reader_queue.dispatch_pending(&mut rstate).map_err(lost)?;
        }
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

    /// A bare surface on the main queue (the windows of W0 Task 5 are built on these); its id is the protocol id.
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

impl ShellBackend for WaylandShell {
    fn wake_fd(&self) -> i32 {
        self.shared.wake.raw()
    }

    fn caps(&self) -> NhShellCaps {
        let mut caps = self.main.globals.caps();
        caps.ssd = self.main.ssd;
        caps.force_csd = self.main.force_csd;
        caps
    }

    /// The next record.  When the records are empty: drain the wake descriptor, dispatch the main queue if it is
    /// pending, and look again, so nothing posted in between is lost.
    fn next_event(&mut self) -> ShellResult<Option<NhShellEvent>> {
        loop {
            if let Some(ev) = self.shared.take()? {
                return Ok(Some(ev));
            }
            self.shared.wake.drain();
            if self.shared.main_pending.swap(false, Ordering::AcqRel) {
                // an error here reaches the host as the reader's DISCONNECTED record
                let _ = self.dispatch_main_queue();
                continue;
            }
            return self.shared.take();
        }
    }

    fn dispatch_main(&mut self) -> ShellResult {
        if self.shared.is_closed() {
            return Err(ShellError::Disconnected);
        }
        self.dispatch_main_queue()?;
        self.present_housekeeping();
        self.flush()
    }

    // The presenter arrives with W0 Task 6.
    fn window_new(&mut self, desc: &WindowDesc) -> ShellResult<WindowId> {
        self.window_create(desc)
    }
    fn window_destroy(&mut self, w: WindowId) -> ShellResult {
        self.window_destroy_impl(w)
    }
    fn set_title(&mut self, w: WindowId, t: &str) -> ShellResult {
        self.window_set_title(w, t)
    }
    fn set_app_id(&mut self, w: WindowId, a: &str) -> ShellResult {
        self.window_set_app_id(w, a)
    }
    fn set_min_size(&mut self, win: WindowId, w: i32, h: i32) -> ShellResult {
        self.window_set_min_size(win, w, h)
    }
    fn set_fullscreen(&mut self, w: WindowId, on: bool) -> ShellResult {
        self.window_set_fullscreen(w, on)
    }
    fn set_maximized(&mut self, w: WindowId, on: bool) -> ShellResult {
        self.window_set_maximized(w, on)
    }
    fn minimize(&mut self, w: WindowId) -> ShellResult {
        self.window_minimize(w)
    }
    fn move_window(&mut self, w: WindowId) -> ShellResult {
        self.window_move(w)
    }
    fn resize(&mut self, w: WindowId, edges: u32) -> ShellResult {
        self.window_resize(w, edges)
    }
    fn show_menu(&mut self, w: WindowId, x: i32, y: i32) -> ShellResult {
        self.window_show_menu(w, x, y)
    }
    fn ack_configure(&mut self, w: WindowId, serial: u32) -> ShellResult {
        self.window_ack_configure(w, serial)
    }
    fn set_cursor(&mut self, w: WindowId, shape: u32) -> ShellResult {
        self.window_set_cursor(w, shape)
    }
    fn window_scale(&self, window: WindowId) -> ShellResult<u32> {
        Ok(self
            .main
            .scales
            .get(&window)
            .ok_or(ShellError::State)?
            .scale120())
    }
    fn window_logical_size(&self, _: WindowId) -> ShellResult<(i32, i32)> {
        Err(ShellError::Unsupported)
    }
    fn present_begin(&mut self, w: WindowId, dw: i32, dh: i32) -> ShellResult<NhShellBuffer> {
        self.present_begin_impl(w, dw, dh)
    }
    fn present_commit(&mut self, w: WindowId, damage: &[Rect]) -> ShellResult {
        self.present_commit_impl(w, damage)
    }
}

delegate_noop!(MainState: WlCompositor);
delegate_noop!(MainState: WlSubcompositor);
delegate_noop!(MainState: ignore WlShm);

impl Dispatch<WlSurface, ()> for MainState {
    fn event(
        state: &mut Self,
        surface: &WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        scale::surface_event(state, surface, event);
    }
}
