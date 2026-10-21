;;; glitnir-gui-tests.el --- GUI tests for Glitnir (the compositor)  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh on a headless mutter (test/src/nidhogg-tests/gui-runner.el
;; loads every *-gui-tests.el).  GLITNIR_GUI_SCALE is the monitor scale the run asked for.

(require 'ert)
(require 'cl-lib)

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
  ;; Calls into Glitnir are made for damage now; `glitnir-gui/inactive-makes-no-calls' checks the no-call path.
  (let ((copies (glitnir--snapshot-copies)))
    (with-temp-buffer
      (switch-to-buffer (current-buffer))
      (dotimes (_ 20) (insert "y") (redisplay t) (sit-for 0.02)))
    (should (= copies (glitnir--snapshot-copies)))))

(ert-deftest glitnir-gui/daemon-first-frame-initialises ()
  (skip-unless (daemonp))
  (should (>= (plist-get (glitnir-stats) :count) 1)))

;; Damage recorded in pgtk's drawing primitives (Task 20).  Each test first takes what is pending, so it sees only
;; what its own steps drew.

(defun glitnir-gui--frame-rect ()
  "The selected frame as one device-pixel rectangle (X Y W H)."
  (list 0 0 (round (* (frame-pixel-width) (frame-scale-factor)))
        (round (* (frame-pixel-height) (frame-scale-factor)))))

(ert-deftest glitnir-gui/typing-damages-one-row ()
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (insert "hello")
    (let ((glitnir-debug 'damage-hold))
      (redisplay t) (glitnir--damage)
      (insert "x") (redisplay t)
      (let ((rows (glitnir--damage)) (lh (* (line-pixel-height) (frame-scale-factor))))
        (should rows)
        (should (cl-every (lambda (r) (<= (nth 3 r) (* 2 lh))) rows))))))

(ert-deftest glitnir-gui/garbaged-frame-damages-all ()
  (let ((glitnir-debug 'damage-hold))
    (redisplay t) (glitnir--damage)
    (redraw-frame) (redisplay t)
    (should (equal (glitnir--damage) (list (glitnir-gui--frame-rect))))))

(ert-deftest glitnir-gui/scroll-records-a-scroll-op ()
  ;; C-v moves the rows it keeps with pgtk_scroll_run: one rectangle spans the window's text area (the scroll's area
  ;; also holds the fringes; the last, partly visible row may be redrawn on its own), and the rows the scroll
  ;; covers add nothing, so the damage is a handful of rectangles rather than one per row.
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (dotimes (i 500) (insert (format "line %d\n" i)))
    (goto-char (point-min))
    (let ((glitnir-debug 'damage-hold) (s (frame-scale-factor)) (lh (line-pixel-height)))
      (redisplay t) (glitnir--damage)
      (scroll-up-command) (redisplay t)
      (pcase-let ((`(,l ,top ,r ,bottom) (window-body-pixel-edges))
                  (rects (glitnir--damage)))
        (should (cl-some (lambda (x)
                           (and (<= (nth 0 x) (* l s)) (>= (+ (nth 0 x) (nth 2 x)) (* r s))
                                (<= (nth 1 x) (* top s)) (>= (+ (nth 1 x) (nth 3 x)) (* (- bottom (* 2 lh)) s))))
                         rects))
        (should (<= (length rects) 8))))))

(ert-deftest glitnir-gui/visible-bell-damages-all ()
  (let ((glitnir-debug 'damage-hold) (visible-bell t) (ring-bell-function nil))
    (redisplay t) (glitnir--damage)
    (ding t)
    (should (equal (glitnir--damage) (list (glitnir-gui--frame-rect))))
    (sleep-for 0.2)                     ; the bell's atimer (50 ms) takes the flash back
    (should (equal (glitnir--damage) (list (glitnir-gui--frame-rect))))))

(ert-deftest glitnir-gui/export-records-nothing ()
  ;; x-export-frames first redisplays the frame for real (that may redraw the cursors), then clears and draws the
  ;; whole frame into its own surface: recorded, that would be the whole frame.
  (let ((glitnir-debug 'damage-hold))
    (redisplay t) (glitnir--damage)
    (should (stringp (x-export-frames nil 'png)))
    (let ((rects (glitnir--damage)) (frame (glitnir-gui--frame-rect)))
      (should-not (member frame rects))
      (should (< (apply #'+ (mapcar (lambda (r) (* (nth 2 r) (nth 3 r))) rects))
                 (/ (* (nth 2 frame) (nth 3 frame)) 100))))))

(ert-deftest glitnir-gui/inactive-makes-no-calls ()
  (should (glitnir-active-p))
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (redisplay t)
    (let ((glitnir-enable nil))
      (should-not (glitnir-active-p))
      (let ((calls (glitnir--calls)))
        (dotimes (_ 20) (insert "z") (redisplay t))
        (should (= calls (glitnir--calls)))))))

;;; glitnir-gui-tests.el ends here
