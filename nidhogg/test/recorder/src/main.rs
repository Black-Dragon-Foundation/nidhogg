//! nh-recorder: a recording Wayland compositor for Nidhogg's client tests.  Test-only; never shipped.
mod clock;
mod control;
mod input;
mod log;
mod render;
mod scale;
mod selection;
mod shell;
mod state;

use calloop::generic::Generic;
use calloop::signals::{Signal, Signals};
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, Mode, PostAction};
use smithay::reexports::wayland_server::Display;
use smithay::wayland::socket::ListeningSocketSource;
use state::{Clock, Profile, State};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;

struct Args {
    socket: String,
    control: PathBuf,
    profile: PathBuf,
    clock: Clock,
    output: (i32, i32, u32),
}

fn usage() -> ! {
    eprintln!(
        "usage: nh-recorder --socket NAME --control PATH --profile FILE [--clock auto|manual] [--output WxH@SCALE120]"
    );
    std::process::exit(2)
}

fn parse_output(s: &str) -> Option<(i32, i32, u32)> {
    let (size, scale) = s.split_once('@')?;
    let (w, h) = size.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?, scale.parse().ok()?))
}

fn parse_args() -> Args {
    let mut it = std::env::args().skip(1);
    let (mut socket, mut control, mut profile) = (None, None, None);
    let mut clock = Clock::Auto;
    let mut output = (1920, 1080, 120);
    while let Some(a) = it.next() {
        let v = it.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--socket" => socket = Some(v),
            "--control" => control = Some(PathBuf::from(v)),
            "--profile" => profile = Some(PathBuf::from(v)),
            "--clock" => {
                clock = match v.as_str() {
                    "auto" => Clock::Auto,
                    "manual" => Clock::Manual,
                    _ => usage(),
                }
            }
            "--output" => output = parse_output(&v).unwrap_or_else(|| usage()),
            _ => usage(),
        }
    }
    match (socket, control, profile) {
        (Some(socket), Some(control), Some(profile)) => Args {
            socket,
            control,
            profile,
            clock,
            output,
        },
        _ => usage(),
    }
}

fn die(msg: String) -> ! {
    eprintln!("nh-recorder: {msg}");
    std::process::exit(1)
}

fn main() {
    let args = parse_args();
    let profile = Profile::load(&args.profile).unwrap_or_else(|e| die(e));
    let mut event_loop: EventLoop<State> =
        EventLoop::try_new().unwrap_or_else(|e| die(e.to_string()));
    let handle = event_loop.handle();

    let display: Display<State> = Display::new().unwrap_or_else(|e| die(e.to_string()));
    let mut state = State::new(
        &display.handle(),
        &profile,
        args.clock,
        args.output,
        event_loop.get_signal(),
        handle.clone(),
    );

    // the Wayland display and its listening socket
    handle
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state| {
                // SAFETY: the display is not dropped while the source is registered
                unsafe { display.get_mut().dispatch_clients(state).unwrap() };
                Ok(PostAction::Continue)
            },
        )
        .unwrap_or_else(|e| die(e.to_string()));
    let listening =
        ListeningSocketSource::with_name(&args.socket).unwrap_or_else(|e| die(e.to_string()));
    handle
        .insert_source(listening, |stream, _, state| {
            let data = state.new_client_data();
            if let Ok(client) = state.display.insert_client(stream, Arc::new(data)) {
                state.note_client_pid(&client);
            }
        })
        .unwrap_or_else(|e| die(e.to_string()));

    // the control socket
    let _ = std::fs::remove_file(&args.control);
    let listener = UnixListener::bind(&args.control).unwrap_or_else(|e| die(e.to_string()));
    listener.set_nonblocking(true).unwrap();
    let conn_handle = handle.clone();
    handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            move |_, listener, _| {
                while let Ok((stream, _)) = listener.accept() {
                    add_control_connection(&conn_handle, stream);
                }
                Ok(PostAction::Continue)
            },
        )
        .unwrap_or_else(|e| die(e.to_string()));

    // SIGTERM / SIGINT end the loop
    let stop = event_loop.get_signal();
    handle
        .insert_source(
            Signals::new(&[Signal::SIGTERM, Signal::SIGINT]).unwrap_or_else(|e| die(e.to_string())),
            move |_, _, _| stop.stop(),
        )
        .unwrap_or_else(|e| die(e.to_string()));

    // frame clock tick and ping timeouts
    handle
        .insert_source(Timer::from_duration(clock::FRAME), |_, _, state| {
            state.tick();
            TimeoutAction::ToDuration(clock::FRAME)
        })
        .unwrap_or_else(|e| die(e.to_string()));

    println!("READY");
    std::io::stdout().flush().ok();

    event_loop
        .run(None, &mut state, |state| {
            let _ = state.display.flush_clients();
        })
        .unwrap_or_else(|e| die(e.to_string()));

    drop(event_loop);
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_default();
    let _ = std::fs::remove_file(runtime.join(&args.socket));
    let _ = std::fs::remove_file(runtime.join(format!("{}.lock", args.socket)));
    let _ = std::fs::remove_file(&args.control);
}

fn add_control_connection(handle: &calloop::LoopHandle<'static, State>, stream: UnixStream) {
    stream.set_nonblocking(true).ok();
    let mut pending: Vec<u8> = Vec::new();
    let _ = handle.insert_source(
        Generic::new(stream, Interest::READ, Mode::Level),
        move |_, stream, state| {
            let mut buf = [0u8; 4096];
            loop {
                match (&**stream).read(&mut buf) {
                    Ok(0) => return Ok(PostAction::Remove),
                    Ok(n) => pending.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return Ok(PostAction::Remove),
                }
            }
            let stream_ref: &UnixStream = stream;
            while let Some(i) = pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = pending.drain(..=i).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Some(reply) = control::handle(state, line, stream_ref) {
                    control::write_reply(state, stream_ref, &reply);
                }
            }
            Ok(PostAction::Continue)
        },
    );
}
