;;; glitnir-gui-tests.el --- GUI tests for Glitnir (the compositor)  -*- lexical-binding: t -*-

;; test-class: cond
;; test-pattern: handshake

;; Run by nidhogg/test/gui.sh (test/src/nidhogg-tests/gui-runner.el loads every *-gui-tests.el), and on macOS by
;; nidhogg/test/nhmac-gui.sh, where the rows that need the Wayland shell's scene tree skip (`glitnir-gui--skip-unless-tree').  GLITNIR_GUI_SCALE is
;; the monitor scale the run asked for.  The rows judge what the shell gave the compositor: `glitnir-snapshot' of a root
;; holds its whole tree, child frames at their places.  Frame sizes and positions on the native shell are device pixels.

(require 'ert)
(require 'cl-lib)
(require 'nidhogg-views)

(defun glitnir-gui--skip-unless-tree (&optional by)
  "Skip the running row unless the shell is the Wayland one.
The rows judge the scene tree the compositor was given (`glitnir-snapshot', `glitnir--stacking', the draw counts),
which Naglfar has no counterpart of (its child frames are layers, read by glitnir-nhmac-gui-tests.el).  The rows
that read none of it run on every backend.  BY, a row's name: the macOS row that checks the same on the screen,
named in the skip."
  (unless (featurep 'nidhogg-wayland)
    (ert-skip (if by
                  (format "Naglfar: layers over the root; checked by %s" by)
                "reads the Wayland shell's scene tree (glitnir-snapshot, stacking)"))))

(defun glitnir-gui--drawn-since (draws)
  "Wait for the selected frame's tree to show something once more than DRAWS, a `glitnir--draw-count'."
  (should (gui-runner-wait (lambda () (> (glitnir--draw-count) draws)))))

