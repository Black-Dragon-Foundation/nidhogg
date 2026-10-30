//! Drag and drop, both ways.  `dnd_offer` makes the recorder a foreign source that drags onto the client's surface
//! (`dnd_motion`, `dnd_leave`, `dnd_drop`, read back through `dnd_log`).  A drag the client starts is logged
//! (`drag_log`), and the recorder plays the foreign target for it (`drag_target`, `drag_drop`, `drag_cancel`).
//! Neither blocks the event loop: `drag_drop` reads the dropped data on a non-blocking socket with a deadline.
//!
//! Smithay ends a client drag it started by sending `wl_data_source.cancelled` when nothing was accepted on a client
//! surface.  A foreign target never has a surface, so after `drag_drop`'s `dnd_finished` the grab is released a moment
//! later: a client that destroys its source on `dnd_finished` never hears that `cancelled`.
use crate::control::{err, write_reply};
use crate::state::{objects_of, State};
use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{Interest, Mode, PostAction, RegistrationToken};
use serde_json::{json, Value};
use smithay::backend::input::ButtonState;
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::input::pointer::{ButtonEvent, GrabStartData, MotionEvent};
use smithay::input::Seat;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_data_device_manager::DndAction;
use smithay::reexports::wayland_server::protocol::wl_data_source::{self, WlDataSource};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::selection::data_device::{
    default_action_chooser, start_dnd, with_source_metadata, ClientDndGrabHandler,
    ServerDndGrabHandler, SourceMetadata,
};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::time::Duration;

const READ_DEADLINE: Duration = Duration::from_millis(1000);
/// How long a finished foreign-target drop keeps the grab, so the client can destroy its source first.
const RELEASE_DELAY: Duration = Duration::from_millis(150);
const BTN_LEFT: u32 = 0x110;

/// One foreign-source drag (`dnd_offer`) and what the client did with it.
struct OfferRec {
    types: BTreeMap<String, String>,
    silent: bool,
    /// The compositor's own preference, used when the client names none.
    preferred: DndAction,
    accepted: Vec<Option<String>>,
    available: DndAction,
    client_preferred: DndAction,
    chosen: DndAction,
    received: Vec<String>,
    finished: bool,
    /// The `wl_data_offer` objects the client was given.
    ids: Vec<ObjectId>,
}

/// One drag the client started.
struct DragRec {
    source: Option<WlDataSource>,
    kind: &'static str,
    origin: Option<u32>,
    mimes: Vec<String>,
    actions: Vec<&'static str>,
    icon: Option<u32>,
    sends: Vec<String>,
    accept: Option<String>,
    action: DndAction,
    result: &'static str,
}

#[derive(Default)]
pub struct Dnd {
    offers: Vec<OfferRec>,
    drags: Vec<DragRec>,
    /// The surface the foreign drag is over (or was last over).
    surface: Option<WlSurface>,
    /// A foreign drag has begun and not yet been dropped.
    active: bool,
    /// The client's pointer drag has just started: its origin surface gets the drag's enter, as a desktop
    /// compositor's does (Smithay's grab enters only at the next motion).
    enter_origin: bool,
}

fn action_names(a: DndAction) -> Vec<&'static str> {
    let mut out = Vec::new();
    for (name, flag) in [
        ("copy", DndAction::Copy),
        ("move", DndAction::Move),
        ("ask", DndAction::Ask),
    ] {
        if a.contains(flag) {
            out.push(name);
        }
    }
    out
}

fn action_of(name: &str) -> Option<DndAction> {
    match name {
        "copy" => Some(DndAction::Copy),
        "move" => Some(DndAction::Move),
        "ask" => Some(DndAction::Ask),
        "none" => Some(DndAction::None),
        _ => None,
    }
}

/// `DataDeviceHandler::action_choice`: the client's preference, else the offer's, else Smithay's.
pub fn choose_action(state: &mut State, available: DndAction, preferred: DndAction) -> DndAction {
    let ours = match state.dnd.offers.last_mut().filter(|_| state.dnd.active) {
        Some(rec) => {
            rec.available = available;
            rec.client_preferred = preferred;
            Some(rec.preferred)
        }
        None => None,
    };
    let chosen = match ours {
        Some(p) if preferred.is_empty() && !p.is_empty() && available.contains(p) => p,
        _ => default_action_chooser(available, preferred),
    };
    if let Some(rec) = state.dnd.offers.last_mut().filter(|_| state.dnd.active) {
        rec.chosen = chosen;
    }
    chosen
}

