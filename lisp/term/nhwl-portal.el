;;; nhwl-portal.el --- File dialogs and activation tokens on Nidhogg's Wayland shell  -*- lexical-binding: t -*-

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

;; `x-file-dialog' asks the desktop's file chooser portal
;; (org.freedesktop.portal.FileChooser) through `dbus.el', parented to
;; the selected frame by the handle `nhwl-frame-export-handle' made
;; (xdg-foreign).  The call returns a Request object; the answer is its
;; `Response' signal.  While waiting, the dialog reads events the way
;; `dbus-call-method' does and gives back the ones that are not D-Bus
;; events.  A cancelled dialog quits, and C-g sends `Close' to the
;; request and quits.  Without the portal, or after a D-Bus error, the
;; minibuffer asks instead.
;;
;; `browse-url-xdg-open' on a shell frame gets an xdg-activation token
;; in its environment, so the browser may raise itself.
;;
;; The handle and the token come from the compositor connection each
;; time and are never cached: a reconnect clears them.
;;
;; This file is loaded at dump time and uses no `cl-lib' or `pcase'.
;; `dbus' and `url-util' are required at the first call.

;;; Code:

(declare-function dbus-call-method "dbus" (bus service path interface method &rest args))
(declare-function dbus-register-signal "dbus" (bus service path interface signal handler &rest args))
(declare-function dbus-unregister-object "dbus" (object))
(declare-function dbus-check-event "dbus" (event))
(declare-function dbus-get-property "dbus" (bus service path interface property))
(declare-function dbus-get-unique-name "dbusbind.c" (bus))

(defconst nhwl-portal--service"org.freedesktop.portal.Desktop")
(defconst nhwl-portal--object "/org/freedesktop/portal/desktop")
(defconst nhwl-portal--chooser "org.freedesktop.portal.FileChooser")
(defconst nhwl-portal--request "org.freedesktop.portal.Request")

(defvar nhwl-portal--counter 0
  "Numbers the `handle_token' of each request.")

