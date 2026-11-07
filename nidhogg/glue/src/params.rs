//! The frame-parameter decisions of the Wayland shell's `nhwlfns.c` handlers: validation, defaults, clamps and
//! what the change touches.
//!
//! C reads the Lisp value into plain records (`NhgParamValue`: a number, a symbol id, or a flag), states the frame
//! facts the decision needs (`NhgParamFrame`), and applies the returned `NhgParamAction`: one table walk,
//! `nhg_frame_param`.  Nothing here touches a display, so another front end (Naglfar's) calls the same entry.
//! An action's `flags` are what to do, in the order of the `NHG_PA_*` list; an error is `status` != 0 and no flag.

/// The parameters with a decision here (`key` of `nhg_frame_param`).  The others stay one-line calls in C.
pub const NHG_PK_ALPHA: u32 = 1;
pub const NHG_PK_MENU_BAR_LINES: u32 = 2;
pub const NHG_PK_TAB_BAR_LINES: u32 = 3;
pub const NHG_PK_TOOL_BAR_LINES: u32 = 4;
pub const NHG_PK_CHILD_FRAME_BORDER_WIDTH: u32 = 5;
pub const NHG_PK_INTERNAL_BORDER_WIDTH: u32 = 6;
pub const NHG_PK_UNDECORATED: u32 = 7;
pub const NHG_PK_Z_GROUP: u32 = 8;
pub const NHG_PK_NO_ACCEPT_FOCUS: u32 = 9;
/// Two values: the cursor colour's pixel and the cursor's foreground pixel (C decoded both).
pub const NHG_PK_CURSOR_COLOR: u32 = 10;
/// One past the last key.
pub const NHG_PK_COUNT: u32 = 11;

/// `NhgParamValue::kind`.
pub const NHG_PV_NIL: u32 = 0;
/// `int` holds the fixnum (a bignum is `NHG_PV_OTHER`).
pub const NHG_PV_FIXNUM: u32 = 1;
pub const NHG_PV_FLOAT: u32 = 2;
/// A symbol other than nil: `sym` is its id, one of `NHG_PS_*` (0 for any other symbol).
pub const NHG_PV_SYMBOL: u32 = 3;
/// A string, a cons or anything else.
pub const NHG_PV_OTHER: u32 = 4;
/// An integer too large for a fixnum: `int` is its sign, -1 or 1.
pub const NHG_PV_BIGNUM: u32 = 5;

/// `NhgParamValue::sym` for `z-group`.
pub const NHG_PS_OTHER: u32 = 0;
pub const NHG_PS_ABOVE: u32 = 1;
pub const NHG_PS_ABOVE_SUSPENDED: u32 = 2;
pub const NHG_PS_BELOW: u32 = 3;

/// `NhgParamAction::status`.
pub const NHG_PE_OK: i32 = 0;
/// `alpha`: a float outside 0..1; C signals `args_out_of_range (0.0, 1.0)`.
pub const NHG_PE_ALPHA_FLOAT_RANGE: i32 = 1;
/// `alpha`: an integer outside 0..100; `args_out_of_range (0, 100)`.
pub const NHG_PE_ALPHA_PERCENT_RANGE: i32 = 2;
/// `alpha`: not a number; `wrong_type_argument (Qnumberp, item)`, `item` being `NhgParamAction::n` (0 or 1).
pub const NHG_PE_ALPHA_TYPE: i32 = 3;
/// `child-frame-border-width`: `signal_error ("Invalid child frame border width", arg)`.
pub const NHG_PE_CHILD_BORDER: i32 = 4;
/// `internal-border-width`: C runs `check_int_nonnegative (arg)`, which signals the exact error (a negative
/// integer is 0, so this is a non-integer or one above `INT_MAX`).
pub const NHG_PE_INTERNAL_BORDER: i32 = 5;
/// `z-group`: `error ("Invalid z-group specification")`.
pub const NHG_PE_Z_GROUP: i32 = 6;
/// A bad call: unknown key, missing value.
pub const NHG_PE_CALL: i32 = 7;

