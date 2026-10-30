;;; nhwl-gui-w3-tests.el --- GUI rows for the native shell's presenter  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh with the base rows (nhwl-gui-tests.el, loaded first: its helpers are used here).  Rows
;; that drive the compositor need `--compositor recorder'; rows named `nhwl-gui/live-*' run on real compositors and
;; are judged by the trace checker.  Each feature appends its rows under its own heading.

(require 'ert)
(require 'nhwl-gui-tests)
(require 'nhwl-gui-w2-tests)                ; nhwl-gui--connected-p

;; The presenter thread.  With `nhwl-presenter-thread' on, a thread of the shell composes and commits each
;; tree when its pacer opens; the main thread only publishes.  `(nhwl-presenter-stats)' carries `:main-wait-max-ns',
;; the longest the main thread waited for the presenter.

(defun nhwl-gui--presenter-on-p ()
  "Non-nil when this Emacs presents on the shell's thread."
  (and (boundp 'nhwl-presenter-thread) nhwl-presenter-thread (fboundp 'nhwl-presenter-stats)
       (eql (plist-get (nhwl-presenter-stats) :thread-on) 1)))

(defun nhwl-gui--damaged-toplevel-commits (mark)
  "The toplevel commits since MARK that carry damage."
  (seq-filter (lambda (c) (and (equal (plist-get c :role) "toplevel") (plist-get c :damage)))
              (nhwl-gui--log-since mark)))

(defun nhwl-gui--presenter-switches ()
  "The context switches of this Emacs's `nh-present' thread so far (nil when it has none)."
  (let ((dir (format "/proc/%d/task" (emacs-pid))))
    (seq-some (lambda (tid)
                (let ((comm (expand-file-name (concat tid "/comm") dir)))
                  (when (and (file-readable-p comm)
                             (equal (string-trim (with-temp-buffer
                                                   (insert-file-contents comm) (buffer-string)))
                                    "nh-present"))
                    (with-temp-buffer
                      (insert-file-contents (expand-file-name (concat tid "/status") dir))
                      (let ((n 0))
                        (goto-char (point-min))
                        (while (re-search-forward "ctxt_switches:[ \t]+\\([0-9]+\\)" nil t)
                          (setq n (+ n (string-to-number (match-string 1)))))
                        n)))))
              (directory-files dir nil "\\`[0-9]+\\'"))))

(ert-deftest nhwl-gui/commit-while-lisp-busy ()
  "Review Focus 3: a redisplay publishes while the pacer waits for its frame callback, and Lisp then runs for 200 ms
without returning to the command loop.  The callback comes during that time, and the presenter commits the picture
before the Lisp ends: the commit does not wait for the main loop."
  (skip-unless (and (nhwl-gui--recorder-p) (nhwl-gui--presenter-on-p)))
  (nhwl-gui--with-blink
   nil
   (lambda ()
     (let ((buf (generate-new-buffer " *nhwl-busy*")))
       (unwind-protect
           (progn
             (switch-to-buffer buf)
             (nhwl-gui--pump)
             (let ((mark (nhwl-gui--mark)))
               ;; the first picture commits at once and leaves the pacer waiting; the second waits for the callback
               (insert "a")
               (redisplay t)
               (insert "b")
               (redisplay t)
               (let ((end (+ (float-time) 0.2)))
                 (while (< (float-time) end)))
               ;; still inside the busy Lisp: nothing has run the command loop since the second redisplay
               (should (>= (length (nhwl-gui--damaged-toplevel-commits mark)) 2))))
         (kill-buffer buf))))))

(ert-deftest nhwl-gui/delete-frame-storm-while-presenting ()
  "Review Focus 4: 200 cycles of making and deleting a frame with a child frame and a tooltip while typing, with the
presenter composing and committing all along.  No hang (the row's own deadline is 120 s), the shell stays connected,
and the main thread never waited 1 ms for the presenter.  Under gui.sh `--daemon' too."
  (skip-unless (and (nhwl-gui--recorder-p) (nhwl-gui--presenter-on-p)))
  (let ((t0 (float-time)) (buf (generate-new-buffer " *nhwl-storm*")))
    (unwind-protect
        (with-current-buffer buf
          (dotimes (i 200)
            (let* ((f (make-frame '((width . 40) (height . 10))))
                   (c (make-frame `((parent-frame . ,f) (width . 10) (height . 3) (minibuffer . nil)))))
              (with-selected-frame f
                (switch-to-buffer buf)
                (insert (format "%d " i))
                (redisplay t)
                (x-show-tip "tip" f nil 1)
                (insert "x")
                (redisplay t))
              (x-hide-tip)
              (when (frame-live-p c) (delete-frame c))
              (delete-frame f))
            (when (zerop (% i 20)) (nhwl-gui--pump 0.05))
            (should (< (- (float-time) t0) 120)))
          (nhwl-gui--pump)
          (should (nhwl-gui--connected-p))
          (should (< (plist-get (nhwl-presenter-stats) :main-wait-max-ns) 1000000)))
      (kill-buffer buf))))

