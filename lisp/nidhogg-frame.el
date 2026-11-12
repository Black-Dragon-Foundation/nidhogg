;;; nidhogg-frame.el --- Nidhogg: the first frame born with a child-frame minibuffer  -*- lexical-binding: t -*-

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

;; A frame cannot gain or lose its minibuffer once it is made.  Stock Emacs makes the first frame with a minibuffer
;; of its own (`frame-initialize'), and when the init files ask for another kind, `frame-notice-user-settings'
;; deletes it and makes a new one, after waiting for it to be visible.  When early-init.el asks for a minibuffer in a
;; child frame, `(minibuffer . child-frame)' under the window system's entry of `window-system-default-frame-alist',
;; these advices make the first frame that way at once and leave it alone afterwards: no second frame, no wait.
;; Asked for nothing, they do nothing.
;;
;; A frame without a minibuffer of its own borrows its display's default minibuffer frame while it is made; the
;; first frame's display has none yet, so C makes one (`make-initial-minibuffer-frame').  At startup that frame is
;; made invisible and deleted once the first frame has its box: it is never shown.  A daemon's first frame on a display
;; borrows one the same way; it is deleted once that frame has its box.

;;; Code:

(defun nidhogg-frame--child-minibuffer-asked-p ()
  "Non-nil when the initial window system's frames are to have a child-frame minibuffer, and nothing more specific
\(`initial-frame-alist') says otherwise."
  (and initial-window-system
       (not (assq 'minibuffer initial-frame-alist))
       (eq (cdr (assq 'minibuffer (cdr (assq initial-window-system window-system-default-frame-alist))))
           'child-frame)))

(defvar nidhogg-frame--stand-ins nil
  "While the first frame is made with a child-frame minibuffer: the minibuffer frames C made for it to borrow from,
`t' while none yet, nil otherwise.")

(defun nidhogg-frame--stand-in (orig display)
  "Around `make-initial-minibuffer-frame': while the first frame is made, the frame it borrows a minibuffer from is
made invisible and noted, to be deleted once the first frame has its box."
  (if (not nidhogg-frame--stand-ins)
      (funcall orig display)
    (let* ((minibuffer-frame-alist (cons '(visibility) minibuffer-frame-alist))
           (frame (funcall orig display)))
      (setq nidhogg-frame--stand-ins (cons frame (remq t nidhogg-frame--stand-ins)))
      frame)))

(defun nidhogg-frame--initialize (orig)
  "Around `frame-initialize': the first frame is made with the child-frame minibuffer asked for."
  (if (not (nidhogg-frame--child-minibuffer-asked-p))
      (funcall orig)
    (let ((cell (cons 'minibuffer 'child-frame))
          ;; `make-frame' makes the box with `minibuffer-frame-alist' and the display it was given; the first frame is
          ;; given a window system, not a display, so the box is told the window system too (else it would be made
          ;; on the initial terminal: "Unknown terminal type").
          (minibuffer-frame-alist (cons (cons 'window-system initial-window-system) minibuffer-frame-alist)))
      (push cell initial-frame-alist)
      (let ((stand-ins nil))
        (unwind-protect
            (let ((nidhogg-frame--stand-ins (list t)))
              (funcall orig)
              (setq stand-ins (remq t nidhogg-frame--stand-ins)))
          (setq initial-frame-alist (delq cell initial-frame-alist)))
        (when (frame-live-p frame-initial-frame)
          ;; the first frame is its display's minibuffer frame now (its minibuffer is the box's), so the stand-in goes
          (when (nidhogg-frame--born-with-child-minibuffer-p frame-initial-frame)
            (setq default-minibuffer-frame frame-initial-frame)
            (dolist (f stand-ins)
              (when (frame-live-p f) (delete-frame f t))))
          ;; deleting the terminal frame selected another frame, which may be the box: the first frame is the one in use
          (select-frame frame-initial-frame))))))

(defun nidhogg-frame--born-with-child-minibuffer-p (frame)
  "Non-nil when FRAME's minibuffer window is the root of a child frame of FRAME."
  (and (frame-live-p frame)
       (let ((mini (window-frame (minibuffer-window frame))))
         (and (not (eq mini frame)) (eq (frame-parent mini) frame)))))

(defun nidhogg-frame--notice-user-settings (orig)
  "Around `frame-notice-user-settings': a first frame already made with the child-frame minibuffer asked for is not
deleted and made again; its other parameters are applied as usual."
  (if (not (and (nidhogg-frame--child-minibuffer-asked-p)
                (boundp 'frame-initial-frame)
                (nidhogg-frame--born-with-child-minibuffer-p frame-initial-frame)))
      (funcall orig)
    (let ((default-frame-alist (assq-delete-all 'minibuffer (copy-sequence default-frame-alist)))
          (window-system-default-frame-alist
           (mapcar (lambda (e) (cons (car e) (assq-delete-all 'minibuffer (copy-sequence (cdr e)))))
                   window-system-default-frame-alist)))
      (funcall orig))))

(defvar nidhogg-frame--lent nil
  "While `make-frame' runs outside startup: the minibuffer frames C made for the new frame to borrow from, `t' while
none yet, nil otherwise.")

(defun nidhogg-frame--note-lent (orig display)
  "Around `make-initial-minibuffer-frame': while `make-frame' runs, note the frame made."
  (let ((frame (funcall orig display)))
    (when nidhogg-frame--lent
      (setq nidhogg-frame--lent (cons frame (remq t nidhogg-frame--lent))))
    frame))

(defun nidhogg-frame--make-frame (orig &rest args)
  "Around `make-frame': a frame made with a child-frame minibuffer on a display that had no minibuffer frame yet (a
daemon's first frame there) borrowed one C made for it while it was made; once the frame has its box, that frame
serves no one and is deleted, as the first frame's is at startup."
  (let ((lent nil) frame)
    (let ((nidhogg-frame--lent (list t)))
      (setq frame (apply orig args))
      (setq lent (remq t nidhogg-frame--lent)))
    (when (and lent (nidhogg-frame--born-with-child-minibuffer-p frame))
      (dolist (f lent)
        (when (and (frame-live-p f) (not (eq f frame)) (not (eq (frame-parent f) frame)))
          (delete-frame f t))))
    frame))

(advice-add 'make-initial-minibuffer-frame :around #'nidhogg-frame--stand-in)
(advice-add 'make-initial-minibuffer-frame :around #'nidhogg-frame--note-lent)
(advice-add 'make-frame :around #'nidhogg-frame--make-frame)
(advice-add 'frame-initialize :around #'nidhogg-frame--initialize)
(advice-add 'frame-notice-user-settings :around #'nidhogg-frame--notice-user-settings)

(provide 'nidhogg-frame)

;;; nidhogg-frame.el ends here
