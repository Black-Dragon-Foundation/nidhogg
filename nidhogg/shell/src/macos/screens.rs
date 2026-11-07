//! The screens, as `OutputAdd` records: position in points, size in device pixels, the scale times 120.

use crate::event::OutputEv;
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;

/// One record per screen, the main screen first (id 1 up, in `NSScreen.screens` order).
pub fn outputs(mtm: MainThreadMarker) -> Vec<OutputEv> {
    NSScreen::screens(mtm)
        .iter()
        .enumerate()
        .map(|(i, screen)| {
            let frame = screen.frame();
            let scale = screen.backingScaleFactor();
            let name = screen.localizedName().to_string();
            let bytes = name.as_bytes();
            let n = bytes.len().min(31);
            let mut buf = [0u8; 32];
            buf[..n].copy_from_slice(&bytes[..n]);
            OutputEv {
                id: i as u64 + 1,
                x: frame.origin.x as i32,
                y: frame.origin.y as i32,
                w: (frame.size.width * scale).round() as i32,
                h: (frame.size.height * scale).round() as i32,
                mm_w: 0,
                mm_h: 0,
                scale120: (scale * 120.0).round() as u32,
                name: buf,
            }
        })
        .collect()
}
