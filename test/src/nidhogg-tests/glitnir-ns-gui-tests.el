;;; glitnir-ns-gui-tests.el --- Glitnir on the macOS (NS) build  -*- lexical-binding: t -*-

;; test-class: cond
;; test-pattern: handshake

;; Run by nidhogg/test/ns-gui.sh on the macOS runner (test/src/nidhogg-tests/gui-runner.el loads this file only on an
;; NS build).  GLITNIR_GUI_SCALE is the display scale the run asked for.  Frame sizes on NS are logical pixels (points);
;; `glitnir-snapshot' and `glitnir--chrome' give device pixels, as on the native shell.  The pixel rows skip with the
;; reason when the process may not capture the screen (`glitnir-no-capture'); the state rows run either way.

(require 'ert)
(require 'cl-lib)

(defun glitnir-ns--scale ()
  (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1")))

(defun glitnir-ns--skip-unless-ns ()
  (unless (featurep 'ns) (ert-skip "not an NS build")))

(defun glitnir-ns--skip-unless-capture ()
  "Skip the running row unless the process may capture the screen."
  (condition-case err
      (glitnir-snapshot nil 'raw)
    (glitnir-no-capture (ert-skip (format "no screen capture: %s" (cadr err))))))

(defun glitnir-ns--child (&optional params)
  "A shown, undecorated child frame of the selected frame, with PARAMS (an alist) applied."
  (let* ((root (selected-frame))
         (f (make-frame `((parent-frame . ,root) (minibuffer . nil) (left . 40) (top . 40)
                          (width . 40) (height . 10) (background-color . "#00ffff")
                          (visibility . nil) (no-accept-focus . t) (no-other-frame . t)
                          (undecorated . t) (internal-border-width . 1)))))
    (select-frame root)
    (when params (modify-frame-parameters f params))
    (make-frame-visible f)
    (redisplay t)
    f))

(defun glitnir-ns--pair (frame)
  "The entry of `glitnir--ns-pairs' for FRAME, or nil."
  (cl-find frame (glitnir--ns-pairs) :key (lambda (e) (plist-get e :frame))))

