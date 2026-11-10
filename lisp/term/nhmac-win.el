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
`:spins-in-redisplay' (run-loop passes outside the pumps), `:pumps',
`:handed-pumps' (a Lisp thread's pumps, handed to the main thread)) and
the commit pacer (`:held', `:held-timeouts', `:pacer-fires',
`:pacer-arms')."
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
and scale, or nil) and `:occluded'.  For tests."
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
nil), `:input' (the input region, points, or nil for all), `:place'
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

Not available yet in Naglfar."
  (ignore selection)
  (nhmac--not-yet))

(declare-function pgtk-display-monitor-attributes-list "nhwlfns.c" (&optional terminal))

(defun ns-display-monitor-attributes-list (&optional terminal)
  "Return a list of physical monitor attributes on the display TERMINAL.
The screens the shell announced, as the shared C lists them (geometry
in device pixels), with `source' \"NS\"."
  (mapcar (lambda (m)
            (mapcar (lambda (kv) (if (eq (car kv) 'source) (cons 'source "NS") kv)) m))
          (pgtk-display-monitor-attributes-list terminal)))

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

Not available yet in Naglfar."
  (ignore selection-name target-type)
  (nhmac--not-yet))

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

Not available yet in Naglfar."
  (ignore selection value)
  (nhmac--not-yet))

(defun ns-perform-service (service send)
  "Perform Nextstep SERVICE on SEND.

Not available yet in Naglfar."
  (ignore service send)
  (nhmac--not-yet))

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

Not available yet in Naglfar."
  (ignore prompt dir mustmatch init dir-only-p)
  (nhmac--not-yet))

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
  "Whether there is an owner for the given X selection.

Not available yet in Naglfar."
  (ignore selection)
  (nhmac--not-yet))

(defun ns-selection-owner-p (&optional selection)
  "Whether the current Emacs process owns the given X Selection.

Not available yet in Naglfar."
  (ignore selection)
  (nhmac--not-yet))

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
