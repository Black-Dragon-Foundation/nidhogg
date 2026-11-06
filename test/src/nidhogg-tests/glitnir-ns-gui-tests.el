;;; glitnir-ns-gui-tests.el --- Glitnir on the macOS (NS) build  -*- lexical-binding: t -*-

;; test-class: cond
;; test-pattern: handshake

;; Run by nidhogg/test/ns-gui.sh on the macOS runner (test/src/nidhogg-tests/gui-runner.el loads this file only on an
;; NS build).  GLITNIR_GUI_SCALE is the display scale the run asked for.  Frame sizes on NS are logical pixels (points);
;; `glitnir-snapshot' and `glitnir--chrome' give device pixels, as on the native shell.  The pixel rows skip with the
;; reason when the process may not capture the screen (`glitnir-no-capture'); the state rows run either way.

(require 'ert)
(require 'cl-lib)

(defun glitnir-ns--scale ()
  (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1")))

(defun glitnir-ns--skip-unless-ns ()
  (unless (featurep 'ns) (ert-skip "not an NS build")))

(defun glitnir-ns--skip-unless-capture ()
  "Skip the running row unless the process may capture the screen."
  (condition-case err
      (glitnir-snapshot nil 'raw)
    (glitnir-no-capture (ert-skip (format "no screen capture: %s" (cadr err))))))

(defun glitnir-ns--child (&optional params)
  "A shown, undecorated child frame of the selected frame, with PARAMS (an alist) applied."
  (let* ((root (selected-frame))
         (f (make-frame `((parent-frame . ,root) (minibuffer . nil) (left . 40) (top . 40)
                          (width . 40) (height . 10) (background-color . "#00ffff")
                          (visibility . nil) (no-accept-focus . t) (no-other-frame . t)
                          (undecorated . t) (internal-border-width . 1)))))
    (select-frame root)
    (when params (modify-frame-parameters f params))
    (make-frame-visible f)
    (redisplay t)
    f))

(defun glitnir-ns--pair (frame)
  "The entry of `glitnir--ns-pairs' for FRAME, or nil."
  (cl-find frame (glitnir--ns-pairs) :key (lambda (e) (plist-get e :frame))))

(ert-deftest glitnir-ns/runner-smoke ()
  ;; The lane's own proof: a frame at the asked scale, and a capture of its size.
  (glitnir-ns--skip-unless-ns)
  (should (< (abs (- (frame-scale-factor) (glitnir-ns--scale))) 0.001))
  (glitnir-ns--skip-unless-capture)
  (pcase-let ((`(,w ,h ,bytes) (glitnir-snapshot nil 'raw))
              (s (frame-scale-factor)))
    (should (= w (round (* s (frame-pixel-width)))))
    (should (= h (round (* s (frame-pixel-height)))))
    (should (= (length bytes) (* 4 w h)))
    (should-not (multibyte-string-p bytes)))
  (should (string-prefix-p "\x89PNG" (glitnir-snapshot nil))))

(ert-deftest glitnir-ns/active-p-follows-enable ()
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--child)))
    (unwind-protect
        (progn
          (should glitnir-enable)
          (should (glitnir-active-p))
          (should (glitnir-active-p c))
          (let ((glitnir-enable nil))
            (should-not (glitnir-active-p))
            (should-not (glitnir-active-p c)))
          (should (glitnir-active-p c)))
      (delete-frame c t))))

(ert-deftest glitnir-ns/chrome-reaches-the-window ()
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--child '((corner-radius . 8)
                                (shadow . (:blur 16 :offset (0 . 6) :spread 2 :opacity 0.35))))))
    (unwind-protect
        (let* ((r (glitnir--resolve c)) (sh (plist-get r :shadow))
               (s (float (frame-scale-factor)))
               (ch (glitnir--chrome c)) (dsh (plist-get ch :shadow)))
          (should r)
          (should ch)
          (should (= (plist-get ch :radius) (* s (plist-get r :radius))))
          (should (= (plist-get dsh :blur) (* s (plist-get sh :blur))))
          (should (= (car (plist-get dsh :offset)) (* s (car (plist-get sh :offset)))))
          (should (= (cdr (plist-get dsh :offset)) (* s (cdr (plist-get sh :offset)))))
          (should (= (plist-get dsh :spread) (* s (plist-get sh :spread))))
          (should (< (abs (- (plist-get dsh :opacity) (plist-get sh :opacity))) 1e-6))
          (should (equal (plist-get dsh :color) (plist-get sh :color)))
          ;; Glitnir off: the window holds nothing
          (let ((glitnir-enable nil))
            (should-not (glitnir--chrome c)))
          (should (glitnir--chrome c))
          ;; opting out removes it
          (modify-frame-parameters c '((corner-radius . 0) (shadow . nil)))
          (should-not (glitnir--chrome c)))
      (delete-frame c t))))

(ert-deftest glitnir-ns/pairs-list-the-children ()
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--child)))
    (unwind-protect
        (let ((e (gui-runner-wait (lambda () (glitnir-ns--pair c)))))
          (should e)
          (should (= (length (plist-get e :child)) 4))
          (should (cl-every #'numberp (plist-get e :child)))
          (should (> (nth 2 (plist-get e :child)) 0))
          ;; no chrome, no shadow window
          (should-not (plist-get e :shadow)))
      (delete-frame c t))))

;;; glitnir-ns-gui-tests.el ends here
