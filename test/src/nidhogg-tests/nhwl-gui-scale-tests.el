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

;;;; The default fringes

(defun nhwl-gui-scale--fringes ()
  "The selected window's fringes (left right) and what a default fringe of 8 logical pixels is at the frame's scale."
  (let ((w (window-fringes)))
    (list (list (nth 0 w) (nth 1 w))
          (let ((px (round (* 8 (frame-scale-factor))))) (list px px)))))

(ert-deftest nhwl-gui/default-fringe-follows-the-scale ()
  "A frame's default fringes are 8 logical pixels, as on NS and pgtk: 8 device pixels times its scale, and again
after the scale changes (the recorder: 1 → 2 → 1.25 → 1)."
  (skip-unless (nhwl-gui--live-p))
  (let ((old (list (frame-parameter nil 'left-fringe) (frame-parameter nil 'right-fringe))))
    (unwind-protect
        (progn
          (modify-frame-parameters nil '((left-fringe) (right-fringe)))
          (let ((f (nhwl-gui-scale--fringes)))
            (glitnir-gui--say "fringe-probe: at %s fringes %S, want %S" (frame-scale-factor) (car f) (cadr f))
            (should (equal (car f) (cadr f))))
          (when (nhwl-gui--recorder-p)
            (nhwl-gui-scale--keeping
              (dolist (s '(240 150 120))
                (nhwl-gui-scale--set s)
                (let ((f (nhwl-gui-scale--fringes)))
                  (glitnir-gui--say "fringe-probe: at %s fringes %S, want %S" (frame-scale-factor) (car f) (cadr f))
                  (should (equal (car f) (cadr f))))))))
      (modify-frame-parameters nil `((left-fringe . ,(car old)) (right-fringe . ,(cadr old)))))))

(defun nhwl-gui-scale--box-lines ()
  "The device width a `:box 1' face adds on each side of a character in the selected window, and what one logical
pixel is at the frame's scale (rounded, at least 1)."
  (with-temp-buffer
    (set-window-buffer nil (current-buffer))
    ;; whole lines: the box's right line goes on the glyph after the run
    (insert "xxx\n")
    (let ((eol (point)))
      (insert "x" (propertize "x" 'face '(:box 1)) "x\n")
      (let ((plain (car (window-text-pixel-size nil 1 (1- eol))))
            (boxed (car (window-text-pixel-size nil eol (1- (point-max))))))
        (list (/ (- boxed plain) 2) (max 1 (round (frame-scale-factor))))))))

(ert-deftest nhwl-gui/box-lines-follow-the-scale ()
  "A face's box line widths are logical pixels, as NS's points: a `:box 1' face is one logical pixel a side, so 2
device pixels at scale 2 (the recorder: 1 → 2 → 1.25 → 1; the box lines rounded, at least 1)."
  (skip-unless (nhwl-gui--live-p))
  (save-window-excursion
    (let ((b (nhwl-gui-scale--box-lines)))
      (glitnir-gui--say "box-probe: at %s a side %S, want %S" (frame-scale-factor) (car b) (cadr b))
      (should (equal (car b) (cadr b))))
    (when (nhwl-gui--recorder-p)
      (nhwl-gui-scale--keeping
        (dolist (s '(240 150 120))
          (nhwl-gui-scale--set s)
          (let ((b (nhwl-gui-scale--box-lines)))
            (glitnir-gui--say "box-probe: at %s a side %S, want %S" (frame-scale-factor) (car b) (cadr b))
            (should (equal (car b) (cadr b)))))))))

;;;; Fonts from their setting, per frame

(ert-deftest nhwl-gui/scale-round-trip-from-1.25-keeps-the-font ()
  "Heights whose sizes sit on the rounding edges, through 1.25 → 1 → 1.25 → 2 → 4/3 → 1.25: at each step the default
font is the size a fresh open at that scale gives, not one carried over from the scale before."
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui-scale--keeping
    (dolist (h '(101 102 105 113))
      (nhwl-gui-scale--set 150)
      (set-face-attribute 'default nil :height h)
      ;; from the first scale change on (setting the height opens the font on Emacs's own path)
      (dolist (s '(120 150 240 160 150))
        (nhwl-gui-scale--set s)
        (glitnir-gui--say "scale-probe: %s" (nhwl-gui-scale--state h s))
        (should (= (nhwl-gui-scale--font-px) (nhwl-gui-scale--px h s)))))))

(ert-deftest nhwl-gui/scale-round-trip-from-2-keeps-the-font ()
  "The same from 2: 2 → 1 → 1.25 → 2."
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui-scale--keeping
    (dolist (h '(102 105))
      (nhwl-gui-scale--set 240)
      (set-face-attribute 'default nil :height h)
      (dolist (s '(120 150 240))
        (nhwl-gui-scale--set s)
        (glitnir-gui--say "scale-probe: %s" (nhwl-gui-scale--state h s))
        (should (= (nhwl-gui-scale--font-px) (nhwl-gui-scale--px h s)))))))

(defface nhwl-gui-scale--probe '((t :height 120)) "A face first realised after the frames have their scales.")

(ert-deftest nhwl-gui/two-frames-keep-their-own-dpi ()
  "Frame A at 1.25, frame B at 2: a face realised on each afterwards has each frame's own size (20 and 32 px for
12 pt), not the size at the scale the last frame heard."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((a (selected-frame)) b)
    (nhwl-gui-scale--keeping
      (unwind-protect
          (progn
            (nhwl-gui-scale--set 150)
            (setq b (make-frame))
            (should (nhwl-gui--wait (lambda () (nhwl-gui--pump) (frame-visible-p b)) 5))
            (nhwl-gui-scale--set 240 "toplevel")
            (should (= (frame-scale-factor a) 1.25))
            (should (= (frame-scale-factor b) 2.0))
            (glitnir-gui--say "scale-probe: probe face %d px on A, %d px on B"
                              (nhwl-gui-scale--font-px 'nhwl-gui-scale--probe a)
                              (nhwl-gui-scale--font-px 'nhwl-gui-scale--probe b))
            (should (= (nhwl-gui-scale--font-px 'nhwl-gui-scale--probe a) (nhwl-gui-scale--px 120 150)))
            (should (= (nhwl-gui-scale--font-px 'nhwl-gui-scale--probe b) (nhwl-gui-scale--px 120 240))))
        (when (frame-live-p b) (delete-frame b))
        (select-frame a)
        (nhwl-gui--pump)))))

;;;; The text zoom

(ert-deftest nhwl-gui/text-zoom-sizes-the-fonts ()
  "A text zoom of 1.1 at 1.25 opens the fonts at 96 × 1.25 × 1.1 = 132 dpi; zoom 1 gives the scale's size back, and
a scale change keeps the zoom."
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui-scale--keeping
    (nhwl-gui-scale--set 150)
    (set-face-attribute 'default nil :height 101)
    ;; the fonts are re-opened inside the call
    (nidhogg-set-frame-text-zoom nil 1.1)
    (glitnir-gui--say "scale-probe: zoom 1.1: %s" (nhwl-gui-scale--state 101 150))
    (should (= (nidhogg-frame-text-zoom) 1.1))
    (should (= (nhwl-gui-scale--font-px) (nhwl-gui-scale--px 101 150 1.1)))
    (nhwl-gui-scale--set 240)
    (should (= (nhwl-gui-scale--font-px) (nhwl-gui-scale--px 101 240 1.1)))
    (nhwl-gui-scale--set 150)
    (should (= (nhwl-gui-scale--font-px) (nhwl-gui-scale--px 101 150 1.1)))
    (nidhogg-set-frame-text-zoom nil 1.0)
    (should (= (nhwl-gui-scale--font-px) (nhwl-gui-scale--px 101 150)))))

(ert-deftest nhwl-gui/child-frame-takes-the-parent-zoom ()
  "A child frame is made at its parent's text zoom."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((child nil))
    (nhwl-gui-scale--keeping
      (unwind-protect
          (progn
            (nidhogg-set-frame-text-zoom nil 1.2)
            (setq child (make-frame `((parent-frame . ,(selected-frame)) (width . 20) (height . 4) (visibility . nil))))
            (should (= (nidhogg-frame-text-zoom child) 1.2)))
        (when (frame-live-p child) (delete-frame child))))))

;;;; A frame's monitor change

(ert-deftest nhwl-gui/monitor-change-runs-move-frame-functions ()
  "A frame that goes from one monitor to another at the same scale runs `move-frame-functions' once, and its
monitor is the new one."
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((moved nil) (f (selected-frame))
         ;; not `fn': `nhwl-gui--with-second-output' binds that name for its own hook
         (on-move (lambda (frame) (push frame moved))))
    (nhwl-gui--with-second-output
     (add-hook 'move-frame-functions on-move)
     (unwind-protect
         (progn
           (gui-runner-recorder '(:cmd "output" :op "enter" :name "nh-1" :surface "toplevel"))
           (gui-runner-recorder '(:cmd "output" :op "leave" :name "nh-0" :surface "toplevel"))
           (nhwl-gui--wait-ev (lambda () moved))
           (should moved)
           ;; absence: a second run for the one move would come now
           (nhwl-gui--observe 0.3)
           (should (equal moved (list f)))
           (should (equal (alist-get 'name (frame-monitor-attributes f)) "nh-1")))
       (remove-hook 'move-frame-functions on-move)
       (gui-runner-recorder '(:cmd "output" :op "enter" :name "nh-0" :surface "toplevel"))
       (gui-runner-recorder '(:cmd "output" :op "leave" :name "nh-1" :surface "toplevel"))
       (nhwl-gui--pump)))))

(ert-deftest nhwl-gui/a-pending-move-frame-is-not-input ()
  "A queued `move-frame' (here: the frame entered another monitor) is in `while-no-input-ignore-events' by default, so
`input-pending-p' does not count it and `while-no-input' runs its body.  company-box fetched a child frame's first
documentation under `while-no-input' as the frame entered its monitor, got nothing and never asked again."
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((moved nil) (f (selected-frame))
         (on-move (lambda (frame) (push frame moved))))
    (nhwl-gui--with-second-output
     (add-hook 'move-frame-functions on-move)
     (unwind-protect
         (progn
           (gui-runner-read-all)
           (gui-runner-recorder '(:cmd "output" :op "enter" :name "nh-1" :surface "toplevel"))
           ;; the event is queued: only focus events are left out of this count
           (should (nhwl-gui--wait (lambda () (let ((input-pending-p-filter-events nil)) (input-pending-p)))))
           (should-not (input-pending-p))
           (should (eq (while-no-input 'ran) 'ran))
           ;; and it was the move: reading it runs the hook
           (nhwl-gui--wait-ev (lambda () moved))
           (should (equal moved (list f))))
       (remove-hook 'move-frame-functions on-move)
       ;; only the enter is undone: the frame is back to no output entered, as the session began
       (gui-runner-recorder '(:cmd "output" :op "leave" :name "nh-1" :surface "toplevel"))
       (nhwl-gui--wait-ev (lambda () (null (alist-get 'frames (nhwl-gui--monitor "nh-1")))))
       (nhwl-gui--pump)))))

;;;; A full-screen frame whose output changes scale

(defun nhwl-gui-scale--fill (s120 w h)
  "Full screen at S120 on a W x H output (device pixels): the compositor's configure with the output's logical size
first, then the scale, as mutter sends a display settings change.  Return (S120 WIDTH HEIGHT) once the frame fills
the output, else as it is."
  (gui-runner-recorder (list :cmd "configure" :surface "focused"
                             :size (vector (round (* w 120) s120) (round (* h 120) s120))
                             :states ["fullscreen" "activated"]))
  (nhwl-gui-scale--set s120)
  (nhwl-gui--wait (lambda () (nhwl-gui--pump) (redisplay t) (= (frame-pixel-width) w)) 5)
  (list s120 (frame-pixel-width) (frame-pixel-height)))

(ert-deftest nhwl-gui/fullscreen-fills-the-output-across-a-scale-change ()
  "A full-screen frame whose output goes 1.25 → 1 → 2 → 1.25 fills the output in device pixels at each step: the
new logical size at the new scale, not at the old one (it was 3600x2250 at 1 after 1.25 on a 2880x1800 output, and a
full-screen frame keeps the compositor's size, so the scale change did not correct it)."
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((size (mapcar #'string-to-number (split-string (getenv "GLITNIR_GUI_SIZE") "x")))
         (w (car size)) (h (cadr size)) got)
    (nhwl-gui-scale--keeping
      (set-frame-parameter nil 'fullscreen 'fullboth)
      (dolist (s '(150 120 240 150))
        (push (nhwl-gui-scale--fill s w h) got)))
    ;; full screen again at the lane's scale, as the runner started it
    (nhwl-gui-scale--fill (round (* 120 (glitnir-gui--scale))) w h)
    (should-not (seq-remove (lambda (r) (equal (cdr r) (list w h))) (nreverse got)))))

(provide 'nhwl-gui-scale-tests)
;;; nhwl-gui-scale-tests.el ends here
