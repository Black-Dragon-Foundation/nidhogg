;;; workers-tests.el --- tests for the worker threads (flag, feature, switches)  -*- lexical-binding: t -*-

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
(require 'loadhist)

(defun workers-tests--tasks ()
  "The number of threads of this process."
  (length (directory-files "/proc/self/task" nil "\\`[0-9]")))

(ert-deftest workers/feature ()
  (should (featurep 'nidhogg-workers)))

(ert-deftest workers/in-the-configuration ()
  (should (string-search "--with-nidhogg-workers" system-configuration-options)))

(ert-deftest workers/lisp-preloaded-compiled ()
  ;; The dump loads nidhogg-workers.el compiled, like the rest of the
  ;; preloaded Lisp, not from source.
  (let ((file (feature-file 'nidhogg-workers-lisp)))
    (should file)
    (should-not (string-suffix-p ".el" file))))

(ert-deftest workers/no-threads-when-unused ()
  (let ((tasks (workers-tests--tasks)))
    (dolist (s nidhogg-workers--switches)
      (symbol-value s))
    (should-not (nidhogg-workers-stats))
    (should (= tasks (workers-tests--tasks)))))

(ert-deftest workers/env-off ()
  ;; A child Nidhogg with NIDHOGG_WORKERS=off: every switch is nil.
  (let* ((process-environment (cons "NIDHOGG_WORKERS=off" process-environment))
         (exe (expand-file-name invocation-name invocation-directory))
         (out (with-temp-buffer
                (should (zerop (call-process
                                exe nil t nil "-Q" "--batch" "--eval"
                                "(princ (mapcar #'symbol-value nidhogg-workers--switches))")))
                (buffer-string))))
    (should (equal (read out) (make-list (length nidhogg-workers--switches) nil)))))

(ert-deftest workers/area-files-load ()
  (require 'jsonrpc)
  (require 'treesit)
  (should (featurep 'nidhogg-workers-jsonrpc))
  (should (featurep 'nidhogg-workers-treesit)))

;;; workers-tests.el ends here
