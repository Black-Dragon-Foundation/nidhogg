// SPDX-License-Identifier: GPL-3.0-or-later AND MIT
// Copyright 2019 Daniel Eklöf
// Adapted-from: https://codeberg.org/dnkl/foot@cb2771788998f6a6288e2de8273c55a8214551e0 ime.c
// Modified for Nidhogg: the logic of foot's text-input-v3 handling (pending state applied on `done`, the commit
// serial check, enable on enter and disable on leave) rewritten in Rust; the state is per window and the results are
// event records for the host instead of terminal cells.

//! text-input-v3: preedit, commit and surrounding text.
//!
//! One `zwp_text_input_v3` object lives on the reader queue, so its events run on the reader thread; the host's
//! requests (`enable`, `cursor`, `surrounding`) run on the host's thread.  Both sides go through one mutex.  The
//! compositor's `preedit_string`, `commit_string` and `delete_surrounding_text` change pending state only; `done`
//! applies it in the protocol's order and posts the records.  Texts travel as ids (`take_text`), single-use.

use crate::api::{ShellError, ShellResult};
use crate::event::{
    NHSHELL_EV_COMMIT_TEXT, NHSHELL_EV_DELETE_SURROUNDING, NHSHELL_EV_PREEDIT, NhShellEvent, TextEv,
};
use crate::wayland::reader::ReaderState;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3;
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{
    self, ChangeCause, ContentHint, ContentPurpose, ZwpTextInputV3,
};

/// The most surrounding text the protocol carries, in bytes.
pub const SURROUNDING_MAX: usize = 4000;
/// Texts handed out and not yet taken; the oldest go first.
const TEXTS_MAX: usize = 256;

/// What the host asked for a window; re-sent whenever the window gets text-input focus.
#[derive(Default)]
struct WinState {
    enabled: bool,
    rect: Option<[i32; 4]>,
    /// The rectangle the compositor holds (reset when focus is lost).
    sent_rect: Option<[i32; 4]>,
    surrounding: Option<(String, i32, i32)>,
}

#[derive(Default)]
struct Pending {
    preedit: Option<(String, i32, i32)>,
    commit: Option<String>,
    delete: Option<(u32, u32)>,
}

#[derive(Default)]
struct Inner {
    obj: Option<ZwpTextInputV3>,
    /// The window with text-input focus.
    entered: Option<u64>,
    wins: HashMap<u64, WinState>,
    /// `commit` requests sent: a `done` carrying another serial is stale.
    commits: u32,
    pending: Pending,
    preedit_active: bool,
    texts: HashMap<u32, Vec<u8>>,
    next_id: u32,
}

#[derive(Default)]
pub struct TextInput {
    inner: Mutex<Inner>,
}

/// `text` cut to at most `SURROUNDING_MAX` bytes around `cursor` on character boundaries, with `cursor` and `anchor`
/// moved into the cut text.  `cursor` and `anchor` are byte offsets into `text`.
pub fn truncate_surrounding(text: &str, cursor: usize, anchor: usize) -> (String, i32, i32) {
    let mut start = 0;
    let mut end = text.len();
    if end > SURROUNDING_MAX {
        start = cursor.saturating_sub(SURROUNDING_MAX / 2);
        end = start + SURROUNDING_MAX;
        if end > text.len() {
            end = text.len();
            start = end - SURROUNDING_MAX;
        }
        while !text.is_char_boundary(start) {
            start += 1;
        }
        while !text.is_char_boundary(end) {
            end -= 1;
        }
    }
    let rel = |p: usize| p.clamp(start, end) - start;
    (
        text[start..end].to_owned(),
        rel(cursor) as i32,
        rel(anchor) as i32,
    )
}