(defvar nhwl-portal--available nil
  "A cons (BUS-NAME . AVAILABLE).
AVAILABLE says whether the session bus BUS-NAME has a file chooser.")

(defvar nhwl-portal--open nil
  "The requests waiting for an answer: a list of (PATH . CELL).")

(defun nhwl-portal--display-lost (_display state)
  "End every open request when the display is gone (STATE is `lost').
Each gets `Close', and its wait ends as a cancelled dialog, so
`x-file-dialog' quits instead of hanging."
  (when (eq state 'lost)
    (dolist (open nhwl-portal--open)
      (unless (car (cdr open))
        (setcar (cdr open) (cons 2 nil))
        (ignore-errors
          (dbus-call-method :session nhwl-portal--service (car open) nhwl-portal--request "Close"
                            :timeout 500))))))

(defun nhwl-portal--file-uri-to-name (uri)
  "Return the local file name of the file: URI, or nil for any other URI.
Percent escapes are decoded as UTF-8.  A host other than empty or
\"localhost\" is not local."
  (when (and (stringp uri)
             (string-match "\\`file://\\(localhost\\)?\\(/.*\\)\\'" uri))
    (require 'url-util)
    (decode-coding-string (url-unhex-string (match-string 2 uri)) 'utf-8)))

(defun nhwl-portal--bytes (string)
  "STRING as the D-Bus byte array `ay' the portal expects.
It is UTF-8 and NUL-terminated."
  (cons :array
        (apply #'append
               (mapcar (lambda (b) (list :byte b))
                       (append (encode-coding-string string 'utf-8) '(0) nil)))))

(defun nhwl-portal--options (method dir default only-dir token)
  "The options of METHOD as an alist (KEY . VARIANT-ARGS).
METHOD is \"OpenFile\" or \"SaveFile\".
DIR is the folder shown, DEFAULT the file name offered (saving only),
ONLY-DIR non-nil to choose a directory (opening only), TOKEN the
request's handle token."
  (append
   (list (list "handle_token" token)
         (list "modal" :boolean t))
   (and dir (list (list "current_folder" (nhwl-portal--bytes (expand-file-name dir)))))
   (if (equal method "OpenFile")
       (list (list "directory" :boolean (and only-dir t)))
     (and default
          (list (list "current_name" (file-name-nondirectory default)))))))

(defun nhwl-portal--dict (options)
  "OPTIONS, an alist as `nhwl-portal--options' makes it, as the `a{sv}' argument."
  (cons :array (mapcar (lambda (o) `(:dict-entry ,(car o) (:variant ,@(cdr o)))) options)))

(defun nhwl-portal--request-path (token)
  "The object path of the request that TOKEN names for this connection."
  (concat nhwl-portal--object "/request/"
          (replace-regexp-in-string "\\." "_" (string-remove-prefix ":" (dbus-get-unique-name :session)))
          "/" token))

(defun nhwl-portal--available-p ()
  "Non-nil when the session bus has a file chooser portal.
It is asked once per bus connection."
  (require 'dbus)
  (let ((bus (condition-case nil (dbus-get-unique-name :session) (error nil))))
    (and bus
         (cdr (if (equal (car nhwl-portal--available) bus)
                  nhwl-portal--available
                (setq nhwl-portal--available
                      (cons bus (condition-case nil
                                    (and (dbus-get-property :session nhwl-portal--service nhwl-portal--object
                                                            nhwl-portal--chooser "version")
                                         t)
                                  (dbus-error nil)))))))))

(defun nhwl-portal-file-chooser-p ()
  "Non-nil when `x-file-dialog' would use the desktop's file chooser portal.
Nil means it asks in the minibuffer.  This only reads the portal's
`version' property (cached per session bus) and never prompts."
  (and (ignore-errors (nhwl-portal--available-p)) t))

(defun nhwl-portal--wait (cell)
  "Read events until the car of CELL is set; return it.
D-Bus events are handled, the others are put back on `unread-command-events',
as `dbus-call-method' does."
  (while (null (car cell))
    (let ((event (let ((inhibit-redisplay t) unread-command-events)
                   (read-event nil nil 0.05))))
      (when (and event (not (ignore-errors (dbus-check-event event))))
        (setq unread-command-events (nconc unread-command-events (list event))))))
  (car cell))

(defun nhwl-portal--request (method prompt dir default only-dir)
  "Call the portal's METHOD and wait for the answer; return (RESPONSE . RESULTS).
Signals `dbus-error' when the call fails and `quit' on C-g, after
sending `Close' to the request."
  (let* ((token (format "nhwl%d" (setq nhwl-portal--counter (1+ nhwl-portal--counter))))
         (path (nhwl-portal--request-path token))
         (cell (list nil))
         (signal-id nil)
         (sent nil))
    (unwind-protect
        (progn
          (setq signal-id (dbus-register-signal
                           :session nhwl-portal--service path nhwl-portal--request "Response"
                           (lambda (response results) (setcar cell (cons response results)))))
          (dbus-call-method :session nhwl-portal--service nhwl-portal--object nhwl-portal--chooser method
                            (or (ignore-errors (nhwl-frame-export-handle)) "")
                            (or prompt "")
                            (nhwl-portal--dict (nhwl-portal--options method dir default only-dir token)))
          (setq sent t)
          (push (cons path cell) nhwl-portal--open)
          (nhwl-portal--wait cell))
      (setq nhwl-portal--open (assoc-delete-all path nhwl-portal--open))
      (when signal-id (dbus-unregister-object signal-id))
      (when (and sent (null (car cell)))
        (ignore-errors
          (dbus-call-method :session nhwl-portal--service path nhwl-portal--request "Close" :timeout 500))))))

(defun nhwl-portal--answer (reply only-dir)
  "The file name that REPLY, as (RESPONSE . RESULTS), chooses.
Quit when it chooses none."
  (let* ((uris (car (cadr (assoc "uris" (cdr reply)))))
         (name (and (eql (car reply) 0)
                    (nhwl-portal--file-uri-to-name (if (listp uris) (car uris) uris)))))
    (cond ((not (eql (car reply) 0)) (signal 'quit nil))
          ((null name) (user-error "The file dialog chose no local file"))
          (only-dir (file-name-as-directory name))
          (t name))))

(defun x-file-dialog (prompt dir &optional default-filename mustmatch only-dir-p)
  "Ask for a file name with the desktop's file chooser.
PROMPT is the dialog's title, DIR the folder it starts in, and
DEFAULT-FILENAME the file name offered.  With MUSTMATCH or ONLY-DIR-P the
dialog opens an existing file (a directory with ONLY-DIR-P); otherwise it
saves.  Cancelling quits.  Without a file chooser portal the minibuffer
asks instead."
  (let ((reply (and (nhwl-portal--available-p)
                    (condition-case nil
                        (nhwl-portal--request (if (or mustmatch only-dir-p) "OpenFile" "SaveFile")
                                              prompt dir default-filename only-dir-p)
                      (dbus-error nil)))))
    (if reply
        (nhwl-portal--answer reply only-dir-p)
      (let ((use-file-dialog nil))
        (if only-dir-p
            (read-directory-name prompt dir default-filename mustmatch)
          (read-file-name prompt dir default-filename mustmatch))))))

(defun nhwl-portal--xdg-open (function &rest args)
  "Call FUNCTION (`browse-url-xdg-open') with ARGS.
On a shell frame it gets an activation token in its environment."
  (let ((token (and (eq (framep (selected-frame)) 'pgtk)
                    (ignore-errors (nhwl-activation-token)))))
    (if token
        (let ((process-environment (cons (concat "XDG_ACTIVATION_TOKEN=" token) process-environment)))
          (apply function args))
      (apply function args))))

(defvar nhwl-display-connection-functions)
(add-hook 'nhwl-display-connection-functions #'nhwl-portal--display-lost)

(with-eval-after-load 'browse-url
  (advice-add 'browse-url-xdg-open :around #'nhwl-portal--xdg-open))

(provide 'nhwl-portal)

;;; nhwl-portal.el ends here
