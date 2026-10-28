;;; nhwl-session-tests.el --- tests for the Wayland shell's session restore  -*- lexical-binding: t -*-

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

;; Batch tests of lisp/term/nhwl-session.el: the id file, the names and the
;; switch.  Frames are not made here: the names are asked for frame
;; objects that are only parameter holders, so `frame-parameter' and
;; `frame-list' are replaced by a table of fake frames.  The compositor's
;; side is the GUI rows' (nhwl-gui-w2-tests.el).

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'nhwl-session)

(defmacro nhwl-session-test--with (frames &rest body)
  "Run BODY with session state fresh, a temporary id file and FRAMES live.
FRAMES maps a symbol standing for a frame to its `session-name'.  The
variable `file' is the id file."
  (declare (indent 1))
  `(let* ((file (make-temp-file "nhwl-session" nil ".eld"))
          (nhwl-session-file file)
          (nhwl-session-restore t)
          (nhwl-session--state 'pending)
          (nhwl-session--counter 0)
          (table ,frames))
     (delete-file file)
     (unwind-protect
         (cl-letf (((symbol-function 'frame-list)
                    (lambda () (mapcar #'car table)))
                   ((symbol-function 'frame-parameter)
                    (lambda (f param)
                      (and (eq param 'session-name) (cdr (assq f table))))))
           ,@body)
       (ignore-errors (delete-file file)))))

(ert-deftest nhwl-session/id-file-round-trip ()
  (nhwl-session-test--with nil
    (should-not (nhwl-session--read-id))
    (nhwl-session--write-id "abc-123")
    (should (equal (nhwl-session--read-id) "abc-123"))
    (should (equal (with-temp-buffer (insert-file-contents file) (read (current-buffer)))
                   '(:id "abc-123")))
    ;; the answer's event writes it too, and an unusable file reads as none
    (nhwl-session '(nhwl-session (created . "new-id")))
    (should (equal (nhwl-session--read-id) "new-id"))
    (with-temp-file file (insert "(:id 5"))
    (should-not (nhwl-session--read-id))
    (with-temp-file file (insert "(:id \"\")"))
    (should-not (nhwl-session--read-id))))

(ert-deftest nhwl-session/names-are-stable ()
  (nhwl-session-test--with '((a . nil) (b . "kept"))
    ;; the display's first frame is "initial"; a name a frame has stays
    (should (equal (nhwl-session-name 'a t) "initial"))
    (should (equal (nhwl-session-name 'a t) "initial"))
    (should (equal (nhwl-session-name 'b nil) "kept"))
    (should (equal (nhwl-session-name 'b t) "kept"))
    ;; "initial" held by another live frame: a generated name of the
    ;; documented shape
    (push '(c . "initial") table)
    (should (string-match-p "\\`frame-[0-9]+-[0-9a-f]\\{4\\}\\'"
                            (nhwl-session-name 'a t)))
    (setq nhwl-session--counter 0)
    (let ((n1 (nhwl-session-name 'x nil))
          (n2 (nhwl-session-name 'y nil)))
      (should (string-match-p "\\`frame-1-" n1))
      (should (string-match-p "\\`frame-2-" n2))
      (should-not (equal n1 n2)))
    ;; a saved name another live frame holds is not used twice
    (push '(d . "kept") table)
    (should-not (equal (nhwl-session-name 'b nil) "kept"))))

(ert-deftest nhwl-session/off-switch ()
  (nhwl-session-test--with '((a . nil))
    (let ((opened 0))
      (cl-letf (((symbol-function 'nhwl--session-open)
                 (lambda (&rest _) (setq opened (1+ opened)) t)))
        ;; on: the session is asked for and frames are named
        (nhwl-session-start 'a)
        (should (= opened 1))
        (should (eq nhwl-session--state 'pending))
        (should (equal (nhwl-session-name 'a t) "initial"))
        ;; off: nothing is asked for and no frame is named
        (let ((nhwl-session-restore nil))
          (nhwl-session-start 'a)
          (should (= opened 1))
          (should-not nhwl-session--state)
          (should-not (nhwl-session-name 'a t)))))
    ;; no session management: asked, refused, nobody named
    (cl-letf (((symbol-function 'nhwl--session-open) (lambda (&rest _) nil)))
      (nhwl-session-start 'a)
      (should (eq nhwl-session--state 'none))
      (should-not (nhwl-session-name 'a t)))
    ;; replaced: names stop
    (nhwl-session '(nhwl-session (replaced)))
    (should-not (nhwl-session-name 'a t))))

(ert-deftest nhwl-session/opens-with-the-saved-id ()
  (nhwl-session-test--with nil
    (let (args)
      (cl-letf (((symbol-function 'nhwl--session-open)
                 (lambda (&rest a) (setq args a) t)))
        (nhwl-session-start 'f)
        (should (equal args '(f nil launch)))
        (nhwl-session--write-id "saved")
        (nhwl-session-start 'f)
        (should (equal args '(f "saved" launch)))))))

(ert-deftest nhwl-session/deleted-frames-leave-unless-exiting ()
  (nhwl-session-test--with '((a . "initial") (b . nil))
    (let (removed)
      (cl-letf (((symbol-function 'nhwl--session-remove)
                 (lambda (f) (push f removed))))
        (nhwl-session--forget 'a)
        (nhwl-session--forget 'b)
        (should (equal removed '(a)))
        (let ((nhwl-session--exiting t))
          (nhwl-session--forget 'a))
        (should (equal removed '(a)))))))

(provide 'nhwl-session-tests)

;;; nhwl-session-tests.el ends here