impl Inner {
    fn store(&mut self, text: String) -> u32 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        if self.texts.len() >= TEXTS_MAX
            && let Some(&old) = self.texts.keys().min()
        {
            self.texts.remove(&old);
        }
        self.texts.insert(self.next_id, text.into_bytes());
        self.next_id
    }

    fn commit(&mut self) {
        if let Some(o) = &self.obj {
            o.commit();
            self.commits = self.commits.wrapping_add(1);
        }
    }

    fn set_surrounding(&self, s: &(String, i32, i32)) {
        if let Some(o) = &self.obj {
            o.set_surrounding_text(s.0.clone(), s.1, s.2);
            o.set_text_change_cause(ChangeCause::Other);
        }
    }

    /// Enable text input for `window` (it has focus) with everything the host told us.
    fn send_enable(&mut self, window: u64) {
        let Some(o) = self.obj.clone() else { return };
        let Some(w) = self.wins.get_mut(&window) else {
            return;
        };
        o.enable();
        if let Some(s) = &w.surrounding {
            o.set_surrounding_text(s.0.clone(), s.1, s.2);
            o.set_text_change_cause(ChangeCause::Other);
        }
        o.set_content_type(ContentHint::None, ContentPurpose::Normal);
        w.sent_rect = w.rect;
        if let Some([x, y, w, h]) = w.rect {
            o.set_cursor_rectangle(x, y, w, h);
        }
        self.commit();
    }

    fn event(&mut self, kind: u32, window: u64, text: Option<String>, tev: TextEv) -> NhShellEvent {
        let mut ev = NhShellEvent::new(kind, window, 0);
        let id = text.map_or(0, |t| self.store(t));
        ev.u.text = TextEv { id, ..tev };
        ev
    }

    fn clear_preedit(&mut self, window: u64, out: &mut Vec<NhShellEvent>) {
        self.preedit_active = false;
        out.push(self.event(NHSHELL_EV_PREEDIT, window, None, TextEv::default()));
    }
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl TextInput {
    pub fn set_object(&self, obj: ZwpTextInputV3) {
        lock(&self.inner).obj = Some(obj);
    }

    /// Whether the compositor offers text input at all.
    pub fn available(&self) -> bool {
        lock(&self.inner).obj.is_some()
    }

    /// The host turns text input for `window` on or off.  Whether a request went out is the result.
    pub fn enable(&self, window: u64, on: bool) -> ShellResult<bool> {
        let mut i = lock(&self.inner);
        if i.obj.is_none() {
            return Err(ShellError::Unsupported);
        }
        let w = i.wins.entry(window).or_default();
        if w.enabled == on {
            return Ok(false);
        }
        w.enabled = on;
        if i.entered != Some(window) {
            return Ok(false);
        }
        if on {
            i.send_enable(window);
        } else {
            if let Some(o) = &i.obj {
                o.disable();
            }
            if let Some(w) = i.wins.get_mut(&window) {
                w.sent_rect = None;
            }
            i.commit();
        }
        Ok(true)
    }

    /// The cursor rectangle (logical pixels in the window); sent only when it changed.
    pub fn cursor(&self, window: u64, rect: [i32; 4]) -> ShellResult<bool> {
        let mut i = lock(&self.inner);
        if i.obj.is_none() {
            return Err(ShellError::Unsupported);
        }
        let live = i.entered == Some(window);
        let w = i.wins.entry(window).or_default();
        w.rect = Some(rect);
        if !live || !w.enabled || w.sent_rect == Some(rect) {
            return Ok(false);
        }
        w.sent_rect = Some(rect);
        if let Some(o) = &i.obj {
            o.set_cursor_rectangle(rect[0], rect[1], rect[2], rect[3]);
        }
        i.commit();
        Ok(true)
    }

    /// The text around the cursor, cut to `SURROUNDING_MAX` bytes; `cursor` and `anchor` are byte offsets.
    pub fn surrounding(
        &self,
        window: u64,
        text: &str,
        cursor: usize,
        anchor: usize,
    ) -> ShellResult<bool> {
        if cursor > text.len()
            || anchor > text.len()
            || !text.is_char_boundary(cursor)
            || !text.is_char_boundary(anchor)
        {
            return Err(ShellError::Arg);
        }
        let s = truncate_surrounding(text, cursor, anchor);
        let mut i = lock(&self.inner);
        if i.obj.is_none() {
            return Err(ShellError::Unsupported);
        }
        let live = i.entered == Some(window);
        let w = i.wins.entry(window).or_default();
        if w.surrounding.as_ref() == Some(&s) {
            return Ok(false);
        }
        let enabled = w.enabled;
        w.surrounding = Some(s.clone());
        if !live || !enabled {
            return Ok(false);
        }
        i.set_surrounding(&s);
        i.commit();
        Ok(true)
    }

    /// A window is gone: forget what the host asked for it.
    pub fn forget(&self, window: u64) {
        let mut i = lock(&self.inner);
        i.wins.remove(&window);
        if i.entered == Some(window) {
            i.entered = None;
            i.preedit_active = false;
            i.pending = Pending::default();
        }
    }

    /// The length of text `id`.
    pub fn text_len(&self, id: u32) -> ShellResult<usize> {
        lock(&self.inner)
            .texts
            .get(&id)
            .map(Vec::len)
            .ok_or(ShellError::State)
    }

    /// Take text `id` (single-use).
    pub fn take_text(&self, id: u32) -> ShellResult<Vec<u8>> {
        lock(&self.inner).texts.remove(&id).ok_or(ShellError::State)
    }

    fn enter(&self, window: u64) {
        let mut i = lock(&self.inner);
        i.entered = Some(window);
        i.pending = Pending::default();
        i.preedit_active = false;
        let on = i.wins.get(&window).is_some_and(|w| w.enabled);
        if on {
            i.send_enable(window);
        }
    }

    fn leave(&self, out: &mut Vec<NhShellEvent>) {
        let mut i = lock(&self.inner);
        let Some(window) = i.entered.take() else {
            return;
        };
        i.pending = Pending::default();
        if let Some(w) = i.wins.get_mut(&window) {
            w.sent_rect = None;
        }
        if i.preedit_active {
            i.clear_preedit(window, out);
        }
    }

    /// `done`: apply the pending state in the protocol's order: replace the preedit, delete surrounding text, insert
    /// the commit string, insert the new preedit.
    fn done(&self, serial: u32, out: &mut Vec<NhShellEvent>) {
        let mut i = lock(&self.inner);
        // foot: a `done` for an older state is ignored
        if serial != i.commits {
            return;
        }
        let p = std::mem::take(&mut i.pending);
        let Some(window) = i.entered else { return };
        let new_preedit = p.preedit.filter(|(t, ..)| !t.is_empty());
        let commit = p.commit.filter(|t| !t.is_empty());
        if i.preedit_active && (p.delete.is_some() || commit.is_some() || new_preedit.is_none()) {
            i.clear_preedit(window, out);
        }
        if let Some((before, after)) = p.delete {
            let tev = TextEv {
                before,
                after,
                ..TextEv::default()
            };
            out.push(i.event(NHSHELL_EV_DELETE_SURROUNDING, window, None, tev));
        }
        if let Some(text) = commit {
            let len = text.len() as u32;
            let tev = TextEv {
                len,
                ..TextEv::default()
            };
            out.push(i.event(NHSHELL_EV_COMMIT_TEXT, window, Some(text), tev));
        }
        if let Some((text, begin, end)) = new_preedit {
            let tev = TextEv {
                len: text.len() as u32,
                cursor_begin: begin,
                cursor_end: end,
                ..TextEv::default()
            };
            out.push(i.event(NHSHELL_EV_PREEDIT, window, Some(text), tev));
            i.preedit_active = true;
        }
    }
}

delegate_noop!(ReaderState: ZwpTextInputManagerV3);

impl Dispatch<ZwpTextInputV3, ()> for ReaderState {
    fn event(
        state: &mut Self,
        _: &ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_text_input_v3::Event as E;
        let ti = state.text_input.clone();
        let window = |s: &WlSurface| u64::from(s.id().protocol_id());
        let mut out = Vec::new();
        match event {
            E::Enter { surface } => ti.enter(window(&surface)),
            E::Leave { .. } => ti.leave(&mut out),
            E::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {
                lock(&ti.inner).pending.preedit = text.map(|t| (t, cursor_begin, cursor_end));
            }
            E::CommitString { text } => lock(&ti.inner).pending.commit = text,
            E::DeleteSurroundingText {
                before_length,
                after_length,
            } => lock(&ti.inner).pending.delete = Some((before_length, after_length)),
            E::Done { serial } => ti.done(serial, &mut out),
            _ => {}
        }
        for ev in out {
            state.post(ev);
        }
    }
}
