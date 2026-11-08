;;; nhwl-gui-w4-tests.el --- GUI rows for the native shell's pointer, through the command loop  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/gui.sh with the base rows (nhwl-gui-tests.el, loaded first: its helpers are used here) on
;; `--compositor recorder'.  The recorder's pointer goes through the shell and `nhwl_click_apply', as a real one does,
;; and Emacs's own command loop runs the commands: area prefixes, transient maps and `pre-command-hook' as for the user.

(require 'ert)
(require 'nhwl-gui-tests)

(defun nhwl-gui--command-loop (steps &optional seconds)
  "Run STEPS under Emacs's command loop (a `recursive-edit'); t when all ran, nil when SECONDS (default 10) passed first.
Each step is (FN . PRED): FN sends input to the recorder, then commands run as typed until PRED holds, then the next
step starts.  A command's error is the command loop's, as for the user: it is in *Messages*."
  (let* ((deadline (+ (float-time) (or seconds 10)))
         (pending steps) (current nil) (done nil) (timer nil))
    (setq timer (run-at-time
                 0 0.02
                 (lambda ()
                   (cond
                    ((> (float-time) deadline) (cancel-timer timer) (exit-recursive-edit))
                    ((and current (not (funcall (cdr current)))))
                    (pending (setq current (pop pending)) (funcall (car current)))
                    (t (setq done t) (cancel-timer timer) (exit-recursive-edit))))))
    (unwind-protect (let ((debug-on-error nil)) (recursive-edit)) (cancel-timer timer))
    done))

(defun nhwl-gui--frame-origin (f)
  "Where F's native area starts in its toplevel surface, (X Y) in device pixels: past its margins and strip.
From F's newest buffer (its size is F's plus less than 100 logical pixels), as `nhwl-gui--click-line' finds it."
  (let* ((s (frame-scale-factor f))
         (b (plist-get (car (last (seq-filter
                                   (lambda (c)
                                     (let ((b (plist-get c :buffer)))
                                       (and (<= 0 (- (plist-get b :w) (frame-pixel-width f)) (* 100 s))
                                            (<= 0 (- (plist-get b :h) (frame-pixel-height f)) (* 100 s)))))
                                   (nhwl-gui--toplevel-commits))))
                       :buffer))
         (offx (/ (- (plist-get b :w) (frame-pixel-width f)) 2)))
    (list offx (- (plist-get b :h) (frame-pixel-height f) offx))))

(defun nhwl-gui--ptr-point (f x y)
  "Put the pointer at F's native pixel X, Y, on the nearest whole logical pixel; return that native (X . Y)."
  (let* ((s (frame-scale-factor f)) (o (nhwl-gui--frame-origin f))
         (lx (round (/ (+ x (nth 0 o)) s))) (ly (round (/ (+ y (nth 1 o)) s))))
    (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
    (gui-runner-recorder (list :cmd "pointer" :pos (vector lx ly)))
    (cons (- (round (* lx s)) (nth 0 o)) (- (round (* ly s)) (nth 1 o)))))

(defun nhwl-gui--ptr-button (state)
  (gui-runner-recorder (list :cmd "button" :button 272 :state state)))

(defun nhwl-gui--ptr-frame (params &optional setup)
  "A new frame with PARAMS, its own buffer, SETUP (when given) called in it, drawn and shown by the recorder."
  (let ((f (make-frame (append params '((width . 80) (height . 20) (vertical-scroll-bars . nil)
                                        (menu-bar-lines . 0) (tool-bar-lines . 0))))))
    (with-selected-frame f
      (switch-to-buffer (get-buffer-create " *nhwl-ptr*"))
      (when setup (funcall setup)))
    (select-frame-set-input-focus f)
    (redisplay t)
    (nhwl-gui--wait (lambda () (nhwl-gui--pump) (frame-visible-p f)) 5)
    (gui-runner-wait-settled)
    (nhwl-gui--pump)
    (redisplay t)
    f))

(defun nhwl-gui--messages-since (pos)
  (with-current-buffer "*Messages*" (buffer-substring-no-properties (min pos (point-max)) (point-max))))

(defun nhwl-gui--messages-end ()
  (with-current-buffer "*Messages*" (point-max)))

;;; The tab bar: a press on an item carries the item, so its binding runs (drgnx's title row is tab-bar items)

(defvar nhwl-gui--ptr-hits 0)
(defvar nhwl-gui--ptr-items nil)
(defun nhwl-gui--ptr-tab-items () nhwl-gui--ptr-items)

(defun nhwl-gui--tab-frame (items)
  "A new frame whose tab bar shows only ITEMS (`tab-bar-format' items), the recorder's newest toplevel."
  (setq nhwl-gui--ptr-items items)
  (nhwl-gui--ptr-frame '((tab-bar-lines . 1))))

(defun nhwl-gui--tab-item-point (f)
  "Native X, Y in the middle of the first tab-bar item's first columns of F."
  (let ((top (nth 1 (window-pixel-edges (frame-root-window f)))))
    (list (* 3 (frame-char-width f)) (- top (/ (tab-bar-height f t) 2)))))

(ert-deftest nhwl-gui/tab-bar-click-runs-the-item ()
  (skip-unless (nhwl-gui--recorder-p))
  (require 'tab-bar)
  (let* ((tab-bar-format '(nhwl-gui--ptr-tab-items))
         (f (nhwl-gui--tab-frame
             `((nhwl-gui-hit menu-item " HIT HERE " ,(lambda () (interactive) (setq nhwl-gui--ptr-hits (1+ nhwl-gui--ptr-hits)))
                             :help "a row's item"))))
         (m (nhwl-gui--messages-end)))
    (setq nhwl-gui--ptr-hits 0)
    (unwind-protect
        (let ((p (nhwl-gui--tab-item-point f)))
          (should (> (tab-bar-height f t) 0))
          (should (nhwl-gui--command-loop
                   `((,(lambda () (nhwl-gui--ptr-point f (nth 0 p) (nth 1 p))
                         (nhwl-gui--ptr-button "press") (nhwl-gui--ptr-button "release"))
                      . ,(lambda () (> nhwl-gui--ptr-hits 0))))
                   5))
          (should (= nhwl-gui--ptr-hits 1)))
      (unless (> nhwl-gui--ptr-hits 0)
        (message "tab-bar row: messages %S" (nhwl-gui--messages-since m)))
      (delete-frame f))))

(defvar nhwl-gui--rd-keys nil)
(defun nhwl-gui--rd-note-keys () (push (this-command-keys-vector) nhwl-gui--rd-keys))

(ert-deftest nhwl-gui/tab-bar-item-drags-the-frame ()
  "drgnx's title row: an item bound to `mouse-drag-frame-move' asks the compositor to move the frame."
  (skip-unless (nhwl-gui--recorder-p))
  (require 'tab-bar)
  (let* ((tab-bar-format '(nhwl-gui--ptr-tab-items))
         (f (nhwl-gui--tab-frame
             `((nhwl-gui-gap menu-item "          "
                               ;; `mouse-drag-frame-move' is a function, not a command
                               ,(lambda (e) (interactive "e") (mouse-drag-frame-move e))
                               :help "Drag to move the window"))))
         (mark (plist-get (gui-runner-recorder '(:cmd "input_log" :since 0)) :next))
         (m (nhwl-gui--messages-end)))
    (setq nhwl-gui--rd-keys nil)
    (add-hook 'pre-command-hook #'nhwl-gui--rd-note-keys)
    (unwind-protect
        (let ((p (nhwl-gui--tab-item-point f)))
          (unless (nhwl-gui--command-loop
                   `((,(lambda () (nhwl-gui--ptr-point f (nth 0 p) (nth 1 p)) (nhwl-gui--ptr-button "press"))
                      . ,(lambda () (nhwl-gui--input-since mark "xdg_toplevel.move"))))
                   5)
            (ert-fail (list :keys nhwl-gui--rd-keys :messages (nhwl-gui--messages-since m)
                            :log (mapcar (lambda (e) (plist-get e :req))
                                         (plist-get (gui-runner-recorder (list :cmd "input_log" :since mark))
                                                    :entries))))))
      (remove-hook 'pre-command-hook #'nhwl-gui--rd-note-keys)
      (nhwl-gui--ptr-button "release")
      (nhwl-gui--pump)
      (delete-frame f))))

;;; `mouse-drag-line': a divider drag reads the pointer through `mouse-absolute-pixel-position'

(defun nhwl-gui--drag-map ()
  "The transient map of the `mouse-drag-line' drag under way (it has the dividers as prefixes of itself), or nil."
  (and (consp overriding-terminal-local-map)
       (seq-find (lambda (m) (and (keymapp m) (eq (lookup-key m [right-divider]) m)))
                 (cdr overriding-terminal-local-map))))

(defun nhwl-gui--split-frame ()
  "A new frame split side by side, 8-pixel right dividers; its left window."
  (let ((f (nhwl-gui--ptr-frame '((right-divider-width . 8) (bottom-divider-width . 0))
                                (lambda () (split-window-right)))))
    (frame-first-window f)))

(defun nhwl-gui--divider-press (f left)
  "Steps that press the divider right of LEFT in its middle row; they end once the drag runs.  Return (STEP . X)
with X the native x the pointer is at."
  (let* ((e (window-pixel-edges left)) (x (- (nth 2 e) 4)) (y (/ (+ (nth 1 e) (nth 3 e)) 2)) (at nil))
    (cons (cons (lambda () (setq at (nhwl-gui--ptr-point f x y)) (nhwl-gui--ptr-button "press"))
                #'nhwl-gui--drag-map)
          (lambda () at))))

(ert-deftest nhwl-gui/divider-drag-resizes-by-the-pointer ()
  "A drag of the divider by N logical pixels makes the window on its left N device pixels wider."
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((window-resize-pixelwise t)
         (left (nhwl-gui--split-frame)) (f (window-frame left))
         (s (frame-scale-factor f)) (n 40) (w0 (window-pixel-width left))
         (press (nhwl-gui--divider-press f left)) (m (nhwl-gui--messages-end)))
    (unwind-protect
        (let ((ok (nhwl-gui--command-loop
                   (list (car press)
                         (cons (lambda () (let ((at (funcall (cdr press))))
                                            (nhwl-gui--ptr-point f (+ (car at) (round (* n s))) (cdr at))))
                               (lambda () (/= (window-pixel-width left) w0)))
                         (cons (lambda () (nhwl-gui--ptr-button "release"))
                               (lambda () (not (nhwl-gui--drag-map)))))
                   5)))
          (unless (= (- (window-pixel-width left) w0) (round (* n s)))
            (ert-fail (list :width-change (- (window-pixel-width left) w0) :want (round (* n s)) :all-steps ok
                            :messages (nhwl-gui--messages-since m))))
          (should ok))
      (nhwl-gui--ptr-button "release")
      (nhwl-gui--pump)
      (delete-frame f))))

(ert-deftest nhwl-gui/divider-drag-released-over-a-fringe-ends ()
  "The divider is dragged left past the left window's minimum width, so the pointer ends over that window's right
fringe; the release there ends the drag, with no undefined key."
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((window-resize-pixelwise t)
         (left (nhwl-gui--split-frame)) (f (window-frame left))
         (w0 (window-pixel-width left))
         (fringe (nth 1 (window-fringes left)))
         ;; where the left window's right fringe is once it has its minimum width
         (fx (- (+ (nth 0 (window-pixel-edges left)) (window-min-size left t nil t)) 8 (/ fringe 2)))
         (press (nhwl-gui--divider-press f left)) (m (nhwl-gui--messages-end)))
    (should (> fringe 2))
    (unwind-protect
        (let ((ok (nhwl-gui--command-loop
                   (list (car press)
                         (cons (lambda () (nhwl-gui--ptr-point f fx (cdr (funcall (cdr press)))))
                               (lambda () (/= (window-pixel-width left) w0)))
                         (cons (lambda () (nhwl-gui--ptr-button "release"))
                               (lambda () (not (nhwl-gui--drag-map)))))
                   5))
              (msgs (nhwl-gui--messages-since m)))
          ;; the pointer was over the fringe at the release
          (let ((e (window-pixel-edges left)))
            (should (eq (posn-area (posn-at-x-y fx (/ (+ (nth 1 e) (nth 3 e)) 2) f)) 'right-fringe)))
          (unless (and ok (not (string-match-p "is undefined" msgs)) (not (nhwl-gui--drag-map)))
            (ert-fail (list :all-steps ok :drag-map (and (nhwl-gui--drag-map) t) :track-mouse track-mouse
                            :messages msgs))))
      (nhwl-gui--ptr-button "release")
      (nhwl-gui--pump)
      (delete-frame f))))

(ert-deftest nhwl-gui/fringe-drag-released-over-the-fringe-ends ()
  "A drag of the left window's right edge begun on its right fringe (`mouse-drag-line' from a fringe press, as drgnx's
card edges do) and released there: the release, `<right-fringe> <drag-mouse-1>', ends the drag, with no undefined
key, and later motion resizes nothing."
  (skip-unless (nhwl-gui--recorder-p))
  (let* ((window-resize-pixelwise t)
         (map (let ((m (make-sparse-keymap)))
                (define-key m [right-fringe down-mouse-1]
                  (lambda (e) (interactive "e") (mouse-drag-line e 'vertical)))
                m))
         (emulation-mode-map-alists (cons `((t . ,map)) emulation-mode-map-alists))
         (left (nhwl-gui--split-frame)) (f (window-frame left))
         (s (frame-scale-factor f)) (w0 (window-pixel-width left))
         (fringe (nth 1 (window-fringes left)))
         (e (window-pixel-edges left))
         (fx (- (nth 2 e) 8 (/ fringe 2))) (y (/ (+ (nth 1 e) (nth 3 e)) 2))
         (d (round (* 40 s)))
         (m (nhwl-gui--messages-end)) (w1 nil))
    (should (> fringe 2))
    (unwind-protect
        (let ((ok (nhwl-gui--command-loop
                   (list (cons (lambda () (nhwl-gui--ptr-point f fx y) (nhwl-gui--ptr-button "press"))
                               #'nhwl-gui--drag-map)
                         (cons (lambda () (nhwl-gui--ptr-point f (- fx d) y))
                               (lambda () (/= (window-pixel-width left) w0)))
                         (cons (lambda () (nhwl-gui--ptr-button "release"))
                               (lambda () (not (nhwl-gui--drag-map)))))
                   5))
              (msgs nil))
          (setq w1 (window-pixel-width left))
          ;; motion after the release resizes nothing
          (nhwl-gui--command-loop (list (cons (lambda () (nhwl-gui--ptr-point f (- fx d d) y)) #'ignore)) 0.5)
          (setq msgs (nhwl-gui--messages-since m))
          (unless (and ok (not (string-match-p "is undefined" msgs)) (not (nhwl-gui--drag-map))
                       (= (window-pixel-width left) w1))
            (ert-fail (list :all-steps ok :drag-map (and (nhwl-gui--drag-map) t) :width (list w0 w1 (window-pixel-width left))
                            :messages msgs))))
      (nhwl-gui--ptr-button "release")
      (nhwl-gui--pump)
      (delete-frame f))))

;;; A child frame's chrome while it is being made: its faces do not exist yet

(ert-deftest nhwl-gui/new-child-frame-chrome-reads-no-missing-face ()
  "The chrome is resolved before `face-set-after-frame-default' gives a new child frame its faces; the shadow colour
must not ask the frame for a face it does not have (\"Invalid face\", logged per popup)."
  (skip-unless (and (nhwl-gui--live-p) (fboundp 'glitnir--chrome-vector)))
  (let* ((m (nhwl-gui--messages-end))
         (c (make-frame `((parent-frame . ,(selected-frame)) (undecorated . t) (corner-radius . 6)
                          (shadow . (:blur 8)) (width . 20) (height . 5)))))
    (unwind-protect
        (let ((msgs (nhwl-gui--messages-since m)))
          (should-not (string-match-p "Invalid face" msgs))
          (should (equal (glitnir--shadow-color c) (glitnir--shadow-color (selected-frame)))))
      (delete-frame c))))

;;; `browse-url' on a shell frame: its window system is `pgtk', so browse-url asks for `x-gtk-launch-uri'

(ert-deftest nhwl-gui/browse-url-reaches-the-launcher ()
  "`browse-url' on a shell frame opens the URL with the desktop's launcher (xdg-open, here a script on PATH that
writes what it was given)."
  (skip-unless (and (nhwl-gui--live-p) (eq (framep (selected-frame)) 'pgtk)))
  (require 'browse-url)
  (let* ((dir (make-temp-file "nhwl-xdg" t))
         (out (expand-file-name "url" dir))
         (script (expand-file-name "xdg-open" dir))
         (url "https://example.org/nidhogg?q=1")
         (exec-path (cons dir exec-path))
         (process-environment (cons (concat "PATH=" dir ":" (getenv "PATH")) process-environment))
         (browse-url-browser-function #'browse-url-default-browser)
         (browse-url-handlers nil))
    (unwind-protect
        (progn
          (with-temp-file script
            (insert "#!/bin/sh\nprintf '%s' \"$1\" > " (shell-quote-argument out) "\n"))
          (set-file-modes script #o755)
          (should-not (condition-case err (progn (browse-url url) nil) (error err)))
          (should (gui-runner-wait (lambda () (file-exists-p out)) 5))
          (gui-runner-wait (lambda () (> (file-attribute-size (file-attributes out)) 0)) 2)
          (should (equal (with-temp-buffer (insert-file-contents out) (buffer-string)) url)))
      (delete-directory dir t))))

;;; A real pointer on real mutter: its RemoteDesktop session's virtual pointer goes through the compositor and the shell
;;; as a mouse's motion and buttons do (the `mutter' lane; gui.sh runs mutter under its own session bus)

(defconst nhwl-gui--rd "org.gnome.Mutter.RemoteDesktop")
(defvar nhwl-gui--rd-session nil)

(defun nhwl-gui--rd-p ()
  "Non-nil on the mutter lane with mutter's RemoteDesktop service on the session bus."
  (and (equal (getenv "GLITNIR_GUI_COMPOSITOR") "mutter") (nhwl-gui--live-p)
       (require 'dbus nil t) (getenv "DBUS_SESSION_BUS_ADDRESS")
       (ignore-errors (member nhwl-gui--rd (dbus-list-names :session)))))

(defun nhwl-gui--rd-call (method &rest args)
  (unless nhwl-gui--rd-session
    (let ((path (dbus-call-method :session nhwl-gui--rd "/org/gnome/Mutter/RemoteDesktop" nhwl-gui--rd "CreateSession")))
      (dbus-call-method :session nhwl-gui--rd path (concat nhwl-gui--rd ".Session") "Start")
      (setq nhwl-gui--rd-session path)))
  (apply #'dbus-call-method :session nhwl-gui--rd nhwl-gui--rd-session (concat nhwl-gui--rd ".Session") method args))

(defvar nhwl-gui--rd-at nil "Where the real pointer is on the screen, (X . Y) logical, once it was sent to the origin.")
(defvar nhwl-gui--rd-origins nil "Each frame's screen origin, (FRAME X . Y) logical, where the pointer last found it.")

(defun nhwl-gui--rd-by (dx dy)
  (nhwl-gui--rd-call "NotifyPointerMotionRelative" :double (float dx) :double (float dy))
  (setq nhwl-gui--rd-at (if (< dx -10000) (cons 0 0)
                          (and nhwl-gui--rd-at (cons (+ (car nhwl-gui--rd-at) dx) (+ (cdr nhwl-gui--rd-at) dy))))))

(defun nhwl-gui--rd-seen (f)
  "Where F saw the pointer last, (X . Y) native, once the motion in flight was read; nil when elsewhere."
  (gui-runner-observe 0.15)
  (while (read-event nil nil 0.05))
  (let ((p (mouse-pixel-position))) (and (eq (car p) f) (cadr p) (cons (cadr p) (cddr p)))))

(defun nhwl-gui--rd-move (f x y)
  "Move the real pointer to F's native pixel X, Y, within a pixel; return where F saw it, or nil.
From where the pointer last found F, else from the screen's origin (a maximized F is there), else stepping across the
screen until F sees the pointer."
  (let* ((s (frame-scale-factor f)) (tb (nhwl--title-bar f)) (m (plist-get tb :margins))
         (top (+ (or (nth 1 m) 0) (or (nth 3 (plist-get tb :strip)) 0)))
         (o (cdr (assq f nhwl-gui--rd-origins)))
         (seen nil))
    (nhwl-gui--rd-by -100000 -100000)
    (if o
        (nhwl-gui--rd-by (+ (car o) (/ x s)) (+ (cdr o) (/ y s)))
      (nhwl-gui--rd-by (/ (+ x (or (nth 0 m) 0)) s) (/ (+ y top) s)))
    (setq seen (nhwl-gui--rd-seen f))
    (dotimes (_ 12)
      (unless seen
        (nhwl-gui--rd-by 120 80)
        (setq seen (nhwl-gui--rd-seen f))))
    (dotimes (_ 4)
      (when (and seen (or (> (abs (- (car seen) x)) 1) (> (abs (- (cdr seen) y)) 1)))
        (nhwl-gui--rd-by (/ (- x (car seen)) s) (/ (- y (cdr seen)) s))
        (setq seen (nhwl-gui--rd-seen f))))
    (when (and seen nhwl-gui--rd-at)
      (setf (alist-get f nhwl-gui--rd-origins)
            (cons (- (car nhwl-gui--rd-at) (/ (car seen) s)) (- (cdr nhwl-gui--rd-at) (/ (cdr seen) s)))))
    seen))

(defun nhwl-gui--rd-click ()
  "Press and release the real pointer's first button without waiting for the replies: a synchronous D-Bus call reads
events while it waits, and `dbus-call-method' then queues a tab-bar click's `tab-bar' prefix again but loses the click
itself (`read-event' put it back in its own binding of `unread-command-events')."
  (dolist (down '(t nil))
    (dbus-call-method-asynchronously :session nhwl-gui--rd nhwl-gui--rd-session (concat nhwl-gui--rd ".Session")
                                     "NotifyPointerButton" nil :int32 272 down)))

(defvar nhwl-gui--rd-ran nil)

(defun nhwl-gui--rd-title-frame (decorated &optional floating)
  "A maximized frame (FLOATING: a floating 60x20 one, on screen at scale 2) with a title row of tab-bar items, as
drgnx draws its own: a left item, then a gap, then three controls at the right end, each recording its name in `nhwl-gui--rd-ran'.  Undecorated unless DECORATED."
  (let* ((f (make-frame `((undecorated . ,(not decorated)) (tab-bar-lines . 1) (internal-border-width . 0)
                          (menu-bar-lines . 0) (tool-bar-lines . 0) (vertical-scroll-bars . nil)))))
    (select-frame-set-input-focus f)
    (if floating
        (set-frame-size f 60 20)
      (set-frame-parameter f 'fullscreen 'maximized))
    (nhwl-gui--wait (lambda () (redisplay t) (and (frame-visible-p f) (> (frame-pixel-width f) 300)
                                                  (or floating (eq (frame-parameter f 'fullscreen) 'maximized))))
                    8)
    (gui-runner-wait-settled t)
    f))

(defvar nhwl-gui--rd-right nil)
(defun nhwl-gui--rd-right-items () nhwl-gui--rd-right)

(defun nhwl-gui--rd-command (name)
  (lambda () (interactive) (push name nhwl-gui--rd-ran)))

(defun nhwl-gui--rd-controls (f)
  "Each title-row item's middle, (NAME . X) native: the left item, then the controls right-aligned at F's right edge,
each as wide as its string in the `tab-bar' face."
  (let* ((w (frame-pixel-width f))
         (pw (lambda (s) (with-selected-frame f (string-pixel-width (propertize s 'face 'tab-bar)))))
         (x w) (right nil))
    (dolist (item (reverse nhwl-gui--rd-right))
      (let ((iw (funcall pw (nth 2 item))))
        (push (cons (nth 1 (assq (car item) '((rd-min minimize) (rd-max maximize) (rd-close close))))
                    (- x (/ iw 2)))
              right)
        (setq x (- x iw))))
    (cons (cons 'left (/ (funcall pw (nth 2 (car nhwl-gui--ptr-items))) 2)) right)))

(defun nhwl-gui--rd-title-row (decorated &optional floating)
  (let* ((tab-bar-format '(nhwl-gui--ptr-tab-items tab-bar-format-align-right nhwl-gui--rd-right-items))
         (f (progn
              (setq nhwl-gui--ptr-items `((rd-left menu-item " L " ,(nhwl-gui--rd-command 'left)))
                    nhwl-gui--rd-right `((rd-min menu-item " - " ,(nhwl-gui--rd-command 'minimize))
                                         (rd-max menu-item " o " ,(nhwl-gui--rd-command 'maximize))
                                         (rd-close menu-item " x " ,(nhwl-gui--rd-command 'close))))
              (nhwl-gui--rd-title-frame decorated floating)))
         (spec (nhwl-gui--rd-controls f)) (y nil) (fails nil))
    (force-mode-line-update t)
    (redisplay t)
    (gui-runner-wait-settled t)
    (setq y (- (nth 1 (window-pixel-edges (frame-root-window f))) (/ (tab-bar-height f t) 2)))
    (should (> (tab-bar-height f t) 0))
    ;; a control: a press in the text area arrives
    (let ((seen (nhwl-gui--rd-move f 100 (/ (frame-pixel-height f) 2))) (evs nil))
      (nhwl-gui--rd-click)
      (gui-runner-wait (lambda () (let ((e (read-event nil nil 0.05))) (when e (push e evs))
                                       (seq-find (lambda (e) (and (consp e) (eq (car e) 'mouse-1))) evs)))
                       3)
      (unless (seq-find (lambda (e) (and (consp e) (eq (car e) 'mouse-1))) evs)
        (push (list 'text-area :seen seen :events evs) fails)))
    (unwind-protect
        (pcase-dolist (`(,name . ,x) spec)
          (setq nhwl-gui--rd-ran nil)
          (setq nhwl-gui--rd-keys nil)
          (let* ((seen (nhwl-gui--rd-move f x y))
                 (ok (and seen
                          (nhwl-gui--command-loop
                           `((,(lambda ()
                                 (add-hook 'pre-command-hook #'nhwl-gui--rd-note-keys)
                                 (nhwl-gui--rd-click))
                              . ,(lambda () (memq name nhwl-gui--rd-ran))))
                           3))))
            (unless ok
              ;; what arrives at all: the raw events of one more click
              (let ((evs nil))
                (nhwl-gui--rd-click)
                (gui-runner-wait (lambda () (let ((e (read-event nil nil 0.05))) (when e (push e evs)) nil)) 1)
                (setq seen (list seen :raw (nreverse evs) :fullscreen (frame-parameter f 'fullscreen)
                                 :chrome (nhwl-toplevel-chrome-p f))))
              (push (list name :at x y :seen seen :keys nhwl-gui--rd-keys :title-bar (nhwl--title-bar f)
                          :tab-bar-height (tab-bar-height f t) :size (cons (frame-pixel-width f) (frame-pixel-height f)))
                    fails))))
      (remove-hook 'pre-command-hook #'nhwl-gui--rd-note-keys)
      (delete-frame f))
    (when fails (ert-fail (nreverse fails)))))

(ert-deftest nhwl-gui/live-title-row-real-click-undecorated ()
  "drgnx's drawn title: an undecorated, maximized frame whose tab bar holds the controls.  A real press and release
on each control (mutter's virtual pointer) runs it."
  (skip-unless (nhwl-gui--rd-p))
  (nhwl-gui--rd-title-row nil))

(ert-deftest nhwl-gui/live-title-row-real-click-decorated ()
  (skip-unless (nhwl-gui--rd-p))
  (nhwl-gui--rd-title-row t))

(ert-deftest nhwl-gui/live-title-row-real-click-undecorated-floating ()
  "As `nhwl-gui/live-title-row-real-click-undecorated', on a floating frame: its resize margins and shadow room
around the window take no press inside it."
  (skip-unless (nhwl-gui--rd-p))
  (nhwl-gui--rd-title-row nil t))

;;; The whole window resized from its margins by a real pointer

(defun nhwl-gui--rd-button (down)
  "Press (DOWN) or release the real pointer's first button, not waiting for the reply (`nhwl-gui--rd-click')."
  (dbus-call-method-asynchronously :session nhwl-gui--rd nhwl-gui--rd-session (concat nhwl-gui--rd ".Session")
                                   "NotifyPointerButton" nil :int32 272 down))

(ert-deftest nhwl-gui/live-undecorated-margins-resize ()
  "A floating undecorated frame on mutter (no server-side decorations): a real drag from 4 logical pixels outside
each edge and corner resizes the window by that edge."
  (skip-unless (nhwl-gui--rd-p))
  (let ((f (nhwl-gui--rd-title-frame nil t)) (fails nil))
    (unwind-protect
        (progn
          (skip-unless (nhwl-toplevel-chrome-p f))
          (pcase-dolist (`(,name ,ax ,ay ,ox ,oy) '((right 1 0.5 1 0) (left 0 0.5 -1 0) (bottom 0.5 1 0 1)
                                                    (top 0.5 0 0 -1) (bottom_right 1 1 1 1) (top_left 0 0 -1 -1)
                                                    (top_right 1 0 1 -1) (bottom_left 0 1 -1 1)))
            (let* ((w (frame-pixel-width f)) (h (frame-pixel-height f)) (s (frame-scale-factor f))
                   ;; found at the middle (a left or top drag moved the window), then moved by its known size
                   (seen (nhwl-gui--rd-move f (/ w 2) (/ h 2)))
                   (ok nil))
              (when seen
                (nhwl-gui--rd-by (+ (/ (- (* ax w) (car seen)) (float s)) (* 4 ox))
                                 (+ (/ (- (* ay h) (cdr seen)) (float s)) (* 4 oy)))
                (gui-runner-observe 0.2)
                (nhwl-gui--rd-button t)
                (gui-runner-observe 0.3)
                (dotimes (_ 4)
                  (nhwl-gui--rd-by (* 10 ox) (* 10 oy))
                  (gui-runner-observe 0.1))
                (nhwl-gui--rd-button nil)
                (setq ok (gui-runner-wait
                          (lambda ()
                            (redisplay t)
                            (and (or (= ox 0) (>= (- (frame-pixel-width f) w) (* 30 s)))
                                 (or (= oy 0) (>= (- (frame-pixel-height f) h) (* 30 s)))
                                 ;; the other side keeps its size, to a pixel of rounding
                                 (or (/= ox 0) (<= (abs (- (frame-pixel-width f) w)) s))
                                 (or (/= oy 0) (<= (abs (- (frame-pixel-height f) h)) s))))
                          3)))
              (unless ok
                (push (list name :seen seen :before (cons w h)
                            :after (cons (frame-pixel-width f) (frame-pixel-height f)) :title-bar (nhwl--title-bar f))
                      fails))
              (gui-runner-wait-settled t))))
      (delete-frame f))
    (when fails (ert-fail (nreverse fails)))))

(provide 'nhwl-gui-w4-tests)
;;; nhwl-gui-w4-tests.el ends here
