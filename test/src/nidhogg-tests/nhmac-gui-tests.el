;;; nhmac-gui-tests.el --- Naglfar's GUI rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh on the macOS runner.  Rows are appended under ";; Task N:" headings.

(require 'ert)
(load (expand-file-name "../../../nidhogg/test/mac-counts.el" (file-name-directory (or load-file-name buffer-file-name))) nil t)

(defun nhmac-gui--ns-build-p ()
  "The stock NS port (not Naglfar): the baselines are measured here."
  (and (eq window-system 'ns) (not (featurep 'nidhogg-macos))))

;; Task 2: the interposer and the counters harness
(ert-deftest nhmac-interposer-counts-ns ()
  "20 keys into a buffer lock a surface and spin the run loop inside a redisplay (NS baseline: the counts are logged).
NS coalesces: a run measured 16 locks per 20 keys at scale 1 and 10 at scale 2, so the bound is only that both happen."
  (skip-unless (nhmac-gui--ns-build-p))
  (switch-to-buffer (generate-new-buffer " *mac-counts*"))
  (mac-counts-install)
  (mac-counts-redisplay t)
  (let ((before (mac-counts-read)))
    (dotimes (_ 20)
      (execute-kbd-macro "x")
      (mac-counts-redisplay t))
    (let* ((after (mac-counts-read))
           (d (lambda (k) (- (plist-get after k) (plist-get before k)))))
      (message "nhmac-counts: 20 keys: locks %d bytes %d spins %d spins-in-redisplay %d"
               (funcall d :surface-locks) (funcall d :surface-bytes) (funcall d :spins) (funcall d :spins-in-redisplay))
      (should (>= (funcall d :surface-locks) 1))
      (should (>= (funcall d :spins-in-redisplay) 1)))))

(ert-deftest nhmac-ns-menu-dump ()
  "NS's real menu bar, built through menu.c, is dumped to a file: the fixture Naglfar's flattener is checked against."
  (skip-unless (nhmac-gui--ns-build-p))
  (let ((file (expand-file-name "nhmac-ns-menu.tsv" (or (getenv "GLITNIR_GUI_OUT") temporary-file-directory))))
    (when (file-exists-p file) (delete-file file))
    (mac-counts-menu-dump file)
    (should (file-exists-p file))
    (with-temp-buffer
      (insert-file-contents file)
      (should (> (count-lines (point-min) (point-max)) 20))
      (should (re-search-forward "^1\t[^\t]*\t" nil t)))))

