;;; workers-blk-tests.el --- tests for workers: directory reads and file blocks  -*- lexical-binding: t -*-

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

;;; Code:

(require 'ert)
(require 'cl-lib)

;;; Directory reads (Task 32)

(defun workers-blk--wait (pred &optional busy)
  "Wait for PRED, running BUSY (a function) between short waits."
  (let ((i 0))
    (while (and (not (funcall pred)) (< i 1500))
      (when busy (funcall busy))
      (accept-process-output nil 0.01)
      (setq i (1+ i)))
    (funcall pred)))

(defun workers-blk--read (dir &optional busy &rest keys)
  "Run `nidhogg-directory-files-async' on DIR with KEYS.
Return (BATCHES ERROR), BATCHES the lists delivered, in order."
  (let (batches error done)
    (apply #'nidhogg-directory-files-async dir
           (lambda (batch d err)
             (push batch batches)
             (when err (setq error err))
             (when d (setq done t)))
           keys)
    (should (workers-blk--wait (lambda () done) busy))
    (list (nreverse batches) error)))

(defun workers-blk--tree ()
  "A fixture tree in a fresh temporary directory; return its name."
  (let* ((root (file-name-as-directory (make-temp-file "workers-blk" t)))
         (big (expand-file-name "big/" root))
         (n (max 2500 (round (* 50000 (string-to-number
                                       (or (getenv "NIDHOGG_WORKERS_TEST_SIZE_SCALE")
                                           "1")))))))
    (dolist (d '("sub/" "sub/deep/" "sub/deep/deeper/" "skipme/" "skipme/in/"
                 "empty/" "big/" "locked/"))
      (make-directory (expand-file-name d root) t))
    (dolist (f '("a.txt" "b.txt" "c.log" "sub/x.el" "sub/y.el" "sub/deep/z.txt"
                 "sub/deep/deeper/w.txt" "skipme/in/hidden.txt" "locked/q"))
      (write-region "x" nil (expand-file-name f root) nil 'silent))
    ;; Names: UTF-8, Latin-1 (unibyte), invalid UTF-8.
    (write-region "" nil (concat root (encode-coding-string "é.txt" 'utf-8))
                  nil 'silent)
    (write-region "" nil (concat root (unibyte-string ?l #xe9 ?.))
                  nil 'silent)
    (write-region "" nil (concat root (unibyte-string ?b ?a ?d #xff #xfe))
                  nil 'silent)
    ;; Symlinks to a file, a directory and nowhere.
    (make-symbolic-link "a.txt" (concat root "link-file"))
    (make-symbolic-link "sub" (concat root "link-dir"))
    (make-symbolic-link "no-such-target" (concat root "link-nowhere"))
    (dotimes (i n)
      (write-region "" nil (format "%sf%06d" big i) nil 'silent))
    (set-file-modes (expand-file-name "locked" root) #o000)
    root))

(defun workers-blk--clean (root)
  (set-file-modes (expand-file-name "locked" root) #o700)
  (delete-directory root t))

(defun workers-blk--walk (dir skip &optional id-format)
  "A Lisp walk of DIR, breadth first, as `nidhogg-directory-files-async'
with :recursive does it."
  (let ((queue (list "")) out)
    (while queue
      (let* ((rel (pop queue))
             (top (equal rel ""))
             (ents (condition-case nil
                       (directory-files-and-attributes
                        (concat dir rel) nil nil nil id-format)
                     (error nil))))
        (dolist (e ents)
          (let ((base (car e)))
            (unless (and (not top) (member base '("." "..")))
              (let ((path (if top base (concat rel "/" base))))
                (push (cons path (cdr e)) out)
                (when (and (eq (cadr e) t)
                           (not (member base '("." "..")))
                           (not (member base skip)))
                  (setq queue (append queue (list (concat (if top "" (concat rel "/"))
                                                          base)))))))))))
    (nreverse out)))

(defun workers-blk--error (dir)
  (condition-case e (progn (directory-files-and-attributes dir) nil)
    (error e)))

(ert-deftest workers/dir-equivalence ()
  (let* ((root (workers-blk--tree))
         (dir (directory-file-name root)))
    (unwind-protect
        (dolist (mode '(threads inline))
          (let ((nidhogg-workers--mode mode))
            (ert-info ((format "mode %s" mode) :prefix "")
              ;; Settle every directory's atime: a read updates it once
              ;; (relatime), and the recursive read lists a directory
              ;; before it opens it, the Lisp walk after.
              (workers-blk--walk root nil)
              ;; MATCH, NOSORT and ID-FORMAT, against the builtin.
              (dolist (match '(nil "\\`[a-c]" "\\.txt\\'"))
                (dolist (nosort '(nil t))
                  (dolist (idf '(nil integer string))
                    (let* ((want (directory-files-and-attributes
                                  dir nil match nosort idf))
                           (got (workers-blk--read
                                 dir nil :match match :nosort nosort
                                 :id-format idf))
                           (batches (car got)))
                      (should-not (cadr got))
                      (should (equal (apply #'append batches) want))
                      (unless nosort (should (= (length batches) 1)))))))
              ;; Large directory: NOSORT streams, sorted delivers once.
              (let* ((big (concat dir "/big"))
                     (want (directory-files-and-attributes big))
                     (sorted (car (workers-blk--read big))))
                (should (= (length sorted) 1))
                (should (equal (car sorted) want))
                (let ((nos (car (workers-blk--read big nil :nosort t))))
                  (should (> (length nos) 1))
                  (should (equal (apply #'append nos)
                                 (directory-files-and-attributes
                                  big nil nil t)))))
              ;; ID-FORMAT string while the main thread works on names.
              (let ((want (directory-files-and-attributes
                           dir nil nil nil 'string))
                    (got (car (workers-blk--read
                               dir
                               (lambda ()
                                 (user-login-name)
                                 (file-attributes dir 'string))
                               :id-format 'string))))
                (should (equal (apply #'append got) want)))
              ;; Recursive, with a skip list, a depth and a cap.
              (let ((got (car (workers-blk--read
                               dir nil :recursive t :skip '("skipme")))))
                (should (= (length got) 1))
                (should (equal (car got)
                               (workers-blk--walk (concat dir "/") '("skipme")))))
              (let ((all (apply #'append
                                (car (workers-blk--read dir nil :recursive t
                                                        :max-depth 1)))))
                (should (assoc "sub/x.el" all))
                (should-not (assoc "sub/deep/z.txt" all)))
              (should (= 5 (length (apply #'append
                                          (car (workers-blk--read
                                                dir nil :max-entries 5))))))
              ;; Errors are the ones directory-files signals.
              (let ((gone (concat dir "/nowhere")))
                (let ((got (workers-blk--read gone)))
                  (should (equal (cadr got) (workers-blk--error gone)))
                  (should (equal (car got) '(nil)))))
              (let ((file (concat dir "/a.txt")))
                (should (equal (cadr (workers-blk--read file))
                               (workers-blk--error file))))
              ;; An unreadable directory: the same outcome as the builtin.
              (let* ((locked (concat dir "/locked"))
                     (want (workers-blk--error locked))
                     (got (workers-blk--read locked)))
                (should (equal (cadr got) want))))))
      (workers-blk--clean root))))

(defun workers-blk--mock-handler (operation &rest args)
  "A file name handler that does OPERATION as if it were not there.
Like a remote one, it answers `expand-file-name', which
`directory-files' calls before it looks for a handler."
  (let ((inhibit-file-name-handlers
         (cons #'workers-blk--mock-handler
               (and (eq inhibit-file-name-operation operation)
                    inhibit-file-name-handlers)))
        (inhibit-file-name-operation operation))
    (apply operation args)))

(ert-deftest workers/dir-remote ()
  (let ((file-name-handler-alist
         ;; Not Tramp's syntax: a "/method:" name would reach Tramp's
         ;; autoload handler, which unhooks itself from the shared list.
         (cons (cons "\\`/nhmock/" #'workers-blk--mock-handler)
               file-name-handler-alist)))
    (should-error (nidhogg-directory-files-async "/nhmock/x/tmp" #'ignore)
                  :type 'nidhogg-not-local)))

;;; workers-blk-tests.el ends here
