;;; watchdog-report-tests.el --- tests for the stall watchdog (report)  -*- lexical-binding: t -*-
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

;;; Code:

(require 'ert)
(require 'cl-lib)

;; Fixture day files: JSON Lines as `nidhogg-watchdog--log-write' makes them.
;; Every line is on one fixed day, and the report's "today" is that day: a
;; run across midnight reads the same file it wrote.

(defconst watchdog-report--day-file "2026-03-01.jsonl"
  "The fixture's day file.")

(defconst watchdog-report--at "2026-03-01T12:00:00.000+00:00"
  "The time of every fixture line, noon UTC of that day.")

(defun watchdog-report--midnight ()
  "Midnight UTC of the fixture's day, the report's start of today.
In any time zone that is on the day before or the day itself, so the
day file is read, and noon UTC is after it."
  (encode-time '(0 0 0 1 3 2026 nil nil t)))

(defun watchdog-report--stall (pkg sig nongc felt &optional stacks)
  "A stall line of PKG and SIG with NONGC ms, FELT, and STACKS (frames, n)."
  (list :type "stall" :at watchdog-report--at
        :wall_ms nongc :nongc_ms nongc :gc_ms 0
        :felt (if felt t :false) :state "running"
        :culprit (list :pkg pkg) :signature sig
        :phases (list :eval 0.75 :redisplay 0.25)
        :stacks (vconcat (mapcar (lambda (s)
                                   (list :frames (vconcat (car s)) :n (cdr s)))
                                 stacks))))

(defun watchdog-report--gc (pause felt)
  "A GC line of PAUSE ms."
  (list :type "gc" :at watchdog-report--at
        :pause_ms pause :felt (if felt t :false) :origin "auto"))

(defmacro watchdog-report--with-day (lines &rest body)
  "Run BODY with a log directory whose today file holds LINES (plists)."
  (declare (indent 1))
  `(let* ((dir (make-temp-file "nh-wd-rep" t))
          (nidhogg-watchdog-log-directory (file-name-as-directory dir))
          (nidhogg-watchdog-threshold 100))
     (unwind-protect
         (progn
           (with-temp-file (expand-file-name watchdog-report--day-file dir)
             (dolist (l ,lines)
               (insert (json-serialize l :false-object :false) "\n")))
           ,@body)
       (delete-directory dir t))))

(defun watchdog-report--day ()
  "A day: B 400 ms once, A 150 ms twice, C under the threshold, two GCs."
  (list (watchdog-report--stall "pkg-a" "sig-a" 150 nil
                                '((("c" "b" "a") . 2)))
        (watchdog-report--stall "pkg-a" "sig-a" 150 t
                                '((("d" "b" "a") . 1)))
        (watchdog-report--stall "pkg-b" "sig-b" 400 t)
        (watchdog-report--stall "pkg-c" "sig-c" 90 t)
        (watchdog-report--gc 30.0 t)
        (watchdog-report--gc 10.0 nil)))

(ert-deftest watchdog/ranking-excess-order ()
  (watchdog-report--with-day (watchdog-report--day)
    (let ((rows (cdr (nidhogg-watchdog-ranking))))
      (should (equal (mapcar (lambda (r) (plist-get r :package)) rows)
                     '("pkg-b" "pkg-a")))
      (should (= (plist-get (car rows) :score) 300))
      (should (= (plist-get (cadr rows) :score) 100))
      (should (equal (mapcar (lambda (s) (plist-get s :name))
                             (plist-get (cadr rows) :signatures))
                     '("sig-a"))))))

(ert-deftest watchdog/ranking-felt-is-a-column ()
  (watchdog-report--with-day (watchdog-report--day)
    (let ((rows (cdr (nidhogg-watchdog-ranking))))
      (should (= (plist-get (car rows) :felt) 1))
      (should (= (plist-get (cadr rows) :count) 2))
      (should (= (plist-get (cadr rows) :felt) 1)))
    (let ((rows (cdr (nidhogg-watchdog-ranking :felt-only t))))
      (should (= (plist-get (cadr rows) :count) 1))
      (should (= (plist-get (nth 0 (nidhogg-watchdog-ranking :felt-only t))
                            :count)
                 1)))))

(ert-deftest watchdog/ranking-gc-row-on-top ()
  (watchdog-report--with-day (watchdog-report--day)
    (let ((gc (car (nidhogg-watchdog-ranking))))
      (should (plist-get gc :gc))
      (should (= (plist-get gc :count) 2))
      (should (= (plist-get gc :max) 30.0))
      (should (= (length (plist-get gc :collections)) 2)))))

(ert-deftest watchdog/ranking-merged-call-tree ()
  (watchdog-report--with-day (watchdog-report--day)
    ;; The GC row, pkg-b (score 300), then pkg-a (score 100).
    (let* ((row (nth 2 (nidhogg-watchdog-ranking)))
           (text (nidhogg-watchdog--rep-tree-string (plist-get row :tree))))
      (should (equal (plist-get row :package) "pkg-a"))
      (should (equal text "3 a\n  3 b\n    2 c\n    1 d\n")))))

(ert-deftest watchdog/report-header ()
  (watchdog-report--with-day (watchdog-report--day)
    (should (equal (nidhogg-watchdog--report-header
                    (nidhogg-watchdog--report-lines nil))
                   "4 stalls over 100 ms, 3 felt, longest 400 ms; GC: 2 collections, max 30 ms; typing ≥ 16 ms: n/a"))))

(ert-deftest watchdog/report-rows ()
  (watchdog-report--with-day (watchdog-report--day)
    (unwind-protect
        (cl-letf (((symbol-function 'nidhogg-watchdog--rep-midnight)
                   #'watchdog-report--midnight))
          (save-window-excursion
            (nidhogg-watchdog-report)
            (with-current-buffer "*Nidhogg Watchdog*"
              (should (derived-mode-p 'nidhogg-watchdog-report-mode))
              ;; GC row, pkg-b, its signature, pkg-a, its signature.
              (should (= (length tabulated-list-entries) 5))
              (should (plist-get (car (car tabulated-list-entries)) :gc))
              (should (string-prefix-p
                       "pkg-b" (aref (cadr (nth 1 tabulated-list-entries)) 0)))
              (should (string-prefix-p
                       "  sig-b" (aref (cadr (nth 2 tabulated-list-entries)) 0))))))
      (when (get-buffer "*Nidhogg Watchdog*")
        (kill-buffer "*Nidhogg Watchdog*")))))

;;; watchdog-report-tests.el ends here
