;;; nhwl-gui-scale-tests.el --- GUI rows for the native shell's fractional scales  -*- lexical-binding: t -*-

;; test-class: cond
;; test-pattern: handshake

;; Run by nidhogg/test/gui.sh with the base rows (nhwl-gui-tests.el, whose helpers are used here); load it with
;; `--files 'nhwl-gui*-tests.el''.  The scales every row knows: 1, 1.25, 4/3 (wire 160) and 2.  A frame's fonts are
;; sized from their setting at the frame's own resolution (96 dpi times its scale times its text zoom): the pixel size
;; of a face of HEIGHT tenths of a point is HEIGHT/10 × dpi/72, rounded half up, as a fresh open gives it.

(require 'ert)
(require 'nhwl-gui-tests
         (expand-file-name "nhwl-gui-tests" (file-name-directory (or load-file-name buffer-file-name))))

(defun nhwl-gui-scale--px (height s120 &optional zoom)
  "The pixel size of a font of HEIGHT (tenths of a point) opened fresh at scale S120/120 and text ZOOM."
  (floor (+ (/ (* (/ height 10.0) 96.0 (/ s120 120.0) (or zoom 1.0)) 72.0) 0.5)))

(defun nhwl-gui-scale--font-px (&optional face frame)
  "The pixel size of the font FACE (default `default') shows on FRAME."
  (aref (font-info (face-font (or face 'default) frame) frame) 2))

(defun nhwl-gui-scale--set (s120 &optional surface)
  "Put the recorder's scale to S120 (for SURFACE only, an id or \"toplevel\", when given) and wait for the selected
frame's scale to follow (with SURFACE: for any frame at that scale)."
  (gui-runner-recorder (append (list :cmd "scale" :value120 s120) (and surface (list :surface surface))))
  (should (nhwl-gui--wait (lambda ()
                            (nhwl-gui--pump) (redisplay t)
                            (seq-some (lambda (f) (< (abs (- (frame-scale-factor f) (/ s120 120.0))) 0.001))
                                      (if surface (nhwl-gui--nhwl-frames) (list (selected-frame)))))
                          5)))

(defmacro nhwl-gui-scale--keeping (&rest body)
  "Run BODY, then put back what the rows here change: the lane's scale, the text zoom, the `default' height (of new
frames and the selected frame) and the frame's fontset, so the rows after see the frame they started with."
  (declare (indent 0) (debug t))
  (let ((h (make-symbol "height")))
    `(let ((,h (face-attribute 'default :height t)))
       (gui-runner-keeping-frame-font
         (unwind-protect
             (progn ,@body)
           (when (fboundp 'nidhogg-set-frame-text-zoom) (nidhogg-set-frame-text-zoom nil 1.0))
           (nhwl-gui-scale--set (round (* 120 (glitnir-gui--scale))))
           (set-face-attribute 'default nil :height ,h)
           (nhwl-gui--windowed))))))

(defun nhwl-gui-scale--state (h s)
  "A line for the log: the default font at HEIGHT H and scale S, what the face says and what the row wants."
  (format "height %d at %d: %d px (want %d), font %s, face heights frame %S new %S, zoom %S"
          h s (nhwl-gui-scale--font-px) (nhwl-gui-scale--px h s) (frame-parameter nil 'font)
          (face-attribute 'default :height (selected-frame)) (face-attribute 'default :height t)
          (and (fboundp 'nidhogg-frame-text-zoom) (nidhogg-frame-text-zoom))))

;;;; The copy the compositor shows (mutter only)

(defun nhwl-gui-scale--stripes (w h)
  "A PBM image W x H of one-pixel columns, black and white, for the copy check."
  (with-temp-buffer
    (insert (format "P1\n%d %d\n" w h))
    (dotimes (_ h)
      (dotimes (x w) (insert (if (zerop (% x 2)) "1 " "0 ")))
      (insert "\n"))
    (buffer-string)))

(ert-deftest nhwl-gui/mutter-copy-is-exact ()
  "On headless mutter at the session's scale, the screen shows the frame's buffer as a 1:1 copy: one-pixel stripes
grabbed from the screen (ScreenCast) equal Glitnir's own snapshot of them, pixel for pixel."
  (skip-unless (and (nhwl-gui--live-p) (equal (glitnir-gui--compositor) "mutter") (fboundp 'glitnir-snapshot)))
  (nhwl-gui--set-fullscreen 'fullboth)
  (unwind-protect
      (with-temp-buffer
        (switch-to-buffer (current-buffer))
        (set-window-margins nil 0 0)
        (insert-image (create-image (nhwl-gui-scale--stripes 256 128) 'pbm t :scale 1 :ascent 100))
        (insert "\n" (make-string 80 ?|))
        (goto-char (point-min))
        (redisplay t)
        (gui-runner-wait-settled)
        (let* ((edges (window-inside-pixel-edges))
               (xy (posn-x-y (posn-at-point (point-min))))
               (rect (list (+ (nth 0 edges) (car xy)) (+ (nth 1 edges) (cdr xy)) 256 128))
               (grab (gui-runner-screen-grab))
               (snap (glitnir-snapshot nil 'raw))
               (diff (gui-runner-pixel-diff snap grab rect)))
          (glitnir-gui--say "copy-check: scale %s, rect %S, max channel diff %d, pixels off %d"
                            (frame-scale-factor) rect (car diff) (cdr diff))
          (should (= (cdr diff) 0))))
    (nhwl-gui--set-fullscreen nil)))

;;;; The scale the shell knows: monitors and new frames

(ert-deftest nhwl-gui/monitor-attributes-report-the-fractional-scale ()
  "The monitor a frame is on reports the scale the compositor gave the frame (1.25, not the integer hint 2)."
  (skip-unless (nhwl-gui--live-p))
  ;; a compositor says which output a window is on; the recorder only when asked
  (when (nhwl-gui--recorder-p)
    (gui-runner-recorder '(:cmd "output" :op "enter" :name "nh-0" :surface "toplevel")))
  (let ((same (lambda () (< (abs (- (alist-get 'scale-factor (frame-monitor-attributes)) (frame-scale-factor)))
                            0.001))))
    (nhwl-gui--wait-ev same)
    (glitnir-gui--say "scale-probe: frame %s, monitor %s" (frame-scale-factor)
                      (alist-get 'scale-factor (frame-monitor-attributes)))
    (should (funcall same))))

(ert-deftest nhwl-gui/first-frame-font-follows-the-scale ()
  "The first frame, opened at the output's integer hint before the compositor named its scale (2 for 1.25), shows its
font at the size a frame opened now at that scale shows."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((a (selected-frame)) b)
    (unwind-protect
        (progn
          (setq b (make-frame))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump) (frame-visible-p b)) 5))
          (glitnir-gui--say "scale-probe: first frame %s at %s, a new one %s at %s"
                            (nhwl-gui-scale--font-px nil a) (frame-scale-factor a)
                            (nhwl-gui-scale--font-px nil b) (frame-scale-factor b))
          (should (= (frame-scale-factor b) (frame-scale-factor a)))
          (should (= (nhwl-gui-scale--font-px nil a) (nhwl-gui-scale--font-px nil b))))
      (when (frame-live-p b) (delete-frame b))
      (select-frame a))))

(ert-deftest nhwl-gui/new-frame-starts-at-the-selected-frame-scale ()
  "A frame made from a frame at 1.25 starts at 1.25, though another frame on another monitor heard 2 last."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((a (selected-frame)) b c)
    (nhwl-gui-scale--keeping
      (unwind-protect
          (progn
            (nhwl-gui-scale--set 150)
            (setq b (make-frame))
            (should (nhwl-gui--wait (lambda () (nhwl-gui--pump) (frame-visible-p b)) 5))
            (nhwl-gui-scale--set 240 "toplevel")
            (should (= (frame-scale-factor b) 2.0))
            (select-frame a)
            (setq c (make-frame))
            (should (= (frame-scale-factor c) 1.25)))
        (when (frame-live-p c) (delete-frame c))
        (when (frame-live-p b) (delete-frame b))
        (select-frame a)
        (nhwl-gui--pump)))))

(provide 'nhwl-gui-scale-tests)
;;; nhwl-gui-scale-tests.el ends here
