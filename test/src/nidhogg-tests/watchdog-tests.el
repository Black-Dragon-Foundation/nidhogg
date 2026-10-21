;;; watchdog-tests.el --- tests for the stall watchdog (flag, edges, spans)  -*- lexical-binding: t -*-

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

(ert-deftest watchdog/feature () (should (featurep 'nidhogg-watchdog)))

(ert-deftest watchdog/in-the-configuration ()
  (should (string-search "--with-nidhogg-watchdog" system-configuration-options)))

(ert-deftest watchdog/no-frames-when-off ()
  ;; Flag on, mode off: no thread, no handler, no stats.
  (let ((tasks (length (directory-files "/proc/self/task" nil "\\`[0-9]"))))
    (should-not (nidhogg-watchdog--stats))
    (should (= tasks (length (directory-files "/proc/self/task" nil "\\`[0-9]"))))))

;;;; Edges, spans and records (Task 7)

;; The deterministic rows run on the virtual clock (`--test-clock'): time
;; moves only when a test sets it, and every idle wait is
;; `--test-idle-wait', which waits with READ_KBD -1 so process.c sets and
;; unwinds the real `waiting_for_user_input_p'.  While the virtual clock
;; is set the monitor never arms (`:requests 0').

(defconst watchdog-test--t0 1000000000000
  "The virtual clock's origin, in ns (1000 s).")

(defvar watchdog-test--now nil
  "The virtual clock's current value, in ns.")

(defmacro watchdog-test--running (args &rest body)
  "Start the watchdog with ARGS (a list), run BODY, always stop it."
  (declare (indent 1))
  `(progn
     (apply #'nidhogg-watchdog--start ,args)
     (unwind-protect (progn ,@body)
       (nidhogg-watchdog--test-clock nil)
       (nidhogg-watchdog--stop))))

(defun watchdog-test--clock-at (ms)
  "Set the virtual clock to its origin plus MS milliseconds."
  (setq watchdog-test--now (+ watchdog-test--t0 (round (* ms 1000000))))
  (nidhogg-watchdog--test-clock watchdog-test--now))

(defun watchdog-test--idle ()
  "One short responsive wait: it ends the current span and starts the next."
  (nidhogg-watchdog--test-idle-wait 0.001))

(defun watchdog-test--span (ms)
  "Advance the virtual clock by MS milliseconds, then wait: a span of MS."
  (setq watchdog-test--now (+ watchdog-test--now (round (* ms 1000000))))
  (nidhogg-watchdog--test-clock watchdog-test--now)
  (watchdog-test--idle))

(defun watchdog-test--align ()
  "Put the virtual clock at its origin, end the span there, drop records."
  (watchdog-test--clock-at 0)
  (watchdog-test--idle)
  (nidhogg-watchdog--take))

(defun watchdog-test--stat (key)
  (plist-get (nidhogg-watchdog--stats) key))

(ert-deftest watchdog/span-counting ()
  ;; A 99 ms span is no stall; a 100 ms span is exactly one record, even
  ;; with no samples at all.
  (watchdog-test--running '(100 2000 nil)
    (watchdog-test--align)
    (let ((spans (watchdog-test--stat :spans)))
      (watchdog-test--span 99)
      (watchdog-test--span 100)
      (let ((stalls (car (nidhogg-watchdog--take))))
        (should (= (length stalls) 1))
        (should (= (plist-get (car stalls) :wall-ms) 100.0))
        (should (eql (plist-get (car stalls) :samples-taken) 0))
        (should (>= (watchdog-test--stat :spans) (+ spans 2)))
        (should (eql (watchdog-test--stat :stalls) 1))
        (should (eql (watchdog-test--stat :requests) 0))))))

(ert-deftest watchdog/histogram-buckets ()
  ;; Buckets (ms): <4 <8 <16 <33 <50 <100 <250 <1000 >=1000.  A wait may
  ;; select more than once, and each extra select ends a 0 ms span, so
  ;; the first bucket is only bounded below.
  (watchdog-test--running '(100 2000 nil :no-monitor)
    (watchdog-test--align)
    (nidhogg-watchdog--stats t)
    (dolist (ms '(3.9 4 15.9 16 99.9 1000))
      (watchdog-test--span ms))
    (let* ((h (watchdog-test--stat :histograms))
           (other (append (plist-get h :other) nil)))
      (should (>= (car other) 1))
      (should (equal (cdr other) '(1 1 1 0 1 0 0 1)))
      (should (equal (plist-get h :command) (make-vector 9 0)))
      (should (= (length (plist-get h :typing)) 9))
      (should (= (length (plist-get h :collections)) 9)))))

(ert-deftest watchdog/take-shape ()
  ;; The §6.1 keys; `--take' empties the ring; `--pending' counts it.
  (watchdog-test--running '(100 2000 nil :no-monitor)
    (watchdog-test--align)
    (watchdog-test--span 150)
    (watchdog-test--span 250)
    (should (>= (nidhogg-watchdog--pending) 2))
    (let* ((taken (nidhogg-watchdog--take))
           (stalls (car taken))
           (s (car stalls)))
      (should (listp (cdr taken)))
      (should (equal (mapcar (lambda (s) (plist-get s :wall-ms)) stalls)
                     '(150.0 250.0)))
      (should (< (plist-get s :id) (plist-get (cadr stalls) :id)))
      (dolist (k '(:id :span :start :wall-ms :gc-ms :gcs :nongc-ms :cpu-ms
                   :late-ms :felt :input-wait-ms :commands :input-queued
                   :hang :killed :lisp-thread :deep :kind :samples-taken
                   :missed :stale :thinned :stride :presented :samples))
        (should (plist-member s k)))
      (should (floatp (plist-get s :start)))
      (should (vectorp (plist-get s :samples)))
      (should (eq (plist-get s :kind) 'other))
      (should (< (abs (- (plist-get s :nongc-ms)
                         (- 150.0 (plist-get s :gc-ms))))
                 1e-6)))
    (should-not (car (nidhogg-watchdog--take)))))

(ert-deftest watchdog/ring-overflow-drops-oldest ()
  ;; 33 stalls into a ring of 32: the oldest goes, and is counted.
  (watchdog-test--running '(100 2000 nil :no-monitor)
    (watchdog-test--align)
    (nidhogg-watchdog--stats t)
    (dotimes (i 33)
      (watchdog-test--span (+ 100 i)))
    (let ((stalls (car (nidhogg-watchdog--take))))
      (should (= (length stalls) 32))
      (should (= (plist-get (car stalls) :wall-ms) 101.0))
      (should (= (plist-get (car (last stalls)) :wall-ms) 132.0))
      (should (eql (watchdog-test--stat :dropped) 1))
      (should (eql (watchdog-test--stat :stalls) 33)))))

(ert-deftest watchdog/responsive-flag-unwinds ()
  ;; §12 (S5): a `throw' out of a nested responsive wait inside a filter
  ;; run by `accept-process-output'.  process.c restores
  ;; `waiting_for_user_input_p' on the way out, so the outer
  ;; `accept-process-output' wait stays busy: one 250 ms stall, not a
  ;; 200 ms stall ended by the outer wait.
  (watchdog-test--running '(100 2000 nil :no-monitor)
    (watchdog-test--align)
    (let* ((state 'start)
           (proc (make-process
                  :name "watchdog-test" :connection-type 'pipe :noquery t
                  :command '("sh" "-c" "echo a; sleep 0.3; echo b")
                  :filter
                  (lambda (_proc out)
                    (when (and (eq state 'start) (string-search "a" out))
                      (setq state 'nested)
                      (catch 'watchdog-test--out
                        (run-at-time 0.02 nil
                                     (lambda () (throw 'watchdog-test--out nil)))
                        (nidhogg-watchdog--test-idle-wait 1))
                      (setq state 'thrown)
                      (watchdog-test--clock-at 200))
                    (when (string-search "b" out)
                      (setq state 'done))))))
      (while (and (not (eq state 'done)) (accept-process-output proc 2)))
      (should (eq state 'done))
      (watchdog-test--clock-at 250)
      (watchdog-test--idle)
      (let ((stalls (car (nidhogg-watchdog--take))))
        (should (= (length stalls) 1))
        (should (= (plist-get (car stalls) :wall-ms) 250.0))))))

(ert-deftest watchdog/inhibit-redisplay-wait-is-busy ()
  ;; In a timer, a 300 ms wait under `inhibit-redisplay' does not end the
  ;; span (`dbus-call-method', jsonrpc's cancel-on-input wait): one 300 ms
  ;; stall, not two of 150.  Its phase (`process-wait') is Task 11's row.
  (watchdog-test--running '(100 2000 nil)
    (watchdog-test--align)
    (run-at-time 0 nil (lambda ()
                         (watchdog-test--clock-at 150)
                         (let ((inhibit-redisplay t))
                           (nidhogg-watchdog--test-idle-wait 0.3))
                         (watchdog-test--clock-at 300)))
    (nidhogg-watchdog--test-idle-wait 0.5)
    (let ((stalls (car (nidhogg-watchdog--take))))
      (should (= (length stalls) 1))
      (should (= (plist-get (car stalls) :wall-ms) 300.0))
      (should (eql (watchdog-test--stat :requests) 0)))))

(ert-deftest watchdog/null-timeout-is-idle ()
  ;; A wait with no timeout of its own (left by a timer's `throw') is
  ;; idle: the span before it ends there (60 ms), and the one after it
  ;; (60 ms) starts when it returns.  Counted as busy it would be 120 ms.
  (watchdog-test--running '(100 2000 nil :no-monitor)
    (watchdog-test--align)
    (watchdog-test--clock-at 60)
    (run-at-time 0.05 nil (lambda () (throw 'watchdog-test--done nil)))
    (catch 'watchdog-test--done
      (nidhogg-watchdog--test-idle-wait nil))
    (watchdog-test--clock-at 120)
    (watchdog-test--idle)
    (should-not (car (nidhogg-watchdog--take)))))

(ert-deftest watchdog/lisp-thread-select-never-counts ()
  ;; A Lisp thread's selects never touch the state, even its responsive
  ;; waits; the main thread waits in `thread-join' (busy, no select).
  (skip-unless (featurep 'threads))
  (watchdog-test--running '(100 2000 nil :no-monitor)
    (watchdog-test--align)
    (let ((spans (watchdog-test--stat :spans)))
      (thread-join
       (make-thread (lambda ()
                      (dotimes (_ 5) (sleep-for 0.01))
                      (dotimes (_ 5) (nidhogg-watchdog--test-idle-wait 0.01)))
                    "watchdog-test"))
      (should (= spans (watchdog-test--stat :spans))))))

(ert-deftest watchdog/start-stop-twice ()
  (unwind-protect
      (progn
        (should-error (nidhogg-watchdog--start 0 2000 nil))
        (should-not (nidhogg-watchdog--stats))
        (nidhogg-watchdog--start 100 2000 nil :no-monitor)
        (should-error (nidhogg-watchdog--start 100 2000 nil))
        (should (nidhogg-watchdog--stats))
        (nidhogg-watchdog--stop)
        (should-not (nidhogg-watchdog--stop))
        (should-not (nidhogg-watchdog--stats))
        (nidhogg-watchdog--start 100 2000 nil)
        (should (eql (watchdog-test--stat :spans) 0))
        (nidhogg-watchdog--stop)
        (should-not (nidhogg-watchdog--stats)))
    (nidhogg-watchdog--stop)))

;;; watchdog-tests.el ends here
