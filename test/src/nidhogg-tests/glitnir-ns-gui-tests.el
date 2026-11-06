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

;;; The pair: a child window and its shadow window, checked in the window server's model (no permission needed)

(defconst glitnir-ns--shadow '(:blur 16 :offset (0 . 6) :spread 2 :opacity 0.35)
  "The shadow the pair rows give their child frames.")

(defun glitnir-ns--chromed (&optional params)
  "A shown child frame with corners and `glitnir-ns--shadow', and PARAMS."
  (glitnir-ns--child (append params `((corner-radius . 8) (shadow . ,glitnir-ns--shadow)))))

(defun glitnir-ns--margin (frame)
  "The margin (points) FRAME's shadow window has around the child window: 1.5 blur + |x| + |y| + spread, up."
  (let* ((sh (plist-get (glitnir--resolve frame) :shadow)) (off (plist-get sh :offset)))
    (ceiling (+ (* 1.5 (plist-get sh :blur)) (abs (car off)) (abs (cdr off)) (plist-get sh :spread)))))

(defun glitnir-ns--pair-problem (frame)
  "Nil when FRAME's shadow window is where it belongs, else what is wrong with the pair.
It belongs at the child window's bounds grown by the margin, directly below the child window, with its alpha."
  (let* ((e (glitnir-ns--pair frame)) (c (plist-get e :child)) (s (plist-get e :shadow))
         (m (and e (glitnir-ns--margin frame))))
    (cond ((null e) "no entry")
          ((null c) (list "the child is not on screen" e))
          ((null s) (list "no shadow window" e))
          ((not (plist-get e :below)) (list "the shadow window is not directly below" e))
          ((cl-notevery (lambda (a b) (< (abs (- a b)) 0.01))
                        s (list (- (nth 0 c) m) (- (nth 1 c) m) (+ (nth 2 c) m m) (+ (nth 3 c) m m)))
           (list "shadow bounds" :margin m e))
          ((> (abs (- (plist-get e :alpha) (plist-get e :shadow-alpha))) 0.01) (list "alpha" e)))))

(defun glitnir-ns--settled-problem (frame)
  "Redisplay, wait (5 s at most) for FRAME's pair to hold, and return what is still wrong with it, or nil."
  (redisplay t)
  (gui-runner-wait (lambda () (null (glitnir-ns--pair-problem frame))) 5)
  (glitnir-ns--pair-problem frame))

(defun glitnir-ns--orphans ()
  "The shadow windows on screen whose child frame is gone (entries without a frame)."
  (cl-remove-if (lambda (e) (plist-get e :frame)) (glitnir--ns-pairs)))

(defun glitnir-ns--gone-p (frame)
  "Wait (5 s at most) until neither FRAME's window nor a shadow window of it is on screen, and no shadow is orphaned."
  (redisplay t)
  (gui-runner-wait (lambda () (and (null (glitnir-ns--pair frame)) (null (glitnir-ns--orphans)))) 5))

(defun glitnir-ns--stack (&rest frames)
  "FRAMES in the window server's order, bottom to top."
  (cl-remove-if-not (lambda (f) (memq f frames)) (mapcar (lambda (e) (plist-get e :frame)) (glitnir--ns-pairs))))

(defun glitnir-ns--stacked (order)
  "Wait (5 s at most) until the window server stacks the frames of ORDER so, bottom to top; return their order."
  (redisplay t)
  (gui-runner-wait (lambda () (equal (apply #'glitnir-ns--stack order) order)) 5)
  (apply #'glitnir-ns--stack order))

(defun glitnir-ns--child-bounds (frame pred)
  "Wait (5 s at most) until FRAME's window bounds in the window server's model satisfy PRED; return them.
The model follows a step asynchronously: a pair checked before it caught up is the old, consistent pair."
  (redisplay t)
  (gui-runner-wait (lambda () (let ((b (plist-get (glitnir-ns--pair frame) :child))) (and b (funcall pred b)))) 5)
  (plist-get (glitnir-ns--pair frame) :child))

;;; Pixels

(defun glitnir-ns--px (raw x y)
  "The pixel at device X, Y of RAW (`glitnir-snapshot' raw) as (R G B A)."
  (pcase-let ((`(,w ,_h ,bytes) raw))
    (let ((i (* 4 (+ (* y w) x))))
      (list (aref bytes (+ i 2)) (aref bytes (+ i 1)) (aref bytes i) (aref bytes (+ i 3))))))

(defun glitnir-ns--luma (px)
  (+ (* 0.2126 (nth 0 px)) (* 0.7152 (nth 1 px)) (* 0.0722 (nth 2 px))))

(defun glitnir-ns--rect (frame)
  "FRAME's window in device pixels of the root's capture, (X Y W H); it waits (5 s at most) for it to be listed."
  (let ((s (frame-scale-factor (frame-parent frame)))
        (e (gui-runner-wait (lambda () (plist-get (glitnir-ns--pair frame) :child)) 5)))
    (should e)
    (mapcar (lambda (v) (round (* s v))) e)))

(defun glitnir-ns--corners (rect)
  "The pixels one device pixel in from each corner of RECT, as (X . Y)."
  (pcase-let ((`(,x ,y ,w ,h) rect))
    (list (cons (+ x 1) (+ y 1)) (cons (+ x w -2) (+ y 1)) (cons (+ x 1) (+ y h -2)) (cons (+ x w -2) (+ y h -2)))))

(defun glitnir-ns--snap-px (spot)
  (glitnir-ns--px (glitnir-snapshot nil 'raw) (car spot) (cdr spot)))

(defun glitnir-ns--diff (a b rect)
  "Compare RECT (X Y W H, device pixels) of raw captures A and B: (DIFFERING-PIXELS MAX-CHANNEL-DIFF)."
  (pcase-let ((`(,x ,y ,w ,h) rect) (`(,aw ,_ ,ab) a) (`(,bw ,_ ,bb) b) (n 0) (mx 0))
    (dotimes (j h)
      (let ((ia (* 4 (+ (* (+ y j) aw) x))) (ib (* 4 (+ (* (+ y j) bw) x))))
        (unless (string= (substring ab ia (+ ia (* 4 w))) (substring bb ib (+ ib (* 4 w))))
          (dotimes (i w)
            (let ((d 0))
              (dotimes (k 4)
                (setq d (max d (abs (- (aref ab (+ ia (* 4 i) k)) (aref bb (+ ib (* 4 i) k)))))))
              (when (> d 0) (setq n (1+ n) mx (max mx d))))))))
    (list n mx)))

(defun glitnir-ns--text-buffer ()
  "A buffer of a few lines of text and no mode line."
  (with-current-buffer (get-buffer-create " *glitnir-ns-text*")
    (erase-buffer)
    (insert "The quick brown fox jumps over the lazy dog.\n0123456789 ([{<>}]) ~!@#$%^&*\nglitnir: ns text\n")
    (goto-char (point-min))
    (setq mode-line-format nil)
    (current-buffer)))

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

;;; Corners (pixels: the window server's composite of the root's content)

(ert-deftest glitnir-ns/corner-shows-what-is-beneath ()
  ;; handshake: each capture follows the window server's showing the step
  (glitnir-ns--skip-unless-ns)
  (glitnir-ns--skip-unless-capture)
  (let ((c (glitnir-ns--child '((corner-radius . 8) (shadow . nil)))))
    (unwind-protect
        (let* ((rect (glitnir-ns--rect c)) (spots (glitnir-ns--corners rect))
               (mid (cons (+ (nth 0 rect) (/ (nth 2 rect) 2)) (+ (nth 1 rect) (/ (nth 3 rect) 2)))))
          ;; the child is drawn (its middle is its cyan background), and its corners stop showing it
          (should (gui-runner-wait (lambda () (let ((p (glitnir-ns--snap-px mid)))
                                                (and (< (nth 0 p) 60) (> (nth 1 p) 200) (> (nth 2 p) 200))))
                                   5))
          (gui-runner-wait (lambda () (not (equal (glitnir-ns--snap-px (car spots)) (glitnir-ns--snap-px mid)))) 5)
          (let ((on (glitnir-snapshot nil 'raw)))
            (make-frame-invisible c)
            (should (glitnir-ns--gone-p c))
            (let ((beneath (glitnir-snapshot nil 'raw)))
              (dolist (p spots)
                (should (equal (list p (glitnir-ns--px on (car p) (cdr p)))
                               (list p (glitnir-ns--px beneath (car p) (cdr p)))))))))
      (delete-frame c t))))

(ert-deftest glitnir-ns/text-identical-to-stock ()
  ;; handshake: the chromed capture follows the window server's showing the corners
  (glitnir-ns--skip-unless-ns)
  (glitnir-ns--skip-unless-capture)
  (let* ((buf (glitnir-ns--text-buffer)) (c (glitnir-ns--child)))
    (unwind-protect
        (let* ((s (frame-scale-factor)) (rect (glitnir-ns--rect c)) (corner (car (glitnir-ns--corners rect)))
               (bg (cons (+ (nth 0 rect) (/ (nth 2 rect) 2)) (+ (nth 1 rect) (nth 3 rect) (round (* -4 s)))))
               (in (ceiling (* 9 s)))
               (inner (list (+ (nth 0 rect) in) (+ (nth 1 rect) in) (- (nth 2 rect) in in) (- (nth 3 rect) in in))))
          (set-window-buffer (frame-root-window c) buf)
          (redisplay t)
          (should (gui-runner-wait (lambda () (let ((p (glitnir-ns--snap-px bg)))
                                                (and (< (nth 0 p) 60) (> (nth 1 p) 200) (> (nth 2 p) 200))))
                                   5))
          (let ((stock (glitnir-snapshot nil 'raw)) (stock-corner (glitnir-ns--snap-px corner)))
            (modify-frame-parameters c `((corner-radius . 8) (shadow . ,glitnir-ns--shadow)))
            (redisplay t)
            (gui-runner-wait (lambda () (not (equal (glitnir-ns--snap-px corner) stock-corner))) 5)
            (let ((on (glitnir-snapshot nil 'raw)))
              ;; the chrome is on: the corner changed
              (should-not (equal (glitnir-ns--px on (car corner) (cdr corner)) stock-corner))
              ;; and the text is the stock frame's, pixel for pixel
              (should (equal (glitnir-ns--diff stock on inner) '(0 0))))))
      (delete-frame c t)
      (kill-buffer buf))))

;;; The shadow window

(ert-deftest glitnir-ns/shadow-darkens-below ()
  ;; handshake: the shadowed capture follows the window server's showing the shadow
  (glitnir-ns--skip-unless-ns)
  (glitnir-ns--skip-unless-capture)
  (let ((c (glitnir-ns--child '((corner-radius . 8) (shadow . nil)))))
    (unwind-protect
        (let* ((s (frame-scale-factor)) (rect (glitnir-ns--rect c))
               (below (cons (+ (nth 0 rect) (/ (nth 2 rect) 2)) (+ (nth 1 rect) (nth 3 rect) (round (* 6 s)))))
               (before (glitnir-ns--luma (glitnir-ns--snap-px below))))
          (modify-frame-parameters c '((shadow . (:blur 16 :offset (0 . 6) :opacity 0.6))))
          (redisplay t)
          (gui-runner-wait (lambda () (< (glitnir-ns--luma (glitnir-ns--snap-px below)) (- before 10))) 5)
          (should (< (glitnir-ns--luma (glitnir-ns--snap-px below)) (- before 10))))
      (delete-frame c t))))

(ert-deftest glitnir-ns/no-shadow-window-without-shadow ()
  ;; handshake: each check waits for the window server's model to show the step
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--chromed)) (d (glitnir-ns--child '((corner-radius . 8)))))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem c))
          ;; the shadow goes: its window goes, the corners stay
          (modify-frame-parameters c '((shadow . nil)))
          (redisplay t)
          (should (gui-runner-wait (lambda () (let ((e (glitnir-ns--pair c))) (and e (null (plist-get e :shadow))))) 5))
          (should-not (glitnir-ns--orphans))
          (should (> (plist-get (glitnir--chrome c) :radius) 0))
          (should-not (plist-get (glitnir--chrome c) :shadow))
          ;; corners alone never make one
          (should (gui-runner-wait (lambda () (glitnir-ns--pair d)) 5))
          (should-not (plist-get (glitnir-ns--pair d) :shadow))
          (should (> (plist-get (glitnir--chrome d) :radius) 0)))
      (delete-frame c t)
      (delete-frame d t))))

(ert-deftest glitnir-ns/alpha-applies-to-pair ()
  ;; handshake: the check waits for the window server's model to show the alpha
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem c))
          (set-frame-parameter c 'alpha 0.5)
          (redisplay t)
          (gui-runner-wait (lambda () (let ((e (glitnir-ns--pair c)))
                                        (and (< (abs (- (or (plist-get e :alpha) 1) 0.5)) 0.01)
                                             (< (abs (- (or (plist-get e :shadow-alpha) 1) 0.5)) 0.01))))
                           5)
          (let ((e (glitnir-ns--pair c)))
            (should (< (abs (- (plist-get e :alpha) 0.5)) 0.01))
            (should (< (abs (- (or (plist-get e :shadow-alpha) 1) 0.5)) 0.01)))
          (should-not (glitnir-ns--settled-problem c)))
      (delete-frame c t))))

;;; The pair through the child window's life

(ert-deftest glitnir-ns/pair-follows-move ()
  ;; handshake: after each step the check waits for the window server's model
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem c))
          (let ((x0 (car (plist-get (glitnir-ns--pair c) :child))))
            (set-frame-position c 120 80)
            (should (> (car (glitnir-ns--child-bounds c (lambda (b) (> (car b) x0)))) x0))
            (should-not (glitnir-ns--settled-problem c))
            (modify-frame-parameters c '((left . 60) (top . 50)))
            (should (< (car (glitnir-ns--child-bounds c (lambda (b) (< (car b) (+ x0 40))))) (+ x0 40)))
            (should-not (glitnir-ns--settled-problem c))))
      (delete-frame c t))))

(ert-deftest glitnir-ns/pair-follows-resize ()
  ;; handshake: after each step the check waits for the window server's model
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem c))
          (let ((w0 (nth 2 (plist-get (glitnir-ns--pair c) :child))))
            (set-frame-size c 50 12)
            (should (> (nth 2 (glitnir-ns--child-bounds c (lambda (b) (> (nth 2 b) w0)))) w0))
            (should-not (glitnir-ns--settled-problem c))
            (set-frame-size c 200 90 t)
            (should (< (nth 2 (glitnir-ns--child-bounds c (lambda (b) (< (nth 2 b) w0)))) w0))
            (should-not (glitnir-ns--settled-problem c))))
      (delete-frame c t))))

