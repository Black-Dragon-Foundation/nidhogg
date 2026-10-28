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
    ;; Names: UTF-8, Latin-1 (unibyte), invalid UTF-8.  APFS refuses a
    ;; name that is not UTF-8 (EILSEQ): on macOS such names cannot exist.
    (write-region "" nil (concat root (encode-coding-string "é.txt" 'utf-8))
                  nil 'silent)
    (unless (eq system-type 'darwin)
      (write-region "" nil (concat root (unibyte-string ?l #xe9 ?.))
                    nil 'silent)
      (write-region "" nil (concat root (unibyte-string ?b ?a ?d #xff #xfe))
                    nil 'silent))
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

(ert-deftest workers/dir-recursive-unsearchable-subdir ()
  ;; A subdirectory that can be listed but not searched (r, no x): its
  ;; entries' attributes fail.  It is unreadable, so the recursive read
  ;; skips it, as the Lisp walk does, instead of ending with an error.
  (skip-unless (not (zerop (user-uid))))
  (let* ((root (file-name-as-directory (make-temp-file "workers-blk" t)))
         (dir (directory-file-name root))
         (sub (expand-file-name "noexec" root)))
    (unwind-protect
        (progn
          (write-region "x" nil (expand-file-name "a.txt" root) nil 'silent)
          (make-directory sub)
          (write-region "x" nil (expand-file-name "in.txt" sub) nil 'silent)
          (set-file-modes sub #o400)
          (dolist (mode '(threads inline))
            (let ((nidhogg-workers--mode mode))
              (ert-info ((format "mode %s" mode) :prefix "")
                (let ((got (workers-blk--read dir nil :recursive t)))
                  (should-not (cadr got))
                  (should (equal (mapcar #'car (apply #'append (car got)))
                                 (mapcar #'car (workers-blk--walk root nil)))))))))
      (set-file-modes sub #o700)
      (delete-directory root t))))

(ert-deftest workers/dir-remote ()
  (let ((file-name-handler-alist
         ;; Not Tramp's syntax: a "/method:" name would reach Tramp's
         ;; autoload handler, which unhooks itself from the shared list.
         (cons (cons "\\`/nhmock/" #'workers-blk--mock-handler)
               file-name-handler-alist)))
    (should-error (nidhogg-directory-files-async "/nhmock/x/tmp" #'ignore)
                  :type 'nidhogg-not-local)))

;;; File blocks (Task 33)

(defun workers-blk-tests--read (file &rest props)
  "Read FILE with `nidhogg-read-file-async'; return (BLOCK ERROR)."
  (let (res (called-now nil))
    (should (apply #'nidhogg-read-file-async file
                   (lambda (block err) (setq res (list block err)))
                   props))
    (setq called-now res)
    (should-not called-now)
    (let ((i 0))
      (while (and (not res) (< i 500))
        (accept-process-output nil 0.02)
        (setq i (1+ i))))
    (should res)
    res))

(defun workers-blk-tests--state (inserter)
  (with-temp-buffer
    (let* ((ret (funcall inserter)))
      (list (buffer-string) enable-multibyte-characters (point)
            last-coding-system-used ret))))

(defun workers-blk-tests--compare (bytes &optional name beg end)
  "Compare inserting BYTES from a file via a block and `insert-file-contents'.
Return the insertion path of the block."
  (let ((file (make-temp-file "wkblk" nil (or name ".txt"))))
    (unwind-protect
        (progn
          (let ((coding-system-for-write 'no-conversion))
            (with-temp-file file
              (set-buffer-multibyte nil)
              (insert bytes)))
          (let* ((ref (workers-blk-tests--state
                       (lambda () (insert-file-contents file nil beg end))))
                 (blk (car (apply #'workers-blk-tests--read file
                                  (append (and beg (list :beg beg))
                                          (and end (list :end end))))))
                 (got (workers-blk-tests--state
                       (lambda () (nidhogg-insert-file-block blk)))))
            (should (user-ptrp blk))
            (should (equal ref got))
            nidhogg-blk--insert-path))
      (delete-file file))))

(ert-deftest workers/file-block-html-meta-past-head ()
  ;; Into an empty buffer `insert-file-contents' shows set-auto-coding
  ;; the whole text, and `sgml-html-meta-auto-coding-function' searches
  ;; all of it: a meta charset past the first KiB, with 3 KiB after it,
  ;; picks the coding for the block too.
  (workers-blk-tests--compare
   (concat "<!DOCTYPE html>\n<html><head>\n<style>\n"
           (make-string 2000 ?x)
           "\n</style>\n<meta charset=\"iso-8859-1\">\n</head><body>\n"
           "caf\351\n" (make-string 4000 ?y) "\n</body></html>\n")
   ".html"))

(ert-deftest workers/file-block ()
  (let ((utf8-first (eq (coding-system-get
                         (car (coding-system-priority-list)) :coding-type)
                        'utf-8)))
    ;; ASCII, with and without a line feed
    (should (eq (workers-blk-tests--compare "hello\nworld\n") 'fast))
    (should (eq (workers-blk-tests--compare "hello") 'fast))
    (should (eq (workers-blk-tests--compare "") 'decode))
    ;; UTF-8
    (let ((p (workers-blk-tests--compare
              (encode-coding-string "héllo 世界 \U0001F600\n" 'utf-8))))
      (when utf8-first (should (eq p 'fast))))
    (should (eq (workers-blk-tests--compare
                 (encode-coding-string "café" 'utf-8))
                (if utf8-first 'fast 'decode)))
    ;; CRLF and CR
    (should (eq (workers-blk-tests--compare "a\r\nb\r\n") 'decode))
    (should (eq (workers-blk-tests--compare "a\rb\r") 'decode))
    (should (eq (workers-blk-tests--compare
                 (encode-coding-string "é\r\nè\r\n" 'utf-8))
                'decode))
    ;; Latin-1, UTF-16 with a BOM, binary
    (workers-blk-tests--compare "caf\351 na\357ve\n")
    (workers-blk-tests--compare
     (encode-coding-string "﻿héllo\n" 'utf-16le-with-signature))
    (workers-blk-tests--compare (unibyte-string 0 1 2 255 254 0 10 13))
    (workers-blk-tests--compare (unibyte-string #xEF #xBB #xBF ?a ?\n))
    (workers-blk-tests--compare "esc \e[0m x\n")
    ;; coding cookies
    (workers-blk-tests--compare ";; -*- coding: latin-1 -*-\ncaf\351\n")
    (should (eq (workers-blk-tests--compare
                 (encode-coding-string
                  ";; -*- coding: latin-1 -*-\ncafé\n" 'utf-8))
                'decode))
    (should (eq (workers-blk-tests--compare
                 (encode-coding-string
                  ";; -*- coding: utf-8 -*-\ncafé\n" 'utf-8))
                'decode))
    (workers-blk-tests--compare
     (encode-coding-string
      (concat "café\n" (make-string 5000 ?x)
              "\n;; Local Var" "iables:\n;; coding: latin-1\n;; End:\n")
      'utf-8))
    (workers-blk-tests--compare
     (concat (make-string 6000 ?a) "\n"
             "\n;; Local Var" "iables:\n;; coding: iso-8859-5\n;; End:\n"))
    ;; file-coding-system-alist
    (let ((file-coding-system-alist
           (cons '("\\.wkalist\\'" . iso-8859-2) file-coding-system-alist)))
      (workers-blk-tests--compare "abc\n" ".wkalist")
      (workers-blk-tests--compare
       (encode-coding-string "café\n" 'utf-8) ".wkalist"))
    ;; coding-system-for-read bound
    (let ((coding-system-for-read 'latin-1))
      (workers-blk-tests--compare
       (encode-coding-string "café\n" 'utf-8)))
    (let ((coding-system-for-read 'utf-8-dos))
      (workers-blk-tests--compare "a\r\nb\n"))
    ;; a range
    (workers-blk-tests--compare "0123456789\nabcdef\n" ".txt" 3 14)
    ;; errors
    (let ((missing (expand-file-name "no-such-file" temporary-file-directory)))
      (let ((got (cadr (workers-blk-tests--read missing)))
            (ref (condition-case e (insert-file-contents missing)
                   (error e))))
        (should (equal got ref))))
    (let ((err (cadr (workers-blk-tests--read temporary-file-directory))))
      (should (eq (car err) 'file-error)))
    (should-error (nidhogg-insert-file-block 42) :type 'wrong-type-argument)
    (should-error (nidhogg-read-file-async "/ssh:nowhere:/etc/passwd" #'ignore)
                  :type 'nidhogg-not-local)))

;;; Host-name lookups (Task 35)

(defun workers-blk--dns-wait (pred)
  "Wait for PRED, at most about a minute (a lookup may time out)."
  (let ((i 0))
    (while (and (not (funcall pred)) (< i 6000))
      (accept-process-output nil 0.01)
      (setq i (1+ i)))
    (funcall pred)))

(defun workers-blk--dns-open (host service events)
  "Open a `:nowait' connection to HOST and SERVICE.
Sentinel events go onto the car of EVENTS.  Return the process, or
\(error ERR) when `make-network-process' signals."
  (condition-case err
      (make-network-process :name "wk-dns" :host host :service service
                            :family 'ipv4 :nowait t :noquery t
                            :sentinel (lambda (_p e) (push e (car events))))
    (error (list 'error err))))

(defun workers-blk--dns-outcome (host service &optional delete-now)
  "What Lisp sees of a `:nowait' connection to HOST and SERVICE.
A plist: `:states' (the status at creation, once the lookup is over
unless DELETE-NOW, and after `delete-process'), `:events' (the
sentinel calls, in order) and `:exit' (`process-exit-status' once the
lookup is over); or (error ERR) when `make-network-process' signals.
With DELETE-NOW the process is deleted right after its creation."
  (let* ((events (list nil))
         (p (workers-blk--dns-open host service events)))
    (if (eq (car-safe p) 'error)
        p
      (let ((states (list (process-status p))) exit)
        (unless delete-now
          (should (workers-blk--dns-wait
                   (lambda () (not (eq (process-status p) 'connect)))))
          (if (eq (process-status p) 'open)
              (should (workers-blk--dns-wait (lambda () (car events))))
            (dotimes (_ 2) (accept-process-output nil 0)))
          (push (process-status p) states)
          (setq exit (process-exit-status p)))
        (delete-process p)
        (dotimes (_ 2) (accept-process-output nil 0))
        (push (process-status p) states)
        (list :states (nreverse states) :events (reverse (car events))
              :exit exit)))))

(defun workers-blk--dns-held (port)
  "A lookup of localhost:PORT held at `in-lookup' and deleted meanwhile.
Return its outcome as `workers-blk--dns-outcome' with DELETE-NOW does."
  ;; Earlier jobs land first, so `:live' counts only this one.
  (should (nidhogg-workers--test-await-landed 'nh-blk))
  (nidhogg-workers--pause-at 'in-lookup)
  (unwind-protect
      (let* ((info (nidhogg-workers--test-dns-info))
             (events (list nil))
             (p (workers-blk--dns-open "localhost" port events))
             states)
        (should (processp p))
        (should (nidhogg-workers--test-await-paused 'in-lookup 5000))
        ;; The wait loop sees the lookup running.
        (accept-process-output nil 0)
        (push (process-status p) states)
        ;; A delete-process that waited on the job would release it here.
        (nidhogg-workers--test-release-on-wait 'in-lookup)
        (unwind-protect (delete-process p)
          (nidhogg-workers--test-release-on-wait nil))
        ;; It returned at once: the lookup is still held.
        (should (nidhogg-workers--test-await-paused 'in-lookup 0))
        (should (= (plist-get (nidhogg-workers--test-dns-info) :dropped)
                   (1+ (plist-get info :dropped))))
        (nidhogg-workers--release 'in-lookup)
        (should (nidhogg-workers--test-await-landed 'nh-blk))
        (dotimes (_ 2) (accept-process-output nil 0))
        (push (process-status p) states)
        ;; The dropped job is freed, and nothing reached the process.
        (should (= (plist-get (nidhogg-workers--test-dns-info) :live)
                   (plist-get info :live)))
        (list :states (nreverse states) :events (reverse (car events))
              :exit nil))
    (nidhogg-workers--release 'in-lookup)))

(ert-deftest workers/dns-nowait ()
  "`:nowait' lookups on nh-blk: Lisp sees what glibc's getaddrinfo_a path shows."
  (skip-unless (featurep 'make-network-process '(:nowait t)))
  (let* ((server (make-network-process :name "wk-dns-server" :server t
                                       :host 'local :service t :family 'ipv4
                                       :noquery t))
         (port (process-contact server :service))
         (cases `(("localhost" ,port)
                  ("localhost" "nh-wk-no-such-service")
                  ("nh-wk-no-such-host.invalid" ,port))))
    (unwind-protect
        (let* ((glibc (eq system-type 'gnu/linux))
               ;; glibc's getaddrinfo_a path.  Without getaddrinfo_a
               ;; (macOS) there is none to measure: nh-blk's inline
               ;; lookups are the reference, held to the same literal
               ;; outcomes below, and threads mode must equal them.
               (nidhogg-workers-dns (not glibc))
               (nidhogg-workers--mode 'inline)
               (nidhogg-workers--test-glibc-dns-off nil)
               (refs (mapcar (lambda (c) (apply #'workers-blk--dns-outcome c))
                             cases))
               (ref-deleted (workers-blk--dns-outcome "localhost" port t)))
          (should (equal (plist-get (nth 0 refs) :states)
                         '(connect open closed)))
          (should (equal (plist-get (nth 1 refs) :exit)
                         "Name lookup of localhost failed"))
          (should (equal (plist-get (nth 2 refs) :exit)
                         "Name lookup of nh-wk-no-such-host.invalid failed"))
          (should (equal (plist-get ref-deleted :states) '(connect closed)))
          (dolist (glibc-off '(nil t))
            (let ((nidhogg-workers--test-glibc-dns-off glibc-off))
              (ert-info ((format "glibc-dns-off %s" glibc-off) :prefix "")
                (when (or glibc-off (not glibc))
                  ;; Off, the path without getaddrinfo_a looks up
                  ;; synchronously: a bad name signals.
                  (let ((nidhogg-workers-dns nil))
                    (should (equal (apply #'workers-blk--dns-outcome
                                          (nth 0 cases))
                                   (nth 0 refs)))
                    (should (eq (car (apply #'workers-blk--dns-outcome
                                            (nth 1 cases)))
                                'error))))
                (dolist (mode '(threads inline))
                  (let ((nidhogg-workers--mode mode)
                        (nidhogg-workers-dns t))
                    (ert-info ((format "mode %s" mode) :prefix "")
                      (let ((started (plist-get (nidhogg-workers--test-dns-info)
                                                :started)))
                        (should (equal (mapcar (lambda (c)
                                                 (apply #'workers-blk--dns-outcome c))
                                               cases)
                                       refs))
                        (should (= (plist-get (nidhogg-workers--test-dns-info)
                                              :started)
                                   (+ started (length cases)))))
                      (when (eq mode 'threads)
                        (should (equal (workers-blk--dns-held port)
                                       ref-deleted))))))))))
      (nidhogg-workers--release nil)
      (dolist (p (process-list))
        (when (string-prefix-p "wk-dns" (process-name p))
          (delete-process p))))))

;;; workers-blk-tests.el ends here
