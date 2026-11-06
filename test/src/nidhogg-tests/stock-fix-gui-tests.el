;;; stock-fix-gui-tests.el --- GUI rows for fixes to stock Emacs code  -*- lexical-binding: t -*-

;; test-class: cond
;; test-pattern: handshake

;; Copyright (C) 2026 Black Dragon Foundation contributors

;; This file is part of Nidhogg.

;; Nidhogg is free software: you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or
;; (at your option) any later version.

;; Nidhogg is distributed in the hope that it will be useful,
;; but WITHOUT ANY WARRANTY; without even the implied warranty of
;; MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
;; GNU General Public License for more details.

;; You should have received a copy of the GNU General Public License
;; along with Nidhogg.  If not, see <https://www.gnu.org/licenses/>.

;;; Commentary:

;; Run by nidhogg/test/gui.sh.  Each row drives redisplay and the command
;; loop's own functions in order, so nothing in it depends on timing.

;;; Code:

(require 'ert)
(require 'pixel-scroll)

(defun stock-fix-gui--scroll (spacing)
  "Scroll a 400-line buffer whose `line-spacing' is SPACING; return the trace.
The trace is the `window-start' after each `next-line' that takes point
past the bottom of the window (`scroll-margin' 3), then (WINDOW-START
VSCROLL POINT) after a pixel scroll down."
  (let ((buf (generate-new-buffer " *stock-fix-scroll*"))
        trace)
    (unwind-protect
        (save-window-excursion
          (switch-to-buffer buf)
          (dotimes (i 400) (insert (format "line %d\n" i)))
          (goto-char (point-min))
          (setq-local scroll-margin 3)
          (setq-local line-spacing spacing)
          (redisplay t)
          (dotimes (_ (+ 10 (window-body-height)))
            (next-line 1)
            (redisplay t)
            (push (window-start) trace))
          (pixel-scroll-precision-scroll-down 37)
          (redisplay t)
          (push (list (window-start) (window-vscroll nil t) (point)) trace)
          (nreverse trace))
      (kill-buffer buf))))

(ert-deftest nidhogg-line-spacing-cons-scrolls-like-its-sum ()
  "Scrolling with `line-spacing' (10 . 10) goes where it goes with 20.
The sum is large against the line height, so a scroll margin that left
the spacing out would trigger its scrolls on other lines."
  (skip-unless (display-graphic-p))
  (should (equal (stock-fix-gui--scroll '(10 . 10))
                 (stock-fix-gui--scroll 20))))

(provide 'stock-fix-gui-tests)

;;; stock-fix-gui-tests.el ends here