;; The window system's names: Naglfar reports NS's, so ns-win.el and packages take their macOS branches
(ert-deftest nhmac-window-system-is-ns ()
  "`window-system', `initial-window-system' and the terminal's type read `ns', and the shell provides
`nidhogg-macos' and `ns'."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (should (featurep 'nidhogg-macos))
  (should (featurep 'ns))
  (should (eq window-system 'ns))
  (should (eq initial-window-system 'ns))
  (should (eq (terminal-live-p nil) 'ns)))

;; Frames: an NSWindow per frame, its root view hosting the shell's layer
(ert-deftest nhmac-frame-opens ()
  "`make-frame' gives a visible NSWindow whose root view hosts the shell's layer (no subviews), sized in device
pixels; the display's width is the main screen's."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((f (make-frame '((width . 60) (height . 20)))))
    (unwind-protect
        (let ((info (progn (redisplay t) (sit-for 0.2) (nhmac--window-info f))))
          (should (frame-visible-p f))
          (should (eq (plist-get info :visible) t))
          (should (eq (plist-get info :layer) t))
          (should (eq (plist-get info :wants-layer) t))
          (should (= (plist-get info :subviews) 0))
          (should (= (frame-native-width f) (plist-get info :device-width)))
          (should (= (display-pixel-width) (plist-get info :screen-device-width))))
      (delete-frame f))))

;; Task 11: the pumps and the take-out scope
(ert-deftest nhmac-reentry-sync-delegates ()
  "AppKit's delegates, called back from inside the shell's own calls, reach its state only through the scope.
`setFrame:' and `makeKeyAndOrderFront:' with the scope free are applied at once; inside a borrow (with a modal session
and a nested event loop) they queue, and each queued event is applied once when the lend ends; a deferred effect runs
with the borrow put back.  No two borrows are ever live."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((before (nhmac-stats))
         (r (nhmac--test-reentry))
         (after (nhmac-stats)))
    (message "nhmac-reentry: %S" r)
    (should (= (plist-get r :overlapping-takes) 0))
    (should (= (plist-get r :sync-queued) 0))
    (should (>= (plist-get r :inner-queued) 2))
    (should (= (plist-get r :after-effect-queued) (plist-get r :inner-queued)))
    (should (= (plist-get r :queued) (plist-get r :applied)))
    (should (>= (plist-get r :takes) 3))
    (should (= (plist-get after :overlapping-takes) (plist-get before :overlapping-takes)))))

(ert-deftest nhmac-pump-modal-resize ()
  "A query that runs a modal session, with a window resized inside it: the delegate takes the lent state, nothing
is left queued, and the display is not poisoned."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((panics (length (nhwl-glue-panics)))
         (r (nhmac--modal-resize 300)))
    (message "nhmac-modal-resize: %S" r)
    (should (>= (plist-get r :rounds) 2))
    (should (>= (plist-get r :takes) 1))
    (should (= (plist-get r :overlapping-takes) 0))
    (should (= (plist-get r :queued) (plist-get r :applied)))
    (should (= (length (nhwl-glue-panics)) panics))
    (should (equal (eval '(+ 1 1) t) 2))))

;; Task 11a: keys and mouse intake
(defconst nhmac-gui--ns-control #x40000 "`NSEventModifierFlagControl'.")

(defun nhmac-gui--events (n &optional secs)
  "Read up to N events, each within SECS seconds (default 2); a list in order."
  (let (events ev)
    (while (and (> n 0) (setq ev (read-event nil nil (or secs 2))))
      (push ev events)
      (setq n (1- n)))
    (nreverse events)))

(defun nhmac-gui--flush ()
  "Drop the input waiting now."
  (while (read-event nil nil 0.1)))

(ert-deftest nhmac-keys-basic ()
  "Plain characters, the named keys and control chords posted as AppKit key events arrive as NS reads them; C-g
ends a `sit-for' early."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--flush)
  (let ((plain "abcdefghijklmnopqrstuvwxyz0123"))
    (mapc #'nhmac--test-post-key plain)
    (should (equal (nhmac-gui--events (length plain)) (append plain nil))))
  (let ((named '((?\r . return) (?\t . tab) (#x7f . backspace) (#x1b . escape)
                 (#xF700 . up) (#xF701 . down) (#xF702 . left) (#xF703 . right)
                 (#xF729 . home) (#xF72B . end) (#xF72C . prior) (#xF72D . next)
                 (#xF728 . delete)))
        (fkeys (number-sequence 1 12)))
    (dolist (k named) (nhmac--test-post-key (car k)))
    (dolist (n fkeys) (nhmac--test-post-key (+ #xF703 n)))
    (should (equal (nhmac-gui--events (+ (length named) (length fkeys)))
                   (append (mapcar #'cdr named)
                           (mapcar (lambda (n) (intern (format "f%d" n))) fkeys)))))
  ;; C-x C-f: AppKit's characters are the control characters, the keys' own are x and f
  (nhmac--test-post-key ?\C-x ?x nhmac-gui--ns-control)
  (nhmac--test-post-key ?\C-f ?f nhmac-gui--ns-control)
  (should (equal (nhmac-gui--events 2) '(?\C-x ?\C-f)))
  (let ((start (float-time)) quit)
    (nhmac--test-post-key ?\C-g ?g nhmac-gui--ns-control)
    (condition-case nil
        (let ((inhibit-quit nil))
          (sit-for 5)
          (setq quit (eq (read-event nil nil 1) ?\C-g)))
      (quit (setq quit t)))
    (should quit)
    (should (< (- (float-time) start) 4))))

(defun nhmac-gui--pos-xy (pos)
  "Device pixels from the frame's top left of the middle of the character at POS."
  (let ((xy (posn-x-y (posn-at-point pos)))
        (edges (window-inside-pixel-edges)))
    (cons (+ (nth 0 edges) (car xy) (/ (frame-char-width) 2))
          (+ (nth 1 edges) (cdr xy) (/ (frame-char-height) 2)))))

(ert-deftest nhmac-mouse-basic ()
  "A click posted as AppKit mouse events lands on its character, a drag spans its region, a wheel notch scrolls,
and `mouse-pixel-position' follows motion."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (with-current-buffer (get-buffer-create " *nhmac-mouse*")
    (erase-buffer)
    (dotimes (i 200) (insert (format "line %03d of the mouse buffer\n" i)))
    (goto-char (point-min)))
  (switch-to-buffer " *nhmac-mouse*")
  (redisplay t)
  (nhmac-gui--flush)
  (let* ((p1 (save-excursion (goto-char (point-min)) (forward-line 2) (+ (point) 5)))
         (p2 (save-excursion (goto-char (point-min)) (forward-line 4) (+ (point) 9)))
         (a (nhmac-gui--pos-xy p1))
         (b (nhmac-gui--pos-xy p2)))
    ;; a click
    (nhmac--test-post-mouse 'down (car a) (cdr a))
    (nhmac--test-post-mouse 'up (car a) (cdr a))
    (let ((evs (nhmac-gui--events 2)))
      (should (equal (mapcar #'event-basic-type evs) '(mouse-1 mouse-1)))
      (should (eq (car (nth 0 evs)) 'down-mouse-1))
      (should (= (posn-point (event-start (nth 1 evs))) p1)))
    ;; a drag
    (nhmac--test-post-mouse 'down (car a) (cdr a))
    (nhmac--test-post-mouse 'drag (car b) (cdr b))
    (nhmac--test-post-mouse 'up (car b) (cdr b))
    (let* ((evs (seq-remove (lambda (e) (eq (car-safe e) 'mouse-movement))
                            (nhmac-gui--events 4 1)))
           (drag (car (last evs))))
      (should (eq (car drag) 'drag-mouse-1))
      (should (= (posn-point (event-start drag)) p1))
      (should (= (posn-point (event-end drag)) p2)))
    ;; motion
    (nhmac--test-post-mouse 'move (car b) (cdr b))
    (sit-for 0.3)
    (let ((mp (mouse-pixel-position)))
      (should (eq (car mp) (selected-frame)))
      (should (< (abs (- (cadr mp) (car b))) 3))
      (should (< (abs (- (cddr mp) (cdr b))) 3)))
    (nhmac-gui--flush)
    ;; a wheel notch down
    (nhmac--test-post-mouse 'wheel (car a) (cdr a) 1)
    (let ((ev (car (seq-remove (lambda (e) (eq (car-safe e) 'mouse-movement))
                               (nhmac-gui--events 3 1)))))
      (should (eq (event-basic-type ev) 'wheel-down)))))

;; The guard: an Objective-C exception in a pump poisons the display, as a panic does, and Emacs goes on
(ert-deftest nhmac-objc-exception-poisons ()
  "An NSException raised inside the AppKit pump is caught; the display is poisoned and Emacs does not abort."
  :tags '(runs-last)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((before (length (nhwl-glue-panics))))
    (nhmac--test-raise-in-pump)
    (sit-for 0.5)
    (should (> (length (nhwl-glue-panics)) before))
    (should (string-match-p "Objective-C exception" (car (nhwl-glue-panics))))
    ;; Emacs is still here and still evaluates
    (should (equal (eval '(+ 1 1) t) 2))))

;;; nhmac-gui-tests.el ends here
