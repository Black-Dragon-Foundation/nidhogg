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
;;
;; The ns compatibility layer (last section): the `ns-*' variables and
;; functions that NS defines in C and that `ns-win.el' and packages use.

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

;; The keys from the menu bar's own key down to the item being walked, newest
;; first, and (while flattening for the shell) a hash from the item's place in
;; `menu_items' to its key path.  A choice comes back as the place; the path
;; is what the command loop is fed (`find_and_call_menu_selection').
(defvar nhmac-menu--path nil)
(defvar nhmac-menu--paths nil)

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
       (lambda (key item)
         (let ((p (nhmac-menu--parse-item item nil)))
           (when p
             (let ((label (aref p 0)) (enable (aref p 1)) (map (aref p 3)))
               (if (and map (string-prefix-p "@" label))
                   (when enable (push (list map label) pending))
                 (let ((node (vector label enable (aref p 4) (aref p 5)
                                     (aref p 6) (nhmac-menu--push-item)
                                     nil nil (reverse (cons key nhmac-menu--path)))))
                   (when (and map enable)
                     (cl-incf nhmac-menu--used)
                     (aset node 6 (nhmac-menu--nodes
                                   (let ((nhmac-menu--path (cons key nhmac-menu--path)))
                                     (nhmac-menu--keymap-panes
                                      map nil (1- maxdepth)))))
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
    (when (and nhmac-menu--paths (> (length node) 8) (aref node 8)
               (>= (aref node 5) 0))
      (puthash (aref node 5) (aref node 8) nhmac-menu--paths))
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
            (push (list nil (vector name t nil nil nil (nhmac-menu--push-item) nil nil
                                    (list (car item))))
                  panes))
        (let ((prompt (keymap-prompt def)))
          (dolist (p (let ((nhmac-menu--path (list (car item))))
                       (nhmac-menu--keymap-panes def (or prompt name) 10)))
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

;;; The menu bar, built at idle

;; The shell shows a native menu bar from the last tree Lisp sent it.  The
;; shallow part (the titles) is in the tree; the deep part is built here, from
;; an idle timer, never when a menu opens (the menu answers from the last tree:
;; no Lisp runs).  Triggers while a build is pending are dropped.

(defvar nhmac-menu-generation 0
  "The generation of the tree the shell holds; a choice carries it.")
(defvar nhmac-menu-paths nil
  "An alist from an item's place in the tree to its key path, for generation
`nhmac-menu-generation'.  C turns a choice into the menu-bar events of the path.")
(defvar nhmac-menu-builds 0 "How many deep builds ran (a test reads it).")
(defvar nhmac-menu--last nil "The flat vector the shell has.")
(defvar nhmac-menu--timer nil "The one pending idle build, or nil.")
(defvar nhmac-menu-delay 0.3 "Seconds of idleness before the deep build.")

(defun nhmac-menu-encode (vec)
  "VEC, a flat menu vector, as the text the shell reads: records of
`nhmac-menu-width' fields, the fields joined by US (31) and the records by RS (30).
A string field has its line breaks and those two characters turned into blanks.
(Arguments of `nhmac--call' are passed as printed, without quotes.)"
  (let ((w nhmac-menu-width) (i 0) (recs nil))
    (while (< i (length vec))
      (push (mapconcat (lambda (f)
                         (if (stringp f)
                             (replace-regexp-in-string "[\n\C-^\C-_]" " " f)
                           (number-to-string f)))
                       (substring vec i (+ i w)) "\C-_")
            recs)
      (setq i (+ i w)))
    (mapconcat #'identity (nreverse recs) "\C-^")))

(defun nhmac-menu-flatten-bar-with-paths ()
  "The flat vector of the menu bar and the alist of its items' key paths: (VECTOR . PATHS)."
  (let* ((nhmac-menu--paths (make-hash-table))
         (vec (nhmac-menu-flatten-bar))
         (paths nil))
    (maphash (lambda (k p) (push (cons k p) paths)) nhmac-menu--paths)
    (cons vec paths)))

(defun nhmac-menu-build ()
  "Build the deep tree now and send it when it changed.  Quits at input."
  (setq nhmac-menu--timer nil)
  (when (and (fboundp 'nhmac--call) (display-graphic-p))
    (let ((r (while-no-input (nhmac-menu-flatten-bar-with-paths))))
      (when (consp r)
        (cl-incf nhmac-menu-builds)
        (unless (equal (car r) nhmac-menu--last)
          (setq nhmac-menu--last (car r)
                nhmac-menu-paths (cdr r)
                nhmac-menu-generation (1+ nhmac-menu-generation))
          (funcall 'nhmac--call 'menu-bar nhmac-menu-generation (nhmac-menu-encode (car r)))))
      (nhmac-dock-menu-send))))

(defun nhmac-menu-invalidate (&rest _)
  "Schedule one deep rebuild `nhmac-menu-delay' after the idle time so far.
More calls while one is pending change nothing."
  (unless nhmac-menu--timer
    (setq nhmac-menu--timer
          (run-with-idle-timer (time-add (or (current-idle-time) 0) nhmac-menu-delay)
                               nil #'nhmac-menu-build))))

(defun nhmac-menu--focus-changed ()
  "A frame gained or lost focus: build at once when one has it."
  (when (and (not nhmac-menu--timer) (display-graphic-p) (frame-focus-state))
    (nhmac-menu-build)))

(add-hook 'post-command-hook #'nhmac-menu-invalidate)
(add-hook 'buffer-list-update-hook #'nhmac-menu-invalidate)
(add-function :after after-focus-change-function #'nhmac-menu--focus-changed)

;; Popup menus: the same records, shown by the shell's tracking loop.  The
;; shell answers the chosen item's place; the value is looked up here.

(defun nhmac-menu--popup-pane-entries (pane depth)
  "Records for PANE's lines at DEPTH: (ENTRIES . VALUES), the places counted in `nhmac-menu--used'."
  (let ((entries nil) (values nil))
    (dolist (line (cdr pane))
      (let* ((label (if (consp line) (car line) line))
             (value (and (consp line) (cdr line)))
             (label (if (stringp label) label ""))
             (sep (nhmac-menu--separator-p label))
             (place (prog1 nhmac-menu--used (cl-incf nhmac-menu--used))))
        (push (vector depth (if sep nhmac-menu-kind-separator nhmac-menu-kind-item)
                      (if sep "" label) "" (if (and value (not sep)) 1 0) 0 place)
              entries)
        (when (and value (not sep)) (push (cons place value) values))))
    (cons (nreverse entries) (nreverse values))))

(defun nhmac-menu--popup-vector (menu)
  "The flat vector of MENU, as `x-popup-menu' takes it, and its values: (VECTOR . ALIST).
ALIST maps an item's place to what choosing it returns."
  (let ((nhmac-menu--used 0) (entries nil) (values nil))
    (cond
     ((or (keymapp menu) (and (consp menu) (keymapp (car menu))))
      ;; keymaps: a choice returns the keys that lead to it
      (let ((nhmac-menu--paths (make-hash-table)) (nhmac-menu--path nil))
        (dolist (map (if (keymapp menu) (list menu) menu))
          (dolist (node (nhmac-menu--nodes (nhmac-menu--keymap-panes map nil 10)))
            (setq entries (nconc entries (nhmac-menu--entry 0 node)))))
        (maphash (lambda (k p) (push (cons k p) values)) nhmac-menu--paths)))
     (t
      (let ((panes (cdr menu)))
        (dolist (pane panes)
          (let* ((single (null (cdr panes)))
                 (r (nhmac-menu--popup-pane-entries pane (if single 0 1))))
            (unless single
              (setq entries (nconc entries
                                   (list (vector 0 nhmac-menu-kind-submenu (or (car pane) "") ""
                                                 1 0 -1)))))
            (setq entries (nconc entries (car r))
                  values (nconc values (cdr r))))))))
    (cons (apply #'vconcat entries) values)))

(defun nhmac-menu--popup-xy (position)
  "POSITION, as `x-popup-menu' takes it, as (X . Y) in the frame's pixels."
  (let ((xy (cond
             ((and (consp position) (consp (car position)) (numberp (caar position)))
              (let ((win (cadr position)) (off (car position)))
                (if (window-live-p win)
                    (let ((e (window-pixel-edges win)))
                      (cons (+ (nth 0 e) (car off)) (+ (nth 1 e) (cadr off))))
                  (cons (car off) (cadr off)))))
             ((and (consp position) (ignore-errors (event-start position)))
              (let* ((p (event-start position)) (pxy (posn-x-y p)) (win (posn-window p)))
                (if (and (windowp win) (window-live-p win) (consp pxy))
                    (let ((e (window-pixel-edges win)))
                      (cons (+ (nth 0 e) (car pxy)) (+ (nth 1 e) (cdr pxy))))
                  (cons 0 0))))
             (t (cons 0 0)))))
    xy))

(defun nhmac-menu-popup (position menu)
  "Show MENU (as `x-popup-menu' takes it) at POSITION in the shell's menu; return the choice.
Nil when nothing is chosen."
  (let* ((vec (nhmac-menu--popup-vector menu))
         (xy (nhmac-menu--popup-xy position))
         (choice (nhmac--query 'menu-popup (selected-frame) (car xy) (cdr xy)
                               (nhmac-menu-encode (car vec)))))
    (and (integerp choice) (cdr (assq choice (cdr vec))))))

(defalias 'nhwl-menu-popup #'nhmac-menu-popup)

;;; Application events

;; The shell reports quitting, files opened from Finder or the Dock, and a
;; reopen as menu-bar events of fixed keys (generation 0 in C): the commands
;; below run in the command loop.

(defvar nhmac-app-paths '((1 . (nhmac-open-files)) (2 . (nhmac-quit)) (3 . (nhmac-reopen))
                          (4 . (nhmac-appearance-changed)) (5 . (nhmac-dock-new-frame))
                          (6 . (nhmac-service)))
  "The key path of each application event, by the shell's number.
The Dock's items are application events too: their places in the Dock's
records are these numbers.")

(defvar nhmac-dock--sent nil "Whether the shell has the Dock's menu.")

(defun nhmac-dock-menu-send ()
  "Send the Dock's menu once: NS's one item, New Frame, as a menu record whose
place is its application event number."
  (unless nhmac-dock--sent
    (funcall 'nhmac--call 'menu-dock
             (nhmac-menu-encode (vector 0 nhmac-menu-kind-item "New Frame" "" 1 0 5)))
    (setq nhmac-dock--sent t)))

(defun nhmac-dock-new-frame ()
  "The Dock menu's New Frame: make a frame, as NS's does."
  (interactive)
  (make-frame))

(declare-function ns-spi-service-call "term/ns-win" ())

(defun nhmac-service ()
  "Answer the Services requests other applications made, each as NS does,
through `ns-spi-service-call'."
  (interactive)
  (dolist (r (nhmac--query 'take-service-requests))
    (let ((ns-input-spi-name (car r)) (ns-input-spi-arg (cdr r)))
      (ns-spi-service-call))))

;; Drops: the shell reports them as the Wayland shell does (C's
;; DRAG_N_DROP_EVENT with `(quote XdndSelection TIME)'), and the offer is
;; read from the shell in NS's form, so `ns-drag-n-drop' handles it.

(defun nhmac--drag-n-drop (orig event)
  "Call ORIG, `ns-drag-n-drop', with EVENT's drop in NS's form, then finish the drop."
  (if (not (eq (car-safe (nth 2 event)) 'quote))
      (funcall orig event)
    (let ((offer (nhmac--query 'dnd-offer)))
      (unwind-protect
          (when offer
            (funcall orig (list (car event) (nth 1 event) offer)))
        (nhmac--call 'dnd-finish)))))

(advice-add 'ns-drag-n-drop :around #'nhmac--drag-n-drop)

(defvar ns-confirm-quit)
(defvar ns-input-file)

(defun nhmac-open-files ()
  "Visit the files the Finder or the Dock opened with Emacs, each as NS's `ns-find-file' does."
  (interactive)
  (dolist (file (nhmac--query 'take-open-files))
    (let ((ns-input-file (list file)))
      (ns-find-file))))

(defun nhmac-quit ()
  "Quit Emacs as the application menu asks: after a question when `ns-confirm-quit' is set."
  (interactive)
  (when (or (not ns-confirm-quit) (y-or-n-p "Are you sure you want to quit Emacs? "))
    (save-buffers-kill-emacs)))

(defun nhmac-reopen ()
  "The Dock icon was clicked with no frame showing: show one."
  (interactive)
  (if (frame-list)
      (make-frame-visible (selected-frame))
    (make-frame)))

(defvar ns-system-appearance nil
  "The system's appearance, `dark' or `light' (nil before the shell reports it).")
(defvar ns-system-appearance-change-functions nil
  "Functions called with the new system appearance when it changes.")

(defun nhmac-appearance-changed ()
  "The system switched between light and dark: record it and run the hook."
  (interactive)
  (setq ns-system-appearance (nhmac--query 'system-appearance))
  (run-hook-with-args 'ns-system-appearance-change-functions ns-system-appearance))

(defun nhmac-apply-frame-look (&optional frame)
  "Give FRAME's window its `ns-appearance' and `ns-transparent-titlebar' parameters."
  (let ((frame (or frame (selected-frame))))
    (when (eq (framep frame) 'ns)
      (nhmac--call 'frame-look frame
                   (or (frame-parameter frame 'ns-appearance) 'nil)
                   (if (frame-parameter frame 'ns-transparent-titlebar) 't 'nil)))))

(add-hook 'after-make-frame-functions #'nhmac-apply-frame-look)

(dolist (cmd '(nhmac-open-files nhmac-quit nhmac-reopen nhmac-appearance-changed
               nhmac-dock-new-frame nhmac-service))
  (define-key global-map (vector 'menu-bar cmd) cmd))

;;; The ns compatibility layer

;; Packages and `ns-win.el' read and call what NS defines in C.  Here the
;; variables are `defvar's with NS's defaults, and the functions are Lisp
;; over `nhmac--call' and `nhmac--query' (the two primitives that pass
;; records to the shell).  A function the shell does not serve yet signals
;; `nhmac--not-yet'.

(declare-function nhmac--call "nhmacterm.c" (what &rest args))
(declare-function nhmac--query "nhmacterm.c" (what &rest args))

(defun nhmac--not-yet ()
  "Signal that the shell does not serve this function yet."
  (error "Naglfar: not available yet"))

(defun nhmac-stats ()
  "Return the shell's counters, a plist.
The pumps' scope (`:takes', `:overlapping-takes', `:queued', `:applied'), the lock
hand-off (`:lock-waiters', `:parks-with-waiters'), the main wait (`:waits',
`:sleeps', `:kq-wakes', `:empty-wakes', `:kicks', `:helper-fds',
`:helper-wakes', `:helper-hops-keys', `:kq-changes') and the surfaces
(`:bytes-copied', `:damage-bytes', `:full-copies', `:commits',
`:surface-busy', `:ring-acquires', `:implicit-open-at-update-end',
`:spins-in-redisplay' (run-loop passes outside the pumps), `:pumps')."
  (nhmac--query 'stats))

(defun nhmac--test-raise-in-pump ()
  "Make the shell's next pump raise an Objective-C exception.
The pump catches it and the display is poisoned, as by a panic.  For tests."
  (nhmac--call 'test-raise-in-pump))

(defun nhmac--window-info (&optional frame)
  "Return the facts of FRAME's window as the shell has them, a plist.
`:visible', `:layer' (the root view hosts the shell's layer),
`:wants-layer', `:subviews', `:device-width' and `:device-height'.  For
tests."
  (nhmac--query 'window-info (or frame (selected-frame))))

(defun nhmac--modal-alert (ms)
  "Run a modal alert for MS milliseconds, then end it; return its rounds.
A query that runs a modal session, for the pumps' tests."
  (nhmac--query 'modal-alert ms))

(defun nhmac--modal-resize (ms)
  "Like `nhmac--modal-alert', resizing a test window inside the modal.
Return its rounds and the scope's counters' change.  For tests."
  (nhmac--query 'modal-resize ms))

(defun nhmac--test-post-key (char &optional ignoring flags frame)
  "Post a press and a release of a key to FRAME's window, as AppKit events.
CHAR is the event's `characters', IGNORING its
`charactersIgnoringModifiers' (default CHAR), both characters; FLAGS
its `NSEventModifierFlags' (default 0).  The next pump sends them
through `sendEvent:'.  For tests."
  (nhmac--call 'test-post-key (or frame (selected-frame)) char
               (or ignoring char) (or flags 0)))

(defun nhmac--test-post-mouse (kind x y &optional button flags frame)
  "Post a mouse event of KIND at X, Y to FRAME's window, as an AppKit event.
KIND is `down', `up', `drag', `move' or `wheel'; X and Y are device
pixels from the frame's top left.  BUTTON is 0 (left, the default), 1
(right) or 2 (middle), or for `wheel' the lines to scroll (positive
down).  FLAGS is the `NSEventModifierFlags' (default 0).  For tests."
  (nhmac--call 'test-post-mouse (or frame (selected-frame))
               (pcase kind ('down 0) ('up 1) ('drag 2) ('move 3) ('wheel 4)
                      (_ (error "Unknown mouse kind %s" kind)))
               x y (or button 0) (or flags 0)))

(defun nhmac--test-reentry ()
  "Make AppKit call the shell's delegates back from inside its own calls.
Return the scope's counters' change and the queued counts at each step,
a plist.  For tests."
  (nhmac--query 'test-reentry))

(defun nhmac--test-helper-next ()
  "Make the main wait's next new fd registration go to its helper thread.
As if kqueue had refused it (`EINVAL').  For tests."
  (nhmac--call 'test-helper-next))

(defun nhmac--test-surface (x y &optional frame)
  "Return the newest surface of FRAME's window, a plist.
`:w', `:h' (device pixels), `:points' (the content size), the layer's
`:scale' and `:gravity', `:contents-is-newest', `:srgb' (`srgb',
`generic' or nil) and `:pixel', the premultiplied ARGB at X, Y.  For
tests."
  (nhmac--query 'test-surface (or frame (selected-frame)) x y))

(defun nhmac--test-kq-probe (path)
  "Open PATH and ask a fresh kqueue for its readability.
Return (:open ERRNO :kq ERRNO), 0 meaning success; :kq is -1 when
the open failed.  For tests."
  (nhmac--query 'test-kq-probe path))


;; Variables

(defvar ns-alternate-modifier 'meta
  "This variable describes the behavior of the alternate or option key.")

(defvar ns-antialias-text t
  "Non-nil (the default) means to render text antialiased.")

(defvar ns-auto-hide-menu-bar nil
  "Non-nil means that the menu bar is hidden.")

(defvar ns-click-through t
  "Whether to pass activation clicks through to Emacs.")

(defvar ns-command-modifier 'super
  "This variable describes the behavior of the command key.")

(defvar ns-confirm-quit nil
  "Whether to confirm application quit using dialog.")

(defvar ns-control-modifier 'control
  "This variable describes the behavior of the control key.")

(defvar ns-drag-motion-function 'ns-handle-drag-motion
  "Function called when another program drags items over Emacs.")

(defvar ns-function-modifier 'none
  "This variable describes the behavior of the function (fn) key.")

(defvar ns-icon-type-alist '(t)
  "Alist of elements (REGEXP .")

(defvar ns-input-file nil
  "The file specified in the last NS event.")

(defvar ns-input-font nil
  "The font specified in the last NS event.")

(defvar ns-input-fontsize nil
  "The fontsize specified in the last NS event.")

(defvar ns-input-line nil
  "The line specified in the last NS event.")

(defvar ns-input-spi-arg nil
  "The service argument specified in the last NS event.")

(defvar ns-input-spi-name nil
  "The service name specified in the last NS event.")

(defvar ns-mwheel-line-height nil
  "The number of pixels touchpad scrolling considers one line.")

(defvar ns-reg-to-script nil
  "Internal map of font registry to Unicode script.")

(defvar ns-right-alternate-modifier 'left
  "This variable describes the behavior of the right alternate or option.")

(defvar ns-right-command-modifier 'left
  "This variable describes the behavior of the right command key.")

(defvar ns-right-control-modifier 'left
  "This variable describes the behavior of the right control key.")

(defvar ns-scroll-event-delta-factor 1.0
  "A factor to apply to pixel deltas reported in scroll events.")

(defvar ns-sent-selection-hooks nil
  "A list of functions to be called when Emacs answers a selection request.")

(defvar ns-use-fullscreen-animation nil
  "Non-nil means use animation on non-native fullscreen.")

(defvar ns-use-mwheel-acceleration t
  "Non-nil means use macOS's standard mouse wheel acceleration.")

(defvar ns-use-mwheel-momentum t
  "Non-nil means mouse wheel scrolling uses momentum.")

(defvar ns-use-native-fullscreen t
  "Non-nil means to use native fullscreen on Mac OS X 10.7 and later.")

(defvar ns-use-proxy-icon t
  "When non-nil display a proxy icon in the titlebar.")

(defvar ns-use-srgb-colorspace t
  "Non-nil means to use sRGB colorspace on Mac OS X 10.7 and later.")

(defvar ns-use-thin-smoothing nil
  "Non-nil turns on a font smoothing method that produces thinner strokes.")

(defvar ns-version-string "Naglfar"
  "Toolkit version for NS Windowing.")

(defvar ns-working-text nil
  "String for visualizing working composition sequence.")


;; Functions

(defun ns-badge (badge)
  "Set the app icon badge to BADGE.

Not available yet in Naglfar."
  (ignore badge)
  (nhmac--not-yet))

(defun ns-begin-drag (frame pboard action &optional return-frame allow-same-frame follow-tooltip)
  "Begin a drag-and-drop operation on FRAME.

Not available yet in Naglfar."
  (ignore frame pboard action return-frame allow-same-frame follow-tooltip)
  (nhmac--not-yet))

(defun ns-block-system-sleep (why allow-display-sleep)
  "Block system idle sleep.

Not available yet in Naglfar."
  (ignore why allow-display-sleep)
  (nhmac--not-yet))

(defun ns-disown-selection-internal (selection)
  "If we own the selection SELECTION, disown it.
Only the clipboard exists on macOS."
  (nhmac--call 'pasteboard-disown selection))

(defun ns-display-monitor-attributes-list (&optional terminal)
  "Return a list of physical monitor attributes on the X display TERMINAL.

Not available yet in Naglfar."
  (ignore terminal)
  (nhmac--not-yet))

(defun ns-do-applescript (script)
  "Execute AppleScript SCRIPT and return the result.

Not available yet in Naglfar."
  (ignore script)
  (nhmac--not-yet))

(defun ns-emacs-info-panel ()
  "Shows the `Info' or `About' panel for Emacs.

Not available yet in Naglfar."
  (nhmac--not-yet))

(defun ns-font-name (name)
  "Determine font PostScript or family name for font NAME.

Not available yet in Naglfar."
  (ignore name)
  (nhmac--not-yet))

(defun ns-frame-edges (&optional frame type)
  "Return edge coordinates of FRAME.

Not available yet in Naglfar."
  (ignore frame type)
  (nhmac--not-yet))

(defun ns-frame-geometry (&optional frame)
  "Return geometric attributes of FRAME.

Not available yet in Naglfar."
  (ignore frame)
  (nhmac--not-yet))

(defun ns-frame-list-z-order (&optional terminal)
  "Return list of Emacs's frames, in Z (stacking) order.

Not available yet in Naglfar."
  (ignore terminal)
  (nhmac--not-yet))

(defun ns-frame-restack (frame1 frame2 &optional above)
  "Restack FRAME1 below FRAME2.

Not available yet in Naglfar."
  (ignore frame1 frame2 above)
  (nhmac--not-yet))

(defun ns-get-resource (owner name)
  "Return the value of the property NAME of OWNER from the defaults.
The user default NAME (`NSUserDefaults'), a string, or nil; OWNER is
ignored."
  (ignore owner)
  (nhmac--query 'get-resource name))

(defun ns-get-selection (selection-name target-type)
  "Return text selected from some X window.
The clipboard's text for TARGET-TYPE (a file's URL for the file types), or nil."
  (nhmac--query 'pasteboard-read selection-name target-type))

(defun ns-hide-emacs (on)
  "If ON is non-nil, the entire Emacs application is hidden.

Not available yet in Naglfar."
  (ignore on)
  (nhmac--not-yet))

(defun ns-hide-others ()
  "Hides all applications other than Emacs.

Not available yet in Naglfar."
  (nhmac--not-yet))

(defun ns-list-colors (&optional frame)
  "Return a list of all available colors.
In Naglfar, for now, the names of `color-name-rgb-alist' (common-win.el
reads this while Emacs is dumped, for `x-colors')."
  (ignore frame)
  (mapcar #'car color-name-rgb-alist))

(defun ns-list-services ()
  "List available Nextstep services by querying NSApp.

Naglfar offers no services yet: the list is empty."
  nil)

(defun ns-mouse-absolute-pixel-position ()
  "Return absolute position of mouse cursor in pixels.

Not available yet in Naglfar."
  (nhmac--not-yet))

(defun ns-own-selection-internal (selection value)
  "Assert an X selection of type SELECTION and value VALUE.
The clipboard takes the text of VALUE; the other selections do not exist on macOS."
  (when (stringp value)
    (nhmac--call 'pasteboard-own selection (substring-no-properties value))))

(defun ns-perform-service (service send)
  "Perform Nextstep SERVICE on SEND, a string.
Return the text the service answers, or nil."
  (nhmac--query 'service-perform (concat service "\C-_" send)))

(defun ns-popup-color-panel (&optional frame)
  "Pop up the color panel.

Not available yet in Naglfar."
  (ignore frame)
  (nhmac--not-yet))

(defun ns-process-is-accessibility-trusted ()
  "Return non-nil if Emacs is trusted by macOS Accessibility.

Not available yet in Naglfar."
  (nhmac--not-yet))

(defun ns-progress-indicator (progress)
  "Bounce the app dock icon to request user attention.

Not available yet in Naglfar."
  (ignore progress)
  (nhmac--not-yet))

(defun ns-read-file-name (prompt &optional dir mustmatch init dir-only-p)
  "Use a graphical panel to read a file name, using prompt PROMPT.
An open panel when MUSTMATCH, else a save panel; DIR-ONLY-P chooses a directory.
The answer is the path, or nil when the panel is cancelled."
  (nhmac--query 'file-dialog
                (mapconcat #'identity
                           (list (or prompt "") (if dir (expand-file-name dir) "") (or init "")
                                 (if mustmatch "1" "0") (if dir-only-p "1" "0"))
                           "\C-_")))

(defun ns-request-user-attention (urgency)
  "Bounce the app dock icon to request user attention.

Not available yet in Naglfar."
  (ignore urgency)
  (nhmac--not-yet))

(defun ns-reset-menu ()
  "Cause the NS menu to be re-calculated.

Not available yet in Naglfar."
  (nhmac--not-yet))

(defun ns-selection-exists-p (&optional selection)
  "Whether there is an owner for the given X selection."
  (nhmac--query 'pasteboard-exists (or selection 'PRIMARY)))

(defun ns-selection-owner-p (&optional selection)
  "Whether the current Emacs process owns the given X Selection."
  (nhmac--query 'pasteboard-owned (or selection 'PRIMARY)))

(defun ns-send-items (items)
  "Send a list of items to macOS applications or services.

Not available yet in Naglfar."
  (ignore items)
  (nhmac--not-yet))

(defun ns-set-mouse-absolute-pixel-position (x y)
  "Move mouse pointer to absolute pixel position (X, Y).

Not available yet in Naglfar."
  (ignore x y)
  (nhmac--not-yet))

(defun ns-set-resource (owner name value)
  "Set property NAME of OWNER to VALUE, from the defaults database.
The user default NAME (`NSUserDefaults'); a nil VALUE removes it.
OWNER is ignored."
  (ignore owner)
  (if value
      (nhmac--call 'set-resource name value)
    (nhmac--call 'unset-resource name)))

(defun ns-show-character-palette ()
  "Show the macOS character palette.

Not available yet in Naglfar."
  (nhmac--not-yet))

(defun ns-unblock-system-sleep (token)
  "Unblock system idle sleep.

Not available yet in Naglfar."
  (ignore token)
  (nhmac--not-yet))

;; Variables the shell reads.  Rust gets the new value when one is set; a
;; `let' binding does not reach it.

(defconst nhmac--watched-variables
  '(ns-alternate-modifier ns-right-alternate-modifier
    ns-command-modifier ns-right-command-modifier
    ns-control-modifier ns-right-control-modifier ns-function-modifier
    ns-use-srgb-colorspace ns-confirm-quit ns-auto-hide-menu-bar
    ns-use-native-fullscreen ns-antialias-text ns-use-thin-smoothing)
  "The variables whose changes `nhmac--variable-changed' passes to the shell.")

(defun nhmac--variable-changed (symbol newval operation where)
  "Pass NEWVAL of SYMBOL to the shell when it is set (a variable watcher).
OPERATION and WHERE are as `add-variable-watcher' gives them; only a global
`set' counts.  Nothing is passed before the shell is up."
  (when (and (eq operation 'set) (null where) (fboundp 'nhmac--call))
    (funcall 'nhmac--call 'variable symbol newval)))

(dolist (variable nhmac--watched-variables)
  (add-variable-watcher variable #'nhmac--variable-changed))

(provide 'nhmac-win)

;;; nhmac-win.el ends here
