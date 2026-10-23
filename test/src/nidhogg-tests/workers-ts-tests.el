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

;;; Parse jobs over snapshots, publish, ownership (Task 10).

(defmacro workers-ts-tests--modes (&rest body)
  "Run BODY with the C grammar, in each mode, with `nidhogg-workers-treesit' on.
BODY sees `mode' and `dir' (the grammar directory)."
  (declare (indent 0) (debug t))
  `(let ((dir (workers-ts-tests--grammar-dir)))
     (skip-unless dir)
     (let ((treesit-extra-load-path (list dir)))
       (should (nidhogg-workers--test-ts-await-idle 60000))
       (dolist (mode '(threads inline))
         (let ((nidhogg-workers--mode mode)
               (nidhogg-workers-treesit t))
           (ert-info ((format "mode %s" mode) :prefix "")
             (unwind-protect
                 (progn ,@body)
               (nidhogg-workers--release))))))))

(defun workers-ts-tests--stat (key)
  "The tree-sitter jobs' counter KEY."
  (plist-get (nidhogg-workers--test-ts-stats) key))

(defun workers-ts-tests--info (parser key)
  "PARSER's worker state KEY."
  (plist-get (nidhogg-workers--test-ts-info parser) key))

(defun workers-ts-tests--pause (mode point)
  "In threads MODE, arm POINT (a job parks there)."
  (when (eq mode 'threads)
    (nidhogg-workers--pause-at point)))

(defun workers-ts-tests--await (mode point)
  "In threads MODE, wait until a job parks at POINT."
  (when (eq mode 'threads)
    (should (nidhogg-workers--test-await-paused point 30000))))

(defun workers-ts-tests--done (parser)
  "Wait until PARSER's job is done (inline jobs already are)."
  (should (eql (nidhogg-workers--test-ts-wait parser 120000) 1)))

(defun workers-ts-tests--sync-sexp (text &optional lang ranges)
  "The S-expression of a synchronous LANG (default C) parse of TEXT with RANGES."
  (with-temp-buffer
    (insert text)
    (let ((p (treesit-parser-create (or lang 'c) nil t)))
      (when ranges
        (treesit-parser-set-included-ranges p ranges))
      (treesit-node-string (treesit-parser-root-node p)))))

(defvar workers-ts-tests--calls nil
  "The notifier calls: (TAG . RANGES), the latest first.")

(defun workers-ts-tests--notifier (ranges parser)
  "Record a notifier call with RANGES from PARSER."
  (push (cons (treesit-parser-tag parser) ranges) workers-ts-tests--calls))

(defun workers-ts-tests--edit ()
  "Edit the current buffer in three places."
  (goto-char (point-min))
  (insert "int g;\n")
  (goto-char (/ (point-max) 2))
  (forward-line 0)
  (insert "int h (void) { return 1; }\n")
  (goto-char (point-max))
  (insert "int k;\n"))

(ert-deftest workers/ts-notifiers ()
  ;; B5: notifiers run only at demand calls, with the synchronous ranges;
  ;; never from a completion or a timer.
  (workers-ts-tests--modes
    (let ((text (workers-ts-tests--c-text 50000))
          (a (generate-new-buffer " ts-a"))
          (b (generate-new-buffer " ts-b")))
      (unwind-protect
          (let (pa pb)
            (setq workers-ts-tests--calls nil)
            (with-current-buffer a
              (insert text)
              (setq pa (treesit-parser-create 'c nil t 'a))
              (treesit-parser-root-node pa)
              (treesit-parser-add-notifier pa #'workers-ts-tests--notifier)
              (workers-ts-tests--edit))
            (with-current-buffer b
              (insert text)
              (setq pb (treesit-parser-create 'c nil t 'b))
              (treesit-parser-root-node pb)
              (treesit-parser-add-notifier pb #'workers-ts-tests--notifier)
              (workers-ts-tests--edit))
            (should (nidhogg-workers--test-ts-start pb))
            (workers-ts-tests--done pb)
            (should (nidhogg-treesit-parser-ready-p pb))
            ;; A drain and a timer check: no notifier.
            (nidhogg-workers--drain)
            (accept-process-output nil 0)
            (should-not workers-ts-tests--calls)
            (let ((pub (workers-ts-tests--stat :published)))
              (treesit-parser-root-node pa)
              (treesit-parser-root-node pb)
              (should (= (workers-ts-tests--stat :published) (1+ pub))))
            (should (= (length workers-ts-tests--calls) 2))
            (should (equal (cdr (assq 'a workers-ts-tests--calls))
                           (cdr (assq 'b workers-ts-tests--calls))))
            ;; A second demand parses nothing and notifies nobody.
            (treesit-parser-root-node pb)
            (should (= (length workers-ts-tests--calls) 2)))
        (kill-buffer a)
        (kill-buffer b)))))

(ert-deftest workers/ts-node-validity ()
  ;; A node taken before the job stays valid until the publish.
  (workers-ts-tests--modes
    (with-temp-buffer
      (insert (workers-ts-tests--c-text 50000))
      (let* ((p (treesit-parser-create 'c nil t))
             (node (treesit-node-child (treesit-parser-root-node p) 0)))
        (goto-char (point-max))
        (insert "int z;\n")
        (workers-ts-tests--pause mode 'before-done)
        (should (nidhogg-workers--test-ts-start p))
        (workers-ts-tests--await mode 'before-done)
        ;; Paused: not done yet (an inline job already is).
        (should (eq (nidhogg-treesit-parser-ready-p p) (eq mode 'inline)))
        (should-not (treesit-node-check node 'outdated))
        (should (treesit-node-start node))
        (nidhogg-workers--release 'before-done)
        (workers-ts-tests--done p)
        (should-not (treesit-node-check node 'outdated))
        (let ((pub (workers-ts-tests--stat :published)))
          (treesit-parser-root-node p)
          (should (= (workers-ts-tests--stat :published) (1+ pub))))
        (should (treesit-node-check node 'outdated))
        (should-error (treesit-node-start node) :type 'treesit-node-outdated)))))

(ert-deftest workers/ts-ranges-change ()
  ;; New included ranges during a job discard it: the synchronous parse runs.
  (workers-ts-tests--modes
    (with-temp-buffer
      (insert (workers-ts-tests--c-text 50000))
      (let ((p (treesit-parser-create 'c nil t))
            (ranges (list (cons 1 5000))))
        (treesit-parser-root-node p)
        (goto-char (point-max))
        (insert "int z;\n")
        (workers-ts-tests--pause mode 'before-done)
        (should (nidhogg-workers--test-ts-start p))
        (workers-ts-tests--await mode 'before-done)
        (treesit-parser-set-included-ranges p ranges)
        (nidhogg-workers--release 'before-done)
        (workers-ts-tests--done p)
        (should-not (nidhogg-treesit-parser-ready-p p))
        (let ((pub (workers-ts-tests--stat :published))
              (dis (workers-ts-tests--stat :discarded))
              (sexp (treesit-node-string (treesit-parser-root-node p))))
          (should (= (workers-ts-tests--stat :published) pub))
          (should (= (workers-ts-tests--stat :discarded) (1+ dis)))
          (should (equal sexp (workers-ts-tests--sync-sexp
                               (buffer-string) 'c (list (cons 1 5000))))))))))

(ert-deftest workers/ts-delete-and-gc ()
  ;; The parser deleted, its buffer killed and a forced GC while a job is
  ;; paused: the job is freed once, when the worker lets go (ASan: no
  ;; use after free).  M9: an indirect buffer's narrowing over its base.
  (workers-ts-tests--modes
    (let ((text (workers-ts-tests--c-text 50000)))
      ;; The parser deleted, then its buffer killed, then GC.
      (let ((buf (generate-new-buffer " ts-del")))
        (with-current-buffer buf
          (insert text)
          (let ((p (treesit-parser-create 'c nil t)))
            (workers-ts-tests--pause mode 'after-snapshot)
            (should (nidhogg-workers--test-ts-start p))
            (workers-ts-tests--await mode 'after-snapshot)
            (treesit-parser-delete p)))
        (kill-buffer buf)
        (garbage-collect)
        (should (= (workers-ts-tests--stat :live)
                   (if (eq mode 'threads) 1 0)))
        (nidhogg-workers--release)
        (should (nidhogg-workers--test-ts-await-idle 60000)))
      ;; The buffer killed under a live parser, then GC.
      (let ((buf (generate-new-buffer " ts-kill")) p)
        (with-current-buffer buf
          (insert text)
          (setq p (treesit-parser-create 'c nil t))
          (treesit-parser-root-node p)
          (insert "int z;\n")
          (workers-ts-tests--pause mode 'before-done)
          (should (nidhogg-workers--test-ts-start p))
          (workers-ts-tests--await mode 'before-done))
        (kill-buffer buf)
        (should-not (workers-ts-tests--info p :job))
        (setq p nil)
        (garbage-collect)
        (nidhogg-workers--release)
        (should (nidhogg-workers--test-ts-await-idle 60000)))
      ;; M9: a parser of an indirect buffer narrowed over its base's text.
      (let* ((base (generate-new-buffer " ts-base"))
             (ind (progn (with-current-buffer base (insert text))
                         (make-indirect-buffer base " ts-ind"))))
        (unwind-protect
            (with-current-buffer ind
              (narrow-to-region 1001 9001)
              (let ((p (treesit-parser-create 'c)))
                (treesit-parser-root-node p)
                (goto-char 1500)
                (insert "int q;\n")
                (should (nidhogg-workers--test-ts-start p))
                (workers-ts-tests--done p)
                (should (equal (treesit-node-string (treesit-parser-root-node p))
                               (workers-ts-tests--sync-sexp
                                (buffer-substring (point-min) (point-max)))))
                (goto-char 2500)
                (insert "int r;\n")
                (workers-ts-tests--pause mode 'after-snapshot)
                (should (nidhogg-workers--test-ts-start p))
                (workers-ts-tests--await mode 'after-snapshot)))
          ;; Killing the base kills the indirect buffer first.
          (kill-buffer base))
        (garbage-collect)
        (nidhogg-workers--release)
        (should (nidhogg-workers--test-ts-await-idle 60000))
        (should (= (workers-ts-tests--stat :live) 0))))))

(ert-deftest workers/ts-temporary-narrowing ()
  ;; S1: no job starts under a temporary narrowing; a start changes neither
  ;; the visible range nor the ranges; embedded parsers keep their ranges.
  (workers-ts-tests--modes
    (with-temp-buffer
      (insert (workers-ts-tests--c-text 50000))
      (let ((p (treesit-parser-create 'c nil t))
            (e (treesit-parser-create 'c nil t 'embed))
            (ranges (list (cons 100 3000))))
        (treesit-parser-set-included-ranges e ranges)
        ;; No tree yet: a narrowed buffer refuses.
        (save-restriction
          (narrow-to-region 50 4000)
          (should-not (nidhogg-workers--test-ts-start p))
          (should-not (nidhogg-workers--test-ts-start e)))
        (treesit-parser-root-node p)
        (treesit-parser-root-node e)
        (goto-char 200)
        (insert "int y;\n")
        (let ((info-p (nidhogg-workers--test-ts-info p))
              (info-e (nidhogg-workers--test-ts-info e)))
          (save-restriction
            (narrow-to-region 50 4000)
            (should-not (nidhogg-workers--test-ts-start p))
            (should-not (nidhogg-workers--test-ts-start e))
            (should (equal (nidhogg-workers--test-ts-info p) info-p))
            (should (equal (nidhogg-workers--test-ts-info e) info-e)))
          (should (nidhogg-workers--test-ts-start p))
          (should (nidhogg-workers--test-ts-start e))
          (dolist (pair (list (cons p info-p) (cons e info-e)))
            (let ((now (nidhogg-workers--test-ts-info (car pair))))
              (should (equal (plist-get now :visible)
                             (plist-get (cdr pair) :visible)))
              (should (eq (plist-get now :ranges)
                          (plist-get (cdr pair) :ranges)))
              (should (= (plist-get now :gen) (plist-get (cdr pair) :gen)))))
          (should (eq (workers-ts-tests--info e :ranges) ranges))
          (should (equal ranges (list (cons 100 3000))))
          (workers-ts-tests--done p)
          (workers-ts-tests--done e)
          (let ((pub (workers-ts-tests--stat :published)))
            (should (equal (treesit-node-string (treesit-parser-root-node e))
                           (workers-ts-tests--sync-sexp
                            (buffer-string) 'c (list (cons 100 3000)))))
            (should (equal (treesit-node-string (treesit-parser-root-node p))
                           (workers-ts-tests--sync-sexp (buffer-string))))
            (should (= (workers-ts-tests--stat :published) (+ pub 2)))))))))

(defconst workers-ts-tests--samples
  '((c . "int f (int a, int b) { if (a > b) return a * 2; else return b + a; }\n")
    (python . "def f(a, b):\n    if a > b:\n        return a * 2\n    return b + a\n\n")
    (rust . "fn f(a: i32, b: i32) -> i32 { if a > b { a * 2 } else { b + a } }\n")
    (typescript . "function f(a: number, b: number): number { return a > b ? a * 2 : b + a; }\n")
    (html . "<div class=\"a\"><p>text <b>bold</b></p><script>var x = 1;</script></div>\n"))
  "A line of each fixture grammar's language.")

(ert-deftest workers/ts-publish-equals-sync ()
  ;; A 1 MB file in each fixture grammar: the published tree (first parse,
  ;; then an incremental one) equals a synchronous parse.
  (workers-ts-tests--modes
    (dolist (s workers-ts-tests--samples)
      (when (file-exists-p (expand-file-name
                            (format "libtree-sitter-%s.so" (car s)) dir))
        (ert-info ((format "%s" (car s)) :prefix "grammar ")
          (with-temp-buffer
            (dotimes (_ (/ 1000000 (length (cdr s))))
              (insert (cdr s)))
            (let ((p (treesit-parser-create (car s) nil t))
                  (pub (workers-ts-tests--stat :published)))
              (should (nidhogg-workers--test-ts-start p))
              (workers-ts-tests--done p)
              (should (equal (treesit-node-string (treesit-parser-root-node p))
                             (workers-ts-tests--sync-sexp (buffer-string) (car s))))
              (goto-char (/ (point-max) 3))
              (forward-line 0)
              (insert (cdr s) (cdr s))
              (should (nidhogg-workers--test-ts-start p))
              (workers-ts-tests--done p)
              (should (equal (treesit-node-string (treesit-parser-root-node p))
                             (workers-ts-tests--sync-sexp (buffer-string) (car s))))
              (should (= (workers-ts-tests--stat :published) (+ pub 2))))))))))

(ert-deftest workers/ts-subtree-stat-cached ()
  ;; M4, first half: the worker's subtree statistics equal the walked ones.
  (workers-ts-tests--modes
    (let ((text (workers-ts-tests--c-text 200000)))
      (with-temp-buffer
        (insert text)
        (let ((p (treesit-parser-create 'c nil t)))
          (should (nidhogg-workers--test-ts-start p))
          (workers-ts-tests--done p)
          (let* ((root (treesit-parser-root-node p))
                 (hits (workers-ts-tests--stat :stat-cached))
                 (cached (treesit-subtree-stat root)))
            (should (= (workers-ts-tests--stat :stat-cached) (1+ hits)))
            (should (equal cached
                           (with-temp-buffer
                             (insert text)
                             (treesit-subtree-stat
                              (treesit-parser-root-node
                               (treesit-parser-create 'c nil t))))))
            ;; The walk above and a child's are not cached.
            (treesit-subtree-stat (treesit-node-child root 0))
            (should (= (workers-ts-tests--stat :stat-cached) (1+ hits)))
            ;; A synchronous reparse drops the cached value.
            (goto-char (point-max))
            (insert "int w;\n")
            (let ((walked (treesit-subtree-stat (treesit-parser-root-node p))))
              (should (= (workers-ts-tests--stat :stat-cached) (1+ hits)))
              (should (equal walked
                             (let ((now (buffer-string)))
                               (with-temp-buffer
                                 (insert now)
                                 (treesit-subtree-stat
                                  (treesit-parser-root-node
                                   (treesit-parser-create 'c nil t))))))))))))))

;;; workers-ts-tests.el ends here
