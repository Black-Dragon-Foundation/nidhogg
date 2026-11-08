;;; nhmac-dnd-gui-tests.el --- Naglfar's drag and drop rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).  A drop is driven in-process:
;; a test hook calls the view's `draggingEntered:', `draggingUpdated:' and `performDragOperation:' with dragging info
;; of its own, as AppKit does for another application's drag.  A drag out needs a window-server dragging session.

(require 'ert)

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
    (funcall (key-binding [drag-n-drop]) drop)))

(ert-deftest nhmac-dnd-drop-file ()
  "A file dropped on a frame arrives as a `drag-n-drop' event, exactly, and is visited."
  (skip-unless (nhmac-dnd--naglfar-p))
  (let ((file (make-temp-file "nhmac-drop" nil ".txt" "dropped\n")))
    (save-window-excursion
      (nhmac-dnd--drop t file)
      (let ((b (get-file-buffer file)))
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
