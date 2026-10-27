;;; watchdog-hang-tests.el --- tests for the stall watchdog (hang)  -*- lexical-binding: t -*-

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

;; Hangs leave evidence: names mode in the signal
;; handler, the hang file the monitor writes and deletes, and the `:killed'
;; records a mode start makes of a gone session's hang files.
;;
;; The names-mode rows take synchronous samples without a monitor.  The
;; hang-file rows run the real monitor (threshold 100 ms, hang 300 ms) and
;; meet it at fixed points with `nidhogg-watchdog--test-hold-rename': its
;; write held between `fsync' and `rename', the write finished, the watch
;; of the hang's span ended.  They assert no times.  The killed-session row
;; rows on real processes are `watchdog/hang-file-survives-kill', `-sigterm'
;; and `-sighup' in the timing group.

;;; Code:

(require 'ert)
(require 'cl-lib)

(defmacro watchdog-hang--sampling (&rest body)
  "Run BODY with the watchdog started without a monitor thread."
  (declare (indent 0) (debug t))
  `(progn
     (nidhogg-watchdog--start 100 2000 nil :no-monitor)
     (unwind-protect
         (progn ,@body)
       (nidhogg-watchdog--stop))))

(defun watchdog-hang--define ()
  "Define `wdtest-outer', which calls `wdtest-inner', which samples.
`wdtest-via-lambda' calls `wdtest-outer' from a closure.  Interpreted, so
no frame is optimized away."
  (eval '(progn
           (defun wdtest-inner () (nidhogg-watchdog--test-sample))
           (defun wdtest-outer () (wdtest-inner))
           (defun wdtest-via-lambda () (funcall (lambda () (wdtest-outer)))))
        t))

(defun watchdog-hang--names ()
  (plist-get (nidhogg-watchdog--test-names) :names))

(defun watchdog-hang--count ()
  (plist-get (nidhogg-watchdog--test-names) :count))

;;;; Names mode

