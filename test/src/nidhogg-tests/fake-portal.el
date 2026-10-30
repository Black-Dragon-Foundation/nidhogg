;;; fake-portal.el --- a scripted xdg-desktop-portal for the GUI rows  -*- lexical-binding: t -*-

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

;; A mock of an external service (xdg-desktop-portal), run by gui.sh --portal fake as
;;   nidhogg -Q --fg-daemon=SOCKET -l fake-portal.el -f fake-portal-serve
;; on the run's private session bus.  It owns org.freedesktop.portal.Desktop and serves
;; org.freedesktop.portal.FileChooser (property `version' 4) OpenFile and SaveFile, each answered through an
;; org.freedesktop.portal.Request object at /org/freedesktop/portal/desktop/request/SENDER/TOKEN.
;;
;; Environment:
;;   NHWL_FAKE_PORTAL_SCRIPT  a file with one plist per line; every call takes (and removes) the first line:
;;                              (:response 0 :uris ("file:///tmp/a"))   answer with response 0 and these URIs
;;                              (:response 1)                           answer with response 1 (cancelled)
;;                              (:never t)                              never answer; Request.Close ends it
;;                            An empty or missing file answers (:response 1).
;;   NHWL_FAKE_PORTAL_LOG     a file each call appends to:
;;                              (:method M :parent_window S :title T :options ALIST)
;;                            and each Request.Close appends (:close PATH).
;;   NHWL_FAKE_PORTAL_READY   a file written once the bus name is owned (gui.sh waits for it).

;;; Code:

(require 'dbus)

(defconst fake-portal-service "org.freedesktop.portal.Desktop")
(defconst fake-portal-object "/org/freedesktop/portal/desktop")
(defconst fake-portal-chooser "org.freedesktop.portal.FileChooser")
(defconst fake-portal-request "org.freedesktop.portal.Request")

(defvar fake-portal-script-file nil "The script file; nil means $NHWL_FAKE_PORTAL_SCRIPT.")
(defvar fake-portal-log-file nil "The log file; nil means $NHWL_FAKE_PORTAL_LOG.")
(defvar fake-portal--pending nil "Paths of the requests that never answer, until `Close'.")
(defvar fake-portal--counter 0 "Numbers the tokens of calls that give no handle_token.")

(defun fake-portal--next-reply ()
  "Remove the first line of the script and return it read as a plist; (:response 1) when there is none."
  (let* ((file (or fake-portal-script-file (getenv "NHWL_FAKE_PORTAL_SCRIPT")))
         (lines (and file (file-exists-p file)
                     (with-temp-buffer (insert-file-contents file) (split-string (buffer-string) "\n" t)))))
    (if (null lines)
        '(:response 1)
      (write-region (mapconcat (lambda (l) (concat l "\n")) (cdr lines) "") nil file nil 'silent)
      (car (read-from-string (car lines))))))

(defun fake-portal--log (entry)
  "Append ENTRY, one line, to the log file."
  (let ((file (or fake-portal-log-file (getenv "NHWL_FAKE_PORTAL_LOG")))
        (print-escape-newlines t))
    (when file
      (write-region (concat (prin1-to-string entry) "\n") nil file t 'silent))))

(defun fake-portal--options (dict)
  "The a{sv} DICT as dbus.el delivers it, ((KEY (VALUE)) ...), as an alist (KEY . VALUE)."
  (mapcar (lambda (e) (cons (car e) (car (cadr e)))) dict))

(defun fake-portal--path (sender token)
  "The Request object's path for the unique name SENDER and TOKEN."
  (concat fake-portal-object "/request/"
          (replace-regexp-in-string "\\." "_" (string-remove-prefix ":" sender)) "/" token))

(defun fake-portal--emit (sender path response uris)
  "Send the Request.Response signal for PATH to SENDER."
  (dbus-send-signal :session sender path fake-portal-request "Response"
                    :uint32 response
                    (if uris
                        `(:array (:dict-entry "uris" (:variant (:array ,@uris))))
                      '(:array :signature "{sv}"))))

(defun fake-portal--call (method sender parent title options)
  "Serve a METHOD call from SENDER: log it, take the next scripted reply, return the request path.
The Response goes out from a timer, after the method reply."
  (let* ((opts (fake-portal--options options))
         (token (or (cdr (assoc "handle_token" opts))
                    (format "fake%d" (setq fake-portal--counter (1+ fake-portal--counter)))))
         (path (fake-portal--path sender token))
         (reply (fake-portal--next-reply)))
    (fake-portal--log (list :method method :parent_window parent :title title :options opts))
    (if (plist-get reply :never)
        (push path fake-portal--pending)
      (run-at-time 0.01 nil #'fake-portal--emit sender path
                   (or (plist-get reply :response) 0) (plist-get reply :uris)))
    path))

(defun fake-portal--close (path)
  "Serve Request.Close on PATH: log it and end the request."
  (fake-portal--log (list :close path))
  (setq fake-portal--pending (delete path fake-portal--pending))
  nil)

(defun fake-portal--dbus-method (method)
  "The D-Bus handler of FileChooser METHOD: answers with the request's object path."
  (lambda (parent title options)
    (condition-case err
        (let* ((sender (dbus-event-service-name last-input-event))
               (path (fake-portal--call method sender parent title options)))
          (dbus-register-method :session fake-portal-service path fake-portal-request "Close"
                                (lambda () (fake-portal--close path)))
          (list :object-path path))
      (error (message "fake-portal: %s failed: %S" method err)
             (signal (car err) (cdr err))))))

(defun fake-portal-serve ()
  "Own the portal name and serve FileChooser.  The caller's command loop (a daemon, not --batch) delivers the calls."
  (unless (memq (dbus-register-service :session fake-portal-service) '(:primary-owner :already-owner))
    (error "fake-portal: %s is already owned" fake-portal-service))
  (dbus-register-property :session fake-portal-service fake-portal-object fake-portal-chooser
                          "version" :read 4)
  (dolist (m '("OpenFile" "SaveFile"))
    (dbus-register-method :session fake-portal-service fake-portal-object fake-portal-chooser m
                          (fake-portal--dbus-method m)))
  ;; The session's Nidhogg reads its desktop settings from the portal when its frame opens.  A call no handler serves
  ;; gets no reply, so each read would wait out D-Bus's 25 s: answer as a portal that has none of them.
  (dolist (m '("Read" "ReadOne" "ReadAll"))
    (dbus-register-method :session fake-portal-service fake-portal-object "org.freedesktop.portal.Settings" m
                          (lambda (&rest _) '(:error "org.freedesktop.portal.Error.NotFound" "no such setting"))))
  (let ((ready (getenv "NHWL_FAKE_PORTAL_READY")))
    (when ready (write-region "ready\n" nil ready nil 'silent))))

(provide 'fake-portal)

;;; fake-portal.el ends here
