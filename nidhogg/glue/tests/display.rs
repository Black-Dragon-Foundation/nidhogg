//! T1 tests of the display: the registry, the ordered close and the poisoned one, against nh-recorder (the shell's
//! harness, `../shell/tests/common`).  The tests run one at a time (`SERIAL`): one of them counts the process's
//! descriptors and threads.

#[path = "../../shell/tests/common/mod.rs"]
mod common;

use common::Recorder;
use nhglue::abi::*;
use nhglue::ffi::*;
use nhglue::frame::*;
use std::ptr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static SERIAL: Mutex<()> = Mutex::new(());

fn open(rec: &Recorder) -> *mut NhgDisplay {
    let name = rec.display();
    // SAFETY: the name is NUL-terminated; no error buffer.
    let d = unsafe { nhg_open(name.as_ptr(), ptr::null_mut(), 0) };
    assert!(!d.is_null(), "nhg_open failed on the recorder");
    d
}

/// The entries of a /proc/self directory.
fn count(dir: &str) -> usize {
    std::fs::read_dir(dir).unwrap().count()
}

/// What each open descriptor is (`socket:[…]`, `anon_inode:[eventfd]`, `/memfd:…`), sorted.
fn fd_kinds() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .map(|p| {
            let s = p.display().to_string();
            // a socket's or pipe's inode number differs per descriptor: keep the kind
            s.split(':').next().unwrap_or("").to_string()
        })
        .collect();
    v.sort();
    v
}

/// The recorder's clients (the `clients` reply lists one `"id"` per client).
fn clients(rec: &Recorder) -> usize {
    rec.control(r#"{"cmd":"clients"}"#)
        .matches("\"id\"")
        .count()
}

/// The plan names these `display::…`.
#[allow(clippy::module_inception)]
mod display {
    use super::*;

    /// A forgotten window's frame is gone for good: calls naming it are refused, and when the shell hands the same id
    /// out again (its ids are protocol object ids, which the client library reuses) it starts as a new frame.
    #[test]
    fn forgotten_window_is_never_reused() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let facts = NhgFrameFacts::default();
        // SAFETY: a live display; the facts are readable.
        unsafe {
            assert_eq!(nhg_frame_register(d, 0, 0), NHG_E_ARG);
            assert_eq!(nhg_frame_register(d, 5, 8), NHG_E_ARG);
            assert_eq!(nhg_frame_register(d, 5, NHG_FRAME_CHILD), NHG_OK);
            assert_eq!(nhg_frame_register(d, 5, 0), NHG_E_STATE);
            assert_eq!(nhg_frame_forget(d, 5), NHG_OK);
            assert_eq!(nhg_frame_forget(d, 5), NHG_E_STATE);
            assert_eq!(nhg_debug_check_frame(d, 5, &facts), NHG_E_STATE);
            assert_eq!(nhg_frame_forget(d, 6), NHG_E_STATE);
        }
        {
            let state = d_ref(d).lock_for_test();
            assert!(state.is_forgotten(5) && state.frame(5).is_err());
            assert!(!state.is_forgotten(6));
        }
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_frame_register(d, 5, NHG_FRAME_TIP) }, NHG_OK);
        {
            let state = d_ref(d).lock_for_test();
            assert!(!state.is_forgotten(5));
            assert_eq!(state.frame(5).unwrap().flags, NHG_FRAME_TIP);
        }
        // SAFETY: a live display, not used again.
        assert_eq!(unsafe { nhg_close(d) }, NHG_OK);
    }

    /// 50 opens and closes leave no descriptor and no thread behind (the first cycle is the baseline's warm-up).
    #[test]
    fn open_close_frees_everything() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("mutter");
        // SAFETY: a live display, not used again.
        assert_eq!(unsafe { nhg_close(open(&rec)) }, NHG_OK);
        let (fds, threads) = (fd_kinds(), count("/proc/self/task"));
        for i in 0..50 {
            let d = open(&rec);
            // SAFETY: a live display.
            unsafe {
                assert_eq!(nhg_frame_register(d, 100 + i, 0), NHG_OK);
                assert!(nhg_fd(d) >= 0);
                assert_eq!(nhg_close(d), NHG_OK);
            }
        }
        assert_eq!(fd_kinds(), fds, "descriptors left behind");
        // other tests' threads may end meanwhile, never start (they wait for SERIAL)
        assert!(count("/proc/self/task") <= threads, "threads left behind");
    }

    /// A poisoned display closes without touching its state: the connection is shut down, so the compositor sees the
    /// client go and destroys its windows, and the close takes at most the bounded wait.
    #[test]
    fn poisoned_close_shuts_down_and_leaks() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rec = Recorder::start("mutter");
        let d = open(&rec);
        let title = c"poisoned";
        let desc = nhshell::ffi::NhShellWindowDesc {
            w: 200,
            h: 100,
            title: title.as_ptr(),
            app_id: title.as_ptr(),
            flags: 0,
            kind: 0,
            parent: 0,
            x: 0,
            y: 0,
            anchor: Default::default(),
            session_name: ptr::null(),
        };
        let mut id = 0u64;
        // SAFETY: a live display and its shell handle; the descriptor's strings are NUL-terminated.
        unsafe {
            let sh = nhg_shell(d);
            assert_eq!(nhshell::ffi::nhshell_window_new(sh, &desc, &mut id), 0);
            assert_eq!(nhg_frame_register(d, id, 0), NHG_OK);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while rec.surfaces("toplevel") == 0 {
            assert!(
                Instant::now() < deadline,
                "the window never reached the recorder"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(clients(&rec), 1);
        // SAFETY: a live display.
        assert_eq!(unsafe { nhg_test_panic(d) }, NHG_E_PANIC);
        let t0 = Instant::now();
        // SAFETY: a live display, not used again.
        assert_eq!(unsafe { nhg_close(d) }, NHG_OK);
        let took = t0.elapsed();
        assert!(took <= Duration::from_millis(250), "close took {took:?}");
        while clients(&rec) > 0 {
            assert!(
                t0.elapsed() < Duration::from_millis(200),
                "the recorder still lists the client"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(rec.surfaces("toplevel"), 0, "a window stayed on screen");
    }
}

fn d_ref<'a>(d: *mut NhgDisplay) -> &'a NhgDisplay {
    // SAFETY: the tests pass live displays only.
    unsafe { &*d }
}
