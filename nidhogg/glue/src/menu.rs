//! The flat menu records: the glue's side of Naglfar's menu bar and popup menus.
//!
//! Lisp walks the menu keymaps (`nhmac-menu--flatten` in `lisp/term/nhmac-win.el`) into one vector of
//! [`WIDTH`] slots per entry; C copies it into an array of records (`nhg_menu_tree`) without looking at it; this
//! module checks the array and builds the tree the native menu builder reads.  The slot layout and the kind numbers
//! are the same in `nhmac-win.el` (`nhmac-menu-width`, `nhmac-menu-kind-*`) and here:
//!
//! | slot | meaning |
//! |------|---------|
//! | 0 | depth: 0 for the bar's titles, one more for each level down |
//! | 1 | kind: [`KIND_ITEM`] ... [`KIND_RADIO`] |
//! | 2 | label (empty for a separator) |
//! | 3 | the key description shown beside it (empty for none) |
//! | 4 | enabled, 0 or 1 |
//! | 5 | state, 1 for a selected toggle or radio |
//! | 6 | the item's place in `menu_items` (what C maps a choice back from), or -1 for a title or a named pane |

use crate::guard::GlueError;

/// Slots per entry in the Lisp vector.
pub const WIDTH: usize = 7;

pub const KIND_ITEM: i32 = 0;
pub const KIND_SUBMENU: i32 = 1;
pub const KIND_SEPARATOR: i32 = 2;
pub const KIND_TOGGLE: i32 = 3;
pub const KIND_RADIO: i32 = 4;

/// The deepest level an entry may sit at (`menu.c` stops a keymap walk at ten, and a title sits above them).
pub const MAX_DEPTH: usize = 16;

/// One entry as C copied it.  The strings borrow from the copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raw<'a> {
    pub depth: i32,
    pub kind: i32,
    pub label: &'a str,
    pub key: &'a str,
    pub enabled: i32,
    pub state: i32,
    pub index: i32,
}

/// One slot of the Lisp vector, as a plain value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Slot<'a> {
    Int(i64),
    Str(&'a str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Item,
    Submenu,
    Separator,
    Toggle,
    Radio,
}

/// An entry with the entries one level below it under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuNode {
    pub kind: Kind,
    pub label: String,
    pub key: String,
    pub enabled: bool,
    /// A toggle or radio that is on.
    pub state: bool,
    /// The place in `menu_items`; `None` for a title.
    pub index: Option<u32>,
    pub children: Vec<MenuNode>,
}

/// A decoded menu: the bar's titles (or a popup menu's top entries), and the generation C stamped it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuTree {
    pub generation: u64,
    pub bar: Vec<MenuNode>,
}

/// Check the entries and build the tree.  `GlueError::Arg` for a vector that is not what the flattener makes: a
/// bad kind, flag, depth or index, a separator with text, an item without a label, a submenu with nothing under it
/// or an item with something.
pub fn decode(generation: u64, entries: &[Raw]) -> Result<MenuTree, GlueError> {
    let mut last_index = -1;
    // The open path: the entries from the title down to the last one read, each with its children so far.
    let mut path: Vec<MenuNode> = Vec::new();
    let mut bar: Vec<MenuNode> = Vec::new();
    let mut prev: Option<Kind> = None;
    for e in entries {
        let kind = match e.kind {
            KIND_ITEM => Kind::Item,
            KIND_SUBMENU => Kind::Submenu,
            KIND_SEPARATOR => Kind::Separator,
            KIND_TOGGLE => Kind::Toggle,
            KIND_RADIO => Kind::Radio,
            _ => return Err(GlueError::Arg),
        };
        let depth = usize::try_from(e.depth).map_err(|_| GlueError::Arg)?;
        if depth > MAX_DEPTH || !matches!(e.enabled, 0 | 1) || !matches!(e.state, 0 | 1) {
            return Err(GlueError::Arg);
        }
        // a level down only under a submenu, and by one; the first entry is a title
        let max_depth = match prev {
            None => 0,
            Some(Kind::Submenu) => path.len(),
            Some(_) => path.len() - 1,
        };
        if depth > max_depth {
            return Err(GlueError::Arg);
        }
        // close the levels this entry leaves
        while path.len() > depth {
            close(&mut path, &mut bar)?;
        }
        let separator = kind == Kind::Separator;
        let button = matches!(kind, Kind::Toggle | Kind::Radio);
        if separator == !e.label.is_empty()
            || (separator && (!e.key.is_empty() || e.enabled != 0))
            || (e.state == 1 && !button)
            || (depth == 0 && (separator || button))
            || e.index < -1
        {
            return Err(GlueError::Arg);
        }
        if e.index >= 0 {
            if e.index <= last_index {
                return Err(GlueError::Arg);
            }
            last_index = e.index;
        }
        path.push(MenuNode {
            kind,
            label: e.label.to_owned(),
            key: e.key.to_owned(),
            enabled: e.enabled == 1,
            state: e.state == 1,
            index: u32::try_from(e.index).ok(),
            children: Vec::new(),
        });
        prev = Some(kind);
    }
    while !path.is_empty() {
        close(&mut path, &mut bar)?;
    }
    Ok(MenuTree { generation, bar })
}

