;;; nhmac-driver-gui-tests.el --- Naglfar's font driver, images and drawing rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh on the macOS runner (nhmac-lane.txt).  Each row judges Naglfar against Core
;; Text's or AppKit's own answers (asked through `test-font-probe') or against a stated expected value, never against
;; the NS port.  Self-contained: the lane may run this file alone.

(require 'ert)
(require 'cl-lib)

(defun nhmac-driver--say (fmt &rest args)
  "Print FMT with ARGS as a line of the lane's log (stderr), not to *Messages*."
  (princ (concat (apply #'format fmt args) "\n") #'external-debugging-output))

(defun nhmac-driver--naglfar-p ()
  "Whether this Emacs is Naglfar (the rows test its driver)."
  (featurep 'nidhogg-macos))

(defun nhmac-driver--probe (&rest args)
  "Core Text's or AppKit's answer to ARGS (`test-font-probe', nidhogg/shell/src/macos/font_probe.rs)."
  (apply #'nhmac--query 'test-font-probe args))

;;; Fonts

(ert-deftest nhmac-font-lang-picks-the-language-s-family ()
  "A script spec's first font follows its `:lang' as Core Text's own cascade does: `list-fonts' of (:script S :lang
L) starts with the family Core Text lays S's representative characters out in for L, for kana and han in Japanese,
Simplified Chinese and Korean (`mac_font_create_preferred_family_for_attributes' lays them out with the spec's
first language)."
  (skip-unless (nhmac-driver--naglfar-p))
  (let ((asked 0))
    (dolist (script '(kana han))
      (let ((chars (append (cdr (assq script script-representative-chars)) nil)))
        (dolist (lang '(ja zh-Hans ko))
          (let* ((want (apply #'nhmac-driver--probe 'preferred (symbol-name lang) chars))
                 (first (car (list-fonts (font-spec :script script :lang lang))))
                 (got (and first (symbol-name (font-get first :family)))))
            (nhmac-driver--say "nhmac-font-lang: %s %s: Core Text %S, listed first %S" script lang want got)
            ;; Core Text has no one family for characters it lays out in several fonts: nothing to move first
            (when want
              (cl-incf asked)
              (should (equal got want)))))))
    ;; kana is one character, so Core Text always names its family
    (should (>= asked 3))))

(defconst nhmac-driver--probe-fonts
  '(("/System/Library/Fonts/Supplemental/Andale Mono.ttf" "Andale Mono" "Nhmac Probe")
    ("/System/Library/Fonts/Supplemental/Courier New.ttf" "Courier New" "Nhmac Probe"))
  "System fonts copied under a family no installed font has: (FILE FAMILY NEW), NEW as long as FAMILY.")

(defun nhmac-driver--renamed-font (file family new)
  "A copy of the font FILE in a temporary file with FAMILY renamed NEW (the same length) in its name table, in
ASCII and UTF-16BE, and its PostScript name (FAMILY without spaces) with it; the copy's name."
  (let ((out (make-temp-file "nhmac-probe" nil ".ttf"))
        (pairs (list (cons family new)
                     (cons (string-replace " " "" family) (string-replace " " "" new)))))
    (with-temp-buffer
      (set-buffer-multibyte nil)
      (insert-file-contents-literally file)
      (dolist (p pairs)
        (dolist (enc (list #'identity
                           (lambda (s) (mapconcat (lambda (c) (unibyte-string 0 c)) s ""))))
          (goto-char (point-min))
          (let ((from (funcall enc (car p))) (to (funcall enc (cdr p))))
            (while (search-forward from nil t)
              (replace-match to t t)))))
      (let ((coding-system-for-write 'no-conversion))
        (write-region nil nil out nil 'silent)))
    out))

(ert-deftest nhmac-font-list-sees-a-font-installed-mid-session ()
  "A font registered while Emacs runs is listed without a restart: after Core Text's registered-fonts notification
the driver lists its faces again (macfont drops its family cache then), so an unnamed `list-fonts' finds it once
`clear-font-cache' drops Emacs's own lists."
  (skip-unless (nhmac-driver--naglfar-p))
  (let* ((src (seq-find (lambda (f) (file-exists-p (car f))) nhmac-driver--probe-fonts))
         (new (nth 2 src))
         (spec (font-spec :spacing 'm :registry 'iso10646-1))
         (listed (lambda ()
                   (clear-font-cache)
                   (cl-some (lambda (e) (equal (symbol-name (font-get e :family)) new)) (list-fonts spec)))))
    (should src)
    (should-not (funcall listed))
    (let ((file (apply #'nhmac-driver--renamed-font src)))
      (unwind-protect
          (progn
            (should (eq (nhmac-driver--probe 'register file) t))
            (let ((deadline (+ (float-time) 5)) found)
              ;; the notification reaches the shell through the run loop
              (while (and (not (setq found (funcall listed))) (< (float-time) deadline))
                (sit-for 0.1))
              (nhmac-driver--say "nhmac-font-list-refresh: %s listed %s, %.1f s left; (notifications generation stale) %S"
                                 new (if found "yes" "no") (- deadline (float-time))
                                 (nhmac-driver--probe 'font-changes))
              (should found)))
        (ignore-errors (delete-file file))))))

(ert-deftest nhmac-font-ultralight-weights-follow-appkit ()
  "Every listed face whose Core Text weight trait is -0.40 (ultra-light) is listed regular when AppKit weighs it 5,
its regular, and ultra-light otherwise (`mac_font_descriptor_get_adjusted_weight'); the answers are Core Text's
traits and AppKit's `weightOfFont:', asked of each face."
  (skip-unless (nhmac-driver--naglfar-p))
  (let ((faces (nhmac-driver--probe 'ultralight))
        (regular (font-get (font-spec :weight 'normal) :weight))
        (light (font-get (font-spec :weight 'ultra-light) :weight)))
    (nhmac-driver--say "nhmac-font-ultralight: %d faces at -0.40: %S" (length faces) faces)
    (dolist (f faces)
      (pcase-let* ((`(,id ,family ,ps ,appkit) f)
                   (entity (cl-find-if (lambda (e) (equal (car (font-get e :font-entity)) id))
                                       (list-fonts (font-spec :family family))))
                   (want (if (eql appkit 5) regular light)))
        (nhmac-driver--say "nhmac-font-ultralight: %s (AppKit %S): listed %S, want %S"
                           ps appkit (and entity (font-get entity :weight)) want)
        (should entity)
        (should (eq (font-get entity :weight) want))))))

(defconst nhmac-driver--sequences
  '(("Hiragino Mincho ProN" #x845B (#xE0100 #xE0101) t)
    ("Hiragino Sans" #x8FBB (#xE0100 #xE0101) nil)
    ("Apple Color Emoji" #x2764 (#xFE0E #xFE0F) nil)
    ("Apple Color Emoji" #x263A (#xFE0E #xFE0F) nil))
  "Variation sequences: (FAMILY BASE SELECTORS MUST), MUST when FAMILY has to have the first sequence (Hiragino's
Adobe-Japan1 ideographic variation sequences).")

(ert-deftest nhmac-font-variation-glyphs ()
  "`font-variation-glyphs' reads the font's own sequences (its cmap format-14 table, macfont_variation_glyphs): for an
ideographic variation sequence in Hiragino and VS15/VS16 on emoji, a sequence the driver lists has the glyph Core
Text lays BASE + SELECTOR out with in that font, and one it does not list is laid out with the base glyph."
  (skip-unless (nhmac-driver--naglfar-p))
  (let ((size 24) (checked 0))
    (pcase-dolist (`(,family ,base ,selectors ,must) nhmac-driver--sequences)
      (let* ((entity (car (list-fonts (font-spec :family family))))
             (font (and entity (open-font entity size)))
             (have (and font (font-variation-glyphs font base)))
             (plain (and font (aref (aref (font-get-glyphs font 0 1 (string base)) 0) 3))))
        (nhmac-driver--say "nhmac-font-variation: %s %X: %S (base glyph %S)" family base have plain)
        (when font
          (dolist (vs selectors)
            (let ((ct (nhmac-driver--probe 'variation family size base vs))
                  (mine (cdr (assq vs have))))
              (nhmac-driver--say "nhmac-font-variation: %s %X %X: Core Text %S, driver %S" family base vs ct mine)
              (when ct
                (cl-incf checked)
                (should (eql ct (or mine plain))))))
          (when must
            (should (assq (car selectors) have))))))
    (should (> checked 0))))

;;; Images

(defun nhmac-driver--pixels (object points)
  "Show OBJECT (an image or a propertized string) at the start of a fresh window-filling buffer and return the
committed surface's premultiplied ARGB at each of POINTS, (DX . DY) device pixels from where OBJECT starts.  POINTS
may be a function: called with where OBJECT starts, (X . Y) in the frame's device pixels, it returns the frame points."
  (let ((buf (generate-new-buffer " *nhmac-driver*")))
    (unwind-protect
        (progn
          (delete-other-windows)
          (switch-to-buffer buf)
          (setq-local cursor-type nil)
          (if (stringp object) (insert object) (insert-image object))
          (goto-char (point-max))
          (redisplay t)
          ;; the commit lands after redisplay: a moment for the pacer
          (sleep-for 0.2)
          (redisplay t)
          (let* ((xy (posn-x-y (posn-at-point (point-min))))
                 (edges (window-inside-pixel-edges))
                 (origin (cons (+ (nth 0 edges) (car xy)) (+ (nth 1 edges) (cdr xy)))))
            (mapcar (lambda (p) (plist-get (nhmac--test-surface (car p) (cdr p)) :pixel))
                    (if (functionp points)
                        (funcall points origin)
                      (mapcar (lambda (p) (cons (+ (car origin) (car p)) (+ (cdr origin) (cdr p)))) points)))))
      (kill-buffer buf))))

(defun nhmac-driver--rgb (name)
  "The opaque ARGB word of the colour NAME."
  (pcase-let ((`(,r ,g ,b) (color-values name)))
    (logior #xff000000 (ash (ash r -8) 16) (ash (ash g -8) 8) (ash b -8))))

(ert-deftest nhmac-xpm-color-symbols ()
  "An XPM's `:color-symbols' work as `xpm_load_image' reads them: a line's `s' symbol found there wins over its `c'
colour, `None' there is transparent (the background shows), and a symbol not there leaves the `c' colour."
  (skip-unless (nhmac-driver--naglfar-p))
  (let* ((xpm (concat "/* XPM */\nstatic char *x[] = {\n\"24 8 3 1\",\n\"a c red s fg\",\n\"b c blue s mask\",\n"
                      "\"c c #0000ff s other\",\n"
                      (mapconcat (lambda (_) "\"aaaaaaaabbbbbbbbcccccccc\"") (number-sequence 1 8) ",\n")
                      "};"))
         (img (create-image xpm 'xpm t :scale 1
                            :color-symbols '(("fg" . "#00ff00") ("mask" . "None")))))
    (should (imagep img))
    (let* ((size (image-size img t))
           (third (/ (car size) 3))
           (mid (/ (cdr size) 2))
           (px (nhmac-driver--pixels img (list (cons (/ third 2) mid) (cons (+ third (/ third 2)) mid)
                                               (cons (+ third third (/ third 2)) mid)))))
      (nhmac-driver--say "nhmac-xpm-color-symbols: size %S, pixels %S, background %s" size
                         (mapcar (lambda (p) (format "#%08x" p)) px) (face-background 'default))
      ;; 24 x 8 at scale 1, more where the display scales images
      (should (and (>= (car size) 24) (= (* 3 (cdr size)) (car size))))
      (should (= (nth 0 px) (nhmac-driver--rgb "#00ff00")))
      (should (= (nth 1 px) (nhmac-driver--rgb (face-background 'default))))
      (should (= (nth 2 px) (nhmac-driver--rgb "#0000ff"))))))

(ert-deftest nhmac-svg-root-colour-wins ()
  "`currentColor' is the foreground, inherited as librsvg's wrapper makes it: an SVG whose root sets `color' draws in
that colour, one that sets none in the image's `:foreground'."
  (skip-unless (and (nhmac-driver--naglfar-p) (image-type-available-p 'svg)))
  (let* ((svg (lambda (root)
                (format "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\"%s>\
<rect width=\"16\" height=\"16\" fill=\"currentColor\"/></svg>" root)))
         (own (nhmac-driver--pixels (create-image (funcall svg " color=\"#00ff00\"") 'svg t :foreground "#ff0000")
                                    '((8 . 8))))
         (fg (nhmac-driver--pixels (create-image (funcall svg "") 'svg t :foreground "#ff0000") '((8 . 8)))))
    (nhmac-driver--say "nhmac-svg-root-colour: own #%08x, foreground #%08x" (car own) (car fg))
    (should (= (car own) (nhmac-driver--rgb "#00ff00")))
    (should (= (car fg) (nhmac-driver--rgb "#ff0000")))))

;;; Drawing

(ert-deftest nhmac-stipple-is-drawn ()
  "A `:stipple' face draws its bitmap in the face's foreground over its background where the bits are set, tiled
from the frame's origin as X and NS tile it, each bit a logical pixel (the frame's whole scale)."
  (skip-unless (nhmac-driver--naglfar-p))
  (let* ((k (max 1 (floor (frame-scale-factor))))
         ;; a 2 x 2 checkerboard: row 0 its left bit, row 1 its right
         (face '(:stipple (2 2 "\1\2") :foreground "#ff0000" :background "#0000ff"))
         (pts nil)
         (px (nhmac-driver--pixels (propertize "        " 'face face)
                                   (lambda (o)
                                     (setq pts (cl-loop for dx below (* 4 k) nconc
                                                        (cl-loop for dy below (* 4 k)
                                                                 collect (cons (+ (car o) (* 2 k) dx)
                                                                               (+ (cdr o) dy)))))
                                     pts)))
         (want (mapcar (lambda (p)
                         (nhmac-driver--rgb (if (= (% (/ (car p) k) 2) (% (/ (cdr p) k) 2)) "#ff0000" "#0000ff")))
                       pts))
         (bad (cl-loop for p in pts for g in px for w in want unless (= g w) collect (list p (format "#%08x" g)))))
    (nhmac-driver--say "nhmac-stipple: scale %s, %d of %d pixels differ, first %S" k (length bad) (length pts)
                       (car bad))
    (should-not bad)))

(ert-deftest nhmac-one-pixel-lines-are-points ()
  "The drawing's one-pixel constants are points, as on NS: a `:box 1' face adds one point a side, an overline is one
point thick, and the default bar cursor (2) two points wide, so twice the device pixels at scale 2."
  (skip-unless (nhmac-driver--naglfar-p))
  (let ((k (max 1 (round (frame-scale-factor)))))
    ;; the box: in the layout
    (with-temp-buffer
      (set-window-buffer nil (current-buffer))
      ;; whole lines: the box's right line goes on the glyph after the run
      (insert "xxx\nx" (propertize "x" 'face '(:box 1)) "x\n")
      (let ((side (/ (- (car (window-text-pixel-size nil 5 8)) (car (window-text-pixel-size nil 1 4))) 2)))
        (nhmac-driver--say "nhmac-one-pixel: scale %s, box side %d" k side)
        (should (= side k))))
    ;; the overline: K rows of its colour, then the background
    (let ((px (nhmac-driver--pixels (propertize "    " 'face '(:overline "#ff0000" :background "#0000ff"))
                                    (list (cons 4 (1- k)) (cons 4 k)))))
      (nhmac-driver--say "nhmac-one-pixel: overline rows %d and %d: %S" (1- k) k
                         (mapcar (lambda (p) (format "#%08x" p)) px))
      (should (= (nth 0 px) (nhmac-driver--rgb "#ff0000")))
      (should (= (nth 1 px) (nhmac-driver--rgb "#0000ff"))))
    ;; the bar cursor: 2K columns of the cursor's colour at point
    (let ((buf (generate-new-buffer " *nhmac-driver-cursor*"))
          (blink (bound-and-true-p blink-cursor-mode)))
      (unwind-protect
          (progn
            (blink-cursor-mode -1)
            (delete-other-windows)
            (switch-to-buffer buf)
            (setq-local cursor-type 'bar)
            (insert (propertize "    " 'face '(:background "#0000ff")))
            (goto-char (point-min))
            (redisplay t) (sleep-for 0.2) (redisplay t)
            (let* ((xy (posn-x-y (posn-at-point (point))))
                   (e (window-inside-pixel-edges))
                   (x (+ (nth 0 e) (car xy)))
                   (y (+ (nth 1 e) (cdr xy) (/ (line-pixel-height) 2)))
                   (in (plist-get (nhmac--test-surface (+ x (* 2 k) -1) y) :pixel))
                   (out (plist-get (nhmac--test-surface (+ x (* 2 k)) y) :pixel)))
              (nhmac-driver--say "nhmac-one-pixel: bar columns %d and %d: #%08x #%08x, cursor %s" (1- (* 2 k)) (* 2 k)
                                 in out (frame-parameter nil 'cursor-color))
              (should (= in (nhmac-driver--rgb (frame-parameter nil 'cursor-color))))
              (should (= out (nhmac-driver--rgb "#0000ff")))))
        (kill-buffer buf)
        (when blink (blink-cursor-mode 1))))))

(provide 'nhmac-driver-gui-tests)
;;; nhmac-driver-gui-tests.el ends here