(ert-deftest glitnir-ns/pair-follows-raise ()
  ;; handshake: after each step the check waits for the window server's model
  (glitnir-ns--skip-unless-ns)
  (let* ((a (glitnir-ns--chromed)) (b (glitnir-ns--chromed '((left . 80) (top . 60)))))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem a))
          (should-not (glitnir-ns--settled-problem b))
          (should (equal (glitnir-ns--stacked (list a b)) (list a b)))
          (raise-frame a)
          (should (equal (glitnir-ns--stacked (list b a)) (list b a)))
          (should-not (glitnir-ns--settled-problem a))
          (should-not (glitnir-ns--settled-problem b))
          (raise-frame b)
          (should (equal (glitnir-ns--stacked (list a b)) (list a b)))
          (should-not (glitnir-ns--settled-problem a))
          (should-not (glitnir-ns--settled-problem b)))
      (delete-frame b t)
      (delete-frame a t))))

(ert-deftest glitnir-ns/pair-follows-order-back ()
  ;; handshake: after each step the check waits for the window server's model
  (glitnir-ns--skip-unless-ns)
  (let* ((a (glitnir-ns--chromed)) (b (glitnir-ns--chromed '((left . 80) (top . 60)))))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem b))
          (lower-frame b)
          (should (equal (glitnir-ns--stacked (list b a)) (list b a)))
          (should-not (glitnir-ns--settled-problem a))
          (should-not (glitnir-ns--settled-problem b)))
      (delete-frame b t)
      (delete-frame a t))))

(ert-deftest glitnir-ns/pair-follows-restack ()
  ;; handshake: after each step the check waits for the window server's model
  (glitnir-ns--skip-unless-ns)
  (let* ((a (glitnir-ns--chromed)) (b (glitnir-ns--chromed '((left . 80) (top . 60))))
         (c (glitnir-ns--chromed '((left . 120) (top . 80)))))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem c))
          (frame-restack c a)             ; c below a
          (should (equal (glitnir-ns--stacked (list c a b)) (list c a b)))
          (dolist (f (list a b c)) (should-not (glitnir-ns--settled-problem f)))
          (frame-restack c a t)           ; c above a
          (should (equal (glitnir-ns--stacked (list a c b)) (list a c b)))
          (dolist (f (list a b c)) (should-not (glitnir-ns--settled-problem f))))
      (delete-frame c t)
      (delete-frame b t)
      (delete-frame a t))))

(ert-deftest glitnir-ns/pair-follows-z-group ()
  ;; handshake: after each step the check waits for the window server's model
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem c))
          (set-frame-parameter c 'z-group 'above)
          (should-not (glitnir-ns--settled-problem c))
          (set-frame-parameter c 'z-group nil)
          (should-not (glitnir-ns--settled-problem c)))
      (delete-frame c t))))

(ert-deftest glitnir-ns/pair-follows-hide-show ()
  ;; handshake: after each step the check waits for the window server's model
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem c))
          (dotimes (_ 3)
            (make-frame-invisible c)
            (should (glitnir-ns--gone-p c))
            (make-frame-visible c)
            (should-not (glitnir-ns--settled-problem c))))
      (delete-frame c t))))

