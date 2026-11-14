;;; nhmac-watchdog-gui-tests.el --- the stall watchdog on Naglfar's run loop  -*- lexical-binding: t -*-

;; test-class: cond
;; test-pattern: handshake

;; Run by nidhogg/test/nhmac-gui.sh on the macOS runner (nhmac-lane.txt).  AppKit runs nested run loops on the main
;; thread while Lisp waits for them (a popup menu's tracking): their sleeps are waits, not a stall, and the shell's
;; pump running the run loop inside Emacs's own wait neither ends that wait nor makes it busy.

(require 'ert)
(require 'cl-lib)
(declare-function gui-runner-observe "gui-runner" (seconds &optional tick))

(defun nhmac-watchdog--say (fmt &rest args)
  "Print FMT with ARGS as a line of the lane's log (stderr)."
  (princ (concat (apply #'format fmt args) "\n") #'external-debugging-output))

(defmacro nhmac-watchdog--running (&rest body)
  "Run BODY with the watchdog on at 100 ms, the test's own span dropped first; stop it after."
  (declare (indent 0))
  `(progn
     (nidhogg-watchdog--start 100 600000 nil)
     (unwind-protect
         (progn
           (nidhogg-watchdog--test-idle-wait 0.001)
           (nidhogg-watchdog--take)
           ,@body)
       (ignore-errors (nidhogg-watchdog--stop)))))

(defun nhmac-watchdog--spin (ms)
  "Keep the main thread busy for MS milliseconds."
  (let ((end (+ (float-time) (/ ms 1000.0))))
    (while (< (float-time) end))))

(defun nhmac-watchdog--walls ()
  "The finished stalls' wall times (ms), oldest first, after a short idle wait closes the current span."
  (nidhogg-watchdog--test-idle-wait 0.001)
  (mapcar (lambda (st) (plist-get st :wall-ms)) (car (nidhogg-watchdog--take))))

(ert-deftest nhmac-watchdog-run-loop-sleeps-are-waits ()
  "A popup menu tracked for 600 ms between two 300 ms spins is two stalls of about 300 ms, not one of 1200 ms (busy);
`sit-for' while the shell's pump runs the run loop is no stall at all (idle)."
  (skip-unless (and (featurep 'nidhogg-macos) (fboundp 'nidhogg-watchdog--start)))
  (let ((menu '("T" ("P" ("a" . first) ("b" . second)))))
    ;; busy: the tracking's sleeps end the span and the wake starts the next
    (nhmac-watchdog--running
      (nhmac-watchdog--spin 300)
      (nhmac--call 'test-popup-end 0.6)
      (x-popup-menu t menu)
      (nhmac-watchdog--spin 300)
      (let ((walls (nhmac-watchdog--walls)))
        (nhmac-watchdog--say "nhmac-watchdog: busy, stalls %S ms" walls)
        (should (>= (length walls) 2))
        (should (cl-every (lambda (w) (< w 700)) walls))))
    ;; idle: inside Emacs's own wait the observer marks nothing, and its wake never makes the wait busy
    (nhmac-watchdog--running
      ;; a span of Emacs idle in its own wait, not a wait for something
      (gui-runner-observe 1)
      (let ((walls (nhmac-watchdog--walls)))
        (nhmac-watchdog--say "nhmac-watchdog: idle, stalls %S ms" walls)
        (should (null walls))))))

(provide 'nhmac-watchdog-gui-tests)
;;; nhmac-watchdog-gui-tests.el ends here
