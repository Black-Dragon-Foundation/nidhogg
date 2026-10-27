;;; watchdog-lisp-tests.el --- tests for the stall watchdog (lisp)  -*- lexical-binding: t -*-

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

;; The mode, the drain and the log (Task 14).  The drain rows run the C side
;; on the virtual clock (`--test-clock', no monitor thread) to make real
;; stalls; the drain's own clock is `nidhogg-watchdog--clock', bound to a
;; virtual one where time matters.  Logs go to a temporary directory.

(defconst watchdog-lisp--t0 1000000000000 "The virtual clock's origin, ns.")

(defvar watchdog-lisp--now nil "The C virtual clock, ns.")

(defmacro watchdog-lisp--with-log (&rest body)
  "Run BODY with a fresh log directory and fresh per-day counters."
  (declare (indent 0))
  `(let* ((dir (make-temp-file "nh-wd-log" t))
          (nidhogg-watchdog-log-directory (file-name-as-directory dir))
          (nidhogg-watchdog--day nil)
          (nidhogg-watchdog--day-dir nil)
          (nidhogg-watchdog--day-stalls 0)
          (nidhogg-watchdog--over-cap 0)
          (nidhogg-watchdog--queue nil)
          (nidhogg-watchdog--last-summary nil)
          (nidhogg-watchdog--drain-hist (make-vector 9 0))
          (nidhogg-watchdog-record-functions nil)
          (nidhogg-watchdog--resolve-function nil)
          (nidhogg-watchdog--collections-function nil))
     (unwind-protect (progn ,@body)
       (delete-directory dir t))))

(defmacro watchdog-lisp--running (&rest body)
  "Run BODY with the C side on (no monitor), on the virtual clock."
  (declare (indent 0))
  `(progn
     (nidhogg-watchdog--start 100 600000 nil :no-monitor)
     (unwind-protect
         (progn (watchdog-lisp--clock-at 0)
                (nidhogg-watchdog--test-idle-wait 0.001)
                (nidhogg-watchdog--take)
                ,@body)
       (nidhogg-watchdog--test-clock nil)
       (nidhogg-watchdog--stop))))

(defun watchdog-lisp--clock-at (ms)
  (setq watchdog-lisp--now (+ watchdog-lisp--t0 (round (* ms 1000000))))
  (nidhogg-watchdog--test-clock watchdog-lisp--now))

(defun watchdog-lisp--span (ms)
  "A busy span of MS milliseconds, ended by a short idle wait."
  (setq watchdog-lisp--now (+ watchdog-lisp--now (round (* ms 1000000))))
  (nidhogg-watchdog--test-clock watchdog-lisp--now)
  (nidhogg-watchdog--test-idle-wait 0.001))

(defun watchdog-lisp--lines (&optional type)
  "The parsed lines of every day file (of TYPE, a string, when given).
Stalls are filed under the day of their start, and the virtual clock's
starts are in 1970."
  (let (out)
    (dolist (file (directory-files nidhogg-watchdog-log-directory t "\\.jsonl\\'"))
      (with-temp-buffer
        (insert-file-contents file)
        (dolist (l (split-string (buffer-string) "\n" t))
          (let ((o (json-parse-string l :object-type 'alist)))
            (when (or (null type) (equal (alist-get 'type o) type))
              (push o out))))))
    (nreverse out)))

(ert-deftest watchdog/drain-fast-path ()
  ;; Nothing pending: the drain allocates nothing (not even the clock read).
  (watchdog-lisp--with-log
    (watchdog-lisp--running
      (should (eq 0 (nidhogg-watchdog--pending)))
      ;; Warm up, then compare the same stretch of code with and without
      ;; the drain: the counters must move alike.
      (nidhogg-watchdog--drain)
      (memory-use-counts)
      ;; (The tests are interpreted, so `let' bindings cons: bind first.)
      (let (c d e f)
        (setq c (memory-use-counts))
        (setq d (memory-use-counts))
        (setq e (memory-use-counts))
        (nidhogg-watchdog--drain)
        (setq f (memory-use-counts))
        (should (equal (cl-mapcar #'- f e) (cl-mapcar #'- d c)))))))

(ert-deftest watchdog/drain-budget ()
  ;; A resolver costing 1 ms of (virtual) time per record: a 2 ms budget
  ;; resolves two of five; the rest wait as Lisp data for the next drains.
  (watchdog-lisp--with-log
    (watchdog-lisp--running
      (dotimes (_ 5) (watchdog-lisp--span 100))
      (let* ((vt 0)
             (nidhogg-watchdog--clock (lambda () (/ vt 1000.0)))
             (nidhogg-watchdog--resolve-function
              (lambda (raw) (setq vt (1+ vt)) raw)))
        (should (= (nidhogg-watchdog--drain) 2))
        (should (= (length nidhogg-watchdog--queue) 3))
        (should (= (length (watchdog-lisp--lines "stall")) 2))
        (should (= (nidhogg-watchdog--drain) 2))
        (should (= (nidhogg-watchdog--drain) 1))
        (should-not nidhogg-watchdog--queue)
        (should (= (length (watchdog-lisp--lines "stall")) 5))
        ;; The drains went into the drain histogram.
        (should (= 3 (apply #'+ (append nidhogg-watchdog--drain-hist nil))))
        (should (equal (plist-get (nidhogg-watchdog-stats) :drain)
                       nidhogg-watchdog--drain-hist))))))

(ert-deftest watchdog/log-schema ()
  (watchdog-lisp--with-log
    (watchdog-lisp--running
      (watchdog-lisp--span 150)
      (nidhogg-watchdog-drain-now)
      (nidhogg-watchdog--log-write
       'gc '(:pause-ms 118.4 :origin "auto" :felt :false :span-wall-ms 131.0
             :heap (:live-mb 742.3) :rss-mb 2410))
      (nidhogg-watchdog--write-summary)
      (let ((stall (car (watchdog-lisp--lines "stall")))
            (gc (car (watchdog-lisp--lines "gc")))
            (summary (car (watchdog-lisp--lines "summary"))))
        (dolist (l (list stall gc summary))
          (should (eql (alist-get 'v l) 1))
          (should (string-match-p
                   "\\`[0-9]\\{4\\}-[0-9][0-9]-[0-9][0-9]T[0-9:]\\{8\\}\\.[0-9]\\{3\\}[-+][0-9][0-9]:[0-9][0-9]\\'"
                   (alist-get 'at l)))
          (should (eql (alist-get 'pid l) (emacs-pid)))
          (should (equal (alist-get 'nidhogg l) nidhogg-version)))
        (should (floatp (alist-get 'wall_ms stall)))
        (should (= (alist-get 'wall_ms stall) 150.0))
        (should (numberp (alist-get 'nongc_ms stall)))
        (should (numberp (alist-get 'cpu_ms stall)))
        (should (integerp (alist-get 'gcs stall)))
        (should (eq (alist-get 'felt stall) :false))
        (should (eq (alist-get 'hang stall) :false))
        (should (eq (alist-get 'killed stall) :false))
        (should (eq (alist-get 'input_queued stall) :false))
        (should (eq (alist-get 'entry stall) :null))
        (should (eq (alist-get 'signature stall) :null))
        (should (= (alist-get 'pause_ms gc) 118.4))
        (should (equal (alist-get 'origin gc) "auto"))
        (should (eq (alist-get 'felt gc) :false))
        (should (= (alist-get 'live_mb (alist-get 'heap gc)) 742.3))
        (should (eq (alist-get 'final summary) :false))
        (dolist (k '(spans stalls stalls_over_cap missed stale ring_overflows
                     gc_dropped monitor_cpu_ms drain_p95_ms histograms))
          (should (assq k summary)))
        (should (= (length (alist-get 'command (alist-get 'histograms summary)))
                   9))
        (should (eql (alist-get 'stalls summary) 1))))))

(ert-deftest watchdog/log-cap ()
  ;; 5,001 stalls: 5,000 lines and a count.  Other lines are not capped.
  (watchdog-lisp--with-log
    (dotimes (_ 5001) (nidhogg-watchdog--log-write 'stall '(:wall-ms 100.0)))
    (should (= (length (watchdog-lisp--lines "stall")) 5000))
    (should (= nidhogg-watchdog--over-cap 1))
    (nidhogg-watchdog--log-write 'gc '(:pause-ms 1.0))
    (should (= (length (watchdog-lisp--lines "gc")) 1))
    ;; A restart the same day keeps counting from the file.
    (let ((nidhogg-watchdog--day nil))
      (nidhogg-watchdog--log-write 'stall '(:wall-ms 100.0))
      (should (= (length (watchdog-lisp--lines "stall")) 5000)))))

(ert-deftest watchdog/log-prune ()
  (watchdog-lisp--with-log
    (let ((old (expand-file-name "2026-01-01.jsonl" nidhogg-watchdog-log-directory))
          (new (expand-file-name "2026-10-20.jsonl" nidhogg-watchdog-log-directory))
          (other (expand-file-name "notes.txt" nidhogg-watchdog-log-directory)))
      (dolist (f (list old new other)) (write-region "{}\n" nil f))
      (set-file-times old (time-subtract nil (* 15 86400)))
      (set-file-times other (time-subtract nil (* 30 86400)))
      (set-file-times new (time-subtract nil (* 13 86400)))
      (nidhogg-watchdog--prune)
      (should-not (file-exists-p old))
      (should (file-exists-p new))
      (should (file-exists-p other)))))

(ert-deftest watchdog/summary-every-10-min ()
  (watchdog-lisp--with-log
    (watchdog-lisp--running
      (let* ((now 1000.0)
             (nidhogg-watchdog--clock (lambda () now)))
        (setq nidhogg-watchdog--last-summary now)
        (nidhogg-watchdog--maybe-summary)
        (should (= (length (watchdog-lisp--lines "summary")) 0))
        (setq now 1599.0)
        (nidhogg-watchdog--maybe-summary)
        (should (= (length (watchdog-lisp--lines "summary")) 0))
        (setq now 1600.0)
        (nidhogg-watchdog--maybe-summary)
        (should (= (length (watchdog-lisp--lines "summary")) 1))
        (setq now 1700.0)
        (nidhogg-watchdog--maybe-summary)
        (should (= (length (watchdog-lisp--lines "summary")) 1))
        (setq now 2200.0)
        (nidhogg-watchdog--maybe-summary)
        (should (= (length (watchdog-lisp--lines "summary")) 2))))))

(ert-deftest watchdog/record-functions-called ()
  (watchdog-lisp--with-log
    (watchdog-lisp--running
      (let (seen)
        (let ((nidhogg-watchdog-record-functions
               (list (lambda (_r) (error "A broken hook function"))
                     (lambda (r) (push (plist-get r :wall-ms) seen)))))
          (watchdog-lisp--span 120)
          (watchdog-lisp--span 130)
          (nidhogg-watchdog-drain-now))
        (should (equal (nreverse seen) '(120.0 130.0)))
        (should (= (length (watchdog-lisp--lines "stall")) 2))))))

(ert-deftest watchdog/mode-off-stops ()
  (watchdog-lisp--with-log
    (let ((tasks (length (directory-files "/proc/self/task" nil "\\`[0-9]"))))
      (unwind-protect
          (progn
            (nidhogg-watchdog-mode 1)
            (should nidhogg-watchdog-mode)
            (should (nidhogg-watchdog--stats))
            (should (timerp nidhogg-watchdog--timer))
            (should (memq nidhogg-watchdog--timer timer-idle-list))
            ;; On again: nothing new starts.
            (let ((timer nidhogg-watchdog--timer))
              (nidhogg-watchdog-mode 1)
              (should (eq timer nidhogg-watchdog--timer)))
            (should (= (1+ tasks)
                       (length (directory-files "/proc/self/task" nil "\\`[0-9]"))))
            (let ((timer nidhogg-watchdog--timer))
              (nidhogg-watchdog-mode -1)
              (should-not (memq timer timer-idle-list)))
            (should-not nidhogg-watchdog--timer)
            (should-not (nidhogg-watchdog--stats))
            (should (= tasks
                       (length (directory-files "/proc/self/task" nil "\\`[0-9]"))))
            (let ((summaries (watchdog-lisp--lines "summary")))
              (should (= (length summaries) 1))
              (should (eq (alist-get 'final (car summaries)) t))))
        (when nidhogg-watchdog-mode (nidhogg-watchdog-mode -1))))))
;;; watchdog-lisp-tests.el ends here
