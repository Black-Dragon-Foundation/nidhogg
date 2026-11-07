;;; nhmac-win-tests.el --- tests for Naglfar's Lisp half  -*- lexical-binding: t -*-

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

;; Batch tests of lisp/term/nhmac-win.el.  Tasks append under ";; Task N:".

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'menu-bar)
(require 'nhmac-win)

(defconst nhmac-win-test--dir
  (file-name-directory (or load-file-name buffer-file-name)))

;; Task 6: the menu flattener

(defun nhmac-win-test--rows (vec)
  "VEC, a flattened menu, as a list of (PATH KIND KEY ENABLED STATE).
KIND is coarse (NS says nothing of toggles and radios): 0 item, 1 submenu,
2 separator.  PATH is the labels from the title down, joined with \"/\"."
  (let ((w nhmac-menu-width) (stack nil) (rows nil))
    (dotimes (i (/ (length vec) w))
      (let* ((o (* i w)) (depth (aref vec o)) (kind (aref vec (+ o 1))))
        (setq stack (append (seq-take (reverse stack) depth) nil)
              stack (reverse stack))
        (push (aref vec (+ o 2)) stack)
        (push (list (mapconcat #'identity (reverse stack) "/")
                    (pcase kind (1 1) (2 2) (_ 0))
                    (substring-no-properties (aref vec (+ o 3)))
                    (aref vec (+ o 4)) (aref vec (+ o 5)))
              rows)))
    (nreverse rows)))

(defun nhmac-win-test--golden ()
  "The NS fixture as rows like `nhmac-win-test--rows'."
  (let ((stack nil))
    (mapcar (lambda (e)
              (setq stack (reverse (seq-take (reverse stack) (nth 0 e))))
              (push (nth 2 e) stack)
              (list (mapconcat #'identity (reverse stack) "/")
                    (nth 1 e) (nth 3 e) (nth 4 e) (nth 5 e)))
            (with-temp-buffer
              (insert-file-contents
               (expand-file-name "nhmac-win-resources/menu-bar-golden.eld"
                                 nhmac-win-test--dir))
              (read (current-buffer))))))

;; What a batch Emacs cannot give the way the NS session did.  The first two
;; lists are dropped from both sides, the others only have the named field
;; ignored.  Everything else in the fixture's 831 rows must match.
(defconst nhmac-win-test--drop
  '("Options/Multilingual Environment/Set Coding Systems/For X Selections/Clipboard"
    "Options/Multilingual Environment/Set Coding Systems/For Next X Selection"
    "Options/Show/Hide/Tool Bar"
    "Options/Use System Font")
  "Rows (and their descendants) that differ in kind between NS and this build, or exist in one only.
The X selection rows need an X display, the tool bar is a submenu of positions on
a toolkit that has them, and `Use System Font' exists when the build has
the `system-font-setting' feature (a GTK one), which NS does not.")

(defconst nhmac-win-test--drop-children
  '("Buffers")
  "Rows whose items are session state (the buffer list).")

(defconst nhmac-win-test--ignore
  '(("Edit/Paste" . enabled)
    ("Options/Multilingual Environment/Set Coding Systems/For Terminal" . enabled)
    ("Options/Show/Hide/Scroll Bar/On the Right" . state)
    ("Options/Customize Emacs/Top-level Emacs Customization Group" . key))
  "Rows whose enable, state or key hint depends on the session (selections, frame, the NS key tables).")

(defun nhmac-win-test--normalize (rows)
  "ROWS without the gaps above."
  (let ((out nil))
    (dolist (r rows)
      (let ((path (car r)))
        (unless (or (cl-some (lambda (d) (or (equal path d) (string-prefix-p (concat d "/") path)))
                             nhmac-win-test--drop)
                    (cl-some (lambda (d) (string-prefix-p (concat d "/") path))
                             nhmac-win-test--drop-children))
          (let ((ign (cdr (assoc path nhmac-win-test--ignore))))
            (push (list path (nth 1 r)
                        (if (eq ign 'key) :ignored (nth 2 r))
                        (if (eq ign 'enabled) :ignored (nth 3 r))
                        (if (eq ign 'state) :ignored (nth 4 r)))
                  out)))))
    (nreverse out)))

(defun nhmac-win-test--ns-edit-menu ()
  "`menu-bar-edit-menu' as it is built when `ns' is a feature."
  (let ((saved features) form)
    (with-temp-buffer
      (insert-file-contents (locate-library "menu-bar.el"))
      (goto-char (point-min))
      (re-search-forward "^(defvar menu-bar-edit-menu\n  (let")
      (goto-char (match-beginning 0))
      (setq form (read (current-buffer))))
    (unwind-protect
        (progn (setq features (cons 'ns features))
               (eval (nth 2 form) nil))
      (setq features saved))))

(defun nhmac-win-test--bind-ns-keys (map)
  "Install in MAP the global bindings `ns-win.el' makes."
  (with-temp-buffer
    (insert-file-contents (locate-library "term/ns-win.el"))
    (goto-char (point-min))
    (condition-case nil
        (while t
          (let ((f (read (current-buffer))))
            (when (and (eq (car-safe f) 'define-key) (eq (cadr f) 'global-map))
              (ignore-errors (eval `(define-key ',map ,@(cddr f)) t)))))
      (end-of-file nil))))

(defun nhmac-win-test--call-as-ns (fn)
  "Call FN in a *scratch* that looks like the NS session's, and return its value.
What NS had at startup is rebuilt: `ns-win.el's key bindings, the Edit menu
with `ns' a feature, a graphic display, transient-mark and blink-cursor on, a
buffer with undo history."
  (let* ((old (current-global-map))
         (new (copy-keymap old))
         (saved-features features)
         (saved-tmm (default-value 'transient-mark-mode))
         (saved-blink (default-value 'blink-cursor-mode)))
    (unwind-protect
        (progn
          (use-global-map new)
          (nhmac-win-test--bind-ns-keys new)
          (let ((bar (copy-keymap (lookup-key new [menu-bar]))))
            (define-key bar [edit] (cons "Edit" (nhmac-win-test--ns-edit-menu)))
            (define-key new [menu-bar] bar))
          (setq-default transient-mark-mode t blink-cursor-mode t)
          (setq features (cons 'ns features))
          (with-current-buffer (get-buffer-create "*scratch*")
            (buffer-enable-undo)
            (insert "a")
            (cl-letf (((symbol-function 'display-graphic-p) (lambda (&rest _) t))
                      ((symbol-function 'window-system) (lambda (&rest _) 'ns)))
              (funcall fn))))
      (setq features saved-features)
      (setq-default transient-mark-mode saved-tmm blink-cursor-mode saved-blink)
      (use-global-map old))))

(ert-deftest nhmac-win-menu-bar-matches-ns ()
  "The flattener on `emacs -Q's menu bar gives what NS built, field by field.
The NS session is rebuilt first (`nhmac-win-test--call-as-ns'); the rows
batch cannot reproduce are listed in `nhmac-win-test--ignore' and the lists beside it."
  (let* ((got (nhmac-win-test--normalize
               (nhmac-win-test--rows
                (nhmac-win-test--call-as-ns #'nhmac-menu-flatten-bar))))
         (want (nhmac-win-test--normalize (nhmac-win-test--golden))))
    (should (> (length want) 700))
    (should (= (length got) (length want)))
    (cl-mapc (lambda (g w) (should (equal g w))) got want)))

(defun nhmac-win-test--bar (&rest items)
  "A menu-bar keymap of ITEMS, each (KEY NAME . SUBMENU-ITEMS) or (KEY NAME COMMAND).
A submenu item is a binding as in a keymap."
  (let ((bar (make-sparse-keymap)))
    (dolist (item (reverse items))
      (let ((rest (cddr item)))
        (define-key bar (vector (car item))
          (if (and (null (cdr rest)) (symbolp (car rest)))
              (list 'menu-item (cadr item) (car rest))
            (cons (cadr item) (let ((m (make-sparse-keymap (cadr item))))
                                (dolist (b (reverse rest))
                                  (define-key m (vector (car b)) (cdr b)))
                                m))))))
    bar))

(defun nhmac-win-test--ref (vec)
  "VEC, flattened, as a list of (DEPTH KIND LABEL KEY ENABLED STATE INDEX)."
  (let ((w nhmac-menu-width))
    (cl-loop for o from 0 below (length vec) by w
             collect (cl-loop for j below w collect (aref vec (+ o j))))))

(ert-deftest nhmac-win-menu-filter-runs-once ()
  "A `:filter' runs once per walk, for an item in a submenu and for a bar item."
  (let ((n 0) (m 0))
    (let ((bar (nhmac-win-test--bar
                `(file "File"
                       (a menu-item "A" nhmac-win-test--cmd
                          :filter ,(lambda (d) (cl-incf n) d)))
                `(tools "Tools" (b "B" . nhmac-win-test--cmd)))))
      (define-key bar [filtered]
        `(menu-item "Filtered" ,(let ((s (make-sparse-keymap "Filtered")))
                                  (define-key s [c] '("C" . nhmac-win-test--cmd))
                                  s)
                    :filter ,(lambda (d) (cl-incf m) d)))
      (nhmac-menu--flatten bar))
    (should (= n 1))
    (should (= m 1))))

(ert-deftest nhmac-win-menu-enable-error-disables ()
  "An error in `:enable' disables the item; `:visible' that errors hides it."
  (let* ((bar (nhmac-win-test--bar
               `(file "File"
                      (a menu-item "A" nhmac-win-test--cmd :enable ,'(error "boom"))
                      (b menu-item "B" nhmac-win-test--cmd :visible ,'(error "boom"))
                      (c menu-item "C" nhmac-win-test--cmd))))
         (rows (nhmac-win-test--rows (nhmac-menu--flatten bar))))
    (should (equal (mapcar #'car rows) '("File" "File/A" "File/C")))
    (should (equal (mapcar (lambda (r) (nth 3 r)) rows) '(1 0 1)))))

(ert-deftest nhmac-win-menu-entry-fields-and-indexes ()
  "Kinds, toggle state, key hints, separators and the `menu_items' indexes."
  (let* ((on t)
         (bar (nhmac-win-test--bar
               `(file "File"
                      (a menu-item "A" nhmac-win-test--cmd :keys "C-x A")
                      (s "--")
                      (t1 menu-item "T" nhmac-win-test--cmd :button (:toggle . ,on))
                      (r1 menu-item "R" nhmac-win-test--cmd :button (:radio . nil))
                      (sub "Sub" . ,(let ((s (make-sparse-keymap "Sub")))
                                      (define-key s [z] '("Z" . nhmac-win-test--cmd))
                                      s)))
                `(edit "Edit" (q "Q" . nhmac-win-test--cmd))))
         (ref (nhmac-win-test--ref (nhmac-menu--flatten bar))))
    (should (equal ref
                   `((0 ,nhmac-menu-kind-submenu "File" "" 1 0 -1)
                     (1 ,nhmac-menu-kind-item "A" "C-x A" 1 0 3)
                     (1 ,nhmac-menu-kind-separator "" "" 0 0 11)
                     (1 ,nhmac-menu-kind-toggle "T" "" 1 1 19)
                     (1 ,nhmac-menu-kind-radio "R" "" 1 0 27)
                     (1 ,nhmac-menu-kind-submenu "Sub" "" 1 0 35)
                     (2 ,nhmac-menu-kind-item "Z" "" 1 0 47)
                     (0 ,nhmac-menu-kind-submenu "Edit" "" 1 0 -1)
                     (1 ,nhmac-menu-kind-item "Q" "" 1 0 59))))))

(provide 'nhmac-win-tests)
;;; nhmac-win-tests.el ends here
