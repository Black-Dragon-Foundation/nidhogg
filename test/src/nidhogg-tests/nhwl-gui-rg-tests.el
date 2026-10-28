;;; nhwl-gui-rg-tests.el --- GUI rows for the native shell's glue in Rust  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh with the W1 rows (nhwl-gui-tests.el, whose helpers are used here).  The runner's default
;; file pattern (`-gui-tests.el') does not match this file's name: load it with `--files 'nhwl-gui*-tests.el''.  Each
;; port task appends its rows under `;; Port Task N:'.

(require 'ert)
(require 'nhwl-gui-tests
         (expand-file-name "nhwl-gui-tests" (file-name-directory (or load-file-name buffer-file-name))))

;; Port Task 1b: a panic inside the glue drops its display, never Emacs.  `nhwl--test-glue-panic' exists only in a
;; build with checking (the glue's `test-hooks').

(defun nhwl-gui--clients ()
  "The recorder's connected clients."
  (plist-get (gui-runner-recorder '(:cmd "clients")) :clients))

(defun nhwl-gui--toplevels ()
  "The number of toplevel surfaces the recorder shows."
  (seq-count (lambda (n) (equal (plist-get n :role) "toplevel")) (nhwl-gui--tree)))

;; The row drops the session's display, so it runs last (tag `runs-last').
(ert-deftest nhwl-gui/glue-panic-loses-the-display-not-emacs ()
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--recorder-p) (daemonp) (fboundp 'nhwl--test-glue-panic)))
  (let ((f (car (nhwl-gui--nhwl-frames))))
    (should f)
    (should (nhwl-gui--wait #'nhwl-gui--toplevel-commits))
    (should (= (length (nhwl-gui--clients)) 1))
    (should (= (nhwl--test-glue-panic f) -1))
    ;; within 1 s the frame is gone and the daemon is not
    (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.05) (not (frame-live-p f))) 1))
    (should (daemonp))
    (should (string-match-p "nhg_test_panic" (or (car (nhwl-glue-panics)) "")))
    ;; the connection was shut down: the recorder saw the client go, and its windows with it
    (should (nhwl-gui--wait (lambda () (null (nhwl-gui--clients))) 1))
    (should (= (nhwl-gui--toplevels) 0))
    ;; a new frame on the (restarted) compositor works
    (when (gui-runner-restart-available-p)
      (gui-runner-restart-compositor))
    (let ((new (make-frame-on-display (getenv "WAYLAND_DISPLAY"))))
      (should (frame-live-p new))
      (should (nhwl-gui--wait (lambda () (with-selected-frame new (redisplay t)) (nhwl-gui--toplevel-commits))
                              5)))))

(provide 'nhwl-gui-rg-tests)

;;; nhwl-gui-rg-tests.el ends here
