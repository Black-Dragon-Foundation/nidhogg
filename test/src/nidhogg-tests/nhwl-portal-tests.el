;;; nhwl-portal-tests.el --- tests for the Wayland shell's file dialog portal  -*- lexical-binding: t -*-

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

;; Batch tests of lisp/term/nhwl-portal.el: the pure parts (URI decoding,
;; the request's options) and the fallback.  The bus is not touched; the
;; portal itself is exercised by the GUI rows (nhwl-gui-w2-tests.el) with
;; the fake portal.

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'dbus)
(require 'nhwl-portal)

(ert-deftest nhwl-portal/file-uri-to-name ()
  (should (equal (nhwl-portal--file-uri-to-name "file:///tmp/a.txt") "/tmp/a.txt"))
  (should (equal (nhwl-portal--file-uri-to-name "file://localhost/tmp/a") "/tmp/a"))
  (should (equal (nhwl-portal--file-uri-to-name "file:///tmp/a%20b%23c") "/tmp/a b#c"))
  (should (equal (nhwl-portal--file-uri-to-name "file:///tmp/caf%C3%A9") "/tmp/café"))
  (should-not (nhwl-portal--file-uri-to-name "https://example.org/a"))
  (should-not (nhwl-portal--file-uri-to-name "file://otherhost/tmp/a"))
  (should-not (nhwl-portal--file-uri-to-name nil)))

(defun nhwl-portal-test--option (options key)
  "The variant arguments of KEY in OPTIONS, or `none'."
  (let ((o (assoc key options))) (if o (cdr o) 'none)))

(ert-deftest nhwl-portal/save-options ()
  (let ((o (nhwl-portal--options "SaveFile" "/home/u/" "/home/u/notes.txt" nil "nhwl7")))
    (should (equal (nhwl-portal-test--option o "handle_token") '("nhwl7")))
    (should (equal (nhwl-portal-test--option o "modal") '(:boolean t)))
    (should (equal (nhwl-portal-test--option o "current_name") '("notes.txt")))
    (should (eq (nhwl-portal-test--option o "directory") 'none))
    ;; the folder is NUL-terminated bytes
    (let ((bytes (car (nhwl-portal-test--option o "current_folder"))))
      (should (eq (car bytes) :array))
      (should (equal (last (cdr bytes) 2) '(:byte 0)))
      (should (equal (cl-loop for (_ b) on (cdr bytes) by #'cddr collect b)
                     (append (encode-coding-string "/home/u/" 'utf-8) '(0) nil))))))

(ert-deftest nhwl-portal/open-directory-options ()
  (let ((o (nhwl-portal--options "OpenFile" "/srv" nil t "nhwl8")))
    (should (equal (nhwl-portal-test--option o "directory") '(:boolean t)))
    (should (eq (nhwl-portal-test--option o "current_name") 'none)))
  (let ((o (nhwl-portal--options "OpenFile" "/srv" "/srv/x" nil "nhwl9")))
    (should (equal (nhwl-portal-test--option o "directory") '(:boolean nil)))
    (should (eq (nhwl-portal-test--option o "current_name") 'none)))
  ;; as the a{sv} argument
  (let ((d (nhwl-portal--dict (nhwl-portal--options "OpenFile" nil nil nil "t"))))
    (should (eq (car d) :array))
    (should (member '(:dict-entry "modal" (:variant :boolean t)) (cdr d)))))

(ert-deftest nhwl-portal/no-portal-falls-back ()
  (let ((asked nil))
    (cl-letf (((symbol-function 'nhwl-portal--available-p) (lambda () t))
              ((symbol-function 'nhwl-portal--request)
               (lambda (&rest _) (signal 'dbus-error '("no reply"))))
              ((symbol-function 'read-file-name)
               (lambda (&rest args) (setq asked (list 'file use-file-dialog args)) "/tmp/chosen"))
              ((symbol-function 'read-directory-name)
               (lambda (&rest args) (setq asked (list 'dir use-file-dialog args)) "/tmp/")))
      (let ((use-file-dialog t))
        (should (equal (x-file-dialog "Open: " "/tmp/" nil t) "/tmp/chosen"))
        (should (equal asked '(file nil ("Open: " "/tmp/" nil t))))
        (should (equal (x-file-dialog "Dir: " "/tmp/" nil t t) "/tmp/"))
        (should (eq (car asked) 'dir))))))

(ert-deftest nhwl-portal/no-interface-falls-back ()
  (cl-letf (((symbol-function 'nhwl-portal--available-p) (lambda () nil))
            ((symbol-function 'read-file-name) (lambda (&rest _) "/tmp/min")))
    (should (equal (x-file-dialog "Save: " "/tmp/") "/tmp/min"))))

(ert-deftest nhwl-portal/cancel-quits-and-uri-chosen ()
  (cl-letf (((symbol-function 'nhwl-portal--available-p) (lambda () t)))
    (cl-letf (((symbol-function 'nhwl-portal--request)
               (lambda (&rest _) (cons 1 nil))))
      (should-error (x-file-dialog "Save: " "/tmp/") :type 'quit))
    (cl-letf (((symbol-function 'nhwl-portal--request)
               (lambda (&rest _) (cons 0 '(("uris" (("file:///tmp/a%20b"))))))))
      (should (equal (x-file-dialog "Save: " "/tmp/") "/tmp/a b")))))

(ert-deftest nhwl-portal/display-lost-closes-open-requests ()
  (let* ((open (list nil))
         (done (list 7))
         (closed nil)
         (nhwl-portal--open (list (cons "/req/a" open) (cons "/req/b" done))))
    (cl-letf (((symbol-function 'dbus-call-method)
               (lambda (_bus _service path _iface method &rest _) (push (list path method) closed))))
      ;; other states change nothing
      (nhwl-portal--display-lost "wayland-0" 'reconnecting)
      (should-not closed)
      (should-not (car open))
      (nhwl-portal--display-lost "wayland-0" 'lost)
      ;; the waiting request is closed and ends as a cancelled dialog (response 2, which quits)
      (should (equal closed '(("/req/a" "Close"))))
      (should (equal (car open) '(2)))
      (should (equal (car done) 7))
      (should (eq (condition-case nil (nhwl-portal--answer (car open) nil) (quit 'quit)) 'quit)))))

(ert-deftest nhwl-portal/lost-handler-is-on-the-hook ()
  (should (memq 'nhwl-portal--display-lost nhwl-display-connection-functions)))

(provide 'nhwl-portal-tests)

;;; nhwl-portal-tests.el ends here
