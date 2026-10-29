;;; nhwl-menu.el --- Emacs-drawn popup menus on Nidhogg's Wayland shell  -*- lexical-binding: t -*-

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

;; `x-popup-menu' on the shell calls `nhwl-menu-popup' (nhwlfns.c).  A
;; menu is read into item vectors (`nhwl-menu--items'), drawn into the
;; buffer " *nhwl-menu-DEPTH*" and shown in a child frame of the frame
;; it was opened from.  That frame carries the `nhwl-popup' parameter,
;; so the shell makes it a grabbing xdg popup: the popup exists while
;; the frame is visible.  The frame of a depth is made once and then
;; hidden and shown again, never deleted, and its `nhwl-popup'
;; parameter is always a valid value.
;;
;; The menu runs its own loop on `read-event'.  Keys never reach the
;; command loop.  The compositor dismisses a popup (a click outside)
;; with a `delete-frame' event for its frame, which `special-event-map'
;; would hand to `handle-delete-frame', so the loop reads with a map
;; that lacks it.
;;
;; A submenu is a nested level: the loop of its parent calls
;; `nhwl-menu--run' for it, and that returns what happened (a choice,
;; `close' for one level, `cancel' for all, or an event that belongs to
;; a shallower level).  The pointer selects the row under it, a button
;; release or a touch tap chooses it, the wheel scrolls a menu taller
;; than the output, and a dialog (`x-popup-dialog' on this shell is a
;; menu) is centred on its frame.

;;; Code:

(defgroup nhwl-menu nil
  "Popup menus drawn by Emacs on Nidhogg's Wayland shell."
  :group 'menu
  :prefix "nhwl-menu-")

(defcustom nhwl-menu-submenu-delay 0.2
  "Seconds the pointer rests on a submenu row before its menu opens."
  :type 'number
  :group 'nhwl-menu)

(defface nhwl-menu
  '((t :inherit menu))
  "Face of the menu's text and background."
  :group 'nhwl-menu)

(defface nhwl-menu-selected
  '((t :inherit highlight))
  "Face of the selected row."
  :group 'nhwl-menu)

(defface nhwl-menu-disabled
  '((t :inherit shadow))
  "Face of disabled items, titles and separators."
  :group 'nhwl-menu)

(defface nhwl-menu-key
  '((t :inherit shadow))
  "Face of the key bindings shown beside the items."
  :group 'nhwl-menu)

;;;; The model

;; An item is [LABEL KEYS ENABLED VISIBLE BUTTON SELECTED HELP KIND VALUE]:
;; KEYS is a string or nil, BUTTON nil, `toggle' or `radio', KIND `item',
;; `separator' or `submenu'.  VALUE is the key in the keymap for an item of
;; a keymap menu, (KEY . KEYMAP) for its submenu, and the VALUE of the
;; (STRING . VALUE) item for a pane menu.

(defsubst nhwl-menu--label (item) (aref item 0))
(defsubst nhwl-menu--keys (item) (aref item 1))
(defsubst nhwl-menu--enabled (item) (aref item 2))
(defsubst nhwl-menu--visible (item) (aref item 3))
(defsubst nhwl-menu--button (item) (aref item 4))
(defsubst nhwl-menu--selected (item) (aref item 5))
(defsubst nhwl-menu--kind (item) (aref item 7))
(defsubst nhwl-menu--value (item) (aref item 8))

(defun nhwl-menu--eval (form)
  "Value of FORM, or nil when evaluating it signals an error."
  (condition-case nil (eval form t) (error nil)))

(defun nhwl-menu--separator-p (label)
  "Non-nil if LABEL, a string, names a separator line."
  (string-match-p "\\`--" label))

(defun nhwl-menu--separator (&optional label)
  "A separator item; LABEL (default \"--\") picks its line."
  (vector (or label "--") nil nil t nil nil nil 'separator nil))

(defun nhwl-menu--where-is (command)
  "The key description of COMMAND outside the menu bar, or nil."
  (let ((key (where-is-internal command nil t)))
    (and key (> (length key) 0)
         (not (eq (elt key 0) 'menu-bar))
         (key-description key))))

(defun nhwl-menu--item (key binding)
  "BINDING, the definition of KEY in a menu keymap, as an item, or nil.
Nil when BINDING is not a menu item."
  (let (name def props help)
    (cond
     ((eq (car-safe binding) 'menu-item)
      (setq name (nth 1 binding)
            def (nth 2 binding)
            props (nthcdr 3 binding))
      (when (plist-get props :filter)
        (setq def (condition-case nil
                      (funcall (plist-get props :filter) def)
                    (error nil)))))
     ((stringp (car-safe binding))
      (setq name (car binding)
            def (cdr binding))
      (when (stringp (car-safe def))
        (setq help (car def)
              def (cdr def)))))
    (unless (or (null name) (stringp name))
      (setq name (nhwl-menu--eval name)))
    (when (stringp name)
      (let* ((help (or (plist-get props :help) help))
             (kind (cond ((nhwl-menu--separator-p name) 'separator)
                         ((keymapp def) 'submenu)
                         (t 'item)))
             (button (plist-get props :button)))
        (vector
         name
         (and (not (eq kind 'separator))
              (cond ((stringp (plist-get props :keys))
                     (plist-get props :keys))
                    ((plist-member props :key-sequence)
                     (let ((seq (plist-get props :key-sequence)))
                       (and seq (key-description seq))))
                    ((and def (symbolp def) (commandp def))
                     (nhwl-menu--where-is def))))
         (and def (not (eq kind 'separator))
              (or (not (plist-member props :enable))
                  (nhwl-menu--eval (plist-get props :enable)))
              t)
         (or (not (plist-member props :visible))
             (and (nhwl-menu--eval (plist-get props :visible)) t))
         (and (consp button)
              (if (eq (car button) :radio) 'radio 'toggle))
         (and (consp button) (nhwl-menu--eval (cdr button)) t)
         (and (stringp help) help)
         kind
         (cond ((eq kind 'separator) nil)
               ((eq kind 'submenu) (cons key def))
               (t key)))))))

(defun nhwl-menu--keymap-items (keymap)
  "The items of KEYMAP, in the order they were defined.
A binding nearer the top shadows the same key in a parent keymap, and
each :filter runs once."
  (let ((seen nil) (items nil))
    (map-keymap
     (lambda (key binding)
       (when (and (or (symbolp key) (integerp key))
                  (not (memql key seen)))
         (push key seen)
         (let ((item (nhwl-menu--item key binding)))
           (when item (push item items)))))
     keymap)
    items))

(defun nhwl-menu--keymaps-p (menu)
  "Non-nil if MENU is a keymap or a list of keymaps."
  (or (keymapp menu) (and (consp menu) (keymapp (car menu)))))

(defun nhwl-menu--pane-item (item)
  "ITEM of a pane, a string or (STRING . VALUE), as an item, or nil."
  (cond
   ((stringp item)
    (if (nhwl-menu--separator-p item)
        (nhwl-menu--separator item)
      (vector item nil nil t nil nil nil 'item nil)))
   ((and (consp item) (stringp (car item)))
    (if (nhwl-menu--separator-p (car item))
        (nhwl-menu--separator (car item))
      (vector (car item) nil t t nil nil nil 'item (cdr item))))))

(defun nhwl-menu--items (menu)
  "The items of MENU, a keymap, a list of keymaps or a list of panes.
A list of keymaps and a list of several panes put a separator between
the keymaps or panes, and the panes also head each with its title."
  (cond
   ((keymapp menu) (nhwl-menu--keymap-items menu))
   ((nhwl-menu--keymaps-p menu)
    (let ((items nil))
      (dolist (map menu)
        (when items (setq items (append items (list (nhwl-menu--separator)))))
        (setq items (append items (nhwl-menu--keymap-items map))))
      items))
   (t
    (let ((panes (cdr-safe menu)) (items nil) (first t))
      (dolist (pane panes)
        (when (consp pane)
          (unless first
            (setq items (append items (list (nhwl-menu--separator)))))
          (when (and (cdr panes) (stringp (car pane)))
            (setq items (append items
                                (list (vector (car pane) nil nil t nil nil nil
                                              'item nil)))))
          (dolist (item (cdr pane))
            (let ((i (nhwl-menu--pane-item item)))
              (when i (setq items (append items (list i))))))
          (setq first nil)))
      items))))

(defun nhwl-menu--title (menu)
  "The title line of MENU, a non-empty string, or nil."
  (let ((title (cond
                ((keymapp menu) (keymap-prompt menu))
                ((nhwl-menu--keymaps-p menu)
                 (let ((found nil))
                   (dolist (map menu)
                     (unless found (setq found (keymap-prompt map))))
                   found))
                ((consp menu) (car menu)))))
    (and (stringp title) (> (length title) 0) title)))

(defun nhwl-menu--return (menu path)
  "What `x-popup-menu' returns for the choice PATH in MENU.
PATH is the list of items chosen from the top menu to the last.  A
keymap menu returns the keys of PATH, a pane menu the VALUE of its
item, and a cancelled menu (PATH nil) nil."
  (when path
    (if (nhwl-menu--keymaps-p menu)
        (mapcar (lambda (item)
                  (if (eq (nhwl-menu--kind item) 'submenu)
                      (car (nhwl-menu--value item))
                    (nhwl-menu--value item)))
                path)
      (nhwl-menu--value (car (last path))))))

;;;; Rows and display

;; A row is an item as drawn: the visible items, preceded by the title and a
;; separator.  Row N is line N + 1 of the buffer.

(defvar nhwl-menu--frames nil
  "Alist of (DEPTH . FRAME) of the menu frames, reused between menus.")

(defvar-local nhwl-menu--rows nil
  "The vector of the rows drawn in this menu buffer.")

(defvar-local nhwl-menu--index nil
  "The index of the selected row of this menu buffer, or nil.")

(defvar-local nhwl-menu--overlay nil
  "The overlay that highlights the selected row.")

(defvar-local nhwl-menu--due nil
  "When the selected submenu row opens its menu (`float-time'), or nil.")

(defvar-local nhwl-menu--touch nil
  "The touch point that began on this menu, (ID . ROW), or nil.")

(defvar nhwl-menu--pressed nil
  "Non-nil while the menu waits for the release of the press that opened it.")

(defvar nhwl-menu--help-shown nil
  "Non-nil while the echo area shows an item's help.")

(defun nhwl-menu--make-rows (title items)
  "The rows for TITLE (a string or nil) and the visible ITEMS."
  (let ((rows nil))
    (when title
      (push (vector title nil nil t nil nil nil 'item nil) rows)
      (push (nhwl-menu--separator) rows))
    (dolist (item items)
      (when (nhwl-menu--visible item) (push item rows)))
    (vconcat (nreverse rows))))

(defun nhwl-menu--mark (item)
  "The three-character toggle or radio mark of ITEM."
  (let ((button (nhwl-menu--button item)))
    (cond ((null button) "   ")
          ((eq button 'toggle) (if (nhwl-menu--selected item) "[x]" "[ ]"))
          (t (if (nhwl-menu--selected item) "(•)" "( )")))))

(defun nhwl-menu--insert (rows)
  "Insert ROWS, one line each and no final newline, into the buffer."
  (let ((marks nil) (subs nil) (label-w 0) (key-w 0) (width 0) (first t))
    (mapc (lambda (row)
            (unless (eq (nhwl-menu--kind row) 'separator)
              (setq label-w (max label-w (string-width (nhwl-menu--label row)))
                    key-w (max key-w (string-width (or (nhwl-menu--keys row)
                                                       ""))))
              (when (nhwl-menu--button row) (setq marks t))
              (when (eq (nhwl-menu--kind row) 'submenu) (setq subs t))))
          rows)
    (setq width (+ 2 (if marks 4 0) label-w
                   (if (> key-w 0) (+ 2 key-w) 0) (if subs 2 0)))
    (mapc
     (lambda (row)
       (unless first (insert "\n"))
       (setq first nil)
       (if (eq (nhwl-menu--kind row) 'separator)
           (insert (propertize
                    (make-string width (if (equal (nhwl-menu--label row)
                                                  "--double-line")
                                           ?═ ?─))
                    'face 'nhwl-menu-disabled))
         (let ((label (nhwl-menu--label row))
               (keys (or (nhwl-menu--keys row) "")))
           (insert
            " "
            (if marks (concat (nhwl-menu--mark row) " ") "")
            (propertize label 'face (unless (nhwl-menu--enabled row)
                                      'nhwl-menu-disabled))
            (make-string (- label-w (string-width label)) ?\s)
            (if (> key-w 0)
                (concat "  "
                        (make-string (- key-w (string-width keys)) ?\s)
                        (propertize keys 'face 'nhwl-menu-key))
              "")
            (if subs (if (eq (nhwl-menu--kind row) 'submenu) " ›" "  ") "")
            " "))))
     rows)))

(defun nhwl-menu--buffer (depth rows)
  "The buffer ` *nhwl-menu-DEPTH*' showing ROWS, nothing selected."
  (let ((buffer (get-buffer-create (format " *nhwl-menu-%d*" depth))))
    (with-current-buffer buffer
      (setq buffer-read-only nil)
      (erase-buffer)
      (kill-all-local-variables)
      (buffer-disable-undo)
      (setq-local mode-line-format nil)
      (setq-local header-line-format nil)
      (setq-local tab-line-format nil)
      (setq-local cursor-type nil)
      (setq-local cursor-in-non-selected-windows nil)
      (setq-local truncate-lines t)
      (setq-local show-trailing-whitespace nil)
      (setq-local face-remapping-alist '((default nhwl-menu)))
      (nhwl-menu--insert rows)
      (setq buffer-read-only t)
      (setq nhwl-menu--rows rows)
      (setq nhwl-menu--index nil)
      (setq nhwl-menu--overlay (make-overlay 1 1))
      (overlay-put nhwl-menu--overlay 'face 'nhwl-menu-selected)
      (overlay-put nhwl-menu--overlay 'priority 10))
    buffer))

;;;; Selection

(defun nhwl-menu--selectable-p (row)
  "Non-nil if ROW can be selected."
  (and (nhwl-menu--enabled row)
       (not (eq (nhwl-menu--kind row) 'separator))))

(defun nhwl-menu--select-row (index)
  "Select row INDEX (nil for none) of the current menu buffer.
The row's help, if it has any, goes to the echo area."
  (setq nhwl-menu--index index
        nhwl-menu--due nil)
  (let ((help (and index (aref (aref nhwl-menu--rows index) 6))))
    (cond (help (setq nhwl-menu--help-shown t)
                (let ((message-log-max nil)) (message "%s" help)))
          (nhwl-menu--help-shown (setq nhwl-menu--help-shown nil)
                                 (message nil))))
  (if index
      (save-excursion
        (goto-char (point-min))
        (forward-line index)
        (move-overlay nhwl-menu--overlay (line-beginning-position)
                      (line-end-position)))
    (move-overlay nhwl-menu--overlay 1 1)))

(defun nhwl-menu--next (from step test)
  "The index of the first row after FROM in direction STEP that passes TEST.
STEP is 1 or -1, the search wraps once, and TEST takes a row.  FROM is
nil to start before the first row (STEP 1) or after the last (-1)."
  (let* ((n (length nhwl-menu--rows))
         (i (or from (if (> step 0) -1 n)))
         (tries 0) (found nil))
    (while (and (not found) (< tries n))
      (setq i (mod (+ i step) n)
            tries (1+ tries))
      (when (funcall test (aref nhwl-menu--rows i)) (setq found i)))
    found))

(defun nhwl-menu--move (from step)
  "Select the next selectable row after FROM in direction STEP."
  (let ((i (nhwl-menu--next from step #'nhwl-menu--selectable-p)))
    (when i (nhwl-menu--select-row i))))

(defun nhwl-menu--jump (char)
  "Select the next selectable row whose label starts with CHAR."
  (let* ((c (downcase char))
         (i (nhwl-menu--next
             nhwl-menu--index 1
             (lambda (row)
               (and (nhwl-menu--selectable-p row)
                    (> (length (nhwl-menu--label row)) 0)
                    (eq (downcase (aref (nhwl-menu--label row) 0)) c))))))
    (when i (nhwl-menu--select-row i))))

(defun nhwl-menu--activate ()
  "The outcome for the selected row, or nil.
`(choose . ROW)' for an item, `(open . ROW)' for a submenu."
  (let ((row (and nhwl-menu--index (aref nhwl-menu--rows nhwl-menu--index))))
    (when (and row (nhwl-menu--selectable-p row))
      (cons (if (eq (nhwl-menu--kind row) 'submenu) 'open 'choose) row))))

(defun nhwl-menu--hover (row)
  "Select ROW (nil for none) under the pointer.
A submenu row opens its menu after `nhwl-menu-submenu-delay'."
  (let ((row (and row (nhwl-menu--selectable-p (aref nhwl-menu--rows row))
                  row)))
    (unless (eql row nhwl-menu--index)
      (nhwl-menu--select-row row)
      (when (and row (eq (nhwl-menu--kind (aref nhwl-menu--rows row))
                         'submenu))
        (setq nhwl-menu--due (+ (float-time) nhwl-menu-submenu-delay))))))

(defun nhwl-menu--scroll (window delta)
  "Scroll the menu in WINDOW by DELTA rows, within its rows."
  (with-current-buffer (window-buffer window)
    (let* ((visible (max 1 (window-body-height window)))
           (last (max 0 (- (length nhwl-menu--rows) visible)))
           (start (max 0 (min last (+ (count-lines 1 (window-start window))
                                      delta)))))
      (save-excursion
        (goto-char (point-min))
        (forward-line start)
        (set-window-start window (point) t)
        (set-window-point window (point))))))

;;;; Frames

(defun nhwl-menu--live-frames ()
  "The live menu frames."
  (let ((frames nil))
    (dolist (entry nhwl-menu--frames)
      (when (frame-live-p (cdr entry)) (push (cdr entry) frames)))
    frames))

(defun nhwl-menu--frame (depth parent anchor buffer &optional center)
  "The frame of DEPTH showing BUFFER as a popup of PARENT at ANCHOR.
Made once per depth and parent, then reused.  It is sized to the buffer,
at most the height of the display, and shown, which makes the popup.
With CENTER the frame is centred on the point of ANCHOR."
  (let ((frame (cdr (assq depth nhwl-menu--frames))))
    (unless (and (frame-live-p frame) (eq (frame-parent frame) parent))
      (when (frame-live-p frame) (delete-frame frame t))
      (setq frame
            (let ((after-make-frame-functions nil))
              (make-frame `((parent-frame . ,parent) (nhwl-popup . ,anchor)
                            (visibility . nil) (minibuffer . nil)
                            (undecorated . t) (no-other-frame . t)
                            (skip-taskbar . t) (unsplittable . t)
                            (no-special-glyphs . t) (menu-bar-lines . 0)
                            (tool-bar-lines . 0) (tab-bar-lines . 0)
                            (vertical-scroll-bars . nil)
                            (horizontal-scroll-bars . nil)
                            (left-fringe . 0) (right-fringe . 0)
                            (internal-border-width . 0)
                            (child-frame-border-width . 0)
                            (width . 20) (height . 5)))))
      (setq nhwl-menu--frames
            (cons (cons depth frame) (assq-delete-all depth nhwl-menu--frames))))
    (let ((window (frame-root-window frame)))
      (set-window-buffer window buffer)
      (set-window-point window 1)
      (set-window-start window 1)
      (set-window-fringes window 0 0)
      (set-window-margins window 0 0))
    (fit-frame-to-buffer frame)
    (let ((limit (- (display-pixel-height frame) 20)))
      (when (> (frame-pixel-height frame) limit)
        (set-frame-size frame (frame-pixel-width frame) limit t)))
    (when center
      (setq anchor (append (list (max 0 (- (nth 0 anchor)
                                           (/ (frame-pixel-width frame) 2)))
                                 (max 0 (- (nth 1 anchor)
                                           (/ (frame-pixel-height frame) 2))))
                           (nthcdr 2 anchor))))
    (set-frame-parameter frame 'nhwl-popup anchor)
    (make-frame-visible frame)
    frame))

(defun nhwl-menu--hide (&optional depth)
  "Hide the visible menu frames from DEPTH (default 0) down, deepest first."
  (dolist (entry (sort (copy-sequence nhwl-menu--frames)
                       (lambda (a b) (> (car a) (car b)))))
    (when (and (>= (car entry) (or depth 0))
               (frame-live-p (cdr entry)) (frame-visible-p (cdr entry)))
      (make-frame-invisible (cdr entry) t))))

;;;; Position

(defun nhwl-menu--point-spot ()
  "The spot (FRAME X Y HEIGHT) of point in the selected window."
  (let* ((window (selected-window))
         (edges (window-pixel-edges window))
         (pos (pos-visible-in-window-p (point) window t)))
    (if (consp pos)
        (list (window-frame window)
              (+ (nth 0 edges) (nth 0 pos)) (+ (nth 1 edges) (nth 1 pos))
              (line-pixel-height))
      (list (window-frame window) (nth 0 edges) (nth 1 edges) 0))))

(defun nhwl-menu--window-spot (window x y)
  "The spot (FRAME X Y 0) of pixel X, Y in WINDOW, a window or a frame."
  (cond ((and (window-live-p window) (numberp x) (numberp y))
         (let ((edges (window-pixel-edges window)))
           (list (window-frame window) (+ (nth 0 edges) x) (+ (nth 1 edges) y)
                 0)))
        ((and (framep window) (numberp x) (numberp y))
         (list window x y 0))
        (t (nhwl-menu--point-spot))))

(defun nhwl-menu--spot (position)
  "The spot (FRAME X Y HEIGHT) POSITION of `x-popup-menu' stands for."
  (cond
   ((eq position t)
    (let ((mouse (mouse-pixel-position)))
      (if (numberp (cadr mouse))
          (list (car mouse) (cadr mouse) (cddr mouse) 0)
        (nhwl-menu--point-spot))))
   ((and (consp position) (consp (car position)))
    (nhwl-menu--window-spot (cadr position) (car (car position))
                            (cadr (car position))))
   ((and (consp position) (symbolp (car position)))
    (let* ((posn (event-start position))
           (xy (posn-x-y posn))
           (spot (nhwl-menu--window-spot (posn-window posn) (car xy) (cdr xy))))
      ;; a click on the menu bar opens the menu below the bar
      (if (eq (posn-area posn) 'menu-bar)
          (list (nth 0 spot) (nth 1 spot)
                (* (or (frame-parameter (nth 0 spot) 'menu-bar-lines) 0)
                   (frame-char-height (nth 0 spot)))
                0)
        spot)))
   (t (nhwl-menu--point-spot))))

(defun nhwl-menu--dialog-p (position)
  "Non-nil if POSITION is the middle of a frame, where a dialog is put.
`x-popup-dialog' falls back to `x-popup-menu' at ((X Y) FRAME), the
middle of FRAME."
  (and (consp position) (consp (car position)) (framep (cadr position))
       (numberp (car (car position))) (numberp (cadr (car position)))
       (= (car (car position)) (/ (frame-pixel-width (cadr position)) 2))
       (= (cadr (car position)) (/ (frame-pixel-height (cadr position)) 2))))

(defun nhwl-menu--anchor (position)
  "The parent frame, the anchor and whether to centre, for POSITION.
The result is (FRAME ANCHOR CENTER).  ANCHOR is the value of `nhwl-popup'
for a grabbing popup; a popup hangs from a top-level frame, so the spot
in a child frame is taken up to the top-level frame."
  (let* ((spot (nhwl-menu--spot position))
         (frame (nth 0 spot))
         (x (nth 1 spot))
         (y (nth 2 spot)))
    (while (frame-parent frame)
      (let ((at (frame-position frame)))
        (setq x (+ x (car at))
              y (+ y (cdr at))
              frame (frame-parent frame))))
    (list frame (list x y 0 (nth 3 spot) :grab t)
          (nhwl-menu--dialog-p position))))

;;;; The loop

(defun nhwl-menu--event-map ()
  "A copy of `special-event-map' without `delete-frame'."
  (let ((map (copy-keymap special-event-map)))
    (define-key map [delete-frame] nil)
    map))

(defun nhwl-menu--read (map timeout)
  "Read an event with `special-event-map' bound to MAP, waiting TIMEOUT seconds.
Nil if none came; C-g reads as C-g."
  (let ((special-event-map map))
    (condition-case nil
        (read-event nil nil timeout)
      (quit ?\C-g))))

(defun nhwl-menu--drain (count)
  "Read COUNT more `delete-frame' events for menu frames, waiting 0.1 s each.
The compositor dismisses every popup of a stack, and each one reports."
  (let ((map (nhwl-menu--event-map)))
    (while (> count 0)
      (let ((event (let ((special-event-map map))
                     (read-event nil nil 0.1))))
        (cond ((null event) (setq count 0))
              ((and (eq (car-safe event) 'delete-frame)
                    (memq (car-safe (nth 1 event)) (nhwl-menu--live-frames)))
               (setq count (1- count))))))))

(defun nhwl-menu--dead-frame-event-p (event)
  "Non-nil when EVENT is a `delete-frame' event for a frame already dead."
  (and (eq (car-safe event) 'delete-frame)
       (framep (car-safe (nth 1 event)))
       (not (frame-live-p (car (nth 1 event))))))

(defun nhwl-menu--drain-dead ()
  "Read the `delete-frame' events of dead frames that are already queued.
When a display goes, its menu frames are deleted with it, and each one
reports; nothing is left for `handle-delete-frame'."
  (let ((map (nhwl-menu--event-map)) (more t))
    (while more
      (let ((event (let ((special-event-map map))
                     (read-event nil nil 0))))
        (cond ((null event) (setq more nil))
              ((nhwl-menu--dead-frame-event-p event))
              (t (setq unread-command-events
                       (cons event unread-command-events))
                 (setq more nil)))))))

(defun nhwl-menu--dismissed (event)
  "Cancel the menu for the `delete-frame' EVENT; the symbol `cancel'.
For a menu frame, the other levels' events are read here.  For a frame
already dead (its display went), the events of the other dead frames
are read here too.  For any other frame the event is left for the
command loop."
  (let ((frame (car-safe (nth 1 event))))
    (cond
     ((memq frame (nhwl-menu--live-frames))
      (let ((others 0))
        (dolist (f (nhwl-menu--live-frames))
          (when (and (not (eq f frame)) (frame-visible-p f))
            (setq others (1+ others))))
        (nhwl-menu--drain others)))
     ((nhwl-menu--dead-frame-event-p event) (nhwl-menu--drain-dead))
     (t (setq unread-command-events (cons event unread-command-events)))))
  'cancel)

(defun nhwl-menu--mouse-kind (event)
  "What kind of pointer or touch EVENT is, or nil.
One of `down', `drag', `click', `move', `wheel', `touch-begin' and
`touch-end'."
  (let ((head (car-safe event)))
    (when (and head (symbolp head))
      (let ((name (symbol-name head)))
        (cond ((string-match-p "down-mouse-[0-9]+\\'" name) 'down)
              ((string-match-p "drag-mouse-[0-9]+\\'" name) 'drag)
              ((string-match-p "wheel-\\(?:up\\|down\\)\\'" name) 'wheel)
              ((string-match-p "\\(?:\\`\\|-\\)mouse-[0-9]+\\'" name) 'click)
              ((eq head 'mouse-movement) 'move)
              ((eq head 'touchscreen-begin) 'touch-begin)
              ((eq head 'touchscreen-end) 'touch-end))))))

(defun nhwl-menu--locate (event)
  "Where the pointer or touch EVENT happened over a menu, or nil.
The result is (DEPTH ROW WINDOW); ROW is nil past the last row."
  (let* ((posn (ignore-errors (event-start event)))
         (window (and (posnp posn) (posn-window posn)))
         (entry (and (window-live-p window)
                     (rassq (window-frame window) nhwl-menu--frames)))
         (y (and entry (cdr-safe (posn-x-y posn)))))
    (when (numberp y)
      (with-current-buffer (window-buffer window)
        (let ((row (+ (count-lines 1 (window-start window))
                      (floor y (frame-char-height (window-frame window))))))
          (list (car entry)
                (and (>= y 0) (< row (length nhwl-menu--rows)) row)
                window))))))

(defun nhwl-menu--mouse (event kind depth parent-row)
  "Act on the pointer or touch EVENT of KIND in the menu level DEPTH.
PARENT-ROW is the row of the level above that opened this one, or nil.
Return what `nhwl-menu--handle' does."
  (let* ((at (nhwl-menu--locate event))
         (row (nth 1 at))
         (window (nth 2 at))
         ;; the release of the press that opened the menu, if it did not
         ;; move, only keeps the menu open; any release ends the wait
         (held (and (eq kind 'click) nhwl-menu--pressed)))
    (when (memq kind '(click drag))
      (setq nhwl-menu--pressed nil))
    (cond
     ((null at) nil)
     ;; an event for a shallower level is that level's to handle, except
     ;; over the row this level opened from
     ((< (car at) depth)
      (unless (and (= (car at) (1- depth)) (eql row parent-row))
        (cons 'retry event)))
     ((> (car at) depth) nil)
     ((eq kind 'wheel)
      (nhwl-menu--scroll window (if (string-match-p "up\\'"
                                                    (symbol-name (car event)))
                                    -3 3))
      nil)
     ((eq kind 'move) (nhwl-menu--hover row) nil)
     ((memq kind '(down touch-begin))
      (nhwl-menu--hover row)
      (when (eq kind 'touch-begin)
        (setq nhwl-menu--touch (and row (cons (car (cadr event)) row))))
      ;; pressing a submenu row opens it at once
      (and (eq kind 'down)
           (let ((outcome (nhwl-menu--activate)))
             (and (eq (car-safe outcome) 'open) outcome))))
     ((eq kind 'touch-end)
      ;; a tap: the end of the touch that began on this row (nth 2 of the
      ;; event is non-nil when the touch was cancelled)
      (let ((tap (and row (equal nhwl-menu--touch
                                 (cons (car (cadr event)) row))
                      (not (nth 2 event)))))
        (setq nhwl-menu--touch nil)
        (when tap
          (nhwl-menu--hover row)
          (nhwl-menu--activate))))
     ;; a release (click or drag) chooses the row
     (held nil)
     (t (nhwl-menu--hover row)
        (nhwl-menu--activate)))))

(defun nhwl-menu--handle (event depth parent-row)
  "Act on EVENT in the current menu buffer, level DEPTH.
PARENT-ROW is the row of the level above that opened this one, or nil.
Return nil to go on, `cancel' (everything), `close' (this level),
`(choose . ROW)', `(open . ROW)' (the submenu of a row), or
`(retry . EVENT)' (EVENT is for a level above)."
  (cond
   ((eq (car-safe event) 'delete-frame) (nhwl-menu--dismissed event))
   ((nhwl-menu--mouse-kind event)
    (nhwl-menu--mouse event (nhwl-menu--mouse-kind event) depth parent-row))
   ((memq event '(down ?\C-n))
    (nhwl-menu--move nhwl-menu--index 1) nil)
   ((memq event '(up ?\C-p))
    (nhwl-menu--move nhwl-menu--index -1) nil)
   ((eq event 'home) (nhwl-menu--move nil 1) nil)
   ((eq event 'end) (nhwl-menu--move nil -1) nil)
   ((memq event '(return kp-enter ?\r ?\s)) (nhwl-menu--activate))
   ((eq event 'right)
    (let ((outcome (nhwl-menu--activate)))
      (and (eq (car-safe outcome) 'open) outcome)))
   ((eq event 'left) (and (> depth 0) 'close))
   ((memq event '(escape ?\e)) (if (> depth 0) 'close 'cancel))
   ((eq event ?\C-g) 'cancel)
   ((and (characterp event) (> event 32)) (nhwl-menu--jump event) nil)))

(defun nhwl-menu--open (depth row)
  "Run the submenu of ROW, selected in menu level DEPTH.
Return what `nhwl-menu--run' returns, or nil if the submenu is empty."
  (let* ((frame (cdr (assq depth nhwl-menu--frames)))
         (window (frame-root-window frame))
         (index (buffer-local-value 'nhwl-menu--index (window-buffer window)))
         (rows (nhwl-menu--make-rows
                nil (nhwl-menu--keymap-items (cdr (nhwl-menu--value row)))))
         ;; the popup sits at the right of the row, its top at the row's
         (anchor (list (frame-pixel-width frame)
                       (* (- index (count-lines 1 (window-start window)))
                          (frame-char-height frame))
                       0 0 :grab t :gravity 'bottom-right
                       :adjust (list 'flip-x 'slide-y))))
    (when (> (length rows) 0)
      (unwind-protect
          (nhwl-menu--run (1+ depth) frame anchor rows index)
        (nhwl-menu--hide (1+ depth))))))

(defun nhwl-menu--run (depth parent anchor rows parent-row &optional center)
  "Show ROWS as menu level DEPTH, a popup of PARENT at ANCHOR.
PARENT-ROW is the row of the level above that opened this one, or nil.
Return `cancel', `close', `(choose ROW...)' (the rows chosen from this
level down), or `(retry . EVENT)'."
  (let ((buffer (nhwl-menu--buffer depth rows))
        (map (nhwl-menu--event-map))
        (pending nil)
        (result nil))
    (nhwl-menu--frame depth parent anchor buffer center)
    (while (not result)
      (let* ((due (buffer-local-value 'nhwl-menu--due buffer))
             (event (or (pop pending)
                        (nhwl-menu--read
                         map (if due
                                 (max 0.01 (min 0.25 (- due (float-time))))
                               0.25))))
             (outcome
              (cond
               ;; the menu's frame is gone (its parent was deleted); when its
               ;; display went, the dead frames' events are read here
               ((not (frame-live-p (cdr (assq depth nhwl-menu--frames))))
                (nhwl-menu--drain-dead)
                'cancel)
               (event (with-current-buffer buffer
                        (nhwl-menu--handle event depth parent-row)))
               ((and due (>= (float-time) due))
                (with-current-buffer buffer
                  (setq nhwl-menu--due nil)
                  (nhwl-menu--activate))))))
        (cond
         ((eq (car-safe outcome) 'open)
          (let ((sub (nhwl-menu--open depth (cdr outcome))))
            (cond ((eq (car-safe sub) 'choose)
                   (setq result (cons 'choose (cons (cdr outcome) (cdr sub)))))
                  ((eq sub 'cancel) (setq result 'cancel))
                  ((eq (car-safe sub) 'retry) (push (cdr sub) pending)))))
         ((eq (car-safe outcome) 'choose)
          (setq result (list 'choose (cdr outcome))))
         (outcome (setq result outcome)))))
    result))

(defun nhwl-menu--down-p (position)
  "Non-nil if the menu for POSITION is opened by a mouse button held down.
POSITION is the event when it is one; otherwise the event that ran the
command (`popup-menu' turns the event into coordinates)."
  (let ((event (if (and (consp position) (symbolp (car position)))
                   position
                 last-nonmenu-event)))
    (and (consp event) (symbolp (car event))
         (string-match-p "down-mouse-[0-9]+\\'" (symbol-name (car event)))
         t)))

;;;###autoload
(defun nhwl-menu-popup (position menu)
  "Show MENU at POSITION and return the choice, as `x-popup-menu' does.
POSITION is t for the mouse, a mouse event, ((X Y) WINDOW), or anything
else for point.  MENU is a keymap, a list of keymaps or a list of panes
\(TITLE (PANE-TITLE (ITEM . VALUE)...)...).  A keymap menu returns the
list of keys from the top menu to the item, a pane menu the item's VALUE,
and a cancelled menu nil."
  (let ((rows (nhwl-menu--make-rows (nhwl-menu--title menu)
                                    (nhwl-menu--items menu)))
        (frame (selected-frame))
        (nhwl-menu--pressed (nhwl-menu--down-p position))
        (result nil))
    (when (> (length rows) 0)
      (let ((anchor (nhwl-menu--anchor position)))
        (unwind-protect
            (setq result (track-mouse
                           (nhwl-menu--run 0 (nth 0 anchor) (nth 1 anchor)
                                           rows nil (nth 2 anchor))))
          (nhwl-menu--hide)
          (when nhwl-menu--help-shown
            (setq nhwl-menu--help-shown nil)
            (message nil))
          (when (and (frame-live-p frame) (not (eq frame (selected-frame))))
            (select-frame frame 'norecord)))))
    (nhwl-menu--return menu (and (eq (car-safe result) 'choose)
                                 (cdr result)))))

(provide 'nhwl-menu)

;;; nhwl-menu.el ends here
