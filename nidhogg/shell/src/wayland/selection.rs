//! Selections: the clipboard (`wl_data_device`) and the primary selection (`zwp_primary_selection_device_v1`).
//!
//! Threads (spec §2.3): the managers, the devices, the offers and our sources are reader-queue objects, so the reader
//! thread tracks the offers and answers another client's `send` whatever the host's thread is doing.  `Selections`
//! (`Shared::selections`) is what both threads see.  The host's thread only sends requests (own, disown, `receive`)
//! and reads its pipe with a timeout; the reader writes our bytes from its poll loop with non-blocking writes
//! (`clipboard::SendBytes`), each descriptor closed when written or after 5 s.
//!
//! Locks: `Selections` is taken before libwayland's (requests are sent while holding it), never the other way round.

use crate::api::{NHSHELL_SEL_CLIPBOARD, NHSHELL_SEL_PRIMARY, ShellError, ShellResult};
use crate::event::{NHSHELL_EV_SELECTION, NhShellEvent, SelectionEv};
use crate::wayland::WaylandShell;
use crate::wayland::clipboard::{OfferTypes, SendBytes, read_with_timeout, receive_pipe, timespec};
use crate::wayland::conn::Shared;
use crate::wayland::reader::ReaderState;
use crate::wayland::registry::Globals;
use rustix::event::{PollFlags, Timespec};
use rustix::fd::OwnedFd;
use std::sync::atomic::Ordering;
use std::sync::{Arc, MutexGuard};
use std::time::{Duration, Instant};
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop, event_created_child};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1;
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_v1::{
    self, ZwpPrimarySelectionDeviceV1,
};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::{
    self, ZwpPrimarySelectionOfferV1,
};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_source_v1::{
    self, ZwpPrimarySelectionSourceV1,
};

/// The current offer of one selection (another client's, or ours coming back).
enum Offer {
    Data(WlDataOffer),
    Primary(ZwpPrimarySelectionOfferV1),
}

impl Offer {
    fn types(&self) -> Vec<String> {
        let types = match self {
            Offer::Data(o) => o.data::<OfferTypes>(),
            Offer::Primary(o) => o.data::<OfferTypes>(),
        };
        types.map(OfferTypes::list).unwrap_or_default()
    }

    fn receive(&self, mime_type: &str) -> ShellResult<OwnedFd> {
        match self {
            Offer::Data(o) => receive_pipe(o, mime_type),
            Offer::Primary(o) => receive_pipe(o, mime_type),
        }
    }

    fn destroy(&self) {
        match self {
            Offer::Data(o) => o.destroy(),
            Offer::Primary(o) => o.destroy(),
        }
    }
}

/// Our source of one selection.
enum Source {
    Data(WlDataSource),
    Primary(ZwpPrimarySelectionSourceV1),
}

impl Source {
    fn id(&self) -> ObjectId {
        match self {
            Source::Data(s) => s.id(),
            Source::Primary(s) => s.id(),
        }
    }

    fn destroy(&self) {
        match self {
            Source::Data(s) => s.destroy(),
            Source::Primary(s) => s.destroy(),
        }
    }
}

/// What we own: our source and a copy of the bytes of each MIME type.
struct Owned {
    source: Source,
    items: Vec<(String, Arc<[u8]>)>,
}

impl Owned {
    fn bytes(&self, mime_type: &str) -> Option<Arc<[u8]>> {
        self.items
            .iter()
            .find(|(m, _)| m == mime_type)
            .map(|(_, b)| b.clone())
    }
}

#[derive(Default)]
struct Slot {
    owned: Option<Owned>,
    offer: Option<Offer>,
}

/// The selections both threads see (`Shared::selections`), indexed by `NHSHELL_SEL_*`.
#[derive(Default)]
pub struct Selections {
    slots: [Slot; 2],
    /// One of our surfaces has keyboard focus (Wayland's condition for owning a selection).
    focus: bool,
    /// The newest keyboard-enter or input serial.
    serial: u32,
    data_manager: Option<WlDataDeviceManager>,
    data_device: Option<WlDataDevice>,
    primary_manager: Option<ZwpPrimarySelectionDeviceManagerV1>,
    primary_device: Option<ZwpPrimarySelectionDeviceV1>,
}

impl Selections {
    /// Keyboard focus came to one of our surfaces with `serial`.
    pub fn enter(&mut self, serial: u32) {
        self.focus = true;
        self.serial = serial;
    }

    pub fn leave(&mut self) {
        self.focus = false;
    }

    /// A key or button press with `serial`.
    pub fn input(&mut self, serial: u32) {
        self.serial = serial;
    }
}

pub fn lock(shared: &Shared) -> MutexGuard<'_, Selections> {
    shared.selections.lock().unwrap_or_else(|e| e.into_inner())
}

/// The slot of `kind` (`NHSHELL_SEL_*`).
fn slot(kind: u32) -> ShellResult<usize> {
    match kind {
        NHSHELL_SEL_CLIPBOARD | NHSHELL_SEL_PRIMARY => Ok(kind as usize),
        _ => Err(ShellError::Arg),
    }
}