/// `NhgParamAction::flags`: what C does, in this order.
/// `fset_redisplay (f)`.
pub const NHG_PA_REDISPLAY: u32 = 1 << 0;
/// Store `n` (the lines, the border width, the z-group in `enum z_group` order, or the flag).
pub const NHG_PA_STORE: u32 = 1 << 1;
/// `nhwl_clear_under_internal_border (f)`, before the resize (the menu bar's order).
pub const NHG_PA_CLEAR_BORDER_FIRST: u32 = 1 << 2;
/// `adjust_frame_size (f, -1, -1, 3, ...)` for the parameter itself.
pub const NHG_PA_ADJUST_SIZE: u32 = 1 << 3;
/// `nhwl_clear_under_internal_border (f)`, after the resize.
pub const NHG_PA_CLEAR_BORDER: u32 = 1 << 4;
/// Clear the area `(0, FRAME_TOP_MARGIN_HEIGHT + y, FRAME_PIXEL_WIDTH, h)` (`nhwl_clear_area`; the margin as the resize left it).
pub const NHG_PA_CLEAR_AREA: u32 = 1 << 5;
/// Clear the area `(0, y2, FRAME_PIXEL_WIDTH, h2)`.
pub const NHG_PA_CLEAR_AREA2: u32 = 1 << 6;
/// Clear the menu bar window's current glyph matrix.
pub const NHG_PA_CLEAR_MENU_MATRIX: u32 = 1 << 7;
/// `adjust_frame_glyphs (f)`.
pub const NHG_PA_ADJUST_GLYPHS: u32 = 1 << 8;
/// Call the terminal's tab bar height hook with `h`.
pub const NHG_PA_TAB_BAR_HOOK: u32 = 1 << 9;
/// Call the terminal's tool bar height hook with `h`.
pub const NHG_PA_TOOL_BAR_HOOK: u32 = 1 << 10;
/// Store `alpha` in the frame and apply it (`nhwl_set_frame_alpha`).
pub const NHG_PA_APPLY_ALPHA: u32 = 1 << 11;
/// Store `pixel[0]` as the cursor colour and `pixel[1]` as its foreground.
pub const NHG_PA_STORE_CURSOR: u32 = 1 << 12;
/// Redraw the cursor with the new colours.
pub const NHG_PA_REDRAW_CURSOR: u32 = 1 << 13;
/// `nhwl_set_undecorated_hint (f, n)`.
pub const NHG_PA_UNDECORATED_HINT: u32 = 1 << 14;
/// Raise (`n` is above) or lower (`n` is below) the child frame among its siblings.
pub const NHG_PA_RAISE_LOWER: u32 = 1 << 15;
/// `nhwl_child_reposition (f)`.
pub const NHG_PA_REPOSITION: u32 = 1 << 16;

/// One Lisp value, as C reads it.  Layout equals `NhgParamValue` in `nhglue.h`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NhgParamValue {
    pub kind: u32,
    pub sym: u32,
    pub int: i64,
    pub flt: f64,
}

/// The frame facts a decision reads (`0` or `1` for the flags).  Layout equals `NhgParamFrame`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NhgParamFrame {
    /// `FRAME_MINIBUF_ONLY_P`.
    pub minibuf_only: i32,
    /// `FRAME_PARENT_FRAME (f)` is set.
    pub has_parent: i32,
    /// The frame has a window (`FRAME_X_WINDOW`).
    pub has_window: i32,
    pub visible: i32,
    /// The frame is a child frame of this shell (`nhwl_child_p`).
    pub child: i32,
    /// The new value is `eq` to the old one.
    pub same: i32,
    /// `f->menu_bar_window` is a window.
    pub has_menu_window: i32,
    /// The current value of the parameter: the bar's lines, or the border's width.
    pub current: i32,
    /// `FRAME_LINE_HEIGHT`.
    pub line_height: i32,
    /// `FRAME_INTERNAL_BORDER_WIDTH`.
    pub border: i32,
    /// `FRAME_PIXEL_WIDTH`.
    pub pixel_width: i32,
    /// `FRAME_BACKGROUND_PIXEL`.
    pub bg_pixel: u64,
    /// The mouse colour's pixel.
    pub mouse_pixel: u64,
}

/// What a parameter's change does.  Layout equals `NhgParamAction`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NhgParamAction {
    /// `NHG_PE_*`.
    pub status: i32,
    /// `NHG_PA_*`; 0 with a status of 0 means the change is ignored.
    pub flags: u32,
    /// What to store, or an error's item.
    pub n: i32,
    /// The area to clear: `y`, `h`.
    pub y: i32,
    pub h: i32,
    /// The second area.
    pub y2: i32,
    pub h2: i32,
    pub alpha: [f64; 2],
    pub pixel: [u64; 2],
}

/// The fixnum in `0..=i32::MAX` that `v` is, if it is one.
fn nat_int(v: &NhgParamValue) -> Option<i32> {
    if v.kind == NHG_PV_FIXNUM {
        i32::try_from(v.int).ok().filter(|n| *n >= 0)
    } else {
        None
    }
}

