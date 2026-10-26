//! The compositor state: Smithay's compositor, shm and xdg-shell states plus the globals a profile lists, each at
//! `min(profile version, the version this recorder supports)`.
use crate::log::{BufferInfo, Commit, Log, Viewport};
use calloop::LoopSignal;
use serde::Deserialize;
use smithay::delegate_compositor;
use smithay::delegate_output;
use smithay::delegate_shm;
use smithay::delegate_xdg_shell;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_compositor::WlCompositor;
use smithay::reexports::wayland_server::protocol::wl_keyboard::WlKeyboard;
use smithay::reexports::wayland_server::protocol::wl_pointer::WlPointer;
use smithay::reexports::wayland_server::protocol::wl_seat::{self, WlSeat};
use smithay::reexports::wayland_server::protocol::wl_shm::WlShm;
use smithay::reexports::wayland_server::protocol::wl_subcompositor::WlSubcompositor;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::protocol::wl_touch::WlTouch;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::utils::{Serial, Transform};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    self, BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, Damage,
    SurfaceAttributes,
};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use smithay::wayland::shm::{with_buffer_contents, ShmHandler, ShmState};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clock {
    Auto,
    Manual,
}

/// A capability record: which globals a real compositor advertises, at which versions.
#[derive(Deserialize, Debug)]
pub struct Profile {
    pub name: String,
    pub globals: BTreeMap<String, u32>,
}

impl Profile {
    pub fn load(path: &Path) -> Result<Profile, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

pub struct ClientInfo {
    pub id: u32,
    pub pid: Option<i32>,
}

/// Per-client data: Smithay's compositor state, our client number, and the shared client list.
pub struct ClientState {
    compositor: CompositorClientState,
    id: u32,
    registry: Arc<Mutex<Vec<ClientInfo>>>,
}

impl ClientData for ClientState {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {
        self.registry.lock().unwrap().retain(|c| c.id != self.id);
    }
}

pub struct State {
    pub display: DisplayHandle,
    pub compositor_state: CompositorState,
    pub shm_state: ShmState,
    pub xdg_shell_state: XdgShellState,
    pub seat_state: SeatState<State>,
    #[allow(dead_code)] // Task 2 sends scale and mode changes through it
    pub output: Output,
    /// The globals advertised, name to version.
    pub globals: BTreeMap<String, u32>,
    pub log: Log,
    pub clients: Arc<Mutex<Vec<ClientInfo>>>,
    #[allow(dead_code)] // Task 2's frame callbacks follow it
    pub clock: Clock,
    start: Instant,
    next_client: u32,
    signal: LoopSignal,
}

/// The highest version of each global this recorder serves.
fn max_version(name: &str) -> Option<u32> {
    Some(match name {
        "wl_compositor" => 6,
        "wl_subcompositor" => 1,
        "wl_shm" => 2,
        "wl_seat" => 9,
        "wl_output" => 4,
        "xdg_wm_base" => 6,
        _ => return None,
    })
}

impl State {
    pub fn new(
        dh: &DisplayHandle,
        profile: &Profile,
        clock: Clock,
        output: (i32, i32, u32),
        signal: LoopSignal,
    ) -> State {
        // Smithay's states create their globals at its maximum versions; drop those and create the profile's.
        let compositor_state = CompositorState::new_v6::<State>(dh);
        let shm_state = ShmState::new::<State>(dh, []);
        let xdg_shell_state = XdgShellState::new::<State>(dh);
        dh.remove_global::<State>(compositor_state.compositor_global());
        dh.remove_global::<State>(compositor_state.subcompositor_global());
        dh.remove_global::<State>(shm_state.global());
        dh.remove_global::<State>(xdg_shell_state.global());

        let (w, h, scale120) = output;
        let out = Output::new(
            "nh-0".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "nh".into(),
                model: "recorder".into(),
            },
        );
        let mode = Mode {
            size: (w, h).into(),
            refresh: 60_000,
        };
        out.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Integer(scale120.div_ceil(120) as i32)),
            Some((0, 0).into()),
        );
        out.set_preferred(mode);

        let mut globals = BTreeMap::new();
        for (name, &want) in &profile.globals {
            let Some(max) = max_version(name) else {
                eprintln!(
                    "nh-recorder: profile {}: global {name} is not served yet, skipped",
                    profile.name
                );
                continue;
            };
            let v = want.min(max);
            match name.as_str() {
                "wl_compositor" => {
                    dh.create_global::<State, WlCompositor, _>(v, ());
                }
                "wl_subcompositor" => {
                    dh.create_global::<State, WlSubcompositor, _>(v, ());
                }
                "wl_shm" => {
                    dh.create_global::<State, WlShm, _>(v, ());
                }
                "wl_seat" => {
                    dh.create_global::<State, WlSeat, _>(v, ());
                }
                "wl_output" => {
                    out.create_global::<State>(dh);
                }
                "xdg_wm_base" => {
                    dh.create_global::<State, smithay::reexports::wayland_protocols::xdg::shell::server::xdg_wm_base::XdgWmBase, _>(v, ());
                }
                _ => unreachable!(),
            }
            globals.insert(name.clone(), v);
        }

        State {
            display: dh.clone(),
            compositor_state,
            shm_state,
            xdg_shell_state,
            seat_state: SeatState::new(),
            output: out,
            globals,
            log: Log::new(),
            clients: Arc::new(Mutex::new(Vec::new())),
            clock,
            start: Instant::now(),
            next_client: 1,
            signal,
        }
    }

    pub fn quit(&self) {
        self.signal.stop();
    }

    /// Data for a freshly accepted client; the client is listed with its pid when the peer credentials say.
    pub fn new_client_data(&mut self) -> ClientState {
        let id = self.next_client;
        self.next_client += 1;
        self.clients
            .lock()
            .unwrap()
            .push(ClientInfo { id, pid: None });
        ClientState {
            compositor: CompositorClientState::default(),
            id,
            registry: self.clients.clone(),
        }
    }

    fn t_ns(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64
    }
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor
    }

    fn commit(&mut self, surface: &WlSurface) {
        let client = surface
            .client()
            .and_then(|c| c.get_data::<ClientState>().map(|d| d.id))
            .unwrap_or(0);
        let role = match compositor::get_role(surface) {
            None => "none".to_string(),
            Some("xdg_toplevel") => "toplevel".to_string(),
            Some("xdg_popup") => "popup".to_string(),
            Some("subsurface") => "subsurface".to_string(),
            Some("cursor_image") => "cursor".to_string(),
            Some(other) => other.to_string(),
        };
        let parent = compositor::get_parent(surface).map(|p| p.id().protocol_id());
        let (buffer, buffer_scale, damage, frame_cb) = compositor::with_states(surface, |states| {
            let mut cached = states.cached_state.get::<SurfaceAttributes>();
            let attrs = cached.current();
            let buffer = match attrs.buffer.take() {
                Some(BufferAssignment::NewBuffer(buf)) => {
                    let info = with_buffer_contents(&buf, |_, _, d| BufferInfo {
                        w: d.width,
                        h: d.height,
                        stride: d.stride,
                        format: format!("{:?}", d.format).to_lowercase(),
                    })
                    .ok();
                    buf.release();
                    info
                }
                _ => None,
            };
            let damage = attrs
                .damage
                .iter()
                .map(|d| match d {
                    Damage::Surface(r) => [r.loc.x, r.loc.y, r.size.w, r.size.h],
                    Damage::Buffer(r) => [r.loc.x, r.loc.y, r.size.w, r.size.h],
                })
                .collect();
            (
                buffer,
                attrs.buffer_scale,
                damage,
                !attrs.frame_callbacks.is_empty(),
            )
        });
        let commit = Commit {
            seq: self.log.next_seq(),
            client,
            surface: surface.id().protocol_id(),
            role,
            parent,
            buffer,
            buffer_scale,
            viewport: Viewport::default(),
            pos: None,
            damage,
            frame_cb,
            presentation: false,
            ack_serial: None,
            t_ns: self.t_ns(),
        };
        self.log.push(commit);
    }
}

