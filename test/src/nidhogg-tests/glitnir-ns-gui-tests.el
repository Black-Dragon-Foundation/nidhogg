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

;;; Motion: the content layer's and the shadow layer's opacity and scale over time, as `glitnir--ns-motion' reads them
;;; from Core Animation's presentation layers, for the leg a show or a hide started.  The runner asks for reduced
;;; motion, so the rows bind `glitnir--reduced-motion' to `off' (and one row to `on').  The legs run 0.6 s, long enough
;;; for the samples to see them on the runner's VM; each sampling ends on the state (no leg left), not on a time.

(defvar glitnir--reduced-motion)

(defconst glitnir-ns--pop '(:show pop :duration 0.6 :curve ease-out) "A pop in over 0.6 s; no hide leg.")
(defconst glitnir-ns--fade-out '(:hide fade :hide-duration 0.6 :hide-curve linear) "A fade out over 0.6 s; no show leg.")
(defconst glitnir-ns--fade-both '(:show fade :duration 0.6 :curve linear :hide fade :hide-duration 0.6 :hide-curve linear)
  "A fade in and a fade out, 0.6 s each.")

(defun glitnir-ns--bezier (curve p)
  "The progress at time fraction P along CSS cubic-bezier CURVE, (X1 Y1 X2 Y2)."
  (pcase-let ((`(,x1 ,y1 ,x2 ,y2) curve) (lo 0.0) (hi 1.0))
    (let ((at (lambda (a b u) (+ (* 3 a u (- 1 u) (- 1 u)) (* 3 b u u (- 1 u)) (* u u u)))))
      (dotimes (_ 40)
        (let ((mid (/ (+ lo hi) 2)))
          (if (< (funcall at x1 x2 mid) p) (setq lo mid) (setq hi mid))))
      (funcall at y1 y2 (/ (+ lo hi) 2)))))

(defun glitnir-ns--at-rest-p (m)
  "Non-nil when motion sample M shows no leg running or waiting to start, and shows the model's values.
The presentation layer shows a change once Core Animation committed it."
  (and m (eql (plist-get m :animations) 0) (not (plist-get m :hiding)) (not (plist-get m :pending))
       (eql (plist-get m :opacity) (plist-get m :model-opacity))
       (eql (plist-get m :scale) (plist-get m :model-scale))))

