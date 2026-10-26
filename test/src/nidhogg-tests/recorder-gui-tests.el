;;; recorder-gui-tests.el --- GUI tests against nh-recorder  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh `--compositor recorder' (Task 4 teaches gui.sh), where $GLITNIR_GUI_RECORDER is the
;; recorder's control socket.  `gui-runner-recorder' lives in gui-runner.el.  The rows skip without it.

(require 'ert)
(require 'json)

(defun recorder-gui--commits ()
  (plist-get (gui-runner-recorder '(:cmd "log" :since 0)) :commits))

(defun recorder-gui--get (c key)
  "KEY of commit C, nil for a JSON null (`json-parse-string' gives `:null', which is non-nil)."
  (let ((v (plist-get c key))) (unless (eq v :null) v)))

(defun recorder-gui--geometry (c)
  "The window geometry (X Y W H) of toplevel commit C, logical: the logged one, else the whole buffer."
  (or (recorder-gui--get c :geometry)
      (let ((b (recorder-gui--get c :buffer)) (s (plist-get c :buffer_scale)))
        (list 0 0 (/ (plist-get b :w) s) (/ (plist-get b :h) s)))))

(defun recorder-gui--frame-toplevel ()
  "The newest commit of the frame's toplevel that carries a buffer."
  (car (last (seq-filter (lambda (c)
                           (and (equal (plist-get c :role) "toplevel")
                                (equal (recorder-gui--get c :app_id) "nidhogg")
                                (recorder-gui--get c :buffer)))
                         (recorder-gui--commits)))))

(defun recorder-gui--frame-origin ()
  "Where the frame's drawing area sits in its toplevel surface: (X . Y), surface-local logical pixels.
GTK puts its widgets (the menu bar and tool bar, and a client-side title bar) above the drawing area and nothing
beside or below it, so the area is the bottom of the window geometry, as wide as it."
  (pcase-let ((`(,gx ,gy ,_gw ,gh) (recorder-gui--geometry (recorder-gui--frame-toplevel))))
    (cons gx (- (+ gy gh) (frame-pixel-height)))))

(ert-deftest recorder-gui/pgtk-frame-maps-as-toplevel ()
  "A toplevel commit of app_id \"nidhogg\" whose window geometry holds the frame (as wide as the frame, at least as
tall: GTK's menu bar and tool bar sit above the drawing area) and whose buffer holds that geometry at the scale."
  (skip-unless (getenv "GLITNIR_GUI_RECORDER"))
  (let* ((scale (frame-scale-factor))
         (hit (seq-find (lambda (c)
                          (and (equal (plist-get c :role) "toplevel")
                               (equal (recorder-gui--get c :app_id) "nidhogg")
                               (recorder-gui--get c :buffer)
                               (pcase-let ((`(,gx ,gy ,gw ,gh) (recorder-gui--geometry c))
                                           (b (plist-get c :buffer)))
                                 (and (= gw (frame-pixel-width))
                                      (>= gh (frame-pixel-height))
                                      (>= (plist-get b :w) (* scale (+ gx gw)))
                                      (>= (plist-get b :h) (* scale (+ gy gh)))))))
                        (recorder-gui--commits))))
    (should hit)))

