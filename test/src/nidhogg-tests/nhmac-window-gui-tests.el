;;; nhmac-window-gui-tests.el --- Naglfar's window behaviour rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).  `test-window-state' reads
;; the NSWindow's own state.

(require 'ert)
(require 'cl-lib)

(defun nhmac-win-gui--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--query)))

(defun nhmac-win-gui--state ()
  (nhmac--query 'test-window-state (selected-frame)))

(defun nhmac-win-gui--wait (key want)
  "Wait up to 4 s for the window state KEY to be WANT; the state."
  (let ((deadline (+ (float-time) 4)) st)
    (while (and (progn (setq st (nhmac-win-gui--state)) (not (eq (plist-get st key) want)))
                (< (float-time) deadline))
      (sit-for 0.2))
    st))

;; Task 32: window behaviours, full screen, Spaces, appearance
(ert-deftest nhmac-fullscreen-native ()
  "Full screen is AppKit's own with `ns-use-native-fullscreen' on."
  (skip-unless (nhmac-win-gui--naglfar-p))
  (let ((ns-use-native-fullscreen t))
    (nhmac--call 'variable 'ns-use-native-fullscreen t)
    (set-frame-parameter nil 'fullscreen 'fullboth)
    (should (eq (plist-get (nhmac-win-gui--wait :fullscreen t) :fullscreen) t))
    (set-frame-parameter nil 'fullscreen nil)
    (should (null (plist-get (nhmac-win-gui--wait :fullscreen nil) :fullscreen)))))

(ert-deftest nhmac-fullscreen-non-native ()
  "With `ns-use-native-fullscreen' off, full screen is the screen's frame without the title bar."
  (skip-unless (nhmac-win-gui--naglfar-p))
  (nhmac--call 'variable 'ns-use-native-fullscreen nil)
  (unwind-protect
      (progn
        (set-frame-parameter nil 'fullscreen 'fullboth)
        (should (eq (plist-get (nhmac-win-gui--wait :fullscreen t) :fullscreen) t))
        (set-frame-parameter nil 'fullscreen nil)
        (should (null (plist-get (nhmac-win-gui--wait :fullscreen nil) :fullscreen))))
    (nhmac--call 'variable 'ns-use-native-fullscreen t)))

(ert-deftest nhmac-miniaturize ()
  "`iconify-frame' miniaturizes the window and `make-frame-visible' brings it back."
  (skip-unless (nhmac-win-gui--naglfar-p))
  (iconify-frame)
  (should (eq (plist-get (nhmac-win-gui--wait :miniaturized t) :miniaturized) t))
  (make-frame-visible)
  (ert-skip "pending: deminiaturize is the window system's `make-frame-visible' (frame visibility, Task 14)"))

(ert-deftest nhmac-appearance-dark ()
  "A change of the system appearance records it in `ns-system-appearance' and runs the hook with `dark'."
  (skip-unless (nhmac-win-gui--naglfar-p))
  (let (got)
    (add-hook 'ns-system-appearance-change-functions (lambda (a) (setq got a)))
    (ert-skip "pending: switching the runner's system appearance (the hook is `nhmac-appearance-changed')")))

(ert-deftest nhmac-transparent-titlebar ()
  "`ns-transparent-titlebar' makes the title bar transparent."
  (skip-unless (nhmac-win-gui--naglfar-p))
  (set-frame-parameter nil 'ns-transparent-titlebar t)
  (nhmac-apply-frame-look)
  (should (eq (plist-get (nhmac-win-gui--state) :titlebar-transparent) t))
  (set-frame-parameter nil 'ns-transparent-titlebar nil)
  (nhmac-apply-frame-look)
  (should (null (plist-get (nhmac-win-gui--state) :titlebar-transparent))))

(ert-deftest nhmac-tabbing-off ()
  "Windows never group into tabs."
  (skip-unless (nhmac-win-gui--naglfar-p))
  (should (eq (plist-get (nhmac-win-gui--state) :tabbing-disallowed) t)))

(ert-deftest nhmac-auto-hide-menu-bar ()
  "`ns-auto-hide-menu-bar' sets the application's presentation options."
  (skip-unless (nhmac-win-gui--naglfar-p))
  (nhmac--call 'variable 'ns-auto-hide-menu-bar t)
  (nhmac--call 'variable 'ns-auto-hide-menu-bar nil)
  (ert-skip "pending: reading the presentation options back"))

;;; nhmac-window-gui-tests.el ends here
