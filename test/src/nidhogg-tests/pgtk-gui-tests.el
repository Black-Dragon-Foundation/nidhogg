;;; pgtk-gui-tests.el --- GUI tests for Nidhogg's pgtk frame handling  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh on a headless mutter (gui-runner.el loads every *-gui-tests.el).

(require 'ert)
(require 'benchmark)
(require 'cl-lib)

(ert-deftest nidhogg-gui/hide-returns-before-the-timeout ()
  (let ((child (make-frame `((parent-frame . ,(selected-frame)) (width . 20) (height . 5)
                             (visibility . t)))))
    (redisplay t)
    (sit-for 0.2)
    ;; The timeout raised far above any scheduling noise: a hide that waits it out takes 2 s, one that returns once
    ;; the pending events are handled takes milliseconds (a fixed 30 ms bound failed at scale 2 on CI: 36 ms).
    (let ((elapsed (let ((pgtk-wait-for-event-timeout 2.0))
                     (benchmark-elapse (make-frame-invisible child)))))
      (princ (format "hide-time: %.4f s\n" elapsed) #'external-debugging-output)
      (should (< elapsed 1.0)))
    (should-not (frame-visible-p child))
    (delete-frame child)))

(ert-deftest nidhogg-gui/show-then-hide-stays-hidden ()
  "The race the wait in `pgtk_make_frame_invisible' exists for."
  :tags '(gnome-smoke)
  (let ((child (make-frame `((parent-frame . ,(selected-frame)) (width . 20) (height . 5)
                             (visibility . nil)))))
    (make-frame-visible child)
    (make-frame-invisible child)
    (sit-for 0.3)
    (should-not (frame-visible-p child))
    (delete-frame child)))

(ert-deftest nidhogg-gui/hiding-a-root-frame-still-works ()
  (let ((root (make-frame '((width . 40) (height . 10) (visibility . t)))))
    (redisplay t)
    (sit-for 0.2)
    (make-frame-invisible root)
    (sit-for 0.3)
    (should-not (frame-visible-p root))
    (delete-frame root)))

(defun pgtk-gui--socket-owner (path)
  "The process name (`/proc/PID/comm') of the process listening on the Unix socket PATH, or nil."
  (let (inode)
    (with-temp-buffer
      (insert-file-contents "/proc/net/unix")
      (goto-char (point-min))
      (when (re-search-forward (concat " \\([0-9]+\\) " (regexp-quote path) "$") nil t)
        (setq inode (match-string 1))))
    (when inode
      (cl-loop for pid in (directory-files "/proc" nil "\\`[0-9]+\\'")
               for found = (ignore-errors
                             (cl-loop for fd in (directory-files (format "/proc/%s/fd" pid) t "[0-9]")
                                      thereis (equal (file-symlink-p fd) (format "socket:[%s]" inode))))
               when found
               return (string-trim (with-temp-buffer
                                     (insert-file-contents (format "/proc/%s/comm" pid))
                                     (buffer-string)))))))

(ert-deftest pgtk-gui/compositor-is-the-asked-one ()
  "The process listening on WAYLAND_DISPLAY's socket is the compositor gui.sh was asked for."
  :tags '(gnome-smoke)
  (let* ((asked (getenv "GLITNIR_GUI_COMPOSITOR"))
         (sock (expand-file-name (getenv "WAYLAND_DISPLAY") (getenv "XDG_RUNTIME_DIR"))))
    (skip-unless asked)
    (should (equal (pgtk-gui--socket-owner sock)
                   (pcase asked ("sway" "sway") ("mutter" "mutter") ("kwin" "kwin_wayland") ("labwc" "labwc") ("recorder" "nh-recorder"))))))

(ert-deftest pgtk-gui/grab-matches-snapshot ()
  "The compositor's grab and Glitnir's own snapshot of a fullscreen frame agree (scale 1)."
  :tags '(gnome-smoke)
  (skip-unless (= (frame-scale-factor) 1))
  (switch-to-buffer "*scratch*")
  (erase-buffer)
  (insert (propertize (make-string 400 ?\s) 'face '(:background "#c03030"))
          "\n"
          (propertize (make-string 400 ?\s) 'face '(:background "#3050c0"))
          "\nsome text\n")
  (goto-char (point-min))
  (redisplay t)
  ;; Glitnir's snapshot first, then wait for the compositor to show that frame, then grab (a grab right after redisplay
  ;; can race Glitnir's paint).
  (let* ((snap (glitnir-snapshot nil 'raw))
         (full (progn (gui-runner-wait-settled) (gui-runner-screen-grab)))
         ;; the snapshot is the frame's drawing area; the grab also holds the GTK widgets above it (the menu bar)
         (top (- (nth 1 full) (nth 1 snap)))
         (grab (progn (should (= (nth 0 full) (nth 0 snap)))
                      (should (>= top 0))
                      (list (nth 0 full) (nth 1 snap) (substring (nth 2 full) (* 4 (nth 0 full) top)))))
         (diff (gui-runner-pixel-diff grab snap))
         (total (* (nth 0 snap) (nth 1 snap))))
    (should (<= (car diff) 2))
    (should (<= (cdr diff) (/ total 1000)))))

(ert-deftest pgtk-gui/windowed-frame-maps-on-gnome ()
  "A 600x400 frame that is not fullscreen maps and is visible on mutter (it checks no decoration)."
  :tags '(gnome-smoke)
  (skip-unless (equal (getenv "GLITNIR_GUI_COMPOSITOR") "mutter"))
  (let ((f (make-frame '((width . 75) (height . 25) (visibility . t) (fullscreen . nil)))))
    (unwind-protect
        (progn
          (redisplay t)
          (sit-for 0.3)
          (should (frame-live-p f))
          (should (frame-visible-p f))
          (should (> (frame-pixel-width f) 0))
          (should (> (frame-pixel-height f) 0))
          (should-not (frame-parameter f 'fullscreen)))
      (when (frame-live-p f) (delete-frame f t)))))

;;; pgtk-gui-tests.el ends here
