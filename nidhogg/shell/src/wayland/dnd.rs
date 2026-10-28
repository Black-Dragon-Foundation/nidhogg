//! Drag and drop on the seat's `wl_data_device`, which `selection.rs` makes on the reader's queue.
//!
//! **Drop target.** The device's enter, motion, leave and drop become `DND_*` records naming the surface under the
//! pointer, with positions in device pixels.  The offer is `NHSHELL_SEL_DND` from enter until leave, or after a drop
//! until the host's `dnd_finish`; the `leave` compositors send after a drop changes nothing.  Reads go through a pipe
//! the host's thread reads with a timeout (and gives up when the display goes), as the selections' do.
//!
//! **Drag source.** `drag_start` makes a source on the reader's queue with copies of the bytes; the reader answers each
//! target's `send` from them with the selections' non-blocking writer, whatever the host's thread is doing, and turns
//! the source's events into `DRAG_*` records.  The source is destroyed at `dnd_finished` (a compositor may send
//! `cancelled` once its grab ends after that, which must not reach the host as a second end) or at `cancelled`.  The
//! bytes stay until the next `drag_start`: a drop on our own window reads them without a round trip.
//!
//! **State** is `Selections::dnd`, under the selections' lock, which is taken before libwayland's (requests are sent
//! while holding it) and never while the records' lock is held.  `Shared::dragging` follows it: a drag over one of our
//! surfaces, or our own drag, holds the pointer, so popups take no grab.

use crate::api::{
    NHSHELL_DND_ASK, NHSHELL_DND_COPY, NHSHELL_DND_MOVE, ShellError, ShellResult, WindowId,
};
use crate::event::*;
use crate::wayland::WaylandShell;
use crate::wayland::clipboard::{OfferTypes, SendBytes, read_with_timeout, receive_pipe};
use crate::wayland::conn::Shared;
use crate::wayland::reader::ReaderState;
use crate::wayland::selection;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use wayland_client::protocol::wl_data_device_manager::DndAction;
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};

const ALL_ACTIONS: u32 = NHSHELL_DND_COPY | NHSHELL_DND_MOVE | NHSHELL_DND_ASK;

/// The protocol's action set as `NHSHELL_DND_*` bits (the values are the protocol's).
fn bits(a: WEnum<DndAction>) -> u32 {
    let raw = match a {
        WEnum::Value(a) => a.bits(),
        WEnum::Unknown(v) => v,
    };
    raw & ALL_ACTIONS
}

/// A `wl_data_offer`'s user data: its MIME types, and for a drag the source's actions and the compositor's choice.
/// Every offer the device announces gets one (it says only later whether the offer is a selection or a drag).
#[derive(Default)]
pub struct DataOfferData {
    pub types: OfferTypes,
    source_actions: AtomicU32,
    action: AtomicU32,
}

/// Our drag's source's user data, which tells it from the clipboard's.
pub struct DragSource;

/// The offer of the drag over one of our surfaces.
struct DropOffer {
    offer: WlDataOffer,
    /// The enter's serial, which `accept` needs.
    serial: u32,
    /// The surface under the pointer (any window kind).
    window: u64,
    /// The latest position (device pixels) and motion time.
    x: f64,
    y: f64,
    time_ms: u32,
    /// The drag is our own.
    own: bool,
    /// What the host answered with `dnd_status`: a type, a non-zero action set.
    accepted: bool,
    actions_set: bool,
    dropped: bool,
}

impl DropOffer {
    fn data(&self) -> Option<&DataOfferData> {
        self.offer.data::<DataOfferData>()
    }

    /// The compositor's chosen action (0: none yet).
    fn action(&self) -> u32 {
        self.data().map_or(0, |d| d.action.load(Ordering::Acquire))
    }

    fn record(&self, kind: u32) -> NhShellEvent {
        let source_actions = self
            .data()
            .map_or(0, |d| d.source_actions.load(Ordering::Acquire));
        let action = self.action();
        let mut ev = NhShellEvent::new(kind, self.window, u64::from(self.time_ms) * 1_000_000);
        ev.u.dnd = DndEv {
            x: self.x,
            y: self.y,
            source_actions,
            preferred: suggested(source_actions, action),
            action,
            time_ms: self.time_ms,
            own: u8::from(self.own),
            ok: 0,
        };
        ev
    }
}

/// The action to suggest: the compositor's choice, else the first the source offers in copy, move, ask order.
fn suggested(source_actions: u32, action: u32) -> u32 {
    if action != 0 {
        return action;
    }
    [NHSHELL_DND_COPY, NHSHELL_DND_MOVE, NHSHELL_DND_ASK]
        .into_iter()
        .find(|a| source_actions & a != 0)
        .unwrap_or(0)
}

