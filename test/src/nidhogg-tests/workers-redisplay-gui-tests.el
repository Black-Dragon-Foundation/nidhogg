;;; workers-redisplay-gui-tests.el --- GUI tests for redisplay's line numbers in large buffers  -*- lexical-binding: t -*-
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

;; Run by nidhogg/test/gui.sh.  `display-line-numbers' in a window that shows no mode line: nothing fills the
;; window's base-line cache (line-number-mode's `%l' does), so before Nidhogg's fix every display iterator of a
;; redisplay counted the lines from the buffer's start.  A window split in a 60,000-line buffer redisplayed in
;; 50 ms that way (drgnx's perf lab, compile-flood).  The rows count those counts
;; (`nidhogg-workers--test-line-number-counts') and check, through a `%l' mode line that reads the same cache,
;; that the numbers it keeps are right.  Handshake: every step is a synchronous `redisplay', nothing runs between.

;;; Code:

(require 'ert)

(defun workers-redisplay-gui--buffer (lines)
  "A buffer of LINES numbered lines, shown with line numbers in the selected window, which has no mode line."
  (let ((b (get-buffer-create " *wr-lnum*")))
    (with-current-buffer b
      (erase-buffer)
      (dotimes (i lines) (insert (format "line %06d %s\n" (1+ i) (make-string 40 ?x))))
      (setq-local display-line-numbers t)
      (goto-char (point-min))
      (forward-line (/ lines 2)))
    (delete-other-windows)
    (set-window-buffer nil b)
    (set-window-point nil (with-current-buffer b (point)))
    (set-window-parameter nil 'mode-line-format 'none)
    (with-current-buffer b (recenter))
    (redisplay t)
    b))

(defun workers-redisplay-gui--mode-line-l (window)
  "WINDOW's `%l', as a number, read through the base-line cache the line numbers keep."
  (with-current-buffer (window-buffer window)
    (string-to-number (format-mode-line "%l" nil window))))

(ert-deftest workers-redisplay-gui/later-redisplays-count-nothing-from-start ()
  ;; The first redisplay counted the lines from the start (once) and kept the result; a full redraw and the
  ;; compile-flood shape (a window opens below, the numbered window shrinks) count from there, never from the start.
  (let ((b (workers-redisplay-gui--buffer 60000)) (other (get-buffer-create " *wr-other*")) w)
    (unwind-protect
        (progn
          (nidhogg-workers--test-line-number-counts t)
          (redraw-frame)
          (redisplay t)
          (setq w (split-window nil -8 'below))
          (set-window-buffer w other)
          (redisplay t)
          (should (= (nidhogg-workers--test-line-number-counts t) 0))
          ;; the kept base line gives the right number
          (should (= (workers-redisplay-gui--mode-line-l (selected-window))
                     (with-current-buffer b (line-number-at-pos (window-point))))))
      (delete-other-windows)
      (set-window-parameter nil 'mode-line-format nil)
      (kill-buffer b)
      (kill-buffer other))))

(ert-deftest workers-redisplay-gui/narrowed-widened-numbers-keep-no-base ()
  ;; `display-line-numbers-widen' numbers from the buffer's real start; the cache holds the accessible portion's
  ;; numbers (what `%l' shows), so a widened count must not land in it.
  (let ((b (workers-redisplay-gui--buffer 20000)) w)
    (unwind-protect
        (progn
          (with-current-buffer b
            (narrow-to-region (line-beginning-position -5000) (point-max))
            (setq-local display-line-numbers-widen t))
          (redisplay t)
          (setq w (split-window nil -8 'below))
          (redisplay t)
          (should (= (workers-redisplay-gui--mode-line-l (selected-window))
                     (with-current-buffer b (line-number-at-pos (window-point))))))
      (delete-other-windows)
      (set-window-parameter nil 'mode-line-format nil)
      (kill-buffer b))))

;;; workers-redisplay-gui-tests.el ends here
