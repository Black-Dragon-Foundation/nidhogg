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

(defun glitnir-nhmac--box (img rgb &optional near)
  "The bounding box (X0 Y0 X1 Y1, inclusive) of the pixels of IMG within NEAR (default 8) of RGB, or nil."
  (pcase-let ((`(,w ,h ,px) img) (x0 nil) (y0 nil) (x1 nil) (y1 nil))
    (dotimes (y h)
      (dotimes (x w)
        (unless (nhmac-gui--far (aref px (+ (* y w) x)) rgb (or near 8))
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

(ert-deftest glitnir-nhmac/ring-outside-the-child ()
  "A child frame's `ring' is a line one point wide just outside its edge, as Glitnir's compositor draws it: the pixel
above the middle of the child's top edge is the ring's colour, and past the ring the root's white shows."
  :tags '(draws)
  (glitnir-nhmac--with-white
   (lambda ()
     (let ((c (glitnir-nhmac--chromed '((shadow . nil) (ring . ("#ff0000" . 1.0))))))
       (unwind-protect
           (let* ((s (glitnir-nhmac--scale))
                  (img (glitnir-nhmac--grab "ring"))
                  (b (glitnir-nhmac--box img #xff00ff))
                  (x (/ (+ (nth 0 b) (nth 2 b)) 2))
                  (rw (max 1 (round s)))
                  (on (glitnir-nhmac--px img x (- (nth 1 b) 1)))
                  (past (glitnir-nhmac--px img x (- (nth 1 b) rw 2))))
             (nhmac-gui--say "glitnir-nhmac/ring-outside-the-child: box %S, ring %06x, past it %06x" b on past)
             (should-not (nhmac-gui--far on #xff0000 24))
             (should-not (nhmac-gui--far past #xffffff 24)))
         (delete-frame c t))))))

(ert-deftest nhmac-resize-shadow-renders ()
  "A chromed child resized five times renders no shadow image (the nine-slice image stretches): `:shadow-renders' 0,
while its shadow layer's frame follows each size.  The child is first made large enough for the image to stretch at
every scale (a smaller one renders its own image, by design: the image's least side is in device pixels, so a child
of the same characters needs more of them at scale 2 while the font's pixel size does not follow the scale), and
counting starts there."
  :tags '(draws)
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
  (glitnir-nhmac--motion 0
    (let ((c (glitnir-nhmac--hidden `((transition . ,glitnir-nhmac--pop)))))
      (unwind-protect
          (progn
            (make-frame-visible c)
            (redisplay t)
            ;; the wait that commits what the redisplay published (the commit is off the redisplay path)
            (accept-process-output nil 0.001)
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

(ert-deftest nhmac-blur-typing ()
  "Typing under a blurred child, timed in the process: 50 typed keys, each a command, a redisplay and its commit.  The
bar is drgnx's typing budget: p95 under 16 ms, max under 50 ms.  The same keys with the blur off are logged beside it."
  :tags '(draws timing)
  (nhmac-gui--timing-build)
  (let* ((c (nhmac-gui--child '((alpha-background . 0.6) (backdrop-blur . 12) (corner-radius . 12)
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
          (switch-to-buffer (generate-new-buffer " *nhmac-blur*"))
          (insert "typing under a blurred child")
          (redisplay t)
          (dotimes (_ 10) (execute-kbd-macro "w") (redisplay))
          (setq on (funcall time-keys))
          (set-frame-parameter c 'backdrop-blur nil)
          (glitnir-refresh c)
          (redisplay t)
          (setq off (funcall time-keys))
          (nhmac-gui--say "nhmac-blur-typing: median %d us blurred, %d us without" (funcall median on) (funcall median off))
          (nhmac-gui--within-budget "nhmac-blur-typing" on 16 50))
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
         (right (glitnir-nhmac--box img glitnir-nhmac--other-bg))
         ;; the root's content in the capture: a box wholly outside it is not the root's state (round 11: a 10 by 12
         ;; point box near the card colour in the title-bar band, at the ruler rows only, none in the surface)
         (root (glitnir-nhmac--root-rect img))
         (inside (lambda (b)
                   (and b (<= (nth 0 b) (+ (nth 0 root) (nth 2 root))) (>= (nth 2 b) (nth 0 root))
                        (<= (nth 1 b) (+ (nth 1 root) (nth 3 root))) (>= (nth 3 b) (nth 1 root))))))
    (nhmac-gui--say "glitnir-nhmac/clean-root: %s: cards %S, surface %S, card colours on the screen %S %S"
                    row cards surface left right)
    (unless (and (funcall inside left) (funcall inside right))
      (when (or left right)
        (nhmac-gui--say "glitnir-nhmac/clean-root: %s: outside the root's content %S: card colours %S %S, pixels %S"
                        row root (and (not (funcall inside left)) left) (and (not (funcall inside right)) right)
                        (let ((b (or (and (not (funcall inside left)) left) right)))
                          (and b (format "%06x" (glitnir-nhmac--px img (nth 0 b) (nth 1 b)))))))
      (unless (funcall inside left) (setq left nil))
      (unless (funcall inside right) (setq right nil)))
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
        (nhmac-gui--say "glitnir-nhmac/clean-root: %s: after redraw-frame (%d draws, %d commits, %d damage bytes, %d bytes copied, %d full copies, %d held): %s"
                        row (nhmac-gui--delta before :draws) (nhmac-gui--delta before :commits)
                        (nhmac-gui--delta before :damage-bytes) (nhmac-gui--delta before :bytes-copied)
                        (nhmac-gui--delta before :full-copies) (nhmac-gui--delta before :held) (funcall count))
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
       ;; the card's bounding box is the window's: its corner pixels are outside the rounded shape, the gap's.  Each
       ;; is read two device pixels in (still outside the arc, radius 10 points): the left window's left edge is the
       ;; frame's, and the window server tints a window's outermost device pixel (its rim: the gap #20a040 shows as
       ;; #36a953 there at either scale) and, near the window's own rounded bottom corners, the one inside it too
       (dolist (c (list (cons (+ (nth 0 b) 2) (+ (nth 1 b) 2)) (cons (- (nth 2 b) 2) (+ (nth 1 b) 2))
                        (cons (+ (nth 0 b) 2) (- (nth 3 b) 2)) (cons (- (nth 2 b) 2) (- (nth 3 b) 2))))
         (should (equal (list c (nhmac-gui--far (glitnir-nhmac--px img (car c) (cdr c)) glitnir-nhmac--gap 8))
                        (list c nil))))
       (should-not (nhmac-gui--far (glitnir-nhmac--px img (+ (nth 0 b) r) (nth 1 b)) glitnir-nhmac--card-bg 8))
       (set-window-parameter left 'corner-radius nil)
       (glitnir-nhmac--cards t)
       ;; read two pixels in, as above: the rim tints the corner pixel itself (#f2dca9 for the card's #f0d8a0)
       (let ((img2 (glitnir-nhmac--grab "card-corner-off")))
         (should-not (nhmac-gui--far (glitnir-nhmac--px img2 (+ (nth 0 b) 2) (+ (nth 1 b) 2))
                                     glitnir-nhmac--card-bg 8)))))))

(ert-deftest glitnir-nhmac/card-shadow-only-in-gap ()
  "The look's card leaves the other window's pixels as they are without it; the gap beside the card is darker."
  :tags '(draws)
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

(defconst glitnir-nhmac--block-ring #xff0000 "A block's ring: a colour no other row shows.")

(defun glitnir-nhmac--ring-on (w)
  "Give window W, whose buffer has at least five lines, a ring block around its lines 2 to 4; return their height.
The line count is the default line height times 3 (no line spacing in the rows)."
  (with-current-buffer (window-buffer w)
    (let ((s (save-excursion (goto-char (point-min)) (forward-line 1) (point-marker)))
          (e (save-excursion (goto-char (point-min)) (forward-line 4) (point-marker))))
      (set-window-parameter w 'glitnir-blocks
                            `((ring ,s ,e :radius 6 :ring (,(glitnir-nhmac--hex glitnir-nhmac--block-ring) . 1)
                                    :beneath ,(glitnir-nhmac--hex glitnir-nhmac--gap))))
      (* 3 (frame-char-height (window-frame w))))))

(defun glitnir-nhmac--ring-check (row img bg height w &optional own)
  "Assert that IMG shows the block ring inside the box of background BG (window W's, as IMG shows it), from its text
area's left edge to the box's right within 3 pixels, below its first line, HEIGHT device pixels tall within 3; ROW
names it.  OWN non-nil (a root frame's window, at 20, 60 points in IMG) also asserts the ring's exact colour in its
card's own pixels on its middle row."
  ;; the ring within 32: a window's outermost device pixels on the screen carry the system's light window rim over
  ;; the content (round 10, scale 1: the card's pure red column 0 showed #xff1919 at the window's left edge, a 1 px
  ;; ring there; at scale 2 the ring's inner pixel is clear of it)
  (let ((r (glitnir-nhmac--box img glitnir-nhmac--block-ring 32))
        (box (glitnir-nhmac--box img bg))
        ;; the text area's left edge in the window (its left margin and fringe)
        (inset (- (nth 0 (window-body-pixel-edges w)) (nth 0 (window-pixel-edges w))))
        (card-rows nil))
    (nhmac-gui--say "%s: ring %S in %S, text area %d px in, rows %d px" row r box inset height)
    ;; what puts the ring's left edge where it is: the window's edges, fringes, margins and scale, and the block
    (nhmac-gui--say "%s: window %S body %S fringes %S margins %S scroll-bars %S scale %S block %S" row
                    (window-pixel-edges w) (window-body-pixel-edges w) (window-fringes w) (window-margins w)
                    (window-scroll-bars w) (frame-scale-factor (window-frame w)) (window-parameter w 'glitnir-blocks))
    ;; round 4 (scale 1): the ring 5 px right of the text area's edge.  The pixels across its left side on its middle
    ;; row: a blended (half-pixel) side line, or a line where the ring was never drawn, are different causes
    (when (and r box)
      (let ((y (/ (+ (nth 1 r) (nth 3 r)) 2)))
        (nhmac-gui--say "%s: middle row %d from x %d: %S" row y (nth 0 box)
                        (cl-loop for x from (nth 0 box) to (min (+ (nth 0 r) 3) (1- (nth 0 img)))
                                 collect (format "%06x" (glitnir-nhmac--px img x y))))
        ;; round 6: #xff1919 on the ring's left side, red on its right.  The cards' places and each one's own image
        ;; on that row (whether the glue drew the blend, or the screen made it)
        (let ((cards (plist-get (nhmac--test-cards (window-frame w)) :cards)))
          (setq card-rows (mapcar (lambda (c)
                                    (condition-case e
                                        ;; the card's own pixels: the capture's row less the frame's place in it
                                        ;; (`glitnir-nhmac--root-rect') and the card's; ARGB (round 9 read a
                                        ;; capture row as the card's and got its transparent inside)
                                        (let ((s (glitnir-nhmac--scale)))
                                          (nhmac--query 'test-card-row (window-frame w) (nth 0 c)
                                                        (- y (round (* 60 s)) (nth 2 c))
                                                        (max 0 (- (nth 0 box) (round (* 20 s)) (nth 1 c) 2)) 12))
                                      (error (list 'error e))))
                                  cards))
          (nhmac-gui--say "%s: cards %S, rows %S" row cards card-rows))))
    (should (and r box))
    ;; the ring's left side in its own card's pixels on its middle row is the ring's exact colour, opaque: the check
    ;; within 32 above lets the screen's window rim through, never a blended or missing side the glue drew
    (when own
      (should (cl-some (lambda (cr) (and (listp cr) (member (format "ff%06x" glitnir-nhmac--block-ring) cr)))
                       card-rows)))
    (should (<= (abs (- (nth 0 r) (+ (nth 0 box) inset))) 3))
    (should (<= (abs (- (nth 2 r) (nth 2 box))) 3))
    (should (> (nth 1 r) (nth 1 box)))
    (should (<= (nth 3 r) (nth 3 box)))
    (should (<= (abs (- (1+ (- (nth 3 r) (nth 1 r))) height)) 3))))

(ert-deftest nhmac-blocks-drawn-like-cards ()
  "A window's ring block shows on the screen around its rows, as one image over the frame's pixels next to its
cards; a child frame's window's ring shows the same way, on the child; each goes with its parameter."
  :tags '(draws)
  (glitnir-nhmac--with-cards
   nil
   (lambda (left _right)
     (with-current-buffer (window-buffer left) (dotimes (i 8) (insert (format "line %d\n" i))))
     (let ((h (glitnir-nhmac--ring-on left)))
       (should (= (length (plist-get (glitnir-nhmac--cards t) :cards)) 1))
       (glitnir-nhmac--ring-check 'nhmac-blocks-drawn-like-cards (glitnir-nhmac--grab "block-ring")
                                  glitnir-nhmac--card-bg h left t)
       (set-window-parameter left 'glitnir-blocks nil)
       (should (null (plist-get (glitnir-nhmac--cards t) :cards)))
       (should-not (glitnir-nhmac--box (glitnir-nhmac--grab "block-ring-gone") glitnir-nhmac--block-ring))))))

;; A box fit to the text (`:fit text'): its face's background fills only the text's band of a taller row, the rest of
;; the row in the colour beneath (glitnir-decor-gui/a-box-fit-to-the-text-fills-only-the-text on Linux, which reads the
;; recorder; here the root's own surface).

(defun glitnir-nhmac--fit-fill (fit)
  "On a row a 40 pixel space (all ascent) makes taller than its text, the run \" ab \" in a green face boxed in green
with FIT as its :fit: (AT-TEXT . ABOVE), the root surface's pixels inside its leading space at the text's middle and
3 pixels below the row's top, as #xRRGGBB."
  (let ((glitnir-enable t)
        (face (make-symbol "glitnir-nhmac-fit-fill")))
    (custom-declare-face face nil "Test face.")
    (set-face-attribute face nil :foreground "white" :background "#00c000"
                        :box `(:line-width (1 . 1) :color "#00c000" :corner-radius 4 ,@(and fit `(:fit ,fit))))
    (save-window-excursion
      (with-temp-buffer
        (switch-to-buffer (current-buffer))
        (face-remap-add-relative 'default :background "#202020" :foreground "white")
        (setq-local cursor-type nil)
        (insert "\n\n" (propertize " " 'display '(space :width (1) :height (40) :ascent 100))
                (propertize " ab " 'face face) "\n\n")
        (goto-char 1)
        (nhmac-gui--settle)
        (pcase-let* ((w (selected-window))
                     (`(,wx ,wy . ,_) (window-inside-pixel-edges w))
                     (`(,px . ,py) (posn-x-y (posn-at-point 4 w)))
                     (h (save-excursion (goto-char 4) (line-pixel-height)))
                     (text (frame-char-height))
                     (x (+ wx px 3)) (y (+ wy py)))
          (nhmac-gui--say "glitnir-nhmac/fit-fill %S: run at %S, row %d px, text %d px" fit (cons x y) h text)
          (cons (glitnir-nhmac--surface-px x (+ y (- h (/ text 2))))
                (glitnir-nhmac--surface-px x (+ y 3))))))))

(ert-deftest glitnir-nhmac/a-box-fit-to-the-text-fills-only-the-text ()
  "`:fit text' confines the face's background to the box (the text and the box's lines), the rest of the row in the
colour beneath; stock fills the row (the premise: the probe above the text is in the row)."
  :tags '(draws)
  (pcase-let ((`(,at . ,above) (glitnir-nhmac--fit-fill 'text)))
    (should (equal (list 'text at) (list 'text #x00c000)))
    (should (equal (list 'above above) (list 'above #x202020))))
  (pcase-let ((`(,at . ,above) (glitnir-nhmac--fit-fill nil)))
    (should (equal (list 'text at) (list 'text #x00c000)))
    (should (equal (list 'stock above) (list 'stock #x00c000)))))

;; The overview ruler (docs/specs/nidhogg/2026-11-13-glitnir-overview-ruler-case.md in the org folder): block bars
;; whose clip is a strip in the right fringe, shown on macOS as the window's block image like any block.

(defconst glitnir-nhmac--ruler-mark #xff00ff "The ruler's marks at lines 1 and 501: a colour no other row shows.")
(defconst glitnir-nhmac--ruler-end #x00ff00 "The ruler's mark at the buffer's end.")
(defconst glitnir-nhmac--ruler-band #x0000ff "The ruler's band over the part shown.")

(defun glitnir-nhmac--ruler-on (w)
  "Give window W, showing 1,000 lines, a ruler 12 points wide 2 in from its right edge in a right fringe of 16 points:
marks at lines 1 and 501 in the left lane, one at the end in the right lane, and the band."
  (let ((s (glitnir-nhmac--scale)))
    (set-window-fringes w nil (round (* 16 s)))
    (with-current-buffer (window-buffer w)
      (set-window-parameter
       w 'glitnir-ruler
       `(:width ,(round (* 12 s)) :right ,(round (* 2 s)) :height ,(round (* 4 s))
                :band (,(glitnir-nhmac--hex glitnir-nhmac--ruler-band) . 1)
                :marks ((1 0 ,(round (* 6 s)) (,(glitnir-nhmac--hex glitnir-nhmac--ruler-mark) . 1))
                        (,(save-excursion (goto-char 1) (forward-line 500) (point)) 0 ,(round (* 6 s))
                         (,(glitnir-nhmac--hex glitnir-nhmac--ruler-mark) . 1))
                        (,(point-max) ,(round (* 6 s)) ,(round (* 12 s))
                         (,(glitnir-nhmac--hex glitnir-nhmac--ruler-end) . 1))))))
    (force-window-update w)
    (redisplay t)))

(defun glitnir-nhmac--ruler-boxes (img w)
  "The ruler's marks, end mark and band in IMG as boxes (X0 Y0 X1 Y1), and window W's text area's top, bottom and
right edge in IMG's pixels (the frame at 20, 60 points in the capture, as `glitnir-nhmac--root-rect'): a plist."
  (pcase-let* ((s (glitnir-nhmac--scale)) (fx (round (* 20 s))) (fy (round (* 60 s)))
               (`(,_ ,ty ,tx1 ,ty1) (window-inside-pixel-edges w)))
    (list :marks (glitnir-nhmac--box img glitnir-nhmac--ruler-mark)
          :end (glitnir-nhmac--box img glitnir-nhmac--ruler-end)
          :band (glitnir-nhmac--box img glitnir-nhmac--ruler-band)
          :top (+ fy ty) :bottom (+ fy ty1) :text-right (+ fx tx1))))

(ert-deftest nhmac-ruler-marks-the-buffer-by-position ()
  "A window's `glitnir-ruler' shows in its right fringe, right of the text area: the marks of lines 1 and 501 at the
strip's top and half way down, the end's at its bottom in the right lane, the band over the part shown; a scroll
moves the band to the middle and leaves the marks (glitnir-gui/the-ruler-marks-the-buffer-by-position on Linux)."
  :tags '(draws)
  (skip-unless (fboundp 'glitnir-ruler-position))
  (glitnir-nhmac--with-white
   (lambda ()
     (let ((w (selected-window)) (buf (window-buffer (selected-window))) (k (round (* 3 (glitnir-nhmac--scale)))))
       (with-current-buffer buf (dotimes (i 1000) (insert (format "line %d\n" (1+ i)))))
       (set-window-start w 1)
       (unwind-protect
           (progn
             (glitnir-nhmac--ruler-on w)
             (let* ((b (glitnir-nhmac--ruler-boxes (glitnir-nhmac--grab "ruler") w))
                    (marks (plist-get b :marks)) (end (plist-get b :end)) (band (plist-get b :band))
                    (mid (/ (+ (plist-get b :top) (plist-get b :bottom)) 2)))
               (nhmac-gui--say "nhmac-ruler-marks-the-buffer-by-position: %S" b)
               (should (and marks end band))
               ;; right of the text area, all of it
               (should (> (nth 0 marks) (plist-get b :text-right)))
               (should (> (nth 0 band) (plist-get b :text-right)))
               ;; line 1 at the top, line 501 half way down (the marks' box spans both)
               (should (<= (abs (- (nth 1 marks) (plist-get b :top))) k))
               (should (<= (abs (- (nth 3 marks) mid)) (* 3 k)))
               ;; the end at the bottom, in the right lane
               (should (<= (abs (- (nth 3 end) (1- (plist-get b :bottom)))) k))
               (should (> (nth 0 end) (nth 2 marks)))
               ;; the band: the top part is shown
               (should (<= (abs (- (nth 1 band) (plist-get b :top))) k))
               (should (< (nth 3 band) mid)))
             ;; a scroll to line 496: the band moves to the middle, the marks stay
             (set-window-start w (with-current-buffer buf (save-excursion (goto-char 1) (forward-line 495) (point))))
             (redisplay t)
             (let* ((b (glitnir-nhmac--ruler-boxes (glitnir-nhmac--grab "ruler-scrolled") w))
                    (marks (plist-get b :marks)) (band (plist-get b :band))
                    (mid (/ (+ (plist-get b :top) (plist-get b :bottom)) 2)))
               (nhmac-gui--say "nhmac-ruler-marks-the-buffer-by-position: scrolled %S" b)
               (should (and marks band))
               (should (<= (abs (- (nth 1 marks) (plist-get b :top))) k))
               (should (> (nth 1 band) (+ (plist-get b :top) k)))
               (should (<= (nth 1 band) mid (nth 3 band)))))
         (set-window-parameter w 'glitnir-ruler nil)
         (set-window-fringes w nil nil)
         (with-current-buffer buf (erase-buffer)))))))

(ert-deftest nhmac-ruler-beside-a-ring ()
  "A window with both a ring block and a ruler shows both: the ring in its text area, the ruler in its fringe (the
glue keeps each its own clip)."
  :tags '(draws)
  (skip-unless (fboundp 'glitnir-ruler-position))
  (glitnir-nhmac--with-white
   (lambda ()
     (let ((w (selected-window)) (buf (window-buffer (selected-window))))
       (with-current-buffer buf (dotimes (i 1000) (insert (format "line %d\n" (1+ i)))))
       (set-window-start w 1)
       (unwind-protect
           (progn
             (glitnir-nhmac--ring-on w)
             (glitnir-nhmac--ruler-on w)
             (let* ((img (glitnir-nhmac--grab "ruler-ring"))
                    (b (glitnir-nhmac--ruler-boxes img w))
                    (ring (glitnir-nhmac--box img glitnir-nhmac--block-ring)))
               (nhmac-gui--say "nhmac-ruler-beside-a-ring: ring %S, ruler %S, cards %S" ring b (glitnir-nhmac--cards t))
               (should ring)
               (should (< (nth 2 ring) (plist-get b :text-right)))
               (should (plist-get b :marks))
               (should (> (nth 0 (plist-get b :marks)) (plist-get b :text-right)))))
         (set-window-parameter w 'glitnir-blocks nil)
         (set-window-parameter w 'glitnir-ruler nil)
         (set-window-fringes w nil nil)
         (with-current-buffer buf (erase-buffer)))))))

(ert-deftest nhmac-blocks-drawn-on-a-child-frame ()
  "A child frame's window with a ring block: the child's own layers show the ring, inside the child on the screen;
the root has no block image."
  :tags '(draws)
  (let* ((child (nhmac-gui--child '((width . 30) (height . 8)) 80 200))
         (w (frame-root-window child)))
    (unwind-protect
        (progn
          (with-current-buffer (window-buffer w)
            (erase-buffer)
            (dotimes (i 7) (insert (format "line %d\n" i))))
          (let ((h (glitnir-nhmac--ring-on w)))
            (force-window-update)
            (redisplay t)
            (nhmac-gui--settle)
            (let ((cards (nhmac--test-cards child)))
              (nhmac-gui--say "nhmac-blocks-drawn-on-a-child-frame: child cards %S, root cards %S"
                              cards (nhmac--test-cards))
              (should (= (length (plist-get cards :cards)) 1))
              (should (null (plist-get (nhmac--test-cards) :cards)))
              (glitnir-nhmac--ring-check 'nhmac-blocks-drawn-on-a-child-frame
                                         (glitnir-nhmac--grab "block-ring-child") #xff00ff h w)))
          (set-window-parameter w 'glitnir-blocks nil)
          (force-window-update)
          (redisplay t)
          (nhmac-gui--settle)
          (should (null (plist-get (nhmac--test-cards child) :cards))))
      (with-current-buffer (window-buffer w) (erase-buffer))
      (delete-frame child)
      (nhmac-gui--settle))))

(ert-deftest glitnir-nhmac/card-removed-no-residue ()
  "A card that goes leaves the screen as it was before it came, and the root has no card layers."
  :tags '(draws)
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
                             (if b (glitnir-nhmac--px img (+ (nth 0 b) 2) (+ (nth 1 b) 2)) 0))
             (should (> (plist-get c :images) n))
             (should b)
             ;; two pixels in, inside the window server's rim (as glitnir-nhmac/card-corner-shows-gap)
             (should-not (nhmac-gui--far (glitnir-nhmac--px img (+ (nth 0 b) 2) (+ (nth 1 b) 2)) new 8)))
         (disable-theme 'glitnir-nhmac-test)
         (set-face-foreground 'window-divider face (selected-frame)))))))

(ert-deftest nhmac-colour-push-not-poll ()
  "Redisplay reads no chrome colour; a theme switch pushes the new shadow colour to a shown child."
  :tags '(draws)
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
  (should (glitnir-active-p))
  (let ((glitnir-enable nil))
    (should-not (glitnir-active-p)))
  (should (glitnir-active-p)))

(ert-deftest glitnir-nhmac/stats-count-the-commits ()
  "`glitnir-stats' on Naglfar holds one entry per drawing the frame's layer shows (each a commit), with its draw time."
  :tags '(draws)
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

;;; P5's remaining rows (Task 35).  The pair of windows P5 checked is one container in the parent's layer here: the
;;; content, and below it (sublayer 0) the shadow layer, whose frame is read back in process (`nhmac--test-child');
;;; the stacking order is the containers' `:index' among their parent's sublayers.

(defun glitnir-nhmac--scale-env ()
  "The display scale the lane asked for (GLITNIR_GUI_SCALE)."
  (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1")))

(defun glitnir-nhmac--info-until (frame pred)
  "Settle, wait (5 s at most) until PRED holds of FRAME's `nhmac--test-child', and return it."
  (nhmac-gui--settle)
  (nhmac-gui--idle-until (lambda () (funcall pred (nhmac--test-child frame))) 5)
  (nhmac--test-child frame))

(defun glitnir-nhmac--layer-problem (frame &optional hidden)
  "Nil when FRAME's layers are as its chrome asks, else what is wrong.
Shown: a shadow layer with an image, below the content in the container (sublayer 0) and around it: starting at or
before the content's corner and reaching past its far sides.  HIDDEN: the container is hidden, with the shadow layer
still below the content in it, so the two go together."
  (let* ((i (nhmac--test-child frame)) (sh (plist-get i :shadow)) (fr (plist-get sh :frame)))
    (cond ((and hidden (not (plist-get i :hidden))) (list "the container is not hidden" i))
          ((and (not hidden) (plist-get i :hidden)) (list "the container is hidden" i))
          ((/= (plist-get i :shadow-index) 0) (list "the shadow layer is not below the content" i))
          (hidden nil)
          ((null sh) (list "no shadow layer" i))
          ((not (eq (plist-get sh :image) t)) (list "the shadow layer has no image" i))
          ((not (and (< (nth 0 fr) 0) (< (nth 1 fr) 0)
                     (> (+ (nth 0 fr) (nth 2 fr)) (plist-get i :w))
                     (> (+ (nth 1 fr) (nth 3 fr)) (plist-get i :h))))
           (list "the shadow layer does not surround the content" i)))))

(defun glitnir-nhmac--layers-settled (frame &optional hidden)
  "Settle, wait (5 s at most) for FRAME's layers to hold, and return what is still wrong with them, or nil."
  (nhmac-gui--settle)
  (nhmac-gui--idle-until (lambda () (null (glitnir-nhmac--layer-problem frame hidden))) 5)
  (glitnir-nhmac--layer-problem frame hidden))

(defun glitnir-nhmac--order (frames)
  "FRAMES in their parent's layer order, bottom to top."
  (sort (copy-sequence frames)
        (lambda (a b) (< (plist-get (nhmac--test-child a) :index) (plist-get (nhmac--test-child b) :index)))))

(defun glitnir-nhmac--ordered (frames)
  "Settle, wait (5 s at most) until the layers stack FRAMES in that order, bottom to top; return their order."
  (nhmac-gui--settle)
  (nhmac-gui--idle-until (lambda () (equal (glitnir-nhmac--order frames) frames)) 5)
  (glitnir-nhmac--order frames))

(ert-deftest glitnir-nhmac/runner-smoke ()
  "The lane's own proof: the frame is at the scale the lane asked for, and a capture has the screen's pixels."
  :tags '(draws)
  (should (< (abs (- (frame-scale-factor) (glitnir-nhmac--scale-env))) 0.001))
  (pcase-let ((`(,w ,h ,px) (glitnir-nhmac--grab "smoke")))
    (nhmac-gui--say "glitnir-nhmac/runner-smoke: capture %dx%d, frame %dx%d at scale %s" w h
                    (frame-pixel-width) (frame-pixel-height) (frame-scale-factor))
    (should (> w 0))
    (should (> h 0))
    (should (<= w (* 900 (glitnir-nhmac--scale-env))))
    (should (= (length px) (* w h)))))

(ert-deftest glitnir-nhmac/layers-list-the-children ()
  "Each child frame without chrome is a container in its parent's layer, with a position and a size and no panel,
shadow or blur, and the containers stack in the order the frames were made."
  :tags '(draws)
  (let ((a (nhmac-gui--child nil 40 40)) (b (nhmac-gui--child nil 120 80)))
    (unwind-protect
        (let ((ia (nhmac--test-child a)) (ib (nhmac--test-child b)))
          (nhmac-gui--say "glitnir-nhmac/layers-list-the-children: %S %S" ia ib)
          (dolist (i (list ia ib))
            (should (eq (plist-get i :host) 'parent))
            (should (cl-every #'numberp (mapcar (lambda (k) (plist-get i k)) '(:x :y :w :h))))
            (should (> (plist-get i :w) 0))
            (should (> (plist-get i :h) 0))
            (should-not (plist-get i :shadow))
            (should-not (plist-get i :panel))
            (should-not (plist-get i :blur)))
          (should (>= (plist-get ia :index) 0))
          (should (< (plist-get ia :index) (plist-get ib :index))))
      (delete-frame b t)
      (delete-frame a t))))

(ert-deftest glitnir-nhmac/layer-follows-move ()
  "A chromed child moved by `set-frame-position' and by its `left' and `top' parameters keeps its shadow layer
below the content and around it, and its container goes with it."
  :tags '(draws)
  (let ((c (glitnir-nhmac--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-nhmac--layers-settled c))
          (let ((x0 (plist-get (nhmac--test-child c) :x)))
            (set-frame-position c 160 200)
            (should (> (plist-get (glitnir-nhmac--info-until c (lambda (i) (> (plist-get i :x) x0))) :x) x0))
            (should-not (glitnir-nhmac--layers-settled c))
            (modify-frame-parameters c '((left . 40) (top . 180)))
            (should (< (plist-get (glitnir-nhmac--info-until c (lambda (i) (< (plist-get i :x) x0))) :x) x0))
            (should-not (glitnir-nhmac--layers-settled c))))
      (delete-frame c t))))

(ert-deftest glitnir-nhmac/layer-follows-resize ()
  "A chromed child made larger and then smaller keeps its shadow layer around its content as the container's size
follows."
  :tags '(draws)
  (let ((c (glitnir-nhmac--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-nhmac--layers-settled c))
          (let ((w0 (plist-get (nhmac--test-child c) :w)))
            (set-frame-size c 50 12)
            (should (> (plist-get (glitnir-nhmac--info-until c (lambda (i) (> (plist-get i :w) w0))) :w) w0))
            (should-not (glitnir-nhmac--layers-settled c))
            (set-frame-size c 200 90 t)
            (should (< (plist-get (glitnir-nhmac--info-until c (lambda (i) (< (plist-get i :w) w0))) :w) w0))
            (should-not (glitnir-nhmac--layers-settled c))))
      (delete-frame c t))))

(ert-deftest glitnir-nhmac/layer-follows-raise ()
  "Raising a chromed child over its sibling and back reorders the containers, each keeping its shadow layer."
  :tags '(draws)
  (let* ((a (glitnir-nhmac--chromed nil 80 200)) (b (glitnir-nhmac--chromed nil 160 260)))
    (unwind-protect
        (progn
          (should-not (glitnir-nhmac--layers-settled a))
          (should-not (glitnir-nhmac--layers-settled b))
          (should (equal (glitnir-nhmac--ordered (list a b)) (list a b)))
          (raise-frame a)
          (should (equal (glitnir-nhmac--ordered (list b a)) (list b a)))
          (should-not (glitnir-nhmac--layers-settled a))
          (should-not (glitnir-nhmac--layers-settled b))
          (raise-frame b)
          (should (equal (glitnir-nhmac--ordered (list a b)) (list a b)))
          (should-not (glitnir-nhmac--layers-settled a))
          (should-not (glitnir-nhmac--layers-settled b)))
      (delete-frame b t)
      (delete-frame a t))))

(ert-deftest glitnir-nhmac/layer-follows-order-back ()
  "Lowering the upper of two chromed children puts its container below the other, each keeping its shadow layer."
  :tags '(draws)
  (let* ((a (glitnir-nhmac--chromed nil 80 200)) (b (glitnir-nhmac--chromed nil 160 260)))
    (unwind-protect
        (progn
          (should-not (glitnir-nhmac--layers-settled b))
          (lower-frame b)
          (should (equal (glitnir-nhmac--ordered (list b a)) (list b a)))
          (should-not (glitnir-nhmac--layers-settled a))
          (should-not (glitnir-nhmac--layers-settled b)))
      (delete-frame b t)
      (delete-frame a t))))

(ert-deftest glitnir-nhmac/layer-follows-restack ()
  "`frame-restack' of a chromed child below and above a sibling puts its container right there, every child keeping
its shadow layer."
  :tags '(draws)
  (let* ((a (glitnir-nhmac--chromed nil 80 200)) (b (glitnir-nhmac--chromed nil 160 260))
         (c (glitnir-nhmac--chromed nil 240 320)))
    (unwind-protect
        (progn
          (should-not (glitnir-nhmac--layers-settled c))
          (frame-restack c a)             ; c below a
          (should (equal (glitnir-nhmac--ordered (list c a b)) (list c a b)))
          (dolist (f (list a b c)) (should-not (glitnir-nhmac--layers-settled f)))
          (frame-restack c a t)           ; c above a
          (should (equal (glitnir-nhmac--ordered (list a c b)) (list a c b)))
          (dolist (f (list a b c)) (should-not (glitnir-nhmac--layers-settled f))))
      (delete-frame c t)
      (delete-frame b t)
      (delete-frame a t))))

(ert-deftest glitnir-nhmac/layer-follows-z-group ()
  "A chromed child put in the `above' z-group and out of it keeps its shadow layer."
  :tags '(draws)
  (let ((c (glitnir-nhmac--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-nhmac--layers-settled c))
          (set-frame-parameter c 'z-group 'above)
          (should-not (glitnir-nhmac--layers-settled c))
          (set-frame-parameter c 'z-group nil)
          (should-not (glitnir-nhmac--layers-settled c)))
      (delete-frame c t))))

(ert-deftest glitnir-nhmac/layer-follows-hide-show ()
  "A chromed child hidden and shown three times: hidden, its container is hidden with the shadow layer still below
the content in it; shown, the layers are whole again."
  :tags '(draws)
  (let ((c (glitnir-nhmac--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-nhmac--layers-settled c))
          (dotimes (_ 3)
            (make-frame-invisible c t)
            (should-not (glitnir-nhmac--layers-settled c t))
            (make-frame-visible c)
            (should-not (glitnir-nhmac--layers-settled c))))
      (delete-frame c t))))

(ert-deftest glitnir-nhmac/layer-follows-parent-move ()
  "Moving the root frame leaves a chromed child's container where it was in the root's layer, with its shadow layer."
  :tags '(draws)
  (let* ((root (selected-frame)) (pos (frame-position root)) (c (glitnir-nhmac--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-nhmac--layers-settled c))
          (let* ((i0 (nhmac--test-child c)) (at (list (plist-get i0 :x) (plist-get i0 :y))))
            (set-frame-position root (+ (car pos) 30) (+ (cdr pos) 20))
            (nhmac-gui--settle)
            (let ((i (nhmac--test-child c)))
              (should (equal (list (plist-get i :x) (plist-get i :y)) at)))
            (should-not (glitnir-nhmac--layers-settled c))))
      (set-frame-position root (car pos) (cdr pos))
      (nhmac-gui--settle)
      (delete-frame c t))))

(ert-deftest glitnir-nhmac/layer-follows-reparent ()
  "A chromed child given another child frame as its parent and then the root again keeps its shadow layer, and no
window of the window server comes of it."
  :tags '(draws)
  (let* ((root (selected-frame)) (n (length (nhmac--test-windows)))
         (p (nhmac-gui--child '((width . 70) (height . 25) (background-color . "#ffff00")) 10 10))
         (c (glitnir-nhmac--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-nhmac--layers-settled c))
          (set-frame-parameter c 'parent-frame p)
          (should (eq (frame-parent c) p))
          (should-not (glitnir-nhmac--layers-settled c))
          (set-frame-parameter c 'parent-frame root)
          (should (eq (frame-parent c) root))
          (should-not (glitnir-nhmac--layers-settled c))
          (should (nhmac-gui--windows-become n)))
      (delete-frame c t)
      (delete-frame p t))))

(ert-deftest glitnir-nhmac/layer-follows-undecorated-toggle ()
  "A child frame made decorated has no chrome (no shadow layer; Glitnir reads none), and undecorated again it has its
shadow layer once more."
  :tags '(draws)
  (let ((c (glitnir-nhmac--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-nhmac--layers-settled c))
          (set-frame-parameter c 'undecorated nil)
          (nhmac-gui--settle)
          (should (nhmac-gui--idle-until
                   (lambda () (condition-case nil (null (plist-get (nhmac--test-child c) :shadow)) (error nil)))
                   5))
          (should-not (glitnir--chrome c))
          (set-frame-parameter c 'undecorated t)
          (let ((problem (glitnir-nhmac--layers-settled c)))
            ;; round 2 (s1 only): no shadow layer here.  Whether C resolved the chrome again (glitnir--chrome) and
            ;; the shell kept none, or C has none to give
            (nhmac-gui--say "glitnir-nhmac/layer-follows-undecorated-toggle: undecorated again: %S; chrome %S, params %S"
                            problem (glitnir--chrome c)
                            (mapcar (lambda (p) (cons p (frame-parameter c p))) '(undecorated shadow corner-radius)))
            ;; round 4: C has the chrome and the shell none.  The requests the shell got: whether the glue sent the
            ;; chrome again after the second toggle
            (nhmac-gui--say "glitnir-nhmac/layer-follows-undecorated-toggle: chrome requests %S"
                            (nhmac--query 'test-chrome-log))
            (should-not problem)))
      (delete-frame c t))))

(ert-deftest glitnir-nhmac/layer-follows-delete ()
  "Deleting a chromed child takes its container, with the shadow layer, out of the parent's layer: the container
above it moves down by one, and the window server lists no window more or less."
  :tags '(draws)
  (let* ((n (length (nhmac--test-windows)))
         (a (glitnir-nhmac--chromed nil 80 200)) (b (glitnir-nhmac--chromed nil 160 260)))
    (unwind-protect
        (progn
          (should-not (glitnir-nhmac--layers-settled a))
          (let ((ib (plist-get (nhmac--test-child b) :index)))
            (delete-frame a t)
            (should (= (plist-get (glitnir-nhmac--info-until b (lambda (i) (< (plist-get i :index) ib))) :index)
                       (1- ib)))
            (should-not (glitnir-nhmac--layers-settled b))
            (should (nhmac-gui--windows-become n))))
      (when (frame-live-p a) (delete-frame a t))
      (delete-frame b t))))

;;; Pixels: the child's own colour is cyan (#00ffff), so the capture's box of it is the child; a blur is read as the
;;; loss of detail (the mean absolute Laplacian of the luminance over a strip inside the child) against the pattern
;;; the root shows beneath.  Detail beneath 35 % is blurred (the spike's measure).

(defconst glitnir-nhmac--cyan #x00ffff "The child's colour in the translucency and blur rows.")

(defun glitnir-nhmac--detail (img rect)
  "The mean absolute Laplacian of the luminance over RECT (X Y W H, device pixels) of IMG."
  (pcase-let ((`(,x ,y ,w ,h) rect) (sum 0.0) (n 0))
    (cl-flet ((l (i j) (let ((p (glitnir-nhmac--px img i j)))
                         (+ (* 0.2126 (ash p -16)) (* 0.7152 (logand (ash p -8) 255)) (* 0.0722 (logand p 255))))))
      (cl-loop for j from (1+ y) below (+ y h -1)
               do (cl-loop for i from (1+ x) below (+ x w -1)
                           do (setq sum (+ sum (abs (- (* 4 (l i j)) (l (1- i) j) (l (1+ i) j) (l i (1- j)) (l i (1+ j)))))
                                    n (1+ n)))))
    (/ sum (max n 1))))

(defun glitnir-nhmac--strip (box)
  "A strip inside BOX (X0 Y0 X1 Y1) away from its corners and edges: 12 points in, 40 points high, as (X Y W H)."
  (let* ((s (glitnir-nhmac--scale)) (in (ceiling (* 12 s)))
         (w (- (1+ (- (nth 2 box) (nth 0 box))) in in)) (h (- (1+ (- (nth 3 box) (nth 1 box))) in in)))
    (list (+ (nth 0 box) in) (+ (nth 1 box) in) w (min (round (* 40 s)) h))))

(defun glitnir-nhmac--diff (a b rect)
  "How images A and B differ over RECT (X Y W H): (PIXELS-DIFFERING LARGEST-CHANNEL-DIFFERENCE)."
  (pcase-let ((`(,x ,y ,w ,h) rect) (n 0) (mx 0))
    (dotimes (j h)
      (dotimes (i w)
        (let ((p (glitnir-nhmac--px a (+ x i) (+ y j))) (q (glitnir-nhmac--px b (+ x i) (+ y j))))
          (unless (eql p q)
            (setq n (1+ n))
            (dolist (sh '(16 8 0))
              (setq mx (max mx (abs (- (logand (ash p (- sh)) 255) (logand (ash q (- sh)) 255))))))))))
    (list n mx)))

(defun glitnir-nhmac--root-rect (img)
  "The root frame's pixels in capture IMG as (X Y W H): the frame's place in the capture (`nhmac-gui--capture'),
4 points in on every side, clipped to IMG."
  (let* ((s (glitnir-nhmac--scale)) (in (round (* 4 s)))
         (x (+ (round (* 20 s)) in)) (y (+ (round (* 60 s)) in)))
    (list x y (min (- (frame-pixel-width) in in) (- (nth 0 img) x)) (min (- (frame-pixel-height) in in) (- (nth 1 img) y)))))

(defun glitnir-nhmac--with-pattern (fn)
  "Call FN with the root's one window showing lines of `#' and `@' (hard edges), no scroll bar, then as before."
  (let ((config (current-window-configuration)) (buf (get-buffer-create " *glitnir-nhmac-pattern*")))
    (unwind-protect
        (progn
          (with-current-buffer buf
            (erase-buffer)
            (setq mode-line-format nil cursor-type nil)
            (dotimes (_ 60) (insert (make-string 120 ?#) "\n" (make-string 120 ?@) "\n"))
            (goto-char (point-min)))
          (delete-other-windows)
          (set-window-buffer (selected-window) buf)
          (set-window-start (selected-window) 1)
          ;; AppKit redraws a scroller's knob with its ends a level apart from one capture to the next
          (set-window-scroll-bars (selected-window) nil nil nil nil)
          (nhmac-gui--settle)
          (funcall fn))
      (set-window-configuration config)
      (kill-buffer buf)
      (nhmac-gui--settle))))

(defun glitnir-nhmac--cyan-child (params)
  "A shown chromed child with PARAMS and the cyan background, and its box in a capture (the child, opaque, alone)."
  (let ((c (glitnir-nhmac--chromed (append params '((shadow . nil) (corner-radius . 8)
                                                    (background-color . "#00ffff"))))))
    (cons c (glitnir-nhmac--box (glitnir-nhmac--grab "cyan-child") glitnir-nhmac--cyan))))

(ert-deftest glitnir-nhmac/alpha-background-shows-beneath ()
  "A child with `alpha-background' 0.5 shows its background half and what lies beneath half (the pixel between its
text and its bottom edge, within 8 per channel of the mean of the opaque child's and the root's), and its text stays
opaque: every pixel inside a stem (black with its four neighbours) is the opaque child's."
  :tags '(draws)
  (glitnir-nhmac--with-white
   (lambda ()
     (let* ((buf (with-current-buffer (get-buffer-create " *glitnir-nhmac-text*")
                   (erase-buffer)
                   (insert "The quick brown fox jumps over the lazy dog.")
                   (setq mode-line-format nil cursor-type nil truncate-lines t)
                   (face-remap-add-relative 'default :height 3.0)
                   (current-buffer)))
            (c (glitnir-nhmac--chromed '((shadow . nil) (corner-radius . 8) (background-color . "#00ffff")
                                         (foreground-color . "#000000")))))
       (unwind-protect
           (progn
             (set-window-buffer (frame-root-window c) buf)
             (nhmac-gui--settle)
             (let* ((s (glitnir-nhmac--scale)) (stock (glitnir-nhmac--grab "alpha-stock"))
                    (box (glitnir-nhmac--box stock glitnir-nhmac--cyan))
                    (bx (/ (+ (nth 0 box) (nth 2 box)) 2)) (by (- (nth 3 box) (round (* 4 s))))
                    (in (ceiling (* 9 s)))
                    (inner (list (+ (nth 0 box) in) (+ (nth 1 box) in)
                                 (- (nth 2 box) (nth 0 box) in in) (- (nth 3 box) (nth 1 box) in in)))
                    beneath on)
               (should box)
               (make-frame-invisible c t)
               (nhmac-gui--settle)
               (setq beneath (glitnir-nhmac--grab "alpha-beneath"))
               (make-frame-visible c)
               (set-frame-parameter c 'alpha-background 0.5)
               (nhmac-gui--settle)
               (nhmac-gui--idle-until
                (lambda () (nhmac-gui--far (glitnir-nhmac--px (setq on (glitnir-nhmac--grab "alpha-on")) bx by)
                                           glitnir-nhmac--cyan 8))
                5)
               (let ((want (mapcar (lambda (sh) (/ (+ (logand (ash (glitnir-nhmac--px stock bx by) (- sh)) 255)
                                                      (logand (ash (glitnir-nhmac--px beneath bx by) (- sh)) 255))
                                                   2.0))
                                   '(16 8 0)))
                     (got (mapcar (lambda (sh) (logand (ash (glitnir-nhmac--px on bx by) (- sh)) 255)) '(16 8 0))))
                 (nhmac-gui--say "glitnir-nhmac/alpha-background-shows-beneath: at %S got %S want %S" (cons bx by) got want)
                 (should (cl-every (lambda (g w) (<= (abs (- g w)) 8)) got want)))
               ;; the text is opaque: a stem's inside is the opaque child's pixel (a black edge pixel can be two
               ;; partial draws, which blend with what is beneath)
               (pcase-let ((`(,x ,y ,w ,h) inner) (n 0) (changed 0))
                 (cl-flet ((black (i j) (eql (glitnir-nhmac--px stock i j) 0)))
                   (dotimes (j (- h 2))
                     (dotimes (i (- w 2))
                       (let ((u (+ x i 1)) (v (+ y j 1)))
                         (when (and (black u v) (black (1- u) v) (black (1+ u) v) (black u (1- v)) (black u (1+ v)))
                           (setq n (1+ n))
                           (unless (eql (glitnir-nhmac--px on u v) (glitnir-nhmac--px stock u v))
                             (setq changed (1+ changed))))))))
                 (nhmac-gui--say "glitnir-nhmac/alpha-background-shows-beneath: %d stem pixels, %d changed" n changed)
                 (should (> n 20))
                 (should (= changed 0)))))
         (delete-frame c t)
         (kill-buffer buf))))))

(ert-deftest glitnir-nhmac/blur-behind-child ()
  "A child over a pattern of hard edges: translucent (`alpha-background' 0.3) the pattern shows through sharp (over
half its detail), and with `backdrop-blur' 20 the strip inside the child has under 35 % of the detail beneath, the
child living in a blur panel.  The system's Reduce transparency setting, if on, would fail the row."
  :tags '(draws)
  (glitnir-nhmac--with-pattern
   (lambda ()
     (pcase-let ((`(,c . ,box) (glitnir-nhmac--cyan-child nil)))
       (unwind-protect
           (let* ((strip (glitnir-nhmac--strip box)) d0 d1 d2)
             (should box)
             (make-frame-invisible c t)
             (nhmac-gui--settle)
             (setq d0 (glitnir-nhmac--detail (glitnir-nhmac--grab "blur-beneath") strip))
             (nhmac-gui--say "glitnir-nhmac/blur-behind-child: detail beneath %.2f" d0)
             (should (> d0 5))
             (make-frame-visible c)
             (set-frame-parameter c 'alpha-background 0.3)
             (nhmac-gui--settle)
             (nhmac-gui--idle-until
              (lambda () (> (setq d1 (glitnir-nhmac--detail (glitnir-nhmac--grab "blur-translucent") strip)) (* 0.5 d0)))
              5)
             (should (> d1 (* 0.5 d0)))
             (set-frame-parameter c 'backdrop-blur 20)
             (nhmac-gui--settle)
             (nhmac-gui--idle-until
              (lambda () (< (setq d2 (glitnir-nhmac--detail (glitnir-nhmac--grab "blur-blurred") strip)) (* 0.35 d0)))
              5)
             (nhmac-gui--say "glitnir-nhmac/blur-behind-child: translucent %.2f, blurred %.2f" d1 d2)
             (should (eq (plist-get (nhmac--test-child c) :blur) t))
             (should (< d2 (* 0.35 d0))))
         (delete-frame c t))))))

(ert-deftest glitnir-nhmac/blur-off-identical ()
  "A blur without a translucent background, or with Glitnir off, is the plain frame: an opaque child with
`backdrop-blur' is pixel for pixel the one without (tolerance 0), and a translucent, blurred, rounded child with
`glitnir-enable' nil is the square child of the same `alpha-background', with no panel and no blur.
`alpha-background' is Emacs's own (NS and pgtk honour it with no Glitnir), so Glitnir off keeps it; round 2 compared
with an opaque child and found the translucency (22736 pixels)."
  :tags '(draws)
  (glitnir-nhmac--with-pattern
   (lambda ()
     (pcase-let ((`(,c . ,box) (glitnir-nhmac--cyan-child '((backdrop-blur . 20)))))
       (unwind-protect
           (let* ((pad 2) (rect (list (- (nth 0 box) pad) (- (nth 1 box) pad)
                                      (+ (- (nth 2 box) (nth 0 box)) 1 pad pad) (+ (- (nth 3 box) (nth 1 box)) 1 pad pad)))
                  with without off stock)
             (should box)
             (setq with (glitnir-nhmac--grab "blur-off-with"))
             (set-frame-parameter c 'backdrop-blur nil)
             (setq without (glitnir-nhmac--grab "blur-off-without"))
             (should (equal (glitnir-nhmac--diff with without rect) '(0 0)))
             (modify-frame-parameters c '((backdrop-blur . 20) (alpha-background . 0.3)))
             (let ((glitnir-enable nil))
               (glitnir-refresh c)
               (nhmac-gui--settle)
               (let ((i (nhmac--test-child c)))
                 (should (= (plist-get i :radius) 0))
                 (should-not (plist-get i :blur))
                 (should-not (plist-get i :panel)))
               (setq off (glitnir-nhmac--grab "blur-off-glitnir-off")))
             (modify-frame-parameters c '((corner-radius . 0) (backdrop-blur . nil)))
             (glitnir-refresh c)
             (setq stock (glitnir-nhmac--grab "blur-off-stock"))
             (should (equal (glitnir-nhmac--diff off stock rect) '(0 0)))
             ;; translucent: its middle is not the opaque cyan (the child itself was found above: `box')
             (should (nhmac-gui--far (glitnir-nhmac--px stock (/ (+ (nth 0 box) (nth 2 box)) 2)
                                                       (/ (+ (nth 1 box) (nth 3 box)) 2))
                                     glitnir-nhmac--cyan 8)))
         (delete-frame c t))))))

(defun glitnir-nhmac--fixture-run ()
  "Type, scroll and show and hide a plain child in the root's one window; the capture after each step."
  (let* ((w (selected-window)) (buf (window-buffer w)) shots)
    (with-current-buffer buf
      (goto-char (point-min))
      (insert "typed: the quick brown fox\n")
      (set-window-point w (point)))
    (push (glitnir-nhmac--grab "fixture-typed") shots)
    (set-window-start w (with-current-buffer buf (goto-char (point-min)) (forward-line 20) (point)))
    (push (glitnir-nhmac--grab "fixture-scrolled") shots)
    (let ((c (nhmac-gui--child nil 80 200)))
      (unwind-protect (push (glitnir-nhmac--grab "fixture-popup") shots)
        (delete-frame c t)))
    (push (glitnir-nhmac--grab "fixture-hidden") shots)
    (nreverse shots)))

(ert-deftest glitnir-nhmac/on-equals-off ()
  "On a fixture without chrome (typing, a scroll, a plain child shown and deleted), `glitnir-enable' t draws what
nil draws, in every step's capture (tolerance 0).  The control, nil against nil again, is logged and not asserted:
it tells a difference of ours from one of the window server's."
  :tags '(draws)
  (let (on off again)
    (cl-flet ((rewind ()
                (with-current-buffer (window-buffer (selected-window)) (delete-region (point-min) (+ (point-min) 27)))
                (set-window-start (selected-window) 1)))
      (glitnir-nhmac--with-pattern
       (lambda ()
         (nhmac-gui--without-blink
          (lambda ()
            (let ((glitnir-enable t)) (setq on (glitnir-nhmac--fixture-run)))
            (rewind)
            (let ((glitnir-enable nil)) (setq off (glitnir-nhmac--fixture-run)))
            (rewind)
            (let ((glitnir-enable nil)) (setq again (glitnir-nhmac--fixture-run))))))))
    (should (= (length on) (length off) (length again) 4))
    (let* ((rect (glitnir-nhmac--root-rect (car on)))
           (control (cl-loop for a in off for b in again collect (glitnir-nhmac--diff a b rect)))
           (diffs (cl-loop for a in on for b in off collect (glitnir-nhmac--diff a b rect))))
      (nhmac-gui--say "glitnir-nhmac/on-equals-off: rect %S, on vs off %S, control off vs off %S" rect diffs control)
      (should (equal diffs '((0 0) (0 0) (0 0) (0 0)))))))

;;; Layout legs: a window's old layout is a layer of its text area's capture, moved by the leg; the glue's legs.rs
;;; takes the vector `glitnir--leg-vector' makes (device pixels).  The leg's state is read by its pixels and by
;;; Lisp's own record of what it sent (`glitnir--leg-sent', its serial), since the shell has no query for it.

(defun glitnir-nhmac--leg-area ()
  "The selected window's text area as (X Y W H) in device pixels."
  (pcase-let ((`(,l ,tp ,r ,b) (window-body-pixel-edges)))
    (list l tp (- r l) (- b tp))))

(defun glitnir-nhmac--leg-end ()
  "End the selected window's leg: both parameters go, and the layers with them."
  (dolist (k '(glitnir-layout-capture glitnir-layout-motion)) (set-window-parameter nil k nil))
  (redisplay t)
  (nhmac-gui--settle))

(defun glitnir-nhmac--leg-capture ()
  "Ask for a capture of the selected window and wait (5 s at most) until the presenter holds it.
A leg left by an earlier row ends first: the same value would not be a new request."
  (glitnir-nhmac--leg-end)
  (set-window-parameter nil 'glitnir-layout-capture t)
  (redisplay t)
  (let ((ok (gui-runner-wait #'glitnir-layout-captured-p 5)))
    (unless ok
      ;; round 2: never captured.  Which step stops: Lisp sent no leg (leg-sent, reduced motion), or the shell
      ;; answers another serial than the one sent (nil: no legs on this frame; 0: no capture taken)
      (let ((sent (window-parameter nil 'glitnir--leg-sent)))
        (nhmac-gui--say "glitnir-nhmac--leg-capture: not captured: to-send %S, reduced motion %S, sent (cap mot serial edges moved) %S, shell's serial %S, shell's legs %S"
                        (glitnir--leg-to-send (selected-window)) (glitnir--reduced-motion-p)
                        (seq-take sent 5) (glitnir--layout-leg (selected-window) (nth 5 sent))
                        (glitnir-nhmac--legs))))
    (should ok)))

(defun glitnir-nhmac--legs ()
  "The shell's layout legs of the selected frame: each window's (KEY CAPTURED SKIPPED PENDING RUNNING) and the log of
captures, legs, starts and ends."
  (condition-case err (nhmac--query 'test-legs (selected-frame)) (error err)))

(defun glitnir-nhmac--leg-scroll ()
  "Show the next line first: the pattern's two kinds of line trade places."
  (set-window-start nil (with-current-buffer (window-buffer) (goto-char (point-min)) (forward-line 1) (point)))
  (redisplay t))

(ert-deftest glitnir-nhmac/layout-leg-shows-old-picture ()
  "The capture is the picture from before the redraw: a stepped leg with no items shows it until its midpoint, to
within 2 per channel, and when the leg ends the new layout is what shows (over 100 pixels differ).
The leg is drawn as `glitnir::layout::compose_leg' draws it for the Wayland presenter: the background is the
capture cross-faded to the new picture (a stepped leg switches at its midpoint), and a move shows the new picture
at its moving place, so a whole-area move would show the new layout from the start."
  :tags '(draws)
  (let ((glitnir-reduced-motion nil) before during after)
    (glitnir-nhmac--with-pattern
     (lambda ()
       (setq before (glitnir-nhmac--grab "leg-before"))
       (glitnir-nhmac--leg-capture)
       ;; the motion goes out with the redraw that draws the new layout; 8 s, as the glue takes at most 10 (round 4
       ;; asked for 30: the glue refused the leg, and the new picture showed at once)
       (set-window-parameter nil 'glitnir-layout-motion '(:duration 8 :step t :background "#000000"))
       (glitnir-nhmac--leg-scroll)
       (should (> (nth 2 (window-parameter nil 'glitnir--leg-sent)) 0))
       (setq during (glitnir-nhmac--grab "leg-during"))
       (nhmac-gui--say "glitnir-nhmac/layout-leg-shows-old-picture: legs during %S" (glitnir-nhmac--legs))
       ;; the shell took the leg (a refused one leaves no line)
       (should (string-match-p "started [0-9]+:" (format "%S" (glitnir-nhmac--legs))))
       (glitnir-nhmac--leg-end)
       (setq after (glitnir-nhmac--grab "leg-after"))
       (let* ((rect (glitnir-nhmac--root-rect before))
              (shown (glitnir-nhmac--diff before during rect)) (new (glitnir-nhmac--diff during after rect)))
         (nhmac-gui--say "glitnir-nhmac/layout-leg-shows-old-picture: during vs before %S, after vs during %S, legs %S"
                         shown new (glitnir-nhmac--legs))
         ;; round 6 (scale 2): the leg ended and the screen still showed the old picture.  Whether the new one
         ;; comes late (a commit after the grab) or never (the root's contents not the newest picture)
         (when (<= (car new) 100)
           (let ((stats (nhmac-stats)))
             (sit-for 2)
             (nhmac-gui--say "glitnir-nhmac/layout-leg-shows-old-picture: 2 s later vs during %S, window start %S, surface %S, stats then %S now %S"
                             (glitnir-nhmac--diff during (glitnir-nhmac--grab "leg-later") rect)
                             (window-start) (nhmac--test-surface 0 0)
                             stats (nhmac-stats))
             ;; round 8 (scale 2): still the old picture 2 s on, the root's contents the newest surface and the leg
             ;; ended.  Which layers sit over the root (a leg's layer left behind), and the surface's own row beside
             ;; the screen's: the drawing behind, or a layer over it
             (nhmac-gui--say "glitnir-nhmac/layout-leg-shows-old-picture: own layers %S (nhwl-scroll-layers %S); surface row %d: %S"
                             (nhwl--scroll-layers) nhwl-scroll-layers (/ (frame-pixel-height) 2)
                             (cl-loop for p across (nhmac--test-surface-region 0 (/ (frame-pixel-height) 2) 64 1)
                                      collect (format "%06x" (logand p #xffffff))))
             ;; round 9: the surface has the pattern; the screen's same row before the leg and after it (the surface
             ;; never redrawn, or the screen not showing it)
             (let* ((s (glitnir-nhmac--scale)) (x0 (round (* 20 s)))
                    (y (+ (round (* 60 s)) (/ (frame-pixel-height) 2))))
               ;; round 10: both rows black where the surface has the pattern: the capture's size, and where the
               ;; surface's background (its first pixel) is in it (the frame not at 20, 60 points at this scale)
               (let ((bg (logand (aref (nhmac--test-surface-region 0 (/ (frame-pixel-height) 2) 1 1) 0) #xffffff)))
                 (nhmac-gui--say "glitnir-nhmac/layout-leg-shows-old-picture: capture %dx%d, rect %S, #%06x at %S"
                                 (nth 0 before) (nth 1 before) rect bg (glitnir-nhmac--box before bg 0))
                 ;; round 11 (scale 2): the frame's region black throughout, its grey at capture x 1302 on: where the
                 ;; frame is against the corner the capture is taken from
                 (nhmac-gui--say "glitnir-nhmac/layout-leg-shows-old-picture: GLITNIR_GUI_FRAME %S, frame-position %S, pixel size %S, geometry %S, window %S"
                                 (getenv "GLITNIR_GUI_FRAME") (frame-position)
                                 (list (frame-pixel-width) (frame-pixel-height)) (frame-geometry)
                                 (nhmac--query 'window-info (selected-frame))))
               (dolist (g (list (cons "before" before) (cons "after" after)))
                 (nhmac-gui--say "glitnir-nhmac/layout-leg-shows-old-picture: screen row %d %s: %S" y (car g)
                                 (cl-loop for x from x0 below (+ x0 64)
                                          collect (format "%06x" (glitnir-nhmac--px (cdr g) x y))))))))
         (should (<= (cadr shown) 2))
         (should (> (car new) 100)))))))

(ert-deftest glitnir-nhmac/layout-leg-ends-plain ()
  "After its duration the leg's layers are gone and the new layout shows as drawn: the root with the leg ended
equals the root drawn at once without one (tolerance 0)."
  :tags '(draws)
  (let ((glitnir-reduced-motion nil) plain moved)
    (glitnir-nhmac--with-pattern
     (lambda ()
       (let ((area (glitnir-nhmac--leg-area)))
         (glitnir-nhmac--leg-scroll)
         (setq plain (glitnir-nhmac--grab "leg-plain"))
         (set-window-start nil 1)
         (redisplay t)
         (glitnir-nhmac--leg-capture)
         (set-window-parameter nil 'glitnir-layout-motion
                               `(:moves ((page ,area ,(list (nth 0 area) (+ 12 (nth 1 area)) (nth 2 area) (nth 3 area))))
                                 :duration 1.0 :background "#000000"))
         (glitnir-nhmac--leg-scroll)
         (should (> (nth 2 (window-parameter nil 'glitnir--leg-sent)) 0))
         ;; the leg's duration and a margin, the wait turning the run loop
         (nhmac-gui--idle-until #'ignore 1.6)
         (setq moved (glitnir-nhmac--grab "leg-moved"))
         (glitnir-nhmac--leg-end)
         (let ((d (glitnir-nhmac--diff plain moved (glitnir-nhmac--root-rect plain))))
           (nhmac-gui--say "glitnir-nhmac/layout-leg-ends-plain: %S, legs %S" d (glitnir-nhmac--legs))
           (should (equal d '(0 0)))))))))

(ert-deftest glitnir-nhmac/layout-leg-jumps-under-reduced-motion ()
  "Reduced motion asks for no capture: `glitnir-layout-captured-p' is nil and Lisp sent no leg."
  :tags '(draws)
  (let ((glitnir-reduced-motion t))
    (glitnir-nhmac--with-pattern
     (lambda ()
       (unwind-protect
           (progn
             (set-window-parameter nil 'glitnir-layout-capture t)
             (redisplay t)
             (should-not (glitnir-layout-captured-p))
             (should-not (> (or (nth 2 (window-parameter nil 'glitnir--leg-sent)) 0) 0)))
         (set-window-parameter nil 'glitnir-layout-capture nil)
         (redisplay t))))))

;;; glitnir-nhmac-gui-tests.el ends here
