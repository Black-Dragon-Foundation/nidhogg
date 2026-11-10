;;; nidhogg-scale.el --- per-frame scale and text zoom on Nidhogg's own shells  -*- lexical-binding: t -*-

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

;; Each frame of the shell opens its fonts at its own resolution: 96 dpi
;; times its scale (`frame-scale-factor') times its root frame's text
;; zoom (`nidhogg-frame-text-zoom').  When the compositor gives a frame a
;; new scale, C sends a `config-changed-event' whose type is
;; (nidhogg-scale . DPI), DPI the resolution the frame's fonts were opened
;; at until then.  This file re-opens the frame's default font from its
;; setting, the default face's height in points, so the size a frame
;; shows is always the one a fresh open at its scale gives, never one
;; carried over by a ratio.  Then `nidhogg-frame-scale-change-functions'
;; run.
;;
;; `nidhogg-set-frame-text-zoom' sets the zoom of a frame and its child
;; frames (they share their root's) and re-opens their fonts the same way.

;;; Code:

(defvar nidhogg-frame-scale-change-functions nil
  "Functions run with a frame after its scale or text zoom changed.
They run after its fonts were re-opened at the new resolution.")

(defun nidhogg--tree-root (frame)
  "The root frame of FRAME's tree of child frames."
  (let ((f (or frame (selected-frame))))
    (while (frame-parent f) (setq f (frame-parent f)))
    f))

(defun nidhogg-frame-text-zoom (&optional frame)
  "Return the text zoom of FRAME, its root frame's; 1.0 when none is set."
  (let ((z (frame-parameter (nidhogg--tree-root frame) 'nidhogg-text-zoom)))
    (if (numberp z) (float z) 1.0)))

(defun nidhogg-scale--dpi (frame)
  "The resolution FRAME's fonts open at: 96 dpi times its scale and text zoom."
  (* 96.0 (frame-scale-factor frame) (nidhogg-frame-text-zoom frame)))

(defun nidhogg-scale--height (frame was)
  "The height in tenths of a point FRAME's default font is set to.
The height set for new frames when it is a number; else the one kept
from FRAME's last re-open while its font is still that one; else worked
out from the font's pixel size and WAS, the resolution it was opened at
\(a face's own height drifts: realising a font writes its rounded size
back)."
  (let ((h (face-attribute 'default :height t))
        (font (face-attribute 'default :font frame))
        (kept (frame-parameter frame 'nidhogg--font-height)))
    (cond ((integerp h) h)
          ((and kept (eq (cdr kept) font)) (car kept))
          ((fontp font) (round (/ (* (font-get font :size) 720.0) was))))))

(defun nidhogg-scale--reopen (frame height)
  "Re-open FRAME's default font at HEIGHT (tenths of a point) and FRAME's resolution.
The pixel size is HEIGHT/10 × dpi/72, rounded half up, as a fresh open
gives it."
  (let ((font (face-attribute 'default :font frame)))
    (when (and height (fontp font))
      ;; the font's family, weight and the rest, at its pixel size here
      (let ((spec (font-spec :size (floor (+ (/ (* (/ height 10.0) (nidhogg-scale--dpi frame)) 72.0)
                                            0.5)))))
        (dolist (key '(:family :foundry :weight :slant :width :adstyle :registry))
          (when-let* ((v (font-get font key))) (font-put spec key v)))
        (set-face-attribute 'default frame :font spec))
      (set-frame-parameter frame 'nidhogg--font-height
                           (cons height (face-attribute 'default :font frame)))))
  (run-hook-with-args 'nidhogg-frame-scale-change-functions frame))

(defun nidhogg-set-frame-text-zoom (frame zoom)
  "Set the text zoom of FRAME's root frame to ZOOM (clamped to 0.5–3).
FRAME nil means the selected frame.  The fonts of the root and its child
frames are re-opened at once."
  ;; this file is preloaded: no seq or pcase here
  (let ((root (nidhogg--tree-root frame)) heights)
    (dolist (f (frame-list))
      (when (eq (nidhogg--tree-root f) root)
        (push (cons f (nidhogg-scale--height f (nidhogg-scale--dpi f))) heights)))
    (set-frame-parameter root 'nidhogg-text-zoom (min 3.0 (max 0.5 (float zoom))))
    (dolist (fh heights)
      (nidhogg-scale--reopen (car fh) (cdr fh)))))

(defun nidhogg-scale-handle-config-changed-event (event)
  "Handle a `config-changed-event' EVENT.
Type (nidhogg-scale . DPI) re-opens the fonts of the frame EVENT names,
and a re-open never resizes the frame (its window keeps its size on the
screen); other types go to `dynamic-setting-handle-config-changed-event'."
  (interactive "e")
  (let ((type (nth 1 event)) (frame (nth 2 event)))
    (if (eq (car-safe type) 'nidhogg-scale)
        (when (frame-live-p frame)
          ;; the window keeps its size on the screen, which the compositor or AppKit
          ;; already gave it at the new scale: the text follows, never the window
          (let ((frame-inhibit-implied-resize t))
            (nidhogg-scale--reopen frame (nidhogg-scale--height frame (cdr type)))))
      (when (fboundp 'dynamic-setting-handle-config-changed-event)
        (dynamic-setting-handle-config-changed-event event)))))

(define-key special-event-map [config-changed-event]
  #'nidhogg-scale-handle-config-changed-event)

(provide 'nidhogg-scale)
(provide 'term/nidhogg-scale)
;;; nidhogg-scale.el ends here