/// Our drag while it runs: its source, the window it started from, the compositor's latest action.
struct Drag {
    source: WlDataSource,
    window: u64,
    action: u32,
}

fn drag_record(kind: u32, window: u64, action: u32, ok: bool) -> NhShellEvent {
    let mut ev = NhShellEvent::new(kind, window, 0);
    ev.u.dnd = DndEv {
        action,
        ok: u8::from(ok),
        ..DndEv::default()
    };
    ev
}

/// Both sides of drag and drop (`Selections::dnd`).
#[derive(Default)]
pub struct Dnd {
    /// The offer of the drag over us (until leave, or until `dnd_finish` after a drop).
    offer: Option<DropOffer>,
    drag: Option<Drag>,
    /// Our latest drag's bytes per MIME type, until the next `drag_start`.
    kept: Vec<(String, Arc<[u8]>)>,
    /// The latest drag that entered was our own: once its offer is gone, `NHSHELL_SEL_DND` still reads `kept`.
    own_last: bool,
    /// A drag is over one of our surfaces (enter until leave or drop).
    over: bool,
}

impl Dnd {
    /// Our drag runs.
    pub fn dragging_ours(&self) -> bool {
        self.drag.is_some()
    }

    /// `NHSHELL_SEL_DND` answers from our own bytes.
    fn reads_kept(&self) -> bool {
        match &self.offer {
            Some(o) => o.own,
            None => self.own_last,
        }
    }

    fn kept_bytes(&self, mime_type: &str) -> Option<Arc<[u8]>> {
        self.kept
            .iter()
            .find(|(m, _)| m == mime_type)
            .map(|(_, b)| b.clone())
    }

    /// `NHSHELL_SEL_DND`'s MIME types (empty without an offer).
    pub fn types(&self) -> Vec<String> {
        if self.reads_kept() {
            return self.kept.iter().map(|(m, _)| m.clone()).collect();
        }
        self.offer
            .as_ref()
            .and_then(DropOffer::data)
            .map(|d| d.types.list())
            .unwrap_or_default()
    }

    fn sync_dragging(&self, shared: &Shared) {
        shared
            .dragging
            .store(self.over || self.drag.is_some(), Ordering::Release);
    }
}

/// The display is lost: our drag ends as cancelled.  `Shared::close` posts the record before `DISCONNECTED`.
pub fn drag_lost(shared: &Shared) -> Option<NhShellEvent> {
    let mut sel = selection::lock(shared);
    let drag = sel.dnd.drag.take()?;
    sel.dnd.sync_dragging(shared);
    Some(drag_record(NHSHELL_EV_DRAG_DONE, drag.window, 0, false))
}

/// The reader's side: the data device's drag events and our source's.
impl ReaderState {
    pub(super) fn dnd_enter(
        &mut self,
        serial: u32,
        surface: &WlSurface,
        x: f64,
        y: f64,
        offer: Option<WlDataOffer>,
    ) {
        let window = u64::from(surface.id().protocol_id());
        let (x, y) = self.to_device(window, x, y);
        let shared = self.shared.clone();
        let mut sel = selection::lock(&shared);
        let dnd = &mut sel.dnd;
        // a new drag replaces an offer the host has not finished
        if let Some(old) = dnd.offer.take() {
            old.offer.destroy();
        }
        dnd.over = true;
        dnd.sync_dragging(&shared);
        // a drag without data (another client's own, which never reaches us) has nothing to report
        let Some(offer) = offer else { return };
        let own = dnd.drag.is_some();
        dnd.own_last = own;
        let o = DropOffer {
            offer,
            serial,
            window,
            x,
            y,
            time_ms: 0,
            own,
            accepted: false,
            actions_set: false,
            dropped: false,
        };
        let ev = o.record(NHSHELL_EV_DND_ENTER);
        dnd.offer = Some(o);
        drop(sel);
        self.post(ev);
    }

    pub(super) fn dnd_motion(&mut self, time: u32, x: f64, y: f64) {
        let shared = self.shared.clone();
        let mut sel = selection::lock(&shared);
        let Some(window) = sel
            .dnd
            .offer
            .as_ref()
            .filter(|o| !o.dropped)
            .map(|o| o.window)
        else {
            return;
        };
        let (x, y) = self.to_device(window, x, y);
        let Some(o) = sel.dnd.offer.as_mut() else {
            return;
        };
        (o.x, o.y, o.time_ms) = (x, y, time);
        let ev = o.record(NHSHELL_EV_DND_MOTION);
        drop(sel);
        self.post(ev);
    }

