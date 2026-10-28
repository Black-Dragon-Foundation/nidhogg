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

(provide 'nhwl-gui-w2-tests)

;;; nhwl-gui-w2-tests.el ends here