(ert-deftest nhwl-gui/presenter-idle-has-no-wakeups ()
  "An idle frame's presenter sleeps: once the quiet period dropped the fronts, its thread is not woken for 3 s.
Idle means no tooltip either: a tip an earlier row left shown hides at its own timer, which is a real wake."
  (skip-unless (and (nhwl-gui--recorder-p) (nhwl-gui--presenter-on-p)))
  (nhwl-gui--with-blink
   nil
   (lambda ()
     (let ((trims (plist-get (nhwl-presenter-stats) :front-trims)))
       (x-hide-tip)
       (redisplay t)
       ;; the quiet period (2 s) and the trim's own wake
       (should (nhwl-gui--wait (lambda ()
                                 (> (plist-get (nhwl-presenter-stats) :front-trims) trims))
                               5))
       (let ((before (nhwl-gui--presenter-switches)))
         (should before)
         (sleep-for 3)
         (should (equal (nhwl-gui--presenter-switches) before)))))))

;; Window layers.  On an opaque top-level frame, a window's full-window scroll gives it a layer of its own (a
;; subsurface of the root whose viewport source moves with the scroll); two quiet seconds, a resize, split or delete of
;; the window, a scale change or translucency take it away at the next publish.  `(nhwl--scroll-layers)' counts the
;; selected frame's.  The recorder logs the layer's commits as a subsurface's, with a viewport source.

(declare-function nhwl--scroll-layers "nhwlterm.c")

(defun nhwl-gui--layers-p ()
  "Non-nil when this Emacs has window layers and they are on."
  (and (nhwl-gui--recorder-p) (fboundp 'nhwl--scroll-layers) (boundp 'nhwl-scroll-layers) nhwl-scroll-layers))

(defun nhwl-gui--with-layer-buffer (fn)
  "Call FN in one window showing 400 numbered lines from the top, with the cursor not blinking; clean up after."
  (nhwl-gui--with-blink
   nil
   (lambda ()
     (let ((buf (generate-new-buffer " *nhwl-layers*")))
       (unwind-protect
           (progn
             (delete-other-windows)
             (switch-to-buffer buf)
             (dotimes (i 400) (insert (format "line %03d of the window-layer rows, wide enough to fill columns\n" i)))
             (goto-char (point-min))
             (redisplay t)
             (gui-runner-wait-settled)
             (funcall fn))
         (kill-buffer buf)
         (delete-other-windows)
         (redisplay t))))))

(defun nhwl-gui--layer-commits (mark)
  "The subsurface commits since MARK that set a viewport source: the window layers'."
  (seq-filter (lambda (c) (and (equal (plist-get c :role) "subsurface")
                               (nhwl-gui--get (plist-get c :viewport) :src)))
              (nhwl-gui--log-since mark)))

(defun nhwl-gui--layer-ids (mark)
  "The surface ids of the window layers that committed since MARK."
  (seq-uniq (mapcar (lambda (c) (plist-get c :surface)) (nhwl-gui--layer-commits mark))))

(defun nhwl-gui--scroll-promote ()
  "Scroll the selected window one line up and down again; return the number of window layers after it."
  (scroll-up 1)
  (redisplay t)
  (scroll-down 1)
  (redisplay t)
  (nhwl--scroll-layers))

(defun nhwl-gui--area (rects)
  (apply #'+ (mapcar (lambda (r) (* (nth 2 r) (nth 3 r))) rects)))

(ert-deftest nhwl-gui/scroll-layer-composite-matches-backing ()
  "At the run's scale (the gate runs this at `--scale' 1, 1.25, 1.5 and 2): 50 scrolls each way with typing between
