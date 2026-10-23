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
(require 'c-ts-mode)
(require 'nidhogg-workers-treesit)

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

;;; The edit log and replay (Task 11).

(defun workers-ts-tests--sexp (parser)
  "The S-expression of PARSER's tree (a demand)."
  (treesit-node-string (treesit-parser-root-node parser)))

(defun workers-ts-tests--thirty-edits ()
  "Thirty edits all over the current buffer, one change each."
  (dotimes (i 30)
    (goto-char (1+ (/ (* (1+ i) (buffer-size)) 31)))
    (forward-line 0)
    (pcase (% i 3)
      (0 (insert (format "int e%d;\n" i)))
      (1 (insert "/* é */ "))
      (_ (delete-region (point) (line-beginning-position 2))))))

(defun workers-ts-tests--type ()
  "Type a 30-character definition in the middle, one character at a time."
  (goto-char (/ (point-max) 2))
  (forward-line 0)
  (dolist (c (string-to-list "int typed (void) { return 0; }"))
    (insert c)))

(defun workers-ts-tests--replay-row (mode text has-tree edit)
  "Edit twin buffers while A's job is paused at `after-snapshot'; check the replay.
MODE is the workers' mode.  TEXT fills both buffers; with HAS-TREE both
parse first and take one more edit.  Buffer B parses where A's job takes
its snapshot (a synchronous replay with the same grouping); EDIT then
makes 30 one-change edits in each.  A's publish replays them and equals
B's tree."
  (let ((a (generate-new-buffer " ts-replay-a"))
        (b (generate-new-buffer " ts-replay-b"))
        pa pb)
    (unwind-protect
        (progn
          (dolist (buf (list a b))
            (with-current-buffer buf
              (insert text)))
          (setq pa (treesit-parser-create 'c a t)
                pb (treesit-parser-create 'c b t))
          (when has-tree
            (treesit-parser-root-node pa)
            (treesit-parser-root-node pb)
            (dolist (buf (list a b))
              (with-current-buffer buf
                (goto-char (point-max))
                (insert "int z;\n"))))
          (workers-ts-tests--pause mode 'after-snapshot)
          (should (nidhogg-workers--test-ts-start pa))
          (workers-ts-tests--await mode 'after-snapshot)
          (treesit-parser-root-node pb)
          (dolist (buf (list a b))
            (with-current-buffer buf
              (funcall edit)))
          (should (eql (workers-ts-tests--info pa :log) 30))
          (should-not (nidhogg-treesit-parser-ready-p pa))
          (nidhogg-workers--release 'after-snapshot)
          (workers-ts-tests--done pa)
          (let ((pub (workers-ts-tests--stat :published))
                (rep (workers-ts-tests--stat :replayed))
                (edits (workers-ts-tests--stat :replayed-edits))
                (sa (workers-ts-tests--sexp pa)))
            (should (= (workers-ts-tests--stat :published) (1+ pub)))
            (should (= (workers-ts-tests--stat :replayed) (1+ rep)))
            (should (= (workers-ts-tests--stat :replayed-edits) (+ edits 30)))
            (should (equal sa (workers-ts-tests--sexp pb)))))
      (kill-buffer a)
      (kill-buffer b))))

(ert-deftest workers/ts-edits-during-job ()
  ;; Edits while a job runs are logged and replayed onto its tree: the
  ;; publish equals a synchronous replay.  The same for a parser with no
  ;; tree yet: typing before the first parse lands (B3).
  (workers-ts-tests--modes
    (let ((text (workers-ts-tests--c-text 50000)))
      (ert-info ("with a tree" :prefix "")
        (workers-ts-tests--replay-row mode text t
                                      #'workers-ts-tests--thirty-edits))
      (ert-info ("no tree yet" :prefix "")
        (workers-ts-tests--replay-row mode text nil
                                      #'workers-ts-tests--type)))))

(ert-deftest workers/ts-linecol ()
  ;; A parser that tracks line and column: edits during a job are replayed
  ;; with their points; a job started without a tree is dropped at its
  ;; first edit (MC-19) and the demand parses synchronously.
  (workers-ts-tests--modes
    (let ((text (workers-ts-tests--c-text 50000))
          (treesit-languages-require-line-column-tracking '(c)))
      (ert-info ("with a tree" :prefix "")
        (workers-ts-tests--replay-row mode text t
                                      #'workers-ts-tests--thirty-edits))
      (ert-info ("no tree yet" :prefix "")
        (with-temp-buffer
          (insert text)
          (let ((p (treesit-parser-create 'c nil t))
                (dis (workers-ts-tests--stat :discarded)))
            (should (treesit-parser-tracking-line-column-p p))
            (workers-ts-tests--pause mode 'after-snapshot)
            (should (nidhogg-workers--test-ts-start p))
            (workers-ts-tests--await mode 'after-snapshot)
            (goto-char (point-min))
            (insert "int y;\n")
            (should-not (workers-ts-tests--info p :job))
            (should (= (workers-ts-tests--stat :discarded) (1+ dis)))
            (nidhogg-workers--release 'after-snapshot)
            (let ((pub (workers-ts-tests--stat :published)))
              (should (equal (workers-ts-tests--sexp p)
                             (workers-ts-tests--sync-sexp (buffer-string))))
              (should (= (workers-ts-tests--stat :published) pub)))
            (should (nidhogg-workers--test-ts-await-idle 60000))))))))

(ert-deftest workers/ts-cancel-restart ()
  ;; §7.2: a start after an edit drops the older job for a new snapshot,
  ;; whether its parse is finished (paused at `before-done') or not begun
  ;; (`after-snapshot': the cancelled job never parses, so a cancelled
  ;; parse is never resumed over changed input).
  (workers-ts-tests--modes
    (with-temp-buffer
      (insert (workers-ts-tests--c-text 50000))
      (let ((p (treesit-parser-create 'c nil t)))
        (treesit-parser-root-node p)
        (dolist (point '(before-done after-snapshot))
          (ert-info ((format "paused at %s" point) :prefix "")
            (goto-char (point-max))
            (insert "int z;\n")
            (let ((sup (workers-ts-tests--stat :superseded))
                  (started (workers-ts-tests--stat :started))
                  (parses (workers-ts-tests--stat :parses)))
              (workers-ts-tests--pause mode point)
              (should (nidhogg-workers--test-ts-start p))
              (workers-ts-tests--await mode point)
              (goto-char (point-min))
              (insert "int y;\n")
              (should (nidhogg-workers--test-ts-start p))
              (should (= (workers-ts-tests--stat :superseded) (1+ sup)))
              (should (= (workers-ts-tests--stat :started) (+ started 2)))
              (should (eql (workers-ts-tests--info p :log) 0))
              (nidhogg-workers--release point)
              (workers-ts-tests--done p)
              (should (nidhogg-treesit-parser-ready-p p))
              ;; An inline job parses at its start; a worker never begins
              ;; the parse of a job cancelled at `after-snapshot'.
              (should (= (workers-ts-tests--stat :parses)
                         (+ parses (if (and (eq mode 'threads)
                                            (eq point 'after-snapshot))
                                       1 2))))
              (let ((pub (workers-ts-tests--stat :published))
                    (rep (workers-ts-tests--stat :replayed)))
                (should (equal (workers-ts-tests--sexp p)
                               (workers-ts-tests--sync-sexp (buffer-string))))
                (should (= (workers-ts-tests--stat :published) (1+ pub)))
                (should (= (workers-ts-tests--stat :replayed) rep)))
              (should (nidhogg-workers--test-ts-await-idle 60000)))))))))

(ert-deftest workers/ts-subtree-stat-after-replay ()
  ;; M4, second half: a replay reparse drops the worker's statistics; the
  ;; answer is the walked value of the published tree.
  (workers-ts-tests--modes
    (with-temp-buffer
      (insert (workers-ts-tests--c-text 200000))
      (let ((p (treesit-parser-create 'c nil t)))
        (treesit-parser-root-node p)
        (goto-char (point-max))
        (insert "int z;\n")
        (workers-ts-tests--pause mode 'after-snapshot)
        (should (nidhogg-workers--test-ts-start p))
        (workers-ts-tests--await mode 'after-snapshot)
        (goto-char (point-min))
        (insert "int y (void) { return 2; }\n")
        (nidhogg-workers--release 'after-snapshot)
        (workers-ts-tests--done p)
        (let* ((hits (workers-ts-tests--stat :stat-cached))
               (rep (workers-ts-tests--stat :replayed))
               (stat (treesit-subtree-stat (treesit-parser-root-node p))))
          (should (= (workers-ts-tests--stat :replayed) (1+ rep)))
          (should (= (workers-ts-tests--stat :stat-cached) hits))
          (should (equal stat
                         (let ((now (buffer-string)))
                           (with-temp-buffer
                             (insert now)
                             (treesit-subtree-stat
                              (treesit-parser-root-node
                               (treesit-parser-create 'c nil t))))))))))))

;; The oracle (§17.3 S12).

(defconst workers-ts-tests--oracle-steps 5000
  "The edit steps of `workers/ts-oracle', per grammar and mode.")

(defun workers-ts-tests--undo ()
  "Undo the current buffer's latest change group, as `undo' would."
  (let ((list buffer-undo-list))
    (while (and list (null (car list)))
      (setq list (cdr list)))
    (when list
      (primitive-undo 1 list))))

(defun workers-ts-tests--random-text (sample)
  "A random piece of SAMPLE, now and then with a multibyte character."
  (let* ((beg (random (length sample)))
         (end (min (length sample) (+ beg 1 (random 40)))))
    (concat (substring sample beg end) (if (zerop (random 6)) "é" ""))))

(defun workers-ts-tests--random-edit (sample)
  "A random edit of the current buffer, for `workers-ts-tests--apply-edit'.
An insert, delete, replace, yank or undo; SAMPLE is a line of the
language.  A buffer under 8000 characters gets a yank."
  (let* ((size (buffer-size))
         (pos (1+ (random (1+ size))))
         (end (min (1+ size) (+ pos (random 200)))))
    (pcase (if (< size 8000) 8 (random 10))
      ((or 0 1 2 3) (list 'insert pos (workers-ts-tests--random-text sample)))
      ((or 4 5) (list 'delete pos end))
      ((or 6 7) (list 'replace pos end (workers-ts-tests--random-text sample)))
      (8 (list 'yank pos (apply #'concat (make-list (1+ (random 5)) sample))))
      (_ (list 'undo)))))

(defun workers-ts-tests--apply-edit (edit)
  "Apply EDIT, from `workers-ts-tests--random-edit', to the current buffer."
  (undo-boundary)
  (pcase edit
    (`(insert ,pos ,text) (goto-char pos) (insert text))
    (`(delete ,beg ,end) (delete-region beg end))
    (`(replace ,beg ,end ,text)
     ;; One change, as `replace-match' makes it.
     (set-match-data (list beg end))
     (replace-match text t t))
    (`(yank ,pos ,text) (goto-char pos) (insert-for-yank text))
    (`(undo) (workers-ts-tests--undo))))

(defun workers-ts-tests--step (a b sample)
  "One random edit, made identically in buffers A and B (SAMPLE: a line)."
  (let ((edit (with-current-buffer a (workers-ts-tests--random-edit sample))))
    (with-current-buffer a (workers-ts-tests--apply-edit edit))
    (with-current-buffer b (workers-ts-tests--apply-edit edit))))

(ert-deftest workers/ts-oracle ()
  ;; S12: seeded edit scripts (insert, delete, replace, yank, undo) with
  ;; demand points.  At each demand the published tree equals a synchronous
  ;; replay with the identical edit grouping: a twin buffer parsed where
  ;; the job took its snapshot.  Divergence from a fresh parse is counted
  ;; and printed, never asserted.
  (workers-ts-tests--modes
    (dolist (s workers-ts-tests--samples)
      (when (file-exists-p (expand-file-name
                            (format "libtree-sitter-%s.so" (car s)) dir))
        (ert-info ((format "%s" (car s)) :prefix "grammar ")
          (let ((a (generate-new-buffer " ts-oracle-a"))
                (b (generate-new-buffer " ts-oracle-b"))
                (text (apply #'concat (make-list (/ 20000 (length (cdr s)))
                                                 (cdr s))))
                (steps 0) (demands 0) (replays 0) (divergent 0)
                pa pb)
            (unwind-protect
                (progn
                  (random (format "workers/ts-oracle %s" (car s)))
                  (dolist (buf (list a b))
                    (with-current-buffer buf
                      (insert text)
                      ;; After the text: an undo never empties the buffer.
                      (buffer-enable-undo)))
                  (setq pa (treesit-parser-create (car s) a t)
                        pb (treesit-parser-create (car s) b t))
                  (should (equal (workers-ts-tests--sexp pa)
                                 (workers-ts-tests--sexp pb)))
                  (while (< steps workers-ts-tests--oracle-steps)
                    ;; Edits before the snapshot.
                    (dotimes (_ (1+ (random 4)))
                      (workers-ts-tests--step a b (cdr s))
                      (setq steps (1+ steps)))
                    (workers-ts-tests--pause mode 'after-snapshot)
                    (let ((started (nidhogg-workers--test-ts-start pa))
                          (pub (workers-ts-tests--stat :published))
                          (rep (workers-ts-tests--stat :replayed)))
                      ;; B parses where A's job takes its snapshot.
                      (when started
                        (workers-ts-tests--await mode 'after-snapshot)
                        (treesit-parser-root-node pb))
                      ;; Edits while the job runs.
                      (dotimes (_ (random 16))
                        (workers-ts-tests--step a b (cdr s))
                        (setq steps (1+ steps)))
                      (nidhogg-workers--release 'after-snapshot)
                      (when started
                        (workers-ts-tests--done pa))
                      ;; The demand.
                      (let ((sa (workers-ts-tests--sexp pa)))
                        (should (equal sa (workers-ts-tests--sexp pb)))
                        (setq demands (1+ demands))
                        (when started
                          (should (= (workers-ts-tests--stat :published)
                                     (1+ pub)))
                          (when (> (workers-ts-tests--stat :replayed) rep)
                            (setq replays (1+ replays))))
                        (unless (equal sa (with-current-buffer a
                                            (workers-ts-tests--sync-sexp
                                             (buffer-string) (car s))))
                          (setq divergent (1+ divergent))))))
                  (should (> replays 0))
                  (message "workers/ts-oracle: %s %s: %d demands, %d replays, %d diverge from a fresh parse"
                           mode (car s) demands replays divergent))
              (kill-buffer a)
              (kill-buffer b))))))))

;;; Start points and the wait rule (Task 12).

(defun workers-ts-tests--fontifying-demand (parser)
  "PARSER's demand as `treesit--pre-redisplay' makes it while redisplay fontifies.
Return whether PARSER is ready afterwards."
  (nidhogg-workers--test-fontifying (treesit-parser-buffer parser)
    (treesit-parser-changed-regions parser))
  (nidhogg-treesit-parser-ready-p parser))

(defun workers-ts-tests--parse-waits ()
  "How many waits on nh-parse jobs the workers have counted."
  (apply #'+ (append (plist-get (plist-get (nidhogg-workers-stats) :nh-parse)
                                :wait-ms)
                     nil)))

(defun workers-ts-tests--big-change (mode p change)
  "Make CHANGE (a function) in P's buffer, then a fontifying demand.
MODE is the workers' mode.  The 1 ms attempt is cancelled and reset, one
job starts from the demand point; in threads mode the demand answers
\"not ready\" (the job is parked at `after-snapshot'), inline the job is
done at its start and the demand publishes it.  The tree that lands
equals a synchronous parse."
  (funcall change)
  (let ((att (workers-ts-tests--stat :attempts))
        (can (workers-ts-tests--stat :cancels))
        (dem (workers-ts-tests--stat :started-demand))
        (started (workers-ts-tests--stat :started)))
    (workers-ts-tests--pause mode 'after-snapshot)
    (let ((ready (workers-ts-tests--fontifying-demand p)))
      (should (= (workers-ts-tests--stat :attempts) (1+ att)))
      (should (= (workers-ts-tests--stat :cancels) (1+ can)))
      (should (= (workers-ts-tests--stat :started-demand) (1+ dem)))
      (should (= (workers-ts-tests--stat :started) (1+ started)))
      (if (eq mode 'inline)
          (should ready)
        (should-not ready)
        (workers-ts-tests--await mode 'after-snapshot)
        (should (eq (workers-ts-tests--info p :job) 'running))
        ;; Another demand of that redisplay: the job runs, no new attempt.
        (should-not (workers-ts-tests--fontifying-demand p))
        (should (= (workers-ts-tests--stat :attempts) (1+ att)))
        (should (= (workers-ts-tests--stat :started) (1+ started)))
        (nidhogg-workers--release 'after-snapshot)
        (workers-ts-tests--done p)))
    (should (equal (workers-ts-tests--sexp p)
                   (workers-ts-tests--sync-sexp (buffer-string))))))

(ert-deftest workers/ts-start-points ()
  ;; §17.3 S1: in a 2 MB buffer that redisplay fontifies, a one-character
  ;; edit finishes inside the 1 ms attempt (no snapshot); a 500 KB yank, a
  ;; revert and an `indent-region' cancel the attempt and start one job
  ;; each from the demand point; edits from a timer start a job at the
  ;; select entry.
  (workers-ts-tests--modes
    (let* ((file (make-temp-file "workers-ts-start" nil ".txt"
                                 (workers-ts-tests--c-text 2000000)))
           (buf (find-file-noselect file)))
      (unwind-protect
          (with-current-buffer buf
            (let ((p (treesit-parser-create 'c nil t)))
              (treesit-parser-root-node p)
              (ert-info ("one character" :prefix "")
                (goto-char (/ (point-max) 2))
                (insert " ")
                (let ((att (workers-ts-tests--stat :attempts))
                      (can (workers-ts-tests--stat :cancels))
                      (started (workers-ts-tests--stat :started)))
                  (should (workers-ts-tests--fontifying-demand p))
                  (should (= (workers-ts-tests--stat :attempts) (1+ att)))
                  (should (= (workers-ts-tests--stat :cancels) can))
                  (should (= (workers-ts-tests--stat :started) started))))
              (ert-info ("not fontifying: today's parse" :prefix "")
                (goto-char (point-min))
                (insert-for-yank (workers-ts-tests--c-text 100000))
                (let ((att (workers-ts-tests--stat :attempts))
                      (started (workers-ts-tests--stat :started)))
                  (should (treesit-parser-changed-regions p))
                  (should (nidhogg-treesit-parser-ready-p p))
                  (should (= (workers-ts-tests--stat :attempts) att))
                  (should (= (workers-ts-tests--stat :started) started))))
              (ert-info ("a 500 KB yank" :prefix "")
                (workers-ts-tests--big-change
                 mode p (lambda ()
                          (goto-char (/ (point-max) 2))
                          (forward-line 0)
                          (insert-for-yank
                           (workers-ts-tests--c-text 500000)))))
              (ert-info ("a revert" :prefix "")
                (workers-ts-tests--big-change
                 mode p (lambda ()
                          (with-temp-file file
                            (insert (string-replace
                                     "a > b" "b > a"
                                     (workers-ts-tests--c-text 2000000))))
                          (revert-buffer t t t))))
              (ert-info ("indent-region" :prefix "")
                (workers-ts-tests--big-change
                 mode p (lambda ()
                          (goto-char (/ (point-max) 2))
                          (let ((indent-line-function
                                 (lambda () (indent-line-to 2))))
                            (indent-region (line-beginning-position)
                                           (line-beginning-position 2001))))))
              (ert-info ("a timer's edit, at the select entry" :prefix "")
                ;; Whatever earlier edits left for the select entry.
                (nidhogg-workers--test-ts-select-entry)
                (let ((sel (workers-ts-tests--stat :started-select))
                      (size (buffer-size)))
                  (run-at-time 0 nil
                               (lambda ()
                                 (with-current-buffer buf
                                   (goto-char (point-min))
                                   (insert-for-yank
                                    (workers-ts-tests--c-text 500000)))))
                  (accept-process-output nil 0)
                  (should (> (buffer-size) size))
                  (nidhogg-workers--test-ts-select-entry)
                  (should (= (workers-ts-tests--stat :started-select) (1+ sel)))
                  (should (workers-ts-tests--info p :job))
                  ;; Nothing edited since: the next entry starts nothing.
                  (nidhogg-workers--test-ts-select-entry)
                  (should (= (workers-ts-tests--stat :started-select) (1+ sel)))
                  (workers-ts-tests--done p)
                  (should (equal (workers-ts-tests--sexp p)
                                 (workers-ts-tests--sync-sexp
                                  (buffer-string))))))))
        (with-current-buffer buf
          (set-buffer-modified-p nil))
        (kill-buffer buf)
        (delete-file file)
        (should (nidhogg-workers--test-ts-await-idle 60000))))))

(ert-deftest workers/ts-start-after-mode ()
  ;; Review M17: opening a 1 MB file in `c-ts-mode' starts a job when the
  ;; mode finishes, before any redisplay.
  (workers-ts-tests--modes
    (let* ((file (make-temp-file "workers-ts-mode" nil ".c"
                                 (workers-ts-tests--c-text 1000000)))
           (auto-mode-alist (cons '("\\.c\\'" . c-ts-mode) auto-mode-alist))
           (c-ts-mode-emacs-sources-support nil)
           (treesit-auto-install-grammar 'never)
           (created (workers-ts-tests--stat :started-create))
           (buf (find-file-noselect file)))
      (unwind-protect
          (with-current-buffer buf
            (should (eq major-mode 'c-ts-mode))
            (let ((p (car (treesit-parser-list nil 'c)))
                  (pub (workers-ts-tests--stat :published)))
              (should (= (workers-ts-tests--stat :started-create) (1+ created)))
              (should (workers-ts-tests--info p :job))
              (workers-ts-tests--done p)
              (should (equal (workers-ts-tests--sexp p)
                             (workers-ts-tests--sync-sexp (buffer-string))))
              (should (= (workers-ts-tests--stat :published) (1+ pub)))))
        (kill-buffer buf)
        (delete-file file)))))

(defun workers-ts-tests--blocked (fn)
  "Call FN while nh-parse's idle-priority thread is busy; return its value.
A test job parks that thread at `before-write', so a tree-sitter job
started in FN stays queued until something waits for it."
  (nidhogg-workers--pause-at 'before-write)
  (let ((blocker (nidhogg-workers--test-job 'nh-parse 0 :speculative t
                                            :pause 'before-write)))
    (unwind-protect
        (progn
          (should (nidhogg-workers--test-await-paused 'before-write 30000))
          (funcall fn))
      (nidhogg-workers--release 'before-write)
      (should (eql (nidhogg-workers--test-wait blocker 30000) 1)))))

(ert-deftest workers/ts-wait-cancel ()
  ;; §17.3 S2: a demand that needs a tree while a job runs cancels a job
  ;; not started, or stale and under half done, and parses on the main
  ;; thread; it waits for one nearly done over the current text.  A job
  ;; not started and waited on runs on the normal-priority thread (PD-15).
  (workers-ts-tests--modes
    (with-temp-buffer
      (insert (workers-ts-tests--c-text 50000))
      (let ((p (treesit-parser-create 'c nil t)))
        (treesit-parser-root-node p)
        (when (eq mode 'threads)
          (ert-info ("not started: cancelled" :prefix "")
            (goto-char (point-max))
            (insert "int z;\n")
            (workers-ts-tests--blocked
             (lambda ()
               (should (nidhogg-workers--test-ts-start p))
               (should (eq (workers-ts-tests--info p :job) 'queued))
               (let ((ab (workers-ts-tests--stat :abandoned))
                     (waited (workers-ts-tests--stat :waited))
                     (pub (workers-ts-tests--stat :published)))
                 (should (equal (workers-ts-tests--sexp p)
                                (workers-ts-tests--sync-sexp (buffer-string))))
                 (should (= (workers-ts-tests--stat :abandoned) (1+ ab)))
                 (should (= (workers-ts-tests--stat :waited) waited))
                 (should (= (workers-ts-tests--stat :published) pub))
                 (should-not (workers-ts-tests--info p :job))))))
          (ert-info ("stale and under half: cancelled" :prefix "")
            (goto-char (point-max))
            (insert "int y;\n")
            (workers-ts-tests--pause mode 'after-snapshot)
            (should (nidhogg-workers--test-ts-start p))
            (workers-ts-tests--await mode 'after-snapshot)
            (goto-char (point-min))
            (insert "int x;\n")
            (let ((ab (workers-ts-tests--stat :abandoned))
                  (waited (workers-ts-tests--stat :waited)))
              (should (equal (workers-ts-tests--sexp p)
                             (workers-ts-tests--sync-sexp (buffer-string))))
              (should (= (workers-ts-tests--stat :abandoned) (1+ ab)))
              (should (= (workers-ts-tests--stat :waited) waited)))
            (nidhogg-workers--release 'after-snapshot)
            (should (nidhogg-workers--test-ts-await-idle 60000)))
          (ert-info ("not started, waited on: the normal-priority thread"
                     :prefix "")
            (goto-char (point-max))
            (insert "int w;\n")
            (workers-ts-tests--blocked
             (lambda ()
               (should (nidhogg-workers--test-ts-start p))
               (should (eq (workers-ts-tests--info p :job) 'queued))
               (workers-ts-tests--done p)
               (should (eq (workers-ts-tests--info p :policy) 'other))))
            (treesit-parser-root-node p)))
        (ert-info ("nearly done: waited" :prefix "")
          (goto-char (point-max))
          (insert "int v;\n")
          (workers-ts-tests--pause mode 'before-done)
          (should (nidhogg-workers--test-ts-start p))
          (workers-ts-tests--await mode 'before-done)
          (when (eq mode 'threads)
            (nidhogg-workers--test-release-on-wait 'before-done))
          (let ((waited (workers-ts-tests--stat :waited))
                (waits (workers-ts-tests--parse-waits))
                (pub (workers-ts-tests--stat :published))
                (ab (workers-ts-tests--stat :abandoned)))
            (unwind-protect
                (should (equal (workers-ts-tests--sexp p)
                               (workers-ts-tests--sync-sexp (buffer-string))))
              (nidhogg-workers--test-release-on-wait nil))
            (should (= (workers-ts-tests--stat :published) (1+ pub)))
            (should (= (workers-ts-tests--stat :abandoned) ab))
            ;; Inline, the job was done at its start: nothing to wait for.
            (let ((n (if (eq mode 'threads) 1 0)))
              (should (= (workers-ts-tests--stat :waited) (+ waited n)))
              (should (= (workers-ts-tests--parse-waits) (+ waits n))))))))))

(ert-deftest workers/ts-demand-from-lisp-thread ()
  ;; Review Focus 2: a Lisp thread's `treesit-node-at' while a job is
  ;; parked: the wait is bounded, the node is right, nothing deadlocks.
  (workers-ts-tests--modes
    (with-temp-buffer
      (insert (workers-ts-tests--c-text 50000))
      (let* ((p (treesit-parser-create 'c nil t))
             (buf (current-buffer))
             (pos (- (point-max) 20))
             (node-type (lambda ()
                          (thread-join
                           (make-thread
                            (lambda ()
                              (with-current-buffer buf
                                (treesit-node-type
                                 (treesit-node-at pos p)))))))))
        (treesit-parser-root-node p)
        (ert-info ("the wait releases the job" :prefix "")
          (goto-char (point-min))
          (insert "int z;\n")
          (workers-ts-tests--pause mode 'before-done)
          (should (nidhogg-workers--test-ts-start p))
          (workers-ts-tests--await mode 'before-done)
          (when (eq mode 'threads)
            (nidhogg-workers--test-release-on-wait 'before-done))
          (let ((pub (workers-ts-tests--stat :published))
                (waited (workers-ts-tests--stat :waited))
                (type (unwind-protect (funcall node-type)
                        (nidhogg-workers--test-release-on-wait nil))))
            (should (equal type (treesit-node-type
                                 (treesit-node-at pos (treesit-parser-create
                                                       'c nil t)))))
            (should (= (workers-ts-tests--stat :published) (1+ pub)))
            (should (= (workers-ts-tests--stat :waited)
                       (+ waited (if (eq mode 'threads) 1 0))))))
        (ert-info ("the job stays parked: the wait ends at its bound"
                   :prefix "")
          (goto-char (point-min))
          (insert "int y;\n")
          (workers-ts-tests--pause mode 'before-done)
          (should (nidhogg-workers--test-ts-start p))
          (workers-ts-tests--await mode 'before-done)
          (let* ((ab (workers-ts-tests--stat :abandoned))
                 (type (unwind-protect (funcall node-type)
                         ;; The main thread releases it afterwards.
                         (nidhogg-workers--release 'before-done))))
            (should (equal type (treesit-node-type
                                 (treesit-node-at pos (treesit-parser-create
                                                       'c nil t)))))
            (should (= (workers-ts-tests--stat :abandoned)
                       (+ ab (if (eq mode 'threads) 1 0))))))
        (should (nidhogg-workers--test-ts-await-idle 60000))))))

;;; Deferral at redisplay (Task 13).

(defmacro workers-ts-tests--deferring (&rest body)
  "Run BODY as `workers-ts-tests--modes' does, with `nidhogg-treesit-defer' on.
Afterwards land every job: a job started with deferral on is freed when
it lands."
  (declare (indent 0) (debug t))
  `(workers-ts-tests--modes
     (let ((nidhogg-treesit-defer t)
           (c-ts-mode-emacs-sources-support nil)
           (treesit-auto-install-grammar 'never))
       (unwind-protect
           (progn ,@body)
         (nidhogg-workers--release)
         (should (nidhogg-workers--test-await-landed 'nh-parse 60000))))))

(defun workers-ts-tests--c-ts-buffer (text)
  "A new buffer in `c-ts-mode' holding TEXT, fontified through jit-lock.
Batch Emacs keeps `font-lock-mode' off, so jit-lock is registered by
hand, as `font-lock-mode' would."
  (let ((buf (generate-new-buffer "workers-ts-defer")))
    (with-current-buffer buf
      (insert text)
      (c-ts-mode)
      (font-lock-set-defaults)
      (jit-lock-register #'font-lock-fontify-region))
    buf))

(defun workers-ts-tests--redisplay-fontify (buf beg end)
  "Fontify BEG..END of BUF as a redisplay does.
A fresh wait budget, the pre-redisplay wrapper (redisplay runs
`pre-redisplay-functions' before it fontifies, with the variable
unbound), then jit-lock with `nidhogg--redisplay-fontifying' bound to BUF."
  (with-current-buffer buf
    (nidhogg-workers--test-redisplay-begin)
    (nidhogg-workers-treesit--pre-redisplay (selected-window))
    (nidhogg-workers--test-fontifying buf
      (jit-lock-fontify-now beg end))))

(defun workers-ts-tests--faces (beg end)
  "The `face' runs between BEG and END: (FROM TO FACE)."
  (let ((pos beg) out)
    (while (< pos end)
      (let ((next (next-single-property-change pos 'face nil end)))
        (push (list pos next (get-text-property pos 'face)) out)
        (setq pos next)))
    (nreverse out)))

(defun workers-ts-tests--has-face (face beg end)
  "Non-nil if some text between BEG and END has FACE."
  (let ((pos beg) found)
    (while (and (not found) (< pos end))
      (when (memq face (ensure-list (get-text-property pos 'face)))
        (setq found t))
      (setq pos (next-single-property-change pos 'face nil end)))
    found))

(defun workers-ts-tests--deferrals ()
  "How many tree-sitter deferrals the workers have counted."
  (plist-get (plist-get (nidhogg-workers-stats) :deferrals) 'treesit))

(ert-deftest workers/ts-deferral ()
  ;; §17.3 B4: redisplay finds the parse on a paused worker: the region
  ;; keeps its old faces, the budget's grant is spent, the tick is not
  ;; recorded; after the release the landing refontifies through a normal
  ;; demand.  `font-lock-ensure' in another buffer, `syntax-ppss' and
  ;; outline search wait and use the current tree.
  (workers-ts-tests--deferring
    (let ((buf (workers-ts-tests--c-ts-buffer
                (workers-ts-tests--c-text 100000)))
          (end 3001))
      (unwind-protect
          (with-current-buffer buf
            (let ((p treesit-primary-parser))
              (should (memq #'nidhogg-workers-treesit--pre-redisplay
                            pre-redisplay-functions))
              (should-not (memq #'treesit--pre-redisplay
                                pre-redisplay-functions))
              (treesit-parser-root-node p)
              (workers-ts-tests--redisplay-fontify buf 1 end)
              (should (workers-ts-tests--has-face 'font-lock-keyword-face 1 end))
              (let ((faces (workers-ts-tests--faces 1 end)))
                (ert-info ("the deferral" :prefix "")
                  ;; Far from the region and far too long for 1 ms.
                  (goto-char (point-max))
                  (insert-for-yank (workers-ts-tests--c-text 500000))
                  ;; What a notifier does for a changed range over it.
                  (with-silent-modifications
                    (put-text-property 1 end 'fontified nil))
                  (workers-ts-tests--pause mode 'after-snapshot)
                  (nidhogg-workers--test-budget-ledger t)
                  (let ((pub (workers-ts-tests--stat :published))
                        (defs (workers-ts-tests--deferrals)))
                    (workers-ts-tests--redisplay-fontify buf 1 end)
                    (should (equal (workers-ts-tests--faces 1 end) faces))
                    (should-not (text-property-not-all 1 end 'fontified t))
                    (if (eq mode 'inline)
                        ;; The job was done at its start: published.
                        (progn
                          (should-not nidhogg-workers-treesit--pending)
                          (should (= (workers-ts-tests--stat :published)
                                     (1+ pub)))
                          (should (= (workers-ts-tests--deferrals) defs)))
                      (workers-ts-tests--await mode 'after-snapshot)
                      (should (eq (workers-ts-tests--info p :job) 'running))
                      (should nidhogg-workers-treesit--pending)
                      (should (= (workers-ts-tests--stat :published) pub))
                      (should (> (workers-ts-tests--deferrals) defs))
                      (should (> (car (nidhogg-workers--test-budget-ledger)) 0))
                      (should-not (eql treesit--pre-redisplay-tick
                                       (buffer-chars-modified-tick))))))
                (ert-info ("font-lock-ensure in another buffer" :prefix "")
                  (let ((other (workers-ts-tests--c-ts-buffer
                                (workers-ts-tests--c-text 50000)))
                        (defs (workers-ts-tests--deferrals)))
                    (unwind-protect
                        (progn
                          (nidhogg-workers--test-redisplay-begin)
                          (nidhogg-workers--test-fontifying buf
                            (with-current-buffer other
                              (font-lock-ensure)))
                          (with-current-buffer other
                            (should (workers-ts-tests--has-face
                                     'font-lock-keyword-face
                                     (point-min) (point-max)))
                            (should-not nidhogg-workers-treesit--pending))
                          (should (= (workers-ts-tests--deferrals) defs)))
                      (kill-buffer other))))
                (ert-info ("the landing" :prefix "")
                  (let ((pub (workers-ts-tests--stat :published)))
                    (nidhogg-workers--release 'after-snapshot)
                    (should (nidhogg-workers--test-await-landed 'nh-parse 60000))
                    (should-not nidhogg-workers-treesit--pending)
                    (when (eq mode 'threads)
                      ;; The landing marked the region for jit-lock.
                      (should (text-property-any 1 end 'fontified nil)))
                    (workers-ts-tests--redisplay-fontify buf 1 end)
                    (should-not (text-property-not-all 1 end 'fontified t))
                    (should (equal (workers-ts-tests--faces 1 end) faces))
                    (should (eql treesit--pre-redisplay-tick
                                 (buffer-chars-modified-tick)))
                    (should (= (workers-ts-tests--stat :published)
                               (+ pub (if (eq mode 'threads) 1 0))))
                    (should (equal (workers-ts-tests--sexp p)
                                   (workers-ts-tests--sync-sexp
                                    (buffer-string))))))
                (dolist (call (list (cons "syntax-ppss"
                                          (lambda ()
                                            (treesit--pre-syntax-ppss
                                             (point-min) (point-max))))
                                    (cons "outline search"
                                          (lambda () (treesit--pre-redisplay)))))
                  (ert-info ((car call) :prefix "")
                    (goto-char (point-max))
                    (insert-for-yank (workers-ts-tests--c-text 200000))
                    (workers-ts-tests--pause mode 'after-snapshot)
                    (should (nidhogg-workers--test-ts-start p))
                    (workers-ts-tests--await mode 'after-snapshot)
                    (when (eq mode 'threads)
                      (nidhogg-workers--test-release-on-wait 'after-snapshot))
                    (let ((att (workers-ts-tests--stat :attempts))
                          (defs (workers-ts-tests--deferrals)))
                      (unwind-protect
                          (progn
                            (nidhogg-workers--test-redisplay-begin)
                            (nidhogg-workers--test-fontifying buf
                              (funcall (cdr call))))
                        (nidhogg-workers--test-release-on-wait nil)
                        (nidhogg-workers--release 'after-snapshot))
                      ;; A demand that ended with a tree: no 1 ms attempt,
                      ;; no deferral, the tick recorded.
                      (should (nidhogg-treesit-parser-ready-p p))
                      (should (= (workers-ts-tests--stat :attempts) att))
                      (should (= (workers-ts-tests--deferrals) defs))
                      (should (eql treesit--pre-redisplay-tick
                                   (buffer-chars-modified-tick)))
                      (should (equal (workers-ts-tests--sexp p)
                                     (workers-ts-tests--sync-sexp
                                      (buffer-string))))))))))
        (kill-buffer buf)))))

(ert-deftest workers/ts-defer-off ()
  ;; With `nidhogg-treesit-defer' nil: today's waits, no pending regions,
  ;; no 1 ms attempt, nothing deferred.
  (workers-ts-tests--modes
    (let* ((nidhogg-treesit-defer nil)
           (c-ts-mode-emacs-sources-support nil)
           (treesit-auto-install-grammar 'never)
           (buf (workers-ts-tests--c-ts-buffer
                 (workers-ts-tests--c-text 100000)))
           (end 3001))
      (unwind-protect
          (with-current-buffer buf
            (let ((p treesit-primary-parser)
                  (att (workers-ts-tests--stat :attempts))
                  (defs (workers-ts-tests--deferrals)))
              (treesit-parser-root-node p)
              (ert-info ("a long edit: today's synchronous parse" :prefix "")
                (goto-char (point-max))
                (insert-for-yank (workers-ts-tests--c-text 500000))
                (with-silent-modifications
                  (put-text-property 1 end 'fontified nil))
                (let ((started (workers-ts-tests--stat :started)))
                  (workers-ts-tests--redisplay-fontify buf 1 end)
                  (should (= (workers-ts-tests--stat :started) started))))
              (ert-info ("a job running: today's wait" :prefix "")
                (goto-char (point-max))
                (insert-for-yank (workers-ts-tests--c-text 200000))
                (with-silent-modifications
                  (put-text-property 1 end 'fontified nil))
                (workers-ts-tests--pause mode 'before-done)
                (should (nidhogg-workers--test-ts-start p))
                (workers-ts-tests--await mode 'before-done)
                (when (eq mode 'threads)
                  (nidhogg-workers--test-release-on-wait 'before-done))
                (let ((waited (workers-ts-tests--stat :waited)))
                  (unwind-protect
                      (workers-ts-tests--redisplay-fontify buf 1 end)
                    (nidhogg-workers--test-release-on-wait nil)
                    (nidhogg-workers--release 'before-done))
                  (should (= (workers-ts-tests--stat :waited)
                             (+ waited (if (eq mode 'threads) 1 0))))))
              (should (workers-ts-tests--has-face 'font-lock-keyword-face 1 end))
              (should-not (text-property-not-all 1 end 'fontified t))
              (should-not nidhogg-workers-treesit--pending)
              (should (= (workers-ts-tests--stat :attempts) att))
              (should (= (workers-ts-tests--deferrals) defs))
              (should (eql treesit--pre-redisplay-tick
                           (buffer-chars-modified-tick)))
              (should (equal (workers-ts-tests--sexp p)
                             (workers-ts-tests--sync-sexp (buffer-string))))))
        (kill-buffer buf)
        (should (nidhogg-workers--test-ts-await-idle 60000))))))

(ert-deftest workers/ts-org-block-coloured ()
  ;; Review B2, the real path: Org fontifies a `c-ts-mode' source block in
  ;; a temporary buffer with `font-lock-ensure' while redisplay fontifies
  ;; the Org buffer; nh-parse is paused, the block is large enough to miss
  ;; a 1 ms attempt, and still it is coloured: the temporary buffer is not
  ;; the one redisplay fontifies, so nothing defers.
  (workers-ts-tests--deferring
    (require 'org)
    (let ((org-src-fontify-natively t)
          (buf (generate-new-buffer "workers-ts-org")))
      (unwind-protect
          (with-current-buffer buf
            (insert "* Code\n#+begin_src c-ts\n"
                    (workers-ts-tests--c-text 200000)
                    "#+end_src\n")
            (org-mode)
            (font-lock-set-defaults)
            (jit-lock-register #'font-lock-fontify-region)
            (workers-ts-tests--pause mode 'after-snapshot)
            (let ((defs (workers-ts-tests--deferrals)))
              (nidhogg-workers--test-redisplay-begin)
              (nidhogg-workers--test-fontifying buf
                (jit-lock-fontify-now (point-min) (point-max)))
              (should (workers-ts-tests--has-face 'font-lock-keyword-face
                                                  (point-min) (point-max)))
              (should (= (workers-ts-tests--deferrals) defs))))
        (nidhogg-workers--release 'after-snapshot)
        (kill-buffer buf)
        ;; Org keeps its fontification buffer: its parser goes with it.
        (dolist (b (buffer-list))
          (when (string-prefix-p " *org-src-fontification:" (buffer-name b))
            (kill-buffer b)))))))

(ert-deftest workers/ts-vertical-motion-waits ()
  ;; Review S3: the display iterator also runs from `vertical-motion'
  ;; (and `pos-visible-in-window-p' and friends), where `redisplaying_p'
  ;; is false: over unfontified text with nh-parse paused it waits and
  ;; colours, and records no pending region.
  (workers-ts-tests--deferring
    (let ((buf (workers-ts-tests--c-ts-buffer
                (workers-ts-tests--c-text 100000))))
      (unwind-protect
          (with-current-buffer buf
            (let ((p treesit-primary-parser))
              (treesit-parser-root-node p)
              (goto-char (point-max))
              (insert-for-yank (workers-ts-tests--c-text 200000))
              (with-silent-modifications
                (put-text-property (point-min) (point-max) 'fontified nil)
                (remove-text-properties (point-min) (point-max) '(face nil)))
              (workers-ts-tests--pause mode 'before-done)
              (should (nidhogg-workers--test-ts-start p))
              (workers-ts-tests--await mode 'before-done)
              (when (eq mode 'threads)
                (nidhogg-workers--test-release-on-wait 'before-done))
              (set-window-buffer (selected-window) buf)
              (goto-char (point-min))
              (let ((att (workers-ts-tests--stat :attempts))
                    (defs (workers-ts-tests--deferrals)))
                (nidhogg-workers--test-redisplay-begin)
                (unwind-protect
                    (vertical-motion 20)
                  (nidhogg-workers--test-release-on-wait nil)
                  (nidhogg-workers--release 'before-done))
                (should (> (point) (point-min)))
                (should (workers-ts-tests--has-face 'font-lock-keyword-face
                                                    (point-min) (point)))
                (should-not nidhogg-workers-treesit--pending)
                (should (= (workers-ts-tests--stat :attempts) att))
                (should (= (workers-ts-tests--deferrals) defs)))))
        (kill-buffer buf)))))

(ert-deftest workers/ts-pre-redisplay-starts-job ()
  ;; Review S2: after a 500 KB yank the pre-redisplay wrapper's call makes
  ;; the 1 ms attempt and starts the job (on the path `read_char' runs
  ;; before the select); jit-lock in the same redisplay finds it running
  ;; and makes no second attempt.
  (workers-ts-tests--deferring
    (let ((buf (workers-ts-tests--c-ts-buffer
                (workers-ts-tests--c-text 100000))))
      (unwind-protect
          (with-current-buffer buf
            (let* ((p treesit-primary-parser)
                   (beg (progn (treesit-parser-root-node p)
                               (goto-char (/ (point-max) 2))
                               (forward-line 0)
                               (point)))
                   (end (+ beg 3000)))
              (insert-for-yank (workers-ts-tests--c-text 500000))
              (workers-ts-tests--pause mode 'after-snapshot)
              (let ((att (workers-ts-tests--stat :attempts))
                    (can (workers-ts-tests--stat :cancels))
                    (dem (workers-ts-tests--stat :started-demand))
                    (started (workers-ts-tests--stat :started))
                    (pub (workers-ts-tests--stat :published)))
                (nidhogg-workers--test-redisplay-begin)
                (nidhogg-workers-treesit--pre-redisplay (selected-window))
                (should (= (workers-ts-tests--stat :attempts) (1+ att)))
                (should (= (workers-ts-tests--stat :cancels) (1+ can)))
                (should (= (workers-ts-tests--stat :started-demand) (1+ dem)))
                (should (= (workers-ts-tests--stat :started) (1+ started)))
                (if (eq mode 'inline)
                    (progn
                      (should (nidhogg-treesit-parser-ready-p p))
                      (should (= (workers-ts-tests--stat :published) (1+ pub)))
                      (should (eql treesit--pre-redisplay-tick
                                   (buffer-chars-modified-tick))))
                  (workers-ts-tests--await mode 'after-snapshot)
                  (should (eq (workers-ts-tests--info p :job) 'running))
                  (should-not (eql treesit--pre-redisplay-tick
                                   (buffer-chars-modified-tick))))
                ;; jit-lock in the same redisplay.
                (nidhogg-workers--test-fontifying buf
                  (jit-lock-fontify-now beg end))
                (should (= (workers-ts-tests--stat :attempts) (1+ att)))
                (should (= (workers-ts-tests--stat :started) (1+ started)))
                (should-not (text-property-not-all beg end 'fontified t))
                (if (eq mode 'inline)
                    (should (workers-ts-tests--has-face 'font-lock-keyword-face
                                                        beg end))
                  (should nidhogg-workers-treesit--pending)
                  (should-not (workers-ts-tests--has-face
                               'font-lock-keyword-face beg end))
                  (nidhogg-workers--release 'after-snapshot)
                  (should (nidhogg-workers--test-await-landed 'nh-parse 60000))
                  (should-not nidhogg-workers-treesit--pending)
                  (workers-ts-tests--redisplay-fontify buf beg end)
                  (should (= (workers-ts-tests--stat :published) (1+ pub)))
                  (should (workers-ts-tests--has-face 'font-lock-keyword-face
                                                      beg end))))))
        (kill-buffer buf)))))

;;; workers-ts-tests.el ends here