fn selection_event(kind: u32) -> NhShellEvent {
    let mut ev = NhShellEvent::new(NHSHELL_EV_SELECTION, 0, 0);
    ev.u.selection = SelectionEv { kind };
    ev
}

/// Bind the two managers and make the seat's devices, all on the reader's queue (`open` calls this).
pub fn bind(
    globals: &Globals,
    registry: &WlRegistry,
    qh: &QueueHandle<ReaderState>,
    shared: &Shared,
    seat: Option<&WlSeat>,
) {
    let mut guard = lock(shared);
    let s = &mut *guard;
    s.data_manager = globals.bind(registry, qh, ());
    s.primary_manager = globals.bind(registry, qh, ());
    let Some(seat) = seat else { return };
    if let Some(m) = &s.data_manager {
        s.data_device = Some(m.get_data_device(seat, qh, ()));
    }
    if let Some(m) = &s.primary_manager {
        s.primary_device = Some(m.get_device(seat, qh, ()));
    }
}

/// The device named the selection's new offer (`None`: the selection is empty now).
fn new_offer(state: &mut ReaderState, kind: u32, offer: Option<Offer>) {
    let i = kind as usize;
    let old = std::mem::replace(&mut lock(&state.shared).slots[i].offer, offer);
    if let Some(old) = old {
        old.destroy();
    }
    state.post(selection_event(kind));
}

impl ReaderState {
    /// Another client asked for our `mime_type`: start writing it.  A type we do not have, or a source that is no
    /// longer ours, closes the descriptor, which ends that paste.
    fn answer_send(&mut self, kind: u32, source: ObjectId, mime_type: &str, fd: OwnedFd) {
        let sel = lock(&self.shared);
        let owned = sel.slots[kind as usize].owned.as_ref();
        let bytes = owned
            .filter(|o| o.source.id() == source)
            .and_then(|o| o.bytes(mime_type));
        drop(sel);
        if let Some(send) = bytes.and_then(|b| SendBytes::start(fd, b)) {
            self.sends.push(send);
        }
    }

    /// Our source of `kind` was replaced by another client's (or refused): forget it and tell the host.
    fn source_cancelled(&mut self, kind: u32, source: ObjectId) {
        let mut sel = lock(&self.shared);
        let cur = &mut sel.slots[kind as usize];
        let ours = cur.owned.as_ref().is_some_and(|o| o.source.id() == source);
        if ours {
            cur.owned = None;
        }
        drop(sel);
        if ours {
            self.post(selection_event(kind));
        }
    }

    /// How long the reader may sleep: until the earliest send's deadline (`None`: no limit).
    pub(super) fn send_timeout(&self) -> Option<Timespec> {
        let first = self.sends.iter().map(|s| s.deadline).min()?;
        Some(timespec(first.saturating_duration_since(Instant::now())))
    }

    /// Write to the descriptors the poll found ready (`revents`, in the order of `sends`), and drop the finished and
    /// the expired sends (closing their descriptors).
    pub(super) fn service_sends(&mut self, revents: &[PollFlags]) {
        let now = Instant::now();
        let mut ready = revents.iter();
        self.sends.retain_mut(|s| {
            let r = ready.next().copied().unwrap_or(PollFlags::empty());
            let done = !r.is_empty() && s.write_some();
            !done && now < s.deadline
        });
    }
}

impl WaylandShell {
    pub(super) fn sel_own(&mut self, kind: u32, items: &[(&str, &[u8])]) -> ShellResult {
        let i = slot(kind)?;
        let qh = self.main.reader_qh.clone().ok_or(ShellError::State)?;
        let mut guard = lock(&self.shared);
        let s = &mut *guard;
        if !s.focus {
            return Err(ShellError::State);
        }
        if items.is_empty() {
            return Err(ShellError::Arg);
        }
        let items: Vec<(String, Arc<[u8]>)> = items
            .iter()
            .map(|&(m, b)| (m.to_owned(), Arc::from(b)))
            .collect();
        let source = if i == 0 {
            let (Some(m), Some(d)) = (&s.data_manager, &s.data_device) else {
                return Err(ShellError::Unsupported);
            };
            let source = m.create_data_source(&qh, ());
            for (mime, _) in &items {
                source.offer(mime.clone());
            }
            d.set_selection(Some(&source), s.serial);
            Source::Data(source)
        } else {
            let (Some(m), Some(d)) = (&s.primary_manager, &s.primary_device) else {
                return Err(ShellError::Unsupported);
            };
            let source = m.create_source(&qh, ());
            for (mime, _) in &items {
                source.offer(mime.clone());
            }
            d.set_selection(Some(&source), s.serial);
            Source::Primary(source)
        };
        let old = s.slots[i].owned.replace(Owned { source, items });
        drop(guard);
        if let Some(old) = old {
            old.source.destroy();
        }
        self.flush()
    }