(defmacro glitnir-gui--drawn (&rest body)
  "Run BODY, which changes what the frame shows, redisplay, and wait for the commit of it."
  (declare (indent 0))
  (let ((draws (make-symbol "draws")))
    `(let ((,draws (glitnir--draw-count)))
       ,@body
       (redisplay t)
       (glitnir-gui--drawn-since ,draws))))

(defun glitnir-gui--painted ()
  "Redisplay, wait for the presenter, and return the picture, a raw snapshot."
  (redisplay t)
  (gui-runner-wait-settled)
  (glitnir-snapshot nil 'raw))

(ert-deftest glitnir-gui/a-frame-at-the-asked-scale ()
  :tags '(gnome-smoke)
  (should (< (abs (- (frame-scale-factor)
                     (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1"))))
             0.001)))

(ert-deftest glitnir-gui/snapshot-has-the-frame-size ()
  :tags '(gnome-smoke)
  (glitnir-gui--skip-unless-tree)
  (pcase-let ((`(,w ,h ,bytes) (glitnir-snapshot nil 'raw)))
    (should (= w (frame-pixel-width)))
    (should (= h (frame-pixel-height)))
    (should (= (length bytes) (* 4 w h)))
    (should-not (multibyte-string-p bytes))))

(ert-deftest glitnir-gui/snapshot-png-is-a-png ()
  :tags '(gnome-smoke)
  (glitnir-gui--skip-unless-tree)
  (let ((png (glitnir-snapshot nil)))
    (should-not (multibyte-string-p png))
    (should (string-prefix-p "\x89PNG" png))))

(ert-deftest glitnir-gui/daemon-first-frame-initialises ()
  (skip-unless (daemonp))
  (should (>= (plist-get (glitnir-stats) :count) 1)))

(ert-deftest glitnir-gui/a-message-of-megabytes-shows-at-once ()
  "A `message' of megabytes (a huge error, a pasted log) is shown at once.  Before, the echo area measured all of it
to size the mini-window, and the bidi bracket scan in that layout never checked for quit: a 5 MB condition printed by
ERT froze the session for good (2026-11-17), C-g and SIGUSR2 did nothing.  The echo area lays out only what its
window can show; the message itself is kept whole."
  (dolist (text (list (concat "(" (apply #'concat (make-list 500000 "(ab cd) ")) ")")   ; one line, all brackets
                      (mapconcat #'identity (make-list 200000 "a line of a pasted log (x)") "\n")))
    (let ((t0 (float-time)))
      (message "%s" text)
      (redisplay t)
      (should (< (- (float-time) t0) 1.0))
      (should (equal (current-message) text))))
  (message nil))

(ert-deftest glitnir-gui/enable-nil-is-inactive ()
  (should (glitnir-active-p))
  (let ((glitnir-enable nil))
    (should-not (glitnir-active-p))))

(defconst glitnir-gui--magenta '(:background "#ff00ff" :foreground "#ff00ff"))

(defun glitnir-gui--frame-px ()
  (* (frame-pixel-width) (frame-pixel-height)))

(defun glitnir-gui--samples (r g b)
  "Samples (every 4th device pixel) of a snapshot of the selected frame's root that have the colour R G B."
  (pcase-let ((`(,w ,h ,bytes) (glitnir-snapshot nil 'raw)) (n 0))
    ;; ARGB32 in the machine's (little-endian) byte order: B G R A
    (cl-loop for y from 0 below h by 4 do
             (cl-loop for x from 0 below w by 4 do
                      (let ((i (* 4 (+ x (* y w)))))
                        (when (and (= (aref bytes i) b) (= (aref bytes (+ i 1)) g)
                                   (= (aref bytes (+ i 2)) r))
                          (cl-incf n)))))
    n))

(defun glitnir-gui--text-buffer (name lines)
  (let ((b (get-buffer-create name)))
    (with-current-buffer b
      (erase-buffer)
      (dotimes (i lines)
        (insert (format "%5d %s\n" i (make-string 70 (+ ?a (% i 26))))))
      (goto-char (point-min)))
    b))

(defun glitnir-gui--child (buffer visible &optional background)
  (let* ((root (selected-frame))
         (f (make-frame `((parent-frame . ,root) (minibuffer . nil) (left . 200) (top . 200)
                          (width . 50) (height . 12) (background-color . ,(or background "#00ffff"))
                          (visibility . nil) (no-accept-focus . t) (no-other-frame . t)
                          (undecorated . t) (internal-border-width . 1)))))
    (select-frame root)
    (set-window-buffer (frame-root-window f) buffer)
    (when visible (make-frame-visible f))
    f))

(ert-deftest glitnir-gui/typing-paints-only-damage ()
  (glitnir-gui--skip-unless-tree)
  (should (glitnir-active-p))
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (insert "hello")
    (glitnir-gui--painted)
    (glitnir-stats nil t)
    (dotimes (_ 20) (glitnir-gui--drawn (insert "x")))
    (let ((s (glitnir-stats)))
      (should (>= (plist-get s :count) 5))
      (should (< (plist-get s :painted-px) (/ (glitnir-gui--frame-px) 10))))))

(ert-deftest glitnir-gui/last-paint-times-the-change ()
  (glitnir-gui--skip-unless-tree)
  ;; Pattern C (handshake): the change's commit is waited for through `glitnir--draw-count', after a settled start,
  ;; so the newest paint is the one that shows it.  Its times are the perf lab's present source.
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (insert "hello")
    (glitnir-gui--painted)
    (let ((t0 (glitnir--now-ns)))
      (glitnir-gui--drawn (insert "x"))
      (let ((p (glitnir--last-paint)))
        (should p)
        (should (<= t0 (plist-get p :commit-ns) (plist-get p :paint-ns) (plist-get p :present-ns)
                    (glitnir--now-ns)))))))

(ert-deftest glitnir-gui/error-in-redisplay-keeps-drawing ()
  (glitnir-gui--skip-unless-tree)
  (let ((buf (generate-new-buffer "*glitnir-err*")))
    (unwind-protect
        (progn
          (switch-to-buffer buf)
          ;; a mode-line :eval that signals (redisplay catches it) ...
          (setq-local mode-line-format '("%b " (:eval (error "Glitnir test error"))))
          (dotimes (_ 5) (insert "e") (force-mode-line-update) (redisplay t))
          ;; ... and one that throws: a non-local exit through unwind_redisplay after the window was drawn
          (setq-local mode-line-format '("%b " (:eval (throw 'glitnir-gui-out 'thrown))))
          (insert "t") (force-mode-line-update)
          (should (eq (catch 'glitnir-gui-out (redisplay t) nil) 'thrown))
          (kill-local-variable 'mode-line-format)
          (insert (propertize "MMMMMMMM" 'face glitnir-gui--magenta))
          (glitnir-gui--painted)
          (should (> (glitnir-gui--samples 255 0 255) 0)))
      (kill-buffer buf))))

(ert-deftest glitnir-gui/a-bar-block-fills-its-rows ()
  "A `bar' block over a window's first two rows: its x0..x1 filled in the line's colour on those rows only."
  (glitnir-gui--skip-unless-tree)
  (let ((buf (glitnir-gui--text-buffer "*glitnir-bar*" 10)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (glitnir-gui--drawn
            (with-current-buffer buf
              (set-window-parameter
               w 'glitnir-blocks
               `((bar ,(copy-marker 1) ,(save-excursion (goto-char 1) (forward-line 2) (point-marker))
                      :x1 6 :ring ("#ff0000" . 1))))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,x ,y . ,_) (window-inside-pixel-edges w))
                       (lh (with-current-buffer buf (line-pixel-height)))
                       (at (lambda (dx row) (glitnir-gui--pixel snap (list (+ x dx) (+ y (* row lh) (/ lh 2)))))))
            (should (equal (funcall at 2 0) '(255 0 0)))
            (should (equal (funcall at 2 1) '(255 0 0)))
            (should-not (equal (funcall at 2 2) '(255 0 0)))   ; the third row: past the block's end
            (should-not (equal (funcall at 12 0) '(255 0 0))))) ; past x1
      (set-window-parameter w 'glitnir-blocks nil)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/a-row-block-lies-beneath-the-runs ()
  "A `row' block over a row with a boxed chip: the block's fill where the row shows the window's default
background (from x0, past the text's end), the chip keeps its own colour inside it, the corner outside the rounded
path and the inset before x0 show the colour beneath.  Moved to the next row, the first row is drawn again without it."
  (glitnir-gui--skip-unless-tree)
  (let* ((buf (get-buffer-create "*glitnir-row*")) (w (selected-window))
         (chip '(:background "#00ff00" :foreground "#00ff00" :box (:line-width 1 :color "#00ff00")))
         chip-at)
    (with-current-buffer buf
      (erase-buffer)
      (insert "      name ")
      (setq chip-at (point))
      (insert (propertize "CHIPCHIP" 'face chip) " tail\nother row\nlast\n"))
    (unwind-protect
        (let* ((line (lambda (n) (with-current-buffer buf
                                   (save-excursion (goto-char 1) (forward-line n) (point-marker)))))
               (block (lambda (from)
                        `((row ,(funcall line from) ,(funcall line (1+ from))
                               :x0 4 :x1 400 :radius 8 :fill ("#0000ff" . 1) :beneath "#ff0000")))))
          (set-window-buffer w buf)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (glitnir-gui--drawn (set-window-parameter w 'glitnir-blocks (funcall block 0)))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,x ,y . ,_) (window-inside-pixel-edges w))
                       (lh (with-current-buffer buf (line-pixel-height)))
                       (cx (car (posn-x-y (posn-at-point chip-at w))))
                       (at (lambda (dx dy) (glitnir-gui--pixel snap (list (+ x dx) (+ y dy))))))
            (should (equal (funcall at 12 (/ lh 2)) '(0 0 255)))          ; the row's start: the fill
            (should (equal (funcall at 390 (/ lh 2)) '(0 0 255)))         ; past the text's end: the fill
            (should (equal (funcall at (+ cx 4) (/ lh 2)) '(0 255 0)))    ; inside the chip: its own colour
            (should (equal (funcall at 4 0) '(255 0 0)))                  ; the corner: beneath
            (should (equal (funcall at 1 (/ lh 2)) '(255 0 0)))           ; before x0: beneath
            (should-not (equal (funcall at 12 (+ lh (/ lh 2))) '(0 0 255)))) ; the next row: not in it
          (glitnir-gui--drawn (set-window-parameter w 'glitnir-blocks (funcall block 1)))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,x ,y . ,_) (window-inside-pixel-edges w))
                       (lh (with-current-buffer buf (line-pixel-height)))
                       (at (lambda (dx dy) (glitnir-gui--pixel snap (list (+ x dx) (+ y dy))))))
            (should-not (equal (funcall at 390 (/ lh 2)) '(0 0 255)))
            (should (equal (funcall at 390 (+ lh (/ lh 2))) '(0 0 255)))))
      (set-window-parameter w 'glitnir-blocks nil)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/a-block-holds-the-after-string-rows-at-its-end ()
  "Rows of nothing but an overlay string at a block's END (a notebook cell's output, an after-string there) are inside
the block; the next block, starting at that END, begins at its own text."
  (glitnir-gui--skip-unless-tree)
  (let* ((buf (glitnir-gui--text-buffer "*glitnir-tail*" 10)) (w (selected-window))
         (end (with-current-buffer buf (save-excursion (goto-char 1) (forward-line 1) (point))))
         (ov (with-current-buffer buf (make-overlay end end))))
    (unwind-protect
        (progn
          (overlay-put ov 'after-string "      out 1\n      out 2\n") ; blank where the bars are
          (set-window-buffer w buf)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (glitnir-gui--drawn
            (with-current-buffer buf
              (set-window-parameter
               w 'glitnir-blocks
               `((bar ,(copy-marker 1) ,(copy-marker end) :x1 6 :ring ("#ff0000" . 1))
                 (bar ,(copy-marker end) ,(save-excursion (goto-char end) (forward-line 1) (point-marker))
                      :x0 8 :x1 14 :ring ("#00ff00" . 1))))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,x ,y . ,_) (window-inside-pixel-edges w))
                       (lh (with-current-buffer buf (line-pixel-height)))
                       (at (lambda (dx row) (glitnir-gui--pixel snap (list (+ x dx) (+ y (* row lh) (/ lh 2)))))))
            ;; rows: the first line, the two output rows, the next line
            (should (equal (funcall at 2 0) '(255 0 0)))
            (should (equal (funcall at 2 1) '(255 0 0)))
            (should (equal (funcall at 2 2) '(255 0 0)))
            (should-not (equal (funcall at 2 3) '(255 0 0)))
            (should-not (equal (funcall at 10 1) '(0 255 0)))
            (should-not (equal (funcall at 10 2) '(0 255 0)))
            (should (equal (funcall at 10 3) '(0 255 0)))))
      (set-window-parameter w 'glitnir-blocks nil)
      (delete-overlay ov)
      (kill-buffer buf))))

(defmacro glitnir-gui--with-card (spec &rest body)
  "Run BODY with the first three lines of a text buffer tinted yellow under the blocks SPEC makes.
SPEC is evaluated with `end' bound to the end of those lines; BODY with
`at', a function of (DX DY) giving the painted pixel at that offset from
the window's text area, and `lh', the line height."
  (declare (indent 1))
  `(let* ((buf (glitnir-gui--text-buffer "*glitnir-card*" 10)) (w (selected-window))
          (end (with-current-buffer buf (save-excursion (goto-char 1) (forward-line 3) (point))))
          (ov (with-current-buffer buf (make-overlay 1 end))))
     (unwind-protect
         (progn
           (overlay-put ov 'face '(:background "#ffff00" :extend t))
           (set-window-buffer w buf)
           (set-window-start w 1)
           (glitnir-gui--painted)
           (glitnir-gui--drawn
             (with-current-buffer buf
               (set-window-parameter w 'glitnir-blocks ,spec)))
           (pcase-let* ((snap (glitnir-gui--painted))
                        (`(,x ,y . ,_) (window-inside-pixel-edges w))
                        (lh (with-current-buffer buf (line-pixel-height)))
                        (at (lambda (dx dy) (glitnir-gui--pixel snap (list (+ x dx) (+ y dy))))))
             (ignore lh)
             ,@body))
       (set-window-parameter w 'glitnir-blocks nil)
       (delete-overlay ov)
       (kill-buffer buf))))

(ert-deftest glitnir-gui/a-ring-block-keeps-a-gap-above-and-below ()
  "A ring's `:gap': the ring sits that many px inside its rows' top and bottom, and the band outside it shows the
colour beneath, so cards on adjacent rows stand apart.  The columns at x 20 are blank in every row."
  (glitnir-gui--skip-unless-tree)
  (glitnir-gui--with-card
      `((ring ,(copy-marker 1) ,(copy-marker end) :x0 10 :x1 300 :radius 4 :gap 6
              :ring ("#ff0000" . 1) :beneath "#0000ff"))
    (should (equal (funcall at 20 2) '(0 0 255)))              ; the gap above
    (should (equal (funcall at 20 6) '(255 0 0)))              ; the ring, 6 px in
    (should (equal (funcall at 20 9) '(255 255 0)))            ; the card's rows
    (should (equal (funcall at 20 (- (* 3 lh) 7)) '(255 0 0))) ; the ring at the bottom
    (should (equal (funcall at 20 (- (* 3 lh) 3)) '(0 0 255))))) ; the gap below

(defun glitnir-gui--line ()
  "A one logical px line in whole device px at the frame's scale, as the glue makes it."
  (max 1 (round (frame-scale-factor))))

(defun glitnir-gui--logical (n)
  "N logical px as device px at the frame's scale (the gap's px are device px)."
  (round (* n (frame-scale-factor))))

(ert-deftest glitnir-gui/a-ring-block-casts-a-shadow ()
  "A ring's `:shadow' and `:shadow-ring': a 1 logical px line just outside the card's edge, and a 3 px blur 1 px down,
both in the gap only, never over the card's rows.  Geometry in logical px times the scale."
  (glitnir-gui--skip-unless-tree)
  (let ((l (glitnir-gui--line)) (g (glitnir-gui--logical 8)))
    (glitnir-gui--with-card
        `((ring ,(copy-marker 1) ,(copy-marker end) :x0 10 :x1 300 :radius 4 :gap ,g
                :ring ("#ff0000" . 1) :beneath "#0000ff" :shadow ("#000000" . 1) :shadow-ring ("#00ff00" . 1)))
      (let ((edge (- (* 3 lh) g)))                             ; the card's bottom edge (exclusive)
        (should (equal (funcall at 20 1) '(0 0 255)))          ; past the shadow's reach above
        (should (equal (funcall at 20 (1- g)) '(0 255 0)))     ; the shadow's ring above the card
        (should (equal (funcall at 20 (- g l)) '(0 255 0)))
        (should (equal (funcall at 20 g) '(255 0 0)))          ; the card's own ring
        (should (equal (funcall at 20 (+ g l -1)) '(255 0 0)))
        (should (equal (funcall at 20 (+ g l)) '(255 255 0)))  ; the rows: no shadow over them
        (should (equal (funcall at 20 edge) '(0 255 0)))       ; the shadow's ring below
        (pcase-let ((`(,r ,gr ,b) (funcall at 20 (+ edge l 1)))) ; the blur below: blue darkened
          (should (and (= r 0) (= gr 0) (< 0 b 250))))
        (should (equal (funcall at 20 (1- (* 3 lh))) '(0 0 255)))))))  ; the gap's far side: past the reach

(ert-deftest glitnir-gui/a-ring-block-has-an-outer-ring ()
  "A ring's `:outer': a second line 1 logical px wide just outside the ring (the focus ring inside, the border step
outside), in the gap.  Geometry in logical px times the scale."
  (glitnir-gui--skip-unless-tree)
  (let ((l (glitnir-gui--line)) (g (glitnir-gui--logical 8)))
    (glitnir-gui--with-card
        `((ring ,(copy-marker 1) ,(copy-marker end) :x0 10 :x1 300 :radius 4 :gap ,g
                :ring ("#ff0000" . 1) :outer ("#00ff00" . 1) :beneath "#0000ff"))
      (let ((edge (- (* 3 lh) g)))
        (should (equal (funcall at 20 (- g l 1)) '(0 0 255)))
        (should (equal (funcall at 20 (1- g)) '(0 255 0)))     ; the outer ring
        (should (equal (funcall at 20 g) '(255 0 0)))          ; the ring inside it
        (should (equal (funcall at 20 (+ g l)) '(255 255 0)))
        (should (equal (funcall at 20 (1- edge)) '(255 0 0)))
        (should (equal (funcall at 20 edge) '(0 255 0)))
        (should (equal (funcall at 20 (+ edge l)) '(0 0 255)))))))

;; A cell's output in the notebook is one after-string of several rows at the cell's END; a bar over the cell must reach the
;; last of them, whatever the rows hold.  The helper puts STRING after line 1 of a text buffer, a bar over line 0 to
;; there, and judges every pixel row from the window's top to where the next line's text starts.
(defun glitnir-gui--tail-check (string)
  "Run a bar block from line 0 to line 1's start, with STRING an after-string there; each row down to line 1 is red."
  (glitnir-gui--skip-unless-tree)
  (let* ((buf (glitnir-gui--text-buffer "*glitnir-tail-rows*" 10)) (w (selected-window))
         (end (with-current-buffer buf (save-excursion (goto-char 1) (forward-line 1) (point))))
         (ov (with-current-buffer buf (make-overlay end end))))
    (unwind-protect
        (progn
          (overlay-put ov 'after-string string)
          (set-window-buffer w buf)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (glitnir-gui--drawn
            (with-current-buffer buf
              (set-window-parameter
               w 'glitnir-blocks
               `((bar ,(copy-marker 1) ,(copy-marker end) :x1 6 :ring ("#ff0000" . 1))))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,x ,y . ,_) (window-inside-pixel-edges w))
                       (next (nth 1 (pos-visible-in-window-p end w t)))
                       (lh (with-current-buffer buf (line-pixel-height))))
            (should (> next (* 2 lh)))  ; the string made rows
            (let ((gaps (cl-loop for dy from 0 below next
                                 unless (equal (glitnir-gui--pixel snap (list (+ x 2) (+ y dy))) '(255 0 0))
                                 collect dy)))
              (should-not gaps))
            (should-not (equal (glitnir-gui--pixel snap (list (+ x 2) (+ y next (/ lh 2)))) '(255 0 0)))))
      (set-window-parameter w 'glitnir-blocks nil)
      (delete-overlay ov)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/a-block-holds-four-after-string-rows ()
  "Four text rows after a block's END, the last with the add row's text: all inside."
  (glitnir-gui--tail-check (propertize "16 \u2026\n64 \u2026\n[image/png 5x5]\n+ Code + Markdown\n" 'face 'bold)))

(ert-deftest glitnir-gui/a-block-holds-an-after-string-row-of-a-display-string ()
  "A row of a display string and one of an image, among the after-string rows at a block's END: inside."
  (glitnir-gui--tail-check
   (concat "16 \u2026\n" (propertize " " 'display '(space :width 40 :height 30)) "\n"
           (propertize " " 'display (create-image "P1\n4 4\n1 0 0 1\n0 1 1 0\n0 1 1 0\n1 0 0 1\n" 'pbm t :scale 6)) "\n"
           "+ Code + Markdown\n")))

;; An `inner' block: a panel inside a card (the notebook's output under its hairline).  Its START and END are one position,
;; so its rows are the after-string's rows there, no row of the text before or after.
(ert-deftest glitnir-gui/an-inner-block-fills-the-after-string-rows-under-a-hairline ()
  "An inner block at END of line 0: a rounded fill (green) over the two after-string rows from x0 to x1, a hairline
(blue) along their top between the corners; the text rows around it keep none of either."
  (glitnir-gui--skip-unless-tree)
  (let* ((buf (glitnir-gui--text-buffer "*glitnir-inner*" 10)) (w (selected-window))
         (end (with-current-buffer buf (save-excursion (goto-char 1) (forward-line 1) (point))))
         (ov (with-current-buffer buf (make-overlay end end))))
    (unwind-protect
        (progn
          (overlay-put ov 'after-string "      out 1\n      out 2\n")
          (set-window-buffer w buf)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (glitnir-gui--drawn
            (with-current-buffer buf
              (set-window-parameter
               w 'glitnir-blocks
               `((inner ,(copy-marker end) ,(copy-marker end) :x0 4 :x1 30 :radius 6
                        :fill ("#00ff00" . 1) :top ("#0000ff" . 1))))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,x ,y . ,_) (window-inside-pixel-edges w))
                       (lh (with-current-buffer buf (line-pixel-height)))
                       (top (+ y lh))              ; the first after-string row: the row after line 0
                       (at (lambda (dx dy) (glitnir-gui--pixel snap (list (+ x dx) dy)))))
            (should (equal (funcall at 16 top) '(0 0 255)))                     ; the hairline, between the corners
            (should (equal (funcall at 16 (+ top (/ lh 2))) '(0 255 0)))        ; the first row's fill
            (should (equal (funcall at 16 (+ top lh (/ lh 2))) '(0 255 0)))     ; the second row's
            (should (equal (funcall at 16 (+ top (* 2 lh) -1)) '(0 255 0)))     ; the last pixel row
            (should-not (equal (funcall at 5 top) '(0 0 255)))                  ; no hairline in the corner
            (should-not (equal (funcall at 4 top) '(0 255 0)))                  ; the corner is rounded off
            (should-not (equal (funcall at 2 (+ top lh)) '(0 255 0)))           ; left of x0
            (should-not (equal (funcall at 32 (+ top lh)) '(0 255 0)))          ; right of x1
            (should-not (equal (funcall at 16 (+ y (/ lh 2))) '(0 255 0)))      ; line 0, above
            (should-not (equal (funcall at 16 (+ top (* 2 lh) (/ lh 2))) '(0 255 0))))) ; line 1, below
      (set-window-parameter w 'glitnir-blocks nil)
      (delete-overlay ov)
      (kill-buffer buf))))

;; The notebook's cell, as drgnx builds it: a head newline with a before-string (the header row) and a tail newline with
;; two overlays ending after it, the output's after-string and the add row's (priority -20), at the position where the
;; next cell's head newline begins, with its own before-string header row.  The output's rows and no others are the
;; inner block's: not the source row before, not the add row, not the next cell's header.
(defun glitnir-gui--notebook-buffer ()
  "A buffer of two cells; return (BUFFER END OUTPUT-OVERLAY), END the tail's end where the second cell's head is."
  (let ((buf (get-buffer-create "*glitnir-nb*")) end out)
    (with-current-buffer buf
      (erase-buffer)
      (insert "\nsrc one\nsrc two\n")                         ; head 1, two source rows, tail newline at the last
      (let ((tail (1- (point))))
        (setq end (point))
        (insert "\nsrc three\n")                               ; head 2, a source row, tail newline
        (let ((ov (make-overlay 1 2)))
          (overlay-put ov 'before-string (propertize "[1] python" 'face 'bold)))
        (let ((ov (make-overlay 1 end)))
          (overlay-put ov 'line-prefix (propertize " " 'display '(space :width 12))))
        (setq out (make-overlay tail end))
        (overlay-put out 'after-string "16 0.0612\n64 0.2487\n[image 640x360]\n")
        (let ((ov (make-overlay tail end)))
          (overlay-put ov 'priority -20)
          (overlay-put ov 'after-string "+ Code + Markdown\n"))
        (let ((ov (make-overlay end (1+ end))))
          (overlay-put ov 'before-string (propertize "[3] python" 'face 'bold)))
        (let ((ov (make-overlay end (point))))
          (overlay-put ov 'line-prefix (propertize " " 'display '(space :width 12)))))
      (goto-char 1))
    (list buf end out)))

(ert-deftest glitnir-gui/a-ring-block-stops-before-the-next-cells-header ()
  "A bar over a cell holds the output's and the add row's after-strings at its END, and stops before the next
cell's header row, a before-string of an overlay starting at that END."
  (glitnir-gui--skip-unless-tree)
  (pcase-let* ((`(,buf ,end ,_out) (glitnir-gui--notebook-buffer)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (glitnir-gui--drawn
            (with-current-buffer buf
              (set-window-parameter
               w 'glitnir-blocks `((bar ,(copy-marker 1) ,(copy-marker end) :x1 6 :ring ("#ff0000" . 1))))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,x ,y . ,_) (window-inside-pixel-edges w))
                       (lh (with-current-buffer buf (line-pixel-height)))
                       (red (lambda (row) (equal (glitnir-gui--pixel snap (list (+ x 2) (+ y (* row lh) (/ lh 2)))) '(255 0 0)))))
            (dotimes (row 7) (should (funcall red row)))     ; header 1, source, the output, the add row
            (should-not (funcall red 7))                     ; header 2
            (should-not (funcall red 8))))
      (set-window-parameter w 'glitnir-blocks nil)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/a-block-starting-at-the-end-leaves-the-previous-cells-output ()
  "The next cell's block starts at the previous one's END, where the output's after-string rows are: its rows are
the header and its source, not the output rows after the first (whose end is past that START)."
  (glitnir-gui--skip-unless-tree)
  (pcase-let* ((`(,buf ,end ,_out) (glitnir-gui--notebook-buffer)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (glitnir-gui--drawn
            (with-current-buffer buf
              (set-window-parameter
               w 'glitnir-blocks
               `((bar ,(copy-marker 1) ,(copy-marker end) :x1 6 :ring ("#ff0000" . 1))
                 (bar ,(copy-marker end) ,(copy-marker (with-current-buffer buf (point-max))) :x0 8 :x1 14 :ring ("#0000ff" . 1))))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,x ,y . ,_) (window-inside-pixel-edges w))
                       (lh (with-current-buffer buf (line-pixel-height)))
                       (blue (lambda (row) (equal (glitnir-gui--pixel snap (list (+ x 10) (+ y (* row lh) (/ lh 2)))) '(0 0 255)))))
            (dotimes (row 7) (should-not (funcall blue row)))   ; header 1, source, the output, the add row
            (should (funcall blue 7))                          ; header 2
            (should (funcall blue 8))))
      (set-window-parameter w 'glitnir-blocks nil)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/a-bar-block-naming-an-overlay-holds-only-its-rows-after-its-end ()
  "A bar with `:overlay' the output's: the output's rows after its END are inside, the add row (another overlay's
after-string at that END) is not."
  (glitnir-gui--skip-unless-tree)
  (pcase-let* ((`(,buf ,end ,out) (glitnir-gui--notebook-buffer)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (glitnir-gui--drawn
            (with-current-buffer buf
              (set-window-parameter
               w 'glitnir-blocks `((bar ,(copy-marker 1) ,(copy-marker end) :x1 6 :overlay ,out :ring ("#ff0000" . 1))))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,x ,y . ,_) (window-inside-pixel-edges w))
                       (lh (with-current-buffer buf (line-pixel-height)))
                       (red (lambda (row) (equal (glitnir-gui--pixel snap (list (+ x 2) (+ y (* row lh) (/ lh 2)))) '(255 0 0)))))
            (dotimes (row 6) (should (funcall red row)))     ; header 1, source, the output
            (should-not (funcall red 6))                     ; the add row
            (should-not (funcall red 7))))
      (set-window-parameter w 'glitnir-blocks nil)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/an-inner-block-holds-an-output-at-the-end-of-the-buffer ()
  "The last cell's output is an after-string at the buffer's end (no next cell, no text after it), its first row
starting with a display space (the card's pad) before its label: an inner block naming its overlay holds every row,
the label row too, and the hairline is at the label row's top."
  (glitnir-gui--skip-unless-tree)
  (let* ((buf (get-buffer-create "*glitnir-nb-end*")) (w (selected-window)) out)
    (with-current-buffer buf
      (erase-buffer)
      (insert "\nsrc\n")                                   ; head newline, one source row, the tail newline at the end
      (setq out (make-overlay (1- (point)) (point)))
      (overlay-put out 'after-string
                   (concat (propertize " " 'display '(space :width 12))
                           "error\nNameError: x\n  Cell In[3]\n"
                           (propertize " " 'display '(space :width (1) :height (8))) "\n"))
      (goto-char 1))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (glitnir-gui--drawn
            (with-current-buffer buf
              (set-window-parameter
               w 'glitnir-blocks
               `((inner ,(copy-marker (point-max)) ,(copy-marker (point-max)) :x0 4 :x1 30 :overlay ,out
                        :fill ("#00ff00" . 1) :top ("#0000ff" . 1))))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,x ,y . ,_) (window-inside-pixel-edges w))
                       (lh (with-current-buffer buf (line-pixel-height)))
                       (px (lambda (row dy) (glitnir-gui--pixel snap (list (+ x 16) (+ y (* row lh) dy)))))
                       (green (lambda (row) (equal (funcall px row (/ lh 2)) '(0 255 0)))))
            ;; rows: 0 head, 1 source, 2 the label, 3-4 the output, 5 the pad row
            (should-not (funcall green 1))
            (should (equal (funcall px 2 0) '(0 0 255)))      ; the hairline at the label row's top
            (dolist (row '(2 3 4 5)) (should (funcall green row)))))
      (set-window-parameter w 'glitnir-blocks nil)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/an-inner-block-pads-its-top-and-bottom ()
  "An inner block's `:pad-top' and `:pad-bottom' (device px) shorten the drawn panel from the top and the bottom, its
rows unchanged: the hairline sits that much below the first row's top, the fill ends that much above the last row's
bottom.  Geometry in logical px times the scale."
  (glitnir-gui--skip-unless-tree)
  (pcase-let* ((`(,buf ,end ,out) (glitnir-gui--notebook-buffer)) (w (selected-window))
               (pt (glitnir-gui--logical 4)) (pb (glitnir-gui--logical 8)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (glitnir-gui--drawn
            (with-current-buffer buf
              (set-window-parameter
               w 'glitnir-blocks
               `((inner ,(copy-marker end) ,(copy-marker end) :x0 4 :x1 30 :overlay ,out :pad-top ,pt :pad-bottom ,pb
                        :fill ("#00ff00" . 1) :top ("#0000ff" . 1))))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,x ,y . ,_) (window-inside-pixel-edges w))
                       (lh (with-current-buffer buf (line-pixel-height)))
                       (top (+ y (* 3 lh))) (bottom (+ y (* 6 lh)))   ; the output's rows 3 to 5
                       (at (lambda (dy) (glitnir-gui--pixel snap (list (+ x 16) dy)))))
            (should-not (member (funcall at (1- (+ top pt))) '((0 0 255) (0 255 0))))   ; above the panel
            (should (equal (funcall at (+ top pt)) '(0 0 255)))                           ; the hairline
            (should (equal (funcall at (+ top pt (glitnir-gui--line))) '(0 255 0)))       ; the fill under it
            (should (equal (funcall at (- bottom pb 1)) '(0 255 0)))                      ; the fill's last pixel row
            (should-not (equal (funcall at (- bottom pb)) '(0 255 0)))                    ; the padding below it
            (should-not (equal (funcall at (1- bottom)) '(0 255 0)))))
      (set-window-parameter w 'glitnir-blocks nil)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/an-inner-block-holds-the-outputs-rows-and-no-others ()
  "An inner block naming the output's overlay holds the rows of that after-string: the first (which follows the
source's row), the last, and not the add row's (another overlay's, at the same END), the next cell's header (a
before-string row) or the source row above."
  (glitnir-gui--skip-unless-tree)
  (pcase-let* ((`(,buf ,end ,out) (glitnir-gui--notebook-buffer)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (glitnir-gui--drawn
            (with-current-buffer buf
              (set-window-parameter
               w 'glitnir-blocks
               `((inner ,(copy-marker end) ,(copy-marker end) :x0 4 :x1 30 :radius 6 :overlay ,out
                        :fill ("#00ff00" . 1) :top ("#0000ff" . 1))))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,x ,y . ,_) (window-inside-pixel-edges w))
                       (lh (with-current-buffer buf (line-pixel-height)))
                       (green (lambda (row) (equal (glitnir-gui--pixel snap (list (+ x 16) (+ y (* row lh) (/ lh 2)))) '(0 255 0)))))
            ;; rows: 0 header 1, 1-2 source, 3-5 the output, 6 the add row, 7 header 2, 8 source
            (should-not (funcall green 2))
            (should (funcall green 3))
            (should (funcall green 4))
            (should (funcall green 5))
            (should-not (funcall green 6))
            (should-not (funcall green 7))
            (should-not (funcall green 8))))
      (set-window-parameter w 'glitnir-blocks nil)
      (kill-buffer buf))))

;; The overview ruler.  Its rows wait through `glitnir-gui--painted' (redisplay, the presenter settled), which every
;; shell gives; `glitnir--draw-count' is the Wayland shell's alone (0 on Naglfar).

(ert-deftest glitnir-gui/the-ruler-marks-the-buffer-by-position ()
  "A window's `glitnir-ruler': a strip at its right inner edge (the right fringe, widened to hold it) where marks
sit at their position's fraction of the buffer and the band covers the part shown; a scroll moves the band, not the
marks; nothing is drawn in the text area; a click's y on the strip gives the position at that fraction.
On Naglfar the strip and the ring are layers over the backing `glitnir-snapshot' reads: the screen's rows are
glitnir-nhmac-gui-tests.el's `nhmac-ruler-*'."
  (glitnir-gui--skip-unless-tree "nhmac-ruler-marks-the-buffer-by-position")
  (let ((buf (glitnir-gui--text-buffer "*glitnir-ruler*" 1000)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-fringes w nil 16)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (progn
            (with-current-buffer buf
              (set-window-parameter
               w 'glitnir-ruler
               `(:width 12 :right 2 :height 4 :band ("#0000ff" . 1)
                        :marks ((1 0 6 ("#ff0000" . 1))
                                (,(save-excursion (goto-char 1) (forward-line 500) (point)) 0 6 ("#ff0000" . 1))
                                (,(point-max) 6 12 ("#00ff00" . 1)))))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,_ ,_ ,right ,_) (window-pixel-edges w))
                       (`(,_ ,ty ,tx1 ,ty1) (window-inside-pixel-edges w))
                       (h (- ty1 ty))
                       (x1 (- right (window-right-divider-width w) (window-scroll-bar-width w) 2))
                       (at (lambda (dx y) (glitnir-gui--pixel snap (list (+ (- x1 12) dx) y)))))
            (should (equal (funcall at 2 (+ ty 1)) '(255 0 0)))              ; the first line's mark, left lane
            (should (equal (funcall at 2 (+ ty (/ h 2) 1)) '(255 0 0)))      ; line 500's, half way down
            (should (equal (funcall at 9 (- ty1 2)) '(0 255 0)))             ; the end's, right lane, at the bottom
            (should (equal (funcall at 9 (+ ty 1)) '(0 0 255)))              ; the band: the top part is shown
            (should-not (equal (funcall at 9 (+ ty (/ h 2) 1)) '(0 0 255)))  ; not the middle
            (should-not (member (glitnir-gui--pixel snap (list (- tx1 3) (+ ty 1))) '((255 0 0) (0 0 255))))
            ;; the click: the strip's middle is the buffer's middle (one line either way)
            (let ((pos (glitnir-ruler-position (list w 'right-fringe (cons 0 (+ (- ty (nth 1 (window-pixel-edges w)))
                                                                                 (/ h 2)))
                                                     0 nil nil nil nil nil nil))))
              (should (< (abs (- (with-current-buffer buf (line-number-at-pos pos)) 501)) 2))))
          ;; a scroll: the band moves to the middle, the marks stay (point goes along, or redisplay brings the
          ;; window back to it)
          (progn
            (let ((p (with-current-buffer buf (save-excursion (goto-char 1) (forward-line 495) (point)))))
              (set-window-point w p)
              (set-window-start w p)))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,_ ,_ ,right ,_) (window-pixel-edges w))
                       (`(,_ ,ty ,_ ,ty1) (window-inside-pixel-edges w))
                       (h (- ty1 ty))
                       (x1 (- right (window-right-divider-width w) (window-scroll-bar-width w) 2))
                       (at (lambda (dx y) (glitnir-gui--pixel snap (list (+ (- x1 12) dx) y)))))
            (should (equal (funcall at 2 (+ ty 1)) '(255 0 0)))
            (should (equal (funcall at 9 (+ ty (/ h 2) 1)) '(0 0 255)))
            (should-not (equal (funcall at 9 (+ ty 1)) '(0 0 255)))))
      (set-window-parameter w 'glitnir-ruler nil)
      (set-window-fringes w nil nil)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/a-ruler-beside-a-ring ()
  "A window with a ring block and a ruler: the ring's line stays whole in the text area and the ruler's mark shows in
the strip (each its own layer, its own clip).
On Naglfar the strip and the ring are layers over the backing `glitnir-snapshot' reads: the screen's rows are
glitnir-nhmac-gui-tests.el's `nhmac-ruler-*'."
  (glitnir-gui--skip-unless-tree "nhmac-ruler-beside-a-ring")
  (let ((buf (glitnir-gui--text-buffer "*glitnir-ruler-ring*" 1000)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-fringes w nil 16)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (progn
            (with-current-buffer buf
              (set-window-parameter
               w 'glitnir-blocks
               `((bar ,(copy-marker 1) ,(save-excursion (goto-char 1) (forward-line 2) (point-marker))
                      :x1 6 :ring ("#ff0000" . 1))))
              (set-window-parameter w 'glitnir-ruler '(:width 12 :right 2 :height 4 :marks ((1 0 12 ("#00ff00" . 1)))))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,_ ,_ ,right ,_) (window-pixel-edges w))
                       (`(,x ,ty ,_ ,_) (window-inside-pixel-edges w))
                       (x1 (- right (window-right-divider-width w) (window-scroll-bar-width w) 2)))
            (should (equal (glitnir-gui--pixel snap (list (+ x 2) (+ ty 1))) '(255 0 0)))
            (should (equal (glitnir-gui--pixel snap (list (- x1 6) (+ ty 1))) '(0 255 0)))))
      (set-window-parameter w 'glitnir-blocks nil)
      (set-window-parameter w 'glitnir-ruler nil)
      (set-window-fringes w nil nil)
      (kill-buffer buf))))

;; The ruler as the window's scroll bar (the minimap design's state A, docs/specs/drgnx/2026-11-15-minimap-folding-
;; design.md §4 in the org folder): hidden while the text fits, a thumb inset and rounded, a view's span.

(defun glitnir-gui--strip-has (snap w rgb)
  "Whether any pixel of window W's ruler strip (12 wide, 2 in from its right edge) in SNAP is RGB."
  (pcase-let* ((`(,_ ,_ ,right ,_) (window-pixel-edges w))
               (`(,_ ,ty ,_ ,ty1) (window-inside-pixel-edges w))
               (x1 (- right (window-right-divider-width w) (window-scroll-bar-width w) 2))
               (found nil))
    (cl-loop for y from ty below ty1 by 2 until found
             do (cl-loop for x from (- x1 12) below x1 until found
                         do (when (equal (glitnir-gui--pixel snap (list x y)) rgb) (setq found t))))
    found))

(ert-deftest glitnir-gui/the-ruler-hides-when-the-text-fits ()
  "With `:hide-short', the strip draws nothing while the window shows all of its buffer, and the thumb and marks
once the text runs past the window; the text area keeps its width throughout (the fringe holds the strip either
way, so nothing reflows), and no Lisp decides it: the same parameter, only the text changes."
  (glitnir-gui--skip-unless-tree "nhmac-ruler-hides-when-the-text-fits")
  (let ((buf (glitnir-gui--text-buffer "*glitnir-ruler-short*" 10)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-fringes w nil 16)
          (set-window-start w 1)
          (set-window-parameter w 'glitnir-ruler
                                '(:width 12 :right 2 :height 4 :hide-short t :band ("#0000ff" . 1)
                                         :marks ((1 0 12 ("#ff0000" . 1)))))
          (let ((body (window-body-width w t)) (snap (glitnir-gui--painted)))
            (should-not (glitnir-gui--strip-has snap w '(0 0 255)))
            (should-not (glitnir-gui--strip-has snap w '(255 0 0)))
            ;; the text grows past the window: the thumb and the mark show
            (with-current-buffer buf
              (save-excursion (goto-char (point-max))
                              (dotimes (i 990) (insert (format "%5d %s\n" i (make-string 70 ?x))))))
            (setq snap (glitnir-gui--painted))
            (should (glitnir-gui--strip-has snap w '(0 0 255)))
            (should (glitnir-gui--strip-has snap w '(255 0 0)))
            (should (= (window-body-width w t) body))
            ;; and shrinks under it again: gone
            (with-current-buffer buf
              (delete-region (save-excursion (goto-char 1) (forward-line 10) (point)) (point-max)))
            (setq snap (glitnir-gui--painted))
            (should-not (glitnir-gui--strip-has snap w '(0 0 255)))
            (should (= (window-body-width w t) body))))
      (set-window-parameter w 'glitnir-ruler nil)
      (set-window-fringes w nil nil)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/the-ruler-hides-in-a-window-fitted-to-its-lines ()
  "A window whose body is exactly its buffer's lines (a header fitted to its text, the final newline's empty line
below the last row) shows all there is: with `:hide-short' the strip draws nothing."
  (glitnir-gui--skip-unless-tree "nhmac-ruler-hides-when-the-text-fits")
  (let* ((buf (get-buffer-create "*glitnir-ruler-fitted*"))
         (top (selected-window))
         (w (split-window top nil 'below)))
    (unwind-protect
        (progn
          (with-current-buffer buf
            (erase-buffer)
            (dotimes (i 3) (insert (format "line %d\n" i)))
            (setq-local mode-line-format nil))
          (set-window-buffer w buf)
          (set-window-fringes w nil 16)
          (set-window-start w 1)
          (let ((window-resize-pixelwise t)
                (want (* 3 (with-current-buffer buf (default-line-height)))))
            (window-resize w (- want (window-body-height w t)) nil t t)
            (should (= (window-body-height w t) want)))
          (set-window-parameter w 'glitnir-ruler
                                '(:width 12 :right 2 :height 4 :hide-short t :band ("#0000ff" . 1)
                                         :marks ((1 0 12 ("#ff0000" . 1)))))
          (let ((snap (glitnir-gui--painted)))
            (should-not (glitnir-gui--strip-has snap w '(0 0 255)))
            (should-not (glitnir-gui--strip-has snap w '(255 0 0)))))
      (set-window-parameter w 'glitnir-ruler nil)
      (delete-window w)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/the-ruler-thumb-is-inset-and-rounded ()
  "`:thumb-inset' 3 and `:radius' 3: the thumb fills only the strip's middle, 3 in from each side, and its top corner
is cut by the rounding (its corner pixel is not the thumb's colour, the pixel two rows in at its middle is)."
  (glitnir-gui--skip-unless-tree "nhmac-ruler-hides-when-the-text-fits")
  (let ((buf (glitnir-gui--text-buffer "*glitnir-ruler-thumb*" 1000)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-fringes w nil 16)
          (set-window-start w 1)
          (set-window-parameter w 'glitnir-ruler
                                '(:width 12 :right 2 :height 4 :thumb-inset 3 :radius 3 :band ("#0000ff" . 1)))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,_ ,_ ,right ,_) (window-pixel-edges w))
                       (`(,_ ,ty ,_ ,_) (window-inside-pixel-edges w))
                       (x1 (- right (window-right-divider-width w) (window-scroll-bar-width w) 2))
                       (at (lambda (dx y) (glitnir-gui--pixel snap (list (+ (- x1 12) dx) y)))))
            (should (equal (funcall at 5 (+ ty 4)) '(0 0 255)))   ; the thumb's middle
            (should (equal (funcall at 8 (+ ty 4)) '(0 0 255)))
            (should-not (equal (funcall at 1 (+ ty 4)) '(0 0 255))) ; the inset, left
            (should-not (equal (funcall at 10 (+ ty 4)) '(0 0 255))) ; and right
            (should-not (equal (funcall at 3 ty) '(0 0 255)))))   ; the rounded corner
      (set-window-parameter w 'glitnir-ruler nil)
      (set-window-fringes w nil nil)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/a-ruler-mark-runs-to-its-end ()
  "A mark with an END (a run of changed lines) runs from its position down to END's: lines 1 to 500 of 1,000 fill the
strip's top half in its lane, and nothing below."
  (glitnir-gui--skip-unless-tree "nhmac-ruler-hides-when-the-text-fits")
  (let ((buf (glitnir-gui--text-buffer "*glitnir-ruler-end*" 1000)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-fringes w nil 16)
          (set-window-start w 1)
          (set-window-parameter
           w 'glitnir-ruler
           `(:width 12 :right 2 :height 2
                    :marks ((1 0 4 ("#ff0000" . 1) 2 0
                               ,(with-current-buffer buf (save-excursion (goto-char 1) (forward-line 500) (point)))))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,_ ,_ ,right ,_) (window-pixel-edges w))
                       (`(,_ ,ty ,_ ,ty1) (window-inside-pixel-edges w))
                       (h (- ty1 ty))
                       (x1 (- right (window-right-divider-width w) (window-scroll-bar-width w) 2))
                       (at (lambda (dx y) (glitnir-gui--pixel snap (list (+ (- x1 12) dx) y)))))
            (should (equal (funcall at 2 (+ ty 1)) '(255 0 0)))
            (should (equal (funcall at 2 (+ ty (/ h 4))) '(255 0 0)))
            (should (equal (funcall at 2 (+ ty (/ h 2) -3)) '(255 0 0)))
            (should-not (equal (funcall at 2 (+ ty (* 3 (/ h 4)))) '(255 0 0)))))
      (set-window-parameter w 'glitnir-ruler nil)
      (set-window-fringes w nil nil)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/the-ruler-measures-visible-lines ()
  "With a line model (`nidhogg-lines-enable') the strip measures visible lines: lines 2 to 899 of 1,000 folded away,
a mark on line 950 sits half way down the strip (52 of 102 visible lines), not near its foot as its character
fraction would put it."
  (glitnir-gui--skip-unless-tree "nhmac-ruler-hides-when-the-text-fits")
  (skip-unless (fboundp 'nidhogg-lines-enable))
  (let ((buf (glitnir-gui--text-buffer "*glitnir-ruler-lines*" 1000)) (w (selected-window)))
    (unwind-protect
        (let* ((line (lambda (n) (with-current-buffer buf
                                   (save-excursion (goto-char 1) (forward-line n) (point)))))
               (beg (1- (funcall line 1))) (end (1- (funcall line 899)))
               (ov (with-current-buffer buf (make-overlay beg end))))
          (overlay-put ov 'invisible t)
          (nidhogg-lines-enable buf t)
          (nidhogg-lines-set-hidden buf (list (cons beg end)))
          (set-window-buffer w buf)
          (set-window-fringes w nil 16)
          (set-window-start w 1)
          (set-window-parameter w 'glitnir-ruler
                                `(:width 12 :right 2 :height 2
                                         :marks ((,(funcall line 949) 0 4 ("#ff0000" . 1)))))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,_ ,_ ,right ,_) (window-pixel-edges w))
                       (`(,_ ,ty ,_ ,ty1) (window-inside-pixel-edges w))
                       (h (- ty1 ty))
                       (x1 (- right (window-right-divider-width w) (window-scroll-bar-width w) 2))
                       (y (cl-loop for y from ty below ty1
                                   when (equal (glitnir-gui--pixel snap (list (+ (- x1 12) 2) y)) '(255 0 0))
                                   return (- y ty))))
            (should y)
            (should (< (* 0.4 h) y (* 0.6 h)))))
      (set-window-parameter w 'glitnir-ruler nil)
      (set-window-fringes w nil nil)
      (nidhogg-lines-disable buf)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/the-ruler-maps-a-view-span ()
  "`:span (500 100 1000)': the buffer holds rows 500 to 600 of a view of 1,000, so a window at the buffer's top shows
the thumb half way down the strip, not at its top; and with `:hide-short' the strip still shows, the view having
more rows than the buffer, though the buffer fits the window."
  (glitnir-gui--skip-unless-tree "nhmac-ruler-hides-when-the-text-fits")
  (let ((buf (glitnir-gui--text-buffer "*glitnir-ruler-span*" 10)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-fringes w nil 16)
          (set-window-start w 1)
          (set-window-parameter w 'glitnir-ruler
                                '(:width 12 :right 2 :height 4 :hide-short t :span (500 10 1000)
                                         :band ("#0000ff" . 1)))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,_ ,_ ,right ,_) (window-pixel-edges w))
                       (`(,_ ,ty ,_ ,ty1) (window-inside-pixel-edges w))
                       (h (- ty1 ty))
                       (x1 (- right (window-right-divider-width w) (window-scroll-bar-width w) 2))
                       (at (lambda (dx y) (glitnir-gui--pixel snap (list (+ (- x1 12) dx) y)))))
            (should (equal (funcall at 6 (+ ty (/ h 2) 1)) '(0 0 255)))
            (should-not (equal (funcall at 6 (+ ty 2)) '(0 0 255)))
            (should-not (equal (funcall at 6 (- ty1 3)) '(0 0 255)))))
      (set-window-parameter w 'glitnir-ruler nil)
      (set-window-fringes w nil nil)
      (kill-buffer buf))))

;; Edges.

(ert-deftest glitnir-gui/an-edge-joins-two-places-and-leaves-with-a-scroll ()
  "A window's `glitnir-edges': a line from one buffer place to another, drawn over the text in its colour, its points
where the places' glyphs are; scrolled past, its start sits one line above the text area, and an edge wholly above
the rows shown is not resolved (none of it is drawn)."
  (glitnir-gui--skip-unless-tree "nhmac-edges-drawn-like-cards")
  (let ((buf (glitnir-gui--text-buffer "*glitnir-edge*" 200)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (glitnir-gui--painted)
          (let* ((lh (with-current-buffer buf (line-pixel-height)))
                 (dy (/ lh 2))
                 (from (with-current-buffer buf (save-excursion (goto-char 1) (forward-line 1) (+ (point) 10))))
                 (to (+ from 30)))
            (set-window-parameter w 'glitnir-edges `(((,from 0 ,dy) (,to 0 ,dy) ("#ff0000" . 1))))
            (pcase-let* ((snap (glitnir-gui--painted))
                         (`(,x ,y . ,_) (window-inside-pixel-edges w))
                         (`(,fx . ,fy) (posn-x-y (posn-at-point from w)))
                         (`(,tx . ,_) (posn-x-y (posn-at-point to w))))
              (should (equal (glitnir-edges-resolved w) (list (list (cons fx (+ fy dy)) (cons tx (+ fy dy))))))
              (should (equal (glitnir-gui--pixel snap (list (+ x (/ (+ fx tx) 2)) (+ y fy dy))) '(255 0 0)))
              (should-not (equal (glitnir-gui--pixel snap (list (+ x (/ (+ fx tx) 2)) (+ y fy dy (/ lh 2) 2)))
                                 '(255 0 0))))
            ;; scrolled down past its start: the start of an edge that reaches the rows shown is one frame line above
            ;; the text area; the first edge, wholly above them, is not resolved.  Point goes along, or redisplay
            ;; brings the window back to it
            (let ((p (with-current-buffer buf (save-excursion (goto-char 1) (forward-line 20) (point))))
                  (far (with-current-buffer buf (save-excursion (goto-char 1) (forward-line 22) (+ (point) 10)))))
              (set-window-parameter w 'glitnir-edges `(((,from 0 ,dy) (,to 0 ,dy) ("#ff0000" . 1))
                                                       ((,from 0 ,dy) (,far 0 ,dy) ("#ff0000" . 1))))
              (set-window-point w p)
              (set-window-start w p))
            (glitnir-gui--painted)
            (pcase-let ((`(,gone ((,_ . ,y0) ,_)) (glitnir-edges-resolved w)))
              (should-not gone)
              (should (= y0 (- (frame-char-height)))))))
      (set-window-parameter w 'glitnir-edges nil)
      (kill-buffer buf))))

;; The stacking list.

(defun glitnir-gui--children (n)
  "N child frames of the selected frame, made and shown one after the other."
  (let ((b (glitnir-gui--text-buffer "*glitnir-stack*" 5)))
    (cl-loop for i below n
             collect (glitnir-gui--child b t (nth (% i 4) '("#00ffff" "#ffff00" "#ff00ff" "#00ff00"))))))

(defun glitnir-gui--nested (parent)
  "A shown child frame of PARENT."
  (let* ((root (selected-frame))
         (f (make-frame `((parent-frame . ,parent) (minibuffer . nil) (left . 10) (top . 10)
                          (width . 20) (height . 5) (background-color . "#0000ff")
                          (visibility . nil) (no-accept-focus . t) (no-other-frame . t)
                          (undecorated . t)))))
    (select-frame root)
    (make-frame-visible f)
    f))

(defun glitnir-gui--delete (frames)
  (dolist (f (reverse frames))
    (when (frame-live-p f) (delete-frame f t)))
  (when (get-buffer "*glitnir-stack*") (kill-buffer "*glitnir-stack*")))

(ert-deftest glitnir-gui/stacking-follows-raise-lower ()
  (glitnir-gui--skip-unless-tree)
  (let ((fs (glitnir-gui--children 3)))
    (unwind-protect
        (pcase-let ((`(,a ,b ,c) fs))
          (glitnir-gui--painted)
          (should (cl-every #'frame-visible-p fs))
          (should (equal (glitnir--stacking) (list a b c)))
          (raise-frame a)
          (should (equal (glitnir--stacking) (list b c a)))
          (lower-frame c)
          (should (equal (glitnir--stacking) (list c b a)))
          (raise-frame b) (redisplay t)
          (should (equal (glitnir--stacking) (list c a b))))
      (glitnir-gui--delete fs))))

(ert-deftest glitnir-gui/map-raises-the-node ()
  (glitnir-gui--skip-unless-tree)
  (let ((fs (glitnir-gui--children 2)))
    (unwind-protect
        (pcase-let ((`(,a ,b) fs))
          (glitnir-gui--painted)
          (should (equal (glitnir--stacking) (list a b)))
          (make-frame-invisible a) (glitnir-gui--painted)
          (make-frame-visible a) (glitnir-gui--painted)
          (should (equal (glitnir--stacking) (list b a))))
      (glitnir-gui--delete fs))))

(ert-deftest glitnir-gui/zgroup-raises-and-lowers-child-frames ()
  (glitnir-gui--skip-unless-tree)
  (let ((fs (glitnir-gui--children 2)))
    (unwind-protect
        (pcase-let ((`(,a ,b) fs))
          (glitnir-gui--painted)
          (set-frame-parameter a 'z-group 'above) (redisplay t)
          (should (equal (glitnir--stacking) (list b a)))
          (set-frame-parameter a 'z-group nil)
          (set-frame-parameter b 'z-group 'below) (redisplay t)
          (should (equal (glitnir--stacking) (list b a))))
      (glitnir-gui--delete fs))))

(ert-deftest glitnir-gui/reparent-moves-the-node ()
  (glitnir-gui--skip-unless-tree)
  (let ((fs (glitnir-gui--children 3)))
    (unwind-protect
        (pcase-let ((`(,a ,b ,c) fs))
          (glitnir-gui--painted)
          ;; a goes on top of c's child frames, inside c
          (set-frame-parameter a 'parent-frame c) (redisplay t)
          (should (eq (frame-parent a) c))
          (should (equal (glitnir--stacking) (list b c a)))
          ;; b goes into a, inside c
          (set-frame-parameter b 'parent-frame a) (redisplay t)
          (should (equal (glitnir--stacking) (list c a b))))
      (glitnir-gui--delete fs))))

(ert-deftest glitnir-gui/unparent-makes-a-root ()
  (glitnir-gui--skip-unless-tree)
  (let* ((root (selected-frame)) (fs (glitnir-gui--children 2)) (d nil))
    (unwind-protect
        (pcase-let ((`(,a ,b) fs))
          (glitnir-gui--painted)
          (set-frame-parameter a 'parent-frame nil)
          (select-frame root)
          (should (gui-runner-wait (lambda () (glitnir-gui--painted) (glitnir-active-p a))))
          (should-not (frame-parent a))
          (should (equal (glitnir--stacking) (list b)))
          (should (glitnir-active-p a))
          (should (null (glitnir--stacking a)))
          ;; a root of its own: its child frames stack in its tree
          (setq d (glitnir-gui--nested a))
          (redisplay t)
          (should (equal (glitnir--stacking a) (list d)))
          (should (equal (glitnir--stacking) (list b))))
      (select-frame root)
      (glitnir-gui--delete (append fs (list d))))))

(ert-deftest glitnir-gui/nested-child-stacks-in-its-parent ()
  (glitnir-gui--skip-unless-tree)
  (let* ((fs (glitnir-gui--children 2)) (c nil))
    (unwind-protect
        (pcase-let ((`(,a ,b) fs))
          (setq c (glitnir-gui--nested a))
          (glitnir-gui--painted)
          (should (equal (glitnir--stacking) (list a c b)))
          (raise-frame c) (redisplay t)
          (should (equal (glitnir--stacking) (list a c b))) ; inside a, below b
          (raise-frame a) (redisplay t)
          (should (equal (glitnir--stacking) (list b a c))))
      (glitnir-gui--delete (append fs (list c))))))

(ert-deftest glitnir-gui/delete-child-from-a-timer ()
  (glitnir-gui--skip-unless-tree)
  ;; A timer deletes a shown child, then the parent of a shown nested child: the tree loses them and stays active.
  (let* ((fs (glitnir-gui--children 2)) (c nil) (deleted nil))
    (cl-flet ((delete-later (f)
                (run-at-time 0 nil (lambda () (delete-frame f t) (setq deleted t)))
                (should (gui-runner-wait (lambda () deleted)))
                (setq deleted nil)
                (should-not (frame-live-p f))))
      (unwind-protect
          (pcase-let ((`(,a ,b) fs))
            (glitnir-gui--painted)
            (delete-later a)
            (should (equal (glitnir--stacking) (list b)))
            ;; the parent of a shown nested child
            (setq c (glitnir-gui--nested b))
            (glitnir-gui--painted)
            (should (equal (glitnir--stacking) (list b c)))
            (delete-later b)
            (should-not (frame-live-p c))
            (should (null (glitnir--stacking)))
            (should (glitnir-active-p)))
        (glitnir-gui--delete (append fs (list c)))))))

;; The Lisp surface: chrome resolved from parameters and faces reaches the shell.

(defun glitnir-gui--chromed (&rest params)
  "A shown child frame with PARAMS (corner-radius, shadow ...) as frame parameters."
  (let* ((b (glitnir-gui--text-buffer "*glitnir-chrome*" 5))
         (c (glitnir-gui--child b t "#00ffff")))
    (modify-frame-parameters c params)
    (glitnir-gui--painted)
    c))

(ert-deftest glitnir-gui/chrome-reaches-the-shell ()
  (glitnir-gui--skip-unless-tree)
  (let ((c (glitnir-gui--chromed '(corner-radius . 8)
                                 '(shadow . (:blur 16 :offset (0 . 6) :spread 2 :opacity 0.35)))))
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
          ;; opting out removes it
          (modify-frame-parameters c '((corner-radius . 0) (shadow . nil)))
          (glitnir-gui--painted)
          (should-not (glitnir--chrome c)))
      (glitnir-gui--delete (list c)))))

(ert-deftest glitnir-gui/per-frame-shadow-face-follows ()
  (glitnir-gui--skip-unless-tree)
  (let ((c (glitnir-gui--chromed '(shadow . (:blur 16 :opacity 0.35)))))
    (unwind-protect
        (progn
          (should (equal (plist-get (plist-get (glitnir--chrome c) :shadow) :color) '(0 0 0)))
          ;; a colour set outside a theme is pushed by `glitnir-refresh' (redisplay reads no colour)
          (set-face-foreground 'child-frame-shadow "red" c)
          (glitnir-refresh c)
          (glitnir-gui--painted)
          (should (equal (plist-get (plist-get (glitnir--chrome c) :shadow) :color) '(255 0 0))))
      (glitnir-gui--delete (list c)))))

;; Child frames in the picture.  The root shows magenta under the popups (cyan, empty), so a rounded corner and a shadow
;; are told apart by their pixels.

(defconst glitnir-gui--chrome '((corner-radius . 8) (shadow . (:blur 16 :offset (0 . 6) :spread 0 :opacity 0.35)))
  "drgnx's popup defaults.")

(deftheme glitnir-gui-test "Colours the child-frame faces (glitnir-gui/theme-switch-recolours-visible-popup).")
(custom-theme-set-faces 'glitnir-gui-test
                        '(child-frame-shadow ((t :foreground "red")))
                        '(child-frame-border ((t :background "blue"))))

(defun glitnir-gui--under ()
  "Show a buffer whose text is magenta from its second line on, point at its start, in the selected window."
  (let ((b (get-buffer-create "*glitnir-under*")))
    (with-current-buffer b
      (erase-buffer)
      (insert "top\n")
      (dotimes (_ 80)
        (insert (propertize (make-string 150 ?M) 'face '(:background "#ff00ff" :foreground "#ff00ff" :extend t))
                "\n"))
      (goto-char (point-min)))
    (switch-to-buffer b)
    (redisplay t)))

(defun glitnir-gui--popup (params &optional left top)
  "A shown child frame at LEFT, TOP (default 200, 200), cyan, empty, 24x6 characters, with the frame PARAMS.
PARAMS come first, so they win over the defaults here."
  (let* ((root (selected-frame))
         (f (make-frame `(,@params (parent-frame . ,root) (minibuffer . nil) (left . ,(or left 200)) (top . ,(or top 200))
                          (width . 24) (height . 6) (background-color . "#00ffff")
                          (visibility . nil) (no-accept-focus . t) (no-other-frame . t)
                          (undecorated . t) (internal-border-width . 1)))))
    (select-frame root)
    (set-window-buffer (frame-root-window f) (get-buffer-create "*glitnir-pop*"))
    (make-frame-visible f)
    (glitnir-gui--painted)
    f))

(defun glitnir-gui--rect (c)
  "C's native area in its parent's picture, (X Y W H) in device pixels."
  (pcase-let ((`(,x . ,y) (frame-position c)))
    (list x y (frame-pixel-width c) (frame-pixel-height c))))

(defun glitnir-gui--pixel (snap xy)
  "The (R G B) of device pixel XY, (X Y), in SNAP, a raw snapshot."
  (pcase-let ((`(,w ,_ ,bytes) snap) (`(,x ,y) xy))
    (let ((i (* 4 (+ x (* y w)))))      ; B G R A
      (list (aref bytes (+ i 2)) (aref bytes (1+ i)) (aref bytes i)))))

(defun glitnir-gui--mid (rect)
  (list (+ (nth 0 rect) (/ (nth 2 rect) 2)) (+ (nth 1 rect) (/ (nth 3 rect) 2))))

(defun glitnir-gui--around (c)
  "C's rectangle grown by 40 logical pixels on each side, its shadow's reach."
  (pcase-let ((`(,x ,y ,w ,h) (glitnir-gui--rect c)) (m (round (* 40 (frame-scale-factor)))))
    (list (- x m) (- y m) (+ w m m) (+ h m m))))

(defun glitnir-gui--differ (a b rect)
  "The number of pixels of RECT (X Y W H, device pixels) whose colour differs between the raw snapshots A and B."
  (pcase-let ((`(,w ,h ,ba) a) (`(,_ ,_ ,bb) b) (`(,x ,y ,rw ,rh) rect) (n 0))
    (cl-loop for yy from (max y 0) below (min (+ y rh) h) do
             (cl-loop for xx from (max x 0) below (min (+ x rw) w) do
                      (let ((i (* 4 (+ xx (* yy w)))))
                        (unless (and (= (aref ba i) (aref bb i)) (= (aref ba (1+ i)) (aref bb (1+ i)))
                                     (= (aref ba (+ i 2)) (aref bb (+ i 2))))
                          (cl-incf n)))))
    n))

(defun glitnir-gui--off (frames)
  "A raw snapshot of the selected frame with it and FRAMES redrawn whole with Glitnir off, redrawn again after."
  (prog1 (let ((glitnir-enable nil))
           (dolist (f (cons (selected-frame) frames)) (redraw-frame f))
           (glitnir-gui--painted))
    (dolist (f (cons (selected-frame) frames)) (redraw-frame f))
    (glitnir-gui--painted)))

(ert-deftest glitnir-gui/corner-shows-what-is-beneath ()
  :tags '(gnome-smoke)
  (glitnir-gui--skip-unless-tree)
  (glitnir-gui--under)
  (let ((c nil))
    (unwind-protect
        (progn
          (setq c (glitnir-gui--popup '((corner-radius . 8) (shadow . nil))))
          (pcase-let ((snap (glitnir-snapshot nil 'raw)) (`(,x ,y ,w ,h) (glitnir-gui--rect c)))
            (should (equal (glitnir-gui--pixel snap (list x y)) '(255 0 255)))
            (should (equal (glitnir-gui--pixel snap (list (+ x w -1) (+ y h -1))) '(255 0 255)))
            (should (equal (glitnir-gui--pixel snap (glitnir-gui--mid (list x y w h))) '(0 255 255)))
            ;; a straight edge is the popup's own border
            (should-not (equal (glitnir-gui--pixel snap (list x (+ y (/ h 2)))) '(255 0 255)))))
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest glitnir-gui/shadow-darkens-below ()
  :tags '(gnome-smoke)
  (glitnir-gui--skip-unless-tree)
  (glitnir-gui--under)
  (let ((c nil))
    (unwind-protect
        (progn
          (setq c (glitnir-gui--popup '((corner-radius . 0) (shadow . (:blur 16 :offset (0 . 6) :opacity 0.35)))))
          (pcase-let* ((snap (glitnir-snapshot nil 'raw)) (s (frame-scale-factor))
                       (`(,x ,y ,w ,h) (glitnir-gui--rect c)) (mx (+ x (/ w 2))))
            (let ((below (glitnir-gui--pixel snap (list mx (+ y h (round (* 4 s)))))))
              (should (< (nth 0 below) 245))
              (should (< (nth 2 below) 245)))
            (should (equal (glitnir-gui--pixel snap (list mx (+ y h (round (* 80 s))))) '(255 0 255)))
            (should (equal (glitnir-gui--pixel snap (glitnir-gui--mid (list x y w h))) '(0 255 255)))))
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest glitnir-gui/no-residue-after-hide ()
  (glitnir-gui--skip-unless-tree)
  (glitnir-gui--under)
  (let ((c nil) (before (glitnir-gui--painted)))
    (unwind-protect
        (progn
          (setq c (glitnir-gui--popup glitnir-gui--chrome))
          (let ((rect (glitnir-gui--around c)))
            (set-frame-position c 230 215) (glitnir-gui--painted)
            (make-frame-invisible c)
            (should (= (glitnir-gui--differ before (glitnir-gui--painted) rect) 0))))
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest glitnir-gui/geometry-parity ()
  (glitnir-gui--skip-unless-tree)
  ;; Chrome widens a child frame's surface, never its frame: its geometry after each Lisp call is the same without.
  (glitnir-gui--under)
  (cl-flet ((run (params)
              (let ((out nil) (c nil))
                (unwind-protect
                    (cl-flet ((note () (glitnir-gui--painted)
                                (push (list (frame-edges c) (frame-edges c 'native-edges) (frame-position c))
                                      out)))
                      (setq c (glitnir-gui--popup params)) (note)
                      (set-frame-position c 260 230) (note)
                      (set-frame-size c 30 8) (note)
                      (make-frame-invisible c) (note)
                      (make-frame-visible c) (note)
                      (modify-frame-parameters c '((left . 180) (top . 210))) (note)
                      (nreverse out))
                  (when (frame-live-p c) (delete-frame c t))))))
    (should (equal (run glitnir-gui--chrome) (run nil)))))

(ert-deftest glitnir-gui/no-chrome-child-identical ()
  (glitnir-gui--skip-unless-tree)
  (glitnir-gui--under)
  (let ((c nil) (old child-frame-chrome))
    (unwind-protect
        (progn
          (setq child-frame-chrome glitnir-gui--chrome)
          (setq c (glitnir-gui--popup '((corner-radius . 0) (shadow . nil))))
          (let ((rect (glitnir-gui--around c)) (on (glitnir-gui--painted)))
            (should-not (glitnir--chrome c))
            (should (= (glitnir-gui--differ on (glitnir-gui--off (list c)) rect) 0))))
      (setq child-frame-chrome old)
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest glitnir-gui/shadowed-child-off-shows-what-is-beneath ()
  (glitnir-gui--skip-unless-tree)
  ;; With Glitnir off a child frame is its own pixels: the room its shadow takes around it shows what lies beneath, as
  ;; around a child without chrome, never the child's background (pattern D: one picture, read back).
  (glitnir-gui--under)
  (let ((c nil))
    (unwind-protect
        (progn
          (setq c (glitnir-gui--popup glitnir-gui--chrome))
          (should (glitnir--chrome c))
          (pcase-let* ((off (glitnir-gui--off (list c)))
                       (`(,x ,y ,w ,h) (glitnir-gui--rect c)) (d (round (* 4 (frame-scale-factor)))))
            (should (equal (glitnir-gui--pixel off (glitnir-gui--mid (list x y w h))) '(0 255 255)))
            (dolist (p (list (list (+ x (/ w 2)) (+ y h d)) (list (+ x (/ w 2)) (- y d))
                             (list (- x d) (+ y (/ h 2))) (list (+ x w d) (+ y (/ h 2)))))
              (should (equal (list p (glitnir-gui--pixel off p)) (list p '(255 0 255)))))))
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest glitnir-gui/decorated-child-identical ()
  (glitnir-gui--skip-unless-tree)
  ;; A decorated (not `undecorated') child frame gets no chrome, from the option or its own
  ;; parameters: its pixels are Glitnir off's.
  (glitnir-gui--under)
  (let ((c nil) (old child-frame-chrome))
    (unwind-protect
        (progn
          (setq child-frame-chrome glitnir-gui--chrome)
          (setq c (glitnir-gui--popup '((undecorated . nil) (corner-radius . 8))))
          (let ((rect (glitnir-gui--around c)) (on (glitnir-gui--painted)))
            (should-not (glitnir--chrome c))
            (should (= (glitnir-gui--differ on (glitnir-gui--off (list c)) rect) 0))))
      (setq child-frame-chrome old)
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest glitnir-gui/per-frame-border-face-follows ()
  (glitnir-gui--skip-unless-tree)
  (let ((c (glitnir-gui--chromed '(corner-radius . 8))))
    (unwind-protect
        (pcase-let ((`(,x ,y ,_ ,h) (glitnir-gui--rect c)))
          (let ((edge (list x (+ y (/ h 2)))))
            (set-face-background 'child-frame-border "red" c)
            (should (equal (glitnir-gui--pixel (glitnir-gui--painted) edge) '(255 0 0)))
            (set-face-background 'child-frame-border "blue" c)
            (should (equal (glitnir-gui--pixel (glitnir-gui--painted) edge) '(0 0 255)))))
      (glitnir-gui--delete (list c)))))

(ert-deftest glitnir-gui/theme-switch-recolours-visible-popup ()
  (glitnir-gui--skip-unless-tree)
  (glitnir-gui--under)
  (let ((c nil))
    (unwind-protect
        (progn
          (setq c (glitnir-gui--popup glitnir-gui--chrome))
          (pcase-let* ((s (frame-scale-factor)) (`(,x ,y ,w ,h) (glitnir-gui--rect c))
                       (below (list (+ x (/ w 2)) (+ y h (round (* 4 s)))))
                       (edge (list x (+ y (/ h 2)))))
            (should (< (nth 0 (glitnir-gui--pixel (glitnir-snapshot nil 'raw) below)) 245)) ; black on magenta
            (enable-theme 'glitnir-gui-test)
            (let ((snap (glitnir-gui--painted)))
              (should (>= (nth 0 (glitnir-gui--pixel snap below)) 250)) ; red on magenta
              (should (< (nth 2 (glitnir-gui--pixel snap below)) 245))
              (should (equal (glitnir-gui--pixel snap edge) '(0 0 255))))))
      (disable-theme 'glitnir-gui-test)
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest glitnir-gui/option-change-applies-at-once ()
  (glitnir-gui--skip-unless-tree)
  (glitnir-gui--under)
  (let ((c nil) (old child-frame-chrome))
    (unwind-protect
        (progn
          (setq child-frame-chrome nil)
          (setq c (glitnir-gui--popup nil))
          (pcase-let ((`(,x ,y . ,_) (glitnir-gui--rect c)))
            (should-not (equal (glitnir-gui--pixel (glitnir-snapshot nil 'raw) (list x y)) '(255 0 255)))
            ;; the option's watcher refreshes from a timer
            (setq child-frame-chrome '((corner-radius . 8)))
            (should (gui-runner-wait
                     (lambda () (equal (glitnir-gui--pixel (glitnir-gui--painted) (list x y)) '(255 0 255)))))
            (setq child-frame-chrome nil)
            (should (gui-runner-wait
                     (lambda () (not (equal (glitnir-gui--pixel (glitnir-gui--painted) (list x y)) '(255 0 255))))))))
      (setq child-frame-chrome old)
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest glitnir-gui/timer-move-without-redraw-commits ()
  (glitnir-gui--skip-unless-tree)
  ;; A timer moves a shown popup and nothing is redrawn: the picture shows it at its new place.
  (glitnir-gui--under)
  (let ((c nil))
    (unwind-protect
        (progn
          (setq c (glitnir-gui--popup glitnir-gui--chrome))
          (let ((old (glitnir-gui--rect c)))
            (run-at-time 0 nil (lambda () (set-frame-position c 520 360)))
            (should (gui-runner-wait (lambda () (not (equal (take 2 (glitnir-gui--rect c)) (take 2 old))))))
            (gui-runner-wait-settled)
            (let ((snap (glitnir-snapshot nil 'raw)))
              (should (equal (glitnir-gui--pixel snap (glitnir-gui--mid (glitnir-gui--rect c))) '(0 255 255)))
              (should (equal (glitnir-gui--pixel snap (glitnir-gui--mid old)) '(255 0 255))))))
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest glitnir-gui/hide-from-timer-paints-without-popup ()
  (glitnir-gui--skip-unless-tree)
  ;; A timer hides a shown popup and nothing is redrawn: the popup leaves the picture.
  (glitnir-gui--under)
  (let ((c nil) (before (glitnir-gui--painted)))
    (unwind-protect
        (progn
          (setq c (glitnir-gui--popup glitnir-gui--chrome))
          (let ((rect (glitnir-gui--around c)))
            (run-at-time 0 nil (lambda () (make-frame-invisible c)))
            (should (gui-runner-wait
                     (lambda () (= (glitnir-gui--differ before (glitnir-snapshot nil 'raw) rect) 0))))))
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest glitnir-gui/visible-bell-clears ()
  (glitnir-gui--skip-unless-tree)
  ;; The bell's inverted rows show, and once its 50 ms atimer has taken it back the picture is the one before.
  (glitnir-gui--under)
  (let* ((visible-bell t) (ring-bell-function nil)
         (top (nth 1 (window-pixel-edges (frame-first-window))))
         (row (list (/ (frame-pixel-width) 2) top (/ (frame-pixel-width) 4) (/ (frame-char-height) 2)))
         (before (glitnir-gui--painted)))
    (ding)
    (should (> (glitnir-gui--differ before (glitnir-snapshot nil 'raw) row) 0))
    (should (gui-runner-wait (lambda () (= (glitnir-gui--differ before (glitnir-snapshot nil 'raw) row) 0))))))

;; Echo-area prompts: a frame whose echo area floats (its minibuffer in a child frame, hidden while idle) sends plain
;; messages elsewhere through `set-message-function', and must tell a prompt, which has to stay where the user looks,
;; from a message.  C binds `nidhogg-echo-prompt' while it shows one.

(ert-deftest glitnir-gui/echo-prompt-marks-what-c-shows-as-a-prompt ()
  "`nidhogg-echo-prompt' is `prompt' while C shows a prompt in the echo area (`read-char', `read-event',
`read-char-exclusive', `read-key-sequence' and `interactive' \"c\"), and nil for a plain `message'."
  (let* ((seen nil)
         (set-message-function (lambda (m) (push (cons (substring-no-properties m) (bound-and-true-p nidhogg-echo-prompt))
                                                 seen)
                                 nil)))
    (message "plain")
    (let ((unread-command-events (list ?a))) (read-char "Char? "))
    (let ((unread-command-events (list ?a))) (read-event "Event? "))
    (let ((unread-command-events (list ?a))) (read-char-exclusive "Excl? "))
    (let ((unread-command-events (list ?a))) (read-key-sequence "Keys? "))
    (let ((unread-command-events (list ?a))) (call-interactively (lambda (c) (interactive "cArg: ") c)))
    (message nil)
    (should (equal (assoc "plain" seen) '("plain")))
    (dolist (p '("Char? " "Event? " "Excl? " "Arg: "))
      (should (equal (assoc p seen) (cons p 'prompt))))
    (should (eq 'prompt (cdr (seq-find (lambda (e) (string-prefix-p "Keys? " (car e))) seen))))))

;; The first frame born with a child-frame minibuffer (lisp/nidhogg-frame.el): a child Nidhogg whose early-init.el asks
;; for one under its window system's `window-system-default-frame-alist' entry makes its first frame that way, deletes
;; none and does not wait; with stock settings the first frame has its own minibuffer.

(defun glitnir-gui--first-frame (early)
  "Start a child Nidhogg whose early-init.el holds EARLY (a string of Lisp) and return what it saw once its frame was
up: (:deleted N :box BOOL :own-mini BOOL :frames N :exited BOOL), or (:timeout OUTPUT), all of the selected frame.  N
deleted counts graphical frames deleted while shown, N frames graphical frames (the initial terminal frame always goes); :exited says the child's
`kill-emacs' ended it within 20 s.  :all lists each frame's (GRAPHICAL CHILD OWN-MINI SELECTED)."
  (let* ((dir (make-temp-file "nh-first-frame" t))
         (out (expand-file-name "out.eld" dir))
         (emacs (expand-file-name invocation-name invocation-directory)))
    (unwind-protect
        (progn
          (with-temp-file (expand-file-name "early-init.el" dir)
            (insert "(message \"nh-first: nidhogg-frame %S\" (featurep 'nidhogg-frame))\n"
                    "(defvar nh-first--deleted 0)\n"
                    "(add-hook 'delete-frame-functions (lambda (f) (when (and (display-graphic-p f) (frame-visible-p f)) (setq nh-first--deleted (1+ nh-first--deleted)))))\n"
                    early "\n"))
          (with-temp-file (expand-file-name "init.el" dir)
            (insert (format "(add-hook 'window-setup-hook (lambda () (let* ((f (selected-frame)) (m (window-frame (minibuffer-window f)))) (with-temp-file %S (prin1 (list :deleted nh-first--deleted :box (and (not (eq m f)) (eq (frame-parent m) f)) :own-mini (eq m f) :frames (length (seq-filter #'display-graphic-p (frame-list))) :all (mapcar (lambda (x) (list (display-graphic-p x) (and (frame-parent x) t) (eq (window-frame (minibuffer-window x)) x) (eq x f))) (frame-list))) (current-buffer)))) (kill-emacs 0)))\n" out)))
          (let* ((log (generate-new-buffer " *nh-first-frame*"))
                 (p (start-process "nh-first-frame" log emacs "--init-directory" dir)))
            (set-process-query-on-exit-flag p nil)
            ;; its end, as Emacs is told of it
            (set-process-sentinel p (lambda (q _) (when (memq (process-status q) '(exit signal))
                                                    (process-put q 'ended t))))
            (unwind-protect
                (if (gui-runner-wait (lambda () (file-exists-p out)) 60)
                    (append (with-temp-buffer (insert-file-contents out) (read (current-buffer)))
                            (list :exited (and (gui-runner-wait (lambda () (process-get p 'ended)) 20) t)))
                  (list :timeout (with-current-buffer log (buffer-string))))
              (delete-process p)
              (kill-buffer log))))
      (delete-directory dir t))))

(ert-deftest glitnir-gui/first-frame-born-with-the-child-minibuffer ()
  "Asked for in early-init.el, the first frame has its minibuffer in a child frame from the start: no frame deleted."
  (let ((r (glitnir-gui--first-frame
            "(setf (alist-get 'minibuffer (alist-get initial-window-system window-system-default-frame-alist)) 'child-frame)")))
    (ert-info ((format "%S" r))
      (should (equal (list (plist-get r :deleted) (plist-get r :box) (plist-get r :own-mini) (plist-get r :frames)
                           (plist-get r :exited))
                     '(0 t nil 2 t))))))

(ert-deftest glitnir-gui/daemon-frame-born-with-the-child-minibuffer ()
  "A daemon's first graphical frame on a display, with the child-frame minibuffer asked for, leaves no other frame
behind: the minibuffer frame C made for it to borrow from while it was made goes once it has its box.  The daemon
makes it on this session's window system and display (round 2 on macOS: no WAYLAND_DISPLAY, so a nil display, a
terminal frame, \"Unknown terminal type\" and a 60 s wait)."
  (let* ((dir (make-temp-file "nh-daemon-frame" t))
         (ws (if (getenv "WAYLAND_DISPLAY") 'pgtk window-system))
         (display (or (getenv "WAYLAND_DISPLAY") (frame-parameter nil 'display) (symbol-name ws)))
         (out (expand-file-name "out.eld" dir))
         (emacs (expand-file-name invocation-name invocation-directory))
         (name (format "nh-daemon-frame-%d" (emacs-pid))))
    (unwind-protect
        (progn
          (with-temp-file (expand-file-name "early-init.el" dir)
            (insert (format "(setf (alist-get 'minibuffer (alist-get '%s window-system-default-frame-alist)) 'child-frame)\n"
                            ws)))
          (with-temp-file (expand-file-name "init.el" dir)
            (insert (format "(add-hook 'after-init-hook (lambda () (run-at-time 0.2 nil (lambda () (let* ((f (make-frame-on-display %S)) (m (window-frame (minibuffer-window f)))) (with-temp-file %S (prin1 (list :box (and (not (eq m f)) (eq (frame-parent m) f)) :roots (length (seq-filter (lambda (x) (and (display-graphic-p x) (not (frame-parent x)))) (frame-list)))) (current-buffer))) (kill-emacs 0))))))\n" display out)))
          (let* ((log (generate-new-buffer " *nh-daemon-frame*"))
                 (p (start-process "nh-daemon-frame" log emacs (concat "--fg-daemon=" name) "--init-directory" dir)))
            (set-process-query-on-exit-flag p nil)
            (unwind-protect
                (let ((r (and (gui-runner-wait (lambda () (file-exists-p out)) 60)
                              (with-temp-buffer (insert-file-contents out) (read (current-buffer))))))
                  (ert-info ((format "%S" (or r (with-current-buffer log (buffer-string)))))
                    (should (equal r '(:box t :roots 1)))))
              (delete-process p)
              (kill-buffer log))))
      (delete-directory dir t))))

(ert-deftest glitnir-gui/first-frame-stock-settings ()
  "Asked for nothing, the first frame has its own minibuffer, as in stock Emacs."
  (let ((r (glitnir-gui--first-frame "")))
    (should (equal (plist-get r :own-mini) t))
    (should (equal (plist-get r :deleted) 0))
    (should (plist-get r :exited))))

(ert-deftest glitnir-gui/an-overlay-string-change-redisplays-only-its-lines ()
  "An inline value (an overlay's after-string at a line's end) changed at a stop redisplays only its lines, with line
numbers on and the current line's number in its own face; with `glitnir-confined-overlay-redisplay' off (stock
Emacs) the same change draws the window again from its start.  An `invisible' overlay still does."
  (skip-unless (featurep 'glitnir-confined-overlays))
  (let ((buf (generate-new-buffer "*confined-overlays*"))
        (frame (selected-frame))
        (weight (face-attribute 'line-number-current-line :weight (selected-frame))))
    (unwind-protect
        (progn
          (switch-to-buffer buf)
          (delete-other-windows)
          ;; as `prog-mode' does: without a fixed direction try_window_id always gives up
          (setq bidi-paragraph-direction 'left-to-right)
          (dotimes (i 200) (insert (format "line %d of the program\n" i)))
          (goto-char (point-min))
          (forward-line 5)
          (set-face-attribute 'line-number-current-line frame :weight 'bold)
          (display-line-numbers-mode 1)
          (let ((o (make-overlay (pos-eol 12) (pos-eol 12)))
                (count (lambda (change)
                         (redisplay t)
                         (setq glitnir--redisplay-same-start 0 glitnir--redisplay-changed-lines 0
                               glitnir--redisplay-gave-up 0)
                         (funcall change)
                         (redisplay t)
                         (list glitnir--redisplay-same-start glitnir--redisplay-changed-lines))))
            (overlay-put o 'after-string "  qty = 20")
            (let ((n (funcall count (lambda () (overlay-put o 'after-string "  qty = 3")))))
              (should (equal (list (nth 0 n) glitnir--redisplay-gave-up) '(0 0)))
              (should (>= (nth 1 n) 1)))
            (should (string-match-p "qty = 3" (format "%S" (overlay-get o 'after-string))))
            (let ((glitnir-confined-overlay-redisplay nil))
              (should (>= (nth 0 (funcall count (lambda () (overlay-put o 'after-string "  qty = 4")))) 1)))
            ;; an invisible overlay may show outside its text: the window is drawn again
            (let ((h (make-overlay (pos-bol 30) (pos-bol 31))))
              (should (>= (nth 0 (funcall count (lambda () (overlay-put h 'invisible t)))) 1)))))
      (set-face-attribute 'line-number-current-line frame :weight weight)
      (kill-buffer buf))))

;; The minimap (glitnir-minimap): bars from the line model over the text's outer edge.

(defun glitnir-gui--shot (name)
  "Write the selected frame's picture to NAME-S.png (S the scale) under GLITNIR_GUI_OUT, when that is set."
  (when-let* ((dir (getenv "GLITNIR_GUI_OUT")))
    (let ((coding-system-for-write 'binary))
      (write-region (glitnir-snapshot nil) nil
                    (expand-file-name (format "%s-%s.png" name (frame-scale-factor)) dir) nil 'silent))))

(defun glitnir-gui--minimap-rect (w width)
  "The device-pixel rectangle (X Y W H) a minimap WIDTH logical pixels wide covers at window W's text's right edge."
  (pcase-let ((`(,_ ,ty ,tx1 ,ty1) (window-inside-pixel-edges w))
              (mw (round (* width (frame-scale-factor)))))
    (list (- tx1 mw) ty mw (- ty1 ty))))

(defun glitnir-gui--count-in (snap rect rgb)
  "How many pixels of RECT (X Y W H) in SNAP are RGB, every other row and column."
  (pcase-let ((`(,x ,y ,w ,h) rect) (n 0))
    (cl-loop for yy from y below (+ y h) by 2 do
             (cl-loop for xx from x below (+ x w) by 2 do
                      (when (equal (glitnir-gui--pixel snap (list xx yy)) rgb) (cl-incf n))))
    n))

(ert-deftest glitnir-gui/the-minimap-draws-the-lines-over-the-text-without-reflow ()
  "An open minimap draws the buffer's lines as bars (the line model's indents and ends) on its surface over the text's
right edge: the text keeps its width and every text pixel outside the minimap stays as it was.  Closed, nothing of it
shows."
  (glitnir-gui--skip-unless-tree)
  (skip-unless (fboundp 'nidhogg-lines-enable))
  (let ((buf (glitnir-gui--text-buffer "*glitnir-minimap*" 300)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (nidhogg-lines-enable buf t)
          (pcase-let* ((before (glitnir-gui--painted))
                       (body (window-body-width w t))
                       (mm (glitnir-gui--minimap-rect w 120))
                       (`(,tx ,ty ,_ ,_) (window-inside-pixel-edges w))
                       (text (list tx ty (- (nth 0 mm) tx) (nth 3 mm))))
            (set-window-parameter w 'glitnir-minimap
                                  '(:width 120 :open t :fill ("#203040" . 1) :text ("#ff00ff" . 1)))
            (let ((after (glitnir-gui--painted)))
              (glitnir-gui--shot "minimap-open")
              (should (= (window-body-width w t) body))
              (should (= 0 (glitnir-gui--differ before after text)))
              ;; the bars, the surface between them, and nothing of the text's own colours left in the minimap
              (should (> (glitnir-gui--count-in after mm '(255 0 255)) 100))
              (should (> (glitnir-gui--count-in after mm '(#x20 #x30 #x40)) 100))
              ;; a line's bar starts at its indent: line 0 ("    0 aaa…") is blank for its first 4 columns
              (let ((y (+ (nth 1 mm) (glitnir-gui--logical 1))))
                (should-not (equal (glitnir-gui--pixel after (list (+ (nth 0 mm) 1) y)) '(255 0 255)))
                (should (equal (glitnir-gui--pixel after (list (+ (nth 0 mm) (* 4 (max 1 (round (frame-scale-factor)))) 2) y))
                               '(255 0 255)))))
            (set-window-parameter w 'glitnir-minimap '(:width 120 :fill ("#203040" . 1) :text ("#ff00ff" . 1)))
            (should (= 0 (glitnir-gui--differ before (glitnir-gui--painted) (list tx ty (+ (nth 2 text) (nth 2 mm)) (nth 3 mm)))))))
      (set-window-parameter w 'glitnir-minimap nil)
      (nidhogg-lines-disable buf)
      (kill-buffer buf))))

(defvar nidhogg-treesit-highlight-slice)

(defconst glitnir-gui--dir (file-name-directory (or load-file-name buffer-file-name "")))

;; Task 11e: a whole-text highlight pass colours the minimap from idle slices.
(ert-deftest glitnir-gui/idle-highlight-slices-colour-the-minimap ()
  "A refold of a text larger than one slice colours the minimap's first
lines at once and the rest from the idle timer's slices; the minimap
shows each slice's colours with no other change to the window."
  (glitnir-gui--skip-unless-tree)
  (let ((grammars (expand-file-name "fixtures/grammars-cache"
                                    glitnir-gui--dir)))
    (skip-unless (and (fboundp 'nidhogg-lines-enable) (fboundp 'nidhogg-treesit-refold)
                      (file-exists-p (expand-file-name "libtree-sitter-c.so" grammars))))
    (let ((buf (get-buffer-create "*glitnir-minimap-slices*")) (w (selected-window))
          (treesit-extra-load-path (list grammars)))
      (unwind-protect
          (progn
            (with-current-buffer buf
              (dotimes (i 300)
                (insert (format "int f%d (void)\n{\n  return %d;\n}\n\n" i i)))
              (nidhogg-lines-enable buf t))
            (set-window-buffer w buf)
            (set-window-start w (with-current-buffer buf
                                  (save-excursion (goto-char (point-max)) (forward-line -20) (point))))
            (set-window-parameter w 'glitnir-minimap
                                  '(:width 120 :open t :fill ("#203040" . 1) :text ("#ff00ff" . 1)
                                    :classes ((type . "#00ff00"))))
            (let ((mm (glitnir-gui--minimap-rect w 120)) (green '(0 255 0)))
              (with-current-buffer buf
                (let ((p (treesit-parser-create 'c)) (nidhogg-treesit-highlight-slice 0))
                  (treesit-parser-root-node p)
                  (setq-local nidhogg-treesit-highlight-query
                              (treesit-query-compile 'c "(primitive_type) @type"))
                  (nidhogg-treesit-refold p)
                  ;; the window shows the text's end: not coloured yet
                  (should (= 0 (glitnir-gui--count-in (glitnir-gui--painted) mm green)))
                  ;; slices from the idle timer, a line each, until the last line.  The runner's rows run inside
                  ;; a timer, where Emacs is never idle (`current-idle-time' is nil), so the row fires the slice's
                  ;; idle timer itself, as an idle period would: each slice must arm the next one
                  (let ((deadline (+ (float-time) 60)))
                    (while (and (not (nidhogg-lines-classes buf 1495)) (< (float-time) deadline))
                      (let ((tm (seq-find (lambda (tm) (eq (timer--function tm) 'nidhogg-treesit--highlight-slice))
                                          timer-idle-list)))
                        (should tm)
                        (timer-event-handler tm))
                      (sit-for 0)))
                  (should (nidhogg-lines-classes buf 1495))))
              (sit-for 0.05)
              (gui-runner-wait-settled)
              (let ((after (glitnir-snapshot nil 'raw)))
                (glitnir-gui--shot "minimap-slices")
                (should (> (glitnir-gui--count-in after mm green) 10)))))
        (set-window-parameter w 'glitnir-minimap nil)
        (nidhogg-lines-disable buf)
        (kill-buffer buf)))))

(ert-deftest glitnir-gui/the-minimap-band-follows-a-scroll ()
  "The band over the lines the window shows moves with a scroll, with no Lisp of the minimap's in between: the same
parameter, only the window's start changes."
  (glitnir-gui--skip-unless-tree)
  (skip-unless (fboundp 'nidhogg-lines-enable))
  (let ((buf (glitnir-gui--text-buffer "*glitnir-minimap-band*" 300)) (w (selected-window)))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (nidhogg-lines-enable buf t)
          (set-window-parameter w 'glitnir-minimap
                                '(:width 120 :open t :fill ("#203040" . 1) :text ("#203040" . 1)
                                         :band ("#00ff00" . 1)))
          (pcase-let* ((mm (glitnir-gui--minimap-rect w 120))
                       (band-top (lambda (snap)
                                   (cl-loop for y from (nth 1 mm) below (+ (nth 1 mm) (nth 3 mm))
                                            when (equal (glitnir-gui--pixel snap (list (+ (nth 0 mm) 20) y))
                                                        '(0 255 0))
                                            return (- y (nth 1 mm)))))
                       (top (funcall band-top (glitnir-gui--painted))))
            (should (equal top 0))
            ;; point goes along, or redisplay brings the start back to it
            (let ((pos (with-current-buffer buf (save-excursion (goto-char 1) (forward-line 100) (point)))))
              (set-window-point w pos)
              (set-window-start w pos))
            (let ((snap (glitnir-gui--painted)))
              (glitnir-gui--shot "minimap-band-scrolled")
              (should (> (funcall band-top snap) 0)))))
      (set-window-parameter w 'glitnir-minimap nil)
      (nidhogg-lines-disable buf)
      (kill-buffer buf))))

;; The pointer over the minimap: gui.sh's virtual pointer (sway, NHWL_GUI_VPOINTER), aimed in frame pixels through a
;; mapping read off two points over the text.

(defun glitnir-gui--vp-start ()
  "Start the virtual pointer and wait for it; skip when gui.sh gave none."
  ;; (`skip-unless' exists only inside an `ert-deftest')
  (unless (getenv "NHWL_GUI_VPOINTER")
    (ert-skip "no virtual pointer (gui.sh --recorder on sway)"))
  (let* ((buf (generate-new-buffer " *glitnir-vp*"))
         (p (make-process :name "glitnir-vp" :buffer buf :noquery t :connection-type 'pipe
                          :command (list (getenv "NHWL_GUI_VPOINTER") "--virtual-pointer"))))
    (should (gui-runner-wait (lambda () (with-current-buffer buf (string-search "READY" (buffer-string)))) 5))
    p))

(defun glitnir-gui--vp-move (vp lx ly)
  "Move VP to LX, LY of a 1000 x 1000 layout; the frame pixel Emacs then has for the pointer, (X . Y), or nil."
  (let ((old (cdr (mouse-pixel-position))))
    ;; one pixel aside first: a move to where the pointer is sends no motion
    (process-send-string vp (format "move %d %d 1000 1000\nmove %d %d 1000 1000\n" (1+ lx) ly lx ly))
    (gui-runner-wait (lambda ()
                       (gui-runner-read-all)
                       (let ((xy (cdr (mouse-pixel-position))))
                         (and (car xy) (not (equal xy old)) xy)))
                     3)))

(defun glitnir-gui--vp-calibrate (vp)
  "The map from frame pixels to VP's layout, (AX AY SX SY): two points over the text read back."
  (let ((a (glitnir-gui--vp-move vp 300 300)) (b (glitnir-gui--vp-move vp 500 500)))
    (should (and a b))
    (list (- 300 (/ (car a) (/ (float (- (car b) (car a))) 200)))
          (- 300 (/ (cdr a) (/ (float (- (cdr b) (cdr a))) 200)))
          (/ (float (- (car b) (car a))) 200) (/ (float (- (cdr b) (cdr a))) 200))))

(defun glitnir-gui--vp-to (vp map x y)
  "Move VP to frame pixel X, Y through MAP (`glitnir-gui--vp-calibrate'), then let the session run a moment."
  (pcase-let ((`(,ax ,ay ,sx ,sy) map))
    (process-send-string vp (format "move %d %d 1000 1000\n" (round (+ ax (/ x sx))) (round (+ ay (/ y sy))))))
  (gui-runner-observe 0.05))

(defmacro glitnir-gui--with-hover-minimap (spec &rest body)
  "Run BODY with VP a virtual pointer, MAP its mapping, MM the minimap's rectangle, X1 the ruler strip's right edge,
MID the text's middle row, over a 300-line buffer with a line model, a ruler and minimap SPEC."
  (declare (indent 1))
  `(let ((buf (glitnir-gui--text-buffer "*glitnir-minimap-hover*" 300)) (w (selected-window)) (vp nil))
     (glitnir-gui--skip-unless-tree)
     (skip-unless (fboundp 'nidhogg-lines-enable))
     (unwind-protect
         (progn
           (set-window-buffer w buf)
           (set-window-start w 1)
           (set-window-fringes w nil 16)
           (nidhogg-lines-enable buf t)
           (set-window-parameter w 'glitnir-ruler '(:width 12 :right 2 :height 4 :band ("#0000ff" . 1)))
           (set-window-parameter w 'glitnir-minimap ,spec)
           (redisplay t)
           (setq vp (glitnir-gui--vp-start))
           (pcase-let* ((map (glitnir-gui--vp-calibrate vp))
                        (mm (glitnir-gui--minimap-rect w 120))
                        (`(,_ ,_ ,right ,_) (window-pixel-edges w))
                        (`(,_ ,ty ,_ ,ty1) (window-inside-pixel-edges w))
                        (x1 (- right (window-right-divider-width w) (window-scroll-bar-width w) 2))
                        (mid (/ (+ ty ty1) 2)))
             (ignore map mm x1 mid)
             ,@body))
       (when vp (delete-process vp))
       (set-window-parameter w 'glitnir-minimap nil)
       (set-window-parameter w 'glitnir-ruler nil)
       (set-window-fringes w nil nil)
       (nidhogg-lines-disable buf)
       (kill-buffer buf))))

(defun glitnir-gui--bars-p (mm)
  "Whether the minimap's bars (#ff00ff) show in MM."
  (> (glitnir-gui--count-in (glitnir-gui--painted) mm '(255 0 255)) 100))

(ert-deftest glitnir-gui/the-pointer-on-the-strip-opens-the-minimap-and-leaving-closes-it ()
  "The pointer resting on the ruler strip opens the minimap (no Lisp: the glue's timer and a publish); over the
minimap it stays open; back over the text it closes, not before half a second (600 ms)."
  (glitnir-gui--with-hover-minimap '(:width 120 :fill ("#203040" . 1) :text ("#ff00ff" . 1))
    (should-not (glitnir-gui--bars-p mm))
    (glitnir-gui--vp-to vp map (- x1 6) mid)
    ;; (the glue's timer comes as an event: the wait reads them, as the command loop does)
    (should (gui-runner-wait (lambda () (gui-runner-read-all) (glitnir-gui--bars-p mm)) 3))
    (glitnir-gui--shot "minimap-hover-open")
    (glitnir-gui--vp-to vp map (+ (nth 0 mm) 60) mid)
    (gui-runner-observe 1.0)
    (should (glitnir-gui--bars-p mm))
    (let ((t0 (float-time)))
      (glitnir-gui--vp-to vp map (+ (nth 0 (window-inside-pixel-edges w)) 100) mid)
      (unless (gui-runner-wait (lambda () (gui-runner-read-all) (not (glitnir-gui--bars-p mm))) 3)
        (ert-fail (list :pointer (mouse-pixel-position) :minimap mm :x1 x1
                        :text (window-inside-pixel-edges w) :scale (frame-scale-factor))))
      (should (>= (- (float-time) t0) 0.5)))))

(ert-deftest glitnir-gui/no-mouse-face-or-help-echo-fires-under-the-minimap ()
  "Over the open minimap Emacs sees no motion (the glue sends it a leave): the text under it neither lights up its
`mouse-face', nor asks its `help-echo', nor calls its `mouse-sensor-functions'.  The same pointer over the text beside
the minimap first lights the face and calls the sensor (the row's control: the text and its properties work here)."
  (glitnir-gui--with-hover-minimap '(:width 120 :open t :fill ("#203040" . 1) :text ("#ff00ff" . 1))
    ;; lines long enough to wrap: text lies under the pointer over the minimap (past a line's end there is none)
    (with-current-buffer buf
      (goto-char (point-min))
      (while (not (eobp))
        (end-of-line)
        (insert (make-string 400 ?x))
        (forward-line 1)))
    (let* ((asked nil) (sensed nil) (mouse-sensor-enabled t)
           (ov (with-current-buffer buf (make-overlay (point-min) (point-max))))
           (tx (nth 0 (window-inside-pixel-edges w)))
           (beside (list tx (nth 1 mm) (- (nth 0 mm) tx) (nth 3 mm)))
           (cyan (lambda () (glitnir-gui--count-in (glitnir-gui--painted) beside '(0 255 255))))
           (fringe (lambda () (glitnir-gui--vp-to vp map (- tx 3) mid) (gui-runner-observe 0.5 #'gui-runner-read-all))))
      (overlay-put ov 'mouse-face '(:background "#00ffff"))
      (overlay-put ov 'help-echo (lambda (&rest _) (setq asked t) "echo"))
      (overlay-put ov 'mouse-sensor-functions (list (lambda (_w _p dir) (push dir sensed))))
      ;; the control: over the text beside the minimap all three fire
      (funcall fringe)
      (setq asked nil sensed nil)
      (glitnir-gui--vp-to vp map (+ tx 100) mid)
      (should (gui-runner-wait (lambda () (gui-runner-read-all) (memq 'entered sensed)) 3))
      (should (> (funcall cyan) 0))
      ;; off the text, then over the minimap: none of them
      (funcall fringe)
      (setq asked nil sensed nil)
      (glitnir-gui--vp-to vp map (+ (nth 0 mm) 60) mid)
      (gui-runner-observe 1.0 #'gui-runner-read-all)
      ;; Emacs never had the pointer over it: its last place is off the minimap
      (let ((xy (cdr (mouse-pixel-position))))
        (should-not (and (car xy) (>= (car xy) (nth 0 mm)) (< (car xy) (+ (nth 0 mm) (nth 2 mm))))))
      (should-not asked)
      (should-not (memq 'entered sensed))
      (should (= 0 (funcall cyan))))))

(ert-deftest glitnir-gui/a-click-on-the-minimap-scrolls-the-window-there ()
  "A click on the minimap's lower part scrolls the window so the row clicked is in its middle, through the
`glitnir-minimap' event (the window at the top: the minimap's rows are not scrolled, row = y / row px)."
  (glitnir-gui--with-hover-minimap '(:width 120 :open t :fill ("#203040" . 1) :text ("#ff00ff" . 1))
    (let* ((seen nil)
           (shown (with-current-buffer buf
                    (- (line-number-at-pos (window-end w t)) (line-number-at-pos (window-start w)))))
           ;; the glue's rounding, half away from zero (`round' is half to even: 2.5 is 2, the glue's 3)
           (row-px (floor (+ 0.5 (* 2 (frame-scale-factor)))))
           ;; 0.8 of the rows drawn (the 300 lines are shorter than the minimap)
           (dy (round (* 0.8 (min (nth 3 mm) (* 300 row-px)))))
           (row (/ dy row-px)))
      (let ((glitnir-minimap-functions (list (lambda (kind &rest _) (push kind seen)))))
        (glitnir-gui--vp-to vp map (+ (nth 0 mm) 60) (+ (nth 1 mm) dy))
        (process-send-string vp "press\nrelease\n")
        (let ((evs nil))
          (unless (gui-runner-wait (lambda () (gui-runner-read-all (lambda (e) (push e evs))) (memq 'click seen)) 3)
            ;; what came instead, for the failure's message
            (ert-fail (list :seen seen :events (reverse evs) :pointer (mouse-pixel-position) :minimap mm
                            :param (window-parameter w 'glitnir--minimap)))))
        (redisplay t)
        ;; the start centres the row: ROW less half the lines shown, within a line or two
        (let ((line (1- (with-current-buffer buf (line-number-at-pos (window-start w))))))
          (should (<= (abs (- line (- row (/ shown 2)))) 2)))))))

(ert-deftest glitnir-gui/with-reduced-motion-the-minimap-opens-in-its-first-frame ()
  "With reduced motion the pointer's open is final in its first frame: the first picture with any of the minimap's
surface has it over the whole width (a reveal grows from the outer edge, so a partial one leaves the inner edge
uncovered)."
  (let ((glitnir-reduced-motion t))
    (glitnir-gui--with-hover-minimap '(:width 120 :fill ("#203040" . 1) :text ("#203040" . 1))
      (let ((inner (list (+ (nth 0 mm) 1) (+ (nth 1 mm) 40)))
            (outer (list (+ (nth 0 mm) (nth 2 mm) -2) (+ (nth 1 mm) 40)))
            (first nil))
        (glitnir-gui--vp-to vp map (- x1 6) mid)
        (should (gui-runner-wait (lambda ()
                                   (gui-runner-read-all)
                                   (let ((snap (glitnir-snapshot nil 'raw)))
                                     (when (equal (glitnir-gui--pixel snap outer) '(#x20 #x30 #x40))
                                       (setq first snap))))
                                 3))
        (should (equal (glitnir-gui--pixel first inner) '(#x20 #x30 #x40)))))))

(ert-deftest glitnir-gui/the-minimap-trail-flash-and-dwell-parts ()
  "The trail draws a bar at the minimap's left edge on each visit's row, the newest strongest; a flash lights its row
in the accent for 400 ms, then it is gone with no Lisp in between; a rest on the marks lane is one `dwell-marks'
event, at most 80 ms after the dwell's 300 ms."
  (glitnir-gui--with-hover-minimap '(:width 120 :open t :fill ("#203040" . 1) :text ("#203040" . 1)
                                            :trail ("#ff0000" . 1) :trail-length 3 :accent "#00ff00")
    (let* ((row-px (floor (+ 0.5 (* 2 (frame-scale-factor)))))
           (pos (lambda (l) (with-current-buffer buf (save-excursion (goto-char (point-min)) (forward-line l) (point)))))
           (row-rect (lambda (l w) (list (nth 0 mm) (+ (nth 1 mm) (* l row-px)) w row-px)))
           (red (lambda (snap l) (glitnir-gui--count-where snap (funcall row-rect l row-px)
                                                           (lambda (r g b) (and (> r 40) (< g 80) (< b 90))))))
           (strength (lambda (snap l) (car (glitnir-gui--pixel snap (list (nth 0 mm) (+ (nth 1 mm) (* l row-px))))))))
      ;; (a bar is as wide as a row is tall: 2 logical px)
      (dolist (l '(5 10 30 50)) (nidhogg-lines-trail-note buf (funcall pos l)))
      ;; (the trail is read at a publish: one comes with the window's redisplay)
      (force-window-update w)
      (redisplay t)
      (should (gui-runner-wait (lambda () (> (funcall red (glitnir-snapshot nil 'raw) 50) 0)) 3))
      (let ((snap (glitnir-snapshot nil 'raw)))
        (should (> (funcall red snap 30) 0))
        (should (> (funcall red snap 10) 0))
        (should (= (funcall red snap 5) 0))
        (should (> (funcall strength snap 50) (funcall strength snap 30) (funcall strength snap 10))))
      (glitnir-gui--shot "minimap-trail")
      ;; the flash: lit, then gone after 400 ms with no redisplay asked
      (let ((green (lambda () (glitnir-gui--count-where (glitnir-snapshot nil 'raw) (funcall row-rect 80 (nth 2 mm))
                                                        (lambda (r g b) (and (> g 120) (< r 80) (< b 90)))))))
        (should (glitnir-minimap-flash w (funcall pos 80)))
        (should (gui-runner-wait (lambda () (> (funcall green) 0)) 1))
        (should (gui-runner-wait (lambda () (gui-runner-read-all) (= (funcall green) 0)) 2)))
      (nidhogg-lines-trail-clear buf)
      ;; the marks lane: the strip's 12 px at the outer edge
      (let* ((seen nil) (at nil)
             (glitnir-minimap-functions (list (lambda (kind &rest _) (push kind seen) (setq at (float-time)))))
             (lane-x (- (+ (nth 0 mm) (nth 2 mm)) (round (* 6 (frame-scale-factor)))))
             (t0 (float-time)))
        (glitnir-gui--vp-to vp map lane-x mid)
        (should (gui-runner-wait (lambda () (gui-runner-read-all) seen) 3))
        (gui-runner-observe 0.5 #'gui-runner-read-all)
        (should (equal seen '(dwell-marks)))
        (should (<= (- at t0) (+ 0.3 0.08 0.05)))))))

(ert-deftest glitnir-gui/the-minimap-search-hits-and-the-label-hover ()
  "The line model's search (X1) draws each hit as a `:hit' bar at its column on its row, as many as `how-many'
finds, the current hit ringed; the pointer onto a label is one `label-hover' event at its line (X7), off it one of
nil."
  (glitnir-gui--with-hover-minimap '(:width 120 :open t :fill ("#000000" . 1) :text ("#000000" . 1)
                                            :hit ("#ff0000" . 1) :band ("#000000" . 0) :pin-kind 9
                                            :pin ("#000000" . 0) :pin-names ["pinned"] :label-face default)
    (let* ((row-px (floor (+ 0.5 (* 2 (frame-scale-factor)))))
           (col-px (max 1 (floor (+ 0.5 (frame-scale-factor)))))
           (at (lambda (l) (+ 1 (* 77 l))))
           (rows (lambda (from n) (list (nth 0 mm) (+ (nth 1 mm) (* from row-px)) (nth 2 mm) (* n row-px))))
           (red (lambda (r g b) (and (> r 150) (< g 60) (< b 60))))
           (n (nidhogg-lines-search buf "kkkkk" t)))
      (while (eq n 'running) (setq n (nidhogg-lines-search-step buf)))
      (should (= n 168))
      (should (= n (with-current-buffer buf (how-many "kkkkk" (point-min) (point-max)))))
      ;; (the search redisplays the buffer's windows itself)
      (redisplay)
      (should (gui-runner-wait (lambda () (> (glitnir-gui--count-where (glitnir-snapshot nil 'raw) (funcall rows 10 1) red) 0))
                               3))
      (let ((snap (glitnir-snapshot nil 'raw)))
        (glitnir-gui--shot "minimap-hits")
        ;; the bars start at column 6, after the line number
        (should (= (glitnir-gui--count-where snap (list (nth 0 mm) (+ (nth 1 mm) (* 10 row-px)) (* 6 col-px) row-px) red) 0))
        (should (= (glitnir-gui--count-where snap (funcall rows 11 1) red) 0))
        (should (= (glitnir-gui--count-where snap (funcall rows 12 20) red) 0)))
      ;; the current hit: a ring 1 px outside its bar, into the next row
      (nidhogg-lines-search-current buf (+ (funcall at 10) 6))
      (redisplay)
      (should (gui-runner-wait (lambda () (> (glitnir-gui--count-where (glitnir-snapshot nil 'raw) (funcall rows 11 1) red) 0))
                               3))
      ;; a label at line 20 (a pin there): the pointer onto it, along it, then off
      (nidhogg-lines-set-marks buf 9 (list (vector (funcall at 20) (1- (funcall at 25)))))
      (force-window-update w)
      (redisplay t)
      (gui-runner-observe 0.3 #'gui-runner-read-all)
      (let* ((seen nil)
             (glitnir-minimap-functions (list (lambda (kind _w pos) (when (eq kind 'label-hover) (push pos seen))))))
        (glitnir-gui--vp-to vp map (+ (nth 0 mm) 30) (+ (nth 1 mm) (* 20 row-px) 3))
        (gui-runner-observe 0.3 #'gui-runner-read-all)
        (glitnir-gui--vp-to vp map (+ (nth 0 mm) 40) (+ (nth 1 mm) (* 20 row-px) 5))
        (gui-runner-observe 0.3 #'gui-runner-read-all)
        (glitnir-gui--vp-to vp map (+ (nth 0 mm) 30) (+ (nth 1 mm) (* 100 row-px)))
        (gui-runner-observe 0.3 #'gui-runner-read-all)
        (should (equal (reverse seen) (list (funcall at 20) nil))))
      (nidhogg-lines-search buf nil))))

(defun glitnir-gui--count-where (snap rect pred)
  "How many pixels of RECT (X Y W H) in SNAP satisfy PRED, called with R, G and B."
  (pcase-let ((`(,x ,y ,w ,h) rect) (n 0))
    (cl-loop for yy from (max 0 y) below (+ y h) do
             (cl-loop for xx from (max 0 x) below (+ x w) do
                      (when (apply pred (glitnir-gui--pixel snap (list xx yy))) (cl-incf n))))
    n))

(ert-deftest glitnir-gui/the-minimap-labels-the-definitions-and-the-mark-sections ()
  "An open minimap with `:labels' draws the hub's definitions of its `:file' (Sjá's labels layer) at their lines, a
nested one in `:sub', and the line model's MARK sections in `:mark', in capitals; with `marks-only' the definitions
go, and without `:labels' nothing of them is drawn.  A `:hunk-labels' label is drawn at its line in `:hunk', with
either kind of labels."
  (glitnir-gui--skip-unless-tree)
  (skip-unless (fboundp 'nidhogg-lines-set-section-regexp))
  (let ((buf (glitnir-gui--text-buffer "*glitnir-minimap-labels*" 300)) (w (selected-window))
        (file "/glitnir-gui/labels.el"))
    (unwind-protect
        (progn
          (with-current-buffer buf
            (dolist (l '((40 . ";;;; Section one") (120 . "// MARK: other part")))
              (goto-char (point-min))
              (forward-line (car l))
              (delete-region (point) (pos-eol))
              (insert (cdr l))))
          (set-window-buffer w buf)
          (set-window-start w 1)
          (nidhogg-lines-enable buf t)
          (nidhogg-lines-set-section-regexp buf "^(?:;;;;|//\\s*MARK:)\\s*(.+)$")
          (nidhogg-lines-enable buf t)
          (should (equal (mapcar #'cdr (nidhogg-lines-sections buf)) '("Section one" "other part")))
          (nidhogg-view-push
           (json-serialize
            `((event . "view-model")
              (data . ((minimap . ((file . ,file) (layer . "labels") (version . 1)
                                   (items . [[11 0 "function" "alpha"] [81 1 "method" "beta"]]))))))))
          (pcase-let* ((s (frame-scale-factor))
                       (row (floor (+ 0.5 (* 2 s))))
                       (box (floor (+ 0.5 (* 13 s))))
                       (mm (glitnir-gui--minimap-rect w 120))
                       (at (lambda (line) (list (nth 0 mm) (+ (nth 1 mm) (* line row)) (nth 2 mm) box)))
                       (red (lambda (r g b) (and (> r 180) (< g 80) (< b 80))))
                       (green (lambda (r g b) (and (< r 80) (> g 180) (< b 80))))
                       (yellow (lambda (r g b) (and (> r 180) (> g 180) (< b 80))))
                       (cyan (lambda (r g b) (and (< r 80) (> g 180) (> b 180))))
                       (spec (lambda (labels)
                               `(:width 120 :open t :fill ("#203040" . 1) :text ("#203040" . 1)
                                        :labels ,labels :file ,file :label ("#ff0000" . 1) :sub ("#00ff00" . 1)
                                        :accent ("#0000ff" . 1) :mark ("#ffff00" . 1) :label-face default
                                        :hunk ("#00ffff" . 1) :hunk-labels [[101 "hunk 1 · +2 −0"]]))))
            (set-window-parameter w 'glitnir-minimap (funcall spec t))
            (let ((snap (glitnir-gui--painted)))
              (glitnir-gui--shot "minimap-labels")
              (should (> (glitnir-gui--count-where snap (funcall at 10) red) 3))
              (should (> (glitnir-gui--count-where snap (funcall at 80) green) 3))
              (should (> (glitnir-gui--count-where snap (funcall at 40) yellow) 3))
              (should (> (glitnir-gui--count-where snap (funcall at 120) yellow) 3))
              ;; a hunk label at its line, in `:hunk'
              (should (> (glitnir-gui--count-where snap (funcall at 100) cyan) 3))
              ;; between them, only the surface
              (should (= 0 (glitnir-gui--count-where snap (funcall at 60) (lambda (r g _) (or (> r 180) (> g 180)))))))
            (set-window-parameter w 'glitnir-minimap (funcall spec 'marks-only))
            (let ((snap (glitnir-gui--painted)))
              (should (= 0 (glitnir-gui--count-where snap (funcall at 10) red)))
              (should (> (glitnir-gui--count-where snap (funcall at 40) yellow) 3))
              (should (> (glitnir-gui--count-where snap (funcall at 100) cyan) 3)))
            (set-window-parameter w 'glitnir-minimap (funcall spec nil))
            (let ((snap (glitnir-gui--painted)))
              (should (= 0 (glitnir-gui--count-where snap (funcall at 40) yellow)))
              (should (= 0 (glitnir-gui--count-where snap (funcall at 100) cyan))))))
      (set-window-parameter w 'glitnir-minimap nil)
      (nidhogg-view-minimap 'forget file)
      (nidhogg-lines-disable buf)
      (kill-buffer buf))))

;; Task 12: fold arrows in the fringe, fold pills in the minimap.
(defun glitnir-gui--fold-row (w pos)
  "The device-pixel rectangle (X Y W H) of the left fringe of window W on the screen row of POS."
  (pcase-let* ((`(,tx ,_ ,_ ,_) (window-inside-pixel-edges w))
               (`(,_ ,wy ,_ ,_) (window-pixel-edges w))
               (`(,_ ,y ,_ ,_ ,_ ,h) (pos-visible-in-window-p pos w t))
               (lf (car (window-fringes w))))
    (list (- tx lf) (+ wy y) lf (or h (frame-char-height)))))

(defun glitnir-gui--extent-where (snap rect pred)
  "The width from RECT's left to the rightmost pixel of RECT in SNAP satisfying PRED (0: none)."
  (pcase-let ((`(,x ,y ,w ,h) rect) (far 0))
    (cl-loop for yy from (max 0 y) below (+ y h) do
             (cl-loop for xx from (max 0 x) below (+ x w) do
                      (when (apply pred (glitnir-gui--pixel snap (list xx yy)))
                        (setq far (max far (1+ (- xx x)))))))
    far))

(ert-deftest glitnir-gui/fold-arrows-and-pills-show-the-folds ()
  "With `glitnir-fold-arrows' `always', a fold range's first line gets a chevron in the left fringe in `:colour', a
folded one (its lines hidden in the line model) in `:folded', other lines none.  An open minimap with `:pill' draws a
pill after the bar of the folded range's first line: the range's name when it has one, else `:pill-format''s count,
wider."
  (glitnir-gui--skip-unless-tree)
  (skip-unless (fboundp 'nidhogg-lines-set-folds))
  (let ((buf (glitnir-gui--text-buffer "*glitnir-fold-arrows*" 300)) (w (selected-window))
        (at (lambda (i) (+ 1 (* 77 i)))))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (set-window-fringes w (round (* 16 (frame-scale-factor))) nil)
          (nidhogg-lines-enable buf t)
          ;; folds on lines 5–8 and 10–15; lines 11–15 hidden as "zz"
          (nidhogg-lines-set-folds buf 7 (list (list (funcall at 5) (1- (funcall at 9)) 0)
                                              (list (funcall at 10) (1- (funcall at 16)) 0)))
          (nidhogg-lines-set-hidden buf (list (list (1- (funcall at 11)) (1- (funcall at 16)) "zz")))
          (let* ((lf (car (window-fringes w)))
                 (red (lambda (r g b) (and (> r 180) (< g 80) (< b 80))))
                 (green (lambda (r g b) (and (< r 80) (> g 180) (< b 80))))
                 (cyan (lambda (r g b) (and (< r 80) (> g 180) (> b 180)))))
            (set-window-parameter w 'glitnir-fold-arrows
                                  `(:mode always :size ,(- lf 4) :inset 2 :colour ("#ff0000" . 1) :folded ("#00ff00" . 1)))
            (force-window-update w)
            (let ((snap (glitnir-gui--painted)))
              (glitnir-gui--shot "fold-arrows")
              (should (> (glitnir-gui--count-where snap (glitnir-gui--fold-row w (funcall at 5)) red) 3))
              (should (= (glitnir-gui--count-where snap (glitnir-gui--fold-row w (funcall at 5)) green) 0))
              (should (> (glitnir-gui--count-where snap (glitnir-gui--fold-row w (funcall at 10)) green) 3))
              (should (= (glitnir-gui--count-where snap (glitnir-gui--fold-row w (funcall at 7))
                                                   (lambda (r g b) (or (funcall red r g b) (funcall green r g b))))
                         0)))
            (set-window-parameter w 'glitnir-fold-arrows nil)
            (pcase-let* ((s (frame-scale-factor))
                         (row (floor (+ 0.5 (* 2 s))))
                         (box (floor (+ 0.5 (* 13 s))))
                         (mm (glitnir-gui--minimap-rect w 120))
                         (line10 (list (nth 0 mm) (+ (nth 1 mm) (* 10 row)) (nth 2 mm) box))
                         (spec `(:width 120 :open t :fill ("#203040" . 1) :text ("#203040" . 1)
                                        :pill ("#00ffff" . 1) :label-face default)))
              (set-window-parameter w 'glitnir-minimap spec)
              (force-window-update w)
              (let* ((snap (glitnir-gui--painted))
                     (named (glitnir-gui--extent-where snap line10 cyan)))
                (glitnir-gui--shot "fold-pill")
                (should (> (glitnir-gui--count-where snap line10 cyan) 3))
                ;; no fold pill but this one
                (should (= 0 (glitnir-gui--count-where
                              snap (list (nth 0 mm) (+ (nth 1 mm) (* 30 row)) (nth 2 mm) box) cyan)))
                (nidhogg-lines-set-hidden buf (list (cons (1- (funcall at 11)) (1- (funcall at 16)))))
                (force-window-update w)
                (let ((counted (glitnir-gui--extent-where (glitnir-gui--painted) line10 cyan)))
                  (should (> counted (+ named (* 8 s)))))))))
      (set-window-parameter w 'glitnir-fold-arrows nil)
      (set-window-parameter w 'glitnir-minimap nil)
      (set-window-fringes w nil nil)
      (nidhogg-lines-disable buf)
      (kill-buffer buf))))

;; Tasks 13 and 17: heat layers and pinned regions in the minimap.
(ert-deftest glitnir-gui/the-minimap-heat-and-pin-bands ()
  "An open minimap with `:heat' colours each line's bar from Sjá's heat:NAME layer of its `:file': `:heat-high' at
value 1000, `:heat-low' at 0, `:text' where no item is; a heat layer not pushed leaves the bars in `:text'.  A mark of
`:pin-kind' with an end is a band over its lines in `:pin', its `:pin-names' label at its first line."
  (glitnir-gui--skip-unless-tree)
  (skip-unless (fboundp 'nidhogg-lines-set-marks))
  (let ((buf (glitnir-gui--text-buffer "*glitnir-minimap-heat*" 300)) (w (selected-window))
        (file "/glitnir-gui/heat.el")
        (at (lambda (i) (+ 1 (* 77 i)))))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (nidhogg-lines-enable buf t)
          (nidhogg-view-push
           (json-serialize
            `((event . "view-model")
              (data . ((minimap . ((file . ,file) (layer . "heat:blame") (version . 1)
                                   (items . [[11 20 1000 "2 days ago"] [31 40 0]]))))))))
          (pcase-let* ((s (frame-scale-factor))
                       (row (floor (+ 0.5 (* 2 s))))
                       (mm (glitnir-gui--minimap-rect w 120))
                       (rows (lambda (from n) (list (nth 0 mm) (+ (nth 1 mm) (* from row)) (nth 2 mm) (* n row))))
                       (red (lambda (r g b) (and (> r 180) (< g 80) (< b 80))))
                       (blue (lambda (r g b) (and (< r 80) (< g 80) (> b 180))))
                       (green (lambda (r g b) (and (< r 80) (> g 180) (< b 80))))
                       (spec (lambda (heat)
                               `(:width 120 :open t :fill ("#000000" . 1) :text ("#00ff00" . 1) :file ,file
                                        :heat ,heat :heat-low ("#0000ff" . 1) :heat-high ("#ff0000" . 1)))))
            (set-window-parameter w 'glitnir-minimap (funcall spec "blame"))
            (let ((snap (glitnir-gui--painted)))
              (glitnir-gui--shot "minimap-heat")
              (should (> (glitnir-gui--count-where snap (funcall rows 10 10) red) 50))
              (should (= (glitnir-gui--count-where snap (funcall rows 10 10) green) 0))
              (should (> (glitnir-gui--count-where snap (funcall rows 30 10) blue) 50))
              (should (> (glitnir-gui--count-where snap (funcall rows 50 10) green) 50))
              (should (= (glitnir-gui--count-where snap (funcall rows 50 10) red) 0)))
            (set-window-parameter w 'glitnir-minimap (funcall spec "coverage"))
            (let ((snap (glitnir-gui--painted)))
              (should (= (glitnir-gui--count-where snap (funcall rows 10 10) red) 0))
              (should (> (glitnir-gui--count-where snap (funcall rows 10 10) green) 50)))
            ;; a pin over lines 60–69, named
            (nidhogg-lines-set-marks buf 9 (list (vector (funcall at 60) (1- (funcall at 70)))))
            (set-window-parameter w 'glitnir-minimap
                                  `(:width 120 :open t :fill ("#000000" . 1) :text ("#000000" . 1) :band ("#808080" . 0.5)
                                           :pin-kind 9 :pin ("#00ffff" . 1) :pin-names ["pinned"] :label-face default))
            (force-window-update w)
            (let* ((snap (glitnir-gui--painted))
                   ;; (the window's own band may lie over it: teal under half grey is still more green and blue than red)
                   (teal (lambda (r g b) (and (< r 100) (> g (+ r 40)) (> b (+ r 40))))))
              (glitnir-gui--shot "minimap-pin")
              ;; the band covers its rows across the width; the label is at its first row; none past it
              (should (> (glitnir-gui--count-where snap (funcall rows 60 10) teal) (* 4 (nth 2 mm))))
              (should (= (glitnir-gui--count-where snap (funcall rows 80 10) teal) 0)))))
      (set-window-parameter w 'glitnir-minimap nil)
      (nidhogg-view-minimap 'forget file)
      (nidhogg-lines-disable buf)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/the-density-strip-draws-the-hubs-buckets ()
  "A `:density' minimap on a buffer with no line model draws the hub's density buckets of its `:file': the full rows
brightest, an `:error' mark at the outer edge on a bucket with errors, the band at the window's fraction."
  (glitnir-gui--skip-unless-tree)
  (skip-unless (fboundp 'nidhogg-view-push))
  (let ((buf (glitnir-gui--text-buffer "*glitnir-density*" 300)) (w (selected-window))
        (file "/glitnir-gui/big.log"))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (should-not (and (fboundp 'nidhogg-lines-describe) (nidhogg-lines-describe buf)))
          ;; 100 buckets: the first ten full, an error in bucket 50, the rest a tenth as full
          (nidhogg-view-push
           (json-serialize
            `((event . "view-model")
              (data . ((minimap . ((file . ,file) (layer . "density") (version . 1)
                                   (items . ((buckets . ,(vconcat (mapcar (lambda (i) (vector (if (< i 10) 1000 100)
                                                                                                (if (= i 50) 3 0) 0))
                                                                          (number-sequence 0 99))))
                                             (total . 100))))))))))
          (set-window-parameter w 'glitnir-minimap
                                `(:width 26 :open t :density t :file ,file :fill ("#000000" . 1) :text ("#ffffff" . 1)
                                         :band ("#000000" . 0) :error "#ff0000"))
          (pcase-let* ((mm (glitnir-gui--minimap-rect w 26))
                       (`(,x ,y ,mw ,h) mm)
                       (row (lambda (frac) (list x (+ y (round (* frac h))) (- mw (round (* 4 (frame-scale-factor)))) 1)))
                       (snap (glitnir-gui--painted))
                       (white (lambda (r g b) (and (> r 200) (> g 200) (> b 200))))
                       (dim (lambda (r g b) (and (> r 10) (< r 60) (= r g b))))
                       (red (lambda (r g b) (and (> r 200) (< g 60) (< b 60)))))
            (glitnir-gui--shot "minimap-density")
            (should (> (glitnir-gui--count-where snap (funcall row 0.05) white) 0))
            (should (> (glitnir-gui--count-where snap (funcall row 0.3) dim) 0))
            (should (= (glitnir-gui--count-where snap (funcall row 0.3) white) 0))
            (should (> (glitnir-gui--count-where snap (list (+ x mw -3) (+ y (round (* 0.505 h))) 3 2) red) 0))))
      (set-window-parameter w 'glitnir-minimap nil)
      (nidhogg-view-minimap 'forget file)
      (kill-buffer buf))))

;; The horizontal strip (glitnir-hstrip): a thumb along the bottom of the text while the window cuts lines wider than
;; it, worked out in C at each redisplay (glitnir-hstrip-state), drawn as a bar when the frame publishes.

(defvar glitnir--hstrip-measures)

(defun glitnir-gui--hstrip-spec ()
  "A strip spec at the frame's scale: 12 logical px, its bottom 8 take presses, a 6 px green thumb, square."
  (let ((s (frame-scale-factor)))
    `(:height ,(round (* 12 s)) :lane ,(round (* 8 s)) :thumb-inset ,(round (* 3 s)) :radius 0
              :least ,(round (* 24 s)) :band ("#00ff00" . 1))))

(defun glitnir-gui--hstrip-buffer (name long)
  "Buffer NAME: 100 lines of 20 columns, line 10 (from 0) LONG columns when LONG is non-nil."
  (let ((b (get-buffer-create name)))
    (with-current-buffer b
      (erase-buffer)
      (dotimes (i 100)
        (insert (format "%5d %s\n" i (make-string (if (and long (= i 10)) (- long 6) 14) (+ ?a (% i 26))))))
      (goto-char (point-min)))
    b))

(defmacro glitnir-gui--with-hstrip (long &rest body)
  "Run BODY with BUF a `glitnir-gui--hstrip-buffer' (LONG, an expression of W, the selected window), shown in W
with a line model, its lines cut, and the strip's spec."
  (declare (indent 1))
  `(let* ((w (selected-window)) (buf (glitnir-gui--hstrip-buffer "*glitnir-hstrip*" ,long)))
     (glitnir-gui--skip-unless-tree)
     (skip-unless (fboundp 'nidhogg-lines-enable))
     (unwind-protect
         (progn
           (set-window-buffer w buf)
           (set-window-start w 1)
           (set-window-hscroll w 0)
           (with-current-buffer buf (setq truncate-lines t))
           (nidhogg-lines-enable buf t)
           (set-window-parameter w 'glitnir-hstrip (glitnir-gui--hstrip-spec))
           (redisplay t)
           ,@body)
       (set-window-parameter w 'glitnir-hstrip nil)
       (set-window-hscroll w 0)
       (when (fboundp 'nidhogg-lines-disable) (nidhogg-lines-disable buf))
       (kill-buffer buf))))

(defun glitnir-gui--hstrip-row (w)
  "Window W's strip as (X0 Y X1), device pixels: its first text column, the middle of its thumb, the text's right edge."
  (pcase-let ((`(,tx ,_ ,tx1 ,ty1) (window-inside-pixel-edges w)))
    (list (+ tx (nth 2 (glitnir-hstrip-state w))) (- ty1 (round (* 6 (frame-scale-factor)))) tx1)))

(defun glitnir-gui--green-p (r g b)
  (and (> g 200) (< r 60) (< b 60)))

(defun glitnir-gui--hstrip-thumb (w &optional snap)
  "The thumb's painted (LEFT . RIGHT) device x in window W's strip row (across the whole window), or nil."
  (pcase-let ((`(,_ ,y ,_) (glitnir-gui--hstrip-row w))
              (`(,wx ,_ ,wx1 ,_) (window-pixel-edges w))
              (snap (or snap (glitnir-gui--painted)))
              (lo nil) (hi nil))
    (cl-loop for x from (max 0 wx) below wx1 do
             (when (apply #'glitnir-gui--green-p (glitnir-gui--pixel snap (list x y)))
               (setq lo (or lo x) hi (1+ x))))
    (and lo (cons lo hi))))

(defun glitnir-gui--hstrip-expected (w)
  "The thumb's (LEFT . RIGHT) as C places it from W's `glitnir-hstrip-state' and hscroll (no right to left)."
  (pcase-let* ((`(,x0 ,_ ,x1) (glitnir-gui--hstrip-row w))
               (`(,whole . ,_) (glitnir-hstrip-state w))
               (len (- x1 x0))
               (least (round (* 24 (frame-scale-factor))))
               (thumb (min len (max least (floor (+ 0.5 (/ (* (float len) len) whole))))))
               (range (- whole len))
               (share (if (> range 0) (min 1.0 (/ (* (window-hscroll w) (frame-char-width)) (float range))) 0))
               (tx (floor (+ 0.5 (* share (- len thumb))))))
    (cons (+ x0 tx) (+ x0 tx thumb))))

(defun glitnir-gui--near (a b &optional slack)
  "Whether the (LEFT . RIGHT) pairs A and B agree within SLACK device pixels (default 1)."
  (and a b (<= (abs (- (car a) (car b))) (or slack 1)) (<= (abs (- (cdr a) (cdr b))) (or slack 1))))

(ert-deftest glitnir-gui/hstrip-shows-only-when-cut-text-overflows ()
  "The strip shows while the window cuts lines and the widest line (the model's, plus 4 columns) is wider than the
text: not while the lines wrap, not once the long line is gone, and while scrolled sideways (hscroll plus the text)."
  (glitnir-gui--with-hstrip (* 3 (window-body-width w))
    (let ((text (- (window-body-width w t) (nth 2 (glitnir-hstrip-state w))))
          (col (nth 3 (glitnir-hstrip-state w))))
      (glitnir-gui--shot "hstrip-shown")
      (should (= (car (glitnir-hstrip-state w)) (* (+ (* 3 (window-body-width w)) 4) col)))
      (should (glitnir-gui--hstrip-thumb w))
      ;; wrapped: none
      (with-current-buffer buf (setq truncate-lines nil))
      (redisplay t)
      (should (= 0 (car (glitnir-hstrip-state w))))
      (should-not (glitnir-gui--hstrip-thumb w))
      ;; cut, the long line made short: every line fits, none
      (with-current-buffer buf
        (setq truncate-lines t)
        (goto-char (point-min)) (forward-line 10)
        (delete-region (+ (point) 20) (pos-eol))
        (goto-char (point-min)))
      (redisplay t)
      (should (= 0 (car (glitnir-hstrip-state w))))
      (should-not (glitnir-gui--hstrip-thumb w))
      ;; scrolled sideways: shown, standing for the hscroll plus the text
      (set-window-hscroll w 5)
      (redisplay t)
      (should (= (car (glitnir-hstrip-state w)) (+ (* 5 (frame-char-width)) text)))
      (should (glitnir-gui--hstrip-thumb w)))))

(ert-deftest glitnir-gui/hstrip-thumb-follows-hscroll ()
  "The thumb is the text's share of the widest line long, at least 24 logical px, at the hscroll's share of what lies
beyond the text: at the left end at hscroll 0, at the right end past the line's end.  A scroll resolves no spec."
  (glitnir-gui--with-hstrip (* 3 (window-body-width w))
    (let ((resolves 0))
      (advice-add 'glitnir--hstrip-vector :before (lambda (&rest _) (cl-incf resolves)) '((name . gui-hstrip)))
      (unwind-protect
          (pcase-let ((`(,x0 ,_ ,x1) (glitnir-gui--hstrip-row w)))
            (let ((at0 (glitnir-gui--hstrip-thumb w)))
              (should (glitnir-gui--near at0 (glitnir-gui--hstrip-expected w)))
              (should (<= (abs (- (car at0) x0)) 1))
              ;; the thumb's length: the text's share
              (should (< (abs (- (- (cdr at0) (car at0))
                                 (/ (* (float (- x1 x0)) (- x1 x0)) (car (glitnir-hstrip-state w)))))
                         2)))
            (dolist (h (list (window-body-width w) (* 2 (window-body-width w))))
              (set-window-hscroll w h)
              (let ((at (glitnir-gui--hstrip-thumb w)))
                (glitnir-gui--shot (format "hstrip-at-%d" h))
                (should (glitnir-gui--near at (glitnir-gui--hstrip-expected w)))
                (should (> (car at) x0))))
            ;; past the end: at the right end, inside the text
            (set-window-hscroll w (* 4 (window-body-width w)))
            (should (<= (abs (- (cdr (glitnir-gui--hstrip-thumb w)) x1)) 1))
            (should (= resolves 0)))
        (advice-remove 'glitnir--hstrip-vector 'gui-hstrip)))))

(ert-deftest glitnir-gui/hstrip-spans-text-not-fringe-or-numbers ()
  "With line numbers on the strip starts at the first text column, where the line's first glyph is drawn, and ends at
the text area's right edge: the thumb never reaches into the numbers or the fringe."
  (glitnir-gui--with-hstrip (* 3 (window-body-width w))
    (with-current-buffer buf (display-line-numbers-mode 1))
    (redisplay t)
    (pcase-let* ((lnum (nth 2 (glitnir-hstrip-state w)))
                 (`(,tx ,_ ,tx1 ,_) (window-inside-pixel-edges w)))
      (should (> lnum 0))
      (should (= lnum (car (posn-x-y (posn-at-point (window-start w) w)))))
      (should (<= (abs (- (car (glitnir-gui--hstrip-thumb w)) (+ tx lnum))) 1))
      (glitnir-gui--shot "hstrip-numbers")
      (set-window-hscroll w (* 4 (window-body-width w)))
      (let ((at (glitnir-gui--hstrip-thumb w)))
        (should (<= (abs (- (cdr at) tx1)) 1))
        (should (<= (cdr at) tx1))
        (should (>= (car at) (+ tx lnum)))))))

(ert-deftest glitnir-gui/hstrip-sees-display-strings-at-hscroll-0 ()
  "Rows cut while the model says their line fits are measured, so the strip shows for them at hscroll 0: a line with a
wide `after-string', then a fold summary (a hidden range's `display' string).  Each such row is walked once per change
of the buffer; a redisplay with no change walks none."
  (glitnir-gui--with-hstrip nil
    (should (= 0 (car (glitnir-hstrip-state w))))
    (let* ((text (- (window-body-width w t) (nth 2 (glitnir-hstrip-state w))))
           (wide (propertize " " 'display `(space :width (,(+ text 300)))))
           (line-end (lambda (n) (with-current-buffer buf (save-excursion (goto-char (point-min)) (forward-line n) (pos-eol)))))
           (ov (with-current-buffer buf (make-overlay (funcall line-end 5) (funcall line-end 5)))))
      (setq glitnir--hstrip-measures 0)
      (overlay-put ov 'after-string wide)
      (redisplay t)
      (glitnir-gui--shot "hstrip-after-string")
      (should (>= (car (glitnir-hstrip-state w)) (+ text 300)))
      (should (glitnir-gui--hstrip-thumb w))
      (should (>= glitnir--hstrip-measures 1))
      (let ((n glitnir--hstrip-measures))
        (force-window-update w)
        (redisplay t)
        (should (= glitnir--hstrip-measures n)))
      (delete-overlay ov)
      (redisplay t)
      (should (= 0 (car (glitnir-hstrip-state w))))
      ;; a fold summary: lines 21 to 30 hidden behind a display string wider than the text (literal text: a display
      ;; string's own `display' properties are not followed)
      (let ((fold (with-current-buffer buf (make-overlay (funcall line-end 20) (funcall line-end 30)))))
        (nidhogg-lines-set-hidden buf (list (cons (funcall line-end 20) (funcall line-end 30))))
        (overlay-put fold 'display (concat " ... " (make-string (+ 2 (ceiling (+ text 300) (frame-char-width))) ?x)))
        (redisplay t)
        (glitnir-gui--shot "hstrip-fold-summary")
        (should (>= (car (glitnir-hstrip-state w)) (+ text 300)))
        (should (glitnir-gui--hstrip-thumb w))
        (delete-overlay fold)
        (nidhogg-lines-set-hidden buf nil)))))

(ert-deftest glitnir-gui/point-on-the-last-row-is-fully-visible ()
  "While the strip shows, the row under it counts as partly visible: point moved onto it scrolls the window one row
(with `scroll-conservatively', as for a partly shown last row), and point's row then ends above the strip."
  (glitnir-gui--with-hstrip (* 3 (window-body-width w))
    (let* ((scroll-conservatively 101)
           (strip (nth 1 (glitnir-hstrip-state w)))
           (body (window-body-height w t))
           (lh (car (window-line-height 0 w)))
           ;; rows spaced so the last whole row ends under the strip (the gap below it less than the strip)
           (_ (with-current-buffer buf
                (setq line-spacing (cl-loop for ls from 0 below lh
                                            when (< (mod body (+ lh ls)) strip) return ls))))
           (_ (redisplay t))
           (vpos (cl-loop for v from 0
                          for (h _ y) = (window-line-height v w)
                          when (> (+ y h) (- body strip)) return v))
           (target (with-current-buffer buf (save-excursion (goto-char (window-start w)) (forward-line vpos) (point)))))
      (should (> strip 0))
      (should (> vpos 0))
      (should (= 0 (nth 3 (window-line-height vpos w))))
      (with-selected-window w (goto-char target))
      (redisplay t)
      (glitnir-gui--shot "hstrip-last-row")
      (pcase-let* ((row (cdr (posn-actual-col-row (posn-at-point target w))))
                   (`(,h ,_ ,y) (window-line-height row w)))
        (should (<= (+ y h) (- body strip))))
      (should (= 2 (with-current-buffer buf (line-number-at-pos (window-start w))))))))

(ert-deftest glitnir-gui/a-click-on-the-last-row-places-point ()
  "Only the strip's bottom 8 px take presses: a press 10 px above the text's bottom, on the last row's text over the
strip, reaches Emacs as a click there, which places point on that row."
  (glitnir-gui--with-hstrip (* 3 (window-body-width w))
    (let ((vp (glitnir-gui--vp-start)) (evs nil))
      (unwind-protect
          (pcase-let* ((map (glitnir-gui--vp-calibrate vp))
                       (`(,tx ,_ ,_ ,ty1) (window-inside-pixel-edges w))
                       (`(,wx ,wy ,_ ,_) (window-pixel-edges w))
                       (x (+ tx (round (* 30 (frame-scale-factor)))))
                       (y (- ty1 (round (* 10 (frame-scale-factor)))))
                       (want (posn-point (posn-at-x-y (- x wx) (- y wy) w))))
            (should (> (nth 1 (glitnir-hstrip-state w)) 0))
            (glitnir-gui--vp-to vp map x y)
            (gui-runner-read-all)
            (process-send-string vp "press\nrelease\n")
            (should (gui-runner-wait (lambda () (gui-runner-read-all (lambda (e) (push e evs)))
                                       (cl-find-if (lambda (e) (eq (car-safe e) 'mouse-1)) evs))
                                     3))
            (let ((click (cl-find-if (lambda (e) (eq (car-safe e) 'mouse-1)) evs)))
              (should (eq (posn-window (event-start click)) w))
              (with-selected-window w (mouse-set-point click))
              (should (= (with-current-buffer buf (line-number-at-pos (window-point w)))
                         (with-current-buffer buf (line-number-at-pos want))))
              (should (= 0 (window-hscroll w)))))
        (delete-process vp)))))

(ert-deftest glitnir-gui/hstrip-press-lands-on-the-thumb-s2 ()
  "A press on the strip's lane scrolls the window so the thumb centres on the pointer, through the `glitnir-hstrip'
event; a drag moves it along; the release sends nothing.  Run at scale 2 too: the lane and the thumb in device px."
  (glitnir-gui--with-hstrip (* 3 (window-body-width w))
    (let ((vp (glitnir-gui--vp-start)) (seen nil))
      (unwind-protect
          (pcase-let* ((map (glitnir-gui--vp-calibrate vp))
                       (`(,x0 ,_ ,x1) (glitnir-gui--hstrip-row w))
                       (`(,_ ,_ ,_ ,ty1) (window-inside-pixel-edges w))
                       (y (- ty1 (round (* 3 (frame-scale-factor)))))
                       (col (frame-char-width))
                       (centre (lambda () (let ((at (glitnir-gui--hstrip-thumb w))) (/ (+ (car at) (cdr at)) 2.0)))))
            (advice-add 'glitnir-hstrip-event :before (lambda (e) (push (nth 2 e) seen)) '((name . gui-hstrip)))
            (dolist (frac '(0.5 0.8))
              (let ((x (+ x0 (round (* frac (- x1 x0))))) (n (length seen)))
                (glitnir-gui--vp-to vp map x y)
                (process-send-string vp "press\nrelease\n")
                (should (gui-runner-wait (lambda () (gui-runner-read-all) (> (length seen) n)) 3))
                (gui-runner-observe 0.1 #'gui-runner-read-all)
                (glitnir-gui--shot (format "hstrip-press-%s" frac))
                (should (> (window-hscroll w) 0))
                (should (<= (abs (- (funcall centre) x)) (+ col 1)))))
            ;; a drag from the thumb's centre to the left end: the window back at its left
            (let ((x (round (funcall centre))))
              (glitnir-gui--vp-to vp map x y)
              (process-send-string vp "press\n")
              (gui-runner-observe 0.1 #'gui-runner-read-all)
              (glitnir-gui--vp-to vp map (+ x0 2) y)
              (gui-runner-observe 0.1 #'gui-runner-read-all)
              (process-send-string vp "release\n")
              (should (gui-runner-wait (lambda () (gui-runner-read-all) (= 0 (window-hscroll w))) 3))))
        (advice-remove 'glitnir-hstrip-event 'gui-hstrip)
        (delete-process vp)))))

(ert-deftest glitnir-gui/hstrip-without-a-model ()
  "Without a line model nothing is measured: the strip shows while a shown row is cut, standing for the hscroll plus
twice the text (a thumb half the strip at hscroll 0); with no row cut and no hscroll, none."
  (glitnir-gui--skip-unless-tree)
  (let* ((w (selected-window)) (buf (glitnir-gui--hstrip-buffer "*glitnir-hstrip-nomodel*" (* 3 (window-body-width w)))))
    (unwind-protect
        (progn
          (set-window-buffer w buf)
          (set-window-start w 1)
          (with-current-buffer buf (setq truncate-lines t))
          (set-window-parameter w 'glitnir-hstrip (glitnir-gui--hstrip-spec))
          (setq glitnir--hstrip-measures 0)
          (redisplay t)
          (should-not (and (fboundp 'nidhogg-lines-describe) (nidhogg-lines-describe buf)))
          (pcase-let* ((`(,x0 ,_ ,x1) (glitnir-gui--hstrip-row w))
                       (text (- x1 x0)))
            (should (= (car (glitnir-hstrip-state w)) (* 2 text)))
            (let ((at (glitnir-gui--hstrip-thumb w)))
              (glitnir-gui--shot "hstrip-no-model")
              (should (<= (abs (- (car at) x0)) 1))
              (should (<= (abs (- (- (cdr at) (car at)) (/ text 2))) 1)))
            ;; the long line scrolled out of view (point with it, so it stays out): no row cut, no strip
            (let ((l20 (with-current-buffer buf (save-excursion (goto-char (point-min)) (forward-line 20) (point)))))
              (set-window-point w l20)
              (set-window-start w l20))
            (redisplay t)
            (glitnir-gui--shot "hstrip-no-model-none")
            (should (= 0 (car (glitnir-hstrip-state w))))
            (glitnir-gui--painted)
            (should (= 0 (car (glitnir-hstrip-state w))))
            (should-not (glitnir-gui--hstrip-thumb w))
            (set-window-hscroll w 3)
            (redisplay t)
            (should (= (car (glitnir-hstrip-state w)) (+ (* 3 (frame-char-width)) (* 2 text))))
            (should (= glitnir--hstrip-measures 0))))
      (set-window-parameter w 'glitnir-hstrip nil)
      (set-window-hscroll w 0)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/hstrip-drag-1mb-line ()
  "Dragging along a 1 MB minified line (long-line optimizations on): no step's redisplay takes over 50 ms and no row
is walked to be measured; the strip stands for the hscroll plus twice the text."
  (glitnir-gui--skip-unless-tree)
  (skip-unless (fboundp 'nidhogg-lines-enable))
  (let* ((w (selected-window)) (buf (get-buffer-create "*glitnir-hstrip-1mb*")) (slow nil))
    (unwind-protect
        (progn
          (with-current-buffer buf
            (erase-buffer)
            (insert (apply #'concat (make-list 80000 "var a=1;b=2;")) "\n")
            (goto-char (point-min))
            (setq truncate-lines t))
          (nidhogg-lines-enable buf t)
          (set-window-buffer w buf)
          (set-window-start w 1)
          (set-window-parameter w 'glitnir-hstrip (glitnir-gui--hstrip-spec))
          (redisplay t)
          (should (with-current-buffer buf (long-line-optimizations-p)))
          (setq glitnir--hstrip-measures 0)
          (dolist (h (number-sequence 0 1040000 52000))
            (set-window-hscroll w h)
            (let ((t0 (float-time)))
              (redisplay t)
              (let ((dt (- (float-time) t0)))
                (when (> dt 0.05) (push (cons h dt) slow)))))
          (should-not slow)
          (should (= glitnir--hstrip-measures 0))
          (should (> (car (glitnir-hstrip-state w)) (* 1040000 (frame-char-width)))))
      (set-window-parameter w 'glitnir-hstrip nil)
      (set-window-hscroll w 0)
      (nidhogg-lines-disable buf)
      (kill-buffer buf))))

(ert-deftest glitnir-gui/hstrip-under-the-minimap ()
  "An open minimap (not sticky) covers the strip's right end: scrolled to the end, the thumb shows left of the minimap
and none of it inside."
  (glitnir-gui--with-hstrip (* 3 (window-body-width w))
    (unwind-protect
        (progn
          (set-window-parameter w 'glitnir-minimap '(:width 120 :open t :fill ("#203040" . 1) :text ("#ff00ff" . 1)))
          ;; just at the end: the thumb a third of the strip, at its right end
          (pcase-let ((`(,x0 ,_ ,x1) (glitnir-gui--hstrip-row w)))
            (set-window-hscroll w (ceiling (- (car (glitnir-hstrip-state w)) (- x1 x0)) (frame-char-width))))
          (pcase-let* ((`(,mx ,_ ,mw ,_) (glitnir-gui--minimap-rect w 120))
                       (`(,_ ,y ,x1) (glitnir-gui--hstrip-row w))
                       (snap (glitnir-gui--painted)))
            (glitnir-gui--shot "hstrip-under-minimap")
            (should (= (+ mx mw) x1))
            (should (= 0 (glitnir-gui--count-where snap (list mx y mw 1) #'glitnir-gui--green-p)))
            (should (> (glitnir-gui--count-where snap (list (- mx 20) y 20 1) #'glitnir-gui--green-p) 10))))
      (set-window-parameter w 'glitnir-minimap nil))))

(ert-deftest glitnir-gui/the-minimap-band-with-wrap-spans-the-logical-lines-shown ()
  "With lines wrapping the minimap keeps a row per logical line, and its band spans the logical lines shown, whole
rows: from the window's first line to its last, far fewer than the screen rows."
  (glitnir-gui--skip-unless-tree)
  (skip-unless (fboundp 'nidhogg-lines-enable))
  (let* ((w (selected-window))
         (buf (get-buffer-create "*glitnir-minimap-wrap*")))
    (unwind-protect
        (progn
          (with-current-buffer buf
            (erase-buffer)
            (dotimes (i 200) (insert (format "%5d %s\n" i (make-string (* 3 (window-body-width w)) ?x))))
            (goto-char (point-min))
            (setq truncate-lines nil))
          (nidhogg-lines-enable buf t)
          (set-window-buffer w buf)
          (set-window-start w 1)
          (set-window-parameter w 'glitnir-minimap
                                '(:width 120 :open t :fill ("#203040" . 1) :text ("#203040" . 1) :band ("#00ffff" . 1)))
          (pcase-let* ((snap (glitnir-gui--painted))
                       (`(,mx ,my ,_ ,mh) (glitnir-gui--minimap-rect w 120))
                       (row-px (floor (+ 0.5 (* 2 (frame-scale-factor)))))
                       (lines (with-current-buffer buf
                                (- (line-number-at-pos (window-end w t)) (line-number-at-pos (window-start w)))))
                       (cyan (lambda (r g b) (and (< r 60) (> g 200) (> b 200))))
                       (band (glitnir-gui--count-where snap (list (+ mx 5) my 1 mh) cyan)))
            (glitnir-gui--shot "minimap-band-wrap")
            (should (< lines (/ (window-body-height w) 2)))
            (should (<= (* lines row-px) band (* (+ lines 2) row-px)))))
      (set-window-parameter w 'glitnir-minimap nil)
      (nidhogg-lines-disable buf)
      (kill-buffer buf))))

;; Wrapping at a column (`nidhogg-wrap-column'): a wrapped line continues after N columns, past the line numbers;
;; only that decision moves (it->wrap_x), so fills, `:align-to right' and images still reach the window's edge.

(defmacro glitnir-gui--with-wrap-column (n &rest body)
  "Run BODY with BUF, shown in W (the selected window), a buffer whose `nidhogg-wrap-column' is N, lines wrapping by
character; START the start of its line 0, 200 x's."
  (declare (indent 1))
  `(let* ((w (selected-window)) (buf (get-buffer-create "*glitnir-wrap-column*")) (start 1))
     (glitnir-gui--skip-unless-tree)
     (unwind-protect
         (progn
           (with-current-buffer buf
             (erase-buffer)
             (insert (make-string 200 ?x) "\n")
             (dotimes (i 30) (insert (format "line %d\n" i)))
             (goto-char (point-min))
             (setq truncate-lines nil word-wrap nil)
             (setq nidhogg-wrap-column ,n))
           (set-window-buffer w buf)
           (set-window-start w 1)
           (redisplay t)
           ,@body)
       (kill-buffer buf))))

(defun glitnir-gui--row-of (pos w)
  "The y (window pixels) of the screen row POS is drawn on in W."
  (nth 1 (pos-visible-in-window-p pos w t)))

(defun glitnir-gui--motion-chars (w start)
  "How many characters `vertical-motion' moves over from START to W's next screen line."
  (with-selected-window w (save-excursion (goto-char start) (vertical-motion 1) (- (point) start))))

(ert-deftest glitnir-gui/wrap-column-wraps-at-n ()
  "With `nidhogg-wrap-column' 40 and line numbers on, a long line's first screen row holds 40 characters, the 40th
starting at the numbers' width plus 39 columns; `vertical-motion' counts the rows as they are drawn.  Line numbers that
grow a digit move the edge with them."
  (glitnir-gui--with-wrap-column 40
    (with-current-buffer buf (display-line-numbers-mode 1))
    (redisplay t)
    (let ((col (frame-char-width))
          (lnum (car (posn-x-y (posn-at-point start w)))))
      (glitnir-gui--shot "wrap-column")
      (should (> lnum 0))
      (should (= (glitnir-gui--row-of (+ start 39) w) (glitnir-gui--row-of start w)))
      (should (> (glitnir-gui--row-of (+ start 40) w) (glitnir-gui--row-of start w)))
      (should (= (car (posn-x-y (posn-at-point (+ start 39) w))) (+ lnum (* 39 col))))
      (should (= (glitnir-gui--motion-chars w start) 40))
      ;; numbers a digit wider (lines from 1000): still 40 characters, after the wider numbers
      (with-current-buffer buf
        (setq-local display-line-numbers-offset 1000))
      (force-window-update w)
      (redisplay t)
      (let ((wide (car (posn-x-y (posn-at-point start w)))))
        (should (> wide lnum))
        (should (= (glitnir-gui--motion-chars w start) 40))
        (should (> (glitnir-gui--row-of (+ start 40) w) (glitnir-gui--row-of (+ start 39) w)))))))

(ert-deftest glitnir-gui/narrow-window-wraps-at-edge ()
  "A window narrower than `nidhogg-wrap-column' wraps at its edge, as with no column."
  (glitnir-gui--with-wrap-column 4000
    (let ((at-col (glitnir-gui--motion-chars w start)))
      (with-current-buffer buf (setq nidhogg-wrap-column nil))
      (redisplay t)
      (should (= at-col (glitnir-gui--motion-chars w start)))
      (should (< at-col 200)))))

(ert-deftest glitnir-gui/hl-line-and-region-reach-the-edge-under-column ()
  "Under a wrap column an extended background (`hl-line', the region, a diff's) still fills its row to the window's
edge, past the column."
  (glitnir-gui--with-wrap-column 40
    (let ((ov (with-current-buffer buf
                (let ((o (make-overlay (point-min) (save-excursion (goto-char (point-min)) (forward-line 1) (point)))))
                  (overlay-put o 'face '(:background "#ff0000" :extend t))
                  o))))
      (ignore ov)
      (pcase-let* ((snap (glitnir-gui--painted))
                   (`(,_ ,ty ,tx1 ,_) (window-inside-pixel-edges w))
                   (y (+ ty (glitnir-gui--row-of start w) 2))
                   (red (lambda (r g b) (and (> r 200) (< g 60) (< b 60)))))
        (glitnir-gui--shot "wrap-column-extend")
        (should (apply red (glitnir-gui--pixel snap (list (- tx1 3) y))))))))

(ert-deftest glitnir-gui/align-to-right-does-not-wrap-under-column ()
  "Under a wrap column a `:align-to right' stretch reaches the window's edge on its own row, not wrapping at the
column: the line stays one screen row and the stretch's background fills to the edge."
  (glitnir-gui--with-wrap-column 40
    (let (bol eol)
      (with-current-buffer buf
        (goto-char (point-max))
        (setq bol (point))
        (insert "abc" (propertize " " 'display '(space :align-to right) 'face '(:background "#0000ff")))
        (setq eol (point))
        (insert "\n")
        (goto-char (point-min)))
      (set-window-start w bol)
      (pcase-let* ((snap (glitnir-gui--painted))
                   (`(,_ ,ty ,tx1 ,_) (window-inside-pixel-edges w))
                   (y (+ ty (glitnir-gui--row-of bol w) 2))
                   (blue (lambda (r g b) (and (< r 60) (< g 60) (> b 200)))))
        (glitnir-gui--shot "wrap-column-align-to")
        (should (= (glitnir-gui--row-of eol w) (glitnir-gui--row-of bol w)))
        (should (= (glitnir-gui--motion-chars w bol) (1+ (- eol bol))))
        (should (apply blue (glitnir-gui--pixel snap (list (- tx1 3) y))))))))

(ert-deftest glitnir-gui/image-wider-than-the-column-is-not-cut ()
  "Under a wrap column an image wider than the column, alone on its row, is drawn whole up to the window's edge."
  (glitnir-gui--with-wrap-column 20
    (let* ((wpx (+ (* 20 (frame-char-width)) 100))
           (ppm (concat (format "P6\n%d 20\n255\n" wpx) (apply #'unibyte-string (cl-loop repeat (* wpx 20) append '(255 0 255)))))
           (img (create-image ppm 'pbm t)))
      (should (< wpx (window-body-width w t)))
      (with-current-buffer buf
        (goto-char (point-max))
        (insert-image img "i")
        (insert "\n")
        (goto-char (point-min)))
      (set-window-start w (with-current-buffer buf (- (point-max) 2)))
      (pcase-let* ((snap (glitnir-gui--painted))
                   (`(,tx ,ty ,_ ,_) (window-inside-pixel-edges w))
                   (y (+ ty 10))
                   (magenta (lambda (r g b) (and (> r 200) (< g 60) (> b 200)))))
        (glitnir-gui--shot "wrap-column-image")
        (should (> (glitnir-gui--extent-where snap (list tx y (window-body-width w t) 1) magenta)
                   (- wpx 2)))))))

(ert-deftest glitnir-gui/click-right-of-the-edge-goes-to-row-end ()
  "Under a wrap column a click right of the column on a continued row lands at that row's end, not on the next row."
  (glitnir-gui--with-wrap-column 40
    (pcase-let* ((`(,wx ,wy ,_ ,_) (window-pixel-edges w))
                 (`(,tx ,ty ,tx1 ,_) (window-inside-pixel-edges w))
                 (x (- (- tx1 20) wx))
                 (y (- (+ ty (glitnir-gui--row-of start w) 2) wy))
                 (p (posn-point (posn-at-x-y x y w))))
      (ignore tx)
      (should (memq (- p start) '(39 40))))))

(ert-deftest glitnir-gui/r2l-under-column ()
  "A right-to-left paragraph under a wrap column wraps after N characters too, its rows set against the window's
right edge (the free space on the left): the row's 40 characters, a left-to-right run, end at that edge."
  (glitnir-gui--with-wrap-column 40
    (with-current-buffer buf (setq bidi-paragraph-direction 'right-to-left))
    (redisplay t)
    (let ((col (frame-char-width))
          (tw (window-body-width w t)))
      (glitnir-gui--shot "wrap-column-r2l")
      (should (= (glitnir-gui--motion-chars w start) 40))
      (should (= (glitnir-gui--row-of (+ start 39) w) (glitnir-gui--row-of start w)))
      (should (> (glitnir-gui--row-of (+ start 40) w) (glitnir-gui--row-of start w)))
      (should (>= (car (posn-x-y (posn-at-point (+ start 39) w))) (- tw (* 2 col))))
      (should (<= (car (posn-x-y (posn-at-point start w))) (- tw (* 39 col)))))))

;; Horizontal scroll followers (`nidhogg-hscroll-of', hscroll design §3.4(e)): the sticky card's window follows its
;; editor's hscroll, copied in C where the hscroll changes.

(defmacro glitnir-gui--with-hscroll-follower (&rest body)
  "Run BODY with LEAD (the selected window) and CARD (a window below it, following LEAD) on BUF: lines of 300
characters whose column N is (format \"%03d\" N)-ish markers, truncated."
  (declare (indent 0))
  `(let ((buf (get-buffer-create "*glitnir-hscroll-of*")))
     (glitnir-gui--skip-unless-tree)
     (save-window-excursion
       (delete-other-windows)
       (unwind-protect
           (let* ((lead (selected-window))
                  (card (split-window lead)))
             (with-current-buffer buf
               (erase-buffer)
               (dotimes (_ 20)
                 (dotimes (i 30) (insert (format "%-10d" (* i 10))))
                 (insert "\n"))
               (setq truncate-lines t)
               (goto-char (point-min)))
             (set-window-buffer lead buf)
             (set-window-buffer card buf)
             (set-window-point card (with-current-buffer buf (point-min)))
             (set-window-parameter card 'nidhogg-hscroll-of lead)
             (redisplay t)
             ,@body)
         (kill-buffer buf)))))

(defun glitnir-gui--first-column (w)
  "The buffer column of the character drawn first on W's first text row."
  (pcase-let* ((`(,wx ,wy ,_ ,_) (window-pixel-edges w))
               (`(,tx ,ty ,_ ,_) (window-inside-pixel-edges w))
               (p (posn-point (posn-at-x-y (+ (- tx wx) 1) (+ (- ty wy) 1) w))))
    (with-current-buffer (window-buffer w) (save-excursion (goto-char p) (current-column)))))

(ert-deftest glitnir-gui/hscroll-follower-follows-auto-hscroll ()
  "Point moved past the window's right edge auto-hscrolls the editor; its follower gets the same hscroll in the same
redisplay, its first drawn column the editor's, with no Lisp run for it."
  (glitnir-gui--with-hscroll-follower
    (with-selected-window lead
      (goto-char (with-current-buffer buf (+ (point-min) 250))))
    (redisplay t)
    (glitnir-gui--shot "hscroll-of-auto")
    (should (> (window-hscroll lead) 0))
    (should (= (window-hscroll card) (window-hscroll lead)))
    (should (= (glitnir-gui--first-column card) (glitnir-gui--first-column lead)))
    ;; the follower's own point, at a line's start, does not scroll it back
    (set-window-point card (with-current-buffer buf (save-excursion (goto-char (point-min)) (forward-line 3) (point))))
    (redisplay t)
    (should (= (window-hscroll card) (window-hscroll lead)))
    ;; and back
    (with-selected-window lead (goto-char (with-current-buffer buf (point-min))))
    (redisplay t)
    (should (= (window-hscroll lead) 0))
    (should (= (window-hscroll card) 0))))

(ert-deftest glitnir-gui/hscroll-follower-pixels-shift ()
  "`set-window-hscroll' on the editor shifts the follower's drawn text by the same pixels: the follower's first drawn
column is the editor's hscroll."
  (glitnir-gui--with-hscroll-follower
    (set-window-hscroll lead 40)
    (redisplay t)
    (glitnir-gui--shot "hscroll-of-set")
    (should (= (window-hscroll card) 40))
    (should (= (glitnir-gui--first-column card) 40))
    (should (= (glitnir-gui--first-column lead) 40))))

;;; The shell's menus (nhwl-menu.el): the metrics a theme gives them and the submenu's tolerance

(defvar nhwl-menu--frames)
(defvar nhwl-menu-min-width)
(defvar nhwl-menu-row-height)
(defvar nhwl-menu-max-height-fraction)
(defvar nhwl-menu-submenu-tolerance)
(defvar nhwl-menu-submenu-delay)

(defun glitnir-gui--menu-steps (menu steps)
  "Open MENU with `x-popup-menu' at 40, 40 of the selected frame and run STEPS while it is open; their values.
STEPS is a list of (DELAY . FUNCTION): each FUNCTION is called from a timer DELAY seconds after the one before it (the
menu's loop runs timers while it reads), and its value, or (error . DATA) when it signals, is kept.  Push the keys
that close the menu last; a C-g closes it a second after the last step in any case."
  (let ((results nil) (timers nil) (at 0))
    (dolist (step steps)
      (setq at (+ at (car step)))
      (let ((fn (cdr step)))
        (push (run-at-time at nil (lambda () (push (condition-case e (funcall fn) (error (cons 'error e))) results)))
              timers)))
    (push (run-at-time (+ at 1) nil (lambda () (push ?\C-g unread-command-events))) timers)
    (unwind-protect (x-popup-menu (list '(40 40) (selected-frame)) menu)
      (mapc #'cancel-timer timers))
    (nreverse results)))

(defun glitnir-gui--menu-frame (depth)
  "The menu frame of DEPTH while it is shown, else nil."
  (let ((f (cdr (assq depth nhwl-menu--frames))))
    (and (frame-live-p f) (frame-visible-p f) f)))

(defun glitnir-gui--menu-row-height (depth row)
  "The pixel height of ROW (from 0) in the menu frame of DEPTH."
  (let ((w (frame-root-window (glitnir-gui--menu-frame depth))))
    (redisplay t)
    (with-current-buffer (window-buffer w)
      (save-excursion
        (goto-char (point-min)) (forward-line row)
        (cdr (window-text-pixel-size w (point) (pos-eol)))))))

(defun glitnir-gui--menu-motion (depth row x)
  "A pointer motion over the menu of DEPTH, X pixels in and a few pixels below the top of ROW (from 0)."
  (let* ((w (frame-root-window (glitnir-gui--menu-frame depth)))
         (y (with-current-buffer (window-buffer w)
              (save-excursion (goto-char (point-min)) (forward-line row)
                              (cdr (posn-x-y (posn-at-point (point) w)))))))
    (list 'mouse-movement (posn-at-x-y x (+ y 3) w))))

(ert-deftest nhwl-menu/min-width-is-honoured ()
  "A menu of three-letter items with `nhwl-menu-min-width' 220 is at least 220 logical pixels wide."
  (skip-unless (featurep 'nidhogg-wayland))
  (require 'nhwl-menu)
  (let* ((nhwl-menu-min-width 220)
         (got (glitnir-gui--menu-steps
               '("M" ("P" ("Cut" . 1) ("Pas" . 2)))
               (list (cons 0.5 (lambda () (redisplay t)
                                 (let ((f (glitnir-gui--menu-frame 0)))
                                   (prog1 (list (frame-pixel-width f) (* 220 (frame-scale-factor f)))
                                     (push ?\C-g unread-command-events)))))))))
    (should (consp (car got)))
    (should (>= (nth 0 (car got)) (floor (nth 1 (car got)))))))

(defun glitnir-gui--first (snap rgb)
  "The first device pixel (X . Y) of SNAP, a raw snapshot, in colour RGB, row by row, or nil."
  (catch 'found
    (dotimes (y (nth 1 snap))
      (dotimes (x (nth 0 snap))
        (when (equal (glitnir-gui--pixel snap (list x y)) rgb) (throw 'found (cons x y)))))
    nil))

(defun glitnir-gui--differ-at (a at-a b at-b w h)
  "The pixels of the W x H rectangles at AT-A of the raw snapshot A and at AT-B of B, each (X . Y), that differ; a
pixel outside either picture differs."
  (let ((n 0))
    (dotimes (y h)
      (dotimes (x w)
        (let ((xa (+ (car at-a) x)) (ya (+ (cdr at-a) y)) (xb (+ (car at-b) x)) (yb (+ (cdr at-b) y)))
          (unless (and (< -1 xa (nth 0 a)) (< -1 ya (nth 1 a)) (< -1 xb (nth 0 b)) (< -1 yb (nth 1 b))
                       (equal (glitnir-gui--pixel a (list xa ya)) (glitnir-gui--pixel b (list xb yb))))
            (cl-incf n)))))
    n))

(ert-deftest glitnir-snapshot/popup-frame-is-its-own-picture ()
  "`glitnir-snapshot' of a popup frame (an open menu, a tree of its own) is that popup's picture from its own corner:
its native area, found by the menu's background, is the same pixels as where the root's picture shows the menu.  Red
first: the popup was placed at its offset in its toplevel's tree, so its own picture started about 41 px in and was
cut off."
  (skip-unless (featurep 'nidhogg-wayland))
  (require 'nhwl-menu)
  (let ((menu-spec (get 'nhwl-menu 'face-defface-spec)))
    (unwind-protect
        (let* ((root (selected-frame))
               (got (progn
                      (face-spec-set 'nhwl-menu '((t :background "#0000ff" :foreground "#ffffff")) 'face-defface-spec)
                      (glitnir-gui--menu-steps
                       '("M" ("P" ("One" . 1) ("Two" . 2) ("Three" . 3)))
                       (list (cons 0.5 (lambda ()
                                         (let* ((f (glitnir-gui--menu-frame 0))
                                                (whole (progn (redisplay t) (gui-runner-wait-settled)
                                                              (glitnir-snapshot root 'raw)))
                                                (own (glitnir-snapshot f 'raw))
                                                (in-whole (glitnir-gui--first whole '(0 0 255)))
                                                (in-own (glitnir-gui--first own '(0 0 255)))
                                                ;; the native area less 2 px all round (its rounded corners blend
                                                ;; over what lies beneath)
                                                (w (- (frame-pixel-width f) 4)) (h (- (frame-pixel-height f) 4)))
                                           (prog1 (list :whole in-whole :own in-own :size (list (nth 0 own) (nth 1 own))
                                                        :differ (and in-whole in-own
                                                                     (glitnir-gui--differ-at
                                                                      whole (cons (+ 2 (car in-whole)) (+ 2 (cdr in-whole)))
                                                                      own (cons (+ 2 (car in-own)) (+ 2 (cdr in-own)))
                                                                      w h)))
                                             (push ?\C-g unread-command-events))))))))))
          (ert-info ((format "%S" got))
            (should (plist-get (car got) :whole))
            (should (plist-get (car got) :own))
            (should (equal 0 (plist-get (car got) :differ)))))
      (face-spec-set 'nhwl-menu menu-spec 'face-defface-spec))))

(ert-deftest nhwl-menu/rows-are-the-row-height ()
  "With `nhwl-menu-row-height' 30 every item row is 30 logical pixels tall."
  (skip-unless (featurep 'nidhogg-wayland))
  (require 'nhwl-menu)
  (let* ((nhwl-menu-row-height 30)
         (got (glitnir-gui--menu-steps
               '("M" ("P" ("One" . 1) ("Two" . 2) ("Three" . 3)))
               (list (cons 0.5 (lambda ()
                                 (let ((f (glitnir-gui--menu-frame 0)))
                                   ;; rows: the title, the separator, the three items
                                   (prog1 (list (round (* 30 (frame-scale-factor f)))
                                                (mapcar (lambda (r) (glitnir-gui--menu-row-height 0 r)) '(2 3 4)))
                                     (push ?\C-g unread-command-events)))))))))
    (should (consp (car got)))
    (should (equal (nth 1 (car got)) (make-list 3 (nth 0 (car got)))))))

(defvar nhwl-menu-separator-height)

(ert-deftest nhwl-menu/frame-is-the-rows-size ()
  "The menu frame is its rows' size to the pixel, also when that is no whole number of the frame's characters (a
least width of 223, rows 27 tall): no strip past the rows' right end or below the last row."
  (skip-unless (featurep 'nidhogg-wayland))
  (require 'nhwl-menu)
  (let* ((nhwl-menu-min-width 223)
         (nhwl-menu-row-height 27)
         (got (glitnir-gui--menu-steps
               '("M" ("P" ("Cut" . 1) ("Paste" . 2)))
               (list (cons 0.5 (lambda ()
                                 (redisplay t)
                                 (let ((w (frame-root-window (glitnir-gui--menu-frame 0))))
                                   (prog1 (with-current-buffer (window-buffer w)
                                            (list (cons (window-body-width w t) (window-body-height w t))
                                                  (cons (save-excursion
                                                          (goto-char (point-min)) (forward-line 2)
                                                          (car (window-text-pixel-size w (point) (pos-eol))))
                                                        (cdr (window-text-pixel-size w (point-min) (point-max))))))
                                     (push ?\C-g unread-command-events)))))))))
    (should (consp (car got)))
    (should (equal (nth 0 (car got)) (nth 1 (car got))))))

(ert-deftest nhwl-menu/separator-is-a-thin-rule ()
  "With `nhwl-menu-separator-height' 9 a separator row is 9 logical pixels tall, not a text line."
  (skip-unless (featurep 'nidhogg-wayland))
  (require 'nhwl-menu)
  (let* ((nhwl-menu-separator-height 9)
         (nhwl-menu-row-height 30)
         (got (glitnir-gui--menu-steps
               '("M" ("P" ("One" . 1) "--" ("Two" . 2)))
               (list (cons 0.5 (lambda ()
                                 (let ((f (glitnir-gui--menu-frame 0)))
                                   ;; rows: the title, the title's separator, One, the separator, Two
                                   (prog1 (list (round (* 9 (frame-scale-factor f)))
                                                (glitnir-gui--menu-row-height 0 3))
                                     (push ?\C-g unread-command-events)))))))))
    (should (consp (car got)))
    (should (equal (nth 1 (car got)) (nth 0 (car got))))))

(defvar nhwl-menu-selection-inset)
(defvar nhwl-menu-selection-radius)
(defvar nhwl-menu-row-padding)

(ert-deftest nhwl-menu/selection-is-an-inset-rounded-block ()
  "With `nhwl-menu-selection-inset' 4 and `nhwl-menu-selection-radius' 6, the selected row is filled from 4 logical
pixels in to 4 short of the right edge: the inset shows the menu's own background, the middle the selection's."
  (skip-unless (featurep 'nidhogg-wayland))
  (require 'nhwl-menu)
  (let ((menu-spec (get 'nhwl-menu 'face-defface-spec))
        (sel-spec (get 'nhwl-menu-selected 'face-defface-spec)))
    (unwind-protect
        (let* ((nhwl-menu-selection-inset 4)
               (nhwl-menu-selection-radius 6)
               (nhwl-menu-row-height 30)
               (nhwl-menu-row-padding '(40 . 40))
               (got (progn
                      (face-spec-set 'nhwl-menu '((t :background "#0000ff" :foreground "#ffffff")) 'face-defface-spec)
                      (face-spec-set 'nhwl-menu-selected '((t :background "#ff0000")) 'face-defface-spec)
                      (glitnir-gui--menu-steps
                       '("M" ("P" ("One" . 1) ("Two" . 2)))
                       (list (cons 0.5 (lambda () (push 'down unread-command-events) t))
                             (cons 0.5 (lambda ()
                                         (let* ((f (glitnir-gui--menu-frame 0))
                                                (w (frame-root-window f))
                                                (s (frame-scale-factor f))
                                                (y (with-current-buffer (window-buffer w)
                                                     (save-excursion
                                                       (goto-char (point-min)) (forward-line 2)   ; title, separator, One
                                                       (+ (cdr (posn-x-y (posn-at-point (point) w)))
                                                          (round (* 15 s))))))
                                                (snap (progn (redisplay t) (gui-runner-wait-settled)
                                                             (glitnir-snapshot nil 'raw)))
                                                (width (frame-pixel-width f))
                                                ;; the root's picture, which holds the menu (a child frame's own
                                                ;; picture starts at an offset and is cut): its first pixel in the
                                                ;; menu's background is the menu's corner
                                                (origin (catch 'found
                                                          (dotimes (py (nth 1 snap))
                                                            (dotimes (px (nth 0 snap))
                                                              (when (equal (glitnir-gui--pixel snap (list px py)) '(0 0 255))
                                                                (throw 'found (cons px py)))))
                                                          (cons 0 0)))
                                                (at (lambda (x) (glitnir-gui--pixel
                                                                 snap (list (min (1- (nth 0 snap)) (+ (car origin) x))
                                                                            (min (1- (nth 1 snap)) (+ (cdr origin) y)))))))
                                           (prog1 (list (funcall at 1) (funcall at (round (* 20 s))) (funcall at (- width 2))
                                                        :origin origin :snap (list (nth 0 snap) (nth 1 snap)))
                                             (push ?\C-g unread-command-events))))))))))
          (should (equal (list '((0 0 255) (255 0 0) (0 0 255)) (nth 1 got))
                         (list (seq-take (nth 1 got) 3) (nth 1 got)))))
      (face-spec-set 'nhwl-menu menu-spec 'face-defface-spec)
      (face-spec-set 'nhwl-menu-selected sel-spec 'face-defface-spec))))

(ert-deftest nhwl-menu/tall-menu-scrolls-at-75-percent ()
  "A menu of 100 items is at most 0.75 of its frame's height; End selects the last item and keeps it in view."
  (skip-unless (featurep 'nidhogg-wayland))
  (require 'nhwl-menu)
  (let* ((nhwl-menu-max-height-fraction 0.75)
         (items (mapcar (lambda (i) (cons (format "Item %d" i) i)) (number-sequence 1 100)))
         (got (glitnir-gui--menu-steps
               (list "M" (cons "P" items))
               (list (cons 0.5 (lambda ()
                                 (redisplay t)
                                 (let ((f (glitnir-gui--menu-frame 0)))
                                   (prog1 (list (frame-pixel-height f) (* 0.75 (frame-pixel-height (frame-parent f))))
                                     (push 'end unread-command-events)))))
                     (cons 0.5 (lambda ()
                                 (redisplay t)
                                 (let* ((w (frame-root-window (glitnir-gui--menu-frame 0)))
                                        (b (window-buffer w)))
                                   (prog1 (list (buffer-local-value 'nhwl-menu--index b)
                                                (pos-visible-in-window-p
                                                 (with-current-buffer b (save-excursion (goto-char (point-max))
                                                                                        (pos-bol)))
                                                 w))
                                     (push ?\C-g unread-command-events)))))))))
    (should (consp (nth 0 got)))
    (should (<= (nth 0 (nth 0 got)) (nth 1 (nth 0 got))))
    ;; rows: the title, the separator, 100 items
    (should (equal (nth 1 got) '(101 t)))))

(ert-deftest nhwl-menu-gui/submenu-survives-a-diagonal-move ()
  "The pointer leaves the open submenu's row towards the submenu, across the row below, within
`nhwl-menu-submenu-tolerance': the submenu stays and its row stays selected; resting there past the tolerance, the
row below takes the selection and the submenu closes."
  (skip-unless (featurep 'nidhogg-wayland))
  (require 'nhwl-menu)
  (let* ((nhwl-menu-submenu-tolerance 0.3)
         (nhwl-menu-submenu-delay 0.1)
         (map (make-sparse-keymap))
         (sub (make-sparse-keymap))
         (index (lambda () (buffer-local-value 'nhwl-menu--index
                                               (window-buffer (frame-root-window (glitnir-gui--menu-frame 0))))))
         (width (lambda () (window-body-width (frame-root-window (glitnir-gui--menu-frame 0)) t))))
    (dolist (n '(5 4 3 2 1)) (define-key sub (vector (intern (format "in%d" n))) (list 'menu-item (format "Inner %d" n) 'ignore)))
    (define-key map [top] '(menu-item "Top" ignore))
    (define-key map [more] (list 'menu-item "More" sub))
    (define-key map [other] '(menu-item "Other" ignore))
    (define-key map [rest] '(menu-item "Rest" ignore))
    (let ((got (glitnir-gui--menu-steps
                map
                (list
                 ;; rows: Top, More, Other, Rest; the pointer rests on More: its submenu opens
                 (cons 0.5 (lambda () (push (glitnir-gui--menu-motion 0 1 10) unread-command-events) t))
                 (cons 0.5 (lambda ()
                             (prog1 (list (and (glitnir-gui--menu-frame 1) t) (funcall index))
                               ;; along More to its right end, then down onto Other, towards the submenu
                               (setq unread-command-events
                                     (append unread-command-events
                                             (list (glitnir-gui--menu-motion 0 1 (- (funcall width) 12))
                                                   (glitnir-gui--menu-motion 0 2 (- (funcall width) 4))))))))
                 (cons 0.1 (lambda () (list (and (glitnir-gui--menu-frame 1) t) (funcall index))))
                 (cons 0.6 (lambda ()
                             (prog1 (list (and (glitnir-gui--menu-frame 1) t) (funcall index))
                               (push ?\C-g unread-command-events))))))))
      (should (equal (nth 1 got) '(t 1)))     ; the submenu open on More
      (should (equal (nth 2 got) '(t 1)))     ; still open, More still selected, within the tolerance
      (should (equal (nth 3 got) '(nil 2)))))) ; after it, Other

;;; glitnir-gui-tests.el ends here
