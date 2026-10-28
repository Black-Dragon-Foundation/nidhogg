// SPDX-License-Identifier: GPL-3.0-or-later AND Apache-2.0
// Copyright 2022-2026 Zed Industries, Inc.
// Adapted-from: https://github.com/zed-industries/zed@347f171a3fd00a3b672382d78348b37bd969d17c crates/gpui_linux/src/linux/wayland/serial.rs
// Modified for Nidhogg: std's HashMap instead of Zed's `collections` crate, `pub` visibility for the shell's crate
// layout, this header, the touch-down kind with the grab serial (the newest press of any kind), the held presses with
// the drag serial (the newest press not yet released), and their tests.  The rest of the logic and the other tests
// are Zed's.

//! The latest input serial of each kind: requests such as an interactive move or a selection need the serial of the
//! input event that caused them.

use std::collections::HashMap;

#[derive(Debug, Hash, PartialEq, Eq)]
pub enum SerialKind {
    DataDevice,
    InputMethod,
    MouseEnter,
    MousePress,
    KeyPress,
    TouchDown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Serial(u32);

impl Serial {
    pub fn as_raw(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionSerial(Serial);

impl SelectionSerial {
    pub fn as_raw(self) -> u32 {
        self.0.as_raw()
    }
}

/// A press whose release has not come: a pointer button (its evdev code) or a touch point (its id).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    Button(u32),
    Touch(i32),
}

#[derive(Debug, Default)]
/// Helper for tracking of different serial kinds.
pub struct SerialTracker {
    serials: HashMap<SerialKind, Serial>,
    selection_serial: Option<SelectionSerial>,
    grab_serial: Option<Serial>,
    /// The presses still held, oldest first, with their serials.
    held: Vec<(Held, Serial)>,
}

impl SerialTracker {
    pub fn new() -> Self {
        Self {
            serials: HashMap::default(),
            selection_serial: None,
            grab_serial: None,
            held: Vec::new(),
        }
    }

    /// A button or touch point went down with `value`; it is held until `release`.
    pub fn press(&mut self, held: Held, value: u32) {
        self.held.retain(|(h, _)| *h != held);
        self.held.push((held, Serial(value)));
    }

    /// A button or touch point came up.
    pub fn release(&mut self, held: Held) {
        self.held.retain(|(h, _)| *h != held);
    }

    /// Forget every held button (the pointer left or entered a surface, or went away: no press of ours is held any
    /// more), or every held touch point (a touch cancel).
    pub fn release_all(&mut self, touch: bool) {
        self.held
            .retain(|(h, _)| matches!(h, Held::Touch(_)) != touch);
    }

    /// The pointer buttons still held, oldest first.
    pub fn held_buttons(&self) -> Vec<u32> {
        self.held
            .iter()
            .filter_map(|(h, _)| match h {
                Held::Button(b) => Some(*b),
                Held::Touch(_) => None,
            })
            .collect()
    }

    /// The serial a drag starts with: the newest press whose release has not come, and whether it is a touch point.
    pub fn drag_serial(&self) -> Option<(u32, bool)> {
        self.held
            .last()
            .map(|(h, s)| (s.as_raw(), matches!(h, Held::Touch(_))))
    }

    pub fn update(&mut self, kind: SerialKind, value: u32) {
        let serial = Serial(value);

        if matches!(&kind, SerialKind::KeyPress | SerialKind::MousePress) {
            self.selection_serial = Some(SelectionSerial(serial));
        }
        if matches!(
            &kind,
            SerialKind::KeyPress | SerialKind::MousePress | SerialKind::TouchDown
        ) {
            self.grab_serial = Some(serial);
        }

        self.serials.insert(kind, serial);
    }

    /// Returns the latest tracked serial of the provided [`SerialKind`].
    ///
    /// Returns a serial with a raw value of 0 if the kind has not been tracked.
    pub fn get(&self, kind: SerialKind) -> Serial {
        self.serials.get(&kind).copied().unwrap_or(Serial(0))
    }

