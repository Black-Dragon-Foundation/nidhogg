;;; varwatch-tests.el --- Variable watchers and aliases  -*- lexical-binding: t -*-
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

;; An alias shares its base variable's watchers: `add-variable-watcher'
;; and `remove-variable-watcher' set the base's write trap and walk the
;; obarray to set every alias's trap the same (`src/data.c').  Stock
;; Emacs walks for every call, so a watcher costs a pass over every
;; interned symbol.  Nidhogg walks only for a variable some
;; `defvaralias' has pointed an alias at; `nidhogg--variable-watcher-walks'
;; counts the walks.  The other rows pin what the walk is for: every
;; way an alias and a watcher can meet.
;;
;; Pure (pattern D): no time, no threads, no processes.

;;; Code:

(require 'ert)

(defun varwatch-tests--fresh (name)
  "A fresh interned symbol for NAME, unbound, with no alias and no watcher.
A symbol a former run left under that name is uninterned first."
  (let ((name (format "varwatch-tests--%s" name)))
    (unintern name obarray)
    (intern name)))

(defmacro varwatch-tests--with-log (&rest body)
  "Run BODY with `watch' a watcher and `log' its calls, newest first."
  (declare (indent 0))
  `(let* ((log nil)
          (watch (lambda (&rest args) (push args log))))
     (ignore watch)
     ,@body))

(ert-deftest varwatch-tests/unaliased-variables-walk-nothing ()
  "A watcher walks the obarray only for a variable an alias points at."
  (skip-unless (fboundp 'nidhogg--variable-watcher-walks))
  (let ((vars (mapcar (lambda (i) (varwatch-tests--fresh (format "plain-%d" i)))
                      (number-sequence 1 20)))
        (watch #'ignore)
        (before (nidhogg--variable-watcher-walks)))
    (dolist (v vars) (add-variable-watcher v watch))
    (dolist (v vars) (remove-variable-watcher v watch))
    (should (= (nidhogg--variable-watcher-walks) before))
    ;; the count is not idle: a variable with an alias walks once per call
    (let ((base (varwatch-tests--fresh "counted-base"))
          (alias (varwatch-tests--fresh "counted-alias")))
      (defvaralias alias base)
      (add-variable-watcher base watch)
      (should (= (nidhogg--variable-watcher-walks) (1+ before)))
      (remove-variable-watcher base watch)
      (should (= (nidhogg--variable-watcher-walks) (+ 2 before))))))

(ert-deftest varwatch-tests/base-watcher-reaches-an-alias-made-before ()
  (varwatch-tests--with-log
    (let ((base (varwatch-tests--fresh "b1")) (alias (varwatch-tests--fresh "a1")))
      (defvaralias alias base)
      (add-variable-watcher base watch)
      (set alias 1)
      (should (equal log `((,base 1 set nil))))
      (should (equal (get-variable-watchers alias) (list watch))))))

(ert-deftest varwatch-tests/base-watcher-reaches-an-alias-made-after ()
  (varwatch-tests--with-log
    (let ((base (varwatch-tests--fresh "b2")) (alias (varwatch-tests--fresh "a2")))
      (add-variable-watcher base watch)
      (defvaralias alias base)
      (setq log nil)
      (set alias 1)
      (should (equal log `((,base 1 set nil))))
      (should (equal (get-variable-watchers alias) (list watch))))))

(ert-deftest varwatch-tests/alias-watcher-reaches-the-base ()
  (varwatch-tests--with-log
    (let ((base (varwatch-tests--fresh "b3")) (alias (varwatch-tests--fresh "a3"))
          (other (varwatch-tests--fresh "o3")))
      (defvaralias alias base)
      (defvaralias other base)
      (add-variable-watcher alias watch)
      (set base 1)
      (set other 2)
      (should (equal log `((,base 2 set nil) (,base 1 set nil))))
      (should (equal (get-variable-watchers base) (list watch)))
      (should (equal (get-variable-watchers other) (list watch))))))

(ert-deftest varwatch-tests/remove-on-the-base-clears-the-alias ()
  (varwatch-tests--with-log
    (let ((base (varwatch-tests--fresh "b4")) (alias (varwatch-tests--fresh "a4")))
      (defvaralias alias base)
      (add-variable-watcher base watch)
      (remove-variable-watcher base watch)
      (set alias 1)
      (set base 2)
      (should-not log)
      (should-not (get-variable-watchers alias)))))

