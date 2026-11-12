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
  "Wait up to 4 s for the window state KEY to be WANT; the state."
  (let ((deadline (+ (float-time) 4)) st)
    (while (and (progn (setq st (nhmac-win-gui--state)) (not (eq (plist-get st key) want)))
                (< (float-time) deadline))
      (sit-for 0.2))
    (unless (eq (plist-get st key) want)
      (glitnir-gui--say "nhmac-win-gui--wait: %S never %S: %S" key want st))
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

(ert-deftest nhmac-fullscreen-native-ns-control ()
  "The control for `nhmac-fullscreen-native' on the stock NS port: on this runner, native full screen is entered and
left, each within 4 s (the frame takes the screen's width, then gives it back)."
  (skip-unless (and (featurep 'ns) (not (nhmac-win-gui--naglfar-p))))
  (let* ((ns-use-native-fullscreen t)
         (w0 (frame-pixel-width))
         (screen (nth 2 (frame-monitor-workarea)))
         (wait (lambda (pred)
                 (let ((t0 (float-time)))
                   (while (and (not (funcall pred)) (< (- (float-time) t0) 4))
                     (sit-for 0.2))
                   (and (funcall pred) (- (float-time) t0))))))
    (set-frame-parameter nil 'fullscreen 'fullboth)
    (let ((in (funcall wait (lambda () (>= (frame-pixel-width) (- screen 20))))))
      (set-frame-parameter nil 'fullscreen nil)
      (let ((out (funcall wait (lambda () (<= (frame-pixel-width) (+ w0 20))))))
        (glitnir-gui--say "nhmac-fullscreen-native-ns-control: width %d, screen %d; in after %S s, out after %S s"
                          w0 screen in out)
        (should in)
        (should out)))))

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
