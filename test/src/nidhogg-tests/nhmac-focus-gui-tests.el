;;; nhmac-focus-gui-tests.el --- Naglfar's focus across window transitions on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh, before the window rows (nhmac-lane.txt) and, for one row, after every other.
;; Round 11: after the full-screen and Dock rows the window was key and the application active, yet every key through
;; the text input system (Option, dead keys, marked text) reached nothing and the cursor was drawn as a frame's that
;; is not the highlight frame.  A user who goes full screen and back must still compose: each row here goes through
;; the transition itself, so it holds whatever ran before it.

(require 'ert)
(declare-function glitnir-gui--say "gui-runner" (fmt &rest args)) ; the lane's log, not *Messages*
(require 'cl-lib)

(defun nhmac-focus--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--query)))

(defun nhmac-focus--wait (key want)
  "Wait up to 12 s (AppKit's animations on a CI runner) for the window state KEY to be WANT; the state."
  (let ((deadline (+ (float-time) 12)) st)
    (while (and (progn (setq st (nhmac--query 'test-window-state (selected-frame)))
                       (not (eq (plist-get st key) want)))
                (< (float-time) deadline))
      (sit-for 0.2))
    st))

(defun nhmac-focus--compose (row what)
  "Type Option-e e with Option not a modifier and return the events that came, logging them with ROW and WHAT.
The keys go through the window server, as typing does: its keyboard layout makes the dead key (round 13: keys the
app posts itself, `test-post-key', never compose, whoever is first responder)."
  (let ((access (nhmac--query 'test-post-access)))
    (unless (eq access t)
      (ert-skip (format "the runner's grant does not allow posting events (CGPreflightPostEventAccess %S)" access))))
  (let ((was ns-alternate-modifier) evs)
    (unwind-protect
        (progn
          (setq ns-alternate-modifier 'none)
          (while (read-event nil nil 0.1))
          ;; key code 14 is e on the ANSI layout; Option is kCGEventFlagMaskAlternate (1 << 19)
          (nhmac--call 'test-real-key 14 (ash 1 19))
          (nhmac--call 'test-real-key 14 0)
          (let ((deadline (+ (float-time) 1.5)) ev)
            (while (and (< (float-time) deadline) (setq ev (read-event nil nil 0.3)))
              (push ev evs))))
      (setq ns-alternate-modifier was))
    (setq evs (nreverse evs))
    (glitnir-gui--say "%s: %s: events %S, app %S, frames %S" row what evs
                      (nhmac--query 'test-app-state (selected-frame)) (nhwl--focus-frames))
    evs))

(defun nhmac-focus--check (row what)
  "Assert, after WHAT, that Option-e e is é and that the selected frame is the display's highlight frame (its cursor
drawn active); ROW names it."
  (should (equal (list row what (memq ?é (nhmac-focus--compose row what)) t)
                 (list row what (list ?é) t)))
  (redisplay t)
  (should (equal (list row what (plist-get (nhwl--focus-frames) :highlight))
                 (list row what (selected-frame)))))

(ert-deftest nhmac-focus-compose-across-full-screen ()
  "Option-e e is é, and the frame is the highlight frame, before native full screen and after it is left."
  (skip-unless (nhmac-focus--naglfar-p))
  (let ((ns-use-native-fullscreen t) (row 'nhmac-focus-compose-across-full-screen))
    (nhmac--call 'variable 'ns-use-native-fullscreen t)
    (nhmac-focus--wait :fs-busy nil)
    (nhmac-focus--check row "before")
    (unwind-protect
        (progn
          (set-frame-parameter nil 'fullscreen 'fullboth)
          (should (plist-get (nhmac-focus--wait :fullscreen t) :fullscreen))
          (nhmac-focus--wait :fs-busy nil)
          (set-frame-parameter nil 'fullscreen nil)
          (should-not (plist-get (nhmac-focus--wait :fullscreen nil) :fullscreen))
          (nhmac-focus--wait :fs-busy nil)
          (glitnir-gui--say "%s: notes %S" row (nhmac--query 'test-key-log))
          (nhmac-focus--check row "after full screen and back"))
      (when (plist-get (nhmac--query 'test-window-state (selected-frame)) :fullscreen)
        (set-frame-parameter nil 'fullscreen nil)
        (nhmac-focus--wait :fullscreen nil)
        (nhmac-focus--wait :fs-busy nil)))))

(ert-deftest nhmac-focus-compose-across-the-dock ()
  "Option-e e is é, and the frame is the highlight frame, after the window was miniaturized and brought back."
  (skip-unless (nhmac-focus--naglfar-p))
  (let ((row 'nhmac-focus-compose-across-the-dock))
    (nhmac-focus--wait :fs-busy nil)
    (unwind-protect
        (progn
          (iconify-frame)
          (should (plist-get (nhmac-focus--wait :miniaturized t) :miniaturized))
          (make-frame-visible)
          (should-not (plist-get (nhmac-focus--wait :miniaturized nil) :miniaturized))
          (glitnir-gui--say "%s: notes %S" row (nhmac--query 'test-key-log))
          (nhmac-focus--check row "after the Dock"))
      (make-frame-visible))))

(ert-deftest nhmac-focus-compose-after-every-transition ()
  "After every other row (full screen, the Dock, child frames), Option-e e is é and the frame is the highlight frame."
  :tags '(runs-last)
  (skip-unless (nhmac-focus--naglfar-p))
  (let ((row 'nhmac-focus-compose-after-every-transition))
    (glitnir-gui--say "%s: notes %S" row (nhmac--query 'test-key-log))
    (nhmac-focus--check row "after every other row")))

;;; nhmac-focus-gui-tests.el ends here
