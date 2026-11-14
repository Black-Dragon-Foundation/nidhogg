;;; workers-fatal-tests.el --- tests for workers: fatal paths  -*- lexical-binding: t -*-
;; test-class: cond
;; test-pattern: handshake

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
;; thread to crash or to hold a job in flight.  One row is about a Lisp
;; thread's select, which a close under it made abort the child.

;;; Code:

(require 'ert)
(require 'seq)

;; These children crash on purpose.  A core_pattern that pipes to a
;; handler ignores a limit of 0: the process stays alive while the kernel
;; writes it all out, seconds under load, which reads as a hung fatal
;; path.  One byte is the limit the kernel takes as no core for a pipe,
;; and `ulimit' cannot say it: it counts blocks.
(defconst workers-fatal-tests--no-core
  "if command -v prlimit >/dev/null; then p='prlimit --core=1'; else p=; ulimit -c 0; fi; "
  "Shell code that keeps a child's crash from dumping core: `exec $p'.")

(defun workers-fatal-tests--child (&rest args)
  "A command that runs this Nidhogg with ARGS, and no core dump."
  (append (list "/bin/sh" "-c"
                (concat workers-fatal-tests--no-core "exec $p \"$@\"")
                "sh" (expand-file-name invocation-name invocation-directory))
          args))

(defun workers-fatal-tests--note-end (p _event)
  "A sentinel: P's end (it exited or a signal ended it), as its `ended' property."
  (when (memq (process-status p) '(exit signal))
    (process-put p 'ended t)))

(defun workers-fatal-tests--wait-end (p polls)
  "Wait until P's sentinel says it ended, at most POLLS waits of 20 ms for its output; whether it did."
  (let ((i 0))
    (while (and (not (process-get p 'ended)) (< i polls))
      (accept-process-output p 0.02)
      (setq i (1+ i))))
  (process-get p 'ended))

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
                :sentinel #'workers-fatal-tests--note-end
                :command (append
                          (list "/bin/sh" "-c"
                                (concat workers-fatal-tests--no-core
                                        "err=$1; shift; exec $p \"$@\" 2>\"$err\"")
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
    (while (and (not (process-get proc 'ended)) (< (- (float-time) start) 30))
      (accept-process-output proc 0.05))
    (let ((end (float-time)))
      (unless (process-get proc 'ended)
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

(defun workers-fatal-tests--exit-3 (why worker point &rest forms)
  "A form that exits the child with code 3, saying WHY on stderr.
For a row whose job never got where it waits for it: with WHY go the
values of FORMS, WORKER's counters from `nidhogg-workers-stats' (its
fallbacks among them) and how many workers are parked at POINT."
  `(progn
     (princ (format "nh-wk: exit 3: %s: %S\n" ,why
                    (list ,@(mapcar (lambda (f) `(cons ',f ,f)) forms)
                          (cons ',worker
                                (plist-get (nidhogg-workers-stats)
                                           ,(intern (format ":%s" worker))))
                          (cons 'parked
                                (nidhogg-workers--test-await-paused
                                 ',point 0 0))))
            #'external-debugging-output)
     (kill-emacs 3)))

(defun workers-fatal-tests--auto-saved (dir name)
  "The text of the auto-save file of DIR/NAME, or nil."
  (let ((f (expand-file-name (concat "#" name "#") dir)))
    (and (file-exists-p f)
         (with-temp-buffer
           (insert-file-contents f)
           (buffer-string)))))

(defun workers-fatal-tests--should-die (r sig name)
  "R (from `workers-fatal-tests--run') died of signal SIG through the fatal path."
  ;; The same on GNU/Linux and macOS (Naglfar's shutdown does not abort,
  ;; as the NS port's did): the child dies of SIG with a Backtrace.
  (should (equal (list (plist-get r :status) (plist-get r :code))
                 (list 'signal (cdr sig))))
  ;; The fatal path ended the child before the helper's 30 s kill: a
  ;; kill would show another exit code.
  (should (plist-get r :secs))
  (let ((err (plist-get r :err)))
    (should (string-match-p (format "^Fatal error %d: %s" (cdr sig) name)
                            err))
    (should (string-match-p "^Backtrace:" err))))

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
                  (let ((h (nidhogg-workers--test-job 'nh-blk 0 :pause 'before-done)))
                    (unless (nidhogg-workers--test-await-paused 'before-done 5000)
                      ,(workers-fatal-tests--exit-3
                        "not parked" 'nh-blk 'before-done
                        'h '(and h (nidhogg-workers--test-job-info h)))))
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
                         (let ((h (nidhogg-workers--test-job 'nh-blk 0 :pause 'before-done)))
                           (run-at-time
                            0 nil
                            (lambda ()
                              (if (nidhogg-workers--test-await-paused 'before-done 5000)
                                  (nidhogg-workers--test-fatal-under-lock 'nh-blk 'sigint)
                                ,(workers-fatal-tests--exit-3
                                  "not parked" 'nh-blk 'before-done
                                  'h '(and h (nidhogg-workers--test-job-info h))))))))))
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
                    ,(workers-fatal-tests--exit-3
                      "not parked" 'nh-save 'before-write
                      '(nidhogg-workers--test-save-jobs)))
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
                    ,(workers-fatal-tests--exit-3
                      "first save not landed" 'nh-save 'before-write
                      '(nidhogg-workers--test-save-jobs)))
                  (insert " more")
                  (nidhogg-workers--pause-at 'before-write)
                  (nidhogg-workers--test-keyboard-auto-save t)
                  (unless (nidhogg-workers--test-await-paused 'before-write 5000)
                    ,(workers-fatal-tests--exit-3
                      "not parked" 'nh-save 'before-write
                      '(nidhogg-workers--test-save-jobs)))
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
                                ,(workers-fatal-tests--exit-3
                                  "not parked" 'nh-save 'before-write
                                  '(nidhogg-workers--test-save-jobs))
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

(ert-deftest workers/worker-crash-waits-for-a-safe-point ()
  ;; The forwarded crash lands on a main thread far from a safe point,
  ;; as inside malloc, where the fatal path, which allocates, would block
  ;; for good on a lock its own thread holds.  So the handler only asks
  ;; for the next safe point and returns, every time: the primitive's
  ;; "deferred" comes before the fatal path's line, which then names the
  ;; worker's signal, and the emergency auto-save exists.  A handler that
  ;; runs the fatal path itself ends the child before "deferred".
  (workers-fatal-tests--in-dir dir
    (let* ((file (expand-file-name "s.txt" dir))
           (r (workers-fatal-tests--batch
               `(progn
                  ,(workers-fatal-tests--edit file "safe point")
                  (princ "nh-wk: ready\n")
                  (unless (nidhogg-workers--test-crash-away-from-safe-points
                           'nh-blk 10000)
                    (kill-emacs 3))
                  (while t
                    (accept-process-output nil 0.05)))
               :pty t :mark "nh-wk: ready")))
      (ert-info ((format "%S" r) :prefix "child: ")
        (workers-fatal-tests--should-die r '(SIGSEGV . 11) "Segmentation fault")
        (should (string-match-p "^nh-wk: deferred\n\\(?:.*\n\\)*Fatal error 11"
                                (plist-get r :err)))
        (should (equal (workers-fatal-tests--auto-saved dir "s.txt")
                       "safe point"))))))

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
             (while t (accept-process-output nil 1))))
         (child (make-process
                 :name "wfatal-child" :noquery t :buffer nil
                 :sentinel #'workers-fatal-tests--note-end
                 :command (workers-fatal-tests--child
                           "-Q" "--batch" "--eval" (prin1-to-string form)))))
    (unwind-protect
        (progn
          (workers-fatal-tests--wait-end child 1000)
          (list (process-status child)
                (let ((auto (expand-file-name "#unsaved.txt#" dir)))
                  (and (file-exists-p auto)
                       (with-temp-buffer
                         (insert-file-contents auto)
                         (string-search "the unsaved text" (buffer-string)))
                       t))))
      (unless (process-get child 'ended)
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

(ert-deftest workers/worker-crash-while-main-allocates ()
  ;; A worker's crash lands on a main thread inside malloc: the fatal
  ;; path allocates for the emergency auto-save, so running it there
  ;; blocks on a lock its own thread holds, for good.  Here the main
  ;; thread allocates as hard as it can (large strings go to malloc).
  (skip-unless (fboundp 'nidhogg-workers--test-fatal-under-lock))
  (dotimes (_ 5)
    (should (equal (workers-fatal-tests--reader-child
                    '(progn (nidhogg-workers--test-io-crash)
                            (while t (make-string 5000 ?x))))
                   '(signal t)))))

(ert-deftest workers/worker-crash-main-in-a-c-wait ()
  ;; A main thread that reaches no safe point still goes down, within
  ;; the worker's 1 s: here it sits in a bounded condition wait, which
  ;; runs no pending signals.
  (skip-unless (fboundp 'nidhogg-workers--test-fatal-under-lock))
  (should (equal (workers-fatal-tests--reader-child
                  '(progn (nidhogg-workers--test-io-crash)
                          (nidhogg-workers--test-await-paused 'after-read 15000)
                          (kill-emacs 5)))
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
             ;; The pid file appears whole, by a rename: a redirection
             ;; creates it empty before `echo' writes, and a crash in
             ;; between hangs up the shell with the file still empty.
             (let ((p (make-process
                       :name "sleeper" :connection-type 'pipe :noquery t
                       :command (list "sh" "-c"
                                      ,(format "echo $$ > %s.new && mv %s.new %s; exec sleep 300"
                                               (shell-quote-argument pidfile)
                                               (shell-quote-argument pidfile)
                                               (shell-quote-argument pidfile))))))
               (while (not (file-exists-p ,pidfile))
                 (accept-process-output nil 0.02))
               (unless (plist-get (nidhogg-workers--test-io-info p) :owned)
                 (kill-emacs 3)))
             (nidhogg-workers--test-io-crash)
             (while t (accept-process-output nil 1))))
         (child (make-process
                 :name "wfatal-hup-child" :noquery t :buffer nil
                 :sentinel #'workers-fatal-tests--note-end
                 :command (workers-fatal-tests--child
                           "-Q" "--batch" "--eval" (prin1-to-string form)))))
    (unwind-protect
        (let ((i 0) pid)
          (workers-fatal-tests--wait-end child 1000)
          (should (eq (process-status child) 'signal))
          (setq pid (with-temp-buffer
                      (insert-file-contents pidfile)
                      (string-to-number (buffer-string))))
          (should (> pid 0))
          (setq i 0)
          (while (and (process-attributes pid) (< i 100))
            (accept-process-output nil 0.02)
            (setq i (1+ i)))
          (prog1 (should-not (process-attributes pid))
            (ignore-errors (signal-process pid 'SIGKILL))))
      (unless (process-get child 'ended)
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

(ert-deftest workers/close-waits-for-the-select ()
  ;; A Lisp thread lets go of the global lock before it selects on the
  ;; fds it computed: a process deleted meanwhile on another thread closed
  ;; its fd under the select, which failed with EBADF and aborted the
  ;; child (process-tests/multiple-threads-waiting, about one run in 240).
  ;; Now a close waits until no select holds the fd.  The thread parks at
  ;; before-select with Q's fd in its set while the main thread deletes Q;
  ;; the reader is off, so Q's real fd is the one selected on and closed.
  (let ((r (workers-fatal-tests--batch
            '(progn
               (setq nidhogg-workers-process-reader nil)
               (let* ((p (make-process :name "p" :command '("cat")
                                       :connection-type 'pipe :noquery t))
                      (q (make-process :name "q" :command '("cat")
                                       :connection-type 'pipe :noquery t))
                      (n 0))
                 (set-process-thread p nil)
                 (set-process-thread q nil)
                 (nidhogg-workers--pause-at 'before-select)
                 (let ((th (make-thread
                            (apply-partially #'accept-process-output p 5))))
                   (while (and (not (nidhogg-workers--test-await-paused
                                     'before-select 0))
                               (< (setq n (1+ n)) 100000))
                     (thread-yield))
                   (unless (nidhogg-workers--test-await-paused 'before-select 0)
                     (kill-emacs 3))
                   (delete-process q)
                   (process-send-string p "x\n")
                   (nidhogg-workers--release 'before-select)
                   (thread-join th))
                 (princ "nh-wk: done\n"))))))
    (ert-info ((format "%S" r) :prefix "child: ")
      (should (equal (list (plist-get r :status) (plist-get r :code))
                     '(exit 0)))
      (should (string-match-p "nh-wk: done" (plist-get r :out))))))

(ert-deftest workers/kill-emacs-in-a-select-closes-its-fds ()
  ;; A SIGTERM that comes while the main thread selects runs `kill-emacs'
  ;; in the handler, on the thread whose select holds the fds.  A close
  ;; from `kill-emacs-hook' waited for that select to end, which it never
  ;; does under its own handler: the child hung until it was killed (the
  ;; fake portal's daemon at the end of a GUI lane).  A close never waits
  ;; for its own thread's select.  The reader is off, so P's real fd is
  ;; the one selected on and closed.
  (let ((r (workers-fatal-tests--batch
            '(progn
               (setq nidhogg-workers-process-reader nil)
               (let ((p (make-process :name "p" :command '("cat")
                                      :connection-type 'pipe :noquery t)))
                 (add-hook 'kill-emacs-hook
                           (lambda ()
                             (delete-process p)
                             (princ "nh-wk: hook done\n")))
                 (make-process :name "k" :noquery t :connection-type 'pipe
                               :command (list "sh" "-c"
                                              (format "sleep 0.5; kill -TERM %d"
                                                      (emacs-pid))))
                 (accept-process-output p 20)
                 (princ "nh-wk: not killed\n"))))))
    (ert-info ((format "%S" r) :prefix "child: ")
      (should (equal (list (plist-get r :status) (plist-get r :code))
                     '(exit 15)))
      (should (string-match-p "nh-wk: hook done" (plist-get r :out))))))

;;; workers-fatal-tests.el ends here
