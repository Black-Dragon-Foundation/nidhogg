//! The variables the Lisp side pushes to the shell (`nhmac--watched-variables`), platform-neutral so the reading is
//! tested on Linux.

/// What the variables say.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub native_fullscreen: bool,
    pub auto_hide_menu_bar: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            native_fullscreen: true,
            auto_hide_menu_bar: false,
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
        _ => return false,
    }
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
}
