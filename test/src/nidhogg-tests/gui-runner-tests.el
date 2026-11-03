;;; gui-runner-tests.el --- tests for the GUI runner's settle  -*- lexical-binding: t -*-
;; test-class: det

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

;; Batch tests of the recorder settle of gui-runner.el (`gui-runner--recorder-quiet') against a scripted presenter and
;; recorder: no display, no recorder process, no clock.  gui-runner.el is loaded for its functions; the run it would
;; start and its checks around every row are taken out again.

;;; Code:

(require 'ert)
(require 'cl-lib)
(load (expand-file-name "gui-runner.el" (file-name-directory (or load-file-name buffer-file-name))) nil t)
(remove-hook 'window-setup-hook #'glitnir-gui--start)
(advice-remove 'ert-run-test #'gui-runner--strict-commits)
(advice-remove 'ert-run-test #'gui-runner--strict-held)

(defvar gui-runner--settled-counts)
(declare-function gui-runner--recorder-quiet "gui-runner")
(declare-function gui-runner--strict-commits "gui-runner")
(declare-function gui-runner--strict-held "gui-runner")
(declare-function glitnir-gui--start "gui-runner")

(defun gui-runner-tests--settle (events &optional owed)
  "Poll `gui-runner--recorder-quiet' (30 times at most) against a scripted presenter and recorder.
Return (HELD SEEN END): whether it held, the log end its last poll read, and the log end once every commit is in.
The settle starts with OWED commits owed (default 1): 16 packets since a settle at (15 . 15), the log ending at 19.
EVENTS lists (K . WHAT): right after the recorder's reply to control query K (the first is 1), WHAT happens.
`commit': the presenter sends a commit, flushes it and counts it; the log holds it from the reply to query K + 2 on,
the second query sent after the flush (the recorder may answer the first before it reads the commit).  `publish': a
packet is published, as a redisplay inside a query can do."
  (let ((queries 0) (packets 16) (commits (- 16 (or owed 1))) (end 19) (landing nil)
        (gui-runner--settled-counts '(15 . 15)))
    (cl-letf (((symbol-function 'nhwl-presenter-stats)
               (lambda (&rest _) (list :packets packets :superseded 0 :commits commits :bare-commits 0)))
              ((symbol-function 'gui-runner-recorder)
               (lambda (plist)
                 (setq queries (1+ queries))
                 (setq end (+ end (cl-count-if (lambda (q) (<= q queries)) landing))
                       landing (cl-remove-if (lambda (q) (<= q queries)) landing))
                 (prog1 (pcase (plist-get plist :cmd)
                          ("outstanding" (list :ok t :callbacks 0 :occluded :false))
                          ("log" (list :ok t :next end)))
                   (dolist (e events)
                     (when (= (car e) queries)
                       (pcase (cdr e)
                         ('commit (setq commits (1+ commits)) (push (+ queries 2) landing))
                         ('publish (setq packets (1+ packets))))))))))
      (let* ((seen (list nil)) (pred (gui-runner--recorder-quiet seen)) (held nil) (n 0))
        (while (and (not (setq held (funcall pred))) (< (setq n (1+ n)) 30)))
        (list held (car seen) (+ end (length landing)))))))

(ert-deftest gui-runner/settle-holds-on-a-log-with-every-counted-commit ()
  "The commit a settle waits for is counted right after one of the recorder's replies, whichever: the settle holds on a
log that has it.  A commit counted between a poll's log query and its read of the counters let the settle end with the
commit still on its way, and an idle row saw it land after its mark."
  (dotimes (i 8)
    (pcase-let ((`(,held ,seen ,end) (gui-runner-tests--settle `((,(1+ i) . commit)))))
      (ert-info ((format "the commit after reply %d" (1+ i)))
        (should held)
        (should (= seen end))))))

(ert-deftest gui-runner/settle-waits-for-a-packet-published-during-a-poll ()
  "A packet is published while a poll's queries run, and its commit is counted three replies later: the settle holds on
a log that has that commit."
  (dotimes (i 6)
    (pcase-let ((`(,held ,seen ,end) (gui-runner-tests--settle `((,(1+ i) . publish) (,(+ i 4) . commit)) 0)))
      (ert-info ((format "the packet after reply %d" (1+ i)))
        (should held)
        (should (= seen end))))))

;;; gui-runner-tests.el ends here