    /// Give up selection `kind` if we own it; destroying our source clears it.
    pub(super) fn sel_disown(&mut self, kind: u32) -> ShellResult {
        let i = slot(kind)?;
        let old = lock(&self.shared).slots[i].owned.take();
        if let Some(old) = old {
            old.source.destroy();
        }
        self.flush()
    }

    pub(super) fn sel_owned(&self, kind: u32) -> ShellResult<bool> {
        let i = slot(kind)?;
        Ok(lock(&self.shared).slots[i].owned.is_some())
    }

    /// Our MIME types while we own the selection, else the current offer's.
    pub(super) fn sel_types(&self, kind: u32) -> ShellResult<Vec<String>> {
        let i = slot(kind)?;
        let sel = lock(&self.shared);
        let cur = &sel.slots[i];
        Ok(match (&cur.owned, &cur.offer) {
            (Some(owned), _) => owned.items.iter().map(|(m, _)| m.clone()).collect(),
            (None, Some(offer)) => offer.types(),
            (None, None) => Vec::new(),
        })
    }

    /// Our own bytes without a round trip, or another client's through a pipe this thread reads for `timeout_ms` at
    /// most.  `State` when there is no selection or it lacks `mime_type`.
    pub(super) fn sel_read(
        &mut self,
        kind: u32,
        mime_type: &str,
        timeout_ms: u32,
    ) -> ShellResult<Vec<u8>> {
        let i = slot(kind)?;
        let fd = {
            let sel = lock(&self.shared);
            let cur = &sel.slots[i];
            if let Some(owned) = &cur.owned {
                return owned
                    .bytes(mime_type)
                    .map(|b| b.to_vec())
                    .ok_or(ShellError::State);
            }
            let offer = cur.offer.as_ref().ok_or(ShellError::State)?;
            if !offer.types().iter().any(|t| t == mime_type) {
                return Err(ShellError::State);
            }
            // sent while the lock is held, so the reader cannot destroy the offer in between
            offer.receive(mime_type)?
        };
        self.flush()?;
        read_with_timeout(
            fd,
            Duration::from_millis(timeout_ms.into()),
            self.shared.gone.fd(),
        )
    }
}

delegate_noop!(ReaderState: WlDataDeviceManager);
delegate_noop!(ReaderState: ZwpPrimarySelectionDeviceManagerV1);

impl Dispatch<WlDataDevice, ()> for ReaderState {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::Selection { id } => {
                new_offer(state, NHSHELL_SEL_CLIPBOARD, id.map(Offer::Data))
            }
            // drag and drop is W2's: an offer that enters is not taken, but the drag is noted (popups take no grab)
            wl_data_device::Event::Enter { id, .. } => {
                state.shared.dragging.store(true, Ordering::Release);
                if let Some(offer) = id {
                    offer.destroy();
                }
            }
            wl_data_device::Event::Leave | wl_data_device::Event::Drop => {
                state.shared.dragging.store(false, Ordering::Release)
            }
            _ => {}
        }
    }

    event_created_child!(ReaderState, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, OfferTypes::default()),
    ]);
}

impl Dispatch<WlDataOffer, OfferTypes> for ReaderState {
    fn event(
        _: &mut Self,
        _: &WlDataOffer,
        event: wl_data_offer::Event,
        types: &OfferTypes,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            types.add(mime_type);
        }
    }
}

impl Dispatch<WlDataSource, ()> for ReaderState {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                state.answer_send(NHSHELL_SEL_CLIPBOARD, source.id(), &mime_type, fd)
            }
            wl_data_source::Event::Cancelled => {
                source.destroy();
                state.source_cancelled(NHSHELL_SEL_CLIPBOARD, source.id());
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpPrimarySelectionDeviceV1, ()> for ReaderState {
    fn event(
        state: &mut Self,
        _: &ZwpPrimarySelectionDeviceV1,
        event: zwp_primary_selection_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_device_v1::Event::Selection { id } = event {
            new_offer(state, NHSHELL_SEL_PRIMARY, id.map(Offer::Primary));
        }
    }

    event_created_child!(ReaderState, ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE => (ZwpPrimarySelectionOfferV1, OfferTypes::default()),
    ]);
}

impl Dispatch<ZwpPrimarySelectionOfferV1, OfferTypes> for ReaderState {
    fn event(
        _: &mut Self,
        _: &ZwpPrimarySelectionOfferV1,
        event: zwp_primary_selection_offer_v1::Event,
        types: &OfferTypes,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_offer_v1::Event::Offer { mime_type } = event {
            types.add(mime_type);
        }
    }
}

impl Dispatch<ZwpPrimarySelectionSourceV1, ()> for ReaderState {
    fn event(
        state: &mut Self,
        source: &ZwpPrimarySelectionSourceV1,
        event: zwp_primary_selection_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_primary_selection_source_v1::Event::Send { mime_type, fd } => {
                state.answer_send(NHSHELL_SEL_PRIMARY, source.id(), &mime_type, fd)
            }
            zwp_primary_selection_source_v1::Event::Cancelled => {
                source.destroy();
                state.source_cancelled(NHSHELL_SEL_PRIMARY, source.id());
            }
            _ => {}
        }
    }
}
