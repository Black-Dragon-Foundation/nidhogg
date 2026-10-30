;;; workers-tests.el --- tests for the worker threads (flag, feature, switches)  -*- lexical-binding: t -*-

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
(require 'loadhist)
(require 'cl-lib)
(require 'seq)

(defconst workers-tests--workers '(nh-io nh-parse nh-img nh-blk nh-save)
  "The workers, by the names their threads carry.")

(defconst workers-tests--count-form
  '(let ((n 0))
     (dolist (d (directory-files "/proc/self/task" nil "\\`[0-9]"))
       (when (member (with-temp-buffer
                       (ignore-errors
                         (insert-file-contents (format "/proc/self/task/%s/comm" d)))
                       (string-trim (buffer-string)))
                     '("nh-io" "nh-parse" "nh-img" "nh-blk" "nh-save"))
         (setq n (1+ n))))
     n)
  "A form that counts this process's worker threads (for children too).")

(defun workers-tests--wd-runs-p ()
  "Non-nil where the watchdog can start: built in and on Linux.
Elsewhere the feature builds but `nidhogg-watchdog--start' signals
until W5 (the C side is `#ifndef __linux__')."
  (and (featurep 'nidhogg-watchdog) (eq system-type 'gnu/linux)))

(defun workers-tests--proc-p ()
  "Non-nil where threads can be counted through /proc (Linux)."
  (file-directory-p "/proc/self/task"))

(defun workers-tests--emacs ()
  "The Emacs binary under test."
  (expand-file-name invocation-name invocation-directory))

(defun workers-tests--client ()
  "The emacsclient of the Emacs under test, or nil.
A build tree has it in ../lib-src; an installed prefix has it next to
the binary, named nidhoggclient."
  (seq-find #'file-executable-p
            (mapcar (lambda (f) (expand-file-name f invocation-directory))
                    '("../lib-src/emacsclient" "nidhoggclient" "emacsclient"))))

(defun workers-tests--child (form &rest args)
  "Run FORM in a child Nidhogg in threads mode; return what it printed, read.
ARGS go before the --eval."
  (with-temp-buffer
    (let ((status (apply #'call-process (workers-tests--emacs) nil '(t nil) nil
                         "-Q" "--batch"
                         (append args
                                 (list "--eval"
                                       (prin1-to-string
                                        `(progn (setq nidhogg-workers--mode 'threads)
                                                (prin1 ,form))))))))
      (should (eql status 0))
      (car (read-from-string (buffer-string))))))

(defun workers-tests--nh-threads ()
  "This process's worker threads, as (TID . COMM)."
  (let (threads)
    (dolist (d (directory-files "/proc/self/task" nil "\\`[0-9]"))
      (let ((comm (with-temp-buffer
                    (ignore-errors
                      (insert-file-contents (format "/proc/self/task/%s/comm" d)))
                    (string-trim (buffer-string)))))
        (when (member comm (mapcar #'symbol-name workers-tests--workers))
          (push (cons d comm) threads))))
    threads))

(defun workers-tests--sigblk (tid)
  "The blocked-signal mask of thread TID, as an integer."
  (with-temp-buffer
    (insert-file-contents (format "/proc/self/task/%s/status" tid))
    (re-search-forward "^SigBlk:\t\\([0-9a-f]+\\)")
    (string-to-number (match-string 1) 16)))

;; Linux's numbers (x86-64 and aarch64 agree on these).
(defconst workers-tests--blocked
  '((SIGINT . 2) (SIGIO . 29) (SIGCHLD . 17) (SIGALRM . 14) (SIGPROF . 27)
    (SIGURG . 23) (SIGUSR1 . 10) (SIGUSR2 . 12) (SIGWINCH . 28)))
(defconst workers-tests--unblocked
  '((SIGSEGV . 11) (SIGBUS . 7) (SIGILL . 4) (SIGFPE . 8) (SIGABRT . 6)
    (SIGTRAP . 5)))

(defmacro workers-tests--modes (&rest body)
  "Run BODY once in each of `nidhogg-workers--mode''s modes."
  (declare (indent 0) (debug t))
  `(dolist (mode '(threads inline))
     (let ((nidhogg-workers--mode mode))
       (ert-info ((format "mode %s" mode) :prefix "")
         (unwind-protect
             (progn ,@body)
           (nidhogg-workers--test-release-on-wait nil)
           (nidhogg-workers--release))))))

(defun workers-tests--settle (pred)
  "Wait in short timer-checking waits until PRED holds (100 at most)."
  (let ((i 0))
    (while (and (not (funcall pred)) (< i 100))
      (accept-process-output nil 0.02)
      (setq i (1+ i)))
    (funcall pred)))

(defun workers-tests--stat (worker key)
  (plist-get (plist-get (nidhogg-workers-stats)
                        (intern (format ":%s" worker)))
             key))

(defun workers-tests--fallbacks (worker reason)
  (or (cdr (assq reason (workers-tests--stat worker :fallbacks))) 0))

(ert-deftest workers/feature ()
  (should (featurep 'nidhogg-workers)))

(ert-deftest workers/in-the-configuration ()
  (should (string-search "--with-nidhogg-workers" system-configuration-options)))

(ert-deftest workers/lisp-preloaded-compiled ()
  ;; The dump loads nidhogg-workers.el compiled, like the rest of the
  ;; preloaded Lisp, not from source.
  (let ((file (feature-file 'nidhogg-workers-lisp)))
    (should file)
    (should-not (string-suffix-p ".el" file))))

(ert-deftest workers/no-threads-when-unused ()
  ;; In a child, since the other rows here start workers: reading every
  ;; switch and the counters starts no thread and submits no job.
  (skip-unless (workers-tests--proc-p))
  (should (equal (workers-tests--child
                  `(let ((tasks (length (directory-files "/proc/self/task" nil "\\`[0-9]")))
                         (stats nil))
                     (dolist (s nidhogg-workers--switches)
                       (symbol-value s))
                     (setq stats (nidhogg-workers-stats))
                     (list ,workers-tests--count-form
                           (= tasks (length (directory-files "/proc/self/task" nil "\\`[0-9]")))
                           (let ((jobs 0))
                             (dolist (w '(:nh-io :nh-parse :nh-img :nh-blk :nh-save))
                               (setq jobs (+ jobs (plist-get (plist-get stats w) :jobs))))
                             jobs))))
                 '(0 t 0))))

(ert-deftest workers/env-off ()
  ;; A child Nidhogg with NIDHOGG_WORKERS=off: every switch is nil.
  (let* ((process-environment (cons "NIDHOGG_WORKERS=off" process-environment))
         (exe (expand-file-name invocation-name invocation-directory))
         (out (with-temp-buffer
                ;; stdout only: sanitizer builds report on stderr.
                (should (zerop (call-process
                                exe nil '(t nil) nil "-Q" "--batch" "--eval"
                                "(princ (mapcar #'symbol-value nidhogg-workers--switches))")))
                (buffer-string))))
    (should (equal (read out) (make-list (length nidhogg-workers--switches) nil)))))

(ert-deftest workers/image-switches-on-by-default ()
  ;; A child Nidhogg with NIDHOGG_WORKERS unset: the image switches are
  ;; on, in the build's defaults too, and tree-sitter's stay off.  The
  ;; parent may itself run with NIDHOGG_WORKERS=on or off (the suite runs
  ;; both ways): the child must not inherit it.
  (skip-unless (workers-tests--proc-p))
  (let ((process-environment
         (seq-remove (lambda (e) (string-match-p "\\`NIDHOGG_WORKERS\\(=\\|\\'\\)" e))
                     process-environment)))
    (should (equal (workers-tests--child
                    '(list nidhogg-workers-images nidhogg-image-defer
                           (cdr (assq 'nidhogg-workers-images nidhogg-workers--defaults))
                           (cdr (assq 'nidhogg-image-defer nidhogg-workers--defaults))
                           nidhogg-workers-treesit nidhogg-treesit-defer))
                   '(t t t t nil nil)))))

(ert-deftest workers/area-files-load ()
  (require 'jsonrpc)
  (require 'treesit)
  (should (featurep 'nidhogg-workers-jsonrpc))
  (should (featurep 'nidhogg-workers-treesit)))

;;;; The runtime (Task 3)

;; Jobs here are `nidhogg-workers--test-job's: a job for any worker that
;; records the thread it ran on, parks at its :pause point while a test
;; arms it, and pushes its :id onto `nidhogg-workers--test-finished' when
;; it lands.  Real-thread rows order themselves through pause points,
;; never sleeps.

(ert-deftest workers/thread-signals ()
  ;; Every worker thread blocks the asynchronous signals, leaves the
  ;; synchronous fatal ones to the crash forwarding, and carries its
  ;; worker's name.
  (skip-unless (file-directory-p "/proc/self/task"))
  (let ((nidhogg-workers--mode 'threads) (cat nil))
    (unwind-protect
        (progn
          (dolist (w workers-tests--workers)
            (if (eq w 'nh-io)
                ;; Once a process is reader-owned (pipes are by default),
                ;; nh-io runs the reader's loop and a test job queued
                ;; behind it never runs: a reader-owned `cat' starts it.
                (let ((nidhogg-workers-process-reader t)
                      (nidhogg-workers-reader-pipes t))
                  (setq cat (make-process :name "workers-sig-cat" :command '("cat")
                                          :connection-type 'pipe :noquery t))
                  (should (plist-get (nidhogg-workers--test-io-info cat) :owned)))
              (let ((id (nidhogg-workers--test-job w 0)))
                (should id)
                (should (eql (nidhogg-workers--test-wait id 5000) 1))
                (should (equal (plist-get (nidhogg-workers--test-job-info id) :comm)
                               (symbol-name w))))))
          (let ((threads (workers-tests--nh-threads)))
            (should (equal (sort (delete-dups (mapcar #'cdr threads)) #'string<)
                           (sort (mapcar #'symbol-name workers-tests--workers) #'string<)))
            (dolist (th threads)
              (let ((mask (workers-tests--sigblk (car th)))
                    wrong)
                (dolist (s workers-tests--blocked)
                  (when (zerop (logand mask (ash 1 (1- (cdr s)))))
                    (push (car s) wrong)))
                (dolist (s workers-tests--unblocked)
                  (unless (zerop (logand mask (ash 1 (1- (cdr s)))))
                    (push (car s) wrong)))
                (should (equal (list th wrong) (list th nil)))))))
      (when cat (delete-process cat)))))

(ert-deftest workers/lazy-start ()
  ;; temacs refuses a submit while dumping (will_dump_p), but has no row
  ;; here: a temacs born to dump runs only loadup.el (sort_args puts
  ;; --temacs before any -l FILE).
  ;; A fresh Nidhogg has no worker thread until its first job.
  (skip-unless (workers-tests--proc-p))
  (should (equal (workers-tests--child
                  `(list ,workers-tests--count-form
                         (nidhogg-workers--test-wait
                          (nidhogg-workers--test-job 'nh-blk 0 :class 'file) 5000)
                         ,workers-tests--count-form))
                 '(0 1 1)))
  ;; A background daemon: the threads start in the daemon, after its
  ;; fork, at the first job.
  (let* ((client (workers-tests--client))
         (dir (make-temp-file "workers-daemon" t))
         (sock (expand-file-name "s" dir)))
    (should (file-executable-p client))
    (unwind-protect
        (progn
          (cl-flet ((ask (form)
                      (with-temp-buffer
                        (should (eql (call-process client nil '(t nil) nil "-s" sock
                                                   "--eval" (prin1-to-string form))
                                     0))
                        (car (read-from-string (buffer-string))))))
            (should (eql (call-process (workers-tests--emacs) nil nil nil
                                       "-Q"
                                       ;; The emacsclient connection is a socket the
                                       ;; reader would own, starting nh-io: this row
                                       ;; is about the first job.
                                       "--eval" "(setq nidhogg-workers-reader-sockets nil)"
                                       (concat "--bg-daemon=" sock))
                         0))
            (should (eql (ask workers-tests--count-form) 0))
            (should (eql (ask '(progn
                                 (setq nidhogg-workers--mode 'threads)
                                 (nidhogg-workers--test-wait
                                  (nidhogg-workers--test-job 'nh-blk 0 :class 'file)
                                  5000)))
                         1))
            (should (eql (ask workers-tests--count-form) 1))))
      (ignore-errors
        (call-process client nil nil nil "-s" sock "--eval" "(kill-emacs)"))
      (delete-directory dir t))))

(ert-deftest workers/wake-protocol ()
  ;; The completion channel: the callback clears the wake fd, then
  ;; re-checks the queue.  10,000 races put a producer's append exactly
  ;; between the two (a hook in the callback), then 10,000 free-running
  ;; completions from two producers: no wake-up is lost.
  (should (equal (nidhogg-workers--test-wake-race 10000)
                 '(:level t :lost 0 :stress-lost 0))))

(ert-deftest workers/completion-order ()
  ;; nh-blk jobs that finish 3, 1, 2 land 3, 1, 2, from a timer check,
  ;; never inside a filter.  The second batch lands while
  ;; `accept-process-output' waits with an integer JUST-THIS-ONE, which
  ;; runs no timer check, so both batches are queued before one runs.
  ;; The jobs' ids are negative: jobs of earlier rows (positive ids) may
  ;; still be landing.
  (workers-tests--modes
    (setq nidhogg-workers--test-finished nil)
    (let* ((threads (eq mode 'threads))
           (proc (make-process :name "workers-cat" :command '("cat")
                               :connection-type 'pipe :noquery t))
           (ours (lambda () (reverse (seq-filter #'cl-minusp
                                                 nidhogg-workers--test-finished))))
           filter-saw j1 j2 j3)
      ;; Today's path for the cat, for good (`set-process-filter' below
      ;; would hand it over again): the row arms `after-read' and
      ;; `before-decode', where nh-io would park reading it.
      (nidhogg-set-process-reader proc nil)
      (unwind-protect
          (progn
            (if (not threads)
                (setq j3 (nidhogg-workers--test-job 'nh-blk 0 :class 'dns :id -3)
                      j1 (nidhogg-workers--test-job 'nh-blk 0 :class 'dir :id -1))
              (dolist (p '(after-read before-decode in-lookup))
                (nidhogg-workers--pause-at p))
              (setq j1 (nidhogg-workers--test-job 'nh-blk 0 :class 'dir
                                                  :pause 'after-read :id -1)
                    j2 (nidhogg-workers--test-job 'nh-blk 0 :class 'file
                                                  :pause 'before-decode :id -2)
                    j3 (nidhogg-workers--test-job 'nh-blk 0 :class 'dns
                                                  :pause 'in-lookup :id -3))
              (should (nidhogg-workers--test-await-paused 'after-read 5000))
              (should (nidhogg-workers--test-await-paused 'before-decode 5000))
              (should (nidhogg-workers--test-await-paused 'in-lookup 5000))
              (nidhogg-workers--release 'in-lookup)
              (should (eql (nidhogg-workers--test-wait j3 5000) 1))
              (nidhogg-workers--release 'after-read)
              (should (eql (nidhogg-workers--test-wait j1 5000) 1)))
            (should (and j1 j3))
            (should-not (funcall ours))
            (set-process-filter
             proc
             (lambda (_p _s)
               (let ((before (funcall ours))
                     (landed (if threads
                                 (progn (nidhogg-workers--release 'before-decode)
                                        (nidhogg-workers--test-wait j2 5000))
                               (setq j2 (nidhogg-workers--test-job
                                         'nh-blk 0 :class 'file :id -2))
                               (nidhogg-workers--test-wait j2 5000))))
                 (setq filter-saw (list before landed (funcall ours))))))
            (process-send-string proc "x\n")
            (should (accept-process-output proc 5 nil 1))
            (should (equal filter-saw '(nil 1 nil)))
            (should-not (funcall ours))
            (should (workers-tests--settle (lambda () (= (length (funcall ours)) 3))))
            (should (equal (funcall ours) '(-3 -1 -2))))
        (delete-process proc)))))

(ert-deftest workers/priority ()
  ;; PD-15, unprivileged: speculative jobs run at SCHED_IDLE; a wait on
  ;; one not yet started moves it to the worker's normal-priority
  ;; thread; a wait on one running at idle priority tries the boost,
  ;; counts `boost-denied' and returns within its bound.
  (let ((nidhogg-workers--mode 'threads))
    (unwind-protect
        (let (a b)
          (nidhogg-workers--pause-at 'after-snapshot)
          (setq a (nidhogg-workers--test-job 'nh-parse 0 :speculative t
                                             :pause 'after-snapshot))
          (should (nidhogg-workers--test-await-paused 'after-snapshot 5000))
          (should (eq (plist-get (nidhogg-workers--test-job-info a) :policy) 'idle))
          (should (equal (plist-get (nidhogg-workers--test-job-info a) :comm) "nh-parse"))
          (setq b (nidhogg-workers--test-job 'nh-parse 0 :speculative t))
          (should (eq (plist-get (nidhogg-workers--test-job-info b) :state) 'queued))
          (should (eql (nidhogg-workers--test-wait b 5000) 1))
          (should (eq (plist-get (nidhogg-workers--test-job-info b) :policy) 'other))
          (should (equal (plist-get (nidhogg-workers--test-job-info b) :comm) "nh-parse"))
          (let ((denied (workers-tests--fallbacks 'nh-parse 'boost-denied)))
            (should (eql (nidhogg-workers--test-wait a 50) 0))
            ;; macOS boosts by a QoS override, which is not denied.
            (when (eq system-type 'gnu/linux)
              (should (> (workers-tests--fallbacks 'nh-parse 'boost-denied) denied))))
          (nidhogg-workers--release 'after-snapshot)
          (should (eql (nidhogg-workers--test-wait a 5000) 1)))
      (nidhogg-workers--release))))

(ert-deftest workers/blk-class-caps ()
  ;; Two hung directory reads and a hung lookup hold three of nh-blk's
  ;; four threads; a third directory read and a second lookup wait (a
  ;; class takes at most two threads, and never the last thread another
  ;; class with nothing running may need), so a file read still runs.
  (let ((nidhogg-workers--mode 'threads))
    (unwind-protect
        (let (jobs file)
          (nidhogg-workers--pause-at 'after-read)
          (nidhogg-workers--pause-at 'in-lookup)
          (dotimes (_ 3)
            (push (nidhogg-workers--test-job 'nh-blk 0 :class 'dir :pause 'after-read) jobs))
          (dotimes (_ 2)
            (push (nidhogg-workers--test-job 'nh-blk 0 :class 'dns :pause 'in-lookup) jobs))
          (setq file (nidhogg-workers--test-job 'nh-blk 0 :class 'file))
          (should (eql (nidhogg-workers--test-wait file 10000) 1))
          (should (eql (nidhogg-workers--test-await-paused 'after-read 5000 2) 2))
          (should (eql (nidhogg-workers--test-await-paused 'in-lookup 5000 1) 1))
          (should (eql (workers-tests--stat 'nh-blk :queued) 2))
          (should (eql (workers-tests--stat 'nh-blk :running) 3))
          (should (<= (plist-get (nidhogg-workers-stats) :blk-threads) 4))
          (nidhogg-workers--release)
          (dolist (j jobs)
            (should (eql (nidhogg-workers--test-wait j 10000) 1))))
      (nidhogg-workers--release))))

(ert-deftest workers/wait-survives-signals ()
  ;; Review Focus 1: SIGURG and SIGCHLD every millisecond during a wait
  ;; on a paused job.  The wait returns 1, and only after the release
  ;; (which follows the last of the 200 signals).
  (let ((nidhogg-workers--mode 'threads))
    (unwind-protect
        (progn
          (nidhogg-workers--pause-at 'before-done)
          (let ((j (nidhogg-workers--test-job 'nh-blk 0 :class 'file
                                              :pause 'before-done)))
            (should (nidhogg-workers--test-await-paused 'before-done 5000))
            (nidhogg-workers--test-signal-storm 200 'before-done)
            (should (eql (nidhogg-workers--test-wait j 10000) 1))
            (should (eql (nidhogg-workers--test-storm-sent) 200))))
      (nidhogg-workers--release))))

(ert-deftest workers/wait-from-lisp-thread ()
  ;; Review Focus 2: a Lisp thread holding the global lock waits on a
  ;; paused job (released when its wait begins) and gets 1.  With the
  ;; watchdog built in, that wait pushes no phase (PD-3: the phase stack
  ;; is the main thread's).
  (skip-unless (featurep 'threads))
  (workers-tests--modes
    (let ((wd (workers-tests--wd-runs-p))
          j result)
      (if (eq mode 'inline)
          (setq j (nidhogg-workers--test-job 'nh-blk 0 :class 'file))
        (nidhogg-workers--pause-at 'before-done)
        (setq j (nidhogg-workers--test-job 'nh-blk 0 :class 'file :pause 'before-done))
        (should (nidhogg-workers--test-await-paused 'before-done 5000))
        (nidhogg-workers--test-release-on-wait 'before-done))
      (when wd
        (nidhogg-watchdog--start 100 2000 nil :no-monitor)
        (set 'nidhogg-watchdog--test-last-sample nil)
        (set 'nidhogg-watchdog--test-sample-on-phase 'worker-wait))
      (unwind-protect
          (setq result (thread-join
                        (make-thread (lambda () (nidhogg-workers--test-wait j 10000))
                                     "workers-waiter")))
        (when wd
          (set 'nidhogg-watchdog--test-sample-on-phase nil)
          (nidhogg-watchdog--stop)))
      (should (eql result 1))
      (when wd
        (should-not (symbol-value 'nidhogg-watchdog--test-last-sample))
        (should (eq (funcall 'nidhogg-watchdog--test-phase) 'lisp))))))

(ert-deftest workers/fd-budget-runtime ()
  ;; With the descriptors filled to within 64 of FD_SETSIZE, a wake fd is
  ;; refused and the first job falls back (`fd-budget'); freed, it runs.
  (should (equal (workers-tests--child
                  '(progn
                     (nidhogg-workers--test-fd-reserve 64)
                     (list (nidhogg-workers--test-wake-open)
                           (nidhogg-workers--test-job 'nh-blk 0 :class 'file)
                           (cdr (assq 'fd-budget
                                      (plist-get (plist-get (nidhogg-workers-stats) :nh-blk)
                                                 :fallbacks)))
                           (progn
                             (nidhogg-workers--test-fd-reserve nil)
                             (nidhogg-workers--test-wake-open))
                           (nidhogg-workers--test-wait
                            (nidhogg-workers--test-job 'nh-blk 0 :class 'file) 5000))))
                 '(nil nil 1 t 1))))

;;;; Watchdog phases and the redisplay wait budget (Task 5)

(defun workers-tests--wd-wait (phase reason)
  "On the main thread, wait on a paused job (released as the wait begins).
Return the wait's value.  Threads mode only: inline jobs are done before
the wait begins, and a finished job pushes no phase."
  (let ((nidhogg-workers--mode 'threads))
    (nidhogg-workers--pause-at 'before-done)
    (let ((j (nidhogg-workers--test-job 'nh-blk 0 :class 'file
                                        :pause 'before-done)))
      (should (nidhogg-workers--test-await-paused 'before-done 5000))
      (nidhogg-workers--test-release-on-wait 'before-done)
      (nidhogg-workers--test-wait j 10000 phase reason))))

(defmacro workers-tests--wd (&rest body)
  "Run BODY with the watchdog started without a monitor thread."
  (declare (indent 0) (debug t))
  `(progn
     (set 'nidhogg-watchdog--test-last-sample nil)
     (nidhogg-watchdog--start 100 2000 nil :no-monitor)
     (unwind-protect (progn ,@body)
       (set 'nidhogg-watchdog--test-sample-on-phase nil)
       (nidhogg-watchdog--test-clock nil)
       (nidhogg-watchdog--stop)
       (nidhogg-workers--release))))

(ert-deftest workers/wait-phase ()
  (skip-unless (workers-tests--wd-runs-p))
  (dolist (c '((worker-wait detach) (treesit-wait nil) (worker-wait display)))
    (workers-tests--wd
      (set 'nidhogg-watchdog--test-sample-on-phase (car c))
      (should (eql (workers-tests--wd-wait (car c) (cadr c)) 1))
      (let ((s (symbol-value 'nidhogg-watchdog--test-last-sample)))
        (should s)
        (should (eq (plist-get s :phase) (car c)))
        (should (eq (plist-get s :reason) (cadr c))))
      (should (eq (nidhogg-watchdog--test-phase) 'lisp)))))

(ert-deftest workers/wait-is-one-span ()
  ;; A worker wait is not a select: the watchdog's span runs through it
  ;; and ends at the next responsive wait, leaving one stall.
  (skip-unless (workers-tests--wd-runs-p))
  (workers-tests--wd
    (let ((t0 1000000000000))
      (nidhogg-watchdog--test-clock t0)
      (nidhogg-watchdog--test-idle-wait 0.001)
      (nidhogg-watchdog--take)
      (set 'nidhogg-watchdog--test-sample-on-phase 'worker-wait)
      (should (eql (workers-tests--wd-wait 'worker-wait 'detach) 1))
      (nidhogg-watchdog--test-clock (+ t0 300000000))
      (nidhogg-watchdog--test-idle-wait 0.001)
      (should (= (length (car (nidhogg-watchdog--take))) 1))
      (should (eq (plist-get (symbol-value 'nidhogg-watchdog--test-last-sample)
                             :phase)
                  'worker-wait)))))

(ert-deftest workers/budget-shared ()
  (nidhogg-workers--test-redisplay-begin)
  (nidhogg-workers--test-budget-ledger t)
  (should (equal (list (nidhogg-workers--test-budget-take 5)
                       (nidhogg-workers--test-budget-take 5)
                       (nidhogg-workers--test-budget-take 5))
                 '(1 0 0)))
  (should (equal (nidhogg-workers--test-budget-ledger t) '(1000000 . 2)))
  (nidhogg-workers--test-redisplay-begin)
  (should (eql (nidhogg-workers--test-budget-take 5) 1))
  (let ((nidhogg-redisplay-wait-budget 2))
    (nidhogg-workers--test-redisplay-begin)
    (should (eql (nidhogg-workers--test-budget-take 5) 2))))

(ert-deftest workers/phase-ms-counted ()
  (let ((nidhogg-workers--mode 'threads)
        j)
    (nidhogg-workers-stats t)
    (nidhogg-workers--pause-at 'before-done)
    (setq j (nidhogg-workers--test-job 'nh-blk 20 :class 'file
                                       :pause 'before-done))
    (should (nidhogg-workers--test-await-paused 'before-done 5000))
    (nidhogg-workers--test-release-on-wait 'before-done)
    (unwind-protect
        (progn
          (should (eql (nidhogg-workers--test-wait j 10000 'worker-wait 'detach) 1))
          (should (>= (plist-get (plist-get (plist-get (nidhogg-workers-stats)
                                                       :phase-ms)
                                            'worker-wait)
                                 'detach)
                      20.0)))
      (nidhogg-workers--release))))

(ert-deftest workers/no-watchdog-build ()
  ;; Built --without-nidhogg-watchdog: phases are no-ops, the wait works.
  (skip-unless (not (featurep 'nidhogg-watchdog)))
  (should (eql (workers-tests--wd-wait 'worker-wait 'detach) 1)))

(ert-deftest workers/stats-shape ()
  (let ((s (nidhogg-workers-stats)))
    (dolist (w '(:nh-io :nh-parse :nh-img :nh-blk :nh-save))
      (let ((ws (plist-get s w)))
        (should ws)
        (dolist (k '(:jobs :queued :queued-bytes :running :cancels :pauses
                     :backpressure :slices))
          (should (natnump (plist-get ws k))))
        (should (vectorp (plist-get ws :wait-ms)))
        (should (seq-every-p #'natnump (plist-get ws :wait-ms)))
        (should (listp (plist-get ws :fallbacks)))
        (should (floatp (plist-get ws :longest-ms)))))
    (let ((ph (plist-get s :phase-ms)))
      (dolist (k '(deliver json filter treesit treesit-wait image))
        (should (floatp (plist-get ph k))))
      (dolist (k '(display image-pixels detach drain))
        (should (floatp (plist-get (plist-get ph 'worker-wait) k)))))
    (should (natnump (plist-get s :blk-threads)))
    (should (natnump (plist-get (plist-get s :deferrals) 'treesit)))
    (should (natnump (plist-get (plist-get s :deferrals) 'image))))
  ;; RESET returns the counters, then zeroes them.
  (let ((nidhogg-workers--mode 'inline)
        (jobs (workers-tests--stat 'nh-save :jobs)))
    (should (nidhogg-workers--test-job 'nh-save 0))
    (should (eql (plist-get (plist-get (nidhogg-workers-stats t) :nh-save) :jobs)
                 (1+ jobs)))
    (should (eql (workers-tests--stat 'nh-save :jobs) 0))))

;;; workers-tests.el ends here

;;;; The inline-or-offload rule (Task 55)

(defmacro workers-tests--policy (&rest body)
  "Run BODY with a clean decision ledger and pressure, then restore."
  (declare (indent 0))
  `(unwind-protect
       (progn (nidhogg-workers--test-decide nil)
              (nidhogg-workers--test-pressure 'quiet)
              (nidhogg-workers-stats t)
              ,@body)
     (nidhogg-workers--test-decide nil)
     (nidhogg-workers--test-pressure nil)))

(defun workers-tests--decisions ()
  (plist-get (nidhogg-workers-stats) :decisions))

(ert-deftest workers/decide-inline-below-threshold ()
  (skip-unless (not (eq nidhogg-workers--mode 'inline)))
  (workers-tests--policy
    (should (eq (nidhogg-workers--decide 'image 3000000) 'inline))
    (should (eq (nidhogg-workers--decide 'image 5000000) 'offload))
    (should (eq (nidhogg-workers--decide 'image nil) 'offload))
    (let ((d (workers-tests--decisions)))
      (should (eql (cdr (assoc '(image inline estimate) d)) 1))
      (should (eql (cdr (assoc '(image offload estimate) d)) 1))
      (should (eql (cdr (assoc '(image offload no-estimate) d)) 1)))
    (should (equal (plist-get (nidhogg-workers--policy) :inline-ns) 4000000))))

(ert-deftest workers/decide-pressure-lowers ()
  (workers-tests--policy
    (should (eq (nidhogg-workers--decide 'ts 2000000) 'inline))
    (nidhogg-workers--test-pressure t)
    (should (eq (nidhogg-workers--decide 'ts 2000000) 'offload))
    (should (eql (cdr (assoc '(ts offload pressure) (workers-tests--decisions)))
                 1))
    ;; base-only ignores it.
    (should (eq (nidhogg-workers--decide 'ts 2000000 t) 'inline))))

(ert-deftest workers/pressure-hysteresis ()
  (workers-tests--policy
    (nidhogg-workers--test-pressure t)
    (nidhogg-workers--test-redisplay-begin)
    (nidhogg-workers--test-pressure 'quiet t)
    (dotimes (_ 2)
      (nidhogg-workers--test-redisplay-begin)
      (should (plist-get (plist-get (nidhogg-workers-stats) :pressure) :now)))
    (nidhogg-workers--test-redisplay-begin)
    (should-not (plist-get (plist-get (nidhogg-workers-stats) :pressure) :now))
    (should (eq (nidhogg-workers--decide 'ts 2000000) 'inline))))

(ert-deftest workers/pressure-forgets-watchdog-off ()
  ;; A 50 ms span closes, then the watchdog stops: no span closes any
  ;; more, so its last one must not keep the threshold lowered.
  (skip-unless (workers-tests--wd-runs-p))
  (workers-tests--policy
    (workers-tests--wd
      (let ((t0 1000000000000))
        (nidhogg-watchdog--test-clock t0)
        (nidhogg-watchdog--test-idle-wait 0.001)
        (nidhogg-watchdog--test-clock (+ t0 50000000))
        (nidhogg-watchdog--test-idle-wait 0.001)))
    (nidhogg-workers--test-pressure nil)
    (dotimes (_ 3)
      (nidhogg-workers--test-redisplay-begin))
    (should-not (plist-get (plist-get (nidhogg-workers-stats) :pressure) :now))
    (should (eq (nidhogg-workers--decide 'ts 2000000) 'inline))))

(ert-deftest workers/pressure-never-raises ()
  (workers-tests--policy
    (should (eq (nidhogg-workers--decide 'ts 5000000) 'offload))
    (should (eql (cdr (assoc '(ts offload estimate) (workers-tests--decisions)))
                 1))))

(ert-deftest workers/decide-forced-inline-mode ()
  (workers-tests--policy
    (let ((nidhogg-workers--mode 'inline))
      (should (eq (nidhogg-workers--decide 'image 1) 'offload))
      (should (eql (cdr (assoc '(image offload forced)
                               (workers-tests--decisions)))
                   1)))
    (nidhogg-workers--test-decide 'inline)
    (should (eq (nidhogg-workers--decide 'image 99000000) 'inline))
    (should (eql (cdr (assoc '(image inline forced) (workers-tests--decisions)))
                 1))))
