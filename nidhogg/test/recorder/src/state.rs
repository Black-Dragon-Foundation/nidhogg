//! The compositor state: Smithay's compositor, shm and xdg-shell states plus the globals a profile lists, each at
//! `min(profile version, the version this recorder supports)`.
use crate::log::{BufferInfo, Commit, DamageRect, Log, Viewport};
use calloop::{LoopHandle, LoopSignal};
use serde::Deserialize;
use smithay::delegate_compositor;
use smithay::delegate_output;
use smithay::delegate_shm;
use smithay::wayland::alpha_modifier::{AlphaModifierState, AlphaModifierSurfaceCachedState};
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::input::{Seat, SeatState};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason, ObjectId};
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
use smithay::utils::Transform;
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    self, BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, Damage,
    SubsurfaceCachedState, SurfaceAttributes,
};
use smithay::wayland::output::OutputHandler;
use smithay::reexports::wayland_protocols::wp::text_input::zv3::server::zwp_text_input_manager_v3::ZwpTextInputManagerV3;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation::WpPresentation;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecoMode;
use smithay::utils::{Clock as SmClock, Monotonic};
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::presentation::{PresentationFeedbackCachedState, PresentationState};
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::xdg_activation::XdgActivationState;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::{
    PopupSurface, ShellClient, SurfaceCachedState as XdgCachedState, ToplevelSurface, XdgShellState,
    XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{with_buffer_contents, ShmHandler, ShmState};
use smithay::wayland::viewporter::{ViewportCachedState, ViewporterState};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// The decoration mode the compositor sends: `server`, or `none` (the default; client-side).
    #[serde(default)]
    pub decoration: String,
    /// `all` (the default) answers the feedback of every commit; `damage-only` only that of commits with damage.
    #[serde(default)]
    pub frames: String,
    /// A later commit discards feedback still waiting.
    #[serde(default)]
    pub discard_on_supersede: bool,
    /// Replaced buffers kept unreleased (until the next replacement pushes them out, or `release`).
    #[serde(default)]
    pub hold_buffers: usize,
    /// The `occlude` command is allowed.
    #[serde(default)]
    pub occlusion: bool,
    /// Globals whose version is below the real compositor's, with the reason (documentation only).
    #[serde(default)]
    #[allow(dead_code)]
    pub capped: BTreeMap<String, String>,
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
    pub seat: Seat<State>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub activation_state: XdgActivationState,
    /// What `offer` set, by clipboard (0) and primary (1); descriptors a silent offer holds open.
    pub offers: [Option<crate::selection::Offer>; 2],
    pub held_fds: Vec<std::os::fd::OwnedFd>,
    /// The `xdg_activation_v1.activate` calls seen: `{token, surface}`.
    pub activations: Vec<serde_json::Value>,
    /// Every wl_output global served (the first is `output`), for the `output` command.
    pub outputs: Vec<(Output, GlobalId)>,
    pub rmlvo: crate::input::Rmlvo,
    /// The keyboard focus (set by the `focus` command).
    pub focus: Option<WlSurface>,
    pub text_inputs: Vec<crate::input::TextInput>,
    pub input_log: Vec<serde_json::Value>,
    pub output: Output,
    /// The globals advertised, name to version.
    pub globals: BTreeMap<String, u32>,
    pub log: Log,
    pub clients: Arc<Mutex<Vec<ClientInfo>>>,
    pub clock: Clock,
    start: Instant,
    next_client: u32,
    signal: LoopSignal,
    /// The scale announced to clients, in 120ths.
    pub scale120: u32,
    /// The decoration mode sent for toplevels (`decoration` command; the profile sets the start value).
    pub decoration_mode: DecoMode,
    /// Every surface created, for scale events and frame callbacks.
    pub surfaces: Vec<WlSurface>,
    pub toplevels: Vec<ToplevelSurface>,
    pub popups: Vec<PopupSurface>,
    /// Newest configure serial sent, and newest acked since the last commit, by surface object (protocol ids are
    /// per client and reused); an entry goes when its surface is destroyed.
    pub sent_serial: HashMap<ObjectId, u32>,
    pub acked: HashMap<ObjectId, u32>,
    pub shell_clients: Vec<Rc<ShellClient>>,
    pub pings: Vec<PendingPing>,
    /// The clock the frame callbacks and presentation feedback report: monotonic base plus the manual advance.
    pub mono_base: Duration,
    pub manual_ns: u64,
    pub present_seq: u64,
    /// Frame pacing modes and what they hold back.
    pub pacing: crate::clock::Pacing,
    /// The one renderer snapshots use (its buffer imports live as long as the surfaces).
    pub renderer: PixmanRenderer,
    /// The event loop, to queue a reply the control socket cannot take at once.
    pub handle: LoopHandle<'static, State>,
}

