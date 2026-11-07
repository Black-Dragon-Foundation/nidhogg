//! The native menus: the bar, popup menus and the Dock's menu, built from the trees `crate::menu` reads.
//!
//! The bar is built when Lisp sends a tree (`menu-bar`, at idle), never when a menu opens: an item's target reports
//! the choice as the tree's generation and the item's place, which C turns into the menu-bar events of that item's
//! key path.  A popup is tracked by AppKit while the backend is lent to the scope, and its choice is the answer to the
//! query.  Every item is enabled or not by Lisp (`autoenablesItems` is off), so AppKit never asks anyone.

use super::runloop::{MacEv, deliver};
use crate::api::{ShellError, ShellResult};
use crate::menu::{self, Kind, Node};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSMenu, NSMenuItem,
    NSWindow,
};
#[cfg(feature = "test-hooks")]
use objc2_foundation::NSArray;
use objc2_foundation::{NSObjectProtocol, NSPoint, NSString};
use std::cell::{Cell, RefCell};

/// What a target does with a chosen item: the bar's reports it, a popup's keeps it for the query.
#[derive(Clone, Copy)]
pub struct Ivars {
    generation: u32,
    popup: bool,
}

#[cfg(feature = "test-hooks")]
thread_local! {
    /// The popup being tracked, and the index of the item the test hook chooses in it.
    static TRACKING: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
    static TEST_CHOOSE: Cell<Option<isize>> = const { Cell::new(None) };
}

thread_local! {
    /// The place of the item a popup's tracking loop returned.
    static CHOSEN: Cell<Option<u32>> = const { Cell::new(None) };
    /// The Dock's menu and the targets that keep it alive; the application delegate answers with it.
    static DOCK: RefCell<Option<(Retained<NSMenu>, Retained<Target>)>> = const { RefCell::new(None) };
}

define_class!(
    /// The target and action of the items of one tree.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NhmacMenuTarget"]
    #[ivars = Ivars]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        #[unsafe(method(choose:))]
        fn choose(&self, sender: &AnyObject) {
            // SAFETY: the sender is the NSMenuItem the action was sent for; `tag` reads its place.
            let tag: isize = unsafe { msg_send![sender, tag] };
            let Ok(index) = u32::try_from(tag) else {
                return;
            };
            let iv = self.ivars();
            if iv.popup {
                CHOSEN.with(|c| c.set(Some(index)));
            } else {
                deliver(MacEv::Menu(iv.generation, index));
            }
        }

        /// Test hook: end the popup now being tracked, choosing the item at `TEST_CHOOSE` first when set.
        #[cfg(feature = "test-hooks")]
        #[unsafe(method(testEndTracking:))]
        fn test_end_tracking(&self, _sender: Option<&AnyObject>) {
            if let Some(m) = TRACKING.with(|t| t.borrow().clone()) {
                if let Some(i) = TEST_CHOOSE.with(Cell::take) {
                    m.performActionForItemAtIndex(i);
                }
                m.cancelTracking();
            }
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker, generation: u32, popup: bool) -> Retained<Target> {
        let this = Self::alloc(mtm).set_ivars(Ivars { generation, popup });
        // SAFETY: NSObject's designated initialiser, on a fresh allocation with its ivars set.
        unsafe { msg_send![super(this), init] }
    }
}

/// The item for `node`; a submenu's items are filled in.  `target` gets the choices.
fn item(mtm: MainThreadMarker, target: &Target, node: &Node) -> Retained<NSMenuItem> {
    if node.kind == Kind::Separator {
        return NSMenuItem::separatorItem(mtm);
    }
    let title = if node.key.is_empty() {
        node.label.clone()
    } else {
        format!("{}\t{}", node.label, node.key)
    };
    // SAFETY: `choose:` is defined on Target; the item holds its target weakly, and the caller keeps the target.
    let it = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(&title),
            Some(sel!(choose:)),
            &NSString::from_str(""),
        )
    };
    it.setEnabled(node.enabled);
    if let Some(i) = node.index {
        it.setTag(i as isize);
    }
    if matches!(node.kind, Kind::Toggle | Kind::Radio) {
        it.setState(if node.state {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }
    if node.kind == Kind::Submenu {
        it.setSubmenu(Some(&menu_of(mtm, target, &node.label, &node.children)));
    } else {
        // SAFETY: a live object that answers `choose:`.
        unsafe { it.setTarget(Some(target)) };
    }
    it
}

/// A menu titled `title` with the items of `nodes`.
fn menu_of(
    mtm: MainThreadMarker,
    target: &Target,
    title: &str,
    nodes: &[Node],
) -> Retained<NSMenu> {
    let m = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    m.setAutoenablesItems(false);
    for n in nodes {
        m.addItem(&item(mtm, target, n));
    }
    m
}

thread_local! {
    /// The target of the bar now installed; the previous one goes when a new tree replaces it.
    static BAR_TARGET: RefCell<Option<Retained<Target>>> = const { RefCell::new(None) };
}

/// `menu-bar` "(GENERATION RECORDS)": install the tree as the main menu.  The first title is the application's own
/// menu, which AppKit names from the bundle; the tree's titles follow.
pub fn bar(mtm: MainThreadMarker, app: &NSApplication, arg: &str) -> ShellResult {
    let (n, text) = menu::split_numbers(arg, 1).ok_or(ShellError::Arg)?;
    let generation = u32::try_from(n[0]).map_err(|_| ShellError::Arg)?;
    let nodes = menu::decode(text.trim_end_matches(')')).ok_or(ShellError::Arg)?;
    let target = Target::new(mtm, generation, false);
    let main = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(""));
    main.setAutoenablesItems(false);
    // the application menu: AppKit shows the app's name over the first item
    let app_item = NSMenuItem::new(mtm);
    app_item.setSubmenu(Some(&app_menu(mtm)));
    main.addItem(&app_item);
    for n in &nodes {
        main.addItem(&item(mtm, &target, n));
    }
    app.setMainMenu(Some(&main));
    BAR_TARGET.with(|t| *t.borrow_mut() = Some(target));
    Ok(())
}