(defun glitnir-ns--samples (frame done &optional seconds)
  "Sample FRAME's motion until DONE, a function of a sample, returns non-nil (SECONDS, default 5, at most).
A sample is `glitnir--ns-motion''s plist with :bounds added, the child window's bounds in the window server's model.
Return the samples, oldest first."
  (let (samples)
    (gui-runner-wait (lambda ()
                       (let ((m (append (glitnir--ns-motion frame)
                                        (list :bounds (plist-get (glitnir-ns--pair frame) :child)))))
                         (push m samples)
                         (funcall done m)))
                     (or seconds 5))
    (nreverse samples)))

(defun glitnir-ns--mid (samples)
  "The samples of SAMPLES taken while a leg ran, strictly between its ends."
  (cl-remove-if-not (lambda (m) (let ((e (plist-get m :elapsed)) (d (plist-get m :duration)))
                                  (and e d (< 0 e d) (< 0 (plist-get m :opacity) 1))))
                    samples))

(defun glitnir-ns--off-curve (samples from to curve)
  "The samples of SAMPLES whose opacity is over 0.1 off the leg from FROM to TO along CURVE at their time."
  (cl-remove-if (lambda (m) (let ((want (+ from (* (- to from) (glitnir-ns--bezier
                                                                  curve (/ (plist-get m :elapsed)
                                                                           (plist-get m :duration)))))))
                              (<= (abs (- (plist-get m :opacity) want)) 0.1)))
                samples))

(defun glitnir-ns--rest (m)
  "Motion sample M's end state: (OPACITY SCALE MODEL-OPACITY MODEL-SCALE SHADOW-OPACITY SHADOW-SCALE ANIMATIONS)."
  (mapcar (lambda (k) (plist-get m k))
          '(:opacity :scale :model-opacity :model-scale :shadow-opacity :shadow-scale :animations)))

(defconst glitnir-ns--opaque '(1.0 1.0 1.0 1.0 1.0 1.0 0) "`glitnir-ns--rest' of a shown pair at rest.")

(defun glitnir-ns--monotone (xs pred)
  "Non-nil when each of XS is PRED of the next one, within 0.01."
  (cl-loop for (a b) on xs while b always (or (funcall pred a b) (< (abs (- a b)) 0.01))))

(defun glitnir-ns--bounds-seen (samples)
  "The child window's bounds SAMPLES saw while it was listed, each one once."
  (delete-dups (delq nil (mapcar (lambda (m) (plist-get m :bounds)) samples))))

(ert-deftest glitnir-ns/pop-in-ends-opaque ()
  ;; handshake: the samples follow Core Animation's presentation layers until no leg is left
  (glitnir-ns--skip-unless-ns)
  (let* ((glitnir--reduced-motion 'off)
         (curve (cdr (assq 'ease-out glitnir--curves)))
         (c (glitnir-ns--chromed `((transition . ,glitnir-ns--pop)))))
    (unwind-protect
        (let* ((samples (glitnir-ns--samples c #'glitnir-ns--at-rest-p))
               (mid (glitnir-ns--mid samples)) (last (car (last samples))))
          ;; the leg ran: opacity along the curve, the scale on the same curve from 0.96, the shadow in step
          (should (>= (length mid) 3))
          (should-not (glitnir-ns--off-curve mid 0 1 curve))
          (dolist (m mid)
            (should (< (plist-get m :scale) 1))
            (should (< (abs (- (plist-get m :opacity) (/ (- (plist-get m :scale) 0.96) 0.04))) 0.02))
            (should (< (abs (- (plist-get m :shadow-opacity) (plist-get m :opacity))) 0.02))
            (should (< (abs (- (plist-get m :shadow-scale) (plist-get m :scale))) 0.002)))
          ;; it ended opaque at full size, nothing left animating
          (should (glitnir-ns--at-rest-p last))
          (should (equal (glitnir-ns--rest last) glitnir-ns--opaque))
          (should-not (glitnir-ns--settled-problem c))
          ;; hidden (no hide leg) and shown again: the window keeps its bounds through the leg and lands on them
          (let ((at (plist-get (glitnir-ns--pair c) :child)))
            (make-frame-invisible c)
            (should (glitnir-ns--gone-p c))
            (make-frame-visible c)
            (redisplay t)
            (let ((again (glitnir-ns--samples c #'glitnir-ns--at-rest-p)))
              (should (glitnir-ns--mid again))
              (should (equal (glitnir-ns--bounds-seen again) (list at)))
              (should (equal (glitnir-ns--rest (car (last again))) glitnir-ns--opaque))))
          (should-not (glitnir-ns--settled-problem c)))
      (delete-frame c t))))

(ert-deftest glitnir-ns/fade-out-ends-hidden ()
  ;; handshake: the samples follow the presentation layers; the end waits for the window server's model
  (glitnir-ns--skip-unless-ns)
  (let* ((glitnir--reduced-motion 'off)
         (c (glitnir-ns--chromed `((transition . ,glitnir-ns--fade-out)))))
    (unwind-protect
        (progn
          ;; no show leg: shown at once
          (should (glitnir-ns--at-rest-p (car (last (glitnir-ns--samples c #'glitnir-ns--at-rest-p)))))
          (should-not (glitnir-ns--settled-problem c))
          (let ((at (plist-get (glitnir-ns--pair c) :child)))
            (make-frame-invisible c)
            ;; Emacs sees the frame hidden at once; its window stays, click-through, while the leg runs
            (should-not (frame-visible-p c))
            (let* ((samples (glitnir-ns--samples c #'glitnir-ns--at-rest-p))
                   (mid (glitnir-ns--mid samples)))
              (should (>= (length mid) 3))
              (should-not (glitnir-ns--off-curve mid 1 0 '(0 0 1 1)))
              (should (glitnir-ns--monotone (mapcar (lambda (m) (plist-get m :opacity)) mid) #'>=))
              (dolist (m mid)
                (should (plist-get m :hiding))
                (should (plist-get m :ignores-mouse))
                (should (= (plist-get m :scale) 1))
                (should (< (abs (- (plist-get m :shadow-opacity) (plist-get m :opacity))) 0.02)))
              ;; the window kept its place while it faded
              (should (equal (glitnir-ns--bounds-seen samples) (list at))))
            ;; at the end the pair is gone and nothing animates
            (should (glitnir-ns--gone-p c))
            (should (eql (plist-get (glitnir--ns-motion c) :animations) 0))
            (should-not (plist-get (glitnir--ns-motion c) :ignores-mouse)))
          ;; shown again (no show leg): opaque at once, in its place
          (make-frame-visible c)
          (redisplay t)
          (let ((samples (glitnir-ns--samples c #'glitnir-ns--at-rest-p)))
            (should-not (glitnir-ns--mid samples))
            (should (equal (glitnir-ns--rest (car (last samples))) glitnir-ns--opaque)))
          (should-not (glitnir-ns--settled-problem c)))
      (delete-frame c t))))

(ert-deftest glitnir-ns/hide-show-one-command-no-motion ()
  ;; handshake: the samples run until no leg is left or waits to start
  (glitnir-ns--skip-unless-ns)
  (let* ((glitnir--reduced-motion 'off)
         (c (glitnir-ns--chromed `((transition . ,glitnir-ns--fade-both)))))
    (unwind-protect
        (progn
          (should (glitnir-ns--at-rest-p (car (last (glitnir-ns--samples c #'glitnir-ns--at-rest-p)))))
          (should-not (glitnir-ns--settled-problem c))
          ;; a hide and a show in one command: nothing moves, the window stays
          (make-frame-invisible c)
          (make-frame-visible c)
          (redisplay t)
          (let ((samples (glitnir-ns--samples c #'glitnir-ns--at-rest-p)))
            (should (glitnir-ns--at-rest-p (car (last samples))))
            (should-not (glitnir-ns--mid samples))
            (dolist (m samples)
              (should (= (plist-get m :opacity) 1))
              (should (plist-get m :bounds)))
            (should (equal (glitnir-ns--rest (car (last samples))) glitnir-ns--opaque)))
          (should (frame-visible-p c))
          (should-not (glitnir-ns--settled-problem c)))
      (delete-frame c t))))

(ert-deftest glitnir-ns/show-during-hide-no-jump ()
  ;; handshake: the samples follow the presentation layers until the state each step waits for
  (glitnir-ns--skip-unless-ns)
  (let* ((glitnir--reduced-motion 'off)
         (c (glitnir-ns--chromed `((transition . ,glitnir-ns--fade-both)))))
    (unwind-protect
        (progn
          (glitnir-ns--samples c #'glitnir-ns--at-rest-p)
          (should-not (glitnir-ns--settled-problem c))
          (make-frame-invisible c)
          (let* ((hide (glitnir-ns--samples c (lambda (m) (< (plist-get m :opacity) 0.7))))
                 (before (plist-get (car (last hide)) :opacity)))
            (should (< before 0.7))
            ;; shown again mid-hide
            (make-frame-visible c)
            (redisplay t)
            (let* ((show (glitnir-ns--samples c #'glitnir-ns--at-rest-p))
                   (legs (cl-remove-if-not (lambda (m) (eq (plist-get m :leg) 'show)) show)))
              ;; it goes on from where the hide was, then up: no jump down or up
              (should legs)
              (should (> (apply #'min (mapcar (lambda (m) (plist-get m :opacity)) show)) (- before 0.15)))
              (should (< (plist-get (car legs) :opacity) (+ before 0.15)))
              (should (glitnir-ns--monotone (mapcar (lambda (m) (plist-get m :opacity)) legs) #'<=))
              ;; for the rest of the way, the share of the leg that is left
              (should (< 0 (plist-get (car legs) :duration) 500))
              ;; the window never left the screen: the hide's end did not order it out
              (should (cl-every (lambda (m) (plist-get m :bounds)) show))
              (should (equal (glitnir-ns--rest (car (last show))) glitnir-ns--opaque))))
          (should (frame-visible-p c))
          (should-not (glitnir-ns--settled-problem c))
          (should-not (plist-get (glitnir--ns-motion c) :ignores-mouse)))
      (delete-frame c t))))

(ert-deftest glitnir-ns/delete-during-fade ()
  ;; handshake: each step waits for the motion state or the window server's model
  (glitnir-ns--skip-unless-ns)
  (let* ((glitnir--reduced-motion 'off)
         (c (glitnir-ns--chromed `((transition . ,glitnir-ns--fade-out)))))
    (unwind-protect
        (progn
          (should-not (glitnir-ns--settled-problem c))
          (make-frame-invisible c)
          (glitnir-ns--samples c (lambda (m) (< (plist-get m :opacity) 0.9)))
          (should (plist-get (glitnir--ns-motion c) :hiding))
          (delete-frame c t)
          ;; the leg's end finds the frame gone: no hide waits, no shadow window is left
          (should (gui-runner-wait (lambda () (eql (plist-get (glitnir--ns-motion) :hides-pending) 0)) 5))
          (should (gui-runner-wait (lambda () (null (glitnir-ns--orphans))) 5))
          (should-not (glitnir-ns--orphans))
          ;; and the next frame's hide works
          (let ((d (glitnir-ns--chromed `((transition . ,glitnir-ns--fade-out)))))
            (unwind-protect
                (progn
                  (should-not (glitnir-ns--settled-problem d))
                  (make-frame-invisible d)
                  (should (gui-runner-wait (lambda () (glitnir-ns--at-rest-p (glitnir--ns-motion d))) 5))
                  (should (glitnir-ns--gone-p d)))
              (delete-frame d t))))
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest glitnir-ns/reduced-motion ()
  ;; handshake: the samples run until no leg is left; the hide waits for the window server's model
  (glitnir-ns--skip-unless-ns)
  ;; nil asks macOS
  (let ((glitnir--reduced-motion nil))
    (should (memq (ns-reduced-motion-p) '(t nil)))
    (should (eq (glitnir--reduced-motion-p) (ns-reduced-motion-p))))
  (let* ((glitnir--reduced-motion 'on)
         (c (glitnir-ns--chromed `((transition . ,glitnir-ns--fade-both)))))
    (unwind-protect
        (progn
          (should (glitnir--reduced-motion-p))
          ;; the show had no motion
          (let ((samples (glitnir-ns--samples c #'glitnir-ns--at-rest-p)))
            (should-not (glitnir-ns--mid samples))
            (should (cl-every (lambda (m) (= (plist-get m :opacity) 1)) samples)))
          (should-not (glitnir-ns--settled-problem c))
          ;; nor the hide: the window goes at once
          (make-frame-invisible c)
          (should-not (plist-get (glitnir--ns-motion c) :hiding))
          (should (glitnir-ns--gone-p c))
          (make-frame-visible c)
          (redisplay t)
          (should-not (glitnir-ns--mid (glitnir-ns--samples c #'glitnir-ns--at-rest-p)))
          (should (equal (glitnir-ns--rest (glitnir--ns-motion c)) glitnir-ns--opaque))
          (should-not (glitnir-ns--settled-problem c)))
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

(defun glitnir-ns--card-px (grab x y)
  "Pixel (X . Y) of GRAB as (B G R)."
  (let ((o (* 4 (+ x (* y (nth 0 grab))))) (b (nth 2 grab)))
    (list (aref b o) (aref b (+ o 1)) (aref b (+ o 2)))))

(defun glitnir-ns--near (px want)
  "Non-nil when every channel of PX is within 2 of WANT's."
  (cl-every (lambda (a b) (<= (abs (- a b)) 2)) px want))

(defun glitnir-ns--gap-px (grab w)
  "The gap's colour in GRAB: the middle of window W's right divider, halfway down."
  (pcase-let ((`(,x ,y ,ww ,h) (glitnir-ns--box w)))
    (glitnir-ns--card-px grab (+ x ww (floor (* (window-right-divider-width w) (frame-scale-factor)) 2))
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
                  (bg (glitnir-ns--card-px grab (+ x (/ w 2)) (+ y (/ h 2))))
                  (r (round (* glitnir-ns--radius (frame-scale-factor)))))
       (should-not (glitnir-ns--near gap bg))
       ;; the window's four outermost corner pixels
       (dolist (c (list (cons x y) (cons (+ x w -1) y) (cons x (+ y h -1)) (cons (+ x w -1) (+ y h -1))))
         (should (equal (list c (glitnir-ns--near (glitnir-ns--card-px grab (car c) (cdr c)) gap)) (list c t))))
       ;; the top edge past the corner square is the window's
       (should (glitnir-ns--near (glitnir-ns--card-px grab (+ x r) y) bg))
       (set-window-parameter left 'corner-radius nil)
       (should (glitnir-ns--near (glitnir-ns--card-px (glitnir-ns--grab) x y) bg))))))

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

;;; Translucent background and backdrop blur.  The root's window shows a pattern of hard edges (lines of `#@') under
;;; the child, whose own buffer is blank; "detail" is the mean |Laplacian| of the luminance over a strip inside the
;;; child, high on the pattern and low on a blur (the spike's measure: blurred below 35 % of the detail before).  The
;;; blur rows bind `glitnir--ns-reduce-transparency' to `off', so that the system's setting on the runner does not
;;; decide them.

(defvar glitnir--ns-reduce-transparency)

(defun glitnir-ns--detail (raw rect)
  "The mean absolute Laplacian of the luminance over RECT (X Y W H, device pixels) of RAW (`glitnir-snapshot' raw)."
  (pcase-let ((`(,x ,y ,w ,h) rect) (`(,rw ,_ ,b) raw) (sum 0.0) (n 0))
    (cl-flet ((luma (i j) (let ((o (* 4 (+ (* j rw) i))))
                            (+ (* 0.0722 (aref b o)) (* 0.7152 (aref b (+ o 1))) (* 0.2126 (aref b (+ o 2)))))))
      (let ((j (1+ y)))
        (while (< j (+ y h -1))
          (let ((i (1+ x)))
            (while (< i (+ x w -1))
              (setq sum (+ sum (abs (- (* 4 (luma i j)) (luma (1- i) j) (luma (1+ i) j)
                                       (luma i (1- j)) (luma i (1+ j)))))
                    n (1+ n)
                    i (1+ i))))
          (setq j (1+ j)))))
    (/ sum (max n 1))))
(byte-compile 'glitnir-ns--detail)

(defun glitnir-ns--blank-buffer ()
  "An empty buffer with no mode line and no cursor."
  (with-current-buffer (get-buffer-create " *glitnir-ns-blank*")
    (erase-buffer)
    (setq mode-line-format nil cursor-type nil)
    (current-buffer)))

(defun glitnir-ns--with-pattern (fn)
  "Call FN with the root's one window showing the pattern; the root's windows come back after."
  (let ((config (current-window-configuration)) (buf (get-buffer-create " *glitnir-ns-pattern*")))
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
          (redisplay t)
          (funcall fn))
      (set-window-configuration config)
      (kill-buffer buf)
      (redisplay t))))

(defun glitnir-ns--strip (rect)
  "A strip inside child RECT (device pixels), away from its corners and edges: 12 points in, 40 points high."
  (let* ((s (frame-scale-factor)) (in (ceiling (* 12 s))))
    (list (+ (nth 0 rect) in) (+ (nth 1 rect) in) (- (nth 2 rect) in in) (min (round (* 40 s)) (- (nth 3 rect) in in)))))

(defun glitnir-ns--cyan-p (px)
  "Non-nil when PX (R G B A) is the child's opaque cyan background."
  (and (< (nth 0 px) 60) (> (nth 1 px) 200) (> (nth 2 px) 200)))

(defun glitnir-ns--blank-child (params)
  "A shown child frame with PARAMS on the blank buffer; it waits (5 s at most) until its middle is its cyan."
  (let* ((c (glitnir-ns--child params)))
    (set-window-buffer (frame-root-window c) (glitnir-ns--blank-buffer))
    (redisplay t)
    (let* ((rect (glitnir-ns--rect c))
           (mid (cons (+ (nth 0 rect) (/ (nth 2 rect) 2)) (+ (nth 1 rect) (/ (nth 3 rect) 2)))))
      (gui-runner-wait (lambda () (glitnir-ns--cyan-p (glitnir-ns--snap-px mid))) 5))
    c))

(ert-deftest glitnir-ns/alpha-background-shows-beneath ()
  ;; handshake: each capture is the window server's settled picture of the step
  (glitnir-ns--skip-unless-ns)
  (glitnir-ns--skip-unless-capture)
  (let* ((buf (glitnir-ns--text-buffer)) (c (glitnir-ns--child '((corner-radius . 8) (shadow . nil)))))
    (unwind-protect
        (let* ((s (frame-scale-factor)) (rect (glitnir-ns--rect c))
               (bg (cons (+ (nth 0 rect) (/ (nth 2 rect) 2)) (+ (nth 1 rect) (nth 3 rect) (round (* -4 s)))))
               (in (ceiling (* 9 s)))
               (inner (list (+ (nth 0 rect) in) (+ (nth 1 rect) in) (- (nth 2 rect) in in) (- (nth 3 rect) in in))))
          ;; one line of large text: its stems hold pixels of the foreground alone
          (with-current-buffer buf
            (narrow-to-region (point-min) (line-end-position))
            (setq truncate-lines t)
            (face-remap-add-relative 'default :height 3.0))
          (set-window-buffer (frame-root-window c) buf)
          (redisplay t)
          (should (gui-runner-wait (lambda () (glitnir-ns--cyan-p (glitnir-ns--snap-px bg))) 5))
          (let ((stock (glitnir-ns--grab)) beneath on)
            (make-frame-invisible c)
            (should (glitnir-ns--gone-p c))
            (setq beneath (glitnir-ns--grab))
            (make-frame-visible c)
            (set-frame-parameter c 'alpha-background 0.5)
            (redisplay t)
            (gui-runner-wait (lambda () (let ((p (glitnir-ns--snap-px bg))) (and p (not (glitnir-ns--cyan-p p))))) 5)
            (setq on (glitnir-ns--grab))
            ;; the background is half the child's colour, half what lies beneath
            (let ((want (cl-mapcar (lambda (a b) (/ (+ a b) 2.0))
                                   (glitnir-ns--px stock (car bg) (cdr bg))
                                   (glitnir-ns--px beneath (car bg) (cdr bg))))
                  (got (glitnir-ns--px on (car bg) (cdr bg))))
              (should (equal (list bg got (cl-every (lambda (g w) (<= (abs (- g w)) 8))
                                                    (cl-subseq got 0 3) (cl-subseq want 0 3)))
                             (list bg got t))))
            ;; the text is opaque: every pixel of the foreground alone is the stock pixel.  That is a stem's inside,
            ;; black with its four neighbours: a black edge pixel can be two partial draws (a glyph's overhang drawn
            ;; again, the cursor's box), which blend with what is beneath.
            (pcase-let ((`(,x ,y ,w ,h) inner) (n 0) (off 0))
              (cl-flet ((black (i j) (equal (cl-subseq (glitnir-ns--px stock i j) 0 3) '(0 0 0))))
                (dotimes (j (- h 2))
                  (dotimes (i (- w 2))
                    (let ((u (+ x i 1)) (v (+ y j 1)))
                      (when (and (black u v) (black (1- u) v) (black (1+ u) v) (black u (1- v)) (black u (1+ v)))
                        (setq n (1+ n))
                        (unless (equal (glitnir-ns--px on u v) (glitnir-ns--px stock u v)) (setq off (1+ off))))))))
              (should (> n 20))
              (should (equal (list :text-pixels n :changed off) (list :text-pixels n :changed 0))))))
      (delete-frame c t)
      (kill-buffer buf))))

(ert-deftest glitnir-ns/blur-behind-child ()
  ;; handshake: each capture is the window server's settled picture of the step
  (glitnir-ns--skip-unless-ns)
  (glitnir-ns--skip-unless-capture)
  (let ((glitnir--ns-reduce-transparency 'off))
    (glitnir-ns--with-pattern
     (lambda ()
       (let ((c (glitnir-ns--blank-child '((corner-radius . 8) (shadow . nil)))))
         (unwind-protect
             (let* ((strip (glitnir-ns--strip (glitnir-ns--rect c))) d0 d1)
               (make-frame-invisible c)
               (should (glitnir-ns--gone-p c))
               (setq d0 (glitnir-ns--detail (glitnir-ns--grab) strip))
               (message "glitnir-ns/blur-behind-child: detail beneath %.2f" d0)
               (should (> d0 5))
               ;; translucent: the pattern shows through, sharp
               (make-frame-visible c)
               (set-frame-parameter c 'alpha-background 0.3)
               (redisplay t)
               (gui-runner-wait (lambda () (> (glitnir-ns--detail (glitnir-snapshot nil 'raw) strip) (* 0.5 d0))) 5)
               (setq d1 (glitnir-ns--detail (glitnir-ns--grab) strip))
               (should (equal (list :translucent d1 (> d1 (* 0.5 d0))) (list :translucent d1 t)))
               ;; and blurred
               (set-frame-parameter c 'backdrop-blur 20)
               (redisplay t)
               (gui-runner-wait (lambda () (< (glitnir-ns--detail (glitnir-snapshot nil 'raw) strip) (* 0.35 d0))) 5)
               (let ((d2 (glitnir-ns--detail (glitnir-ns--grab) strip)))
                 (message "glitnir-ns/blur-behind-child: translucent %.2f, blurred %.2f" d1 d2)
                 (should (equal (list :blurred d2 (< d2 (* 0.35 d0))) (list :blurred d2 t)))))
           (delete-frame c t)))))))

(ert-deftest glitnir-ns/blur-off-identical ()
  ;; guard: a blur without a translucent background, or with Glitnir off, is the stock frame
  (glitnir-ns--skip-unless-ns)
  (glitnir-ns--skip-unless-capture)
  (let ((glitnir--ns-reduce-transparency 'off))
    (glitnir-ns--with-pattern
     (lambda ()
       (let ((c (glitnir-ns--blank-child '((corner-radius . 8) (shadow . nil) (backdrop-blur . 20)))))
         (unwind-protect
             (let ((rect (glitnir-ns--rect c)) with without off stock)
               ;; opaque: `backdrop-blur' makes no difference
               (setq with (glitnir-ns--grab))
               (set-frame-parameter c 'backdrop-blur nil)
               (setq without (glitnir-ns--grab))
               (should (equal (glitnir-ns--diff with without rect) '(0 0)))
               ;; Glitnir off: translucent and blurred is the stock (opaque, square) frame
               (modify-frame-parameters c '((backdrop-blur . 20) (alpha-background . 0.3)))
               (let ((glitnir-enable nil))
                 (glitnir-refresh c)
                 (setq off (glitnir-ns--grab)))
               (modify-frame-parameters c '((corner-radius . 0) (backdrop-blur . nil)))
               (setq stock (glitnir-ns--grab))
               (should (equal (glitnir-ns--diff off stock rect) '(0 0)))
               (should (glitnir-ns--cyan-p (glitnir-ns--px stock (+ (nth 0 rect) (/ (nth 2 rect) 2))
                                                           (+ (nth 1 rect) (/ (nth 3 rect) 2))))))
           (delete-frame c t)))))))

(ert-deftest glitnir-ns/blur-follows-reduce-transparency ()
  ;; handshake: each capture is the window server's settled picture of the step
  (glitnir-ns--skip-unless-ns)
  (glitnir-ns--skip-unless-capture)
  (glitnir-ns--with-pattern
   (lambda ()
     (let ((c (glitnir-ns--blank-child '((corner-radius . 8) (shadow . nil)))))
       (unwind-protect
           (let* ((strip (glitnir-ns--strip (glitnir-ns--rect c))) (opaque (glitnir-ns--grab))
                  (mid (cons (+ (nth 0 strip) (/ (nth 2 strip) 2)) (+ (nth 1 strip) (/ (nth 3 strip) 2)))))
             ;; reduce transparency on: the blurred, translucent frame is opaque, its pixels the opaque frame's
             (let ((glitnir--ns-reduce-transparency 'on))
               (modify-frame-parameters c '((alpha-background . 0.3) (backdrop-blur . 20)))
               (should (equal (glitnir-ns--diff opaque (glitnir-ns--grab) strip) '(0 0))))
             ;; off: the same frame shows what lies beneath, blurred
             (let ((glitnir--ns-reduce-transparency 'off))
               (glitnir-refresh c)
               (redisplay t)
               (gui-runner-wait (lambda () (not (glitnir-ns--cyan-p (glitnir-ns--snap-px mid)))) 5)
               (should-not (glitnir-ns--cyan-p (glitnir-ns--px (glitnir-ns--grab) (car mid) (cdr mid))))))
         (delete-frame c t))))))

;;; glitnir-ns-gui-tests.el ends here
