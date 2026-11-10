;;; glitnir-decor-gui-tests.el --- GUI tests for Glitnir decorations (PD)  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/gui.sh (gui-runner.el loads every *-gui-tests.el).  From the pgtk rows of the same
;; decorations, taken over for the native Wayland shell: the rows run only there.  The shell's frame pixels are device
;; pixels, so a frame pixel is one pixel of the grab whatever the monitor scale (`glitnir-decor-gui--scale').
;; No row's assertion is about GDK or GTK, so none is dropped.

(require 'ert)
(require 'cl-lib)

(defun glitnir-decor-gui--scale ()
  "Device pixels per frame pixel: 1 on the shell, whose frame pixels are the device's."
  1)

(defun glitnir-decor-gui--grab (box &optional enable)
  "Pixels of a line `[ab]' in a face with BOX on a white-on-black buffer: (GRAB . (X . Y) of the run's start).
X and Y are device pixels of GRAB (see `glitnir-decor-gui--at')."
  (let ((glitnir-enable (if (eq enable 'off) nil t))
        (face (make-symbol "glitnir-decor-gui-face")))
    (custom-declare-face face nil "Test face.")
    (set-face-attribute face nil :background "#303060" :foreground "white" :box box)
    (with-temp-buffer
      (switch-to-buffer (current-buffer))
      (setq-local cursor-type nil)      ; beside the run it would blend into the run's first column at a fractional scale
      (face-remap-add-relative 'default :background "#202020" :foreground "white")
      (insert "\n\n  " (propertize " ab " 'face face) "  \n\n")
      (goto-char 4)
      (gui-runner-wait-settled)
      (glitnir-decor-gui--at (gui-runner-screen-grab) 5))))

(defun glitnir-decor-gui--dev (s v)
  "V logical pixels at scale S in device pixels, halves rounded up (as Glitnir's grid does; `round' goes to even)."
  (floor (+ (* s v) 0.5)))

