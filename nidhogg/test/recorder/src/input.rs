//! Input: the seat (Smithay's, for profiles that ask for wl_seat >= 9), keyboard keymaps from RMLVO, key, pointer,
//! button, axis, focus, our own text-input-v3 (so every client request lands in the input log), the cursor requests, and
//! the `input_log` command.
use crate::control::err;
use crate::state::State;
use serde_json::{json, Value};
use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState};
use smithay::input::keyboard::{xkb, FilterResult, Keycode, XkbConfig};
use smithay::input::pointer::{AxisFrame, ButtonEvent, CursorImageAttributes, CursorImageStatus, MotionEvent};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_protocols::wp::text_input::zv3::server::zwp_text_input_manager_v3::{
    self, ZwpTextInputManagerV3,
};
use smithay::reexports::wayland_protocols::wp::text_input::zv3::server::zwp_text_input_v3::{
    self, ZwpTextInputV3,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::compositor::with_states;
use smithay::wayland::tablet_manager::TabletSeatHandler;
use smithay::{delegate_alpha_modifier, delegate_cursor_shape, delegate_seat};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

const KEY_LEFTSHIFT: u32 = 42;

/// The keyboard layout names (rules, model, layout, variant, options).
#[derive(Clone)]
pub struct Rmlvo {
    rules: String,
    model: String,
    layout: String,
    variant: String,
    options: String,
}

impl Default for Rmlvo {
    fn default() -> Self {
        Rmlvo {
            rules: "evdev".into(),
            model: "pc105".into(),
            layout: "us".into(),
            variant: String::new(),
            options: String::new(),
        }
    }
}

impl Rmlvo {
    pub fn xkb_config(&self) -> XkbConfig<'_> {
        XkbConfig {
            rules: &self.rules,
            model: &self.model,
            layout: &self.layout,
            variant: &self.variant,
            options: (!self.options.is_empty()).then(|| self.options.clone()),
        }
    }

    /// The evdev keycode (and whether Shift is needed) of the key that types `c` in this keymap.
    fn key_for_char(&self, c: char) -> Option<(u32, bool)> {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &ctx,
            self.rules.as_str(),
            self.model.as_str(),
            self.layout.as_str(),
            self.variant.as_str(),
            (!self.options.is_empty()).then(|| self.options.clone()),
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )?;
        let sym = xkb::utf32_to_keysym(c as u32);
        for level in 0..2 {
            for code in keymap.min_keycode().raw()..=keymap.max_keycode().raw() {
                let key = Keycode::new(code);
                if keymap.num_levels_for_key(key, 0) > level
                    && keymap.key_get_syms_by_level(key, 0, level).contains(&sym)
                {
                    return Some((code - 8, level == 1));
                }
            }
        }
        None
    }
}

/// What a client's text-input-v3 object remembers.
#[derive(Default)]
pub struct TiData {
    commits: AtomicU32,
    pending_enable: Mutex<Option<bool>>,
    enabled: AtomicBool,
}

pub struct TextInput {
    pub res: ZwpTextInputV3,
    data: Arc<TiData>,
}

impl State {
    /// Append an entry to the input log (`seq` counts from 1).
    pub fn log_input(&mut self, mut entry: Value) {
        entry["seq"] = json!(self.input_log.len() as u64 + 1);
        entry["t_ns"] = json!(self.t_ns());
        self.input_log.push(entry);
    }

    fn time_ms(&self) -> u32 {
        (self.t_ns() / 1_000_000) as u32
    }

    /// Keyboard focus moved: text-input objects of the old client get `leave`, of the new one `enter`.
    fn set_text_focus(&mut self, new: Option<WlSurface>) {
        self.text_inputs.retain(|t| t.res.is_alive());
        if let Some(old) = self.focus.take() {
            for t in &self.text_inputs {
                if old.is_alive() && t.res.id().same_client_as(&old.id()) {
                    t.res.leave(&old);
                    t.data.enabled.store(false, Ordering::SeqCst);
                }
            }
        }
        if let Some(new) = &new {
            for t in &self.text_inputs {
                if t.res.id().same_client_as(&new.id()) {
                    t.res.enter(new);
                }
            }
        }
        self.focus = new;
    }
}

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }
    fn focus_changed(&mut self, _: &Seat<State>, _: Option<&WlSurface>) {}
    // Smithay hands over neither the request's serial nor the wl_pointer, so these entries carry `serial: null`.
    fn cursor_image(&mut self, _: &Seat<State>, image: CursorImageStatus) {
        let entry = match image {
            CursorImageStatus::Hidden => {
                json!({"req": "set_cursor", "surface": null, "serial": null})
            }
            CursorImageStatus::Named(icon) => {
                json!({"req": "set_shape", "shape": icon.name(), "serial": null})
            }
            CursorImageStatus::Surface(s) => {
                let hotspot = with_states(&s, |st| {
                    st.data_map
                        .get::<Mutex<CursorImageAttributes>>()
                        .map(|m| m.lock().unwrap().hotspot)
                });
                json!({
                    "req": "set_cursor",
                    "surface": s.id().protocol_id(),
                    "hotspot": hotspot.map(|h| [h.x, h.y]),
                    "serial": null,
                })
            }
        };
        self.log_input(entry);
    }
}

