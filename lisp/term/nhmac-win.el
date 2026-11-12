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
;; per-frame scale and text zoom, preloaded with this file (`load': the
;; dump refuses a `require' of a file not loaded yet); fonts open at 72
;; dpi times the scale here, as C opens them (`NHWL_DPI')
(unless (featurep 'nidhogg-scale) (load "term/nidhogg-scale" nil t))
(defvar nidhogg-scale-dpi)
(setq nidhogg-scale-dpi 72.0)

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
  "Build the deep tree now and send it when it changed.  Quits at input.
A build allocates about 1 MB (a byte-compiled build of the default menu bar:
19k conses and 84k vector cells), which crosses the default
`gc-cons-threshold' and collected twice in round 2's lane (0.16 s of a
0.19 s build): no collection starts inside the build and its send, so the
tree reaches the shell first and the next allocation after it collects."
  (setq nhmac-menu--timer nil)
  (when (and (fboundp 'nhmac--call) (display-graphic-p))
    (let* ((gc-cons-threshold (max gc-cons-threshold (* 8 1024 1024)))
           (r (while-no-input (nhmac-menu-flatten-bar-with-paths))))
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
;; read from the shell in NS's form, so `ns-drag-n-drop' handles it.  The
;; drop is finished through the glue, as on Wayland, so the glue's drop
;; ends with it.

(declare-function pgtk-drop-finish "nhwlfns.c" (success timestamp delete))

(defun nhmac--drag-n-drop (orig event)
  "Call ORIG, `ns-drag-n-drop', with EVENT's drop in NS's form, then finish the drop."
  (if (not (eq (car-safe (nth 2 event)) 'quote))
      (funcall orig event)
    (let ((offer (nhmac--query 'dnd-offer)))
      (unwind-protect
          (when offer
            (funcall orig (list (car event) (nth 1 event) offer)))
        (pgtk-drop-finish t (nth 2 (nth 2 event)) nil)))))

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

;;; The input method

;; The shell reports marked text as the Wayland shell does (C's
;; PREEDIT_TEXT_EVENT, a `preedit-text' event of (TEXT PROPS) parts), and
;; it is shown as NS shows it: `ns-working-text', put and taken out by
;; `ns-win.el''s own functions.  A commit comes as keystrokes.

(declare-function ns-put-working-text "term/ns-win" ())
(declare-function ns-unput-working-text "term/ns-win" ())
(defvar ns-working-text)

(defun nhmac-preedit-text (event)
  "Show the marked text of EVENT, a `preedit-text' event, as NS's working text."
  (interactive "e")
  (let ((text (mapconcat #'car (nth 1 event) "")))
    (if (string-empty-p text)
        (ns-unput-working-text)
      (setq ns-working-text text)
      (ns-put-working-text))))

(define-key special-event-map [preedit-text] #'nhmac-preedit-text)

;;; The ns compatibility layer

;; Packages and `ns-win.el' read and call what NS defines in C.  Here the
;; variables are `defvar's with NS's defaults, and the functions are Lisp
;; over `nhmac--call' and `nhmac--query' (the two primitives that pass
;; records to the shell), or the shared C's where the Wayland code serves
;; the same (geometry, monitors, drags).

(declare-function nhmac--call "nhmacterm.c" (what &rest args))
(declare-function nhmac--query "nhmacterm.c" (what &rest args))

(defun nhmac-stats ()
  "Return the shell's counters, a plist.
The pumps' scope (`:takes', `:overlapping-takes', `:queued', `:applied'), the lock
hand-off (`:lock-waiters', `:parks-with-waiters'), the main wait (`:waits',
`:sleeps', `:kq-wakes', `:empty-wakes', `:kicks', `:helper-fds',
`:helper-wakes', `:helper-hops-keys', `:kq-changes') and the surfaces
(`:bytes-copied', `:damage-bytes', `:full-copies', `:commits',
`:surface-busy', `:ring-acquires', `:implicit-open-at-update-end',
`:spins-in-redisplay' (run-loop passes outside the pumps), `:pumps',
`:handed-pumps' (a Lisp thread's pumps, handed to the main thread)) and
the commit pacer (`:held', `:held-timeouts', `:pacer-fires',
`:pacer-arms') and the font driver (`:font-calls', the glue's
`nhg_font_*' calls, and `:font-fills', glyph-table pages filled), and
Zoom's cursor moves (`:zoom-calls')."
  (nhmac--query 'stats))

(defun nhmac--test-raise-in-pump ()
  "Make the shell's next pump raise an Objective-C exception.
The pump catches it and the display is poisoned, as by a panic.  For tests."
  (nhmac--call 'test-raise-in-pump))

(defun nhmac--window-info (&optional frame)
  "Return the facts of FRAME's window as the shell has them, a plist.
`:visible', `:layer' (the root view hosts the shell's layer),
`:wants-layer', `:subviews', `:device-width', `:device-height',
`:screen-device-width', `:frame' (AppKit's frame in points, y up from
the main screen's bottom), `:on-screen' (the screen it is on: its frame
and scale, or nil), `:occluded', `:has-shadow' (AppKit draws the
window's shadow) and `:sublayers' (how many layers the shell's layer
holds: children's containers and window cards).  For tests."
  (nhmac--query 'window-info (or frame (selected-frame))))

(defun nhmac--startup-log ()
  "Return the shell's startup trace, a list.
Its first events (a window made, its configure queued and applied, the
visibility pushes, the first draw and commit) as strings, each with its
milliseconds since the first, then each window's waits for its first
configure as (wait WINDOW :rounds R :timeouts T).  For tests."
  (nhmac--query 'startup-log))

(defun nhmac--shell-view ()
  "Return the shell's own view, a plist.
`:windows' (each one's id, size in points, scale, visibility, occlusion
and whether it is key), `:queued' (the records waiting for Emacs: window
and event) and `:wake-readable'.  For tests."
  (nhmac--query 'shell-view))

(defun nhmac--image-draws ()
  "Return the last image draws, oldest first, a list of plists.
Each has the container's `:size', `:bpp', `:mask' and first pixel
`:word', the picture's first word when that draw made it (`:made'),
the drawn `:rect' and its first `:clip'.  For tests."
  (nhmac--query 'image-draws))

(defun nhmac--pump-trace ()
  "Return the pump decisions since the last call, oldest first, and forget them.
Each says who asked (a drain's end with its facts, or a call outside a
drain) and whether AppKit was pumped.  For tests."
  (nhmac--query 'pump-trace))

(defun nhmac--test-entries (on)
  "Return the glue entry points timed since the last call, and forget them.
A list of (NAME CALLS NS), the most time first: the calls of each entry
point and the nanoseconds spent inside them.  Timing is on from now when
ON is non-nil, off when it is nil.  For tests."
  (nhmac--query 'test-entries (if on 1 0)))

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
KIND is `down', `up', `drag', `move', `wheel' or `exit' (the pointer
leaving the window there); X and Y are device pixels from the frame's
top left.  BUTTON is 0 (left, the default), 1
(right) or 2 (middle), or for `wheel' the lines to scroll (positive
down).  FLAGS is the `NSEventModifierFlags' (default 0).  For tests."
  (nhmac--call 'test-post-mouse (or frame (selected-frame))
               (pcase kind ('down 0) ('up 1) ('drag 2) ('move 3) ('wheel 4) ('exit 5)
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

(defun nhmac--test-cards (&optional frame)
  "Root FRAME's window cards on their own layers, a plist.
:images, how many card images its layers took since it opened; :cards,
each card's (ID X Y W H): its place in the frame, device pixels.  For
tests."
  (nhmac--query 'test-cards (or frame (selected-frame))))

(defun nhmac--test-signal-helper (n)
  "Interrupt the main wait's helper thread N times, 20 ms apart, with a signal.
A handler that does nothing catches it.  An error before the helper has
started.  For tests."
  (nhmac--call 'test-signal-helper n))

(defun nhmac--test-force-in-use (on &optional frame)
  "Make every surface of FRAME's window read as held by the window server, if ON.
The commit pacer then holds each commit, until its bound.  For tests."
  (nhmac--call 'test-force-in-use (or frame (selected-frame)) (if on 1 0)))

(defun nhmac--test-surface (x y &optional frame)
  "Return the newest surface of FRAME's window, a plist.
`:w', `:h' (device pixels), `:points' (the content size), the layer's
`:scale' and `:gravity', `:contents-is-newest', `:srgb' (`srgb',
`generic' or nil), `:pixel', the premultiplied ARGB at X, Y,
`:slots', the surfaces the window's ring has made, `:held', whether
the commit pacer holds a commit, and `:free', whether the next commit
would find a surface the window server does not hold.  For tests."
  (nhmac--query 'test-surface (or frame (selected-frame)) x y))

(defun nhmac--test-surface-region (x y w h &optional frame)
  "Return the W x H pixels from X, Y of the newest surface of FRAME's window.
A vector of premultiplied ARGB integers, row by row (an answer holds
some 1400 pixels at most: read a row at a time).  For tests."
  (nhmac--query 'test-surface-region (or frame (selected-frame)) x y w h))

(defun nhmac--test-child (frame)
  "Return the layers of child frame FRAME's window, a plist.
`:x', `:y' (its container's position, points from the parent's bottom
left), `:w', `:h', `:host' (`parent', `panel' or nil), `:hidden',
`:opacity', `:index' (among its layer's sublayers),
`:contents-is-newest', `:panel' (the overhang panel's screen frame, or
nil), `:blur' (t while its panel holds a blur), `:input' (the input
region, points, or nil for all), `:place'
(the surface's place in its parent's, points), its chrome (`:radius',
`:masks', `:border-width', `:shadow', `:shadow-index'), `:animations'
(the keys of the container's running animations) and `:leg' (the last
show or hide leg handed to Core Animation: `:show', `:from' and `:to'
as (OPACITY SCALE), `:seconds'; or nil).  For tests."
  (nhmac--query 'test-child frame))

(defun nhmac--test-reduced-motion (v)
  "Make the shell read reduced motion as V: 1 on, 0 off, -1 the system's.
For tests."
  (nhmac--call 'test-reduced-motion v))

(defun nhmac-reduced-motion-p ()
  "Non-nil when the system asks for reduced motion.
Child frames' show and hide transitions then run no leg."
  (nhmac--query 'reduced-motion))

(defun nhmac--test-windows ()
  "Return the window server's numbers for this application's visible windows.
For tests."
  (nhmac--query 'test-windows))

(defun nhmac--test-capture-switch (r g b)
  "Make the next overhang switch capture the screen around its root.
Once right after the panel is ordered in, once after the child's
layer moved into it; each counts the pixels of colour R G B (0 to
255).  `nhmac--test-captures' returns the counts.  For tests."
  (nhmac--call 'test-capture-switch r g b))

(defun nhmac--test-captures ()
  "Return the counts of the captures `nhmac--test-capture-switch' armed.
A list, nil for a capture the window server refused.  For tests."
  (nhmac--query 'test-captures))

(defun nhmac--test-kq-probe (path)
  "Open PATH and ask a fresh kqueue for its readability.
Return (:open ERRNO :kq ERRNO), 0 meaning success; :kq is -1 when
the open failed.  For tests."
  (nhmac--query 'test-kq-probe path))

(defun nhmac--test-kq-layers (n)
  "Count the readiness kqueue loses on a pipe and a pty, N bytes each.
A writer thread writes the bytes while the caller's thread polls the
kqueue and then blocks on it directly or through a second kqueue
watching it.  Return (:pipe-direct M :pipe-nested M :pty-direct M
:pty-nested M), M the blocks that timed out with the descriptor
readable.  For tests."
  (nhmac--query 'test-kq-layers n))


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
BADGE should be a string short enough to display nicely in the short
space intended for badges.  If BADGE is nil, clear the app badge."
  (unless (null badge) (cl-check-type badge string))
  (nhmac--call 'badge (or badge "")))

(declare-function nhwl--drag-start "nhwlfns.c"
                  (frame items-fn offered preferred allow-current return-frame))

(defun nhmac--drag-items (pboard)
  "The (MIME . BYTES) items of PBOARD, `ns-begin-drag''s pasteboard alist."
  (let (items)
    (pcase-dolist (`(,type . ,value) pboard)
      (pcase type
        ('string
         (push (cons "text/plain;charset=utf-8" (encode-coding-string value 'utf-8)) items))
        ('file
         (push (cons "text/uri-list"
                     (encode-coding-string
                      (mapconcat (lambda (f)
                                   (if (string-prefix-p "file://" f) f
                                     (url-encode-url (concat "file://" (expand-file-name f)))))
                                 (if (listp value) value (list value))
                                 "\r\n")
                      'utf-8))
               items))))
    (nreverse items)))

(defun ns-begin-drag (frame pboard action &optional return-frame allow-same-frame follow-tooltip)
  "Begin a drag-and-drop operation on FRAME.
PBOARD is an alist of (TYPE . VALUE): TYPE `string' with a string, or
`file' with a file URL or a list of file names.  ACTION is one of
`XdndActionCopy' or `XdndActionMove'.  Return the action the target
performed, as `x-begin-drag'; RETURN-FRAME and ALLOW-SAME-FRAME are as
there, and FOLLOW-TOOLTIP is ignored."
  (ignore follow-tooltip)
  (let* ((move (eq action 'XdndActionMove))
         (r (nhwl--drag-start (or frame (selected-frame))
                              (lambda () (nhmac--drag-items pboard))
                              (if move 3 1) (if move 2 1)
                              allow-same-frame return-frame)))
    (pcase r (1 'XdndActionCopy) (2 'XdndActionMove) (4 'XdndActionPrivate) (_ r))))

(defun ns-block-system-sleep (why allow-display-sleep)
  "Block system idle sleep.
WHY is a string reason for the block.  If ALLOW-DISPLAY-SLEEP is nil,
block the screen from sleeping too.  Return a token to unblock this
block using `ns-unblock-system-sleep', or nil if the block fails."
  (unless (null why) (cl-check-type why string))
  (nhmac--query 'sleep-block (if allow-display-sleep 1 0) (or why "")))

(defun ns-disown-selection-internal (selection)
  "If we own the selection SELECTION, disown it.
Only the clipboard exists on macOS."
  (nhmac--call 'pasteboard-disown selection))

(declare-function pgtk-display-monitor-attributes-list "nhwlfns.c" (&optional terminal))

(defun ns-display-monitor-attributes-list (&optional terminal)
  "Return a list of physical monitor attributes on the display TERMINAL.
The screens the shell announced, as the shared C lists them (geometry
in device pixels), with `source' \"NS\".  Each `workarea' is the
screen's `visibleFrame' (what the menu bar and the Dock leave), in the
same device pixels."
  (let ((screens (nhmac--query 'screens)))
    (cl-loop
     for m in (pgtk-display-monitor-attributes-list terminal)
     for s = (pop screens)
     collect
     (mapcar (lambda (kv)
               (pcase (car kv)
                 ('source (cons 'source "NS"))
                 ((and 'workarea (guard s))
                  (pcase-let* ((`(,x ,y . ,_) (alist-get 'geometry m))
                               (`(,fx ,fy ,_ ,_ ,vx ,vy ,vw ,vh ,k) s))
                    (list 'workarea
                          (+ x (round (* (- vx fx) k))) (+ y (round (* (- vy fy) k)))
                          (round (* vw k)) (round (* vh k)))))
                 (_ kv)))
             m))))

(defun ns-do-applescript (script)
  "Execute AppleScript SCRIPT and return the result.
If compilation and execution are successful, the resulting script value
is returned as a string, a number or, in the case of other constructs,
nil."
  (cl-check-type script string)
  (let ((r (nhmac--query 'applescript script)))
    (if (and (consp r) (eq (car r) 'error))
        (error "%s" (cadr r))
      r)))

(defun ns-emacs-info-panel ()
  "Shows the `Info' or `About' panel for Emacs."
  (nhmac--call 'about-panel))

(defun ns-font-name (name)
  "Determine font PostScript or family name for font NAME.
NAME should be a string containing either the font name or an XLFD
font descriptor.  If string contains `fontset' and not
`fontset-startup', it is left alone."
  (if (and (string-match-p "fontset" name) (not (string-match-p "fontset-startup" name)))
      name
    (let ((font (find-font (font-spec :name name))))
      (if font (symbol-name (font-get font :family)) name))))

(declare-function pgtk-frame-edges "nhwlfns.c" (&optional frame type))
(declare-function pgtk-frame-geometry "nhwlfns.c" (&optional frame))

(defun ns-frame-edges (&optional frame type)
  "Return edge coordinates of FRAME.
See `frame-edges'."
  (pgtk-frame-edges frame type))

(defun ns-frame-geometry (&optional frame)
  "Return geometric attributes of FRAME.
See `frame-geometry'."
  (pgtk-frame-geometry frame))

(defun ns-frame-list-z-order (&optional terminal)
  "Return list of Emacs's frames, in Z (stacking) order.
The topmost frame comes first.  TERMINAL is ignored: macOS has one
display.  Child frames and frames that are not shown are left out."
  (ignore terminal)
  (let (frames)
    (dolist (f (frame-list))
      (when (and (eq (framep f) 'ns) (not (frame-parameter f 'parent-frame)))
        (push f frames)))
    (setq frames (nreverse frames))
    (and frames
         (mapcar (lambda (i) (nth i frames)) (apply #'nhmac--query 'z-order frames)))))

(defun ns-frame-restack (frame1 frame2 &optional above)
  "Restack FRAME1 below FRAME2.
With ABOVE non-nil, restack FRAME1 above FRAME2."
  (nhmac--call 'restack (window-normalize-frame frame1) (window-normalize-frame frame2)
               (if above 1 0)))

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
Otherwise if Emacs is hidden, it is unhidden.  If ON is equal to
`activate', Emacs is unhidden and becomes the active application."
  (nhmac--call 'hide (cond ((eq on 'activate) 2) (on 1) (t 0))))

(defun ns-hide-others ()
  "Hides all applications other than Emacs."
  (nhmac--call 'hide-others))

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
The position is returned as a cons cell (X . Y) of the coordinates of
the mouse cursor position in pixels relative to a position (0, 0) of the
selected frame's terminal."
  (nhmac--query 'mouse-position))

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
  "Pop up the color panel."
  (ignore frame)
  (nhmac--call 'color-panel))

(defun ns-process-is-accessibility-trusted ()
  "Return non-nil if Emacs is trusted by macOS Accessibility.
This is necessary for Emacs to support Zoom and related accessibility
features."
  (nhmac--query 'ax-trusted))

(defun ns-progress-indicator (progress)
  "Show PROGRESS, a float between 0.0 and 1.0, on the app's dock icon.
If PROGRESS is nil, remove the progress indicator."
  (nhmac--call 'dock-progress (if (floatp progress) progress -1.0)))

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
If URGENCY nil, cancel the outstanding request, if any.  If URGENCY is
the symbol `informational', bouncing lasts a few seconds.  If URGENCY is
the symbol `critical', bouncing lasts until Emacs is focused."
  (nhmac--call 'attention (pcase urgency ('informational 1) ('critical 2) (_ 0))))

(defun ns-reset-menu ()
  "Cause the NS menu to be re-calculated."
  (setq nhmac-menu--last nil)
  (nhmac-menu-build))

(defun ns-selection-exists-p (&optional selection)
  "Whether there is an owner for the given X selection."
  (nhmac--query 'pasteboard-exists (or selection 'PRIMARY)))

(defun ns-selection-owner-p (&optional selection)
  "Whether the current Emacs process owns the given X Selection."
  (nhmac--query 'pasteboard-owned (or selection 'PRIMARY)))

(defun ns-send-items (items)
  "Send a list of items to macOS applications or services.
ITEMS is a list of strings: file names of existing files are sent as
files, any other string as text."
  (cl-check-type items list)
  (dolist (i items) (unless (stringp i) (signal 'error (list "Element not a string" i))))
  (nhmac--call 'send-items (selected-frame) (mapconcat #'identity items "\C-_")))

(defun ns-set-mouse-absolute-pixel-position (x y)
  "Move mouse pointer to absolute pixel position (X, Y).
The coordinates X and Y are interpreted in pixels relative to a position
\(0, 0) of the selected frame's terminal."
  (nhmac--call 'warp-mouse x y))

(defun ns-set-resource (owner name value)
  "Set property NAME of OWNER to VALUE, from the defaults database.
The user default NAME (`NSUserDefaults'); a nil VALUE removes it.
OWNER is ignored."
  (ignore owner)
  (if value
      (nhmac--call 'set-resource name value)
    (nhmac--call 'unset-resource name)))

(defun ns-show-character-palette ()
  "Show the macOS character palette."
  (nhmac--call 'character-palette))

(defun ns-unblock-system-sleep (token)
  "Unblock system idle sleep.
TOKEN is an object returned by `ns-block-system-sleep'.  Return non-nil
if the TOKEN block was unblocked."
  (cl-check-type token natnum)
  (nhmac--query 'sleep-unblock token))

;; Variables the shell reads.  Rust gets the new value when one is set; a
;; `let' binding does not reach it.

(defconst nhmac--watched-variables
  '(ns-alternate-modifier ns-right-alternate-modifier
    ns-command-modifier ns-right-command-modifier
    ns-control-modifier ns-right-control-modifier ns-function-modifier
    ns-use-srgb-colorspace ns-confirm-quit ns-auto-hide-menu-bar
    ns-use-native-fullscreen ns-antialias-text ns-use-thin-smoothing)
  "The variables whose changes `nhmac--variable-changed' passes to the shell.")

(defun nhmac--frame-up-p ()
  "Whether a macOS frame is open: the shell is up (batch, or a daemon before its first frame, has none)."
  (and (fboundp 'nhmac--call)
       (let ((up nil))
         (dolist (f (frame-list) up)
           (when (eq (framep f) 'ns) (setq up t))))))

(defun nhmac--variable-changed (symbol newval operation where)
  "Pass NEWVAL of SYMBOL to the shell when it is set (a variable watcher).
OPERATION and WHERE are as `add-variable-watcher' gives them; only a global
`set' counts.  Nothing is passed while no macOS frame is open:
`nhmac--push-variables' passes every value when the first one opens."
  (when (and (eq operation 'set) (null where) (nhmac--frame-up-p))
    (funcall 'nhmac--call 'variable symbol newval)))

(defvar nhmac--variables-pushed nil
  "Whether the shell has the watched variables' values (`nhmac--push-variables').")

(defun nhmac--push-variables (frame)
  "On the first macOS FRAME, pass the shell every watched variable's global value (set before it was up)."
  (when (and (not nhmac--variables-pushed) (eq (framep frame) 'ns) (fboundp 'nhmac--call))
    (setq nhmac--variables-pushed t)
    (dolist (v nhmac--watched-variables)
      (when (boundp v)
        (funcall 'nhmac--call 'variable v (default-value v))))))

(dolist (variable nhmac--watched-variables)
  (add-variable-watcher variable #'nhmac--variable-changed))
(add-hook 'after-make-frame-functions #'nhmac--push-variables)

(provide 'nhmac-win)

;;; nhmac-win.el ends here