/// Take the last open entry off `path` and hang it under its parent (or in the bar).  A submenu must have
/// children and nothing else may.
fn close(path: &mut Vec<MenuNode>, bar: &mut Vec<MenuNode>) -> Result<(), GlueError> {
    let node = path.pop().ok_or(GlueError::Arg)?;
    if (node.kind == Kind::Submenu) == node.children.is_empty() {
        return Err(GlueError::Arg);
    }
    match path.last_mut() {
        Some(parent) => parent.children.push(node),
        None => bar.push(node),
    }
    Ok(())
}

/// Split a vector of slots ([`WIDTH`] per entry) into entries and decode them.  `GlueError::Arg` for a length that is
/// not a multiple of [`WIDTH`], a slot of the wrong type, or a number out of `i32`.
pub fn from_slots<'a>(generation: u64, slots: &[Slot<'a>]) -> Result<MenuTree, GlueError> {
    if !slots.len().is_multiple_of(WIDTH) {
        return Err(GlueError::Arg);
    }
    let int = |s: &Slot| match s {
        Slot::Int(n) => i32::try_from(*n).map_err(|_| GlueError::Arg),
        Slot::Str(_) => Err(GlueError::Arg),
    };
    let string = |s: &Slot<'a>| match s {
        Slot::Str(t) => Ok(*t),
        Slot::Int(_) => Err(GlueError::Arg),
    };
    let mut entries = Vec::with_capacity(slots.len() / WIDTH);
    for c in slots.as_chunks::<WIDTH>().0 {
        entries.push(Raw {
            depth: int(&c[0])?,
            kind: int(&c[1])?,
            label: string(&c[2])?,
            key: string(&c[3])?,
            enabled: int(&c[4])?,
            state: int(&c[5])?,
            index: int(&c[6])?,
        });
    }
    decode(generation, &entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(
        depth: i32,
        kind: i32,
        label: &'static str,
        key: &'static str,
        enabled: i32,
        state: i32,
        index: i32,
    ) -> Raw<'static> {
        Raw {
            depth,
            kind,
            label,
            key,
            enabled,
            state,
            index,
        }
    }

    /// The `menu_items` shape of File (A, a separator, a toggle, a submenu with Z) and Edit (Q), as the Lisp
    /// flattener's test pins it.
    fn bar() -> Vec<Raw<'static>> {
        vec![
            raw(0, KIND_SUBMENU, "File", "", 1, 0, -1),
            raw(1, KIND_ITEM, "A", "C-x A", 1, 0, 3),
            raw(1, KIND_SEPARATOR, "", "", 0, 0, 11),
            raw(1, KIND_TOGGLE, "T", "", 1, 1, 19),
            raw(1, KIND_SUBMENU, "Sub", "", 1, 0, 27),
            raw(2, KIND_ITEM, "Z", "", 1, 0, 39),
            raw(0, KIND_SUBMENU, "Edit", "", 1, 0, -1),
            raw(1, KIND_RADIO, "Q", "", 0, 0, 51),
        ]
    }

    #[test]
    fn a_bar_decodes_into_a_tree() {
        let t = decode(7, &bar()).unwrap();
        assert_eq!(t.generation, 7);
        assert_eq!(t.bar.len(), 2);
        let file = &t.bar[0];
        assert_eq!(
            (file.kind, file.label.as_str(), file.index),
            (Kind::Submenu, "File", None)
        );
        assert_eq!(file.children.len(), 4);
        assert_eq!(file.children[0].key, "C-x A");
        assert_eq!(file.children[0].index, Some(3));
        assert_eq!(file.children[1].kind, Kind::Separator);
        assert!(!file.children[1].enabled);
        assert_eq!(
            (file.children[2].kind, file.children[2].state),
            (Kind::Toggle, true)
        );
        let sub = &file.children[3];
        assert_eq!(sub.children.len(), 1);
        assert_eq!(sub.children[0].label, "Z");
        let q = &t.bar[1].children[0];
        assert_eq!((q.kind, q.enabled, q.state), (Kind::Radio, false, false));
    }

    #[test]
    fn a_top_level_command_is_an_item_with_an_index() {
        let t = decode(1, &[raw(0, KIND_ITEM, "Quit", "", 1, 0, 3)]).unwrap();
        assert_eq!(t.bar[0].kind, Kind::Item);
        assert_eq!(t.bar[0].index, Some(3));
    }

    #[test]
    fn an_empty_bar_is_a_tree() {
        assert_eq!(decode(1, &[]).unwrap().bar.len(), 0);
    }

    #[test]
    fn a_vector_in_slots_is_split_and_decoded() {
        let slots = vec![
            Slot::Int(0),
            Slot::Int(1),
            Slot::Str("File"),
            Slot::Str(""),
            Slot::Int(1),
            Slot::Int(0),
            Slot::Int(-1),
            Slot::Int(1),
            Slot::Int(0),
            Slot::Str("A"),
            Slot::Str("C-x A"),
            Slot::Int(1),
            Slot::Int(0),
            Slot::Int(3),
        ];
        let t = from_slots(2, &slots).unwrap();
        assert_eq!(t.bar[0].children[0].label, "A");
        assert_eq!(from_slots(2, &slots[..13]), Err(GlueError::Arg));
        let mut wrong = slots.clone();
        wrong[2] = Slot::Int(5);
        assert_eq!(from_slots(2, &wrong), Err(GlueError::Arg));
    }

    fn rejects(mut f: impl FnMut(&mut Vec<Raw<'static>>)) {
        let mut v = bar();
        f(&mut v);
        assert_eq!(decode(1, &v), Err(GlueError::Arg), "{v:?}");
    }

    #[test]
    fn a_bad_vector_is_rejected() {
        // the first entry is not at depth 0
        rejects(|v| v[0].depth = 1);
        // a jump of two levels
        rejects(|v| v[5].depth = 3);
        // a child under an item
        rejects(|v| v[2].depth = 2);
        // a submenu with nothing under it
        rejects(|v| {
            v.remove(5);
        });
        // an unknown kind, a negative depth
        rejects(|v| v[1].kind = 9);
        rejects(|v| v[1].depth = -1);
        // flags that are not 0 or 1; a state on a plain item
        rejects(|v| v[1].enabled = 2);
        rejects(|v| v[1].state = 1);
        // a separator with a label, a key, or enabled
        rejects(|v| v[2].label = "-");
        rejects(|v| v[2].key = "x");
        rejects(|v| v[2].enabled = 1);
        // a labelless item
        rejects(|v| v[1].label = "");
        // indexes that go back or repeat
        rejects(|v| v[3].index = 3);
        rejects(|v| v[5].index = 2);
        // a menu_items index below -1
        rejects(|v| v[1].index = -2);
        // a separator, a toggle or a radio at the bar's top level
        rejects(|v| {
            v[0].kind = KIND_SEPARATOR;
            v[0].label = "";
            v[0].enabled = 0;
        });
        rejects(|v| v[0].kind = KIND_TOGGLE);
        // a depth beyond the limit
        rejects(|v| {
            v.truncate(1);
            for d in 1..=MAX_DEPTH as i32 + 1 {
                v.last_mut().unwrap().kind = KIND_SUBMENU;
                v.push(raw(d, KIND_ITEM, "x", "", 1, 0, d));
            }
        });
    }

    #[test]
    fn the_widest_legal_menu_decodes() {
        let mut v = vec![raw(0, KIND_SUBMENU, "m0", "", 1, 0, -1)];
        for d in 1..MAX_DEPTH as i32 {
            v.push(raw(d, KIND_SUBMENU, "m", "", 1, 0, d));
        }
        v.push(raw(MAX_DEPTH as i32, KIND_ITEM, "leaf", "", 1, 0, 99));
        assert!(decode(1, &v).is_ok());
    }
}