/// The application menu: hide, hide others, show all.  (Quit is the application's lifecycle.)
fn app_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let m = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(""));
    for (title, action, key) in [
        ("Hide", sel!(hide:), "h"),
        ("Hide Others", sel!(hideOtherApplications:), ""),
        ("Show All", sel!(unhideAllApplications:), ""),
    ] {
        // SAFETY: standard NSApplication actions, sent to the first responder chain's NSApp.
        let it = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        m.addItem(&it);
    }
    m
}

/// `menu-dock` "(RECORDS)": the tree the Dock's menu is built from (served by the application delegate).
pub fn dock(mtm: MainThreadMarker, arg: &str) -> ShellResult {
    let text = arg.trim().trim_start_matches('(').trim_end_matches(')');
    let nodes = menu::decode(text).ok_or(ShellError::Arg)?;
    let target = Target::new(mtm, 0, false);
    let m = menu_of(mtm, &target, "", &nodes);
    DOCK.with(|d| *d.borrow_mut() = Some((m, target)));
    Ok(())
}

/// The Dock's menu, for `applicationDockMenu:`.
pub fn dock_menu() -> Option<Retained<NSMenu>> {
    DOCK.with(|d| d.borrow().as_ref().map(|(m, _)| m.clone()))
}

/// A popup of `nodes` at (`x`, `y`) device pixels of `window`'s content (the screen's pointer without a window): the
/// chosen item's place.  AppKit runs the tracking loop until an item is chosen or the menu is dismissed.
pub fn popup(
    mtm: MainThreadMarker,
    window: Option<&NSWindow>,
    x: i64,
    y: i64,
    nodes: &[Node],
) -> Option<u32> {
    let target = Target::new(mtm, 0, true);
    let m = menu_of(mtm, &target, "", nodes);
    CHOSEN.with(|c| c.set(None));
    #[cfg(feature = "test-hooks")]
    TRACKING.with(|t| *t.borrow_mut() = Some(m.clone()));
    match window.and_then(|w| w.contentView().map(|v| (w.backingScaleFactor(), v))) {
        Some((scale, view)) => {
            let height = view.frame().size.height;
            let at = NSPoint::new(x as f64 / scale, height - y as f64 / scale);
            m.popUpMenuPositioningItem_atLocation_inView(None, at, Some(&view));
        }
        None => {
            m.popUpMenuPositioningItem_atLocation_inView(None, NSEvent::mouseLocation(), None);
        }
    }
    #[cfg(feature = "test-hooks")]
    TRACKING.with(|t| *t.borrow_mut() = None);
    CHOSEN.with(Cell::take)
}

/// Test hook: `secs` from now, in every run loop mode (a popup's tracking loop has its own), end the popup then
/// tracked, choosing item `choose` of it first when given.
#[cfg(feature = "test-hooks")]
pub fn test_end_popup_after(mtm: MainThreadMarker, secs: f64, choose: Option<isize>) {
    TEST_CHOOSE.with(|c| c.set(choose));
    let target = Target::new(mtm, 0, true);
    let modes = NSArray::from_retained_slice(&[NSString::from_str("kCFRunLoopCommonModes")]);
    // SAFETY: `testEndTracking:` is defined on Target; the run loop retains the target until it fires.
    unsafe {
        let _: () = msg_send![&target, performSelector: sel!(testEndTracking:), withObject: std::ptr::null::<AnyObject>(), afterDelay: secs, inModes: &*modes];
    }
}

/// Test hook: the main menu as lines "DEPTH<TAB>TITLE<TAB>TAG<TAB>ENABLED", for the rows to read what the user sees.
#[cfg(feature = "test-hooks")]
pub fn test_dump(app: &NSApplication) -> String {
    fn walk(m: &NSMenu, depth: usize, out: &mut String) {
        for it in m.itemArray() {
            out.push_str(&format!(
                "{depth}\t{}\t{}\t{}\n",
                it.title(),
                it.tag(),
                u8::from(it.isEnabled())
            ));
            if let Some(sub) = it.submenu() {
                walk(&sub, depth + 1, out);
            }
        }
    }
    let mut out = String::new();
    if let Some(m) = app.mainMenu() {
        walk(&m, 0, &mut out);
    }
    out
}

/// Test hook: what AppKit does when a menu is about to open (`update`), so a row can count the Lisp calls it makes.
#[cfg(feature = "test-hooks")]
pub fn test_update(app: &NSApplication) {
    if let Some(m) = app.mainMenu() {
        m.update();
    }
}
