;;; glitnir-gui-tests.el --- GUI tests for Glitnir (the compositor)  -*- lexical-binding: t -*-

;; test-class: cond
;; test-pattern: handshake

;; Run by nidhogg/test/gui.sh (test/src/nidhogg-tests/gui-runner.el loads every *-gui-tests.el), and on macOS by
;; nidhogg/test/nhmac-gui.sh, where the rows that need the Wayland shell's scene tree skip (`glitnir-gui--skip-unless-tree').  GLITNIR_GUI_SCALE is
;; the monitor scale the run asked for.  The rows judge what the shell gave the compositor: `glitnir-snapshot' of a root
;; holds its whole tree, child frames at their places.  Frame sizes and positions on the native shell are device pixels.

(require 'ert)
(require 'cl-lib)

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

;;; glitnir-gui-tests.el ends here
