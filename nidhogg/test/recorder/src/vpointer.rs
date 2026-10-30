//! `nh-recorder --virtual-pointer`: not the recorder but a client of the compositor on `$WAYLAND_DISPLAY`, for a
//! compositor without input devices (gui.sh's headless sway).  It makes a wlr virtual pointer, which gives the seat a
//! pointer, prints READY and then drives it from the lines on stdin:
//!   `move X Y W H`   the pointer to X, Y of a layout W x H (logical pixels)
//!   `press`, `release`   the left button
//! It exits at the end of stdin or when the compositor goes.

use std::io::BufRead;
use std::time::Instant;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_pointer::ButtonState;
use wayland_client::protocol::{wl_registry::WlRegistry, wl_seat::WlSeat};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

/// BTN_LEFT.
const LEFT: u32 = 0x110;

struct Client;

impl Dispatch<WlRegistry, GlobalListContents> for Client {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(Client: ignore WlSeat);
delegate_noop!(Client: ZwlrVirtualPointerManagerV1);
delegate_noop!(Client: ZwlrVirtualPointerV1);

/// Run the client; exits 0 at the end of stdin or when the compositor goes, 1 without a virtual pointer manager.
pub fn run() -> ! {
    let fail = |e: String| -> ! {
        eprintln!("nh-recorder --virtual-pointer: {e}");
        std::process::exit(1)
    };
    let conn = Connection::connect_to_env().unwrap_or_else(|e| fail(e.to_string()));
    let (globals, mut queue) =
        registry_queue_init::<Client>(&conn).unwrap_or_else(|e| fail(e.to_string()));
    let qh = queue.handle();
    let manager: ZwlrVirtualPointerManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .unwrap_or_else(|e| fail(e.to_string()));
    let seat: Option<WlSeat> = globals.bind(&qh, 1..=1, ()).ok();
    let pointer = manager.create_virtual_pointer(seat.as_ref(), &qh, ());
    queue
        .roundtrip(&mut Client)
        .unwrap_or_else(|e| fail(e.to_string()));
    println!("READY");
    let start = Instant::now();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let time = start.elapsed().as_millis() as u32;
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["move", x, y, w, h] => {
                let n: Vec<u32> = [x, y, w, h].iter().filter_map(|v| v.parse().ok()).collect();
                if let [x, y, w, h] = n.as_slice() {
                    pointer.motion_absolute(time, *x, *y, *w, *h);
                }
            }
            ["press"] => pointer.button(time, LEFT, ButtonState::Pressed),
            ["release"] => pointer.button(time, LEFT, ButtonState::Released),
            _ => {
                eprintln!("nh-recorder --virtual-pointer: unknown line {line:?}");
                continue;
            }
        }
        pointer.frame();
        if conn.flush().is_err() {
            break;
        }
        println!("ok");
    }
    std::process::exit(0)
}
