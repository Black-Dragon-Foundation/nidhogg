;;; callproc-env-tests.el --- tests for the environment given to child processes  -*- lexical-binding: t -*-

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

;; add_env (src/callproc.c) builds a child's environment from
;; `process-environment'.  A bare name (an unset) followed by a longer
;; NAME=value that starts with it used to make the duplicate check walk
;; past the end of the bare name's string; when the bytes after it were
;; zero the later definition was dropped.  Only a loop inside one Emacs
;; process shows it (the heap layout varies), so the test spawns many
;; children from here.

;;; Code:

(require 'ert)

(ert-deftest nidhogg-callproc-bare-name-keeps-a-longer-name ()
  (skip-unless (executable-find "printenv"))
  (let ((missing 0)
        (n 3000))
    (dotimes (_ n)
      ;; An 8-character bare name: the bytes after its NUL are padding
      ;; zeros in the heap, which is what lets the old loop run on to the
      ;; "=" of the next name (about a third of the children, measured).
      ;; Fresh strings each time, so they land at different offsets.
      (let ((process-environment
             (list (copy-sequence "NHADDENV")
                   (copy-sequence "NHADDENV_KIND=claude")
                   (copy-sequence "NHADDENV_KIND=ignored")
                   "NHADDENV_OTHER=1")))
        (with-temp-buffer
          (call-process "printenv" nil t nil "NHADDENV_KIND")
          (unless (equal (buffer-string) "claude\n")
            (setq missing (1+ missing))))))
    (should (= missing 0))))

(ert-deftest nidhogg-callproc-bare-name-still-unsets ()
  "A bare name stays a placeholder: it hides NAME=value, and the first definition wins."
  (skip-unless (executable-find "printenv"))
  (let ((process-environment
         (list "NHADDENV_GONE" "NHADDENV_GONE=1" "NHADDENV_WIN=a" "NHADDENV_WIN=b")))
    (with-temp-buffer
      (should (= 1 (call-process "printenv" nil t nil "NHADDENV_GONE")))
      (should (equal (buffer-string) "")))
    (with-temp-buffer
      (should (= 0 (call-process "printenv" nil t nil "NHADDENV_WIN")))
      (should (equal (buffer-string) "a\n")))))

;;; callproc-env-tests.el ends here
