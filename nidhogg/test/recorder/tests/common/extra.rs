//! Client side of the seat, keyboard, pointer, text-input, cursor-shape and subsurface objects: the events land in
//! `Events` as short strings, so a test compares them as text.
use super::Client;
use std::io::Read;
use wayland_client::protocol::{
    wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source, wl_keyboard, wl_pointer,
    wl_region, wl_seat, wl_subcompositor, wl_subsurface,
};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::alpha_modifier::v1::client::{
    wp_alpha_modifier_surface_v1, wp_alpha_modifier_v1,
};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1,
};
use wayland_protocols::wp::primary_selection::zv1::client::{
    zwp_primary_selection_device_manager_v1, zwp_primary_selection_device_v1,
    zwp_primary_selection_offer_v1::{self, ZwpPrimarySelectionOfferV1},
    zwp_primary_selection_source_v1,
};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3, zwp_text_input_v3,
};
use wayland_protocols::xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1};
use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_positioner};

#[derive(Default)]
pub struct Events {
    /// The newest keymap text a wl_keyboard delivered.
    pub keymap: Option<String>,
    pub keyboard: Vec<String>,
    pub pointer: Vec<String>,
    pub text: Vec<String>,
    /// The serial of the newest wl_pointer.enter.
    pub enter_serial: u32,
    /// The names of the wl_output globals currently advertised.
    pub outputs: Vec<u32>,
    /// The MIME types of the newest clipboard and primary-selection offers the client heard.
    pub clipboard: Option<(wl_data_offer::WlDataOffer, Vec<String>)>,
    pub primary: Option<(ZwpPrimarySelectionOfferV1, Vec<String>)>,
    /// `wl_data_source.send` requests heard; the data source writes `source_text` unless `source_mute`.
    pub sends: Vec<String>,
    pub source_text: String,
    pub source_mute: bool,
    pub activation_token: Option<String>,
    pub popup_done: u32,
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
            E::DeleteSurroundingText {
                before_length,
                after_length,
            } => format!("delete {before_length} {after_length}"),
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
    wl_subsurface::WlSubsurface,
    wl_region::WlRegion,
    wl_data_device_manager::WlDataDeviceManager,
    zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
    zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
    xdg_activation_v1::XdgActivationV1,
    xdg_positioner::XdgPositioner,
    wp_alpha_modifier_v1::WpAlphaModifierV1,
    wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1
);

impl Dispatch<wl_data_device::WlDataDevice, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_data_device::WlDataDevice,
        e: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            wl_data_device::Event::DataOffer { id } => s.ev.clipboard = Some((id, Vec::new())),
            wl_data_device::Event::Selection { id: None } => s.ev.clipboard = None,
            _ => {}
        }
    }
    event_created_child!(Client, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_data_offer::WlDataOffer,
        e: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let (wl_data_offer::Event::Offer { mime_type }, Some((_, types))) =
            (e, &mut s.ev.clipboard)
        {
            types.push(mime_type);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for Client {
    fn event(
        s: &mut Self,
        _: &wl_data_source::WlDataSource,
        e: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_source::Event::Send { mime_type, fd } = e {
            if !s.ev.source_mute {
                std::io::Write::write_all(
                    &mut std::fs::File::from(fd),
                    s.ev.source_text.as_bytes(),
                )
                .unwrap();
            } else {
                std::mem::forget(fd); // held open: the reader neither gets data nor an end
            }
            s.ev.sends.push(mime_type);
        }
    }
}

impl Dispatch<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, ()> for Client {
    fn event(
        s: &mut Self,
        _: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        e: zwp_primary_selection_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_device_v1::Event::DataOffer { offer } = e {
            s.ev.primary = Some((offer, Vec::new()));
        }
    }
    event_created_child!(Client, zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE => (ZwpPrimarySelectionOfferV1, ()),
    ]);
}

impl Dispatch<ZwpPrimarySelectionOfferV1, ()> for Client {
    fn event(
        s: &mut Self,
        _: &ZwpPrimarySelectionOfferV1,
        e: zwp_primary_selection_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let (zwp_primary_selection_offer_v1::Event::Offer { mime_type }, Some((_, types))) =
            (e, &mut s.ev.primary)
        {
            types.push(mime_type);
        }
    }
}

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ()> for Client {
    fn event(
        s: &mut Self,
        _: &xdg_activation_token_v1::XdgActivationTokenV1,
        e: xdg_activation_token_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = e {
            s.ev.activation_token = Some(token);
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, ()> for Client {
    fn event(
        s: &mut Self,
        _: &xdg_popup::XdgPopup,
        e: xdg_popup::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_popup::Event::PopupDone = e {
            s.ev.popup_done += 1;
        }
    }
}
