;;; nhwl-win.el --- undecorated frames on Nidhogg's Wayland shell  -*- lexical-binding: t -*-

;; Copyright (C) 2026 Black Dragon Foundation contributors.

;; This file is part of Nidhogg.

;; Nidhogg is free software: you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or (at
;; your option) any later version.

;; Nidhogg is distributed in the hope that it will be useful,
;; but WITHOUT ANY WARRANTY; without even the implied warranty of
;; MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
;; GNU General Public License for more details.

;; You should have received a copy of the GNU General Public License
;; along with Nidhogg.  If not, see <https://www.gnu.org/licenses/>.

;;; Commentary:

;; Wayland ignores `set-frame-position', so dragging an undecorated
;; top-level frame by its header line, mode line or internal border has
;; to ask the compositor to move or resize it.  `mouse-drag-frame-move'
;; and `mouse-drag-frame-resize' are advised to do that on a top-level
;; frame of the shell, with the primitives `nhwl-frame-begin-move',
;; `nhwl-frame-begin-resize' and `nhwl-frame-show-window-menu'.  Child
;; frames and other displays keep the stock behavior.

;;; Code:

(require 'mouse)

(declare-function nhwl-frame-begin-move "nhwlfns.c")
(declare-function nhwl-frame-begin-resize "nhwlfns.c")

(defun nhwl-win--frame (start-event)
  "The top-level shell frame START-EVENT started on, or nil."
  (let* ((window (posn-window (event-start start-event)))
         (frame (if (window-live-p window) (window-frame window) window)))
    (and (framep frame)
         (display-graphic-p frame)
         (not (frame-parent frame))
         frame)))

(defun nhwl-win--drag-move (orig start-event)
  "Around ORIG (`mouse-drag-frame-move'): the compositor moves a shell frame."
  (if-let* ((frame (nhwl-win--frame start-event)))
      (progn
        (run-hooks 'mouse-leave-buffer-hook)
        (nhwl-frame-begin-move frame))
    (funcall orig start-event)))

(defun nhwl-win--drag-resize (orig start-event part)
  "Around ORIG (`mouse-drag-frame-resize'): the compositor resizes a shell frame."
  (if-let* ((frame (nhwl-win--frame start-event)))
      (progn
        (run-hooks 'mouse-leave-buffer-hook)
        (if (eq part 'move)
            (nhwl-frame-begin-move frame)
          (nhwl-frame-begin-resize part frame)))
    (funcall orig start-event part)))

;;; The `nhwl-popup' frame parameter

(defconst nhwl-win--popup-gravities
  '("top" "bottom" "left" "right" "top-left" "bottom-left" "top-right"
    "bottom-right")
  "The `:gravity' names; their positions from 1 are the shell's gravity numbers.")

(defconst nhwl-win--popup-adjusts
  '(("slide-x" . 1) ("slide-y" . 2) ("flip-x" . 4) ("flip-y" . 8))
  "The `:adjust' names and their bits (NHG_ADJUST_*).")

(defun nhwl-popup-decode (value)
  "Check the `nhwl-popup' parameter VALUE and decode it into integers.
VALUE is (X Y W H . PLIST), X and Y and W and H device pixels of the
parent's native area, PLIST with `:gravity' (default `bottom-right'),
`:adjust' (a list of `flip-x', `flip-y', `slide-x' and `slide-y'; all
by default) and `:grab'.  Return (X Y W H GRAVITY ADJUST FLAGS): a
negative W or H is 0, GRAVITY is 1 to 8, ADJUST the bits of the names
and FLAGS 1 for a grab.  Signal an `error' for an invalid VALUE.
Called by the shell (nhwlfns.c) whenever it reads the parameter."
  (let ((tail value) (xywh nil))
    (dotimes (_ 4)
      (unless (and (consp tail) (integerp (car tail))
                   (<= -1073741823 (car tail) 1073741823))
        (signal 'error (list "Invalid `nhwl-popup' parameter" value)))
      (push (car tail) xywh)
      (setq tail (cdr tail)))
    (setq xywh (nreverse xywh))
    (let* ((gravity (plist-get tail :gravity))
           (rest (and gravity (symbolp gravity)
                      (member (symbol-name gravity) nhwl-win--popup-gravities)))
           (adjust (if (plist-member tail :adjust) 0 15)))
      (unless (or rest (null gravity))
        (signal 'error (list "Invalid `nhwl-popup' parameter" value)))
      (let ((adj (plist-get tail :adjust)))
        (while (consp adj)
          (let ((bit (and (symbolp (car adj))
                          (cdr (assoc (symbol-name (car adj))
                                      nhwl-win--popup-adjusts)))))
            (unless bit
              (signal 'error (list "Invalid `nhwl-popup' parameter" value)))
            (setq adjust (logior adjust bit)
                  adj (cdr adj)))))
      (list (nth 0 xywh) (nth 1 xywh) (max (nth 2 xywh) 0) (max (nth 3 xywh) 0)
            (if rest (- 9 (length rest)) 8)
            adjust
            (if (plist-get tail :grab) 1 0)))))

(advice-add 'mouse-drag-frame-move :around #'nhwl-win--drag-move)
(advice-add 'mouse-drag-frame-resize :around #'nhwl-win--drag-resize)

(provide 'nhwl-win)
(provide 'term/nhwl-win)

;;; nhwl-win.el ends here
