;;; glitnir-nhmac-gui-tests.el --- Glitnir's child chrome on Naglfar  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh on the macOS runner, with nhmac-gui-tests.el (its helpers).  Glitnir P5's NS rows,
;; rebuilt on Naglfar's layers (design §5.7): the corners and the border are the content layer's properties, the
;; shadow a nine-slice image on a sublayer of the child's container, below its content.  The state rows read the
;; layers back in process (`nhmac--test-child'); the pixel rows capture the screen and find the child by its colour,
;; so they need no window coordinates.

(require 'ert)
(require 'cl-lib)

(defvar nhwl-scroll-layers)
(unless (fboundp 'nhmac-gui--child)
  (load (expand-file-name "nhmac-gui-tests" (file-name-directory (or load-file-name buffer-file-name))) nil t))

(defconst glitnir-nhmac--shadow '(:blur 16 :offset (0 . 6) :spread 2 :opacity 0.35)
  "The shadow the chrome rows give their child frames (P5's).")

(defconst glitnir-nhmac--bg "#ffffff" "The root's background in the pixel rows.")

(defun glitnir-nhmac--chromed (&optional params left top)
  "A shown child frame (`nhmac-gui--child', #ff00ff) of 30x8 characters with corners of 12 and
`glitnir-nhmac--shadow', then PARAMS, at LEFT, TOP (default 80, 200: below the root's tool bar)."
  (nhmac-gui--child (append params
                            (seq-remove (lambda (d) (assq (car d) params))
                                        `((corner-radius . 12) (shadow . ,glitnir-nhmac--shadow)
                                          (width . 30) (height . 8))))
                    (or left 80) (or top 200)))

(defun glitnir-nhmac--scale ()
  "The device pixels per point of the selected frame's window."
  (plist-get (nhmac--test-surface 0 0) :scale))

