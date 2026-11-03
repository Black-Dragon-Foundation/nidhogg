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

;; Batch tests of the settles of gui-runner.el against a scripted presenter (and recorder): the recorder's
;; (`gui-runner--recorder-quiet') and the presenter's side alone, as on mutter (`gui-runner--presented-p').  No display,
;; no recorder process, no clock.  gui-runner.el is loaded for its functions; the run it would start and its checks
;; around every row are taken out again.

;;; Code:

(require 'ert)
(require 'cl-lib)
(load (expand-file-name "gui-runner.el" (file-name-directory (or load-file-name buffer-file-name))) nil t)
(remove-hook 'window-setup-hook #'glitnir-gui--start)
(advice-remove 'ert-run-test #'gui-runner--strict-commits)
(advice-remove 'ert-run-test #'gui-runner--strict-held)

(declare-function gui-runner--recorder-quiet "gui-runner")
(declare-function gui-runner--strict-commits "gui-runner")
(declare-function gui-runner--strict-held "gui-runner")
(declare-function gui-runner-wait-settled "gui-runner")
(declare-function glitnir-gui--start "gui-runner")

(defun gui-runner-tests--settle (events &optional owed bare)
  "Poll `gui-runner--recorder-quiet' (30 times at most) against a scripted presenter and recorder.
Return (HELD SEEN END): whether it held, the log end its last poll read, and the log end once every commit is in.
The settle starts with OWED commits owed (default 1): 16 requests published, the commits of the first 16 - OWED of
them gone out, the log ending at 19, and BARE bare commits (default none) sent too.  EVENTS lists (K . WHAT): right
after the recorder's
reply to control query K (the first is 1), WHAT happens.  `commit': the presenter sends a commit, flushes it and
counts it; the log holds it from the reply to query K + 2 on, the second query sent after the flush (the recorder may
answer the first before it reads the commit).  `publish': a packet is published, as a redisplay inside a query can do."
  (let ((queries 0) (packets 16) (commits (- 16 (or owed 1))) (bare (or bare 0)) (end 19) (landing nil))
    (cl-letf (((symbol-function 'nhwl-presenter-stats)
               (lambda (&rest _) (list :packets packets :superseded 0 :commits commits :bare-commits bare
                                       :published packets :presented commits)))
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

(ert-deftest gui-runner/settle-holds-on-a-log-with-the-newest-picture ()
  "A bare commit went out too, and the newest packet's commit is counted after reply 5, once nothing else changes: the
settle holds on a log that has that commit (the log ended at 19).  The bare commit does not stand for it."
  (pcase-let ((`(,held ,seen ,end) (gui-runner-tests--settle '((5 . commit)) 1 1)))
    (should held)
    (should (= seen end 20))))

(defun gui-runner-tests--plain-settle (published presented bare reads)
  "Run `gui-runner-wait-settled' on mutter (the presenter's side alone) against a scripted presenter.
PUBLISHED packets have left Emacs, the commits of the first PRESENTED of them have gone to the compositor, and BARE
bare commits too.  READS lists the reads of the presenter's counters before which the oldest waiting packet's commit
goes out (the first read is 1).  Return (PUBLISHED . PRESENTED) as the settle left them."
  (let ((n 0))
    (cl-letf (((symbol-function 'nhwl-presenter-stats)
               (lambda (&rest _)
                 (setq n (1+ n))
                 (when (and (memql n reads) (< presented published))
                   (setq presented (1+ presented)))
                 (list :packets published :superseded 0 :commits presented :bare-commits bare
                       :published published :presented presented))))
      (with-environment-variables (("GLITNIR_GUI_COMPOSITOR" "mutter"))
        (gui-runner-wait-settled))
      (cons published presented))))

(ert-deftest gui-runner/settle-waits-for-the-newest-picture ()
  "A settle ends only once every packet published has gone out in a commit: no other commit stands for a packet's.
The settle counted the commits sent since the settle before against the packets published since.  A bare commit, a
tree's second commit, or the commit of a packet the settle before had not waited for (a session's first settle waited
for none) let it end with the newest picture still waiting for the compositor's frame callback, and the row after it
asked mutter for fullscreen before the size it had set reached mutter."
  ;; everything is out
  (should (equal (gui-runner-tests--plain-settle 5 5 0 nil) '(5 . 5)))
  ;; a bare commit went out since, and packet 6 waits for its commit, which goes out before the third read
  (should (equal (gui-runner-tests--plain-settle 6 5 1 '(3)) '(6 . 6))))

;;; gui-runner-tests.el ends here