(defun glitnir-decor-gui--calibrate ()
  "The device pixel of a grab at the frame's top-left, found with the selected window's text area empty and the
cursor off (so the first block of the buffer's background #202020 is the text area's corner)."
  (gui-runner-wait-settled)
  (let* ((s (glitnir-decor-gui--scale))
         (o (gui-runner-grab-find-block (gui-runner-screen-grab) '(32 32 32) 4 4))
         (e (window-body-pixel-edges)))
    (unless o (error "glitnir-decor-gui--calibrate: no text area in the grab"))
    (cons (- (car o) (glitnir-decor-gui--dev s (nth 0 e))) (- (cdr o) (glitnir-decor-gui--dev s (nth 1 e))))))

(defun glitnir-decor-gui--at (grab pos &optional frame-origin)
  "(GRAB . (X . Y)): the device pixel of GRAB at the top-left of POS's glyph row cell.
`window-absolute-pixel-position' cannot be used: the compositor places the window, and the title bar and margins
the shell draws lie between the grab's corner and the Emacs area.  So the window's text area is found in GRAB as the
first block of the buffer's background \(its first line is empty), and POS's text-area position is added.  With FRAME-ORIGIN (from
`glitnir-decor-gui--calibrate': for a buffer whose first line is not empty, or whose background changes) the
window's body edges and POS's position are added to that."
  (let* ((s (glitnir-decor-gui--scale))
         (o (and (not frame-origin) (gui-runner-grab-find-block grab '(32 32 32) 4 4)))
         (e (window-body-pixel-edges))
         (p (pos-visible-in-window-p pos nil t)))
    (unless (and (or o frame-origin) p)
      (error "glitnir-decor-gui--at: grab %dx%d text area %S, position %S" (car grab) (cadr grab) o p))
    (let ((r (if frame-origin
                 (cons (+ (car frame-origin) (glitnir-decor-gui--dev s (+ (nth 0 e) (nth 0 p))))
                       (+ (cdr frame-origin) (glitnir-decor-gui--dev s (+ (nth 1 e) (nth 1 p)))))
               (cons (+ (car o) (glitnir-decor-gui--dev s (nth 0 p))) (+ (cdr o) (glitnir-decor-gui--dev s (nth 1 p)))))))
      (glitnir-gui--say "GLITNIR-DECOR-GUI-AT s=%s grab=%dx%d origin=%S body=%S pos=%S -> %S pixel %S"
                        s (car grab) (cadr grab) (or frame-origin o) e p r
                        (glitnir-decor-gui--px grab (car r) (cdr r)))
      (cons grab r))))

(defun glitnir-decor-gui--px (grab x y)
  "The (R G B) of GRAB at X, Y."
  (let* ((w (nth 0 grab)) (bytes (nth 2 grab)) (i (* 4 (+ x (* y w)))))
    (list (aref bytes (+ i 2)) (aref bytes (+ i 1)) (aref bytes i))))

(defconst glitnir-decor-gui--box '(:line-width 2 :color "#ff8000" :corner-radius 8))

(ert-deftest glitnir-decor-gui/decor-corner-is-colour-beneath ()
  (skip-unless (featurep 'nidhogg-wayland))
  (let* ((r (glitnir-decor-gui--grab glitnir-decor-gui--box))
         (g (car r)) (at (cdr r)))
    ;; The run's top-left device pixel is outside the arc: the buffer background, not the face's or the ring's.
    (should (equal (glitnir-decor-gui--px g (car at) (cdr at)) '(32 32 32)))))

(ert-deftest glitnir-decor-gui/decor-interior-equals-radius-0 ()
  (skip-unless (featurep 'nidhogg-wayland))
  (let* ((a (glitnir-decor-gui--grab glitnir-decor-gui--box))
         (b (glitnir-decor-gui--grab '(:line-width 2 :color "#ff8000")))
         (x (+ (car (cdr a)) 20)) (y (+ (cdr (cdr a)) 8)))
    (should (equal (glitnir-decor-gui--px (car a) x y) (glitnir-decor-gui--px (car b) x y)))))

(ert-deftest glitnir-decor-gui/decor-arc-has-ring-colour ()
  (skip-unless (featurep 'nidhogg-wayland))
  (let* ((r (glitnir-decor-gui--grab glitnir-decor-gui--box))
         (g (car r)) (at (cdr r)) (found nil)
         (s (glitnir-decor-gui--scale))
         (n (round (* 8 s))) (lw (* 2 s)))
    ;; In the top-left corner's radius square, some pixel whose centre lies in the ring band (between the inner
    ;; radius N - LW and the outer radius N from the arcs' centre (N, N), +-0.5 px) is wholly the ring's colour.
    ;; (Not on the exact diagonal: at scale 1 the 2 px ring between radii 6 and 8 holds no whole diagonal pixel,
    ;; whose centres lie 6.36 and 7.78 px from the arcs' centre.)
    (dotimes (i n)
      (dotimes (j n)
        (let ((d (sqrt (+ (expt (- (+ i 0.5) n) 2) (expt (- (+ j 0.5) n) 2)))))
          (when (and (<= (- n lw 0.5) d (+ n 0.5))
                     (equal (glitnir-decor-gui--px g (+ (car at) i) (+ (cdr at) j)) '(255 128 0)))
            (setq found t)))))
    (should found)
    ;; The square's outermost corner pixel lies outside the arc: not the ring's colour (a square corner would be).
    (should-not (equal (glitnir-decor-gui--px g (car at) (cdr at)) '(255 128 0)))))

(ert-deftest glitnir-decor-gui/decor-open-sides-undrawn ()
  (skip-unless (featurep 'nidhogg-wayland))
  (let* ((r (glitnir-decor-gui--grab '(:line-width 2 :color "#ff8000" :corner-radius 8 :open (top))))
         (g (car r)) (at (cdr r)))
    ;; Top open: the first row of the run carries the face background, not the ring colour.
    (should (equal (glitnir-decor-gui--px g (+ (car at) 20) (cdr at)) '(48 48 96)))))

(ert-deftest glitnir-decor-gui/decor-off-is-stock-box ()
  (skip-unless (featurep 'nidhogg-wayland))
  (let* ((on (glitnir-decor-gui--grab '(:line-width 2 :color "#ff8000") 'off))
         (off (glitnir-decor-gui--grab '(:line-width 2 :color "#ff8000" :corner-radius 8) 'off))
         (at (cdr on)))
    ;; `glitnir-enable' nil: the keys change nothing.
    (dotimes (i 6)
      (should (equal (glitnir-decor-gui--px (car on) (+ (car at) i) (+ (cdr at) i))
                     (glitnir-decor-gui--px (car off) (+ (car at) i) (+ (cdr at) i)))))))

(ert-deftest glitnir-decor-gui/decor-no-keys-pixel-identical ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; A box without the keys takes the stock path: the ring's top-left pixel is the ring colour at once.
  (let* ((r (glitnir-decor-gui--grab '(:line-width 2 :color "#ff8000")))
         (at (cdr r)))
    (should (equal (glitnir-decor-gui--px (car r) (car at) (cdr at)) '(255 128 0)))))

;; The neighbour corner repaint: a chip's rounded ends stay right when the glyph string beside it is redrawn alone.

(defun glitnir-decor-gui--chip (before edit &optional face-props after)
  "Draw BEFORE then a rounded chip \" ab \"; call EDIT with the start of BEFORE's end (a marker) to redraw
only the neighbour; return (TOP-LEFT-PX TOP-RIGHT-PX FULL-GRAB FRAME-ORIGIN AFTER-VALUE) with pixels of the
chip's corners.  AFTER, called last with the chip's buffer still shown, makes AFTER-VALUE."
  (let ((glitnir-enable t)
        (face (make-symbol "glitnir-decor-gui-chip")))
    (custom-declare-face face nil "Test face.")
    (apply #'set-face-attribute face nil :background "#303060" :foreground "white"
           :box glitnir-decor-gui--box face-props)
    (with-temp-buffer
      (switch-to-buffer (current-buffer))
      (face-remap-add-relative 'default :background "#202020" :foreground "white")
      (setq-local cursor-type nil)      ; the cursor must not hide the text area's corner from the origin search
      (let* ((fo (glitnir-decor-gui--calibrate))) ; with the first line empty, before any edit changes the background
        (insert "\n\n  " before (propertize " ab " 'face face) "  \n\n")
        (let* ((chip (+ 5 (length before))) (m (copy-marker chip)))
        (goto-char 1)
        (gui-runner-wait-settled)
        (glitnir-stats nil t)
        (funcall edit m)
        (gui-runner-wait-settled)
        (let* ((g (gui-runner-screen-grab))
               (at (cdr (glitnir-decor-gui--at g chip fo)))
               (right (cdr (glitnir-decor-gui--at g (+ chip 4) fo))))
          (list (glitnir-decor-gui--px g (car at) (cdr at))
                (glitnir-decor-gui--px g (1- (car right)) (cdr at))
                g fo (and after (funcall after)))))))))

(ert-deftest glitnir-decor-gui/decor-repaint-before-draw-survives-overhang ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; The chip's left neighbour is redrawn alone, as plain text and as an italic glyph overhanging into the chip: the chip's
  ;; top-left corner stays the colour beneath.
  (dolist (italic '(nil t))
    (let ((r (glitnir-decor-gui--chip
              (if italic "ff" "xy")
              (lambda (m) (save-excursion (goto-char (- m 1)) (delete-char -1) (insert "z")
                                          (when italic
                                            (put-text-property (1- (point)) (point) 'face '(:slant italic))))))))
      (should (equal (nth 0 r) '(32 32 32))))))

(ert-deftest glitnir-decor-gui/decor-region-edge-repaints-corner ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; A region that ends next to the chip changes the neighbour's background: the corner shows the new colour beneath.
  (let ((r (glitnir-decor-gui--chip
            "xy"
            (lambda (m)
              (face-remap-add-relative 'region :background "#804040")
              (set-mark (- m 2)) (goto-char m) (activate-mark)
              (let ((transient-mark-mode t)) (force-mode-line-update))))))
    (should (equal (nth 0 r) '(128 64 64)))))

(ert-deftest glitnir-decor-gui/decor-mouse-face-neighbour ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; The neighbour takes `mouse-face' (redrawn alone): the chip's corner follows its background.
  (let ((r (glitnir-decor-gui--chip
            "xy"
            (lambda (m)
              (put-text-property (- m 2) m 'mouse-face '(:background "#006000"))
              ;; frame pixels (logical): the body's corner plus the neighbour's position in it
              (let ((e (window-body-pixel-edges)) (p (pos-visible-in-window-p (- m 2) nil t)))
                (set-mouse-pixel-position (selected-frame) (+ (nth 0 e) (nth 0 p) 1) (+ (nth 1 e) (nth 1 p) 1)))
              (redisplay t)))))
    (should (member (nth 0 r) '((0 96 0) (32 32 32))))))

(ert-deftest glitnir-decor-gui/decor-theme-switch-recolours-corners ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; A background change of the default face (a theme switch) repaints every corner in the new colour beneath.
  (let ((r (glitnir-decor-gui--chip
            "xy"
            (lambda (_m)
              (face-remap-add-relative 'default :background "#103010" :foreground "white")))))
    (should (equal (nth 0 r) '(16 48 16)))
    (should (equal (nth 1 r) '(16 48 16)))))

(ert-deftest glitnir-decor-gui/decor-corners-at-the-row-edges ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; A rounded run that is its row's first glyph has nothing to its left, one that is its row's last nothing to its
  ;; right: their corners there are what the row shows past its glyphs (the window's background), not the far
  ;; neighbour's colour (the run's own fill when the run is split into several glyph strings: a button's pads are
  ;; stretches, its label characters).
  (let ((glitnir-enable t)
        (face (make-symbol "glitnir-decor-gui-edge")))
    (custom-declare-face face nil "Test face.")
    (set-face-attribute face nil :background "#303060" :foreground "white" :box glitnir-decor-gui--box)
    (with-temp-buffer
      (switch-to-buffer (current-buffer))
      (face-remap-add-relative 'default :background "#202020" :foreground "white")
      (setq-local cursor-type nil)
      (let ((fo (glitnir-decor-gui--calibrate))
            (pad (propertize " " 'face face 'display '(space :width (12)))))
        (insert "\n\n" pad (propertize "ab" 'face face) pad "\n\n")
        (goto-char 1)
        (gui-runner-wait-settled)
        (let* ((g (gui-runner-screen-grab))
               (left (cdr (glitnir-decor-gui--at g 3 fo)))
               (right (cdr (glitnir-decor-gui--at g 7 fo))))
          (should (equal (list 'left (glitnir-decor-gui--px g (car left) (cdr left))) '(left (32 32 32))))
          (should (equal (list 'right (glitnir-decor-gui--px g (1- (car right)) (cdr left))) '(right (32 32 32)))))))))

(ert-deftest glitnir-decor-gui/box-closes-at-the-line-end ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; A boxed row that ends its line in a stretch to the right edge (a list row: its text, then an overlay string that
  ;; fills to the edge, the newline in the same face so the run goes on into the string) draws the box's right side
  ;; at the edge: the run ends with its line, it cannot go on to the next.  The stretch fills the text area exactly, so
  ;; the newline overflows into the fringe and the stretch is the row's last glyph.
  (let ((glitnir-enable t)
        (face (make-symbol "glitnir-decor-gui-line-end")))
    (custom-declare-face face nil "Test face.")
    (set-face-attribute face nil :background "#303060" :foreground "white"
                        :box '(:line-width (-2 . -2) :color "#ff8000"))
    (with-temp-buffer
      (switch-to-buffer (current-buffer))
      (face-remap-add-relative 'default :background "#202020" :foreground "white")
      (setq-local cursor-type nil truncate-lines t)
      (let ((fo (glitnir-decor-gui--calibrate)))
        (insert "\n\n" (propertize "ab" 'face face) "\n\n")
        (let ((tail (make-overlay 3 5)) (nl (make-overlay 5 6)))
          (overlay-put tail 'after-string
                       (propertize " " 'face face 'display '(space :align-to right)))
          (overlay-put nl 'face face))
        (goto-char 1)
        (gui-runner-wait-settled)
        (let* ((s (glitnir-decor-gui--scale))
               (g (gui-runner-screen-grab))
               (row (cdr (glitnir-decor-gui--at g 3 fo)))
               (e (window-body-pixel-edges))
               (y (+ (cdr row) (glitnir-decor-gui--dev s (/ (line-pixel-height) 2))))
               ;; the text area's last pixel: the ring; two in (past the 2 px ring): the fill
               (x (+ (car fo) (glitnir-decor-gui--dev s (nth 2 e)) -1)))
          (should (equal (list 'ring (glitnir-decor-gui--px g x y)) '(ring (255 128 0))))
          (should (equal (list 'fill (glitnir-decor-gui--px g (- x (* 2 s)) y)) '(fill (48 48 96)))))))))

(defun glitnir-decor-gui--fit-column (fit)
  "On a row 16 px taller than its text (`line-spacing'), the ring's left side boxing \" ab \" with FIT as its :fit:
\(AT-TEXT . IN-SPACING), the pixels at the text's middle and 6 px above the row's bottom (in the line spacing, clear of the rounded
corner)."
  (let ((glitnir-enable t)
        (face (make-symbol "glitnir-decor-gui-fit")))
    (custom-declare-face face nil "Test face.")
    (set-face-attribute face nil :foreground "white"
                        :box `(:line-width (1 . 1) :color "#ff8000" :corner-radius 4 ,@(and fit `(:fit ,fit))))
    (with-temp-buffer
      (switch-to-buffer (current-buffer))
      (face-remap-add-relative 'default :background "#202020" :foreground "white")
      (setq-local cursor-type nil line-spacing 16)
      (let ((fo (glitnir-decor-gui--calibrate)))
        (insert "\n\n" (propertize " ab " 'face face) "\n\n")
        (goto-char 1)
        (gui-runner-wait-settled)
        (let* ((s (glitnir-decor-gui--scale))
               (g (gui-runner-screen-grab))
               (row (cdr (glitnir-decor-gui--at g 3 fo)))
               (x (car row))
               (text (save-excursion (goto-char 3) (- (line-pixel-height) 16))))
          (cons (glitnir-decor-gui--px g x (+ (cdr row) (glitnir-decor-gui--dev s (/ text 2))))
                (glitnir-decor-gui--px g x (+ (cdr row) (glitnir-decor-gui--dev s (- (+ text 16) 6))))))))))

(ert-deftest glitnir-decor-gui/a-box-fit-to-the-text-stays-off-the-line-spacing ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; A keycap on a list row made tall by `line-spacing' is a ring as tall as its text, not as the row: `:fit text'
  ;; boxes the font's ascent and descent; without it the ring runs down through the line spacing (stock).
  (pcase-let ((`(,at . ,below) (glitnir-decor-gui--fit-column 'text)))
    (should (equal (list 'ring at) '(ring (255 128 0))))
    (should (equal (list 'spacing below) '(spacing (32 32 32)))))
  (pcase-let ((`(,at . ,below) (glitnir-decor-gui--fit-column nil)))
    (should (equal (list 'ring at) '(ring (255 128 0))))
    (should (equal (list 'stock below) '(stock (255 128 0))))))

(ert-deftest glitnir-decor-gui/box-across-a-newline-is-stock ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; Buffer text in a boxed face that runs on over its newline (no string, no stretch) is drawn as stock Emacs draws
  ;; it: the run goes on past the last character onto the newline, so no side is drawn between them.
  (let ((glitnir-enable t)
        (face (make-symbol "glitnir-decor-gui-across")))
    (custom-declare-face face nil "Test face.")
    (set-face-attribute face nil :background "#303060" :foreground "white" :extend t
                        :box '(:line-width (-2 . -2) :color "#ff8000"))
    (with-temp-buffer
      (switch-to-buffer (current-buffer))
      (face-remap-add-relative 'default :background "#202020" :foreground "white")
      (setq-local cursor-type nil truncate-lines t)
      (let ((fo (glitnir-decor-gui--calibrate)))
        (insert "\n\n" (propertize "ab\n" 'face face) "\n")
        (goto-char 1)
        (gui-runner-wait-settled)
        (let* ((s (glitnir-decor-gui--scale))
               (g (gui-runner-screen-grab))
               (nl (cdr (glitnir-decor-gui--at g 5 fo)))
               (y (+ (cdr nl) (glitnir-decor-gui--dev s (/ (line-pixel-height) 2)))))
          ;; the last pixel of `b' and the first of the newline: the face's fill, no ring between them
          (should (equal (list 'b (glitnir-decor-gui--px g (1- (car nl)) y)) '(b (48 48 96))))
          (should (equal (list 'nl (glitnir-decor-gui--px g (car nl) y)) '(nl (48 48 96)))))))))

;; Two boxed faces side by side in one row, from the text and an overlay: the first is `ab' + `ef' around the
;; overlay's `cd'.  POSITIONS are their buffer positions (a at 3).
(defun glitnir-decor-gui--two-runs (outer inner)
  "Grab `abcdef' in face OUTER with `cd' in face INNER over it: (GRAB Y C-X E-X) for the row's middle, device pixels."
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (face-remap-add-relative 'default :background "#202020" :foreground "white")
    (setq-local cursor-type nil truncate-lines t)
    (let ((fo (glitnir-decor-gui--calibrate)))
      (insert "\n\n" (propertize "abcdef" 'face outer) "\n\n")
      (overlay-put (make-overlay 5 7) 'face inner)
      (goto-char 1)
      (gui-runner-wait-settled)
      (let* ((s (glitnir-decor-gui--scale)) (g (gui-runner-screen-grab))
             (c (cdr (glitnir-decor-gui--at g 5 fo))) (e (cdr (glitnir-decor-gui--at g 7 fo))))
        (list g (+ (cdr c) (glitnir-decor-gui--dev s (/ (line-pixel-height) 2))) (car c) (car e))))))

(defun glitnir-decor-gui--box-face (name bg colour)
  "A face NAME with background BG and a 2 px box, inside the glyphs, of COLOUR."
  (let ((face (make-symbol name)))
    (custom-declare-face face nil "Test face.")
    (set-face-attribute face nil :background bg :foreground "white" :box `(:line-width (-2 . -2) :color ,colour))
    face))

(ert-deftest glitnir-decor-gui/box-nested-in-another-box-has-sides ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; A boxed face over part of a run of another boxed face whose box differs (a search match in the selection, the
  ;; current match's ring in it too): the inner run starts and ends with its own sides, so its ring is whole.  Stock
  ;; Emacs starts a run only where the face before has no box, so it drew the inner box's top and bottom alone.
  (let* ((glitnir-enable t)
         (r (glitnir-decor-gui--two-runs (glitnir-decor-gui--box-face "outer" "#303060" "#303060")
                                         (glitnir-decor-gui--box-face "inner" "#306030" "#ff8000"))))
    (pcase-let ((`(,g ,y ,cx ,ex) r))
      (should (equal (list 'left (glitnir-decor-gui--px g cx y)) '(left (255 128 0))))
      (should (equal (list 'right (glitnir-decor-gui--px g (1- ex) y)) '(right (255 128 0)))))))

;; The same in an overlay string (a completion list's current row with a keycap in it): `ab' + `ef' in OUTER around
;; `cd' in INNER, all in one after-string after the buffer's `x' (at 3).
(defun glitnir-decor-gui--two-runs-in-a-string (outer inner)
  "Grab `x' then an after-string `abcdef' in face OUTER with `cd' in face INNER: (GRAB Y C-X E-X), device pixels."
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (face-remap-add-relative 'default :background "#202020" :foreground "white")
    (setq-local cursor-type nil truncate-lines t)
    (let ((fo (glitnir-decor-gui--calibrate))
          (str (concat (propertize "ab" 'face outer) (propertize "cd" 'face inner) (propertize "ef" 'face outer))))
      (insert "\n\nx\n\n")
      (overlay-put (make-overlay 3 4) 'after-string str)
      (goto-char 1)
      (gui-runner-wait-settled)
      (let* ((s (glitnir-decor-gui--scale)) (g (gui-runner-screen-grab))
             (x (cdr (glitnir-decor-gui--at g 3 fo)))
             (w (glitnir-decor-gui--dev s (frame-char-width)))
             ;; the string starts after `x': its `c' is three columns on, its `e' five
             (cx (+ (car x) (* 3 w))) (ex (+ (car x) (* 5 w))))
        (list g (+ (cdr x) (glitnir-decor-gui--dev s (/ (line-pixel-height) 2))) cx ex)))))

(ert-deftest glitnir-decor-gui/box-nested-in-a-string-has-sides ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; In a display or overlay string, a boxed run whose box differs from the next one's ends with its own side, as in
  ;; buffer text: a keycap's ring on a completion list's current row is whole.
  (let* ((glitnir-enable t)
         (r (glitnir-decor-gui--two-runs-in-a-string
             (glitnir-decor-gui--box-face "outer" "#303060" "#303060")
             (glitnir-decor-gui--box-face "inner" "#306030" "#ff8000"))))
    (pcase-let ((`(,g ,y ,cx ,ex) r))
      (should (equal (list 'left (glitnir-decor-gui--px g cx y)) '(left (255 128 0))))
      (should (equal (list 'right (glitnir-decor-gui--px g (1- ex) y)) '(right (255 128 0)))))))

(ert-deftest glitnir-decor-gui/adjacent-same-boxes-are-one-run ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; Two faces whose boxes are the same (colour and kind), side by side, are one run as in stock Emacs: no side
  ;; between them, only their fills.
  (let* ((glitnir-enable t)
         (r (glitnir-decor-gui--two-runs (glitnir-decor-gui--box-face "first" "#303060" "#ff8000")
                                         (glitnir-decor-gui--box-face "second" "#306030" "#ff8000"))))
    (pcase-let ((`(,g ,y ,cx ,_) r))
      (should (equal (list 'b (glitnir-decor-gui--px g (1- cx) y)) '(b (48 48 96))))
      (should (equal (list 'c (glitnir-decor-gui--px g cx y)) '(c (48 96 48)))))))

(ert-deftest glitnir-decor-gui/decor-partial-equals-full ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; Redrawing only the neighbour gives the same pixels as a full redraw.
  (let ((r (glitnir-decor-gui--chip "xy" (lambda (m) (save-excursion (goto-char m) (delete-char -1) (insert "y")))
                                    nil (lambda ()
                                          (redraw-frame)
                                          (gui-runner-wait-settled)
                                          (gui-runner-screen-grab)))))
    (let* ((full (nth 4 r)) (part (nth 2 r)))
      (should (equal (aref (nth 2 full) 0) (aref (nth 2 part) 0)))
      ;; the title bar around the text area changes between two grabs (focus, hover): compare the text area
      (let* ((s (glitnir-decor-gui--scale))
             (e (window-body-pixel-edges)) (fo (nth 3 r)) (w (car full))
             (x0 (+ (car fo) (glitnir-decor-gui--dev s (nth 0 e)))) (y0 (+ (cdr fo) (glitnir-decor-gui--dev s (nth 1 e))))
             (rw (* 4 (glitnir-decor-gui--dev s (- (nth 2 e) (nth 0 e)))))
             (bad 0))
        (dotimes (dy (glitnir-decor-gui--dev s (- (nth 3 e) (nth 1 e))))
          (let ((i (* 4 (+ x0 (* w (+ y0 dy))))))
            (unless (string= (substring (nth 2 full) i (+ i rw)) (substring (nth 2 part) i (+ i rw)))
              (cl-incf bad))))
        (should (= bad 0))))))

;; Labels: a face whose :box has :label floats a pill above the run's first glyph (below it on the window's first row).

(defconst glitnir-decor-gui--orange '(255 128 0))

(defun glitnir-decor-gui--label-face (&optional label)
  "A new face with an orange pill-coloured box; LABEL nil: no :label."
  (let ((face (make-symbol "glitnir-decor-gui-label")))
    (custom-declare-face face nil "Test face.")
    (set-face-attribute face nil :background "#303060" :foreground "white"
                        :box (append '(:line-width 1 :color "#ff8000") (and label (list :label label))))
    face))

(defvar glitnir-decor-gui--origin nil "The grab's pixel of the frame's top-left, found with the buffer empty.")

(defmacro glitnir-decor-gui--with-labels (&rest body)
  "Run BODY in a temp buffer on a dark background, Glitnir on.  The grab's frame origin is found first, with the
buffer empty (a label test's first line is not empty, so the text area's corner cannot be searched later)."
  `(let ((glitnir-enable t))
     (with-temp-buffer
       (switch-to-buffer (current-buffer))
       (face-remap-add-relative 'default :background "#202020" :foreground "white")
       (let ((glitnir-decor-gui--origin (progn (setq-local cursor-type nil)
                                              (prog1 (glitnir-decor-gui--calibrate)
                                                (kill-local-variable 'cursor-type)))))
         ,@body))))

(defun glitnir-decor-gui--above-p (pos &optional below)
  "True if the pixel one glyph-height above (BELOW: below) POS, half a glyph in, is the label's pill colour."
  (gui-runner-wait-settled)
  (let* ((s (glitnir-decor-gui--scale))
         (g (gui-runner-screen-grab))
         (at (cdr (glitnir-decor-gui--at g pos glitnir-decor-gui--origin)))
         (dys (if below                 ; the pill floats a few pixels from the run, and the gap grows with the text scale
                  (cl-loop for d from (1+ (frame-char-height)) to (+ (frame-char-height) 12) collect (round (* s d)))
                (cl-loop for d from -12 to -1 collect (round (* s d)))))
         (dy (car (last dys))))
    (or (cl-some (lambda (dy) (equal (glitnir-decor-gui--px g (+ (car at) (round (* s (/ (frame-char-width) 2)))) (+ (cdr at) dy))
                                     glitnir-decor-gui--orange))
                 dys)
        (let ((x (+ (car at) (round (* s (/ (frame-char-width) 2))))))
          (glitnir-gui--say "GLITNIR-I2F2 no pill: column x=%d from y=%d (anchor %d, dy %d): %S" x (- (cdr at) 12) (cdr at) dy
                            (cl-loop for y from (- (cdr at) 12) to (+ (cdr at) (round (* s (frame-char-height))) 16) by 2
                                     collect (car (glitnir-decor-gui--px g x y))))
          nil))))

(ert-deftest glitnir-decor-gui/label-above-anchor ()
  (skip-unless (featurep 'nidhogg-wayland))
  (glitnir-decor-gui--with-labels
   (insert "\n\n\n  " (propertize "ab" 'face (glitnir-decor-gui--label-face "AG")) "\n\n")
   (goto-char 1)
   (should (glitnir-decor-gui--above-p 6))
   ;; The same text without :label has nothing above it.
   (put-text-property 6 8 'face (glitnir-decor-gui--label-face))
   (should-not (glitnir-decor-gui--above-p 6))))

(ert-deftest glitnir-decor-gui/label-below-on-first-row ()
  (skip-unless (featurep 'nidhogg-wayland))
  (glitnir-decor-gui--with-labels
   (insert (propertize "ab" 'face (glitnir-decor-gui--label-face "AG")) "\n\n\n")
   (goto-char 1)
   (should (glitnir-decor-gui--above-p 1 t))))

(ert-deftest glitnir-decor-gui/label-follows-scroll ()
  (skip-unless (featurep 'nidhogg-wayland))
  (glitnir-decor-gui--with-labels
   (insert (make-string 12 ?\n) "  " (propertize "ab" 'face (glitnir-decor-gui--label-face "AG")) (make-string 40 ?\n))
   (goto-char 1)
   (redisplay t)
   (let ((pos 15))
     (should (glitnir-decor-gui--above-p pos))
     (set-window-start nil (save-excursion (goto-char 1) (forward-line 3) (point)))
     (should (glitnir-decor-gui--above-p pos))
     (set-window-start nil 1)
     (should (glitnir-decor-gui--above-p pos)))))

(ert-deftest glitnir-decor-gui/label-no-residue-after-redraw ()
  (skip-unless (featurep 'nidhogg-wayland))
  (glitnir-decor-gui--with-labels
   (insert "\n\n\n  " (propertize "ab" 'face (glitnir-decor-gui--label-face "AG")) "\n\n")
   (goto-char 1)
   (should (glitnir-decor-gui--above-p 6))
   (redraw-frame)
   (should (glitnir-decor-gui--above-p 6))
   (remove-text-properties 6 8 '(face nil))
   (should-not (glitnir-decor-gui--above-p 6))))

(ert-deftest glitnir-decor-gui/label-anchor-deleted-no-residue ()
  (skip-unless (featurep 'nidhogg-wayland))
  (glitnir-decor-gui--with-labels
   (insert "\n\n\n  " (propertize "ab" 'face (glitnir-decor-gui--label-face "AG")) "\n\n")
   (goto-char 1)
   (should (glitnir-decor-gui--above-p 6))
   (delete-region 6 8)
   (should-not (glitnir-decor-gui--above-p 6))))

(ert-deftest glitnir-decor-gui/label-two-windows ()
  (skip-unless (featurep 'nidhogg-wayland))
  (glitnir-decor-gui--with-labels
   (insert "\n\n\n  " (propertize "ab" 'face (glitnir-decor-gui--label-face "AG")) "\n\n")
   (goto-char 1)
   (let ((w2 (split-window-right)))
     (set-window-buffer w2 (current-buffer))
     (should (glitnir-decor-gui--above-p 6))
     (with-selected-window w2
       (should (glitnir-decor-gui--above-p 6)))
     ;; Closing one window leaves the other's label.
     (delete-window w2)
     (should (glitnir-decor-gui--above-p 6)))))

(ert-deftest glitnir-decor-gui/label-rerenders-on-scale-change ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; A sprite is keyed by its scale and size; a size change (text scale here, the monitor scale under gui.sh --scale)
  ;; renders another one and the old pill leaves no pixels.
  (glitnir-decor-gui--with-labels
   (insert "\n\n\n  " (propertize "ab" 'face (glitnir-decor-gui--label-face "AG")) "\n\n")
   (goto-char 1)
   (should (glitnir-decor-gui--above-p 6))
   (text-scale-set 2)
   (should (glitnir-decor-gui--above-p 6))
   (text-scale-set 0)
   (should (glitnir-decor-gui--above-p 6))))

(ert-deftest glitnir-decor-gui/label-inside-card-row ()
  (skip-unless (featurep 'nidhogg-wayland))
  ;; A cursor face with :label continuing a card's (unlabelled) run still anchors its own label.
  (glitnir-decor-gui--with-labels
   (let ((card (glitnir-decor-gui--label-face)))
     (set-face-attribute card nil :box '(:line-width 1 :color "#4040a0"))
     (insert "\n\n\n  "
             (propertize "xxxx" 'face card)
             (propertize "ab" 'face (glitnir-decor-gui--label-face "AG"))
             (propertize "xxxx" 'face card)
             "\n\n")
     (goto-char 1)
     (should (glitnir-decor-gui--above-p 10))
     (should-not (glitnir-decor-gui--above-p 7)))))

;;; PD cost and construction spike (corner cost, tag softness).  The numbers are written to stderr as "GLITNIR-PD ..." lines.

(defun glitnir-decor-gui--board (radius)
  "Fill the current buffer with 300 decorated runs (chips, buttons, card rows) of box RADIUS.
Return the number of runs."
  (let ((faces (cl-loop for (bg . lw) in '(("#303060" . 1) ("#406040" . 2) ("#604030" . 1))
                        for f = (make-symbol "glitnir-decor-gui-bench")
                        do (custom-declare-face f nil "Bench face.")
                           (set-face-attribute f nil :background bg :foreground "white"
                                               :box (list :line-width lw :color "#ff8000"
                                                          :corner-radius radius))
                        collect f))
        (n 0))
    (erase-buffer)
    (dotimes (row 30)
      (insert " ")
      (dotimes (col 10)
        (insert (propertize (format " r%03d " n) 'face (nth (% col 3) faces)) " ")
        (setq n (1+ n)))
      (insert "\n"))
    n))

(defun glitnir-decor-gui--bench (radius runs)
  "p50/p95 microseconds per decorated string over RUNS redraws, faces with box RADIUS."
  (let ((glitnir-enable t))
    (with-temp-buffer
      (switch-to-buffer (current-buffer))
      (face-remap-add-relative 'default :background "#202020" :foreground "white")
      (let ((n (glitnir-decor-gui--board radius)))
        (gui-runner-wait-settled)
        (glitnir--decor-bench 20 n)     ; warm the sprite and glyph caches
        (glitnir--decor-bench runs n)))))

(ert-deftest glitnir-decor-gui/decor-bench ()
  "Corner cost: 300 x (p95 radius 6 - p95 radius 0) <= 2 ms, at the run's scale (1, 2 or 1.25)."
  (skip-unless (featurep 'nidhogg-wayland))
  ;; A timing row: judged only in a pinned step (the runner sets GLITNIR_GUI_PINNED=1 under perf-exclusive.sh); an
  ;; unpinned GUI sweep's p95 measures the host's noise, not the code.
  (skip-unless (equal (getenv "GLITNIR_GUI_PINNED") "1")) ; reason: the corner cost is judged pinned only (GLITNIR_GUI_PINNED=1)
  (let* ((r0 (glitnir-decor-gui--bench 0 200))
         (r6 (glitnir-decor-gui--bench 6 200))
         (delta (* 300 (- (plist-get r6 :p95-us) (plist-get r0 :p95-us)))))
    (message "GLITNIR-PD corner-cost scale=%s radius0=%S radius6=%S delta-us=%.0f (limit 2000)"
             (or (getenv "GLITNIR_GUI_SCALE") "1") r0 r6 delta)
    (when (> delta 2000)
      (ert-fail (format "corner cost over the limit: radius0 %S radius6 %S delta %.0f us (limit 2000)" r0 r6 delta)))))

(ert-deftest glitnir-decor-gui/decor-svg-tag-softness-record ()
  "Tag softness (record only): an svg-lib style tag image at the run's scale; write a crop to look at softness."
  (skip-unless (featurep 'nidhogg-wayland))
  (skip-unless (image-type-available-p 'svg))
  (let ((glitnir-enable t)
        (svg "<svg xmlns='http://www.w3.org/2000/svg' width='60' height='20'><rect width='60' height='20' rx='6' fill='#303060'/><text x='8' y='14' font-size='11' fill='white'>tag</text></svg>"))
    (with-temp-buffer
      (switch-to-buffer (current-buffer))
      (face-remap-add-relative 'default :background "#202020" :foreground "white")
      (insert "\n\n  " (propertize "T" 'display (create-image svg 'svg t)) "\n\n")
      (gui-runner-wait-settled)
      (let* ((grab (gui-runner-screen-grab))
             (o (gui-runner-grab-find-block grab '(96 48 48) 8 8)) ; #303060 as B G R
             (soft 0))
        (when o
          (dotimes (i 6)                ; the 6 pixels left of the tag's left edge, at mid height
            (let ((px (glitnir-decor-gui--px grab (- (car o) 1 i) (+ (cdr o) 4))))
              (unless (member px '((32 32 32) (48 48 96))) (cl-incf soft)))))
        (message "GLITNIR-PD tag-softness scale=%s tag-found=%s edge-intermediate-pixels=%s"
                 (or (getenv "GLITNIR_GUI_SCALE") "1") (and o t) soft)
        (should o)))))

;;; glitnir-decor-gui-tests.el ends here