fn ok(flags: u32) -> NhgParamAction {
    NhgParamAction {
        flags,
        ..Default::default()
    }
}

fn err(status: i32) -> NhgParamAction {
    NhgParamAction {
        status,
        ..Default::default()
    }
}

fn alpha(v: &[NhgParamValue]) -> NhgParamAction {
    let mut out = [0.0; 2];
    for (i, slot) in out.iter_mut().enumerate() {
        // one value stands for both
        let item = if v.len() > 1 { &v[i] } else { &v[0] };
        let fail = |status| NhgParamAction {
            status,
            n: i as i32,
            ..Default::default()
        };
        *slot = match item.kind {
            NHG_PV_NIL => -1.0,
            NHG_PV_FLOAT if (0.0..=1.0).contains(&item.flt) => item.flt,
            NHG_PV_FLOAT => return fail(NHG_PE_ALPHA_FLOAT_RANGE),
            NHG_PV_FIXNUM if (0..=100).contains(&item.int) => item.int as f64 / 100.0,
            NHG_PV_FIXNUM => return fail(NHG_PE_ALPHA_PERCENT_RANGE),
            _ => return fail(NHG_PE_ALPHA_TYPE),
        };
    }
    NhgParamAction {
        flags: NHG_PA_APPLY_ALPHA,
        alpha: out,
        ..Default::default()
    }
}

fn menu_bar_lines(f: &NhgParamFrame, v: &NhgParamValue) -> NhgParamAction {
    // menu bars don't work in minibuffer-only frames, and a child frame has none
    if f.minibuf_only != 0 || f.has_parent != 0 {
        return ok(0);
    }
    let n = if v.kind == NHG_PV_FIXNUM {
        i32::try_from(v.int).unwrap_or(0)
    } else {
        0
    };
    let mut a = ok(NHG_PA_STORE | NHG_PA_REDISPLAY);
    a.n = n;
    if f.has_window != 0 {
        a.flags |= NHG_PA_CLEAR_BORDER_FIRST;
    }
    if n != f.current {
        a.flags |= NHG_PA_ADJUST_SIZE;
        // the border below the top margin
        if f.has_window != 0 && f.border > 0 && f.pixel_width > 0 {
            a.flags |= NHG_PA_CLEAR_AREA;
            a.y = 0;
            a.h = f.border;
        }
        // a larger bar: the added rows, except the first line, which is drawn later
        if n > 1 && n > f.current {
            let y = f.current.max(1).saturating_mul(f.line_height);
            a.flags |= NHG_PA_CLEAR_AREA2;
            a.y2 = y;
            a.h2 = n.saturating_mul(f.line_height).saturating_sub(y);
        }
        if n == 0 && f.has_menu_window != 0 {
            a.flags |= NHG_PA_CLEAR_MENU_MATRIX;
        }
    }
    a.flags |= NHG_PA_ADJUST_GLYPHS;
    a
}

fn bar_lines(f: &NhgParamFrame, v: &NhgParamValue, hook: u32, only_edges: bool) -> NhgParamAction {
    if f.minibuf_only != 0 {
        return ok(0);
    }
    let n = nat_int(v).unwrap_or(0);
    // the tab bar's hook only runs when the bar appears or goes
    if only_edges && !(n != f.current && (f.current == 0 || n == 0)) {
        return ok(0);
    }
    NhgParamAction {
        flags: hook,
        h: n.saturating_mul(f.line_height),
        ..Default::default()
    }
}

/// A border width that changed: stored, and a frame with a window is laid out and its border cleared again.
fn border_change(f: &NhgParamFrame, n: i32) -> NhgParamAction {
    let mut a = ok(0);
    a.n = n;
    if n != f.current {
        a.flags = NHG_PA_STORE;
        if f.has_window != 0 {
            a.flags |= NHG_PA_ADJUST_SIZE | NHG_PA_CLEAR_BORDER;
        }
    }
    a
}

fn cursor_color(f: &NhgParamFrame, v: &[NhgParamValue]) -> NhgParamAction {
    let [p, fg] = v else {
        return err(NHG_PE_CALL);
    };
    if p.kind != NHG_PV_FIXNUM || fg.kind != NHG_PV_FIXNUM {
        return err(NHG_PE_CALL);
    }
    let (mut pixel, mut fore) = (p.int as u64, fg.int as u64);
    // the cursor differs from the background
    if pixel == f.bg_pixel {
        pixel = f.mouse_pixel;
        if pixel == fore {
            fore = f.bg_pixel;
        }
    }
    let mut a = ok(NHG_PA_STORE_CURSOR);
    a.pixel = [pixel, fore];
    if f.has_window != 0 && f.visible != 0 {
        a.flags |= NHG_PA_REDRAW_CURSOR;
    }
    a
}

