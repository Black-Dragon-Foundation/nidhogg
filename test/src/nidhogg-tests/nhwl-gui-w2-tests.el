;;; nhwl-gui-w2-tests.el --- GUI rows for the native shell's parity features  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh with the base rows (nhwl-gui-tests.el, loaded first: its helpers are used here).  Rows
;; that drive the compositor need `--compositor recorder'; rows named `nhwl-gui/live-*' run on real compositors and
;; are judged by the trace checker.

(require 'ert)
(require 'nhwl-gui-tests)

;; Popup frames.  A frame made with `nhwl-popup' (and `parent-frame') is an xdg popup of its parent's
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
          ;; a frame is made on the selected frame's display, which must be its parent's
          (setq p (with-selected-frame f (nhwl-gui--popup-frame f '(40 30 1 1 :grab t))))
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
  (let* ((parent (selected-frame)) (before (nhwl-gui--popup-ids)) (frames nil)
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
  ;; no window layer (an earlier scroll's): the flash's rows would be the layer's damage, not the toplevel's
  (let ((nhwl-scroll-layers nil) (visible-bell t) (ring-bell-function nil) (before nil) (mark nil) (flash nil)
        (restore nil))
    (redisplay t)
    (gui-runner-wait-settled)
    (setq before (gui-runner-screen-grab))
    (setq mark (nhwl-gui--mark))
    (ding)
    ;; damage is in the toplevel's buffer rows: the native area's bottom is the flash's bottom, not the screen's
    (let* ((h (+ (nhwl-gui--native-top) (frame-pixel-height)))
           (line (frame-char-height))
           ;; the flash's top rows start below the menu and tool bars (the frame's top margin), as pgtk's do
           (y0 (+ (nhwl-gui--native-top) (nth 1 (window-pixel-edges (frame-root-window)))))
           (top (lambda (c) (seq-some (lambda (r) (< (nth 1 r) (+ y0 (* 3 line)))) (nhwl-gui--damage c))))
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

;; The drop target.  The recorder is a foreign source (`dnd_offer', `dnd_motion', `dnd_drop', read back
;; with `dnd_log'); `pgtk-dnd.el' decides what a drop does.  Positions are logical surface pixels of the toplevel.

(defun nhwl-gui--dnd-offer (types &rest args)
  "A foreign drag enters the toplevel at (100, 100) offering TYPES, a plist of MIME keyword and text; ARGS override."
  (let ((req (list :cmd "dnd_offer" :types types :actions ["copy" "move"] :preferred "copy"
                   :surface "toplevel" :at [100 100])))
    (while args (setq req (plist-put req (pop args) (pop args))))
    (gui-runner-recorder req)
    (nhwl-gui--pump 0.3)))

(defun nhwl-gui--dnd-end ()
  "End a foreign drag that was not dropped: it leaves the surface and the button comes up over nothing.  A drag left
going holds the recorder's pointer for the next rows."
  (gui-runner-recorder '(:cmd "dnd_leave"))
  (gui-runner-recorder '(:cmd "dnd_drop"))
  (nhwl-gui--pump 0.3))

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
          ;; the surface position: past the client-side title bar and margins where there are some
          (let ((at (apply #'vector (nhwl-gui--native-to-logical 60 (+ (nth 3 (window-inside-pixel-edges)) 3)))))
            (nhwl-gui--dnd-offer '(:text/plain "X") :at at)
            (gui-runner-recorder `(:cmd "dnd_motion" :at ,at))
            (nhwl-gui--pump 0.3)
            (gui-runner-recorder '(:cmd "dnd_drop")))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (> (buffer-size) 3)) 3))
          (should (equal (buffer-string) "aXbc")))
      (kill-buffer b))))

(ert-deftest nhwl-gui/drop-on-a-child-frame-goes-to-the-child ()
  "A drop over a child frame is the child's: its window gets the text and the toplevel's does not."
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((parent (selected-frame)) (b (generate-new-buffer " *dnd-child*")) (child nil)
         (host (get-buffer-create " *dnd-host*")) (before (nhwl-gui--sub-ids)) (id nil))
    (unwind-protect
        (progn
          (switch-to-buffer host)
          ;; no menu or tool bar (a child frame has them by default): the drop is on the window
          (setq child (make-frame `((parent-frame . ,parent) (left . 20) (top . 20) (width . 30) (height . 8)
                                    (minibuffer . nil) (no-other-frame . t) (menu-bar-lines . 0)
                                    (tool-bar-lines . 0))))
          (with-selected-frame child (switch-to-buffer b))
          (nhwl-gui--pump 0.5)
          ;; the child's own surface (the newest surface can be another one, such as the pointer's)
          (setq id (plist-get (nhwl-gui--child-node before) :id))
          (should id)
          (nhwl-gui--dnd-offer '(:text/plain "to the child") :surface id :at [40 40])
          (gui-runner-recorder `(:cmd "dnd_motion" :at [40 40] :surface ,id))
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
  (unwind-protect
      (progn
        (gui-runner-recorder '(:cmd "dnd_motion" :at [101 101]))
        (nhwl-gui--pump 0.5)
        (let ((log (nhwl-gui--dnd-log)))
          (should-not (stringp (nhwl-gui--get log :accepted)))
          (should-not (nhwl-gui--get log :finished))))
    (nhwl-gui--dnd-end)))

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
          ;; the surface position: past the client-side title bar and margins where there are some
          (let ((y (+ (nth 1 (window-inside-pixel-edges)) (* 4 (default-line-height)) 2)))
            (nhwl-gui--dnd-offer '(:text/plain "x") :at (apply #'vector (nhwl-gui--native-to-logical 30 y)))
            (gui-runner-recorder `(:cmd "dnd_motion" :at ,(apply #'vector (nhwl-gui--native-to-logical 32 y)))))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (= (line-number-at-pos) 5)) 3)))
      (nhwl-gui--dnd-end)
      (kill-buffer b))))

;; Touch screens and touchpad pinch.  The recorder's `touch' and `pinch' commands stand in for the
;; compositor; the events Lisp sees are pgtk's: `touchscreen-begin', `touchscreen-update', `touchscreen-end', `pinch'.
;; The rows read them here (`nhwl-gui--pump' would read and drop them): touch events are logged, a `pinch' runs its
;; binding as the command loop would.

(defvar nhwl-gui--touch-log nil "The touch events Lisp read, newest first.")

(defmacro nhwl-gui--with-touch-log (&rest body)
  "Run BODY with the touch events Lisp reads collected in `nhwl-gui--touch-log'."
  (declare (indent 0))
  `(progn
     (setq nhwl-gui--touch-log nil)
     ,@body))

(defun nhwl-gui--pump-input (seconds)
  "Read events for SECONDS: log the touch events, run each `pinch' event's binding, drop the rest."
  (let ((deadline (+ (float-time) seconds)))
    (while (< (float-time) deadline)
      (let ((e (read-event nil nil 0.05)))
        (pcase (car-safe e)
          ((or 'touchscreen-begin 'touchscreen-update 'touchscreen-end) (push e nhwl-gui--touch-log))
          ('pinch (let ((last-input-event e))
                    (call-interactively (key-binding (vector e)) nil (vector e)))))))))

(defun nhwl-gui--touch (op id at &optional surface)
  "Send a touch OP for point ID at logical AT (X Y), ending its frame; SURFACE names the target of a down."
  (gui-runner-recorder `(:cmd "touch" :op ,op :id ,id :at ,(vector (car at) (cadr at))
                              ,@(and surface (list :surface surface))))
  (nhwl-gui--pump-input 0.2))

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
    (nhwl-gui--pump-input 0.3)
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
          ;; no menu or tool bar (a child frame has them by default): the touch is on the window
          (setq c (nhwl-gui--child '(menu-bar-lines . 0) '(tool-bar-lines . 0)))
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
  (nhwl-gui--pump-input 0.2))

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

;; Frames survive a compositor restart.  The recorder's `restart' drops every client and serves again on
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

;; The Lisp menu (lisp/term/nhwl-menu.el).  `x-popup-menu' runs its own loop, so the keys are typed by
;; a timer once the menu's popup shows.  The recorder gives a grabbing popup the keyboard when told (`focus'), as a
;; compositor does; special keys go by evdev code (Down 108, Up 103, Enter 28, Esc 1, Space 57), characters by text.

(defun nhwl-gui--menu-keymap ()
  "A menu of Alpha, Beta (disabled) and Gamma."
  (let ((map (make-sparse-keymap "Menu")))
    (define-key map [alpha] '(menu-item "Alpha" ignore))
    (define-key map [beta] '(menu-item "Beta" ignore :enable nil))
    (define-key map [gamma] '(menu-item "Gamma" ignore))
    map))

(defun nhwl-gui--menu-send (key)
  "Type KEY on the recorder's keyboard: a string is text, a number an evdev code."
  (if (stringp key)
      (gui-runner-recorder (list :cmd "key" :text key))
    (gui-runner-recorder (list :cmd "key" :keycode key :state "press"))
    (gui-runner-recorder (list :cmd "key" :keycode key :state "release"))))

(defun nhwl-gui--menu-run (menu on-open &optional position hold)
  "Open MENU at POSITION (default the pointer) after a click; return what `x-popup-menu' returns, or `timeout' after 10 s.
MENU can also be a function, called instead of `x-popup-menu', whose value is returned.
Once its popup shows, the keyboard goes to it and ON-OPEN is called with the popup's id (from a timer: it must not
read events).  The popup is gone afterwards.  With HOLD the right button is pressed over the toplevel and held
while the menu opens, as for a menu opened by a `down-mouse-3' event."
  (let ((before (nhwl-gui--popup-ids)) (timer nil) (result nil))
    (nhwl-gui--press-serial)
    (when hold
      (nhwl-gui--point 100 100)
      (gui-runner-recorder '(:cmd "button" :button 273 :state "press"))
      (nhwl-gui--pump 0.1))
    (setq timer (run-at-time 0.1 0.1
                             (lambda ()
                               (let ((new (seq-difference (nhwl-gui--popup-ids) before)))
                                 (when new
                                   (cancel-timer timer)
                                   (gui-runner-recorder (list :cmd "focus" :surface (car new)))
                                   (funcall on-open (car new)))))))
    (unwind-protect
        (setq result (with-timeout (10 'timeout)
                       (if (functionp menu) (funcall menu) (x-popup-menu (or position t) menu))))
      (cancel-timer timer))
    (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (equal (nhwl-gui--popup-ids) before)) 5))
    result))

(defun nhwl-gui--menu-typing (menu keys)
  "Open MENU and type KEYS (see `nhwl-gui--menu-send') once it shows; the result."
  (nhwl-gui--menu-run menu (lambda (_id) (dolist (k keys) (nhwl-gui--menu-send k)))))

(ert-deftest nhwl-gui/menu-keyboard-select ()
  "Down then Return chooses the first item: the result is the list of keys, one here."
  :tags '(gnome-smoke)
  (skip-unless (nhwl-gui--recorder-p))
  (should (equal (nhwl-gui--menu-typing (nhwl-gui--menu-keymap) '(108 28)) '(alpha)))
  ;; the same menu opens again after it closed
  (should (equal (nhwl-gui--menu-typing (nhwl-gui--menu-keymap) '(103 28)) '(gamma))))

(ert-deftest nhwl-gui/menu-escape-returns-nil ()
  (skip-unless (nhwl-gui--recorder-p))
  (should-not (nhwl-gui--menu-typing (nhwl-gui--menu-keymap) '(108 1)))
  (should (nhwl-gui--connected-p)))

(ert-deftest nhwl-gui/menu-type-ahead-jumps ()
  "A letter selects the next item that starts with it (either case); Return chooses it."
  (skip-unless (nhwl-gui--recorder-p))
  (should (equal (nhwl-gui--menu-typing (nhwl-gui--menu-keymap) '("g" 28)) '(gamma)))
  (should (equal (nhwl-gui--menu-typing (nhwl-gui--menu-keymap) '("G" 28)) '(gamma))))

(ert-deftest nhwl-gui/menu-disabled-item-skipped ()
  "Down from Alpha lands on Gamma, not the disabled Beta; typing b selects nothing, so Return chooses nothing."
  (skip-unless (nhwl-gui--recorder-p))
  (should (equal (nhwl-gui--menu-typing (nhwl-gui--menu-keymap) '(108 108 28)) '(gamma)))
  (should-not (nhwl-gui--menu-typing (nhwl-gui--menu-keymap) '("b" 28 1))))

(ert-deftest nhwl-gui/menu-keys-never-reach-the-buffer ()
  "Keys typed while the menu is open leave the buffer unchanged and cause no focus-out of the frame under it; once
the menu has closed, a key arrives in the frame again."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((buf (generate-new-buffer " *nhwl-menu-keys*")) (outs 0) (parent (selected-frame))
        (count (lambda () (setq outs (1+ outs)))))
    (unwind-protect
        (progn
          (set-window-buffer (selected-window) buf)
          (nhwl-gui--focus-toplevel)
          (nhwl-gui--pump)
          (add-hook 'focus-out-hook count)
          (should-not (nhwl-gui--menu-typing (nhwl-gui--menu-keymap) '("x" "y" 108 "z" 1)))
          (should (equal (with-current-buffer buf (buffer-string)) ""))
          (should (= outs 0))
          (should (eq (frame-focus-state parent) t))
          (nhwl-gui--focus-toplevel)
          (should (eq (nhwl-gui--key-frame ?x) parent)))
      (remove-hook 'focus-out-hook count)
      (kill-buffer buf))))

(ert-deftest nhwl-gui/menu-open-commits-nothing-while-idle ()
  "An open menu that nobody touches commits nothing on its popup (strict): 0.5 s after it shows to 2.5 s."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((mark nil) (commits :unset))
    (should-not
     (nhwl-gui--menu-run
      (nhwl-gui--menu-keymap)
      (lambda (_id)
        (run-at-time 0.5 nil (lambda () (setq mark (nhwl-gui--mark))))
        (run-at-time 2.5 nil (lambda ()
                               (setq commits (and mark (length (nhwl-gui--popup-commits mark))))
                               (nhwl-gui--menu-send 1))))))
    (should (eql commits 0))))

;; Dragging out of Emacs (`x-begin-drag').  The recorder is the foreign target (`drag_target', `drag_drop',
;; `drag_cancel', read back with `drag_log').  `x-begin-drag' blocks until the drag ends, so the target's part is sent
;; by a batch Nidhogg started just before the call (`nhwl-gui--recorder-later'), whose output holds the replies.
;; Positions are logical surface pixels of the toplevel.

(defun nhwl-gui--recorder-later (steps)
  "Send STEPS to the recorder from a batch Nidhogg, one after another; return its process.
Each step is (SECONDS . PLIST): wait SECONDS, send PLIST and wait for the reply, which goes to the process's buffer
as one line."
  (let ((form `(let ((path ,(getenv "GLITNIR_GUI_RECORDER")))
                 (dolist (step ',steps)
                   (sleep-for (car step))
                   (let* ((reply "")
                          (p (make-network-process
                              :name "later" :family 'local :service path :coding 'utf-8
                              :filter (lambda (_p out) (setq reply (concat reply out))))))
                     (process-send-string p (concat (json-serialize (cdr step)) "\n"))
                     (let ((deadline (+ (float-time) 10)))
                       (while (and (not (string-suffix-p "\n" reply)) (< (float-time) deadline))
                         (accept-process-output p 0.1)))
                     (delete-process p)
                     (princ reply))))))
    (make-process :name "nhwl-gui-later" :noquery t :connection-type 'pipe
                  :buffer (generate-new-buffer " *nhwl-gui-later*")
                  :command (list (expand-file-name invocation-name invocation-directory)
                                 "-Q" "--batch" "--eval" (prin1-to-string form)))))

(defun nhwl-gui--later-output (later)
  "The replies LATER (from `nhwl-gui--recorder-later') printed, once it has exited (10 s at most)."
  (nhwl-gui--wait (lambda () (not (process-live-p later))) 10)
  (with-current-buffer (process-buffer later) (buffer-string)))

(defun nhwl-gui--drag-cleanup (later)
  "Stop LATER, release the button and read what is left, so the next row starts clean."
  (when later
    (when (process-live-p later) (delete-process later))
    (kill-buffer (process-buffer later)))
  (ignore-errors (gui-runner-recorder '(:cmd "button" :button 272 :state "release")))
  (nhwl-gui--pump))

(defun nhwl-gui--drag-press (&optional x y)
  "Press the left button over the toplevel at X, Y (default 100, 100) and keep it held; the press is read and dropped."
  (nhwl-gui--point (or x 100) (or y 100))
  (gui-runner-recorder '(:cmd "button" :button 272 :state "press"))
  (nhwl-gui--pump))

(defun nhwl-gui--drags ()
  "The drags the recorder has logged, oldest first."
  (plist-get (gui-runner-recorder '(:cmd "drag_log")) :drags))

(defun nhwl-gui--drag-log ()
  "The newest drag the recorder has logged."
  (car (last (nhwl-gui--drags))))

(defun nhwl-gui--mouse-event-p (e)
  "Non-nil when E is a mouse button event."
  (and (consp e) (memq (event-basic-type e) '(mouse-1 down-mouse-1))))

(ert-deftest nhwl-gui/drag-text-out-copy ()
  "Text dragged out starts with the held button's serial and offers text/plain;charset=utf-8; the target reads
\"hello\", and the call returns `copy'."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((later nil))
    (unwind-protect
        (progn
          (nhwl-gui--drag-press)
          (setq later (nhwl-gui--recorder-later
                       '((0.8 :cmd "drag_target" :accept "text/plain;charset=utf-8" :action "copy")
                         (0.2 :cmd "drag_drop" :finish t))))
          (should (eq (dnd-begin-text-drag "hello") 'copy))
          (should (string-search "\"data\":\"hello\"" (nhwl-gui--later-output later)))
          (let ((log (nhwl-gui--drag-log)))
            (should (equal (plist-get log :serial_kind) "button"))
            (should (member "text/plain;charset=utf-8" (plist-get log :mimes)))
            (should (equal (plist-get log :result) "finished"))))
      (nhwl-gui--drag-cleanup later))))

(ert-deftest nhwl-gui/drag-file-out-move ()
  "A file dragged out with `move', which the target picks: the call returns `move', and the bytes are its
file:// URI."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((file (make-temp-file "nhwl-drag-" nil ".txt" "dragged file\n")) (later nil))
    (unwind-protect
        (progn
          (nhwl-gui--drag-press)
          (setq later (nhwl-gui--recorder-later
                       '((0.8 :cmd "drag_target" :accept "text/uri-list" :action "move")
                         (0.2 :cmd "drag_drop" :finish t))))
          (should (eq (dnd-begin-file-drag file nil 'move) 'move))
          (let ((out (nhwl-gui--later-output later)))
            (should (string-search "\"data\":\"file:///" out))
            (should (string-search (file-name-nondirectory file) out)))
          (should (member "text/uri-list" (plist-get (nhwl-gui--drag-log) :mimes))))
      (nhwl-gui--drag-cleanup later)
      (delete-file file))))

(ert-deftest nhwl-gui/drag-out-cancelled-returns-nil ()
  "A drag cancelled over nothing returns nil within 1 s of the cancel, the buffer is unchanged, and the next click
arrives."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((b (generate-new-buffer " *drag*")) (later nil) (r 'unset) (t0 nil))
    (unwind-protect
        (progn
          (switch-to-buffer b)
          (insert "keep this text")
          (nhwl-gui--drag-press)
          (setq later (nhwl-gui--recorder-later '((0.8 :cmd "drag_cancel"))))
          (setq t0 (float-time))
          (setq r (dnd-begin-text-drag "keep this text" nil 'move))
          (should-not r)
          ;; the batch Nidhogg's start and its 0.8 s wait, then at most 1 s
          (should (< (- (float-time) t0) 2.5))
          (should (equal (buffer-string) "keep this text"))
          (nhwl-gui--point 120 100)
          (gui-runner-recorder '(:cmd "button" :button 272 :state "press"))
          (gui-runner-recorder '(:cmd "button" :button 272 :state "release"))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--mouse-event-p (read-event nil nil 0.1))) 3)))
      (nhwl-gui--drag-cleanup later)
      (kill-buffer b))))

(ert-deftest nhwl-gui/drag-out-target-never-finishes ()
  "A target that takes the drop as a move but never finishes: the call returns nil within
`nhwl-dnd-finish-timeout' + 1 s of the drop, and the region a move would delete stays."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((b (generate-new-buffer " *drag*")) (later nil) (t0 nil) (nhwl-dnd-finish-timeout 1.0))
    (unwind-protect
        (progn
          (switch-to-buffer b)
          (insert "moved text")
          (nhwl-gui--drag-press)
          (setq later (nhwl-gui--recorder-later
                       '((0.8 :cmd "drag_target" :accept "text/plain;charset=utf-8" :action "move")
                         (0.2 :cmd "drag_drop" :finish :false))))
          (setq t0 (float-time))
          (let ((r (dnd-begin-text-drag "moved text" nil 'move)))
            (should-not r)
            ;; the batch Nidhogg's start, its 1 s of waits, the timeout and 1 s
            (should (< (- (float-time) t0) 4.0))
            ;; what `mouse-drag-and-drop-region' does with the result
            (when (eq r 'move) (delete-region (point-min) (point-max))))
          (should (equal (buffer-string) "moved text")))
      (nhwl-gui--drag-cleanup later)
      (kill-buffer b))))

(ert-deftest nhwl-gui/drag-out-without-a-button-returns-nil ()
  "Without a held button the call returns nil at once, and no drag starts."
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--click 100 100)
  (let ((before (length (nhwl-gui--drags))) (t0 (float-time)))
    (should-not (dnd-begin-text-drag "x"))
    (should (< (- (float-time) t0) 0.5))
    (should (= (length (nhwl-gui--drags)) before))))

(ert-deftest nhwl-gui/quit-during-drag-cancels ()
  "C-g during a drag gives it up and quits, at once; the display stays, and the next click arrives."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((later nil) (t0 nil))
    (unwind-protect
        (progn
          (nhwl-gui--drag-press)
          (setq later (nhwl-gui--recorder-later
                       '((0.8 :cmd "key" :keycode 29 :state "press")
                         (0 :cmd "key" :text "g")
                         (0 :cmd "key" :keycode 29 :state "release"))))
          (setq t0 (float-time))
          (should (eq (condition-case nil
                          (progn (dnd-begin-text-drag "x") 'returned)
                        (quit 'quit))
                      'quit))
          (should (< (- (float-time) t0) 3.0))
          (should (nhwl-gui--connected-p))
          (nhwl-gui--drag-cleanup later)
          (setq later nil)
          (nhwl-gui--point 120 100)
          (gui-runner-recorder '(:cmd "button" :button 272 :state "press"))
          (gui-runner-recorder '(:cmd "button" :button 272 :state "release"))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--mouse-event-p (read-event nil nil 0.1))) 3)))
      (nhwl-gui--drag-cleanup later))))

(ert-deftest nhwl-gui/drag-region-cross-program ()
  "With `mouse-drag-and-drop-region-cross-program', the region dragged out of the frame reaches the other client."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((b (generate-new-buffer " *drag-region*")) (later nil)
        (mouse-drag-and-drop-region t) (mouse-drag-and-drop-region-cross-program t))
    (unwind-protect
        (progn
          (switch-to-buffer b)
          (delete-other-windows)
          (insert "the dragged region")
          (set-mark (point-min))
          (goto-char (point-max))
          (activate-mark)
          (redisplay t)
          (let* ((e (window-inside-pixel-edges))
                 (x (+ (nth 0 e) 10))
                 (y (+ (nth 1 e) 3))
                 (out (+ (frame-pixel-width) 40)))
            (nhwl-gui--point x y)
            (gui-runner-recorder '(:cmd "button" :button 272 :state "press"))
            (setq later (nhwl-gui--recorder-later
                         `((0.6 :cmd "pointer" :pos [,(+ x 20) ,y])
                           (0.3 :cmd "pointer" :pos [,out ,y])
                           (1.0 :cmd "drag_target" :accept "text/plain;charset=utf-8" :action "copy")
                           (0.2 :cmd "drag_drop" :finish t))))
            (let ((ev (nhwl-gui--press-event nil)))
              (should ev)
              (funcall (key-binding (vector 'down-mouse-1)) ev)))
          (should (string-search "\"data\":\"the dragged region\"" (nhwl-gui--later-output later)))
          (should (equal (buffer-string) "the dragged region")))
      (nhwl-gui--drag-cleanup later)
      (kill-buffer b))))

(ert-deftest nhwl-gui/drag-back-over-emacs-returns-the-frame ()
  "With RETURN-FRAME, a drag that leaves the frame and comes back over it is given up, and the call returns the
frame."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((later nil) (out (+ (frame-pixel-width) 40)))
    (unwind-protect
        (progn
          (gui-set-selection 'XdndSelection "x")
          (nhwl-gui--drag-press)
          (setq later (nhwl-gui--recorder-later
                       `((0.8 :cmd "pointer" :pos [,out 100])
                         (0.3 :cmd "pointer" :pos [120 100]))))
          (should (eq (x-begin-drag '("text/plain;charset=utf-8") 'XdndActionCopy nil t) (selected-frame)))
          (should (nhwl-gui--connected-p)))
      (nhwl-gui--drag-cleanup later))))

(ert-deftest nhwl-gui/drag-onto-own-frame-drops ()
  "With ALLOW-CURRENT-FRAME, text dragged onto another window of the same frame is inserted there, and the call
returns `copy'."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((src (generate-new-buffer " *drag-src*")) (dst (generate-new-buffer " *drag-dst*")) (later nil))
    (unwind-protect
        (progn
          (delete-other-windows)
          (switch-to-buffer src)
          (insert "source")
          (let ((w (split-window-right)))
            (set-window-buffer w dst)
            (redisplay t)
            (let* ((se (window-inside-pixel-edges))
                   (de (window-inside-pixel-edges w)))
              (nhwl-gui--drag-press (+ (nth 0 se) 10) (+ (nth 1 se) 3))
              (setq later (nhwl-gui--recorder-later
                           `((0.8 :cmd "pointer" :pos [,(+ (nth 0 de) 10) ,(+ (nth 1 de) 3)])
                             (0.4 :cmd "button" :button 272 :state "release"))))))
          (should (eq (dnd-begin-text-drag "dragged in" nil 'copy t) 'copy))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (> (buffer-size dst) 0)) 3))
          (should (equal (with-current-buffer dst (buffer-string)) "dragged in"))
          (should (equal (with-current-buffer src (buffer-string)) "source")))
      (nhwl-gui--drag-cleanup later)
      (delete-other-windows)
      (kill-buffer src)
      (kill-buffer dst))))

(defun nhwl-gui--sway-node (node pid)
  "The node of sway's tree NODE (an alist), or one below it, whose process is PID."
  (if (eql (alist-get 'pid node) pid)
      node
    (seq-some (lambda (n) (nhwl-gui--sway-node n pid))
              (append (alist-get 'nodes node) (alist-get 'floating_nodes node)))))

(defun nhwl-gui--sway-centre (pid)
  "The centre (X . Y), in sway's layout pixels, of the window of process PID, or nil."
  (with-temp-buffer
    (when (eq 0 (call-process "swaymsg" nil t nil "-t" "get_tree"))
      (let* ((tree (json-parse-string (buffer-string) :object-type 'alist :array-type 'list
                                      :false-object nil :null-object nil))
             (r (alist-get 'rect (nhwl-gui--sway-node tree pid))))
        (when r
          (cons (+ (alist-get 'x r) (/ (alist-get 'width r) 2))
                (+ (alist-get 'y r) (/ (alist-get 'height r) 2))))))))

(ert-deftest nhwl-gui/live-drag-between-two-nidhoggs ()
  "On sway, text dragged out of this Nidhogg onto a second `nidhogg -Q' is inserted there, and the call returns
`copy'.  The pointer is driven by `swaymsg seat - cursor'; the trace rules (errors, dnd) judge what was sent."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--live-p) (equal (glitnir-gui--compositor) "sway") (executable-find "swaymsg")))
  (let* ((file (make-temp-file "nhwl-drag-target-"))
         (second (make-process
                  :name "nhwl-drag-target" :noquery t :buffer nil
                  :command (list (expand-file-name invocation-name invocation-directory) "-Q"
                                 "--eval" (prin1-to-string
                                           `(run-at-time 10 nil
                                                         (lambda ()
                                                           (write-region (point-min) (point-max) ,file)
                                                           (kill-emacs)))))))
         (driver nil))
    (unwind-protect
        (let ((from nil) (to nil))
          (should (nhwl-gui--wait (lambda ()
                                    (nhwl-gui--live-settle 0.2)
                                    (setq from (nhwl-gui--sway-centre (emacs-pid))
                                          to (nhwl-gui--sway-centre (process-id second)))
                                    (and from to))
                                  8))
          (call-process "swaymsg" nil nil nil "seat" "-" "cursor" "set"
                        (number-to-string (car from)) (number-to-string (cdr from)))
          (call-process "swaymsg" nil nil nil "seat" "-" "cursor" "press" "button1")
          (nhwl-gui--pump 0.3)
          (setq driver (make-process
                        :name "nhwl-drag-driver" :noquery t :buffer nil
                        :command (list "sh" "-c"
                                       (format "sleep 1; swaymsg seat - cursor set %d %d; sleep 0.5; \
swaymsg seat - cursor set %d %d; sleep 0.5; swaymsg seat - cursor release button1"
                                               (car to) (cdr to) (+ 5 (car to)) (cdr to)))))
          (should (eq (dnd-begin-text-drag "from the first Nidhogg") 'copy))
          (should (nhwl-gui--wait (lambda () (not (process-live-p second))) 15))
          (should (string-search "from the first Nidhogg"
                                 (with-temp-buffer (insert-file-contents file) (buffer-string)))))
      (when (and driver (process-live-p driver)) (delete-process driver))
      (when (process-live-p second) (delete-process second))
      (call-process "swaymsg" nil nil nil "seat" "-" "cursor" "release" "button1")
      (delete-file file))))

;; Menus by pointer and touch, submenus, dialogs and the menu bar.  The pointer goes to the surface that has the
;; keyboard (`focus'), in its own logical pixels, so a row is at x 10 and y (row + 1/2) times the character height.
;; The steps after a menu opened run from timers (the menu's loop is reading): they send recorder commands only.

(defun nhwl-gui--menu-over (id depth row)
  "Move the pointer over row ROW of the menu level DEPTH, whose popup is ID, giving that popup the keyboard."
  (let ((height (frame-char-height (cdr (assq depth nhwl-menu--frames)))))
    (gui-runner-recorder (list :cmd "focus" :surface id))
    (gui-runner-recorder (list :cmd "pointer" :pos (vector 10 (floor (* (+ row 0.5) height)))))))

(defun nhwl-gui--menu-button (button &optional release-only)
  "Press and release BUTTON (an evdev code) on the pointer's surface; only release it with RELEASE-ONLY."
  (unless release-only
    (gui-runner-recorder (list :cmd "button" :button button :state "press")))
  (gui-runner-recorder (list :cmd "button" :button button :state "release")))

(defun nhwl-gui--submenu-keymap ()
  "A menu of Top and More; More holds Inner1 and Inner2.  Rows: title, separator, Top, More."
  (let ((map (make-sparse-keymap "Menu")) (sub (make-sparse-keymap)))
    (define-key sub [inner1] '(menu-item "Inner1" ignore))
    (define-key sub [inner2] '(menu-item "Inner2" ignore))
    (define-key map [top] '(menu-item "Top" ignore))
    (define-key map [more] (list 'menu-item "More" sub))
    map))

(defun nhwl-gui--submenu-run (on-sub)
  "Open the submenu keymap and hover More; 0.6 s later, when the submenu's popup shows, ON-SUB is called with its id
and the parent's (from a timer).  With no submenu popup the menu is dismissed.  The result of `x-popup-menu'."
  (let ((before (nhwl-gui--popup-ids)) (timers nil))
    (prog1
        (nhwl-gui--menu-run
         (nhwl-gui--submenu-keymap)
         (lambda (id)
           (nhwl-gui--menu-over id 0 3)
           (push (run-at-time 0.6 nil
                              (lambda ()
                                (let ((sub (car (seq-difference (nhwl-gui--popup-ids) (cons id before)))))
                                  (if sub
                                      (funcall on-sub sub id)
                                    (gui-runner-recorder '(:cmd "click_outside"))))))
                 timers)))
      (mapc #'cancel-timer timers))))

(ert-deftest nhwl-gui/context-menu-press-drag-release ()
  "A menu opened by a `down-mouse-3' event, with the button still held, chooses the item under the release."
  :tags '(gnome-smoke)
  (skip-unless (nhwl-gui--recorder-p))
  (should (equal (nhwl-gui--menu-run
                  (nhwl-gui--menu-keymap)
                  (lambda (id)
                    (nhwl-gui--menu-over id 0 4)
                    (nhwl-gui--menu-button 273 t))
                  (list 'down-mouse-3 (posn-at-x-y 100 100 (selected-frame) t))
                  t)
                 '(gamma))))

(ert-deftest nhwl-gui/context-menu-click-keeps-it-open ()
  "The press that opened the menu coming up where it went down (a click, not a drag) only keeps the menu open; a
click on an item then chooses."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((before (nhwl-gui--popup-ids)) (open-after-first :unset) (timers nil))
    (unwind-protect
        (should (equal (nhwl-gui--menu-run
                        (nhwl-gui--menu-keymap)
                        (lambda (id)
                          (nhwl-gui--menu-button 273 t)
                          (nhwl-gui--menu-over id 0 4)
                          (push (run-at-time 0.4 nil
                                             (lambda ()
                                               (setq open-after-first
                                                     (and (seq-difference (nhwl-gui--popup-ids) before) t))
                                               (nhwl-gui--menu-button 272)))
                                timers))
                        (list 'down-mouse-3 (posn-at-x-y 100 100 (selected-frame) t))
                        t)
                       '(gamma)))
      (mapc #'cancel-timer timers))
    (should (eq open-after-first t))))

(ert-deftest nhwl-gui/menu-click-outside-returns-nil ()
  "A click outside the popup dismisses it: `x-popup-menu' returns nil and the connection is intact."
  (skip-unless (nhwl-gui--recorder-p))
  (should-not (nhwl-gui--menu-run (nhwl-gui--menu-keymap)
                                  (lambda (_id) (gui-runner-recorder '(:cmd "click_outside")))))
  (should (nhwl-gui--connected-p)))

(ert-deftest nhwl-gui/menu-hover-highlights-and-help-is-echoed ()
  "The row under the pointer is the selected one, and its :help is in the echo area."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((map (make-sparse-keymap "Menu")) (seen nil) (timers nil))
    (define-key map [alpha] '(menu-item "Alpha" ignore :help "Alpha does nothing"))
    (define-key map [gamma] '(menu-item "Gamma" ignore))
    (unwind-protect
        (nhwl-gui--menu-run
         map
         (lambda (id)
           (nhwl-gui--menu-over id 0 2)
           (push (run-at-time 0.4 nil
                              (lambda ()
                                (setq seen (list (buffer-local-value 'nhwl-menu--index
                                                                     (get-buffer " *nhwl-menu-0*"))
                                                 (current-message)))
                                (gui-runner-recorder '(:cmd "click_outside"))))
                 timers)))
      (mapc #'cancel-timer timers))
    (should (equal seen '(2 "Alpha does nothing")))
    (should-not (current-message))))

(ert-deftest nhwl-gui/menu-wheel-scrolls-a-tall-menu ()
  "A menu taller than the display is cut to it, and the wheel scrolls its rows."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((map (make-sparse-keymap "Tall")) (seen nil) (timers nil))
    (dotimes (i 200)
      (define-key map (vector (intern (format "item%d" i))) (list 'menu-item (format "Item %d" i) 'ignore)))
    (unwind-protect
        (nhwl-gui--menu-run
         map
         (lambda (id)
           (let* ((frame (cdr (assq 0 nhwl-menu--frames))) (window (frame-root-window frame))
                  ;; the first row shown: a line count in the menu's buffer, not the timer's
                  (top (lambda () (with-current-buffer (window-buffer window) (count-lines 1 (window-start window))))))
             (setq seen (list (<= (frame-pixel-height frame) (display-pixel-height frame)) (funcall top)))
             (gui-runner-recorder (list :cmd "focus" :surface id))
             (gui-runner-recorder (list :cmd "pointer" :pos (vector 10 20)))
             (gui-runner-recorder '(:cmd "axis" :source "wheel" :v120 120))
             (push (run-at-time 0.4 nil
                                (lambda ()
                                  (setq seen (append seen (list (funcall top))))
                                  (gui-runner-recorder '(:cmd "click_outside"))))
                   timers))))
      (mapc #'cancel-timer timers))
    (should (eq (car seen) t))
    (should (= (nth 1 seen) 0))
    (should (> (nth 2 seen) 0))))

(ert-deftest nhwl-gui/submenu-opens-and-selects ()
  "Resting on More opens its submenu beside the row; choosing Inner2 there returns both keys."
  :tags '(gnome-smoke)
  (skip-unless (nhwl-gui--recorder-p))
  (should (equal (nhwl-gui--submenu-run
                  (lambda (sub _parent)
                    (nhwl-gui--menu-over sub 1 1)
                    (nhwl-gui--menu-button 272)))
                 '(more inner2)))
  (should (nhwl-gui--connected-p)))

(ert-deftest nhwl-gui/submenu-keys-open-and-close-a-level ()
  "Right opens the submenu of the selected row, Left closes only that level, and Escape then cancels the menu."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((before (nhwl-gui--popup-ids)) (levels nil) (timers nil))
    (unwind-protect
        (should-not
         (nhwl-gui--menu-run
          (nhwl-gui--submenu-keymap)
          (lambda (_id)
            (dolist (k '(108 108 106))          ; Down Down Right
              (nhwl-gui--menu-send k))
            (push (run-at-time 0.5 nil
                               (lambda ()
                                 (push (length (seq-difference (nhwl-gui--popup-ids) before)) levels)
                                 (nhwl-gui--menu-send 105)   ; Left
                                 (push (run-at-time 0.5 nil
                                                    (lambda ()
                                                      (push (length (seq-difference (nhwl-gui--popup-ids) before))
                                                            levels)
                                                      (nhwl-gui--menu-send 1)))
                                       timers)))
                  timers))))
      (mapc #'cancel-timer timers))
    (should (equal (reverse levels) '(2 1)))))

(ert-deftest nhwl-gui/submenu-parent-deleted-closes-all ()
  "The menu frame a submenu hangs from is deleted from a timer while the submenu shows: the menu returns nil, only the
toplevel remains in the tree, and the connection has no protocol error."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((deleted nil))
    (should-not (nhwl-gui--submenu-run
                 (lambda (_sub _parent)
                   (setq deleted t)
                   (delete-frame (cdr (assq 0 nhwl-menu--frames)) t))))
    (should deleted)
    (should (nhwl-gui--connected-p))
    (should (frame-live-p (selected-frame)))
    (should-not (nhwl-gui--popup-ids))))

(ert-deftest nhwl-gui/menu-bar-click-opens-menu ()
  "A click on an item of the Emacs-drawn menu bar opens that item's menu below the bar."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((menu-bar-mode-was menu-bar-mode) (seen nil) (timers nil)
        (before (nhwl-gui--popup-ids)))
    (unwind-protect
        (progn
          (menu-bar-mode 1)
          (redisplay t)
          (nhwl-gui--menu-run
           (lambda ()
             (push (run-at-time 0.2 nil
                                (lambda ()
                                  (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
                                  ;; past the client-side title bar and margins where there are some
                                  (gui-runner-recorder (list :cmd "pointer"
                                                             :pos (apply #'vector (nhwl-gui--native-to-logical 5 5))))
                                  (nhwl-gui--menu-button 272)))
                   timers)
             ;; the menu is dismissed: a menu-bar menu cancelled by a click quits, as on every Emacs
             (condition-case nil (read-key-sequence nil) (quit nil)))
           (lambda (_id)
             (let ((frame (cdr (assq 0 nhwl-menu--frames))))
               (setq seen (list (nth 1 (frame-parameter frame 'nhwl-popup))
                                (frame-char-height (selected-frame)))))
             (gui-runner-recorder '(:cmd "click_outside")))))
      (mapc #'cancel-timer timers)
      (menu-bar-mode (if menu-bar-mode-was 1 -1)))
    (should seen)
    (should (>= (car seen) (cadr seen)))
    (should (equal (nhwl-gui--popup-ids) before))))

(ert-deftest nhwl-gui/f10-opens-the-menu-bar-menu ()
  "F10 (`menu-bar-open') shows `mouse-menu-bar-map' as a menu whose rows are the menu bar's items."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((seen nil))
    (should-not (nhwl-gui--menu-run
                 (lambda () (menu-bar-open) nil)
                 (lambda (_id)
                   (setq seen (with-current-buffer (get-buffer " *nhwl-menu-0*") (buffer-string)))
                   (nhwl-gui--menu-send 1))))
    (should (string-match-p "File" seen))
    (should (string-match-p "Edit" seen))))

(ert-deftest nhwl-gui/dialog-box-yes-or-no ()
  "`y-or-n-p' from a mouse command with `use-dialog-box' shows a dialog (a menu centred on the frame, the prompt as
its title); choosing Yes returns t."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((centred nil) (title nil))
    (should (eq t (nhwl-gui--menu-run
                   (lambda ()
                     (let ((use-dialog-box t)
                           (last-nonmenu-event (list 'mouse-1 (posn-at-x-y 10 10 (selected-frame) t)))
                           (last-input-event (list 'mouse-1 (posn-at-x-y 10 10 (selected-frame) t))))
                       (y-or-n-p "Proceed? ")))
                   (lambda (id)
                     (let* ((frame (cdr (assq 0 nhwl-menu--frames))) (parent (frame-parent frame))
                            (anchor (frame-parameter frame 'nhwl-popup)))
                       (setq centred (and (< (abs (- (+ (nth 0 anchor) (/ (frame-pixel-width frame) 2))
                                                     (/ (frame-pixel-width parent) 2)))
                                             3)
                                          (< (abs (- (+ (nth 1 anchor) (/ (frame-pixel-height frame) 2))
                                                     (/ (frame-pixel-height parent) 2)))
                                             3))
                             title (with-current-buffer (get-buffer " *nhwl-menu-0*")
                                     (buffer-substring 1 (line-end-position))))
                       (nhwl-gui--menu-over id 0 2)
                       (nhwl-gui--menu-button 272))))))
    (should centred)
    (should (string-match-p "Proceed" title))))

(ert-deftest nhwl-gui/menu-touch-tap-selects ()
  "A touch that begins and ends on one item chooses it."
  (skip-unless (nhwl-gui--recorder-p))
  (should (equal (nhwl-gui--menu-run
                  (nhwl-gui--menu-keymap)
                  (lambda (id)
                    (let* ((height (frame-char-height (cdr (assq 0 nhwl-menu--frames))))
                           (at (vector 10 (floor (* 4.5 height)))))
                      (gui-runner-recorder (list :cmd "touch" :op "down" :id 0 :at at :surface id))
                      (gui-runner-recorder (list :cmd "touch" :op "up" :id 0 :at [0 0])))))
                 '(gamma))))

(ert-deftest nhwl-gui/live-menus ()
  "Menus on a real compositor: one opened and chosen from, a submenu opened and chosen from, one dismissed with a
submenu open, all driven by queued events.  The trace rules (errors, popup, grab, popup-order) judge what was sent."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--live-p) (nhwl-gui--nhwl-frames)))
  (let ((map (nhwl-gui--submenu-keymap)))
    (nhwl-gui--live-settle 0.3)
    ;; Down Down Right Down Return: Top, More, the submenu, Inner1
    (let ((unread-command-events (list 'down 'down 'right 'down 'return)))
      (should (equal (x-popup-menu t map) '(more inner1))))
    (nhwl-gui--live-settle 0.3)
    ;; the same menus again: the frames are shown again; the compositor dismisses the stack with a submenu open
    (let ((unread-command-events (list 'down 'down 'right
                                       (list 'delete-frame (list (cdr (assq 0 nhwl-menu--frames)))))))
      (should-not (x-popup-menu t map)))
    (nhwl-gui--live-settle 0.3)
    (should (frame-live-p (selected-frame)))))

;; Losing the display.  When the compositor goes and nothing waits for it (here `nhwl-reconnect' is nil), the
;; display's frames are deleted, every loop waiting on the display ends, and the daemon stays.  The recorder's
;; `restart' in mode `gone' drops every client until `restart_up', as a compositor that quits does.  The rows end the
;; session's connection, so they run last and only under `--daemon' (Emacs outlives its last display there); each opens
;; a frame on the display again at its end, so the rows after it find one.

(defun nhwl-gui--lose-display (&optional delay)
  "End the session's connection for good; from a timer DELAY seconds later when DELAY is non-nil (the timer)."
  (if delay
      (run-at-time delay nil (lambda () (nhwl-gui--restart "gone")))
    (nhwl-gui--restart "gone")
    nil))

(defun nhwl-gui--display-gone-p ()
  "Read events for up to 3 s until no frame of the session's display is left; non-nil then."
  (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (null (nhwl-gui--nhwl-frames))) 3))

(defun nhwl-gui--display-terminal-p (display)
  "Non-nil when a live terminal is named DISPLAY."
  (seq-some (lambda (term) (equal (terminal-name term) display)) (terminal-list)))

(defun nhwl-gui--reopen-display ()
  "Serve the session's display again and select a new frame on it, once it has drawn; the frame."
  (ignore-errors (gui-runner-recorder '(:cmd "restart_up")))
  (let ((f (make-frame-on-display (getenv "WAYLAND_DISPLAY"))))
    (select-frame f)
    (nhwl-gui--wait (lambda () (with-selected-frame f (redisplay t)) (nhwl-gui--toplevel-commits)) 5)
    f))

(ert-deftest nhwl-gui/display-lost-with-everything-open ()
  "A child frame, a tooltip and a menu are open when the compositor goes for good: `x-popup-menu' returns nil and
signals nothing, no frame and no terminal of the display is left, the daemon stays, and the connection hook runs
`lost' once, with the display's name."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--recorder-p) (daemonp)))
  (let ((nhwl-reconnect nil) (display (getenv "WAYLAND_DISPLAY")) (names nil) (timer nil) (result 'unset) (err nil))
    (unwind-protect
        (nhwl-gui--with-connection-log
         (let ((note (lambda (name _state) (push name names))))
           (add-hook 'nhwl-display-connection-functions note)
           (unwind-protect
               (progn
                 (nhwl-gui--child)
                 (x-show-tip "a tip" nil nil 60)
                 (nhwl-gui--press-serial)
                 (setq timer (nhwl-gui--lose-display 0.5))
                 (setq result (condition-case e
                                  (with-timeout (10 'timeout)
                                    (x-popup-menu (list '(40 40) (selected-frame))
                                                  '("Menu" ("Pane" ("one" . 1) ("two" . 2)))))
                                (error (setq err e) 'error)))
                 (should-not err)
                 (should-not result)
                 (should (nhwl-gui--display-gone-p))
                 (should-not (nhwl-gui--display-terminal-p display))
                 (should (daemonp))
                 (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) nhwl-gui--connection-log) 2))
                 (nhwl-gui--pump 0.3)
                 (should (equal nhwl-gui--connection-log '(lost)))
                 (should (equal names (list display))))
             (remove-hook 'nhwl-display-connection-functions note))))
      (when timer (cancel-timer timer))
      (nhwl-gui--reopen-display))))

(ert-deftest nhwl-gui/display-lost-during-drag-out ()
  "A drag out of Emacs is in flight when the compositor goes for good: `x-begin-drag' returns nil and signals nothing,
no frame of the display is left, and the daemon stays."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--recorder-p) (daemonp) (fboundp 'x-begin-drag)))
  (let ((nhwl-reconnect nil) (timer nil) (result 'unset) (err nil))
    (unwind-protect
        (nhwl-gui--with-connection-log
         (ignore-errors (gui-set-selection 'XdndSelection "dragged text"))
         (nhwl-gui--point 100 100)
         (gui-runner-recorder '(:cmd "button" :button 272 :state "press"))
         (nhwl-gui--pump 0.2)
         (setq timer (nhwl-gui--lose-display 0.3))
         (setq result (condition-case e
                          (with-timeout (10 'timeout)
                            (x-begin-drag '("text/plain;charset=utf-8") 'XdndActionCopy nil nil t))
                        (error (setq err e) 'error)))
         (should-not err)
         (should-not result)
         (should (nhwl-gui--display-gone-p))
         (should (daemonp))
         (should (nhwl-gui--wait (lambda () (nhwl-gui--pump 0.1) (memq 'lost nhwl-gui--connection-log)) 2)))
      (when timer (cancel-timer timer))
      (nhwl-gui--reopen-display))))

(ert-deftest nhwl-gui/display-restart-twenty-cycles ()
  "Twenty cycles of a frame, a child frame and a tooltip, then a compositor restart (`NHWL_GUI_RESTART') with
reconnecting off: each time the display's frames are deleted and the daemon stays.  After cycle 20 the descriptor and
thread counts equal cycle 5's, VmRSS grew less than 8 MiB since cycle 5, and `make-frame-on-display' works."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--recorder-p) (daemonp) (gui-runner-restart-available-p)))
  (let ((nhwl-reconnect nil) (display (getenv "WAYLAND_DISPLAY")) (at5 nil) (f nil))
    (unwind-protect
        (progn
          (dotimes (i 20)
            (setq f (or (car (nhwl-gui--nhwl-frames)) (make-frame-on-display display)))
            (select-frame f)
            (nhwl-gui--child)
            (x-show-tip (format "cycle %d" i) nil nil 60)
            (nhwl-gui--pump 0.2)
            (gui-runner-restart-compositor)
            (should (nhwl-gui--display-gone-p))
            (should (daemonp))
            (x-hide-tip)
            (when (= i 4)
              (garbage-collect)
              (setq at5 (nhwl-gui--proc-counts))))
          (garbage-collect)
          (let ((at20 (nhwl-gui--proc-counts)))
            (should (= (nth 0 at20) (nth 0 at5)))
            (should (= (nth 1 at20) (nth 1 at5)))
            (should (< (- (nth 2 at20) (nth 2 at5)) (* 8 1024)))))
      (should (frame-live-p (nhwl-gui--reopen-display))))))

(ert-deftest nhwl-gui/live-sway-restart ()
  "sway is killed and started again on the same socket (`NHWL_GUI_RESTART') with reconnecting off: the display's
frames are deleted, the daemon stays, and a new frame opens on the new sway.  The trace rule `errors' judges the second
connection."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--live-p) (daemonp) (gui-runner-restart-available-p)
                    (equal (getenv "GLITNIR_GUI_COMPOSITOR") "sway")))
  (let ((nhwl-reconnect nil) (f nil))
    (nhwl-gui--live-settle 0.5)
    (gui-runner-restart-compositor)
    (should (nhwl-gui--display-gone-p))
    (should (daemonp))
    (setq f (make-frame-on-display (getenv "WAYLAND_DISPLAY")))
    (select-frame f)
    (nhwl-gui--live-settle 0.5)
    (should (frame-live-p f))))

;; The file chooser portal and activation tokens for xdg-open (lisp/term/nhwl-portal.el).  Rows that need the
;; portal run with gui.sh --portal fake (its script and log files are in the environment); the minibuffer row runs
;; with --portal none.

(defun nhwl-gui--fake-portal-p ()
  "Non-nil on the recorder when the run has the fake portal."
  (and (nhwl-gui--recorder-p) (getenv "NHWL_FAKE_PORTAL_SCRIPT") t))

(defun nhwl-gui--portal-calls ()
  "The fake portal's logged calls (not the closes), oldest first."
  (seq-filter (lambda (e) (plist-get e :method)) (gui-runner-portal-log)))

(ert-deftest nhwl-gui/file-dialog-uses-the-portal ()
  "`x-file-dialog' calls the portal parented by the toplevel's export handle, and returns the scripted file."
  (skip-unless (and (nhwl-gui--fake-portal-p) (nhwl-gui--has-global-p :zxdg_exporter_v2)))
  (gui-runner-portal-script '((:response 0 :uris ("file:///tmp/nhwl-chosen%20file"))))
  (let ((before (length (nhwl-gui--portal-calls))))
    (should (equal (x-file-dialog "Open a file" "/tmp/" nil t) "/tmp/nhwl-chosen file"))
    (let* ((calls (nhwl-gui--portal-calls))
           (call (car (last calls)))
           (exports (plist-get (gui-runner-recorder '(:cmd "exports")) :exports)))
      (should (= (length calls) (1+ before)))
      (should (equal (plist-get call :method) "OpenFile"))
      (should (equal (plist-get call :title) "Open a file"))
      (should (equal (plist-get call :parent_window)
                     (concat "wayland:" (plist-get (seq-first exports) :handle))))
      (should (cdr (assoc "modal" (plist-get call :options)))))))

(ert-deftest nhwl-gui/file-dialog-cancel-quits ()
  "A cancelled portal dialog (response 1) quits."
  (skip-unless (nhwl-gui--fake-portal-p))
  (gui-runner-portal-script '((:response 1)))
  (should (eq (condition-case nil (progn (x-file-dialog "Save as" "/tmp/" "/tmp/a.txt") 'returned)
                (quit 'quit))
              'quit))
  (should (equal (plist-get (car (last (nhwl-gui--portal-calls))) :method) "SaveFile")))

(ert-deftest nhwl-gui/file-dialog-c-g-closes-the-request ()
  "C-g while the dialog waits sends Close to its request and quits."
  (skip-unless (nhwl-gui--fake-portal-p))
  (gui-runner-portal-script '((:never t)))
  (nhwl-gui--point 100 100)
  (let ((later (nhwl-gui--recorder-later
                '((0.8 :cmd "key" :keycode 29 :state "press")
                  (0 :cmd "key" :text "g")
                  (0 :cmd "key" :keycode 29 :state "release")))))
    (unwind-protect
        (progn
          (should (eq (condition-case nil (progn (x-file-dialog "Open" "/tmp/" nil t) 'returned)
                        (quit 'quit))
                      'quit))
          (should (nhwl-gui--wait (lambda () (seq-find (lambda (e) (plist-get e :close)) (gui-runner-portal-log)))
                                  3))
          (should (nhwl-gui--connected-p)))
      (nhwl-gui--drag-cleanup later))))

(ert-deftest nhwl-gui/file-dialog-without-portal-uses-the-minibuffer ()
  "Without the file chooser interface `x-file-dialog' asks in the minibuffer."
  :tags '(gnome-smoke)
  (skip-unless (and (nhwl-gui--live-p) (not (getenv "NHWL_FAKE_PORTAL_SCRIPT"))))
  ;; MUSTMATCH: the typed file exists (a missing one would leave the minibuffer waiting); the minibuffer starts
  ;; with the folder, so its name alone is typed
  (let ((file (make-temp-file "/tmp/nhwl-typed")))
    (unwind-protect
        (let ((unread-command-events (append (file-name-nondirectory file) '(return))))
          (should (equal (x-file-dialog "Open: " "/tmp/" nil t) file)))
      (delete-file file))))

(ert-deftest nhwl-gui/browse-url-passes-an-activation-token ()
  "`browse-url-xdg-open' runs xdg-open with an XDG_ACTIVATION_TOKEN the compositor issued."
  (skip-unless (nhwl-gui--recorder-p))
  (require 'browse-url)
  (let* ((dir (make-temp-file "nhwl-xdg" t))
         (out (expand-file-name "env" dir))
         (script (expand-file-name "xdg-open" dir))
         (exec-path (cons dir exec-path)))
    (unwind-protect
        (progn
          (with-temp-file script (insert "#!/bin/sh\nenv > '" out "'\n"))
          (set-file-modes script #o755)
          (nhwl-gui--press-serial)
          (browse-url-xdg-open "https://example.org/")
          (should (nhwl-gui--wait (lambda () (and (file-exists-p out) (> (file-attribute-size (file-attributes out)) 0)))
                                  5))
          (let ((token (with-temp-buffer
                         (insert-file-contents out)
                         (and (re-search-forward "^XDG_ACTIVATION_TOKEN=\\(.+\\)$" nil t) (match-string 1)))))
            (should (stringp token))
            (should (seq-find (lambda (a) (equal (plist-get a :token) token))
                              (plist-get (gui-runner-recorder '(:cmd "activation_log")) :activations)))))
      (delete-directory dir t))))

(provide 'nhwl-gui-w2-tests)

;;; nhwl-gui-w2-tests.el ends here
