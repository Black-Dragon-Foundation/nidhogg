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

(provide 'nhwl-gui-rg-tests)

;;; nhwl-gui-rg-tests.el ends here
