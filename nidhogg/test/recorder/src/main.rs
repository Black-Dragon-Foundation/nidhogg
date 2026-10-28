//! nh-recorder: a recording Wayland compositor for Nidhogg's client tests.  Test-only; never shipped.
mod clock;
mod control;
mod dnd;
mod extras;
mod input;
mod log;
mod render;
mod restart;
mod scale;
mod selection;
mod shell;
mod state;
mod touch;

use calloop::generic::Generic;
use calloop::signals::{Signal, Signals};
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, Mode, PostAction};
use smithay::reexports::wayland_server::Display;
use state::{Clock, Profile, State};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

struct Args {
    socket: String,
    control: PathBuf,
    profile: PathBuf,
    clock: Clock,
    output: (i32, i32, u32),
    refresh_mhz: i32,
    session_store: Option<PathBuf>,
}

fn usage() -> ! {
    eprintln!(
        "usage: nh-recorder --socket NAME --control PATH --profile FILE [--clock auto|manual] [--output WxH@SCALE120] [--refresh-mhz N] [--session-store FILE]"
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
    let mut refresh_mhz = 60_000;
    let mut session_store = None;
    while let Some(a) = it.next() {
        // `--name=value` or `--name value`
        let (a, v) = match a.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k.to_string(), v.to_string()),
            _ => (a, it.next().unwrap_or_else(|| usage())),
        };
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
            "--refresh-mhz" => {
                refresh_mhz = v.parse().ok().filter(|n| *n > 0).unwrap_or_else(|| usage())
            }
            "--session-store" => session_store = Some(PathBuf::from(v)),
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
            refresh_mhz,
            session_store,
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
        args.refresh_mhz,
        event_loop.get_signal(),
        handle.clone(),
    );
    if let Some(path) = args.session_store.clone() {
        state.extras.use_store(path).unwrap_or_else(|e| die(e));
    }

    // the Wayland display and its listening socket; the display is shared with the restart's kick below
    let display = std::rc::Rc::new(std::cell::RefCell::new(display));
    let display_fd = {
        use std::os::fd::{AsFd, AsRawFd};
        display.borrow().as_fd().as_raw_fd()
    };
    let d = display.clone();
    handle
        .insert_source(
            // SAFETY: the display's poll descriptor lives as long as the display, which this source's closure keeps
            Generic::new(
                unsafe { std::os::fd::BorrowedFd::borrow_raw(display_fd) },
                Interest::READ,
                Mode::Level,
            ),
            move |_, _, state| {
                d.borrow_mut().dispatch_clients(state).unwrap();
                Ok(PostAction::Continue)
            },
        )
        .unwrap_or_else(|e| die(e.to_string()));
    // a restart's killed clients are cleaned up (their sockets closed) at once, not at the next dispatch that some
    // client's request starts
    let (kick, kick_source) = calloop::ping::make_ping().unwrap_or_else(|e| die(e.to_string()));
    let d = display.clone();
    handle
        .insert_source(kick_source, move |_, _, state| {
            if let Some(id) = state.restart.killed.take() {
                let _ = d.borrow_mut().backend().dispatch_single_client(state, id);
            }
        })
        .unwrap_or_else(|e| die(e.to_string()));
    state.restart.kick = Some(kick);
    state.restart.configure(
        &args.socket,
        &args.profile,
        profile,
        args.output,
        args.refresh_mhz,
    );
    restart::listen(&mut state).unwrap_or_else(|e| die(e));

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
    let frame = state.pacing.frame;
    handle
        .insert_source(Timer::from_duration(frame), move |_, _, state| {
            state.tick();
            TimeoutAction::ToDuration(frame)
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