    pub(super) fn dnd_leave(&mut self) {
        let shared = self.shared.clone();
        let mut sel = selection::lock(&shared);
        let dnd = &mut sel.dnd;
        dnd.over = false;
        dnd.sync_dragging(&shared);
        // after a drop the offer waits for the host's finish
        let Some(o) = dnd.offer.take_if(|o| !o.dropped) else {
            return;
        };
        let ev = o.record(NHSHELL_EV_DND_LEAVE);
        o.offer.destroy();
        drop(sel);
        self.post(ev);
    }

    pub(super) fn dnd_drop(&mut self) {
        let shared = self.shared.clone();
        let mut sel = selection::lock(&shared);
        let dnd = &mut sel.dnd;
        dnd.over = false;
        dnd.sync_dragging(&shared);
        let Some(o) = dnd.offer.as_mut() else { return };
        o.dropped = true;
        let ev = o.record(NHSHELL_EV_DND_DROP);
        drop(sel);
        self.post(ev);
    }

    fn drag_source_event(&mut self, source: &WlDataSource, event: wl_data_source::Event) {
        let shared = self.shared.clone();
        let mut sel = selection::lock(&shared);
        let dnd = &mut sel.dnd;
        let Some(drag) = dnd.drag.as_mut().filter(|d| d.source == *source) else {
            // a source we gave up (`drag_cancel`, a newer drag): only its descriptor needs closing, which dropping
            // the event does
            return;
        };
        let ev = match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                let bytes = dnd.kept_bytes(&mime_type);
                drop(sel);
                if let Some(send) = bytes.and_then(|b| SendBytes::start(fd, b)) {
                    self.sends.push(send);
                }
                return;
            }
            wl_data_source::Event::Action { dnd_action } => {
                drag.action = bits(dnd_action);
                drag_record(NHSHELL_EV_DRAG_ACTION, drag.window, drag.action, false)
            }
            wl_data_source::Event::DndDropPerformed => {
                drag_record(NHSHELL_EV_DRAG_DROPPED, drag.window, drag.action, false)
            }
            wl_data_source::Event::DndFinished => dnd.drag_end(&shared, true),
            wl_data_source::Event::Cancelled => dnd.drag_end(&shared, false),
            _ => return,
        };
        drop(sel);
        self.post(ev);
    }
}

impl Dnd {
    /// Our drag ended (finished or cancelled): its source goes, and the one `DRAG_DONE`.
    fn drag_end(&mut self, shared: &Shared, ok: bool) -> NhShellEvent {
        let drag = self.drag.take();
        self.sync_dragging(shared);
        let (window, action) = drag.map_or((0, 0), |d| {
            d.source.destroy();
            (d.window, if ok { d.action } else { 0 })
        });
        drag_record(NHSHELL_EV_DRAG_DONE, window, action, ok)
    }
}

/// The host's side, behind `ShellBackend`'s drag-and-drop calls.
impl WaylandShell {
    /// `NHSHELL_SEL_DND`'s bytes as `mime_type`: our own without a round trip, another client's through a pipe read
    /// for `timeout_ms` at most (`Disconnected` at once when the display goes).
    pub(super) fn dnd_read(&mut self, mime_type: &str, timeout_ms: u32) -> ShellResult<Vec<u8>> {
        let fd = {
            let sel = selection::lock(&self.shared);
            let dnd = &sel.dnd;
            if dnd.reads_kept() {
                return dnd
                    .kept_bytes(mime_type)
                    .map(|b| b.to_vec())
                    .ok_or(ShellError::State);
            }
            let o = dnd.offer.as_ref().ok_or(ShellError::State)?;
            let has = o
                .data()
                .is_some_and(|d| d.types.list().iter().any(|t| t == mime_type));
            if !has {
                return Err(ShellError::State);
            }
            // sent while the lock is held, so the reader cannot destroy the offer in between
            receive_pipe(&o.offer, mime_type)?
        };
        self.flush()?;
        read_with_timeout(
            fd,
            Duration::from_millis(timeout_ms.into()),
            self.shared.gone.fd(),
        )
    }

    pub(super) fn dnd_status_impl(
        &mut self,
        mime: Option<&str>,
        actions: u32,
        preferred: u32,
    ) -> ShellResult {
        let bad_preferred =
            preferred != 0 && (preferred.count_ones() != 1 || actions & preferred == 0);
        if actions & !ALL_ACTIONS != 0 || bad_preferred {
            return Err(ShellError::Arg);
        }
        {
            let mut sel = selection::lock(&self.shared);
            let o = sel.dnd.offer.as_mut().ok_or(ShellError::State)?;
            o.offer.accept(o.serial, mime.map(str::to_owned));
            if o.offer.version() >= 3 {
                o.offer.set_actions(
                    DndAction::from_bits_truncate(actions),
                    DndAction::from_bits_truncate(preferred),
                );
            }
            o.accepted = mime.is_some();
            o.actions_set = actions != 0;
        }
        self.flush()
    }

