//! Test-only (never shipped).  One dylib, three jobs:
//!
//! * `DYLD_INSERT_LIBRARIES` interposes `IOSurfaceLock` / `IOSurfaceUnlock` and counts the locks and, for a write lock,
//!   the surface's whole allocation as the bytes locked (a surface's seed moves at a write unlock, and the locked range
//!   is the surface: the count is the upper bound of what was written);
//! * a `CFRunLoopObserver` on the main run loop, added at load, counts run-loop iterations (`kCFRunLoopBeforeTimers`
//!   fires once per iteration), split by a flag Lisp sets around redisplays;
//! * `module-load` of the same dylib registers `mac-counts--read-raw`, `mac-counts--mark` and `mac-counts--menu-dump`
//!   (nidhogg/test/mac-counts.el wraps them).  Emacs calls these on its main thread, which on NS is AppKit's.
//!
//! No dependencies: the Objective-C runtime and the three C APIs are declared by hand.
#![cfg(target_os = "macos")]
#![allow(non_camel_case_types, non_upper_case_globals)]

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

static LOCKS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static SPINS: AtomicU64 = AtomicU64::new(0);
static SPINS_IN: AtomicU64 = AtomicU64::new(0);
static IN_REDISPLAY: AtomicBool = AtomicBool::new(false);

// ---- IOSurface interposing ------------------------------------------------------------------------------------------

type IOSurfaceRef = *mut c_void;

#[link(name = "IOSurface", kind = "framework")]
extern "C" {
    fn IOSurfaceLock(surface: IOSurfaceRef, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceUnlock(surface: IOSurfaceRef, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceGetAllocSize(surface: IOSurfaceRef) -> usize;
}

const LOCK_READ_ONLY: u32 = 1;

unsafe extern "C" fn my_lock(surface: IOSurfaceRef, options: u32, seed: *mut u32) -> i32 {
    LOCKS.fetch_add(1, Relaxed);
    IOSurfaceLock(surface, options, seed)
}

unsafe extern "C" fn my_unlock(surface: IOSurfaceRef, options: u32, seed: *mut u32) -> i32 {
    if options & LOCK_READ_ONLY == 0 {
        BYTES.fetch_add(IOSurfaceGetAllocSize(surface) as u64, Relaxed);
    }
    IOSurfaceUnlock(surface, options, seed)
}

#[repr(C)]
struct Interpose {
    replacement: *const c_void,
    replacee: *const c_void,
}
unsafe impl Sync for Interpose {}

#[used]
#[link_section = "__DATA,__interpose"]
static INTERPOSE: [Interpose; 2] = [
    Interpose {
        replacement: my_lock as *const c_void,
        replacee: IOSurfaceLock as *const c_void,
    },
    Interpose {
        replacement: my_unlock as *const c_void,
        replacee: IOSurfaceUnlock as *const c_void,
    },
];

// ---- the run-loop observer ------------------------------------------------------------------------------------------

type CFRunLoopRef = *mut c_void;
type CFRunLoopObserverRef = *mut c_void;

#[repr(C)]
struct CFRunLoopObserverContext {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopCommonModes: *const c_void;
    fn CFRunLoopGetMain() -> CFRunLoopRef;
    fn CFRunLoopObserverCreate(
        allocator: *const c_void,
        activities: usize,
        repeats: u8,
        order: isize,
        callout: extern "C" fn(CFRunLoopObserverRef, usize, *mut c_void),
        context: *mut CFRunLoopObserverContext,
    ) -> CFRunLoopObserverRef;
    fn CFRunLoopAddObserver(rl: CFRunLoopRef, observer: CFRunLoopObserverRef, mode: *const c_void);
}

const kCFRunLoopBeforeTimers: usize = 1 << 1;

extern "C" fn on_spin(_: CFRunLoopObserverRef, _: usize, _: *mut c_void) {
    SPINS.fetch_add(1, Relaxed);
    if IN_REDISPLAY.load(Relaxed) {
        SPINS_IN.fetch_add(1, Relaxed);
    }
}

extern "C" fn install_observer() {
    let mut ctx = CFRunLoopObserverContext {
        version: 0,
        info: std::ptr::null_mut(),
        retain: std::ptr::null(),
        release: std::ptr::null(),
        copy_description: std::ptr::null(),
    };
    unsafe {
        let obs = CFRunLoopObserverCreate(
            std::ptr::null(),
            kCFRunLoopBeforeTimers,
            1,
            0,
            on_spin,
            &mut ctx,
        );
        if !obs.is_null() {
            CFRunLoopAddObserver(CFRunLoopGetMain(), obs, kCFRunLoopCommonModes);
        }
    }
}

#[used]
#[link_section = "__DATA,__mod_init_func"]
static INIT: extern "C" fn() = install_observer;

// ---- the menu dump (the Objective-C runtime, by hand) ---------------------------------------------------------------

type Id = *mut c_void;
type Sel = *mut c_void;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
}

fn sel(name: &str) -> Sel {
    let c = CString::new(name).unwrap();
    unsafe { sel_registerName(c.as_ptr()) }
}

// Typed views of objc_msgSend (arm64 and x86-64 both pass these the same way).
unsafe fn send_id(o: Id, s: &str) -> Id {
    let f: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
    f(o, sel(s))
}
unsafe fn send_id_id(o: Id, s: &str, a: Id) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, Id) -> Id =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(o, sel(s), a)
}
unsafe fn send_bool(o: Id, s: &str) -> bool {
    let f: unsafe extern "C" fn(Id, Sel) -> u8 = std::mem::transmute(objc_msgSend as *const c_void);
    f(o, sel(s)) != 0
}
unsafe fn send_bool_sel(o: Id, s: &str, arg: Sel) -> bool {
    let f: unsafe extern "C" fn(Id, Sel, Sel) -> u8 =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(o, sel(s), arg) != 0
}
unsafe fn send_isize(o: Id, s: &str) -> isize {
    let f: unsafe extern "C" fn(Id, Sel) -> isize =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(o, sel(s))
}
unsafe fn send_usize(o: Id, s: &str) -> usize {
    let f: unsafe extern "C" fn(Id, Sel) -> usize =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(o, sel(s))
}
unsafe fn send_id_usize(o: Id, s: &str, i: usize) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, usize) -> Id =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(o, sel(s), i)
}
unsafe fn send_str(o: Id, s: &str) -> String {
    let f: unsafe extern "C" fn(Id, Sel) -> *const c_char =
        std::mem::transmute(objc_msgSend as *const c_void);
    let p = f(o, sel(s));
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}
/// `[o s]` where s returns an NSString: its UTF-8.
unsafe fn ns_string(o: Id, s: &str) -> String {
    let n = send_id(o, s);
    if n.is_null() {
        String::new()
    } else {
        send_str(n, "UTF8String")
    }
}

