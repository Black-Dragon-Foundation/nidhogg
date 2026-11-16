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

;;; lines-tests.el ends here
