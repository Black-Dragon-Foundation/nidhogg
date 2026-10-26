//! Client side of the seat, keyboard, pointer, text-input, cursor-shape and subsurface objects: the events land in
//! `Events` as short strings, so a test compares them as text.
use super::Client;
use std::io::Read;
use wayland_client::protocol::{wl_keyboard, wl_pointer, wl_seat, wl_subcompositor, wl_subsurface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1,
};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3, zwp_text_input_v3,
};

#[derive(Default)]
pub struct Events {
    /// The newest keymap text a wl_keyboard delivered.
    pub keymap: Option<String>,
    pub keyboard: Vec<String>,
    pub pointer: Vec<String>,
    pub text: Vec<String>,
    /// The serial of the newest wl_pointer.enter.
    pub enter_serial: u32,
}

impl Dispatch<wl_seat::WlSeat, ()> for Client {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        e: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            wl_keyboard::Event::Keymap { fd, size, .. } => {
                let mut text = String::new();
                std::fs::File::from(fd)
                    .take(u64::from(size))
                    .read_to_string(&mut text)
                    .unwrap();
                s.ev.keymap = Some(text.trim_end_matches('\0').to_string());
            }
            wl_keyboard::Event::Key { key, state, .. } => {
                s.ev.keyboard.push(format!("key {key} {}", name(state)))
            }
            wl_keyboard::Event::Modifiers { mods_depressed, .. } => {
                s.ev.keyboard.push(format!("mods {mods_depressed}"))
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_pointer::WlPointer,
        e: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let line = match e {
            wl_pointer::Event::Enter {
                serial,
                surface_x,
                surface_y,
                ..
            } => {
                s.ev.enter_serial = serial;
                format!("enter {surface_x} {surface_y}")
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => format!("motion {surface_x} {surface_y}"),
            wl_pointer::Event::Button { button, state, .. } => {
                format!("button {button} {}", name(state))
            }
            wl_pointer::Event::AxisSource { axis_source } => {
                format!("axis_source {}", name(axis_source))
            }
            wl_pointer::Event::Axis { axis, value, .. } => format!("axis {} {value}", name(axis)),
            wl_pointer::Event::AxisValue120 { axis, value120 } => {
                format!("axis_value120 {} {value120}", name(axis))
            }
            wl_pointer::Event::AxisStop { axis, .. } => format!("axis_stop {}", name(axis)),
            _ => return,
        };
        s.ev.pointer.push(line);
    }
}

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for Client {
    fn event(
        s: &mut Self,
        _: &zwp_text_input_v3::ZwpTextInputV3,
        e: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_text_input_v3::Event as E;
        s.ev.text.push(match e {
            E::Enter { .. } => "enter".into(),
            E::Leave { .. } => "leave".into(),
            E::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => format!(
                "preedit {:?} {cursor_begin} {cursor_end}",
                text.unwrap_or_default()
            ),
            E::CommitString { text } => format!("commit {:?}", text.unwrap_or_default()),
            E::Done { serial } => format!("done {serial}"),
            _ => return,
        });
    }
}

/// A protocol enum value's name (or `?N` for one this client does not know).
fn name<T: std::fmt::Debug>(v: WEnum<T>) -> String {
    match v {
        WEnum::Value(v) => format!("{v:?}"),
        WEnum::Unknown(n) => format!("?{n}"),
    }
}

macro_rules! ignore {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for Client {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection,
                     _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore!(
    zwp_text_input_manager_v3::ZwpTextInputManagerV3,
    wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
    wp_cursor_shape_device_v1::WpCursorShapeDeviceV1,
    wl_subcompositor::WlSubcompositor,
    wl_subsurface::WlSubsurface
);
