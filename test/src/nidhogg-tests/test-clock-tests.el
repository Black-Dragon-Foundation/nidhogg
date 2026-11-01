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

;;; test-clock-tests.el ends here