    pub fn selection_serial(&self) -> Option<SelectionSerial> {
        self.selection_serial
    }

    /// The serial a popup grab is made with: the newest button press, key press or touch down, in arrival order.
    pub fn grab_serial(&self) -> Option<u32> {
        self.grab_serial.map(Serial::as_raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_selection_serial(serial_tracker: &SerialTracker) -> Option<u32> {
        serial_tracker
            .selection_serial()
            .map(SelectionSerial::as_raw)
    }

    #[test]
    fn test_selection_serial_ignores_unrelated_serial_kinds() {
        let mut serial_tracker = SerialTracker::new();
        serial_tracker.update(SerialKind::MousePress, 3783);
        serial_tracker.update(SerialKind::InputMethod, 5011);
        serial_tracker.update(SerialKind::KeyPress, 3787);
        serial_tracker.update(SerialKind::MouseEnter, 6000);
        serial_tracker.update(SerialKind::DataDevice, 7000);

        assert_eq!(raw_selection_serial(&serial_tracker), Some(3787));
    }

    #[test]
    fn test_selection_serial_uses_mouse_press_without_key_press() {
        let mut serial_tracker = SerialTracker::new();
        serial_tracker.update(SerialKind::MousePress, 3783);

        assert_eq!(raw_selection_serial(&serial_tracker), Some(3783));
    }

    #[test]
    fn test_selection_serial_uses_event_arrival_order_across_rollover() {
        let mut serial_tracker = SerialTracker::new();
        serial_tracker.update(SerialKind::KeyPress, 0xffff_fff0);
        serial_tracker.update(SerialKind::MousePress, 0x0000_0010);

        assert_eq!(raw_selection_serial(&serial_tracker), Some(0x0000_0010));
    }

    #[test]
    fn test_selection_serial_is_unavailable_without_eligible_input() {
        let mut serial_tracker = SerialTracker::new();

        assert_eq!(raw_selection_serial(&serial_tracker), None);

        serial_tracker.update(SerialKind::InputMethod, 5011);
        serial_tracker.update(SerialKind::MouseEnter, 6000);
        serial_tracker.update(SerialKind::DataDevice, 7000);

        assert_eq!(raw_selection_serial(&serial_tracker), None);
    }

    #[test]
    fn test_zero_is_a_valid_selection_serial() {
        let mut serial_tracker = SerialTracker::new();
        serial_tracker.update(SerialKind::KeyPress, 0);

        assert_eq!(raw_selection_serial(&serial_tracker), Some(0));
    }

    #[test]
    fn grab_serial_is_the_newest_press_of_any_kind() {
        let mut serial_tracker = SerialTracker::new();
        assert_eq!(serial_tracker.grab_serial(), None);
        serial_tracker.update(SerialKind::MouseEnter, 5);
        serial_tracker.update(SerialKind::InputMethod, 6);
        assert_eq!(serial_tracker.grab_serial(), None);
        serial_tracker.update(SerialKind::MousePress, 10);
        serial_tracker.update(SerialKind::KeyPress, 11);
        assert_eq!(serial_tracker.grab_serial(), Some(11));
        serial_tracker.update(SerialKind::TouchDown, 12);
        assert_eq!(serial_tracker.grab_serial(), Some(12));
        assert_eq!(raw_selection_serial(&serial_tracker), Some(11));
    }

    #[test]
    fn drag_serial_is_the_newest_held_press() {
        let mut t = SerialTracker::new();
        assert_eq!(t.drag_serial(), None);
        t.press(Held::Button(0x110), 20);
        t.press(Held::Touch(3), 21);
        assert_eq!(t.drag_serial(), Some((21, true)));
        t.release(Held::Touch(3));
        assert_eq!(t.drag_serial(), Some((20, false)));
        // a second press of the same button replaces the first
        t.press(Held::Button(0x110), 22);
        t.press(Held::Touch(4), 23);
        t.release_all(true);
        assert_eq!(t.drag_serial(), Some((22, false)));
        t.release_all(false);
        assert_eq!(t.drag_serial(), None);
    }
}