impl TabletSeatHandler for State {}

delegate_seat!(State);
delegate_cursor_shape!(State);
delegate_alpha_modifier!(State);

impl GlobalDispatch<ZwpTextInputManagerV3, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpTextInputManagerV3>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<ZwpTextInputManagerV3, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwpTextInputManagerV3,
        request: zwp_text_input_manager_v3::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zwp_text_input_manager_v3::Request::GetTextInput { id, .. } = request {
            let data = Arc::new(TiData::default());
            let res = init.init(id, data.clone());
            if let Some(f) = &state.focus {
                if res.id().same_client_as(&f.id()) {
                    res.enter(f);
                }
            }
            state.text_inputs.push(TextInput { res, data });
        }
    }
}

impl Dispatch<ZwpTextInputV3, Arc<TiData>> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwpTextInputV3,
        request: zwp_text_input_v3::Request,
        data: &Arc<TiData>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        use zwp_text_input_v3::Request as R;
        let entry = match request {
            R::Enable => {
                *data.pending_enable.lock().unwrap() = Some(true);
                json!({"req": "text_input.enable"})
            }
            R::Disable => {
                *data.pending_enable.lock().unwrap() = Some(false);
                json!({"req": "text_input.disable"})
            }
            R::SetSurroundingText {
                text,
                cursor,
                anchor,
            } => {
                json!({"req": "text_input.set_surrounding_text", "text": text, "cursor": cursor, "anchor": anchor})
            }
            R::SetTextChangeCause { cause } => {
                json!({"req": "text_input.set_text_change_cause", "cause": format!("{cause:?}")})
            }
            R::SetContentType { hint, purpose } => {
                json!({"req": "text_input.set_content_type", "hint": format!("{hint:?}"), "purpose": format!("{purpose:?}")})
            }
            R::SetCursorRectangle {
                x,
                y,
                width,
                height,
            } => json!({"req": "text_input.set_cursor_rectangle", "rect": [x, y, width, height]}),
            R::Commit => {
                let n = data.commits.fetch_add(1, Ordering::SeqCst) + 1;
                if let Some(on) = data.pending_enable.lock().unwrap().take() {
                    data.enabled.store(on, Ordering::SeqCst);
                }
                json!({"req": "text_input.commit", "serial": n})
            }
            _ => return,
        };
        state.log_input(entry);
    }

    fn destroyed(
        state: &mut Self,
        _: smithay::reexports::wayland_server::backend::ClientId,
        res: &ZwpTextInputV3,
        _: &Arc<TiData>,
    ) {
        state.text_inputs.retain(|t| t.res != *res);
    }
}

/// The surface a `focus` command names.
fn find_surface(state: &mut State, v: &Value) -> Result<WlSurface, String> {
    state.surfaces.retain(|s| s.is_alive());
    state.toplevels.retain(|t| t.alive());
    match v {
        Value::Number(n) => {
            let id = n.as_u64().unwrap_or(0) as u32;
            state
                .surfaces
                .iter()
                .find(|s| s.id().protocol_id() == id)
                .cloned()
                .ok_or_else(|| "no such surface".to_string())
        }
        Value::String(s) if s == "toplevel" => state
            .toplevels
            .last()
            .map(|t| t.wl_surface().clone())
            .ok_or_else(|| "no toplevel".to_string()),
        _ => Err("\"surface\" must be an id or \"toplevel\"".into()),
    }
}

fn str_of<'a>(req: &'a Value, key: &str) -> Option<&'a str> {
    req.get(key).and_then(Value::as_str)
}

