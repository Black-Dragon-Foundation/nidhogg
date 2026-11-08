;;; nhmac-ns-gui-tests.el --- Naglfar's ns compatibility layer on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).  Each ns function NS
;; defines in C answers on Naglfar, through the shell, as NS's does.

(require 'ert)
(require 'cl-lib)

(defun nhmac-ns--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--query)))

(ert-deftest nhmac-ns-win-loads ()
  "`ns-win' is loaded and every ns function it and packages call answers without an error."
  (skip-unless (nhmac-ns--naglfar-p))
  (should (featurep 'ns-win))
  (let ((f (selected-frame)))
    ;; the geometry, the edges and the monitors are the shared C's
    (should (consp (assq 'outer-size (ns-frame-geometry f))))
    (should (= (length (ns-frame-edges f 'inner-edges)) 4))
    (should (consp (ns-display-monitor-attributes-list)))
    ;; the stacking order holds the selected frame
    (should (memq f (ns-frame-list-z-order)))
    (ns-frame-restack f f t)
    ;; the pointer goes where it is sent
    (ns-set-mouse-absolute-pixel-position 200 150)
    (should (equal (ns-mouse-absolute-pixel-position) '(200 . 150)))
    ;; the Dock tile
    (ns-badge "3")
    (ns-badge nil)
    (ns-progress-indicator 0.5)
    (ns-progress-indicator nil)
    (ns-request-user-attention 'informational)
    (ns-request-user-attention nil)
    ;; idle sleep
    (let ((token (ns-block-system-sleep "nhmac-ns-win-loads" t)))
      (should (natnump token))
      (should (eq (ns-unblock-system-sleep token) t))
      (should-not (ns-unblock-system-sleep token)))
    ;; AppleScript, both kinds of answer (as NS reads them) and an error
    (should (equal (ns-do-applescript "return \"abc\"") "abc"))
    (should (equal (ns-do-applescript "return 3") 3))
    (should-error (ns-do-applescript "this is not a script"))
    (should (memq (ns-process-is-accessibility-trusted) '(t nil)))
    (should (stringp (ns-font-name "Menlo")))
    (ns-reset-menu)))

;;; nhmac-ns-gui-tests.el ends here
