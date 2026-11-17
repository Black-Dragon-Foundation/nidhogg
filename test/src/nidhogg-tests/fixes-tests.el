;;; fixes-tests.el --- Fixes to upstream Emacs bugs  -*- lexical-binding: t -*-
;; test-class: cond

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

;; `--with-nidhogg-fixes' (README.md, "Fixes to upstream bugs"): bugs of
;; upstream Emacs that touch no Nidhogg feature.  One row per fix.
;;
;; Conditions only: a child Emacs is waited for by its output and its
;; exit, never by the clock.

;;; Code:

(require 'ert)

(defconst fixes-tests--reloads 400
  "How many times each Emacs loads the shared .eln again.")

(defun fixes-tests--reload-form (eln n)
  "A form that loads ELN once, says it is ready, waits for a line on standard input, then loads ELN again N
times and prints how many of those loads signalled, and the first error."
  `(let ((errors 0) (first nil))
     (native-elisp-load ,eln)
     (princ "ready\n")
     (read-from-minibuffer "")
     (dotimes (_ ,n)
       (condition-case err (native-elisp-load ,eln)
         (error (setq errors (1+ errors)) (unless first (setq first err)))))
     (princ (format "errors %d %S\n" errors first))))

(ert-deftest fixes/two-emacs-reloading-one-eln-never-lose-it ()
  "Two Emacs processes load one .eln of a shared eln-cache again (a module turned off and on, a package reloaded
in a daemon and in a second Emacs).  Upstream, a load of a file this session loaded before renamed the shared file
aside, opened it and renamed it back, so the other process found it missing (`file-missing' \"Renaming\", or a load
that failed).  Each process now opens a private copy, and neither signals."
  (skip-unless (and (fboundp 'native-comp-available-p) (native-comp-available-p)))
  (let* ((dir (make-temp-file "fixes-eln" t))
         (src (expand-file-name "fixes-reload.el" dir))
         (eln (expand-file-name "fixes-reload.eln" dir))
         (out "") (proc nil))
    (unwind-protect
        (progn
          (with-temp-file src
            (insert ";;; fixes-reload.el  -*- lexical-binding: t -*-\n(defvar fixes-reload--n 0)\n"
                    "(defun fixes-reload-step () (setq fixes-reload--n (1+ fixes-reload--n)))\n"))
          (native-compile src eln)
          (should (file-exists-p eln))
          (setq proc (make-process
                      :name "fixes-reload" :connection-type 'pipe :noquery t
                      :command (list (expand-file-name invocation-name invocation-directory) "-Q" "--batch" "--eval"
                                     (prin1-to-string (fixes-tests--reload-form eln fixes-tests--reloads)))
                      :filter (lambda (_p s) (setq out (concat out s)))))
          (while (and (process-live-p proc) (not (string-search "ready\n" out)))
            (accept-process-output proc))
          (should (string-search "ready\n" out))
          ;; both start reloading together
          (process-send-string proc "go\n")
          (let ((errors 0) (first nil))
            (native-elisp-load eln)
            (dotimes (_ fixes-tests--reloads)
              (condition-case err (native-elisp-load eln)
                (error (setq errors (1+ errors)) (unless first (setq first err)))))
            (while (process-live-p proc) (accept-process-output proc))
            (should (equal (list :here errors first :child (string-trim (substring out (or (string-search "errors" out) 0))))
                           (list :here 0 nil :child "errors 0 nil"))))
          (should (file-exists-p eln)))
      (when (process-live-p proc) (delete-process proc))
      (delete-directory dir t))))

;;; fixes-tests.el ends here
