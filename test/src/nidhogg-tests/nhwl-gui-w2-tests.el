;;; nhwl-gui-w2-tests.el --- GUI rows for the native shell's W2 (parity)  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh with the W1 rows (nhwl-gui-tests.el, loaded first: its helpers are used here).  Rows
;; that drive the compositor need `--compositor recorder'; rows named `nhwl-gui/live-*' run on real compositors and
;; are judged by the trace checker.

(require 'ert)
(require 'nhwl-gui-tests)

;; W2 Task 12: popup frames.  A frame made with `nhwl-popup' (and `parent-frame') is an xdg popup of its parent's
;; window, shown and hidden with the frame; with `:grab' it grabs the keyboard and pointer when the shell has an input
;; serial.  The menus (nhwl-menu.el) are made of them.

(defun nhwl-gui--press-serial ()
  "Click over the toplevel, so the shell has a button serial to grab with; the click's events are read and dropped."
  (nhwl-gui--point 100 100)
  (gui-runner-recorder '(:cmd "button" :button 272 :state "press"))
  (gui-runner-recorder '(:cmd "button" :button 272 :state "release"))
  (nhwl-gui--pump))

(defun nhwl-gui--popup-frame (parent anchor &rest params)
  "A shown popup frame of PARENT at ANCHOR (the `nhwl-popup' value), 20 x 5 characters; PARAMS first."
  (make-frame `(,@params (parent-frame . ,parent) (nhwl-popup . ,anchor) (width . 20) (height . 5)
                (minibuffer . nil) (undecorated . t) (no-other-frame . t))))

(defun nhwl-gui--popup-node (before)
  "The newest popup node not in the id list BEFORE once it shows a buffer, waiting up to 5 s."
  (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (nhwl-gui--new-popup before)) 5))

(defun nhwl-gui--grabbed-ids ()
  "The ids of the popups that hold a grab."
  (mapcar (lambda (n) (plist-get n :id))
          (seq-filter (lambda (n) (and (equal (plist-get n :role) "popup") (nhwl-gui--get n :grab)))
                      (nhwl-gui--tree))))

(defun nhwl-gui--connected-p ()
  "Non-nil while this Emacs is a client of the recorder (a protocol error would have disconnected it)."
  (seq-find (lambda (c) (eql (plist-get c :pid) (emacs-pid)))
            (plist-get (gui-runner-recorder '(:cmd "clients")) :clients)))

(ert-deftest nhwl-gui/popup-frame-is-an-xdg-popup-with-grab ()
  "A popup frame shown after a click is an xdg popup of its parent's window that grabs with the click's serial, and
its buffer is the frame's size."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((before (nhwl-gui--popup-ids)) (p nil))
    (unwind-protect
        (progn
          (nhwl-gui--press-serial)
          (setq p (nhwl-gui--popup-frame (selected-frame) '(40 30 1 1 :grab t)))
          (should (eq (frame-parent p) (selected-frame)))
          (should (frame-visible-p p))
          (let ((n (nhwl-gui--popup-node before)))
            (should n)
            (should (equal (plist-get (plist-get n :grab) :serial_kind) "button"))
            (should (nhwl-gui--wait (lambda ()
                                      (nhwl-gui--pump 0.1)
                                      (let ((b (plist-get (nhwl-gui--node (plist-get n :id)) :buffer)))
                                        (and (eql (plist-get b :w) (frame-pixel-width p))
                                             (eql (plist-get b :h) (frame-pixel-height p)))))
                                    5))))
      (when (frame-live-p p) (delete-frame p)))))

(ert-deftest nhwl-gui/popup-frame-without-serial-has-no-grab ()
  "On a connection that has seen no input (a second recorder), a popup frame that asks for a grab gets none: the
shell sends no `grab' (a stale serial would be a dismissal), and the popup is shown all the same."
  (skip-unless (and (nhwl-gui--recorder-p) (getenv "GLITNIR_GUI_RECORDER_BIN")))
  (let* ((second (nhwl-gui--second-recorder)) (f nil) (p nil)
         (process-environment (cons (concat "GLITNIR_GUI_RECORDER=" (cdr second)) process-environment)))
    (unwind-protect
        (progn
          (setq f (make-frame-on-display "nhwl-second"))
          (should (nhwl-gui--wait (lambda () (with-selected-frame f (redisplay t)) (nhwl-gui--toplevel-commits)) 5))
          (setq p (nhwl-gui--popup-frame f '(40 30 1 1 :grab t)))
          (let ((n (nhwl-gui--wait (lambda () (redisplay t) (nhwl-gui--new-popup nil)) 5)))
            (should n)
            (should-not (nhwl-gui--get n :grab))
            (should (frame-visible-p p))))
      (when (frame-live-p p) (delete-frame p))
      (when (frame-live-p f) (delete-frame f t))
      (delete-process (car second)))))

(ert-deftest nhwl-gui/menu-grab-keeps-frame-focus ()
  "With a grabbing popup frame shown and the keyboard moved to it, the frame under it keeps Emacs's focus: no
focus-out, `frame-focus-state' stays t, and a key typed on the popup arrives in the popup frame."
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((parent (selected-frame)) (before (nhwl-gui--popup-ids)) (outs 0) (p nil)
         (count (lambda () (setq outs (1+ outs)))))
    (unwind-protect
        (progn
          (nhwl-gui--focus-toplevel)
          (nhwl-gui--pump)
          (should (eq (frame-focus-state parent) t))
          (add-hook 'focus-out-hook count)
          (nhwl-gui--press-serial)
          (setq p (nhwl-gui--popup-frame parent '(40 30 1 1 :grab t)))
          (let ((n (nhwl-gui--popup-node before)))
            (should n)
            (should (nhwl-gui--get n :grab))
            ;; the compositor gives the grabbing popup the keyboard (the recorder does it when told)
            (gui-runner-recorder (list :cmd "focus" :surface (plist-get n :id)))
            (nhwl-gui--pump)
            (should (= outs 0))
            (should (eq (frame-focus-state parent) t))
            (should (eq (nhwl-gui--key-frame ?x) p))
            (should (= outs 0))))
      (remove-hook 'focus-out-hook count)
      (when (frame-live-p p) (delete-frame p))
      (nhwl-gui--focus-toplevel))))

(ert-deftest nhwl-gui/popup-frame-dismissed-sends-delete-frame ()
  "A click outside dismisses the grabbing popup: Lisp reads a `delete-frame' event for its frame, which stays live
until Lisp deletes or hides it."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((before (nhwl-gui--popup-ids)) (p nil))
    (unwind-protect
        (progn
          (nhwl-gui--press-serial)
          (setq p (nhwl-gui--popup-frame (selected-frame) '(40 30 1 1 :grab t)))
          (should (nhwl-gui--get (nhwl-gui--popup-node before) :grab))
          (gui-runner-recorder '(:cmd "click_outside"))
          ;; with the global binding (`handle-delete-frame') out of the way, the event comes back from `read-event'
          (let* ((special-event-map (make-sparse-keymap))
                 (ev (nhwl-gui--wait (lambda ()
                                       (let ((e (read-event nil nil 0.1)))
                                         (and (eq (car-safe e) 'delete-frame) e)))
                                     3)))
            (should (equal ev (list 'delete-frame (list p)))))
          (should (frame-live-p p))
          (delete-frame p)
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (equal (nhwl-gui--popup-ids) before)) 5)))
      (when (frame-live-p p) (delete-frame p)))))

(ert-deftest nhwl-gui/nested-popup-frames-destroyed-topmost-first ()
  "Three grabbing popup frames, each on the one before.  Hiding the first hides the other two, and deleting it
deletes them; either way the popups go topmost first, so the compositor never sees `not_the_topmost_popup' (the
recorder ends a client for it), and only the toplevel is left."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((parent (selected-frame)) (before (nhwl-gui--popup-ids)) (frames nil)
        (all-shown (lambda ()
                     (nhwl-gui--wait (lambda ()
                                       (nhwl-gui--pump 0.1)
                                       (= (length (seq-difference (nhwl-gui--grabbed-ids) before)) 3))
                                     5)))
        (none-left (lambda ()
                     (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (equal (nhwl-gui--popup-ids) before)) 5))))
    (unwind-protect
        (progn
          (nhwl-gui--press-serial)
          (let ((p parent))
            (dotimes (_ 3)
              (setq p (nhwl-gui--popup-frame p '(20 10 1 1 :grab t :gravity right)))
              (push p frames)))
          (setq frames (nreverse frames))
          (should (funcall all-shown))
          (make-frame-invisible (car frames))
          (should (funcall none-left))
          (should-not (seq-some #'frame-visible-p frames))
          (should (nhwl-gui--connected-p))
          (dolist (f frames) (make-frame-visible f))
          (should (funcall all-shown))
          (delete-frame (car frames))
          (should (funcall none-left))
          (should-not (seq-some #'frame-live-p frames))
          (should (frame-live-p parent))
          (should (nhwl-gui--connected-p)))
      (dolist (f (reverse frames)) (when (frame-live-p f) (delete-frame f))))))

(ert-deftest nhwl-gui/popup-frame-resize-repositions ()
  "A popup frame that changes size gives its popup the new size (`xdg_popup.reposition'; below version 3 the popup
is made again and drawn whole): the popup's buffer follows the frame, and it stays shown."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((before (nhwl-gui--popup-ids)) (p nil))
    (unwind-protect
        (progn
          (setq p (nhwl-gui--popup-frame (selected-frame) '(40 30 1 1)))
          (should (nhwl-gui--popup-node before))
          (dolist (size '((30 . 8) (12 . 3)))
            (set-frame-size p (car size) (cdr size))
            (should (nhwl-gui--wait
                     (lambda ()
                       (nhwl-gui--pump 0.1)
                       (redisplay t)
                       (let* ((n (seq-find (lambda (n) (and (equal (plist-get n :role) "popup")
                                                            (not (memq (plist-get n :id) before))))
                                           (nhwl-gui--tree)))
                              (b (plist-get n :buffer)))
                         (and (nhwl-gui--get n :mapped)
                              (eql (plist-get b :w) (frame-pixel-width p))
                              (eql (plist-get b :h) (frame-pixel-height p)))))
                     5))))
      (when (frame-live-p p) (delete-frame p)))))

(ert-deftest nhwl-gui/live-popup-frames ()
  "Popup frames on a real compositor: three levels shown, hidden, shown again, resized, moved and deleted.  The trace
rules (errors, acks, unconfigured, popup, grab, popup-order) judge what was sent."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--live-p) (nhwl-gui--nhwl-frames)))
  (let ((frames nil))
    (unwind-protect
        (let ((p (selected-frame)))
          (dotimes (_ 3)
            (setq p (nhwl-gui--popup-frame p '(20 10 1 1 :grab t :gravity right)))
            (push p frames))
          (setq frames (nreverse frames))
          (nhwl-gui--live-settle 0.5)
          (make-frame-invisible (car frames))
          (nhwl-gui--live-settle 0.3)
          (dolist (f frames) (make-frame-visible f))
          (nhwl-gui--live-settle 0.5)
          (set-frame-size (nth 2 frames) 30 8)
          (nhwl-gui--live-settle 0.3)
          (set-frame-parameter (car frames) 'nhwl-popup '(60 40 1 1 :grab t :gravity right))
          (nhwl-gui--live-settle 0.3)
          (delete-frame (car frames))
          (nhwl-gui--live-settle 0.3)
          (should-not (seq-some #'frame-live-p frames))
          (should (frame-live-p (selected-frame))))
      (dolist (f (reverse frames)) (when (frame-live-p f) (delete-frame f))))))

;; The bell, idle inhibition, export handles and activation tokens.  The system bell, the idle inhibitor and the
;; exporter are protocols of the recorder's profile (mutter has all three; sway has no system bell), and the recorder
;; logs what a client asked of them: `bell_log', `idle_inhibitors' and `exports'.

(defun nhwl-gui--has-global-p (name)
  "Non-nil when the recorder's profile offers the global NAME (a keyword such as `:xdg_system_bell_v1')."
  (plist-get (plist-get (gui-runner-recorder '(:cmd "globals")) :globals) name))

(defun nhwl-gui--toplevel-id ()
  "The id of the recorder's toplevel surface."
  (plist-get (seq-find (lambda (n) (equal (plist-get n :role) "toplevel")) (nhwl-gui--tree)) :id))

(defun nhwl-gui--bells ()
  "The recorder's bell log, a list of surface ids (nil for a ring without a surface)."
  (mapcar (lambda (b) (nhwl-gui--get b :surface)) (plist-get (gui-runner-recorder '(:cmd "bell_log")) :bells)))

(defun nhwl-gui--live-inhibitors ()
  "How many idle inhibitors the recorder has that are alive."
  (length (seq-filter (lambda (i) (eq (plist-get i :alive) t))
                      (plist-get (gui-runner-recorder '(:cmd "idle_inhibitors")) :inhibitors))))

(ert-deftest nhwl-gui/visible-bell-flashes-and-restores ()
  "`visible-bell' t and `ding': a commit with damage on the top and bottom lines, then within 200 ms another that
restores them; the recorder's composite equals the pre-bell composite."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((visible-bell t) (ring-bell-function nil) (before nil) (mark nil) (flash nil) (restore nil))
    (redisplay t)
    (gui-runner-wait-settled)
    (setq before (gui-runner-screen-grab))
    (setq mark (nhwl-gui--mark))
    (ding)
    (let* ((h (nth 1 before))
           (line (frame-char-height))
           (top (lambda (c) (seq-some (lambda (r) (< (nth 1 r) (* 3 line))) (nhwl-gui--damage c))))
           (bottom (lambda (c) (seq-some (lambda (r) (> (+ (nth 1 r) (nth 3 r)) (- h (* 4 line))))
                                         (nhwl-gui--damage c)))))
      (setq flash (nhwl-gui--wait (lambda ()
                                    (nhwl-gui--pump 0.05)
                                    (seq-find (lambda (c) (and (funcall top c) (funcall bottom c)))
                                              (nhwl-gui--toplevel-all-commits mark)))
                                  3))
      (should flash)
      ;; the restore follows the flash by about 50 ms, in a commit of its own
      (setq restore (nhwl-gui--wait (lambda ()
                                      (nhwl-gui--pump 0.05)
                                      (seq-find (lambda (c) (and (> (plist-get c :seq) (plist-get flash :seq))
                                                                 (funcall top c)))
                                                (nhwl-gui--toplevel-all-commits mark)))
                                    1))
      (should restore))
    (gui-runner-wait-settled)
    (should (<= (car (gui-runner-pixel-diff before (gui-runner-screen-grab))) 2))))

(ert-deftest nhwl-gui/bell-rings-the-system-bell ()
  "Mutter profile (xdg_system_bell_v1): `ding' without `visible-bell' logs a ring on the toplevel's surface."
  (skip-unless (and (nhwl-gui--recorder-p) (nhwl-gui--has-global-p :xdg_system_bell_v1)))
  (let ((visible-bell nil) (ring-bell-function nil) (id (nhwl-gui--toplevel-id)))
    (should id)
    (ding)
    (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.05) (member id (nhwl-gui--bells))) 3))))

(ert-deftest nhwl-gui/bell-without-protocol-does-nothing ()
  "Sway profile (no xdg_system_bell_v1): `ding' logs no ring, and the connection stays up."
  (skip-unless (and (nhwl-gui--recorder-p) (not (nhwl-gui--has-global-p :xdg_system_bell_v1))))
  (let ((visible-bell nil) (ring-bell-function nil))
    (ding)
    (nhwl-gui--pump 0.3)
    (should-not (nhwl-gui--bells))
    (should (nhwl-gui--connected-p))))

(ert-deftest nhwl-gui/inhibit-idle-parameter ()
  "The `inhibit-idle' frame parameter t holds one idle inhibitor, nil releases it, and a second t holds one again."
  (skip-unless (and (nhwl-gui--recorder-p) (nhwl-gui--has-global-p :zwp_idle_inhibit_manager_v1)))
  (unwind-protect
      (progn
        (should (zerop (nhwl-gui--live-inhibitors)))
        (set-frame-parameter nil 'inhibit-idle t)
        (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.05) (= (nhwl-gui--live-inhibitors) 1)) 3))
        ;; set again: still one
        (set-frame-parameter nil 'inhibit-idle t)
        (nhwl-gui--pump 0.2)
        (should (= (nhwl-gui--live-inhibitors) 1))
        (set-frame-parameter nil 'inhibit-idle nil)
        (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.05) (zerop (nhwl-gui--live-inhibitors))) 3))
        (set-frame-parameter nil 'inhibit-idle t)
        (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.05) (= (nhwl-gui--live-inhibitors) 1)) 3)))
    (set-frame-parameter nil 'inhibit-idle nil)))

(ert-deftest nhwl-gui/export-handle-matches-the-compositor ()
  "`nhwl-frame-export-handle' is \"wayland:\" and the handle the compositor made for the toplevel, and asking again
gives the same one."
  (skip-unless (and (nhwl-gui--recorder-p) (nhwl-gui--has-global-p :zxdg_exporter_v2)))
  (let ((handle (nhwl-frame-export-handle)))
    (should (stringp handle))
    (should (string-prefix-p "wayland:" handle))
    (let ((exports (plist-get (gui-runner-recorder '(:cmd "exports")) :exports)))
      (should (= (length exports) 1))
      (should (equal (substring handle (length "wayland:")) (plist-get (seq-first exports) :handle))))
    (should (equal (nhwl-frame-export-handle) handle))))

(ert-deftest nhwl-gui/activation-token-from-a-click ()
  "Before any input there is no token; after a click `nhwl-activation-token' returns one, and every call a fresh one."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((second (and (getenv "GLITNIR_GUI_RECORDER_BIN") (nhwl-gui--second-recorder))) (f nil))
    ;; a connection that has seen no input has no serial to make a token with
    (when second
      (let ((process-environment (cons (concat "GLITNIR_GUI_RECORDER=" (cdr second)) process-environment)))
        (unwind-protect
            (progn
              (setq f (make-frame-on-display "nhwl-second"))
              (should-not (nhwl-activation-token f)))
          (when (frame-live-p f) (delete-frame f t))
          (delete-process (car second)))))
    (nhwl-gui--press-serial)
    (let ((token (nhwl-activation-token)))
      (should (stringp token))
      (should (> (length token) 0))
      (should-not (equal (nhwl-activation-token) token)))))

;; Session restore (lisp/term/nhwl-session.el).  The recorder's mutter profile offers xdg_session_manager_v1 and keeps
;; sessions in the `--session-store' file gui.sh gives it; its `session_log' lists what a client asked of the
;; session.  The first frame is made before these rows run, so they read what it asked for and make more frames.

(defun nhwl-gui--session-p ()
  "Non-nil on the recorder with session management."
  (and (nhwl-gui--recorder-p) (nhwl-gui--has-global-p :xdg_session_manager_v1)))

(defun nhwl-gui--session-log (op)
  "The recorder's session log entries whose :op is OP."
  (seq-filter (lambda (e) (equal (plist-get e :op) op))
              (plist-get (gui-runner-recorder '(:cmd "session_log")) :session_log)))

(ert-deftest nhwl-gui/session-opened-once-per-display ()
  "One session was asked for at the display's first frame, as a launch; more frames do not ask again, and the first
frame joined it as \"initial\"."
  (skip-unless (nhwl-gui--session-p))
  (should (= (length (nhwl-gui--session-log "get_session")) 1))
  (should (equal (plist-get (car (nhwl-gui--session-log "get_session")) :reason) "launch"))
  (let ((f (make-frame '(( minibuffer . nil)))))
    (unwind-protect
        (progn
          (nhwl-gui--pump 0.3)
          (should (= (length (nhwl-gui--session-log "get_session")) 1))
          (should (stringp (frame-parameter f 'session-name)))
          (should-not (equal (frame-parameter f 'session-name) "initial"))
          (should (seq-some (lambda (fr) (equal (frame-parameter fr 'session-name) "initial")) (frame-list))))
      (when (frame-live-p f) (delete-frame f)))))

(ert-deftest nhwl-gui/frame-restores-before-its-first-commit ()
  "A frame made with a `session-name' the session knows is restored, and the restore went out before the frame's
first commit.  The name is one an earlier frame left in the session."
  (skip-unless (nhwl-gui--session-p))
  (let ((f (make-frame '((session-name . "row-restore") (minibuffer . nil)))))
    (unwind-protect
        (progn
          (nhwl-gui--pump 0.3)
          (should (nhwl-gui--wait (lambda ()
                                    (nhwl-gui--pump 0.05)
                                    (seq-find (lambda (e) (equal (plist-get e :name) "row-restore"))
                                              (nhwl-gui--session-log "add")))
                                  3))
          (should (eq (plist-get (seq-find (lambda (e) (equal (plist-get e :name) "row-restore"))
                                           (nhwl-gui--session-log "add"))
                                 :before_first_commit)
                      t))
          ;; deleted and made again under the same name: it left the session with the frame, so it is an add again,
          ;; and the compositor raises no protocol error
          (delete-frame f)
          (setq f (make-frame '((session-name . "row-restore") (minibuffer . nil))))
          (nhwl-gui--pump 0.3)
          (should (nhwl-gui--connected-p)))
      (when (frame-live-p f) (delete-frame f)))))

(ert-deftest nhwl-gui/user-deleted-frame-leaves-the-session ()
  "A frame deleted before Emacs exits is removed from the session (a `remove' with its name)."
  (skip-unless (nhwl-gui--session-p))
  (let ((f (make-frame '((session-name . "row-deleted") (minibuffer . nil)))))
    (nhwl-gui--pump 0.3)
    (delete-frame f)
    (should (nhwl-gui--wait (lambda ()
                              (nhwl-gui--pump 0.05)
                              (seq-find (lambda (e) (equal (plist-get e :name) "row-deleted"))
                                        (nhwl-gui--session-log "remove")))
                            3))))

(ert-deftest nhwl-gui/session-replaced-stops-naming ()
  "After the answer `replaced' (another client took the session over) new frames get no name and send nothing to
the session, and the connection stays up.  The takeover itself is the glue's and the shell's tests; the answer
is delivered here as the special event C makes of it."
  (skip-unless (nhwl-gui--session-p))
  (let ((adds (length (nhwl-gui--session-log "add"))) (f nil) (old nhwl-session--state))
    (unwind-protect
        (progn
          (nhwl-session '(nhwl-session (replaced)))
          (setq f (make-frame '((minibuffer . nil))))
          (nhwl-gui--pump 0.3)
          (should-not (frame-parameter f 'session-name))
          (should (= (length (nhwl-gui--session-log "add")) adds))
          (should (nhwl-gui--connected-p)))
      (setq nhwl-session--state old)
      (when (frame-live-p f) (delete-frame f)))))

(ert-deftest nhwl-gui/no-session-manager-does-nothing ()
  "Sway profile (no xdg_session_manager_v1): frames get no session name and the connection stays up."
  (skip-unless (and (nhwl-gui--recorder-p) (not (nhwl-gui--has-global-p :xdg_session_manager_v1))))
  (should (eq nhwl-session--state 'none))
  (let ((f (make-frame '((minibuffer . nil)))))
    (unwind-protect
        (progn
          (nhwl-gui--pump 0.3)
          (should-not (frame-parameter f 'session-name))
          (should (nhwl-gui--connected-p)))
      (when (frame-live-p f) (delete-frame f)))))

(ert-deftest nhwl-gui/live-session ()
  "Session restore on a real compositor (mutter): frames made, deleted and made again under their names.  The trace
rule `errors' judges what was sent."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--live-p) (nhwl-gui--nhwl-frames)))
  (let ((frames nil))
    (unwind-protect
        (progn
          (dotimes (_ 2) (push (make-frame '((minibuffer . nil))) frames))
          (nhwl-gui--live-settle 0.5)
          (let ((name (frame-parameter (car frames) 'session-name)))
            (delete-frame (car frames))
            (setq frames (cdr frames))
            (nhwl-gui--live-settle 0.3)
            (push (make-frame `((session-name . ,name) (minibuffer . nil))) frames)
            (nhwl-gui--live-settle 0.5))
          (should (frame-live-p (selected-frame))))
      (dolist (f frames) (when (frame-live-p f) (delete-frame f))))))

;; W2 Task 10: the drop target.  The recorder is a foreign source (`dnd_offer', `dnd_motion', `dnd_drop', read back
;; with `dnd_log'); `pgtk-dnd.el' decides what a drop does.  Positions are logical surface pixels of the toplevel.

(defun nhwl-gui--dnd-offer (types &rest args)
  "A foreign drag enters the toplevel at (100, 100) offering TYPES, a plist of MIME keyword and text; ARGS override."
  (let ((req (list :cmd "dnd_offer" :types types :actions ["copy" "move"] :preferred "copy"
                   :surface "toplevel" :at [100 100])))
    (while args (setq req (plist-put req (pop args) (pop args))))
    (gui-runner-recorder req)
    (nhwl-gui--pump 0.3)))

(defun nhwl-gui--dnd-log ()
  "The newest offer the recorder has logged."
  (car (last (plist-get (gui-runner-recorder '(:cmd "dnd_log")) :offers))))

(defun nhwl-gui--dnd-drop-on (buffer &optional at)
  "Show BUFFER (emptied) in the selected window, drag text onto it (at AT, a vector, else (100, 100)) and drop."
  (switch-to-buffer buffer)
  (let ((inhibit-read-only t)) (erase-buffer))
  (nhwl-gui--dnd-offer '(:text/plain "dropped text") :at (or at [100 100]))
  (gui-runner-recorder `(:cmd "dnd_motion" :at ,(or at [100 100])))
  (nhwl-gui--pump 0.3)
  (gui-runner-recorder '(:cmd "dnd_drop"))
  (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1)
                    (with-current-buffer buffer (> (buffer-size) 0)))
                  3))

(ert-deftest nhwl-gui/drop-text-inserts-at-the-drop-point ()
  "Dropping text on a window inserts it, and the source is told the drop finished."
  :tags '(gnome-smoke)
  (skip-unless (nhwl-gui--recorder-p))
  (let ((b (generate-new-buffer " *dnd*")))
    (unwind-protect
        (progn
          (should (nhwl-gui--dnd-drop-on b))
          (should (equal (with-current-buffer b (buffer-string)) "dropped text"))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1)
                                    (nhwl-gui--get (nhwl-gui--dnd-log) :finished))
                                  3)))
      (kill-buffer b))))

(ert-deftest nhwl-gui/drop-uri-list-opens-the-file ()
  "Dropping a `text/uri-list' of one file visits it."
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((file (make-temp-file "nhwl-dnd-" nil ".txt" "from a file\n")) (b nil))
    (unwind-protect
        (progn
          (switch-to-buffer (get-buffer-create " *dnd-host*"))
          (nhwl-gui--dnd-offer (list :text/uri-list (concat "file://" file "\r\n")))
          (gui-runner-recorder '(:cmd "dnd_motion" :at [100 100]))
          (nhwl-gui--pump 0.3)
          (gui-runner-recorder '(:cmd "dnd_drop"))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (setq b (find-buffer-visiting file))) 3)))
      (when b (kill-buffer b))
      (kill-buffer " *dnd-host*")
      (delete-file file))))

(ert-deftest nhwl-gui/drop-on-the-mode-line-inserts-at-point ()
  "A drop on the mode line has no buffer position: the text goes in at point."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((b (generate-new-buffer " *dnd*")))
    (unwind-protect
        (progn
          (switch-to-buffer b)
          (insert "abc")
          (goto-char 2)
          (let ((y (+ (nth 3 (window-inside-pixel-edges)) 3)))
            (nhwl-gui--dnd-offer '(:text/plain "X") :at (vector 60 y))
            (gui-runner-recorder `(:cmd "dnd_motion" :at [60 ,y]))
            (nhwl-gui--pump 0.3)
            (gui-runner-recorder '(:cmd "dnd_drop")))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (> (buffer-size) 3)) 3))
          (should (equal (buffer-string) "aXbc")))
      (kill-buffer b))))

(ert-deftest nhwl-gui/drop-on-a-child-frame-goes-to-the-child ()
  "A drop over a child frame is the child's: its window gets the text and the toplevel's does not."
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((parent (selected-frame)) (b (generate-new-buffer " *dnd-child*")) (child nil)
         (host (get-buffer-create " *dnd-host*")))
    (unwind-protect
        (progn
          (switch-to-buffer host)
          (setq child (make-frame `((parent-frame . ,parent) (left . 20) (top . 20) (width . 30) (height . 8)
                                    (minibuffer . nil) (no-other-frame . t))))
          (with-selected-frame child (switch-to-buffer b))
          (nhwl-gui--pump 0.5)
          (nhwl-gui--dnd-offer '(:text/plain "to the child") :surface "newest" :at [40 40])
          (gui-runner-recorder '(:cmd "dnd_motion" :at [40 40] :surface "newest"))
          (nhwl-gui--pump 0.3)
          (gui-runner-recorder '(:cmd "dnd_drop"))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (> (buffer-size b) 0)) 3))
          (should (equal (with-current-buffer b (buffer-string)) "to the child"))
          (should (zerop (buffer-size host))))
      (when (frame-live-p child) (delete-frame child))
      (kill-buffer b)
      (kill-buffer host))))

(ert-deftest nhwl-gui/drop-unknown-type-is-refused ()
  "A drag that offers only `image/png' is not accepted and is never finished."
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--dnd-offer '(:image/png "PNGDATA"))
  (gui-runner-recorder '(:cmd "dnd_motion" :at [101 101]))
  (nhwl-gui--pump 0.5)
  (let ((log (nhwl-gui--dnd-log)))
    (should-not (stringp (nhwl-gui--get log :accepted)))
    (should-not (nhwl-gui--get log :finished)))
  (gui-runner-recorder '(:cmd "dnd_leave"))
  (nhwl-gui--pump 0.3))

(ert-deftest nhwl-gui/drop-from-a-silent-source-times-out ()
  "A source that never sends its data: Emacs gives up at `nhwl-selection-timeout', destroys the offer within 1.5 s
of the drop, answers at once after it, and the next, normal drop inserts."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((b (generate-new-buffer " *dnd*")))
    (unwind-protect
        (progn
          (switch-to-buffer b)
          (nhwl-gui--dnd-offer '(:text/plain "never read") :silent t)
          (gui-runner-recorder '(:cmd "dnd_motion" :at [101 101]))
          (nhwl-gui--pump 0.3)
          (gui-runner-recorder '(:cmd "dnd_drop"))
          (let ((t0 (float-time)))
            (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.05)
                                      (nhwl-gui--get (nhwl-gui--dnd-log) :destroyed))
                                    1.5))
            (should (= (+ 1 1) 2))
            (should (< (- (float-time) t0) 1.6)))
          (should (zerop (buffer-size b)))
          (should (nhwl-gui--dnd-drop-on b))
          (should (equal (with-current-buffer b (buffer-string)) "dropped text")))
      (kill-buffer b))))

(ert-deftest nhwl-gui/drop-motion-shows-the-insertion-point ()
  "With `dnd-indicate-insertion-point', a drag moving over the text moves point to where the drop would go."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((b (generate-new-buffer " *dnd*")) (dnd-indicate-insertion-point t))
    (unwind-protect
        (progn
          (switch-to-buffer b)
          (insert (mapconcat #'identity (make-list 10 "0123456789") "\n"))
          (goto-char (point-min))
          (let ((y (+ (nth 1 (window-inside-pixel-edges)) (* 4 (default-line-pixel-height)) 2)))
            (nhwl-gui--dnd-offer '(:text/plain "x") :at (vector 30 y))
            (gui-runner-recorder `(:cmd "dnd_motion" :at [32 ,y])))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (= (line-number-at-pos) 5)) 3))
          (gui-runner-recorder '(:cmd "dnd_leave"))
          (nhwl-gui--pump 0.3))
      (kill-buffer b))))

;; W2 Task 15: touch screens and touchpad pinch.  The recorder's `touch' and `pinch' commands stand in for the
;; compositor; the events Lisp sees are pgtk's: `touchscreen-begin', `touchscreen-update', `touchscreen-end', `pinch'.

(defvar nhwl-gui--touch-log nil "The events `touch-screen-handle-touch' saw, newest first.")

(defun nhwl-gui--touch-note (event &rest _)
  (push event nhwl-gui--touch-log))

(defmacro nhwl-gui--with-touch-log (&rest body)
  "Run BODY with the touch events Lisp sees collected in `nhwl-gui--touch-log'."
  (declare (indent 0))
  `(progn
     (setq nhwl-gui--touch-log nil)
     (advice-add 'touch-screen-handle-touch :before #'nhwl-gui--touch-note)
     (unwind-protect (progn ,@body)
       (advice-remove 'touch-screen-handle-touch #'nhwl-gui--touch-note))))

(defun nhwl-gui--touch (op id at &optional surface)
  "Send a touch OP for point ID at logical AT (X Y), ending its frame; SURFACE names the target of a down."
  (gui-runner-recorder `(:cmd "touch" :op ,op :id ,id :at ,(vector (car at) (cadr at))
                              ,@(and surface (list :surface surface))))
  (nhwl-gui--pump 0.2))

(defun nhwl-gui--touch-kinds ()
  "The kinds of the logged events, oldest first."
  (mapcar #'car (reverse nhwl-gui--touch-log)))

(ert-deftest nhwl-gui/touch-begin-update-end ()
  "A down, a motion and a lift reach Lisp as a begin with the point's id and position, an update listing the
point, and an end."
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--with-touch-log
    (nhwl-gui--touch "down" 0 '(30 40))
    (nhwl-gui--touch "motion" 0 '(50 60))
    (nhwl-gui--touch "up" 0 '(0 0))
    (should (equal (nhwl-gui--touch-kinds) '(touchscreen-begin touchscreen-update touchscreen-end)))
    (let ((begin (car (last nhwl-gui--touch-log))))
      (should (integerp (car (cadr begin)))))
    (let ((update (cadr (nth 1 nhwl-gui--touch-log))))
      (should (= (length update) 1)))))

(ert-deftest nhwl-gui/touch-cancel-ends-all-points ()
  "A compositor cancel ends every live point (two ends), and a later motion of a cancelled point reports
nothing."
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--with-touch-log
    (nhwl-gui--touch "down" 0 '(30 40))
    (nhwl-gui--touch "down" 1 '(60 70))
    (setq nhwl-gui--touch-log nil)
    ;; the recorder sends a cancel only while a point has an event its frame has not closed yet
    (gui-runner-recorder '(:cmd "touch" :op "motion" :id 0 :at [30 40] :frame :false))
    (gui-runner-recorder '(:cmd "touch" :op "cancel"))
    (nhwl-gui--pump 0.3)
    (should (= (seq-count (lambda (e) (eq (car e) 'touchscreen-end)) nhwl-gui--touch-log) 2))
    (setq nhwl-gui--touch-log nil)
    (nhwl-gui--touch "motion" 0 '(31 41))
    (should-not nhwl-gui--touch-log)))

(ert-deftest nhwl-gui/touch-on-a-child-frame ()
  "A touch that goes down on a child frame is that frame's: the begin's position is in the child's coordinates."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((before (nhwl-gui--sub-ids)) (c nil))
    (unwind-protect
        (progn
          (setq c (nhwl-gui--child))
          (let ((n (nhwl-gui--child-node before)))
            (should n)
            (nhwl-gui--with-touch-log
              (nhwl-gui--touch "down" 0 '(10 10) (plist-get n :id))
              (nhwl-gui--touch "up" 0 '(0 0))
              (should (equal (nhwl-gui--touch-kinds) '(touchscreen-begin touchscreen-end)))
              (let ((posn (cdr (cadr (car (last nhwl-gui--touch-log))))))
                (should (eq (posn-window posn) (frame-root-window c)))))))
      (when (frame-live-p c) (delete-frame c t)))))

(defvar nhwl-gui--pinches 0)

(defun nhwl-gui--pinch (&rest args)
  "Send a pinch command with ARGS (a plist) and let Lisp read its event."
  (gui-runner-recorder (cons :cmd (cons "pinch" args)))
  (nhwl-gui--pump 0.2))

(ert-deftest nhwl-gui/pinch-scales-text ()
  "A pinch begin and an update to scale 1.5 make `text-scale-pinch' scale the text up."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((buf (window-buffer (selected-window))))
    (nhwl-gui--point 100 100)
    (unwind-protect
        (progn
          (nhwl-gui--pinch :op "begin" :fingers 2)
          (nhwl-gui--pinch :op "update" :dx 0 :dy 0 :scale 1.5 :rotation 0)
          (with-current-buffer buf
            (should (> (or (bound-and-true-p text-scale-mode-amount) 0) 0))))
      (nhwl-gui--pinch :op "end")
      (with-current-buffer buf (text-scale-set 0)))))

(ert-deftest nhwl-gui/pinch-end-sends-nothing ()
  "Begin and update each make one `pinch' event; the end makes none."
  (skip-unless (nhwl-gui--recorder-p))
  (setq nhwl-gui--pinches 0)
  (let ((count (lambda (&rest _) (setq nhwl-gui--pinches (1+ nhwl-gui--pinches)))))
    (nhwl-gui--point 100 100)
    (advice-add 'text-scale-pinch :before count)
    (unwind-protect
        (progn
          (nhwl-gui--pinch :op "begin" :fingers 2)
          (nhwl-gui--pinch :op "update" :dx 0 :dy 0 :scale 1.2 :rotation 0)
          (should (= nhwl-gui--pinches 2))
          (nhwl-gui--pinch :op "end")
          (should (= nhwl-gui--pinches 2)))
      (advice-remove 'text-scale-pinch count)
      (with-current-buffer (window-buffer (selected-window)) (text-scale-set 0)))))

;; W2 Task 24: frames survive a compositor restart.  The recorder's `restart' drops every client and serves again on
;; the same socket (`keep-socket': the connection waits in the backlog for `down_ms'; `rebind': the name is bound
;; again after `down_ms'; `gone': until `restart_up'), and the shell connects again on its backoff timer.  Lisp hears
;; of it through `nhwl-display-connection-functions'.  The rows end the first connection, so they run last; the gate
;; runs them with `--daemon' and with the trace rule `unconfigured' on the second connection.

(defvar nhwl-gui--connection-log nil
  "The STATE of each `nhwl-display-connection-functions' call, oldest first.")

(defun nhwl-gui--connection-note (_display state)
  (setq nhwl-gui--connection-log (append nhwl-gui--connection-log (list state))))

(defmacro nhwl-gui--with-connection-log (&rest body)
  "Run BODY with the connection hook's states logged in `nhwl-gui--connection-log' (emptied first)."
  `(progn
     (setq nhwl-gui--connection-log nil)
     (add-hook 'nhwl-display-connection-functions #'nhwl-gui--connection-note)
     (unwind-protect (progn ,@body)
       (remove-hook 'nhwl-display-connection-functions #'nhwl-gui--connection-note))))

(defun nhwl-gui--restart (mode &rest args)
  "Restart the recorder in place in MODE (\"keep-socket\", \"rebind\" or \"gone\"), with ARGS (`:down_ms', `:profile')."
  (gui-runner-recorder `(:cmd "restart" :mode ,mode ,@args)))

(defun nhwl-gui--reconnected (&optional seconds)
  "Read events for up to SECONDS (default 10) until the connection hook has run with `reconnected'."
  (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (memq 'reconnected nhwl-gui--connection-log)) (or seconds 10)))

(defun nhwl-gui--restart-and-reconnect (&rest args)
  "A keep-socket restart with ARGS, then the wait for `reconnected'; non-nil when it came."
  (apply #'nhwl-gui--restart "keep-socket" args)
  (nhwl-gui--reconnected))

(defun nhwl-gui--accepted ()
  "How many connections the recorder has accepted since it started."
  (plist-get (gui-runner-recorder '(:cmd "connections")) :accepted))

(defun nhwl-gui--proc-counts ()
  "This process's (FDS TASKS RSS-KB)."
  (list (length (directory-files "/proc/self/fd" nil "\\`[0-9]"))
        (length (directory-files "/proc/self/task" nil "\\`[0-9]"))
        (with-temp-buffer
          (insert-file-contents "/proc/self/status")
          (and (re-search-forward "^VmRSS:[ \t]+\\([0-9]+\\)" nil t)
               (string-to-number (match-string 1))))))

(ert-deftest nhwl-gui/compositor-restart-keeps-frames ()
  "A keep-socket restart with the compositor down 1 s: the same frame stays live with its native size and title, and
the hook runs `reconnecting', then `reconnected'."
  :tags '(runs-last)
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((f (selected-frame)) (w (frame-native-width f)) (h (frame-native-height f))
         (title (frame-parameter f 'title)) (accepted (nhwl-gui--accepted)))
    (nhwl-gui--with-connection-log
     (should (nhwl-gui--restart-and-reconnect :down_ms 1000))
     (should (equal nhwl-gui--connection-log '(reconnecting reconnected))))
    (should (frame-live-p f))
    (should (eq (selected-frame) f))
    (should (= (frame-native-width f) w))
    (should (= (frame-native-height f) h))
    (should (equal (frame-parameter f 'title) title))
    (should (> (nhwl-gui--accepted) accepted))
    (should (nhwl-gui--connected-p))))

(ert-deftest nhwl-gui/compositor-restart-keeps-buffer-text-and-point ()
  "Buffer text, point and `window-start' are unchanged, and the picture after `reconnected' is the one before."
  :tags '(runs-last)
  (skip-unless (nhwl-gui--recorder-p))
  (let ((buf (generate-new-buffer " *nhwl-reconnect*")) (before nil))
    (unwind-protect
        (progn
          (switch-to-buffer buf)
          (dotimes (i 200) (insert (format "line %d of the text that stays\n" i)))
          (goto-char (point-min))
          (forward-line 50)
          (recenter 3)
          (redisplay t)
          (gui-runner-wait-settled)
          (setq before (gui-runner-screen-grab))
          (let ((text (buffer-string)) (pt (point)) (start (window-start)))
            (nhwl-gui--with-connection-log
             (should (nhwl-gui--restart-and-reconnect :down_ms 300)))
            (should (equal (buffer-string) text))
            (should (= (point) pt))
            (should (= (window-start) start)))
          (gui-runner-wait-settled)
          (should (<= (car (gui-runner-pixel-diff before (gui-runner-screen-grab))) 2)))
      (kill-buffer buf))))

(ert-deftest nhwl-gui/compositor-restart-falls-back-after-timeout ()
  "The compositor stays gone and `nhwl-reconnect-timeout' is 1.0: the display's frames are deleted within 2 s, the
daemon stays, and the hook runs `reconnecting', then `lost'."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--recorder-p) (daemonp)))
  (let ((nhwl-reconnect-timeout 1.0) (frames (nhwl-gui--nhwl-frames)) (t0 nil))
    (unwind-protect
        (nhwl-gui--with-connection-log
         (setq t0 (float-time))
         (nhwl-gui--restart "gone")
         (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (not (seq-some #'frame-live-p frames))) 4))
         (should (< (- (float-time) t0) 2.5))
         (should (daemonp))
         (should (equal nhwl-gui--connection-log '(reconnecting lost))))
      (ignore-errors (gui-runner-recorder '(:cmd "restart_up"))))))

(ert-deftest nhwl-gui/compositor-restart-socket-reappears ()
  "The socket name is bound again after 1.5 s: the shell finds it on its backoff and the frame stays."
  :tags '(runs-last)
  (skip-unless (nhwl-gui--recorder-p))
  (let ((f (selected-frame)))
    (nhwl-gui--with-connection-log
     (nhwl-gui--restart "rebind" :down_ms 1500)
     (should (nhwl-gui--reconnected))
     (should (equal nhwl-gui--connection-log '(reconnecting reconnected))))
    (should (frame-live-p f))
    (should (nhwl-gui--connected-p))))

(ert-deftest nhwl-gui/compositor-restart-closes-popups-and-menus ()
  "A menu open from a timer and a tooltip when the compositor restarts: `x-popup-menu' returns nil, the tooltip is
hidden, and a child frame stays, a subsurface of the new connection's tree."
  :tags '(runs-last)
  (skip-unless (nhwl-gui--recorder-p))
  (let ((c nil) (result 'unset) (timer nil))
    (unwind-protect
        (nhwl-gui--with-connection-log
         (setq c (nhwl-gui--child))
         (should (nhwl-gui--child-node nil))
         (x-show-tip "a tip" nil nil 60)
         (nhwl-gui--press-serial)
         (setq timer (run-at-time 0.5 nil (lambda () (nhwl-gui--restart "keep-socket" :down_ms 300))))
         (setq result (x-popup-menu (list '(40 40) (selected-frame))
                                    '("Menu" ("Pane" ("one" . 1) ("two" . 2)))))
         (should-not result)
         (should (nhwl-gui--reconnected))
         (should-not (seq-some (lambda (f) (and (frame-live-p f) (frame-visible-p f) (frame-parameter f 'tooltip)))
                               (frame-list)))
         (should (frame-live-p c))
         (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (nhwl-gui--subs)) 5)))
      (when timer (cancel-timer timer))
      (x-hide-tip)
      (when (and c (frame-live-p c)) (delete-frame c t)))))

(ert-deftest nhwl-gui/compositor-restart-stops-key-repeat ()
  "A key held across the restart does not repeat afterwards: no key event in the second after `reconnected'."
  :tags '(runs-last)
  (skip-unless (nhwl-gui--recorder-p))
  (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
  (gui-runner-recorder '(:cmd "key" :keycode 30 :state "press"))
  (nhwl-gui--with-connection-log
   (should (nhwl-gui--restart-and-reconnect)))
  (let ((deadline (+ (float-time) 1.0)) (keys 0) ev)
    (while (< (float-time) deadline)
      (setq ev (read-event nil nil 0.05))
      (when (eq ev ?a) (setq keys (1+ keys))))
    (should (= keys 0))))

(ert-deftest nhwl-gui/compositor-restart-reoffers-the-clipboard ()
  "Text copied before the restart is offered again on the new connection: after a focus, another client reads it."
  :tags '(runs-last)
  (skip-unless (nhwl-gui--recorder-p))
  (let ((text "reconnect clipboard text"))
    (nhwl-gui--focus-toplevel)
    (with-temp-buffer
      (insert text)
      (kill-ring-save (point-min) (point-max)))
    (should (equal (nhwl-gui--recorder-read "clipboard" "text/plain;charset=utf-8") text))
    (nhwl-gui--with-connection-log
     (should (nhwl-gui--restart-and-reconnect)))
    (nhwl-gui--focus-toplevel)
    (should (equal (nhwl-gui--recorder-read "clipboard" "text/plain;charset=utf-8") text))))

(ert-deftest nhwl-gui/compositor-restart-monitors-hook-only-on-change ()
  "With the same outputs after the restart `display-monitors-changed-functions' does not run; with an output added
while the compositor is down, it runs once."
  :tags '(runs-last)
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((runs 0) (count (lambda (_) (setq runs (1+ runs)))))
    (add-hook 'display-monitors-changed-functions count)
    (unwind-protect
        (progn
          (nhwl-gui--with-connection-log
           (should (nhwl-gui--restart-and-reconnect)))
          (nhwl-gui--pump 0.5)
          (should (= runs 0))
          (nhwl-gui--with-connection-log
           (nhwl-gui--restart "keep-socket" :down_ms 800)
           (gui-runner-recorder '(:cmd "output" :op "add" :name "nh-rc24" :size [800 600]))
           (should (nhwl-gui--reconnected)))
          (nhwl-gui--pump 0.5)
          (should (= runs 1)))
      (remove-hook 'display-monitors-changed-functions count)
      (ignore-errors (gui-runner-recorder '(:cmd "output" :op "remove" :name "nh-rc24"))))))

(ert-deftest nhwl-gui/compositor-restart-during-drag-and-preedit ()
  "A preedit shown and a drag out in flight when the compositor restarts: `x-begin-drag' returns nil and the preedit
overlay is gone."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--recorder-p) (fboundp 'x-begin-drag)))
  (let ((buf (generate-new-buffer " *nhwl-reconnect-ime*")) (timer nil) (result 'unset))
    (unwind-protect
        (progn
          (nhwl-gui--ime-ready buf)
          (gui-runner-recorder '(:cmd "text_input" :preedit "ni" :cursor [2 2]))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--ime-pump) (equal (nhwl-gui--preedit-string) "ni"))))
          (nhwl-gui--point 100 100)
          (gui-runner-recorder '(:cmd "button" :button 272 :state "press"))
          (nhwl-gui--with-connection-log
           (setq timer (run-at-time 0.3 nil (lambda () (nhwl-gui--restart "keep-socket" :down_ms 300))))
           (setq result (x-begin-drag '("text/plain;charset=utf-8") 'XdndActionCopy nil nil t))
           (should-not result)
           (should (nhwl-gui--reconnected)))
          (should (equal (nhwl-gui--preedit-string) "")))
      (when timer (cancel-timer timer))
      (kill-buffer buf))))

(ert-deftest nhwl-gui/make-frame-while-reconnecting-signals ()
  "`make-frame' on a display that is reconnecting signals \"Display is reconnecting\"; once it is back, it works."
  :tags '(runs-last)
  (skip-unless (nhwl-gui--recorder-p))
  (let ((err nil) (f nil))
    (unwind-protect
        (nhwl-gui--with-connection-log
         (nhwl-gui--restart "gone")
         (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (memq 'reconnecting nhwl-gui--connection-log)) 5))
         (setq err (condition-case e (progn (setq f (make-frame)) nil) (error (cadr e))))
         (should (equal err "Display is reconnecting"))
         (gui-runner-recorder '(:cmd "restart_up"))
         (should (nhwl-gui--reconnected))
         (setq f (make-frame '((minibuffer . nil))))
         (should (frame-live-p f)))
      (ignore-errors (gui-runner-recorder '(:cmd "restart_up")))
      (when (and f (frame-live-p f)) (delete-frame f)))))

(ert-deftest nhwl-gui/compositor-restart-twenty-cycles-keeps-frames ()
  "Twenty keep-socket restarts with a frame, a child frame and a tooltip: after the last the same frames live, the
descriptor and thread counts equal cycle 5's, VmRSS grew less than 8 MiB since cycle 5, and the recorder accepted one
connection per cycle."
  :tags '(runs-last)
  (skip-unless (nhwl-gui--recorder-p))
  (let ((f (selected-frame)) (c nil) (at5 nil) (accepted nil))
    (unwind-protect
        (progn
          (setq c (nhwl-gui--child))
          (should (nhwl-gui--child-node nil))
          (setq accepted (nhwl-gui--accepted))
          (dotimes (i 20)
            (x-show-tip (format "cycle %d" i) nil nil 60)
            (nhwl-gui--pump 0.2)
            (nhwl-gui--with-connection-log
             (should (nhwl-gui--restart-and-reconnect)))
            (should (= (nhwl-gui--accepted) (+ accepted i 1)))
            (when (= i 4)
              (garbage-collect)
              (setq at5 (nhwl-gui--proc-counts))))
          (garbage-collect)
          (let ((at20 (nhwl-gui--proc-counts)))
            (should (frame-live-p f))
            (should (frame-live-p c))
            (should (= (nth 0 at20) (nth 0 at5)))
            (should (= (nth 1 at20) (nth 1 at5)))
            (should (< (- (nth 2 at20) (nth 2 at5)) (* 8 1024)))))
      (x-hide-tip)
      (when (and c (frame-live-p c)) (delete-frame c t)))))

(ert-deftest nhwl-gui/live-kwin-restart-keeps-frames ()
  "KWin through its wrapper: its child is killed and started again (`NHWL_GUI_RESTART'), and the frames stay.  The
trace rules `errors,acks,unconfigured,subsurface' judge the second connection."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--live-p) (gui-runner-restart-available-p)
                    (equal (getenv "GLITNIR_GUI_COMPOSITOR") "kwin")))
  (let ((frames (nhwl-gui--nhwl-frames)) (c nil))
    (unwind-protect
        (nhwl-gui--with-connection-log
         (setq c (nhwl-gui--child))
         (nhwl-gui--live-settle 0.5)
         (gui-runner-restart-compositor)
         (should (nhwl-gui--reconnected))
         (nhwl-gui--live-settle 0.5)
         (should (seq-every-p #'frame-live-p frames))
         (should (frame-live-p c)))
      (when (and c (frame-live-p c)) (delete-frame c t)))))

(ert-deftest nhwl-gui/live-sway-restart-reconnects ()
  "sway restarted on the same socket name within the timeout: the frames stay.  The trace rules `errors,unconfigured'
judge the second connection."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--live-p) (gui-runner-restart-available-p)
                    (equal (getenv "GLITNIR_GUI_COMPOSITOR") "sway")))
  (let ((frames (nhwl-gui--nhwl-frames)))
    (nhwl-gui--with-connection-log
     (nhwl-gui--live-settle 0.5)
     (gui-runner-restart-compositor)
     (should (nhwl-gui--reconnected))
     (nhwl-gui--live-settle 0.5)
     (should (seq-every-p #'frame-live-p frames)))))

(provide 'nhwl-gui-w2-tests)

;;; nhwl-gui-w2-tests.el ends here
