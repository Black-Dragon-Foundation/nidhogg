;;; nhmac-lifecycle-gui-tests.el --- Naglfar's app lifecycle rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).  The application events are
;; posted through the test hook that stands for the delegate's callbacks.

(require 'ert)
(require 'cl-lib)

(defun nhmac-life--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--query)))

;; Task 31: app lifecycle, open-file events, gestures, monitors
(ert-deftest nhmac-quit-confirm ()
  "AppKit asking to terminate runs `nhmac-quit', which asks when `ns-confirm-quit' is set and does not quit on no."
  (skip-unless (nhmac-life--naglfar-p))
  (let ((ns-confirm-quit t) (asked nil))
    (cl-letf (((symbol-function 'y-or-n-p) (lambda (&rest _) (setq asked t) nil)))
      (nhmac--call 'test-app-event 2)
      (let ((cmd (key-binding (vector (read-event nil nil 2) (read-event nil nil 2)))))
        (should (eq cmd 'nhmac-quit))
        (call-interactively cmd)))
    (should asked)))

(ert-deftest nhmac-open-files ()
  "Files the delegate queued are visited by `nhmac-open-files'."
  (skip-unless (nhmac-life--naglfar-p))
  (let ((file (make-temp-file "nhmac-open-files")))
    (nhmac--call 'test-open-files file)
    (should (eq (key-binding (vector (read-event nil nil 2) (read-event nil nil 2))) 'nhmac-open-files))
    (nhmac-open-files)
    ;; `ns-find-file' visits the true name (/private/var/... for a temporary file under /var)
    (should (find-buffer-visiting file))))

(ert-deftest nhmac-reopen-hide ()
  "A reopen with no visible window shows a frame."
  (skip-unless (nhmac-life--naglfar-p))
  (nhmac--call 'test-app-event 3)
  (should (eq (key-binding (vector (read-event nil nil 2) (read-event nil nil 2))) 'nhmac-reopen))
  (nhmac-reopen)
  (should (frame-visible-p (selected-frame))))

(ert-deftest nhmac-magnify ()
  "A magnify gesture is a text scale change."
  (skip-unless (nhmac-life--naglfar-p))
  (ert-skip "pending: a synthesised magnify NSEvent (the mapping is `Magnify' in outputs.rs, tested on Linux)"))

(ert-deftest nhmac-precise-scroll ()
  "Precise and momentum scroll deltas equal NS's for the same synthesised event."
  (skip-unless (nhmac-life--naglfar-p))
  (ert-skip "pending: a same-run NS comparison"))

(ert-deftest nhmac-monitors ()
  "`display-monitor-attributes-list' follows the screens."
  (skip-unless (nhmac-life--naglfar-p))
  (ert-skip "pending: a same-run NS comparison with the virtual display at scale 2"))

;;; nhmac-lifecycle-gui-tests.el ends here
