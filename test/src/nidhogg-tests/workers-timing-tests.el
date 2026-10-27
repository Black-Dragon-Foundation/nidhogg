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
(require 'treesit)
(require 'nidhogg-workers-treesit)

(defconst workers-timing-tests--dir
  (file-name-directory (or load-file-name buffer-file-name "")))

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

(ert-deftest workers/ts-attempt-clock ()
  ;; §17.3 S1 against the real clock (`workers/ts-start-points' fixes the
  ;; bound): in a buffer that redisplay fontifies, a one-character edit's
  ;; reparse stays on the main thread (its estimate is under the inline
  ;; base, or its 16 ms attempt finishes), and a 500 KB yank's goes to a
  ;; job.  A one-character reparse costs in proportion to the
  ;; siblings of the edited repetition, with the workers off too (desktop
  ;; dev build, 2026-10-23: 2 MB of one-line functions 19 ms, 2 MB of
  ;; 50-statement functions 0.6 ms, 200 KB of those 0.1 ms): the wide
  ;; margin is that last buffer, far under the base.  Needs the C
  ;; grammar (fixtures/grammars.sh).
  (let* ((dir (or (getenv "NIDHOGG_TEST_GRAMMARS")
                  (expand-file-name "fixtures/grammars-cache"
                                    workers-timing-tests--dir)))
         (chunk (concat "int f (int a, int b) {\n"
                        (apply #'concat
                               (make-list
                                50 "  if (a > b) a = a * 2; else b = b + a;\n"))
                        "  return a;\n}\n"))
         (text (lambda (bytes)
                 (apply #'concat (make-list (/ bytes (length chunk)) chunk))))
         (stat (lambda (key) (plist-get (nidhogg-workers--test-ts-stats) key))))
    (skip-unless (file-exists-p (expand-file-name "libtree-sitter-c.so" dir)))
    (let ((treesit-extra-load-path (list dir)))
      (dolist (mode '(threads inline))
        (let ((nidhogg-workers--mode mode)
              (nidhogg-workers-treesit t))
          (ert-info ((format "mode %s" mode) :prefix "")
            (with-temp-buffer
              (insert (funcall text 200000))
              (let ((p (treesit-parser-create 'c nil t))
                    (demand
                     (lambda (p)
                       (let ((t0 (float-time))
                             (can (funcall stat :started-demand)))
                         (nidhogg-workers--test-fontifying (current-buffer)
                           (treesit-parser-changed-regions p))
                         (message "workers/ts-attempt-clock: %s: %.0f us"
                                  mode (* 1e6 (- (float-time) t0)))
                         (- (funcall stat :started-demand) can)))))
                (treesit-parser-root-node p)
                (ert-info ("one character" :prefix "")
                  (goto-char (/ (point-max) 2))
                  ;; Before a `;': the text stays valid C.
                  (skip-chars-forward "^;")
                  (insert " ")
                  ;; (Inline mode runs every offload decision as a job, done
                  ;; at its start.)
                  (should (= (funcall demand p) (if (eq mode 'threads) 0 1)))
                  (should (nidhogg-treesit-parser-ready-p p)))
                (ert-info ("a 500 KB yank" :prefix "")
                  ;; Its estimate: the edited bytes at a seeded rate.
                  (nidhogg-workers--test-ts-cost p 100000 100)
                  (goto-char (/ (point-max) 2))
                  (forward-line 0)
                  (insert-for-yank (funcall text 500000))
                  (should (= (funcall demand p) 1))
                  ;; Inline, the job was done at its start and published.
                  (when (eq mode 'threads)
                    (should (eql (nidhogg-workers--test-ts-wait p 120000)
                                 1))))))))))
    (should (nidhogg-workers--test-ts-await-idle 60000))))

;;; workers-timing-tests.el ends here
