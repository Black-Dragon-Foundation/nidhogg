;;; watchdog-timing-tests.el --- tests for the stall watchdog (timing)  -*- lexical-binding: t -*-

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

;; The timing group: real wall-clock spins, a blocked call,
;; and child Nidhoggs on a pty (the only way to drive the real keyboard
;; hooks, an interactive SA_RESTART default, `suspend-emacs' and a Lisp
;; thread that holds the global lock while the main thread idles).  The
;; margins are wide (1 s against the 100 ms threshold) and the rows assert
;; counts, never sample numbers: a sample assertion is "at least one, or
;; `:missed' counted" (counting is the main thread's, so a starved
;; monitor costs samples, never a record).
;;
;; The deterministic rows are in watchdog-tests.el and friends.

;;; Code:

(require 'ert)
(require 'cl-lib)

(defmacro watchdog-timing--running (threshold &rest body)
  "Run BODY with the watchdog on at THRESHOLD ms and its monitor thread.
The span of the test itself is ended and dropped first."
  (declare (indent 1))
  `(progn
     (nidhogg-watchdog--start ,threshold 600000 nil)
     (unwind-protect
         (progn
           (nidhogg-watchdog--test-idle-wait 0.01)
           (nidhogg-watchdog--take)
           (nidhogg-watchdog--stats t)
           ,@body)
       (nidhogg-watchdog--stop))))

(defun watchdog-timing--spin (ms)
  "Busy-loop for MS milliseconds without any wait."
  (let ((end (+ (float-time) (/ ms 1000.0))))
    (while (< (float-time) end))))

(defun watchdog-timing--stalls-from-timer (fn wait)
  "Run FN from a timer inside a responsive wait of WAIT s; return the stalls."
  (run-at-time 0 nil fn)
  (nidhogg-watchdog--test-idle-wait wait)
  ;; A timer that ran past WAIT leaves no time for a select: wait once
  ;; more, so the span has ended.
  (nidhogg-watchdog--test-idle-wait 0.02)
  (car (nidhogg-watchdog--take)))

(defun watchdog-timing--stat (key)
  (plist-get (nidhogg-watchdog--stats) key))

(defun watchdog-timing--sampled-p (stall)
  "Whether STALL has a sample, or counted the ones it missed."
  (or (>= (plist-get stall :samples-taken) 1)
      (> (plist-get stall :missed) 0)))

(defun watchdog-timing--frames (sample)
  "The functions in SAMPLE's inner and outer frames, as a list."
  (append (plist-get sample :frames) (plist-get sample :outer) nil))

;;;; Real spins and blocked calls

(defun watchdog-timing--spin-1s () (watchdog-timing--spin 1000))

(ert-deftest watchdog/spin-from-timer ()
  (watchdog-timing--running 100
    (let* ((stalls (watchdog-timing--stalls-from-timer
                    #'watchdog-timing--spin-1s 1.5))
           (s (car stalls)))
      (should (= (length stalls) 1))
      (should (>= (plist-get s :wall-ms) 1000))
      ;; State `cpu': the main thread ran for most of the span.
      (should (>= (/ (plist-get s :cpu-ms) (plist-get s :wall-ms)) 0.8))
      (should (watchdog-timing--sampled-p s))
      ;; The timer function is on the stack of the samples it got.
      (when (> (length (plist-get s :samples)) 0)
        (should (cl-some (lambda (sm)
                           (memq 'watchdog-timing--spin-1s
                                 (watchdog-timing--frames sm)))
                         (plist-get s :samples)))))))

(defun watchdog-timing--sleep-0.5 () (call-process "sleep" nil nil nil "0.5"))