fn press_state(req: &Value) -> Result<bool, String> {
    match str_of(req, "state") {
        Some("press") => Ok(true),
        Some("release") => Ok(false),
        _ => Err("\"state\" must be \"press\" or \"release\"".into()),
    }
}

fn key_event(state: &mut State, code: u32, press: bool) {
    let Some(kb) = state.seat.get_keyboard() else {
        return;
    };
    let ks = if press {
        KeyState::Pressed
    } else {
        KeyState::Released
    };
    let time = state.time_ms();
    kb.input::<(), _>(
        state,
        Keycode::new(code + 8),
        ks,
        SERIAL_COUNTER.next_serial(),
        time,
        |_, _, _| FilterResult::Forward,
    );
}

/// The input commands; `None` when `cmd` is not one of them.
pub fn command(state: &mut State, cmd: &str, req: &Value) -> Option<Value> {
    Some(match cmd {
        "keymap" => keymap(state, req),
        "key" => key(state, req),
        "focus" => focus(state, req),
        "pointer" => pointer(state, req),
        "button" => button(state, req),
        "axis" => axis(state, req),
        "text_input" => text_input(state, req),
        "input_log" => {
            let since = req.get("since").and_then(Value::as_u64).unwrap_or(0);
            let entries: Vec<&Value> = state
                .input_log
                .iter()
                .filter(|e| e["seq"].as_u64().unwrap_or(0) >= since)
                .collect();
            json!({"ok": true, "next": state.input_log.len() as u64 + 1, "entries": entries})
        }
        _ => return None,
    })
}

/// `{"cmd":"keymap","rmlvo":{"rules":"evdev","model":"pc105","layout":"us","variant":"","options":"…"}}`
fn keymap(state: &mut State, req: &Value) -> Value {
    let Some(kb) = state.seat.get_keyboard() else {
        return err("this profile's seat has no keyboard");
    };
    let mut rmlvo = Rmlvo::default();
    if let Some(r) = req.get("rmlvo") {
        for (key, slot) in [
            ("rules", &mut rmlvo.rules),
            ("model", &mut rmlvo.model),
            ("layout", &mut rmlvo.layout),
            ("variant", &mut rmlvo.variant),
            ("options", &mut rmlvo.options),
        ] {
            if let Some(s) = str_of(r, key) {
                *slot = s.to_string();
            }
        }
    }
    match kb.set_xkb_config(state, rmlvo.xkb_config()) {
        Ok(()) => {
            state.rmlvo = rmlvo;
            json!({"ok": true})
        }
        Err(_) => err("the keymap does not compile"),
    }
}

/// `{"cmd":"key","keycode":N,"state":"press|release"}` or `{"cmd":"key","text":"a"}`.
fn key(state: &mut State, req: &Value) -> Value {
    if state.seat.get_keyboard().is_none() {
        return err("this profile's seat has no keyboard");
    }
    if let Some(text) = str_of(req, "text") {
        let mut chars = text.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return err("\"text\" must be one character");
        };
        let Some((code, shift)) = state.rmlvo.key_for_char(c) else {
            return err(format!("the current keymap has no key for {c:?}"));
        };
        if shift {
            key_event(state, KEY_LEFTSHIFT, true);
        }
        key_event(state, code, true);
        key_event(state, code, false);
        if shift {
            key_event(state, KEY_LEFTSHIFT, false);
        }
        return json!({"ok": true, "keycode": code});
    }
    let Some(code) = req.get("keycode").and_then(Value::as_u64) else {
        return err("\"keycode\" or \"text\" is required");
    };
    match press_state(req) {
        Ok(press) => {
            key_event(state, code as u32, press);
            json!({"ok": true})
        }
        Err(e) => err(e),
    }
}

/// `{"cmd":"focus","surface":ID|"toplevel"|null}`: keyboard focus (and text-input enter/leave).
fn focus(state: &mut State, req: &Value) -> Value {
    let Some(kb) = state.seat.get_keyboard() else {
        return err("this profile's seat has no keyboard");
    };
    let surface = match req.get("surface") {
        Some(Value::Null) => None,
        Some(v) => match find_surface(state, v) {
            Ok(s) => Some(s),
            Err(e) => return err(e),
        },
        None => return err("\"surface\" is required"),
    };
    kb.set_focus(state, surface.clone(), SERIAL_COUNTER.next_serial());
    state.set_text_focus(surface);
    json!({"ok": true})
}

