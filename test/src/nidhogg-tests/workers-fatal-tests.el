;;; workers-fatal-tests.el --- tests for workers: fatal paths  -*- lexical-binding: t -*-

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

;; The fatal path and exit with worker jobs in flight (Task 4).  Every
;; row runs a child Nidhogg, which may crash: core dumps are off, its
;; stderr goes to a file.  A crash on a worker or under a worker mutex
;; must reach the main thread's fatal path (the "Fatal error" line and
;; the backtrace on stderr, the emergency auto-save) and exit within
;; the bound; a signal handler never takes a worker mutex.  The crash
;; rows run in `threads' mode only: in `inline' mode there is no worker
;; thread to crash or to hold a job in flight.

;;; Code:

(require 'ert)
(require 'seq)

(defun workers-fatal-tests--run (args &rest props)
  "Run a child Nidhogg with ARGS; return a plist of what happened.
PROPS: `:pty' (non-nil: its terminal is a pty, so a fatal error prints
its \"Fatal error\" line), `:env' (strings added to the environment),
`:answers' (an alist (REGEXP . STRING): the first time the output
matches REGEXP, STRING is sent to the child), `:mark' (a regexp in
the output from which `:secs' counts).  The child is killed after 30
seconds.  The plist: `:status' and `:code' (`process-status' and
`process-exit-status'), `:secs' (from the mark to the exit, or nil),
`:out' (its output), `:err' (its stderr)."
  (let* ((err (make-temp-file "nh-wk-err"))
         (out "")
         (pending (plist-get props :answers))
         (mark (plist-get props :mark))
         (marked nil)
         (start (float-time))
         (process-environment (append (plist-get props :env)
                                      process-environment))
         (proc (make-process
                :name "nh-wk-fatal" :buffer nil :noquery t
                :connection-type (if (plist-get props :pty) 'pty 'pipe)
                :command (append
                          (list "/bin/sh" "-c"
                                "ulimit -c 0; err=$1; shift; exec \"$@\" 2>\"$err\""
                                "sh" err
                                (expand-file-name invocation-name
                                                  invocation-directory))
                          args)
                :filter (lambda (p s)
                          (setq out (concat out s))
                          (when (and mark (not marked) (string-match-p mark out))
                            (setq marked (float-time)))
                          (dolist (a (copy-sequence pending))
                            (when (string-match-p (car a) out)
                              (setq pending (delq a pending))
                              (process-send-string p (cdr a))))))))
    (while (and (process-live-p proc) (< (- (float-time) start) 30))
      (accept-process-output proc 0.05))
    (let ((end (float-time)))
      (when (process-live-p proc)
        (delete-process proc))
      (accept-process-output nil 0)
      (prog1 (list :status (process-status proc)
                   :code (process-exit-status proc)
                   :secs (and marked (- end marked))
                   :out out
                   :err (with-temp-buffer
                          (insert-file-contents err)
                          (buffer-string)))
        (delete-file err)))))

(defun workers-fatal-tests--batch (form &rest props)
  "Run FORM in a batch child Nidhogg in `threads' mode; see `workers-fatal-tests--run'."
  (apply #'workers-fatal-tests--run
         (list "-Q" "--batch" "--eval"
               (prin1-to-string
                `(progn (setq nidhogg-workers--mode 'threads) ,form)))
         props))

(defun workers-fatal-tests--edit (file text)
  "A form that visits FILE, inserts TEXT and turns on auto-saving."
  `(progn (find-file ,file)
          (insert ,text)
          (auto-save-mode 1)))

(defun workers-fatal-tests--auto-saved (dir name)
  "The text of the auto-save file of DIR/NAME, or nil."
  (let ((f (expand-file-name (concat "#" name "#") dir)))
    (and (file-exists-p f)
         (with-temp-buffer
           (insert-file-contents f)
           (buffer-string)))))

(defun workers-fatal-tests--should-die (r sig name)
  "R (from `workers-fatal-tests--run') died of signal SIG through the fatal path."
  (should (equal (list (plist-get r :status) (plist-get r :code))
                 (list 'signal (cdr sig))))
  (should (plist-get r :secs))
  (should (< (plist-get r :secs) 5))
  (let ((err (plist-get r :err)))
    (should (string-match-p (format "^Fatal error %d: %s" (cdr sig) name) err))
    (should (string-match-p "^Backtrace:" err))))

(defmacro workers-fatal-tests--in-dir (dir &rest body)
  "Run BODY with DIR bound to a new temporary directory, then delete it."
  (declare (indent 1) (debug t))
  `(let ((,dir (make-temp-file "nh-wk-fatal" t)))
     (unwind-protect (progn ,@body)
       (delete-directory ,dir t))))

(defun workers-fatal-tests--terminfo-p ()
  (seq-some (lambda (d) (file-exists-p (expand-file-name "x/xterm" d)))
            '("/usr/share/terminfo" "/lib/terminfo" "/etc/terminfo")))

(ert-deftest workers/fatal-under-lock ()
  ;; §17.3 B4.  The main thread holds nh-blk's queue mutex with a job in
  ;; flight (parked at before-done) when it crashes: the fatal path
  ;; never takes a worker mutex, so the child prints the backtrace,
  ;; writes the emergency auto-save and dies, within 5 s.
  (workers-fatal-tests--in-dir dir
    (let* ((file (expand-file-name "a.txt" dir))
           (r (workers-fatal-tests--batch
               `(progn
                  ,(workers-fatal-tests--edit file "fatal under lock")
                  (nidhogg-workers--pause-at 'before-done)
                  (nidhogg-workers--test-job 'nh-blk 0 :pause 'before-done)
                  (unless (nidhogg-workers--test-await-paused 'before-done 5000)
                    (kill-emacs 3))
                  (princ "nh-wk: ready\n")
                  (nidhogg-workers--test-fatal-under-lock 'nh-blk))
               :pty t :mark "nh-wk: ready")))
      (ert-info ((format "%S" r) :prefix "child: ")
        (workers-fatal-tests--should-die r '(SIGSEGV . 11) "Segmentation fault")
        (should (equal (workers-fatal-tests--auto-saved dir "a.txt")
                       "fatal under lock")))))
  ;; The same through SIGINT's tty emergency escape: a tty child (-nw on
  ;; a pty) holds the mutex in a timer, gets C-g twice, answers "y" to
  ;; "Auto-save?" and to "Abort (and dump core)?".
  (should (workers-fatal-tests--terminfo-p))
  (workers-fatal-tests--in-dir dir
    (let* ((file (expand-file-name "b.txt" dir))
           (r (workers-fatal-tests--run
               (list "-nw" "-Q" "--eval"
                     (prin1-to-string
                      `(progn
                         (setq nidhogg-workers--mode 'threads)
                         ,(workers-fatal-tests--edit file "emergency escape")
                         (nidhogg-workers--pause-at 'before-done)
                         (nidhogg-workers--test-job 'nh-blk 0 :pause 'before-done)
                         (run-at-time
                          0 nil
                          (lambda ()
                            (if (nidhogg-workers--test-await-paused 'before-done 5000)
                                (nidhogg-workers--test-fatal-under-lock 'nh-blk 'sigint)
                              (kill-emacs 3)))))))
               :pty t :env '("TERM=xterm")
               :mark "nh-wk: holding"
               :answers `(("nh-wk: holding" . "\C-g")
                          ("nh-wk: quit" . "\C-g")
                          (,(regexp-quote "Auto-save? (y or n)") . "y\n")
                          (,(regexp-quote "Abort (and dump core)? (y or n)") . "y\n")))))
      (ert-info ((format "%S" (plist-put (copy-sequence r) :out nil))
                 :prefix "child: ")
        (should (string-search "Auto-save done" (plist-get r :out)))
        (workers-fatal-tests--should-die r '(SIGABRT . 6) "Aborted")
        (should (equal (workers-fatal-tests--auto-saved dir "b.txt")
                       "emergency escape"))))))

(ert-deftest workers/worker-crash-forwarded ()
  ;; §17.3 S10.  A job on nh-blk writes through a null pointer: the
  ;; worker's SIGSEGV reaches the main thread's fatal path, whose
  ;; diagnostic names the signal, and the emergency auto-save exists.
  (workers-fatal-tests--in-dir dir
    (let* ((file (expand-file-name "c.txt" dir))
           (r (workers-fatal-tests--batch
               `(progn
                  ,(workers-fatal-tests--edit file "worker crash")
                  (princ "nh-wk: ready\n")
                  (unless (nidhogg-workers--test-crash 'nh-blk)
                    (kill-emacs 3))
                  (while t
                    (accept-process-output nil 0.05)))
               :pty t :mark "nh-wk: ready")))
      (ert-info ((format "%S" r) :prefix "child: ")
        (workers-fatal-tests--should-die r '(SIGSEGV . 11) "Segmentation fault")
        (should (equal (workers-fatal-tests--auto-saved dir "c.txt")
                       "worker crash"))))))

(ert-deftest workers/kill-emacs-with-jobs-in-flight ()
  ;; Review Focus 4.  Every worker has a job parked at a pause point when
  ;; a timer calls `kill-emacs': the exit does not hang (status 0 within
  ;; 2 s), no temporary auto-save file is left, and the auto-save file
  ;; holds the buffer's text.  Tasks 29 and 34 add their workers' jobs.
  (workers-fatal-tests--in-dir dir
    (let* ((file (expand-file-name "d.txt" dir))
           (r (workers-fatal-tests--batch
               `(progn
                  ,(workers-fatal-tests--edit file "jobs in flight")
                  (nidhogg-workers--pause-at 'before-done)
                  (dolist (w '(nh-io nh-parse nh-img nh-blk nh-save))
                    (nidhogg-workers--test-job w 0 :pause 'before-done))
                  (unless (nidhogg-workers--test-await-paused 'before-done 5000 5)
                    (kill-emacs 3))
                  (princ "nh-wk: ready\n")
                  (run-at-time 0 nil #'kill-emacs)
                  (while t
                    (accept-process-output nil 0.05)))
               :mark "nh-wk: ready")))
      (ert-info ((format "%S" r) :prefix "child: ")
        (should (equal (list (plist-get r :status) (plist-get r :code))
                       '(exit 0)))
        (should (plist-get r :secs))
        (should (< (plist-get r :secs) 2))
        (should-not (directory-files dir nil "\\.nh-"))
        (should (equal (workers-fatal-tests--auto-saved dir "d.txt")
                       "jobs in flight"))))))

(ert-deftest workers/no-lock-in-signal-context ()
  ;; §4 item 11.  A SIGCHLD storm of 1,000 signals while the main thread
  ;; holds each worker mutex in turn: each handler calls submit, wait and
  ;; cancel, which refuse without a lock; the mutex-in-signal counter
  ;; stays 0 (a lock in signal context aborts the child).
  (dolist (mode '(threads inline))
    (ert-info ((format "mode %s" mode) :prefix "")
      (let ((r (workers-fatal-tests--run
                (list "-Q" "--batch" "--eval"
                      (prin1-to-string
                       `(progn
                          (setq nidhogg-workers--mode ',mode)
                          (prin1 (nidhogg-workers--test-signal-under-locks 1000))))))))
        (ert-info ((format "%S" r) :prefix "child: ")
          (should (equal (list (plist-get r :status) (plist-get r :code))
                         '(exit 0)))
          (let ((res (car (read-from-string (plist-get r :out)))))
            (should (>= (plist-get res :handled) 1000))
            (should (eql (plist-get res :signal-locks) 0))))))))

;;; workers-fatal-tests.el ends here