    pub(super) fn dnd_finish_impl(&mut self, success: bool) -> ShellResult {
        let o = selection::lock(&self.shared)
            .dnd
            .offer
            .take()
            .ok_or(ShellError::State)?;
        // `finish` without all of these is a protocol error, which would end the connection
        let can =
            o.dropped && o.accepted && o.actions_set && o.action() != 0 && o.offer.version() >= 3;
        if success && can {
            o.offer.finish();
        }
        o.offer.destroy();
        self.flush()?;
        if success && !can {
            return Err(ShellError::State);
        }
        Ok(())
    }

    pub(super) fn drag_start_impl(
        &mut self,
        window: WindowId,
        items: &[(&str, &[u8])],
        actions: u32,
    ) -> ShellResult {
        if items.is_empty() || actions == 0 || actions & !ALL_ACTIONS != 0 {
            return Err(ShellError::Arg);
        }
        let surface = self
            .main
            .surfaces
            .get(&window)
            .ok_or(ShellError::State)?
            .clone();
        let qh = self.main.reader_qh.clone().ok_or(ShellError::State)?;
        let (serial, _touch) = self
            .shared
            .serials
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drag_serial()
            .ok_or(ShellError::State)?;
        let shared = self.shared.clone();
        let mut sel = selection::lock(&shared);
        let s = &mut *sel;
        let (Some(manager), Some(device)) = (&s.data_manager, &s.data_device) else {
            return Err(ShellError::Unsupported);
        };
        let source = manager.create_data_source(&qh, DragSource);
        for (mime, _) in items {
            source.offer((*mime).to_owned());
        }
        if source.version() >= 3 {
            source.set_actions(DndAction::from_bits_truncate(actions));
        }
        // no icon: the compositor shows its drag cursor
        device.start_drag(Some(&source), &surface, None, serial);
        s.dnd.kept = items
            .iter()
            .map(|&(m, b)| (m.to_owned(), Arc::from(b)))
            .collect();
        let old = s.dnd.drag.replace(Drag {
            source,
            window,
            action: 0,
        });
        s.dnd.sync_dragging(&shared);
        drop(sel);
        if let Some(old) = old {
            old.source.destroy();
        }
        self.flush()
    }

    pub(super) fn drag_cancel_impl(&mut self) -> ShellResult {
        let drag = {
            let mut sel = selection::lock(&self.shared);
            let drag = sel.dnd.drag.take();
            sel.dnd.sync_dragging(&self.shared);
            drag
        };
        match drag {
            Some(d) => {
                d.source.destroy();
                self.flush()
            }
            None => Ok(()),
        }
    }
}

impl Dispatch<WlDataOffer, DataOfferData> for ReaderState {
    fn event(
        _: &mut Self,
        _: &WlDataOffer,
        event: wl_data_offer::Event,
        data: &DataOfferData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_offer::Event::Offer { mime_type } => data.types.add(mime_type),
            wl_data_offer::Event::SourceActions { source_actions } => data
                .source_actions
                .store(bits(source_actions), Ordering::Release),
            wl_data_offer::Event::Action { dnd_action } => {
                data.action.store(bits(dnd_action), Ordering::Release)
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataSource, DragSource> for ReaderState {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        _: &DragSource,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.drag_source_event(source, event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggested_action() {
        assert_eq!(
            suggested(NHSHELL_DND_COPY | NHSHELL_DND_MOVE, 0),
            NHSHELL_DND_COPY
        );
        assert_eq!(
            suggested(NHSHELL_DND_MOVE | NHSHELL_DND_ASK, 0),
            NHSHELL_DND_MOVE
        );
        assert_eq!(
            suggested(NHSHELL_DND_ASK, NHSHELL_DND_MOVE),
            NHSHELL_DND_MOVE
        );
        assert_eq!(suggested(0, 0), 0);
    }

    #[test]
    fn action_bits_are_the_protocols() {
        assert_eq!(bits(WEnum::Value(DndAction::Copy)), NHSHELL_DND_COPY);
        assert_eq!(bits(WEnum::Value(DndAction::Move)), NHSHELL_DND_MOVE);
        assert_eq!(bits(WEnum::Value(DndAction::Ask)), NHSHELL_DND_ASK);
        assert_eq!(bits(WEnum::Unknown(0xff)), ALL_ACTIONS);
    }
}
