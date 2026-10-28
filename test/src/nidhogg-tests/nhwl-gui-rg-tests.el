;;; nhwl-gui-rg-tests.el --- GUI rows for the native shell's glue in Rust  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh with the W1 rows (nhwl-gui-tests.el, whose helpers are used here).  The runner's default
;; file pattern (`-gui-tests.el') does not match this file's name: load it with `--files 'nhwl-gui*-tests.el''.  Each
;; port task appends its rows under `;; Port Task N:'.

(require 'ert)
(require 'nhwl-gui-tests
         (expand-file-name "nhwl-gui-tests" (file-name-directory (or load-file-name buffer-file-name))))

;; Port Task 1b: a panic inside the glue drops its display, never Emacs.  `nhwl--test-glue-panic' exists only in a
;; build with checking (the glue's `test-hooks').

(defun nhwl-gui--clients ()
  "The recorder's connected clients."
  (plist-get (gui-runner-recorder '(:cmd "clients")) :clients))

(defun nhwl-gui--toplevels ()
  "The number of toplevel surfaces the recorder shows."
  (seq-count (lambda (n) (equal (plist-get n :role) "toplevel")) (nhwl-gui--tree)))

;; The row drops the session's display, so it runs last (tag `runs-last').
(ert-deftest nhwl-gui/glue-panic-loses-the-display-not-emacs ()
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--recorder-p) (daemonp) (fboundp 'nhwl--test-glue-panic)))
  (let ((f (car (nhwl-gui--nhwl-frames))))
    (should f)
    (should (nhwl-gui--wait #'nhwl-gui--toplevel-commits))
    (should (= (length (nhwl-gui--clients)) 1))
    (should (= (nhwl--test-glue-panic f) -1))
    ;; within 1 s the frame is gone and the daemon is not
    (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.05) (not (frame-live-p f))) 1))
    (should (daemonp))
    (should (string-match-p "nhg_test_panic" (or (car (nhwl-glue-panics)) "")))
    ;; the connection was shut down: the recorder saw the client go, and its windows with it
    (should (nhwl-gui--wait (lambda () (null (nhwl-gui--clients))) 1))
    (should (= (nhwl-gui--toplevels) 0))
    ;; a new frame on the (restarted) compositor works
    (when (gui-runner-restart-available-p)
      (gui-runner-restart-compositor))
    (let ((new (make-frame-on-display (getenv "WAYLAND_DISPLAY"))))
      (should (frame-live-p new))
      (should (nhwl-gui--wait (lambda () (with-selected-frame new (redisplay t)) (nhwl-gui--toplevel-commits))
                              5)))))

;; Port Task 4a: the event drain is a step in the glue (one shell event per call), applied by C before the next.

(defun nhwl-gui--ctrl-key (text)
  "Type C-TEXT on the focused window (KEY_LEFTCTRL is evdev 29)."
  (gui-runner-recorder '(:cmd "key" :keycode 29 :state "press"))
  (gui-runner-recorder (list :cmd "key" :text text))
  (gui-runner-recorder '(:cmd "key" :keycode 29 :state "release")))

(defun nhwl-gui--read-status (file)
  "The plist a child Emacs last wrote to FILE, or nil."
  (ignore-errors
    (with-temp-buffer
      (insert-file-contents file)
      (read (current-buffer)))))

;; `emacs -nw' on a pty with a frame on the recorder: SIGINT runs `handle_interrupt' from the signal handler, and while
;; Emacs waits for input (`waiting_for_input') it longjmps out of whatever runs, the drain included.  NHG clears the flag
;; around every glue call, so the jump happens in C once the call is over.  A guard frame crossed by the jump would
;; leave the display's lock held, and the next call would poison the display: (nhwl-glue-panics) shows it.
(ert-deftest nhwl-gui/sigint-during-the-drain-never-crosses-rust ()
  :tags '(sigint)
  (skip-unless (and (nhwl-gui--recorder-p) (fboundp 'nhwl-glue-panics) (executable-find "script")))
  (let* ((dir (make-temp-file "nhwl-rg-sigint" t))
         (status (expand-file-name "status" dir))
         (init (expand-file-name "init.el" dir))
         (emacs (expand-file-name invocation-name invocation-directory))
         (process-environment (cons "TERM=xterm-256color" process-environment))
         (proc nil))
    (with-temp-file init
      (insert (format "%S\n"
                      `(progn
                         (defvar nh-quits 0)
                         (setq command-error-function
                               (lambda (data context caller)
                                 (when (eq (car-safe data) 'quit)
                                   (setq nh-quits (1+ nh-quits)))
                                 (command-error-default-function data context caller)))
                         ;; the shell frame is selected, so a SIGINT is a quit, not the tty's emergency escape
                         (let ((f (make-frame-on-display (getenv "WAYLAND_DISPLAY"))))
                           (select-frame-set-input-focus f)
                           (with-selected-frame f
                             (switch-to-buffer (get-buffer-create "typed"))))
                         (run-with-timer
                          0.1 0.1
                          (lambda ()
                            (with-temp-file ,status
                              (prin1 (list :pid (emacs-pid) :quits nh-quits
                                           :panics (length (nhwl-glue-panics))
                                           :typed (with-current-buffer "typed" (buffer-string)))
                                     (current-buffer)))))))))
    (unwind-protect
        (let (st pid)
          (setq proc (make-process :name "nhwl-rg-sigint" :connection-type 'pty :noquery t
                                   :buffer (generate-new-buffer " *nhwl-rg-sigint*")
                                   :command (list "script" "-qfc"
                                                  (format "%s -nw -Q -l %s"
                                                          (shell-quote-argument emacs)
                                                          (shell-quote-argument init))
                                                  "/dev/null")))
          (should (nhwl-gui--wait (lambda () (setq st (nhwl-gui--read-status status))) 30))
          (setq pid (plist-get st :pid))
          ;; the child's window is the newest toplevel
          (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
          (sleep-for 0.3)
          ;; a 200-key burst, with a SIGINT every 10 keys, 5 ms apart from the keys around it
          (dotimes (i 200)
            (gui-runner-recorder '(:cmd "key" :text "a"))
            (when (= (% i 10) 5)
              (sleep-for 0.005)
              (signal-process pid 'SIGINT)
              (sleep-for 0.005)))
          (sleep-for 1)
          (setq st (nhwl-gui--read-status status))
          (should (process-live-p proc))
          (should (> (plist-get st :quits) 0))
          (should (= (plist-get st :panics) 0))
          ;; the frame still takes input: 10 keys typed afterwards arrive
          (dolist (k (split-string "0123456789" "" t))
            (gui-runner-recorder (list :cmd "key" :text k)))
          (should (nhwl-gui--wait
                   (lambda ()
                     (setq st (nhwl-gui--read-status status))
                     (string-suffix-p "0123456789" (or (plist-get st :typed) "")))
                   5))
          (should (= (plist-get (nhwl-gui--read-status status) :panics) 0)))
      (when (process-live-p proc)
        (delete-process proc))
      (delete-directory dir t))))

;; A C-g inside a burst of keys quits once, and the other keys arrive in order: the drain stores the keys after C-g
;; while C-g waits in `hold_quit', as the C drain did.
(ert-deftest nhwl-gui/quit-in-a-key-burst ()
  (skip-unless (nhwl-gui--recorder-p))
  (let ((f (car (nhwl-gui--nhwl-frames)))
        (quits 0) (got nil))
    (should f)
    (with-selected-frame f
      (nhwl-gui--focus-toplevel)
      (nhwl-gui--pump 0.2)
      (condition-case nil
          (progn
            (dotimes (i 20) (gui-runner-recorder (list :cmd "key" :text (string (+ ?a i)))))
            (nhwl-gui--ctrl-key "g")
            (dotimes (i 20) (gui-runner-recorder (list :cmd "key" :text (string (+ ?A i))))))
        (quit (setq quits (1+ quits))))
      (let ((deadline (+ (float-time) 5)))
        (while (and (< (length got) 40) (< (float-time) deadline))
          (condition-case nil
              (let ((ev (read-event nil nil 0.1)))
                (cond ((eq ev ?\C-g) (setq quits (1+ quits)))
                      ((characterp ev) (push ev got))))
            (quit (setq quits (1+ quits)))))))
    (should (= quits 1))
    (should (equal (concat (nreverse got)) "abcdefghijklmnopqrstABCDEFGHIJKLMNOPQRST"))))

;; A frame deleted while its input and a configure are still queued in the shell, and one deleted from a timer during
;; redisplay: their events give nothing, nothing touches a freed frame, and the display keeps working.
(ert-deftest nhwl-gui/delete-frame-with-input-queued ()
  (skip-unless (and (nhwl-gui--recorder-p) (fboundp 'nhwl-glue-panics)))
  (let ((f (car (nhwl-gui--nhwl-frames)))
        (n (nhwl-gui--toplevels)))
    (should f)
    (let ((g (make-frame-on-display (getenv "WAYLAND_DISPLAY"))))
      (with-selected-frame g (redisplay t))
      (should (nhwl-gui--wait (lambda () (> (nhwl-gui--toplevels) n)) 5))
      ;; G's window is the newest toplevel: queue its keys, a pointer move and a configure, unread
      (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
      (dotimes (_ 10) (gui-runner-recorder '(:cmd "key" :text "x")))
      (gui-runner-recorder '(:cmd "pointer" :pos [10 10]))
      (gui-runner-recorder '(:cmd "configure" :surface "focused" :size [300 200]))
      (delete-frame g)
      (nhwl-gui--pump 0.5)
      (should-not (frame-live-p g)))
    (should (frame-live-p f))
    (should (null (nhwl-glue-panics)))
    (let ((h (make-frame-on-display (getenv "WAYLAND_DISPLAY"))))
      (with-selected-frame h (redisplay t))
      (should (nhwl-gui--wait (lambda () (> (nhwl-gui--toplevels) n)) 5))
      (gui-runner-recorder '(:cmd "configure" :surface "focused" :size [320 220]))
      (run-at-time 0 nil (lambda () (delete-frame h)))
      (redisplay t)
      (nhwl-gui--pump 0.5)
      (should-not (frame-live-p h)))
    (should (frame-live-p f))
    (should (null (nhwl-glue-panics)))
    ;; the display still draws and takes input
    (let ((mark (length (nhwl-gui--commits))))
      (with-selected-frame f
        (nhwl-gui--focus-toplevel)
        (should (nhwl-gui--run-key "z"))
        (redisplay t))
      (should (nhwl-gui--wait (lambda () (> (length (nhwl-gui--commits)) mark)))))))

;; Port Task 3b: presenting from the glue.  Emacs publishes each tree at a consistency point (after a redisplay, or a
;; draw outside one), and the shell presents it: on its own thread (`nhwl-presenter-thread', the default), or inline at
;; the tree's FRAME_READY (NHWL_PRESENTER=0).  Rows tagged `both-modes' run in both at the gate.

(defun nhwl-gui--thread-on-p ()
  "Non-nil when this Emacs presents on the shell's thread."
  (and (fboundp 'nhwl-presenter-stats) (eql (plist-get (nhwl-presenter-stats) :thread-on) 1)))

(defun nhwl-gui--since (mark)
  "Every commit since MARK."
  (plist-get (gui-runner-recorder (list :cmd "log" :since mark)) :commits))

(defun nhwl-gui--commit-kind (c)
  "Commit C's kind: `buffer', `ack' (an ack with a buffer), `bare-ack' (an ack without one) or `bare'."
  (let ((ack (nhwl-gui--get c :ack_serial)) (buf (nhwl-gui--get c :buffer)))
    (cond ((and ack buf) 'ack) (ack 'bare-ack) (buf 'buffer) (t 'bare))))

(defun nhwl-gui--toplevel-kinds (mark)
  "The kinds of the toplevel's commits since MARK, in order."
  (mapcar #'nhwl-gui--commit-kind
          (seq-filter (lambda (c) (equal (plist-get c :role) "toplevel")) (nhwl-gui--since mark))))

(defun nhwl-gui--grab-matches-snapshot ()
  "(MAX-CHANNEL-DIFF . PIXELS-OVER-2) between the recorder's composite and the selected frame's backing: the backing
sits at the toplevel's top-left, or at its native area when the snapshot is the native area only."
  (pcase-let* ((`(,w ,h ,bytes) (glitnir-snapshot nil 'raw))
               (b (plist-get (nhwl-gui--last-commit) :buffer))
               (whole (= w (plist-get b :w)))
               (x (if whole 0 (/ (- (plist-get b :w) w) 2)))
               (y (if whole 0 (nhwl-gui--native-top)))
               (`(,gw ,gh ,gbytes) (gui-runner-screen-grab))
               (sub (make-string (* 4 w h) 0 nil)))
    ;; the grab's part under the backing, as a W x H grab of its own
    (dotimes (row h)
      (let ((from (* 4 (+ x (* (+ y row) gw)))))
        (when (<= (+ y row) (1- gh))
          (store-substring sub (* 4 row w) (substring gbytes from (+ from (* 4 (min w (- gw x)))))))))
    (gui-runner-pixel-diff (list w h bytes) (list w h sub))))

(ert-deftest nhwl-gui/presenter-final-picture-matches-backing ()
  "Review Focus 2: 300 typed characters and 40 scrolls, each at a random point of the frame clock (a random wait of
0 to 20 ms before it) → once settled, the recorder's composite equals the backing (`glitnir-snapshot'): the last
published picture is the one shown, whichever packets were superseded."
  :tags '(both-modes)
  (skip-unless (and (nhwl-gui--recorder-p) (fboundp 'nhwl-presenter-stats)))
  (nhwl-gui--with-blink
   nil
   (lambda ()
     (let ((buf (generate-new-buffer " *nhwl-final*")))
       (random "nhwl-final")              ; the same waits every run
       (unwind-protect
           (progn
             (switch-to-buffer buf)
             (dotimes (i 120) (insert (format "line %d of the final-picture row\n" i)))
             (goto-char (point-min))
             (redisplay t)
             (dotimes (i 300)
               (insert (char-to-string (+ ?a (% i 26))))
               (when (zerop (% i 60)) (insert "\n"))
               (redisplay t)
               (sleep-for (/ (random 21) 1000.0)))
             (dotimes (i 40)
               (if (< (% i 10) 5) (scroll-up 3) (scroll-down 2))
               (redisplay t)
               (sleep-for (/ (random 21) 1000.0)))
             (redisplay t)
             (gui-runner-wait-settled)
             (should (equal (nhwl-gui--grab-matches-snapshot) '(0 . 0))))
         (kill-buffer buf))))))

(ert-deftest nhwl-gui/inline-and-thread-commit-the-same ()
  "A scripted session gives the same sequence of commit kinds in both modes (the gate runs it with and without
NHWL_PRESENTER=0): a typed character is one buffer commit; a configure of a new size, one ack with a buffer; a
configure of the same size and states, one bare ack; with nothing drawn, nothing."
  :tags '(both-modes)
  (skip-unless (and (nhwl-gui--recorder-p) (fboundp 'nhwl-presenter-stats)))
  (nhwl-gui--with-blink
   nil
   (lambda ()
     (let ((buf (generate-new-buffer " *nhwl-kinds*")))
       (unwind-protect
           (progn
             (nhwl-gui--windowed)
             (switch-to-buffer buf)
             (redisplay t)
             (gui-runner-wait-settled)
             (let ((mark (nhwl-gui--mark)))
               (insert "a")
               (redisplay t)
               (gui-runner-wait-settled)
               (should (equal (nhwl-gui--toplevel-kinds mark) '(buffer))))
             (let ((mark (nhwl-gui--mark)))
               (gui-runner-recorder '(:cmd "configure" :surface "focused" :size [760 540] :states ["activated"]))
               (should (nhwl-gui--wait (lambda () (redisplay t) (nhwl-gui--toplevel-kinds mark)) 5))
               (gui-runner-wait-settled)
               (should (equal (nhwl-gui--toplevel-kinds mark) '(ack))))
             (let ((mark (nhwl-gui--mark)))
               (gui-runner-recorder '(:cmd "configure" :surface "focused" :size [760 540] :states ["activated"]))
               (should (nhwl-gui--wait (lambda () (redisplay t) (nhwl-gui--toplevel-kinds mark)) 5))
               (gui-runner-wait-settled)
               (should (equal (nhwl-gui--toplevel-kinds mark) '(bare-ack))))
             (let ((mark (nhwl-gui--mark)))
               (redisplay t)
               (sit-for 0.5)
               (should (equal (nhwl-gui--toplevel-kinds mark) nil))))
         (kill-buffer buf)
         (nhwl-gui--windowed))))))

(ert-deftest nhwl-gui/resize-acks-ride-new-size-buffers ()
  "50 configures of random sizes at random intervals, presenting on the shell's thread: every commit that carries an
ack attaches a buffer of that configure's size (logical pixels), and the newest is acked."
  (skip-unless (and (nhwl-gui--recorder-p) (nhwl-gui--thread-on-p)))
  (let ((mark (nhwl-gui--mark)) (sizes nil) (last nil))
    (unwind-protect
        (progn
          (set-frame-parameter nil 'fullscreen nil)
          (dotimes (_ 50)
            (let* ((size (vector (+ 600 (random 300)) (+ 400 (random 300))))
                   (serial (plist-get (gui-runner-recorder
                                       (list :cmd "configure" :surface "focused" :size size
                                             :states ["activated"]))
                                      :serial)))
              (push (cons serial (append size nil)) sizes)
              (setq last serial)
              (insert "x")
              (redisplay t)
              (sleep-for (/ (random 31) 1000.0))))
          (should (nhwl-gui--wait
                   (lambda ()
                     (redisplay t)
                     (seq-find (lambda (c) (eql (nhwl-gui--get c :ack_serial) last)) (nhwl-gui--since mark)))
                   5))
          (gui-runner-wait-settled)
          (dolist (c (nhwl-gui--since mark))
            (let ((ack (nhwl-gui--get c :ack_serial)))
              (when (and ack (equal (plist-get c :role) "toplevel"))
                (should (nhwl-gui--get c :buffer))
                (should (equal (nhwl-gui--logical-size c) (cdr (assq ack sizes))))))))
      (nhwl-gui--windowed))))

(ert-deftest nhwl-gui/live-resize-thread ()
  "The live twin of `nhwl-gui/resize-acks-ride-new-size-buffers': resizes at random intervals while typing, on a
real compositor; the trace checker's rule `acks' judges that every ack rides a buffer of its configure's size."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--live-p) (nhwl-gui--nhwl-frames) (nhwl-gui--thread-on-p)))
  (let ((old (frame-parameter nil 'fullscreen)))
    (unwind-protect
        (progn
          (set-frame-parameter nil 'fullscreen nil)
          (nhwl-gui--live-settle 0.5)
          (dotimes (_ 30)
            (set-frame-size nil (+ 50 (random 30)) (+ 15 (random 15)))
            (insert "x")
            (redisplay t)
            (sleep-for (/ (random 31) 1000.0)))
          (nhwl-gui--live-settle 0.5)
          (should (frame-live-p (selected-frame))))
      (set-frame-parameter nil 'fullscreen old)
      (nhwl-gui--live-settle 0.5))))

(ert-deftest nhwl-gui/idle-publishes-nothing ()
  "With the cursor's blink off, 2 s of idle redisplays publish no packet."
  :tags '(both-modes)
  (skip-unless (and (nhwl-gui--recorder-p) (fboundp 'nhwl-presenter-stats)))
  (nhwl-gui--with-blink
   nil
   (lambda ()
     (redisplay t)
     (gui-runner-wait-settled)
     (let ((packets (plist-get (nhwl-presenter-stats) :packets)))
       (sit-for 2)
       (redisplay t)
       (should (= (plist-get (nhwl-presenter-stats) :packets) packets))))))

(ert-deftest nhwl-gui/bench-runs-through-the-presenter ()
  "`nhwl--bench-full-redraw' publishes each frame and waits for the presenter's commit, in both modes; on the thread
the presenter's own count of buffer commits rises by the frames presented."
  :tags '(both-modes)
  (skip-unless (and (nhwl-gui--recorder-p) (fboundp 'nhwl-presenter-stats)))
  (let* ((before (nhwl-presenter-stats))
         (r (nhwl--bench-full-redraw 30))
         (after (nhwl-presenter-stats)))
    (should (= (plist-get r :frames) 30))
    (should (> (plist-get r :present-interval-p95) 0))
    (should (>= (- (plist-get after :packets) (plist-get before :packets)) 30))
    (when (nhwl-gui--thread-on-p)
      (should (>= (- (plist-get after :commits) (plist-get before :commits)) 30)))))

(ert-deftest nhwl-gui/child-resize-while-typing-no-old-size-buffer ()
  "Review Focus 5: a child frame resized in the redisplay that also shows typing in its parent shows no buffer of
its old size after the resize: it keeps its last buffer until its own redisplay, then shows the new size only."
  :tags '(both-modes)
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((before (nhwl-gui--sub-ids))
         (c (nhwl-gui--child))
         (buf (generate-new-buffer " *nhwl-child-typing*"))
         (prev nil))
    (unwind-protect
        (let ((id (plist-get (nhwl-gui--child-node before) :id)))
          (should id)
          (switch-to-buffer buf)
          (dotimes (i 20)
            (let ((mark (nhwl-gui--mark)))
              (set-frame-size c (+ 12 (* 2 (% i 5))) (+ 3 (% i 3)))
              (insert "y")
              (redisplay t)
              (should (nhwl-gui--wait
                       (lambda ()
                         (redisplay t)
                         (seq-find (lambda (x) (and (eql (plist-get x :surface) id) (nhwl-gui--get x :buffer)))
                                   (nhwl-gui--since mark)))
                       5))
              (gui-runner-wait-settled)
              (let ((sizes (seq-uniq
                            (mapcar (lambda (x) (let ((b (plist-get x :buffer)))
                                                  (list (plist-get b :w) (plist-get b :h))))
                                    (seq-filter (lambda (x) (and (eql (plist-get x :surface) id)
                                                                 (nhwl-gui--get x :buffer)))
                                                (nhwl-gui--since mark))))))
                ;; one size per step, and a step that changed the size shows none of the old one
                (should (= (length sizes) 1))
                (when (and prev (not (equal (car sizes) prev)))
                  (should-not (member prev sizes)))
                (setq prev (car sizes))))))
      (when (frame-live-p c) (delete-frame c t))
      (kill-buffer buf))))

;; Port Task 4b: the keyboard's state, its repeats and the focus are the glue's; the key's target frame, the keysym
;; cascade and Emacs's focus stay C.

;; Review Focus 2: a press on a child frame and a key, queued in one burst and taken in one drain.  C applies the press
;; (the child takes its top-level's keys) before the glue takes the key, so the key reaches the child.  The recorder
;; moves the pointer over the surface with the keyboard: the burst focuses the child's surface for the press and the
;; top-level's again for the key.
(ert-deftest nhwl-gui/click-child-then-type-same-burst ()
  (skip-unless (nhwl-gui--recorder-p))
  (let ((parent (selected-frame)) (before (nhwl-gui--sub-ids)) (c nil) (got nil))
    (unwind-protect
        (progn
          (nhwl-gui--focus-toplevel)
          (select-frame-set-input-focus parent)
          (setq c (nhwl-gui--child))
          (let ((id (plist-get (nhwl-gui--child-node before) :id)))
            (should id)
            (nhwl-gui--pump)
            ;; the keys go to the parent before the click
            (should (eq (nhwl-gui--key-frame ?w) parent))
            ;; the burst, sent without reading in between
            (gui-runner-recorder (list :cmd "focus" :surface id))
            (gui-runner-recorder '(:cmd "pointer" :pos [10 10]))
            (gui-runner-recorder '(:cmd "button" :button 272 :state "press"))
            (gui-runner-recorder '(:cmd "button" :button 272 :state "release"))
            (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
            (gui-runner-recorder '(:cmd "key" :text "x"))
            (let ((deadline (+ (float-time) 3)))
              (while (and (not got) (< (float-time) deadline))
                (when (eql (read-event nil nil 0.05) ?x)
                  (setq got last-event-frame))))
            (unless (eq got c)
              (ert-fail (format "the key reached %S, the child is %S, the parent %S" got c parent)))))
      (ignore-errors (gui-runner-recorder '(:cmd "focus" :surface "toplevel")))
      (when (frame-live-p c) (delete-frame c t))
      (when (frame-live-p parent) (select-frame-set-input-focus parent)))))

;; A Lisp thread that redisplays and reads input while the user types: the glue runs in whichever Lisp thread holds the
;; global lock, so the drain and the presents move between threads.  No deadlock (the thread finishes), no panic, and
;; every key reaches a reader once, in order; once the thread is done, keys reach the command loop.
(ert-deftest nhwl-gui/lisp-thread-redisplay-and-input ()
  (skip-unless (and (nhwl-gui--recorder-p) (fboundp 'make-thread) (fboundp 'nhwl-glue-panics)))
  (let ((f (car (nhwl-gui--nhwl-frames))) (done nil) (in-thread nil) (in-main nil) (th nil))
    (should f)
    (with-selected-frame f
      (nhwl-gui--focus-toplevel)
      (nhwl-gui--pump 0.2)
      (setq th (make-thread
                (lambda ()
                  (dotimes (_ 100)
                    (redisplay t)
                    (let ((e (read-event nil nil 0.01)))
                      (when (characterp e) (push e in-thread))))
                  (setq done t))
                "nhwl-rg-thread"))
      (dotimes (i 20)
        (gui-runner-recorder (list :cmd "key" :text (string (+ ?a i))))
        (sleep-for 0.01))
      (let ((deadline (+ (float-time) 20)))
        (while (and (not done) (< (float-time) deadline))
          (let ((e (read-event nil nil 0.05)))
            (when (characterp e) (push e in-main)))))
      (should done)
      (thread-join th)
      ;; what the thread left unread
      (let ((deadline (+ (float-time) 1)))
        (while (< (float-time) deadline)
          (let ((e (read-event nil nil 0.05)))
            (when (characterp e) (push e in-main)))))
      (should (null (nhwl-glue-panics)))
      ;; each reader saw its keys in order, and together they saw all 20 once
      (should (equal (sort (append in-thread in-main nil) #'<)
                     (number-sequence ?a (+ ?a 19))))
      (should (equal (reverse in-thread) (sort (copy-sequence in-thread) #'<)))
      (should (equal (reverse in-main) (sort (copy-sequence in-main) #'<)))
      ;; the command loop's reads work afterwards
      (should (nhwl-gui--run-key "z"))
      (should (null (nhwl-glue-panics))))))

(provide 'nhwl-gui-rg-tests)

;;; nhwl-gui-rg-tests.el ends here
