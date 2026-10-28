//! T1 tests of session restore through the glue, against nh-recorder (mutter profile: `xdg_session_manager_v1`; sway
//! profile: none).  Each test stands in for C: it opens the session, makes windows with a session name in their
//! descriptor, and reads the answers as `nhg_next` gives them.

#[path = "../../shell/tests/common/mod.rs"]
mod common;
mod step;

use common::Recorder;
use nhglue::abi::*;
use nhglue::action::NhgAction;
use nhglue::ffi::*;
use nhglue::session::{
    NHG_ACT_SESSION, NHG_SESSION_CREATED, NHG_SESSION_LAUNCH, NHG_SESSION_REPLACED,
    NHG_SESSION_RESTORED,
};
use nhglue::window::NhgWindowDesc;
use nhshell::api::NHSHELL_KIND_TOPLEVEL;
use std::ffi::c_char;
use std::ptr;
use step::*;

const TITLE: &str = "t";
const APP_ID: &str = "nidhogg.test";

/// A top-level's descriptor, 200 x 100 at scale 1, joining the session as `name` (none: empty).
fn desc(name: &str) -> NhgWindowDesc {
    NhgWindowDesc {
        w: 200,
        h: 100,
        title: TITLE.as_ptr().cast::<c_char>(),
        title_len: TITLE.len(),
        app_id: APP_ID.as_ptr().cast::<c_char>(),
        app_id_len: APP_ID.len(),
        session_name: if name.is_empty() {
            ptr::null()
        } else {
            name.as_ptr().cast::<c_char>()
        },
        session_name_len: name.len(),
        flags: 0,
        kind: NHSHELL_KIND_TOPLEVEL,
        parent: 0,
        x: 0,
        y: 0,
        anchor: Default::default(),
        role: 0,
        facts: 0,
        native_w: 200,
        native_h: 100,
        scale120: 120,
    }
}

/// A window made through the glue under `name`.
fn named(d: *mut NhgDisplay, name: &str) -> (i32, u64) {
    let mut id = 0;
    // SAFETY: a live display; the descriptor and its texts are readable, `id` writable.
    let rc = unsafe { nhg_window_new(d, &desc(name), &mut id) };
    (rc, id)
}

fn open_session(d: *mut NhgDisplay, id: &str, reason: u32) -> i32 {
    // SAFETY: a live display; `id` is readable for its length.
    unsafe { nhg_session_open(d, id.as_ptr().cast::<c_char>(), id.len(), reason) }
}

/// The session's id, or the error code.
fn session_id(d: *mut NhgDisplay) -> Result<String, i32> {
    let mut buf = [0 as c_char; 128];
    let mut len = 0usize;
    // SAFETY: a live display; `buf` holds 128 writable bytes, `len` is writable.
    let rc = unsafe { nhg_session_id(d, buf.as_mut_ptr(), buf.len(), &mut len) };
    if rc != NHG_OK {
        return Err(rc);
    }
    let bytes: Vec<u8> = buf[..len].iter().map(|&c| c as u8).collect();
    Ok(String::from_utf8(bytes).unwrap())
}

/// Step until the session's answer comes; its code.
fn answer(d: *mut NhgDisplay) -> i32 {
    let seen = until(d, |a: &NhgAction| a.kind == NHG_ACT_SESSION);
    seen.last().unwrap().code
}

fn session_log(rec: &Recorder) -> String {
    rec.control(r#"{"cmd":"session_log"}"#)
}

#[test]
fn opening_gives_a_new_session_and_its_id() {
    let rec = Recorder::start("mutter");
    let d = open(&rec);
    assert_eq!(session_id(d), Err(NHG_E_STATE), "no session yet");
    assert_eq!(open_session(d, "", NHG_SESSION_LAUNCH), NHG_OK);
    assert_eq!(answer(d), NHG_SESSION_CREATED);
    assert!(!session_id(d).unwrap().is_empty());
    close(d);
}

#[test]
fn a_named_window_joins_before_its_first_commit() {
    let rec = Recorder::start("mutter");
    let d = open(&rec);
    assert_eq!(open_session(d, "", NHG_SESSION_LAUNCH), NHG_OK);
    answer(d);
    let (rc, w) = named(d, "initial");
    assert_eq!(rc, NHG_OK);
    configured(d, w);
    let log = session_log(&rec);
    assert!(log.contains("\"op\":\"add\""), "{log}");
    assert!(log.contains("\"before_first_commit\":true"), "{log}");
    close(d);
}

#[test]
fn a_saved_session_restores_the_name_and_replaces_the_old_holder() {
    let rec = Recorder::start("mutter");
    let a = open(&rec);
    assert_eq!(open_session(a, "", NHG_SESSION_LAUNCH), NHG_OK);
    answer(a);
    let id = session_id(a).unwrap();
    let (_, w) = named(a, "initial");
    configured(a, w);

    let b = open(&rec);
    assert_eq!(open_session(b, &id, NHG_SESSION_LAUNCH), NHG_OK);
    assert_eq!(answer(b), NHG_SESSION_RESTORED);
    assert_eq!(session_id(b).unwrap(), id);
    assert_eq!(answer(a), NHG_SESSION_REPLACED);
    let (rc, w2) = named(b, "initial");
    assert_eq!(rc, NHG_OK);
    configured(b, w2);
    let log = session_log(&rec);
    assert!(log.contains("\"op\":\"restore\""), "{log}");
    close(b);
    close(a);
}

#[test]
fn removing_a_window_takes_its_name_out_once() {
    let rec = Recorder::start("mutter");
    let d = open(&rec);
    assert_eq!(open_session(d, "", NHG_SESSION_LAUNCH), NHG_OK);
    answer(d);
    let (_, w) = named(d, "second");
    let (_, plain) = named(d, "");
    configured(d, w);
    configured(d, plain);
    // SAFETY: a live display.
    unsafe {
        assert_eq!(nhg_session_remove(d, w), 1);
        assert_eq!(nhg_session_remove(d, w), 0, "already out");
        assert_eq!(nhg_session_remove(d, plain), 0, "never named");
        assert_eq!(nhg_session_remove(d, 9999), 0, "not a window");
    }
    close(d);
}

#[test]
fn a_reason_out_of_range_is_refused() {
    let rec = Recorder::start("mutter");
    let d = open(&rec);
    assert_eq!(open_session(d, "", 3), NHG_E_ARG);
    close(d);
}

#[test]
fn without_session_management_nothing_happens() {
    let rec = Recorder::start("sway");
    let d = open(&rec);
    assert_eq!(open_session(d, "", NHG_SESSION_LAUNCH), NHG_E_UNSUPPORTED);
    let (rc, w) = named(d, "initial");
    assert_eq!(rc, NHG_OK, "the name is ignored");
    configured(d, w);
    // SAFETY: a live display.
    assert_eq!(unsafe { nhg_session_remove(d, w) }, 0);
    close(d);
}
