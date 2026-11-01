;;; test-clock-tests.el --- tests for the test clock  -*- lexical-binding: t -*-
;; test-class: det

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

;; Time that moves only when told, or only when nothing can run.

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'nidhogg-test-peer)

(defmacro test-clock-tests--with (auto &rest body)
  "Run BODY on the test clock, auto-advancing if AUTO."
  (declare (indent 1) (debug t))
  `(progn
     (skip-unless (featurep 'nidhogg-test-clock))
     (let ((process-environment
            (cons "NIDHOGG_TEST_CLOCK=1" process-environment)))
       (nidhogg-test-clock-start ,auto)
       (unwind-protect (progn ,@body)
         (nidhogg-test-clock-stop)))))

(defun test-clock-tests--emacs ()
  "The Emacs under test."
  (expand-file-name invocation-name invocation-directory))

(defun test-clock-tests--child (form)
  "Return the standard output of a batch Emacs that evaluates FORM."
  (with-temp-buffer
    (should (eql 0 (call-process (test-clock-tests--emacs) nil '(t nil) nil
                                 "-Q" "--batch" "--eval" (prin1-to-string form))))
    (buffer-string)))

(ert-deftest test-clock/current-time-moves-only-when-told ()
  (test-clock-tests--with nil
    (let ((before (current-time)))
      (dotimes (_ 100000) (ignore))
      (should (time-equal-p before (current-time)))
      (nidhogg-test-clock-advance 5)
      (should (time-equal-p (time-add before 5) (current-time)))
      (should (= 5.0 (nidhogg-test-clock-now))))))

(ert-deftest test-clock/timer-fires-on-advance ()
  (test-clock-tests--with nil
    (let* ((fired nil)
           (note (lambda (n) (push (cons n (nidhogg-test-clock-now)) fired)))
           (late (run-at-time 3 nil note 'late))
           (early (run-at-time 1 nil note 'early)))
      (unwind-protect
          (progn
            (nidhogg-test-clock-advance 0.5)
            (should-not fired)
            (nidhogg-test-clock-advance 4.5)
            (should (equal (reverse fired) '((early . 1.0) (late . 3.0))))
            (should (= 5.0 (nidhogg-test-clock-now))))
        (cancel-timer late)
        (cancel-timer early)))))

(ert-deftest test-clock/wall-and-monotonic-move-in-step ()
  (test-clock-tests--with t
    (let ((before (nidhogg-test-clock--readings)))
      (nidhogg-test-clock-advance 5)
      (should (equal (cl-mapcar #'- (nidhogg-test-clock--readings) before)
                     '(5000000000 5000000000 5000000000))))))

(ert-deftest test-clock/file-times-stay-real ()
  (test-clock-tests--with nil
    (let ((file (make-temp-file "test-clock")))
      (unwind-protect
          (progn
            (nidhogg-test-clock-advance 100000)
            (write-region "x" nil file nil 'silent)
            (should (time-less-p
                     (file-attribute-modification-time (file-attributes file))
                     (time-subtract (current-time) 99000))))
        (delete-file file)))))

(ert-deftest test-clock/env-alone-changes-nothing ()
  :tags '(:cond)
  (skip-unless (featurep 'nidhogg-test-clock))
  (let ((process-environment
         (cons "NIDHOGG_TEST_CLOCK=1" process-environment)))
    (should (equal "nil t"
                   (test-clock-tests--child
                    '(let ((before (float-time)))
                       (princ (nidhogg-test-clock-active-p))
                       (sleep-for 0.1)
                       (princ " ")
                       (princ (>= (- (float-time) before) 0.09))))))))

(ert-deftest test-clock/start-refused-without-the-env ()
  (skip-unless (featurep 'nidhogg-test-clock))
  (let ((process-environment (copy-sequence process-environment)))
    (setenv "NIDHOGG_TEST_CLOCK" nil)
    (should-error (nidhogg-test-clock-start))
    (setenv "NIDHOGG_TEST_CLOCK" "0")
    (should-error (nidhogg-test-clock-start))
    (should-not (nidhogg-test-clock-active-p))))

(defun test-clock-tests--blocks ()
  "How many selects blocked for real since the clock started."
  (plist-get (nidhogg-test-clock-state) :blocks))

(ert-deftest test-clock/auto-runs-a-timer-without-waiting ()
  (test-clock-tests--with t
    (let* ((at nil)
           (timer (run-at-time 3600 nil
                               (lambda () (setq at (nidhogg-test-clock-now))))))
      (unwind-protect
          (progn
            (sit-for 7200)
            (should (= 3600.0 at))
            (should (= 7200.0 (nidhogg-test-clock-now)))
            (should (= 0 (test-clock-tests--blocks))))
        (cancel-timer timer)))))

(ert-deftest test-clock/sit-for-blocks-nothing ()
  (test-clock-tests--with t
    (sit-for 86400)
    (should (= 86400.0 (nidhogg-test-clock-now)))
    (should (= 0 (test-clock-tests--blocks)))))

(ert-deftest test-clock/a-real-child-holds-the-clock-and-is-named ()
  :tags '(:cond)
  (test-clock-tests--with t
    (let ((proc (make-process :name "test-clock-child" :noquery t
                              :connection-type 'pipe
                              :command '("sleep" "0.2"))))
      (should (equal '("test-clock-child")
                     (plist-get (nidhogg-test-clock-state) :children)))
      (sit-for 1)
      ;; The clock could not move while it lived.
      (should-not (process-live-p proc))
      (should (< 0 (test-clock-tests--blocks)))
      (should (= 1.0 (nidhogg-test-clock-now))))))

(ert-deftest test-clock/a-peer-does-not-hold-the-clock ()
  (test-clock-tests--with t
    (let ((peer (nidhogg-test-peer-make :name "test-clock-peer")))
      (unwind-protect
          (progn
            (should-not (plist-get (nidhogg-test-clock-state) :children))
            (sit-for 5)
            (should (<= 5.0 (nidhogg-test-clock-now))))
        (nidhogg-test-peer-close peer)))))

(ert-deftest test-clock/deadlock-reports-the-state ()
  (test-clock-tests--with t
    (let ((peer (nidhogg-test-peer-make :name "test-clock-stuck"))
          (timer-list nil)
          (timer-idle-list nil))
      (unwind-protect
          (let ((err (should-error (accept-process-output peer)
                                   :type 'nidhogg-test-clock-deadlock)))
            (should (equal 0 (plist-get (cdr err) :timers)))
            (should (plist-member (cdr err) :jobs))
            (should (plist-member (cdr err) :fds))
            (should (plist-member (cdr err) :children)))
        (nidhogg-test-peer-close peer)))))

(ert-deftest test-clock/job-without-finish-wakes ()
  (test-clock-tests--with t
    (let ((job (nidhogg-workers--test-job 'nh-parse 30 :no-finish t)))
      (should job)
      (sit-for 1)
      (should (= 1 (nidhogg-workers--test-wait job 0)))
      (should (= 1.0 (nidhogg-test-clock-now))))))

(ert-deftest test-clock/cancelled-job-wakes ()
  (test-clock-tests--with t
    (let ((job (nidhogg-workers--test-job 'nh-parse 60000)))
      (should job)
      (nidhogg-workers--test-cancel job)
      (sit-for 1)
      (should (= 1.0 (nidhogg-test-clock-now)))
      (should (nidhogg-workers--test-await-landed 'nh-parse 5000)))))

(ert-deftest test-clock/pause-point-wakes-and-counts-idle ()
  (test-clock-tests--with t
    (nidhogg-workers--pause-at 'after-snapshot)
    (let ((job (nidhogg-workers--test-job 'nh-parse 0 :pause 'after-snapshot)))
      (unwind-protect
          (progn
            (should job)
            ;; The wait is under way when the worker parks, or after.
            (sit-for 10)
            (should (= 10.0 (nidhogg-test-clock-now)))
            (should (nidhogg-workers--test-await-paused 'after-snapshot 5000)))
        (nidhogg-workers--release 'after-snapshot))
      (should (nidhogg-workers--test-await-landed 'nh-parse 5000)))))

(defmacro test-clock-tests--with-held-output (point &rest body)
  "Run BODY with a peer's bytes held back by nh-io's pause POINT.
BODY sees `peer', `calls' (a list of the clock's readings at each
filter call) and the bytes on their way."
  (declare (indent 1) (debug t))
  `(test-clock-tests--with t
     (let* ((nidhogg-workers--mode 'threads)
            (nidhogg-workers-process-reader t)
            (nidhogg-workers-reader-sockets t)
            (calls nil)
            (peer (nidhogg-test-peer-make :name "test-clock-held")))
       (unwind-protect
           (progn
             (should (plist-get (nidhogg-workers--test-io-info peer) :owned))
             ;; Not `nidhogg-test-peer-write': it waits for the filter.
             (set-process-filter peer (lambda (_p _s)
                                        (push (nidhogg-test-clock-now) calls)))
             (nidhogg-workers--pause-at ,point)
             (process-send-string
              (nidhogg-test-peer--state-client
               (process-get peer 'nidhogg-test-peer))
              "x")
             (should (nidhogg-workers--test-await-paused ,point 5000))
             ,@body)
         (nidhogg-workers--release ,point)
         (nidhogg-test-peer-close peer)))))

