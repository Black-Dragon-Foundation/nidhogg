;;; nhwl-session.el --- session restore for Nidhogg's Wayland shell  -*- lexical-binding: t -*-

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

;; Compositors that offer xdg-session-management remember where a
;; client's windows were, by name.  When the display's first frame is
;; made, C calls `nhwl-session-start', which asks the compositor for the
;; session whose id was saved in `nhwl-session-file' (a new one when
;; there is none).  C then asks `nhwl-session-name' for each top-level
;; frame's name and creates the frame's window under it, so the
;; compositor puts the window back before its first commit.
;;
;; The first frame on a display is named "initial".  Any other frame
;; takes its `session-name' parameter, or gets a generated name that is
;; stored in the parameter: frameset saves it, so `desktop-save-mode'
;; brings frames back to where they were.  A frame deleted before Emacs
;; exits is taken out of the session; frames still open at `kill-emacs'
;; stay in it.
;;
;; The compositor's answer comes as the special event `nhwl-session':
;; the session's id is saved on `created' and `restored', and on
;; `replaced' (another client took the session over) frames are no
;; longer named.  Without the protocol nothing happens.

;;; Code:

(defcustom nhwl-session-restore t
  "Whether Nidhogg asks the compositor to restore its frames' places.
This needs a compositor with xdg-session-management.  The switch is read
when a display's first frame is made, and again for every new frame."
  :type 'boolean
  :group 'frames
  :version "31.1")

(defcustom nhwl-session-file nil
  "File that keeps the compositor's session id between runs.
Nil means `nhwl-session.eld' in `user-emacs-directory'.  The file holds a
property list, (:id ID)."
  :type '(choice (const :tag "In the Emacs directory" nil) file)
  :group 'frames
  :version "31.1")

(declare-function nhwl--session-open "nhwlfns.c" (frame id reason))
(declare-function nhwl--session-remove "nhwlfns.c" (frame))

(defvar nhwl-session--state nil
  "Where the compositor session stands.
Nil before it is asked for, `pending' until the compositor answers (frames
are named meanwhile), then `created', `restored' or `replaced'; `none' when
the compositor has no session management.")

(defvar nhwl-session--exiting nil
  "Non-nil once `kill-emacs' runs: frames deleted then stay in the session.")

(defvar nhwl-session--counter 0
  "How many names `nhwl-session--generate' made.")

(defun nhwl-session--file ()
  "The file that keeps the session id."
  (or nhwl-session-file (locate-user-emacs-file "nhwl-session.eld")))

(defun nhwl-session--read-id ()
  "The saved session id, or nil when there is none or the file is unusable."
  (condition-case nil
      (let ((file (nhwl-session--file)))
        (when (file-readable-p file)
          (with-temp-buffer
            (insert-file-contents file)
            (let* ((read-circle nil)
                   (id (plist-get (read (current-buffer)) :id)))
              (and (stringp id) (not (string-empty-p id)) id)))))
    (error nil)))

(defun nhwl-session--write-id (id)
  "Save session ID in `nhwl-session-file'."
  (unless (equal id (nhwl-session--read-id))
    (condition-case nil
        (let ((file (nhwl-session--file)))
          (make-directory (file-name-directory file) t)
          (with-temp-file file
            (prin1 (list :id id) (current-buffer))
            (insert "\n")))
      (file-error nil))))

(defun nhwl-session-start (frame)
  "Ask the compositor for the session, from FRAME's display.
Called by C when the display's first frame is made, before its window."
  (setq nhwl-session--state nil)
  (when nhwl-session-restore
    (setq nhwl-session--state
          (if (nhwl--session-open frame (nhwl-session--read-id) 'launch)
              'pending
            'none))))

(defun nhwl-session--held-p (name frame)
  "Non-nil if a live frame other than FRAME has session NAME."
  (let (held)
    (dolist (f (frame-list))
      (when (and (not (eq f frame))
                 (equal (frame-parameter f 'session-name) name))
        (setq held t)))
    held))

(defun nhwl-session--generate (frame)
  "A session name no live frame other than FRAME has."
  (let (name)
    (while (or (null name) (nhwl-session--held-p name frame))
      (setq name (format "frame-%d-%04x"
                         (setq nhwl-session--counter (1+ nhwl-session--counter))
                         (random #x10000))))
    name))

(defun nhwl-session-name (frame first)
  "The session name of top-level FRAME, or nil for none.
FIRST is non-nil for the display's first frame.  A name the frame has
already is kept, unless another live frame holds it."
  (when (and nhwl-session-restore
             (memq nhwl-session--state '(pending created restored)))
    (let ((name (frame-parameter frame 'session-name)))
      (cond ((and (stringp name) (not (nhwl-session--held-p name frame))) name)
            ((and first (not (nhwl-session--held-p "initial" frame))) "initial")
            (t (nhwl-session--generate frame))))))

(defun nhwl-session (event)
  "Take the compositor's answer about the session.
EVENT is (nhwl-session (STATE . ID)): STATE is `created', `restored' or
`replaced', ID the session's id or nil.  The id is saved; a replaced
session stops naming frames."
  (interactive "e")
  (let ((state (car-safe (nth 1 event)))
        (id (cdr-safe (nth 1 event))))
    (setq nhwl-session--state state)
    (when (and (memq state '(created restored)) (stringp id))
      (nhwl-session--write-id id))))

(defun nhwl-session--forget (frame)
  "Take FRAME out of the session unless Emacs is exiting."
  (when (and nhwl-session-restore
             (not nhwl-session--exiting)
             (memq nhwl-session--state '(pending created restored))
             (stringp (frame-parameter frame 'session-name)))
    (nhwl--session-remove frame)))

(define-key special-event-map [nhwl-session] #'nhwl-session)
(add-hook 'delete-frame-functions #'nhwl-session--forget)
(add-hook 'kill-emacs-hook (lambda () (setq nhwl-session--exiting t)))

(provide 'nhwl-session)
(provide 'term/nhwl-session)

;;; nhwl-session.el ends here
