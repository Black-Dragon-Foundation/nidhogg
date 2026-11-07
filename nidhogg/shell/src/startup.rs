//! What Emacs's `main` needs from the platform before Lisp exists, as pure functions so their tests run on Linux: the
//! app bundle's root, the relocation of the build's relative paths into it (`ns_relocate`, `nsterm.m`), and the locale
//! (`ns_init_locale`).  The macOS half (the autorelease pool, `NSLocale`) is `macos::startup`.

use std::path::{Path, PathBuf};

/// The bundle that holds the executable `exe`: `X.app` for `X.app/Contents/MacOS/Emacs`, `None` for any other place
/// (a build tree's `src/emacs`, an unbundled install).
pub fn bundle_root(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let is = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension()? == "app";
    is.then(|| app.to_path_buf())
}

/// `epath` placed under the bundle `root`, as a self-contained NS app resolves its configured paths: only a relative
/// path that exists there moves.  An absolute path, or one that is not in the bundle, comes back as it is (the user
/// may be running the bundle's binary as a plain Unix install).
pub fn relocate(root: &Path, epath: &str, exists: impl Fn(&Path) -> bool) -> String {
    if epath.is_empty() || Path::new(epath).is_absolute() {
        return epath.to_string();
    }
    let moved = root.join(epath);
    if exists(&moved) {
        moved.to_string_lossy().into_owned()
    } else {
        epath.to_string()
    }
}

/// The `LANG` to run with.  `current` is the environment's (`None` or empty: unset); `system` is the system's locale
/// as `NSLocale` names it (`en_US`).  A value the C library refuses (`usable` false) gives `en_US.UTF-8`.  Returns the
/// value and whether it replaced one that could not be used.
pub fn lang(current: Option<&str>, system: &str, usable: impl Fn(&str) -> bool) -> (String, bool) {
    let want = match current {
        Some(c) if !c.is_empty() => c.to_string(),
        _ => format!("{system}.UTF-8"),
    };
    if usable(&want) {
        (want, false)
    } else {
        ("en_US.UTF-8".to_string(), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_of_a_bundled_executable() {
        assert_eq!(
            bundle_root(Path::new("/Applications/Nidhogg.app/Contents/MacOS/Emacs")),
            Some(PathBuf::from("/Applications/Nidhogg.app"))
        );
    }

    #[test]
    fn no_root_outside_a_bundle() {
        assert_eq!(bundle_root(Path::new("/build/obj/src/emacs")), None);
        assert_eq!(bundle_root(Path::new("/x.app/Contents/Other/Emacs")), None);
        assert_eq!(bundle_root(Path::new("/x/Contents/MacOS/Emacs")), None);
        assert_eq!(bundle_root(Path::new("Emacs")), None);
    }

    #[test]
    fn relative_path_that_exists_moves_into_the_bundle() {
        let root = Path::new("/A.app");
        let got = relocate(root, "Contents/Resources/lisp", |p| {
            p == Path::new("/A.app/Contents/Resources/lisp")
        });
        assert_eq!(got, "/A.app/Contents/Resources/lisp");
    }

    #[test]
    fn relative_path_that_is_missing_stays() {
        assert_eq!(
            relocate(Path::new("/A.app"), "Contents/Resources/lisp", |_| false),
            "Contents/Resources/lisp"
        );
    }

    #[test]
    fn absolute_and_empty_paths_stay() {
        assert_eq!(
            relocate(Path::new("/A.app"), "/usr/share/emacs", |_| true),
            "/usr/share/emacs"
        );
        assert_eq!(relocate(Path::new("/A.app"), "", |_| true), "");
    }

    #[test]
    fn lang_from_the_system_when_unset() {
        assert_eq!(
            lang(None, "de_DE", |_| true),
            ("de_DE.UTF-8".to_string(), false)
        );
        assert_eq!(
            lang(Some(""), "fr_CA", |_| true),
            ("fr_CA.UTF-8".to_string(), false)
        );
    }

    #[test]
    fn lang_kept_when_set() {
        assert_eq!(
            lang(Some("C.UTF-8"), "de_DE", |_| true),
            ("C.UTF-8".to_string(), false)
        );
    }

    #[test]
    fn lang_falls_back_when_unusable() {
        assert_eq!(
            lang(None, "xx_XX", |l| l == "en_US.UTF-8"),
            ("en_US.UTF-8".to_string(), true)
        );
    }
}
