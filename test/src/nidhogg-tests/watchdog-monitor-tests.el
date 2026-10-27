;;; watchdog-monitor-tests.el --- tests for the stall watchdog (monitor)  -*- lexical-binding: t -*-

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

;; The monitor thread and the park protocol (src/nidhogg-watchdog-monitor.c,
;; src/nidhogg-watchdog-park.h).  The real-thread check of the same
;; protocol is nidhogg/test/watchdog-park-test.sh.

;;; Code:

(require 'ert)

(defun watchdog-monitor-tests--tasks ()
  "The ids of this process's tasks (threads), as strings."
  (directory-files "/proc/self/task" nil "\\`[0-9]"))

(defun watchdog-monitor-tests--task (name)
  "The id of this process's task named NAME, or nil."
  (catch 'found
    (dolist (id (watchdog-monitor-tests--tasks))
      (let ((comm (ignore-errors
                    (with-temp-buffer
                      (insert-file-contents
                       (format "/proc/self/task/%s/comm" id))
                      (string-trim (buffer-string))))))
        (when (equal comm name)
          (throw 'found id))))))

(defun watchdog-monitor-tests--sigblk (id)
  "The blocked-signal mask of task ID, as an integer."
  (with-temp-buffer
    (insert-file-contents (format "/proc/self/task/%s/status" id))
    (goto-char (point-min))
    (re-search-forward "^SigBlk:[ \t]*\\([0-9a-f]+\\)$")
    (string-to-number (match-string 1) 16)))

(ert-deftest watchdog/no-lost-wakeup ()
  "Every interleaving of the park protocol's steps loses no wake-up.
The model runs the steps of nidhogg-watchdog-park.h one at a time,
each side's stores going through its own store buffer, against a
model condition variable."
  (let ((r (nidhogg-watchdog--test-interleavings)))
    (should (>= (plist-get r :cases) 10))
    (should (= (plist-get r :lost) 0))))

(ert-deftest watchdog/monitor-start-stop ()
  "Starting makes one more task, named nh-watchdog; stopping removes it."
  (let ((before (length (watchdog-monitor-tests--tasks))))
    (should-not (watchdog-monitor-tests--task "nh-watchdog"))
    (unwind-protect
        (progn
          (nidhogg-watchdog--start 100 2000 nil)
          (should (= (1+ before) (length (watchdog-monitor-tests--tasks))))
          (should (watchdog-monitor-tests--task "nh-watchdog"))
          (nidhogg-watchdog--stop)
          (should-not (watchdog-monitor-tests--task "nh-watchdog"))
          (should (= before (length (watchdog-monitor-tests--tasks))))
          ;; A restart works.
          (nidhogg-watchdog--start 100 2000 nil)
          (should (watchdog-monitor-tests--task "nh-watchdog")))
      (nidhogg-watchdog--stop))))

(ert-deftest watchdog/monitor-blocks-every-signal ()
  "The monitor thread blocks every signal it can block.
The kernel never shows SIGKILL and SIGSTOP blocked, and glibc keeps
its two internal signals (32 and 33) out of any mask."
  (unwind-protect
      (progn
        (nidhogg-watchdog--start 100 2000 nil)
        (let ((id (watchdog-monitor-tests--task "nh-watchdog"))
              (want 0))
          (should id)
          (dotimes (i 64)
            (unless (memq (1+ i) '(9 19 32 33))
              (setq want (logior want (ash 1 i)))))
          (should (= (watchdog-monitor-tests--sigblk id) want))))
    (nidhogg-watchdog--stop)))

(ert-deftest watchdog/monitor-parks-when-idle ()
  "One idle wait of 0.3 s costs the monitor at most two wake-ups.
A wake-up is a return from the monitor's condition-variable wait
that finds no busy span: at most the end of the span before the
wait (its T/2 deadline) and none while the wait lasts."
  (unwind-protect
      (progn
        (nidhogg-watchdog--start 100 2000 nil)
        (let ((before (plist-get (nidhogg-watchdog--stats) :monitor-wakeups)))
          (nidhogg-watchdog--test-idle-wait 0.3)
          (should (<= (- (plist-get (nidhogg-watchdog--stats) :monitor-wakeups)
                         before)
                      2))))
    (nidhogg-watchdog--stop)))

(ert-deftest watchdog/monitor-never-arms-on-the-test-clock ()
  "While the virtual clock is set, a real 300 ms spin raises no request."
  (unwind-protect
      (progn
        (nidhogg-watchdog--start 100 2000 nil)
        (nidhogg-watchdog--test-clock 0)
        (let ((before (plist-get (nidhogg-watchdog--stats) :requests)))
          ;; The spin is a span of its own, between two idle waits.
          (nidhogg-watchdog--test-idle-wait 0.01)
          (let ((end (+ (float-time) 0.3)))
            (while (< (float-time) end)))
          (nidhogg-watchdog--test-idle-wait 0.01)
          (should (= (plist-get (nidhogg-watchdog--stats) :requests) before))))
    (nidhogg-watchdog--test-clock nil)
    (nidhogg-watchdog--stop)))

;;; watchdog-monitor-tests.el ends here
