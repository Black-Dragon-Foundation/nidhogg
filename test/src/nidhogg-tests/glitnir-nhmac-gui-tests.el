;;; glitnir-nhmac-gui-tests.el --- Glitnir's child chrome on Naglfar  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh on the macOS runner, with nhmac-gui-tests.el (its helpers).  Glitnir P5's NS rows,
;; rebuilt on Naglfar's layers (design §5.7): the corners and the border are the content layer's properties, the
;; shadow a nine-slice image on a sublayer of the child's container, below its content.  The state rows read the
;; layers back in process (`nhmac--test-child'); the pixel rows capture the screen and find the child by its colour,
;; so they need no window coordinates.

(require 'ert)
(require 'cl-lib)
(unless (fboundp 'nhmac-gui--child)
  (load (expand-file-name "nhmac-gui-tests" (file-name-directory (or load-file-name buffer-file-name))) nil t))

(defconst glitnir-nhmac--shadow '(:blur 16 :offset (0 . 6) :spread 2 :opacity 0.35)
  "The shadow the chrome rows give their child frames (P5's).")

(defconst glitnir-nhmac--bg "#ffffff" "The root's background in the pixel rows.")

(defun glitnir-nhmac--chromed (&optional params left top)
  "A shown child frame (`nhmac-gui--child', #ff00ff) of 30x8 characters with corners of 12 and
`glitnir-nhmac--shadow', then PARAMS, at LEFT, TOP (default 80, 80)."
  (nhmac-gui--child (append params `((corner-radius . 12) (shadow . ,glitnir-nhmac--shadow) (width . 30) (height . 8)))
                    (or left 80) (or top 80)))

(defun glitnir-nhmac--scale ()
  "The device pixels per point of the selected frame's window."
  (plist-get (nhmac--test-surface 0 0) :scale))

(defun glitnir-nhmac--with-white (fn)
  "Call FN with the selected frame's background `glitnir-nhmac--bg', then as before."
  (let* ((f (selected-frame)) (bg (frame-parameter f 'background-color)))
    (set-frame-parameter f 'background-color glitnir-nhmac--bg)
    (unwind-protect (progn (nhmac-gui--settle) (funcall fn))
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
while its shadow layer's frame follows each size."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((c (glitnir-nhmac--chromed)))
    (unwind-protect
        (let ((before (nhmac-stats)) (frames nil))
          (dolist (w '(32 36 40 44 48))
            (set-frame-size c w 10)
            (nhmac-gui--settle)
            (push (nth 2 (plist-get (plist-get (nhmac--test-child c) :shadow) :frame)) frames))
          (nhmac-gui--say "nhmac-resize-shadow-renders: renders %d, shadow widths %S"
                          (nhmac-gui--delta before :shadow-renders) (reverse frames))
          (should (= (nhmac-gui--delta before :shadow-renders) 0))
          (should (equal frames (sort (copy-sequence frames) #'>)))
          (should (= (length (delete-dups (copy-sequence frames))) 5)))
      (delete-frame c t))))