(ert-deftest watchdog/names-mode-below-innermost ()
  (watchdog-hang--define)
  (watchdog-hang--sampling
    (should (= 0 (watchdog-hang--count)))
    (nidhogg-watchdog--test-hang-now)
    (wdtest-via-lambda)
    (let ((names (watchdog-hang--names)))
      (should (= 1 (watchdog-hang--count)))
      (should (<= 3 (length names) 32))
      ;; The innermost frame (the sampling primitive) is never followed.
      (should (null (car names)))
      (should-not (member "nidhogg-watchdog--test-sample" names))
      ;; The frames below it are named, innermost first; a closure is
      ;; `<lambda>'.
      (should (equal "wdtest-inner" (nth 1 names)))
      (should (equal "wdtest-outer" (nth 2 names)))
      (should (member "<lambda>" names))
      (should (cl-every #'stringp (cdr names))))
    ;; Without the flag (and no monitor to set names mode) no names.
    (wdtest-outer)
    (should (= 1 (watchdog-hang--count)))))

(ert-deftest watchdog/names-mode-off-during-gc ()
  (watchdog-hang--define)
  (watchdog-hang--sampling
    (nidhogg-watchdog--test-hang-now)
    (wdtest-outer)
    (should (= 1 (watchdog-hang--count)))
    ;; In a collection: the raw words are copied, no name is followed.
    (nidhogg-watchdog--test-hang-now t)
    (let ((s (wdtest-outer)))
      (should (plist-get s :gc))
      (should (memq 'wdtest-outer (append (plist-get s :frames) nil))))
    (should (= 1 (watchdog-hang--count)))
    ;; The flag is used up by one sample; outside a collection the next
    ;; forced one names again.
    (wdtest-outer)
    (should (= 1 (watchdog-hang--count)))
    (nidhogg-watchdog--test-hang-now)
    (wdtest-outer)
    (should (= 2 (watchdog-hang--count)))
    (should (member "wdtest-outer" (watchdog-hang--names)))))

(ert-deftest watchdog/names-cut-at-a-character ()
  ;; A name is cut at 63 bytes, before a character and never inside one: a
  ;; split UTF-8 sequence would make the hang file invalid text.  40 two-
  ;; byte characters keep 31.
  (let ((sym (intern (make-string 40 #xe9))))
    (unwind-protect
        (progn
          (watchdog-hang--define)
          (eval `(defun ,sym () (wdtest-outer)) t)
          (watchdog-hang--sampling
            (nidhogg-watchdog--test-hang-now)
            (funcall sym)
            (should (member (make-string 31 #xe9) (watchdog-hang--names)))))
      (fmakunbound sym))))

;;;; Hang files

(defmacro watchdog-hang--with-log (&rest body)
  "Run BODY with a fresh log directory and fresh per-day counters."
  (declare (indent 0) (debug t))
  `(let* ((dir (make-temp-file "nh-wd-hang" t))
          (nidhogg-watchdog-log-directory (file-name-as-directory dir))
          (nidhogg-watchdog--day nil)
          (nidhogg-watchdog--day-dir nil)
          (nidhogg-watchdog--day-stalls 0)
          (nidhogg-watchdog--over-cap 0)
          (nidhogg-watchdog-record-functions nil))
     (unwind-protect (progn ,@body)
       (delete-directory dir t))))

(defun watchdog-hang--fixture (name pid proc-start boot-id)
  "Write the hang file NAME for PID with PROC-START and BOOT-ID; return it."
  (let ((file (expand-file-name name nidhogg-watchdog-log-directory)))
    (with-temp-file file
      (insert "nidhogg-watchdog-hang 1\n"
              (format "pid %d\nproc-start %s\nboot-id %s\n"
                      pid proc-start boot-id)
              "start 1792345678.5\nduration-ms 4012.0\nphase regex\n"
              "stack\twdtest-outer\trun-hooks\tcommand-execute\n"
              "stack\twdtest-older\n"))
    file))

(defun watchdog-hang--log-lines ()
  "Every line of every day file, as strings."
  (let (out)
    (dolist (f (directory-files nidhogg-watchdog-log-directory t "\\.jsonl\\'"))
      (with-temp-buffer
        (insert-file-contents f)
        (setq out (append out (split-string (buffer-string) "\n" t)))))
    out))

(ert-deftest watchdog/hang-file-identity ()
  ;; A hang file names its process by pid, start time and boot id: a live
  ;; pid with another start time (reused) or another boot is gone.
  (skip-unless (file-readable-p "/proc/self/stat"))
  (watchdog-hang--with-log
    (let* ((pid (emacs-pid))
           (start (nidhogg-watchdog--proc-start pid))
           (boot (nidhogg-watchdog--boot-id))
           (reused (watchdog-hang--fixture (format "hang-%d-1.txt" pid)
                                           pid "1" boot))
           (rebooted (watchdog-hang--fixture
                      (format "hang-%d-2.txt" pid) pid start
                      "00000000-0000-0000-0000-000000000000"))
           (alive (watchdog-hang--fixture (format "hang-%d-3.txt" pid)
                                          pid start boot))
           (seen nil)
           (nidhogg-watchdog-record-functions
            (list (lambda (r) (push r seen)))))
      (should (string-match-p "\\`[0-9]+\\'" start))
      (should boot)
      (should (= 2 (nidhogg-watchdog--collect-hangs)))
      (should-not (file-exists-p reused))
      (should-not (file-exists-p rebooted))
      ;; A live session's hang is its own business.
      (should (file-exists-p alive))
      (should (= 2 (length seen)))
      (dolist (r seen)
        (should (eq t (plist-get r :killed)))
        (should (eq t (plist-get r :hang)))
        (should (= pid (plist-get r :hang-pid)))
        (should (= 4012.0 (plist-get r :wall-ms)))
        (should (equal '(:regex 1.0) (plist-get r :phases)))
        (should (equal '((:n 1 :frames ["wdtest-outer" "run-hooks"
                                         "command-execute"])
                         (:n 1 :frames ["wdtest-older"]))
                       (plist-get r :stacks))))
      ;; Both went to the log.
      (should (= 2 (cl-count-if (lambda (l) (string-search "\"killed\":true" l))
                                (watchdog-hang--log-lines)))))))