(defun glitnir-nhmac--with-white (fn)
  "Call FN with the selected frame's background `glitnir-nhmac--bg' and an empty buffer in its window, then as before.
The probes then read the root's background wherever no child is.  FN runs after `glitnir-nhmac--clean-root'."
  (let* ((f (selected-frame)) (bg (frame-parameter f 'background-color))
         (conf (current-window-configuration f))
         (blank (get-buffer-create " *glitnir-nhmac-blank*")))
    (with-current-buffer blank (setq-local mode-line-format nil cursor-type nil))
    (set-frame-parameter f 'background-color glitnir-nhmac--bg)
    ;; one window: an earlier row's split leaves no mode line or text where a probe reads
    (delete-other-windows (frame-selected-window f))
    (set-window-buffer (frame-selected-window f) blank)
    (unwind-protect (progn (nhmac-gui--settle) (glitnir-nhmac--clean-root) (funcall fn))
      (set-window-configuration conf)
      (set-frame-parameter f 'background-color bg))))

(defun glitnir-nhmac--grab (name)
  "The screen around the selected frame, as `nhmac-gui--bmp-read' gives it, after the layers settled.
Saved as NAME.bmp under GLITNIR_GUI_OUT when that is set, so a failing row leaves its evidence."
  (nhmac-gui--settle)
  (sit-for 0.3)
  (let* ((dir (or (getenv "GLITNIR_GUI_OUT") temporary-file-directory))
         (file (expand-file-name (format "%s-%s.bmp" name (getenv "GLITNIR_GUI_SCALE")) dir)))
    (make-directory dir t)
    (nhmac-gui--bmp-read (nhmac-gui--capture file))))

(defun glitnir-nhmac--box (img rgb)
  "The bounding box (X0 Y0 X1 Y1, inclusive) of the pixels of IMG within 8 of RGB, or nil."
  (pcase-let ((`(,w ,h ,px) img) (x0 nil) (y0 nil) (x1 nil) (y1 nil))
    (dotimes (y h)
      (dotimes (x w)
        (unless (nhmac-gui--far (aref px (+ (* y w) x)) rgb 8)
          (setq x0 (min (or x0 x) x) y0 (min (or y0 y) y) x1 (max (or x1 x) x) y1 (max (or y1 y) y)))))
    (and x0 (list x0 y0 x1 y1))))

(defun glitnir-nhmac--px (img x y)
  "IMG's #xRRGGBB at X, Y."
  (aref (nth 2 img) (+ (* y (nth 0 img)) x)))

(defun glitnir-nhmac--luma (rgb)
  "RGB's luma, 0 to 255."
  (/ (+ (* 299 (ash rgb -16)) (* 587 (logand (ash rgb -8) 255)) (* 114 (logand rgb 255))) 1000))

(ert-deftest glitnir-nhmac/chrome-reaches-the-layers ()
  "A child's corner radius, border and shadow reach its layers: the content's `cornerRadius' (points) with
`masksToBounds', its `borderWidth', and a shown shadow sublayer with an image, below the content and larger than it."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((c (glitnir-nhmac--chromed '((internal-border-width . 2)))))
    (unwind-protect
        (let* ((i (nhmac--test-child c)) (sh (plist-get i :shadow)) (fr (plist-get sh :frame)))
          (nhmac-gui--say "glitnir-nhmac/chrome-reaches-the-layers: %S" i)
          (should (= (plist-get i :radius) 12))
          (should (eq (plist-get i :masks) t))
          (should (> (plist-get i :border-width) 0))
          (should sh)
          (should (eq (plist-get sh :image) t))
          (should (= (plist-get i :shadow-index) 0))
          (should (> (nth 2 fr) (plist-get i :w)))
          (should (> (nth 3 fr) (plist-get i :h)))
          ;; the offset is down: the shadow reaches further below the content than above it
          (should (< (nth 1 fr) (- (plist-get i :h) (+ (nth 1 fr) (nth 3 fr)))))
          ;; no margin: the container is the native size
          (should (= (round (* (glitnir-nhmac--scale) (plist-get i :h))) (frame-pixel-height c))))
      (delete-frame c t))))

(ert-deftest glitnir-nhmac/no-shadow-layer-without-shadow ()
  "A child without a shadow has its shadow layer hidden and no extra window in the window server's list."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((n (length (nhmac--test-windows)))
         (c (nhmac-gui--child '((corner-radius . 12)))))
    (unwind-protect
        (let ((i (nhmac--test-child c)))
          (nhmac-gui--say "glitnir-nhmac/no-shadow-layer-without-shadow: %S" i)
          (should (= (plist-get i :radius) 12))
          (should-not (plist-get i :shadow))
          (should (nhmac-gui--windows-become n)))
      (delete-frame c t))))

(ert-deftest glitnir-nhmac/alpha-applies-to-container ()
  "A child's alpha is its container's opacity, which holds the shadow and the content together."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((c (glitnir-nhmac--chromed '((alpha . 0.5)))))
    (unwind-protect
        (let ((i (nhmac--test-child c)))
          (nhmac-gui--say "glitnir-nhmac/alpha-applies-to-container: %S" i)
          (should (< (abs (- (plist-get i :opacity) 0.5)) 0.01))
          (should (= (plist-get i :shadow-index) 0)))
      (delete-frame c t))))

(ert-deftest glitnir-nhmac/corner-shows-what-is-beneath ()
  "A rounded child's corner shows the frame beneath it: at the corner of the child's bounding box the root's white,
on the middle of its top edge the child's own colour."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--with-white
   (lambda ()
     (let ((c (glitnir-nhmac--chromed '((shadow . nil)))))
       (unwind-protect
           (let* ((img (glitnir-nhmac--grab "corner"))
                  (b (glitnir-nhmac--box img #xff00ff)))
             (nhmac-gui--say "glitnir-nhmac/corner-shows-what-is-beneath: box %S, corner %06x, edge %06x" b
                             (and b (glitnir-nhmac--px img (nth 0 b) (nth 1 b)))
                             (and b (glitnir-nhmac--px img (/ (+ (nth 0 b) (nth 2 b)) 2) (nth 1 b))))
             (should b)
             (should-not (nhmac-gui--far (glitnir-nhmac--px img (nth 0 b) (nth 1 b)) #xffffff 24))
             (should-not (nhmac-gui--far (glitnir-nhmac--px img (nth 2 b) (nth 3 b)) #xffffff 24))
             (should-not (nhmac-gui--far (glitnir-nhmac--px img (/ (+ (nth 0 b) (nth 2 b)) 2) (nth 1 b)) #xff00ff 8)))
         (delete-frame c t))))))

(ert-deftest glitnir-nhmac/shadow-darkens-below ()
  "The shadow darkens the root below the child more than above it (its offset is down), and a child without one
leaves the root white there."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--with-white
   (lambda ()
     (let* ((s (glitnir-nhmac--scale))
            (probe (lambda (name params)
                     (let ((c (glitnir-nhmac--chromed params)))
                       (nhmac-gui--say "glitnir-nhmac/shadow-darkens-below: %s shadow %S, chrome %S" name
                                       (frame-parameter c 'shadow) (plist-get (nhmac--test-child c) :shadow))
                       (unwind-protect
                           (let* ((img (glitnir-nhmac--grab name))
                                  (b (glitnir-nhmac--box img #xff00ff))
                                  (mx (/ (+ (nth 0 b) (nth 2 b)) 2))
                                  (d (round (* 4 s))))
                             (list (glitnir-nhmac--luma (glitnir-nhmac--px img mx (+ (nth 3 b) d)))
                                   (glitnir-nhmac--luma (glitnir-nhmac--px img mx (- (nth 1 b) d)))))
                         (delete-frame c t)))))
            (on (funcall probe "shadow-on" nil))
            (off (funcall probe "shadow-off" '((shadow . nil)))))
       (nhmac-gui--say "glitnir-nhmac/shadow-darkens-below: on (below above) %S, off %S" on off)
       (should (< (nth 0 on) (- 255 8)))
       (should (< (nth 0 on) (nth 1 on)))
       (should (>= (nth 0 off) (- 255 2)))))))

(ert-deftest glitnir-nhmac/border-follows-the-curve ()
  "The border is drawn along the rounded corner (`borderWidth' on the masked content): the box's corner pixel is the
root's white, and a pixel just inside the curve at 45° is not the child's colour."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--with-white
   (lambda ()
     (let ((c (glitnir-nhmac--chromed '((shadow . nil) (internal-border-width . 3)))))
       (set-face-background 'child-frame-border "#000000" c)
       (unwind-protect
           (let* ((s (glitnir-nhmac--scale))
                  (img (glitnir-nhmac--grab "border"))
                  ;; the child's box: its border is black, so find the box from the magenta inside and grow it
                  (in (glitnir-nhmac--box img #xff00ff))
                  (bw (round (* 3 s)))
                  (b (list (- (nth 0 in) bw) (- (nth 1 in) bw) (+ (nth 2 in) bw) (+ (nth 3 in) bw)))
                  (r (* 12 s))
                  ;; on the curve at 45°, a pixel inward
                  (k (round (- r (/ r (sqrt 2.0)) -1)))
                  (on-curve (glitnir-nhmac--px img (+ (nth 0 b) k) (+ (nth 1 b) k))))
             (nhmac-gui--say "glitnir-nhmac/border-follows-the-curve: box %S, corner %06x, on the curve %06x" b
                             (glitnir-nhmac--px img (nth 0 b) (nth 1 b)) on-curve)
             (should-not (nhmac-gui--far (glitnir-nhmac--px img (nth 0 b) (nth 1 b)) #xffffff 24))
             (should (nhmac-gui--far on-curve #xff00ff 64)))
         (delete-frame c t))))))

(ert-deftest nhmac-resize-shadow-renders ()
  "A chromed child resized five times renders no shadow image (the nine-slice image stretches): `:shadow-renders' 0,
while its shadow layer's frame follows each size.  The child is first made large enough for the image to stretch at
every scale (a smaller one renders its own image, by design: the image's least side is in device pixels, so a child
of the same characters needs more of them at scale 2 while the font's pixel size does not follow the scale), and
counting starts there."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((c (glitnir-nhmac--chromed '((width . 40) (height . 12))))
        (cw 40) (ch 12))
    (unwind-protect
        (let (before (frames nil))
          (set-frame-size c cw ch)
          (nhmac-gui--settle)
          (while (and (not (plist-get (plist-get (nhmac--test-child c) :shadow) :stretches)) (< ch 48))
            (setq cw (+ cw 8) ch (+ ch 6))
            (set-frame-size c cw ch)
            (nhmac-gui--settle))
          (nhmac-gui--say "nhmac-resize-shadow-renders: from %dx%d: %S" cw ch (plist-get (nhmac--test-child c) :shadow))
          (should (plist-get (plist-get (nhmac--test-child c) :shadow) :stretches))
          (setq before (nhmac-stats))
          (dolist (w (list (+ cw 2) (+ cw 6) (+ cw 10) (+ cw 14) (+ cw 18)))
            (set-frame-size c w ch)
            (nhmac-gui--settle)
            (push (nth 2 (plist-get (plist-get (nhmac--test-child c) :shadow) :frame)) frames))
          (nhmac-gui--say "nhmac-resize-shadow-renders: renders %d, shadow widths %S"
                          (nhmac-gui--delta before :shadow-renders) (reverse frames))
          (should (= (nhmac-gui--delta before :shadow-renders) 0))
          (should (equal frames (sort (copy-sequence frames) #'>)))
          (should (= (length (delete-dups (copy-sequence frames))) 5)))
      (delete-frame c t))))

;;; Motion: the container's model values are the end state at once, and Core Animation runs the leg (its animations'
;;; keys and the leg handed over read back through `nhmac--test-child').  The legs run 0.6 s, long enough to read them
;;; while they run on the runner; the system's reduced-motion setting is stood in for, so the runner's own (it asks
;;; for reduced motion) does not turn them off.

(defconst glitnir-nhmac--pop '(:show pop :duration 0.6 :curve ease-out) "A pop in over 0.6 s; no hide leg.")
(defconst glitnir-nhmac--fade-out '(:hide fade :hide-duration 0.6 :hide-curve linear)
  "A fade out over 0.6 s; no show leg.")

(defmacro glitnir-nhmac--motion (reduced &rest body)
  "Run BODY with the shell reading reduced motion as REDUCED (1 or 0), then the system's again."
  (declare (indent 1))
  `(progn (nhmac--test-reduced-motion ,reduced)
          (unwind-protect (progn ,@body) (nhmac--test-reduced-motion -1))))

(defun glitnir-nhmac--hidden (params)
  "A chromed child with PARAMS, made invisible and settled."
  (let ((c (glitnir-nhmac--chromed params)))
    (make-frame-invisible c t)
    (nhmac-gui--settle)
    c))

(ert-deftest nhmac-show-from-hidden-starts-at-leg ()
  "A pop shown from hidden: the container is shown at once (the model) and its leg starts at its own start, opacity
0 and the pop's small scale, for the whole duration, as Core Animation's opacity and transform animations."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--motion 0
    (let ((c (glitnir-nhmac--hidden `((transition . ,glitnir-nhmac--pop)))))
      (unwind-protect
          (let ((before (nhmac-stats)))
            (should (plist-get (nhmac--test-child c) :hidden))
            (make-frame-visible c)
            (nhmac-gui--settle)
            (let* ((i (nhmac--test-child c)) (leg (plist-get i :leg)))
              (nhmac-gui--say "nhmac-show-from-hidden-starts-at-leg: %S" i)
              (should-not (plist-get i :hidden))
              (should (= (nhmac-gui--delta before :motion-legs) 1))
              (should (eq (plist-get leg :show) t))
              (should (equal (mapcar (lambda (v) (/ (round (* v 100)) 100.0)) (plist-get leg :from)) '(0.0 0.96)))
              (should (equal (plist-get leg :to) '(1 1)))
              (should (< (abs (- (plist-get leg :seconds) 0.6)) 0.001))
              (should (member "opacity" (plist-get i :animations)))
              (should (member "transform" (plist-get i :animations)))))
        (delete-frame c t)))))

(ert-deftest nhmac-hide-leg-then-hidden ()
  "A fade out: the container is hidden in the model at once (the hit-test ignores it), an animation holds it shown
while its opacity runs to 0, and once the leg is over no animation is left."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--motion 0
    (let ((c (glitnir-nhmac--chromed `((transition . ,glitnir-nhmac--fade-out)))))
      (unwind-protect
          (progn
            (make-frame-invisible c t)
            (nhmac-gui--settle)
            (let* ((i (nhmac--test-child c)) (leg (plist-get i :leg)))
              (nhmac-gui--say "nhmac-hide-leg-then-hidden: %S" i)
              (should (plist-get i :hidden))
              (should-not (plist-get leg :show))
              (should (equal (plist-get leg :to) '(0 1)))
              (should (member "hidden" (plist-get i :animations)))
              (should (member "opacity" (plist-get i :animations))))
            (should (nhmac-gui--idle-until (lambda () (null (plist-get (nhmac--test-child c) :animations))) 3)))
        (delete-frame c t)))))

(ert-deftest nhmac-motion-no-redisplay ()
  "A leg running costs Emacs nothing: while a 0.6 s pop runs, idling makes no commit and no draw (Core Animation runs
it), and no animation outlives it."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--motion 0
    (let ((c (glitnir-nhmac--hidden `((transition . ,glitnir-nhmac--pop)))))
      (unwind-protect
          (progn
            (make-frame-visible c)
            (redisplay t)
            (let ((before (nhmac-stats)))
              ;; idle through the leg, the wait turning the run loop as Emacs's does
              (nhmac-gui--idle-until #'ignore 0.5)
              (nhmac-gui--say "nhmac-motion-no-redisplay: commits %d draws %d animations %S"
                              (nhmac-gui--delta before :commits) (nhmac-gui--delta before :draws)
                              (plist-get (nhmac--test-child c) :animations))
              (should (= (nhmac-gui--delta before :commits) 0))
              (should (= (nhmac-gui--delta before :draws) 0)))
            (should (nhmac-gui--idle-until (lambda () (null (plist-get (nhmac--test-child c) :animations))) 3)))
        (delete-frame c t)))))

(ert-deftest nhmac-reduced-motion ()
  "When the system asks for reduced motion, `nhmac-reduced-motion-p' says so and a show or a hide runs no leg: the
container takes its state with no animation."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--motion 1
    (should (nhmac-reduced-motion-p))
    (let ((c (glitnir-nhmac--hidden `((transition . (:show pop :duration 0.6 :hide fade :hide-duration 0.6))))))
      (unwind-protect
          (let ((before (nhmac-stats)))
            (make-frame-visible c)
            (nhmac-gui--settle)
            (should-not (plist-get (nhmac--test-child c) :hidden))
            (should-not (plist-get (nhmac--test-child c) :animations))
            (make-frame-invisible c t)
            (nhmac-gui--settle)
            (should (plist-get (nhmac--test-child c) :hidden))
            (should-not (plist-get (nhmac--test-child c) :animations))
            (should (= (nhmac-gui--delta before :motion-legs) 0)))
        (delete-frame c t))))
  (glitnir-nhmac--motion 0
    (should-not (nhmac-reduced-motion-p))))

(ert-deftest glitnir-nhmac/panel-holds-the-shadow ()
  "A chromed child that overhangs its root lives in a panel grown by its shadow's reach, so the shadow is not cut:
the panel is larger than the child on every side, the container sits inside it by the reach (more below than to the
left, as the shadow's offset is down), and the shadow layer's frame fits in the panel."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((rw (frame-pixel-width (selected-frame)))
         (c (glitnir-nhmac--chromed nil 40 80)))
    (unwind-protect
        (progn
          (set-frame-position c (- rw (/ (frame-pixel-width c) 2)) 80)
          (nhmac-gui--settle)
          (let* ((i (nhmac--test-child c)) (p (plist-get i :panel))
                 (fr (plist-get (plist-get i :shadow) :frame)))
            (nhmac-gui--say "glitnir-nhmac/panel-holds-the-shadow: %S" i)
            (should (eq (plist-get i :host) 'panel))
            (should p)
            (should (> (plist-get i :x) 0))
            (should (> (plist-get i :y) (plist-get i :x)))
            (should (> (nth 2 p) (+ (plist-get i :w) (* 2 (plist-get i :x)) -1)))
            (should (> (nth 3 p) (+ (plist-get i :h) (plist-get i :y))))
            ;; the shadow's frame (from the container's bottom left) inside the panel
            (should (>= (+ (plist-get i :x) (nth 0 fr)) 0))
            (should (>= (+ (plist-get i :y) (nth 1 fr)) 0))
            (should (<= (+ (plist-get i :x) (nth 0 fr) (nth 2 fr)) (+ (nth 2 p) 0.5)))
            (should (<= (+ (plist-get i :y) (nth 1 fr) (nth 3 fr)) (+ (nth 3 p) 0.5)))))
      (delete-frame c t))))

;;; Blur: a translucent child with a backdrop blur lives in its panel, over an NSVisualEffectView, while the blur is on.

(ert-deftest glitnir-nhmac/blur-in-a-panel ()
  "A translucent child with `backdrop-blur' lives in a panel over a blur though it fits in its root, and goes back
into the root's layer once the blur is off."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((c (glitnir-nhmac--chromed '((alpha-background . 0.6) (backdrop-blur . 12)))))
    (unwind-protect
        (let ((i (nhmac--test-child c)))
          (nhmac-gui--say "glitnir-nhmac/blur-in-a-panel: on %S" i)
          (should (eq (plist-get i :host) 'panel))
          (should (plist-get i :panel))
          (should (eq (plist-get i :blur) t))
          (set-frame-parameter c 'backdrop-blur nil)
          (glitnir-refresh c)
          (nhmac-gui--settle)
          (setq i (nhmac--test-child c))
          (nhmac-gui--say "glitnir-nhmac/blur-in-a-panel: off %S" i)
          (should (eq (plist-get i :host) 'parent))
          (should-not (plist-get i :panel))
          (should-not (plist-get i :blur)))
      (delete-frame c t))))

(ert-deftest nhmac-blur-pair ()
  "Typing under a blurred child, timed in the process: 50 typed keys, each a command and a redisplay, written to
GLITNIR_GUI_OUT/pair-BUILD.txt (microseconds, one per line), and the same with the blur off, logged.  Both builds run
it in turn (--pair, ABAB); nidhogg/test/nhmac-pair.sh --row nhmac-blur-pair compares the pairs: not slower than NS's
blurred child."
  :tags '(draws)
  (let* ((build (if (nhmac-gui--ns-build-p) "ns" "naglfar"))
         (file (expand-file-name (format "pair-%s.txt" build)
                                 (or (getenv "GLITNIR_GUI_OUT") temporary-file-directory)))
         (c (nhmac-gui--child '((alpha-background . 0.6) (backdrop-blur . 12) (corner-radius . 12)
                                (width . 30) (height . 8))
                              80 120))
         (time-keys (lambda ()
                      (let (times)
                        (dotimes (_ 50)
                          (let ((t0 (float-time)))
                            (execute-kbd-macro "x")
                            (redisplay)
                            (push (round (* 1e6 (- (float-time) t0))) times)))
                        (nreverse times))))
         (median (lambda (l) (nth (/ (length l) 2) (sort (copy-sequence l) #'<))))
         on off)
    (unwind-protect
        (progn
          (switch-to-buffer (generate-new-buffer " *nhmac-blur-pair*"))
          (insert "typing under a blurred child")
          (redisplay t)
          (dotimes (_ 10) (execute-kbd-macro "w") (redisplay))
          (setq on (funcall time-keys))
          (set-frame-parameter c 'backdrop-blur nil)
          (glitnir-refresh c)
          (redisplay t)
          (setq off (funcall time-keys))
          (nhmac-gui--say "nhmac-blur-pair: %s median %d us blurred, %d us without" build
                          (funcall median on) (funcall median off))
          (with-temp-buffer
            (dolist (us on) (insert (format "%d\n" us)))
            (write-region (point-min) (point-max) file t 'silent))
          (should (file-exists-p file)))
      (kill-buffer)
      (delete-frame c t))))

(defun glitnir-nhmac--region (img box pad)
  "IMG's pixels in BOX (X0 Y0 X1 Y1) grown by PAD on every side and clipped to IMG, row by row, as a list."
  (pcase-let ((`(,w ,h ,_) img) (`(,x0 ,y0 ,x1 ,y1) box) (out nil))
    (cl-loop for y from (max 0 (- y0 pad)) to (min (1- h) (+ y1 pad))
             do (cl-loop for x from (max 0 (- x0 pad)) to (min (1- w) (+ x1 pad))
                         do (push (glitnir-nhmac--px img x y) out)))
    (nreverse out)))

(ert-deftest nhmac-glitnir-off ()
  "With `glitnir-enable' nil a child asking for corners, a shadow and a blur gets none: no corner radius, no shadow
layer, no panel and no blur; and the screen around it is the plain child's, pixel for pixel (tolerance 0).  Both
children are opaque, so only the chrome could tell them apart."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--with-white
   (lambda ()
     (let (plain off box)
       (let ((c (nhmac-gui--child '((width . 30) (height . 8)) 80 200)))
         (unwind-protect
             (setq plain (glitnir-nhmac--grab "off-plain")
                   box (glitnir-nhmac--box plain #xff00ff))
           (delete-frame c t)))
       (let* ((glitnir-enable nil)
              (c (glitnir-nhmac--chromed '((backdrop-blur . 12)))))
         (unwind-protect
             (let ((i (nhmac--test-child c)))
               (nhmac-gui--say "nhmac-glitnir-off: %S" i)
               (should (= (plist-get i :radius) 0))
               (should-not (plist-get i :shadow))
               (should (eq (plist-get i :host) 'parent))
               (should-not (plist-get i :panel))
               (should-not (plist-get i :blur))
               (setq off (glitnir-nhmac--grab "off-chromed")))
           (delete-frame c t)))
       (should box)
       (let ((a (glitnir-nhmac--region plain box 40)) (b (glitnir-nhmac--region off box 40)))
         (nhmac-gui--say "nhmac-glitnir-off: box %S, %d of %d pixels differ" box
                         (cl-count nil (cl-mapcar #'eql a b)) (length a))
         (should (equal a b)))))))

(ert-deftest glitnir-nhmac/text-identical-to-off ()
  "Text in a rounded, shadowed child is the text Glitnir off draws: inside the child, away from its corners, the
screen is the same pixel for pixel (the corner mask and the shadow touch nothing else).  As in the ledger's L3, an
edge pixel of a glyph (one whose four neighbours are not all its colour) is left out: a glyph's overhang drawn again
blends it twice, and how often that happens follows the redisplays, not the chrome (at 1x the frame with Glitnir off
showed 5 such pixels darker, on descenders' lower edge)."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--with-white
   (lambda ()
     (let ((text (lambda (c)
                   (with-current-buffer (window-buffer (frame-root-window c))
                     (erase-buffer)
                     (insert "The quick brown fox\njumps over the lazy dog\n0123456789 ()[]{}"))
                   (nhmac-gui--settle)))
           on off box)
       ;; the children share their buffer: it is left empty for the rows after
       (unwind-protect
           (progn
             (let ((c (glitnir-nhmac--chromed)))
               (unwind-protect
                   (progn (funcall text c)
                          (setq on (glitnir-nhmac--grab "text-on")
                                box (glitnir-nhmac--box on #xff00ff)))
                 (delete-frame c t)))
             (let* ((glitnir-enable nil) (c (glitnir-nhmac--chromed)))
               (unwind-protect
                   (progn (funcall text c)
                          (setq off (glitnir-nhmac--grab "text-off")))
                 (delete-frame c t))))
         (with-current-buffer (get-buffer-create " *nhmac-child*") (erase-buffer)))
       (should box)
       ;; the box less the corners' reach (the radius, in device pixels, and one more)
       (let* ((r (1+ (ceiling (* 12 (glitnir-nhmac--scale)))))
              (inner (list (+ (nth 0 box) r) (+ (nth 1 box) r) (- (nth 2 box) r) (- (nth 3 box) r)))
              (a (glitnir-nhmac--region on inner 0)) (b (glitnir-nhmac--region off inner 0)))
         (nhmac-gui--say "glitnir-nhmac/text-identical-to-off: inner %S, %d of %d pixels differ%s" inner
                         (cl-count nil (cl-mapcar #'eql a b)) (length a)
                         ;; the first few, as (x y on off), to tell an edge from the text
                         (let ((w (1+ (- (nth 2 inner) (nth 0 inner)))) (i 0) (out nil))
                           (cl-mapc (lambda (p q)
                                      (unless (or (eql p q) (>= (length out) 8))
                                        (push (list (+ (nth 0 inner) (% i w)) (+ (nth 1 inner) (/ i w))
                                                    (format "%06x" p) (format "%06x" q))
                                              out))
                                      (cl-incf i))
                                    a b)
                           (if out (format ": %S" (nreverse out)) "")))
         (should (> (length a) 0))
         ;; the flat pixels (all four neighbours the same colour in the picture with chrome) are the same
         (let* ((w (1+ (- (nth 2 inner) (nth 0 inner)))) (va (vconcat a)) (vb (vconcat b)) (n (length va))
                (flat (lambda (i)
                        (let ((p (aref va i)))
                          (and (>= i w) (< (+ i w) n) (> (% i w) 0) (< (% i w) (1- w))
                               (eql (aref va (1- i)) p) (eql (aref va (1+ i)) p)
                               (eql (aref va (- i w)) p) (eql (aref va (+ i w)) p)))))
                (diff 0) (count 0))
           (dotimes (i n)
             (when (funcall flat i)
               (cl-incf count)
               (unless (eql (aref va i) (aref vb i)) (cl-incf diff))))
           (nhmac-gui--say "glitnir-nhmac/text-identical-to-off: %d of %d flat pixels differ" diff count)
           (should (> count 0))
           (should (= diff 0))))))))

;;; Window cards: each card is an image the glue drew alone, on a layer of its own over the root's content.  Two
;;; windows side by side, a 6-pixel divider between them (the gap, whose colour is what a card's corners show).

(defconst glitnir-nhmac--card-bg #xf0d8a0
  "The card window's background: a colour of its own (a light grey is in the title bar's and the echo area's
anti-aliased text, which a search for the window's box then took in).")
(defconst glitnir-nhmac--other-bg #xc07030 "The other window's background.")
(defconst glitnir-nhmac--gap #x20a040 "The divider colour: the gap, and what a card's corners show.")
(defconst glitnir-nhmac--card '((corner-radius . 10)) "A card with corners only.")
(defconst glitnir-nhmac--card-look
  '((corner-radius . 10) (shadow . (:blur 3 :offset (0 . 1) :opacity 0.45)) (ring . ("#20a040" . 0.6)))
  "The look's card: corners, a soft shadow one point down and the edge ring.")

(defun glitnir-nhmac--hex (rgb) "RGB as \"#rrggbb\"." (format "#%06x" rgb))

(defun glitnir-nhmac--clean-root ()
  "Assert, as a row starts, that no earlier row left its state on the root: the root has no cards, its surface is
not held, and the screen shows neither card row background (colours only the card rows use)."
  (let* ((row (ert-test-name (ert-running-test)))
         (cards (nhmac--test-cards)) (surface (nhmac--test-surface 0 0))
         (img (glitnir-nhmac--grab (format "clean-%s" (replace-regexp-in-string "/" "-" (symbol-name row)))))
         (left (glitnir-nhmac--box img glitnir-nhmac--card-bg))
         (right (glitnir-nhmac--box img glitnir-nhmac--other-bg)))
    (nhmac-gui--say "glitnir-nhmac/clean-root: %s: cards %S, surface %S, card colours on the screen %S %S"
                    row cards surface left right)
    (when (or left right)
      ;; the screen shows a card row's picture: is it in the root's newest surface too (then Emacs did not redraw),
      ;; or only on the screen (then the layer shows something else)?  Every 16th row of the surface, counted; then
      ;; again after a full redraw (if that clears it, Emacs's matrices thought the surface already showed them).
      (let ((count (lambda ()
                     (let* ((surface (nhmac--test-surface 0 0))
                            (w (plist-get surface :w)) (h (plist-get surface :h)) (n 0) (o 0) (seen 0))
                       (cl-loop for y from 0 below h by 16
                                do (cl-loop for p across (nhmac--test-surface-region 0 y (min w 1400) 1)
                                            do (cl-incf seen)
                                            (unless (nhmac-gui--far (logand p #xffffff) glitnir-nhmac--card-bg 8)
                                              (cl-incf n))
                                            (unless (nhmac-gui--far (logand p #xffffff) glitnir-nhmac--other-bg 8)
                                              (cl-incf o))))
                       (format "%d (and %d of the other window's colour) of %d" n o seen))))
            (before (nhmac-stats)))
        (nhmac-gui--say "glitnir-nhmac/clean-root: %s: windows %S; own layers %S (nhwl-scroll-layers %S), glitnir--cards %S"
                        row (mapcar (lambda (w) (list (buffer-name (window-buffer w)) (window-pixel-edges w)))
                                    (window-list nil 'never))
                        (nhwl--scroll-layers) nhwl-scroll-layers (frame-parameter nil 'glitnir--cards))
        (nhmac-gui--say "glitnir-nhmac/clean-root: %s: %s sampled surface pixels are the card background"
                        row (funcall count))
        ;; may the root show a buffer (visible, no size change pending)?  The newest size changes, newest first.
        (nhmac-gui--say "glitnir-nhmac/clean-root: %s: visible %S, size %S, size history %S"
                        row (frame-visible-p (selected-frame))
                        (list (frame-pixel-width) (frame-pixel-height) (frame-native-width) (frame-native-height))
                        (seq-take (cdr-safe frame-size-history) 12))
        (redraw-frame)
        (nhmac-gui--settle)
        (nhmac-gui--say "glitnir-nhmac/clean-root: %s: after redraw-frame (%d draws, %d commits): %s" row
                        (nhmac-gui--delta before :draws) (nhmac-gui--delta before :commits) (funcall count))
        ;; a new backing: one pixel wider, and back
        (let ((pw (frame-text-width)) (ph (frame-text-height)))
          (set-frame-size nil (1+ pw) ph t)
          (nhmac-gui--settle)
          (set-frame-size nil pw ph t)
          (nhmac-gui--settle))
        (nhmac-gui--say "glitnir-nhmac/clean-root: %s: after a resize there and back: %s" row (funcall count))))
    (should (equal (list row (plist-get cards :cards) (plist-get surface :held) left right)
                   (list row nil nil nil nil)))))

(defun glitnir-nhmac--with-cards (params fn)
  "Call FN with the left and right windows of the cards' layout, the left with window PARAMS, drawn.
It starts with `glitnir-nhmac--clean-root'.  Everything goes back after: the window layout, the frame's divider
width and colour."
  (let* ((f (selected-frame))
         (conf (current-window-configuration f))
         (gap (frame-parameter f 'right-divider-width))
         (divider (face-attribute 'window-divider :foreground f))
         (a (get-buffer-create " *glitnir-nhmac-card-a*"))
         (b (get-buffer-create " *glitnir-nhmac-card-b*"))
         left right)
    (unwind-protect
        (progn
          (glitnir-nhmac--clean-root)
          ;; the frames' size changes from here on, for a later row's dirty root
          (unless (consp frame-size-history) (setq frame-size-history (list 200)))
          (delete-other-windows)
          (set-face-foreground 'window-divider (glitnir-nhmac--hex glitnir-nhmac--gap) f)
          (set-frame-parameter f 'right-divider-width 6)
          (dolist (x (list (cons a glitnir-nhmac--card-bg) (cons b glitnir-nhmac--other-bg)))
            (with-current-buffer (car x)
              (setq mode-line-format nil cursor-type nil)
              (face-remap-add-relative 'default :background (glitnir-nhmac--hex (cdr x)))))
          (setq left (selected-window) right (split-window-right))
          (set-window-buffer left a)
          (set-window-buffer right b)
          (dolist (w (list left right))
            (set-window-fringes w 0 0)
            (set-window-scroll-bars w nil nil nil nil))
          (dolist (p params) (set-window-parameter left (car p) (cdr p)))
          (message nil)
          (force-window-update)
          (nhmac-gui--settle)
          (funcall fn left right))
      (nhmac-gui--say "glitnir-nhmac/with-cards: %s ends with %S own layers"
                      (ert-test-name (ert-running-test)) (nhwl--scroll-layers))
      (when (window-live-p left)
        (dolist (k '(corner-radius shadow ring)) (set-window-parameter left k nil)))
      (set-window-configuration conf)
      (set-frame-parameter f 'right-divider-width gap)
      (set-face-attribute 'window-divider f :foreground divider)
      (kill-buffer a)
      (kill-buffer b)
      (force-window-update)
      (nhmac-gui--settle))))

(defun glitnir-nhmac--wbox (w)
  "Window W's edges without its dividers, (X0 Y0 X1 Y1) in device pixels of the root's surface (inclusive).
A Naglfar frame's pixels are device pixels already."
  (let* ((e (window-pixel-edges w)) (x (nth 0 e)) (y (nth 1 e)))
    (list x y
          (1- (+ x (- (window-pixel-width w) (window-right-divider-width w))))
          (1- (+ y (- (window-pixel-height w) (window-bottom-divider-width w)))))))

(defun glitnir-nhmac--surface-px (x y)
  "The root's newest surface's pixel at X, Y (device pixels) as 0xRRGGBB."
  (logand (aref (nhmac--test-surface-region x y 1 1) 0) #xffffff))

(defun glitnir-nhmac--cards (&optional settle)
  "The root's cards (`nhmac--test-cards'), after a redisplay and, with SETTLE, the commits it makes."
  (force-window-update)
  (redisplay t)
  (when settle (nhmac-gui--settle))
  (nhmac--test-cards))

(ert-deftest glitnir-nhmac/card-layer-follows-the-window ()
  "A card window gets one card layer at its place, grown by its chrome; the root's own surface has no card pixels
(its corner is the window's own colour there); the card's layer goes with the card."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--with-cards
   glitnir-nhmac--card-look
   (lambda (left _right)
     (let* ((c (glitnir-nhmac--cards t)) (b (glitnir-nhmac--wbox left)) (cards (plist-get c :cards)))
       (nhmac-gui--say "glitnir-nhmac/card-layer-follows-the-window: %S, window %S" c b)
       (should (= (length cards) 1))
       (pcase-let ((`(,_ ,x ,y ,w ,h) (car cards)))
         (should (<= x (nth 0 b)))
         (should (<= y (nth 1 b)))
         (should (>= (+ x w -1) (nth 2 b)))
         (should (>= (+ y h -1) (nth 3 b))))
       ;; the host composes no card into the root's surface: the corner pixel is the window's own
       (should-not (nhmac-gui--far (glitnir-nhmac--surface-px (nth 0 b) (nth 1 b)) glitnir-nhmac--card-bg 8))
       (dolist (k '(corner-radius shadow ring)) (set-window-parameter left k nil))
       (should (null (plist-get (glitnir-nhmac--cards t) :cards)))))))

(ert-deftest glitnir-nhmac/card-corner-shows-gap ()
  "A card window's four outermost corner pixels show the gap's colour on the screen; the top edge past the corner is
the window's; without the card the corner is the window's."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--with-cards
   glitnir-nhmac--card
   (lambda (left _right)
     (glitnir-nhmac--cards t)
     (let* ((img (glitnir-nhmac--grab "card-corner"))
            (b (glitnir-nhmac--box img glitnir-nhmac--card-bg))
            (r (round (* 10 (frame-scale-factor)))))
       (nhmac-gui--say "glitnir-nhmac/card-corner-shows-gap: box %S, corners %S" b
                       (and b (mapcar (lambda (c) (format "%06x" (glitnir-nhmac--px img (car c) (cdr c))))
                                      (list (cons (nth 0 b) (nth 1 b)) (cons (nth 2 b) (nth 1 b))))))
       (should b)
       ;; the card's bounding box is the window's: its corner pixels are outside the rounded shape, the gap's
       (dolist (c (list (cons (nth 0 b) (nth 1 b)) (cons (nth 2 b) (nth 1 b))
                        (cons (nth 0 b) (nth 3 b)) (cons (nth 2 b) (nth 3 b))))
         (should (equal (list c (nhmac-gui--far (glitnir-nhmac--px img (car c) (cdr c)) glitnir-nhmac--gap 8))
                        (list c nil))))
       (should-not (nhmac-gui--far (glitnir-nhmac--px img (+ (nth 0 b) r) (nth 1 b)) glitnir-nhmac--card-bg 8))
       (set-window-parameter left 'corner-radius nil)
       (glitnir-nhmac--cards t)
       (let ((img2 (glitnir-nhmac--grab "card-corner-off")))
         (should-not (nhmac-gui--far (glitnir-nhmac--px img2 (nth 0 b) (nth 1 b)) glitnir-nhmac--card-bg 8)))))))

(ert-deftest glitnir-nhmac/card-shadow-only-in-gap ()
  "The look's card leaves the other window's pixels as they are without it; the gap beside the card is darker."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--with-cards
   glitnir-nhmac--card-look
   (lambda (left right)
     (glitnir-nhmac--cards t)
     (let* ((with (glitnir-nhmac--grab "card-shadow-on"))
            (o (glitnir-nhmac--box with glitnir-nhmac--other-bg))
            (l (glitnir-nhmac--box with glitnir-nhmac--card-bg)))
       (should (and o l))
       (dolist (k '(corner-radius shadow ring)) (set-window-parameter left k nil))
       (glitnir-nhmac--cards t)
       (let* ((without (glitnir-nhmac--grab "card-shadow-off"))
              (inner (list (+ (nth 0 o) 2) (+ (nth 1 o) 2) (- (nth 2 o) 2) (- (nth 3 o) 2)))
              (a (glitnir-nhmac--region with inner 0)) (b (glitnir-nhmac--region without inner 0))
              (gx (/ (+ (nth 2 l) (nth 0 o)) 2)) (gy (/ (+ (nth 1 l) (nth 3 l)) 2)))
         (nhmac-gui--say "glitnir-nhmac/card-shadow-only-in-gap: other %S, %d of %d differ; gap %06x with, %06x without"
                         o (cl-count nil (cl-mapcar #'eql a b)) (length a)
                         (glitnir-nhmac--px with gx gy) (glitnir-nhmac--px without gx gy))
         (should (equal a b))
         (should (< (glitnir-nhmac--luma (glitnir-nhmac--px with gx gy))
                    (glitnir-nhmac--luma (glitnir-nhmac--px without gx gy)))))
       (ignore right)))))

(ert-deftest nhmac-cards-typing-no-change ()
  "A card makes its image once: 50 characters typed in the card window change no card image and no card layer."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--with-cards
   glitnir-nhmac--card-look
   (lambda (left _right)
     (let* ((c0 (glitnir-nhmac--cards t)) (buf (window-buffer left)))
       (should (> (plist-get c0 :images) 0))
       (with-current-buffer buf (dotimes (_ 12) (insert "\n")) (forward-line -6) (insert "  "))
       (set-window-point left (with-current-buffer buf (point)))
       (let ((c1 (glitnir-nhmac--cards t)))
         (dotimes (i 50)
           (with-current-buffer buf (insert (if (= (% (1+ i) 20) 0) "\n" "x")))
           (redisplay t))
         (let ((c2 (glitnir-nhmac--cards t)))
           (nhmac-gui--say "nhmac-cards-typing-no-change: %S -> %S" c1 c2)
           (should (equal c1 c2))))))))

(ert-deftest glitnir-nhmac/card-removed-no-residue ()
  "A card that goes leaves the screen as it was before it came, and the root has no card layers."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--with-cards
   nil
   (lambda (left _right)
     (glitnir-nhmac--cards t)
     (let ((base (glitnir-nhmac--grab "card-base")))
       (dolist (p glitnir-nhmac--card-look) (set-window-parameter left (car p) (cdr p)))
       (glitnir-nhmac--cards t)
       (should-not (equal base (glitnir-nhmac--grab "card-on")))
       (dolist (p glitnir-nhmac--card-look) (set-window-parameter left (car p) nil))
       (should (null (plist-get (glitnir-nhmac--cards t) :cards)))
       (should (equal base (glitnir-nhmac--grab "card-gone")))))))

(deftheme glitnir-nhmac-test
  "Colours the shadow and the dividers (`nhmac-cards-theme-switch', `nhmac-colour-push-not-poll').")
(custom-theme-set-faces 'glitnir-nhmac-test
                        '(child-frame-shadow ((t :foreground "#ff0000")))
                        '(window-divider ((t :foreground "#4060e0"))))

(ert-deftest nhmac-cards-theme-switch ()
  "A theme that gives the dividers a new colour gives the card new images at once: its corners show the new gap
colour."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (glitnir-nhmac--with-cards
   glitnir-nhmac--card
   (lambda (_left _right)
     (let ((n (plist-get (glitnir-nhmac--cards t) :images)) (new #x4060e0)
           (face (face-foreground 'window-divider (selected-frame))))
       ;; enabling a theme recomputes the face on every frame from the themes (the row's own colour goes)
       (enable-theme 'glitnir-nhmac-test)
       (unwind-protect
           (let* ((c (glitnir-nhmac--cards t))
                  (img (glitnir-nhmac--grab "card-theme"))
                  (b (glitnir-nhmac--box img glitnir-nhmac--card-bg)))
             (nhmac-gui--say "nhmac-cards-theme-switch: images %d -> %d, corner %06x" n (plist-get c :images)
                             (if b (glitnir-nhmac--px img (nth 0 b) (nth 1 b)) 0))
             (should (> (plist-get c :images) n))
             (should b)
             (should-not (nhmac-gui--far (glitnir-nhmac--px img (nth 0 b) (nth 1 b)) new 8)))
         (disable-theme 'glitnir-nhmac-test)
         (set-face-foreground 'window-divider face (selected-frame)))))))

(ert-deftest nhmac-colour-push-not-poll ()
  "Redisplay reads no chrome colour; a theme switch pushes the new shadow colour to a shown child."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((c (glitnir-nhmac--chromed)) (reads 0))
    (unwind-protect
        (let ((count (lambda (&rest _) (setq reads (1+ reads)))))
          (nhmac-gui--settle)
          (should (equal (plist-get (plist-get (glitnir--chrome c) :shadow) :color) '(0 0 0)))
          (advice-add 'glitnir--shadow-color :before count)
          (unwind-protect
              (dotimes (_ 5)
                (force-window-update)
                (redisplay t))
            (advice-remove 'glitnir--shadow-color count))
          (nhmac-gui--say "nhmac-colour-push-not-poll: %d colour reads in 5 redisplays" reads)
          (should (= reads 0))
          (enable-theme 'glitnir-nhmac-test)
          (nhmac-gui--settle)
          (should (equal (plist-get (plist-get (glitnir--chrome c) :shadow) :color) '(255 0 0))))
      (disable-theme 'glitnir-nhmac-test)
      (delete-frame c t))))

(ert-deftest glitnir-nhmac/active-p-follows-enable ()
  "`glitnir-active-p' is t for a Naglfar frame while `glitnir-enable' is on, nil while it is off."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (should (glitnir-active-p))
  (let ((glitnir-enable nil))
    (should-not (glitnir-active-p)))
  (should (glitnir-active-p)))

(ert-deftest glitnir-nhmac/stats-count-the-commits ()
  "`glitnir-stats' on Naglfar holds one entry per drawing the frame's layer shows (each a commit), with its draw time."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer "stats")
  (let ((blink blink-cursor-mode))
    (blink-cursor-mode -1)
    (unwind-protect
        (let (before)
          (nhmac-gui--redisplay)
          (glitnir-stats nil t)
          (setq before (nhmac-stats))
          (dotimes (_ 5)
            (insert "x")
            (nhmac-gui--redisplay))
          (let ((commits (nhmac-gui--delta before :commits))
                (s (glitnir-stats)))
            (nhmac-gui--say "glitnir-nhmac/stats-count-the-commits: commits %d stats %S" commits s)
            (should (>= commits 5))
            (should (<= 5 (plist-get s :count) commits))
            (should (> (plist-get (plist-get s :draw-ms) :max) 0))))
      (blink-cursor-mode (if blink 1 -1))
      (kill-buffer (current-buffer)))))

(ert-deftest glitnir-nhmac/toplevel-chrome-is-the-systems ()
  "A decorated root keeps AppKit's shadow and corners: Glitnir puts no layer on it, also when it asks for
`toplevel-chrome' (which Glitnir draws only for an undecorated frame on Wayland)."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((f (make-frame '((width . 40) (height . 10)
                         (toplevel-chrome . ((corner-radius . 12) (shadow . t))))))
        (panics (length (nhwl-glue-panics))))
    (unwind-protect
        (progn
          (select-frame f)
          (nhmac-gui--settle)
          (let ((info (nhmac--window-info f)))
            (nhmac-gui--say "glitnir-nhmac/toplevel-chrome-is-the-systems: %S" info)
            (should (eq (plist-get info :has-shadow) t))
            (should (eql (plist-get info :sublayers) 0)))
          (should-not (glitnir--chrome-vector f))
          (should (= (length (nhwl-glue-panics)) panics)))
      (delete-frame f t))))
