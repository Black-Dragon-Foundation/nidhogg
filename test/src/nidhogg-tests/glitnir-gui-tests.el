;;; glitnir-gui-tests.el --- GUI tests for Glitnir (the compositor)  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh on a headless mutter (test/src/nidhogg-tests/gui-runner.el
;; loads every *-gui-tests.el).  GLITNIR_GUI_SCALE is the monitor scale the run asked for.

(require 'ert)

(ert-deftest glitnir-gui/a-pgtk-frame-at-the-asked-scale ()
  (let ((s (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1"))))
    (should (eq (framep (selected-frame)) 'pgtk))
    (should (= (frame-scale-factor) (ceiling s)))))

;; Stats and snapshot of the stock present path (Task 5).

(ert-deftest glitnir-gui/stats-record-the-stock-paint ()
  (glitnir-stats nil t)
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (dotimes (_ 5) (insert "x") (redisplay t) (sit-for 0.05))
    (let ((s (glitnir-stats)))
      (should (>= (plist-get s :count) 5))
      (should (= (plist-get s :painted-px)      ; stock paints the whole widget
                 (round (* (frame-pixel-width) (frame-pixel-height)
                           (expt (frame-scale-factor) 2))))))))

(ert-deftest glitnir-gui/snapshot-has-the-frame-size ()
  (pcase-let ((`(,w ,h ,bytes) (glitnir-snapshot nil 'raw)))
    (should (= w (round (* (frame-pixel-width) (frame-scale-factor)))))
    (should (= h (round (* (frame-pixel-height) (frame-scale-factor)))))
    (should (= (length bytes) (* 4 w h)))
    (should-not (multibyte-string-p bytes))))

(ert-deftest glitnir-gui/snapshot-png-is-a-png ()
  (let ((png (glitnir-snapshot nil)))
    (should-not (multibyte-string-p png))
    (should (string-prefix-p "\x89PNG" png))))

(ert-deftest glitnir-gui/snapshot-unarmed-costs-nothing ()
  (let ((copies (glitnir--snapshot-copies)) (calls (glitnir--calls)))
    (with-temp-buffer
      (switch-to-buffer (current-buffer))
      (dotimes (_ 20) (insert "y") (redisplay t) (sit-for 0.02)))
    (should (= copies (glitnir--snapshot-copies)))
    (should (= calls (glitnir--calls)))
    (should (= 0 (glitnir--calls)))))

(ert-deftest glitnir-gui/daemon-first-frame-initialises ()
  (skip-unless (daemonp))
  (should (>= (plist-get (glitnir-stats) :count) 1)))

;;; glitnir-gui-tests.el ends here