unsafe fn dump_menu(menu: Id, depth: usize, out: &mut dyn Write) {
    // menuNeedsUpdate: on the menu's delegate, as AppKit does when the menu opens: this builds the deep tree.
    let delegate = send_id(menu, "delegate");
    if !delegate.is_null()
        && send_bool_sel(delegate, "respondsToSelector:", sel("menuNeedsUpdate:"))
    {
        send_id_id(delegate, "menuNeedsUpdate:", menu);
    }
    let items = send_id(menu, "itemArray");
    if items.is_null() {
        return;
    }
    let n = send_usize(items, "count");
    for i in 0..n {
        let item = send_id_usize(items, "objectAtIndex:", i);
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}",
            depth,
            ns_string(item, "title"),
            send_bool(item, "isEnabled") as u8,
            send_isize(item, "state"),
            ns_string(item, "keyEquivalent"),
            send_bool(item, "isSeparatorItem") as u8
        );
        let sub = send_id(item, "submenu");
        if !sub.is_null() {
            dump_menu(sub, depth + 1, out);
        }
    }
}

fn menu_dump(path: &str) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    unsafe {
        let class = CString::new("NSApplication").unwrap();
        let app = send_id(objc_getClass(class.as_ptr()), "sharedApplication");
        let main = send_id(app, "mainMenu");
        if !main.is_null() {
            dump_menu(main, 0, &mut f);
        }
    }
    f.flush()
}

// ---- the Emacs module (emacs-module.h, up to make_string; the struct is the module ABI's prefix) --------------------

type emacs_value = *mut c_void;

#[repr(C)]
struct emacs_runtime {
    size: isize,
    private_members: *mut c_void,
    get_environment: unsafe extern "C" fn(*mut emacs_runtime) -> *mut emacs_env,
}

