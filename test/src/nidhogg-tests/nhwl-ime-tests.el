;;; nhwl-ime-tests.el --- input-method rows for the native shell  -*- lexical-binding: t -*-

;; Run by nidhogg/test/ime-matrix.sh (through gui.sh --ime IM --files nhwl-ime-tests.el), one container per
;; combination.  The rows type real keys at the compositor (wtype on sway and KWin, mutter's RemoteDesktop D-Bus
;; interface on mutter) and the input method (fcitx5 or ibus, pinyin) answers with preedit and commit_string.
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

(defun nhwl-ime--type (letters &optional space)
  "Type the lower-case LETTERS, then a space when SPACE, at the compositor.
Skip the row if it cannot be driven."
  (if (equal (getenv "GLITNIR_GUI_COMPOSITOR") "mutter")
      (condition-case err
          (let ((svc "org.gnome.Mutter.RemoteDesktop"))
            (dolist (c (append letters (and space " ") nil))
              (dolist (down '(t nil))
                (dbus-call-method :session svc (nhwl-ime--rd-session) (concat svc ".Session")
                                  "NotifyKeyboardKeysym" :uint32 c :boolean down))
              (nhwl-ime--pump 0.05)))
        (error (nhwl-ime--skip "mutter's RemoteDesktop refused the keys: %S" err)))
    (unless (executable-find "wtype") (nhwl-ime--skip "no wtype"))
    (with-temp-buffer
      (let ((rc (apply #'call-process "wtype" nil t nil "-d" "60" letters (and space '("-k" "space")))))
        (unless (eq rc 0)
          (nhwl-ime--skip "%s cannot type through wtype: %s" (getenv "GLITNIR_GUI_COMPOSITOR")
                          (string-trim (buffer-string))))))
    (nhwl-ime--pump 0.3)))

(defun nhwl-ime--ready ()
  "Skip unless this run is an IME combination; show a fresh buffer."
  (unless (getenv "NHWL_IME_COMBO") (nhwl-ime--skip "not an IME-matrix run"))
  (unless (featurep 'nidhogg-wayland) (nhwl-ime--skip "not the native shell"))
  (unless (nhwl-ime--wait (lambda () (frame-focus-state)) 5) (nhwl-ime--skip "the frame never got the keyboard"))
  (switch-to-buffer (get-buffer-create "*ime*"))
  (erase-buffer)
  (when (overlayp (bound-and-true-p pgtk-preedit-overlay)) (delete-overlay pgtk-preedit-overlay))
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
        (when (re-search-backward (concat "zwp_text_input_v3@[0-9]+\\.set_cursor_rectangle("
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

;;; nhwl-ime-tests.el ends here