(ert-deftest varwatch-tests/remove-on-the-alias-clears-the-base ()
  (varwatch-tests--with-log
    (let ((base (varwatch-tests--fresh "b5")) (alias (varwatch-tests--fresh "a5")))
      (add-variable-watcher base watch)
      (defvaralias alias base)          ; binds the unbound base: a `let' call
      (setq log nil)
      (remove-variable-watcher alias watch)
      (set alias 1)
      (set base 2)
      (should-not log)
      (should-not (get-variable-watchers base)))))

(ert-deftest varwatch-tests/remove-one-of-two-keeps-the-alias-trapped ()
  (varwatch-tests--with-log
    (let ((base (varwatch-tests--fresh "b6")) (alias (varwatch-tests--fresh "a6")))
      (defvaralias alias base)
      (add-variable-watcher base watch)
      (add-variable-watcher alias #'ignore)
      (remove-variable-watcher alias #'ignore)
      (set alias 1)
      (should (equal log `((,base 1 set nil)))))))

(ert-deftest varwatch-tests/defvaralias-of-a-watched-variable ()
  "A watched variable made an alias reports it and takes its base's trap."
  (varwatch-tests--with-log
    (let ((base (varwatch-tests--fresh "b7")) (alias (varwatch-tests--fresh "a7")))
      (set base 0)
      (add-variable-watcher alias watch)
      (defvaralias alias base)
      (should (equal log `((,alias ,base defvaralias nil))))
      (setq log nil)
      (set alias 1)
      (should-not log)
      ;; a watcher added to the base now reaches it
      (add-variable-watcher base watch)
      (set alias 2)
      (should (equal log `((,base 2 set nil)))))))

(ert-deftest varwatch-tests/chain-made-from-the-far-end ()
  "A -> B made after B -> C: a watcher on C reaches A."
  (varwatch-tests--with-log
    (let ((c (varwatch-tests--fresh "c8")) (b (varwatch-tests--fresh "b8"))
          (a (varwatch-tests--fresh "a8")))
      (defvaralias b c)
      (defvaralias a b)
      (add-variable-watcher c watch)
      (set a 1)
      (should (equal log `((,c 1 set nil)))))))

(ert-deftest varwatch-tests/chain-made-from-the-near-end ()
  "B -> C made after A -> B: a watcher on C, or added through A, reaches both."
  (varwatch-tests--with-log
    (let ((c (varwatch-tests--fresh "c9")) (b (varwatch-tests--fresh "b9"))
          (a (varwatch-tests--fresh "a9")))
      (defvaralias a b)
      (defvaralias b c)
      (add-variable-watcher c watch)
      (set a 1)
      (set b 2)
      (should (equal log `((,c 2 set nil) (,c 1 set nil))))
      (remove-variable-watcher c watch)
      (setq log nil)
      (add-variable-watcher a watch)
      (set b 3)
      (set c 4)
      (should (equal log `((,c 4 set nil) (,c 3 set nil)))))))

(ert-deftest varwatch-tests/alias-from-the-dump ()
  "An alias preloaded at build time still shares its base's watchers."
  (should (eq (indirect-variable 'find-file-hooks) 'find-file-hook))
  (should-not (get-variable-watchers 'find-file-hook))
  (unwind-protect
      (progn
        (add-variable-watcher 'find-file-hook #'ignore)
        (should (equal (get-variable-watchers 'find-file-hooks) '(ignore))))
    (remove-variable-watcher 'find-file-hook #'ignore))
  (should-not (get-variable-watchers 'find-file-hooks)))

(ert-deftest varwatch-tests/uninterned-alias-of-an-interned-base ()
  "As in stock Emacs: an uninterned alias made before the watcher misses it.
The walk sees only the symbols interned in `obarray'."
  (varwatch-tests--with-log
    (let ((base (varwatch-tests--fresh "b10")) (alias (make-symbol "varwatch-tests--a10")))
      (defvaralias alias base)
      (add-variable-watcher base watch)
      (set alias 1)
      (should-not log)
      (set base 2)
      (should (equal log `((,base 2 set nil)))))))

;;; varwatch-tests.el ends here
