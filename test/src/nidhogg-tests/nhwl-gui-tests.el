;;; nhwl-gui-tests.el --- GUI rows for Nidhogg's own Wayland shell  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh in a build made with --with-nidhogg-wayland.  The rows that drive or read the compositor
;; need `--compositor recorder' ($GLITNIR_GUI_RECORDER, the recorder's control socket); the others run on any
;; compositor.  `gui-runner-recorder' lives in gui-runner.el.  The rows run from a timer, so input sits unread until a
;; row calls `read-event'.  Rows tagged `runs-last' run after every other row (gui-runner.el).

(require 'ert)
(require 'json)
(require 'seq)

(defun nhwl-gui--live-p ()
  "Non-nil on Nidhogg's own Wayland shell."
  (featurep 'nidhogg-wayland))

(defun nhwl-gui--recorder-p ()
  "Non-nil on the shell with the recorder's control channel."
  (and (nhwl-gui--live-p) (getenv "GLITNIR_GUI_RECORDER")))

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
  (skip-unless (nhwl-gui--live-p))
  (should (eq (window-system) 'pgtk))
  (should (featurep 'pgtk))
  ;; the shell takes the compositor's scale exactly
  (should (< (abs (- (frame-scale-factor) (glitnir-gui--scale))) 0.001)))

(ert-deftest nhwl-gui/frame-maps-on-recorder ()
  (skip-unless (nhwl-gui--recorder-p))
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
  (skip-unless (nhwl-gui--recorder-p))
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

(defun nhwl-gui--recorder-read (kind type)
  "What the recorder reads from the selection KIND (\"clipboard\" or \"primary\") as TYPE, retrying for up to 3 s."
  (let ((data nil))
    (nhwl-gui--wait
     (lambda ()
       (setq data (condition-case nil
                      (plist-get (gui-runner-recorder (list :cmd "read" :kind kind :type type)) :data)
                    (error nil)))))
    data))

(defun nhwl-gui--focus-toplevel ()
  "Give the toplevel keyboard focus (a selection needs it to be owned)."
  (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
  (sit-for 0.3))

(ert-deftest nhwl-gui/kill-reaches-another-client ()
  :tags '(selection)
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--focus-toplevel)
  (let ((select-enable-clipboard t) (kill-ring nil) (kill-ring-yank-pointer nil))
    (kill-new "abc")
    (should (equal (nhwl-gui--recorder-read "clipboard" "text/plain;charset=utf-8") "abc"))
    (should (gui-backend-selection-owner-p 'CLIPBOARD))))

(ert-deftest nhwl-gui/kill-reaches-wl-paste ()
  "On a real compositor another client (wl-paste) reads what was killed."
  :tags '(selection)
  (skip-unless (and (nhwl-gui--live-p) (not (nhwl-gui--recorder-p))))
  (skip-unless (executable-find "wl-paste"))
  (let ((select-enable-clipboard t) (kill-ring nil) (kill-ring-yank-pointer nil) (got nil))
    (select-frame-set-input-focus (selected-frame))
    (sit-for 0.5)
    (kill-new "from emacs")
    (should (nhwl-gui--wait
             (lambda ()
               (setq got (with-temp-buffer
                           (call-process "wl-paste" nil t nil "--no-newline" "--type" "text/plain")
                           (buffer-string)))
               (equal got "from emacs"))))))

(ert-deftest nhwl-gui/losing-the-clipboard-runs-the-hook ()
  "Another client taking the clipboard from us runs `pgtk-lost-selection-functions' once and Emacs goes on."
  :tags '(selection)
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--focus-toplevel)
  (let ((lost nil)
        (pgtk-lost-selection-functions nil)
        (select-enable-clipboard t) (kill-ring nil) (kill-ring-yank-pointer nil))
    (add-hook 'pgtk-lost-selection-functions (lambda (sel) (push sel lost)))
    (kill-new "abc")
    (should (nhwl-gui--recorder-read "clipboard" "text/plain;charset=utf-8"))
    (should (pgtk-selection-owner-p 'CLIPBOARD))
    (gui-runner-recorder '(:cmd "offer" :kind "clipboard"
                           :types (:text/plain\;charset=utf-8 "theirs")))
    ;; the clear event sits in the input queue until Emacs reads input
    (should (nhwl-gui--wait (lambda () (read-event nil nil 0.05) lost)))
    (sit-for 0.3)
    (should (equal lost '(CLIPBOARD)))
    (should-not (pgtk-selection-owner-p 'CLIPBOARD))
    (redisplay t)
    (with-temp-buffer
      (yank)
      (should (equal (buffer-string) "theirs")))))

(ert-deftest nhwl-gui/yank-from-another-client ()
  :tags '(selection)
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--focus-toplevel)
  (gui-runner-recorder '(:cmd "offer" :kind "clipboard"
                         :types (:text/plain\;charset=utf-8 "from elsewhere")))
  (should (nhwl-gui--wait (lambda () (equal (gui-get-selection 'CLIPBOARD 'UTF8_STRING) "from elsewhere"))))
  (should-not (gui-backend-selection-owner-p 'CLIPBOARD))
  (with-temp-buffer
    (let ((select-enable-clipboard t) (kill-ring nil) (kill-ring-yank-pointer nil))
      (yank)
      (should (equal (buffer-string) "from elsewhere")))))

(ert-deftest nhwl-gui/primary-from-region ()
  :tags '(selection)
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--focus-toplevel)
  (with-temp-buffer
    (let ((select-enable-primary t) (select-active-regions t) (transient-mark-mode t)
          (saved-region-selection nil) (deactivate-mark nil))
      (insert "region text")
      (push-mark (point-min) t t)
      (should (region-active-p))
      (deactivate-mark)
      (should (equal (nhwl-gui--recorder-read "primary" "text/plain;charset=utf-8") "region text")))))

(ert-deftest nhwl-gui/selection-read-from-a-silent-source-times-out ()
  :tags '(selection)
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--focus-toplevel)
  (gui-runner-recorder '(:cmd "offer" :kind "clipboard" :silent t
                         :types (:text/plain\;charset=utf-8 "never sent")))
  (sit-for 0.3)
  (let ((t0 (float-time)))
    (should-not (gui-get-selection 'CLIPBOARD 'UTF8_STRING))
    (should (< (- (float-time) t0) 1.5)))
  ;; the next read works
  (gui-runner-recorder '(:cmd "offer" :kind "clipboard"
                         :types (:text/plain\;charset=utf-8 "after the silence")))
  (should (nhwl-gui--wait (lambda () (equal (gui-get-selection 'CLIPBOARD 'UTF8_STRING) "after the silence"))))
  (with-temp-buffer
    (let ((select-enable-clipboard t) (kill-ring nil) (kill-ring-yank-pointer nil))
      (yank)
      (should (equal (buffer-string) "after the silence")))))

(ert-deftest nhwl-gui/targets-lists-offer-types ()
  :tags '(selection)
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--focus-toplevel)
  (gui-runner-recorder '(:cmd "offer" :kind "clipboard"
                         :types (:text/plain\;charset=utf-8 "x" :text/html "<b>x</b>")))
  (let ((targets (nhwl-gui--wait
                  (lambda ()
                    (let ((v (gui-get-selection 'CLIPBOARD 'TARGETS)))
                      (and (memq 'text/html (append v nil)) v))))))
    (should targets)
    (should (memq 'text/plain\;charset=utf-8 (append targets nil)))))

(ert-deftest nhwl-gui/keymap-option-gives-control ()
  (skip-unless (nhwl-gui--recorder-p))
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
  (skip-unless (nhwl-gui--recorder-p))
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
  (skip-unless (and (nhwl-gui--recorder-p) (daemonp)))
  (let ((f (car (nhwl-gui--nhwl-frames))))
    (should f)
    (should (equal (terminal-name (frame-terminal f)) (getenv "WAYLAND_DISPLAY")))
    (should (nhwl-gui--wait #'nhwl-gui--toplevel-commits))))

(ert-deftest nhwl-gui/delete-and-reopen-frame ()
  (skip-unless (and (nhwl-gui--recorder-p) (daemonp)))
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
  (skip-unless (and (nhwl-gui--recorder-p) (daemonp)))
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
  (skip-unless (and (nhwl-gui--live-p) (daemonp)))
  (let ((frames (nhwl-gui--nhwl-frames)))
    (should-error (make-frame-on-display "wayland-nonexistent"))
    (should (daemonp))
    (should (equal (nhwl-gui--nhwl-frames) frames))
    (should (frame-live-p (car frames)))
    (with-selected-frame (car frames) (redisplay t))))

(defun nhwl-gui--second-recorder ()
  "Start a second recorder on the display \"nhwl-second\"; return (PROCESS . CONTROL-SOCKET) once it is up."
  (let* ((ctl (expand-file-name "nhwl-second.ctl" (getenv "XDG_RUNTIME_DIR")))
         (out "")
         (proc (make-process :name "nh-recorder-second" :noquery t :connection-type 'pipe
                             :command (list (getenv "GLITNIR_GUI_RECORDER_BIN") "--output" "1600x1000@120"
                                            "--profile" (getenv "GLITNIR_GUI_RECORDER_PROFILE")
                                            "--socket" "nhwl-second" "--control" ctl)
                             :filter (lambda (_p s) (setq out (concat out s))))))
    (nhwl-gui--wait (lambda () (string-match-p "^READY" out)) 5)
    (cons proc ctl)))

;; The row ends the session's recorder, so it runs last (tag `runs-last').
(ert-deftest nhwl-gui/display-lost-daemon-survives ()
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--recorder-p) (daemonp)))
  (should (nhwl-gui--nhwl-frames))
  (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
  (nhwl-gui--pump)
  ;; a key and the loss in one read: the drain that stores the key also finds the display gone
  (nhwl-gui--recorder-twice '(:cmd "key" :keycode 30 :state "press") '(:cmd "quit"))
  (should (nhwl-gui--wait (lambda () (nhwl-gui--pump) (null (nhwl-gui--nhwl-frames))) 3))
  (should (daemonp))
  ;; the daemon still opens frames, on another display
  (when (getenv "GLITNIR_GUI_RECORDER_BIN")
    (let* ((second (nhwl-gui--second-recorder)) (f nil))
      (unwind-protect
          (progn
            (should (process-live-p (car second)))
            (setq f (make-frame-on-display "nhwl-second"))
            (should (frame-live-p f))
            (should (equal (terminal-name (frame-terminal f)) "nhwl-second"))
            (let ((process-environment (cons (concat "GLITNIR_GUI_RECORDER=" (cdr second)) process-environment)))
              (should (nhwl-gui--wait (lambda () (with-selected-frame f (redisplay t)) (nhwl-gui--toplevel-commits))
                                      5))))
        (when (frame-live-p f) (delete-frame f t))
        (delete-process (car second))))))

;; Drawing

(defun nhwl-gui--mark ()
  "The recorder's next commit sequence number."
  (plist-get (gui-runner-recorder '(:cmd "log" :since 0)) :next))

(defun nhwl-gui--new (mark)
  "The toplevel's commits with a buffer since MARK."
  (seq-filter (lambda (c)
                (and (equal (plist-get c :role) "toplevel") (nhwl-gui--get c :buffer)))
              (plist-get (gui-runner-recorder (list :cmd "log" :since mark)) :commits)))

(defun nhwl-gui--native-top ()
  "Where the native area starts in the toplevel's buffer, in buffer pixel rows: the client-side title bar and the
margins sit around it (margins M on every side, the strip on top), 0 without them."
  (let* ((b (plist-get (nhwl-gui--last-commit) :buffer))
         (m (/ (- (plist-get b :w) (frame-pixel-width)) 2)))
    (- (plist-get b :h) (frame-pixel-height) m)))

(defun nhwl-gui--logical-size (c)
  "The window size commit C shows the compositor, (W H) in logical pixels: its window geometry when it set one
\(the client-side title bar's margins lie outside it), else its buffer's size."
  (let ((g (nhwl-gui--get c :geometry)) (b (plist-get c :buffer)) (s (frame-scale-factor)))
    (if g
        (list (nth 2 g) (nth 3 g))
      (list (round (/ (plist-get b :w) s)) (round (/ (plist-get b :h) s))))))

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
  ;; recorder only: on headless sway grim's grab and the frame's snapshot differ (channel 255, not yet looked into)
  (skip-unless (nhwl-gui--recorder-p))
  (switch-to-buffer "*scratch*")
  (erase-buffer)
  (insert "The quick brown fox jumps over the lazy dog 0123456789\n")
  (insert "Nidhogg draws this with pgtk's Cairo code on its own backing.\n")
  (let ((mark (nhwl-gui--mark)))
    (redisplay t)
    ;; the frame's own picture first, then the compositor's
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
  (skip-unless (nhwl-gui--recorder-p))
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
  (skip-unless (nhwl-gui--recorder-p))
  (switch-to-buffer "*scratch*")
  (erase-buffer)
  (insert "one\ntwo\n")
  ;; an echo-area message (the startup one, "Mark set") gets one cell redrawn at its end on the first change after it
  (message nil)
  (redisplay t)
  (gui-runner-wait-settled)
  ;; damage is in buffer pixels; the edges are native ones
  (let* ((lh (frame-char-height))
         (top (nhwl-gui--native-top))
         (edges (window-inside-pixel-edges))
         (row0 (+ top (nth 1 edges) (* 2 lh)))
         (mode0 (+ top (nth 3 edges)))
         (mode1 (+ top (nth 3 (window-pixel-edges))))
         (mark (nhwl-gui--mark)))
    (dotimes (i 10)
      (insert (string (+ ?a i)))
      (redisplay t)
      (sit-for 0.03))
    (gui-runner-wait-settled)
    (let* ((all (nhwl-gui--new mark))
           ;; a buffer never written (age 0) goes out whole: at most one such commit per pool buffer (3)
           (fresh (seq-filter (lambda (c)
                                (let ((b (nhwl-gui--get c :buffer)))
                                  (equal (nhwl-gui--damage c)
                                         (list (list 0 0 (plist-get b :w) (plist-get b :h))))))
                              all))
           (commits (seq-difference all fresh)))
      (should all)
      (should (<= (length fresh) 3))
      ;; no rectangle outside the typed row and the mode line (a failure shows the rows, then the rectangles)
      (let ((rows (list row0 (+ row0 lh) mode0 mode1)))
        (should (equal (cons rows (seq-remove (lambda (r) (or (nhwl-gui--rect-in-p r row0 (+ row0 lh))
                                                              (nhwl-gui--rect-in-p r mode0 mode1)))
                                              (mapcan #'nhwl-gui--damage commits)))
                       (list rows)))))))

(ert-deftest nhwl-gui/scroll-run-damages-exposed-rows ()
  (skip-unless (nhwl-gui--recorder-p))
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
      ;; damaged too (a scroll of a text-filled window damages more than half the frame)
      (let ((y (- mode0 (* 3 lh))))
        (while (< y (nth 3 (window-pixel-edges)))
          (should (seq-find (lambda (r) (and (<= (nth 1 r) y) (< y (+ (nth 1 r) (nth 3 r))))) rects))
          (setq y (1+ y))))
      (should (= (car (gui-runner-pixel-diff mine theirs)) 0)))))

(defun nhwl-gui--scroll-events (record &optional regexp)
  "Send the recorder's axis RECORD(s) (one plist or a list of them); return the events whose symbol matches REGEXP."
  (nhwl-gui--point 100 50)
  (dolist (r (if (keywordp (car record)) (list record) record))
    (gui-runner-recorder r))
  (let ((deadline (+ (float-time) 0.6)) (events nil))
    (while (< (float-time) deadline)
      (let ((e (read-event nil nil 0.05)))
        (when (and (consp e) (string-match-p (or regexp "wheel-\\|touch-end") (symbol-name (car e))))
          (push e events))))
    (nreverse events)))

(ert-deftest nhwl-gui/wheel-v120-halves-accumulate ()
  (skip-unless (nhwl-gui--recorder-p))
  (switch-to-buffer "*scratch*")
  (let ((mwheel-coalesce-scroll-events t))
    (let ((events (nhwl-gui--scroll-events
                   '((:cmd "axis" :source "wheel" :v120 60) (:cmd "axis" :source "wheel" :v120 60)))))
      (should (= (length events) 1))
      (should (string-match-p "wheel-down" (symbol-name (car (car events)))))
      (should (eql (nth 3 (car events)) 1)))
    ;; a change of direction drops the fraction: -60 then +60 is nothing
    (should-not (nhwl-gui--scroll-events
                 '((:cmd "axis" :source "wheel" :v120 -60) (:cmd "axis" :source "wheel" :v120 60))))
    (should (= (length (nhwl-gui--scroll-events '((:cmd "axis" :source "wheel" :v120 -120)))) 1))))

(ert-deftest nhwl-gui/finger-scroll-gives-pixel-deltas ()
  (skip-unless (nhwl-gui--recorder-p))
  (switch-to-buffer "*scratch*")
  (erase-buffer)
  (dotimes (i 200) (insert (format "line %d of the finger test\n" i)))
  (goto-char (point-min))
  (redisplay t)
  (let ((pixel-scroll-precision-mode nil) (mwheel-coalesce-scroll-events nil))
    (pixel-scroll-precision-mode 1)
    (unwind-protect
        (progn
          (set-window-vscroll nil 0 t)
          ;; 80/8 = 10 logical px
          (let* ((events (nhwl-gui--scroll-events '(:cmd "axis" :source "finger" :v120 80) "wheel-down"))
                 (ev (car events))
                 (want (* 10 (frame-scale-factor))))
            (should (= (length events) 1))
            (should (= (cdr (nth 4 ev)) (- want)))
            (pixel-scroll-precision ev)
            (should (< want (frame-char-height)))
            (should (= (window-vscroll nil t) want))))
      (pixel-scroll-precision-mode -1)
      (set-window-vscroll nil 0 t))))

(ert-deftest nhwl-gui/scroll-stop-sends-touch-end ()
  (skip-unless (nhwl-gui--recorder-p))
  (switch-to-buffer "*scratch*")
  (let ((events (nhwl-gui--scroll-events '(:cmd "axis" :source "finger" :stop t) "touch-end")))
    (should (= (length events) 1))
    (should (eq (car (car events)) 'touch-end))))

(ert-deftest nhwl-gui/resize-during-redisplay-acks-newest ()
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((mark (nhwl-gui--mark))
         (replies (nhwl-gui--recorder-twice
                   '(:cmd "configure" :surface "focused" :size [900 640] :states ["activated"])
                   '(:cmd "configure" :surface "focused" :size [1000 700] :states ["activated"])))
         (newest (plist-get (cadr replies) :serial))
         (acked (lambda ()
                  (seq-filter (lambda (c) (eql (nhwl-gui--get c :ack_serial) newest)) (nhwl-gui--new mark)))))
    (should (/= (plist-get (car replies) :serial) newest))
    (redisplay t)
    (should (nhwl-gui--wait (lambda () (redisplay t) (funcall acked)) 5))
    ;; in logical pixels: with the client-side title bar the buffer is larger than the window
    (should (equal (seq-uniq (mapcar #'nhwl-gui--logical-size (funcall acked))) '((1000 700))))
    (dolist (c (nhwl-gui--new mark))
      (should-not (nhwl-gui--get c :ack_stale)))))

(ert-deftest nhwl-gui/suspended-skips-redisplay ()
  (skip-unless (nhwl-gui--recorder-p))
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

(ert-deftest nhwl-gui/configure-with-frame-done-acks-new-size ()
  "A configure and a frame callback read in one drain: the ack goes out with a buffer of the configure's size."
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--windowed)
  (switch-to-buffer "*scratch*")
  (let ((mark (nhwl-gui--mark)) serial)
    ;; a commit whose frame is still to come, then the configure (the recorder's frames tick every 16.7 ms)
    (insert "x")
    (redisplay t)
    (setq serial (plist-get (gui-runner-recorder
                             '(:cmd "configure" :surface "focused" :size [700 500] :states ["activated"]))
                            :serial))
    ;; one read takes the configure and the FRAME_DONE, with no redisplay before it
    (sleep-for 0.1)
    (input-pending-p)
    (should (nhwl-gui--wait
             (lambda ()
               (redisplay t)
               (seq-find (lambda (c) (eql (nhwl-gui--get c :ack_serial) serial)) (nhwl-gui--new mark)))
             5))
    (should (equal (seq-uniq (mapcar #'nhwl-gui--logical-size
                                     (seq-filter (lambda (c) (eql (nhwl-gui--get c :ack_serial) serial))
                                                 (nhwl-gui--new mark))))
                   '((700 500))))))

(ert-deftest nhwl-gui/huge-size-clamped ()
  "A size beyond what the backing can hold is clamped; the frame stays usable."
  ;; recorder only: on headless sway the frame paints nothing for the next row after this one (not yet looked into)
  (skip-unless (nhwl-gui--recorder-p))
  (switch-to-buffer "*scratch*")
  (set-frame-parameter nil 'fullscreen nil)
  (unwind-protect
      (progn
        (set-frame-size nil 40000 600 t)
        (redisplay t)
        (sit-for 0.3)
        (insert "after the huge size\n")
        (redisplay t)
        (should (frame-live-p (selected-frame)))
        (should (<= (frame-pixel-width) 16384))
        (gui-runner-wait-settled)
        (should (<= (plist-get (plist-get (nhwl-gui--last-commit) :buffer) :w) 16384)))
    ;; fullscreen again, as the runner started it (a 16384-pixel frame would slow the later rows' screen grabs)
    (let* ((s (frame-scale-factor))
           (size (mapcar #'string-to-number (split-string (getenv "GLITNIR_GUI_SIZE") "x"))))
      (set-frame-parameter nil 'fullscreen 'fullboth)
      (gui-runner-recorder (list :cmd "configure" :surface "focused"
                                 :size (vector (round (/ (car size) s)) (round (/ (cadr size) s)))
                                 :states ["fullscreen" "activated"]))
      (nhwl-gui--wait (lambda () (redisplay t) (= (frame-pixel-width) (car size))) 5))))

(ert-deftest nhwl-gui/held-key-one-repeat-while-busy ()
  "A key held while Lisp is busy (the input unread) queues at most one repeat."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((buf (generate-new-buffer " *nhwl-repeat*")) (n 0))
    (unwind-protect
        (progn
          (switch-to-buffer buf)
          (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
          (nhwl-gui--pump)
          ;; KEY_A held for 5 s (repeat after 600 ms at 25 Hz) while this row sleeps: the input poll runs meanwhile
          (gui-runner-recorder '(:cmd "key" :keycode 30 :state "press"))
          (sleep-for 5)
          (gui-runner-recorder '(:cmd "key" :keycode 30 :state "release"))
          (let ((deadline (+ (float-time) 1)))
            (while (< (float-time) deadline)
              (when (eq (read-event nil nil 0.1) ?a) (setq n (1+ n)))))
          ;; the press and at most one repeat
          (should (<= 1 n 2)))
      (kill-buffer buf))))

;; Task 10: child frames (nhwlchild.c).  A child frame is a subsurface of its parent's window; the recorder's `tree'
;; shows each surface's role, parent, position (logical), input region and buffer.

(defun nhwl-gui--tree ()
  (plist-get (gui-runner-recorder '(:cmd "tree")) :surfaces))

(defun nhwl-gui--subs ()
  (seq-filter (lambda (n) (equal (plist-get n :role) "subsurface")) (nhwl-gui--tree)))

(defun nhwl-gui--sub-ids ()
  (mapcar (lambda (n) (plist-get n :id)) (nhwl-gui--subs)))

(defun nhwl-gui--node (id)
  (seq-find (lambda (n) (eql (plist-get n :id) id)) (nhwl-gui--tree)))

(defun nhwl-gui--new-sub (before)
  "The subsurface node that is not in the id list BEFORE (the newest), once it has a buffer."
  (seq-find (lambda (n) (and (not (memq (plist-get n :id) before)) (nhwl-gui--get n :buffer)))
            (nhwl-gui--subs)))

(defun nhwl-gui--child (&rest params)
  "A child frame of the selected frame at left 40, top 30 (device px), 20 x 5 characters; PARAMS first."
  (make-frame `(,@params (parent-frame . ,(selected-frame)) (left . 40) (top . 30) (width . 20) (height . 5)
                (undecorated . t) (minibuffer . nil))))

(defun nhwl-gui--child-node (before)
  (nhwl-gui--wait (lambda () (nhwl-gui--new-sub before)) 5))

(defun nhwl-gui--parent-offset (node)
  "The native area's offset (L T) in NODE's parent surface, device px: the margin and the title bar."
  (let* ((b (plist-get (nhwl-gui--node (plist-get node :parent)) :buffer))
         (l (/ (- (plist-get b :w) (frame-pixel-width (selected-frame))) 2)))
    (list l (- (plist-get b :h) (frame-pixel-height (selected-frame)) l))))

(ert-deftest nhwl-gui/child-frame-is-a-subsurface ()
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((s (frame-scale-factor)) (before (nhwl-gui--sub-ids)) (c nil))
    (unwind-protect
        (progn
          (setq c (nhwl-gui--child))
          (should (eq (frame-parent c) (selected-frame)))
          (let* ((n (nhwl-gui--child-node before))
                 (off (and n (nhwl-gui--parent-offset n))))
            (should n)
            (should (equal (plist-get n :role) "subsurface"))
            (should (equal (plist-get n :position)
                           (list (round (/ (+ (car off) 40) s)) (round (/ (+ (cadr off) 30) s)))))))
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest nhwl-gui/child-frame-move-is-a-position-change ()
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((s (frame-scale-factor)) (before (nhwl-gui--sub-ids)) (c nil))
    (unwind-protect
        (progn
          (setq c (nhwl-gui--child))
          (let* ((n (nhwl-gui--child-node before))
                 (id (plist-get n :id))
                 (off (nhwl-gui--parent-offset n))
                 (mark (nhwl-gui--mark)))
            (set-frame-position c 80 60)
            (should (nhwl-gui--wait
                     (lambda ()
                       (equal (plist-get (nhwl-gui--node id) :position)
                              (list (round (/ (+ (car off) 80) s)) (round (/ (+ (cadr off) 60) s)))))
                     5))
            (should-not (seq-filter (lambda (cm) (and (eql (plist-get cm :surface) id) (nhwl-gui--get cm :buffer)))
                                    (plist-get (gui-runner-recorder (list :cmd "log" :since mark)) :commits)))))
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest nhwl-gui/child-frame-corners-and-shadow ()
  (skip-unless (and (nhwl-gui--recorder-p) (boundp 'child-frame-chrome)))
  (let* ((s (frame-scale-factor)) (before (nhwl-gui--sub-ids)) (c nil)
         (old child-frame-chrome))
    (unwind-protect
        (progn
          (setq child-frame-chrome '((corner-radius . 8) (shadow :blur 6 :offset (0 . 2) :opacity 0.3)))
          (setq c (nhwl-gui--child))
          (glitnir-refresh c)
          (let* ((fw (frame-pixel-width c)) (fh (frame-pixel-height c))
                 (n (nhwl-gui--wait
                     (lambda ()
                       (let ((n (nhwl-gui--new-sub before)))
                         (and n (> (plist-get (plist-get n :buffer) :w) fw) n)))
                     5))
                 (b (plist-get n :buffer))
                 (r (car (nhwl-gui--get n :input_region))))
            (should n)
            (should (> (plist-get b :h) fh))
            ;; the input region is the frame's rectangle, inside the shadow margin
            (should r)
            (should (= (nth 2 r) (round (/ fw s))))
            (should (= (nth 3 r) (round (/ fh s))))
            (should (> (nth 0 r) 0))
            (should (> (nth 1 r) 0))
            (should (<= (+ (nth 0 r) (nth 2 r)) (round (/ (plist-get b :w) s))))))
      (setq child-frame-chrome old)
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest nhwl-gui/child-frame-churn-leaves-no-surface ()
  (skip-unless (nhwl-gui--recorder-p))
  (let ((before (nhwl-gui--sub-ids)) (c nil))
    (unwind-protect
        (progn
          (setq c (nhwl-gui--child))
          (should (nhwl-gui--child-node before))
          (dotimes (_ 100)
            (make-frame-invisible c t)
            (make-frame-visible c)
            (accept-process-output nil 0.001))
          (redisplay t)
          (dotimes (_ 20)
            (delete-frame (nhwl-gui--child) t))
          ;; delete while shown
          (should (frame-visible-p c))
          (delete-frame c t)
          (setq c nil)
          (should (nhwl-gui--wait (lambda () (null (nhwl-gui--subs))) 5))
          (should (frame-live-p (selected-frame))))
      (when (and c (frame-live-p c)) (delete-frame c t)))))

(ert-deftest nhwl-gui/child-frame-click-goes-to-the-child ()
  (skip-unless (nhwl-gui--recorder-p))
  (let ((before (nhwl-gui--sub-ids)) (c nil) (events nil))
    (unwind-protect
        (progn
          (setq c (nhwl-gui--child))
          (let ((id (plist-get (nhwl-gui--child-node before) :id)))
            (should id)
            (gui-runner-recorder (list :cmd "focus" :surface id))
            (gui-runner-recorder '(:cmd "pointer" :pos [10 10]))
            (gui-runner-recorder '(:cmd "button" :button 272 :state "press"))
            (gui-runner-recorder '(:cmd "button" :button 272 :state "release"))
            (let ((deadline (+ (float-time) 1)))
              (while (< (float-time) deadline)
                (let ((e (read-event nil nil 0.05)))
                  (when e (push e events)))))
            (let ((click (seq-find (lambda (e) (and (consp e) (memq (car e) '(down-mouse-1 mouse-1)))) events)))
              (should click)
              ;; a click outside every window of the frame names the frame itself
              (let* ((w (posn-window (event-start click)))
                     (fr (if (framep w) w (window-frame w))))
                (unless (eq fr c)
                  (ert-fail (format "click %S: frame %S, child %S (live %S), selected %S, all %S"
                                    click fr c (frame-live-p c) (selected-frame) events)))))))
      (ignore-errors (gui-runner-recorder '(:cmd "focus" :surface "toplevel")))
      (when (frame-live-p c) (delete-frame c t)))))

(ert-deftest nhwl-gui/child-frames-stack ()
  (skip-unless (nhwl-gui--recorder-p))
  (let ((a nil) (b nil))
    (unwind-protect
        (let* ((before (nhwl-gui--sub-ids))
               (ia (progn (setq a (nhwl-gui--child '(left . 10))) (plist-get (nhwl-gui--child-node before) :id)))
               (ib (progn (setq b (nhwl-gui--child '(left . 20))) (plist-get (nhwl-gui--child-node (cons ia before)) :id)))
               (parent (plist-get (nhwl-gui--node ia) :parent))
               (order (lambda ()
                        (seq-filter (lambda (i) (memq i (list ia ib)))
                                    (plist-get (nhwl-gui--node parent) :children)))))
          (should (equal (funcall order) (list ia ib)))
          (raise-frame a)
          (should (nhwl-gui--wait (lambda () (equal (funcall order) (list ib ia))) 5))
          (lower-frame a)
          (should (nhwl-gui--wait (lambda () (equal (funcall order) (list ia ib))) 5)))
      (dolist (f (list a b)) (when (and f (frame-live-p f)) (delete-frame f t))))))

;; Live rows: they run on any compositor, the real sway and mutter included, and assert only that the frame is live and
;; nothing signalled.  What the shell said to the compositor is judged afterwards by the trace checker
;; (`gui.sh --wayland-debug F --trace-check RULES', nidhogg/test/wl-trace-check.el).

(defun nhwl-gui--live-settle (&optional seconds)
  "Redisplay and run process output for SECONDS (default 0.3), so the shell's commits and the compositor's answers happen."
  (let ((end (+ (float-time) (or seconds 0.3))))
    (while (< (float-time) end)
      (redisplay t)
      (accept-process-output nil 0.05))))

;; They leave frames, a tip and the window size behind, so they run after the other rows (`runs-last'); a row that ended
;; the session's display (the display-lost row) leaves no native frame, and they skip.
(ert-deftest nhwl-gui/live-child-frames ()
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--live-p) (nhwl-gui--nhwl-frames)))
  (let ((a nil) (b nil) (g nil) (old (and (boundp 'child-frame-chrome) child-frame-chrome)))
    (unwind-protect
        (progn
          (setq a (nhwl-gui--child '(left . 10)) b (nhwl-gui--child '(left . 120)))
          (nhwl-gui--live-settle)
          (dotimes (i 5)
            (set-frame-position a (+ 20 (* i 15)) (+ 30 (* i 10)))
            (nhwl-gui--live-settle))
          (raise-frame a) (nhwl-gui--live-settle)
          (lower-frame a) (nhwl-gui--live-settle)
          (make-frame-invisible b) (nhwl-gui--live-settle)
          (make-frame-visible b) (nhwl-gui--live-settle)
          ;; the chrome (shadow, rounded corners) sets the input region; with and without
          (when (and (boundp 'child-frame-chrome) (fboundp 'glitnir-refresh))
            (setq child-frame-chrome '((corner-radius . 8) (shadow :blur 6 :offset (0 . 2) :opacity 0.3)))
            (glitnir-refresh a) (nhwl-gui--live-settle)
            (setq child-frame-chrome nil)
            (glitnir-refresh a) (nhwl-gui--live-settle))
          ;; a grandchild
          (setq g (make-frame `((parent-frame . ,a) (left . 5) (top . 5) (width . 8) (height . 2)
                                (undecorated . t) (minibuffer . nil))))
          (nhwl-gui--live-settle)
          (set-frame-position g 15 12) (nhwl-gui--live-settle)
          (set-frame-position g 25 20) (nhwl-gui--live-settle)
          (should (frame-live-p a))
          (should (frame-live-p b))
          (should (frame-live-p g)))
      (when (boundp 'child-frame-chrome) (setq child-frame-chrome old))
      (dolist (f (list g b a)) (when (and f (frame-live-p f)) (delete-frame f t))))
    (nhwl-gui--live-settle)
    (should (frame-live-p (selected-frame)))))

(ert-deftest nhwl-gui/live-tip-moves ()
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--live-p) (nhwl-gui--nhwl-frames)))
  (unwind-protect
      (progn
        (x-show-tip "hello" nil nil 10)
        (nhwl-gui--live-settle 0.5)
        ;; only the offset changes: the tip moves with DX and DY and nothing else
        (dotimes (i 5)
          (x-show-tip "hello" nil nil 10 (* (1+ i) 7) (* (1+ i) 3))
          (nhwl-gui--live-settle))
        (should (frame-live-p (selected-frame))))
    (x-hide-tip)
    (nhwl-gui--live-settle)))

(ert-deftest nhwl-gui/live-resize ()
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--live-p) (nhwl-gui--nhwl-frames)))
  (let ((old (frame-parameter nil 'fullscreen)))
    (unwind-protect
        (progn
          (set-frame-parameter nil 'fullscreen nil)
          (nhwl-gui--live-settle 0.5)
          (set-frame-parameter nil 'fullscreen 'fullboth)
          (nhwl-gui--live-settle 0.5)
          (set-frame-parameter nil 'fullscreen nil)
          (nhwl-gui--live-settle 0.5)
          (set-frame-size nil 60 20)
          (nhwl-gui--live-settle 0.5)
          (set-frame-size nil 70 24)
          (nhwl-gui--live-settle 0.5)
          (should (frame-live-p (selected-frame))))
      (set-frame-parameter nil 'fullscreen old)
      (nhwl-gui--live-settle 0.5))))

;; `skip-unless' is `(unless (ignore-errors FORM) (ert-skip ...))': a void function in its form skips the row silently.
;; This row fails when a file's `skip-unless' forms call a function that is not defined.
(defvar nhwl-gui--source (or load-file-name buffer-file-name))

(ert-deftest nhwl-gui/skip-predicates-are-defined ()
  (let (missing)
    (with-temp-buffer
      (insert-file-contents nhwl-gui--source)
      (goto-char (point-min))
      (while (re-search-forward "^ +(skip-unless " nil t)
        (let ((form (save-excursion (goto-char (match-beginning 0)) (read (current-buffer)))))
          (letrec ((walk (lambda (f)
                           (when (and (consp f) (not (eq (car f) 'quote)))
                             (when (and (symbolp (car f)) (not (keywordp (car f))) (not (fboundp (car f))))
                               (push (car f) missing))
                             (when (listp (cdr f)) (mapc walk (cdr f)))))))
            (funcall walk (cadr form))))))
    (should-not missing)))

;;; nhwl-gui-tests.el ends here

;; The bare client-side title bar (nhwlcsd.c).  Logical sizes: the strip is 32, the resize margins are 8.

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
          ;; the shell's frames only (a daemon has its own frame too)
          (should (= (length (nhwl-gui--nhwl-frames)) 2))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump) (redisplay t) (funcall fresh)) 5))
          (gui-runner-wait-settled)
          (let* ((dst (plist-get (plist-get (car (last (funcall fresh))) :viewport) :dst)))
            ;; the close button is the rightmost 32 px of the strip, inside the 8 px margin
            (nhwl-gui--click (- (car dst) 8 16) (+ 8 16))
            (should (nhwl-gui--wait (lambda () (nhwl-gui--pump) (= (length (nhwl-gui--nhwl-frames)) 1))))
            (should (frame-live-p old))))
      (when (and new (frame-live-p new)) (delete-frame new t)))))

(ert-deftest nhwl-gui/ssd-no-strip ()
  (skip-unless (and (nhwl-gui--recorder-p) (equal (nhwl-gui--decoration) "server")))
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

(ert-deftest nhwl-gui/stats-after-typing ()
  "`glitnir-stats' answers on a shell frame with the keys budgets.sh reads."
  (skip-unless (nhwl-gui--live-p))
  ;; A blinking cursor redisplays on its own timer; each such redisplay is a paint with no damage in the record, and
  ;; the record's damage is a median, so the blink would outvote the one typed paint.
  (let ((blink blink-cursor-mode))
    (blink-cursor-mode -1)
    (unwind-protect
        (nhwl-gui--stats-after-typing)
      (when blink (blink-cursor-mode 1)))))

(defun nhwl-gui--stats-after-typing ()
  (switch-to-buffer "*scratch*")
  (erase-buffer)
  (redisplay t)
  (gui-runner-wait-settled)
  (glitnir-stats nil t)
  (insert "x")
  (redisplay t)
  (gui-runner-wait-settled)
  (let ((s (glitnir-stats)))
    (should s)
    (should (> (plist-get s :count) 0))
    (should (>= (plist-get (plist-get s :latency-ms) :p95) 0))
    (should (> (plist-get s :damage-px) 0))
    (should (> (plist-get s :painted-px) 0))))

;; The pointer while typing: hidden by a key, back with the mouse.

(defun nhwl-gui--pointer-entries (mark)
  "The recorder's cursor requests since MARK, oldest first."
  (seq-filter (lambda (e) (member (plist-get e :req) '("set_cursor" "set_shape")))
              (plist-get (gui-runner-recorder (list :cmd "input_log" :since mark)) :entries)))

(defun nhwl-gui--cursor-hidden-p (e)
  (and (equal (plist-get e :req) "set_cursor") (eq (plist-get e :surface) :null)))

(ert-deftest nhwl-gui/pointer-hidden-while-typing ()
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--windowed)
  (let ((make-pointer-invisible t)
        (buf (generate-new-buffer " *nhwl-pointer*"))
        (mark (plist-get (gui-runner-recorder '(:cmd "input_log" :since 0)) :next)))
    (unwind-protect
        (progn
          (switch-to-buffer buf)
          (nhwl-gui--point 100 100)
          (gui-runner-recorder '(:cmd "key" :text "a"))
          (should (nhwl-gui--wait
                   (lambda ()
                     (let ((ev (read-event nil nil 0.1)))
                       (when (characterp ev)
                         (setq last-command-event ev)
                         (command-execute (key-binding (vector ev)) nil (vector ev))))
                     (nhwl-gui--cursor-hidden-p (car (last (nhwl-gui--pointer-entries mark)))))))
          (nhwl-gui--point 120 110)
          (should (nhwl-gui--wait
                   (lambda ()
                     (nhwl-gui--pump)
                     (let ((e (car (last (nhwl-gui--pointer-entries mark)))))
                       (and e (not (nhwl-gui--cursor-hidden-p e))))))))
      (kill-buffer buf))))

;; The recorder honours only tokens it issued, so the row first has the frame ask for one (a click gives the input serial).
(defun nhwl-gui--issued-token ()
  (nhwl-gui--click 20 20)
  (select-frame-set-input-focus (selected-frame))
  (plist-get (nhwl-gui--wait
              (lambda ()
                (nhwl-gui--pump)
                (car (last (plist-get (gui-runner-recorder '(:cmd "activation_log")) :activations))))
              5)
             :token))

(ert-deftest nhwl-gui/client-frame-activates-with-its-token ()
  (skip-unless (and (nhwl-gui--recorder-p) (daemonp)))
  (let* ((token (nhwl-gui--issued-token))
         (mark (nhwl-gui--mark))
         (old-surface (plist-get (nhwl-gui--last-commit) :surface))
         (new (make-frame `((environment . (,(concat "XDG_ACTIVATION_TOKEN=" token)))))))
    (should (stringp token))
    (unwind-protect
        (let* ((entries (lambda ()
                          (seq-filter (lambda (a) (equal (plist-get a :token) token))
                                      (plist-get (gui-runner-recorder '(:cmd "activation_log")) :activations))))
               (surfaces (lambda ()
                           (seq-remove (lambda (id) (eql id old-surface))
                                       (mapcar (lambda (c) (plist-get c :surface))
                                               (plist-get (gui-runner-recorder (list :cmd "log" :since mark)) :commits))))))
          ;; the new frame's surface carries the handed-in token
          (should (nhwl-gui--wait
                   (lambda ()
                     (seq-find (lambda (a) (memql (plist-get a :surface) (funcall surfaces))) (funcall entries)))
                   5)))
      (when (frame-live-p new) (delete-frame new t)))))

;; Task 11: tip frames.  A tooltip is an xdg popup of the frame under the pointer; the recorder's `tree' shows it with
;; role "popup".

(defun nhwl-gui--popup-ids ()
  (mapcar (lambda (n) (plist-get n :id))
          (seq-filter (lambda (n) (equal (plist-get n :role) "popup")) (nhwl-gui--tree))))

(defun nhwl-gui--new-popup (before)
  "The popup node that is not in the id list BEFORE, once it has a buffer."
  (seq-find (lambda (n) (and (equal (plist-get n :role) "popup")
                             (not (memq (plist-get n :id) before))
                             (nhwl-gui--get n :buffer)))
            (nhwl-gui--tree)))

(ert-deftest nhwl-gui/tooltip-is-a-popup ()
  (skip-unless (nhwl-gui--recorder-p))
  (let ((before (nhwl-gui--popup-ids)))
    (unwind-protect
        (progn
          (x-show-tip "hello" nil nil 5)
          (let ((n (nhwl-gui--wait (lambda () (nhwl-gui--pump) (nhwl-gui--new-popup before)) 5)))
            (should n)
            (should (> (plist-get (plist-get n :buffer) :w) 0))
            (should (> (plist-get (plist-get n :buffer) :h) 0))))
      (x-hide-tip))))

(ert-deftest nhwl-gui/tooltip-hidden-by-x-hide-tip ()
  (skip-unless (nhwl-gui--recorder-p))
  (let ((before (nhwl-gui--popup-ids)))
    (unwind-protect
        (progn
          (x-show-tip "hello" nil nil 5)
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump) (nhwl-gui--new-popup before)) 5))
          (should (x-hide-tip))
          (should (nhwl-gui--wait (lambda () (equal (nhwl-gui--popup-ids) before)) 5))
          (should-not (x-hide-tip)))
      (x-hide-tip))))

(ert-deftest nhwl-gui/tooltip-dismissed-by-the-compositor ()
  (skip-unless (nhwl-gui--recorder-p))
  (let ((before (nhwl-gui--popup-ids)))
    (unwind-protect
        (progn
          (x-show-tip "hello" nil nil 5)
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump) (nhwl-gui--new-popup before)) 5))
          (gui-runner-recorder '(:cmd "popup_done" :surface "newest"))
          (should (nhwl-gui--wait (lambda () (nhwl-gui--pump) (equal (nhwl-gui--popup-ids) before)) 5))
          (should-not (x-hide-tip)))
      (x-hide-tip))))

;; Present path S2: acks are recorded and go out with the commit that carries them.

(defun nhwl-gui--popup-commits (mark)
  "The popups' commits since MARK."
  (seq-filter (lambda (c) (equal (plist-get c :role) "popup"))
              (plist-get (gui-runner-recorder (list :cmd "log" :since mark)) :commits)))

(ert-deftest nhwl-gui/tip-move-applies ()
  "A tip that only moves (DX and DY) is moved by the commit that carries the reposition's ack."
  (skip-unless (nhwl-gui--recorder-p))
  (let ((before (nhwl-gui--popup-ids)))
    (unwind-protect
        (progn
          (x-show-tip "hello" nil nil 30 0 0)
          (let* ((n (nhwl-gui--wait (lambda () (nhwl-gui--pump) (nhwl-gui--new-popup before)) 5))
                 (mark (nhwl-gui--mark))
                 (was (plist-get n :position)))
            (should n)
            (x-show-tip "hello" nil nil 30 20 12)
            (should (nhwl-gui--wait
                     (lambda ()
                       (nhwl-gui--pump)
                       (and (seq-find (lambda (c) (nhwl-gui--get c :ack_serial)) (nhwl-gui--popup-commits mark))
                            (not (equal (plist-get (nhwl-gui--node (plist-get n :id)) :position) was))))
                     1))))
      (x-hide-tip))))

(ert-deftest nhwl-gui/ack-carries-the-new-size ()
  "Two configures while the pacer waits: every ack comes with a buffer of its own configure's size (the older
serial is never acked early, with the old buffer)."
  ;; the profile must be able to hold frame callbacks (mutter's occlusion)
  (skip-unless (and (nhwl-gui--recorder-p) (ignore-errors (gui-runner-recorder '(:cmd "unocclude")))))
  (nhwl-gui--windowed)
  (switch-to-buffer "*scratch*")
  (unwind-protect
      (progn
        (gui-runner-recorder '(:cmd "occlude"))
        (insert "x")
        (redisplay t)
        (sleep-for 0.1)
        ;; the commit above waits for its callback, which is held
        (let* ((mark (nhwl-gui--mark))
               (a (let ((inhibit-redisplay t))
                    (prog1 (plist-get (gui-runner-recorder
                                       '(:cmd "configure" :surface "focused" :size [700 500] :states ["activated"]))
                                      :serial)
                      (sleep-for 0.2)
                      (input-pending-p))))
               (b (plist-get (gui-runner-recorder
                              '(:cmd "configure" :surface "focused" :size [760 540] :states ["activated"]))
                             :serial)))
          (sleep-for 0.2)
          (input-pending-p)
          (should (nhwl-gui--wait
                   (lambda ()
                     (redisplay t)
                     (seq-find (lambda (c) (eql (nhwl-gui--get c :ack_serial) b)) (nhwl-gui--new mark)))
                   5))
          (let ((acks (seq-filter (lambda (c) (nhwl-gui--get c :ack_serial)) (nhwl-gui--new mark))))
            ;; each ack comes with a buffer of its own configure's size
            (dolist (c acks)
              (should (equal (nhwl-gui--logical-size c)
                             (if (eql (nhwl-gui--get c :ack_serial) a) '(700 500) '(760 540))))))))
    (gui-runner-recorder '(:cmd "unocclude"))))

;;; Monitors (native shells W1 Task 14)

(defun nhwl-gui--pump (seconds)
  "Read events for SECONDS: the shell's records reach Lisp (the monitors-changed hook) only through the input loop."
  (let ((end (+ (float-time) seconds)))
    (while (< (float-time) end) (read-event nil nil 0.05))))

(defun nhwl-gui--wait-ev (pred)
  "Like `nhwl-gui--wait' with PRED, reading events between the tries."
  (nhwl-gui--wait (lambda () (read-event nil nil 0.05) (funcall pred))))

(defun nhwl-gui--monitor (name)
  "The `display-monitor-attributes-list' entry called NAME."
  (seq-find (lambda (m) (equal (alist-get 'name m) name)) (display-monitor-attributes-list)))

(defvar nhwl-gui--hook-runs 0)

(defmacro nhwl-gui--with-second-output (&rest body)
  "Run BODY with a second output, nh-1 (800x600 at 2x), counting the monitors-changed hook in `nhwl-gui--hook-runs'."
  `(let* ((nhwl-gui--hook-runs 0)
          (fn (lambda (_terminal) (setq nhwl-gui--hook-runs (1+ nhwl-gui--hook-runs)))))
     (add-hook 'display-monitors-changed-functions fn)
     (unwind-protect
         (progn
           (gui-runner-recorder '(:cmd "output" :op "add" :name "nh-1" :size [800 600] :scale120 240))
           (should (nhwl-gui--wait-ev (lambda () (nhwl-gui--monitor "nh-1"))))
           (nhwl-gui--pump 0.3)
           ,@body)
       ;; the removal's own record must be read before the next row counts the hook
       (ignore-errors (gui-runner-recorder '(:cmd "output" :op "remove" :name "nh-1")))
       (nhwl-gui--wait-ev (lambda () (null (nhwl-gui--monitor "nh-1"))))
       (nhwl-gui--pump 0.3)
       (remove-hook 'display-monitors-changed-functions fn))))

(ert-deftest nhwl-gui/monitor-list-has-the-output ()
  (skip-unless (nhwl-gui--recorder-p))
  (let ((m (nhwl-gui--monitor "nh-0")))
    (should m)
    (should (equal (alist-get 'geometry m) (alist-get 'workarea m)))
    (should (consp (alist-get 'mm-size m)))
    (should (numberp (alist-get 'scale-factor m)))
    (should (equal (alist-get 'source m) "Wayland"))
    (should (memq (selected-frame) (alist-get 'frames m)))))

(ert-deftest nhwl-gui/output-added-runs-the-hook-once ()
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--with-second-output
   (should (= nhwl-gui--hook-runs 1))
   (let ((m (nhwl-gui--monitor "nh-1")))
     (should (= (alist-get 'scale-factor m) 2.0))
     (should (equal (nthcdr 2 (alist-get 'geometry m)) '(800 600))))
   (should (= (length (display-monitor-attributes-list)) 2))))

(ert-deftest nhwl-gui/output-removed-under-a-frame ()
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--with-second-output
   (gui-runner-recorder '(:cmd "output" :op "enter" :name "nh-1" :surface "toplevel"))
   (should (nhwl-gui--wait-ev (lambda () (alist-get 'frames (nhwl-gui--monitor "nh-1")))))
   (setq nhwl-gui--hook-runs 0)
   (let ((seq (plist-get (gui-runner-recorder '(:cmd "log" :since 0)) :next))
         (f (selected-frame)))
     (gui-runner-recorder '(:cmd "output" :op "remove" :name "nh-1"))
     (should (nhwl-gui--wait-ev (lambda () (null (nhwl-gui--monitor "nh-1")))))
     (nhwl-gui--pump 0.3)
     (should (= (length (display-monitor-attributes-list)) 1))
     (should (= nhwl-gui--hook-runs 1))
     (should (frame-live-p f))
     (with-temp-buffer
       (insert "after the output went away")
       (set-window-buffer (frame-selected-window f) (current-buffer))
       (redisplay t)
       (should (nhwl-gui--wait
                (lambda ()
                  (plist-get (gui-runner-recorder `(:cmd "log" :since ,seq)) :commits))))))))

(ert-deftest nhwl-gui/frame-lists-its-monitor ()
  (skip-unless (nhwl-gui--recorder-p))
  (nhwl-gui--with-second-output
   (gui-runner-recorder '(:cmd "output" :op "enter" :name "nh-1" :surface "toplevel"))
   (should (nhwl-gui--wait-ev (lambda () (alist-get 'frames (nhwl-gui--monitor "nh-1")))))
   (let ((on-1 (alist-get 'frames (nhwl-gui--monitor "nh-1")))
         (on-0 (alist-get 'frames (nhwl-gui--monitor "nh-0"))))
     (should-not (seq-intersection on-1 on-0)))
   (gui-runner-recorder '(:cmd "output" :op "leave" :name "nh-1" :surface "toplevel"))
   (should (nhwl-gui--wait-ev (lambda () (null (alist-get 'frames (nhwl-gui--monitor "nh-1"))))))))
