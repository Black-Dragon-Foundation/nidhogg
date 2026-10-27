;;; nhwl-gui-tests.el --- GUI rows for Nidhogg's own Wayland shell  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh `--compositor recorder' (the rows need $GLITNIR_GUI_RECORDER, the recorder's control
;; socket) in a build made with --with-nidhogg-wayland.  `gui-runner-recorder' lives in gui-runner.el.  The rows run
;; from a timer, so input sits unread until a row calls `read-event'.

(require 'ert)
(require 'json)
(require 'seq)

(defvar nhwl-gui--display-lost nil)

(defun nhwl-gui--on-p ()
  "Non-nil on the shell with the recorder up (`nhwl-gui/display-lost-daemon-survives' ends it)."
  (and (featurep 'nidhogg-wayland) (getenv "GLITNIR_GUI_RECORDER") (not nhwl-gui--display-lost)))

(defun nhwl-gui--get (c key)
  "KEY of commit C, nil for a JSON null or false (`json-parse-string' gives `:null' and `:false', both non-nil)."
  (let ((v (plist-get c key))) (unless (memq v '(:null :false)) v)))

(defun nhwl-gui--damage (c)
  "Commit C's damage rectangles (X Y W H), in buffer pixels: the shell sends `damage_buffer' only."
  (mapcar (lambda (d) (plist-get d :rect)) (plist-get c :damage)))

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
  ;; after the daemon's display is lost the selected frame is the daemon's own, with no window system
  (skip-unless (and (featurep 'nidhogg-wayland) (not nhwl-gui--display-lost)))
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

(ert-deftest nhwl-gui/kill-and-yank-without-selections ()
  "Selections are not in W0, yet killing and yanking work: the selection
primitives answer nil instead of signalling."
  (skip-unless (nhwl-gui--on-p))
  (with-temp-buffer
    (let ((select-enable-clipboard t))
      (insert "hello")
      (kill-region (point-min) (point-max))
      (should (equal (buffer-string) ""))
      (yank)
      (should (equal (buffer-string) "hello")))))

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

;; The daemon rows run under gui.sh `--daemon' (the runner is in the daemon).

(defun nhwl-gui--nhwl-frames ()
  (seq-filter (lambda (f) (and (frame-live-p f) (eq (framep f) 'pgtk))) (frame-list)))

(ert-deftest nhwl-gui/daemon-frame-from-client ()
  (skip-unless (and (nhwl-gui--on-p) (daemonp)))
  (let ((f (car (nhwl-gui--nhwl-frames))))
    (should f)
    (should (equal (terminal-name (frame-terminal f)) (getenv "WAYLAND_DISPLAY")))
    (should (nhwl-gui--wait #'nhwl-gui--toplevel-commits))))

(ert-deftest nhwl-gui/delete-and-reopen-frame ()
  (skip-unless (and (nhwl-gui--on-p) (daemonp)))
  (let* ((display (getenv "WAYLAND_DISPLAY"))
         (old (car (nhwl-gui--nhwl-frames)))
         (before (length (nhwl-gui--toplevel-commits)))
         (new (progn (make-frame-on-display display)
                     (delete-frame old t)
                     (car (nhwl-gui--nhwl-frames)))))
    (should (and new (not (eq new old))))
    (should (equal (terminal-name (frame-terminal new)) display))
    (should (nhwl-gui--wait (lambda () (> (length (nhwl-gui--toplevel-commits)) before))))))

(ert-deftest nhwl-gui/ping-answered-while-a-lisp-thread-sleeps ()
  (skip-unless (and (nhwl-gui--on-p) (daemonp) (not nhwl-gui--display-lost)))
  (let ((buf (generate-new-buffer " *nhwl-ping*")))
    (unwind-protect
        (progn
          (switch-to-buffer buf)
          (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
          (make-thread (lambda () (sleep-for 5)))
          (let ((r nil))
            (run-at-time 0.5 nil
                         (lambda ()
                           (setq r (gui-runner-recorder '(:cmd "ping")))
                           (gui-runner-recorder '(:cmd "key" :text "x"))))
            (sit-for 3)
            (should r)
            (should (< (plist-get r :pong_ms) 100))
            (should (nhwl-gui--wait
                     (lambda ()
                       (let ((ev (read-event nil nil 0.1)))
                         (when (characterp ev)
                           (setq last-command-event ev)
                           (command-execute (key-binding (vector ev)) nil (vector ev))))
                       (equal (buffer-string) "x"))))))
      (kill-buffer buf))))

(ert-deftest nhwl-gui/no-such-display-leaves-the-daemon-up ()
  (skip-unless (and (nhwl-gui--on-p) (daemonp) (not nhwl-gui--display-lost)))
  (let ((frames (nhwl-gui--nhwl-frames)))
    (should-error (make-frame-on-display "wayland-nonexistent"))
    (should (daemonp))
    (should (equal (nhwl-gui--nhwl-frames) frames))
    (should (frame-live-p (car frames)))
    (with-selected-frame (car frames) (redisplay t))))

;; ERT runs rows alphabetically and this row ends the recorder, so it first runs the two rows that need a live display; they
;; skip afterwards.
(ert-deftest nhwl-gui/display-lost-daemon-survives ()
  (skip-unless (and (nhwl-gui--on-p) (daemonp)))
  (dolist (name '(nhwl-gui/no-such-display-leaves-the-daemon-up
                  nhwl-gui/ping-answered-while-a-lisp-thread-sleeps))
    (let ((test (ert-get-test name)))
      (should (ert-test-result-expected-p test (ert-run-test test)))))
  (setq nhwl-gui--display-lost t)
  (should (nhwl-gui--nhwl-frames))
  (gui-runner-recorder '(:cmd "quit"))
  (should (nhwl-gui--wait (lambda () (null (nhwl-gui--nhwl-frames))) 2))
  (should (daemonp)))

;; Task 10: drawing

(defun nhwl-gui--mark ()
  "The recorder's next commit sequence number."
  (plist-get (gui-runner-recorder '(:cmd "log" :since 0)) :next))

(defun nhwl-gui--new (mark)
  "The toplevel's commits with a buffer since MARK."
  (seq-filter (lambda (c)
                (and (equal (plist-get c :role) "toplevel") (nhwl-gui--get c :buffer)))
              (plist-get (gui-runner-recorder (list :cmd "log" :since mark)) :commits)))

(defun nhwl-gui--rect-in-p (r y0 y1)
  "Non-nil when the damage rectangle R (X Y W H) lies within rows Y0 to Y1."
  (and (>= (nth 1 r) y0) (<= (+ (nth 1 r) (nth 3 r)) y1)))

(defun nhwl-gui--shown (mark)
  "Wait until the recorder commits something since MARK, let it settle, and return its screen grab."
  (nhwl-gui--wait (lambda () (nhwl-gui--new mark)) 5)
  (gui-runner-wait-settled)
  (gui-runner-screen-grab))

(defun nhwl-gui--recorder-twice (a b)
  "Send the recorder commands A and B in one write, so the shell sees both configures in one drain.
Return the two replies."
  (let* ((reply "")
         (proc (make-network-process
                :name "nh-recorder-twice" :family 'local :service (getenv "GLITNIR_GUI_RECORDER") :coding 'utf-8
                :filter (lambda (_p s) (setq reply (concat reply s))))))
    (unwind-protect
        (progn
          (process-send-string proc (concat (json-serialize a) "\n" (json-serialize b) "\n"))
          (let ((deadline (+ (float-time) 10)))
            (while (and (< (length (split-string reply "\n" t)) 2) (< (float-time) deadline))
              (accept-process-output proc 0.1)))
          (mapcar (lambda (l) (json-parse-string l :object-type 'plist :array-type 'list))
                  (split-string reply "\n" t)))
      (delete-process proc))))

(ert-deftest nhwl-gui/text-renders ()
  (skip-unless (nhwl-gui--on-p))
  (switch-to-buffer "*scratch*")
  (erase-buffer)
  (insert "The quick brown fox jumps over the lazy dog 0123456789\n")
  (insert "Nidhogg draws this with pgtk's Cairo code on its own backing.\n")
  (let ((mark (nhwl-gui--mark)))
    (redisplay t)
    ;; the frame's own picture first, then the recorder's
    (let* ((mine (glitnir-snapshot nil 'raw))
           (theirs (nhwl-gui--shown mark))
           (diff (gui-runner-pixel-diff mine theirs)))
      (should (= (car diff) 0))
      ;; something is drawn: the text area is not one flat colour
      (let ((bytes (nth 2 mine)) (first (substring (nth 2 mine) 0 4)) (found nil) (i 0))
        (while (and (not found) (< i (min (length bytes) 400000)))
          (unless (equal (substring bytes i (+ i 4)) first) (setq found t))
          (setq i (+ i 4)))
        (should found)))))

(ert-deftest nhwl-gui/scale-150-buffer-exact ()
  (skip-unless (nhwl-gui--on-p))
  (skip-unless (= (frame-scale-factor) 1.5))
  (let* ((size (getenv "GLITNIR_GUI_SIZE"))
         (w (string-to-number (car (split-string size "x"))))
         (h (string-to-number (cadr (split-string size "x")))))
    (gui-runner-recorder (list :cmd "configure" :surface "focused"
                               :size (vector (round (/ w 1.5)) (round (/ h 1.5)))
                               :states ["fullscreen" "activated"]))
    (nhwl-gui--wait
     (lambda ()
       (redisplay t)
       (let* ((c (car (last (nhwl-gui--toplevel-commits)))) (b (plist-get c :buffer)))
         (and c (= (plist-get b :w) w) (= (plist-get b :h) h))))
     5)
    (should (equal (list w h)
                   (let ((b (plist-get (car (last (nhwl-gui--toplevel-commits))) :buffer)))
                     (list (plist-get b :w) (plist-get b :h)))))
    (let ((c (car (last (nhwl-gui--toplevel-commits)))))
      (should (equal (plist-get (plist-get c :viewport) :dst) (list (round (/ w 1.5)) (round (/ h 1.5)))))
      (should (= (plist-get c :buffer_scale) 1)))
    ;; the font opened at the device size: the frame's character height is the font's own height
    (should (= (frame-char-height)
               (aref (font-info (face-font 'default) (selected-frame)) 3)))))

(ert-deftest nhwl-gui/typing-damages-rows-only ()
  (skip-unless (nhwl-gui--on-p))
  (switch-to-buffer "*scratch*")
  (erase-buffer)
  (insert "one\ntwo\n")
  ;; an echo-area message (the startup one, "Mark set") gets one cell redrawn at its end on the first change after it
  (message nil)
  (redisplay t)
  (gui-runner-wait-settled)
  (let* ((lh (frame-char-height))
         (edges (window-inside-pixel-edges))
         (row0 (+ (nth 1 edges) (* 2 lh)))
         (mode0 (nth 3 edges))
         (mode1 (nth 3 (window-pixel-edges)))
         (mark (nhwl-gui--mark)))
    (dotimes (i 10)
      (insert (string (+ ?a i)))
      (redisplay t)
      (sit-for 0.03))
    (gui-runner-wait-settled)
    (let ((commits (nhwl-gui--new mark)))
      (should commits)
      (dolist (c commits)
        (dolist (r (nhwl-gui--damage c))
          (should (or (nhwl-gui--rect-in-p r row0 (+ row0 lh))
                      (nhwl-gui--rect-in-p r mode0 mode1))))))))

(ert-deftest nhwl-gui/scroll-run-damages-exposed-rows ()
  (skip-unless (nhwl-gui--on-p))
  (switch-to-buffer "*scratch*")
  (erase-buffer)
  (dotimes (i 200) (insert (format "line %d of the scroll test\n" i)))
  (goto-char (point-min))
  (redisplay t)
  (gui-runner-wait-settled)
  (let* ((lh (frame-char-height))
         (edges (window-inside-pixel-edges))
         (mode0 (nth 3 edges))
         (mark (nhwl-gui--mark)))
    (scroll-up 3)
    (redisplay t)
    (let* ((theirs (nhwl-gui--shown mark))
           (mine (glitnir-snapshot nil 'raw))
           (rects (apply #'append (mapcar (lambda (c) (nhwl-gui--damage c)) (nhwl-gui--new mark)))))
      ;; the exposed rows and the mode line are damaged; the shm buffer cannot scroll, so the rows that moved are
      ;; damaged too (the plan's "less than half the frame" does not hold for a text-filled window)
      (let ((y (- mode0 (* 3 lh))))
        (while (< y (nth 3 (window-pixel-edges)))
          (should (seq-find (lambda (r) (and (<= (nth 1 r) y) (< y (+ (nth 1 r) (nth 3 r))))) rects))
          (setq y (1+ y))))
      (should (= (car (gui-runner-pixel-diff mine theirs)) 0)))))

(ert-deftest nhwl-gui/resize-during-redisplay-acks-newest ()
  (skip-unless (nhwl-gui--on-p))
  (let* ((s (frame-scale-factor))
         (mark (nhwl-gui--mark))
         (replies (nhwl-gui--recorder-twice
                   '(:cmd "configure" :surface "focused" :size [900 640] :states ["activated"])
                   '(:cmd "configure" :surface "focused" :size [1000 700] :states ["activated"])))
         (newest (plist-get (cadr replies) :serial)))
    (should (/= (plist-get (car replies) :serial) newest))
    (redisplay t)
    (should (nhwl-gui--wait
             (lambda ()
               (redisplay t)
               (seq-find (lambda (c)
                           (and (eql (nhwl-gui--get c :ack_serial) newest)
                                (= (plist-get (nhwl-gui--get c :buffer) :w) (round (* 1000 s)))
                                (= (plist-get (nhwl-gui--get c :buffer) :h) (round (* 700 s)))))
                         (nhwl-gui--new mark)))
             5))
    (dolist (c (nhwl-gui--new mark))
      (should-not (nhwl-gui--get c :ack_stale)))))

(ert-deftest nhwl-gui/suspended-skips-redisplay ()
  (skip-unless (nhwl-gui--on-p))
  ;; xdg_wm_base below 6 (the sway profile's 5) has no suspended state
  (skip-unless (>= (plist-get (plist-get (gui-runner-recorder '(:cmd "globals")) :globals) :xdg_wm_base) 6))
  (switch-to-buffer "*scratch*")
  (erase-buffer)
  (redisplay t)
  (gui-runner-wait-settled)
  (let* ((m0 (nhwl-gui--mark))
         (serial (plist-get (gui-runner-recorder
                             '(:cmd "configure" :surface "focused" :states ["suspended" "activated"]))
                            :serial)))
    ;; the configure's ack reaches the compositor even while suspended
    (should (nhwl-gui--wait
             (lambda ()
               (redisplay t)
               (seq-find (lambda (c) (eql (nhwl-gui--get c :ack_serial) serial)) (nhwl-gui--new m0)))
             5)))
  (gui-runner-wait-settled)
  (let ((mark (nhwl-gui--mark)))
    (dotimes (i 20)
      (insert (format "change %d\n" i))
      (redisplay t)
      (sit-for 0.02))
    (sit-for 0.3)
    (should-not (nhwl-gui--new mark))
    (gui-runner-recorder '(:cmd "configure" :surface "focused" :states ["activated"]))
    (should (nhwl-gui--wait (lambda () (redisplay t) (nhwl-gui--new mark)) 5))
    (sit-for 0.3)
    ;; one commit carries everything that accumulated; a cursor redraw may follow it, nothing else
    (let ((cs (nhwl-gui--new mark)))
      (should (> (length (nhwl-gui--damage (car cs))) 10))
      (dolist (c (cdr cs))
        (should (<= (length (nhwl-gui--damage c)) 3))))))

;;; nhwl-gui-tests.el ends here

;; Task 11: the bare client-side title bar (nhwlcsd.c).  Logical sizes: the strip is 32, the resize margins are 8.

(defun nhwl-gui--live-p ()
  (and (featurep 'nidhogg-wayland) (not nhwl-gui--display-lost)))

(defun nhwl-gui--decoration ()
  "The decoration mode the recorder sent for the newest toplevel (nil: none; nil off the recorder)."
  (when (getenv "GLITNIR_GUI_RECORDER")
    (nhwl-gui--get (car (last (nhwl-gui--toplevel-commits))) :decoration)))

(defun nhwl-gui--csd-p ()
  (and (nhwl-gui--live-p)
       (if (getenv "GLITNIR_GUI_RECORDER")
           (not (equal (nhwl-gui--decoration) "server"))
         (getenv "GLITNIR_GUI_CSD"))))

(defun nhwl-gui--windowed ()
  "Leave fullscreen; on the recorder to an 800 x 600 geometry."
  (set-frame-parameter nil 'fullscreen nil)
  (when (getenv "GLITNIR_GUI_RECORDER")
    (gui-runner-recorder '(:cmd "configure" :surface "focused" :size [800 600] :states ["activated"]))
    (nhwl-gui--wait (lambda () (redisplay t) (= (frame-pixel-width) (round (* 800 (frame-scale-factor))))) 5))
  (sit-for 0.3)
  (redisplay t)
  (gui-runner-wait-settled))

(defun nhwl-gui--last-commit ()
  (car (last (nhwl-gui--toplevel-commits))))

(defun nhwl-gui--pump ()
  "Let the shell's input events be read and the keyboard queue drained."
  (let ((deadline (+ (float-time) 0.3)))
    (while (< (float-time) deadline) (read-event nil nil 0.05))))

(defun nhwl-gui--point (x y)
  (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
  (gui-runner-recorder (list :cmd "pointer" :pos (vector x y)))
  (nhwl-gui--pump))

(defun nhwl-gui--click (x y)
  (nhwl-gui--point x y)
  (gui-runner-recorder '(:cmd "button" :button 272 :state "press"))
  (gui-runner-recorder '(:cmd "button" :button 272 :state "release"))
  (nhwl-gui--pump))

(defun nhwl-gui--input-since (mark req)
  (seq-find (lambda (e) (equal (plist-get e :req) req))
            (plist-get (gui-runner-recorder (list :cmd "input_log" :since mark)) :entries)))

(ert-deftest nhwl-gui/csd-strip-drawn ()
  (skip-unless (nhwl-gui--csd-p))
  (nhwl-gui--windowed)
  (let* ((s (frame-scale-factor))
         (m (round (* 8 s))) (strip (round (* 32 s)))
         (snap (glitnir-snapshot nil 'raw))
         (w (nth 0 snap)) (h (nth 1 snap)) (bytes (nth 2 snap))
         (bg (mapcar (lambda (v) (/ v 257)) (color-values (face-background 'default))))
         (px (lambda (x y) (let ((i (* 4 (+ (* y w) x))))
                             (list (aref bytes (+ i 2)) (aref bytes (+ i 1)) (aref bytes i) (aref bytes (+ i 3)))))))
    ;; the native area excludes the strip and the margins
    (should (= (frame-pixel-width) (- w (* 2 m))))
    (should (= (frame-pixel-height) (- h (* 2 m) strip)))
    ;; the strip's top rows over the native area are not the background
    (dotimes (i 3)
      (let ((p (funcall px (/ w 2) (+ m 2 (* i 4)))))
        (should (= (nth 3 p) 255))
        (should (seq-some #'identity (seq-mapn (lambda (a b) (> (abs (- a b)) 2)) (seq-take p 3) bg)))))
    ;; the margins are transparent
    (should (= (nth 3 (funcall px 1 (/ h 2))) 0))
    (when (getenv "GLITNIR_GUI_RECORDER")
      (should (equal (nhwl-gui--get (nhwl-gui--last-commit) :geometry)
                     (list 8 8 (- (round (/ w s)) 16) (- (round (/ h s)) 16)))))))

(ert-deftest nhwl-gui/csd-press-moves ()
  (skip-unless (and (getenv "GLITNIR_GUI_RECORDER") (nhwl-gui--csd-p)))
  (nhwl-gui--windowed)
  (let ((mark (plist-get (gui-runner-recorder '(:cmd "input_log" :since 0)) :next)))
    (nhwl-gui--point 300 24)
    (gui-runner-recorder '(:cmd "button" :button 272 :state "press"))
    (nhwl-gui--pump)
    (gui-runner-recorder '(:cmd "button" :button 272 :state "release"))
    (let ((e (nhwl-gui--wait (lambda () (nhwl-gui--pump) (nhwl-gui--input-since mark "xdg_toplevel.move")))))
      (should e)
      (should (integerp (plist-get e :serial))))))

(ert-deftest nhwl-gui/csd-edge-resizes ()
  (skip-unless (and (getenv "GLITNIR_GUI_RECORDER") (nhwl-gui--csd-p)))
  (nhwl-gui--windowed)
  (let* ((dst (plist-get (plist-get (nhwl-gui--last-commit) :viewport) :dst))
         (mark (plist-get (gui-runner-recorder '(:cmd "input_log" :since 0)) :next)))
    (nhwl-gui--point (- (car dst) 2) (/ (cadr dst) 2))
    (gui-runner-recorder '(:cmd "button" :button 272 :state "press"))
    (let ((e (nhwl-gui--wait (lambda () (nhwl-gui--pump) (nhwl-gui--input-since mark "xdg_toplevel.resize")))))
      (should e)
      (should (equal (plist-get e :edges) "right")))
    (gui-runner-recorder '(:cmd "button" :button 272 :state "release"))))

(ert-deftest nhwl-gui/csd-close-deletes-frame ()
  (skip-unless (and (getenv "GLITNIR_GUI_RECORDER") (nhwl-gui--csd-p)))
  (nhwl-gui--windowed)
  (let* ((old (selected-frame)) (new nil) (mark (nhwl-gui--mark))
         (old-surface (plist-get (nhwl-gui--last-commit) :surface))
         (fresh (lambda ()
                  (seq-filter (lambda (c) (not (eql (plist-get c :surface) old-surface))) (nhwl-gui--new mark)))))
    (unwind-protect
        (progn
          (setq new (make-frame))
          (should (= (length (frame-list)) 2))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump) (redisplay t) (funcall fresh)) 5))
          (gui-runner-wait-settled)
          (let* ((dst (plist-get (plist-get (car (last (funcall fresh))) :viewport) :dst)))
            ;; the close button is the rightmost 32 px of the strip, inside the 8 px margin
            (nhwl-gui--click (- (car dst) 8 16) (+ 8 16))
            (should (nhwl-gui--wait (lambda () (nhwl-gui--pump) (= (length (frame-list)) 1))))
            (should (frame-live-p old))))
      (when (and new (frame-live-p new)) (delete-frame new t)))))

(ert-deftest nhwl-gui/ssd-no-strip ()
  (skip-unless (and (nhwl-gui--on-p) (equal (nhwl-gui--decoration) "server")))
  (nhwl-gui--windowed)
  (let* ((s (frame-scale-factor))
         (c (nhwl-gui--last-commit))
         (b (plist-get c :buffer))
         (geom (nhwl-gui--get c :geometry)))
    ;; the native size is the whole surface, and no geometry narrows it
    (should (= (frame-pixel-width) (plist-get b :w)))
    (should (= (frame-pixel-height) (plist-get b :h)))
    (should (or (null geom)
                (equal geom (list 0 0 (round (/ (plist-get b :w) s)) (round (/ (plist-get b :h) s))))))))
