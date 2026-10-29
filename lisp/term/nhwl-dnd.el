;;; nhwl-dnd.el --- Dragging out of Emacs on Nidhogg's Wayland shell  -*- lexical-binding: t -*-

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

;; `x-begin-drag' with X's arguments and results.  The `XdndSelection'
;; value (`nhwl--xdnd-value', set by `gui-set-selection') is converted
;; for every target when the drag starts, through
;; `selection-converter-alist', as X converts a selection it owns.  The
;; drag itself is `nhwl--drag-start' (nhwlfns.c): the glue runs it and
;; C waits for its end.
;;
;; This file is loaded at dump time and uses no `cl-lib' or `pcase'.

;;; Code:

(defconst nhwl-dnd--copy 1 "The copy action's bit (NHG_DND_COPY).")
(defconst nhwl-dnd--move 2 "The move action's bit (NHG_DND_MOVE).")
(defconst nhwl-dnd--ask 4 "The ask action's bit (NHG_DND_ASK).")

(defvar selection-converter-alist)
(defvar nhwl--xdnd-value)
(declare-function nhwl--drag-start "nhwlfns.c"
                  (frame items-fn offered preferred allow-current return-frame))

(defun nhwl-dnd--actions (action)
  "ACTION of `x-begin-drag' as (OFFERED . PREFERRED), action bits.
Wayland has no link or private action: they are copies.  A move also
offers copy, so a target that only copies still takes the drop (the
result says which it did).  A list asks."
  (cond
   ((eq action 'XdndActionMove)
    (cons (logior nhwl-dnd--move nhwl-dnd--copy) nhwl-dnd--move))
   ((eq action 'XdndActionAsk)
    (cons (logior nhwl-dnd--ask nhwl-dnd--copy nhwl-dnd--move) nhwl-dnd--ask))
   ((consp action)
    (let ((offered nhwl-dnd--ask))
      (unless (proper-list-p action)
        (signal 'wrong-type-argument (list 'listp action)))
      (dolist (item action)
        (unless (consp item) (signal 'wrong-type-argument (list 'consp item)))
        (unless (symbolp (car item))
          (signal 'wrong-type-argument (list 'symbolp (car item))))
        (unless (stringp (cdr item))
          (signal 'wrong-type-argument (list 'stringp (cdr item))))
        (setq offered (logior offered (if (eq (car item) 'XdndActionMove)
                                          nhwl-dnd--move
                                        nhwl-dnd--copy))))
      (cons offered nhwl-dnd--ask)))
   ;; Any other symbol is one of X's own protocols' (such as XDS), which
   ;; Wayland does not have: a copy.
   ((symbolp action) (cons nhwl-dnd--copy nhwl-dnd--copy))
   (t (signal 'error (list "Invalid drag-and-drop action" action)))))

(defun nhwl-dnd--result (action)
  "`x-begin-drag's result for the action bit ACTION the target performed."
  (cond ((eql action nhwl-dnd--copy) 'XdndActionCopy)
        ((eql action nhwl-dnd--move) 'XdndActionMove)
        ((eql action nhwl-dnd--ask) 'XdndActionPrivate)))

(defun nhwl-dnd--text-target-p (target)
  "Whether the symbol TARGET names text."
  (memq target '(UTF8_STRING STRING TEXT COMPOUND_TEXT text/plain
                             text/plain\;charset=utf-8)))

(defun nhwl-dnd--convert (value target)
  "The XdndSelection VALUE converted for TARGET (a symbol), or nil.
Converted as X converts a selection it owns, through
`selection-converter-alist': a unibyte string, or nil when no converter
makes a string (or it signals).  A string VALUE's text property named
TARGET stands for it."
  (let* ((alist (and (boundp 'selection-converter-alist)
                     (symbol-value 'selection-converter-alist)))
         (handler (cdr-safe (assq target alist))))
    ;; (AVAILABLE-P . CONVERTER): X's local conversion skips the test.
    (when (consp handler) (setq handler (cdr handler)))
    (when handler
      (when (stringp value)
        (let ((alt (get-text-property 0 target value)))
          (when alt (setq value alt))))
      (let ((r (condition-case nil
                   (funcall handler 'XdndSelection target value)
                 (error nil))))
        ;; (TYPE . DATA)
        (when (and (consp r) (symbolp (car r)))
          (setq r (cdr r)))
        (when (stringp r)
          (if (multibyte-string-p r)
              (let (last-coding-system-used)
                (encode-coding-string r 'utf-8 t))
            r))))))

(defun nhwl-dnd--items (value targets)
  "The bytes VALUE offers for TARGETS, a list of strings.
A list of (MIME . BYTES) in TARGETS' order: every target that converts,
plus text/plain;charset=utf-8 when a text target converted and TARGETS
lacks it."
  (let ((utf8 "text/plain;charset=utf-8")
        (items nil)
        (text nil))
    (dolist (mime targets)
      (let ((target (intern mime)))
        (unless (assoc mime items)
          (let ((bytes (nhwl-dnd--convert value target)))
            (when bytes
              (push (cons mime bytes) items)
              (when (nhwl-dnd--text-target-p target)
                (setq text t)))))))
    (when (and text (not (assoc utf8 items)))
      (let ((bytes (nhwl-dnd--convert value (intern utf8))))
        (when bytes
          (push (cons utf8 bytes) items))))
    (nreverse items)))

(defun x-begin-drag (targets &optional action frame return-frame
                             allow-current-frame follow-tooltip)
  "Begin dragging contents on FRAME, with targets TARGETS.
TARGETS is a list of strings, which defines the selection targets that
will be available to the drop target.  Block until the mouse buttons
are released, then return the action chosen by the target, or `nil' if
the drop was not accepted by the drop target.  Dragging starts when the
mouse is pressed on FRAME, and the contents of the selection
`XdndSelection' will be sent to the client underneath the mouse
pointer (the drop target) when the mouse button is released.  Each
target is converted when the drag starts, through
`selection-converter-alist'; the ones that convert are offered, and
text is also offered as text/plain;charset=utf-8.

ACTION is a symbol which tells the target what it should do, and can
be one of the following:

 - `XdndActionCopy', which means to copy the contents from the drag
   source (FRAME) to the drop target.

 - `XdndActionMove', which means to first take the contents of
   `XdndSelection', and to delete whatever was saved into that
   selection afterwards.  A target that only copies may still copy.

`XdndActionPrivate' is also a valid return value, and means that the
drop target chose to perform an unspecified or unknown action (on
Wayland, the target asked the user and did not say what it did).

The source is also expected to cooperate with the target to perform
the action chosen by the target.  For example, callers should delete
the buffer text that was dragged if `XdndActionMove' is returned.

`XdndActionLink' and other actions are copies here: Wayland has only
copy, move and ask.

If RETURN-FRAME is non-nil, this function will return the frame if the
mouse pointer moves onto an Emacs frame, after first moving out of
FRAME.  If RETURN-FRAME is the symbol `now', any frame underneath the
mouse pointer will be returned immediately.  The drag is given up
then.

If ACTION is a list and not nil, its elements are assumed to be a cons
of (ITEM . STRING), where ITEM is the name of an action, and STRING is
a string describing ITEM to the user.  The drop target is expected to
prompt the user to choose between any of the actions in the list.

If ACTION is not specified or nil, `XdndActionCopy' is used
instead.

If ALLOW-CURRENT-FRAME is nil, a drop on FRAME itself is refused.
Otherwise it is taken: a drop on one of our own frames is handled by
`pgtk-dnd.el' after this function returns.

FOLLOW-TOOLTIP is ignored.

When the target was dropped on but does not finish within
`nhwl-dnd-finish-timeout' seconds, the drag is given up and nil is
returned, so a move never deletes text the target may not have taken.
C-g gives the drag up and quits.

This function returns nil immediately if no mouse button is held
down.  It should only be called when it is known that mouse buttons
are being held down, such as immediately after a `down-mouse-1' (or
similar) event."
  (ignore follow-tooltip)
  (unless (listp targets)
    (signal 'wrong-type-argument (list 'listp targets)))
  ;; A circular list ends in a cons after its distinct conses.
  (when (consp (nthcdr (safe-length targets) targets))
    (signal 'circular-list (list targets)))
  (let ((tail targets))
    (while (consp tail)
      (unless (stringp (car tail))
        (signal 'wrong-type-argument (list 'stringp (car tail))))
      (setq tail (cdr tail)))
    (when tail
      (signal 'wrong-type-argument (list 'listp targets))))
  (let* ((actions (nhwl-dnd--actions action))
         (r (nhwl--drag-start
             frame
             (lambda () (nhwl-dnd--items nhwl--xdnd-value targets))
             (car actions) (cdr actions) allow-current-frame return-frame)))
    (if (integerp r) (nhwl-dnd--result r) r)))

;; A drop goes in at the current buffer (pgtk-dnd.el inserts where it
;; runs): a drop on a window that is not selected, a child frame's say,
;; runs in that window.
(declare-function pgtk-dnd-handle-drag-n-drop-event "pgtk-dnd" (event))

(defun nhwl-dnd-handle-drag-n-drop-event (event)
  "`pgtk-dnd-handle-drag-n-drop-event' in the window EVENT is over."
  (interactive "e")
  (let ((window (posn-window (event-start event))))
    (if (window-live-p window)
        (with-selected-window window
          (pgtk-dnd-handle-drag-n-drop-event event))
      (pgtk-dnd-handle-drag-n-drop-event event))))

(define-key special-event-map [drag-n-drop] #'nhwl-dnd-handle-drag-n-drop-event)

(provide 'nhwl-dnd)
(provide 'term/nhwl-dnd)

;;; nhwl-dnd.el ends here