(ert-deftest glitnir-ns/runner-smoke ()
  ;; The lane's own proof: a frame at the asked scale, and a capture of its size.
  (glitnir-ns--skip-unless-ns)
  (should (< (abs (- (frame-scale-factor) (glitnir-ns--scale))) 0.001))
  (glitnir-ns--skip-unless-capture)
  (pcase-let ((`(,w ,h ,bytes) (glitnir-snapshot nil 'raw))
              (s (frame-scale-factor)))
    (should (= w (round (* s (frame-pixel-width)))))
    (should (= h (round (* s (frame-pixel-height)))))
    (should (= (length bytes) (* 4 w h)))
    (should-not (multibyte-string-p bytes)))
  (should (string-prefix-p "\x89PNG" (glitnir-snapshot nil))))

(ert-deftest glitnir-ns/active-p-follows-enable ()
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--child)))
    (unwind-protect
        (progn
          (should glitnir-enable)
          (should (glitnir-active-p))
          (should (glitnir-active-p c))
          (let ((glitnir-enable nil))
            (should-not (glitnir-active-p))
            (should-not (glitnir-active-p c)))
          (should (glitnir-active-p c)))
      (delete-frame c t))))

(ert-deftest glitnir-ns/chrome-reaches-the-window ()
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--child '((corner-radius . 8)
                                (shadow . (:blur 16 :offset (0 . 6) :spread 2 :opacity 0.35))))))
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
          ;; Glitnir off: the window holds nothing
          (let ((glitnir-enable nil))
            (should-not (glitnir--chrome c)))
          (should (glitnir--chrome c))
          ;; opting out removes it
          (modify-frame-parameters c '((corner-radius . 0) (shadow . nil)))
          (should-not (glitnir--chrome c)))
      (delete-frame c t))))

(ert-deftest glitnir-ns/pairs-list-the-children ()
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--child)))
    (unwind-protect
        (let ((e (gui-runner-wait (lambda () (glitnir-ns--pair c)))))
          (should e)
          (should (= (length (plist-get e :child)) 4))
          (should (cl-every #'numberp (plist-get e :child)))
          (should (> (nth 2 (plist-get e :child)) 0))
          ;; no chrome, no shadow window
          (should-not (plist-get e :shadow)))
      (delete-frame c t))))

;;; Window cards.  Two windows side by side, the left a card, a gap of 6 points between them in the divider colour.
;;; Each buffer has a colour of its own, no mode line, no cursor, no fringes and no scroll bars.  A window's place in a
;;; snapshot is its edges (points) times the scale; the colours a row compares come from the same snapshot (the middle
;;; of the gap, the middle of the window), as the window server shows them.

(defconst glitnir-ns--card-bg "#3070c0" "The card window's background.")
(defconst glitnir-ns--other-bg "#c07030" "The other window's background.")
(defconst glitnir-ns--gap "#20a040" "The divider colour: the gap, and what the card's corners show.")
(defconst glitnir-ns--radius 10 "The card's corner radius, points.")

(defconst glitnir-ns--card `((corner-radius . ,glitnir-ns--radius)) "A card with corners only.")

(defconst glitnir-ns--card-look
  `((corner-radius . ,glitnir-ns--radius) (shadow . (:blur 3 :offset (0 . 1) :opacity 0.45))
    (ring . (,glitnir-ns--gap . 0.6)))
  "The look's card: corners, a soft shadow one point down and the edge ring.")

(defun glitnir-ns--grab ()
  "The root's snapshot once the window server shows the last redisplay: two snapshots in a row are the same.
Every window is drawn again first (a window parameter change draws nothing by itself)."
  (force-window-update)
  (redisplay t)
  (let ((deadline (+ (float-time) 3)) (a (glitnir-snapshot nil 'raw)) b)
    (while (and (progn (accept-process-output nil 0.02)
                       (not (equal a (setq b (glitnir-snapshot nil 'raw)))))
                (< (float-time) deadline))
      (setq a b))
    (unless (equal a b) (ert-fail "the window server's picture did not settle in 3 s"))
    b))

(defun glitnir-ns--box (w)
  "Window W's edges without its dividers in a snapshot, (X Y WIDTH HEIGHT) in device pixels."
  (let ((s (frame-scale-factor)) (e (window-pixel-edges w)))
    (mapcar (lambda (v) (round (* s v)))
            (list (nth 0 e) (nth 1 e)
                  (- (window-pixel-width w) (window-right-divider-width w))
                  (- (window-pixel-height w) (window-bottom-divider-width w))))))

(defun glitnir-ns--px (grab x y)
  "Pixel (X . Y) of GRAB as (B G R)."
  (let ((o (* 4 (+ x (* y (nth 0 grab))))) (b (nth 2 grab)))
    (list (aref b o) (aref b (+ o 1)) (aref b (+ o 2)))))

(defun glitnir-ns--near (px want)
  "Non-nil when every channel of PX is within 2 of WANT's."
  (cl-every (lambda (a b) (<= (abs (- a b)) 2)) px want))

(defun glitnir-ns--gap-px (grab w)
  "The gap's colour in GRAB: the middle of window W's right divider, halfway down."
  (pcase-let ((`(,x ,y ,ww ,h) (glitnir-ns--box w)))
    (glitnir-ns--px grab (+ x ww (floor (* (window-right-divider-width w) (frame-scale-factor)) 2))
                    (+ y (/ h 2)))))

