;;; nhmac-gui-tests.el --- Naglfar's GUI rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh on the macOS runner.  Rows are appended under ";; Task N:" headings.

(require 'ert)
(require 'cl-lib)
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
