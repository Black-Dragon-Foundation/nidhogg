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

;; The interposer and the counters harness
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
      (nhmac-gui--say "nhmac-counts: 20 keys: locks %d bytes %d spins %d spins-in-redisplay %d"
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

;; The pumps and the take-out scope
(ert-deftest nhmac-reentry-sync-delegates ()
  "AppKit's delegates, called back from inside the shell's own calls, reach its state only through the scope.
`setFrame:' and `makeKeyAndOrderFront:' with the scope free are applied at once; inside a borrow (with a modal session
and a nested event loop) they queue, and each queued event is applied once when the lend ends; a deferred effect runs
with the borrow put back.  No two borrows are ever live."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((before (nhmac-stats))
         (r (nhmac--test-reentry))
         (after (nhmac-stats)))
    (nhmac-gui--say "nhmac-reentry: %S" r)
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
  ;; a frame deleted with no pump after it (at scale 2, its window's callbacks queue outside a lend): what waits from
  ;; before the session is not the session's
  (delete-frame (make-frame '((width . 20) (height . 5))) t)
  (let* ((panics (length (nhwl-glue-panics)))
         (r (nhmac--modal-resize 300)))
    (nhmac-gui--say "nhmac-modal-resize: %S" r)
    (should (>= (plist-get r :rounds) 2))
    (should (>= (plist-get r :takes) 1))
    (should (= (plist-get r :overlapping-takes) 0))
    (should (= (plist-get r :queued) (plist-get r :applied)))
    (should (= (length (nhwl-glue-panics)) panics))
    (should (equal (eval '(+ 1 1) t) 2))))

;; Keys and mouse intake
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

;; The wait: kqueue in the run loop, the helper, keys before process output
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
    (nhmac-gui--say "nhmac-kq-probe: %s %S" p (nhmac--test-kq-probe p)))
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
    (maphash (lambda (k v) (nhmac-gui--say "nhmac-kq-fds: %s %d lines, longest gap %.3f s" k (car v) (cdr v))) gaps)
    (nhmac-gui--say "nhmac-kq-fds: stats %S" (nhmac-stats))
    (maphash (lambda (_k v)
               (should (= (car v) 100))
               (should (< (cdr v) 1.0)))
             gaps)
    (should (>= helper-fds 1))
    (should (> (nhmac-gui--delta before :kq-wakes) 0))
    (should (= (nhmac-gui--delta before :empty-wakes) 0))))

(ert-deftest nhmac-kq-layers ()
  "No layer under the wait loses a readiness: the kernel's kqueue, directly and through a second kqueue (the run
loop's source's shape), on a pipe and on a pty (0 misses in 2000 bytes each); and the whole wait, with one pipe or
one pty alone, never sleeps through output (no gap near its 2 s timeout, three rounds each)."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((layers (nhmac--test-kq-layers 2000))
        (gaps (make-hash-table :test #'equal)))
    (nhmac-gui--say "nhmac-kq-layers: kernel %S" layers)
    (dotimes (i 3)
      (dolist (type '(pipe pty))
        (let ((p (nhmac-gui--feed (format "kq-%s-%d" type i) type gaps))
              (start (float-time)))
          (unwind-protect
              (while (and (process-live-p p) (< (- (float-time) start) 20))
                (accept-process-output nil 2))
            (delete-process p)))))
    (maphash (lambda (k v) (nhmac-gui--say "nhmac-kq-layers: %s %d lines, longest gap %.3f s" k (car v) (cdr v)))
             gaps)
    (should (equal layers '(:pipe-direct 0 :pipe-nested 0 :pty-direct 0 :pty-nested 0)))
    (maphash (lambda (_k v)
               (should (= (car v) 100))
               (should (< (cdr v) 1.0)))
             gaps)))

(ert-deftest nhmac-keys-before-output ()
  "Under a `yes' flood, 50 keys posted one at a time each reach Lisp at once and in order: the pump that begins
every wait stores a key before the wait reads the flood, so keys never wait behind output, and none takes the
helper's hop (`:helper-hops-keys' 0)."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  ;; each phase's seconds, so a slow row names the phase it spent them in
  (let ((phases nil) (t0 (float-time)))
    (nhmac-gui--flush)
    (push (cons 'flush (- (float-time) t0)) phases)
    (let* ((bytes 0)
           (before (nhmac-stats))
           (keys (append "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWX" nil))
           (proc (make-process :name "nhmac-flood" :command '("yes") :connection-type 'pipe :noquery t
                               :filter (lambda (_p s) (setq bytes (+ bytes (length s))))))
           (worst 0.0)
           got)
      (unwind-protect
          (progn
            (setq t0 (float-time))
            ;; not (accept-process-output proc ...): waiting on the flood itself reads on, without a deadline, until
            ;; its pipe is empty, which a `yes' outrunning the filter leaves it only by chance
            (while (and (= bytes 0) (< (- (float-time) t0) 2))
              (accept-process-output nil 0.05))
            (push (cons 'first-output (- (float-time) t0)) phases)
            (should (> bytes 0))
            (setq t0 (float-time))
            (dolist (c keys)
              (nhmac--test-post-key c)
              (let ((t1 (float-time)))
                (push (read-event nil nil 2) got)
                (setq worst (max worst (- (float-time) t1)))))
            (push (cons 'keys (- (float-time) t0)) phases))
        (setq t0 (float-time))
        (delete-process proc)
        (push (cons 'delete (- (float-time) t0)) phases))
      (nhmac-gui--say "nhmac-keys-before-output: worst %.3f s, %d bytes of output, phases %S" worst bytes
                      (nreverse phases))
      (should (equal (nreverse got) keys))
      (should (< worst 0.5))
      (should (= (nhmac-gui--delta before :helper-hops-keys) 0))
      (should (> (nhmac-gui--delta before :waits) 0)))))

;; The lock hand-off
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
    (nhmac-gui--say "nhmac-lock-handoff: %d rounds, %d lock waits over 2 ms; stats %S"
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
    (nhmac-gui--say "nhmac-quit-reaches-busy-loop: quit %S after %.2f s" quit (- (float-time) start))
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
    (nhmac-gui--say "nhmac-thread-wakes-idle-main: woken %.3f s after the notify" (- woken notified))
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
          (nhmac-gui--say "nhmac-objc-exception-poisons: child %S %S: %s" (process-status proc) (process-exit-status proc)
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

;; The IOSurface ring, damaged rectangles, the commit
(defconst nhmac-gui--ring 3 "The ring's size (surface.rs `RING'): the oldest age a slot copies for.")

(defun nhmac-gui--surface (&optional frame)
  "The surface facts of FRAME's window, a plist (`nhmac--test-surface')."
  (nhmac--test-surface 0 0 frame))

(defun nhmac-gui--row-bytes ()
  "Bytes of one full-width text line of the selected frame."
  (* 4 (frame-pixel-width) (frame-char-height)))

(defconst nhmac-gui--key-repeat 0.03
  "Seconds between typed keys at macOS's fastest key repeat: longer than a display frame.")

(defun nhmac-gui--slots ()
  "The surfaces the selected frame's ring has made."
  (plist-get (nhmac-gui--surface) :slots))

(defun nhmac-gui--redisplay ()
  "Redisplay, then wait (bounded) until the commit pacer holds none of the selected frame's commits: what the
redisplay drew is on the layer, and counted."
  (redisplay)
  (nhmac-gui--wait-unheld 1))

(defun nhmac-gui--fill-ring ()
  "Commit back to back until the ring has made all its slots, at most 500 commits; the slots it has.
A slot is made only while the window server holds the others, which commits faster than it composites make it do.
Each commit is waited for when the pacer held it, so each is drawn on its own.
At least the ring's size and one more small commits are made, so no slot's age reaches back to the drawing before
\(a new buffer's whole window).  The text is left as it was."
  (let ((n 0))
    (while (and (< n 500)
                (or (< n (* 2 (1+ nhmac-gui--ring))) (< (nhmac-gui--slots) nhmac-gui--ring)))
      (insert "w")
      (nhmac-gui--redisplay)
      (delete-char -1)
      (nhmac-gui--redisplay)
      (setq n (+ n 2)))
    (nhmac-gui--slots)))

(defun nhmac-gui--typing-buffer (text)
  "A fresh buffer showing TEXT in the selected window, point at its end, drawn."
  (switch-to-buffer (generate-new-buffer " *nhmac-typing*"))
  (insert text)
  (redisplay t)
  ;; the ring's slots are made first (whole copies); typing is measured after them
  (nhmac-gui--fill-ring))

(defun nhmac-gui--type (n &optional before pace)
  "Type N characters as commands, each followed by a redisplay; the stats' changes since BEFORE (now if nil).
With PACE, keys come at the fastest key repeat, the wait before each one turning the run loop, as typing does;
without, back to back."
  (let ((before (or before (nhmac-stats))))
    (dotimes (_ n)
      (when pace
        (accept-process-output nil nhmac-gui--key-repeat))
      (execute-kbd-macro "x")
      ;; paced, as typing is: a held commit lands before the next key; back to back, the pacer may merge them
      (if pace (nhmac-gui--redisplay) (redisplay)))
    (let ((after (nhmac-stats)))
      (cl-loop for (k v) on after by #'cddr
               when (numberp v) append (list k (- v (or (plist-get before k) 0)))))))

;; The ring's figures follow when the window server lets go of a surface, which it does on schedule only for a window
;; it composites: an occluded lane cannot judge them, and says so instead of giving a number.
(defun nhmac-gui--require-composited ()
  "Fail the running row when the selected frame's window is occluded."
  (when (eq (plist-get (nhmac--window-info) :occluded) t)
    (ert-fail (list "window occluded, can't judge" (nhmac--window-info)))))

(ert-deftest nhmac-typing-bytes ()
  "Typing copies only damage into the surfaces: per key at most the damage times the ring's age, a few lines at
most, in the three §5.4 cases (end of line, mid-line, the worst plain key); never a whole surface.  The keys come
at the fastest key repeat, on a ring that has made all its slots."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--require-composited)
  (let ((row (nhmac-gui--row-bytes))
        (keys 10))
    (pcase-dolist (`(,case ,text ,goto)
                   `((end-of-line "short line" ,#'end-of-buffer)
                     (mid-line ,(make-string 70 ?m) ,(lambda () (goto-char (+ (point-min) 35))))
                     (worst ,(make-string (- (window-body-width) 2) ?w) ,#'end-of-buffer)))
      (nhmac-gui--typing-buffer text)
      ;; a slot made while the keys are measured would be a whole copy
      (should (= (nhmac-gui--slots) nhmac-gui--ring))
      (funcall goto)
      (redisplay)
      (let* ((d (nhmac-gui--type keys nil t))
             (bytes (plist-get d :bytes-copied))
             (damage (plist-get d :damage-bytes)))
        (nhmac-gui--say "nhmac-typing-bytes: %s: %d keys: copied %d (%d per key), damage %d, full %d, row %d"
                 case keys bytes (/ bytes keys) damage (plist-get d :full-copies) row)
        (should (= (plist-get d :full-copies) 0))
        (should (= (plist-get d :held-timeouts) 0))
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
    (nhmac-gui--say "nhmac-commits-per-redisplay: 20 keys: %d commits" (plist-get d :commits))
    (should (<= (plist-get d :commits) 20))
    (should (>= (plist-get d :commits) 1)))
  (kill-buffer))

(ert-deftest nhmac-no-spins-in-redisplay ()
  "Redisplay turns no run loop: 0 passes of the main run loop outside the pumps while typing (NS's figure, from the
interposer in the NS lane of the same run, is logged by `nhmac-interposer-counts-ns')."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer "spins")
  (let ((d (nhmac-gui--type 20)))
    (nhmac-gui--say "nhmac-no-spins-in-redisplay: 20 keys: %d spins outside the pumps" (plist-get d :spins-in-redisplay))
    (should (= (plist-get d :spins-in-redisplay) 0)))
  (kill-buffer))

(ert-deftest nhmac-no-implicit-transaction ()
  "No implicit Core Animation transaction is left open when a publish ends, while typing."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer "implicit")
  (should (= (plist-get (nhmac-gui--type 20) :implicit-open-at-update-end) 0))
  (kill-buffer))

(ert-deftest nhmac-key-to-commit-counts ()
  "A key posted to the window reaches Lisp through one pump, with no fd wait, and its redisplay commits once.
Each key comes once the window server has handed back a surface, as a typed key does a frame or more after the last:
a key posted while it still composites the last commits is held by the pacer, whose release is the run loop's
\(`nhmac-surface-busy-burst' is that row)."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer "key")
  ;; the blinking cursor's timer redraws the cursor on its own (one commit of the cursor's cell); the row counts the
  ;; key's commits
  (let ((blink blink-cursor-mode))
    (blink-cursor-mode -1)
    (unwind-protect
        (progn
          ;; one key first, not counted: a window that is not key yet becomes key at it, and its cursor's redraw
          ;; (hollow to filled) is a commit of its own
          (nhmac--test-post-key ?k)
          (should (eq (read-event nil nil 2) ?k))
          (redisplay t)
          (nhmac-gui--flush)
          (dotimes (i 5)
            (let* ((free (nhmac-gui--wait-free 2))
                   (before (progn (nhmac--pump-trace) (nhmac-stats))))
              (should free)
              (nhmac--test-post-key ?k)
              (let ((ev (read-event nil nil 2)))
                (should (eq ev ?k))
                (insert ev))
              ;; a commit the pacer held is published by its release, without a pump
              (nhmac-gui--redisplay)
              (let ((pumps (nhmac-gui--delta before :pumps))
                    (commits (nhmac-gui--delta before :commits))
                    (sleeps (nhmac-gui--delta before :sleeps)))
                (nhmac-gui--say "nhmac-key-to-commit-counts: key %d: pumps %d commits %d fd waits %d (a surface \
free after %.3f s)"
                                i pumps commits sleeps free)
                ;; who asked for each pump, and each commit's damage
                (ert-info ((format "pump decisions %S" (nhmac--pump-trace)))
                  (should (= pumps 1))
                  (should (= commits 1))
                  (should (= sleeps 0))))))
          (nhmac-gui--flush))
      (blink-cursor-mode (if blink 1 -1))
      (kill-buffer))))

(ert-deftest nhmac-surface-busy-typing ()
  "While typing (keys at the fastest key repeat) the ring always has a surface the window server does not hold."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--require-composited)
  (nhmac-gui--typing-buffer "busy")
  (let ((d (nhmac-gui--type 30 nil t)))
    (nhmac-gui--say "nhmac-surface-busy-typing: 30 keys: %d busy, %d acquires" (plist-get d :surface-busy)
                    (plist-get d :ring-acquires))
    (should (= (plist-get d :surface-busy) 0))
    (should (= (plist-get d :held-timeouts) 0)))
  (kill-buffer))

(defun nhmac-gui--wait-unheld (secs)
  "Wait as Emacs waits for input until the commit pacer holds no commit of the selected frame, at most SECS; whether
it does not.  The pacer's request for the publish comes as a shell event, which Emacs reads where it reads keys (here
`input-pending-p', without a redisplay that would publish anyway)."
  (let ((end (+ (float-time) secs)))
    (while (and (plist-get (nhmac-gui--surface) :held) (< (float-time) end))
      (input-pending-p)
      (accept-process-output nil 0.01))
    (not (plist-get (nhmac-gui--surface) :held))))

(defun nhmac-gui--wait-free (secs)
  "Wait as Emacs waits for input until the selected frame's next commit would find a surface the window server does
not hold, at most SECS; how many seconds that took, or nil if it never did."
  (let ((start (float-time)))
    (while (and (not (plist-get (nhmac-gui--surface) :free)) (< (- (float-time) start) secs))
      (input-pending-p)
      (accept-process-output nil 0.01))
    (and (plist-get (nhmac-gui--surface) :free) (- (float-time) start))))

(ert-deftest nhmac-surface-busy-burst ()
  "Redisplays back to back, faster than the window server composites (a keyboard macro, edits streaming in): the
commit pacer holds a commit while every other surface is held, so no held surface is drawn into (0 busy), and no hold
reaches its bound on a composited window (0 held timeouts)."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--require-composited)
  (nhmac-gui--typing-buffer "burst")
  (let ((d (nhmac-gui--type 30)))
    (nhmac-gui--say "nhmac-surface-busy-burst: 30 keys back to back: %d busy, %d held, %d held timeouts, %d acquires, %d full copies"
                    (plist-get d :surface-busy) (plist-get d :held) (plist-get d :held-timeouts)
                    (plist-get d :ring-acquires) (plist-get d :full-copies))
    (should (> (plist-get d :ring-acquires) 0))
    (should (= (plist-get d :surface-busy) 0))
    (should (= (plist-get d :held-timeouts) 0)))
  (should (nhmac-gui--wait-unheld 1))
  (kill-buffer))

(defun nhmac-gui--hold-one ()
  "Make the commit pacer hold a commit of the selected frame: every surface reads as held while a character is drawn.
The surfaces read as free again when it returns; the held commit is owed."
  (nhmac--test-force-in-use t)
  (unwind-protect
      (progn
        (insert "x")
        (redisplay))
    (nhmac--test-force-in-use nil))
  (should (plist-get (nhmac-gui--surface) :held)))

(ert-deftest nhmac-burst-last-frame-shows ()
  "A burst's last commit, held while the window server held the other surfaces, is published once one frees, while
Emacs sits in its wait (the pacer's timer asks for it): the layer then shows the newest surface, and the hold ended
without reaching its bound."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer "last")
  (let ((before (nhmac-stats)))
    (nhmac-gui--hold-one)
    (let ((shown (nhmac-gui--wait-unheld 1)) (s (nhmac-gui--surface)))
      (nhmac-gui--say "nhmac-burst-last-frame-shows: unheld %S, %d held, %d held timeouts, %d fires, surface %S"
                      shown (nhmac-gui--delta before :held) (nhmac-gui--delta before :held-timeouts)
                      (nhmac-gui--delta before :pacer-fires) s)
      (should shown)
      (should (plist-get s :contents-is-newest))
      (should (= (nhmac-gui--delta before :held) 1))
      (should (= (nhmac-gui--delta before :held-timeouts) 0))
      (should (= (nhmac-gui--delta before :surface-busy) 0))))
  (kill-buffer))

(ert-deftest nhmac-pacer-idle-disarmed ()
  "After a held commit shows, the pacer's timer is disarmed: it was armed for the hold (its release may come from a
pump's check before the timer fires), and does not fire over 2 s of idle after."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer "idle")
  (let ((before (nhmac-stats)))
    (nhmac-gui--hold-one)
    (should (nhmac-gui--wait-unheld 1))
    ;; the check that finds nothing held disarms it: one look after the commit shows
    (accept-process-output nil 0.05)
    (should (> (nhmac-gui--delta before :pacer-arms) 0)))
  (let ((before (nhmac-stats)))
    (accept-process-output nil 2)
    (nhmac-gui--say "nhmac-pacer-idle-disarmed: %d fires over 2 s idle" (nhmac-gui--delta before :pacer-fires))
    (should (= (nhmac-gui--delta before :pacer-fires) 0)))
  (kill-buffer))

(ert-deftest nhmac-held-commit-bound ()
  "A commit held past the pacer's bound (every surface forced to read as held throughout) is drawn into the oldest
anyway, counted as a held timeout and a busy surface, so a stuck window server never freezes the frame."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--typing-buffer "bound")
  (let ((before (nhmac-stats)) (t0 (float-time)) shown)
    (nhmac--test-force-in-use t)
    (unwind-protect
        (progn
          (insert "x")
          (redisplay)
          (should (plist-get (nhmac-gui--surface) :held))
          (setq shown (nhmac-gui--wait-unheld 1)))
      (nhmac--test-force-in-use nil))
    (nhmac-gui--say "nhmac-held-commit-bound: unheld %S after %.3f s, %d held, %d held timeouts, %d busy" shown
                    (- (float-time) t0) (nhmac-gui--delta before :held) (nhmac-gui--delta before :held-timeouts)
                    (nhmac-gui--delta before :surface-busy))
    (should shown)
    (should (>= (- (float-time) t0) 0.05))
    (should (= (nhmac-gui--delta before :held-timeouts) 1))
    (should (= (nhmac-gui--delta before :surface-busy) 1)))
  (kill-buffer))

(ert-deftest nhmac-typing-fills-the-ring ()
  "The typing rows measure on a ring that has made all its slots: right after the ring starts over (the colour space
changed) it has fewer, which their check sees, and the warm-up fills it."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--require-composited)
  (let ((srgb ns-use-srgb-colorspace))
    (switch-to-buffer (generate-new-buffer " *nhmac-fill*"))
    (insert "fill")
    (unwind-protect
        (progn
          (setq ns-use-srgb-colorspace (not srgb))
          (redraw-frame)
          (redisplay t)
          (let ((short (nhmac-gui--slots)))
            (nhmac-gui--say "nhmac-typing-fills-the-ring: %d slots after the ring started over" short)
            (should (< short nhmac-gui--ring)))
          (let ((filled (nhmac-gui--fill-ring)))
            (nhmac-gui--say "nhmac-typing-fills-the-ring: %d slots after the warm-up" filled)
            (should (= filled nhmac-gui--ring))))
      (setq ns-use-srgb-colorspace srgb)
      (redraw-frame)
      (redisplay t)
      (kill-buffer))))

(ert-deftest nhmac-scroll-bytes ()
  "Scrolling a window by a line copies the scrolled rows, not the surface."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (nhmac-gui--require-composited)
  (nhmac-gui--typing-buffer (mapconcat #'number-to-string (number-sequence 1 300) "\n"))
  (goto-char (point-min))
  (redisplay t)
  ;; every slot made before the count (an earlier row may leave the ring with fewer): a slot made lazily is a full copy
  (nhmac-gui--fill-ring)
  (dotimes (_ nhmac-gui--ring) (scroll-up 1) (nhmac-gui--redisplay))
  (let* ((before (nhmac-stats))
         (_ (progn (scroll-up 1) (nhmac-gui--redisplay)))
         (bytes (nhmac-gui--delta before :bytes-copied))
         (rows (* 4 (frame-pixel-width) (window-body-height nil t))))
    (nhmac-gui--say "nhmac-scroll-bytes: one line: copied %d, the window's rows %d" bytes rows)
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
      (nhmac-gui--say "nhmac-commit-shows-without-spin: %S" s)
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
with a sign test.
Both builds are timed to the same point: the change handed to the window server, the flush to the screen left out.
NS's flush comes after its redisplay (Core Animation's commit at the end of the run-loop turn), so its time is the
key's command and redisplay; Naglfar commits and flushes its layer transaction inside the redisplay, so that
redisplay's commit time (`nhmac-stats' :commit-ns) is taken off."
  (let* ((naglfar (not (nhmac-gui--ns-build-p)))
         (build (if naglfar "naglfar" "ns"))
         (file (expand-file-name (format "pair-%s.txt" build)
                                 (or (getenv "GLITNIR_GUI_OUT") temporary-file-directory)))
         (commit-ns (lambda () (if naglfar (plist-get (nhmac-stats) :commit-ns) 0)))
         times)
    (switch-to-buffer (generate-new-buffer " *nhmac-pair*"))
    (insert "paired timing")
    (redisplay t)
    (dotimes (_ 10) (execute-kbd-macro "w") (redisplay))
    (dotimes (_ 50)
      (let* ((c0 (funcall commit-ns)) (t0 (float-time)))
        (execute-kbd-macro "x")
        (redisplay)
        (let ((t1 (float-time)))
          (push (round (- (* 1e6 (- t1 t0)) (/ (- (funcall commit-ns) c0) 1000.0))) times))))
    (with-temp-buffer
      (dolist (us (nreverse times)) (insert (format "%d\n" us)))
      (write-region (point-min) (point-max) file t 'silent))
    (should (file-exists-p file))
    (kill-buffer)))

;; Where the paired row's time goes (the M1 gate's paired timing): the same 50 keys, each split into the command and
;; the redisplay, and on Naglfar the redisplay's shell stages from `nhmac-stats' (painting the draw records, filling
;; the surfaces, committing and flushing the layer transaction); the rest of the redisplay is C and the glue.
(defun nhmac-gui--median (xs)
  "The median of the numbers XS."
  (let ((v (vconcat (sort (copy-sequence xs) #'<)))) (aref v (/ (length v) 2))))

(ert-deftest nhmac-pair-stages ()
  "The paired row's keys split by stage, medians in microseconds, written to GLITNIR_GUI_OUT/stages-BUILD.txt.
It measures; it asserts only that it ran."
  (let* ((naglfar (not (nhmac-gui--ns-build-p)))
         (build (if naglfar "naglfar" "ns"))
         (file (expand-file-name (format "stages-%s.txt" build)
                                 (or (getenv "GLITNIR_GUI_OUT") temporary-file-directory)))
         (us (lambda (a b) (/ (- b a) 1000.0)))
         cmd redisp paint fill commit)
    (switch-to-buffer (generate-new-buffer " *nhmac-pair*"))
    (insert "paired timing")
    (redisplay t)
    (dotimes (_ 10) (execute-kbd-macro "w") (redisplay))
    (dotimes (_ 50)
      (let* ((s0 (and naglfar (nhmac-stats))) (t0 (float-time)))
        (execute-kbd-macro "x")
        (let ((t1 (float-time)))
          (redisplay)
          (let ((t2 (float-time)))
            (push (* 1e6 (- t1 t0)) cmd)
            (push (* 1e6 (- t2 t1)) redisp)
            (when naglfar
              (let ((s1 (nhmac-stats)))
                (push (funcall us (plist-get s0 :paint-ns) (plist-get s1 :paint-ns)) paint)
                (push (funcall us (plist-get s0 :fill-ns) (plist-get s1 :fill-ns)) fill)
                (push (funcall us (plist-get s0 :commit-ns) (plist-get s1 :commit-ns)) commit)))))))
    (let ((line (if naglfar
                    (format "%s: command %.0f redisplay %.0f (paint %.0f fill %.0f commit %.0f, the rest %.0f) us"
                            build (nhmac-gui--median cmd) (nhmac-gui--median redisp) (nhmac-gui--median paint)
                            (nhmac-gui--median fill) (nhmac-gui--median commit)
                            (nhmac-gui--median (cl-mapcar (lambda (r p f c) (- r p f c)) redisp paint fill commit)))
                  (format "%s: command %.0f redisplay %.0f us" build (nhmac-gui--median cmd)
                          (nhmac-gui--median redisp)))))
      (nhmac-gui--say "nhmac-pair-stages: %s" line)
      (write-region (concat line "\n") nil file t 'silent))
    ;; Naglfar: 50 more keys with the glue's entry points timed (no stats read between them), per key: the redisplay,
    ;; the time inside the glue (the shell's stages are inside it) and each entry point's calls and time
    (when naglfar
      (let ((redisp nil) entries)
        (nhmac--test-entries t)
        (dotimes (_ 50)
          (execute-kbd-macro "x")
          (let ((t1 (float-time)))
            (redisplay)
            (push (* 1e6 (- (float-time) t1)) redisp)))
        (setq entries (nhmac--test-entries nil))
        (let* ((per (lambda (ns) (/ ns 50000.0)))
               (glue (apply #'+ (mapcar (lambda (e) (nth 2 e)) entries)))
               (line (format "%s: redisplay %.0f us (mean %.0f), in the glue %.0f us per key: %s"
                             build (nhmac-gui--median redisp) (/ (apply #'+ redisp) 50) (funcall per glue)
                             (mapconcat (lambda (e) (format "%s %.1f calls %.0f us" (nth 0 e) (/ (nth 1 e) 50.0)
                                                            (funcall per (nth 2 e))))
                                        entries ", "))))
          (nhmac-gui--say "nhmac-pair-stages: %s" line)
          (write-region (concat line "\n") nil file t 'silent))))
    ;; both builds: macOS's `sample' profiles this process (1 ms interval) while it types for 3 s, so the C side's
    ;; time can be compared function by function; the call tree goes to GLITNIR_GUI_OUT/sample-BUILD.txt
    (when (executable-find "sample")
      (let* ((out (expand-file-name (format "sample-%s.txt" build) (file-name-directory file)))
             (p (let ((process-environment (nhmac-gui--capture-env)))
                  (start-process "nhmac-sample" (generate-new-buffer " *nhmac-sample*") "sample"
                                 (number-to-string (emacs-pid)) "4" "1" "-mayDie" "-file" out)))
             (end (+ (float-time) 4)))
        (sleep-for 0.5)
        (while (< (float-time) end) (execute-kbd-macro "x") (redisplay))
        (while (and (process-live-p p) (< (float-time) (+ end 10))) (accept-process-output p 0.2))
        ;; sample's own output says why a file is missing
        (nhmac-gui--say "nhmac-pair-stages: %s: sample %s %s, exit %S: %s" build out (file-exists-p out)
                        (process-exit-status p)
                        (with-current-buffer (process-buffer p) (string-trim (buffer-string))))
        (kill-buffer (process-buffer p))))
    (kill-buffer)
    (should (file-exists-p file))))

;; Draw records painted with Core Graphics
(defun nhmac-gui--pixel-at (x y)
  "The committed surface's premultiplied ARGB at device pixel X, Y of the selected frame, once the commit pacer holds
none of its commits (bounded)."
  (nhmac-gui--wait-unheld 1)
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

(ert-deftest nhmac-rounded-box-draws-rounded ()
  "A face's box with `:corner-radius' 8 draws rounded: the run's outermost corner pixel is neither the box's colour
nor its background (it shows what is beneath), while the middle of its top edge is the box's colour.  A square box,
the shell drawing no rounded path, fails here."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (switch-to-buffer (generate-new-buffer " *nhmac-rounded*"))
  (setq-local cursor-type nil)
  (insert (propertize " " 'display '(space :width (40))
                      'face '(:background "#336699"
                              :box (:line-width 2 :color "#ff8000" :corner-radius 8)))
          "\n")
  (redraw-frame)
  (redisplay t)
  (let* ((xy (posn-x-y (posn-at-point (point-min))))
         (edges (window-inside-pixel-edges))
         (x (+ (nth 0 edges) (car xy)))
         (y (+ (nth 1 edges) (cdr xy)))
         (corner (nhmac-gui--pixel-at x y))
         (edge (nhmac-gui--pixel-at (+ x 20) y)))
    (nhmac-gui--say "nhmac-rounded-box-draws-rounded: at %d,%d corner %08x edge %08x" x y corner edge)
    (should (= edge #xffff8000))
    (should-not (memql corner '(#xffff8000 #xff336699))))
  (kill-buffer))

(ert-deftest nhmac-draw-fills-boxes-cursors ()
  "Fills, a box and a bar cursor reach the committed surface with their exact colours (sRGB, opaque).
A same-run comparison with NS's pixels needs a capture of the stock build; until then the colours are
checked against the faces, tolerance 0 for fills and box lines."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((o (nhmac-gui--draw-scene))
         (x (car o)) (y (cdr o)) (h (frame-char-height)))
    (nhmac-gui--say "nhmac-draw-fills-boxes-cursors: origin %S, stats %S" o (nhmac-stats))
    (should (> (plist-get (nhmac-stats) :draws) 0))
    ;; the plain stretch's middle
    (should (= (nhmac-gui--pixel-at (+ x 6) (+ y (/ h 2))) #xff336699))
    ;; the boxed stretch: its top line, then its inside
    (should (= (nhmac-gui--pixel-at (+ x 18) (+ y 1)) #xffff0000))
    (should (= (nhmac-gui--pixel-at (+ x 18) (+ y (/ h 2))) #xff336699))
    ;; the white stretch
    (should (= (nhmac-gui--pixel-at (+ x 30) (+ y (/ h 2))) #xffffffff))
    ;; a bar cursor 4 pixels wide in the cursor colour, at the white stretch (the trace from here)
    (nhmac--pump-trace)
    (set-face-background 'cursor "#00ff00")
    (setq-local cursor-type '(bar . 4))
    (goto-char (+ (point-min) 2))
    (redisplay t)
    (should (= (nhmac-gui--pixel-at (+ x 25) (+ y (/ h 2))) #xff00ff00))
    ;; and gone again: the stretch's own colour where it was; the trace names both publishes and their damage
    (let ((draws (plist-get (nhmac-stats) :draws)))
      (setq-local cursor-type nil)
      ;; a new cursor type alone does not mark the window for redisplay (the blink timer's redraw would erase it later)
      (force-window-update (selected-window))
      (redisplay t)
      (setq draws (- (plist-get (nhmac-stats) :draws) draws))
      (ert-info ((format "pump decisions %S, %d draws, surface %S" (nhmac--pump-trace) draws (nhmac-gui--surface)))
        (should (= (nhmac-gui--pixel-at (+ x 25) (+ y (/ h 2))) #xffffffff)))))
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
    (nhmac-gui--say "nhmac-draw-other-thread: %S" (nhmac-stats))
    (should (>= (nhmac-gui--delta before :deferred-commits) 1))
    ;; the main thread's pump commits it
    (sit-for 0.1)
    (should (eq (plist-get (nhmac-gui--surface) :contents-is-newest) t))
    (should (= (nhmac-gui--pixel-at (+ x 6) (+ y (/ h 2))) #xff00ff00)))
  (kill-buffer))

;; Core Text text, compared with NS's in the same run
(defconst nhmac-gui--font-bg "#f0e0d0" "The text rows' background: no window chrome or desktop has it.")

(defun nhmac-gui--font-file (ext &optional ns)
  "The NS text row's output, nhmac-ns-font-scaleS.EXT: in this lane's GLITNIR_GUI_OUT, or with NS, in the NS lane's
(Naglfar's lane writes to its naglfar/ subdirectory)."
  (let ((out (or (getenv "GLITNIR_GUI_OUT") temporary-file-directory)))
    (expand-file-name (format "nhmac-ns-font-scale%s.%s" (or (getenv "GLITNIR_GUI_SCALE") "1") ext)
                      (if ns (expand-file-name ".." out) out))))

(defconst nhmac-gui--crop-marker "#00ff00"
  "The colour of the rule the capture scenes draw across the top of their window's text area.")

(defun nhmac-gui--marker-line ()
  "A line of `nhmac-gui--crop-marker' as wide as the window's text area: `nhmac-gui--text-crop' crops below it and
within its width, so the title bar, a tool bar and scroll bars (each build has its own) are never in a crop."
  (concat (propertize " " 'display '(space :align-to right) 'face `(:background ,nhmac-gui--crop-marker)) "\n"))

(defmacro nhmac-gui--bare-frame (&rest body)
  "Run BODY with the selected frame showing one window, no tool bar and an empty echo area; put them back after.
drgnx never shows Emacs's tool bar, and an earlier window (a *Compile-Log* of the test files' loading) or message
differs between the builds' captures."
  (declare (indent 0))
  `(let ((conf (current-window-configuration))
         (tb (frame-parameter nil 'tool-bar-lines)))
     (unwind-protect
         (progn
           (set-frame-parameter nil 'tool-bar-lines 0)
           (delete-other-windows)
           (message nil)
           ,@body)
       (set-window-configuration conf)
       (set-frame-parameter nil 'tool-bar-lines tb))))

(defmacro nhmac-gui--with-font-scene (&rest body)
  "Run BODY with the frame in Menlo 12, black on `nhmac-gui--font-bg', no cursor, mode line or fringes, showing the
crop marker (`nhmac-gui--marker-line'), a blank line and then a line of ASCII, in a bare frame
\(`nhmac-gui--bare-frame'); the frame's font and colours are put back after."
  (declare (indent 0))
  `(let ((font (frame-parameter nil 'font))
         (fg (frame-parameter nil 'foreground-color))
         (bg (frame-parameter nil 'background-color)))
     (nhmac-gui--bare-frame
       (unwind-protect
           (progn
             (set-frame-font "Menlo-12" nil (list (selected-frame)))
             (set-foreground-color "#000000")
             (set-background-color nhmac-gui--font-bg)
             (switch-to-buffer (generate-new-buffer " *nhmac-font*"))
             (setq-local cursor-type nil mode-line-format nil)
             (set-window-fringes nil 0 0)
             (insert (nhmac-gui--marker-line) "\n    Hello, world! 0123456789 {}[]()<>;:'\",./?\n")
             (redraw-frame)
             (redisplay t)
             (sit-for 0.5)
             ,@body)
         (kill-buffer)
         (set-window-fringes nil nil nil)
         (set-frame-font font nil (list (selected-frame)))
         (set-foreground-color fg)
         (set-background-color bg)))))

(defun nhmac-gui--font-metrics ()
  "The default face's font's (ascent descent average-width space-width), from `font-info'."
  (let ((i (font-info (face-font 'default))))
    (list (aref i 8) (aref i 9) (aref i 11) (aref i 10))))

(defun nhmac-gui--capture-env ()
  "`process-environment' without the counting interposer: no DYLD_* and no NHMAC_INTERPOSE_LIB.
A capture child must never load the interposer the app runs with."
  (seq-remove (lambda (v) (string-match-p "\\`\\(DYLD_\\|NHMAC_INTERPOSE_LIB=\\)" v)) process-environment))

(ert-deftest nhmac-gui-capture-env-clean ()
  "A child started with the capture's environment sees no DYLD_INSERT_LIBRARIES, even while the app has it."
  (let ((process-environment (cons "DYLD_INSERT_LIBRARIES=/nonexistent/libmac_interpose.dylib" process-environment)))
    (let ((process-environment (nhmac-gui--capture-env)))
      (with-temp-buffer
        (should (eql 0 (call-process "env" nil t nil)))
        (should-not (string-match-p "^DYLD_" (buffer-string)))
        (should (string-match-p "^PATH=" (buffer-string)))))))

(defun nhmac-gui--capture (file)
  "The screen around the selected frame (GLITNIR_GUI_FRAME's corner, 900 by 700 points) as a BMP in FILE.
Device pixels, as the window server composited them; FILE.  The command line, its exit code and output, and the file
afterwards are logged; a non-zero exit or no file fails the row with them.  It runs without the interposer's
environment (`nhmac-gui--capture-env')."
  (let* ((g (mapcar #'string-to-number (split-string (or (getenv "GLITNIR_GUI_FRAME") "0 0"))))
         (r (format "%d,%d,900,700" (- (nth 0 g) 20) (- (nth 1 g) 60)))
         (args (list "-x" "-t" "bmp" "-R" r file))
         (rc nil) (output nil))
    (when (file-exists-p file) (delete-file file))
    (with-temp-buffer
      (setq rc (let ((process-environment (nhmac-gui--capture-env)))
                 (apply #'call-process "screencapture" nil t nil args))
            output (string-trim (buffer-string))))
    (let* ((size (and (file-exists-p file) (file-attribute-size (file-attributes file))))
           (report (format "screencapture %s: exit %S, output %S, file %s; frame at %S, %dx%d, GLITNIR_GUI_FRAME %S"
                           (mapconcat #'identity args " ") rc output
                           (if size (format "%d bytes" size) "missing")
                           (condition-case e (frame-position) (error e))
                           (frame-pixel-width) (frame-pixel-height) (getenv "GLITNIR_GUI_FRAME"))))
      ;; *Messages* only: in the echo area the report wraps to three lines, the mini-window grows and the next
      ;; capture shows other window heights than the one before it
      (let ((inhibit-message t)) (message "nhmac-gui--capture: %s" report))
      (unless (and (eql rc 0) size)
        (ert-fail (list "nhmac-gui--capture failed" report))))
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

(defun nhmac-gui--text-crop (img &optional pad)
  "The text in IMG (from `nhmac-gui--bmp-read'): the commonest colour is the frame's background; inside the box of
the pixels near it, the box of the pixels away from it.  Where IMG shows the crop marker (`nhmac-gui--marker-line'),
that inside is below the marker and within its width widened by PAD pixels on each side (a fringe), down to 16 pixels
above the background box's bottom (the window's rounded corners); elsewhere it is the box less 16 pixels on each
side.  Returns (W H PIXELS) of that box."
  (pcase-let* ((`(,w ,h ,px) img)
               (counts (make-hash-table))
               (bg (progn (mapc (lambda (p) (puthash p (1+ (gethash p counts 0)) counts)) px)
                          (let ((best nil) (n 0))
                            (maphash (lambda (k v) (when (> v n) (setq best k n v))) counts)
                            best)))
               (marker (string-to-number (substring nhmac-gui--crop-marker 1) 16))
               (x0 w) (y0 h) (x1 -1) (y1 -1)
               (mx0 w) (mx1 -1) (my1 -1))
    (dotimes (y h)
      (dotimes (x w)
        (let ((p (aref px (+ (* y w) x))))
          (unless (nhmac-gui--far p bg 2)
            (setq x0 (min x0 x) y0 (min y0 y) x1 (max x1 x) y1 (max y1 y)))
          (unless (nhmac-gui--far p marker 8)
            (setq mx0 (min mx0 x) mx1 (max mx1 x) my1 (max my1 y))))))
    (pcase-let ((`(,cx0 ,cy0 ,cx1 ,cy1)
                 (if (>= mx1 0)
                     (list (max x0 (- mx0 (or pad 0))) (1+ my1) (min x1 (+ mx1 (or pad 0))) (- y1 16))
                   (list (+ x0 16) (+ y0 16) (- x1 16) (- y1 16))))
                (tx0 w) (ty0 h) (tx1 -1) (ty1 -1))
      (cl-loop for y from cy0 to cy1 do
               (cl-loop for x from cx0 to cx1 do
                        (when (nhmac-gui--far (aref px (+ (* y w) x)) bg 2)
                          (setq tx0 (min tx0 x) ty0 (min ty0 y) tx1 (max tx1 x) ty1 (max ty1 y)))))
      (if (< tx1 0)
          (list 0 0 [])
        (let* ((cw (1+ (- tx1 tx0))) (ch (1+ (- ty1 ty0))) (out (make-vector (* cw ch) 0)))
          (dotimes (y ch)
            (dotimes (x cw)
              (aset out (+ (* y cw) x) (aref px (+ (* (+ ty0 y) w) tx0 x)))))
          (list cw ch out))))))

;; These loop over every pixel of a 900 by 700 point capture: interpreted in a checking build, a capture's read and
;; crop took minutes (nhmac-images-worker: about 300 s a file); compiled, they take seconds.
(mapc #'byte-compile '(nhmac-gui--bmp-read nhmac-gui--far nhmac-gui--text-crop))

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
        (nhmac-gui--say "nhmac-font-ascii: scale %d: NS %S, Naglfar %S, stats %S" scale ns mine (nhmac-stats))
        (should (equal mine (mapcar (lambda (v) (* v scale)) ns))))
      (pcase-let* ((`(,nw ,nh ,npx) (nhmac-gui--text-crop (nhmac-gui--bmp-read (nhmac-gui--font-file "bmp" t))))
                   (`(,mw ,mh ,mpx) (nhmac-gui--text-crop
                                     (nhmac-gui--bmp-read (nhmac-gui--capture (nhmac-gui--font-file "bmp")))))
                   (worst 0) (off 0))
        (nhmac-gui--say "nhmac-font-ascii: text NS %dx%d, Naglfar %dx%d" nw nh mw mh)
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
        (nhmac-gui--say "nhmac-font-ascii: worst channel difference %d, %d channels over 1" worst off)
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
        (nhmac-gui--say "nhmac-font-open-close: a text pixel %S, stats %S" dark (nhmac-stats))
        (should dark)))))

;; Images through ImageIO
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

(defun nhmac-gui--image-pixel-size (file)
  "FILE's pixel size (W . H) as sips reads it (ImageIO's own header read), or nil."
  (with-temp-buffer
    (call-process "sips" nil t nil "-g" "pixelWidth" "-g" "pixelHeight" file)
    (goto-char (point-min))
    (let ((w (and (re-search-forward "pixelWidth: \\([0-9]+\\)" nil t) (string-to-number (match-string 1))))
          (h (and (re-search-forward "pixelHeight: \\([0-9]+\\)" nil t) (string-to-number (match-string 1)))))
      (and w h (cons w h)))))

(defmacro nhmac-gui--with-image-scene (file &rest body)
  "Run BODY with FILE's image alone below the crop marker (`nhmac-gui--marker-line') of a buffer on
`nhmac-gui--font-bg', no cursor, mode line or fringes, in a bare frame (`nhmac-gui--bare-frame'); the frame's colours
are put back after."
  (declare (indent 1))
  `(let ((bg (frame-parameter nil 'background-color)))
     (nhmac-gui--bare-frame
       (unwind-protect
           (progn
             (set-background-color nhmac-gui--font-bg)
             (switch-to-buffer (generate-new-buffer " *nhmac-img*"))
             (setq-local cursor-type nil mode-line-format nil)
             (set-window-fringes nil 0 0)
             (insert (nhmac-gui--marker-line) "\n    ")
             (insert-image (create-image ,file nil nil :scale 1))
             (redraw-frame)
             (redisplay t)
             (sit-for 0.5)
             ,@body)
         (kill-buffer)
         (set-window-fringes nil nil nil)
         (set-background-color bg)))))

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
  "PNG, JPEG, GIF (animated), TIFF, WebP and HEIC decode through ImageIO at their files' sizes; at scale 1 the pixels
on screen of each that NS decoded are within 2/255 of NS's in the same run (NS points are pixels there)."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (dolist (type '(png jpeg gif tiff webp heic))
    (should (image-type-available-p type)))
  (let ((ns (with-temp-buffer
              (insert-file-contents (nhmac-gui--font-file "images.eld" t))
              (read (current-buffer)))))
    (dolist (f (nhmac-gui--image-files))
      (let* ((name (file-name-nondirectory f))
             (size (image-size (create-image f nil nil :scale 1) t))
             (pixels (nhmac-gui--image-pixel-size f))
             ;; the stock NS build on the runner decodes no TIFF or HEIC (its image is Emacs's 30 x 30 stand-in,
             ;; nothing on screen): the file's own size is the reference, and only images NS decoded are compared
             ;; pixel by pixel
             (ns-decoded (equal (cdr (assoc name ns)) pixels)))
        (nhmac-gui--say "nhmac-images-native: %s NS %S Naglfar %S file %S" name (cdr (assoc name ns)) size pixels)
        (should (equal size pixels))
        (when (and ns-decoded (equal (getenv "GLITNIR_GUI_SCALE") "1"))
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
              (nhmac-gui--say "nhmac-images-native: %s %dx%d, worst channel difference %d" name mw mh worst)
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

(defun nhmac-gui--image-screen (file &optional worker)
  "FILE's image as the committed surface holds it, (W H PIXELS) in device pixels: loaded inline, or with WORKER
decoded on nh-img and landed first.  Read from the surface in process, so the window server's compositing and the
screen capture are not in the comparison."
  (let ((nidhogg-workers-images (and worker t))
        (nidhogg-image-defer (and worker t)))
    (clear-image-cache t)
    (nhmac-gui--with-image-scene file
      (when worker
        (should (nidhogg-workers--test-await-landed 'nh-img 5000))
        (redisplay t)
        (should (= 0 (plist-get (nidhogg-workers--test-image-info) :pending))))
      (nhmac-gui--wait-unheld 1)
      (let* ((at (1- (point-max)))
             (xy (posn-x-y (posn-at-point at)))
             (edges (window-inside-pixel-edges))
             (size (image-size (get-char-property at 'display) t))
             (x (+ (nth 0 edges) (car xy)))
             (y (+ (nth 1 edges) (cdr xy)))
             (w (car size))
             (h (cdr size)))
        (list w h (apply #'vconcat (mapcar (lambda (row) (nhmac--test-surface-region x (+ y row) w 1))
                                           (number-sequence 0 (1- h)))))))))

(ert-deftest nhmac-images-worker ()
  "PNG, JPEG, GIF and WebP decoded off the main thread by the image worker (Glitnir's decoders, ImageIO behind them
and for JPEG) land and draw the pixels the inline load draws, within 2/255."
  :tags '(draws)
  (skip-unless (and (not (nhmac-gui--ns-build-p)) (equal (getenv "GLITNIR_GUI_SCALE") "1")))
  (should (fboundp 'nidhogg-workers--test-decide))
  (dolist (type '(png jpeg gif webp))
    (should (nidhogg-workers--deferrable-p type)))
  (let ((nidhogg-workers--mode 'threads))
    (dolist (f (nhmac-gui--image-files))
      (when (string-match-p "\\.\\(png\\|jpg\\|gif\\|webp\\)\\'" f)
        (let* ((name (file-name-nondirectory f))
               (t0 (float-time))
               (inline (nhmac-gui--image-screen f))
               (landed (plist-get (nidhogg-workers--test-image-info) :landed))
               (off (unwind-protect
                        (progn (nidhogg-workers--test-decide 'offload)
                               (nhmac-gui--image-screen f t))
                      (nidhogg-workers--test-decide nil)))
               (t1 (float-time))
               ;; a control: the inline load again, so a difference between two inline loads (ImageIO)
               ;; is told apart from one the worker makes
               (again (and (string-suffix-p ".jpg" name) (nhmac-gui--image-screen f)))
               (worst 0))
          ;; the worker took it: one more landing
          (should (> (plist-get (nidhogg-workers--test-image-info) :landed) landed))
          (should (equal (take 2 off) (take 2 inline)))
          (dotimes (i (length (nth 2 inline)))
            (let ((a (aref (nth 2 inline) i)) (b (aref (nth 2 off) i)))
              (dolist (sh '(16 8 0))
                (setq worst (max worst (abs (- (logand (ash a (- sh)) 255) (logand (ash b (- sh)) 255))))))))
          (nhmac-gui--say "nhmac-images-worker: %s %dx%d, worst channel difference %d%s; reads %.1f s, compare %.1f s"
                          name (car inline) (nth 1 inline) worst
                          (if again
                              (format " (inline against inline: %d)"
                                      (let ((m 0))
                                        (dotimes (i (min (length (nth 2 inline)) (length (nth 2 again))))
                                          (let ((a (aref (nth 2 inline) i)) (b (aref (nth 2 again) i)))
                                            (dolist (sh '(16 8 0))
                                              (setq m (max m (abs (- (logand (ash a (- sh)) 255)
                                                                     (logand (ash b (- sh)) 255))))))))
                                        m))
                            "")
                          (- t1 t0) (- (float-time) t1))
          (should (<= worst 2)))))))

(ert-deftest nhmac-images-stand-in ()
  "While a zoomed image's decode is parked on the worker, the picture it replaces draws scaled into the new box (the
glue's stand-in), and the stand-in goes when the decode lands."
  :tags '(draws)
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
            (nhmac-gui--say "nhmac-images-stand-in: old %S, pixel %x" size px)
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
      (nhmac-gui--say "nhmac-images-builtin: xbm %x pbm %x" (nhmac-gui--pixel-at (+ x 2) (+ y 2))
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
      ;; one comparison, so a failure shows every pixel:
      ;; scaled 2: rows 0 and 1 lit, row 2 not;
      ;; turned 90 degrees clockwise (from x 16): the top row is the right column;
      ;; turned 270 (the top row to the left column), then flipped (from x 24): the right column again;
      ;; the 8-pixel turned images sit centred on the 16-pixel line (ascent 50%), from y 4, so their row 7
      ;; the draws name each image's rectangle, source origin and matrix
      (ert-info ((format "image draws %S at (%d %d)" (nhmac--image-draws) x y))
        (should (equal (mapcar (lambda (p) (format "%x" (funcall px (car p) (cdr p))))
                               '((3 . 1) (3 . 2) (23 . 7) (16 . 7) (31 . 7) (24 . 7)))
                       '("ffff0000" "ff0000ff" "ffff0000" "ff0000ff" "ffff0000" "ff0000ff"))))))
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
      (nhmac-gui--say "nhmac-images-svg: currentColor %x background %x" (nth 0 px) (nth 1 px))
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
    (nhmac-gui--say "nhmac-images-xpm: red %x None %x" (nth 0 px) (nth 1 px))
    (should (equal px '(#xffff0000 #xff0000ff)))))

(ert-deftest nhmac-images-image-tests ()
  "test/src/image-tests.el passes on Naglfar."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (load (expand-file-name "../image-tests" (getenv "GLITNIR_GUI_TESTS")) nil t)
  (let ((stats (ert-run-tests "\\`image-tests-" #'ignore)))
    (nhmac-gui--say "nhmac-images-image-tests: %d run, %d unexpected, %d skipped"
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
    (nhmac-gui--say "nhmac-images-no-homebrew: %s links %S" bin libs)
    (should libs)
    (should-not (nhmac-gui--outside-image-libs libs))))

;; Child frames as layers, overhanging ones as panels
(defun nhmac-gui--windows-become (n)
  "Whether the window server lists N windows of this application within 2 s.
Its list follows AppKit's ordering a little later, so a row waits for it, bounded."
  (gui-runner-wait (lambda () (= (length (nhmac--test-windows)) n)) 2))

(defun nhmac-gui--without-blink (fn)
  "Call FN with the blinking cursor off, so no frame redraws its cursor on its own meanwhile; then as before."
  (let ((blink blink-cursor-mode))
    (blink-cursor-mode -1)
    (unwind-protect (funcall fn)
      (blink-cursor-mode (if blink 1 -1)))))

(defun nhmac-gui--settle ()
  "Redisplay, let the pumps run for 0.2 s reading the events that come, redisplay again.
Reading runs the special events (a scale's config-changed event, a move-frame event, a monitors change), which the
command loop would run and which nothing else here reads: left queued, they are pending input, so `sit-for' returns at
once and a `redisplay' that is not forced does nothing.  Other events (keys a row posted) go back to the queue, in
order, for the row to read."
  (redisplay t)
  (let ((end (+ (float-time) 0.2)) (kept nil) e)
    (while (< (float-time) end)
      (when (setq e (read-event nil nil (max 0.01 (- end (float-time)))))
        (push e kept)))
    (setq unread-command-events (append unread-command-events (nreverse kept))))
  (redisplay t))

(defun nhmac-gui--child (&optional params left top parent)
  "A shown child frame of PARENT (default the selected frame) at LEFT, TOP (device pixels, default 40, 40).
16x3 characters, all #ff00ff: an empty buffer with no mode line, cursor, tool bar or tab bar.  PARAMS come first,
so they win."
  (let* ((root (selected-frame))
         (buf (get-buffer-create " *nhmac-child*"))
         (f (make-frame `(,@params (parent-frame . ,(or parent root)) (minibuffer . nil)
                          (left . ,(or left 40)) (top . ,(or top 40)) (width . 16) (height . 3)
                          (background-color . "#ff00ff") (left-fringe . 0) (right-fringe . 0)
                          (visibility . nil) (no-accept-focus . t) (no-other-frame . t)
                          (undecorated . t) (internal-border-width . 0) (tool-bar-lines . 0) (tab-bar-lines . 0)))))
    (with-current-buffer buf
      (setq-local mode-line-format nil cursor-type nil cursor-in-non-selected-windows nil))
    (select-frame root)
    (set-window-buffer (frame-root-window f) buf)
    (make-frame-visible f)
    (nhmac-gui--settle)
    f))

(defun nhmac-gui--copies (count area)
  "How many copies of a child of AREA device pixels a capture that counted COUNT of its colour shows.
0 when almost none, 2 when clearly more than one copy's worth, else 1."
  (cond ((null count) nil)
        ((< count (* 0.05 area)) 0)
        ((> count (* 1.2 area)) 2)
        (t 1)))

(ert-deftest nhmac-child-move ()
  "A child frame is a container layer in its parent's root; a move changes the container's position and nothing
else (no contents, so no shadow render either), in one commit, with no picture drawn.
The root's cursor is off meanwhile: every redisplay draws the selected window's cursor again (xdisp.c's
`display_and_set_cursor' does not check `phys_cursor_on_p'), with or without a move, which would count as draws."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((c (nhmac-gui--child))
        (root-buf (window-buffer (frame-selected-window (selected-frame)))))
    (unwind-protect
        (let ((info (nhmac--test-child c))
              (s (/ (frame-pixel-width c) (float (plist-get (nhmac--test-child c) :w)))))
          (should (eq (plist-get info :host) 'parent))
          (should (eq (plist-get info :hidden) nil))
          (should (eq (plist-get info :contents-is-newest) t))
          (should (equal (plist-get info :place) (list (round (/ 40 s)) (round (/ 40 s)))))
          (nhmac-gui--without-blink
           (lambda ()
             (with-current-buffer root-buf
               (let ((cursor-type nil))
                 (nhmac-gui--settle)
                 (let ((before (progn (nhmac--pump-trace) (nhmac-stats)))
                       (keys '(:props-position :props-contents :props-bounds :props-hidden :props-opacity :props-order
                               :props-host :overhang-switches :draws :commits)))
                   (set-frame-position c 80 60)
                   (nhmac-gui--settle)
                   (let ((after (nhmac--test-child c)))
                     ;; the trace names each publish: which window drew, and where
                     (nhmac-gui--say "nhmac-child-move: %S -> %S, %S, child window %S, trace %S" info after
                                     (mapcan (lambda (k) (list k (nhmac-gui--delta before k))) keys)
                                     (frame-parameter c 'window-id) (nhmac--pump-trace))
                     (should (equal (plist-get after :place) (list (round (/ 80 s)) (round (/ 60 s)))))
                     (should (= (plist-get after :x) (round (/ 80 s)))))
                   (should (= (nhmac-gui--delta before :props-position) 1))
                   (dolist (k '(:props-contents :props-bounds :props-hidden :props-opacity :props-order :props-host
                                :overhang-switches :draws))
                     (should (= (nhmac-gui--delta before k) 0)))
                   (should (= (nhmac-gui--delta before :commits) 1))))))))
      (delete-frame c t))))

(ert-deftest nhmac-child-click-through-shadow ()
  "A child with a shadow takes clicks on its native area only: a click on its shadow reaches the frame beneath, its
parent.  On Core Animation the shadow is a layer beside the content, not a margin of the surface (as on Wayland):
the surface is the native area, all of it takes input."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((root (selected-frame))
         (c (nhmac-gui--child '((shadow . (:blur 16 :offset (0 . 6) :opacity 0.35))) 100 100)))
    (unwind-protect
        (let* ((info (nhmac--test-child c))
               (s (plist-get (nhmac--test-surface 0 0) :scale))
               (h (frame-pixel-height c))
               (frame-of (lambda (ev) (window-frame (posn-window (event-start ev)))))
               (click (lambda (x y)
                        (nhmac-gui--flush)
                        (nhmac--test-post-mouse 'down x y nil nil root)
                        (nhmac--test-post-mouse 'up x y nil nil root)
                        (seq-find (lambda (ev) (and (consp ev) (memq 'click (event-modifiers ev))))
                                  (nhmac-gui--events 4)))))
          (nhmac-gui--say "nhmac-child-click-through-shadow: %S" info)
          ;; no margin: the container is the frame's native size at its place, and no input region narrows it
          (should (equal (plist-get info :place) (list (round (/ 100 s)) (round (/ 100 s)))))
          ;; the container is whole points: within a device pixel of the frame's height
          (should (< (abs (- (* s (plist-get info :h)) h)) s))
          (should-not (plist-get info :input))
          (let ((on-child (funcall click (+ 100 10) (+ 100 (/ h 2))))
                (on-shadow (funcall click (- 100 6) (+ 100 (/ h 2)))))
            (nhmac-gui--say "nhmac-child-click-through-shadow: on the child %S, on the shadow %S" on-child on-shadow)
            (should on-child)
            (should (eq (funcall frame-of on-child) c))
            (should on-shadow)
            (should (eq (funcall frame-of on-shadow) root))))
      (delete-frame c t))))

(defun nhmac-gui--window-follows (what before)
  "Whether the selected frame's window shows its newest surface at the surface's size, after WHAT (a string).
BEFORE is the frame's native size (width . height) before WHAT: the frame has another now, keeps it over a second
settle (no configure took it back), and the content size times the scale is the surface's size, to a device pixel."
  (let* ((f (selected-frame))
         (size (lambda () (cons (frame-pixel-width f) (frame-pixel-height f))))
         (now (progn (nhmac-gui--settle) (funcall size)))
         (kept (progn (nhmac-gui--settle) (funcall size)))
         (s (nhmac--test-surface 0 0))
         (scale (plist-get s :scale))
         (points (plist-get s :points)))
    (nhmac-gui--say "nhmac-set-frame-size: %s: %S -> %S, then %S, %S" what before now kept s)
    (and (not (equal now before))
         (equal now kept)
         (<= (abs (- (* scale (nth 0 points)) (plist-get s :w))) 1)
         (<= (abs (- (* scale (nth 1 points)) (plist-get s :h))) 1))))

(ert-deftest nhmac-set-frame-size ()
  "`set-frame-size', `set-frame-width' and `set-frame-height' on a top-level frame: its window takes the size of
the surface the frame draws at its new size, and no configure takes the frame back to the old one."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((f (selected-frame))
         (size (lambda () (cons (frame-pixel-width f) (frame-pixel-height f))))
         (w (frame-text-width f))
         (h (frame-text-height f)))
    (unwind-protect
        (let ((before (funcall size)))
          (set-frame-size f (+ w 64) (+ h 48) t)
          (should (nhmac-gui--window-follows "set-frame-size" before))
          (setq before (funcall size))
          (set-frame-width f (- (frame-width f) 4))
          (should (nhmac-gui--window-follows "set-frame-width" before))
          (setq before (funcall size))
          (set-frame-height f (- (frame-height f) 3))
          (should (nhmac-gui--window-follows "set-frame-height" before)))
      (set-frame-size f w h t)
      (nhmac-gui--settle))))

(ert-deftest nhmac-overhang-switch ()
  "A child moved across its root's right edge goes into an overhang panel once per crossing and comes back once,
with no picture drawn; captures across the switch show exactly one copy of it."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((root (selected-frame))
         (rw (frame-pixel-width root))
         (c (nhmac-gui--child nil 40 40))
         (cw (frame-pixel-width c))
         (area (* cw (frame-pixel-height c)))
         (windows (length (nhmac--test-windows))))
    (unwind-protect
        (let ((before (nhmac-stats)))
          ;; step across the edge and back, 8 device pixels at a time, settling after each
          (nhmac--test-capture-switch 255 0 255)
          (let ((x (- rw cw 24)) hosts)
            (while (< x (- rw (/ cw 2)))
              (set-frame-position c x 40) (nhmac-gui--settle)
              (push (plist-get (nhmac--test-child c) :host) hosts)
              (nhmac-gui--say "nhmac-overhang-switch: out x %d: %S" x (nhmac--test-child c))
              (setq x (+ x 8)))
            (should (eq (car hosts) 'panel))
            (should (nhmac-gui--windows-become (1+ windows)))
            (should (plist-get (nhmac--test-child c) :panel))
            (while (> x (- rw cw 24))
              (setq x (- x 8))
              (set-frame-position c x 40) (nhmac-gui--settle)
              (nhmac-gui--say "nhmac-overhang-switch: back x %d: %S" x (nhmac--test-child c)))
            (should (eq (plist-get (nhmac--test-child c) :host) 'parent))
            (should-not (plist-get (nhmac--test-child c) :panel))
            (should (nhmac-gui--windows-become windows)))
          (let ((caps (nhmac--test-captures)))
            (nhmac-gui--say "nhmac-overhang-switch: captures %S of %d, %S" caps area (nhmac-stats))
            (should (= (length caps) 2))
            (should (equal (mapcar (lambda (n) (nhmac-gui--copies n area)) caps) '(1 1))))
          (should (= (nhmac-gui--delta before :overhang-switches) 2))
          (should (= (nhmac-gui--delta before :panel-orders) 2))
          (should (= (nhmac-gui--delta before :draws) 0)))
      (delete-frame c t))))

(ert-deftest nhmac-parent-frame-changes ()
  "`parent-frame' changed all three ways with no picture drawn: a child becomes a top-level window, the top-level
a child again, and the child moves to another root."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((root (selected-frame))
         (other (make-frame '((width . 40) (height . 10))))
         (c nil)
         (windows nil)
         (blink blink-cursor-mode))
    (unwind-protect
        (progn
          ;; no cursor blinks while the row counts draws
          (blink-cursor-mode -1)
          (select-frame root)
          ;; the other root finishes showing (placed, configured, drawn) before anything is counted
          (dotimes (_ 3) (nhmac-gui--settle))
          (setq c (nhmac-gui--child))
          (dotimes (_ 3) (nhmac-gui--settle))
          (setq windows (length (nhmac--test-windows)))
          (let ((before (progn (nhmac--pump-trace) (nhmac-stats))))
            (set-frame-parameter c 'parent-frame nil)
            (nhmac-gui--settle)
            ;; the pump decisions and paints name what drew, when something did
            (nhmac-gui--say "nhmac-parent-frame-changes: to a top-level: %d draws; scale %s, %S px at %S; pumps %S"
                            (nhmac-gui--delta before :draws) (frame-scale-factor c)
                            (list (frame-pixel-width c) (frame-pixel-height c)) (frame-position c)
                            (and (> (nhmac-gui--delta before :draws) 0) (nhmac--pump-trace)))
            (should-not (frame-parent c))
            (should (nhmac-gui--windows-become (1+ windows)))
            (should (eq (plist-get (nhmac--window-info c) :visible) t))
            (set-frame-parameter c 'parent-frame root)
            (nhmac-gui--settle)
            (nhmac-gui--say "nhmac-parent-frame-changes: a child again: %d draws; scale %s (root %s), %S px at %S; %S"
                            (nhmac-gui--delta before :draws) (frame-scale-factor c) (frame-scale-factor root)
                            (list (frame-pixel-width c) (frame-pixel-height c)) (frame-position c)
                            (nhmac--test-child c))
            (should (eq (frame-parent c) root))
            (should (nhmac-gui--windows-become windows))
            (should (eq (plist-get (nhmac--test-child c) :host) 'parent))
            (should (eq (plist-get (nhmac--test-child c) :contents-is-newest) t))
            (set-frame-parameter c 'parent-frame other)
            (nhmac-gui--settle)
            (nhmac-gui--say "nhmac-parent-frame-changes: under the other root: %d draws; scale %s (other %s, %S px); %S"
                            (nhmac-gui--delta before :draws) (frame-scale-factor c) (frame-scale-factor other)
                            (list (frame-pixel-width other) (frame-pixel-height other)) (nhmac--test-child c))
            (let ((look (lambda (when)
                          (nhmac-gui--say "nhmac-parent-frame-changes: %s: child %S px, font %d px, host %S" when
                                          (list (frame-pixel-width c) (frame-pixel-height c))
                                          (nhmac-gui--font-px c) (plist-get (nhmac--test-child c) :host)))))
              (funcall look "at once")
              (dotimes (_ 3) (nhmac-gui--settle))
              (funcall look "after 3 more settles"))
            (should (eq (frame-parent c) other))
            (should (eq (plist-get (nhmac--test-child c) :host) 'parent))
            (should (eq (plist-get (nhmac--test-child c) :contents-is-newest) t))
            (nhmac-gui--say "nhmac-parent-frame-changes: %S" (nhmac-stats))
            (should (= (nhmac-gui--delta before :draws) 0))))
      (blink-cursor-mode (if blink 1 -1))
      (when (and c (frame-live-p c)) (delete-frame c t))
      (delete-frame other t)
      (select-frame root))))

(ert-deftest nhmac-reparent-across-scales-reopens-the-font ()
  "A child frame moved under a root of another scale takes that scale and re-opens its font at it once, as a frame
the window server moves to such a screen does (`nidhogg-frame-scale-change-functions' run once)."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((root (selected-frame))
         (other (make-frame '((width . 40) (height . 10))))
         (c nil) (reopens 0)
         (hook (lambda (frame) (when (eq frame c) (cl-incf reopens)))))
    (unwind-protect
        (progn
          (dotimes (_ 3) (nhmac-gui--settle))
          (skip-unless (/= (frame-scale-factor other) (frame-scale-factor root)))
          (select-frame root)
          (setq c (nhmac-gui--child))
          (nhmac-gui--settle)
          (add-hook 'nidhogg-frame-scale-change-functions hook)
          (let ((pt (/ (face-attribute 'default :height c t) 10.0)))
            (set-frame-parameter c 'parent-frame other)
            (dotimes (_ 3) (nhmac-gui--settle))
            (nhmac-gui--say "nhmac-reparent-across-scales-reopens-the-font: scale %s (root %s, other %s), font %d px, %d re-opens"
                            (frame-scale-factor c) (frame-scale-factor root) (frame-scale-factor other)
                            (nhmac-gui--font-px c) reopens)
            (should (= (frame-scale-factor c) (frame-scale-factor other)))
            (should (= (nhmac-gui--font-px c) (floor (+ (* pt (frame-scale-factor other)) 0.5))))
            (should (= reopens 1))))
      (remove-hook 'nidhogg-frame-scale-change-functions hook)
      (when (and c (frame-live-p c)) (delete-frame c t))
      (delete-frame other t)
      (select-frame root)
      (nhmac-gui--settle))))

(ert-deftest nhmac-one-window-per-root ()
  "The window server lists one window per root frame: child frames add none, an overhanging one adds its panel."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((root (selected-frame))
         (windows (length (nhmac--test-windows)))
         (a (nhmac-gui--child nil 20 20))
         (b (nhmac-gui--child nil 60 60)))
    (unwind-protect
        (progn
          (should (nhmac-gui--windows-become windows))
          (set-frame-position b (- (frame-pixel-width root) (/ (frame-pixel-width b) 2)) 60)
          (nhmac-gui--settle)
          (should (eq (plist-get (nhmac--test-child b) :host) 'panel))
          (should (nhmac-gui--windows-become (1+ windows)))
          (set-frame-position b 60 60)
          (nhmac-gui--settle)
          (should (nhmac-gui--windows-become windows)))
      (delete-frame a t)
      (delete-frame b t))))


;; Task 22: glyph tables C reads without a call
(ert-deftest nhmac-font-calls-per-glyph ()
  "After warm-up, redisplaying a window of 1,000 glyphs in Menlo 12 makes no `nhg_font_*' call: C reads glyphs and
metrics from the font's glyph table.  A character on a page not read yet makes calls (the fill), so the counter counts."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((font (frame-parameter nil 'font))
        (buf (generate-new-buffer " *nhmac-glyphs*")))
    (unwind-protect
        (progn
          (set-frame-font "Menlo-12" nil (list (selected-frame)))
          (switch-to-buffer buf)
          (setq-local mode-line-format nil truncate-lines t)
          ;; 25 lines of 40: ASCII and Latin-1
          (dotimes (i 25)
            (dotimes (j 40)
              (insert (let ((k (+ (* i 40) j))) (if (= (% k 7) 0) (+ #xc0 (% k 60)) (+ ?! (% k 94))))))
            (insert "\n"))
          (goto-char (point-min))
          (dotimes (_ 2) (redraw-frame) (redisplay t))
          (let ((before (nhmac-stats)))
            (dotimes (_ 5) (redraw-frame) (redisplay t))
            (message "nhmac-font-calls-per-glyph: warm %S" (nhmac-stats))
            (should (= (nhmac-gui--delta before :font-calls) 0)))
          ;; Greek: a page no glyph above is on
          (let ((before (nhmac-stats)))
            (goto-char (point-min))
            (insert "αβγ\n")
            (redisplay t)
            (message "nhmac-font-calls-per-glyph: a miss %S" (nhmac-stats))
            (should (> (nhmac-gui--delta before :font-calls) 0))
            (should (> (nhmac-gui--delta before :font-fills) 0))))
      (kill-buffer buf)
      (set-frame-font font nil (list (selected-frame))))))

;; Task 23: listing, fallback and Core Text shaping, compared with NS's in the same run
(require 'composite)

(defconst nhmac-gui--parity-fonts
  '(("Menlo" . "Hg.W") ("SF Mono" . "Hg.W") ("Helvetica" . "Hg.W") ("PingFang SC" . "中文字")
    ("Apple Color Emoji" . "😀🐉"))
  "The fonts the parity rows compare, each with the text whose glyphs are compared.")

(defconst nhmac-gui--parity-heights '(100 120 160) "The `:height's the parity rows compare at.")

(defun nhmac-gui--lane-file (name ext &optional ns)
  "The NS row NAME's output nhmac-ns-NAME-scaleS.EXT, in this lane's GLITNIR_GUI_OUT, or with NS, the NS lane's."
  (let ((out (or (getenv "GLITNIR_GUI_OUT") temporary-file-directory)))
    (expand-file-name (format "nhmac-ns-%s-scale%s.%s" name (or (getenv "GLITNIR_GUI_SCALE") "1") ext)
                      (if ns (expand-file-name ".." out) out))))

(defun nhmac-gui--lane-read (name)
  "The NS lane's NAME output, read."
  (with-temp-buffer
    (insert-file-contents (nhmac-gui--lane-file name "eld" t))
    (read (current-buffer))))

(defun nhmac-gui--lane-write (name value)
  "Write VALUE as the NS row NAME's output."
  (with-temp-file (nhmac-gui--lane-file name "eld")
    (let ((print-length nil) (print-level nil)) (prin1 value (current-buffer)))))

(defmacro nhmac-gui--with-text (face text &rest body)
  "Run BODY in a buffer showing TEXT in FACE (a face spec), black on `nhmac-gui--font-bg', the window's only text,
point at its start, after a full redisplay; the frame's colours are put back after."
  (declare (indent 2))
  `(let ((fg (frame-parameter nil 'foreground-color))
         (bg (frame-parameter nil 'background-color))
         (buf (generate-new-buffer " *nhmac-text*")))
     (unwind-protect
         (progn
           (set-foreground-color "#000000")
           (set-background-color nhmac-gui--font-bg)
           (switch-to-buffer buf)
           (setq-local cursor-type nil mode-line-format nil)
           (set-window-fringes nil 0 0)
           (insert "\n    ")
           (insert (propertize ,text 'face ,face))
           (goto-char (+ (point-min) 5))
           (redraw-frame)
           (redisplay t)
           (sit-for 0.3)
           ,@body)
       (kill-buffer buf)
       (set-window-fringes nil nil nil)
       (set-foreground-color fg)
       (set-background-color bg))))

(defun nhmac-gui--glyph-facts (font text)
  "TEXT's glyphs in FONT (a font object): each (code width lbearing rbearing ascent descent), `font-get-glyphs'."
  (mapcar (lambda (g) (and g (list (aref g 3) (aref g 4) (aref g 5) (aref g 6) (aref g 7) (aref g 8))))
          (font-get-glyphs font 0 (length text) text)))

(defun nhmac-gui--font-facts (family height text)
  "FAMILY at HEIGHT, nil when it is not the font shown: (:info (PIXEL-SIZE ASCENT DESCENT SPACE-WIDTH AVERAGE-WIDTH
LINE-HEIGHT) :glyphs GLYPHS) for TEXT, and at :height 120 :underline (THICKNESS DROP): the underline's rows, and how
far below the text's bottom it ends, both from captures."
  (nhmac-gui--with-text (list :family family :height height) text
    (let ((font (font-at (point))))
      (when (and font (string-equal-ignore-case (symbol-name (font-get font :family)) family))
        (let* ((q (query-font font))
               (facts (list :info (list (aref q 2) (aref q 4) (aref q 5) (aref q 6) (aref q 7) (line-pixel-height))
                            :glyphs (nhmac-gui--glyph-facts font text))))
          (when (= height 120)
            (let* ((plain (nth 1 (nhmac-gui--text-crop (nhmac-gui--bmp-read
                                                         (nhmac-gui--capture (nhmac-gui--lane-file "tmp" "bmp")))))))
              (let ((inhibit-read-only t))
                (goto-char (point-max))
                (insert (propertize "    " 'face (list :family family :height height :underline t))))
              (redisplay t)
              (sit-for 0.3)
              (let ((both (nth 1 (nhmac-gui--text-crop (nhmac-gui--bmp-read
                                                         (nhmac-gui--capture (nhmac-gui--lane-file "tmp" "bmp")))))))
                (erase-buffer)
                (insert "\n    " (propertize "    " 'face (list :family family :height height :underline t)))
                (redisplay t)
                (sit-for 0.3)
                (let ((line (nth 1 (nhmac-gui--text-crop (nhmac-gui--bmp-read
                                                           (nhmac-gui--capture (nhmac-gui--lane-file "tmp" "bmp")))))))
                  (setq facts (append facts (list :underline (list line (- both plain)))))))))
          facts)))))

(defun nhmac-gui--parity-facts ()
  "Every parity font's facts at every parity height: ((FAMILY HEIGHT FACTS) ...)."
  (let (out)
    (dolist (f nhmac-gui--parity-fonts (nreverse out))
      (dolist (h nhmac-gui--parity-heights)
        (push (list (car f) h (nhmac-gui--font-facts (car f) h (cdr f))) out)))))

(defun nhmac-gui--near (ns mine scale)
  "Whether MINE equals NS times SCALE, numbers within 1 at scale 2 (NS rounds in points, Naglfar in device pixels);
lists element by element."
  (cond ((and (numberp ns) (numberp mine)) (<= (abs (- mine (* ns scale))) (if (= scale 1) 0 1)))
        ((and (consp ns) (consp mine)) (and (= (length ns) (length mine)) (cl-every (lambda (a b) (nhmac-gui--near a b scale)) ns mine)))
        (t (equal ns mine))))

(ert-deftest nhmac-ns-font-parity-dump ()
  "NS's metrics, glyph extents and underlines of the parity fonts, for nhmac-font-metrics-parity in the Naglfar lane;
its fallback fonts for nhmac-font-fallback-same; its shaping for nhmac-font-shaping."
  (skip-unless (nhmac-gui--ns-build-p))
  ;; the fallback and shaping dumps first: they need no capture, so a capture failure below does not take them along
  (nhmac-gui--lane-write "font-fallback" (nhmac-gui--fallback-facts))
  (nhmac-gui--lane-write "font-shaping" (nhmac-gui--shaping-facts))
  (let ((facts (nhmac-gui--parity-facts)))
    (message "nhmac-ns-font-parity-dump: %S" facts)
    (nhmac-gui--lane-write "font-parity" facts)
    (should (cl-some (lambda (r) (nth 2 r)) facts))))

(ert-deftest nhmac-font-metrics-parity ()
  "For Menlo, SF Mono, Helvetica, PingFang SC and Apple Color Emoji at :height 100, 120 and 160: the pixel size,
ascent, descent, space and average widths, line height, each glyph's code and extents, and the underline's thickness
and drop equal NS's in the same run (times the scale; within a device pixel at scale 2).  A font NS does not have is
reported and skipped; one NS has and Naglfar does not is a failure."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((scale (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1")))
        (ns (nhmac-gui--lane-read "font-parity"))
        (bad nil) (compared 0))
    (dolist (row ns)
      (pcase-let ((`(,family ,height ,want) row))
        (if (not want)
            (message "nhmac-font-metrics-parity: %s is not on this machine (NS), skipped" family)
          (let ((got (nhmac-gui--font-facts family height (cdr (assoc family nhmac-gui--parity-fonts)))))
            (cl-incf compared)
            (message "nhmac-font-metrics-parity: %s %d: NS %S, Naglfar %S" family height want got)
            (unless (and got
                         (nhmac-gui--near (plist-get want :info) (plist-get got :info) scale)
                         (equal (mapcar #'car (plist-get want :glyphs)) (mapcar #'car (plist-get got :glyphs)))
                         (nhmac-gui--near (mapcar #'cdr (plist-get want :glyphs)) (mapcar #'cdr (plist-get got :glyphs))
                                          scale)
                         ;; the underline is captured in device pixels on both
                         (or (= scale 2) (equal (plist-get want :underline) (plist-get got :underline))))
              (push (list family height) bad))))))
    (message "nhmac-font-metrics-parity: %d compared, differ: %S" compared bad)
    (should (> compared 0))
    (should-not bad)))

(defconst nhmac-gui--fallback-text "中あ한😀αא€ก"
  "Characters Menlo lacks, or has: each one's font is compared.")

(defun nhmac-gui--fallback-facts ()
  "Each of `nhmac-gui--fallback-text''s characters with the family of the font it is shown in, the frame's font
Menlo 12."
  (let ((font (frame-parameter nil 'font)))
    (unwind-protect
        (progn
          (set-frame-font "Menlo-12" nil (list (selected-frame)))
          (nhmac-gui--with-text 'default nhmac-gui--fallback-text
            (cl-loop for i from 0 below (length nhmac-gui--fallback-text)
                     collect (let ((f (font-at (+ (point) i))))
                               (list (aref nhmac-gui--fallback-text i)
                                     (and f (symbol-name (font-get f :family))))))))
      (set-frame-font font nil (list (selected-frame))))))

(ert-deftest nhmac-font-fallback-same ()
  "With Menlo 12 as the frame's font, CJK, emoji, Greek, Hebrew, Thai and the euro sign are shown in the same
families as on NS."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((ns (nhmac-gui--lane-read "font-fallback"))
        (mine (nhmac-gui--fallback-facts)))
    (message "nhmac-font-fallback-same: NS %S, Naglfar %S" ns mine)
    (should (cl-every #'cadr mine))
    (should (equal mine ns))))

(defconst nhmac-gui--shaping-texts '(("مرحبا بالعالم" . R2L) ("नमस्ते दुनिया" . L2R))
  "Arabic and Devanagari text, and the direction each is shaped in.")

(defun nhmac-gui--shaping-facts ()
  "Each shaping text's font family and its glyphs as Core Text shapes them (`font-shape-gstring'): each glyph's
(FROM TO CODE WIDTH XOFF YOFF WADJUST)."
  (mapcar
   (lambda (td)
     (let ((text (car td)))
       (nhmac-gui--with-text 'default text
         (let* ((font (font-at (point)))
                (g (font-shape-gstring (composition-get-gstring 0 (length text) font text) (cdr td))))
           (list (symbol-name (font-get font :family))
                 (and g
                      (cl-loop for i from 0 below (lgstring-glyph-len g)
                               for gl = (lgstring-glyph g i)
                               while gl
                               collect (let ((adj (lglyph-adjustment gl)))
                                         (list (lglyph-from gl) (lglyph-to gl) (lglyph-code gl) (lglyph-width gl)
                                               (if adj (aref adj 0) 0) (if adj (aref adj 1) 0)
                                               (if adj (aref adj 2) (lglyph-width gl)))))))))))
   nhmac-gui--shaping-texts))

(ert-deftest nhmac-font-shaping ()
  "Arabic and Devanagari runs give the same fonts, clusters and glyph ids as on NS, and the same widths and offsets
(times the scale; within a device pixel at scale 2)."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((scale (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1")))
        (ns (nhmac-gui--lane-read "font-shaping"))
        (mine (nhmac-gui--shaping-facts)))
    (message "nhmac-font-shaping: NS %S, Naglfar %S" ns mine)
    (should (cl-every #'cadr mine))
    (cl-loop for (nf nglyphs) in ns
             for (mf mglyphs) in mine
             do (should (equal mf nf))
                (should (equal (mapcar (lambda (g) (seq-take g 3)) mglyphs)
                               (mapcar (lambda (g) (seq-take g 3)) nglyphs)))
                (should (nhmac-gui--near (mapcar (lambda (g) (nthcdr 3 g)) nglyphs)
                                         (mapcar (lambda (g) (nthcdr 3 g)) mglyphs) scale)))))

(ert-deftest nhmac-paired-cjk-emoji-scroll ()
  "Scrolling CJK and emoji text by a line, timed in the process: 50 scrolls, each a redisplay, written to
GLITNIR_GUI_OUT/pair-BUILD.txt (microseconds, one per line), as nhmac-paired-key-to-commit's times are; both builds
run it in turn (--pair, nidhogg/test/nhmac-pair.sh --row), which judges Naglfar not slower."
  (let* ((build (if (nhmac-gui--ns-build-p) "ns" "naglfar"))
         (file (expand-file-name (format "pair-%s.txt" build)
                                 (or (getenv "GLITNIR_GUI_OUT") temporary-file-directory)))
         times)
    (switch-to-buffer (generate-new-buffer " *nhmac-pair-cjk*"))
    (dotimes (i 400)
      (insert (format "%3d 中文字体的回退和表情 😀🐉🌸 日本語のテキスト 한국어 %s\n" i (make-string (% i 7) ?字))))
    (goto-char (point-min))
    (redisplay t)
    (dotimes (_ 10) (scroll-up 1) (redisplay))
    (dotimes (_ 50)
      (let ((t0 (float-time)))
        (scroll-up 1)
        (redisplay)
        (push (round (* 1e6 (- (float-time) t0))) times)))
    (with-temp-buffer
      (dolist (us (nreverse times)) (insert (format "%d\n" us)))
      (write-region (point-min) (point-max) file t 'silent))
    (should (file-exists-p file))
    (kill-buffer)))

(ert-deftest nhmac-advances-for-fit ()
  "For the parity fonts at :height 120, each glyph's `font-get-glyphs' width is Core Text's own advance
(CTFontGetAdvancesForGlyphs, test-ct-advances) rounded, at the frame's device scale: a skrifa-vs-Core Text
difference in drgnx's fit row is then the fitter's, not the driver's."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((bad nil) (compared 0))
    (dolist (f nhmac-gui--parity-fonts)
      (nhmac-gui--with-text (list :family (car f) :height 120) (cdr f)
        (let ((font (font-at (point))))
          (if (not (string-equal-ignore-case (symbol-name (font-get font :family)) (car f)))
              (message "nhmac-advances-for-fit: %s is not on this machine, skipped" (car f))
            (let* ((glyphs (font-get-glyphs font 0 (length (cdr f)) (cdr f)))
                   (codes (mapcar (lambda (g) (aref g 3)) glyphs))
                   (widths (mapcar (lambda (g) (aref g 4)) glyphs))
                   (ct (apply #'nhmac--query 'test-ct-advances (symbol-name (font-get font :family))
                              (font-get font :size) codes)))
              (cl-incf compared)
              (message "nhmac-advances-for-fit: %s: widths %S, Core Text %S" (car f) widths ct)
              (unless (and ct (equal widths (mapcar (lambda (a) (floor (+ a 0.5))) ct)))
                (push (car f) bad)))))))
    (should (> compared 0))
    (should-not bad)))

;; Task 24: every draw record, compared with NS's in the same run
(defun nhmac-gui--draw-k ()
  "Device pixels per NS point for the scene's pixel sizes: the lane's scale on Naglfar, 1 on NS (which counts points)."
  (if (nhmac-gui--ns-build-p) 1 (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1"))))

(defconst nhmac-gui--draw-cursors '(box hollow bar hbar)
  "The cursor kinds the draw rows show over an image.")

(defun nhmac-gui--draw-cursor (kind k)
  "`cursor-type' for KIND with K device pixels per point."
  (pcase kind ('bar (cons 'bar (* 2 k))) ('hbar (cons 'hbar (* 2 k))) (_ kind)))

(defmacro nhmac-gui--with-draw-scene (&rest body)
  "Run BODY showing a buffer that uses every face attribute and glyph kind the draw records carry (decorations, boxes
and reliefs, stretches, glyphless and composed characters, images, fringe bitmaps), in Menlo 12 on
`nhmac-gui--font-bg', with pixel sizes in NS points (times `nhmac-gui--draw-k').  Point is on the first image."
  (declare (indent 0))
  `(let* ((k (nhmac-gui--draw-k))
          (font (frame-parameter nil 'font))
          (fg (frame-parameter nil 'foreground-color))
          (bg (frame-parameter nil 'background-color))
          (glyphless (copy-sequence glyphless-char-display))
          (img (create-image "P1\n6 6\n1 0 1 0 1 0\n0 1 0 1 0 1\n1 0 1 0 1 0\n0 1 0 1 0 1\n1 0 1 0 1 0\n0 1 0 1 0 1\n"
                             'pbm t :scale (* 2 k) :foreground "#003399" :background "#ffffff")))
     (unwind-protect
         (progn
           (set-frame-font "Menlo-12" nil (list (selected-frame)))
           (set-foreground-color "#000000")
           (set-background-color nhmac-gui--font-bg)
           (aset glyphless-char-display #xE000 'hex-code)
           (aset glyphless-char-display #xE001 "ABC")
           (switch-to-buffer (generate-new-buffer " *nhmac-draw*"))
           (setq-local cursor-type nil mode-line-format nil indicate-empty-lines t truncate-lines t)
           (set-window-fringes nil (* 8 k) (* 8 k))
           (set-window-scroll-bars nil 0 nil 0 nil)
           (let ((b (lambda (s &rest face) (insert (propertize s 'face face)))))
             (insert (propertize " " 'display '(left-fringe right-triangle)))
             (funcall b "under" :underline t) (insert " ")
             (funcall b "wave" :underline '(:style wave :color "red")) (insert " ")
             (funcall b "dash" :underline '(:style dashes :color "blue"))
             (insert (propertize " " 'display '(right-fringe left-arrow)) "\n")
             (funcall b "over" :overline t) (insert " ")
             (funcall b "strike" :strike-through "red") (insert " ")
             (funcall b "both" :overline "blue" :strike-through t) (insert "\n\n")
             (funcall b "box" :box (list :line-width k :color "blue")) (insert " ")
             (funcall b "raised" :box (list :line-width (* 2 k) :style 'released-button)) (insert " ")
             (funcall b "pressed" :box (list :line-width (* 2 k) :style 'pressed-button)) (insert " ")
             (funcall b "inner" :box (list :line-width (cons (- k) (- k)) :color "dark green")) (insert "\n\n")
             (funcall b "inverse" :inverse-video t) (insert " ")
             (funcall b "bold" :weight 'bold) (insert " ")
             (funcall b "italic" :slant 'italic) (insert " ")
             (funcall b "tall" :height 1.5 :background "light blue") (insert "\n")
             (insert "a" (propertize " " 'display '(space :width 4) 'face '(:background "orange"))
                     "b" (propertize " " 'display '(space :align-to 30) 'face '(:background "pink")) "c\n")
             (insert "glyphless " (string #xE000) " " (string #xE001) " composed ")
             (insert (compose-string (string ?e #x301)) "\n")
             (insert "image ")
             (let ((at (point)))
               (insert-image img)
               (insert " ")
               (insert-image (append img (list :relief (* 2 k) :margin (* 2 k))))
               (insert "\n\n\n")
               (goto-char at)))
           (redraw-frame)
           (redisplay t)
           (sit-for 0.5)
           ,@body)
       (setq glyphless-char-display glyphless)
       (kill-buffer)
       (set-window-fringes nil nil nil)
       (set-window-scroll-bars nil nil nil nil nil)
       (set-frame-font font nil (list (selected-frame)))
       (set-foreground-color fg)
       (set-background-color bg))))

(defun nhmac-gui--draw-geometry ()
  "Where each line of the draw scene starts and ends, and where the images are: (X . Y) of `posn-at-point' at every
line's start and end and at each image."
  (save-excursion
    (let (out)
      (goto-char (point-min))
      (while (not (eobp))
        (push (posn-x-y (posn-at-point (point))) out)
        (end-of-line)
        (push (posn-x-y (posn-at-point (point))) out)
        (forward-line 1))
      (nreverse out))))

(defun nhmac-gui--crop-diff (a b)
  "Crops A and B (`nhmac-gui--text-crop'): (SAME-SIZE WORST OFF), WORST the largest channel difference and OFF how
many pixels differ by more than 24 in a channel."
  (pcase-let ((`(,aw ,ah ,apx) a) (`(,bw ,bh ,bpx) b) (worst 0) (off 0))
    (if (not (and (= aw bw) (= ah bh)))
        (list nil 255 (* aw ah))
      (dotimes (i (length apx))
        (let ((d (cl-loop for sh in '(16 8 0)
                          maximize (abs (- (logand (ash (aref apx i) (- sh)) 255)
                                           (logand (ash (aref bpx i) (- sh)) 255))))))
          (setq worst (max worst d))
          (when (> d 24) (cl-incf off))))
      (list t worst off))))

(defun nhmac-gui--draw-captures ()
  "The draw scene's captures: the scene without a cursor, then each of `nhmac-gui--draw-cursors' over the image.
An alist of (NAME . FILE) in this lane's output directory."
  (let ((out (list (cons "draw" (nhmac-gui--capture (nhmac-gui--font-file "draw.bmp"))))))
    (dolist (c nhmac-gui--draw-cursors)
      (setq-local cursor-type (nhmac-gui--draw-cursor c (nhmac-gui--draw-k)))
      (redisplay t)
      (sit-for 0.3)
      (push (cons (format "draw-%s" c) (nhmac-gui--capture (nhmac-gui--font-file (format "draw-%s.bmp" c)))) out))
    (nreverse out)))

(defmacro nhmac-gui--with-fringe-scene (&rest body)
  "Run BODY showing only fringe bitmaps: empty lines with an arrow in each fringe, then the empty-line indicator, the
fringe in black on `nhmac-gui--font-bg', fringes 8 NS points wide, below the crop marker (`nhmac-gui--marker-line';
crop with `nhmac-gui--fringe-pad'), in a bare frame (`nhmac-gui--bare-frame')."
  (declare (indent 0))
  `(let ((k (nhmac-gui--draw-k))
         (bg (frame-parameter nil 'background-color))
         (ffg (face-foreground 'fringe)) (fbg (face-background 'fringe)))
     (nhmac-gui--bare-frame
       (unwind-protect
           (progn
             (set-background-color nhmac-gui--font-bg)
             (set-face-attribute 'fringe nil :foreground "#000000" :background nhmac-gui--font-bg)
             (switch-to-buffer (generate-new-buffer " *nhmac-fringe*"))
             (setq-local cursor-type nil mode-line-format nil indicate-empty-lines t)
             (set-window-fringes nil (* 8 k) (* 8 k))
             (set-window-scroll-bars nil 0 nil 0 nil)
             (insert (nhmac-gui--marker-line)
                     (propertize " " 'display '(left-fringe right-triangle))
                     (propertize " " 'display '(right-fringe left-arrow)) "\n"
                     (propertize " " 'display '(left-fringe up-arrow))
                     (propertize " " 'display '(right-fringe filled-rectangle)) "\n")
             (goto-char (point-min))
             (redraw-frame)
             (redisplay t)
             (sit-for 0.5)
             ,@body)
         (kill-buffer)
         (set-face-attribute 'fringe nil :foreground (or ffg 'unspecified) :background (or fbg 'unspecified))
         (set-window-fringes nil nil nil)
         (set-window-scroll-bars nil nil nil nil nil)
         (set-background-color bg)))))

(defun nhmac-gui--fringe-pad ()
  "The fringe scene's fringe width in capture pixels: 8 points at the lane's scale, on both builds."
  (* 8 (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1"))))

(ert-deftest nhmac-ns-draw-dump ()
  "NS's draw scene: its geometry and a capture of it without a cursor and with each cursor kind over the image, for
nhmac-draw-all-records in the Naglfar lane; and the fringe scene's capture, for nhmac-draw-fringe-bitmaps."
  (skip-unless (nhmac-gui--ns-build-p))
  (nhmac-gui--with-draw-scene
    ;; measured in the scene's buffer, before `with-temp-file' makes its own current
    (let ((g (nhmac-gui--draw-geometry)))
      (should g)
      (with-temp-file (nhmac-gui--font-file "draw.eld")
        (prin1 g (current-buffer))))
    (dolist (c (nhmac-gui--draw-captures))
      (should (> (car (nhmac-gui--text-crop (nhmac-gui--bmp-read (cdr c)))) 0))))
  (nhmac-gui--with-fringe-scene
    (nhmac-gui--capture (nhmac-gui--font-file "fringe.bmp"))
    (should (> (car (nhmac-gui--text-crop (nhmac-gui--bmp-read (nhmac-gui--font-file "fringe.bmp"))
                                             (nhmac-gui--fringe-pad))) 0))))

(ert-deftest nhmac-draw-all-records ()
  "The draw scene on Naglfar against NS's in the same run: its geometry is NS's times the scale; at scale 1 (NS points
are pixels) each capture, without a cursor and with each cursor kind over the image, is NS's size, and at most 0.1%
of its pixels differ by more than 24/255 in a channel.  At scale 2 the pixel differences are reported: Naglfar's
one-pixel lines (waves, reliefs, cursor bars) are device pixels there, NS's are points."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (should (file-exists-p (nhmac-gui--font-file "draw.eld" t)))
  (nhmac-gui--with-draw-scene
    (let ((ns (with-temp-buffer (insert-file-contents (nhmac-gui--font-file "draw.eld" t)) (read (current-buffer))))
          (mine (nhmac-gui--draw-geometry)))
      (message "nhmac-draw-all-records: scale %d geometry NS %S Naglfar %S" k ns mine)
      (should (equal mine (mapcar (lambda (p) (cons (* k (car p)) (* k (cdr p)))) ns))))
    (dolist (c (nhmac-gui--draw-captures))
      (let* ((name (car c))
             (d (nhmac-gui--crop-diff
                 (nhmac-gui--text-crop (nhmac-gui--bmp-read (nhmac-gui--font-file (concat name ".bmp") t)))
                 (nhmac-gui--text-crop (nhmac-gui--bmp-read (cdr c)))))
             (n (let ((cr (nhmac-gui--text-crop (nhmac-gui--bmp-read (cdr c))))) (* (nth 0 cr) (nth 1 cr)))))
        (message "nhmac-draw-all-records: scale %d %s: same size %S, worst %d, %d of %d pixels off; stats %S"
                 k name (nth 0 d) (nth 1 d) (nth 2 d) n (nhmac-stats))
        (when (= k 1)
          (should (nth 0 d))
          (should (<= (* 1000 (nth 2 d)) n)))))))

(ert-deftest nhmac-draw-fringe-bitmaps ()
  "Fringe bitmaps against NS's in the same run: at a scale NS draws each bit as a point, so Naglfar's fringes, as
wide as NS's, draw it as scale by scale pixels.  The bitmaps' crop is NS's size and has as many dark pixels."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (should (file-exists-p (nhmac-gui--font-file "fringe.bmp" t)))
  (nhmac-gui--with-fringe-scene
    (let* ((dark (lambda (c) (cl-count-if (lambda (p) (< (logand (ash p -8) 255) 128)) (nth 2 c))))
           (ns (nhmac-gui--text-crop (nhmac-gui--bmp-read (nhmac-gui--font-file "fringe.bmp" t)) (nhmac-gui--fringe-pad)))
           (mine (nhmac-gui--text-crop (nhmac-gui--bmp-read (nhmac-gui--capture (nhmac-gui--font-file "fringe.bmp")))
                                       (nhmac-gui--fringe-pad)))
           (d (nhmac-gui--crop-diff ns mine)))
      (message "nhmac-draw-fringe-bitmaps: scale %d: NS %dx%d %d dark, Naglfar %dx%d %d dark; diff %S"
               k (nth 0 ns) (nth 1 ns) (funcall dark ns) (nth 0 mine) (nth 1 mine) (funcall dark mine) d)
      (should (> (nth 0 ns) 0))
      (should (equal (list (nth 0 mine) (nth 1 mine)) (list (nth 0 ns) (nth 1 ns))))
      (should (= (funcall dark mine) (funcall dark ns))))))

;;; nhmac-gui-tests.el ends here

(ert-deftest nhmac-kq-helper-signals ()
  "A signal that lands on the wait's helper thread while it watches a fd does not stop the watch: a pipe the helper
serves, interrupted 40 times while its 100 lines come, gets every line with no gap near a second.  (A signal lands on
any thread that does not block it: Emacs's SIGCHLD can reach the helper.)"
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((gaps (make-hash-table :test #'equal))
        (start (float-time))
        p)
    (nhmac--test-helper-next)
    (setq p (nhmac-gui--feed "kq-helper-signals" 'pipe gaps))
    (accept-process-output nil 0.05)
    (nhmac--test-signal-helper 40)
    (unwind-protect
        (while (and (process-live-p p) (< (- (float-time) start) 30))
          (accept-process-output nil 5))
      (delete-process p))
    (let ((v (gethash "kq-helper-signals" gaps)))
      (nhmac-gui--say "nhmac-kq-helper-signals: %d lines, longest gap %.3f s, stats %S" (car v) (cdr v)
                      (nhmac-stats))
      (should (= (car v) 100))
      (should (< (cdr v) 1.0)))))

;; Scale and monitors (the fractional-scaling plan's Naglfar hookup): the runner's lane display and the main screen
;; are two monitors, at different scales in the scale-2 lane.

(defun nhmac-gui--other-monitor (pred)
  "The attributes of a monitor other than the selected frame's for which PRED (given its attributes) holds, or nil."
  (let ((mine (frame-monitor-attributes)))
    (seq-find (lambda (m) (and (not (equal (alist-get 'geometry m) (alist-get 'geometry mine))) (funcall pred m)))
              (display-monitor-attributes-list))))

(defun nhmac-gui--move-to (frame monitor)
  "Move FRAME into MONITOR's work area; wait (bounded) until AppKit puts its window on that screen, then settle.
The settle reads input, so the special events the move raised (the scale's config-changed event, which re-opens the
fonts, and the move-frame event) have run when this returns; `accept-process-output' alone leaves them queued."
  (let ((g (alist-get 'workarea monitor)) (end (+ (float-time) 5)))
    (set-frame-position frame (+ (nth 0 g) 40) (+ (nth 1 g) 40))
    (while (and (not (equal (alist-get 'geometry (frame-monitor-attributes frame)) (alist-get 'geometry monitor)))
                (< (float-time) end))
      (accept-process-output nil 0.05)
      (redisplay t))
    (nhmac-gui--settle)))

(defun nhmac-gui--font-px (&optional frame)
  "The pixel size of FRAME's default font."
  (font-get (face-attribute 'default :font (or frame (selected-frame))) :size))

(ert-deftest nhmac-scale-round-trip-keeps-the-font ()
  "The root frame moved to a monitor of another scale and back: its font is re-opened at each scale's device size
\(a point is 1/72 inch, so the pixel size is the height in points times the scale), and comes back to the size it had."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((f (selected-frame)) (s0 (frame-scale-factor f))
         (other (nhmac-gui--other-monitor (lambda (m) (/= (alist-get 'scale-factor m 1.0) s0))))
         (home (frame-monitor-attributes f)) (pos (frame-position f))
         (pt (/ (face-attribute 'default :height f t) 10.0))
         (px0 (nhmac-gui--font-px f)) s1 px1 px2)
    (skip-unless other)
    (unwind-protect
        (progn
          (nhmac-gui--move-to f other)
          (setq s1 (frame-scale-factor f) px1 (nhmac-gui--font-px f))
          (nhmac-gui--move-to f home)
          (setq px2 (nhmac-gui--font-px f)))
      (set-frame-position f (car pos) (cdr pos))
      (nhmac-gui--settle))
    (nhmac-gui--say "nhmac-scale-round-trip-keeps-the-font: %.1f pt: scale %s %d px, scale %s %d px, back %d px"
                    pt s0 px0 s1 px1 px2)
    (should (/= s1 s0))
    (should (= px0 (floor (+ (* pt s0) 0.5))))
    (should (= px1 (floor (+ (* pt s1) 0.5))))
    (should (= px2 px0))))

(ert-deftest nhmac-no-session-calls ()
  "A Naglfar frame names no compositor session: making a frame calls none of the Wayland shell's session functions
\(they are not loaded here, so each call would only log a void-function error)."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((f (make-frame '((width . 20) (height . 5)))))
    (unwind-protect
        (progn
          (nhmac-gui--settle)
          (should-not (string-match-p "nhwl-session" (with-current-buffer (messages-buffer) (buffer-string)))))
      (delete-frame f t))))

(defvar nidhogg-frame-scale-change-functions)

(ert-deftest nhmac-new-frame-opens-its-font-once ()
  "A new frame opens its default font once for the screen it shows on: it starts at the selected frame's scale, so
its font is re-opened only when AppKit puts it on a screen of another scale (once), never when it shows at the scale
it started at.  Either way the font's pixel size is its height in points times the frame's scale, and the window keeps
the size on the screen (points) it was made with: the text follows.  (Each re-open runs
`nidhogg-frame-scale-change-functions'.)"
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((root (selected-frame)) (f nil) (made nil) (reopens 0) (monitors 0) (moves 0)
         (on-scale (lambda (frame) (when (eq frame f) (cl-incf reopens))))
         (on-monitors (lambda (_) (cl-incf monitors)))
         (on-move (lambda (frame) (when (eq frame f) (cl-incf moves)))))
    (add-hook 'nidhogg-frame-scale-change-functions on-scale)
    (add-hook 'display-monitors-changed-functions on-monitors)
    (add-hook 'move-frame-functions on-move)
    (unwind-protect
        (progn
          (setq f (make-frame '((width . 40) (height . 10))))
          ;; its size in points as first drawn, at the selected frame's scale.  Not as `make-frame' returns it: the
          ;; tool bar's first measure (`tool-bar-height' in `x-create-frame', when the window is sized) comes out
          ;; 2 px a row shorter than redisplay's, which grows the window before anything is drawn (the text keeps
          ;; its lines; `nhmac-new-frame-laid-out-once' holds the first draw to its size)
          (nhmac-gui--settle)
          (setq made (list (/ (frame-pixel-width f) (frame-scale-factor root))
                           (/ (frame-pixel-height f) (frame-scale-factor root))))
          (dotimes (_ 3) (nhmac-gui--settle))
          (let ((s (frame-scale-factor f)) (pt (/ (face-attribute 'default :height f t) 10.0)))
            (nhmac-gui--say
             (concat "nhmac-new-frame-opens-its-font-once: scale %s (root %s), %.1f pt, %d px (root %d px), "
                     "%d re-opens, %d monitor changes, %d moves")
             s (frame-scale-factor root) pt (nhmac-gui--font-px f) (nhmac-gui--font-px root) reopens monitors moves)
            (should (= reopens (if (= s (frame-scale-factor root)) 0 1)))
            (should (= (nhmac-gui--font-px f) (floor (+ (* pt s) 0.5))))
            ;; it keeps its size on the screen (points) at whichever scale it shows: the text follows
            (let ((now (list (/ (frame-pixel-width f) s) (/ (frame-pixel-height f) s))))
              (nhmac-gui--say "nhmac-new-frame-opens-its-font-once: made %S points, now %S points (%dx%d characters)"
                              made now (frame-width f) (frame-height f))
              (should (<= (abs (- (nth 0 now) (nth 0 made))) 1))
              (should (<= (abs (- (nth 1 now) (nth 1 made))) 1)))))
      (remove-hook 'nidhogg-frame-scale-change-functions on-scale)
      (remove-hook 'display-monitors-changed-functions on-monitors)
      (remove-hook 'move-frame-functions on-move)
      (when (frame-live-p f) (delete-frame f t))
      (nhmac-gui--settle))))

(ert-deftest nhmac-glyphs-measured-once ()
  "Redrawing text already shown makes no font driver call: its glyphs and their metrics are in the glyph tables."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (switch-to-buffer (generate-new-buffer " *nhmac-glyphs*"))
  (unwind-protect
      (progn
        (dotimes (_ 20) (insert "The quick brown fox jumps over the lazy dog 0123456789 (){}[]<>;:.,!?\n"))
        (redraw-frame)
        (redisplay t)
        (let ((before (nhmac-stats)))
          (dotimes (_ 5)
            (redraw-frame)
            (redisplay t))
          (nhmac-gui--say "nhmac-glyphs-measured-once: %d font calls over 5 full redraws"
                          (nhmac-gui--delta before :font-calls))
          (should (= (nhmac-gui--delta before :font-calls) 0))))
    (kill-buffer)))

(ert-deftest nhmac-new-frame-keeps-its-columns ()
  "A new frame AppKit puts on a screen of another scale shows the columns and lines it was made with: the window keeps
its points, and everything in it (font, fringes, borders) scales with it."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((f nil))
    (unwind-protect
        (progn
          (setq f (make-frame '((width . 40) (height . 10))))
          (dotimes (_ 3) (nhmac-gui--settle))
          (nhmac-gui--say "nhmac-new-frame-keeps-its-columns: scale %s: %dx%d characters, fringes %S"
                          (frame-scale-factor f) (frame-width f) (frame-height f) (window-fringes (frame-root-window f)))
          (should (equal (list (frame-width f) (frame-height f)) '(40 10))))
      (when (frame-live-p f) (delete-frame f t))
      (nhmac-gui--settle))))

(defun nhmac-gui--scale-at (x y)
  "The scale factor of the monitor whose geometry holds X, Y, as `display-monitor-attributes-list' gives it; nil."
  (cl-loop for m in (display-monitor-attributes-list)
           for g = (alist-get 'geometry m)
           when (and (<= (nth 0 g) x) (< x (+ (nth 0 g) (nth 2 g))) (<= (nth 1 g) y) (< y (+ (nth 1 g) (nth 3 g))))
           return (alist-get 'scale-factor m)))

(ert-deftest nhmac-new-frame-laid-out-once ()
  "A new frame opens on its screen at that screen's scale, and has the lines and columns it was asked for from its
first draw on, with no size change after it: nothing is measured at a size or scale it never shows at.  Without a
position it opens beside the selected frame, at its scale (the lane's root sits on its scale's screen); with `left'
and `top' it opens there, at the scale of the screen there (the main screen, the lane's other scale)."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((root (selected-frame)))
    (pcase-dolist (`(,case ,params ,want)
                   `((beside nil ,(frame-scale-factor root))
                     (placed ((left . 100) (top . 100)) ,(nhmac-gui--scale-at 100 100))))
      (let ((f nil) first)
        (unwind-protect
            (progn
              (setq f (make-frame (append '((width . 40) (height . 10)) params)))
              (nhmac-gui--settle)
              (setq first (list (frame-scale-factor f) (frame-width f) (frame-height f)
                                (frame-pixel-width f) (frame-pixel-height f)))
              (dotimes (_ 3) (nhmac-gui--settle))
              (let ((now (list (frame-scale-factor f) (frame-width f) (frame-height f)
                               (frame-pixel-width f) (frame-pixel-height f))))
                (nhmac-gui--say "nhmac-new-frame-laid-out-once: %s: root scale %s, want %s; first draw %S, after %S"
                                case (frame-scale-factor root) want first now)
                (should (equal (list case (nth 0 first)) (list case want)))
                (should (equal (list case (nth 1 first) (nth 2 first)) (list case 40 10)))
                (should (equal (list case now) (list case first)))))
          (when (frame-live-p f) (delete-frame f t))
          (nhmac-gui--settle))))))

(ert-deftest nhmac-monitor-change-runs-move-frame-functions ()
  "A root frame moved to another monitor runs `move-frame-functions' with it."
  :tags '(draws)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let* ((f (selected-frame)) (other (nhmac-gui--other-monitor #'always))
         (home (frame-monitor-attributes f)) (pos (frame-position f)) (ran nil)
         (hook (lambda (frame) (when (eq frame f) (push (frame-monitor-attributes frame) ran)))))
    (skip-unless other)
    (add-hook 'move-frame-functions hook)
    (unwind-protect
        (progn
          (nhmac-gui--move-to f other)
          (nhmac-gui--settle)
          (nhmac-gui--say "nhmac-monitor-change-runs-move-frame-functions: %d runs" (length ran))
          (should (cl-some (lambda (m) (equal (alist-get 'geometry m) (alist-get 'geometry other))) ran)))
      (remove-hook 'move-frame-functions hook)
      (nhmac-gui--move-to f home)
      (set-frame-position f (car pos) (cdr pos))
      (nhmac-gui--settle))))