(defun glitnir-ns--with-cards (params fn)
  "Call FN with the left and right windows of the cards' layout, the left with window PARAMS, drawn.
Everything goes back after: one window on `*scratch*', the frame's divider width and colour."
  (let* ((f (selected-frame))
         (gap (frame-parameter f 'right-divider-width))
         (divider (face-attribute 'window-divider :foreground f))
         (a (get-buffer-create " *glitnir-ns-card-a*"))
         (b (get-buffer-create " *glitnir-ns-card-b*"))
         left right)
    (unwind-protect
        (progn
          (delete-other-windows)
          (set-face-foreground 'window-divider glitnir-ns--gap f)
          (set-frame-parameter f 'right-divider-width 6)
          (dolist (x (list (cons a glitnir-ns--card-bg) (cons b glitnir-ns--other-bg)))
            (with-current-buffer (car x)
              (setq mode-line-format nil cursor-type nil)
              (face-remap-add-relative 'default :background (cdr x))))
          (setq left (selected-window) right (split-window-right))
          (set-window-buffer left a)
          (set-window-buffer right b)
          (dolist (w (list left right))
            (set-window-fringes w 0 0)
            (set-window-scroll-bars w nil nil nil nil))
          (dolist (p params) (set-window-parameter left (car p) (cdr p)))
          (message nil)
          (force-window-update)
          (redisplay t)
          (funcall fn left right))
      (when (window-live-p left)
        (dolist (k '(corner-radius shadow ring)) (set-window-parameter left k nil)))
      (delete-other-windows)
      (switch-to-buffer "*scratch*")
      (set-frame-parameter f 'right-divider-width gap)
      (set-face-attribute 'window-divider f :foreground divider)
      (kill-buffer a)
      (kill-buffer b)
      (force-window-update)
      (redisplay t))))

(ert-deftest glitnir-ns/card-corner-shows-gap ()
  "A card window's corner pixels show the gap's colour; the same window without a card shows its own there."
  (glitnir-ns--skip-unless-ns)
  (glitnir-ns--skip-unless-capture)
  (glitnir-ns--with-cards
   glitnir-ns--card
   (lambda (left _right)
     (should (window-parameter left 'glitnir--card))
     (pcase-let* ((`(,x ,y ,w ,h) (glitnir-ns--box left))
                  (grab (glitnir-ns--grab))
                  (gap (glitnir-ns--gap-px grab left))
                  (bg (glitnir-ns--px grab (+ x (/ w 2)) (+ y (/ h 2))))
                  (r (round (* glitnir-ns--radius (frame-scale-factor)))))
       (should-not (glitnir-ns--near gap bg))
       ;; the window's four outermost corner pixels
       (dolist (c (list (cons x y) (cons (+ x w -1) y) (cons x (+ y h -1)) (cons (+ x w -1) (+ y h -1))))
         (should (equal (list c (glitnir-ns--near (glitnir-ns--px grab (car c) (cdr c)) gap)) (list c t))))
       ;; the top edge past the corner square is the window's
       (should (glitnir-ns--near (glitnir-ns--px grab (+ x r) y) bg))
       (set-window-parameter left 'corner-radius nil)
       (should (glitnir-ns--near (glitnir-ns--px (glitnir-ns--grab) x y) bg))))))

(ert-deftest glitnir-ns/card-shadow-only-in-gap ()
  "The look's card leaves the other window's pixels as they are without it; the gap beside the card is darker."
  (glitnir-ns--skip-unless-ns)
  (glitnir-ns--skip-unless-capture)
  (glitnir-ns--with-cards
   glitnir-ns--card-look
   (lambda (left right)
     (pcase-let* ((`(,x ,y ,w ,_) (glitnir-ns--box left))
                  (s (frame-scale-factor))
                  (with (glitnir-ns--grab))
                  (other (glitnir-ns--box right))
                  (gap (list (+ x w) (+ y (round (* 20 s)))
                             (round (* (window-right-divider-width left) s)) (round (* 20 s)))))
       (dolist (k '(corner-radius shadow ring)) (set-window-parameter left k nil))
       (let ((without (glitnir-ns--grab)))
         (should (eql 0 (cdr (gui-runner-pixel-diff with without other))))
         (should (> (cdr (gui-runner-pixel-diff with without gap)) 0)))))))

(ert-deftest glitnir-ns/card-typing-no-layer-change ()
  "A card makes its layers once; 50 characters typed in the card window change no layer."
  (glitnir-ns--skip-unless-ns)
  (let ((before (glitnir--ns-card-updates)))
    (glitnir-ns--with-cards
     glitnir-ns--card
     (lambda (left _right)
       (should (> (glitnir--ns-card-updates) before))
       (let ((buf (window-buffer left)))
         (with-current-buffer buf
           (dotimes (_ 12) (insert "\n"))
           (forward-line -6)
           (insert "  "))
         (set-window-point left (with-current-buffer buf (point)))
         (redisplay t)
         (let ((n (glitnir--ns-card-updates)))
           ;; a newline every 20 keeps the line from wrapping
           (dotimes (i 50)
             (with-current-buffer buf (insert (if (= (% (1+ i) 20) 0) "\n" "x")))
             (redisplay t))
           (should (= (glitnir--ns-card-updates) n))))))))

(ert-deftest glitnir-ns/card-removed-no-residue ()
  "A card that goes leaves the frame as it was before it came, and the frame has no cards."
  (glitnir-ns--skip-unless-ns)
  (glitnir-ns--skip-unless-capture)
  (glitnir-ns--with-cards
   nil
   (lambda (left _right)
     (let ((base (glitnir-ns--grab)))
       (dolist (p glitnir-ns--card-look) (set-window-parameter left (car p) (cdr p)))
       (should (> (cdr (gui-runner-pixel-diff base (glitnir-ns--grab))) 0))
       (dolist (p glitnir-ns--card-look) (set-window-parameter left (car p) nil))
       (should (eql 0 (cdr (gui-runner-pixel-diff base (glitnir-ns--grab)))))
       (should-not (frame-parameter nil 'glitnir--cards))))))

;;; glitnir-ns-gui-tests.el ends here
