;;; workers-timing-tests.el --- tests for workers: the timing group (wide margins)  -*- lexical-binding: t -*-

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

(ert-deftest workers/wait-deadline ()
  ;; Review Focus 1, the deadline variant: a wait on a job that never
  ;; finishes, under a storm of SIGURG and SIGCHLD, returns 0 at its
  ;; 100 ms deadline (not before; within 1 s, a wide margin).
  (let ((nidhogg-workers--mode 'threads))
    (unwind-protect
        (progn
          (nidhogg-workers--pause-at 'before-done)
          (let ((j (nidhogg-workers--test-job 'nh-blk 0 :class 'file
                                              :pause 'before-done)))
            (should (nidhogg-workers--test-await-paused 'before-done 5000))
            (nidhogg-workers--test-signal-storm 300 nil)
            (let* ((t0 (float-time))
                   (r (nidhogg-workers--test-wait j 100))
                   (dt (- (float-time) t0)))
              (should (eql r 0))
              (should (>= dt 0.1))
              (should (< dt 1.0)))))
      (nidhogg-workers--release))))

;;; workers-timing-tests.el ends here
