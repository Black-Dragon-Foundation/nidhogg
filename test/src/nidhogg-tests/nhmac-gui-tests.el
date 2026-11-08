;;; nhmac-gui-tests.el --- Naglfar's GUI rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh on the macOS runner.  Rows are appended under ";; Task N:" headings.  A row tagged
;; `draws' needs the frame to have drawn: when it has not at a scale, gui-runner fails those rows at once ("no frame
;; drew at scale N") and runs the rest.

(require 'ert)
(require 'cl-lib)
(load (expand-file-name "../../../nidhogg/test/mac-counts.el" (file-name-directory (or load-file-name buffer-file-name))) nil t)

(defun nhmac-gui--say (fmt &rest args)
  "Print FMT with ARGS as a line of the lane's log (stderr), not to *Messages*."
  (princ (concat (apply #'format fmt args) "\n") #'external-debugging-output))

(defun nhmac-gui--ns-build-p ()
  "The stock NS port (not Naglfar): the baselines are measured here."
  (and (eq window-system 'ns) (not (featurep 'nidhogg-macos))))

;; Task 2: the interposer and the counters harness
(ert-deftest nhmac-interposer-counts-ns ()
  "20 keys into a buffer lock a surface and spin the run loop inside a redisplay (NS baseline: the counts are logged).
NS coalesces: a run measured 16 locks per 20 keys at scale 1 and 10 at scale 2, so the bound is only that both happen."
  (skip-unless (nhmac-gui--ns-build-p))
  (mac-counts-require)
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
  (mac-counts-require)
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
  :tags '(draws)
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
  :tags '(draws)
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
  (let* ((p0 (save-excursion (goto-char (point-min)) (forward-line 7) (+ (point) 3)))
         (p1 (save-excursion (goto-char (point-min)) (forward-line 2) (+ (point) 5)))
         (p2 (save-excursion (goto-char (point-min)) (forward-line 4) (+ (point) 9)))
         (c (nhmac-gui--pos-xy p0))
         (a (nhmac-gui--pos-xy p1))
         (b (nhmac-gui--pos-xy p2)))
    ;; a click, away from where the drag starts: a press within `double-click-time' and `double-click-fuzz' of the
    ;; last one is a double one (keyboard.c counts them, on every port), and the drag below follows at once
    (nhmac--test-post-mouse 'down (car c) (cdr c))
    (nhmac--test-post-mouse 'up (car c) (cdr c))
    (let ((evs (nhmac-gui--events 2)))
      (should (equal (mapcar #'event-basic-type evs) '(mouse-1 mouse-1)))
      (should (eq (car (nth 0 evs)) 'down-mouse-1))
      (should (= (posn-point (event-start (nth 1 evs))) p0)))
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

;; Task 12: the wait: kqueue in the run loop, the helper, keys before process output
(defun nhmac-gui--delta (before key)
  "How much the counter KEY of `nhmac-stats' grew since BEFORE."
  (- (plist-get (nhmac-stats) key) (plist-get before key)))

(defun nhmac-gui--feed (name type gaps)
  "Start NAME, writing 100 lines 10 ms apart through TYPE (`pipe' or `pty').
Its filter keeps NAME's line count and longest gap between outputs in
the hash table GAPS, as (LINES . GAP)."
  (let ((last (float-time)))
    (puthash name (cons 0 0.0) gaps)
    (make-process
     :name name :connection-type type :noquery t
     :command '("sh" "-c" "i=0; while [ $i -lt 100 ]; do echo $i; i=$((i+1)); sleep 0.01; done")
     :filter (lambda (_p s)
               (let ((now (float-time)) (r (gethash name gaps)))
                 (puthash name (cons (+ (car r) (cl-count ?\n s))
                                     (max (cdr r) (- now last)))
                          gaps)
                 (setq last now))))))

(ert-deftest nhmac-kq-fds ()
  "A pipe, a pty and a pipe served by the helper, under load: every line arrives with no gap near the wait's 5 s
timeout (0 missed), the kqueue's source never ends a sleep with nothing ready (0 empty wake-ups), and the helper
serves its fd.  /dev/null, /dev/tty and every /dev/cu.* are probed and logged: which kqueue refuses (EINVAL)."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (dolist (p (append '("/dev/null" "/dev/tty") (file-expand-wildcards "/dev/cu.*")))
    (message "nhmac-kq-probe: %s %S" p (nhmac--test-kq-probe p)))
  (let* ((gaps (make-hash-table :test #'equal))
         (before (nhmac-stats))
         (procs (list (nhmac-gui--feed "kq-pipe" 'pipe gaps)
                      (nhmac-gui--feed "kq-pty" 'pty gaps)))
         (helper-fds 0)
         (start (float-time)))
    ;; one wait registers the two, so the helper takes the third's fd
    (accept-process-output nil 0.05)
    (nhmac--test-helper-next)
    (push (nhmac-gui--feed "kq-helper" 'pipe gaps) procs)
    (unwind-protect
        (while (and (seq-some #'process-live-p procs) (< (- (float-time) start) 30))
          (accept-process-output nil 5)
          (setq helper-fds (max helper-fds (plist-get (nhmac-stats) :helper-fds))))
      (mapc #'delete-process procs))
    (maphash (lambda (k v) (message "nhmac-kq-fds: %s %d lines, longest gap %.3f s" k (car v) (cdr v))) gaps)
    (message "nhmac-kq-fds: stats %S" (nhmac-stats))
    (maphash (lambda (_k v)
               (should (= (car v) 100))
               (should (< (cdr v) 1.0)))
             gaps)
    (should (>= helper-fds 1))
    (should (> (nhmac-gui--delta before :kq-wakes) 0))
    (should (= (nhmac-gui--delta before :empty-wakes) 0))))

(ert-deftest nhmac-keys-before-output ()
  "Under a `yes' flood, 50 keys posted one at a time each reach Lisp at once and in order: the pump that begins
every wait stores a key before the wait reads the flood, so keys never wait behind output, and none takes the
helper's hop (`:helper-hops-keys' 0)."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--flush)
  (let* ((bytes 0)
         (before (nhmac-stats))
         (keys (append "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWX" nil))
         (proc (make-process :name "nhmac-flood" :command '("yes") :connection-type 'pipe :noquery t
                             :filter (lambda (_p s) (setq bytes (+ bytes (length s))))))
         (worst 0.0)
         got)
    (unwind-protect
        (progn
          (accept-process-output proc 0.2)
          (should (> bytes 0))
          (dolist (c keys)
            (nhmac--test-post-key c)
            (let ((t0 (float-time)))
              (push (read-event nil nil 2) got)
              (setq worst (max worst (- (float-time) t0))))))
      (delete-process proc))
    (message "nhmac-keys-before-output: worst %.3f s, %d bytes of output" worst bytes)
    (should (equal (nreverse got) keys))
    (should (< worst 0.5))
    (should (= (nhmac-gui--delta before :helper-hops-keys) 0))
    (should (> (nhmac-gui--delta before :waits) 0))))

;; Task 13: the lock hand-off
(defun nhmac-gui--idle-until (pred secs)
  "Idle the main thread in its wait until PRED holds or SECS pass; PRED's last value."
  (let ((end (+ (float-time) secs)) v)
    (while (and (not (setq v (funcall pred))) (< (float-time) end))
      (accept-process-output nil 0.05))
    v))

(defun nhmac-gui--contenders (n stop)
  "N Lisp threads contending for the lock and a Lisp mutex until (car STOP) is set.
Each round takes the mutex, notifies a condition (ALL t and nil in turn) and yields.  Return
(THREADS . STATE), STATE a vector [ROUNDS SLOW]: rounds done, and lock waits over 2 ms."
  (let* ((m (make-mutex)) (c (make-condition-variable m)) (state (vector 0 0))
         (work (lambda ()
                 (while (not (car stop))
                   (let ((t0 (float-time)))
                     (with-mutex m
                       (aset state 0 (1+ (aref state 0)))
                       (condition-notify c (zerop (% (aref state 0) 2))))
                     (when (> (- (float-time) t0) 0.002)
                       (aset state 1 (1+ (aref state 1)))))
                   (thread-yield)))))
    (cons (cl-loop for i below n collect (make-thread work (format "nhmac-c%d" i))) state)))

(ert-deftest nhmac-lock-handoff ()
  "4 Lisp threads contend for the global lock and a Lisp mutex while the main thread idles in its wait, 10,000
rounds, through `acquire_global_lock', the condition's return, `condition-notify' with ALL nil and t, and
`thread-join': every round completes, no park outlives a waiter (`:parks-with-waiters' 0), and the count is 0 at the
end.  The lock waits over 2 ms are logged, not gated."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((before (nhmac-stats))
         (stop (list nil))
         (c (nhmac-gui--contenders 4 stop))
         (state (cdr c)))
    (should (nhmac-gui--idle-until (lambda () (>= (aref state 0) 10000)) 60))
    (setcar stop t)
    (mapc #'thread-join (car c))
    (message "nhmac-lock-handoff: %d rounds, %d lock waits over 2 ms; stats %S"
             (aref state 0) (aref state 1) (nhmac-stats))
    (should (>= (aref state 0) 10000))
    (should (= (nhmac-gui--delta before :parks-with-waiters) 0))
    (should (= (plist-get (nhmac-stats) :lock-waiters) 0))))

(ert-deftest nhmac-lock-handoff-signal ()
  "3 waiters on one condition, one `condition-notify' with ALL nil per round: exactly one proceeds each round, and the
lock-waiter count is back to 0 after each (a signal marks one waiter, not all)."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((m (make-mutex)) (c (make-condition-variable m))
         (tickets 0) (taken 0) (stop nil)
         (waiters (cl-loop for i below 3 collect
                           (make-thread
                            (lambda ()
                              (with-mutex m
                                (while (not stop)
                                  (if (> tickets 0)
                                      (setq tickets (1- tickets) taken (1+ taken))
                                    (condition-wait c)))))
                            (format "nhmac-w%d" i)))))
    ;; the waiters are waiting
    (nhmac-gui--idle-until (lambda () (cl-every (lambda (th) (eq (thread--blocker th) c)) waiters)) 5)
    (dotimes (round 50)
      (with-mutex m
        (setq tickets (1+ tickets))
        (condition-notify c nil))
      (should (nhmac-gui--idle-until (lambda () (= taken (1+ round))) 5))
      (should (nhmac-gui--idle-until (lambda () (= (plist-get (nhmac-stats) :lock-waiters) 0)) 5))
      (should (= taken (1+ round))))
    (with-mutex m
      (setq stop t)
      (condition-notify c t))
    (mapc #'thread-join waiters)
    (should (= (plist-get (nhmac-stats) :lock-waiters) 0))))

(defun nhmac-gui--live-under-load ()
  "Keys, process output, a timer and C-g, all while Lisp threads hold the main thread in its yield loop."
  (nhmac-gui--flush)
  ;; 50 keys, each read and inserted
  (with-temp-buffer
    (let ((keys (append "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWX" nil)))
      (dolist (k keys)
        (nhmac--test-post-key k)
        (let ((ev (read-event nil nil 2)))
          (when (characterp ev) (insert ev))))
      (should (equal (buffer-string) (concat keys)))))
  ;; a cat echoes 20 lines
  (let* ((lines 0)
         (p (make-process :name "nhmac-cat" :command '("cat") :connection-type 'pipe :noquery t
                          :filter (lambda (_p s) (setq lines (+ lines (cl-count ?\n s)))))))
    (unwind-protect
        (progn
          (dotimes (i 20) (process-send-string p (format "%d\n" i)))
          (should (nhmac-gui--idle-until (lambda () (= lines 20)) 5)))
      (delete-process p)))
  ;; a 0.1 s timer fires once before 1 s
  (let ((n 0))
    (run-at-time 0.1 nil (lambda () (setq n (1+ n))))
    (should (nhmac-gui--idle-until (lambda () (> n 0)) 1))
    (nhmac-gui--idle-until (lambda () nil) 0.3)
    (should (= n 1)))
  ;; C-g ends a (sit-for 5) early
  (let ((start (float-time)) quit)
    (nhmac--test-post-key ?\C-g ?g nhmac-gui--ns-control)
    (condition-case nil
        (let ((inhibit-quit nil))
          (sit-for 5)
          (setq quit (eq (read-event nil nil 1) ?\C-g)))
      (quit (setq quit t)))
    (should quit)
    (should (< (- (float-time) start) 5))))

(ert-deftest nhmac-quit-reaches-busy-loop ()
  "A C-g queued in AppKit while Lisp runs a busy loop (no input read, no redisplay, no wait) quits it: the loop's
input polls pump AppKit.  The loop ends itself after 5 s, failing the row."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--flush)
  (let ((start (float-time)) quit)
    (nhmac--test-post-key ?\C-g ?g nhmac-gui--ns-control)
    (condition-case nil
        (let ((inhibit-quit nil)
              (deadline (+ start 5)))
          (while (< (float-time) deadline)))
      (quit (setq quit t)))
    (message "nhmac-quit-reaches-busy-loop: quit %S after %.2f s" quit (- (float-time) start))
    (should quit)
    (should (< (- (float-time) start) 4))))

(ert-deftest nhmac-keys-under-yield-loop ()
  "While a Lisp thread spins on `thread-yield', and again under 4-thread contention, keys, a process's output, a
timer and C-g all reach Lisp: the main thread's yield loop pumps and polls, it never parks with waiters."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((stop (list nil))
         (spinner (make-thread (lambda () (while (not (car stop)) (thread-yield))) "nhmac-spin")))
    (unwind-protect (nhmac-gui--live-under-load)
      (setcar stop t)
      (thread-join spinner)))
  (let* ((stop (list nil))
         (c (nhmac-gui--contenders 4 stop)))
    (unwind-protect (nhmac-gui--live-under-load)
      (setcar stop t)
      (mapc #'thread-join (car c))))
  (should (= (plist-get (nhmac-stats) :lock-waiters) 0)))

(ert-deftest nhmac-thread-input-hands-the-pump ()
  "A Lisp thread that reads input (its `read_socket' asks for a pump) never pumps AppKit itself: the pump is handed
to the main thread, the display lives and keys still arrive."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--flush)
  (let* ((before (nhmac-stats))
         (th (make-thread (lambda () (dotimes (_ 20) (input-pending-p) (thread-yield))) "nhmac-input")))
    (thread-join th)
    (should (> (nhmac-gui--delta before :handed-pumps) 0))
    (nhmac--test-post-key ?a)
    (should (equal (nhmac-gui--events 1) '(?a)))))

(ert-deftest nhmac-thread-wakes-idle-main ()
  "A thread waiting on a condition is woken by another Lisp thread's notify while the main thread idles in its
wait, with no AppKit event: the notifier's want of the lock ends the main thread's park."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--flush)
  (let* ((m (make-mutex)) (c (make-condition-variable m))
         (go nil) (woken nil) (notified nil)
         (waiter (make-thread (lambda ()
                                (with-mutex m
                                  (while (not go) (condition-wait c))
                                  (setq woken (float-time))))
                              "nhmac-cw"))
         (notifier (make-thread (lambda ()
                                  (sleep-for 0.2)
                                  (with-mutex m
                                    (setq go t notified (float-time))
                                    (condition-notify c)))
                                "nhmac-cn")))
    (should (nhmac-gui--idle-until (lambda () woken) 5))
    (thread-join notifier)
    (thread-join waiter)
    (message "nhmac-thread-wakes-idle-main: woken %.3f s after the notify" (- woken notified))
    (should (< (- woken notified) 0.5))))

;; The guard: an Objective-C exception in a pump poisons the display, as a panic does, and Emacs goes on
(ert-deftest nhmac-objc-exception-poisons ()
  "An NSException raised inside the AppKit pump is caught: the display is poisoned and says so, and Emacs does not
abort.  With its only display gone Emacs ends with status 70 (nhwl_connection_lost, as xterm does), so the exception
is raised in a child Emacs on the same window server, never in the runner; the child has 30 s."
  :tags '(runs-last)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((buf (generate-new-buffer " *nhmac-poison*"))
         (proc (make-process
                :name "nhmac-poison" :buffer buf :connection-type 'pipe
                :command (list (expand-file-name invocation-name invocation-directory) "-Q" "--eval"
                               "(run-at-time 1 nil (lambda () (nhmac--test-raise-in-pump) (sit-for 5) (kill-emacs 0)))"))))
    (unwind-protect
        (progn
          (should (gui-runner-wait (lambda () (not (process-live-p proc))) 30))
          (message "nhmac-objc-exception-poisons: child %S %S: %s" (process-status proc) (process-exit-status proc)
                   (with-current-buffer buf (buffer-string)))
          ;; ended by itself, with the lost display's status, not by a signal (an abort)
          (should (eq (process-status proc) 'exit))
          (should (= (process-exit-status proc) 70))
          (with-current-buffer buf
            (should (string-match-p "internal error in the shell glue" (buffer-string)))
            (should (string-match-p "Objective-C exception" (buffer-string)))))
      (when (process-live-p proc) (delete-process proc))
      (kill-buffer buf))))

(defconst nhmac-gui--start-report
  "(progn
     (let ((spec (getenv \"GLITNIR_GUI_FRAME\")))
       (when (and spec (not (string-empty-p spec)))
         (pcase-let ((`(,left ,top ,cols ,lines) (mapcar #'string-to-number (split-string spec))))
           (set-frame-size nil cols lines)
           (set-frame-position nil left top))))
     (run-at-time 1 nil
       (lambda ()
         (princ (format \"NHMAC-START %S\\n\"
                        (list :visible (frame-visible-p (selected-frame))
                              :draws (plist-get (nhmac-stats) :draws)
                              :startup (nhmac--startup-log)
                              :view (nhmac--shell-view)))
                #'external-debugging-output)
         (kill-emacs 0))))"
  "What a child Emacs in `nhmac-frame-shows-at-every-start' runs: it places its frame as gui-runner does, and a
second later prints whether the frame is visible, its draws, the shell's startup trace and view, and ends.")

(ert-deftest nhmac-frame-shows-at-every-start ()
  "A frame shows at every start: 20 child Emacsen at scale 1, on the runner's display and placed as the runner places
its frame, each become visible and draw.  Guards the start that once drew nothing at scale 1; a failing start's facts
(its configure waits, the startup trace, the shell's view) are the message.  Each child has 30 s."
  :tags '(runs-last)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (skip-unless (= (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1")) 1))
  (let ((emacs (expand-file-name invocation-name invocation-directory))
        (bad nil))
    (dotimes (i 20)
      (let* ((buf (generate-new-buffer " *nhmac-start*"))
             (t0 (float-time))
             (proc (make-process :name "nhmac-start" :buffer buf :connection-type 'pipe
                                 :command (list emacs "-Q" "--eval" nhmac-gui--start-report))))
        (unwind-protect
            (let* ((ended (gui-runner-wait (lambda () (not (process-live-p proc))) 30))
                   (out (with-current-buffer buf (buffer-string)))
                   (facts (and (string-match "^NHMAC-START \\(.*\\)$" out)
                               (car (read-from-string (match-string 1 out))))))
              (nhmac-gui--say "nhmac-frame-shows-at-every-start: start %d: %.1f s, %s: %S" i (- (float-time) t0)
                              (process-status proc) facts)
              (unless (and ended facts (plist-get facts :visible) (> (or (plist-get facts :draws) 0) 0))
                (push (list :start i :ended ended :status (process-status proc) :facts facts :output out) bad)))
          (when (process-live-p proc) (delete-process proc))
          (kill-buffer buf))))
    (should-not (nreverse bad))))

;; Task 14: the IOSurface ring, damaged rectangles, the commit
(defconst nhmac-gui--ring 3 "The ring's size (surface.rs `RING'): the oldest age a slot copies for.")

(defun nhmac-gui--surface (&optional frame)
  "The surface facts of FRAME's window, a plist (`nhmac--test-surface')."
  (nhmac--test-surface 0 0 frame))

(defun nhmac-gui--row-bytes ()
  "Bytes of one full-width text line of the selected frame."
  (* 4 (frame-pixel-width) (frame-char-height)))

(defun nhmac-gui--typing-buffer (text)
  "A fresh buffer showing TEXT in the selected window, point at its end, drawn."
  (switch-to-buffer (generate-new-buffer " *nhmac-typing*"))
  (insert text)
  (redisplay t)
  ;; the first commits make the ring's slots (whole copies); typing is measured after them
  (dotimes (_ (1+ nhmac-gui--ring))
    (insert "w")
    (redisplay)))

(defun nhmac-gui--type (n &optional before)
  "Type N characters as commands, each followed by a redisplay; the stats' changes since BEFORE (now if nil)."
  (let ((before (or before (nhmac-stats))))
    (dotimes (_ n)
      (execute-kbd-macro "x")
      (redisplay))
    (let ((after (nhmac-stats)))
      (cl-loop for (k v) on after by #'cddr
               when (numberp v) append (list k (- v (or (plist-get before k) 0)))))))

(ert-deftest nhmac-typing-bytes ()
  "Typing copies only damage into the surfaces: per key at most the damage times the ring's age, a few lines at
most, in the three §5.4 cases (end of line, mid-line, the worst plain key); never a whole surface."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((row (nhmac-gui--row-bytes))
        (keys 10))
    (pcase-dolist (`(,case ,text ,goto)
                   `((end-of-line "short line" ,#'end-of-buffer)
                     (mid-line ,(make-string 70 ?m) ,(lambda () (goto-char (+ (point-min) 35))))
                     (worst ,(make-string (- (window-body-width) 2) ?w) ,#'end-of-buffer)))
      (nhmac-gui--typing-buffer text)
      (funcall goto)
      (redisplay)
      (let* ((d (nhmac-gui--type keys))
             (bytes (plist-get d :bytes-copied))
             (damage (plist-get d :damage-bytes)))
        (message "nhmac-typing-bytes: %s: %d keys: copied %d (%d per key), damage %d, full %d, row %d"
                 case keys bytes (/ bytes keys) damage (plist-get d :full-copies) row)
        (should (= (plist-get d :full-copies) 0))
        (should (> bytes 0))
        (should (<= bytes (* nhmac-gui--ring damage)))
        ;; the text line and the mode line, over the ring's ages
        (should (<= (/ bytes keys) (* nhmac-gui--ring 2 row))))
      (kill-buffer))))

(ert-deftest nhmac-commits-per-redisplay ()
  "A redisplay after a key commits the frame's tree at most once."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer "commits")
  (let ((d (nhmac-gui--type 20)))
    (message "nhmac-commits-per-redisplay: 20 keys: %d commits" (plist-get d :commits))
    (should (<= (plist-get d :commits) 20))
    (should (>= (plist-get d :commits) 1)))
  (kill-buffer))

(ert-deftest nhmac-no-spins-in-redisplay ()
  "Redisplay turns no run loop: 0 passes of the main run loop outside the pumps while typing (NS's figure, from the
interposer in the NS lane of the same run, is logged by `nhmac-interposer-counts-ns')."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer "spins")
  (let ((d (nhmac-gui--type 20)))
    (message "nhmac-no-spins-in-redisplay: 20 keys: %d spins outside the pumps" (plist-get d :spins-in-redisplay))
    (should (= (plist-get d :spins-in-redisplay) 0)))
  (kill-buffer))

(ert-deftest nhmac-no-implicit-transaction ()
  "No implicit Core Animation transaction is left open when a publish ends, while typing."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer "implicit")
  (should (= (plist-get (nhmac-gui--type 20) :implicit-open-at-update-end) 0))
  (kill-buffer))

(ert-deftest nhmac-key-to-commit-counts ()
  "A key posted to the window reaches Lisp through one pump, with no fd wait, and its redisplay commits once."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer "key")
  (nhmac-gui--flush)
  (dotimes (_ 5)
    (let ((before (progn (nhmac--pump-trace) (nhmac-stats))))
      (nhmac--test-post-key ?k)
      (let ((ev (read-event nil nil 2)))
        (should (eq ev ?k))
        (insert ev))
      (redisplay)
      ;; the release's event, if any, is left for the next pump
      (let ((pumps (nhmac-gui--delta before :pumps))
            (commits (nhmac-gui--delta before :commits))
            (sleeps (nhmac-gui--delta before :sleeps)))
        (message "nhmac-key-to-commit-counts: pumps %d commits %d fd waits %d" pumps commits sleeps)
        ;; who asked for each pump
        (ert-info ((format "pump decisions %S" (nhmac--pump-trace)))
          (should (= pumps 1))
          (should (= commits 1))
          (should (= sleeps 0))))))
  (nhmac-gui--flush)
  (kill-buffer))

(ert-deftest nhmac-surface-busy-typing ()
  "While typing the ring always has a surface the window server does not hold."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer "busy")
  (let ((d (nhmac-gui--type 30)))
    (message "nhmac-surface-busy-typing: 30 keys: %d busy, %d acquires" (plist-get d :surface-busy)
             (plist-get d :ring-acquires))
    (should (= (plist-get d :surface-busy) 0)))
  (kill-buffer))

(ert-deftest nhmac-scroll-bytes ()
  "Scrolling a window by a line copies the scrolled rows, not the surface."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer (mapconcat #'number-to-string (number-sequence 1 300) "\n"))
  (goto-char (point-min))
  (redisplay t)
  (dotimes (_ nhmac-gui--ring) (scroll-up 1) (redisplay))
  (let* ((before (nhmac-stats))
         (_ (progn (scroll-up 1) (redisplay)))
         (bytes (nhmac-gui--delta before :bytes-copied))
         (rows (* 4 (frame-pixel-width) (window-body-height nil t))))
    (message "nhmac-scroll-bytes: one line: copied %d, the window's rows %d" bytes rows)
    (should (= (nhmac-gui--delta before :full-copies) 0))
    (should (> bytes 0))
    (should (<= bytes (* nhmac-gui--ring rows))))
  (kill-buffer))

(ert-deftest nhmac-commit-shows-without-spin ()
  "After `redisplay' the layer shows the newest surface, read back in the process, with no run-loop turn."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer "shows")
  (let ((before (nhmac-stats)))
    (insert "y")
    (redisplay)
    (let ((s (nhmac-gui--surface)))
      (message "nhmac-commit-shows-without-spin: %S" s)
      (should (eq (plist-get s :contents-is-newest) t))
      (should (equal (plist-get s :gravity) "topLeft"))
      (should (= (plist-get s :scale) (/ (plist-get (nhmac--window-info) :device-width)
                                         (float (car (plist-get s :points))))))
      (should (equal (list (plist-get s :w) (plist-get s :h))
                     (list (frame-pixel-width) (frame-pixel-height))))
      ;; a fresh backing is the frame's background (drawing comes with the record emitter)
      (should (= (logand (plist-get s :pixel) #xff000000) #xff000000)))
    (should (= (nhmac-gui--delta before :spins-in-redisplay) 0))
    (should (>= (nhmac-gui--delta before :commits) 1)))
  (kill-buffer))

(ert-deftest nhmac-colour-tagged ()
  "The surfaces' colour space follows `ns-use-srgb-colorspace': sRGB, else Generic RGB."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (unwind-protect
      (progn
        (setq ns-use-srgb-colorspace nil)
        (redraw-frame)
        (redisplay t)
        (should (eq (plist-get (nhmac-gui--surface) :srgb) 'generic))
        (setq ns-use-srgb-colorspace t)
        (redraw-frame)
        (redisplay t)
        (should (eq (plist-get (nhmac-gui--surface) :srgb) 'srgb)))
    (setq ns-use-srgb-colorspace t)))

(ert-deftest nhmac-paired-key-to-commit ()
  "Key to commit, timed in the process: 50 typed keys, each a command and a redisplay, written to
GLITNIR_GUI_OUT/pair-BUILD.txt (microseconds, one per line).  Both builds run it in turn (--pair, ABAB);
nidhogg/test/nhmac-pair.sh compares the pairs: not slower than NS on the median and p95 of the differences,
with a sign test."
  (let* ((build (if (nhmac-gui--ns-build-p) "ns" "naglfar"))
         (file (expand-file-name (format "pair-%s.txt" build)
                                 (or (getenv "GLITNIR_GUI_OUT") temporary-file-directory)))
         times)
    (switch-to-buffer (generate-new-buffer " *nhmac-pair*"))
    (insert "paired timing")
    (redisplay t)
    (dotimes (_ 10) (execute-kbd-macro "w") (redisplay))
    (dotimes (_ 50)
      (let ((t0 (float-time)))
        (execute-kbd-macro "x")
        (redisplay)
        (push (round (* 1e6 (- (float-time) t0))) times)))
    (with-temp-buffer
      (dolist (us (nreverse times)) (insert (format "%d\n" us)))
      (write-region (point-min) (point-max) file t 'silent))
    (should (file-exists-p file))
    (kill-buffer)))

;; Task 15: draw records painted with Core Graphics
(defun nhmac-gui--pixel-at (x y)
  "The committed surface's premultiplied ARGB at device pixel X, Y of the selected frame."
  (plist-get (nhmac--test-surface x y) :pixel))

(defun nhmac-gui--draw-scene ()
  "A buffer of three 12-pixel stretches: plain #336699, boxed in #ff0000 (3 pixels), then #ffffff; drawn.
Returns the frame position of the first stretch's top left."
  (switch-to-buffer (generate-new-buffer " *nhmac-draw*"))
  (setq-local cursor-type nil)
  (insert (propertize " " 'display '(space :width (12)) 'face '(:background "#336699"))
          (propertize " " 'display '(space :width (12))
                      'face '(:background "#336699" :box (:line-width 3 :color "#ff0000")))
          (propertize " " 'display '(space :width (12)) 'face '(:background "#ffffff"))
          "\n")
  (redraw-frame)
  (redisplay t)
  (let ((xy (posn-x-y (posn-at-point (point-min))))
        (edges (window-inside-pixel-edges)))
    (cons (+ (nth 0 edges) (car xy)) (+ (nth 1 edges) (cdr xy)))))

(ert-deftest nhmac-draw-fills-boxes-cursors ()
  "Fills, a box and a bar cursor reach the committed surface with their exact colours (sRGB, opaque).
A same-run comparison with NS's pixels needs a capture of the stock build; until then the colours are
checked against the faces, tolerance 0 for fills and box lines."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((o (nhmac-gui--draw-scene))
         (x (car o)) (y (cdr o)) (h (frame-char-height)))
    (message "nhmac-draw-fills-boxes-cursors: origin %S, stats %S" o (nhmac-stats))
    (should (> (plist-get (nhmac-stats) :draws) 0))
    ;; the plain stretch's middle
    (should (= (nhmac-gui--pixel-at (+ x 6) (+ y (/ h 2))) #xff336699))
    ;; the boxed stretch: its top line, then its inside
    (should (= (nhmac-gui--pixel-at (+ x 18) (+ y 1)) #xffff0000))
    (should (= (nhmac-gui--pixel-at (+ x 18) (+ y (/ h 2))) #xff336699))
    ;; the white stretch
    (should (= (nhmac-gui--pixel-at (+ x 30) (+ y (/ h 2))) #xffffffff))
    ;; a bar cursor 4 pixels wide in the cursor colour, at the white stretch
    (set-face-background 'cursor "#00ff00")
    (setq-local cursor-type '(bar . 4))
    (goto-char (+ (point-min) 2))
    (redisplay t)
    (should (= (nhmac-gui--pixel-at (+ x 25) (+ y (/ h 2))) #xff00ff00))
    ;; and gone again: the stretch's own colour where it was
    (setq-local cursor-type nil)
    (redisplay t)
    (should (= (nhmac-gui--pixel-at (+ x 25) (+ y (/ h 2))) #xffffffff)))
  (kill-buffer))

(ert-deftest nhmac-draw-other-thread ()
  "A Lisp thread's redisplay paints, but its commit waits for the main thread's next pump."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((o (nhmac-gui--draw-scene))
         (x (car o)) (y (cdr o)) (h (frame-char-height))
         (before (nhmac-stats))
         (th (make-thread
              (lambda ()
                (put-text-property (point-min) (1+ (point-min)) 'face '(:background "#00ff00"))
                (redisplay t))
              "nhmac-draw")))
    (let ((deadline (+ (float-time) 10)))
      (while (and (thread-live-p th) (< (float-time) deadline))
        (thread-yield)))
    (should-not (thread-live-p th))
    (message "nhmac-draw-other-thread: %S" (nhmac-stats))
    (should (>= (nhmac-gui--delta before :deferred-commits) 1))
    ;; the main thread's pump commits it
    (sit-for 0.1)
    (should (eq (plist-get (nhmac-gui--surface) :contents-is-newest) t))
    (should (= (nhmac-gui--pixel-at (+ x 6) (+ y (/ h 2))) #xff00ff00)))
  (kill-buffer))

;; Task 16: Core Text text, compared with NS's in the same run
(defconst nhmac-gui--font-bg "#f0e0d0" "The text rows' background: no window chrome or desktop has it.")

(defun nhmac-gui--font-file (ext &optional ns)
  "The NS text row's output, nhmac-ns-font-scaleS.EXT: in this lane's GLITNIR_GUI_OUT, or with NS, in the NS lane's
(Naglfar's lane writes to its naglfar/ subdirectory)."
  (let ((out (or (getenv "GLITNIR_GUI_OUT") temporary-file-directory)))
    (expand-file-name (format "nhmac-ns-font-scale%s.%s" (or (getenv "GLITNIR_GUI_SCALE") "1") ext)
                      (if ns (expand-file-name ".." out) out))))

(defmacro nhmac-gui--with-font-scene (&rest body)
  "Run BODY with the frame in Menlo 12, black on `nhmac-gui--font-bg', no cursor, mode line or fringes, showing a
blank line and then a line of ASCII; the frame's font and colours are put back after."
  (declare (indent 0))
  `(let ((font (frame-parameter nil 'font))
         (fg (frame-parameter nil 'foreground-color))
         (bg (frame-parameter nil 'background-color)))
     (unwind-protect
         (progn
           (set-frame-font "Menlo-12" nil (list (selected-frame)))
           (set-foreground-color "#000000")
           (set-background-color nhmac-gui--font-bg)
           (switch-to-buffer (generate-new-buffer " *nhmac-font*"))
           (setq-local cursor-type nil mode-line-format nil)
           (set-window-fringes nil 0 0)
           (insert "\n    Hello, world! 0123456789 {}[]()<>;:'\",./?\n")
           (redraw-frame)
           (redisplay t)
           (sit-for 0.5)
           ,@body)
       (kill-buffer)
       (set-window-fringes nil nil nil)
       (set-frame-font font nil (list (selected-frame)))
       (set-foreground-color fg)
       (set-background-color bg))))

(defun nhmac-gui--font-metrics ()
  "The default face's font's (ascent descent average-width space-width), from `font-info'."
  (let ((i (font-info (face-font 'default))))
    (list (aref i 8) (aref i 9) (aref i 11) (aref i 10))))

(defun nhmac-gui--capture (file)
  "The screen around the selected frame (GLITNIR_GUI_FRAME's corner, 900 by 700 points) as a BMP in FILE.
Device pixels, as the window server composited them; FILE."
  (let* ((g (mapcar #'string-to-number (split-string (or (getenv "GLITNIR_GUI_FRAME") "0 0"))))
         (r (format "%d,%d,900,700" (- (nth 0 g) 20) (- (nth 1 g) 60))))
    (when (file-exists-p file) (delete-file file))
    (call-process "screencapture" nil nil nil "-x" "-t" "bmp" "-R" r file)
    file))

(defun nhmac-gui--bmp-read (file)
  "FILE, a BMP of 24 or 32 bits per pixel, as (WIDTH HEIGHT PIXELS): PIXELS a vector of #xRRGGBB, rows top down."
  (with-temp-buffer
    (set-buffer-multibyte nil)
    (insert-file-contents-literally file)
    (let* ((s (buffer-string))
           (u (lambda (o n) (let ((v 0)) (dotimes (i n v) (setq v (logior v (ash (aref s (+ o i)) (* 8 i))))))))
           (off (funcall u 10 4)) (w (funcall u 18 4)) (h (funcall u 22 4))
           (bytes (/ (funcall u 28 2) 8))
           (down (> h #x7fffffff)) (h (if down (- #x100000000 h) h))
           (stride (* 4 (/ (+ (* w bytes) 3) 4)))
           (px (make-vector (* w h) 0)))
      (dotimes (y h)
        (let ((row (+ off (* stride (if down y (- h 1 y))))))
          (dotimes (x w)
            (let ((o (+ row (* x bytes))))
              (aset px (+ (* y w) x)
                    (logior (ash (aref s (+ o 2)) 16) (ash (aref s (+ o 1)) 8) (aref s o)))))))
      (list w h px))))

(defun nhmac-gui--far (a b tol)
  "Whether #xRRGGBB colours A and B differ by more than TOL in a channel."
  (cl-some (lambda (sh) (> (abs (- (logand (ash a (- sh)) 255) (logand (ash b (- sh)) 255))) tol)) '(16 8 0)))

(defun nhmac-gui--text-crop (img)
  "The text in IMG (from `nhmac-gui--bmp-read'): the commonest colour is the frame's background; inside the box of
the pixels near it (16 pixels in, past the window's rounded corners), the box of the pixels away from it.
Returns (W H PIXELS) of that box."
  (pcase-let* ((`(,w ,h ,px) img)
               (counts (make-hash-table))
               (bg (progn (mapc (lambda (p) (puthash p (1+ (gethash p counts 0)) counts)) px)
                          (let ((best nil) (n 0))
                            (maphash (lambda (k v) (when (> v n) (setq best k n v))) counts)
                            best)))
               (x0 w) (y0 h) (x1 -1) (y1 -1))
    (dotimes (y h)
      (dotimes (x w)
        (unless (nhmac-gui--far (aref px (+ (* y w) x)) bg 2)
          (setq x0 (min x0 x) y0 (min y0 y) x1 (max x1 x) y1 (max y1 y)))))
    (let ((tx0 w) (ty0 h) (tx1 -1) (ty1 -1))
      (cl-loop for y from (+ y0 16) to (- y1 16) do
               (cl-loop for x from (+ x0 16) to (- x1 16) do
                        (when (nhmac-gui--far (aref px (+ (* y w) x)) bg 2)
                          (setq tx0 (min tx0 x) ty0 (min ty0 y) tx1 (max tx1 x) ty1 (max ty1 y)))))
      (if (< tx1 0)
          (list 0 0 [])
        (let* ((cw (1+ (- tx1 tx0))) (ch (1+ (- ty1 ty0))) (out (make-vector (* cw ch) 0)))
          (dotimes (y ch)
            (dotimes (x cw)
              (aset out (+ (* y cw) x) (aref px (+ (* (+ ty0 y) w) tx0 x)))))
          (list cw ch out))))))

(ert-deftest nhmac-ns-font-dump ()
  "NS's Menlo 12: its metrics and a capture of its text, for nhmac-font-ascii in the Naglfar lane."
  (skip-unless (nhmac-gui--ns-build-p))
  (nhmac-gui--with-font-scene
    (with-temp-file (nhmac-gui--font-file "eld")
      (prin1 (nhmac-gui--font-metrics) (current-buffer)))
    (nhmac-gui--capture (nhmac-gui--font-file "bmp"))
    (should (file-exists-p (nhmac-gui--font-file "bmp")))
    (should (> (car (nhmac-gui--text-crop (nhmac-gui--bmp-read (nhmac-gui--font-file "bmp")))) 0))))

(ert-deftest nhmac-font-ascii ()
  "Menlo 12: `font-info''s ascent, descent, average width and space width are NS's times the scale (NS counts in
points, Naglfar in device pixels), and the text's pixels are within 1/255 of NS's, both captured from the screen."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((scale (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1"))))
    (should (file-exists-p (nhmac-gui--font-file "eld" t)))
    (nhmac-gui--with-font-scene
      (let ((ns (with-temp-buffer
                  (insert-file-contents (nhmac-gui--font-file "eld" t))
                  (read (current-buffer))))
            (mine (nhmac-gui--font-metrics)))
        (message "nhmac-font-ascii: scale %d: NS %S, Naglfar %S, stats %S" scale ns mine (nhmac-stats))
        (should (equal mine (mapcar (lambda (v) (* v scale)) ns))))
      (pcase-let* ((`(,nw ,nh ,npx) (nhmac-gui--text-crop (nhmac-gui--bmp-read (nhmac-gui--font-file "bmp" t))))
                   (`(,mw ,mh ,mpx) (nhmac-gui--text-crop
                                     (nhmac-gui--bmp-read (nhmac-gui--capture (nhmac-gui--font-file "bmp")))))
                   (worst 0) (off 0))
        (message "nhmac-font-ascii: text NS %dx%d, Naglfar %dx%d" nw nh mw mh)
        ;; an empty crop is a capture that missed the frame, never a size to compare
        (should (> nw 0))
        (should (> mw 0))
        (should (equal (list mw mh) (list nw nh)))
        (dotimes (i (length npx))
          (let ((a (aref npx i)) (b (aref mpx i)))
            (dolist (sh '(16 8 0))
              (let ((d (abs (- (logand (ash a (- sh)) 255) (logand (ash b (- sh)) 255)))))
                (setq worst (max worst d))
                (when (> d 1) (cl-incf off))))))
        (message "nhmac-font-ascii: worst channel difference %d, %d channels over 1" worst off)
        (should (<= worst 1))))))

(ert-deftest nhmac-font-open-close ()
  "Fonts open at many sizes and close when the font cache is cleared, again and again; each opens at its size and the
frame's text still reaches the surface."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((scale (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1"))))
    (dotimes (i 30)
      (let* ((pt (+ 9 (% i 7)))
             (info (font-info (format "Menlo-%d" pt))))
        (should info)
        (should (= (aref info 2) (* pt scale))))
      (when (= (% i 7) 6)
        (clear-font-cache)))
    (nhmac-gui--with-font-scene
      ;; some pixel of the text line is dark
      (let* ((edges (window-inside-pixel-edges))
             (y0 (+ (nth 1 edges) (frame-char-height)))
             (dark nil))
        (cl-loop for y from y0 below (+ y0 (frame-char-height)) until dark do
                 (cl-loop for x from (nth 0 edges) below (+ (nth 0 edges) (* 20 (frame-char-width))) until dark do
                          (let ((p (nhmac-gui--pixel-at x y)))
                            (when (and p (< (logand (ash p -8) 255) 64))
                              (setq dark (list x y p))))))
        (message "nhmac-font-open-close: a text pixel %S, stats %S" dark (nhmac-stats))
        (should dark)))))

;; Task 17: images through ImageIO
(defconst nhmac-gui--gif2
  (unibyte-string #x47 #x49 #x46 #x38 #x39 #x61 #x04 #x00 #x04 #x00 #x81 #x00 #x00 #xff #x00 #x00 #x00 #x00 #x00 #x00
                  #x00 #x00 #x00 #x00 #x00 #x21 #xff #x0b #x4e #x45 #x54 #x53 #x43 #x41 #x50 #x45 #x32 #x2e #x30 #x03
                  #x01 #x00 #x00 #x00 #x21 #xf9 #x04 #x00 #x0a #x00 #x00 #x00 #x2c #x00 #x00 #x00 #x00 #x04 #x00 #x04
                  #x00 #x00 #x08 #x09 #x00 #x01 #x08 #x1c #x48 #xb0 #x20 #x80 #x80 #x00 #x21 #xf9 #x04 #x01 #x0a #x00
                  #x01 #x00 #x2c #x00 #x00 #x00 #x00 #x04 #x00 #x04 #x00 #x81 #x00 #x00 #xff #x00 #x00 #x00 #x00 #x00
                  #x00 #x00 #x00 #x00 #x08 #x09 #x00 #x01 #x08 #x1c #x48 #xb0 #x20 #x80 #x80 #x00 #x3b)
  "A 4x4 GIF of two frames, red then blue, 0.1 s apart (made with Pillow).")

(defun nhmac-gui--image-files ()
  "The images both lanes show: test/data/image's, and a TIFF and a HEIC that sips makes from its JPEG."
  (let* ((dir (expand-file-name "../../data/image" (getenv "GLITNIR_GUI_TESTS")))
         (tmp (make-temp-file "nhmac-img" t))
         (files (directory-files dir t "\\.\\(png\\|jpg\\|gif\\|webp\\)\\'")))
    (dolist (fmt '("tiff" "heic"))
      (let ((out (expand-file-name (concat "black." fmt) tmp)))
        (call-process "sips" nil nil nil "-s" "format" fmt (expand-file-name "black.jpg" dir) "--out" out)
        (push out files)))
    (sort files #'string<)))

(defmacro nhmac-gui--with-image-scene (file &rest body)
  "Run BODY with FILE's image alone at the top left of a buffer on `nhmac-gui--font-bg', no cursor, mode line or
fringes; the frame's colours are put back after."
  (declare (indent 1))
  `(let ((bg (frame-parameter nil 'background-color)))
     (unwind-protect
         (progn
           (set-background-color nhmac-gui--font-bg)
           (switch-to-buffer (generate-new-buffer " *nhmac-img*"))
           (setq-local cursor-type nil mode-line-format nil)
           (set-window-fringes nil 0 0)
           (insert "\n    ")
           (insert-image (create-image ,file nil nil :scale 1))
           (redraw-frame)
           (redisplay t)
           (sit-for 0.5)
           ,@body)
       (kill-buffer)
       (set-window-fringes nil nil nil)
       (set-background-color bg))))

(ert-deftest nhmac-ns-images-dump ()
  "NS's sizes of the shared images, and at scale 1 a capture of each, for nhmac-images-native in the Naglfar lane."
  (skip-unless (nhmac-gui--ns-build-p))
  (let (sizes)
    (dolist (f (nhmac-gui--image-files))
      (push (cons (file-name-nondirectory f) (image-size (create-image f nil nil :scale 1) t)) sizes)
      (when (equal (getenv "GLITNIR_GUI_SCALE") "1")
        (nhmac-gui--with-image-scene f
          (nhmac-gui--capture (nhmac-gui--font-file (concat (file-name-nondirectory f) ".bmp"))))))
    (with-temp-file (nhmac-gui--font-file "images.eld")
      (prin1 sizes (current-buffer)))
    (should (= (length sizes) 8))))

(ert-deftest nhmac-images-native ()
  "PNG, JPEG, GIF (animated), TIFF, WebP and HEIC decode through ImageIO at NS's sizes; at scale 1 their pixels on
screen are within 2/255 of NS's in the same run (NS points are pixels there)."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (dolist (type '(png jpeg gif tiff webp heic))
    (should (image-type-available-p type)))
  (let ((ns (with-temp-buffer
              (insert-file-contents (nhmac-gui--font-file "images.eld" t))
              (read (current-buffer)))))
    (dolist (f (nhmac-gui--image-files))
      (let* ((name (file-name-nondirectory f))
             (size (image-size (create-image f nil nil :scale 1) t)))
        (message "nhmac-images-native: %s NS %S Naglfar %S" name (cdr (assoc name ns)) size)
        (should (equal size (cdr (assoc name ns))))
        (when (equal (getenv "GLITNIR_GUI_SCALE") "1")
          (nhmac-gui--with-image-scene f
            (pcase-let* ((`(,nw ,nh ,npx) (nhmac-gui--text-crop
                                          (nhmac-gui--bmp-read (nhmac-gui--font-file (concat name ".bmp") t))))
                         (`(,mw ,mh ,mpx) (nhmac-gui--text-crop
                                          (nhmac-gui--bmp-read
                                           (nhmac-gui--capture (nhmac-gui--font-file (concat name ".bmp"))))))
                         (worst 0))
              ;; an empty crop is a capture that missed the frame, never a size to compare
              (should (> nw 0))
              (should (> mw 0))
              (should (equal (list mw mh) (list nw nh)))
              (dotimes (i (length npx))
                (let ((a (aref npx i)) (b (aref mpx i)))
                  (dolist (sh '(16 8 0))
                    (setq worst (max worst (abs (- (logand (ash a (- sh)) 255) (logand (ash b (- sh)) 255))))))))
              (message "nhmac-images-native: %s %dx%d, worst channel difference %d" name mw mh worst)
              (should (<= worst 2))))))))
  ;; an animated GIF's frames, count and delay
  (let ((img (create-image nhmac-gui--gif2 'gif t :index 1 :scale 1)))
    (should (equal (image-metadata img) '(count 2 delay 0.1)))
    (switch-to-buffer (generate-new-buffer " *nhmac-gif*"))
    (setq-local cursor-type nil)
    (insert-image img)
    (redisplay t)
    (let ((xy (posn-x-y (posn-at-point (point-min))))
          (edges (window-inside-pixel-edges)))
      (should (= (nhmac-gui--pixel-at (+ (nth 0 edges) (car xy) 1) (+ (nth 1 edges) (cdr xy) 1)) #xff0000ff)))
    (kill-buffer)))

(defun nhmac-gui--image-screen (file name &optional worker)
  "FILE's image on screen (`nhmac-gui--text-crop' of a capture saved as NAME): loaded inline, or with WORKER
decoded on nh-img and landed first."
  (let ((nidhogg-workers-images (and worker t))
        (nidhogg-image-defer (and worker t)))
    (clear-image-cache t)
    (nhmac-gui--with-image-scene file
      (when worker
        (should (nidhogg-workers--test-await-landed 'nh-img 5000))
        (redisplay t)
        (should (= 0 (plist-get (nidhogg-workers--test-image-info) :pending))))
      (nhmac-gui--text-crop (nhmac-gui--bmp-read (nhmac-gui--capture (nhmac-gui--font-file name)))))))

(ert-deftest nhmac-images-worker ()
  "PNG, JPEG, GIF and WebP decoded off the main thread by the image worker (Glitnir's decoders, ImageIO behind them
and for JPEG) land and draw the pixels the inline load draws, within 2/255."
  (skip-unless (and (not (nhmac-gui--ns-build-p)) (equal (getenv "GLITNIR_GUI_SCALE") "1")))
  (should (fboundp 'nidhogg-workers--test-decide))
  (dolist (type '(png jpeg gif webp))
    (should (nidhogg-workers--deferrable-p type)))
  (let ((nidhogg-workers--mode 'threads))
    (dolist (f (nhmac-gui--image-files))
      (when (string-match-p "\\.\\(png\\|jpg\\|gif\\|webp\\)\\'" f)
        (let* ((name (file-name-nondirectory f))
               (inline (nhmac-gui--image-screen f (concat name ".inline.bmp")))
               (landed (plist-get (nidhogg-workers--test-image-info) :landed))
               (off (unwind-protect
                        (progn (nidhogg-workers--test-decide 'offload)
                               (nhmac-gui--image-screen f (concat name ".worker.bmp") t))
                      (nidhogg-workers--test-decide nil)))
               (worst 0))
          ;; the worker took it: one more landing
          (should (> (plist-get (nidhogg-workers--test-image-info) :landed) landed))
          (should (equal (take 2 off) (take 2 inline)))
          (dotimes (i (length (nth 2 inline)))
            (let ((a (aref (nth 2 inline) i)) (b (aref (nth 2 off) i)))
              (dolist (sh '(16 8 0))
                (setq worst (max worst (abs (- (logand (ash a (- sh)) 255) (logand (ash b (- sh)) 255))))))))
          (message "nhmac-images-worker: %s %dx%d, worst channel difference %d" name (car inline) (nth 1 inline)
                   worst)
          (should (<= worst 2)))))))

(ert-deftest nhmac-images-stand-in ()
  "While a zoomed image's decode is parked on the worker, the picture it replaces draws scaled into the new box (the
glue's stand-in), and the stand-in goes when the decode lands."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((file (expand-file-name "../../data/image/black.jpg" (getenv "GLITNIR_GUI_TESTS")))
         (old `(image :type jpeg :file ,file :scale 1))
         (new `(image :type jpeg :file ,file :scale 2))
         (nidhogg-workers-images t)
         (nidhogg-image-defer t)
         (nidhogg-workers--mode 'threads)
         (buf (generate-new-buffer " *nhmac-stand*"))
         (info (lambda (k) (plist-get (nidhogg-workers--test-image-info) k))))
    (unwind-protect
        (progn
          (nidhogg-workers--test-decide 'offload)
          (clear-image-cache t)
          (switch-to-buffer buf)
          (setq-local cursor-type nil)
          (face-remap-add-relative 'default :background "#0000ff")
          (insert (propertize "x" 'display old))
          (redisplay t)
          (while (> (funcall info :pending) 0)
            (should (nidhogg-workers--test-await-landed 'nh-img 5000))
            (redisplay t))
          (nidhogg-workers--pause-at 'after-header)
          (put-text-property 1 2 'display new)
          (redisplay t)
          (should (nidhogg-workers--test-await-paused 'after-header 5000 1))
          (should (= 1 (funcall info :pending)))
          (should (= 1 (funcall info :standing)))
          ;; a pixel past the old picture's size, inside the new box: the stand-in's black, not the blue background
          (let* ((size (image-size old t))
                 (xy (posn-x-y (posn-at-point (point-min))))
                 (edges (window-inside-pixel-edges))
                 (px (nhmac-gui--pixel-at (+ (nth 0 edges) (car xy) (round (* 1.5 (car size))))
                                          (+ (nth 1 edges) (cdr xy) (round (* 1.5 (cdr size)))))))
            (message "nhmac-images-stand-in: old %S, pixel %x" size px)
            (dolist (sh '(16 8 0))
              (should (< (logand (ash px (- sh)) 255) 16))))
          (nidhogg-workers--release 'after-header)
          (should (nidhogg-workers--test-await-landed 'nh-img 5000))
          (redisplay t)
          (should (= 0 (funcall info :pending)))
          (should (= 0 (funcall info :standing))))
      (nidhogg-workers--test-decide nil)
      (nidhogg-workers--release)
      (kill-buffer buf))))

(ert-deftest nhmac-images-builtin ()
  "Emacs's own decoders still draw: an XBM and a PBM reach the surface in the face's colours."
  :tags '(draws)
  (switch-to-buffer (generate-new-buffer " *nhmac-xbm*"))
  (setq-local cursor-type nil)
  (face-remap-add-relative 'default :foreground "#ff0000" :background "#0000ff")
  (let ((xbm (create-image (make-bool-vector 64 t) 'xbm t :data-width 8 :data-height 8 :scale 1))
        (pbm (create-image "P1\n4 4\n0 0 0 0\n0 0 0 0\n0 0 0 0\n0 0 0 0\n" 'pbm t :scale 1)))
    (insert-image xbm)
    (insert-image pbm)
    (redisplay t)
    ;; where each image sits, on NS (the reference) and on Naglfar alike, in the lane's log
    (nhmac-gui--say "nhmac-images-builtin: %s: edges %S line height %S; xbm size %S posn %S; pbm size %S posn %S"
                    (if (nhmac-gui--ns-build-p) "NS" "Naglfar") (window-inside-pixel-edges) (line-pixel-height)
                    (image-size xbm t) (posn-at-point (point-min)) (image-size pbm t) (posn-at-point (1+ (point-min))))
    (nhmac-gui--say "nhmac-images-builtin: messages: %S"
                    (with-current-buffer (messages-buffer)
                      (buffer-substring-no-properties (max (point-min) (- (point-max) 600)) (point-max)))))
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((xy (posn-x-y (posn-at-point (point-min))))
        (edges (window-inside-pixel-edges)))
    (let ((x (+ (nth 0 edges) (car xy))) (y (+ (nth 1 edges) (cdr xy))))
      (message "nhmac-images-builtin: xbm %x pbm %x" (nhmac-gui--pixel-at (+ x 2) (+ y 2))
               (nhmac-gui--pixel-at (+ x 10) (+ y 2)))
      ;; every XBM bit set: the foreground; every PBM pixel 0 (white in PBM's sense): the background.  The draws name
      ;; whether a wrong pixel is the container's (`:word') or the drawing's.
      (ert-info ((format "image draws %S at (%d %d)" (nhmac--image-draws) x y))
        (should (= (nhmac-gui--pixel-at (+ x 2) (+ y 2)) #xffff0000))
        (should (= (nhmac-gui--pixel-at (+ x 10) (+ y 2)) #xff0000ff)))))
  (kill-buffer))

(ert-deftest nhmac-images-transform ()
  "Scales, quarter turns and flips draw through the image's matrix: the sizes follow the spec, and an 8x8 XBM's lit
top row lands where the turn puts it, its pixels doubled (not smoothed) when scaled up."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (should (equal (image-transforms-p) '(scale rotate90)))
  (let ((png (expand-file-name "../../data/image/blank-100x200.png" (getenv "GLITNIR_GUI_TESTS"))))
    (should (equal (image-size (create-image png nil nil :scale 2) t) '(200 . 400)))
    (should (equal (image-size (create-image png nil nil :scale 1 :rotation 90) t) '(200 . 100)))
    (should (equal (image-size (create-image png nil nil :scale 1 :max-width 50) t) '(50 . 100))))
  (let ((bits (make-bool-vector 64 nil)))
    (dotimes (i 8) (aset bits i t))
    (switch-to-buffer (generate-new-buffer " *nhmac-xform*"))
    (setq-local cursor-type nil)
    (face-remap-add-relative 'default :foreground "#ff0000" :background "#0000ff")
    (insert-image (create-image bits 'xbm t :data-width 8 :data-height 8 :scale 2))
    (insert-image (create-image bits 'xbm t :data-width 8 :data-height 8 :scale 1 :rotation 90))
    (insert-image (create-image bits 'xbm t :data-width 8 :data-height 8 :scale 1 :rotation 270 :flip t))
    (redisplay t)
    (let* ((xy (posn-x-y (posn-at-point (point-min))))
           (edges (window-inside-pixel-edges))
           (x (+ (nth 0 edges) (car xy)))
           (y (+ (nth 1 edges) (cdr xy)))
           (px (lambda (dx dy) (nhmac-gui--pixel-at (+ x dx) (+ y dy)))))
      (message "nhmac-images-transform: scaled %x %x, 90 %x %x, 270 flipped %x %x"
               (funcall px 3 1) (funcall px 3 2) (funcall px 23 3) (funcall px 16 3)
               (funcall px 31 3) (funcall px 24 3))
      ;; scaled 2: rows 0 and 1 lit, row 2 not
      (should (= (funcall px 3 1) #xffff0000))
      (should (= (funcall px 3 2) #xff0000ff))
      ;; turned 90 degrees clockwise (from x 16): the top row is the right column
      (should (= (funcall px 23 3) #xffff0000))
      (should (= (funcall px 16 3) #xff0000ff))
      ;; turned 270 (the top row to the left column), then flipped (from x 24): the right column again
      (should (= (funcall px 31 3) #xffff0000))
      (should (= (funcall px 24 3) #xff0000ff))))
  (kill-buffer))

(defun nhmac-gui--image-pixels (img &rest at)
  "Insert IMG alone in a buffer on blue, no cursor, and the pixels at each (DX . DY) from its top left."
  (switch-to-buffer (generate-new-buffer " *nhmac-px*"))
  (setq-local cursor-type nil)
  (face-remap-add-relative 'default :foreground "#ffffff" :background "#0000ff")
  (insert-image img)
  (redisplay t)
  (prog1 (let* ((xy (posn-x-y (posn-at-point (point-min))))
                (edges (window-inside-pixel-edges))
                (x (+ (nth 0 edges) (car xy)))
                (y (+ (nth 1 edges) (cdr xy))))
           (mapcar (lambda (d) (nhmac-gui--pixel-at (+ x (car d)) (+ y (cdr d)))) at))
    (kill-buffer)))

(ert-deftest nhmac-images-svg ()
  "SVG through resvg with svg_load_image's keywords: the size from the image and :scale, currentColor the
:foreground over the :background, the image's :css after Emacs's, and the splash screen's SVG."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (should (image-type-available-p 'svg))
  (let ((svg "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"4\">\
<rect width=\"4\" height=\"4\" fill=\"currentColor\"/></svg>"))
    (should (equal (image-size (create-image svg 'svg t :scale 1) t) '(8 . 4)))
    (should (equal (image-size (create-image svg 'svg t :scale 2) t) '(16 . 8)))
    (let ((px (nhmac-gui--image-pixels
               (create-image svg 'svg t :scale 1 :foreground "#ff0000" :background "#00ff00")
               '(1 . 1) '(6 . 1))))
      (message "nhmac-images-svg: currentColor %x background %x" (nth 0 px) (nth 1 px))
      (should (equal px '(#xffff0000 #xff00ff00))))
    (let ((px (nhmac-gui--image-pixels (create-image svg 'svg t :scale 1 :css "rect{fill:#ffff00}") '(1 . 1))))
      (should (equal px '(#xffffff00)))))
  (let ((splash (find-image '((:file "splash.svg" :type svg)))))
    (should splash)
    (should (> (car (image-size splash t)) 0))))

(ert-deftest nhmac-images-xpm ()
  "XPM through the glue's reader: the splash screen's XPM at the size its header gives, and None transparent over
the background beside a named colour."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (should (image-type-available-p 'xpm))
  (let* ((file (image-search-load-path "splash.xpm"))
         (head (with-temp-buffer
                 (insert-file-contents file)
                 (re-search-forward "\"\\([0-9]+\\) \\([0-9]+\\) ")
                 (cons (string-to-number (match-string 1)) (string-to-number (match-string 2))))))
    (should (equal (image-size (create-image file 'xpm nil :scale 1) t) head)))
  (let* ((xpm "/* XPM */\nstatic char *t[] = {\n\"4 2 2 1\",\n\"  c None\",\n\". c red\",\n\"..  \",\n\"..  \"};")
         (px (nhmac-gui--image-pixels (create-image xpm 'xpm t :scale 1) '(0 . 0) '(3 . 1))))
    (message "nhmac-images-xpm: red %x None %x" (nth 0 px) (nth 1 px))
    (should (equal px '(#xffff0000 #xff0000ff)))))

(ert-deftest nhmac-images-image-tests ()
  "test/src/image-tests.el passes on Naglfar."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (load (expand-file-name "../image-tests" (getenv "GLITNIR_GUI_TESTS")) nil t)
  (let ((stats (ert-run-tests "\\`image-tests-" #'ignore)))
    (message "nhmac-images-image-tests: %d run, %d unexpected, %d skipped"
             (ert-stats-completed stats) (ert-stats-completed-unexpected stats) (ert-stats-skipped stats))
    (should (> (ert-stats-completed stats) 0))
    (should (zerop (ert-stats-completed-unexpected stats)))))

(defconst nhmac-gui--image-lib-re
  (concat "/lib\\(?:" (regexp-opt '("png" "jpeg" "turbojpeg" "gif" "tiff" "webp" "sharpyuv" "rsvg" "cairo" "pango"
                                    "heif" "Magick" "pixman" "Xpm"))
          "\\)[^/]*\\.dylib")
  "An image library's file in an `otool -L' line (the image tests' libraries, never gnutls or tree-sitter).")

(defun nhmac-gui--outside-image-libs (lines)
  "LINES of `otool -L' that name an image library outside /System and /usr/lib."
  (seq-filter (lambda (l) (and (string-match-p nhmac-gui--image-lib-re l)
                               (not (string-match-p "\\`[ \t]*/\\(System\\|usr/lib\\)/" l))))
              lines))

(ert-deftest nhmac-images-no-homebrew ()
  "The binary links no image library outside the system: otool -L names none outside /System and /usr/lib.
The filter checks itself first on a Homebrew libpng line (caught), the system ImageIO and a Homebrew gnutls (not)."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (should (equal (nhmac-gui--outside-image-libs
                  '("\t/opt/homebrew/opt/libpng/lib/libpng16.16.dylib (compatibility version 61.0.0)"
                    "\t/System/Library/Frameworks/ImageIO.framework/Versions/A/ImageIO (compatibility version 1.0.0)"
                    "\t/opt/homebrew/opt/gnutls/lib/libgnutls.30.dylib (compatibility version 73.0.0)"))
                 '("\t/opt/homebrew/opt/libpng/lib/libpng16.16.dylib (compatibility version 61.0.0)")))
  (let* ((bin (expand-file-name invocation-name invocation-directory))
         (libs (cdr (process-lines "otool" "-L" bin))))
    (message "nhmac-images-no-homebrew: %s links %S" bin libs)
    (should libs)
    (should-not (nhmac-gui--outside-image-libs libs))))

;;; nhmac-gui-tests.el ends here