/// `{"cmd":"pointer","pos":[X,Y]}`: moves the pointer over the focused surface (surface-local logical).
fn pointer(state: &mut State, req: &Value) -> Value {
    let Some(ptr) = state.seat.get_pointer() else {
        return err("this profile's seat has no pointer");
    };
    let (Some(x), Some(y)) = (
        req.get("pos")
            .and_then(|p| p.get(0))
            .and_then(Value::as_f64),
        req.get("pos")
            .and_then(|p| p.get(1))
            .and_then(Value::as_f64),
    ) else {
        return err("\"pos\" must be [X,Y]");
    };
    let Some(surface) = state.focus.clone() else {
        return err("no focused surface (send focus first)");
    };
    let time = state.time_ms();
    ptr.motion(
        state,
        Some((surface, (0.0, 0.0).into())),
        &MotionEvent {
            location: (x, y).into(),
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    ptr.frame(state);
    json!({"ok": true})
}

/// `{"cmd":"button","button":272,"state":"press|release"}`
fn button(state: &mut State, req: &Value) -> Value {
    let Some(ptr) = state.seat.get_pointer() else {
        return err("this profile's seat has no pointer");
    };
    let Some(button) = req.get("button").and_then(Value::as_u64) else {
        return err("\"button\" is required");
    };
    let press = match press_state(req) {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    let time = state.time_ms();
    ptr.button(
        state,
        &ButtonEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time,
            button: button as u32,
            state: if press {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            },
        },
    );
    ptr.frame(state);
    json!({"ok": true})
}

/// `{"cmd":"axis","source":"wheel|finger","v120":N,"stop":bool}` (vertical; `"axis":"horizontal"` for the other).
fn axis(state: &mut State, req: &Value) -> Value {
    let Some(ptr) = state.seat.get_pointer() else {
        return err("this profile's seat has no pointer");
    };
    let source = match str_of(req, "source") {
        Some("wheel") | None => AxisSource::Wheel,
        Some("finger") => AxisSource::Finger,
        _ => return err("\"source\" must be \"wheel\" or \"finger\""),
    };
    let axis = match str_of(req, "axis") {
        Some("horizontal") => Axis::Horizontal,
        _ => Axis::Vertical,
    };
    let v120 = req.get("v120").and_then(Value::as_i64).unwrap_or(0) as i32;
    let mut frame = AxisFrame::new(state.time_ms()).source(source);
    if req.get("stop").and_then(Value::as_bool) == Some(true) {
        frame = frame.stop(axis);
    } else {
        // 15 logical pixels per wheel notch, as libinput reports
        frame = frame.value(axis, f64::from(v120) / 8.0);
        if source == AxisSource::Wheel {
            frame = frame.v120(axis, v120);
        }
    }
    ptr.axis(state, frame);
    ptr.frame(state);
    json!({"ok": true})
}

/// `{"cmd":"text_input","preedit":"…","cursor":[B,E],"commit":"…"}`: text-input-v3 events to the focused client's
/// enabled text input, in protocol order (preedit_string, commit_string), then one `done`.
fn text_input(state: &mut State, req: &Value) -> Value {
    state.text_inputs.retain(|t| t.res.is_alive());
    let Some(focus) = state.focus.clone() else {
        return err("no focused surface (send focus first)");
    };
    let Some(ti) = state
        .text_inputs
        .iter()
        .find(|t| t.res.id().same_client_as(&focus.id()) && t.data.enabled.load(Ordering::SeqCst))
    else {
        return err("the focused client has no enabled text-input");
    };
    let preedit = str_of(req, "preedit");
    let commit = str_of(req, "commit");
    if preedit.is_none() && commit.is_none() {
        return err("\"preedit\" or \"commit\" is required");
    }
    if let Some(p) = preedit {
        let cursor = |i: usize| {
            req.get("cursor")
                .and_then(|c| c.get(i))
                .and_then(Value::as_i64)
                .map(|n| n as i32)
        };
        let end = p.len() as i32;
        ti.res.preedit_string(
            Some(p.to_string()),
            cursor(0).unwrap_or(end),
            cursor(1).unwrap_or(end),
        );
    } else {
        // committing text replaces the preedit
        ti.res.preedit_string(None, 0, 0);
    }
    if let Some(c) = commit {
        ti.res.commit_string(Some(c.to_string()));
    }
    ti.res.done(ti.data.commits.load(Ordering::SeqCst));
    json!({"ok": true})
}