(ert-deftest watchdog/blocked-call-process ()
  (watchdog-timing--running 100
    (let* ((stalls (watchdog-timing--stalls-from-timer
                    #'watchdog-timing--sleep-0.5 1))
           (s (car stalls)))
      (should (= (length stalls) 1))
      (should (>= (plist-get s :wall-ms) 500))
      ;; State `blocked': almost no CPU.
      (should (< (/ (plist-get s :cpu-ms) (plist-get s :wall-ms)) 0.2))
      (should (watchdog-timing--sampled-p s))
      (when (> (length (plist-get s :samples)) 0)
        (should (cl-some (lambda (sm) (eq (plist-get sm :phase) 'call-process))
                         (plist-get s :samples)))))))

(defun watchdog-timing--spin-150 () (watchdog-timing--spin 150))
(defun watchdog-timing--spin-800 () (watchdog-timing--spin 800))

(ert-deftest watchdog/requests-only-past-half-threshold ()
  ;; The monitor asks for samples only once a span is past half the
  ;; threshold (200 ms of 400): a 150 ms spin gets none; a 800 ms one does
  ;; (the control: the counter does move).
  (watchdog-timing--running 400
    (let ((before (watchdog-timing--stat :requests)))
      (should (null (watchdog-timing--stalls-from-timer
                     #'watchdog-timing--spin-150 0.5)))
      (should (eql (watchdog-timing--stat :requests) before))
      (should (= (length (watchdog-timing--stalls-from-timer
                          #'watchdog-timing--spin-800 1))
                 1))
      (should (> (watchdog-timing--stat :requests) before)))))

;;;; Child Nidhoggs on a pty

(defconst watchdog-timing--child-prelude
  ";; -*- lexical-binding: t -*-
(setq make-backup-files nil auto-save-default nil create-lockfiles nil
      inhibit-startup-screen t)
(defvar wdc--dir (getenv \"WDT_DIR\"))
(defvar wdc--spans0 nil)
(defvar wdc--extra nil)
(defun wdc--touch (name)
  (write-region \"\" nil (expand-file-name name wdc--dir) nil 'silent))
(defun wdc--spin (ms)
  (let ((end (+ (float-time) (/ ms 1000.0))))
    (while (< (float-time) end))))
(defun wdc--start ()
  \"Start the watchdog (threshold 100 ms) and end the startup span.\"
  (nidhogg-watchdog--start 100 600000 nil)
  (nidhogg-watchdog--test-idle-wait 0.01)
  (nidhogg-watchdog--take)
  (setq wdc--spans0 (plist-get (nidhogg-watchdog--stats) :spans)))
(defun wdc--strip (s)
  (let (o)
    (while s
      (unless (eq (car s) :samples) (push (car s) o) (push (cadr s) o))
      (setq s (cddr s)))
    (nreverse o)))
(defun wdc--dump ()
  \"Write the stalls (without their samples) and the counters, then exit.\"
  (let ((stalls (car (nidhogg-watchdog--take)))
        (stats (nidhogg-watchdog--stats)))
    (with-temp-file (expand-file-name \"result.eld\" wdc--dir)
      (let ((print-length nil) (print-level nil))
        (prin1 (list :stalls (mapcar #'wdc--strip stalls)
                     :spans0 wdc--spans0 :stats stats :extra wdc--extra)
               (current-buffer)))))
  (kill-emacs 0))
"
  "The helpers every child script starts with.")

(defun watchdog-timing--emacs ()
  (expand-file-name invocation-name invocation-directory))

(defun watchdog-timing--wait-for (proc file timeout)
  "Wait until FILE exists, PROC has died, or TIMEOUT s passed; non-nil if FILE."
  (let ((deadline (+ (float-time) timeout)))
    (while (and (not (file-exists-p file))
                (process-live-p proc)
                (< (float-time) deadline))
      (accept-process-output proc 0.02))
    (file-exists-p file)))

(defvar watchdog-timing--child-output ""
  "What the last child wrote to its terminal (for a failure message).")

(defun watchdog-timing--run-child (script &optional on-ready shell-wrap)
  "Run a child Nidhogg on a pty whose -l script is the prelude plus SCRIPT.
ON-READY, when non-nil, is called with the process once the child has
touched `ready'.  SHELL-WRAP, a format string with one %s for the
Nidhogg command line, replaces the plain `exec'.  Return the child's
result plist (:stalls :spans0 :stats :extra), or nil when it never wrote one."
  (let* ((dir (make-temp-file "wdt-child" t))
         (file (expand-file-name "child.el" dir))
         (result (expand-file-name "result.eld" dir))
         (ready (expand-file-name "ready" dir))
         (cmd (format "%s -nw -Q -l %s"
                      (shell-quote-argument (watchdog-timing--emacs))
                      (shell-quote-argument file)))
         (shell (format "stty rows 24 cols 80; %s"
                        (if shell-wrap (format shell-wrap cmd)
                          (concat "exec " cmd))))
         (process-environment (append (list "TERM=xterm-256color"
                                            (concat "WDT_DIR=" dir))
                                      process-environment))
         proc)
    (setq watchdog-timing--child-output "")
    (unwind-protect
        (progn
          (with-temp-file file
            (insert watchdog-timing--child-prelude script))
          (setq proc (make-process :name "wdt-child" :noquery t
                                   :connection-type 'pty
                                   :command (list "bash" "-c" shell)
                                   :filter (lambda (_p out)
                                             (setq watchdog-timing--child-output
                                                   (concat watchdog-timing--child-output out)))
                                   :sentinel #'ignore))
          (when on-ready
            (should (watchdog-timing--wait-for proc ready 20))
            (funcall on-ready proc))
          (watchdog-timing--wait-for proc result 30)
          (when (file-exists-p result)
            (with-temp-buffer
              (insert-file-contents result)
              (read (current-buffer)))))
      (when (process-live-p proc)
        (ignore-errors (signal-process proc 'SIGKILL))
        (delete-process proc))
      (delete-directory dir t))))

(defun watchdog-timing--long-stalls (result ms)
  "The stalls in RESULT that lasted at least MS."
  (cl-remove-if-not (lambda (s) (>= (plist-get s :wall-ms) ms))
                    (plist-get result :stalls)))

(defconst watchdog-timing--spin-script
  "(wdc--start)
(run-at-time 0.5 nil
             (lambda ()
               (wdc--touch \"ready\")
               (wdc--spin 600)
               (run-at-time 0.4 nil #'wdc--dump)))
"
  "A 600 ms spin in a timer, after a marker.")

(defun watchdog-timing--send-key (proc)
  "Write a key to PROC's pty 150 ms after the marker, inside the spin."
  (accept-process-output proc 0.15)
  (process-send-string proc "x"))

(ert-deftest watchdog/interactive-pty ()
  ;; An interactive child (so not the batch default of the signal's
  ;; SA_RESTART): the spin is felt when a key arrives during it, and not
  ;; when none does.  No EINTR clause: Emacs's reads loop on EINTR in
  ;; `emacs_read', so it could not fail; `handler-sa-restart' pins the
  ;; flag.
  (let* ((felt (watchdog-timing--run-child watchdog-timing--spin-script
                                           #'watchdog-timing--send-key))
         (quiet (watchdog-timing--run-child watchdog-timing--spin-script))
         (a (watchdog-timing--long-stalls felt 550))
         (b (watchdog-timing--long-stalls quiet 550)))
    (should felt)
    (should quiet)
    (should (= (length a) 1))
    (should (eq (plist-get (car a) :felt) t))
    (should (> (plist-get (car a) :input-wait-ms) 0))
    (should (= (length b) 1))
    (should (null (plist-get (car b) :felt)))
    (should (null (plist-get (car b) :input-wait-ms)))))

(defconst watchdog-timing--thread-script
  "(wdc--start)
(make-thread (lambda ()
               (wdc--touch \"ready\")
               (wdc--spin 500))
             \"wdt-worker\")
(read-event)
(nidhogg-watchdog--test-idle-wait 0.05)
(wdc--dump)
"
  "The main thread idles in `read-event' while a worker spins 500 ms.")

(ert-deftest watchdog/lisp-thread-holds-lock-while-idle ()
  ;; The worker takes the global lock when the main thread goes idle and
  ;; does not yield for 500 ms.  A key ends the idle select, and the main
  ;; thread waits for the lock: busy, with a Lisp thread holding it.
  (skip-unless (featurep 'threads))
  (let* ((result (watchdog-timing--run-child watchdog-timing--thread-script
                                             #'watchdog-timing--send-key))
         (stalls (watchdog-timing--long-stalls result 250)))
    (should result)
    (should (= (length stalls) 1))
    (should (plist-get (car stalls) :lisp-thread))))

(defconst watchdog-timing--suspend-script
  "(wdc--start)
(let ((t0 (float-time)))
  (suspend-emacs)
  (setq wdc--extra (- (float-time) t0)))
(nidhogg-watchdog--test-idle-wait 0.05)
(wdc--dump)
"
  "Suspend the child with SIGTSTP, as C-z does.")

(ert-deftest watchdog/suspend-is-waiting ()
  ;; `suspend-emacs' sends SIGTSTP to the process group, which the kernel
  ;; discards in an orphaned group, so the child runs under a job-control
  ;; shell (`set -m'); the shell resumes it after 300 ms.  (A bare SIGCONT
  ;; from outside would leave the child in a background group that the
  ;; terminal setup stops again with SIGTTOU.)  The child measures that it
  ;; really stopped.
  (let* ((result (watchdog-timing--run-child
                  watchdog-timing--suspend-script nil
                  "set -m; %s; sleep 0.3; fg %%1")))
    (unless result
      (message "suspend child output: %S" watchdog-timing--child-output))
    (should result)
    (should (>= (plist-get result :extra) 0.25))
    (should (null (watchdog-timing--long-stalls result 100)))))

(defconst watchdog-timing--sit-for-script
  "(wdc--start)
(sit-for 1)
(nidhogg-watchdog--test-idle-wait 0.05)
(wdc--dump)
"
  "`sit-for' 1 s, alone.")

(ert-deftest watchdog/sit-for-is-waiting-tty ()
  (let ((result (watchdog-timing--run-child watchdog-timing--sit-for-script)))
    (should result)
    (should (null (watchdog-timing--long-stalls result 100)))
    (should (> (plist-get (plist-get result :stats) :spans)
               (plist-get result :spans0)))))

;;;; Stress

(defvar watchdog-timing--var nil)

(defun watchdog-timing--alloc ()
  "Allocation-heavy work: conses, strings, vectors, hash tables."
  (let ((acc nil) (h (make-hash-table :test 'equal)))
    (dotimes (i 20000)
      (push (cons i (number-to-string i)) acc)
      (puthash (number-to-string i) (make-vector 4 i) h))
    (length (sort (mapcar #'car acc) #'>))))

(defun watchdog-timing--bytecode-work (n)
  (let ((s 0)) (dotimes (i n) (setq s (+ s (* i i)))) s))

(defun watchdog-timing--native-work ()
  "Preloaded (so native-compiled in a dev build) Lisp."
  (dotimes (_ 200)
    (split-string "a b c d e f g h" " ")
    (string-join '("x" "y" "z") ",")
    (alist-get 'c '((a . 1) (b . 2) (c . 3)))))

(defun watchdog-timing--deep-bindings ()
  "Bind 120 000 special variables at once: the specpdl grows past 10^5."
  (let* ((syms (cl-loop repeat 120000 collect (make-symbol "wd")))
         (max-lisp-eval-depth 100000))
    (eval `(let ,(mapcar (lambda (s) (list s 1)) syms)
             (watchdog-timing--alloc)
             (garbage-collect)
             (watchdog-timing--bytecode-work 200000))
          nil)))

(defvar watchdog-timing--stress-result nil)

(defun watchdog-timing--stress-10s ()
  (let ((end (+ (float-time) 10.0)) (round 0))
    (while (< (float-time) end)
      (setq round (1+ round))
      (watchdog-timing--alloc)
      (watchdog-timing--bytecode-work 300000)
      (watchdog-timing--native-work)
      (when (zerop (% round 3)) (garbage-collect))
      (when (zerop (% round 5)) (watchdog-timing--deep-bindings)))
    (setq watchdog-timing--stress-result round)))

(ert-deftest watchdog/stress-10k-samples ()
  ;; 10 s of `--deep' sampling (1 ms) across allocation-heavy, byte-compiled
  ;; and native-compiled code, with forced collections and a specpdl past
  ;; 10^5 entries.  No crash, and every word the samples hold is a live
  ;; object or `nidhogg-watchdog--gone'.  Missed over requests is reported,
  ;; never asserted: it depends on scheduling.
  ;; Byte code for two of the workers; the rest stays interpreted or is
  ;; preloaded (native) Lisp.
  (mapc #'watchdog-timing--compile
        '(watchdog-timing--alloc watchdog-timing--bytecode-work))
  (watchdog-timing--running 100
    (nidhogg-watchdog--deep 1)
    (let* ((stalls (watchdog-timing--stalls-from-timer
                    #'watchdog-timing--stress-10s 10.5))
           (s (car stalls))
           (requests (watchdog-timing--stat :requests))
           (missed (watchdog-timing--stat :missed))
           (seen (make-hash-table :test 'eq)))
      (should (= (length stalls) 1))
      (should (>= (plist-get s :wall-ms) 10000))
      (should (plist-get s :deep))
      (should (watchdog-timing--sampled-p s))
      (should (> watchdog-timing--stress-result 0))
      (dolist (sm (append (plist-get s :samples) nil))
        (dolist (f (watchdog-timing--frames sm))
          (puthash f t seen)))
      (maphash (lambda (f _)
                 (should (or (eq f 'nidhogg-watchdog--gone) (symbolp f)
                             (functionp f) (consp f)))
                 (should (stringp (prin1-to-string f))))
               seen)
      (garbage-collect)
      (message "watchdog/stress-10k-samples: rounds %d, wall %.0f ms, samples-taken %d, kept %d, thinned %d, stride %d, requests %d, missed %d (%.2f), stale %d, distinct frame words %d, native-comp %s"
               watchdog-timing--stress-result (plist-get s :wall-ms)
               (plist-get s :samples-taken) (length (plist-get s :samples))
               (plist-get s :thinned) (plist-get s :stride)
               requests missed (if (> requests 0) (/ (float missed) requests) 0.0)
               (watchdog-timing--stat :stale) (hash-table-count seen)
               (if (native-comp-function-p (symbol-function 'split-string)) "yes" "no")))))

(defun watchdog-timing--compile (fn)
  "Byte-compile FN, so the stress row runs byte code."
  (unless (byte-code-function-p (symbol-function fn))
    (byte-compile fn)))

;;;; Hangs

(defun watchdog-timing--hang-script (log body)
  "A child script: the mode logs to LOG, hang-ms 1000, then BODY."
  (format "(setq nidhogg-watchdog-log-directory %S
      nidhogg-watchdog-threshold 100
      nidhogg-watchdog-hang-ms 1000)
%s"
          (file-name-as-directory log) body))

(defconst watchdog-timing--hang-spin
  "(defun wdc--hang-spin () (wdc--spin 5000))
(nidhogg-watchdog-mode 1)
(run-at-time 0.5 nil (lambda ()
                       (wdc--touch \"ready\")
                       (wdc--hang-spin)
                       (wdc--dump)))
"
  "A 5 s spin in a timer, with the mode on.")

(defconst watchdog-timing--hang-reader
  "(add-hook 'nidhogg-watchdog-record-functions
          (lambda (r) (push r wdc--extra)))
(nidhogg-watchdog-mode 1)
(setq wdc--extra (list :records wdc--extra
                       :left (directory-files nidhogg-watchdog-log-directory
                                              nil \"\\\\`hang-\")))
(wdc--dump)
"
  "Turn the mode on, keep the records its start made, then dump.")

(defun watchdog-timing--await-hang-file (proc dir pid timeout)
  "Wait up to TIMEOUT s for PID's hang file in DIR to name the spin.
Return the file's text, or nil."
  (let ((deadline (+ (float-time) timeout))
        (re (format "\\`hang-%d-[0-9]+\\.txt\\'" pid))
        text)
    (while (and (not text) (process-live-p proc) (< (float-time) deadline))
      (dolist (f (directory-files dir t re))
        (let ((s (ignore-errors
                   (with-temp-buffer (insert-file-contents f) (buffer-string)))))
          (when (and s (string-search "wdc--hang-spin" s))
            (setq text s))))
      (unless text
        (accept-process-output proc 0.05)))
    text))

(defun watchdog-timing--hang-killed-by (sig)
  "End a child's hang with SIG; a fresh child must record it as killed.
The child spins 5 s with hang-ms 1000 and gets SIG once its hang file
names the spin function (within 4 s of the spin's start).  The fresh
child's mode start turns the file into one `:killed' record with that
name and deletes it."
  (let ((log (make-temp-file "wdt-hang" t))
        pid text)
    (unwind-protect
        (progn
          (watchdog-timing--run-child
           (watchdog-timing--hang-script log watchdog-timing--hang-spin)
           (lambda (proc)
             (setq pid (process-id proc)
                   text (watchdog-timing--await-hang-file proc log pid 4))
             (signal-process proc sig)))
          (should pid)
          (should text)
          (should (string-search (format "pid %d\n" pid) text))
          (let* ((result (watchdog-timing--run-child
                          (watchdog-timing--hang-script
                           log watchdog-timing--hang-reader)))
                 (extra (plist-get result :extra))
                 (killed (cl-remove-if-not (lambda (r) (plist-get r :killed))
                                           (plist-get extra :records))))
            (should result)
            (should (= 1 (length killed)))
            (should (= pid (plist-get (car killed) :hang-pid)))
            (should (eq t (plist-get (car killed) :hang)))
            (should (cl-some (lambda (st)
                               (member "wdc--hang-spin"
                                       (append (plist-get st :frames) nil)))
                             (plist-get (car killed) :stacks)))
            (should-not (plist-get extra :left))))
      (delete-directory log t))))

(ert-deftest watchdog/hang-file-survives-kill ()
  ;; SIGKILL: nothing runs in the child; the file simply stays.
  (watchdog-timing--hang-killed-by 'SIGKILL))

(ert-deftest watchdog/hang-file-survives-sigterm ()
  ;; SIGTERM (`kill PID', a service restart): Emacs runs `kill-emacs' from
  ;; the signal handler while the main thread is still busy, so the mode is
  ;; turned off inside the hang; that stop must keep the file.
  (watchdog-timing--hang-killed-by 'SIGTERM))

(ert-deftest watchdog/hang-file-survives-sighup ()
  ;; SIGHUP (a logout, a closed terminal) takes the same path as SIGTERM.
  (watchdog-timing--hang-killed-by 'SIGHUP))

;;; watchdog-timing-tests.el ends here
