;;; fake-portal-tests.el --- tests for the scripted fake portal  -*- lexical-binding: t -*-

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

;; Batch tests of fake-portal.el.  The service's handlers are called directly and the signal is
;; replaced by a recorder, so no bus is needed.

;;; Code:

(require 'ert)
(require 'cl-lib)
(load (expand-file-name "fake-portal.el" (file-name-directory (or load-file-name buffer-file-name))) nil t)

(defmacro fake-portal-tests--with (script &rest body)
  "Run BODY with a script of the lines SCRIPT, an empty log and `fake-portal--emit' recording in `emitted'."
  (declare (indent 1))
  `(let* ((dir (make-temp-file "fake-portal" t))
          (fake-portal-script-file (expand-file-name "script" dir))
          (fake-portal-log-file (expand-file-name "log" dir))
          (fake-portal--pending nil)
          (emitted nil))
     (unwind-protect
         (progn
           (with-temp-file fake-portal-script-file
             (dolist (l ,script) (insert l "\n")))
           (cl-letf (((symbol-function 'fake-portal--emit)
                      (lambda (&rest args) (push args emitted))))
             ,@body))
       (delete-directory dir t))))

(defun fake-portal-tests--log ()
  (with-temp-buffer
    (insert-file-contents fake-portal-log-file)
    (mapcar #'read (split-string (buffer-string) "\n" t))))

(ert-deftest fake-portal/script-replies-in-order ()
  (fake-portal-tests--with '("(:response 0 :uris (\"file:///tmp/a\"))" "(:response 1)")
    (let ((p1 (fake-portal--call "OpenFile" ":1.5" "wayland:abc" "Open" '(("handle_token" ("t1")) ("multiple" (t)))))
          (p2 (fake-portal--call "SaveFile" ":1.5" "" "Save" '(("handle_token" ("t2"))))))
      (should (equal p1 "/org/freedesktop/portal/desktop/request/1_5/t1"))
      (should (equal p2 "/org/freedesktop/portal/desktop/request/1_5/t2"))
      (accept-process-output nil 0.2)
      (should (equal (reverse emitted)
                     `((":1.5" ,p1 0 ("file:///tmp/a")) (":1.5" ,p2 1 nil))))
      (should (equal (fake-portal-tests--log)
                     '((:method "OpenFile" :parent_window "wayland:abc" :title "Open"
                        :options (("handle_token" . "t1") ("multiple" . t)))
                       (:method "SaveFile" :parent_window "" :title "Save"
                        :options (("handle_token" . "t2"))))))
      ;; the script is used up: the next call is cancelled
      (fake-portal--call "OpenFile" ":1.5" "" "x" nil)
      (accept-process-output nil 0.2)
      (should (eq (nth 2 (car emitted)) 1)))))

(ert-deftest fake-portal/never-reply-closes-on-close ()
  (fake-portal-tests--with '("(:never t)")
    (let ((p (fake-portal--call "OpenFile" ":1.9" "" "Open" '(("handle_token" ("t"))))))
      (accept-process-output nil 0.2)
      (should-not emitted)
      (should (equal fake-portal--pending (list p)))
      (fake-portal--close p)
      (should-not fake-portal--pending)
      (accept-process-output nil 0.2)
      (should-not emitted)
      (should (equal (car (last (fake-portal-tests--log))) (list :close p))))))

(provide 'fake-portal-tests)

;;; fake-portal-tests.el ends here
