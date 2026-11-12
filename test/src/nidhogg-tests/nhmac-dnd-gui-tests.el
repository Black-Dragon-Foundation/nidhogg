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
  "A drag out of a frame through the window server (a real press, `ns-begin-drag', real moves and a release over a
second frame): the second frame gets the text as a `drag-n-drop' event, `ns-begin-drag' returns the copy, and the
pumps' scope never takes inside a take (`:overlapping-takes' stays 0)."
  (skip-unless (nhmac-dnd--naglfar-p))
  (let ((access (nhmac--query 'test-post-access)))
    (unless (eq access t)
      (ert-skip (format "the runner's grant does not allow posting events (CGPreflightPostEventAccess %S)" access))))
  (let* ((a (selected-frame))
         (b (make-frame '((width . 40) (height . 10))))
         (text "nhmac dragged text")
         (overlap (plist-get (nhmac-stats) :overlapping-takes))
         result drop)
    (unwind-protect
        (with-temp-buffer
          (set-window-buffer (frame-selected-window b) (current-buffer))
          (let ((pa (frame-position a)) (pb (frame-position b)))
            ;; side by side, B right of A
            (set-frame-position b (+ (car pa) (frame-outer-width a) 20) (cdr pa))
            (sit-for 0.5)
            (glitnir-gui--say "nhmac-dnd-drag-out: A at %S, B moved from %S to %S" pa pb (frame-position b)))
          (select-frame-set-input-focus a)
          (sit-for 0.3)
          (while (read-event nil nil 0.1))
          (nhmac--call 'test-real-drag a 40 40 b 60 40 300)
          ;; the press reaches the view first: it is the event the drag starts from
          ;; round 2: result nil, no drop.  The events read here and after the drag say which step stopped: no press
          ;; (the drag had nothing to start from), or a press and no drag session
          (let ((deadline (+ (float-time) 2)) (seen nil) ev)
            (while (and (< (float-time) deadline)
                        (not (eq (event-basic-type (setq ev (read-event nil nil 0.2))) 'mouse-1)))
              (when ev (push ev seen)))
            ;; round 4: no press.  Where the press was posted and which window (by number) is under it
            (glitnir-gui--say "nhmac-dnd-drag-out: press %S after %S; drag %s" ev (nreverse seen)
                              (nhmac--query 'test-drag-log)))
          (setq result (ns-begin-drag a `((string . ,text)) 'XdndActionCopy nil nil))
          (let ((deadline (+ (float-time) 3)) (seen nil) ev)
            (while (and (not drop) (< (float-time) deadline))
              (setq ev (read-event nil nil 0.3))
              (when ev (push (car-safe ev) seen))
              (when (eq (car-safe ev) 'drag-n-drop) (setq drop ev)))
            (glitnir-gui--say "nhmac-dnd-drag-out: after the drag, events %S" (nreverse seen)))
          (glitnir-gui--say "nhmac-dnd-drag-out: result %S, drop %S, overlapping %S -> %S" result drop overlap
                            (plist-get (nhmac-stats) :overlapping-takes))
          (should (eq result 'XdndActionCopy))
          (should drop)
          (should (eq (posn-window (event-start drop)) (frame-selected-window b)))
          (funcall (key-binding [drag-n-drop]) drop)
          (should (string-match-p text (buffer-string)))
          (should (= (plist-get (nhmac-stats) :overlapping-takes) overlap)))
      (delete-frame b))))

;;; nhmac-dnd-gui-tests.el ends here
