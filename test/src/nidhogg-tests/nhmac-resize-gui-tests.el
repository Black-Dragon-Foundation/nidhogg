;;; nhmac-resize-gui-tests.el --- Naglfar's own edge-drag resize, on macOS  -*- lexical-binding: t -*-

;; test-class: cond
;; test-pattern: handshake

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).  The rows drive the window
;; through `sendEvent:' with posted AppKit mouse events, so they need no Accessibility grant, except
;; `nhmac-resize-band-outside-real', which posts through the window server.  `test-resize' reads the window's frame
;; (screen points, origin bottom left), its content size, `inLiveResize', whether a resize waits for the host, and
;; the counters.

(require 'ert)
(require 'cl-lib)
(declare-function nhmac-gui--until "nhmac-gui-tests" (pred secs))
(declare-function nhmac-gui--shown "nhmac-gui-tests" (&optional secs))

(defconst nhmac-resize--option #x80000 "`NSEventModifierFlagOption'.")
(defconst nhmac-resize--shift #x20000 "`NSEventModifierFlagShift'.")
(defconst nhmac-resize--edges '((left . 1) (right . 2) (bottom . 4) (top . 8))
  "The edge bits `test-resize-cursor' takes.")

(defun nhmac-resize--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--query)))

(defun nhmac-resize--state ()
  (nhmac--query 'test-resize (selected-frame)))

(defun nhmac-resize--stretched ()
  (plist-get (nhmac-stats) :stretched-frames))

(defun nhmac-resize--answered (&optional secs)
  "Wait (SECS at most, default 1) until the pump has sent every posted mouse event (`test-posted' 0) and the host has
answered the resize waiting for it (`:pending' nil); the state."
  (let (st)
    (nhmac-gui--until (lambda () (and (eql (nhmac--query 'test-posted) 0)
                                      (not (plist-get (setq st (nhmac-resize--state)) :pending))))
                      (or secs 1))
    st))

(defun nhmac-resize--settle ()
  "Let the host answer the resize waiting, up to 4 s, and its frame be shown; the state."
  (nhmac-resize--answered 4)
  (nhmac-gui--shown 1)
  (nhmac-resize--state))

(defun nhmac-resize--point (edges &optional out)
  "A point OUT points (default 3) outside the frame on EDGES, as device pixels from the content's top left."
  (let* ((st (nhmac-resize--state))
         (scale (/ (plist-get st :scale120) 120.0))
         (frame (plist-get st :frame)) (content (plist-get st :content))
         (cw (nth 0 content)) (ch (nth 1 content))
         (title (- (nth 3 frame) ch))
         (out (or out 3))
         (x (cond ((memq 'left edges) (- out)) ((memq 'right edges) (+ cw out)) (t (/ cw 2.0))))
         (y (cond ((memq 'top edges) (- (+ title out))) ((memq 'bottom edges) (+ ch out)) (t (/ ch 2.0)))))
    (cons (* x scale) (* y scale))))

(defun nhmac-resize--post (kind at flags)
  (nhmac--call 'test-post-mouse (selected-frame) kind (car at) (cdr at) 0 flags))

(defun nhmac-resize--drag (edges steps dx dy &optional flags busy)
  "Press on EDGES' band, drag STEPS steps of DX, DY points (y up), release; Lisp busy 50 ms every fifth step with
BUSY.  The settled state."
  (let* ((at (nhmac-resize--point edges))
         (scale (/ (plist-get (nhmac-resize--state) :scale120) 120.0))
         (x (car at)) (y (cdr at)))
    (nhmac-resize--post 0 at (or flags 0))
    (nhmac-resize--answered)
    (dotimes (i steps)
      (setq x (+ x (* dx scale)) y (- y (* dy scale)))
      (nhmac-resize--post 2 (cons x y) (or flags 0))
      (if (and busy (= (% i 5) 4))
          (let ((end (+ (float-time) 0.05))) (while (< (float-time) end)))
        ;; each step's resize waits for the host's answer
        (nhmac-resize--answered)))
    (nhmac-resize--post 1 (cons x y) (or flags 0))
    (nhmac-resize--settle)))

(defun nhmac-resize--reset (w h)
  "Text area W x H points, settled.  Emacs's pixels are device pixels: at scale 2, 600 of them were 300 points, and
band-outside's drag never grew the frame past 600 points (the live session, 2026-11-13)."
  (let ((s (frame-scale-factor)))
    (set-frame-size nil (round (* w s)) (round (* h s)) t))
  (nhmac-resize--settle))

(defun nhmac-resize--fullscreen (value)
  "Set the frame's `fullscreen' parameter to VALUE; wait (4 s at most) for the window to be in or out of full screen,
its transition over.  While AppKit's exit animation runs the style mask still has full screen, so the band does not
apply: the first press of min-size after green-button at scale 2 was not a band press (the live session, 2026-11-13)."
  (unless (eq (frame-parameter nil 'fullscreen) value)
    (set-frame-parameter nil 'fullscreen value))
  (let ((want (and (memq value '(fullboth fullscreen)) t)))
    (nhmac-gui--until (lambda () (let ((st (nhmac--query 'test-window-state (selected-frame))))
                                   (and (eq (and (plist-get st :fullscreen) t) want)
                                        (eq (and (plist-get st :style-full) t) want)
                                        (not (plist-get st :fs-busy)))))
                      4))
  (nhmac-resize--settle))

(defmacro nhmac-resize--row (&rest body)
  "Run BODY on the frame out of full screen at 600 x 400 points, then put the frame back as it was.
The runner starts the frame in full screen (`--fullscreen'), where the band does not apply and a size is not taken."
  (declare (indent 0))
  `(let ((fullscreen (frame-parameter nil 'fullscreen)))
     (unwind-protect
         (progn
           (nhmac-resize--fullscreen nil)
           (nhmac-resize--reset 600 400)
           ,@body)
       (nhmac-resize--reset 600 400)
       (nhmac-resize--fullscreen fullscreen))))

(defun nhmac-resize--outward (edges)
  "A step of 1 point outward on EDGES: (DX . DY), y up."
  (cons (cond ((memq 'left edges) -1) ((memq 'right edges) 1) (t 0))
        (cond ((memq 'bottom edges) -1) ((memq 'top edges) 1) (t 0))))

(ert-deftest nhmac-resize-no-stretched-frames ()
  "60 drag steps on every edge and corner, Lisp busy for 50 ms on every fifth: each grows the window by its drag,
and no commit shows a surface of a size other than the window's content."
  (skip-unless (nhmac-resize--naglfar-p))
  (nhmac-resize--row
    (dolist (edges '((right) (left) (top) (bottom) (top left) (top right) (bottom left) (bottom right)))
      (nhmac-resize--reset 600 400)
      (let* ((before (nhmac-resize--state)) (stretched (nhmac-resize--stretched))
             (d (nhmac-resize--outward edges))
             (after (nhmac-resize--drag edges 60 (car d) (cdr d) 0 t))
             (f0 (plist-get before :frame)) (f1 (plist-get after :frame)))
        (should (equal (list edges (- (nth 2 f1) (nth 2 f0)) (- (nth 3 f1) (nth 3 f0)))
                       (list edges (if (/= (car d) 0) 60 0) (if (/= (cdr d) 0) 60 0))))
        (should (equal (list edges (- (nhmac-resize--stretched) stretched)) (list edges 0)))
        (should-not (plist-get after :live))))))

(ert-deftest nhmac-resize-band-outside ()
  "A press 3 points outside the right edge starts our resize, and `inLiveResize' stays NO throughout."
  (skip-unless (nhmac-resize--naglfar-p))
  (nhmac-resize--row
    (let* ((presses (plist-get (nhmac-resize--state) :band-presses))
           (at (nhmac-resize--point '(right)))
           (live nil))
      (nhmac-resize--post 0 at 0)
      (nhmac-resize--answered)
      (dotimes (i 10)
        (nhmac-resize--post 2 (cons (+ (car at) (* (1+ i) 2)) (cdr at)) 0)
        (nhmac-resize--answered)
        (push (plist-get (nhmac-resize--state) :live) live))
      (nhmac-resize--post 1 (cons (+ (car at) 20) (cdr at)) 0)
      (let ((st (nhmac-resize--settle)))
        (should (= (plist-get st :band-presses) (1+ presses)))
        (should (equal (delq nil live) nil))
        (should (> (nth 0 (plist-get st :content)) 600))))))

(ert-deftest nhmac-resize-band-outside-real ()
  "Through the window server: a real press 3 points outside the right edge, then the bottom-right corner, reaches
the window's `sendEvent:', starts our resize (`inLiveResize' NO) and grows the frame, with no stretched frame."
  (skip-unless (nhmac-resize--naglfar-p))
  (nhmac-resize--row
    (let ((access (nhmac--query 'test-post-access)))
      (unless (eq access t)
        (ert-skip (format "the runner's grant does not allow posting events (CGPreflightPostEventAccess %S)" access))))
    (dolist (edges '((right) (bottom right)))
      (nhmac-resize--reset 600 400)
      (let* ((before (nhmac-resize--state)) (stretched (nhmac-resize--stretched))
             (d (nhmac-resize--outward edges)) (live nil))
        (nhmac--call 'test-post-real (selected-frame) 0 (if (memq 'bottom edges) 1 0) 3 0 0)
        ;; the window server delivers it: the window counts the band press
        (nhmac-gui--until (lambda () (> (plist-get (nhmac-resize--state) :band-presses)
                                        (plist-get before :band-presses)))
                          1)
        (dotimes (i 10)
          (let ((frame (plist-get (nhmac-resize--state) :frame)))
            (nhmac--call 'test-post-real (selected-frame) 2 (if (memq 'bottom edges) 1 0) 3
                         (* (1+ i) 2 (car d)) (* (1+ i) 2 (cdr d)))
            ;; delivered: the window's frame follows the drag, and the host answered
            (nhmac-gui--until (lambda () (not (equal (plist-get (nhmac-resize--state) :frame) frame))) 1)
            (nhmac-resize--answered))
          (push (plist-get (nhmac-resize--state) :live) live))
        (nhmac--call 'test-post-real (selected-frame) 1 (if (memq 'bottom edges) 1 0) 3
                     (* 20 (car d)) (* 20 (cdr d)))
        (let ((after (nhmac-resize--settle)))
          (should (= (plist-get after :band-presses) (1+ (plist-get before :band-presses))))
          (should (equal (delq nil live) nil))
          (should (> (nth 2 (plist-get after :frame)) (nth 2 (plist-get before :frame))))
          (should (= (- (nhmac-resize--stretched) stretched) 0)))))))

(ert-deftest nhmac-resize-option-drag ()
  "Option-drag on the right edge resizes about the centre, to within 1 device pixel."
  (skip-unless (nhmac-resize--naglfar-p))
  (nhmac-resize--row
    (let* ((before (plist-get (nhmac-resize--state) :frame))
           (st (nhmac-resize--drag '(right) 20 1 0 nhmac-resize--option))
           (after (plist-get st :frame))
           (scale (/ (plist-get st :scale120) 120.0))
           (mid (lambda (f) (+ (nth 0 f) (/ (nth 2 f) 2.0)))))
      (should (= (nth 2 after) (+ (nth 2 before) 40)))
      (should (<= (abs (* scale (- (funcall mid after) (funcall mid before)))) 1)))))

(ert-deftest nhmac-resize-shift-drag ()
  "Shift-drag on the right edge holds the aspect ratio."
  (skip-unless (nhmac-resize--naglfar-p))
  (nhmac-resize--row
    (let* ((before (plist-get (nhmac-resize--state) :frame))
           (after (plist-get (nhmac-resize--drag '(right) 30 2 0 nhmac-resize--shift) :frame)))
      (should (> (nth 2 after) (nth 2 before)))
      ;; whole points: the ratio to within one point of the height
      (should (<= (abs (- (nth 3 after) (/ (* (nth 2 after) (nth 3 before)) (float (nth 2 before))))) 1)))))

(ert-deftest nhmac-resize-cursors ()
  "Over each band position the current cursor is the system's resize cursor for it."
  (skip-unless (nhmac-resize--naglfar-p))
  (nhmac-resize--row
    (dolist (edges '((right) (left) (top) (bottom) (top left) (top right) (bottom left) (bottom right)))
      (nhmac-resize--post 3 (nhmac-resize--point edges 2) 0)
      (let ((bits (apply #'logior (mapcar (lambda (e) (alist-get e nhmac-resize--edges)) edges))))
        (should (equal (list edges (nhmac-gui--until
                                    (lambda () (nhmac--query 'test-resize-cursor (selected-frame) bits)) 1))
                       (list edges t)))))))

(ert-deftest nhmac-resize-green-button ()
  "`zoom:' zooms the window and back, and full screen enters and leaves, with no stretched frame across any of the
animations."
  (skip-unless (nhmac-resize--naglfar-p))
  (nhmac-resize--row
    (let ((before (plist-get (nhmac-resize--state) :frame))
          (stretched (nhmac-resize--stretched)))
      (nhmac--call 'test-zoom (selected-frame))
      (let ((zoomed (plist-get (nhmac-resize--settle) :frame)))
        (should-not (equal zoomed before))
        (should (eq (plist-get (nhmac--query 'test-window-state (selected-frame)) :zoomed) t))
        (should (equal (list 'zoom (- (nhmac-resize--stretched) stretched)) '(zoom 0))))
      (nhmac--call 'test-zoom (selected-frame))
      (should (equal (plist-get (nhmac-resize--settle) :frame) before))
      (setq stretched (nhmac-resize--stretched))
      (set-frame-parameter nil 'fullscreen 'fullboth)
      (nhmac-gui--until (lambda () (plist-get (nhmac--query 'test-window-state (selected-frame)) :fullscreen)) 4)
      (nhmac-resize--settle)
      (should (equal (list 'enter (- (nhmac-resize--stretched) stretched)) '(enter 0)))
      (setq stretched (nhmac-resize--stretched))
      (set-frame-parameter nil 'fullscreen nil)
      (nhmac-gui--until (lambda () (not (plist-get (nhmac--query 'test-window-state (selected-frame)) :fullscreen))) 4)
      (nhmac-resize--settle)
      (should (equal (list 'leave (- (nhmac-resize--stretched) stretched)) '(leave 0))))))

(ert-deftest nhmac-resize-tiling ()
  "The Window menu's Move & Resize items tile the window with no stretched frame."
  (skip-unless (nhmac-resize--naglfar-p))
  (ert-skip "no public way to choose AppKit's own Move & Resize items in-process (see DEFERRED)"))

(ert-deftest nhmac-resize-min-size ()
  "Dragging the left edge far past the right one stops at the window's minimum content size."
  (skip-unless (nhmac-resize--naglfar-p))
  (nhmac-resize--row
    (let* ((st (nhmac-resize--drag '(left) 20 50 0))
           (min (plist-get st :min)))
      (should (= (nth 0 (plist-get st :content)) (nth 0 min)))
      (should-not (plist-get st :pending)))))

;;; nhmac-resize-gui-tests.el ends here