/// What one commit asked for, taken from the pending state before it is merged into the surface's current one
/// (Smithay's merge accumulates damage and frame callbacks until the renderer takes them).
#[derive(Default)]
struct CommitRequest {
    damage: Vec<DamageRect>,
    frame_cb: bool,
}

/// A `ping` control command waiting for pongs.
pub struct PendingPing {
    pub reply: UnixStream,
    pub started: Instant,
    pub waiting: Vec<Rc<ShellClient>>,
}

/// The highest version of each global this recorder serves.
fn max_version(name: &str) -> Option<u32> {
    Some(match name {
        "wl_compositor" => 6,
        "wl_subcompositor" => 1,
        "wl_shm" => 2,
        "wl_seat" => 9,
        "wl_data_device_manager" => 3,
        "zwp_primary_selection_device_manager_v1" => 1,
        "xdg_activation_v1" => 1,
        "zwp_text_input_manager_v3" => 1,
        "wp_cursor_shape_manager_v1" => 2,
        "wp_alpha_modifier_v1" => 1,
        "wl_output" => 4, // Smithay's fixed version: a lower profile value is served as 4 (and reported as 4)
        "xdg_wm_base" => 6,
        "zxdg_decoration_manager_v1" => 1,
        "wp_fractional_scale_manager_v1" => 1,
        "wp_viewporter" => 1,
        "wp_presentation" => 2,
        _ => return None,
    })
}

/// The role of a surface as the logs name it.
pub fn role_name(surface: &WlSurface) -> String {
    match compositor::get_role(surface) {
        None => "none".to_string(),
        Some("xdg_toplevel") => "toplevel".to_string(),
        Some("xdg_popup") => "popup".to_string(),
        Some("subsurface") => "subsurface".to_string(),
        Some("cursor_image") => "cursor".to_string(),
        Some(other) => other.to_string(),
    }
}

impl State {
    pub fn new(
        dh: &DisplayHandle,
        profile: &Profile,
        clock: Clock,
        output: (i32, i32, u32),
        signal: LoopSignal,
        handle: LoopHandle<'static, State>,
    ) -> State {
        // Smithay's states create their globals at its maximum versions; drop those and create the profile's.
        let compositor_state = CompositorState::new_v6::<State>(dh);
        let shm_state = ShmState::new::<State>(dh, []);
        let xdg_shell_state = XdgShellState::new::<State>(dh);
        dh.remove_global::<State>(compositor_state.compositor_global());
        dh.remove_global::<State>(compositor_state.subcompositor_global());
        dh.remove_global::<State>(shm_state.global());
        dh.remove_global::<State>(xdg_shell_state.global());
        // The extra protocols: Smithay creates each at its one version; a global the profile does not list is removed.
        // Smithay's seat global is version 9; a profile with a lower wl_seat gets the capability-less one below.
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(dh, "seat0");
        let rmlvo = crate::input::Rmlvo::default();
        seat.add_keyboard(rmlvo.xkb_config(), 600, 25)
            .expect("the default keymap compiles");
        seat.add_pointer();
        let seat_global = seat.global();
        // GTK 3 adds no seat until a wl_data_device_manager is bound, so every profile lists it
        let data_device_state = DataDeviceState::new::<State>(dh);
        let primary_selection_state = PrimarySelectionState::new::<State>(dh);
        let activation_state = XdgActivationState::new::<State>(dh);
        let extra = [
            (
                "zwp_primary_selection_device_manager_v1",
                primary_selection_state.global(),
            ),
            ("xdg_activation_v1", activation_state.global()),
            ("wl_data_device_manager", data_device_state.global()),
            (
                "zxdg_decoration_manager_v1",
                XdgDecorationState::new::<State>(dh).global(),
            ),
            (
                "wp_fractional_scale_manager_v1",
                FractionalScaleManagerState::new::<State>(dh).global(),
            ),
            ("wp_viewporter", ViewporterState::new::<State>(dh).global()),
            (
                "wp_cursor_shape_manager_v1",
                CursorShapeManagerState::new::<State>(dh).global(),
            ),
            (
                "wp_alpha_modifier_v1",
                AlphaModifierState::new::<State>(dh).global(),
            ),
            // wp_presentation is recreated below at the profile's version, on CLOCK_MONOTONIC (1)
            (
                "wp_presentation",
                PresentationState::new::<State>(dh, 1).global(),
            ),
        ];
        for (name, id) in extra {
            if !profile.globals.contains_key(name) || name == "wp_presentation" {
                dh.remove_global::<State>(id);
            }
        }

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
        let mut outputs = Vec::new();
        for (name, &want) in &profile.globals {
            let Some(max) = max_version(name) else {
                eprintln!(
                    "nh-recorder: profile {}: global {name} is not served yet, skipped",
                    profile.name
                );
                continue;
            };
            let v = if name == "wl_output" {
                max
            } else {
                want.min(max)
            };
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
                "wl_seat" if v >= 9 => {}
                "wl_seat" => {
                    dh.create_global::<State, WlSeat, _>(v, ());
                }
                "zwp_text_input_manager_v3" => {
                    dh.create_global::<State, ZwpTextInputManagerV3, _>(v, ());
                }
                "wl_output" => {
                    // Smithay creates wl_output at its one version (4); `max_version` says so, and `globals` below
                    // reports it
                    outputs.push((out.clone(), out.create_global::<State>(dh)));
                }
                "xdg_wm_base" => {
                    dh.create_global::<State, smithay::reexports::wayland_protocols::xdg::shell::server::xdg_wm_base::XdgWmBase, _>(v, ());
                }
                "wp_presentation" => {
                    dh.create_global::<State, WpPresentation, u32>(v, 1);
                }
                // created above
                "wl_data_device_manager"
                | "zwp_primary_selection_device_manager_v1"
                | "xdg_activation_v1"
                | "zxdg_decoration_manager_v1"
                | "wp_fractional_scale_manager_v1"
                | "wp_viewporter"
                | "wp_cursor_shape_manager_v1"
                | "wp_alpha_modifier_v1" => {}
                _ => unreachable!(),
            }
            globals.insert(name.clone(), v);
        }
        if globals.get("wl_seat").is_none_or(|&v| v < 9) {
            if let Some(id) = seat_global {
                dh.remove_global::<State>(id);
            }
        }
        let decoration_mode = if profile.decoration == "server" {
            DecoMode::ServerSide
        } else {
            DecoMode::ClientSide
        };
        let mono_base = Duration::from(SmClock::<Monotonic>::new().now());

