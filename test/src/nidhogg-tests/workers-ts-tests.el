;;; workers-ts-tests.el --- tests for workers: tree-sitter off the main thread  -*- lexical-binding: t -*-

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
(require 'profiler)
(require 'treesit)

;; The rows need real grammars: fixtures/grammars.sh builds them into
;; NIDHOGG_TEST_GRAMMARS (the `workers-asan' and `workers-pkg' CI jobs run it
;; first and fail when one is missing; elsewhere the rows skip).

(defconst workers-ts-tests--dir
  (file-name-directory (or load-file-name buffer-file-name "")))

(defun workers-ts-tests--grammar-dir ()
  "The directory holding libtree-sitter-c.so, or nil."
  (let* ((dir (or (getenv "NIDHOGG_TEST_GRAMMARS")
                  (expand-file-name "fixtures/grammars-cache"
                                    workers-ts-tests--dir)))
         (lib (expand-file-name "libtree-sitter-c.so" dir)))
    (and (file-exists-p lib) dir)))

(defun workers-ts-tests--c-text (bytes)
  "A C source text of about BYTES bytes."
  (let ((chunk "int f (int a, int b) { if (a > b) return a * 2; else return b + a; }\n"))
    (apply #'concat (make-list (/ bytes (length chunk)) chunk))))

(ert-deftest workers/ts-alloc-under-profiler ()
  ;; A parse on a worker never reaches malloc_probe (B1).
  (let ((dir (workers-ts-tests--grammar-dir)))
    (skip-unless dir)
    (let ((treesit-extra-load-path (list dir))
          (text (workers-ts-tests--c-text 100000)))
      ;; Initialise tree-sitter (and its allocator) on the main thread.
      (with-temp-buffer
        (insert "int x;")
        (treesit-parser-root-node (treesit-parser-create 'c)))
      (profiler-start 'mem)
      (let ((res (unwind-protect
                     (nidhogg-workers--test-ts-alloc
                      (expand-file-name "libtree-sitter-c.so" dir)
                      "tree_sitter_c" text)
                   (profiler-stop))))
        (should (> (car res) 0))
        (should (= (cdr res) 0))))))

(ert-deftest workers/ts-alloc-main-thread-unchanged ()
  ;; A synchronous parse allocates through xmalloc: the profiler sees it.
  (let ((dir (workers-ts-tests--grammar-dir)))
    (skip-unless dir)
    (let ((treesit-extra-load-path (list dir)))
      (with-temp-buffer
        (insert (workers-ts-tests--c-text 200000))
        (let ((parser (treesit-parser-create 'c)))
          (profiler-start 'mem)
          (unwind-protect
              (treesit-parser-root-node parser)
            (profiler-stop))
          ;; `profiler-stop' moved the log into the variable.
          (let ((bytes 0))
            (maphash (lambda (_k v) (setq bytes (+ bytes v)))
                     profiler-memory-log)
            (should (> bytes 100000))))))))

;;; workers-ts-tests.el ends here