impl OutputHandler for State {}

// Smithay's xdg-shell wants a SeatHandler; the seat global itself is ours (below), so this state stays empty.
impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }
    fn focus_changed(&mut self, _: &Seat<State>, _: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _: &Seat<State>, _: smithay::input::pointer::CursorImageStatus) {}
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }
    fn new_toplevel(&mut self, _surface: ToplevelSurface) {}
    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}
    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {}
    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
    }
}

// A bare wl_seat with no capabilities: Smithay's seat global is fixed at version 9, and a profile may ask for less.
impl GlobalDispatch<WlSeat, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WlSeat>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let seat = init.init(resource, ());
        seat.capabilities(wl_seat::Capability::empty());
        if seat.version() >= 2 {
            seat.name("seat0".into());
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlSeat,
        request: wl_seat::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_seat::Request::GetPointer { id } => {
                init.init(id, ());
            }
            wl_seat::Request::GetKeyboard { id } => {
                init.init(id, ());
            }
            wl_seat::Request::GetTouch { id } => {
                init.init(id, ());
            }
            _ => {}
        }
    }
}

macro_rules! inert_dispatch {
    ($($iface:ty),*) => {$(
        impl Dispatch<$iface, ()> for State {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &$iface,
                _: <$iface as Resource>::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
            }
        }
    )*};
}
inert_dispatch!(WlPointer, WlKeyboard, WlTouch);

delegate_compositor!(State);
delegate_shm!(State);
delegate_output!(State);
delegate_xdg_shell!(State);
