;;; nhmac-window-gui-tests.el --- Naglfar's window behaviour rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).  `test-window-state' reads
;; the NSWindow's own state.

(require 'ert)
(declare-function glitnir-gui--say "gui-runner" (fmt &rest args)) ; the lane's log, not *Messages*
(require 'cl-lib)

(defun nhmac-win-gui--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--query)))

(defun nhmac-win-gui--state ()
  (nhmac--query 'test-window-state (selected-frame)))

(defun nhmac-win-gui--wait (key want)
  "Wait for the window state KEY to be WANT; the state.  Up to 4 s, or 12 s for `:fs-busy': that waits for AppKit's
own full-screen animation to end (its did-notification), which round 6's runner took 2.6 to 4.8 s to enter and 4.0
to 6.0 s to leave, and a toggle sent during one runs after it (FS log step 8)."
  (let* ((start (float-time)) (deadline (+ start (if (eq key :fs-busy) 12 4))) st)
    (while (and (progn (setq st (nhmac-win-gui--state)) (not (eq (plist-get st key) want)))
                (< (float-time) deadline))
      (sit-for 0.2))
    (if (eq (plist-get st key) want)
        (when (eq key :fs-busy)
          (glitnir-gui--say "nhmac-win-gui--wait: %S %S after %.1f s" key want (- (float-time) start)))
      (glitnir-gui--say "nhmac-win-gui--wait: %S never %S: %S" key want st))
    st))

(defun nhmac-win-gui--settle ()
  "Leave the window out of full screen with no transition running, so the next row starts from a plain window: a
native transition takes seconds on a CI runner and outlives a row that only waited for the style mask."
  (nhmac-win-gui--wait :fs-busy nil)
  (when (plist-get (nhmac-win-gui--state) :fullscreen)
    (set-frame-parameter nil 'fullscreen nil)
    (nhmac-win-gui--wait :fullscreen nil)
    (nhmac-win-gui--wait :fs-busy nil)))

;; Task 32: window behaviours, full screen, Spaces, appearance
(ert-deftest nhmac-fullscreen-app-running ()
  "A native full-screen transition ends both ways: `windowDidEnterFullScreen:' and `windowDidExitFullScreen:' reach
the delegate and are applied, so the window is not left busy (FS notes: will-enter will-exit did-enter did-exit
fail-enter fail-exit applied).  `[NSApp isRunning]' is logged, not judged: round 2 showed both transitions end with
it nil (the pumps' `nextEventMatchingMask:' turns the run loop the transitions finish on), and it is nil under NS
too whenever Lisp runs, since `ns_select's `[NSApp run]' has returned by then."
  (skip-unless (nhmac-win-gui--naglfar-p))
  (let ((ns-use-native-fullscreen t))
    (nhmac--call 'variable 'ns-use-native-fullscreen t)
    (nhmac-win-gui--settle)
    (let ((before (nhmac-win-gui--state)))
      (glitnir-gui--say "nhmac-fullscreen-app-running: before %S" before)
      (unwind-protect
          (progn
            (set-frame-parameter nil 'fullscreen 'fullboth)
            (nhmac-win-gui--wait :fullscreen t)
            (let ((after (nhmac-win-gui--wait :fs-busy nil)))
              (glitnir-gui--say "nhmac-fullscreen-app-running: after entering %S" after)
              (should (> (nth 2 (plist-get after :fs-notes)) (nth 2 (plist-get before :fs-notes))))
              (should (> (nth 6 (plist-get after :fs-notes)) (nth 6 (plist-get before :fs-notes))))
              (should (null (plist-get after :fs-busy)))
              (should (eq (plist-get after :style-full) t)))
            (set-frame-parameter nil 'fullscreen nil)
            (nhmac-win-gui--wait :fullscreen nil)
            (let ((after (nhmac-win-gui--wait :fs-busy nil)))
              (glitnir-gui--say "nhmac-fullscreen-app-running: after leaving %S" after)
              (should (> (nth 3 (plist-get after :fs-notes)) (nth 3 (plist-get before :fs-notes))))
              (should (null (plist-get after :fs-busy)))
              (should (null (plist-get after :style-full)))))
        (nhmac-win-gui--settle)))))

(ert-deftest nhmac-fullscreen-native ()
  "Full screen is AppKit's own with `ns-use-native-fullscreen' on."
  (skip-unless (nhmac-win-gui--naglfar-p))
  (let ((ns-use-native-fullscreen t))
    (nhmac--call 'variable 'ns-use-native-fullscreen t)
    (nhmac-win-gui--settle)
    (unwind-protect
        (progn
          (set-frame-parameter nil 'fullscreen 'fullboth)
          (should (eq (plist-get (nhmac-win-gui--wait :fullscreen t) :fullscreen) t))
          (nhmac-win-gui--wait :fs-busy nil)
          (set-frame-parameter nil 'fullscreen nil)
          (should (null (plist-get (nhmac-win-gui--wait :fullscreen nil) :fullscreen))))
      (nhmac-win-gui--settle))))

(ert-deftest nhmac-fullscreen-non-native ()
  "With `ns-use-native-fullscreen' off, full screen is the screen's frame without the title bar."
  (skip-unless (nhmac-win-gui--naglfar-p))
  (nhmac-win-gui--settle)
  (nhmac--call 'variable 'ns-use-native-fullscreen nil)
  (unwind-protect
      (progn
        (set-frame-parameter nil 'fullscreen 'fullboth)
        (should (null (plist-get (nhmac-win-gui--state) :fs-busy)))
        (should (eq (plist-get (nhmac-win-gui--wait :fullscreen t) :fullscreen) t))
        (set-frame-parameter nil 'fullscreen nil)
        (should (null (plist-get (nhmac-win-gui--wait :fullscreen nil) :fullscreen))))
    (nhmac--call 'variable 'ns-use-native-fullscreen t)
    (nhmac-win-gui--settle)))

(ert-deftest nhmac-miniaturize ()
  "`iconify-frame' miniaturizes the window and `make-frame-visible' brings it back."
  (skip-unless (nhmac-win-gui--naglfar-p))
  (nhmac-win-gui--settle)
  (iconify-frame)
  (should (eq (plist-get (nhmac-win-gui--wait :miniaturized t) :miniaturized) t))
  (make-frame-visible)
  (should (null (plist-get (nhmac-win-gui--wait :miniaturized nil) :miniaturized))))

(ert-deftest nhmac-appearance-dark ()
  "A change of the system appearance records it in `ns-system-appearance' and runs the hook with `dark'."
  (skip-unless (nhmac-win-gui--naglfar-p))
  (let* ((got nil) (fn (lambda (a) (setq got a))))
    (add-hook 'ns-system-appearance-change-functions fn)
    (unwind-protect
        (progn
          (nhmac--call 'test-app-appearance (if (eq ns-system-appearance 'dark) 0 1))
          (let ((want (if (eq ns-system-appearance 'dark) 'light 'dark))
                (cmd (key-binding (vector (read-event nil nil 2) (read-event nil nil 2)))))
            (should (eq cmd 'nhmac-appearance-changed))
            (call-interactively cmd)
            (should (eq got want))
            (should (eq ns-system-appearance want))))
      (remove-hook 'ns-system-appearance-change-functions fn)
      (nhmac--call 'test-app-appearance 0))))

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
  (unwind-protect
      (progn
        (nhmac--call 'variable 'ns-auto-hide-menu-bar t)
        ;; AutoHideDock (1) and AutoHideMenuBar (4), as NS sets them
        (should (= (logand (nhmac--query 'test-presentation) 5) 5)))
    (nhmac--call 'variable 'ns-auto-hide-menu-bar nil))
  (should (= (logand (nhmac--query 'test-presentation) 5) 0)))

;;; nhmac-window-gui-tests.el ends here