(defun recorder-gui--configure-and-ack (size states)
  "Send the frame's toplevel a configure of SIZE (a vector [W H]) and STATES; wait (2 s at most) for the commit that
acks it and return that commit (nil when none came)."
  (let* ((seq-before (plist-get (gui-runner-recorder '(:cmd "log" :since 0)) :next))
         (serial (plist-get (gui-runner-recorder
                             (list :cmd "configure" :surface "focused" :size size :states states))
                            :serial))
         (acked nil)
         (deadline (+ (float-time) 2)))
    (while (and (not acked) (< (float-time) deadline))
      (accept-process-output nil 0.05)
      (setq acked (seq-find (lambda (c)
                              (and (>= (plist-get c :seq) seq-before)
                                   (eql (plist-get c :ack_serial) serial)))
                            (recorder-gui--commits))))
    acked))

(ert-deftest recorder-gui/pgtk-acks-maximize ()
  (skip-unless (getenv "GLITNIR_GUI_RECORDER"))
  (pcase-let ((`(,_x ,_y ,w ,h) (recorder-gui--geometry (recorder-gui--frame-toplevel))))
    (unwind-protect
        (let ((acked (recorder-gui--configure-and-ack [800 600] ["maximized" "activated"])))
          (should acked)
          (should-not (eq (plist-get acked :ack_stale) t))
          (should (eq (frame-parameter nil 'fullscreen) 'maximized)))
      ;; the rows after this one run on the fullscreen frame
      (recorder-gui--configure-and-ack (vector w h) ["fullscreen" "activated"]))))

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
        ;; the rows run from a timer, outside the command loop: the key waits in the input queue until read, and
        ;; `last-input-event' is the last event read
        (should (recorder-gui--wait (lambda () (read-event nil nil 0.1) (eq last-input-event ?\C-a)))))
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
          ;; over the middle of the selected window's body (GTK's tool bar takes the surface's top)
          (pcase-let ((`(,ox . ,oy) (recorder-gui--frame-origin))
                      (`(,l ,top ,r ,bottom) (window-body-pixel-edges)))
            (gui-runner-recorder (list :cmd "pointer" :pos (vector (+ ox (/ (+ l r) 2)) (+ oy (/ (+ top bottom) 2))))))
          (let ((start (window-start)))
            (gui-runner-recorder '(:cmd "axis" :source "wheel" :v120 120 :stop :false))
            ;; outside the command loop (the rows run from a timer) the row reads the wheel event and runs its binding
            (should (recorder-gui--wait
                     (lambda ()
                       (let ((ev (read-event nil nil 0.1)))
                         (when (and (consp ev) (memq (event-basic-type ev) '(wheel-down wheel-up)))
                           (command-execute (key-binding (vector ev) t nil (event-start ev)) nil (vector ev))))
                       (> (window-start) start))))))
      (kill-buffer buf))))

(ert-deftest recorder-gui/snapshot-equals-glitnir-snapshot ()
  (skip-unless (getenv "GLITNIR_GUI_RECORDER"))
  (skip-unless (= (frame-scale-factor) 1))
  (let ((blink blink-cursor-mode)
        (path (make-temp-file "recorder-snapshot" nil ".bgra")))
    (unwind-protect
        (progn
          ;; a blink between the two snapshots would commit another picture
          (blink-cursor-mode -1)
          (redisplay t)
          ;; Glitnir presents what redisplay drew only at its next paint: `glitnir-snapshot' makes that paint and
          ;; returns its picture, and the recorder's snapshot follows once the paint's commit has arrived
          (pcase-let* ((since (plist-get (gui-runner-recorder '(:cmd "log" :since 0)) :next))
                       (`(,gw ,gh ,gbytes) (glitnir-snapshot nil 'raw))
                       (_arrived (should (recorder-gui--wait
                                          (lambda ()
                                            (seq-find (lambda (c) (equal (plist-get c :role) "toplevel"))
                                                      (plist-get (gui-runner-recorder (list :cmd "log" :since since))
                                                                 :commits))))))
                       (reply (gui-runner-recorder (list :cmd "snapshot" :path path :format "bgra")))
                       (w (plist-get reply :w)) (h (plist-get reply :h))
                       (rbytes (with-temp-buffer
                                 (set-buffer-multibyte nil)
                                 (insert-file-contents-literally path)
                                 (buffer-string)))
                       ;; the toplevel is at (0,0) of the recorder's output, the frame's drawing area at this origin
                       ;; in the surface (scale 1: logical pixels are device pixels)
                       (`(,ox . ,oy) (recorder-gui--frame-origin)))
            ;; compare over the frame's rectangle
            (should (<= (+ ox gw) w))
            (should (<= (+ oy gh) h))
            (let ((worst 0) (pixels 0))
              (dotimes (y gh)
                (dotimes (x gw)
                  (let ((gi (* 4 (+ x (* y gw)))) (ri (* 4 (+ ox x (* (+ oy y) w)))) (d 0))
                    (dotimes (c 4)
                      (setq d (max d (abs (- (aref gbytes (+ gi c)) (aref rbytes (+ ri c)))))))
                    (when (> d 0) (setq pixels (1+ pixels) worst (max worst d))))))
              (should (equal (list :worst worst :pixels pixels) '(:worst 0 :pixels 0))))))
      (when blink (blink-cursor-mode 1))
      (delete-file path))))

;;; recorder-gui-tests.el ends here
