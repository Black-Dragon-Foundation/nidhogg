;;; recorder-gui-tests.el --- GUI tests against nh-recorder  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh `--compositor recorder' (Task 4 teaches gui.sh), where $GLITNIR_GUI_RECORDER is the
;; recorder's control socket.  `gui-runner-recorder' lives in gui-runner.el.  The rows skip without it.

(require 'ert)
(require 'json)

(defun recorder-gui--commits ()
  (plist-get (gui-runner-recorder '(:cmd "log" :since 0)) :commits))

(ert-deftest recorder-gui/pgtk-frame-maps-as-toplevel ()
  (skip-unless (getenv "GLITNIR_GUI_RECORDER"))
  (let* ((scale (frame-scale-factor))
         (w (* scale (frame-pixel-width)))
         (h (* scale (frame-pixel-height)))
         (hit (seq-find (lambda (c)
                          (and (equal (plist-get c :role) "toplevel")
                               (equal (plist-get c :app_id) "nidhogg")
                               (plist-get c :buffer)
                               (= (plist-get (plist-get c :buffer) :w) w)
                               (= (plist-get (plist-get c :buffer) :h) h)))
                        (recorder-gui--commits))))
    (should hit)))

(ert-deftest recorder-gui/pgtk-acks-maximize ()
  (skip-unless (getenv "GLITNIR_GUI_RECORDER"))
  (let* ((seq-before (plist-get (gui-runner-recorder '(:cmd "log" :since 0)) :next))
         (serial (plist-get (gui-runner-recorder
                             '(:cmd "configure" :surface "focused" :size [800 600] :states ["maximized" "activated"]))
                            :serial))
         (acked nil)
         (deadline (+ (float-time) 2)))
    (while (and (not acked) (< (float-time) deadline))
      (accept-process-output nil 0.05)
      (setq acked (seq-find (lambda (c)
                              (and (>= (plist-get c :seq) seq-before)
                                   (eql (plist-get c :ack_serial) serial)))
                            (recorder-gui--commits))))
    (should acked)
    (should-not (eq (plist-get acked :ack_stale) t))
    (should (eq (frame-parameter nil 'fullscreen) 'maximized))))

(defun recorder-gui--wait (pred &optional seconds)
  "Wait up to SECONDS (default 3) for PRED to return non-nil; return its value."
  (let ((deadline (+ (float-time) (or seconds 3))) (v nil))
    (while (and (not (setq v (funcall pred))) (< (float-time) deadline))
      (accept-process-output nil 0.05))
    v))

(ert-deftest recorder-gui/keymap-options-reach-the-client ()
  (skip-unless (getenv "GLITNIR_GUI_RECORDER"))
  (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
  (unwind-protect
      (progn
        (gui-runner-recorder '(:cmd "keymap" :rmlvo (:rules "evdev" :model "pc105" :layout "us" :variant ""
                                                     :options "ctrl:swap_lalt_lctl_lwin")))
        (sit-for 0.3)
        (setq last-input-event nil)
        ;; KEY_LEFTALT (56) is Control_L under the option; "a" then arrives as C-a
        (gui-runner-recorder '(:cmd "key" :keycode 56 :state "press"))
        (gui-runner-recorder '(:cmd "key" :text "a"))
        (gui-runner-recorder '(:cmd "key" :keycode 56 :state "release"))
        (should (recorder-gui--wait (lambda () (eq last-input-event ?\C-a)))))
    (gui-runner-recorder '(:cmd "keymap" :rmlvo (:options "")))))

(ert-deftest recorder-gui/wheel-scrolls ()
  (skip-unless (getenv "GLITNIR_GUI_RECORDER"))
  (let ((buf (generate-new-buffer " *recorder-wheel*")))
    (unwind-protect
        (progn
          (switch-to-buffer buf)
          (dotimes (i 200) (insert (format "line %d\n" i)))
          (goto-char (point-min))
          (set-window-start (selected-window) (point-min))
          (redisplay t)
          (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
          (gui-runner-recorder '(:cmd "pointer" :pos [100 100]))
          (let ((start (window-start)))
            (gui-runner-recorder '(:cmd "axis" :source "wheel" :v120 120 :stop :false))
            (should (recorder-gui--wait (lambda () (> (window-start) start))))))
      (kill-buffer buf))))

(ert-deftest recorder-gui/snapshot-equals-glitnir-snapshot ()
  (skip-unless (getenv "GLITNIR_GUI_RECORDER"))
  (skip-unless (= (frame-scale-factor) 1))
  (redisplay t)
  (let* ((path (make-temp-file "recorder-snapshot" nil ".bgra"))
         (reply (progn (gui-runner-recorder (list :cmd "snapshot" :path path :format "bgra"))))
         (w (plist-get reply :w)) (h (plist-get reply :h)))
    (unwind-protect
        (pcase-let ((`(,gw ,gh ,gbytes) (glitnir-snapshot nil 'raw))
                    (rbytes (with-temp-buffer
                              (set-buffer-multibyte nil)
                              (insert-file-contents-literally path)
                              (buffer-string))))
          ;; compare over the frame's rectangle: the frame is at (0,0) of the recorder's output
          (should (<= gw w))
          (should (<= gh h))
          (let ((worst 0))
            (dotimes (y gh)
              (dotimes (x gw)
                (let ((gi (* 4 (+ x (* y gw)))) (ri (* 4 (+ x (* y w)))))
                  (dotimes (c 4)
                    (setq worst (max worst (abs (- (aref gbytes (+ gi c)) (aref rbytes (+ ri c))))))))))
            (should (= worst 0))))
      (delete-file path))))

;;; recorder-gui-tests.el ends here
