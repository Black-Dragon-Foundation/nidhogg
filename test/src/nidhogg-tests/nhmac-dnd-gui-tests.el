;;; nhmac-dnd-gui-tests.el --- Naglfar's drag and drop rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).  A drop is driven in-process:
;; a test hook calls the view's `draggingEntered:', `draggingUpdated:' and `performDragOperation:' with dragging info
;; of its own, as AppKit does for another application's drag.  A drag out needs a window-server dragging session.

(require 'ert)
(declare-function glitnir-gui--say "gui-runner" (fmt &rest args)) ; the lane's log, not *Messages*

(defun nhmac-dnd--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--query)))

;; Task 29: drag and drop in and out
(defun nhmac-dnd--drop (file payload)
  "Drop PAYLOAD (a file's name when FILE) on the selected frame and handle the drop event as the command loop does."
  (nhmac--call 'test-drop (selected-frame) (if file 1 0) payload)
  (let ((deadline (+ (float-time) 5)) ev drop)
    (while (and (not drop) (< (float-time) deadline))
      (setq ev (read-event nil nil 1))
      (when (and (eq (car-safe ev) 'drag-n-drop) (eq (car-safe (nth 2 ev)) 'quote))
        (setq drop ev)))
    (should drop)
    (glitnir-gui--say "nhmac-dnd--drop: %S; frame %S, window edges %S" drop
             (list (frame-pixel-width) (frame-pixel-height)) (window-pixel-edges))
    (funcall (key-binding [drag-n-drop]) drop)))

(ert-deftest nhmac-dnd-drop-file ()
  "A file dropped on a frame arrives as a `drag-n-drop' event, exactly, and is visited."
  (skip-unless (nhmac-dnd--naglfar-p))
  (let ((file (make-temp-file "nhmac-drop" nil ".txt" "dropped\n")))
    (save-window-excursion
      (nhmac-dnd--drop t file)
      ;; by its true name too (/private/var/... for a temporary file under /var)
      (let ((b (find-buffer-visiting file)))
        (should b)
        (kill-buffer b)))))

(ert-deftest nhmac-dnd-drop-text ()
  "Text dropped on a frame arrives as a `drag-n-drop' event and is inserted."
  (skip-unless (nhmac-dnd--naglfar-p))
  (with-temp-buffer
    (let ((win (selected-window)) (old (window-buffer)))
      (set-window-buffer win (current-buffer))
      (unwind-protect
          (progn
            (nhmac-dnd--drop nil "nhmac dropped text")
            (should (string-match-p "nhmac dropped text" (buffer-string))))
        (set-window-buffer win old)))))

(ert-deftest nhmac-dnd-drag-out ()
  "`x-begin-drag' to a test drop target in a second window; the nested loop holds `:overlapping-takes' at 0."
  (skip-unless (nhmac-dnd--naglfar-p))
  (ert-skip "pending: a window-server dragging session (no synthetic one on the hosted runner)"))

;;; nhmac-dnd-gui-tests.el ends here
