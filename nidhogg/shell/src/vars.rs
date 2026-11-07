//! The variables the Lisp side pushes to the shell (`nhmac--watched-variables`), platform-neutral so the reading is
//! tested on Linux.

use crate::mac_input::{Choice, ModMap, set_modmap};

/// What the variables say.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub native_fullscreen: bool,
    pub auto_hide_menu_bar: bool,
    pub modifiers: ModMap,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            native_fullscreen: true,
            auto_hide_menu_bar: false,
            modifiers: ModMap::default(),
        }
    }
}

/// Apply a `variable` call "(NAME VALUE)" to the settings; true when it was one of ours.
pub fn set_variable(s: &mut Settings, arg: &str) -> bool {
    let Some((name, value)) = crate::pasteboard::word_and_text(arg) else {
        return false;
    };
    let on = value.trim() != "nil";
    match name {
        "ns-use-native-fullscreen" => s.native_fullscreen = on,
        "ns-auto-hide-menu-bar" => s.auto_hide_menu_bar = on,
        _ => return set_modifier(s, name, value.trim()),
    }
    true
}

/// `ns-alternate-modifier` and the like: a modifier name, or `nil` for the default; for the right-hand ones `left`
/// (also `nil`) follows the left key.
fn set_modifier(s: &mut Settings, name: &str, value: &str) -> bool {
    let m = &mut s.modifiers;
    let d = ModMap::default();
    let choice = Choice::parse(value);
    match name {
        "ns-alternate-modifier" => m.alternate = choice.unwrap_or(d.alternate),
        "ns-command-modifier" => m.command = choice.unwrap_or(d.command),
        "ns-control-modifier" => m.control = choice.unwrap_or(d.control),
        "ns-function-modifier" => m.function = choice.unwrap_or(d.function),
        "ns-right-alternate-modifier" => m.right_alternate = choice,
        "ns-right-command-modifier" => m.right_command = choice,
        "ns-right-control-modifier" => m.right_control = choice,
        _ => return false,
    }
    set_modmap(*m);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_set_the_settings() {
        let mut s = Settings::default();
        assert!(s.native_fullscreen);
        assert!(set_variable(&mut s, "(ns-use-native-fullscreen nil)"));
        assert!(!s.native_fullscreen);
        assert!(set_variable(&mut s, "(ns-auto-hide-menu-bar t)"));
        assert!(s.auto_hide_menu_bar);
        assert!(!set_variable(&mut s, "(ns-confirm-quit t)"));
    }

    #[test]
    fn modifier_variables_set_the_map() {
        let mut s = Settings::default();
        assert!(set_variable(&mut s, "(ns-alternate-modifier super)"));
        assert_eq!(s.modifiers.alternate, Choice::Super);
        assert!(set_variable(&mut s, "(ns-right-alternate-modifier hyper)"));
        assert_eq!(s.modifiers.right_alternate, Some(Choice::Hyper));
        assert!(set_variable(&mut s, "(ns-right-alternate-modifier left)"));
        assert_eq!(s.modifiers.right_alternate, None);
        assert!(set_variable(&mut s, "(ns-alternate-modifier nil)"));
        assert_eq!(s.modifiers.alternate, Choice::Meta);
        assert!(set_variable(&mut s, "(ns-function-modifier alt)"));
        assert_eq!(s.modifiers.function, Choice::Alt);
        set_modmap(ModMap::default());
    }
}