/// What changing parameter `key` to `v` does on a frame with the facts `f`.  `v` holds one value; `alpha` takes
/// one or two (the car and the next item of a cons), `cursor-color` two pixels.
pub fn decide(key: u32, f: &NhgParamFrame, v: &[NhgParamValue]) -> NhgParamAction {
    if key == 0 || key >= NHG_PK_COUNT || v.is_empty() {
        return err(NHG_PE_CALL);
    }
    let x = &v[0];
    match key {
        NHG_PK_ALPHA => alpha(v),
        NHG_PK_MENU_BAR_LINES => menu_bar_lines(f, x),
        NHG_PK_TAB_BAR_LINES => bar_lines(f, x, NHG_PA_TAB_BAR_HOOK, true),
        NHG_PK_TOOL_BAR_LINES => bar_lines(f, x, NHG_PA_TOOL_BAR_HOOK, false),
        NHG_PK_CHILD_FRAME_BORDER_WIDTH => match (x.kind, nat_int(x)) {
            (NHG_PV_NIL, _) => border_change(f, -1),
            (_, Some(n)) => border_change(f, n),
            _ => err(NHG_PE_CHILD_BORDER),
        },
        NHG_PK_INTERNAL_BORDER_WIDTH => match x.kind {
            // a negative integer is 0
            NHG_PV_BIGNUM if x.int < 0 => border_change(f, 0),
            NHG_PV_FIXNUM if x.int < 0 => border_change(f, 0),
            _ => match nat_int(x) {
                Some(n) => border_change(f, n),
                None => err(NHG_PE_INTERNAL_BORDER),
            },
        },
        NHG_PK_UNDECORATED if f.same != 0 => ok(0),
        NHG_PK_UNDECORATED => NhgParamAction {
            flags: NHG_PA_STORE | NHG_PA_UNDECORATED_HINT,
            n: i32::from(x.kind != NHG_PV_NIL),
            ..Default::default()
        },
        NHG_PK_Z_GROUP => {
            // the order of `enum z_group'
            let (n, move_flag) = match (x.kind, x.sym) {
                (NHG_PV_NIL, _) => (0, 0),
                (NHG_PV_SYMBOL, NHG_PS_ABOVE) => (1, NHG_PA_RAISE_LOWER),
                (NHG_PV_SYMBOL, NHG_PS_BELOW) => (2, NHG_PA_RAISE_LOWER),
                (NHG_PV_SYMBOL, NHG_PS_ABOVE_SUSPENDED) => (3, 0),
                _ => return err(NHG_PE_Z_GROUP),
            };
            // a child frame among its siblings, when the value changed
            let moved = if f.child != 0 && f.same == 0 {
                move_flag
            } else {
                0
            };
            NhgParamAction {
                flags: NHG_PA_STORE | moved,
                n,
                ..Default::default()
            }
        }
        NHG_PK_NO_ACCEPT_FOCUS => NhgParamAction {
            flags: NHG_PA_STORE | NHG_PA_REPOSITION,
            n: i32::from(x.kind != NHG_PV_NIL),
            ..Default::default()
        },
        _ => cursor_color(f, v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nil() -> NhgParamValue {
        NhgParamValue::default()
    }
    fn fix(i: i64) -> NhgParamValue {
        NhgParamValue {
            kind: NHG_PV_FIXNUM,
            int: i,
            ..Default::default()
        }
    }
    fn flt(x: f64) -> NhgParamValue {
        NhgParamValue {
            kind: NHG_PV_FLOAT,
            flt: x,
            ..Default::default()
        }
    }
    fn sym(id: u32) -> NhgParamValue {
        NhgParamValue {
            kind: NHG_PV_SYMBOL,
            sym: id,
            ..Default::default()
        }
    }
    fn big(sign: i64) -> NhgParamValue {
        NhgParamValue {
            kind: NHG_PV_BIGNUM,
            int: sign,
            ..Default::default()
        }
    }
    fn other() -> NhgParamValue {
        NhgParamValue {
            kind: NHG_PV_OTHER,
            ..Default::default()
        }
    }
    fn frame() -> NhgParamFrame {
        NhgParamFrame {
            line_height: 20,
            pixel_width: 800,
            ..Default::default()
        }
    }
    fn run(key: u32, f: &NhgParamFrame, v: &[NhgParamValue]) -> NhgParamAction {
        decide(key, f, v)
    }

    /// Every key of the table has at least one accepted and one rejected-or-ignored row below; this lists the keys
    /// the rows cover, so a new key without rows fails.
    const COVERED: &[u32] = &[
        NHG_PK_ALPHA,
        NHG_PK_MENU_BAR_LINES,
        NHG_PK_TAB_BAR_LINES,
        NHG_PK_TOOL_BAR_LINES,
        NHG_PK_CHILD_FRAME_BORDER_WIDTH,
        NHG_PK_INTERNAL_BORDER_WIDTH,
        NHG_PK_UNDECORATED,
        NHG_PK_Z_GROUP,
        NHG_PK_NO_ACCEPT_FOCUS,
        NHG_PK_CURSOR_COLOR,
    ];

    #[test]
    fn every_key_has_rows() {
        for k in 1..NHG_PK_COUNT {
            assert!(COVERED.contains(&k), "key {k} has no rows");
            // an accepted value never reports a bad call
            let a = run(k, &frame(), &[fix(1), fix(2)]);
            assert_ne!(a.status, NHG_PE_CALL, "key {k}");
        }
        assert_eq!(run(0, &frame(), &[nil()]).status, NHG_PE_CALL);
        assert_eq!(run(NHG_PK_COUNT, &frame(), &[nil()]).status, NHG_PE_CALL);
        assert_eq!(run(NHG_PK_ALPHA, &frame(), &[]).status, NHG_PE_CALL);
    }

    #[test]
    fn alpha_accepts() {
        let f = frame();
        for (v, want) in [
            (vec![nil()], [-1.0, -1.0]),
            (vec![flt(0.5)], [0.5, 0.5]),
            (vec![fix(80)], [0.8, 0.8]),
            (vec![fix(0)], [0.0, 0.0]),
            (vec![flt(1.0)], [1.0, 1.0]),
            (vec![fix(100), flt(0.25)], [1.0, 0.25]),
            (vec![flt(0.9), nil()], [0.9, -1.0]),
        ] {
            let a = run(NHG_PK_ALPHA, &f, &v);
            assert_eq!(a.status, NHG_PE_OK, "{v:?}");
            assert_eq!(a.flags, NHG_PA_APPLY_ALPHA, "{v:?}");
            assert_eq!(a.alpha, want, "{v:?}");
        }
    }

    #[test]
    fn alpha_rejects() {
        let f = frame();
        for (v, status, item) in [
            (vec![flt(1.5)], NHG_PE_ALPHA_FLOAT_RANGE, 0),
            (vec![flt(-0.1)], NHG_PE_ALPHA_FLOAT_RANGE, 0),
            (vec![flt(f64::NAN)], NHG_PE_ALPHA_FLOAT_RANGE, 0),
            (vec![fix(101)], NHG_PE_ALPHA_PERCENT_RANGE, 0),
            (vec![fix(-1)], NHG_PE_ALPHA_PERCENT_RANGE, 0),
            (vec![other()], NHG_PE_ALPHA_TYPE, 0),
            (vec![sym(NHG_PS_OTHER)], NHG_PE_ALPHA_TYPE, 0),
            (vec![fix(50), other()], NHG_PE_ALPHA_TYPE, 1),
            (vec![fix(50), flt(2.0)], NHG_PE_ALPHA_FLOAT_RANGE, 1),
        ] {
            let a = run(NHG_PK_ALPHA, &f, &v);
            assert_eq!(a.status, status, "{v:?}");
            assert_eq!(a.flags, 0, "{v:?}");
            if status == NHG_PE_ALPHA_TYPE {
                assert_eq!(a.n, item, "{v:?}");
            }
        }
    }

    #[test]
    fn menu_bar_lines() {
        let mut f = frame();
        f.has_window = 1;
        f.border = 4;
        f.current = 0;
        let a = run(NHG_PK_MENU_BAR_LINES, &f, &[fix(1)]);
        assert_eq!(
            a.flags,
            NHG_PA_STORE
                | NHG_PA_REDISPLAY
                | NHG_PA_CLEAR_BORDER_FIRST
                | NHG_PA_ADJUST_SIZE
                | NHG_PA_CLEAR_AREA
                | NHG_PA_ADJUST_GLYPHS
        );
        assert_eq!((a.n, a.y, a.h), (1, 0, 4));
        // more than one line grows: the added rows are cleared as well (two areas: the border's, then the rows')
        f.current = 1;
        let a = run(NHG_PK_MENU_BAR_LINES, &f, &[fix(3)]);
        assert_eq!(a.n, 3);
        assert_ne!(a.flags & NHG_PA_ADJUST_SIZE, 0);
        // unchanged: no resize
        let a = run(NHG_PK_MENU_BAR_LINES, &f, &[fix(1)]);
        assert_eq!(a.flags & NHG_PA_ADJUST_SIZE, 0);
        // a bad value is 0 lines, and the menu bar window's matrix goes
        f.has_menu_window = 1;
        let a = run(NHG_PK_MENU_BAR_LINES, &f, &[other()]);
        assert_eq!(a.n, 0);
        assert_ne!(a.flags & NHG_PA_CLEAR_MENU_MATRIX, 0);
        let a = run(NHG_PK_MENU_BAR_LINES, &f, &[fix(i64::from(i32::MAX) + 1)]);
        assert_eq!(a.n, 0);
        assert_eq!(run(NHG_PK_MENU_BAR_LINES, &f, &[big(1)]).n, 0);
        // minibuffer-only and child frames ignore it
        for g in [
            NhgParamFrame {
                minibuf_only: 1,
                ..f
            },
            NhgParamFrame { has_parent: 1, ..f },
        ] {
            let a = run(NHG_PK_MENU_BAR_LINES, &g, &[fix(2)]);
            assert_eq!((a.status, a.flags), (NHG_PE_OK, 0));
        }
        // no window: nothing is cleared
        let g = NhgParamFrame {
            has_window: 0,
            current: 0,
            ..f
        };
        let a = run(NHG_PK_MENU_BAR_LINES, &g, &[fix(2)]);
        assert_eq!(a.flags & (NHG_PA_CLEAR_BORDER_FIRST | NHG_PA_CLEAR_AREA), 0);
    }

    #[test]
    fn menu_bar_added_rows_are_cleared() {
        let f = NhgParamFrame {
            has_window: 1,
            border: 0,
            current: 0,
            line_height: 20,
            pixel_width: 800,
            ..frame()
        };
        // 0 -> 3: the first line is drawn later, rows 1..3 are cleared (y 20, h 40)
        let a = run(NHG_PK_MENU_BAR_LINES, &f, &[fix(3)]);
        assert_eq!((a.y2, a.h2), (20, 40));
        assert_ne!(a.flags & NHG_PA_CLEAR_AREA2, 0);
        assert_eq!(a.flags & NHG_PA_CLEAR_AREA, 0); // no border to clear
        // 2 -> 3
        let f = NhgParamFrame { current: 2, ..f };
        let a = run(NHG_PK_MENU_BAR_LINES, &f, &[fix(3)]);
        assert_eq!((a.y2, a.h2), (40, 20));
    }

    #[test]
    fn tab_bar_lines() {
        let mut f = frame();
        f.current = 0;
        let a = run(NHG_PK_TAB_BAR_LINES, &f, &[fix(2)]);
        assert_eq!(a.flags, NHG_PA_TAB_BAR_HOOK);
        assert_eq!(a.h, 40);
        // 1 -> 2: only 0 <-> n calls the hook
        let g = NhgParamFrame { current: 1, ..f };
        assert_eq!(run(NHG_PK_TAB_BAR_LINES, &g, &[fix(2)]).flags, 0);
        // 1 -> bad value (0): the hook, with height 0
        let a = run(NHG_PK_TAB_BAR_LINES, &g, &[fix(-3)]);
        assert_eq!((a.flags, a.h), (NHG_PA_TAB_BAR_HOOK, 0));
        let a = run(NHG_PK_TAB_BAR_LINES, &g, &[flt(1.0)]);
        assert_eq!((a.flags, a.h), (NHG_PA_TAB_BAR_HOOK, 0));
        let m = NhgParamFrame {
            minibuf_only: 1,
            ..f
        };
        assert_eq!(run(NHG_PK_TAB_BAR_LINES, &m, &[fix(2)]).flags, 0);
    }

    #[test]
    fn tool_bar_lines() {
        let f = NhgParamFrame {
            current: 3,
            ..frame()
        };
        let a = run(NHG_PK_TOOL_BAR_LINES, &f, &[fix(3)]);
        assert_eq!((a.flags, a.h), (NHG_PA_TOOL_BAR_HOOK, 60));
        let a = run(NHG_PK_TOOL_BAR_LINES, &f, &[nil()]);
        assert_eq!((a.flags, a.h), (NHG_PA_TOOL_BAR_HOOK, 0));
        let a = run(NHG_PK_TOOL_BAR_LINES, &f, &[fix(i64::from(i32::MAX))]);
        assert_eq!(a.h, i32::MAX); // saturates rather than overflows
        let m = NhgParamFrame {
            minibuf_only: 1,
            ..f
        };
        assert_eq!(run(NHG_PK_TOOL_BAR_LINES, &m, &[fix(2)]).flags, 0);
    }

    #[test]
    fn child_frame_border_width() {
        let mut f = frame();
        f.current = -1;
        let a = run(NHG_PK_CHILD_FRAME_BORDER_WIDTH, &f, &[nil()]);
        assert_eq!((a.status, a.flags), (NHG_PE_OK, 0)); // -1 is what it is
        let a = run(NHG_PK_CHILD_FRAME_BORDER_WIDTH, &f, &[fix(3)]);
        assert_eq!((a.flags, a.n), (NHG_PA_STORE, 3)); // no window yet: only stored
        f.has_window = 1;
        let a = run(NHG_PK_CHILD_FRAME_BORDER_WIDTH, &f, &[fix(3)]);
        assert_eq!(
            a.flags,
            NHG_PA_STORE | NHG_PA_ADJUST_SIZE | NHG_PA_CLEAR_BORDER
        );
        let a = run(NHG_PK_CHILD_FRAME_BORDER_WIDTH, &f, &[nil()]);
        assert_eq!((a.flags, a.n), (0, -1));
        f.current = 3;
        let a = run(NHG_PK_CHILD_FRAME_BORDER_WIDTH, &f, &[nil()]);
        assert_eq!((a.flags & NHG_PA_STORE, a.n), (NHG_PA_STORE, -1));
        for bad in [
            fix(-1),
            flt(2.0),
            other(),
            big(1),
            sym(NHG_PS_OTHER),
            fix(i64::from(i32::MAX) + 1),
        ] {
            let a = run(NHG_PK_CHILD_FRAME_BORDER_WIDTH, &f, &[bad]);
            assert_eq!((a.status, a.flags), (NHG_PE_CHILD_BORDER, 0), "{bad:?}");
        }
        let a = run(
            NHG_PK_CHILD_FRAME_BORDER_WIDTH,
            &f,
            &[fix(i64::from(i32::MAX))],
        );
        assert_eq!(a.status, NHG_PE_OK);
    }

    #[test]
    fn internal_border_width() {
        let mut f = frame();
        f.current = 2;
        f.has_window = 1;
        let a = run(NHG_PK_INTERNAL_BORDER_WIDTH, &f, &[fix(5)]);
        assert_eq!(
            (a.flags, a.n),
            (NHG_PA_STORE | NHG_PA_ADJUST_SIZE | NHG_PA_CLEAR_BORDER, 5)
        );
        let a = run(NHG_PK_INTERNAL_BORDER_WIDTH, &f, &[fix(2)]);
        assert_eq!(a.flags, 0);
        f.has_window = 0;
        let a = run(NHG_PK_INTERNAL_BORDER_WIDTH, &f, &[fix(0)]);
        assert_eq!((a.flags, a.n), (NHG_PA_STORE, 0));
        // a negative integer is 0
        for neg in [fix(-1), big(-1)] {
            let a = run(NHG_PK_INTERNAL_BORDER_WIDTH, &f, &[neg]);
            assert_eq!(
                (a.status, a.flags, a.n),
                (NHG_PE_OK, NHG_PA_STORE, 0),
                "{neg:?}"
            );
        }
        for bad in [
            nil(),
            flt(1.0),
            other(),
            fix(i64::from(i32::MAX) + 1),
            big(1),
        ] {
            let a = run(NHG_PK_INTERNAL_BORDER_WIDTH, &f, &[bad]);
            assert_eq!((a.status, a.flags), (NHG_PE_INTERNAL_BORDER, 0), "{bad:?}");
        }
    }

    #[test]
    fn undecorated() {
        let f = frame();
        let a = run(NHG_PK_UNDECORATED, &f, &[sym(NHG_PS_OTHER)]);
        assert_eq!((a.flags, a.n), (NHG_PA_STORE | NHG_PA_UNDECORATED_HINT, 1));
        let a = run(NHG_PK_UNDECORATED, &f, &[nil()]);
        assert_eq!((a.flags, a.n), (NHG_PA_STORE | NHG_PA_UNDECORATED_HINT, 0));
        let g = NhgParamFrame { same: 1, ..f };
        assert_eq!(run(NHG_PK_UNDECORATED, &g, &[nil()]).flags, 0);
        // a string or any value counts as non-nil
        assert_eq!(run(NHG_PK_UNDECORATED, &f, &[other()]).n, 1);
    }

    #[test]
    fn z_group() {
        let mut f = frame();
        for (v, n) in [
            (nil(), 0),
            (sym(NHG_PS_ABOVE), 1),
            (sym(NHG_PS_BELOW), 2),
            (sym(NHG_PS_ABOVE_SUSPENDED), 3),
        ] {
            let a = run(NHG_PK_Z_GROUP, &f, &[v]);
            assert_eq!(
                (a.status, a.flags, a.n),
                (NHG_PE_OK, NHG_PA_STORE, n),
                "{v:?}"
            );
        }
        for bad in [sym(NHG_PS_OTHER), fix(1), other()] {
            let a = run(NHG_PK_Z_GROUP, &f, &[bad]);
            assert_eq!((a.status, a.flags), (NHG_PE_Z_GROUP, 0), "{bad:?}");
        }
        // a child frame among its siblings is raised or lowered, when the value changed
        f.child = 1;
        assert_eq!(
            run(NHG_PK_Z_GROUP, &f, &[sym(NHG_PS_ABOVE)]).flags,
            NHG_PA_STORE | NHG_PA_RAISE_LOWER
        );
        assert_eq!(
            run(NHG_PK_Z_GROUP, &f, &[sym(NHG_PS_BELOW)]).flags,
            NHG_PA_STORE | NHG_PA_RAISE_LOWER
        );
        assert_eq!(
            run(NHG_PK_Z_GROUP, &f, &[sym(NHG_PS_ABOVE_SUSPENDED)]).flags,
            NHG_PA_STORE
        );
        assert_eq!(run(NHG_PK_Z_GROUP, &f, &[nil()]).flags, NHG_PA_STORE);
        f.same = 1;
        assert_eq!(
            run(NHG_PK_Z_GROUP, &f, &[sym(NHG_PS_ABOVE)]).flags,
            NHG_PA_STORE
        );
    }

    #[test]
    fn no_accept_focus() {
        let f = frame();
        let a = run(NHG_PK_NO_ACCEPT_FOCUS, &f, &[sym(NHG_PS_OTHER)]);
        assert_eq!((a.flags, a.n), (NHG_PA_STORE | NHG_PA_REPOSITION, 1));
        let a = run(NHG_PK_NO_ACCEPT_FOCUS, &f, &[nil()]);
        assert_eq!((a.flags, a.n), (NHG_PA_STORE | NHG_PA_REPOSITION, 0));
    }

    #[test]
    fn cursor_color() {
        let mut f = frame();
        f.bg_pixel = 0x111111;
        f.mouse_pixel = 0x222222;
        // normal: both stored
        let a = run(NHG_PK_CURSOR_COLOR, &f, &[fix(0xaaaaaa), fix(0x333333)]);
        assert_eq!(
            (a.flags, a.pixel),
            (NHG_PA_STORE_CURSOR, [0xaaaaaa, 0x333333])
        );
        // the cursor colour equal to the background: the mouse colour instead
        let a = run(NHG_PK_CURSOR_COLOR, &f, &[fix(0x111111), fix(0x333333)]);
        assert_eq!(a.pixel, [0x222222, 0x333333]);
        // ...and when that equals the foreground, the foreground becomes the background
        let a = run(NHG_PK_CURSOR_COLOR, &f, &[fix(0x111111), fix(0x222222)]);
        assert_eq!(a.pixel, [0x222222, 0x111111]);
        // a window that is up redraws the cursor, if visible
        f.has_window = 1;
        assert_eq!(
            run(NHG_PK_CURSOR_COLOR, &f, &[fix(1), fix(2)]).flags,
            NHG_PA_STORE_CURSOR
        );
        f.visible = 1;
        assert_eq!(
            run(NHG_PK_CURSOR_COLOR, &f, &[fix(1), fix(2)]).flags,
            NHG_PA_STORE_CURSOR | NHG_PA_REDRAW_CURSOR
        );
        // a non-pixel is a bad call
        assert_eq!(
            run(NHG_PK_CURSOR_COLOR, &f, &[nil(), fix(2)]).status,
            NHG_PE_CALL
        );
        assert_eq!(run(NHG_PK_CURSOR_COLOR, &f, &[fix(2)]).status, NHG_PE_CALL);
    }
}
