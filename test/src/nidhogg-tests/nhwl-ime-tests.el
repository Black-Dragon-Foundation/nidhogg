;;; nhwl-ime-tests.el --- input-method rows for the native shell  -*- lexical-binding: t -*-
;; test-class: cond
;; test-pattern: handshake

;; Run by nidhogg/test/ime-matrix.sh (through gui.sh --ime IM --files nhwl-ime-tests.el), one container per
;; combination.  The rows type real keys at the compositor with wtype and the input method (fcitx5, pinyin) answers
;; with preedit and commit_string; on mutter only the cursor rectangle runs (mutter's RemoteDesktop D-Bus interface
;; attaches the keyboard).
;; Without NHWL_IME_COMBO (any other run) every row skips.  A combination that cannot be driven skips with a reason,
;; printed as "nhwl-ime-skip ROW: REASON" for the rig; it is never a failure.
;;
;; Environment: NHWL_IME_COMBO (sway-fcitx5, kwin-fcitx5, gnome-ibus), NHWL_IME (fcitx5 or ibus),
;; NHWL_IME_TRACE (the WAYLAND_DEBUG trace file, which the cursor-rectangle row reads).

(require 'ert)
(require 'dbus)
(require 'seq)

(defun nhwl-ime--skip (fmt &rest args)
  "Skip the running row, naming the reason on the log."
  (let ((reason (apply #'format fmt args)))
    (when (fboundp 'glitnir-gui--say) (glitnir-gui--say "nhwl-ime-skip %s: %s" (ert-test-name (ert-running-test)) reason))
    (ert-skip reason)))

(defun nhwl-ime--pump (&optional seconds)
  "Read events for SECONDS (default 0.5).
Committed characters arrive as keystrokes; insert them."
  (let ((deadline (+ (float-time) (or seconds 0.5))))
    (while (< (float-time) deadline)
      (let ((ev (read-event nil nil 0.05)))
        (when (and ev (characterp ev))
          (let ((last-command-event ev)) (self-insert-command 1))))
      (redisplay t))))

(defun nhwl-ime--wait (pred &optional seconds)
  "Pump until PRED returns non-nil, up to SECONDS (default 5); return its value."
  (let ((deadline (+ (float-time) (or seconds 5))) v)
    (while (and (not (setq v (funcall pred))) (< (float-time) deadline))
      (nhwl-ime--pump 0.1))
    v))

(defvar nhwl-ime--rd nil
  "The RemoteDesktop session path (mutter), held by Emacs's own D-Bus connection.")

(defun nhwl-ime--rd-session ()
  (or nhwl-ime--rd
      (let* ((svc "org.gnome.Mutter.RemoteDesktop")
             (s (dbus-call-method :session svc "/org/gnome/Mutter/RemoteDesktop" svc "CreateSession")))
        (dbus-call-method :session svc s (concat svc ".Session") "Start")
        (setq nhwl-ime--rd s))))

(defun nhwl-ime--mutter-key (keysym)
  "Press and release KEYSYM through mutter's RemoteDesktop session."
  (let ((svc "org.gnome.Mutter.RemoteDesktop"))
    (dolist (down '(t nil))
      (dbus-call-method :session svc (nhwl-ime--rd-session) (concat svc ".Session")
                        "NotifyKeyboardKeysym" :uint32 keysym :boolean down))))

(defun nhwl-ime--wtype (&rest args)
  "Run wtype with ARGS, its own protocol trace off; skip the row if it fails."
  (with-temp-buffer
    (let* ((process-environment (cons "WAYLAND_DEBUG" process-environment))
           (rc (apply #'call-process "wtype" nil t nil args)))
      (unless (eq rc 0)
        (nhwl-ime--skip "%s cannot type through wtype: %s" (getenv "GLITNIR_GUI_COMPOSITOR")
                        (string-trim (buffer-string)))))))

(defun nhwl-ime--type (letters &optional space)
  "Type the lower-case LETTERS, then a space when SPACE, at the compositor.
Skip the row if it cannot be driven."
  (when (equal (getenv "GLITNIR_GUI_COMPOSITOR") "mutter")
    (nhwl-ime--skip "bare mutter has no input method: GNOME's bridge to IBus is gnome-shell's, which needs logind"))
  (apply #'nhwl-ime--wtype "-d" "60" letters (and space '("-k" "space")))
  (nhwl-ime--pump 0.3))

(defvar nhwl-ime--keyboard nil
  "The wtype process that keeps a keyboard on the seat.")

(defun nhwl-ime--keyboard-runs-p ()
  "Whether wtype's virtual keyboard (started once, kept for the run) still runs."
  (and (processp nhwl-ime--keyboard) (eq (process-status nhwl-ime--keyboard) 'run)))

(defun nhwl-ime--attach-keyboard ()
  "Give the seat a keyboard, as a real session has.
A headless compositor has none, so Emacs gets no keyboard focus and never turns text input on.  Mutter's session keeps
one after a first key; on the others wtype's virtual keyboard lasts as long as wtype, which sleeps for the whole run."
  (if (equal (getenv "GLITNIR_GUI_COMPOSITOR") "mutter")
      (condition-case err
          (nhwl-ime--mutter-key #xffe1)   ; Shift_L, which types nothing
        (error (nhwl-ime--skip "mutter's RemoteDesktop refused the keys: %S" err)))
    (unless (executable-find "wtype") (nhwl-ime--skip "no wtype"))
    (unless (nhwl-ime--keyboard-runs-p)
      (let ((process-environment (cons "WAYLAND_DEBUG" process-environment)))
        (setq nhwl-ime--keyboard
              (start-process "nhwl-ime-keyboard" " *nhwl-ime-keyboard*" "wtype" "-s" "600000"))
        (set-process-query-on-exit-flag nhwl-ime--keyboard nil)))))

(defun nhwl-ime--ready ()
  "Skip unless this run is an IME combination; show a fresh buffer with the keyboard focus."
  (unless (getenv "NHWL_IME_COMBO") (nhwl-ime--skip "not an IME-matrix run"))
  (unless (featurep 'nidhogg-wayland) (nhwl-ime--skip "not the native shell"))
  (switch-to-buffer (get-buffer-create "*ime*"))
  (erase-buffer)
  (when (overlayp (bound-and-true-p pgtk-preedit-overlay)) (delete-overlay pgtk-preedit-overlay))
  (nhwl-ime--attach-keyboard)
  (unless (nhwl-ime--wait (lambda () (eq (frame-focus-state) t)) 5)
    (nhwl-ime--skip "%s gave no keyboard focus: %s" (getenv "GLITNIR_GUI_COMPOSITOR")
                    (if (nhwl-ime--keyboard-runs-p)
                        "the keyboard is attached"
                      (with-current-buffer " *nhwl-ime-keyboard*" (string-trim (buffer-string))))))
  (nhwl-ime--pump 0.5))

(ert-deftest nhwl-ime/pinyin-commits-cjk ()
  (nhwl-ime--ready)
  (nhwl-ime--type "nihao" t)
  (should (nhwl-ime--wait (lambda () (string-match-p "你好" (buffer-string))) 5)))

(ert-deftest nhwl-ime/preedit-overlay-shown ()
  (nhwl-ime--ready)
  (nhwl-ime--type "ni")
  (should (nhwl-ime--wait (lambda ()
                            (and (bound-and-true-p pgtk-preedit-overlay)
                                 (overlay-buffer pgtk-preedit-overlay)
                                 (not (string-empty-p (or (overlay-get pgtk-preedit-overlay 'before-string) "")))))
                          5)))

(defun nhwl-ime--last-rect ()
  "The last set_cursor_rectangle (X Y W H) in the trace's final 2 MB, or nil."
  (let ((f (getenv "NHWL_IME_TRACE")))
    (when (and f (file-readable-p f))
      (with-temp-buffer
        (insert-file-contents f nil (max 0 (- (file-attribute-size (file-attributes f)) 2000000)))
        (goto-char (point-max))
        (when (re-search-backward (concat "zwp_text_input_v3[#@][0-9]+\\.set_cursor_rectangle("
                                          "\\(-?[0-9]+\\), \\(-?[0-9]+\\), \\(-?[0-9]+\\), \\(-?[0-9]+\\))")
                                  nil t)
          (mapcar (lambda (i) (string-to-number (match-string i))) '(1 2 3 4)))))))

(ert-deftest nhwl-ime/cursor-rectangle-follows-point ()
  (nhwl-ime--ready)
  (unless (getenv "NHWL_IME_TRACE") (nhwl-ime--skip "no protocol trace (gui.sh --wayland-debug)"))
  (insert "abcdefghijklmnop")
  (beginning-of-line)
  (nhwl-ime--pump 1)
  (let ((before (nhwl-ime--last-rect)))
    (unless before (nhwl-ime--skip "the trace has no set_cursor_rectangle (text input never enabled)"))
    (end-of-line)
    (should (nhwl-ime--wait (lambda () (let ((r (nhwl-ime--last-rect))) (and r (> (car r) (car before))))) 5))))

(defmacro nhwl-ime--in-child (child &rest body)
  "BODY with CHILD bound to a child frame of the selected frame at (200, 120), its window showing a fresh buffer and
holding the keyboard focus; the child is deleted after."
  (declare (indent 1))
  `(let* ((root (selected-frame))
          (buf (get-buffer-create "*ime-child*"))
          (,child (make-frame `((parent-frame . ,root) (left . 200) (top . 120) (width . 40) (height . 6)
                                (minibuffer . nil) (visibility . t)))))
     (unwind-protect
         (progn
           (with-current-buffer buf (erase-buffer))
           (set-window-buffer (frame-root-window ,child) buf)
           (select-frame-set-input-focus ,child)
           (select-window (frame-root-window ,child))
           (set-buffer buf)
           (nhwl-ime--pump 0.5)
           ,@body)
       (delete-frame ,child)
       (select-frame-set-input-focus root))))

(ert-deftest nhwl-ime/preedit-and-commit-in-a-child-frame ()
  ;; a child frame is a subsurface of its root, which holds the text input: preedit and commit reach the child's buffer
  (nhwl-ime--ready)
  (nhwl-ime--in-child child
    (nhwl-ime--type "ni")
    (should (nhwl-ime--wait (lambda ()
                              (and (bound-and-true-p pgtk-preedit-overlay)
                                   (eq (overlay-buffer pgtk-preedit-overlay) (get-buffer "*ime-child*"))
                                   (not (string-empty-p (or (overlay-get pgtk-preedit-overlay 'before-string) "")))))
                            5))
    (nhwl-ime--wtype "-k" "space")
    (should (nhwl-ime--wait (lambda () (with-current-buffer "*ime-child*" (string-match-p "\\cc" (buffer-string))))
                            5))))

(ert-deftest nhwl-ime/cursor-rectangle-in-a-child-frame ()
  ;; the rectangle is in the root's surface, so it carries the child's offset within the root
  (nhwl-ime--ready)
  (unless (getenv "NHWL_IME_TRACE") (nhwl-ime--skip "no protocol trace (gui.sh --wayland-debug)"))
  (nhwl-ime--in-child child
    (insert "abcdefgh")
    (let ((pos (frame-position child)))
      (should (nhwl-ime--wait (lambda () (let ((r (nhwl-ime--last-rect)))
                                           (and r (>= (nth 0 r) (car pos)) (>= (nth 1 r) (cdr pos)))))
                              5)))))

;;; nhwl-ime-tests.el ends here
