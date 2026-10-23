;;; glitnir-gui-tests.el --- GUI tests for Glitnir (the compositor)  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh on a headless mutter (test/src/nidhogg-tests/gui-runner.el
;; loads every *-gui-tests.el).  GLITNIR_GUI_SCALE is the monitor scale the run asked for.

(require 'ert)
(require 'cl-lib)

(ert-deftest glitnir-gui/a-pgtk-frame-at-the-asked-scale ()
  (let ((s (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1"))))
    (should (eq (framep (selected-frame)) 'pgtk))
    (should (= (frame-scale-factor) (ceiling s)))))

;; Stats and snapshot of the stock present path (Task 5).

(ert-deftest glitnir-gui/stats-record-the-stock-paint ()
  (let ((glitnir-enable nil))                   ; the stock path (Glitnir paints only the damage)
    (redisplay t)
    (glitnir-stats nil t)
    (with-temp-buffer
      (switch-to-buffer (current-buffer))
      (dotimes (_ 5) (insert "x") (redisplay t) (sit-for 0.05))
      (let ((s (glitnir-stats)))
        (should (>= (plist-get s :count) 5))
        (should (= (plist-get s :painted-px)    ; stock paints the whole widget
                   (round (* (frame-pixel-width) (frame-pixel-height)
                             (expt (frame-scale-factor) 2)))))))))

(ert-deftest glitnir-gui/snapshot-has-the-frame-size ()
  (pcase-let ((`(,w ,h ,bytes) (glitnir-snapshot nil 'raw)))
    (should (= w (round (* (frame-pixel-width) (frame-scale-factor)))))
    (should (= h (round (* (frame-pixel-height) (frame-scale-factor)))))
    (should (= (length bytes) (* 4 w h)))
    (should-not (multibyte-string-p bytes))))

(ert-deftest glitnir-gui/snapshot-png-is-a-png ()
  (let ((png (glitnir-snapshot nil)))
    (should-not (multibyte-string-p png))
    (should (string-prefix-p "\x89PNG" png))))

(ert-deftest glitnir-gui/snapshot-unarmed-costs-nothing ()
  ;; Calls into Glitnir are made for damage now; `glitnir-gui/inactive-makes-no-calls' checks the no-call path.
  (let ((copies (glitnir--snapshot-copies)))
    (with-temp-buffer
      (switch-to-buffer (current-buffer))
      (dotimes (_ 20) (insert "y") (redisplay t) (sit-for 0.02)))
    (should (= copies (glitnir--snapshot-copies)))))

(ert-deftest glitnir-gui/daemon-first-frame-initialises ()
  (skip-unless (daemonp))
  (should (>= (plist-get (glitnir-stats) :count) 1)))

;; Damage recorded in pgtk's drawing primitives (Task 20).  Each test first takes what is pending, so it sees only
;; what its own steps drew.

(defun glitnir-gui--frame-rect ()
  "The selected frame as one device-pixel rectangle (X Y W H)."
  (list 0 0 (round (* (frame-pixel-width) (frame-scale-factor)))
        (round (* (frame-pixel-height) (frame-scale-factor)))))

(ert-deftest glitnir-gui/typing-damages-one-row ()
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (insert "hello")
    (let ((glitnir-debug 'damage-hold))
      (redisplay t) (glitnir--damage)
      (insert "x") (redisplay t)
      (let ((rows (glitnir--damage)) (lh (* (line-pixel-height) (frame-scale-factor))))
        (should rows)
        (should (cl-every (lambda (r) (<= (nth 3 r) (* 2 lh))) rows))))))

(ert-deftest glitnir-gui/garbaged-frame-damages-all ()
  (let ((glitnir-debug 'damage-hold))
    (redisplay t) (glitnir--damage)
    (redraw-frame) (redisplay t)
    (should (equal (glitnir--damage) (list (glitnir-gui--frame-rect))))))

(ert-deftest glitnir-gui/scroll-records-a-scroll-op ()
  ;; C-v moves the rows it keeps with pgtk_scroll_run: one rectangle spans the window's text area (the scroll's area
  ;; also holds the fringes; the last, partly visible row may be redrawn on its own), and the rows the scroll
  ;; covers add nothing, so the damage is a handful of rectangles rather than one per row.
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (dotimes (i 500) (insert (format "line %d\n" i)))
    (goto-char (point-min))
    (let ((glitnir-debug 'damage-hold) (s (frame-scale-factor)) (lh (line-pixel-height)))
      (redisplay t) (glitnir--damage)
      (scroll-up-command) (redisplay t)
      (pcase-let ((`(,l ,top ,r ,bottom) (window-body-pixel-edges))
                  (rects (glitnir--damage)))
        (should (cl-some (lambda (x)
                           (and (<= (nth 0 x) (* l s)) (>= (+ (nth 0 x) (nth 2 x)) (* r s))
                                (<= (nth 1 x) (* top s)) (>= (+ (nth 1 x) (nth 3 x)) (* (- bottom (* 2 lh)) s))))
                         rects))
        (should (<= (length rects) 8))))))

(ert-deftest glitnir-gui/visible-bell-damages-all ()
  (let ((glitnir-debug 'damage-hold) (visible-bell t) (ring-bell-function nil))
    (redisplay t) (glitnir--damage)
    (ding t)
    (should (equal (glitnir--damage) (list (glitnir-gui--frame-rect))))
    (sleep-for 0.2)                     ; the bell's atimer (50 ms) takes the flash back
    (should (equal (glitnir--damage) (list (glitnir-gui--frame-rect))))))

(ert-deftest glitnir-gui/export-records-nothing ()
  ;; x-export-frames first redisplays the frame for real (that may redraw the cursors), then clears and draws the
  ;; whole frame into its own surface: recorded, that would be the whole frame.
  (let ((glitnir-debug 'damage-hold))
    (redisplay t) (glitnir--damage)
    (should (stringp (x-export-frames nil 'png)))
    (let ((rects (glitnir--damage)) (frame (glitnir-gui--frame-rect)))
      (should-not (member frame rects))
      (should (< (apply #'+ (mapcar (lambda (r) (* (nth 2 r) (nth 3 r))) rects))
                 (/ (* (nth 2 frame) (nth 3 frame)) 100))))))

(ert-deftest glitnir-gui/inactive-makes-no-calls ()
  (should (glitnir-active-p))
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (redisplay t)
    (let ((glitnir-enable nil))
      (should-not (glitnir-active-p))
      (let ((calls (glitnir--calls)))
        (dotimes (_ 20) (insert "z") (redisplay t))
        (should (= calls (glitnir--calls)))))))

;; Consistency points, the root freeze and damage-only present (Task 21).

(defconst glitnir-gui--magenta '(:background "#ff00ff" :foreground "#ff00ff"))

(defun glitnir-gui--busy (s)
  (let ((end (+ (float-time) s)))
    (while (< (float-time) end))))

(defun glitnir-gui--settle ()
  "Wait (5 s at most) for a 0.1 s span in which the selected frame takes no whole-frame damage; non-nil if it came.
Right after the session starts the frame is still redrawn whole for a while, and Emacs defers a mouse highlight
until that redisplay is done."
  (let ((glitnir-debug 'damage-hold) (n 0))
    (redisplay t) (glitnir--damage)
    (while (and (< n 50) (progn (sit-for 0.1) (member (glitnir-gui--frame-rect) (glitnir--damage))))
      (cl-incf n))
    (< n 50)))

(defun glitnir-gui--frame-px ()
  (round (* (frame-pixel-width) (frame-pixel-height) (expt (frame-scale-factor) 2))))

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

(defvar glitnir-gui--jit-hook nil "A function the jit-lock function calls after `input-pending-p'.")

(defun glitnir-gui--jit (beg end)
  "A jit-lock function that takes 2 ms a chunk and calls `input-pending-p', as a slow mode would."
  (glitnir-gui--busy 0.002)
  (input-pending-p)
  (when glitnir-gui--jit-hook (funcall glitnir-gui--jit-hook))
  (put-text-property beg end 'face (if (cl-evenp (/ beg 1000)) 'bold 'italic))
  `(jit-lock-bounds ,beg . ,end))

(defun glitnir-gui--text-buffer (name lines)
  (let ((b (get-buffer-create name)))
    (with-current-buffer b
      (erase-buffer)
      (dotimes (i lines)
        (insert (format "%5d %s\n" i (make-string 70 (+ ?a (% i 26))))))
      (goto-char (point-min)))
    b))

(defun glitnir-gui--jit-buffer (name)
  (let ((b (glitnir-gui--text-buffer name 2000)))
    (with-current-buffer b
      (setq-local jit-lock-chunk-size 400)
      (jit-lock-register #'glitnir-gui--jit))
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

(defun glitnir-gui--scroll (buffers steps lines)
  "STEPS back-to-back redisplays; each moves every window of BUFFERS LINES further, all text unfontified."
  (dotimes (i steps)
    (dolist (b buffers)
      (with-current-buffer b
        (with-silent-modifications
          (put-text-property (point-min) (point-max) 'fontified nil)))
      (let ((start (with-current-buffer b
                     (save-excursion (goto-char (point-min)) (forward-line (* (% i 90) lines)) (point)))))
        (dolist (w (get-buffer-window-list b nil t))
          (set-window-start w start))))
    (redisplay t)))

(ert-deftest glitnir-gui/typing-paints-only-damage ()
  (should (glitnir-active-p))
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (insert "hello")
    (redisplay t) (sit-for 0.1)
    (glitnir-stats nil t)
    (dotimes (_ 20) (insert "x") (redisplay t) (sit-for 0.02))
    (let ((s (glitnir-stats)))
      (should (>= (plist-get s :count) 5))
      (should (< (plist-get s :painted-px) (/ (glitnir-gui--frame-px) 10))))))

(ert-deftest glitnir-gui/stats-sum-a-frames-draws ()
  ;; A point that paints at once draws outside the frame clock; several such draws can come before the clock's
  ;; after-paint records the frame.  Its entry sums them all (one draw's numbers would hide the others).
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (insert "hello")
    (redisplay t) (sit-for 0.1)
    (glitnir-stats nil t)
    (let ((draws (glitnir--draw-count)))
      ;; two whole-widget draws with no GLib dispatch between them: one frame
      (dotimes (_ 2) (should (glitnir--compose-with-pending)))
      (sit-for 0.1)
      (let ((s (glitnir-stats)))
        (should (>= (- (glitnir--draw-count) draws) 2))
        (should (= (plist-get s :count) 1))
        (should (>= (plist-get s :painted-px) (* 2 (glitnir-gui--frame-px))))))))

(defun glitnir-gui--new-buffers (enable)
  "GDK's new buffers over 20 tab-like switches with `glitnir-enable' ENABLE: each switch redraws the window, lets GLib
dispatch (GDK commits that paint), then types and redraws again, as a command and the timer after it do."
  (let ((a (glitnir-gui--text-buffer "*glitnir-tab-a*" 200))
        (b (glitnir-gui--text-buffer "*glitnir-tab-b*" 200))
        (glitnir-enable enable))
    (unwind-protect
        (progn
          (switch-to-buffer a)
          (redisplay t) (sit-for 0.1)
          (let ((before (plist-get (glitnir--paint-counters) :new-buffers)))
            (dotimes (i 20)
              (switch-to-buffer (if (cl-evenp i) b a))
              (redisplay t)
              (accept-process-output nil 0.002)
              (insert "x")
              (redisplay t)
              (sit-for 0.03))
            (- (plist-get (glitnir--paint-counters) :new-buffers) before)))
      (kill-buffer a) (kill-buffer b))))

(ert-deftest glitnir-gui/points-reuse-the-buffer ()
  ;; A paint while the compositor still holds the last committed buffer makes GDK allocate a new one and copy every
  ;; pixel the paint left alone into it (5 MP at 2880x1800 scale 2).  The points must paint no more often into a new
  ;; buffer than the stock present does.
  (let ((off (glitnir-gui--new-buffers nil)) (on (glitnir-gui--new-buffers t)))
    (princ (format "new buffers: on %d, off %d\n" on off) #'external-debugging-output)
    (should (<= on (+ off 2)))))

(ert-deftest glitnir-gui/child-churn-paints-the-child-only ()
  ;; posframe's cycle (popup-churn): show a child, move it, hide it; the paints should cover the child's rectangles,
  ;; not the root.  Known failure GL-POPUP-CHURN-FULL-1: a child frame is a GtkWidget in its parent's EmacsFixed, so
  ;; its show/hide restyles the parent's CSS node and GTK's style-updated handlers (gtk_widget_real_style_updated,
  ;; gtk_fixed_style_updated) invalidate the whole root in the next layout phase.  MP-3's widget-less child frames
  ;; remove it.
  :expected-result :failed
  (let* ((cb (glitnir-gui--text-buffer "*glitnir-churn*" 5))
         (c (glitnir-gui--child cb nil))
         (buf (generate-new-buffer "*glitnir-churn-root*")))
    (unwind-protect
        (progn
          (switch-to-buffer buf)
          (insert "root")
          (redisplay t) (sit-for 0.1)
          (glitnir-stats nil t)
          (dotimes (i 20)
            (set-frame-position c (+ 200 (* 4 (% i 10))) 200)
            (make-frame-visible c)
            (set-frame-position c (+ 204 (* 4 (% i 10))) 200)
            (redisplay t)
            (make-frame-invisible c)
            (redisplay t)
            (sit-for 0.03))
          (let ((s (glitnir-stats)))
            (princ (format "child churn: painted px p50 %d of %d\n" (plist-get s :painted-px) (glitnir-gui--frame-px))
                   #'external-debugging-output)
            (should (>= (plist-get s :count) 10))
            (should (< (plist-get s :painted-px) (/ (glitnir-gui--frame-px) 4)))))
      (delete-frame c)
      (kill-buffer cb) (kill-buffer buf))))

(ert-deftest glitnir-gui/no-draw-while-frozen ()
  ;; The child frame is newer, so redisplay draws it first: its damage freezes the root before the root's jit-lock
  ;; runs.  The jit-lock function then moves the child (GTK invalidates the root on its next layout) and lets GLib
  ;; dispatch for 30 ms through `input-pending-p': the root must not paint until the consistency point.
  (let* ((a (glitnir-gui--jit-buffer "*glitnir-a*"))
         (b (glitnir-gui--text-buffer "*glitnir-b*" 30))
         (c (glitnir-gui--child b t))
         (deltas nil) (n 0))
    (unwind-protect
        (progn
          (switch-to-buffer a)
          (redisplay t) (sit-for 0.1)
          (setq glitnir-gui--jit-hook
                (lambda ()
                  (when (and (< n 10) (glitnir--frozen-p))
                    (let ((before (glitnir--draw-count)))
                      (cl-incf n)
                      (set-frame-position c (+ 100 (* 10 (% n 20))) 150)
                      (dotimes (_ 3) (glitnir-gui--busy 0.01) (input-pending-p))
                      (push (- (glitnir--draw-count) before) deltas)))))
          (dotimes (_ 20)
            (with-current-buffer b (goto-char (point-min)) (insert "x"))
            (glitnir-gui--scroll (list a) 1 20))
          (setq glitnir-gui--jit-hook nil)
          (let ((draws (glitnir--draw-count)))
            (redisplay t) (sit-for 0.1)
            (should (> (glitnir--draw-count) draws))))
      (setq glitnir-gui--jit-hook nil)
      (delete-frame c)
      (kill-buffer a) (kill-buffer b))
    (should (>= (length deltas) 5))
    (should (cl-every #'zerop deltas))
    (should-not (glitnir--frozen-p))))

(ert-deftest glitnir-gui/error-in-redisplay-thaws ()
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
          (redisplay t)
          (should-not (glitnir--frozen-p))
          (should (> (glitnir-gui--samples 255 0 255) 0)))
      (kill-buffer buf))))

(ert-deftest glitnir-gui/nested-map-wait-consistent ()
  (let* ((cb (glitnir-gui--text-buffer "*glitnir-c*" 10))
         (c (glitnir-gui--child cb nil))
         (buf (generate-new-buffer "*glitnir-map*")))
    (unwind-protect
        (progn
          (switch-to-buffer buf)
          (dotimes (_ 5)
            (make-frame-invisible c)
            (insert "a") (redisplay t)
            (make-frame-visible c)      ; the map wait's nested loop, inside this command
            (insert (propertize "MM" 'face glitnir-gui--magenta))
            (redisplay t))
          ;; the child, already shown, changes: its damage reaches the root's commit
          (with-current-buffer cb
            (goto-char (point-min))
            (insert (propertize "YYYYYYYY" 'face '(:background "#ffff00" :foreground "#ffff00"))))
          (redisplay t)
          (should-not (glitnir--frozen-p))
          (should (> (glitnir-gui--samples 255 0 255) 0))  ; the typed text
          (should (> (glitnir-gui--samples 0 255 255) 0))  ; the child
          (should (> (glitnir-gui--samples 255 255 0) 0))) ; the child's new text
      (delete-frame c)
      (kill-buffer cb) (kill-buffer buf))))

(ert-deftest glitnir-gui/enable-nil-is-stock ()
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (redisplay t) (sit-for 0.05)
    (let ((glitnir-enable nil))
      (should-not (glitnir-active-p))
      (redisplay t) (sit-for 0.1)
      (glitnir-stats nil t)
      (let ((calls (glitnir--calls)) (draws (glitnir--draw-count)))
        (dotimes (_ 20)
          (insert "z") (redisplay t)
          (should-not (glitnir--frozen-p))
          (sit-for 0.02))
        (should (= calls (glitnir--calls)))
        (should (> (glitnir--draw-count) draws))
        (should (= (plist-get (glitnir-stats) :painted-px) (glitnir-gui--frame-px)))))))

(ert-deftest glitnir-gui/mouse-highlight-commits ()
  (let ((buf (generate-new-buffer "*glitnir-mouse*")))
    (unwind-protect
        (progn
          (switch-to-buffer buf)
          (insert "\n\n  " (propertize "HOVERHOVERHOVERHOVER" 'mouse-face glitnir-gui--magenta) "\n")
          (redisplay t) (sit-for 0.1)
          (should (glitnir-gui--settle))  ; first in a session, the motions would come while the frame settles
          (should (= (glitnir-gui--samples 255 0 255) 0))
          (pcase-let ((`(,x . ,y) (posn-x-y (posn-at-point (+ (point-min) 6))))
                      (`(,left ,top . ,_) (window-inside-pixel-edges)))
            ;; `glitnir--motion' hands the motion straight to GTK's handler (no GDK queue, which holds the newest
            ;; motion back until another event follows it, and no wait): a motion elsewhere, then one over the
            ;; text, no redisplay between.  The handler draws the highlight; the stock path would show it only at
            ;; the next redisplay.
            (dolist (xy (list (cons (+ left 2) (+ top 2)) (cons (+ left x 4) (+ top y 4))))
              (should (glitnir--motion nil (car xy) (cdr xy)))))
          (should (> (glitnir-gui--samples 255 0 255) 0)))
      (kill-buffer buf))))

(ert-deftest glitnir-gui/no-starvation-while-redisplaying ()
  ;; R7's scenario S3-1: GLib dispatch happens inside redisplay (jit-lock's `input-pending-p') and finds the root
  ;; frozen; each consistency point paints, so the display keeps up.
  (let* ((a (glitnir-gui--jit-buffer "*glitnir-a*"))
         (b (glitnir-gui--jit-buffer "*glitnir-b*"))
         (c (glitnir-gui--child b t)))
    (unwind-protect
        (progn
          (switch-to-buffer a)
          (redisplay t) (sit-for 0.1)
          (glitnir--max-paint-gap-ms nil t)
          (glitnir-gui--scroll (list a b) 100 20)
          (let ((gap (glitnir--max-paint-gap-ms)))
            (princ (format "longest paint gap: %.1f ms\n" gap) #'external-debugging-output)
            (should (< gap 500))))
      (delete-frame c)
      (kill-buffer a) (kill-buffer b))))

(ert-deftest glitnir-gui/latency-recorded-on-and-off ()
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (dolist (on '(t nil))
      (let ((glitnir-enable on))
        (redisplay t) (sit-for 0.1)
        (glitnir-stats nil t)
        (dotimes (_ 20) (insert "l") (redisplay t) (sit-for 0.03))
        (let ((s (glitnir-stats)) (last (glitnir--last-paint)))
          (should (> (plist-get s :count) 0))
          (should (> (plist-get (plist-get s :latency-ms) :p50) 0))
          (should (null (plist-get s :backfill-rate)))
          (should (<= (plist-get last :commit-ns) (plist-get last :paint-ns) (plist-get last :present-ns)
                      (glitnir--now-ns))))))))

;; Debug modes (Task 22).

(defun glitnir-gui--violations ()
  (plist-get (glitnir-stats) :violations))

(ert-deftest glitnir-gui/verify-catches-a-missed-rect ()
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (should (glitnir-gui--settle))
    (let ((glitnir-debug 'verify))
      (insert "a") (redisplay t) (sit-for 0.1) ; this commit keeps the copy the next one is compared with
      (glitnir-stats nil t)
      (should (glitnir--draw-undamaged))
      (insert "b") (redisplay t) (sit-for 0.1)
      (should (>= (glitnir-gui--violations) 1))
      ;; the rectangle is in the copy now: the next commit is clean
      (glitnir-stats nil t)
      (insert "c") (redisplay t) (sit-for 0.1)
      (should (= (glitnir-gui--violations) 0)))
    (redraw-frame)))

(ert-deftest glitnir-gui/verify-quiet-on-typing ()
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (should (glitnir-gui--settle))
    (dotimes (i 3000) (insert (format "line %d\n" i)))
    (goto-char (point-min))
    (let ((glitnir-debug 'verify))
      (redisplay t) (sit-for 0.2)
      (glitnir-stats nil t)
      (dotimes (_ 200) (insert "k") (redisplay t))
      (dotimes (_ 5) (scroll-up-command) (redisplay t))
      (dotimes (_ 5) (scroll-down-command) (redisplay t))
      (let ((w (frame-pixel-width)) (h (frame-pixel-height)))
        (set-frame-size nil (+ w 40) h t) (redisplay t) (sit-for 0.3)
        (set-frame-size nil w h t) (redisplay t) (sit-for 0.3))
      (dotimes (_ 20) (insert "k") (redisplay t))
      (sit-for 0.3)
      (should (= (glitnir-gui--violations) 0)))))

(ert-deftest glitnir-gui/verify-catches-compose-with-pending ()
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (should (glitnir-gui--settle))
    (let ((glitnir-debug 'verify))
      (redisplay t) (sit-for 0.2)
      (glitnir-stats nil t)
      (should (glitnir--compose-with-pending))
      (should (= (glitnir-gui--violations) 1))
      (should-not (glitnir--frozen-p))
      ;; an ordinary commit afterwards is clean
      (glitnir-stats nil t)
      (insert "x") (redisplay t) (sit-for 0.1)
      (should (= (glitnir-gui--violations) 0)))))

(ert-deftest glitnir-gui/full-paints-everything ()
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (should (glitnir-gui--settle))
    (let ((glitnir-debug 'full))
      (redisplay t) (sit-for 0.2)
      (glitnir-stats nil t)
      (dotimes (_ 10) (insert "f") (redisplay t) (sit-for 0.05))
      (let ((s (glitnir-stats)))
        (should (>= (plist-get s :count) 5))
        (should (= (plist-get s :painted-px) (glitnir-gui--frame-px)))))))

;; The panic fallback (Task 23).

(defun glitnir-gui--panics (where)
  "Make the next call at WHERE panic, and check what the fallback leaves.
Glitnir is turned back on after (`revive'), for the tests that follow."
  (with-temp-buffer
    (switch-to-buffer (current-buffer))
    (should (glitnir-gui--settle))
    (let ((glitnir-debug 'verify) (kept nil))
      (unwind-protect
          (progn
            (insert (propertize "YYYYYYYY" 'face '(:background "#ffff00" :foreground "#ffff00")))
            (redisplay t) (sit-for 0.1)
            (should (glitnir-active-p))
            (glitnir--debug-panic where)
            (insert (propertize "MM" 'face glitnir-gui--magenta))
            (redisplay t) (sit-for 0.3)
            (should-not (glitnir-active-p))
            (should-not (glitnir-stats))
            (should-not (glitnir--frozen-p))
            (let ((calls (glitnir--calls)) (draws (glitnir--draw-count)))
              ;; a typed key shows, in a whole-frame paint, with the rest of the picture
              (insert (propertize "KKKK" 'face '(:background "#00ff00" :foreground "#00ff00")))
              (redisplay t) (sit-for 0.3)
              (redraw-frame) (redisplay t) (sit-for 0.3)
              (should (> (glitnir--draw-count) draws))
              (should-not (glitnir--frozen-p))
              (should (> (glitnir-gui--samples 0 255 0) 0))
              (should (> (glitnir-gui--samples 255 0 255) 0))
              (should (> (glitnir-gui--samples 255 255 0) 0))
              (pcase-let ((`(,w ,h ,_) (glitnir-snapshot nil 'raw)))
                (should (= (* w h) (glitnir-gui--frame-px))))
              (should-not (glitnir-active-p))
              (should-not (glitnir-stats))
              (should (= calls (glitnir--calls))))
            (setq kept t))
        (glitnir--debug-panic 'revive)
        (redraw-frame) (redisplay t) (sit-for 0.2))
      (should kept)
      (should (glitnir-active-p)))))

(ert-deftest glitnir-gui/panic-in-commit-falls-back ()
  (glitnir-gui--panics 'commit))

(ert-deftest glitnir-gui/panic-in-draw-paints-stock ()
  (glitnir-gui--panics 'draw))

;; Layers, the stacking list and scene commits from pgtk's hooks (Task 32).  Each test runs under `verify', which
;; also checks at every commit that Glitnir stacks the child frames as GDK stacks their windows.

(defmacro glitnir-gui--verified (&rest body)
  "Run BODY with `glitnir-debug' `verify'; no violation may be counted meanwhile."
  (declare (indent 0))
  `(let ((glitnir-debug 'verify))
     (redisplay t) (sit-for 0.1)
     (glitnir-stats nil t)
     ,@body
     (redisplay t) (sit-for 0.1)
     (should (= (glitnir-gui--violations) 0))))

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
  (let ((fs (glitnir-gui--children 3)))
    (unwind-protect
        (glitnir-gui--verified
          (pcase-let ((`(,a ,b ,c) fs))
            (redisplay t)
            (should (cl-every #'frame-visible-p fs))
            (should (equal (glitnir--stacking) (list a b c)))
            (raise-frame a)
            (should (equal (glitnir--stacking) (list b c a)))
            (lower-frame c)
            (should (equal (glitnir--stacking) (list c b a)))
            (raise-frame b) (redisplay t)
            (should (equal (glitnir--stacking) (list c a b)))))
      (glitnir-gui--delete fs))))

(ert-deftest glitnir-gui/map-raises-the-node ()
  (let ((fs (glitnir-gui--children 2)))
    (unwind-protect
        (glitnir-gui--verified
          (pcase-let ((`(,a ,b) fs))
            (redisplay t)
            (should (equal (glitnir--stacking) (list a b)))
            (make-frame-invisible a) (redisplay t)
            (should (equal (glitnir--stacking) (list a b))) ; hidden nodes keep their place
            (make-frame-visible a) (redisplay t)
            (should (equal (glitnir--stacking) (list b a)))))
      (glitnir-gui--delete fs))))

(ert-deftest glitnir-gui/zgroup-changes-nothing-for-child-frames ()
  ;; pgtk child frames have no z-group (pgtk_set_z_group needs an outer widget).
  (let ((fs (glitnir-gui--children 2)))
    (unwind-protect
        (glitnir-gui--verified
          (pcase-let ((`(,a ,b) fs))
            (redisplay t)
            (set-frame-parameter a 'z-group 'above) (redisplay t)
            (should (equal (glitnir--stacking) (list a b)))
            (set-frame-parameter b 'z-group 'below) (redisplay t)
            (should (equal (glitnir--stacking) (list a b)))))
      (glitnir-gui--delete fs))))

(ert-deftest glitnir-gui/reparent-moves-the-node ()
  (let ((fs (glitnir-gui--children 3)))
    (unwind-protect
        (glitnir-gui--verified
          (pcase-let ((`(,a ,b ,c) fs))
            (redisplay t)
            ;; a goes on top of c's child frames, inside c
            (set-frame-parameter a 'parent-frame c) (redisplay t)
            (should (eq (frame-parent a) c))
            (should (equal (glitnir--stacking) (list b c a)))
            ;; b goes into a, inside c
            (set-frame-parameter b 'parent-frame a) (redisplay t)
            (should (equal (glitnir--stacking) (list c a b)))))
      (glitnir-gui--delete fs))))

(ert-deftest glitnir-gui/unparent-makes-a-root ()
  (let* ((root (selected-frame)) (fs (glitnir-gui--children 2)) (d nil))
    (unwind-protect
        (glitnir-gui--verified
          (pcase-let ((`(,a ,b) fs))
            (redisplay t)
            (set-frame-parameter a 'parent-frame nil)
            (select-frame root)
            (redisplay t) (sit-for 0.3)
            (should-not (frame-parent a))
            (should (equal (glitnir--stacking) (list b)))
            (should (glitnir-active-p a))
            (should (null (glitnir--stacking a)))
            ;; a root of its own: its child frames stack in its scene
            (setq d (glitnir-gui--nested a))
            (redisplay t)
            (should (equal (glitnir--stacking a) (list d)))
            (should (equal (glitnir--stacking) (list b)))))
      (select-frame root)
      (glitnir-gui--delete (append fs (list d))))))

(ert-deftest glitnir-gui/nested-child-stacks-in-its-parent ()
  (let* ((fs (glitnir-gui--children 2)) (c nil))
    (unwind-protect
        (glitnir-gui--verified
          (pcase-let ((`(,a ,b) fs))
            (setq c (glitnir-gui--nested a))
            (redisplay t)
            (should (equal (glitnir--stacking) (list a c b)))
            (raise-frame c) (redisplay t)
            (should (equal (glitnir--stacking) (list a c b))) ; inside a, below b
            (raise-frame a) (redisplay t)
            (should (equal (glitnir--stacking) (list b a c)))))
      (glitnir-gui--delete (append fs (list c))))))

(ert-deftest glitnir-gui/delete-child-while-frozen ()
  ;; Review Focus 1: a timer deletes a shown child (then, in the second round, the parent of a shown child) between
  ;; the root's freeze and its next consistency point.  The commit drops the node (and the nodes under it), nothing
  ;; reads their pixels (no `verify' violation), and the root thaws at the next point, which outside redisplay is
  ;; GLib's next idle run.  The freeze is the first damage's, made by `glitnir--debug-freeze' in the same timer, so
  ;; GLib runs nothing between it and the deletion.
  (let* ((fs (glitnir-gui--children 2)) (c nil) (frozen nil))
    (cl-flet ((delete-frozen (f)
                (setq frozen 'unset)
                (run-at-time 0 nil (lambda ()
                                     (glitnir--debug-freeze)
                                     (setq frozen (glitnir--frozen-p))
                                     (delete-frame f t)))
                (accept-process-output nil 0.05)
                (should (eq frozen t))
                (should-not (frame-live-p f))
                (should-not (glitnir--frozen-p))
                (accept-process-output nil 0.2)))
      (unwind-protect
          (glitnir-gui--verified
            (pcase-let ((`(,a ,b) fs))
              (redisplay t) (sit-for 0.1)
              (delete-frozen a)
              (should (equal (glitnir--stacking) (list b)))
              ;; the parent of a shown nested child
              (setq c (glitnir-gui--nested b))
              (redisplay t) (sit-for 0.1)
              (should (equal (glitnir--stacking) (list b c)))
              (delete-frozen b)
              (should-not (frame-live-p c))
              (should (null (glitnir--stacking)))
              (should (glitnir-active-p))))
        (glitnir-gui--delete (append fs (list c)))))))

(ert-deftest glitnir-gui/resized-child-keeps-its-last-complete-picture ()
  (let ((fs (glitnir-gui--children 1)))
    (unwind-protect
        (glitnir-gui--verified
          (let* ((c (car fs)) (s (frame-scale-factor))
                 (size (lambda () (list (round (* (frame-pixel-width c) s)) (round (* (frame-pixel-height c) s)))))
                 (old (progn (redisplay t) (sit-for 0.1) (funcall size))))
            (should (equal (cddr (glitnir--layer-rect c)) old))
            ;; GTK allocates the new size at its next layout (GLib runs, redisplay does not): until the child is
            ;; redrawn, its node keeps the size of the picture it has
            (set-frame-size c 70 20)
            (accept-process-output nil 0.2)
            (glitnir--stacking)
            (should (equal (cddr (glitnir--layer-rect c)) old))
            (redisplay t) (sit-for 0.1)
            (should-not (equal (funcall size) old))
            (should (equal (cddr (glitnir--layer-rect c)) (funcall size)))))
      (glitnir-gui--delete fs))))

(ert-deftest glitnir-gui/scene-hook-commits-outside-redisplay ()
  ;; A timer moves a shown child and nothing is redrawn: the move hook commits the scene at once, so the next paint
  ;; shows a commit made after the move (with no Emacs damage, no point would have committed).
  (let ((fs (glitnir-gui--children 1)))
    (unwind-protect
        (glitnir-gui--verified
          (let ((c (car fs)) (s (frame-scale-factor)))
            (redisplay t) (sit-for 0.2)
            (let ((t0 (glitnir--now-ns)) (n0 (plist-get (glitnir-stats) :count)))
              (run-at-time 0 nil (lambda () (set-frame-position c 260 230)))
              (accept-process-output nil 0.3)
              (should-not (glitnir--frozen-p))
              (should (>= (plist-get (glitnir-stats) :count) (1+ n0)))
              (should (>= (plist-get (glitnir--last-paint) :commit-ns) t0))
              (should (equal (take 2 (glitnir--layer-rect c)) (list (round (* 260 s)) (round (* 230 s))))))))
      (glitnir-gui--delete fs))))

;; The Lisp surface (Task 33): chrome resolved from parameters and faces reaches the layer.

(defun glitnir-gui--chromed (&rest params)
  "A shown child frame with PARAMS (corner-radius, shadow ...) as frame parameters."
  (let* ((b (glitnir-gui--text-buffer "*glitnir-chrome*" 5))
         (c (glitnir-gui--child b t "#00ffff")))
    (modify-frame-parameters c params)
    (redisplay t) (sit-for 0.1)
    c))

(ert-deftest glitnir-gui/chrome-reaches-glitnir ()
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
          (redisplay t) (sit-for 0.1)
          (should-not (glitnir--chrome c)))
      (glitnir-gui--delete (list c)))))

(ert-deftest glitnir-gui/per-frame-border-face-follows ()
  (let ((c (glitnir-gui--chromed '(corner-radius . 8))))
    (unwind-protect
        (progn
          (set-face-background 'child-frame-border "red" c)
          (redisplay t) (sit-for 0.2) (redisplay t)
          (let ((b (plist-get (glitnir--chrome c) :border)))
            (should b)
            (should (equal (take 3 (plist-get b :color)) '(255 0 0)))
            (should (> (plist-get b :width) 0)))
          (set-face-background 'child-frame-border "blue" c)
          (redisplay t) (sit-for 0.2) (redisplay t)
          (should (equal (take 3 (plist-get (plist-get (glitnir--chrome c) :border) :color)) '(0 0 255))))
      (glitnir-gui--delete (list c)))))

(ert-deftest glitnir-gui/per-frame-shadow-face-follows ()
  (let ((c (glitnir-gui--chromed '(shadow . (:blur 16 :opacity 0.35)))))
    (unwind-protect
        (progn
          (should (equal (plist-get (plist-get (glitnir--chrome c) :shadow) :color) '(0 0 0)))
          (set-face-foreground 'child-frame-shadow "red" c)
          (redisplay t) (sit-for 0.2) (redisplay t)
          (should (equal (plist-get (plist-get (glitnir--chrome c) :shadow) :color) '(255 0 0))))
      (glitnir-gui--delete (list c)))))

;;; glitnir-gui-tests.el ends here
