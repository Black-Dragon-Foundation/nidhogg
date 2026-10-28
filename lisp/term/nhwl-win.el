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

(advice-add 'mouse-drag-frame-move :around #'nhwl-win--drag-move)
(advice-add 'mouse-drag-frame-resize :around #'nhwl-win--drag-resize)

(provide 'nhwl-win)
(provide 'term/nhwl-win)

;;; nhwl-win.el ends here