(ert-deftest watchdog/hang-file-deleted-when-its-record-fails ()
  ;; A killed hang whose line cannot be written (say, a name that is not
  ;; valid text) is still deleted, and the files after it are collected:
  ;; kept, it would fail again at every start.
  (skip-unless (file-readable-p "/proc/self/stat"))
  (watchdog-hang--with-log
    (let* ((pid (emacs-pid))
           (boot (nidhogg-watchdog--boot-id))
           (a (watchdog-hang--fixture (format "hang-%d-1.txt" pid) pid "x" boot))
           (b (watchdog-hang--fixture (format "hang-%d-2.txt" pid) pid "x" boot))
           (calls 0))
      (cl-letf (((symbol-function 'nidhogg-watchdog--log-write)
                 (lambda (&rest _)
                   (when (= 1 (setq calls (1+ calls)))
                     (error "A line that cannot be written"))
                   t)))
        (should (= 1 (nidhogg-watchdog--collect-hangs))))
      (should (= 2 calls))
      (should-not (file-exists-p a))
      (should-not (file-exists-p b)))))

(defun watchdog-hang--hanging (fn)
  "Call FN with the hang file's name, the watchdog and its monitor on.
Threshold 100 ms, hang 300 ms; FN runs in a fresh busy span."
  (watchdog-hang--with-log
    (let ((file (expand-file-name "hang-1-1.txt" nidhogg-watchdog-log-directory)))
      (nidhogg-watchdog--start 100 300 file)
      (unwind-protect
          (progn (nidhogg-watchdog--test-idle-wait 0.01)
                 (nidhogg-watchdog--take)
                 (funcall fn file))
        (nidhogg-watchdog--test-hold-rename nil)
        (nidhogg-watchdog--stop)))))

(defun watchdog-hang--await ()
  "Wait, busy, until the monitor holds a hang-file write; t if it does."
  (nidhogg-watchdog--test-hold-rename :held))

(defun watchdog-hang--files ()
  "The hang files and their temporary files in the log directory."
  (directory-files nidhogg-watchdog-log-directory nil "\\`hang-"))

(defun watchdog-hang--stall ()
  "Take the stalls; return the hang's (at least 300 ms)."
  (cl-find-if (lambda (s) (>= (plist-get s :wall-ms) 300))
              (car (nidhogg-watchdog--take))))

(ert-deftest watchdog/hang-file-schema ()
  (watchdog-hang--hanging
   (lambda (file)
     (nidhogg-watchdog--test-hold-rename t)
     (should (watchdog-hang--await))
     ;; Held between fsync and rename: only the temporary file exists.
     (should-not (file-exists-p file))
     (should (file-exists-p (concat file ".tmp")))
     ;; Released while the span is still busy: renamed.
     (should (nidhogg-watchdog--test-hold-rename nil))
     (should (file-exists-p file))
     (should-not (file-exists-p (concat file ".tmp")))
     (let ((h (nidhogg-watchdog--read-hang-file file)))
       (should h)
       (should (= (emacs-pid) (plist-get h :pid)))
       (should (equal (nidhogg-watchdog--proc-start (emacs-pid))
                      (plist-get h :proc-start)))
       (should (equal (nidhogg-watchdog--boot-id) (plist-get h :boot-id)))
       (should (< (abs (- (plist-get h :start) (float-time))) 60))
       (should (>= (plist-get h :duration-ms) 300))
       (should (member (plist-get h :phase)
                       '("lisp" "process-wait" "call-process")))
       ;; Names-mode stacks: the frames below the innermost (the waiting
       ;; primitive, which is never named).
       (should (plist-get h :stacks))
       (should (cl-some (lambda (st) (member "watchdog-hang--await" st))
                        (plist-get h :stacks)))
       (should-not (cl-some (lambda (st)
                              (member "nidhogg-watchdog--test-hold-rename" st))
                            (plist-get h :stacks)))
       ;; This session is alive: not a killed hang.
       (should-not (nidhogg-watchdog--hang-dead-p h))))))

(ert-deftest watchdog/hang-file-deleted-at-span-end ()
  (watchdog-hang--hanging
   (lambda (file)
     (nidhogg-watchdog--test-hold-rename t)
     (should (watchdog-hang--await))
     (should (nidhogg-watchdog--test-hold-rename nil))
     (should (file-exists-p file))
     ;; The hang's span ends; the monitor ends its watch and deletes the
     ;; file before it parks.
     (nidhogg-watchdog--test-idle-wait 0.01)
     (should (nidhogg-watchdog--test-hold-rename :ended))
     (should-not (watchdog-hang--files))
     (let ((s (watchdog-hang--stall)))
       (should s)
       (should (eq t (plist-get s :hang)))))))

(ert-deftest watchdog/hang-ends-during-write ()
  ;; The span ends while the monitor is between fsync and rename: the
  ;; rename is skipped, so no finished hang comes back.
  (watchdog-hang--hanging
   (lambda (_file)
     (nidhogg-watchdog--test-hold-rename t)
     (should (watchdog-hang--await))
     (nidhogg-watchdog--test-idle-wait 0.01)
     (should (nidhogg-watchdog--test-hold-rename nil))
     (should-not (watchdog-hang--files))
     (should (nidhogg-watchdog--test-hold-rename :ended))
     (should-not (watchdog-hang--files))
     (let ((s (watchdog-hang--stall)))
       (should s)
       (should (eq t (plist-get s :hang)))))))

(ert-deftest watchdog/hang-file-kept-by-stop-in-hang ()
  ;; `kill-emacs', run by SIGTERM or SIGHUP from the signal handler, turns
  ;; the mode off while the hang's span is still busy: the stop keeps the
  ;; file for the next start's `:killed' record.
  (watchdog-hang--hanging
   (lambda (file)
     (nidhogg-watchdog--test-hold-rename t)
     (should (watchdog-hang--await))
     (should (nidhogg-watchdog--test-hold-rename nil))
     (should (file-exists-p file))
     (nidhogg-watchdog--stop)
     (should (file-exists-p file)))))

(ert-deftest watchdog/own-hang-file-deleted ()
  ;; A file kept by a stop inside a hang in this same session (Emacs did
  ;; not exit) is no killed hang: the next start deletes it, no record.
  (skip-unless (file-readable-p "/proc/self/stat"))
  (watchdog-hang--with-log
    (let* ((own (watchdog-hang--fixture
                 (file-name-nondirectory (nidhogg-watchdog--hang-file))
                 (emacs-pid) (nidhogg-watchdog--proc-start (emacs-pid))
                 (nidhogg-watchdog--boot-id)))
           (seen nil)
           (nidhogg-watchdog-record-functions (list (lambda (r) (push r seen)))))
      (should (= 0 (nidhogg-watchdog--collect-hangs)))
      (should-not (file-exists-p own))
      (should-not seen))))

(defun watchdog-hang--open-files ()
  "The files this process has open, by `/proc/self/fd'."
  (delq nil (mapcar #'file-symlink-p
                    (directory-files "/proc/self/fd" t "\\`[0-9]"))))

(ert-deftest watchdog/hang-dir-held-for-fsync ()
  ;; The monitor holds the log directory open while it runs, to fsync it
  ;; after a hang file's first rename (only then is the rename durable),
  ;; and closes it at stop.
  (skip-unless (file-directory-p "/proc/self/fd"))
  (watchdog-hang--hanging
   (lambda (file)
     (let ((dir (directory-file-name
                 (file-truename (file-name-directory file)))))
       (should (member dir (watchdog-hang--open-files)))
       (nidhogg-watchdog--stop)
       (should-not (member dir (watchdog-hang--open-files)))))))

;;; watchdog-hang-tests.el ends here