impl ServerDndGrabHandler for State {
    fn accept(&mut self, mime_type: Option<String>, _: Seat<State>) {
        if let Some(rec) = self.dnd.offers.last_mut() {
            rec.accepted.push(mime_type);
        }
    }

    fn dropped(&mut self, _: Seat<State>) {
        self.dnd.active = false;
    }

    fn cancelled(&mut self, _: Seat<State>) {
        self.dnd.active = false;
    }

    fn send(&mut self, mime_type: String, fd: OwnedFd, _: Seat<State>) {
        let Some(rec) = self.dnd.offers.last_mut() else {
            return;
        };
        rec.received.push(mime_type.clone());
        if rec.silent {
            self.held_fds.push(fd);
            return;
        }
        let Some(text) = rec.types.get(&mime_type).cloned() else {
            return;
        };
        std::thread::spawn(move || {
            let _ = std::fs::File::from(fd).write_all(text.as_bytes());
        });
    }

    fn finished(&mut self, _: Seat<State>) {
        if let Some(rec) = self.dnd.offers.last_mut() {
            rec.finished = true;
        }
    }
}

impl ClientDndGrabHandler for State {
    fn started(
        &mut self,
        source: Option<WlDataSource>,
        icon: Option<WlSurface>,
        seat: Seat<State>,
    ) {
        let pointer = seat.get_pointer().filter(|p| p.is_grabbed());
        let origin = match (&pointer, seat.get_touch()) {
            (Some(p), _) => p
                .grab_start_data()
                .and_then(|d| d.focus.map(|(s, _)| s.id().protocol_id())),
            (None, Some(t)) => t
                .grab_start_data()
                .and_then(|d| d.focus.map(|(s, _)| s.id().protocol_id())),
            _ => None,
        };
        let meta = source
            .as_ref()
            .and_then(|s| with_source_metadata(s, |m| (m.mime_types.clone(), m.dnd_action)).ok());
        let (mimes, actions) = meta.unwrap_or_else(|| (Vec::new(), DndAction::empty()));
        self.dnd.enter_origin = pointer.is_some();
        self.dnd.drags.push(DragRec {
            source,
            kind: if pointer.is_some() { "button" } else { "touch" },
            origin,
            mimes,
            actions: action_names(actions),
            icon: icon.map(|i| i.id().protocol_id()),
            sends: Vec::new(),
            accept: None,
            action: DndAction::None,
            result: "started",
        });
    }

    fn dropped(&mut self, _: Option<WlSurface>, validated: bool, _: Seat<State>) {
        if let Some(d) = self.dnd.drags.last_mut().filter(|d| d.result == "started") {
            d.result = if validated { "dropped" } else { "cancelled" };
        }
    }
}