them, in a window with a layer of its own → once settled, the recorder's composite equals the backing.  The layer's
view of its ring and the frame around it line up with no seam."
  (skip-unless (nhwl-gui--layers-p))
  (nhwl-gui--with-layer-buffer
   (lambda ()
     (let ((layered 0))
       (dotimes (i 50)
         (scroll-up 1)
         (redisplay t)
         (setq layered (max layered (nhwl--scroll-layers)))
         (when (zerop (% i 7))
           (insert "t")
           (redisplay t)))
       (dotimes (i 50)
         (scroll-down 1)
         (redisplay t)
         (when (zerop (% i 7))
           (insert "u")
           (redisplay t)))
       (should (= layered 1))
       (redisplay t)
       (gui-runner-wait-settled)
       (should (equal (nhwl-gui--grab-matches-snapshot) '(0 . 0)))))))

(ert-deftest nhwl-gui/scroll-layer-promotes-on-scroll-and-demotes-when-quiet ()
  "No layer before the first scroll; a full-window scroll makes one (a subsurface with a viewport source); two quiet
seconds later the next publish takes it away, and its unmap is applied by a root commit (one tree commit)."
  (skip-unless (nhwl-gui--layers-p))
  (nhwl-gui--with-layer-buffer
   (lambda ()
     (should (= (nhwl--scroll-layers) 0))
     (let ((mark (nhwl-gui--mark)))
       (should (= (nhwl-gui--scroll-promote) 1))
       (gui-runner-wait-settled)
       (let ((ids (nhwl-gui--layer-ids mark)))
         (should (= (length ids) 1))
         (sleep-for 2.2)
         (let ((quiet (nhwl-gui--mark)))
           (insert "q")
           (redisplay t)
           (should (= (nhwl--scroll-layers) 0))
           (gui-runner-wait-settled)
           (let ((unmap (seq-find (lambda (c) (not (nhwl-gui--get c :buffer)))
                                  (nhwl-gui--surface-commits (car ids) quiet))))
             (should unmap)
             (should (nhwl-gui--get unmap :applied_by)))
           (should (equal (nhwl-gui--grab-matches-snapshot) '(0 . 0)))))))))

(ert-deftest nhwl-gui/scroll-layer-not-for-translucent-frames ()
  "A frame with `alpha' 0.9, or `alpha-background' 0.8, gets no layer; a layered window loses its layer at the next
publish once its frame turns translucent."
  (skip-unless (nhwl-gui--layers-p))
  (let ((alpha (frame-parameter nil 'alpha)) (bg (frame-parameter nil 'alpha-background)))
    (unwind-protect
        (nhwl-gui--with-layer-buffer
         (lambda ()
           (set-frame-parameter nil 'alpha 0.9)
           (redisplay t)
           (should (= (nhwl-gui--scroll-promote) 0))
           (set-frame-parameter nil 'alpha alpha)
           (set-frame-parameter nil 'alpha-background 0.8)
           (redisplay t)
           (should (= (nhwl-gui--scroll-promote) 0))
           (set-frame-parameter nil 'alpha-background bg)
           (redisplay t)
           (should (= (nhwl-gui--scroll-promote) 1))
           (set-frame-parameter nil 'alpha 0.9)
           (redisplay t)
           (should (= (nhwl--scroll-layers) 0))))
      (set-frame-parameter nil 'alpha alpha)
      (set-frame-parameter nil 'alpha-background bg)
      (redisplay t))))

(ert-deftest nhwl-gui/scroll-layer-partial-scroll-is-damage ()
  "`open-line' in the middle of the window moves the rows below it only: no layer, and the frame's commit damages
the moved rows."
  (skip-unless (nhwl-gui--layers-p))
  (nhwl-gui--with-layer-buffer
   (lambda ()
     (forward-line (/ (window-body-height) 2))
     (redisplay t)
     (gui-runner-wait-settled)
     (let ((mark (nhwl-gui--mark)))
       (open-line 1)
       (redisplay t)
       (gui-runner-wait-settled)
       (should (= (nhwl--scroll-layers) 0))
       (should-not (nhwl-gui--layer-commits mark))
       (let ((damage (apply #'append (mapcar #'nhwl-gui--damage (nhwl-gui--new mark))))
             (half (+ (nhwl-gui--native-top) (/ (window-body-height nil t) 2))))
         ;; the rows below the new line moved: damage reaches past the middle
         (should (seq-some (lambda (r) (> (+ (nth 1 r) (nth 3 r)) half)) damage)))))))

(ert-deftest nhwl-gui/scroll-layer-window-split-and-delete ()
  "Splitting a layered window takes its layer away at the next publish; a scroll in one half layers that half; deleting
the other window (the half grows) takes it away again."
  (skip-unless (nhwl-gui--layers-p))
  (nhwl-gui--with-layer-buffer
   (lambda ()
     (should (= (nhwl-gui--scroll-promote) 1))
     (split-window-below)
     (redisplay t)
     (should (= (nhwl--scroll-layers) 0))
     (should (= (nhwl-gui--scroll-promote) 1))
     (delete-other-windows)
     (redisplay t)
     (should (= (nhwl--scroll-layers) 0))
     (gui-runner-wait-settled)
     (should (equal (nhwl-gui--grab-matches-snapshot) '(0 . 0))))))

(ert-deftest nhwl-gui/scroll-layer-under-child-frame ()
  "A posframe-style child frame over a layered window stays above the layer: among the root's subsurfaces, bottom to
top, the layer comes before the child, and stays there while the window scrolls on."
  (skip-unless (nhwl-gui--layers-p))
  (nhwl-gui--with-layer-buffer
   (lambda ()
     (let ((mark (nhwl-gui--mark)) (before nil) (layer nil) (c nil))
       (should (= (nhwl-gui--scroll-promote) 1))
       (gui-runner-wait-settled)
       ;; the layer is a new subsurface too: note it, and the subsurfaces, before the child comes
       (setq layer (car (nhwl-gui--layer-ids mark))
             before (nhwl-gui--sub-ids))
       (unwind-protect
           (progn
             (setq c (nhwl-gui--child))
             (let ((child (nhwl-gui--child-node before)))
               (should (and child layer))
               (dotimes (_ 10) (scroll-up 1) (redisplay t))
               (gui-runner-wait-settled)
               (let* ((root (plist-get child :parent))
                      (kids (append (plist-get (nhwl-gui--node root) :children) nil)))
                 (should (< (seq-position kids layer) (seq-position kids (plist-get child :id)))))))
         (when (frame-live-p c) (delete-frame c)))))))

(ert-deftest nhwl-gui/scroll-upload-is-exposed-rows-only ()
  "Per one-line scroll of a layered window: the layer's damage is at most the exposed line and the cursor's two lines,
and the root's damage stays in the mode line (and the one-row overlap a fractional scale snaps in)."
  (skip-unless (nhwl-gui--layers-p))
  (nhwl-gui--with-layer-buffer
   (lambda ()
     ;; an echo-area message (the startup one) has its last cell drawn again by each scroll's redisplay, outside the
     ;; mode line
     (message nil)
     ;; an earlier row's pointer may rest on the menu bar (it is at (100, 50)): the redisplay that handles that
     ;; pointer draws the menu bar's item again, outside the mode line, so the pointer goes into the text first
     (nhwl-gui--point 300 400)
     (redisplay t)
     (should (= (nhwl-gui--scroll-promote) 1))
     (gui-runner-wait-settled)
     ;; frame pixels are device pixels here; the layer is the window's box snapped outward (one row or column more)
     (let* ((top (nhwl-gui--native-top))
            (line (frame-char-height))
            (width (window-pixel-width))
            (mode-y0 (+ top (nth 3 (window-inside-pixel-edges)) -1))
            (mode-y1 (+ top (nth 3 (window-pixel-edges)) 1))
            (rebases 0) (last-y nil) (ring nil))
       (dotimes (_ 20)
         (let ((mark (nhwl-gui--mark)) (cs nil))
           (scroll-up 1)
           (redisplay t)
           (gui-runner-wait-settled)
           ;; the ring's view going back to the middle of its buffers (it would have left them) draws the whole view
           ;; there, by design, and so does a ring that was just made (some runs make the layer's ring at the first
           ;; scroll, with its view in the middle); the bound is computed from the ring's geometry below
           (dolist (c (nhwl-gui--layer-commits mark))
             (let ((y (nhwl-gui--ring-middle-y c)))
               (when y
                 (setq ring (cons (plist-get (plist-get c :buffer) :h)
                                  (round (nth 3 (nhwl-gui--get (plist-get c :viewport) :src))))))
               (if (and y (car y) (not (equal last-y y)))
                   (setq rebases (1+ rebases))
                 (push c cs))
               (setq last-y y)))
           (let ((layer (apply #'append (mapcar #'nhwl-gui--damage cs)))
                 (root (apply #'append (mapcar #'nhwl-gui--damage (nhwl-gui--new mark)))))
             (should (<= (nhwl-gui--area layer) (* 3 (1+ line) (1+ width))))
             (should (seq-every-p (lambda (r) (nhwl-gui--rect-in-p r mode-y0 mode-y1)) root)))))
       ;; The view starts in the middle of a ring of twice its rows and moves one line a scroll, so it can make
       ;; (ring - view) / 2 rows of moves, one scroll more rebases: a period of P scrolls, P = moves + 1.  A ring that
       ;; was there rebases at scrolls P, 2P, ... (floor (20 / P) times); one made by the first scroll draws the
       ;; middle then and rebases at 1 + P, 1 + 2P, ... (1 + floor (19 / P)), which is the larger.
       (let* ((slack (- (car ring) (cdr ring)))
              (period (1+ (/ (- slack (/ slack 2)) line))))
         (should (<= rebases (1+ (/ 19 period)))))))))

(defun nhwl-gui--ring-middle-y (c)
  "Layer commit C's view: (MIDDLE-P . Y), Y its row in the buffer, MIDDLE-P when that is the middle (where a new or
rebased ring shows its view); nil without a buffer and viewport source."
  (let ((h (plist-get (plist-get c :buffer) :h)) (src (nhwl-gui--get (plist-get c :viewport) :src)))
    (when (and h src)
      (let ((y (round (nth 1 src))))
        (cons (= y (/ (- h (round (nth 3 src))) 2)) y)))))

(ert-deftest nhwl-gui/presenter-scroll-never-writes-a-held-buffer ()
  "Under the recorder's kwin profile (it holds buffers): 200 one-line scrolls back to back, without waiting for the
compositor, in a layered window → no buffer, the layer's ring buffers included, is written while the compositor holds
it."
  (skip-unless (nhwl-gui--layers-p))
  (nhwl-gui--with-layer-buffer
   (lambda ()
     (let ((since (plist-get (gui-runner-recorder '(:cmd "held_buffer_writes" :since 1000000000)) :next)))
       (should (= (nhwl-gui--scroll-promote) 1))
       (dotimes (i 200)
         (if (< (% i 40) 25) (scroll-up 1) (scroll-down 1))
         (redisplay t))
       (gui-runner-wait-settled)
       (should-not (plist-get (gui-runner-recorder (list :cmd "held_buffer_writes" :since since)) :writes))
       (should (equal (nhwl-gui--grab-matches-snapshot) '(0 . 0)))))))

(ert-deftest nhwl-gui/live-scroll-layers ()
  "Window layers on a real compositor (sway and mutter at 1 and 1.25, KWin at the gate): scrolls each way, a split, a
quiet period and scrolling again.  The trace rules (errors, acks, sync, viewport, held-reuse) judge what was sent."
  :tags '(gnome-smoke)
  (skip-unless (and (nhwl-gui--live-p) (not (getenv "GLITNIR_GUI_RECORDER")) (nhwl-gui--nhwl-frames)
                    (boundp 'nhwl-scroll-layers) nhwl-scroll-layers))
  (let ((buf (generate-new-buffer " *nhwl-live-layers*")))
    (unwind-protect
        (progn
          (delete-other-windows)
          (switch-to-buffer buf)
          (dotimes (i 400) (insert (format "line %03d of the live window-layer row\n" i)))
          (goto-char (point-min))
          (nhwl-gui--live-settle 0.5)
          (dotimes (i 60)
            (if (< (% i 20) 12) (scroll-up 1) (scroll-down 1))
            (nhwl-gui--live-settle 0.02))
          (split-window-below)
          (nhwl-gui--live-settle 0.3)
          (dotimes (_ 20) (scroll-up 1) (nhwl-gui--live-settle 0.02))
          (delete-other-windows)
          (nhwl-gui--live-settle 2.5)
          (dotimes (_ 20) (scroll-down 1) (nhwl-gui--live-settle 0.02))
          (nhwl-gui--live-settle 0.5))
      (kill-buffer buf)
      (delete-other-windows))))

;; Reconnect while presenting.  The compositor restarts while a window scrolls on a layer of its own and the user
;; types: the presenter sends nothing from the loss until each frame's new configure, the layer goes at the loss, and
;; once `reconnected' has run the frame shows its backing again; the next full-window scroll makes a layer on the new
;; connection.  The restart helpers are the frames-survive rows' (nhwl-gui-w2-tests.el, loaded with `--files'
;; 'nhwl-gui*-tests.el').  These rows end the first connection, so they run last; the gate runs them with `--daemon'
;; and the trace rules `errors,unconfigured,sync,held-reuse' on the second connection.

(declare-function nhwl-gui--restart "nhwl-gui-w2-tests")
(declare-function nhwl-gui--reconnected "nhwl-gui-w2-tests")
(declare-function nhwl-gui--connection-note "nhwl-gui-w2-tests")
(defvar nhwl-gui--connection-log)

(defun nhwl-gui--scroll-and-type (n)
  "Scroll the selected window N times (twelve up, eight down, over), typing a character every fifth time, reading
events between steps."
  (dotimes (i n)
    (if (< (% i 20) 12) (scroll-up 1) (scroll-down 1))
    (when (zerop (% i 5)) (insert "k"))
    (redisplay t)
    (nhwl-gui--pump 0.02)))

(ert-deftest nhwl-gui/compositor-restart-while-scrolling ()
  "The recorder restarts (keep-socket, down 300 ms) while a window with a layer of its own scrolls and the user types:
the frame stays, the hook runs `reconnecting' then `reconnected', the composite equals the backing after it, and a
full-window scroll makes a layer again, with the composite still equal to the backing."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--layers-p) (fboundp 'nhwl-gui--restart) (fboundp 'nhwl-gui--reconnected)))
  (nhwl-gui--with-layer-buffer
   (lambda ()
     (let ((f (selected-frame)) (timer nil))
       (should (= (nhwl-gui--scroll-promote) 1))
       (setq nhwl-gui--connection-log nil)
       (add-hook 'nhwl-display-connection-functions #'nhwl-gui--connection-note)
       (unwind-protect
           (progn
             (setq timer (run-at-time 0.2 nil (lambda () (nhwl-gui--restart "keep-socket" :down_ms 300))))
             (nhwl-gui--scroll-and-type 60)
             (should (nhwl-gui--reconnected))
             (should (equal nhwl-gui--connection-log '(reconnecting reconnected))))
         (when timer (cancel-timer timer))
         (remove-hook 'nhwl-display-connection-functions #'nhwl-gui--connection-note))
       (should (frame-live-p f))
       (redisplay t)
       (gui-runner-wait-settled)
       (should (equal (nhwl-gui--grab-matches-snapshot) '(0 . 0)))
       (should (= (nhwl-gui--scroll-promote) 1))
       (gui-runner-wait-settled)
       (should (equal (nhwl-gui--grab-matches-snapshot) '(0 . 0)))))))

(ert-deftest nhwl-gui/live-kwin-restart-while-scrolling ()
  "KWin restarts (its wrapper keeps the socket) while a window scrolls on a layer and the user types: Nidhogg
reconnects and goes on scrolling.  The trace rules errors, unconfigured, sync and held-reuse judge the second
connection."
  :tags '(runs-last)
  (skip-unless (and (nhwl-gui--live-p) (not (getenv "GLITNIR_GUI_RECORDER")) (nhwl-gui--nhwl-frames)
                    (equal (getenv "GLITNIR_GUI_COMPOSITOR") "kwin") (gui-runner-restart-available-p)
                    (fboundp 'nhwl-gui--reconnected)))
  (let ((buf (generate-new-buffer " *nhwl-live-restart*")) (f (selected-frame)))
    (setq nhwl-gui--connection-log nil)
    (add-hook 'nhwl-display-connection-functions #'nhwl-gui--connection-note)
    (unwind-protect
        (progn
          (delete-other-windows)
          (switch-to-buffer buf)
          (dotimes (i 400) (insert (format "line %03d of the live restart row\n" i)))
          (goto-char (point-min))
          (nhwl-gui--live-settle 0.5)
          (nhwl-gui--scroll-and-type 20)
          ;; the script stops KWin and starts it again while the window scrolls
          (start-process "nhwl-kwin-restart" nil (getenv "NHWL_GUI_RESTART"))
          (nhwl-gui--scroll-and-type 100)
          (should (nhwl-gui--reconnected 20))
          (nhwl-gui--scroll-and-type 60)
          (nhwl-gui--live-settle 0.5)
          (should (frame-live-p f)))
      (remove-hook 'nhwl-display-connection-functions #'nhwl-gui--connection-note)
      (kill-buffer buf)
      (delete-other-windows))))

(provide 'nhwl-gui-w3-tests)
;;; nhwl-gui-w3-tests.el ends here
