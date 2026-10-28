;;; nhwl-menu-tests.el --- tests for the Wayland shell's popup menus  -*- lexical-binding: t -*-

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

;; Batch tests of the model in lisp/term/nhwl-menu.el: how a menu is read
;; into items and what it returns.  The frames and the loop need a shell
;; and are covered by the GUI rows (nhwl-gui-w2-tests.el).

;;; Code:

(require 'ert)
(require 'nhwl-menu)

(defun nhwl-menu-test--labels (items)
  "The labels of ITEMS."
  (mapcar (lambda (item) (aref item 0)) items))

(defun nhwl-menu-test--item (items label)
  "The item of ITEMS labelled LABEL."
  (let ((found nil))
    (dolist (item items)
      (when (equal (aref item 0) label) (setq found item)))
    found))

(ert-deftest nhwl-menu/items-from-a-keymap ()
  (let ((map (make-sparse-keymap "Title")))
    (define-key map [plain] '("Plain" . ignore))
    (define-key map [off] '(menu-item "Off" ignore :enable nil))
    (define-key map [hidden] '(menu-item "Hidden" ignore :visible nil))
    (define-key map [tog] '(menu-item "Toggle" ignore :button (:toggle . t)))
    (define-key map [rad] '(menu-item "Radio" ignore :button (:radio . nil)))
    (define-key map [sep] '(menu-item "--"))
    (define-key map [dbl] '(menu-item "--double-line"))
    (define-key map [keyed]
      '(menu-item "Keyed" ignore :keys "C-c k" :help "Helpful"))
    (define-key map [sub]
      (list 'menu-item "Sub"
            (let ((sub (make-sparse-keymap "Sub")))
              (define-key sub [x] '("X" . ignore))
              sub)))
    (define-key map [find] '(menu-item "Find" find-file))
    (define-key map [old] '("Old" "Old help" . ignore))
    (let ((items (nhwl-menu--items map)))
      ;; in the order they were defined; the prompt is the title
      (should (equal (nhwl-menu-test--labels items)
                     '("Plain" "Off" "Hidden" "Toggle" "Radio" "--"
                       "--double-line" "Keyed" "Sub" "Find" "Old")))
      (should (equal (nhwl-menu--title map) "Title"))
      (let ((plain (nhwl-menu-test--item items "Plain")))
        (should (aref plain 2))
        (should (aref plain 3))
        (should (eq (aref plain 7) 'item))
        (should (eq (aref plain 8) 'plain)))
      (should-not (aref (nhwl-menu-test--item items "Off") 2))
      (should-not (aref (nhwl-menu-test--item items "Hidden") 3))
      (let ((tog (nhwl-menu-test--item items "Toggle"))
            (rad (nhwl-menu-test--item items "Radio")))
        (should (eq (aref tog 4) 'toggle))
        (should (eq (aref tog 5) t))
        (should (eq (aref rad 4) 'radio))
        (should-not (aref rad 5)))
      (should (eq (aref (nhwl-menu-test--item items "--") 7) 'separator))
      (should (eq (aref (nhwl-menu-test--item items "--double-line") 7)
                  'separator))
      (let ((keyed (nhwl-menu-test--item items "Keyed")))
        (should (equal (aref keyed 1) "C-c k"))
        (should (equal (aref keyed 6) "Helpful")))
      (should (equal (aref (nhwl-menu-test--item items "Find") 1) "C-x C-f"))
      (should (equal (aref (nhwl-menu-test--item items "Old") 6) "Old help"))
      (let ((sub (nhwl-menu-test--item items "Sub")))
        (should (eq (aref sub 7) 'submenu))
        (should (eq (car (aref sub 8)) 'sub))
        (should (keymapp (cdr (aref sub 8))))
        (should (aref sub 2))))))

(ert-deftest nhwl-menu/items-from-panes ()
  (let ((one (nhwl-menu--items '("Menu" ("Pane" ("a" . 1) "text" ("b" . nil)
                                         "--")))))
    (should (equal (nhwl-menu-test--labels one) '("a" "text" "b" "--")))
    (should (equal (aref (nth 0 one) 8) 1))
    (should (aref (nth 0 one) 2))
    (should-not (aref (nth 1 one) 2))
    (should (aref (nth 2 one) 2))
    (should (eq (aref (nth 3 one) 7) 'separator)))
  ;; several panes: a heading for each and a separator between them
  (let ((two (nhwl-menu--items '("Menu" ("A" ("x" . 1)) ("B" ("y" . 2))))))
    (should (equal (nhwl-menu-test--labels two) '("A" "x" "--" "B" "y")))
    (should-not (aref (nth 0 two) 2))
    (should (eq (aref (nth 2 two) 7) 'separator)))
  (should (equal (nhwl-menu--title '("Menu" ("A" ("x" . 1)))) "Menu")))

(ert-deftest nhwl-menu/filter-runs-once ()
  (let ((calls 0) (map (make-sparse-keymap)))
    (define-key map [f]
      (list 'menu-item "Filtered" 'ignore
            :filter (lambda (def) (setq calls (1+ calls)) def)))
    (define-key map [g]
      (list 'menu-item "Becomes a submenu" 'ignore
            :filter (lambda (_def)
                      (let ((sub (make-sparse-keymap)))
                        (define-key sub [z] '("Z" . ignore))
                        sub))))
    (let ((items (nhwl-menu--items map)))
      (should (= calls 1))
      (should (eq (aref (nhwl-menu-test--item items "Becomes a submenu") 7)
                  'submenu)))))

(ert-deftest nhwl-menu/return-value-for-a-keymap-path ()
  (let ((map (make-sparse-keymap))
        (sub (make-sparse-keymap)))
    (define-key sub [inner] '("Inner" . ignore))
    (define-key map [top] '("Top" . ignore))
    (define-key map [more] (list 'menu-item "More" sub))
    (let* ((items (nhwl-menu--items map))
           (top (nhwl-menu-test--item items "Top"))
           (more (nhwl-menu-test--item items "More"))
           (inner (nhwl-menu-test--item (nhwl-menu--items
                                         (cdr (aref more 8)))
                                        "Inner")))
      (should (equal (nhwl-menu--return map (list top)) '(top)))
      (should (equal (nhwl-menu--return map (list more inner))
                     '(more inner)))
      ;; a list of keymaps is a keymap menu too
      (should (equal (nhwl-menu--return (list map) (list top)) '(top)))
      (should-not (nhwl-menu--return map nil)))))

(ert-deftest nhwl-menu/return-value-for-a-pane ()
  (let* ((menu '("Menu" ("Pane" ("a" . 1) ("b" . (x y)))))
         (items (nhwl-menu--items menu)))
    (should (equal (nhwl-menu--return menu (list (nth 0 items))) 1))
    (should (equal (nhwl-menu--return menu (list (nth 1 items))) '(x y)))
    (should-not (nhwl-menu--return menu nil))))

(provide 'nhwl-menu-tests)

;;; nhwl-menu-tests.el ends here