/// After a dispatch: the client's new pointer drag enters its origin surface where the pointer is (the grab is set
/// once `started` returns, so this cannot run there).
pub fn enter_origin(state: &mut State) {
    if !std::mem::take(&mut state.dnd.enter_origin) {
        return;
    }
    let Some(ptr) = state.seat.get_pointer() else {
        return;
    };
    let Some((surface, origin)) = ptr.grab_start_data().and_then(|d| d.focus) else {
        return;
    };
    // a drag that starts with the pointer off the surface (held there by the button's grab) is over nothing
    let at = ptr.current_location() - origin;
    if !with_renderer_surface_state(&surface, |s| s.surface_size())
        .flatten()
        .is_none_or(|sz| {
            at.x >= 0.0 && at.y >= 0.0 && at.x < f64::from(sz.w) && at.y < f64::from(sz.h)
        })
    {
        return;
    }
    let time = state.time_ms();
    ptr.motion(
        state,
        Some((surface, origin)),
        &MotionEvent {
            location: ptr.current_location(),
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    ptr.frame(state);
}

/// The surface a command names: an id, `"toplevel"` or `"newest"`; without a name, the one the drag is over.
pub fn surface_of(state: &mut State, v: Option<&Value>) -> Result<WlSurface, String> {
    state.surfaces.retain(|s| s.is_alive());
    match v {
        Some(Value::String(s)) if s == "newest" => state
            .surfaces
            .last()
            .cloned()
            .ok_or_else(|| "no surface".to_string()),
        Some(v) => crate::input::find_surface(state, v),
        None => state
            .dnd
            .surface
            .clone()
            .filter(|s| s.is_alive())
            .ok_or_else(|| "no target surface".to_string()),
    }
}

fn at_of(req: &Value) -> (f64, f64) {
    let at = |i| {
        req.get("at")
            .and_then(|a| a.get(i))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    (at(0), at(1))
}

/// Move the pointer, with the drag over `focus` (or over nothing), and note the offers the client was given.
fn move_to(state: &mut State, focus: Option<WlSurface>, at: (f64, f64)) {
    let Some(ptr) = state.seat.get_pointer() else {
        return;
    };
    let before = focus
        .as_ref()
        .map(|s| objects_of(&state.display, s, "wl_data_offer"))
        .unwrap_or_default();
    let time = state.time_ms();
    ptr.motion(
        state,
        focus.clone().map(|s| (s, (0.0, 0.0).into())),
        &MotionEvent {
            location: at.into(),
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    ptr.frame(state);
    if let Some(s) = focus {
        let new: Vec<ObjectId> = objects_of(&state.display, &s, "wl_data_offer")
            .into_iter()
            .filter(|id| !before.contains(id))
            .collect();
        if let Some(rec) = state.dnd.offers.last_mut() {
            rec.ids.extend(new);
        }
    }
}

/// `{"cmd":"dnd_offer","types":{"MIME":"TEXT"},"actions":["copy","move"],"preferred":"copy","silent":false,
/// "surface":"toplevel"|"newest"|ID,"at":[x,y]}`: a foreign source enters the surface.
pub fn offer(state: &mut State, req: &Value) -> Value {
    let Some(ptr) = state.seat.get_pointer() else {
        return err("this profile's seat has no pointer");
    };
    if state.dnd.active {
        // the foreign drag before this one left the window (`dnd_leave`) but still holds the button: it ends over
        // nothing first, as a new drag cannot start while the old one is held
        dropped(state);
    }
    if ptr.is_grabbed() {
        return err("a pointer grab is active");
    }
    let mut types = BTreeMap::new();
    match req.get("types").and_then(Value::as_object) {
        Some(o) if !o.is_empty() => {
            for (k, v) in o {
                types.insert(k.clone(), v.as_str().unwrap_or_default().to_string());
            }
        }
        _ => return err("\"types\" must be a non-empty object of MIME to text"),
    }
    let mut actions = DndAction::empty();
    for name in req
        .get("actions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        match name.as_str().and_then(action_of) {
            Some(a) => actions |= a,
            None => return err("\"actions\" must be copy, move or ask"),
        }
    }
    if actions.is_empty() {
        actions = DndAction::Copy;
    }
    let preferred = req
        .get("preferred")
        .and_then(Value::as_str)
        .and_then(action_of)
        .unwrap_or(DndAction::None);
    let toplevel = json!("toplevel");
    let surface = match surface_of(state, Some(req.get("surface").unwrap_or(&toplevel))) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let at = at_of(req);
    let mime_types: Vec<String> = types.keys().cloned().collect();
    state.dnd.offers.push(OfferRec {
        types,
        silent: req.get("silent").and_then(Value::as_bool).unwrap_or(false),
        preferred,
        accepted: Vec::new(),
        available: DndAction::empty(),
        client_preferred: DndAction::empty(),
        chosen: DndAction::empty(),
        received: Vec::new(),
        finished: false,
        ids: Vec::new(),
    });
    state.dnd.surface = Some(surface.clone());
    state.dnd.active = true;
    let (dh, seat) = (state.display.clone(), state.seat.clone());
    let start = GrabStartData {
        focus: None,
        button: BTN_LEFT,
        location: at.into(),
    };
    let metadata = SourceMetadata {
        mime_types,
        dnd_action: actions,
    };
    start_dnd(
        &dh,
        &seat,
        state,
        SERIAL_COUNTER.next_serial(),
        Some(start),
        None,
        metadata,
    );
    move_to(state, Some(surface), at);
    json!({"ok": true})
}

/// `{"cmd":"dnd_motion","at":[x,y],"surface":ID}`: the foreign drag moves (onto another surface, if one is named).
pub fn motion(state: &mut State, req: &Value) -> Value {
    if !state.dnd.active {
        return err("no foreign drag is active");
    }
    let surface = match surface_of(state, req.get("surface")) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    state.dnd.surface = Some(surface.clone());
    move_to(state, Some(surface), at_of(req));
    json!({"ok": true})
}

/// `{"cmd":"dnd_leave"}`: the foreign drag leaves the surface (the drag goes on).
pub fn leave(state: &mut State) -> Value {
    if !state.dnd.active {
        return err("no foreign drag is active");
    }
    move_to(state, None, (0.0, 0.0));
    json!({"ok": true})
}

/// `{"cmd":"dnd_drop"}`: the button comes up.  Smithay sends `drop` when the client accepted with an action.
pub fn dropped(state: &mut State) -> Value {
    let Some(ptr) = state.seat.get_pointer() else {
        return err("this profile's seat has no pointer");
    };
    if !state.dnd.active {
        return err("no foreign drag is active");
    }
    let time = state.time_ms();
    ptr.button(
        state,
        &ButtonEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time,
            button: BTN_LEFT,
            state: ButtonState::Released,
        },
    );
    ptr.frame(state);
    json!({"ok": true})
}

/// `{"cmd":"dnd_log"}`: what the client did with each foreign offer.
pub fn log(state: &State) -> Value {
    let handle = state.display.backend_handle();
    let offers: Vec<Value> = state
        .dnd
        .offers
        .iter()
        .map(|r| {
            let destroyed = !r.ids.is_empty()
                && r.ids
                    .iter()
                    .all(|id| handle.object_info(id.clone()).is_err());
            json!({
                "accepted": r.accepted,
                "actions": action_names(r.available),
                "preferred": action_names(r.client_preferred).first(),
                "chosen": action_names(r.chosen).first(),
                "received": r.received,
                "finished": r.finished,
                "destroyed": destroyed,
            })
        })
        .collect();
    json!({"ok": true, "offers": offers})
}

/// `{"cmd":"drag_log"}`: the drags the client started.
pub fn drag_log(state: &State) -> Value {
    let drags: Vec<Value> = state
        .dnd
        .drags
        .iter()
        .map(|d| {
            // a drag whose client destroyed its source (it gave the drag up) is cancelled: Smithay reports no drop
            let gone = d.source.as_ref().is_some_and(|s| !s.is_alive());
            let result = if d.result == "started" && gone {
                "cancelled"
            } else {
                d.result
            };
            json!({
                "serial_kind": d.kind,
                "origin": d.origin,
                "mimes": d.mimes,
                "actions": d.actions,
                "icon": d.icon,
                "sends": d.sends,
                "result": result,
            })
        })
        .collect();
    json!({"ok": true, "drags": drags})
}

/// The newest client drag that is still going, with its source.
fn running(state: &mut State) -> Result<&mut DragRec, Value> {
    match state.dnd.drags.last_mut() {
        Some(d) if d.result == "started" && d.source.is_some() => Ok(d),
        _ => Err(err("no client drag is running")),
    }
}

/// `{"cmd":"drag_target","accept":MIME|null,"action":"copy"|"move"|"none"}`: the foreign target's answer, sent to the
/// client's source now and used by `drag_drop`.  The pointer drag is over the foreign target, so it leaves the
/// client's surface first (a target of the client's own would answer the source too).
pub fn drag_target(state: &mut State, req: &Value) -> Value {
    if let Err(e) = running(state) {
        return e;
    }
    if let Some(ptr) = state.seat.get_pointer().filter(|p| p.is_grabbed()) {
        let time = state.time_ms();
        ptr.motion(
            state,
            None,
            &MotionEvent {
                location: ptr.current_location(),
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        ptr.frame(state);
    }
    let drag = match running(state) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let accept = req.get("accept").and_then(Value::as_str).map(String::from);
    let action = match req.get("action").and_then(Value::as_str).map(action_of) {
        None => DndAction::None,
        Some(Some(a)) => a,
        Some(None) => return err("\"action\" must be copy, move, ask or none"),
    };
    drag.accept = accept.clone();
    drag.action = action;
    if let Some(source) = &drag.source {
        source.target(accept);
        if source.version() >= wl_data_source::EVT_ACTION_SINCE {
            source.action(action);
        }
    }
    json!({"ok": true})
}

/// End the client's drag: the button comes up, or the touch point.
fn release(state: &mut State) {
    let touch = state.dnd.drags.last().is_some_and(|d| d.kind == "touch");
    let time = state.time_ms();
    if touch {
        if let Some(t) = state.seat.get_touch().filter(|t| t.is_grabbed()) {
            t.unset_grab(state);
        }
    } else if let Some(p) = state.seat.get_pointer().filter(|p| p.is_grabbed()) {
        p.unset_grab(state, SERIAL_COUNTER.next_serial(), time);
    }
}

/// `{"cmd":"drag_cancel"}`: the drag ends over nothing (Escape): the grab ends, and Smithay cancels the source.
pub fn drag_cancel(state: &mut State) -> Value {
    if let Err(e) = running(state) {
        return e;
    }
    release(state);
    if let Some(d) = state.dnd.drags.last_mut() {
        d.result = "cancelled";
    }
    json!({"ok": true})
}

/// What `collect` calls when the data is in (`Some`) or the deadline passed (`None`); its value is the reply.
type Done = Box<dyn FnOnce(&mut State, Option<String>) -> Value>;

struct Job {
    buf: Vec<u8>,
    reply: UnixStream,
    done: Option<Done>,
    token: Option<RegistrationToken>,
}

fn finish(state: &mut State, job: &Rc<RefCell<Job>>, complete: bool) {
    let (done, reply, text) = {
        let mut j = job.borrow_mut();
        let Some(done) = j.done.take() else {
            return;
        };
        let text = complete.then(|| String::from_utf8_lossy(&j.buf).into_owned());
        (done, j.reply.try_clone(), text)
    };
    let value = done(state, text);
    if let Ok(reply) = reply {
        write_reply(state, &reply, &value);
    }
}

/// Read `ours` to its end without blocking the loop, and answer `reply` with what `done` makes of the text.
fn collect(
    state: &mut State,
    ours: UnixStream,
    reply: UnixStream,
    done: Done,
) -> Result<(), String> {
    ours.set_nonblocking(true).map_err(|e| e.to_string())?;
    let job = Rc::new(RefCell::new(Job {
        buf: Vec::new(),
        reply,
        done: Some(done),
        token: None,
    }));
    let j = job.clone();
    let source = state.handle.insert_source(
        Generic::new(ours, Interest::READ, Mode::Level),
        move |_, stream, state| {
            let mut chunk = [0u8; 4096];
            loop {
                let n = match (&**stream).read(&mut chunk) {
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        return Ok(PostAction::Continue)
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => 0,
                };
                if n > 0 {
                    j.borrow_mut().buf.extend_from_slice(&chunk[..n]);
                    continue;
                }
                j.borrow_mut().token = None;
                finish(state, &j, true);
                return Ok(PostAction::Remove);
            }
        },
    );
    job.borrow_mut().token = Some(source.map_err(|e| e.to_string())?);
    let _ = state
        .handle
        .insert_source(Timer::from_duration(READ_DEADLINE), move |_, _, state| {
            let token = job.borrow_mut().token.take();
            if let Some(t) = token {
                state.handle.remove(t);
            }
            finish(state, &job, false);
            TimeoutAction::Drop
        });
    Ok(())
}

/// `{"cmd":"drag_drop","finish":true}`: the foreign target accepts the drop (as `drag_target` said), reads the accepted
/// type from the client's source and finishes unless `finish` is false.  A refusal cancels the source.  The reply goes
/// out when the data is in (None now).
pub fn drag_drop(state: &mut State, req: &Value, reply_to: &UnixStream) -> Option<Value> {
    let finishing = req.get("finish").and_then(Value::as_bool).unwrap_or(true);
    let drag = match running(state) {
        Ok(d) => d,
        Err(e) => return Some(e),
    };
    let (Some(source), Some(mime)) = (drag.source.clone(), drag.accept.clone()) else {
        drag.result = "cancelled";
        release(state);
        return Some(json!({"ok": true, "result": "cancelled", "data": null}));
    };
    if drag.action.is_empty() {
        drag.result = "cancelled";
        release(state);
        return Some(json!({"ok": true, "result": "cancelled", "data": null}));
    }
    drag.sends.push(mime.clone());
    if source.version() >= wl_data_source::EVT_DND_DROP_PERFORMED_SINCE {
        source.dnd_drop_performed();
    }
    let Ok((ours, theirs)) = UnixStream::pair() else {
        return Some(err("socketpair failed"));
    };
    let Ok(reply) = reply_to.try_clone() else {
        return Some(err("cannot clone the control stream"));
    };
    source.send(mime, theirs.as_fd());
    drop(theirs);
    let done: Done = Box::new(move |state, text| {
        let result = if finishing { "finished" } else { "dropped" };
        if finishing && source.version() >= wl_data_source::EVT_DND_FINISHED_SINCE {
            source.dnd_finished();
        }
        if let Some(d) = state.dnd.drags.last_mut() {
            d.result = result;
        }
        let _ = state
            .handle
            .insert_source(Timer::from_duration(RELEASE_DELAY), |_, _, state| {
                release(state);
                TimeoutAction::Drop
            });
        json!({"ok": true, "result": result, "data": text})
    });
    match collect(state, ours, reply, done) {
        Ok(()) => None,
        Err(e) => Some(err(e)),
    }
}
