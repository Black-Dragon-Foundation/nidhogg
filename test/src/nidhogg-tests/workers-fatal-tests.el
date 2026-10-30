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
  ;; On macOS the NS build ends every fatal signal with abort(): after
  ;; shut_down_emacs prints "Fatal error N: ..." (src/emacs.c:3145),
  ;; ns_term_shutdown calls emacs_abort to "force a stack trace"
  ;; (src/nsterm.m:6186-6187), which re-enters terminate_due_to_signal
  ;; with the fatal error already in progress (src/emacs.c:451-472) and
  ;; raises SIGABRT.  So the child dies of signal 6, prints no Backtrace,
  ;; and the system names the signal itself ("Abort trap: 6").
  (let ((darwin (eq system-type 'darwin)))
    (should (equal (list (plist-get r :status) (plist-get r :code))
                   (list 'signal (if darwin 6 (cdr sig)))))
    (should (plist-get r :secs))
    (should (< (plist-get r :secs) 5))
    (let ((err (plist-get r :err)))
      (should (string-match-p (format "^Fatal error %d: %s" (cdr sig)
                                      (if darwin "" name))
                              err))
      (unless darwin
        (should (string-match-p "^Backtrace:" err))))))

(defmacro workers-fatal-tests--in-dir (dir &rest body)
  "Run BODY with DIR bound to a new temporary directory, then delete it."
  (declare (indent 1) (debug t))
  `(let ((,dir (make-temp-file "nh-wk-fatal" t)))
     (unwind-protect (progn ,@body)
       (delete-directory ,dir t))))

(defun workers-fatal-tests--terminfo-p ()
  ;; Linux files entries under the first letter, macOS under its hex code.
  (seq-some (lambda (d) (or (file-exists-p (expand-file-name "x/xterm" d))
                            (file-exists-p (expand-file-name "78/xterm" d))))
            '("/usr/share/terminfo" "/lib/terminfo" "/etc/terminfo")))

(defun workers-fatal-tests--escape-answers ()
  "The `:answers' of a tty child taken through SIGINT's emergency escape.
C-g twice, then \"y\" to \"Auto-save?\" and to \"Abort (and dump core)?\".
On macOS one more newline after \"Auto-save done\": there a C-g that
flushes the tty also raises SIGIO (XNU's ttyinput -> ttyflush ->
ttwakeup), so `pending_signals' is set when the escape starts.  The
escape's auto-save clears `quit-flag' (src/fileio.c:6401), and its
first `maybe_quit' (find-file-name-handler, src/fileio.c:431, from
src/fileio.c:6411) then processes the pending signals
(src/eval.c:1900-1901): `gobble_input' reads the newline of \"y\\n\"
into the keyboard buffer, and the escape's loop waiting for that
newline (src/keyboard.c:12725-12726) waits for another one, as it
would for a user at the terminal."
  `(("nh-wk: holding" . "\C-g")
    ("nh-wk: quit" . "\C-g")
    (,(regexp-quote "Auto-save? (y or n)") . "y\n")
    ,@(and (eq system-type 'darwin) '(("Auto-save done" . "\n")))
    (,(regexp-quote "Abort (and dump core)? (y or n)") . "y\n")))

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
               :answers (workers-fatal-tests--escape-answers))))
      (ert-info ((format "%S" (plist-put (copy-sequence r) :out nil))
                 :prefix "child: ")
        (should (string-search "Auto-save done" (plist-get r :out)))
        (workers-fatal-tests--should-die r '(SIGABRT . 6) "Aborted")
        (should (equal (workers-fatal-tests--auto-saved dir "b.txt")
                       "emergency escape")))))
  ;; Task 34: an auto-save job on nh-save, parked at before-write, when
  ;; the main thread dies holding nh-save's mutex.  The counters already
  ;; describe the job's snapshot; the emergency auto-save still writes
  ;; the buffer, to the real name, and no temporary file is left.
  (workers-fatal-tests--in-dir dir
    (let* ((file (expand-file-name "e.txt" dir))
           (r (workers-fatal-tests--batch
               `(progn
                  ,(workers-fatal-tests--edit file "nh-save under lock")
                  (setq nidhogg-workers-autosave t)
                  (nidhogg-workers--pause-at 'before-write)
                  (nidhogg-workers--test-keyboard-auto-save t)
                  (unless (nidhogg-workers--test-await-paused 'before-write 5000)
                    (kill-emacs 3))
                  (princ "nh-wk: ready\n")
                  (nidhogg-workers--test-fatal-under-lock 'nh-save))
               :pty t :mark "nh-wk: ready")))
      (ert-info ((format "%S" r) :prefix "child: ")
        (workers-fatal-tests--should-die r '(SIGSEGV . 11) "Segmentation fault")
        (should (equal (workers-fatal-tests--auto-saved dir "e.txt")
                       "nh-save under lock"))
        (should-not (directory-files dir nil "\\.nh-")))))
  ;; Task 47 review: the same with nh-save's arena mutex held too.  The
  ;; emergency auto-save of a buffer that has an arena frees nothing on
  ;; the fatal path (it took the mutex and deadlocked).
  (workers-fatal-tests--in-dir dir
    (let* ((file (expand-file-name "g.txt" dir))
           (r (workers-fatal-tests--batch
               `(progn
                  ,(workers-fatal-tests--edit file "arena under lock")
                  (setq nidhogg-workers-autosave t)
                  ;; A first save leaves the buffer an arena.
                  (nidhogg-workers--test-keyboard-auto-save t)
                  (unless (nidhogg-workers--test-save-await 5000)
                    (kill-emacs 3))
                  (insert " more")
                  (nidhogg-workers--pause-at 'before-write)
                  (nidhogg-workers--test-keyboard-auto-save t)
                  (unless (nidhogg-workers--test-await-paused 'before-write 5000)
                    (kill-emacs 3))
                  (princ "nh-wk: ready\n")
                  (nidhogg-workers--test-fatal-under-lock 'nh-save 'arena))
               :pty t :mark "nh-wk: ready")))
      (ert-info ((format "%S" r) :prefix "child: ")
        (workers-fatal-tests--should-die r '(SIGSEGV . 11) "Segmentation fault")
        (should (equal (workers-fatal-tests--auto-saved dir "g.txt")
                       "arena under lock more")))))
  ;; Review S15: the SIGINT escape arrives while a keyboard-triggered
  ;; auto-save holds its flag (its auto-save-hook holds nh-save's mutex,
  ;; with an earlier job parked).  The escape's auto-save runs in signal
  ;; context: it queues nothing, takes no worker lock and writes the
  ;; buffer's current text to the real name.
  (workers-fatal-tests--in-dir dir
    (let* ((file (expand-file-name "f.txt" dir))
           (r (workers-fatal-tests--run
               (list "-nw" "-Q" "--eval"
                     (prin1-to-string
                      `(progn
                         (setq nidhogg-workers--mode 'threads
                               nidhogg-workers-autosave t)
                         ,(workers-fatal-tests--edit file "escape in auto-save")
                         (nidhogg-workers--pause-at 'before-write)
                         (nidhogg-workers--test-keyboard-auto-save t)
                         (insert " more")
                         (run-at-time
                          0 nil
                          (lambda ()
                            (if (not (nidhogg-workers--test-await-paused
                                      'before-write 5000))
                                (kill-emacs 3)
                              (setq auto-save-hook
                                    (list
                                     (lambda ()
                                       (setq auto-save-hook nil)
                                       (nidhogg-workers--test-fatal-under-lock
                                        'nh-save 'sigint))))
                              (nidhogg-workers--test-keyboard-auto-save t)))))))
               :pty t :env '("TERM=xterm")
               :mark "nh-wk: holding"
               :answers (workers-fatal-tests--escape-answers))))
      (ert-info ((format "%S" (plist-put (copy-sequence r) :out nil))
                 :prefix "child: ")
        (should (string-search "Auto-save done" (plist-get r :out)))
        (workers-fatal-tests--should-die r '(SIGABRT . 6) "Aborted")
        (should (equal (workers-fatal-tests--auto-saved dir "f.txt")
                       "escape in auto-save more"))))))

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

(defconst workers-fatal-tests--crash-grammar-dir
  (let ((dir (or (getenv "NIDHOGG_TEST_GRAMMARS")
                 (expand-file-name
                  "fixtures/grammars-cache"
                  (file-name-directory (or load-file-name buffer-file-name ""))))))
    (and (file-exists-p (expand-file-name "libtree-sitter-crash.so" dir))
         dir))
  "The directory holding the `crash' test grammar, or nil.
fixtures/grammars.sh builds it; found as `workers-ts-tests' finds its grammars.")

(ert-deftest workers/worker-crash-forwarded-grammar ()
  ;; §17.3 S10.  A grammar's external scanner dereferences NULL on a
  ;; marker input (the `crash' grammar, fixtures/crash-scanner.c: Python
  ;; whose scanner writes through a null pointer at a backquote) while
  ;; nh-parse parses: the worker's SIGSEGV reaches the main thread's
  ;; fatal path, and the emergency auto-save exists.  The main thread
  ;; never parses here: the job is queued, then the child only waits.
  (let ((grammars workers-fatal-tests--crash-grammar-dir))
    (skip-unless grammars)
    (workers-fatal-tests--in-dir dir
      (let* ((file (expand-file-name "g.txt" dir))
             (text "x=1\ny=`\nz=(`)\n`\n")
             (r (workers-fatal-tests--batch
                 `(progn
                    ,(workers-fatal-tests--edit file text)
                    (require 'treesit)
                    (setq treesit-extra-load-path '(,grammars)
                          nidhogg-workers-treesit t)
                    (unless (treesit-language-available-p 'crash)
                      (kill-emacs 3))
                    (let ((p (treesit-parser-create 'crash)))
                      (princ "nh-wk: ready\n")
                      (unless (nidhogg-workers--test-ts-start p)
                        (kill-emacs 4)))
                    (while t
                      (accept-process-output nil 0.05)))
                 :pty t :mark "nh-wk: ready")))
        (ert-info ((format "%S" r) :prefix "child: ")
          (workers-fatal-tests--should-die r '(SIGSEGV . 11) "Segmentation fault")
          (should (equal (workers-fatal-tests--auto-saved dir "g.txt")
                         text)))))))

(ert-deftest workers/kill-emacs-with-jobs-in-flight ()
  ;; Review Focus 4.  Every worker has a job parked at a pause point when
  ;; a timer calls `kill-emacs': the exit does not hang (status 0 within
  ;; 2 s), no temporary auto-save file is left, and the auto-save file
  ;; holds the buffer's text.  Tasks 29 and 34 add their workers' jobs:
  ;; Task 34's is a real auto-save job, queued behind nh-save's parked
  ;; test job, so the counters say the buffer is auto-saved while
  ;; nothing is on disk; the exit's auto-save writes it all the same.
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
                  ;; Task 29: an image decode job, parked before it decodes.
                  (when (fboundp 'nidhogg-workers--test-image-job)
                    (nidhogg-workers--pause-at 'after-header)
                    (nidhogg-workers--test-image-job
                     '(image :type png :data "\211PNG\r\n\032\n") 1)
                    (unless (nidhogg-workers--test-await-paused
                             'after-header 5000 1)
                      (kill-emacs 4)))
                  (setq nidhogg-workers-autosave t)
                  (nidhogg-workers--test-keyboard-auto-save t)
                  (unless (equal (mapcar #'cadr (nidhogg-workers--test-save-jobs))
                                 '(queued))
                    (kill-emacs 5))
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

;;; The reader on the fatal path (Task 16).  Both rows need Task 4's
;;; fatal path (the workers-dead flag set first in shut_down_emacs, the
;;; crash forwarding that parks the faulting worker), whose primitive
;;; `nidhogg-workers--test-fatal-under-lock' marks it.

(defun workers-fatal-tests--reader-child (crash-form)
  "Run a child Nidhogg with live reader-owned pipes and a socket, then CRASH-FORM.
The child visits a file with unsaved text first.  Return (STATUS
AUTO-SAVED): how the child ended and whether its emergency auto-save
holds the text.  Waits 20 s at most for the child."
  (let* ((dir (make-temp-file "wfatal-" t))
         (file (expand-file-name "unsaved.txt" dir))
         (form
          `(progn
             (setq nidhogg-workers--mode 'threads
                   nidhogg-workers-process-reader t
                   nidhogg-workers-reader-pipes t
                   nidhogg-workers-reader-sockets t)
             (find-file ,file)
             (insert "the unsaved text\n")
             (auto-save-mode 1)
             (dotimes (i 3)
               (make-process :name (format "cat-%d" i) :command '("cat")
                             :connection-type 'pipe :noquery t))
             ;; eglot-style: a `:stderr' pipe process beside stdout.
             (dotimes (i 2)
               (make-process :name (format "eg-%d" i) :command '("cat")
                             :connection-type 'pipe :noquery t
                             :stderr (make-pipe-process
                                      :name (format "eg-%d-stderr" i)
                                      :noquery t)))
             (let* ((server (make-network-process :name "srv" :server t
                                                  :host 'local :service t
                                                  :family 'ipv4 :noquery t))
                    (client (make-network-process
                             :name "client" :host "127.0.0.1" :noquery t
                             :service (process-contact server :service))))
               (unless (plist-get (nidhogg-workers--test-io-info client) :owned)
                 (kill-emacs 3)))
             ,crash-form
             (sleep-for 30)
             (kill-emacs 4)))
         (child (make-process
                 :name "wfatal-child" :noquery t :buffer nil
                 :command (list (expand-file-name invocation-name
                                                  invocation-directory)
                                "-Q" "--batch" "--eval" (prin1-to-string form)))))
    (unwind-protect
        (let ((i 0))
          (while (and (process-live-p child) (< i 1000))
            (accept-process-output child 0.02)
            (setq i (1+ i)))
          (list (process-status child)
                (let ((auto (expand-file-name "#unsaved.txt#" dir)))
                  (and (file-exists-p auto)
                       (with-temp-buffer
                         (insert-file-contents auto)
                         (string-search "the unsaved text" (buffer-string)))
                       t))))
      (when (process-live-p child)
        (delete-process child))
      (delete-directory dir t))))

(ert-deftest workers/worker-crash-forwarded-live-pipes ()
  ;; nh-io dereferences NULL while five pipe processes (two with
  ;; `:stderr' pipes) and a socket are reader-owned: the crash reaches
  ;; the main thread's fatal path, which closes every reader-owned fd
  ;; without a handshake, and the emergency auto-save is written
  ;; (§17.3).
  (skip-unless (fboundp 'nidhogg-workers--test-fatal-under-lock))
  (should (equal (workers-fatal-tests--reader-child
                  '(nidhogg-workers--test-io-crash))
                 '(signal t))))

(ert-deftest workers/fatal-hups-reader-owned-children ()
  ;; A reader-owned subprocess (pipes) gets its SIGHUP on the fatal path,
  ;; as upstream's kill_buffer_processes sends it: the reader's fds close
  ;; after the signal, not before it.
  (skip-unless (fboundp 'nidhogg-workers--test-fatal-under-lock))
  (let* ((dir (make-temp-file "wfatal-hup-" t))
         (pidfile (expand-file-name "pid" dir))
         (form
          `(progn
             (setq nidhogg-workers--mode 'threads
                   nidhogg-workers-process-reader t
                   nidhogg-workers-reader-pipes t)
             (let ((p (make-process
                       :name "sleeper" :connection-type 'pipe :noquery t
                       :command (list "sh" "-c"
                                      ,(format "echo $$ > %s; exec sleep 300"
                                               (shell-quote-argument pidfile))))))
               (while (not (file-exists-p ,pidfile))
                 (accept-process-output nil 0.02))
               (unless (plist-get (nidhogg-workers--test-io-info p) :owned)
                 (kill-emacs 3)))
             (nidhogg-workers--test-io-crash)
             (sleep-for 30)
             (kill-emacs 4)))
         (child (make-process
                 :name "wfatal-hup-child" :noquery t :buffer nil
                 :command (list (expand-file-name invocation-name
                                                  invocation-directory)
                                "-Q" "--batch" "--eval" (prin1-to-string form)))))
    (unwind-protect
        (let ((i 0) pid)
          (while (and (process-live-p child) (< i 1000))
            (accept-process-output child 0.02)
            (setq i (1+ i)))
          (should (eq (process-status child) 'signal))
          (setq pid (with-temp-buffer
                      (insert-file-contents pidfile)
                      (string-to-number (buffer-string))))
          (should (> pid 0))
          (setq i 0)
          (while (and (process-attributes pid) (< i 100))
            (sleep-for 0.02)
            (setq i (1+ i)))
          (prog1 (should-not (process-attributes pid))
            (ignore-errors (signal-process pid 'SIGKILL))))
      (when (process-live-p child)
        (delete-process child))
      (delete-directory dir t))))

(ert-deftest workers/fatal-under-io-lock ()
  ;; The main thread holds nh-io's queue mutex with reader-owned pipes
  ;; alive when SIGSEGV arrives: the fatal path takes no reader lock and
  ;; does no hand-back handshake, and the auto-save is written (§17.3 B4,
  ;; review M5).
  (skip-unless (fboundp 'nidhogg-workers--test-fatal-under-lock))
  (should (equal (workers-fatal-tests--reader-child
                  '(progn (nidhogg-workers--test-io-hold-lock t)
                          (signal-process (emacs-pid) 'SIGSEGV)))
                 '(signal t))))

;;; workers-fatal-tests.el ends here