        State {
            display: dh.clone(),
            compositor_state,
            shm_state,
            xdg_shell_state,
            seat_state,
            seat,
            data_device_state,
            primary_selection_state,
            activation_state,
            offers: [None, None],
            held_fds: Vec::new(),
            activations: Vec::new(),
            outputs,
            rmlvo,
            focus: None,
            text_inputs: Vec::new(),
            input_log: Vec::new(),
            output: out,
            globals,
            log: Log::new(),
            clients: Arc::new(Mutex::new(Vec::new())),
            clock,
            start: Instant::now(),
            next_client: 1,
            signal,
            scale120,
            decoration_mode,
            surfaces: Vec::new(),
            toplevels: Vec::new(),
            popups: Vec::new(),
            sent_serial: HashMap::new(),
            acked: HashMap::new(),
            shell_clients: Vec::new(),
            pings: Vec::new(),
            mono_base,
            manual_ns: 0,
            present_seq: 0,
            pacing: crate::clock::Pacing::from_profile(profile),
            renderer: PixmanRenderer::new().expect("the pixman renderer"),
            handle,
        }
    }

    pub fn quit(&self) {
        self.signal.stop();
    }

    /// Fill in a new client's pid from its peer credentials.
    pub fn note_client_pid(&mut self, client: &Client) {
        let (Some(data), Ok(cred)) = (
            client.get_data::<ClientState>(),
            client.get_credentials(&self.display),
        ) else {
            return;
        };
        if let Some(c) = self
            .clients
            .lock()
            .unwrap()
            .iter_mut()
            .find(|c| c.id == data.id)
        {
            c.pid = Some(cred.pid);
        }
    }

    /// Data for a freshly accepted client; `note_client_pid` lists it with its pid once it is inserted.
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

    pub fn t_ns(&self) -> u64 {
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

    fn new_surface(&mut self, surface: &WlSurface) {
        self.surfaces.retain(|s| s.is_alive());
        self.surfaces.push(surface.clone());
        // a surface born at a scale above 1 hears it at once
        compositor::add_pre_commit_hook::<State, _>(surface, |_, _, surface| {
            compositor::with_states(surface, |states| {
                let request = {
                    let mut cached = states.cached_state.get::<SurfaceAttributes>();
                    let pending = cached.pending();
                    CommitRequest {
                        damage: pending
                            .damage
                            .iter()
                            .map(|d| match d {
                                Damage::Surface(r) => {
                                    DamageRect::new("surface", r.loc.x, r.loc.y, r.size.w, r.size.h)
                                }
                                Damage::Buffer(r) => {
                                    DamageRect::new("buffer", r.loc.x, r.loc.y, r.size.w, r.size.h)
                                }
                            })
                            .collect(),
                        frame_cb: !pending.frame_callbacks.is_empty(),
                    }
                };
                states
                    .data_map
                    .insert_if_missing_threadsafe(|| Mutex::new(CommitRequest::default()));
                if let Some(slot) = states.data_map.get::<Mutex<CommitRequest>>() {
                    *slot.lock().unwrap() = request;
                }
            })
        });
        compositor::with_states(surface, |states| {
            compositor::send_surface_state(
                surface,
                states,
                self.scale120.div_ceil(120) as i32,
                Transform::Normal,
            )
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        let sid = surface.id().protocol_id();
        let client = surface
            .client()
            .and_then(|c| c.get_data::<ClientState>().map(|d| d.id))
            .unwrap_or(0);
        let role = role_name(surface);
        let parent = compositor::get_parent(surface).map(|p| p.id().protocol_id());
        let (buffer, buffer_scale, request, viewport, presentation, pos, title, app_id, alpha) =
            compositor::with_states(surface, |states| {
                let viewport = {
                    let mut cached = states.cached_state.get::<ViewportCachedState>();
                    let v = cached.current();
                    Viewport {
                        src: v.src.map(|r| [r.loc.x, r.loc.y, r.size.w, r.size.h]),
                        dst: v.dst.map(|s| [s.w, s.h]),
                    }
                };
                let alpha = states
                    .cached_state
                    .get::<AlphaModifierSurfaceCachedState>()
                    .current()
                    .multiplier_f64();
                let presentation = !states
                    .cached_state
                    .get::<PresentationFeedbackCachedState>()
                    .current()
                    .callbacks
                    .is_empty();
                let pos = if role == "subsurface" {
                    let mut cached = states.cached_state.get::<SubsurfaceCachedState>();
                    let l = cached.current().location;
                    Some([l.x, l.y])
                } else {
                    None
                };
                let (title, app_id) = if role == "toplevel" {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .map(|d| {
                            let d = d.lock().unwrap();
                            (d.title.clone(), d.app_id.clone())
                        })
                        .unwrap_or_default()
                } else {
                    (None, None)
                };
                let mut cached = states.cached_state.get::<SurfaceAttributes>();
                let attrs = cached.current();
                let buffer = match attrs.buffer.as_ref() {
                    // the renderer's surface state (below) keeps the buffer and releases it when replaced
                    Some(BufferAssignment::NewBuffer(buf)) => {
                        with_buffer_contents(buf, |_, _, d| BufferInfo {
                            w: d.width,
                            h: d.height,
                            stride: d.stride,
                            format: format!("{:?}", d.format).to_lowercase(),
                        })
                        .ok()
                    }
                    _ => None,
                };
                let request = states
                    .data_map
                    .get::<Mutex<CommitRequest>>()
                    .map(|m| std::mem::take(&mut *m.lock().unwrap()))
                    .unwrap_or_default();
                (
                    buffer,
                    attrs.buffer_scale,
                    request,
                    viewport,
                    presentation,
                    pos,
                    title,
                    app_id,
                    alpha,
                )
            });
        let geometry = if role == "toplevel" || role == "popup" {
            compositor::with_states(surface, |states| {
                let mut cached = states.cached_state.get::<XdgCachedState>();
                cached
                    .current()
                    .geometry
                    .map(|g| [g.loc.x, g.loc.y, g.size.w, g.size.h])
            })
        } else {
            None
        };
        let old_buffer = self.pacing_before_commit(surface);
        on_commit_buffer_handler::<State>(surface);
        self.pacing_after_commit(surface, old_buffer, !request.damage.is_empty());
        let ack_serial = self.acked.remove(&surface.id());
        let ack_stale = match (ack_serial, self.sent_serial.get(&surface.id())) {
            (Some(a), Some(&newest)) => a < newest,
            _ => false,
        };
        let decoration = self
            .toplevels
            .iter()
            .find(|t| t.wl_surface() == surface)
            .and_then(|t| t.with_pending_state(|s| s.decoration_mode))
            .map(|m| {
                if m == DecoMode::ServerSide {
                    "server"
                } else {
                    "client"
                }
                .to_string()
            });
        let commit = Commit {
            seq: self.log.next_seq(),
            client,
            surface: sid,
            role,
            parent,
            buffer,
            buffer_scale,
            viewport,
            pos,
            damage: request.damage,
            frame_cb: request.frame_cb,
            presentation,
            ack_serial,
            ack_stale,
            scale120_at_commit: self.scale120,
            decoration,
            title,
            app_id,
            geometry,
            alpha,
            t_ns: self.t_ns(),
        };
        self.log.push(commit);
        self.initial_configure(surface);
    }

    fn destroyed(&mut self, surface: &WlSurface) {
        self.sent_serial.remove(&surface.id());
        self.acked.remove(&surface.id());
        self.pacing_surface_gone(surface);
    }
}

impl OutputHandler for State {}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
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
