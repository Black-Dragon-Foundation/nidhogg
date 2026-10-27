;;; watchdog-gui-tests.el --- GUI tests for the stall watchdog  -*- lexical-binding: t -*-

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

;;; Code:

(require 'ert)
(require 'cl-lib)

;; Run by nidhogg/test/gui.sh (gui-runner.el loads every *-gui-tests.el).

;; Real-time rows of the stall watchdog on a pgtk frame:
;; `sit-for' is waiting, and a mode-line :eval that spins is a stall in
;; the `redisplay' phase with the :eval function innermost.

(defmacro watchdog-gui--running (&rest body)
  "Run BODY with the watchdog on at 100 ms; stop it after.
The test's own span is ended (a short `sit-for') and dropped first."
  (declare (indent 0))
  `(progn
     (nidhogg-watchdog--start 100 600000 nil)
     (unwind-protect
         (progn
           (sit-for 0.05)
           (nidhogg-watchdog--take)
           (nidhogg-watchdog--stats t)
           ,@body)
       (ignore-errors (nidhogg-watchdog--stop)))))

(ert-deftest watchdog-gui/sit-for-is-waiting ()
  (watchdog-gui--running
    (sit-for 1)
    (sit-for 0.05)
    (should (null (car (nidhogg-watchdog--take))))
    (should (> (plist-get (nidhogg-watchdog--stats) :spans) 0))))

(defvar watchdog-gui--evals 0
  "How many times the mode line ran `watchdog-gui--eval-spin'.")

(defun watchdog-gui--eval-spin ()
  "A mode-line :eval that spins for 300 ms.
Byte-compiled by the test, so its inner loop makes no frame of its own:
a sample lands there, not in `float-time', all but every time."
  (setq watchdog-gui--evals (1+ watchdog-gui--evals))
  (let ((end (+ (float-time) 0.3)))
    (while (< (float-time) end)
      (let ((i 0))
        (while (< i 100000) (setq i (1+ i))))))
  "")

(ert-deftest watchdog-gui/redisplay-phase ()
  (byte-compile 'watchdog-gui--eval-spin)
  (let ((buf (generate-new-buffer " *wd-redisplay*")))
    (unwind-protect
        (save-window-excursion
          (switch-to-buffer buf)
          (watchdog-gui--running
            (setq mode-line-format '("x" (:eval (watchdog-gui--eval-spin))))
            (force-mode-line-update)
            (setq watchdog-gui--evals 0)
            (redisplay t)
            (should (> watchdog-gui--evals 0))
            (sit-for 0.05)
            (let* ((stalls (car (nidhogg-watchdog--take)))
                   (s (car stalls)))
              (should (= (length stalls) 1))
              (should (>= (plist-get s :wall-ms) 250))
              (should (or (> (plist-get s :samples-taken) 0)
                          (> (plist-get s :missed) 0)))
              (when (> (length (plist-get s :samples)) 0)
                (should (cl-some
                         (lambda (sm)
                           (let ((f (plist-get sm :frames)))
                             (and (eq (plist-get sm :phase) 'redisplay)
                                  (> (length f) 0)
                                  (eq (aref f 0) 'watchdog-gui--eval-spin))))
                         (plist-get s :samples)))))))
      (kill-buffer buf))))

;;; watchdog-gui-tests.el ends here