(ert-deftest glitnir-ns/pair-follows-parent-move ()
  ;; handshake: after each step the check waits for the window server's model
  (glitnir-ns--skip-unless-ns)
  (let* ((root (selected-frame)) (pos (frame-position root)) (c (glitnir-ns--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem c))
          (let ((at (plist-get (glitnir-ns--pair c) :child)))
            (set-frame-position root (+ (car pos) 30) (+ (cdr pos) 20))
            ;; the bounds are the root content's: the child keeps its place in it, once the model has both moves
            (should (equal (glitnir-ns--child-bounds c (lambda (b) (equal b at))) at))
            (should-not (glitnir-ns--settled-problem c))))
      (set-frame-position root (car pos) (cdr pos))
      (delete-frame c t))))

(ert-deftest glitnir-ns/pair-follows-reparent ()
  ;; handshake: after each step the check waits for the window server's model
  (glitnir-ns--skip-unless-ns)
  (let* ((root (selected-frame))
         (p (glitnir-ns--child '((left . 10) (top . 10) (width . 70) (height . 25) (background-color . "#ffff00"))))
         (c (glitnir-ns--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem c))
          (set-frame-parameter c 'parent-frame p)
          (should (eq (frame-parent c) p))
          (should (equal (glitnir-ns--stacked (list p c)) (list p c)))
          (should-not (glitnir-ns--settled-problem c))
          (set-frame-parameter c 'parent-frame root)
          (should-not (glitnir-ns--settled-problem c))
          (should-not (glitnir-ns--orphans)))
      (delete-frame c t)
      (delete-frame p t))))

(ert-deftest glitnir-ns/pair-follows-undecorated-toggle ()
  ;; handshake: after each step the check waits for the window server's model
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem c))
          ;; a decorated child frame has no chrome: the shadow window goes with the old window
          (set-frame-parameter c 'undecorated nil)
          (redisplay t)
          (should (gui-runner-wait (lambda () (let ((e (glitnir-ns--pair c)))
                                                (and e (plist-get e :child) (null (plist-get e :shadow))
                                                     (null (glitnir-ns--orphans)))))
                                   5))
          (should-not (glitnir--chrome c))
          ;; undecorated again: a new window, and the pair again
          (set-frame-parameter c 'undecorated t)
          (should-not (glitnir-ns--settled-problem c))
          (should-not (glitnir-ns--orphans)))
      (delete-frame c t))))

(ert-deftest glitnir-ns/pair-follows-delete ()
  ;; handshake: the check waits for the window server's model
  (glitnir-ns--skip-unless-ns)
  (let ((c (glitnir-ns--chromed)))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem c))
          (delete-frame c t)
          (should (gui-runner-wait (lambda () (null (glitnir-ns--orphans))) 5))
          (should-not (glitnir-ns--orphans)))
      (when (frame-live-p c) (delete-frame c t)))))

;;; glitnir-ns-gui-tests.el ends here
