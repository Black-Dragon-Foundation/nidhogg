;;; nhmac-lifecycle-gui-tests.el --- Naglfar's app lifecycle rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond
;; test-pattern: handshake

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).  The application events are
;; posted through the test hook that stands for the delegate's callbacks.

(require 'ert)
(require 'cl-lib)

(defun nhmac-life--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--query)))

;; Task 31: app lifecycle, open-file events, gestures, monitors
(ert-deftest nhmac-quit-confirm ()
  "AppKit asking to terminate runs `nhmac-quit', which asks when `ns-confirm-quit' is set and does not quit on no."
  (skip-unless (nhmac-life--naglfar-p))
  (let ((ns-confirm-quit t) (asked nil))
    (cl-letf (((symbol-function 'y-or-n-p) (lambda (&rest _) (setq asked t) nil)))
      (nhmac--call 'test-app-event 2)
      (let ((cmd (key-binding (vector (read-event nil nil 2) (read-event nil nil 2)))))
        (should (eq cmd 'nhmac-quit))
        (call-interactively cmd)))
    (should asked)))

(ert-deftest nhmac-open-files ()
  "Files the delegate queued are visited by `nhmac-open-files'."
  (skip-unless (nhmac-life--naglfar-p))
  (let ((file (make-temp-file "nhmac-open-files")))
    (nhmac--call 'test-open-files file)
    (should (eq (key-binding (vector (read-event nil nil 2) (read-event nil nil 2))) 'nhmac-open-files))
    (nhmac-open-files)
    ;; `ns-find-file' visits the true name (/private/var/... for a temporary file under /var)
    (should (find-buffer-visiting file))))

(ert-deftest nhmac-reopen-hide ()
  "A reopen with no visible window shows a frame."
  (skip-unless (nhmac-life--naglfar-p))
  (nhmac--call 'test-app-event 3)
  (should (eq (key-binding (vector (read-event nil nil 2) (read-event nil nil 2))) 'nhmac-reopen))
  (nhmac-reopen)
  (should (frame-visible-p (selected-frame))))

(ert-deftest nhmac-magnify ()
  "A magnify gesture is a text scale change."
  (skip-unless (nhmac-life--naglfar-p))
  (ert-skip "pending: a synthesised magnify NSEvent (the mapping is `Magnify' in outputs.rs, tested on Linux)"))

;; AppKit's documented behaviour (`hasPreciseScrollingDeltas', `scrollingDeltaY' in points, `momentumPhase'): a
;; trackpad's scroll of D points, in its gesture or in the momentum after it, is one wheel event whose pixel delta is D
;; points in device pixels, as `pixel-scroll-precision-mode' takes it.
(defun nhmac-life--precise-events (points phase)
  "Post a precise scroll of POINTS (positive down) in PHASE (0 the gesture, 1 momentum) over the selected window;
the wheel events that arrive."
  (let* ((e (window-inside-pixel-edges)) (events nil))
    (nhmac--call 'test-post-mouse (selected-frame) 6 (+ (nth 0 e) 20) (+ (nth 1 e) 20) points phase)
    (let ((end (+ (float-time) 2)))
      (while (and (< (float-time) end) (not events))
        (let ((ev (read-event nil nil 0.2)))
          (when (and (consp ev) (memq (event-basic-type ev) '(wheel-up wheel-down)))
            (push ev events))))
      (while-let ((ev (read-event nil nil 0.2)))
        (when (and (consp ev) (memq (event-basic-type ev) '(wheel-up wheel-down)))
          (push ev events))))
    (nreverse events)))

(ert-deftest nhmac-precise-scroll ()
  "A trackpad's precise scroll of 10 points down, in the gesture and in momentum, and of 7 up: each is one wheel
event in its direction whose pixel delta is the points times the window's screen's `backingScaleFactor' (device
pixels).  The window's, not the main screen's: the lane's display is another screen, at scale 2 in the scale-2 lane
\(round 2 took the main screen's 1 and expected -10 for the window's -20)."
  (skip-unless (nhmac-life--naglfar-p))
  (switch-to-buffer (get-buffer-create "nhmac-precise-scroll"))
  (erase-buffer)
  (dotimes (i 200) (insert (format "line %d\n" i)))
  (goto-char (point-min))
  (redisplay t)
  (let ((mwheel-coalesce-scroll-events nil)
        (k (plist-get (nhmac--test-surface 0 0) :scale)))
    (pcase-dolist (`(,points ,phase ,type) '((10 0 wheel-down) (10 1 wheel-down) (-7 0 wheel-up)))
      (let ((events (nhmac-life--precise-events points phase)))
        (message "nhmac-precise-scroll: %d points, phase %d: %S" points phase events)
        (should (= (length events) 1))
        (should (eq (event-basic-type (car events)) type))
        (should (= (cdr (nth 4 (car events))) (- (* points k)))))))
  (kill-buffer))

(ert-deftest nhmac-monitors ()
  "`display-monitor-attributes-list' is the screens as AppKit has them, the main one first: each geometry's size is
the screen's `frame' size times its `backingScaleFactor', each workarea its `visibleFrame' (size, and offset from the
geometry) in the same device pixels, each scale factor the backing scale."
  (skip-unless (nhmac-life--naglfar-p))
  (let ((screens (nhmac--query 'screens))
        (monitors (display-monitor-attributes-list)))
    (message "nhmac-monitors: AppKit %S, Emacs %S" screens monitors)
    (should (= (length monitors) (length screens)))
    (cl-loop for m in monitors for s in screens
             do (pcase-let ((`(,fx ,fy ,fw ,fh ,vx ,vy ,vw ,vh ,k) s)
                            (`(,gx ,gy ,gw ,gh) (alist-get 'geometry m))
                            (`(,wx ,wy ,ww ,wh) (alist-get 'workarea m)))
                  (should (equal (list gw gh) (list (round (* fw k)) (round (* fh k)))))
                  (should (equal (list (- wx gx) (- wy gy) ww wh)
                                 (list (round (* (- vx fx) k)) (round (* (- vy fy) k))
                                       (round (* vw k)) (round (* vh k)))))
                  (should (= (alist-get 'scale-factor m) k))))))

;;; nhmac-lifecycle-gui-tests.el ends here