(defmacro test-clock-tests--landed-before-time-moved (calls)
  "Wait 10 s and check that the one filter call in CALLS came first."
  `(let ((start (nidhogg-test-clock-now)))
     (sit-for 10)
     (should (equal (list start) ,calls))
     (should (< (abs (- (nidhogg-test-clock-now) start 10)) 1e-6))))

(ert-deftest test-clock/bytes-in-the-kernel-hold-the-clock ()
  (test-clock-tests--with-held-output 'in-poll
    (should-not (nidhogg-test-clock--quiet-p))
    (should (plist-get (nidhogg-test-clock-state) :fds))
    (nidhogg-workers--release 'in-poll)
    (test-clock-tests--landed-before-time-moved calls)))

(ert-deftest test-clock/delivered-unread-output-holds-the-clock ()
  (test-clock-tests--with-held-output 'before-decode
    (should-not (nidhogg-test-clock--quiet-p))
    (should (plist-get (nidhogg-test-clock-state) :fds))
    (nidhogg-workers--release 'before-decode)
    (test-clock-tests--landed-before-time-moved calls)))

(ert-deftest test-clock/runnable-lisp-thread-holds-the-clock ()
  (test-clock-tests--with t
    (let* ((at nil)
           (slept nil)
           (runner (make-thread
                    (lambda ()
                      (dotimes (_ 3) (thread-yield))
                      (setq at (nidhogg-test-clock-now)))
                    "test-clock-runner"))
           (sleeper (make-thread
                     (lambda ()
                       (sleep-for 5)
                       (setq slept (nidhogg-test-clock-now)))
                     "test-clock-sleeper")))
      (sit-for 100)
      (should (= 0.0 at))
      (should (= 5.0 slept))
      (should (= 100.0 (nidhogg-test-clock-now)))
      (thread-join runner)
      (thread-join sleeper))))

(ert-deftest test-clock/child-emacs-on-its-own-clock ()
  :tags '(:cond)
  (test-clock-tests--with nil
    (should (equal "100.0"
                   (test-clock-tests--child
                    '(progn
                       (nidhogg-test-clock-start t)
                       (sit-for 100)
                       (princ (nidhogg-test-clock-now))))))
    (should (= 0.0 (nidhogg-test-clock-now)))))

;;; test-clock-tests.el ends here
