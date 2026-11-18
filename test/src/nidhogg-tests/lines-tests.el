;;; lines-tests.el --- The line model (Hlidskjalf) through nidhogg-lines.c  -*- lexical-binding: t -*-
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

;; `nidhogg-lines-enable' keeps a line model of a buffer's text, fed
;; from the edit history's change sites.  The rows change the text
;; through every site and assert the model's lines are the text's, with
;; no rebuild (a change the hook missed or sent wrong would make the
;; model rebuild from the text, which the :rebuilds count shows); then
;; marks, folds, hidden lines, the trail and the background build.
;;
;; Pure (pattern D) but for the background build's row, which waits for
;; its thread with `nidhogg-lines-enable''s WAIT.  The rows skip on a
;; build configured without --with-nidhogg-history.

;;; Code:

(require 'ert)
(require 'nidhogg-views nil t)      ; the hub's folds: Sjá's minimap layers pushed as hub lines

(defvar nidhogg-lines-sync-limit)

(defun lines-tests--built-p ()
  "Whether this Emacs has the line model."
  (fboundp 'nidhogg-lines-enable))

(defun lines-tests--starts (buffer)
  "The positions BUFFER's lines start at, ignoring narrowing."
  (with-current-buffer buffer
    (save-excursion
      (save-restriction
        (widen)
        (goto-char (point-min))
        (let ((starts (list (point))))
          (while (search-forward "\n" nil t)
            (push (point) starts))
          (nreverse starts))))))

(defun lines-tests--check (buffer)
  "Assert BUFFER's line model has the lines of its text."
  (let ((starts (lines-tests--starts buffer))
        (d (nidhogg-lines-describe buffer)))
    (should (eq (plist-get d :state) 'ready))
    (should (= (plist-get d :lines) (length starts)))
    (should (= (plist-get d :chars)
               (with-current-buffer buffer
                 (save-restriction (widen) (1- (point-max))))))
    (let ((i 0))
      (dolist (s starts)
        (should (equal (list i (nidhogg-lines-line-start buffer i))
                       (list i s)))
        (should (= (nidhogg-lines-line buffer s) i))
        (setq i (1+ i))))))

(defmacro lines-tests--with (spec &rest body)
  "Run BODY with (VAR TEXT) of SPEC a fresh buffer holding TEXT and a model.
After BODY the model is checked against the text, and no change may have
rebuilt it.  The buffer is killed after."
  (declare (indent 1) (debug ((symbolp form) body)))
  (let ((var (car spec)) (r0 (make-symbol "r0")))
    `(let ((,var (generate-new-buffer " *lines-tests*")) ,r0)
       (unwind-protect
           (progn
             (with-current-buffer ,var (insert ,(cadr spec)))
             (should (nidhogg-lines-enable ,var t))
             (setq ,r0 (plist-get (nidhogg-lines-describe ,var) :rebuilds))
             (with-current-buffer ,var ,@body)
             (lines-tests--check ,var)
             (should (= (plist-get (nidhogg-lines-describe ,var) :rebuilds)
                        ,r0)))
         (when (buffer-live-p ,var)
           (kill-buffer ,var))))))

(ert-deftest lines/every-change-site-keeps-the-lines ()
  "Insertion, deletion, yank, case, translation, transposition, replacement, undo."
  (skip-unless (lines-tests--built-p))
  (let ((src (generate-new-buffer " *lines-src*"))
        (file (make-temp-file "lines-tests" nil ".txt" "from a file\nsecond\n")))
    (unwind-protect
        (lines-tests--with (b "héllo wörld\n  two\n\tthree\n\nfive")
          (buffer-enable-undo)
          (goto-char 3)
          (insert "X\nY")
          (insert-before-markers "🐉\n")
          (insert-char ?\n 3)
          (delete-region 1 2)
          (goto-char (point-max))
          (insert "\nlast\n")
          (narrow-to-region 4 12)
          (goto-char (point-min))
          (insert "[n]\n")
          (delete-char 2)
          (widen)
          (let ((kill-ring nil) (kill-ring-yank-pointer nil)
                (interprogram-paste-function nil))
            (kill-new "yanked\nlines\n")
            (goto-char 6)
            (yank))
          (with-current-buffer src (insert "from\nanother\nbuffer"))
          (insert-buffer-substring src)
          (insert-file-contents file)
          (upcase-region 1 30)
          (capitalize-region 5 40)
          (subst-char-in-region 1 (point-max) ?e ?\n)
          (translate-region 1 20 (let ((s (make-string 256 0)))
                                   (dotimes (i 256) (aset s i i))
                                   (aset s ?o ?\n)
                                   s))
          (transpose-regions 1 8 20 31)
          (undo-boundary)
          (let ((new (generate-new-buffer " *lines-new*")))
            (with-current-buffer new (insert "replaced\nwhole\n  text\n"))
            (replace-buffer-contents new)
            (kill-buffer new))
          (undo-boundary)
          (undo)
          (erase-buffer)
          (insert "after\n"))
      (kill-buffer src)
      (delete-file file))))

(ert-deftest lines/indent-and-blank-lines ()
  "A model's lines carry their indent; the rows only check positions, so this row checks a big text too."
  (skip-unless (lines-tests--built-p))
  (lines-tests--with (b (mapconcat (lambda (i) (format "%sline %d\n" (make-string (% i 7) ?\s) i))
                                   (number-sequence 1 20000) ""))
    (goto-char (/ (point-max) 2))
    (insert "\n\n  new\n")
    (goto-char 1)
    (kill-line 3)))

(ert-deftest lines/marks-shift-and-step ()
  "Marks move with the text; next and previous by kind; counts."
  (skip-unless (lines-tests--built-p))
  (lines-tests--with (b "aaa\nbbb\nccc\nddd\n")
    (nidhogg-lines-set-marks b 1 (list [5 8 0 0 0] [13]))
    (nidhogg-lines-set-marks b 2 (list [9 nil 1 2 3]))
    (goto-char 1)
    (insert "x\ny\n")
    (should (= (nidhogg-lines-next-mark b 1 '(1)) 9))
    (should (= (nidhogg-lines-next-mark b 9 '(1)) 17))
    (should (= (nidhogg-lines-next-mark b 9) 13))
    (should (= (nidhogg-lines-next-mark b 17 '(1) -1) 9))
    (should-not (nidhogg-lines-next-mark b 13 '(2)))
    (let ((c (nidhogg-lines-counts b 1 (point-max))))
      (should (= (aref c 1) 2))
      (should (= (aref c 2) 1)))
    (should (= (plist-get (nidhogg-lines-describe b) :marks) 3))))

(ert-deftest lines/folds-move-and-toggle ()
  "Fold ranges shift with edits, nest, and `fold-at' picks the line's range."
  (skip-unless (lines-tests--built-p))
  (lines-tests--with (b "fn a() {\n  x\n  y\n}\nfn b() {\n  z\n}\n")
    (nidhogg-lines-set-folds b 3 '((1 19 1) (20 33 1) (10 15 2)))
    (should (equal (nidhogg-lines-folds b)
                   '((1 19 1 0 3) (10 15 2 1 3) (20 33 1 0 3))))
    (goto-char 13)
    (insert "  w\n")
    (should (equal (mapcar (lambda (f) (list (nth 0 f) (nth 1 f)))
                           (nidhogg-lines-folds b))
                   '((1 23) (10 19) (24 37))))
    (should (= (car (nidhogg-lines-fold-at b 26)) 24))
    (should (= (car (nidhogg-lines-fold-at b 11)) 10))
    (should (equal (mapcar #'car (nidhogg-lines-folds b 20 30)) '(1 24)))
    (should-not (nidhogg-lines-fold-at b (point-max)))))

(ert-deftest lines/hidden-lines-leave-the-fraction ()
  "Hidden lines do not count in the scroll bar's measure."
  (skip-unless (lines-tests--built-p))
  (lines-tests--with (b (mapconcat (lambda (i) (format "%02d\n" i))
                                   (number-sequence 0 9) ""))
    (should (= (nidhogg-lines-fraction b 16) 0.5))
    (nidhogg-lines-set-hidden b '((3 . 18)))
    (should (< (abs (- (nidhogg-lines-fraction b 19) 0.2)) 1e-9))
    (should (= (nidhogg-lines-fraction b 10) (nidhogg-lines-fraction b 4)))
    ;; narrowed: the accessible part is the whole
    (narrow-to-region 19 (point-max))
    (should (= (nidhogg-lines-fraction b 19) 0.0))
    (should (= (nidhogg-lines-fraction b (point-max)) 1.0))))

(ert-deftest lines/trail-keeps-one-visit-per-line ()
  "The trail is newest first, one per line, at most 20."
  (skip-unless (lines-tests--built-p))
  (lines-tests--with (b (make-string 40 ?\n))
    (dolist (p '(1 5 2 5 9)) (nidhogg-lines-trail-note b p))
    (should (equal (nidhogg-lines-trail b) '(9 5 2 1)))
    (dotimes (i 30) (nidhogg-lines-trail-note b (+ 10 i)))
    (should (= (length (nidhogg-lines-trail b)) 20))
    (goto-char 1)
    (insert "\n\n")
    (should (= (car (nidhogg-lines-trail b)) 41))))

(ert-deftest lines/swap-text-rebuilds ()
  "`buffer-swap-text' and `set-buffer-multibyte' read the text again."
  (skip-unless (lines-tests--built-p))
  (let ((a (generate-new-buffer " *lines-a*"))
        (o (generate-new-buffer " *lines-o*")))
    (unwind-protect
        (progn
          (with-current-buffer a (insert "one\ntwo\n"))
          (with-current-buffer o (insert "x\ny\nz\nw\n"))
          (nidhogg-lines-enable a t)
          (with-current-buffer a (buffer-swap-text o))
          (nidhogg-lines-enable a t)
          (lines-tests--check a)
          (with-current-buffer a
            (set-buffer-multibyte nil)
            (insert "\351\n"))
          (nidhogg-lines-enable a t)
          (lines-tests--check a))
      (kill-buffer a)
      (kill-buffer o))))

(ert-deftest lines/indirect-buffer-shares-the-model ()
  "An indirect buffer's edits reach its base's model, once."
  (skip-unless (lines-tests--built-p))
  (lines-tests--with (b "base\ntext\n")
    (let ((ind (make-indirect-buffer b " *lines-ind*")))
      (unwind-protect
          (progn
            (should (nidhogg-lines-describe ind))
            (with-current-buffer ind
              (goto-char (point-max))
              (insert "from the indirect buffer\n")))
        (kill-buffer ind)))
    (should (= (plist-get (nidhogg-lines-describe b) :edits) 1))))

(ert-deftest lines/a-large-text-builds-in-the-background ()
  "Past the sync limit the lines build on a thread; edits meanwhile are replayed."
  (skip-unless (lines-tests--built-p))
  (let ((b (generate-new-buffer " *lines-big*"))
        (nidhogg-lines-sync-limit 1000))
    (unwind-protect
        (progn
          (with-current-buffer b
            (dotimes (i 50000) (insert (format "line %d\n" i))))
          (nidhogg-lines-enable b)
          (with-current-buffer b
            (goto-char 7)
            (insert "\nnew\n")
            (goto-char (point-max))
            (delete-char -3))
          (nidhogg-lines-enable b t)
          (lines-tests--check b))
      (kill-buffer b))))

(ert-deftest lines/killing-the-buffer-forgets-it ()
  "A killed buffer's model goes; disable drops it too."
  (skip-unless (lines-tests--built-p))
  (let ((b (generate-new-buffer " *lines-k*")))
    (with-current-buffer b (insert "x\n"))
    (nidhogg-lines-enable b t)
    (should (nidhogg-lines-describe b))
    (nidhogg-lines-disable b)
    (should-not (nidhogg-lines-describe b))
    (should-error (nidhogg-lines-line b 1))
    (nidhogg-lines-enable b t)
    (kill-buffer b)
    (should-not (nidhogg-lines-describe b))))

(provide 'lines-tests)
(ert-deftest lines/mark-sections-follow-edits ()
  "With an expression (Rust regex syntax) the model finds the MARK sections, (POS . NAME) with POS the line's start,
shifts them with an edit above and finds one typed; BEG limits them.  An expression that does not compile signals
`invalid-regexp' and keeps the one before; nil drops them."
  (with-temp-buffer
    (insert "a\n;;;; One\nb\n")
    (nidhogg-lines-enable nil t)
    (should-not (nidhogg-lines-sections))
    (nidhogg-lines-set-section-regexp nil "^;;;;\\s*(.+)$")
    (should (equal (nidhogg-lines-sections) '((3 . "One"))))
    (goto-char 1)
    (insert "x\n")
    (should (equal (nidhogg-lines-sections) '((5 . "One"))))
    (goto-char (point-max))
    (insert ";;;; Two\n")
    (should (equal (nidhogg-lines-sections) '((5 . "One") (16 . "Two"))))
    (should (equal (nidhogg-lines-sections nil 6) '((16 . "Two"))))
    (should-error (nidhogg-lines-set-section-regexp nil "(") :type 'invalid-regexp)
    (should (= (length (nidhogg-lines-sections)) 2))
    (nidhogg-lines-set-section-regexp nil nil)
    (should-not (nidhogg-lines-sections))
    (nidhogg-lines-disable)))

(ert-deftest lines/fold-sources-indent-and-headings ()
  "The model's own fold ranges: by indentation (source 4, kind 0), from a line to the last deeper one; by headings
\(source 2, kind 8), to the line before the next heading of the level or above, found again after an edit.  A heading
expression that does not compile signals `invalid-regexp'; nil sources drop the ranges."
  (with-temp-buffer
    (insert "a\n  b\n  c\nd\n")
    (nidhogg-lines-enable nil t)
    (should-not (nidhogg-lines-folds nil))
    (nidhogg-lines-set-fold-sources nil t nil)
    (should (equal (nidhogg-lines-folds nil) '((1 10 0 0 4))))
    (should (equal (nidhogg-lines-fold-at nil 3) '(1 10 0 0 4)))
    (nidhogg-lines-set-fold-sources nil nil nil)
    (should-not (nidhogg-lines-folds nil))
    (erase-buffer)
    (insert "# A\ntext\n## B\nmore\n# C\nlast\n")
    (nidhogg-lines-set-fold-sources nil nil "^(#+)\\s")
    (should (equal (nidhogg-lines-folds nil) '((1 19 8 0 2) (10 19 8 1 2) (20 28 8 0 2))))
    (goto-char (point-max))
    (insert "## D\nz\n")
    (should (equal (nidhogg-lines-folds nil 20)
                   '((20 35 8 0 2) (29 35 8 1 2))))
    (should-error (nidhogg-lines-set-fold-sources nil nil "(") :type 'invalid-regexp)
    (should (= (length (nidhogg-lines-folds nil)) 4))
    (nidhogg-lines-disable)))

(defvar nidhogg-treesit-fold-query)

(defconst lines-tests--dir (file-name-directory (or load-file-name buffer-file-name "")))

(defun lines-tests--grammars ()
  "The directory of the test grammars (fixtures/grammars.sh: C and Python), or nil."
  (let ((dir (or (getenv "NIDHOGG_TEST_GRAMMARS")
                 (expand-file-name "fixtures/grammars-cache" lines-tests--dir))))
    (and (file-exists-p (expand-file-name "libtree-sitter-c.so" dir)) dir)))

(defun lines-tests--range (a b)
  "Lines A..B (0-based) of the current buffer as (BEG END): A's start to B's end."
  (save-excursion
    (goto-char (point-min))
    (forward-line a)
    (let ((beg (point)))
      (forward-line (- b a))
      (list beg (line-end-position)))))

(ert-deftest lines/tree-sitter-folds-follow-each-parse ()
  "The buffer's fold query (`nidhogg-treesit-fold-query') runs at each
parse's publish over the changed ranges, in C: its nodes are source 0's
ranges, widened to whole lines, of the `@fold.KIND' capture's kind (plain
`@fold' is 0, other captures none).  An edit inside one function replaces
that function's ranges only; narrowing keeps the ranges outside; a query
of another language is not run."
  (skip-unless (and (lines-tests--built-p) (boundp 'nidhogg-treesit-fold-query) (lines-tests--grammars)))
  (let ((treesit-extra-load-path (list (lines-tests--grammars))))
   (with-temp-buffer
    (insert "int a (void)\n{\n  return 1;\n}\n\nint b (void)\n{\n  return 2;\n}\n")
    (nidhogg-lines-enable nil t)
    (let ((p (treesit-parser-create 'c))
          (folds (lambda () (mapcar (lambda (f) (list (nth 0 f) (nth 1 f) (nth 2 f) (nth 4 f)))
                                    (nidhogg-lines-folds nil)))))
      (setq-local nidhogg-treesit-fold-query
                  (treesit-query-compile 'c "(function_definition) @fold.function (compound_statement) @fold
                                             (translation_unit) @scope"))
      (treesit-parser-root-node p)
      (should (equal (funcall folds)
                     `((,@(lines-tests--range 0 3) 1 0) (,@(lines-tests--range 1 3) 0 0)
                       (,@(lines-tests--range 5 8) 1 0) (,@(lines-tests--range 6 8) 0 0))))
      ;; a line typed into b: b's ranges grow, a's stay (and are not sent again: a set-folds sent meanwhile stays)
      (goto-char (point-min))
      (search-forward "return 2;")
      (insert "\n  x++;")
      (treesit-parser-root-node p)
      (should (equal (funcall folds)
                     `((,@(lines-tests--range 0 3) 1 0) (,@(lines-tests--range 1 3) 0 0)
                       (,@(lines-tests--range 5 9) 1 0) (,@(lines-tests--range 6 9) 0 0))))
      ;; b made one line: its ranges go
      (let ((b (car (lines-tests--range 5 5))))
        (delete-region b (point-max))
        (goto-char b)
        (insert "int b (void) { return 2; }\n"))
      (treesit-parser-root-node p)
      (should (equal (funcall folds)
                     `((,@(lines-tests--range 0 3) 1 0) (,@(lines-tests--range 1 3) 0 0))))
      ;; narrowed to b, a new body for b: a's ranges, outside, stay
      (let ((b (car (lines-tests--range 5 5))))
        (narrow-to-region b (point-max))
        (goto-char (point-max))
        (insert "int c (void)\n{\n}\n")
        (treesit-parser-root-node p)
        (widen))
      (should (equal (funcall folds)
                     `((,@(lines-tests--range 0 3) 1 0) (,@(lines-tests--range 1 3) 0 0)
                       (,@(lines-tests--range 6 8) 1 0) (,@(lines-tests--range 7 8) 0 0))))
      ;; a query of another language: nothing runs (the ranges stay as they were)
      (setq-local nidhogg-treesit-fold-query (treesit-query-compile 'python "(block) @fold"))
      (goto-char (point-min))
      (insert "\n")
      (treesit-parser-root-node p)
      (should (= (length (funcall folds)) 4)))
    (nidhogg-lines-disable))))

(ert-deftest lines/tree-sitter-fold-predicates-and-refold ()
  "A fold query's text predicates hold in C (`#eq?', `#not-eq?', `#any-of?',
`#match?', `#not-match?'; a pattern with another predicate matches nothing),
and `nidhogg-treesit-refold' runs a query set after the buffer was parsed
over the whole tree."
  (skip-unless (and (lines-tests--built-p) (fboundp 'nidhogg-treesit-refold) (lines-tests--grammars)))
  (let ((treesit-extra-load-path (list (lines-tests--grammars))))
    (with-temp-buffer
      (insert "int test_a (void)\n{\n  return 1;\n}\n\nint b (void)\n{\n  return 2;\n}\n\nint c (void)\n{\n  return 3;\n}\n")
      (nidhogg-lines-enable nil t)
      (let ((p (treesit-parser-create 'c))
            (kinds (lambda () (mapcar (lambda (f) (list (nidhogg-lines-line nil (car f)) (nth 2 f)))
                                      (nidhogg-lines-folds nil)))))
        ;; parsed before the query is set: nothing until the refold
        (treesit-parser-root-node p)
        (setq-local nidhogg-treesit-fold-query
                    (treesit-query-compile
                     'c "((function_definition declarator: (function_declarator declarator: (identifier) @_n)) @fold.test
                          (#match? @_n \"^test_\"))
                         ((function_definition declarator: (function_declarator declarator: (identifier) @_n)) @fold.function
                          (#eq? @_n \"b\"))
                         ((function_definition declarator: (function_declarator declarator: (identifier) @_n)) @fold.class
                          (#any-of? @_n \"x\" \"c\"))
                         ((function_definition declarator: (function_declarator declarator: (identifier) @_n)) @fold.import
                          (#not-match? @_n \"^[bc]\") (#not-eq? @_n \"test_a\"))
                         ((function_definition) @fold.comment (#unknown? @fold.comment))"))
        (should-not (nidhogg-lines-folds nil))
        (nidhogg-treesit-refold p)
        ;; test_a a test, b a function, c a class; the not- and unknown patterns match none
        (should (equal (funcall kinds) '((0 5) (5 1) (10 2)))))
      (nidhogg-lines-disable))))

;; Design section 7.2: the highlight query's captures as the lines' class runs.
(ert-deftest lines/tree-sitter-highlight-classes ()
  "A highlight query's captures become the class runs of their lines: the
class by the capture's leading name (or an Emacs face name), a shorter
capture over a longer one and a later over an earlier (also across a
list of queries), an unknown name nothing; a refold runs a query set after the parse, and a reparse
replaces the runs of the lines it changed."
  (skip-unless (and (lines-tests--built-p) (fboundp 'nidhogg-lines-classes)
                    (lines-tests--grammars)))
  (let ((treesit-extra-load-path (list (lines-tests--grammars))))
    (with-temp-buffer
      (insert "int a (void)\n{\n  return 1; /* x */\n}\n")
      (nidhogg-lines-enable nil t)
      (let ((p (treesit-parser-create 'c)))
        (treesit-parser-root-node p)
        (setq-local nidhogg-treesit-highlight-query
                    (treesit-query-compile
                     'c "(primitive_type) @type.builtin
                         (return_statement \"return\" @keyword.return)
                         (number_literal) @font-lock-number-face
                         (comment) @comment
                         (identifier) @variable
                         ((identifier) @function (#eq? @function \"a\"))
                         (parameter_list) @no.such.class"))
        (should-not (nidhogg-lines-classes nil 0))
        (nidhogg-treesit-refold p)
        ;; int: type (3), a: function (2) over variable, void: type; the
        ;; parameter list's unknown name draws nothing
        (should (equal (nidhogg-lines-classes nil 0) '((0 3 3) (4 1 2) (7 4 3))))
        (should-not (nidhogg-lines-classes nil 1))
        ;; return: keyword (1), 1: constant (6), the comment (5)
        (should (equal (nidhogg-lines-classes nil 2) '((2 6 1) (9 1 6) (12 7 5))))
        ;; a reparse recolours the line it changed
        (goto-char (point-min))
        (search-forward "1;")
        (replace-match "22;")
        (treesit-parser-root-node p)
        (should (equal (nidhogg-lines-classes nil 2) '((2 6 1) (9 2 6) (13 7 5))))
        ;; an edit of the same size within a token: the parse's changed
        ;; ranges hold it, and the line (its runs cleared by the edit) is
        ;; coloured again
        (goto-char (point-min))
        (search-forward "22;")
        (replace-match "33;")
        (treesit-parser-root-node p)
        (should (equal (nidhogg-lines-classes nil 2) '((2 6 1) (9 2 6) (13 7 5))))
        (should (equal (nidhogg-lines-classes nil 0) '((0 3 3) (4 1 2) (7 4 3))))
        ;; a list of queries (a mode's font-lock rules): a later one's
        ;; capture over the same text wins
        (setq-local nidhogg-treesit-highlight-query
                    (list (treesit-query-compile 'c "(primitive_type) @type")
                          (treesit-query-compile 'c "(primitive_type) @keyword")))
        (nidhogg-treesit-refold p)
        (should (equal (nidhogg-lines-classes nil 0) '((0 3 1) (7 4 1))))
        (should-not (nidhogg-lines-classes nil 2)))
      (nidhogg-lines-disable))))

(defvar nidhogg-treesit-highlight-slice)

(defun lines-tests--many-functions (n)
  "N small C functions, five lines each."
  (mapconcat (lambda (i) (format "int f%d (void)\n{\n  return %d; /* c */\n}\n\n" i i))
             (number-sequence 0 (1- n)) ""))

(defun lines-tests--all-classes ()
  "Every line's class runs in the current buffer."
  (mapcar (lambda (l) (nidhogg-lines-classes nil l))
          (number-sequence 0 (1- (count-lines (point-min) (point-max))))))

(defun lines-tests--drain ()
  "Run highlight slices until none is left; how many ran."
  (let ((n 1))
    (while (and (nidhogg-treesit--highlight-slice) (< n 100000))
      (setq n (1+ n)))
    n))

;; Task 11e: a whole-text highlight pass runs in slices on an idle timer.
(ert-deftest lines/a-whole-highlight-pass-runs-in-idle-slices ()
  "A refold (or a model that missed its classes) colours its lines in
slices of `nidhogg-treesit-highlight-slice' seconds, the first at once and
the rest from an idle timer, each going on from where the last stopped.
The result is the whole pass's; an edit meanwhile shifts where the next
slice starts and its parse colours the changed lines at once; a killed
buffer's pass is dropped."
  (skip-unless (and (lines-tests--built-p) (fboundp 'nidhogg-lines-classes)
                    (lines-tests--grammars)))
  (let ((treesit-extra-load-path (list (lines-tests--grammars)))
        (query "(primitive_type) @type (number_literal) @constant
                (comment) @comment (identifier) @function")
        whole)
    ;; the whole pass, in one slice
    (with-temp-buffer
      (insert "/* top */\n" (lines-tests--many-functions 300))
      (nidhogg-lines-enable nil t)
      (let ((p (treesit-parser-create 'c))
            (nidhogg-treesit-highlight-slice 60))
        (treesit-parser-root-node p)
        (setq-local nidhogg-treesit-highlight-query (treesit-query-compile 'c query))
        (nidhogg-treesit-refold p)
        (should-not (nidhogg-treesit--highlight-slice))
        (setq whole (lines-tests--all-classes)))
      (nidhogg-lines-disable))
    (should (equal (nth 1496 whole) '((0 3 3) (4 4 2) (10 4 3))))
    ;; slices of no time: a line each
    (with-temp-buffer
      (insert (lines-tests--many-functions 300))
      (nidhogg-lines-enable nil t)
      (let ((p (treesit-parser-create 'c))
            (nidhogg-treesit-highlight-slice 0))
        (treesit-parser-root-node p)
        (setq-local nidhogg-treesit-highlight-query (treesit-query-compile 'c query))
        (nidhogg-treesit-refold p)
        (should (nidhogg-lines-classes nil 0))
        (should-not (nidhogg-lines-classes nil 1495))
        (should (seq-find (lambda (tm) (eq (timer--function tm) 'nidhogg-treesit--highlight-slice))
                          timer-idle-list))
        (dotimes (_ 20) (nidhogg-treesit--highlight-slice))
        (should (nidhogg-lines-classes nil 7))
        (should-not (nidhogg-lines-classes nil 1495))
        ;; a line typed above where the slices are: the next slice goes on
        ;; from the same text
        (goto-char (point-min))
        (insert "/* top */\n")
        (treesit-parser-root-node p)
        (should (equal (nidhogg-lines-classes nil 0) '((0 9 5))))
        (should (> (lines-tests--drain) 1000))
        (should (equal (lines-tests--all-classes) whole)))
      (nidhogg-lines-disable))
    ;; a killed buffer's pass is dropped
    (let ((b (generate-new-buffer " lines-slices")))
      (with-current-buffer b
        (insert (lines-tests--many-functions 50))
        (nidhogg-lines-enable nil t)
        (let ((p (treesit-parser-create 'c))
              (nidhogg-treesit-highlight-slice 0))
          (treesit-parser-root-node p)
          (setq-local nidhogg-treesit-highlight-query (treesit-query-compile 'c query))
          (nidhogg-treesit-refold p)))
      (kill-buffer b)
      (should-not (nidhogg-treesit--highlight-slice)))))

;; Design section 7.2: buffers without a highlight query take their
;; classes from the faces fontification gives them.
(defconst lines-tests--elisp ";; note\n(defun f () \"s\")\n")

(defun lines-tests--elisp-classes ()
  "Check the current buffer's classes for `lines-tests--elisp': the
comment one run (delimiter and body), `defun' a keyword, `f' a function,
the docstring a comment (`font-lock-doc-face')."
  (should (equal (nidhogg-lines-classes nil 0) '((0 7 5))))
  (should (equal (nidhogg-lines-classes nil 1) '((1 5 1) (7 1 2) (12 3 5)))))

(ert-deftest lines/faces-become-classes ()
  "Fontified text's faces become the class runs of its lines, by the
faces' names, through `nidhogg-lines-take-faces' (text fontified outside
redisplay)."
  (skip-unless (and (lines-tests--built-p) (fboundp 'nidhogg-lines-take-faces)))
  (with-temp-buffer
    (insert lines-tests--elisp)
    (emacs-lisp-mode)
    (nidhogg-lines-enable nil t)
    (font-lock-ensure)
    (should-not (nidhogg-lines-classes nil 1))
    (nidhogg-lines-take-faces nil)
    (lines-tests--elisp-classes)
    (nidhogg-lines-disable)))

(ert-deftest lines/redisplay-fontifies-classes ()
  "Redisplay, fontifying a shown buffer, gives the fontified text's faces
to the class runs of its lines."
  (skip-unless (and (lines-tests--built-p) (fboundp 'nidhogg-lines-take-faces)
                    (display-graphic-p)))
  (let ((b (generate-new-buffer "lines-faces")))
    (unwind-protect
        (with-current-buffer b
          (insert lines-tests--elisp)
          (emacs-lisp-mode)
          (font-lock-mode 1)
          (nidhogg-lines-enable nil t)
          (should-not (nidhogg-lines-classes nil 1))
          (save-window-excursion
            (set-window-buffer (selected-window) b)
            (redisplay t))
          (lines-tests--elisp-classes)
          (nidhogg-lines-disable))
      (kill-buffer b))))

;; Design section 7.4: a buffer with no parser of its own folds by the
;; hub's ranges, taken from Sjá on demand.
(ert-deftest lines/the-hubs-folds-are-taken-on-demand ()
  "`nidhogg-lines-take-hub-folds' copies Sjá's `folds' layer of a file
into the line model's hub source (1) as whole lines: nil before the
layer comes, t once taken (and again with no change), a newer version
replacing the ranges; a buffer whose parser folds it keeps those."
  (skip-unless (and (lines-tests--built-p) (fboundp 'nidhogg-view-push)))
  (let ((file "/t/lines-hub.txt")
        (push (lambda (v items)
                (nidhogg-view-push
                 (format "{\"event\":\"view-model\",\"data\":{\"minimap\":{\"file\":\"%s\",\"layer\":\"folds\",\"version\":%d,\"items\":%s}}}"
                         "/t/lines-hub.txt" v items)))))
    (unwind-protect
        (with-temp-buffer
          (insert "a {\n  b\n}\nc {\n  d\n}\n")
          (nidhogg-lines-enable nil t)
          (nidhogg-lines-set-fold-sources nil nil nil)
          (should-not (nidhogg-lines-take-hub-folds nil file))
          (funcall push 5 "[[1,3,\"function\"]]")
          (should (nidhogg-lines-take-hub-folds nil file))
          (should (equal (mapcar (lambda (f) (list (nidhogg-lines-line nil (car f))
                                                   (nidhogg-lines-line nil (cadr f)) (nth 2 f) (nth 4 f)))
                                 (nidhogg-lines-folds nil))
                         '((0 2 1 1))))
          (should (nidhogg-lines-take-hub-folds nil file))
          (funcall push 6 "[[4,6,\"class\"]]")
          (should (nidhogg-lines-take-hub-folds nil file))
          (should (equal (mapcar (lambda (f) (list (nidhogg-lines-line nil (car f)) (nth 2 f)))
                                 (nidhogg-lines-folds nil))
                         '((3 2))))
          ;; folded by its own parser (source 0): the hub's newer layer is
          ;; not taken
          (nidhogg-lines-set-folds nil 0 '((1 10 1)) 100)
          (funcall push 7 "[[1,6,\"block\"]]")
          (should-not (nidhogg-lines-take-hub-folds nil file))
          (should (equal (mapcar (lambda (f) (list (nidhogg-lines-line nil (car f)) (nth 4 f)))
                                 (nidhogg-lines-folds nil))
                         '((0 0) (3 1))))
          (nidhogg-lines-disable))
      (nidhogg-view-minimap 'forget file))))

;;; lines-tests.el ends here
