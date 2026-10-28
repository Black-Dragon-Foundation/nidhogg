;;; watchdog-gcrow-tests.el --- tests for the stall watchdog (gcrow)  -*- lexical-binding: t -*-

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

;; The GC row (lisp/nidhogg-watchdog-gc.el): the `post-gc-hook'
;; notes joined with the C GC ring on `gcs-done', the collection lines, and
;; the numbers `nidhogg-watchdog-gc-row' derives from the day files.  Logs
;; go to a temporary directory; automatic collections are kept out by
;; binding `gc-cons-threshold' high, except where a row wants one.

;;; Code:

(require 'ert)
(require 'cl-lib)

(defmacro watchdog-gcrow--with-log (&rest body)
  "Run BODY with a fresh log directory, fresh day counters and notes."
  (declare (indent 0))
  `(let* ((dir (make-temp-file "nh-wd-gcrow" t))
          (nidhogg-watchdog-log-directory (file-name-as-directory dir))
          (nidhogg-watchdog--day nil)
          (nidhogg-watchdog--day-dir nil)
          (nidhogg-watchdog--day-stalls 0)
          (nidhogg-watchdog--over-cap 0)
          (nidhogg-watchdog--queue nil)
          (nidhogg-watchdog--last-summary nil)
          (nidhogg-watchdog--drain-hist (make-vector 9 0))
          (nidhogg-watchdog--drain-take-hist (make-vector 9 0))
          (nidhogg-watchdog--drain-over 0)
          (nidhogg-watchdog--summary-base nil)
          (nidhogg-watchdog-record-functions nil)
          (nidhogg-watchdog--resolve-function nil))
     (nidhogg-watchdog-gc--reset)
     (unwind-protect (progn ,@body)
       (nidhogg-watchdog-gc--reset)
       (delete-directory dir t))))

(defmacro watchdog-gcrow--with-mode (&rest body)
  "Run BODY with `nidhogg-watchdog-mode' on; turn it off after."
  (declare (indent 0))
  `(unwind-protect
       (progn (nidhogg-watchdog-mode 1)
              (nidhogg-watchdog--take)
              ,@body)
     (when nidhogg-watchdog-mode (nidhogg-watchdog-mode -1))))

(defun watchdog-gcrow--lines ()
  "The collection lines of every day file, as alists, in file order."
  (let (out)
    (dolist (file (directory-files nidhogg-watchdog-log-directory t "\\.jsonl\\'"))
      (with-temp-buffer
        (insert-file-contents file)
        (dolist (l (split-string (buffer-string) "\n" t))
          (let ((o (json-parse-string l :object-type 'alist)))
            (when (equal (alist-get 'type o) "gc")
              (push o out))))))
    (nreverse out)))

(defun watchdog-gcrow--line (gcs)
  "The one collection line of GCS (its `gcs_done')."
  (let ((ls (delq nil (mapcar (lambda (l) (and (eql (alist-get 'gcs_done l) gcs) l))
                              (watchdog-gcrow--lines)))))
    (should (= (length ls) 1))
    (car ls)))

(defun watchdog-gcrow--near (a b)
  (and (numberp a) (numberp b) (< (abs (- a b)) 0.001)))

(defun watchdog-gcrow--force-auto-collection ()
  "Cons until Emacs collects by itself (not through `garbage-collect')."
  (let ((gc-cons-threshold 100000)
        (gc-cons-percentage 0.0)
        (n gcs-done)
        (junk nil))
    (while (= n gcs-done)
      (setq junk (make-list 1000 nil)))
    junk))

(defun watchdog-gcrow--heapsize (mb)
  "A `garbage-collect-heapsize' fixture: MB of live conses, nothing else."
  `((conses 16 ,(* mb 65536) 0) (symbols 48 0 0)))

(defun watchdog-gcrow--day (date pauses)
  "Write the day file DATE with one collection line per element of PAUSES.
Each is MS or (MS LIVE-MB FELT).  A stall line goes in too (skipped)."
  (with-temp-file (expand-file-name (concat date ".jsonl")
                                    nidhogg-watchdog-log-directory)
    (insert (json-serialize (list :v 1 :type "stall" :wall_ms 900.0)) "\n")
    (dolist (p pauses)
      (let ((ms (if (consp p) (nth 0 p) p))
            (live (and (consp p) (nth 1 p)))
            (felt (and (consp p) (nth 2 p))))
        (insert (json-serialize
                 (append (list :v 1 :type "gc"
                               :at (concat date "T12:00:00.000+00:00")
                               :pause_ms (float ms) :origin "auto"
                               :felt (if felt t :false))
                         (and live (list :heap (list :live_mb (float live)
                                                     :conses_mb (float live))))))
                "\n")))))

(ert-deftest watchdog/gcrow-collection-line ()
  ;; A forced collection with the mode on: one line, origin `explicit',
  ;; the C ring's exact pause, the heap from `garbage-collect-heapsize'.
  (watchdog-gcrow--with-log
    (let* ((gc-cons-threshold most-positive-fixnum)
           (taken nil) (heap nil) (gcs nil)
           (nidhogg-watchdog--collections-function
            (lambda (cs)
              (setq taken (append taken cs))
              (nidhogg-watchdog-gc--collections cs))))
      (watchdog-gcrow--with-mode
        ;; `garbage-collect' returns what `garbage-collect-heapsize' says.
        (setq heap (garbage-collect)
              gcs gcs-done)
        (nidhogg-watchdog-drain-now))
      (let ((c (car (delq nil (mapcar (lambda (c) (and (eql (plist-get c :gcs-done) gcs) c))
                                      taken))))
            (l (watchdog-gcrow--line gcs))
            (want (cdr (nidhogg-watchdog-gc--heap heap))))
        (should c)
        (should (equal (alist-get 'origin l) "explicit"))
        (should (> (alist-get 'pause_ms l) 0))
        (should (watchdog-gcrow--near (alist-get 'pause_ms l) (plist-get c :pause-ms)))
        (let ((h (alist-get 'heap l)))
          (should (> (alist-get 'live_mb h) 0))
          (should (= (alist-get 'live_mb h) (plist-get want :live-mb)))
          (should (= (alist-get 'free_mb h) (plist-get want :free-mb)))
          (should (= (alist-get 'conses_mb h) (plist-get want :conses-mb)))
          (should (= (alist-get 'vectors_mb h) (plist-get want :vectors-mb)))
          (should (= (alist-get 'string_bytes_mb h) (plist-get want :string-bytes-mb))))
        (should (numberp (alist-get 'threshold_mb l)))
        (should (or (eq (alist-get 'rss_mb l) :null) (> (alist-get 'rss_mb l) 0)))
        (should (= 0 nidhogg-watchdog-gc--c-only nidhogg-watchdog-gc--lisp-only))))))

(ert-deftest watchdog/gcrow-origin ()
  ;; The origin is the caller's binding of `nidhogg-watchdog-gc-origin';
  ;; an automatic collection with no binding is `auto'.
  (watchdog-gcrow--with-log
    (let ((gc-cons-threshold most-positive-fixnum)
          (seen nil))
      (watchdog-gcrow--with-mode
        (dolist (origin '(idle gap audit))
          (let ((nidhogg-watchdog-gc-origin origin))
            (garbage-collect))
          (push gcs-done seen))
        (watchdog-gcrow--force-auto-collection)
        (push gcs-done seen)
        (nidhogg-watchdog-drain-now))
      (should (equal (mapcar (lambda (gcs)
                               (alist-get 'origin (watchdog-gcrow--line gcs)))
                             (nreverse seen))
                     '("idle" "gap" "audit" "auto"))))))

(ert-deftest watchdog/gcrow-derived-numbers ()
  ;; Eight days of fixture lines: per-day numbers, and the 7 days ending
  ;; on the newest day leave the oldest out.
  (watchdog-gcrow--with-log
    (watchdog-gcrow--day "2026-10-01" (make-list 30 '(200 900 nil)))
    (watchdog-gcrow--day "2026-10-08" '((10 400 t) (20 410 nil) (30 420 nil) (120 430 t)))
    (let* ((row (nidhogg-watchdog-gc-row :days 8))
           (days (plist-get row :days))
           (d1 (nth 0 days))
           (d8 (nth 1 days))
           (week (plist-get row :week)))
      (should (= (length days) 2))
      (should (equal (plist-get d8 :date) "2026-10-08"))
      (should (= (plist-get d8 :count) 4))
      (should (watchdog-gcrow--near (plist-get d8 :total-s) 0.18))
      (should (= (plist-get d8 :max) 120.0))
      ;; Buckets <16: 1, <33: 2, <250: 1; linear inside a bucket, at most the max.
      (should (watchdog-gcrow--near (plist-get d8 :p50) 24.5))
      (should (= (plist-get d8 :p95) 120.0))
      (should (= (plist-get d8 :p99) 120.0))
      (should (= (plist-get d8 :felt) 2))
      (should (= (plist-get (plist-get d8 :heap) :live_mb) 430.0))
      (should (plist-get d8 :meets))
      ;; Day 1: its own 7 days hold 30 collections of 200 ms.
      (should (= (plist-get d1 :count) 30))
      (should (= (plist-get d1 :max) 200.0))
      (should (watchdog-gcrow--near (plist-get d1 :total-s) 6.0))
      (should-not (plist-get d1 :meets))
      ;; The week ends on 10-08: 10-01 is out.
      (should (= (plist-get week :count) 4))
      (should (= (plist-get week :max) 120.0))
      (should-not (plist-get week :p95))
      (should (= (plist-get (plist-get week :heap) :live_mb) 430.0))
      (should (plist-get row :meets))
      (should (= (plist-get row :target) nidhogg-watchdog-gc-target-p95))
      (should (= (plist-get row :gc-max) nidhogg-watchdog-gc-max))
      ;; :days counts day files from the newest.
      (should (equal (mapcar (lambda (d) (plist-get d :date))
                             (plist-get (nidhogg-watchdog-gc-row :days 1) :days))
                     '("2026-10-08"))))))

(ert-deftest watchdog/gcrow-p95-needs-20 ()
  (watchdog-gcrow--with-log
    (watchdog-gcrow--day "2026-10-08" (make-list 19 30))
    (let ((row (nidhogg-watchdog-gc-row)))
      (should (= (plist-get (plist-get row :week) :count) 19))
      (should-not (plist-get (plist-get row :week) :p95))
      ;; Under 20 the max alone judges the day.
      (should (plist-get row :meets)))
    (watchdog-gcrow--day "2026-10-08" (make-list 20 30))
    (let ((week (plist-get (nidhogg-watchdog-gc-row) :week)))
      (should (= (plist-get week :count) 20))
      (should (= (plist-get week :p95) 30.0)))))

(ert-deftest watchdog/gcrow-slope ()
  ;; Pause = 10 + 0.2 × live MB: 20 ms per 100 MB.  Each day's last live
  ;; heap grows 10 MB a day (a day without a file in between).
  (watchdog-gcrow--with-log
    (watchdog-gcrow--day "2026-10-01" '((110 500)))
    (should-not (plist-get (plist-get (nidhogg-watchdog-gc-row) :week) :slope))
    (should-not (plist-get (plist-get (nidhogg-watchdog-gc-row) :week) :heap-trend))
    (watchdog-gcrow--day "2026-10-02" '((112 510)))
    (watchdog-gcrow--day "2026-10-04" '((30 100) (50 200) (70 300) (90 400) (116 530)))
    (let ((week (plist-get (nidhogg-watchdog-gc-row) :week)))
      (should (= (plist-get week :points) 7))
      (should (watchdog-gcrow--near (plist-get week :slope) 20.0))
      (should (watchdog-gcrow--near (plist-get week :heap-trend) 10.0)))
    ;; A heap that never changes gives no slope.
    (dolist (f (directory-files nidhogg-watchdog-log-directory t "\\.jsonl\\'"))
      (delete-file f))
    (watchdog-gcrow--day "2026-10-04" '((30 100) (50 100)))
    (should-not (plist-get (plist-get (nidhogg-watchdog-gc-row) :week) :slope))))

(ert-deftest watchdog/gcrow-reads-each-day-once ()
  ;; The row reads only the newest 13 day files (7 rows, each over 7
  ;; days), and a file again only when it changed: an idle check that asks
  ;; for the row every half hour must not parse 14 days each time.
  (watchdog-gcrow--with-log
    (dotimes (i 14)
      (watchdog-gcrow--day (format "2026-10-%02d" (1+ i)) '(10 20)))
    (let* ((read nil)
           (orig (symbol-function 'nidhogg-watchdog-gc--read-day)))
      (cl-letf (((symbol-function 'nidhogg-watchdog-gc--read-day)
                 (lambda (f) (push (file-name-base f) read) (funcall orig f))))
        (should (= 14 (plist-get (plist-get (nidhogg-watchdog-gc-row) :week)
                                 :count)))
        (should (= 13 (length read)))
        (should-not (member "2026-10-01" read))
        (setq read nil)
        (should (= 14 (plist-get (plist-get (nidhogg-watchdog-gc-row) :week)
                                 :count)))
        (should-not read)
        ;; Today's file grows: only it is read again.
        (watchdog-gcrow--day "2026-10-14" '(10 20 30))
        (should (= 15 (plist-get (plist-get (nidhogg-watchdog-gc-row) :week)
                                 :count)))
        (should (equal read '("2026-10-14")))))))

(ert-deftest watchdog/gcrow-day-verdict ()
  ;; A day meets when the p95 of its 7 days is at most the target (or
  ;; unknown) and no collection in it reaches GC-MAX.
  (watchdog-gcrow--with-log
    (let ((nidhogg-watchdog-gc-target-p95 50)
          (nidhogg-watchdog-gc-max 250))
      (watchdog-gcrow--day "2026-10-01" (make-list 20 30))   ; p95 30, max 30
      (watchdog-gcrow--day "2026-10-02" '(260))              ; p95 ~33, max 260
      (watchdog-gcrow--day "2026-10-03" (make-list 25 120))  ; p95 ~242, max 120
      (watchdog-gcrow--day "2026-10-12" '(250))              ; p95 -, max = GC-MAX
      (let ((row (nidhogg-watchdog-gc-row)))
        (should (equal (mapcar (lambda (d) (list (plist-get d :date)
                                                 (and (plist-get d :meets) t)))
                               (plist-get row :days))
                       '(("2026-10-01" t) ("2026-10-02" nil)
                         ("2026-10-03" nil) ("2026-10-12" nil))))
        (should-not (plist-get row :meets)))
      ;; A higher target passes 10-03; GC-MAX still fails 10-02.
      (let* ((nidhogg-watchdog-gc-target-p95 300)
             (days (plist-get (nidhogg-watchdog-gc-row) :days)))
        (should (plist-get (nth 2 days) :meets))
        (should-not (plist-get (nth 1 days) :meets))))))

(ert-deftest watchdog/gcrow-hook-does-no-io ()
  ;; The `post-gc-hook' function only records: every file operation in it
  ;; would go through the catch-all handler and fail.
  (watchdog-gcrow--with-log
    (let ((ops nil)
          (gc-cons-threshold most-positive-fixnum))
      (let ((post-gc-hook (list #'nidhogg-watchdog-gc--note))
            (file-name-handler-alist
             (list (cons "" (lambda (op &rest _)
                              (push op ops)
                              (error "File I/O in the GC hook"))))))
        (garbage-collect)
        (nidhogg-watchdog-gc--note))
      (should-not ops)
      (should (= nidhogg-watchdog-gc--count 2))
      (let ((notes (nidhogg-watchdog-gc--take-notes most-positive-fixnum)))
        (should (= (length notes) 2))
        (should (eql (car (nth 1 notes)) gcs-done))
        (should (assq 'conses (plist-get (cdr (car notes)) :heap))))
      ;; Nothing was written.
      (should-not (directory-files nidhogg-watchdog-log-directory nil "\\.jsonl\\'")))))

(ert-deftest watchdog/gcrow-join-after-drop ()
  ;; Notes for 1, 2, 4, 5 (the hook's ring dropped 3); collections 1, 3, 4
  ;; (the C ring dropped 2; 5 is not taken yet).  Every heap lands on the
  ;; pause with its `gcs-done', never on its neighbour's.
  (watchdog-gcrow--with-log
    (dolist (n '((1 101 idle) (2 102 nil) (4 104 nil) (5 105 nil)))
      (nidhogg-watchdog-gc--push (nth 0 n) (watchdog-gcrow--heapsize (nth 1 n))
                                 (nth 2 n) 800000 0.1))
    (let ((now (float-time))
          (nidhogg-watchdog--queue
           (list (list :id 1 :span 7 :felt t :wall-ms 131.0 :nongc-ms 12.6))))
      (nidhogg-watchdog-gc--collections
       (list (list :gc 10 :gcs-done 1 :start now :pause-ms 11.0 :span 7
                   :explicit nil :lisp-thread nil)
             (list :gc 11 :gcs-done 3 :start now :pause-ms 13.0 :span 8
                   :explicit t :lisp-thread nil)
             (list :gc 12 :gcs-done 4 :start now :pause-ms 14.0 :span 9
                   :explicit nil :lisp-thread nil))))
    (let ((lines (watchdog-gcrow--lines)))
      (should (equal (mapcar (lambda (l) (alist-get 'gcs_done l)) lines) '(1 2 3 4))))
    (let ((l1 (watchdog-gcrow--line 1)))
      (should (= (alist-get 'pause_ms l1) 11.0))
      (should (= (alist-get 'live_mb (alist-get 'heap l1)) 101.0))
      (should (equal (alist-get 'origin l1) "idle"))
      ;; Its span is a waiting stall: felt, wall and non-GC time.
      (should (eq (alist-get 'felt l1) t))
      (should (= (alist-get 'span_wall_ms l1) 131.0))
      (should (= (alist-get 'span_nongc_ms l1) 12.6))
      (should (= (alist-get 'threshold_mb l1) 10.1)))
    (let ((l2 (watchdog-gcrow--line 2)))
      (should (eq (alist-get 'pause_ms l2) :null))
      (should (= (alist-get 'live_mb (alist-get 'heap l2)) 102.0)))
    (let ((l3 (watchdog-gcrow--line 3)))
      (should (= (alist-get 'pause_ms l3) 13.0))
      (should (eq (alist-get 'heap l3) :null))
      (should (equal (alist-get 'origin l3) "explicit"))
      ;; Its span made no stall: felt unknown.
      (should (eq (alist-get 'felt l3) :null)))
    (let ((l4 (watchdog-gcrow--line 4)))
      (should (= (alist-get 'pause_ms l4) 14.0))
      (should (= (alist-get 'live_mb (alist-get 'heap l4)) 104.0))
      (should (equal (alist-get 'origin l4) "auto")))
    (should (= nidhogg-watchdog-gc--c-only 1))
    (should (= nidhogg-watchdog-gc--lisp-only 1))
    ;; Note 5 waits for its collection.
    (should (= nidhogg-watchdog-gc--count 1))
    (should (eql (car (car (nidhogg-watchdog-gc--take-notes 5))) 5))))

;;; watchdog-gcrow-tests.el ends here
