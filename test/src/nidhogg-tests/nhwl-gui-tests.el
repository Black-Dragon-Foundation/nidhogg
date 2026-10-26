;;; nhwl-gui-tests.el --- GUI rows for Nidhogg's own Wayland shell  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh `--compositor recorder' (the rows need $GLITNIR_GUI_RECORDER, the recorder's control
;; socket) in a build made with --with-nidhogg-wayland.  `gui-runner-recorder' lives in gui-runner.el.  The rows run
;; from a timer, so input sits unread until a row calls `read-event'.

(require 'ert)
(require 'json)
(require 'seq)

(defun nhwl-gui--on-p ()
  (and (featurep 'nidhogg-wayland) (getenv "GLITNIR_GUI_RECORDER")))

(defun nhwl-gui--get (c key)
  "KEY of commit C, nil for a JSON null (`json-parse-string' gives `:null', which is non-nil)."
  (let ((v (plist-get c key))) (unless (eq v :null) v)))

(defun nhwl-gui--commits ()
  (plist-get (gui-runner-recorder '(:cmd "log" :since 0)) :commits))

(defun nhwl-gui--wait (pred &optional seconds)
  "Wait up to SECONDS (default 3) for PRED to return non-nil; return its value."
  (let ((deadline (+ (float-time) (or seconds 3))) (v nil))
    (while (and (not (setq v (funcall pred))) (< (float-time) deadline))
      (accept-process-output nil 0.05))
    v))

(defun nhwl-gui--toplevel-commits ()
  (seq-filter (lambda (c)
                (and (equal (plist-get c :role) "toplevel")
                     (equal (nhwl-gui--get c :app_id) "nidhogg")
                     (nhwl-gui--get c :buffer)))
              (nhwl-gui--commits)))

(ert-deftest nhwl-gui/features ()
  (skip-unless (featurep 'nidhogg-wayland))
  (should (eq (window-system) 'pgtk))
  (should (featurep 'pgtk))
  (should (featurep 'nidhogg-wayland))
  (should (floatp (frame-scale-factor))))

(ert-deftest nhwl-gui/frame-maps-on-recorder ()
  (skip-unless (nhwl-gui--on-p))
  (let* ((s (frame-scale-factor))
         (hit (nhwl-gui--wait
               (lambda ()
                 (seq-find
                  (lambda (c)
                    (let* ((b (plist-get c :buffer))
                           (dst (plist-get (plist-get c :viewport) :dst))
                           (lw (if (consp dst) (car dst) (plist-get b :w)))
                           (lh (if (consp dst) (cadr dst) (plist-get b :h))))
                      (and (= (plist-get c :buffer_scale) 1)
                           (= (plist-get b :w) (round (* lw s)))
                           (= (plist-get b :h) (round (* lh s))))))
                  (nhwl-gui--toplevel-commits))))))
    (should hit)))

(ert-deftest nhwl-gui/typing-inserts ()
  (skip-unless (nhwl-gui--on-p))
  (let ((buf (generate-new-buffer " *nhwl-typing*")))
    (unwind-protect
        (progn
          (switch-to-buffer buf)
          (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
          (sit-for 0.2)
          (dolist (ch '("a" "b" "c"))
            (gui-runner-recorder (list :cmd "key" :text ch)))
          ;; the keys wait in the input queue: read them and run their bindings
          (should (nhwl-gui--wait
                   (lambda ()
                     (let ((ev (read-event nil nil 0.1)))
                       (when (characterp ev)
                         (setq last-command-event ev)
                         (command-execute (key-binding (vector ev)) nil (vector ev))))
                     (equal (buffer-string) "abc")))))
      (kill-buffer buf))))

(ert-deftest nhwl-gui/keymap-option-gives-control ()
  (skip-unless (nhwl-gui--on-p))
  (let ((buf (generate-new-buffer " *nhwl-keymap*")))
    (unwind-protect
        (progn
          (switch-to-buffer buf)
          (insert "hello")
          (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
          (gui-runner-recorder '(:cmd "keymap" :rmlvo (:rules "evdev" :model "pc105" :layout "us" :variant ""
                                                       :options "ctrl:swap_lalt_lctl_lwin")))
          (sit-for 0.3)
          ;; KEY_LEFTALT (56) is Control_L under the option; "a" then arrives as C-a
          (gui-runner-recorder '(:cmd "key" :keycode 56 :state "press"))
          (gui-runner-recorder '(:cmd "key" :text "a"))
          (gui-runner-recorder '(:cmd "key" :keycode 56 :state "release"))
          (should (nhwl-gui--wait
                   (lambda ()
                     (let ((ev (read-event nil nil 0.1)))
                       (when (eq ev ?\C-a)
                         (command-execute (key-binding (vector ev)) nil (vector ev))))
                     (= (point) (point-min))))))
      (gui-runner-recorder '(:cmd "keymap" :rmlvo (:options "")))
      (kill-buffer buf))))

(ert-deftest nhwl-gui/fullscreen-parameter ()
  (skip-unless (nhwl-gui--on-p))
  (let* ((s (frame-scale-factor))
         (size (getenv "GLITNIR_GUI_SIZE"))
         (ow (string-to-number (car (split-string size "x"))))
         (oh (string-to-number (cadr (split-string size "x"))))
         (lw (round (/ ow s))) (lh (round (/ oh s))))
    ;; the runner starts the frame fullscreen: leave it first, to a small window
    (set-frame-parameter nil 'fullscreen nil)
    (gui-runner-recorder '(:cmd "configure" :surface "focused" :size [800 600] :states ["activated"]))
    (should (nhwl-gui--wait
             (lambda ()
               (redisplay t)
               (and (<= (frame-pixel-width) (round (* 800 s)))
                    (>= (frame-pixel-width) (- (round (* 800 s)) (frame-char-width)))))))
    (let ((since (plist-get (gui-runner-recorder '(:cmd "input_log" :since 0)) :next)))
      (set-frame-parameter nil 'fullscreen 'fullboth)
      (should (nhwl-gui--wait
               (lambda ()
                 (seq-find (lambda (e) (equal (plist-get e :req) "xdg_toplevel.set_fullscreen"))
                           (plist-get (gui-runner-recorder (list :cmd "input_log" :since since)) :entries)))))
      ;; the compositor answers with the output's size
      (gui-runner-recorder (list :cmd "configure" :surface "focused" :size (vector lw lh)
                                 :states ["fullscreen" "activated"]))
      (should (nhwl-gui--wait
               (lambda ()
                 (redisplay t)
                 (and (= (frame-pixel-width) (round (* lw s)))
                      (= (frame-pixel-height) (round (* lh s))))))))))

;;; nhwl-gui-tests.el ends here
