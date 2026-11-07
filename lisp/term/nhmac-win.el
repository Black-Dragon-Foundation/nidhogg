;;; nhmac-win.el --- Lisp half of Naglfar, Nidhogg's Rust macOS shell  -*- lexical-binding: t -*-

;; Copyright (C) 2026 Black Dragon Foundation contributors.

;; This file is part of Nidhogg.

;; Nidhogg is free software: you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or (at
;; your option) any later version.

;; Nidhogg is distributed in the hope that it will be useful,
;; but WITHOUT ANY WARRANTY; without even the implied warranty of
;; MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
;; GNU General Public License for more details.

;; You should have received a copy of the GNU General Public License
;; along with Nidhogg.  If not, see <https://www.gnu.org/licenses/>.

;;; Commentary:

;; Naglfar is the shell Nidhogg builds on macOS (`--with-nhmac').  This file
;; is the part of it that is Lisp.
;;
;; The menu flattener: the C side of a native menu bar would have to walk
;; `menu_items' and a tree of `widget_value's.  Here the walk of `menu.c'
;; (`parse_single_submenu' and `digest_single_submenu') is done in Lisp,
;; with `:filter', `:enable', `:visible', `:button', `:keys' and `:help'
;; evaluated, and the result is one flat vector.  C only copies that vector
;; into an array of records (`nhg_menu_tree'); `menu.rs' in the glue
;; decodes and validates it.
;;
;; The vector holds `nhmac-menu-width' slots per entry, in menu order (a
;; submenu's entry is followed by its items):
;;
;;   0 depth     0 for the menu bar's own titles, 1 for their items, ...
;;   1 kind      one of the `nhmac-menu-kind-*' numbers
;;   2 label     the item's text ("" for a separator)
;;   3 key       the key description shown beside it ("" for none)
;;   4 enabled   1 or 0
;;   5 state     1 for a selected toggle or radio button, else 0
;;   6 index     where the item sits in `menu_items' (the slot C maps a
;;               choice back from), or -1 for an entry with no slot (a
;;               title, a named pane)
;;
;; The same numbers are in `nidhogg/glue/src/menu.rs'.

;;; Code:

(eval-when-compile (require 'cl-lib))

(defconst nhmac-menu-width 7
  "Slots per entry in a flattened menu.")

(defconst nhmac-menu-kind-item 0 "A command.")
(defconst nhmac-menu-kind-submenu 1 "An item that opens the entries one level deeper that follow it.")
(defconst nhmac-menu-kind-separator 2 "A separator line.")
(defconst nhmac-menu-kind-toggle 3 "A toggle (check mark) item.")
(defconst nhmac-menu-kind-radio 4 "A radio item.")

;; `menu_items' layout (keyboard.h): a pane takes 3 slots, an item 8, and a
;; submenu's start and end one each.
(defconst nhmac-menu--pane-length 3)
(defconst nhmac-menu--item-length 8)

(defconst nhmac-menu--separator-names
  '("space" "no-line" "single-line" "double-line" "single-dashed-line"
    "double-dashed-line" "shadow-etched-in" "shadow-etched-out"
    "shadow-etched-in-dash" "shadow-etched-out-dash"
    "shadow-double-etched-in" "shadow-double-etched-out"
    "shadow-double-etched-in-dash" "shadow-double-etched-out-dash")
  "The names after \"--\" that make a label a separator (`menu_separator_name_p').")

(defun nhmac-menu--separator-p (label)
  "Non-nil if LABEL is a separator line, as `menu_separator_name_p' decides."
  (cond
   ((and (>= (length label) 4) (string-prefix-p "--" label)
         (/= (aref label 2) ?-))
    (member (substring label 2) nhmac-menu--separator-names))
   (t (string-match-p "\\`-*\\'" label))))

;; The slot of the simulated `menu_items' vector the next push uses, and the
;; entries produced, newest first.
(defvar nhmac-menu--used 0)

(defun nhmac-menu--eval (form)
  "Value of FORM, or nil when it signals (`menu_item_eval_property')."
  (let ((inhibit-redisplay t))
    (condition-case nil (eval form nil) (error nil))))

(defun nhmac-menu--parse-item (item inbar)
  "Parse ITEM, a keymap binding, as `parse_menu_item' does.
Nil if it is not an item (or is hidden).  Else a vector
[NAME ENABLE DEF MAP KEYEQ TYPE SELECTED HELP], with DEF and MAP the
submenu's keymap when it has one.  INBAR is non-nil at the menu bar's top level."
  (when (consp item)
    (let ((enable t) def name help keyeq type selected filter keyhint
          (rest (cdr item)))
      (cond
       ((stringp (car item))
        (setq name (car item))
        (when (stringp (car-safe rest))
          (setq help (substitute-command-keys (car rest))
                rest (cdr rest)))
        (when (and (consp rest) (consp (car rest))
                   (or (null (caar rest)) (vectorp (caar rest))))
          (setq rest (cdr rest)))
        (setq def rest)
        (when (symbolp def)
          (let ((tem (get def 'menu-enable)))
            (cond (enable-disabled-menus-and-buttons (setq enable t))
                  (tem (setq enable tem))))))
       ((and (eq (car item) 'menu-item) (consp rest))
        (setq name (car rest))
        (let ((start (cdr rest)))
          (if (consp start)
              (let ((props (cdr start)))
                (setq def (car start))
                (when (and (consp props) (consp (car props)))
                  (setq props (cdr props)))
                (while (consp (cdr props))
                  (let ((prop (car props)) (val (cadr props)))
                    (setq props (cddr props))
                    (pcase prop
                      (:enable (setq enable
                                     (if enable-disabled-menus-and-buttons t val)))
                      (:visible (unless (nhmac-menu--eval val)
                                  (setq name :hidden)))
                      (:help (setq help (if (stringp val)
                                            (substitute-command-keys val)
                                          val)))
                      (:filter (setq filter val))
                      (:key-sequence
                       (when (or (symbolp val) (stringp val) (vectorp val))
                         (setq keyhint (list val))))
                      (:keys
                       (cond ((functionp val) (setq keyeq (funcall val)))
                             ((or (consp val) (stringp val)) (setq keyeq val))))
                      (:button
                       (when (and (consp val) (memq (car val) '(:toggle :radio)))
                         (setq selected (cdr val) type (car val))))))))
            (when (or inbar start) (setq name :hidden)))))
       (t (setq name :hidden)))
      (unless (eq name :hidden)
        (unless (stringp name)
          (setq name (nhmac-menu--eval name)))
        (when (stringp name)
          (when filter
            (setq def (nhmac-menu--eval (list filter (list 'quote def)))))
          (unless (eq enable t)
            (setq enable (nhmac-menu--eval enable)))
          (cond
           ((and inbar (null enable)) nil)
           ((null def) (and (not inbar) (vector name enable def nil keyeq type selected help)))
           (t
            (let ((map (nhmac-menu--keymap-of def)))
              (cond
               (map (vector name enable map map keyeq type selected help))
               (inbar (vector name enable def nil keyeq type selected help))
               (t
                (vector name enable def nil
                        (nhmac-menu--key-hint def keyeq keyhint)
                        type
                        (and selected (nhmac-menu--eval selected))
                        help)))))))))))

(defun nhmac-menu--keymap-of (def)
  "The keymap DEF stands for (`get_keymap'), or nil."
  (let ((map (indirect-function def)))
    (and (keymapp map) (if (symbolp def) map def))))

(defun nhmac-menu--key-hint (def keyeq keyhint)
  "The key description to show beside DEF (`parse_menu_item''s tail), or nil.
KEYEQ is the item's `:keys' value, KEYHINT its `:key-sequence' as a list."
  (if (and (stringp keyeq) (not keyhint))
      (substitute-command-keys keyeq)
    (let ((prefix keyeq) keys)
      (when (consp prefix)
        (setq def (car prefix) prefix (cdr prefix)))
      (when (and keyhint (car keyhint))
        (setq keys (car keyhint))
        (let ((tem (key-binding keys)))
          (when (or (null tem)
                    (and (not (eq tem def))
                         (not (and (symbolp def) (eq tem (symbol-function def))))))
            (setq keys nil))))
      (unless keys
        (setq keys (where-is-internal def nil t)))
      (when keys
        (let ((tem (key-description keys)))
          (when (consp prefix)
            (when (stringp (car prefix)) (setq tem (concat (car prefix) tem)))
            (when (stringp (cdr prefix)) (setq tem (concat tem (cdr prefix)))))
          tem)))))

;; A node is [LABEL ENABLE HINT TYPE SELECTED INDEX CHILDREN SUBMENU]: one
;; item pushed by `single_menu_item'.  A pane is (NAME . NODES).

(defun nhmac-menu--push-pane ()
  "Take a pane's slots in the simulated `menu_items'."
  (cl-incf nhmac-menu--used nhmac-menu--pane-length))

(defun nhmac-menu--push-item ()
  "Take an item's slots; return the first."
  (prog1 nhmac-menu--used (cl-incf nhmac-menu--used nhmac-menu--item-length)))

(defun nhmac-menu--keymap-panes (keymap name maxdepth)
  "The panes `single_keymap_panes' makes for KEYMAP: the main one, then those
of its `@' items.  NAME is the main pane's."
  (when (> maxdepth 0)
    (let ((pane (list name)) (pending nil) (nodes nil))
      (nhmac-menu--push-pane)
      (map-keymap-internal
       (lambda (_key item)
         (let ((p (nhmac-menu--parse-item item nil)))
           (when p
             (let ((label (aref p 0)) (enable (aref p 1)) (map (aref p 3)))
               (if (and map (string-prefix-p "@" label))
                   (when enable (push (list map label) pending))
                 (let ((node (vector label enable (aref p 4) (aref p 5)
                                     (aref p 6) (nhmac-menu--push-item)
                                     nil nil)))
                   (when (and map enable)
                     (cl-incf nhmac-menu--used)
                     (aset node 6 (nhmac-menu--nodes
                                   (nhmac-menu--keymap-panes
                                    map nil (1- maxdepth))))
                     (aset node 7 (and (aref node 6) t))
                     (cl-incf nhmac-menu--used))
                   (push node nodes)))))))
       (condition-case nil (keymap-canonicalize keymap) (error nil)))
      (setcdr pane (nreverse nodes))
      (cons pane
            (apply #'append
                   (mapcar (lambda (e)
                             (nhmac-menu--keymap-panes
                              (car e) (substring (cadr e) 1) (1- maxdepth)))
                           (nreverse pending)))))))

(defun nhmac-menu--nodes (panes)
  "The nodes of PANES in one list: inside a submenu the panes' names are dropped."
  (apply #'append (mapcar #'cdr panes)))

(defun nhmac-menu--entry (depth node)
  "The entries of NODE at DEPTH, as a list of vectors, newest last."
  (let* ((label (aref node 0))
         (sep (nhmac-menu--separator-p label))
         (type (aref node 3))
         (kind (cond (sep nhmac-menu-kind-separator)
                     ((aref node 7) nhmac-menu-kind-submenu)
                     ((eq type :toggle) nhmac-menu-kind-toggle)
                     ((eq type :radio) nhmac-menu-kind-radio)
                     (t nhmac-menu-kind-item))))
    (cons (vector depth kind (if sep "" label)
                  (if sep "" (nhmac-menu--prettify-key (aref node 2)))
                  (if (and (aref node 1) (not sep)) 1 0)
                  (if (and (aref node 4) (memq type '(:toggle :radio))) 1 0)
                  (aref node 5))
          (apply #'append
                 (mapcar (lambda (c) (nhmac-menu--entry (1+ depth) c))
                         (aref node 6))))))

(defconst nhmac-menu--key-symbols
  '(("<backspace>" . "⌫") ("DEL" . "⌫") ("<deletechar>" . "⌦")
    ("<return>" . "↩") ("RET" . "↩") ("<left>" . "←") ("<right>" . "→")
    ("<up>" . "↑") ("<down>" . "↓") ("<prior>" . "⇞") ("<next>" . "⇟")
    ("<home>" . "↖") ("<end>" . "↘") ("<tab>" . "⇥") ("TAB" . "⇥")
    ("<backtab>" . "⇤"))
  "The keyboard symbols NS shows for keys (`prettify_key' in nsmenu.m).")

(defun nhmac-menu--prettify-key (key)
  "KEY, a key description or nil, as NS shows it: no leading blanks, symbols."
  (if (null key)
      ""
    (setq key (string-trim-left key " +"))
    (dolist (s nhmac-menu--key-symbols)
      (setq key (string-replace (car s) (cdr s) key)))
    key))

(defun nhmac-menu--bar-items (maps)
  "The menu bar's items for MAPS, `menu_bar_items' does: a list of (KEY NAME DEFS).
MAPS are keymaps in priority order, each already the `menu-bar' prefix map."
  (let ((items nil))
    (dolist (map (reverse maps))
      (when (consp map)
        (let ((seen nil))
          (map-keymap-internal
           (lambda (key item)
             (cond
              ((eq item 'undefined)
               (setq items (assq-delete-all key items))))
             (unless (or (memq key seen) (null item))
               (push key seen)
               (let ((p (nhmac-menu--parse-item item t)))
                 (when p
                   (let* ((def (aref p 2))
                          (old (assq key items)))
                     (if (null old)
                         (setq items (append items (list (list key (aref p 0) (list def)))))
                       (setcar (cddr old)
                               (cons def (and (keymapp def) (keymapp (car (nth 2 old)))
                                              (nth 2 old))))))))))
           (keymap-canonicalize map)))))
    ;; Move the items that belong at the end.
    (dolist (final menu-bar-final-items)
      (let ((cell (assq final items)))
        (when cell
          (setq items (append (delq cell items) (list cell))))))
    items))

(defun nhmac-menu--flatten-bar-item (item)
  "The entries for the menu bar ITEM (KEY NAME DEFS): `parse_single_submenu' and
`digest_single_submenu'.  Returns (ENTRIES)."
  (let* ((name (nth 1 item))
         (panes nil) (top nil))
    (dolist (def (nth 2 item))
      (if (not (keymapp def))
          (progn
            (setq top t)
            (nhmac-menu--push-pane)
            (push (list nil (vector name t nil nil nil (nhmac-menu--push-item) nil nil))
                  panes))
        (let ((prompt (keymap-prompt def)))
          (dolist (p (nhmac-menu--keymap-panes def (or prompt name) 10))
            (push p panes)))))
    (setq panes (nreverse panes))
    (let* ((top-level-panes (length panes))
           (children nil))
      (dolist (pane panes)
        (let ((pname (if (= top-level-panes 1) "" (or (car pane) ""))))
          (if (string-empty-p pname)
              (setq children (append children (cdr pane)))
            (setq children
                  (append children
                          (list (vector pname t nil nil nil -1 (cdr pane) t)))))))
      (if (and top (= (length children) 1))
          (nhmac-menu--entry 0 (car children))
        (nhmac-menu--entry 0 (vector name t nil nil nil -1 children t))))))

(defun nhmac-menu--flatten (maps)
  "Flatten the menu bar whose `menu-bar' prefix keymaps are MAPS.
MAPS is a keymap or a list of them in priority order, as
`current-active-maps' gives their `menu-bar' bindings.  Return the flat
vector described in the commentary.  The walk is `menu.c''s; no frame is
needed."
  (setq maps (if (keymapp maps) (list maps) maps))
  (let ((nhmac-menu--used 0)
        (entries nil))
    (dolist (item (nhmac-menu--bar-items maps))
      (setq entries (nconc entries (nhmac-menu--flatten-bar-item item))))
    (apply #'vconcat entries)))

(defun nhmac-menu-bar-maps ()
  "The `menu-bar' prefix keymaps of the current buffer, in priority order."
  (delq nil (mapcar (lambda (map)
                      (let ((m (lookup-key map [menu-bar])))
                        (and (keymapp m) m)))
                    (current-active-maps))))

(defun nhmac-menu-flatten-bar ()
  "Flatten the selected window's buffer's menu bar, after its update hooks."
  (let ((inhibit-quit t))
    (run-hooks 'activate-menubar-hook 'menu-bar-update-hook)
    (nhmac-menu--flatten (nhmac-menu-bar-maps))))

(provide 'nhmac-win)

;;; nhmac-win.el ends here