type emacs_fn =
    unsafe extern "C" fn(*mut emacs_env, isize, *mut emacs_value, *mut c_void) -> emacs_value;

#[repr(C)]
struct emacs_env {
    size: isize,
    private_members: *mut c_void,
    make_global_ref: *const c_void,
    free_global_ref: *const c_void,
    non_local_exit_check: *const c_void,
    non_local_exit_clear: *const c_void,
    non_local_exit_get: *const c_void,
    non_local_exit_signal: *const c_void,
    non_local_exit_throw: *const c_void,
    make_function: unsafe extern "C" fn(
        *mut emacs_env,
        isize,
        isize,
        emacs_fn,
        *const c_char,
        *mut c_void,
    ) -> emacs_value,
    funcall:
        unsafe extern "C" fn(*mut emacs_env, emacs_value, isize, *mut emacs_value) -> emacs_value,
    intern: unsafe extern "C" fn(*mut emacs_env, *const c_char) -> emacs_value,
    type_of: *const c_void,
    is_not_nil: *const c_void,
    eq: *const c_void,
    extract_integer: unsafe extern "C" fn(*mut emacs_env, emacs_value) -> i64,
    make_integer: unsafe extern "C" fn(*mut emacs_env, i64) -> emacs_value,
    extract_float: *const c_void,
    make_float: *const c_void,
    copy_string_contents:
        unsafe extern "C" fn(*mut emacs_env, emacs_value, *mut c_char, *mut isize) -> bool,
}

#[no_mangle]
pub static plugin_is_GPL_compatible: c_int = 0;

unsafe extern "C" fn read_raw(
    env: *mut emacs_env,
    _n: isize,
    _a: *mut emacs_value,
    _d: *mut c_void,
) -> emacs_value {
    let e = &*env;
    let mut v = [
        (e.make_integer)(env, LOCKS.load(Relaxed) as i64),
        (e.make_integer)(env, BYTES.load(Relaxed) as i64),
        (e.make_integer)(env, SPINS.load(Relaxed) as i64),
        (e.make_integer)(env, SPINS_IN.load(Relaxed) as i64),
    ];
    let list = (e.intern)(env, c"list".as_ptr());
    (e.funcall)(env, list, 4, v.as_mut_ptr())
}

unsafe extern "C" fn mark(
    env: *mut emacs_env,
    _n: isize,
    a: *mut emacs_value,
    _d: *mut c_void,
) -> emacs_value {
    let e = &*env;
    IN_REDISPLAY.store((e.extract_integer)(env, *a) != 0, Relaxed);
    (e.intern)(env, c"nil".as_ptr())
}

unsafe extern "C" fn menu_dump_fn(
    env: *mut emacs_env,
    _n: isize,
    a: *mut emacs_value,
    _d: *mut c_void,
) -> emacs_value {
    let e = &*env;
    let mut buf = vec![0 as c_char; 4096];
    let mut len = buf.len() as isize;
    if (e.copy_string_contents)(env, *a, buf.as_mut_ptr(), &mut len) {
        let path = CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
        let _ = menu_dump(&path);
    }
    (e.intern)(env, c"nil".as_ptr())
}

unsafe fn defun(env: *mut emacs_env, name: &CStr, min: isize, max: isize, f: emacs_fn) {
    let e = &*env;
    let func = (e.make_function)(
        env,
        min,
        max,
        f,
        c"Test-only counter (nidhogg/test/mac-interpose).".as_ptr(),
        std::ptr::null_mut(),
    );
    let mut args = [(e.intern)(env, name.as_ptr()), func];
    let fset = (e.intern)(env, c"fset".as_ptr());
    (e.funcall)(env, fset, 2, args.as_mut_ptr());
}

#[no_mangle]
pub unsafe extern "C" fn emacs_module_init(rt: *mut emacs_runtime) -> c_int {
    let env = ((*rt).get_environment)(rt);
    defun(env, c"mac-counts--read-raw", 0, 0, read_raw);
    defun(env, c"mac-counts--mark", 1, 1, mark);
    defun(env, c"mac-counts--menu-dump", 1, 1, menu_dump_fn);
    let e = &*env;
    let mut args = [(e.intern)(env, c"mac-interpose".as_ptr())];
    let provide = (e.intern)(env, c"provide".as_ptr());
    (e.funcall)(env, provide, 1, args.as_mut_ptr());
    0
}
