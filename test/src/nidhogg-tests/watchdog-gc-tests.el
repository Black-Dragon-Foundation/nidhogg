;;; watchdog-gc-tests.el --- tests for the stall watchdog (gc)  -*- lexical-binding: t -*-

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

;; The GC end hook (one line at the end of `garbage_collect') and the
;; GC ring (src/nidhogg-watchdog-gc.c): every collection's exact pause,
;; the span it fell in, Emacs's `gcs-done' after it (the key the Lisp
;; GC row and drgnx's census join on), and each span's GC time.
;;
;; Automatic collections are kept out of the rows that count entries by
;; binding `gc-cons-threshold' high; the rows that want one lower it.

;;; Code:

(require 'ert)

(defmacro watchdog-gc--with-mode (threshold-ms &rest body)
  "Run BODY with the watchdog on at THRESHOLD-MS, no monitor thread.
The rings are emptied first and after; the watchdog is stopped after."
  (declare (indent 1))
  `(let ((hang-file (make-temp-file "nh-wd-hang")))
     (unwind-protect
         (progn
           (nidhogg-watchdog--start ,threshold-ms 600000 hang-file :no-monitor)
           (nidhogg-watchdog--take)
           ,@body)
       (ignore-errors (nidhogg-watchdog--take))
       (ignore-errors (nidhogg-watchdog--stop))
       (ignore-errors (delete-file hang-file)))))

(defun watchdog-gc--collections ()
  "The collections the watchdog has recorded since the last take."
  (cdr (nidhogg-watchdog--take)))

(defun watchdog-gc--spin (ms)
  "Busy-loop for MS milliseconds without any wait."
  (let ((end (+ (float-time) (/ ms 1000.0))))
    (while (< (float-time) end))))

(defun watchdog-gc--force-auto-collection ()
  "Cons until Emacs collects by itself (not through `garbage-collect')."
  (let ((gc-cons-threshold 100000)
        (gc-cons-percentage 0.0)
        (n gcs-done)
        (junk nil))
    (while (= n gcs-done)
      (setq junk (make-list 1000 nil)))
    junk))

(ert-deftest watchdog/gc-ring-entries ()
  (let ((gc-cons-threshold most-positive-fixnum))
    (watchdog-gc--with-mode 100
      (dotimes (_ 3) (garbage-collect))
      (let ((cs (watchdog-gc--collections)))
        (should (= (length cs) 3))
        (dolist (c cs)
          (should (> (plist-get c :pause-ms) 0))
          (should (natnump (plist-get c :span)))
          (should (floatp (plist-get c :start)))
          (should (eq (plist-get c :explicit) t))
          (should-not (plist-get c :lisp-thread)))
        ;; Oldest first, ids and starts increasing.
        (should (< (plist-get (nth 0 cs) :gc) (plist-get (nth 1 cs) :gc)
                   (plist-get (nth 2 cs) :gc)))
        (should (<= (plist-get (nth 0 cs) :start) (plist-get (nth 1 cs) :start)
                    (plist-get (nth 2 cs) :start)))
        ;; The epoch, like a stall's :start.
        (should (< (abs (- (plist-get (nth 2 cs) :start) (float-time))) 60)))
      ;; `--take' empties the ring.
      (should-not (watchdog-gc--collections)))))

(ert-deftest watchdog/gc-ring-overflow ()
  (let ((gc-cons-threshold most-positive-fixnum))
    (watchdog-gc--with-mode 100
      (let ((dropped (plist-get (nidhogg-watchdog--stats) :gc-dropped)))
        (dotimes (_ 65) (garbage-collect))
        (let ((cs (watchdog-gc--collections)))
          (should (= (length cs) 64))
          ;; The newest are kept.
          (should (= (plist-get (car (last cs)) :gcs-done) gcs-done))
          (should (= (plist-get (car cs) :gcs-done) (- gcs-done 63))))
        (should (= (plist-get (nidhogg-watchdog--stats) :gc-dropped)
                   (1+ dropped)))))))

(ert-deftest watchdog/gc-ring-reset-at-start ()
  ;; Collections a run left untaken are not the next run's.
  (let ((gc-cons-threshold most-positive-fixnum))
    (watchdog-gc--with-mode 100
      (garbage-collect)
      (nidhogg-watchdog--stop)
      (nidhogg-watchdog--start 100 600000 nil :no-monitor)
      (should-not (cdr (nidhogg-watchdog--take))))))

(ert-deftest watchdog/gc-hook-off-costs-nothing ()
  (let ((gc-cons-threshold most-positive-fixnum))
    ;; Empty whatever an earlier row left, then collect with the mode off.
    (watchdog-gc--with-mode 100)
    (garbage-collect)
    (watchdog-gc--with-mode 100
      (should-not (watchdog-gc--collections)))))

(ert-deftest watchdog/gc-explicit-cleared-while-off ()
  (let ((gc-cons-threshold most-positive-fixnum))
    ;; `garbage-collect' marks its collection explicit; with the mode off
    ;; the mark must still be cleared, or it would tag the next one.
    (garbage-collect)
    (watchdog-gc--with-mode 100
      (watchdog-gc--force-auto-collection)
      (let ((cs (watchdog-gc--collections)))
        (should (= (length cs) 1))
        (should (eq (plist-get (car cs) :explicit) nil))))))

(ert-deftest watchdog/gc-gcs-done ()
  ;; Each entry's :gcs-done is the `gcs-done' a `post-gc-hook' function
  ;; sees for the same collection: the join key of the Lisp GC row and
  ;; of drgnx's census lines (their `gcs_done').
  (let ((gc-cons-threshold most-positive-fixnum)
        (seen nil))
    (let ((note (lambda () (push gcs-done seen))))
      (add-hook 'post-gc-hook note)
      (unwind-protect
          (watchdog-gc--with-mode 100
            (setq seen nil)
            (garbage-collect)
            (watchdog-gc--force-auto-collection)
            (garbage-collect)
            (let ((cs (watchdog-gc--collections)))
              (should (= (length cs) 3))
              (should (equal (mapcar (lambda (c) (plist-get c :gcs-done)) cs)
                             (reverse seen)))
              (should (equal (mapcar (lambda (c) (plist-get c :explicit)) cs)
                             '(t nil t)))))
        (remove-hook 'post-gc-hook note)))))

(ert-deftest watchdog/gc-not-in-goal ()
  ;; Virtual clock, no monitor: a 150 ms span with 120 ms of GC has
  ;; 30 ms of non-GC time (the GC row's, not the 100 ms goal's); one
  ;; with 40 ms of GC has 110 ms against the goal.
  (watchdog-gc--with-mode 100
    (unwind-protect
        (let ((t0 1000000000))
          (nidhogg-watchdog--test-clock t0)
          (nidhogg-watchdog--test-idle-wait 0.001)
          (nidhogg-watchdog--take)
          (pcase-dolist (`(,gc-ms ,nongc-ms ,bucket) '((120 30 3) (40 110 6)))
            (nidhogg-watchdog--stats t)
            (setq t0 (+ t0 150000000))
            (nidhogg-watchdog--test-gc-ns (* gc-ms 1000000))
            (nidhogg-watchdog--test-clock t0)
            (nidhogg-watchdog--test-idle-wait 0.001)
            (let ((stalls (car (nidhogg-watchdog--take))))
              (should (= (length stalls) 1))
              (let ((s (car stalls)))
                (should (= (plist-get s :wall-ms) 150.0))
                (should (= (plist-get s :gc-ms) (float gc-ms)))
                (should (= (plist-get s :gcs) 1))
                (should (= (plist-get s :nongc-ms) (float nongc-ms)))))
            ;; The span histogram counts the non-GC time.  Bucket 0 also
            ;; holds the idle wait's own zero-length spans (it may select
            ;; more than once, and the virtual clock stands still).
            (let* ((stats (nidhogg-watchdog--stats))
                   (other (plist-get (plist-get stats :histograms) :other)))
              (should (= (aref other bucket) 1))
              (should (= (apply #'+ (cdr (append other nil))) 1))
              (should (= (plist-get stats :requests) 0)))))
      (nidhogg-watchdog--test-clock nil))))

(ert-deftest watchdog/gc-split ()
  ;; Threshold 1 ms; a timer forces a collection on a 2M-cons heap
  ;; and spins.  Its span is a record with the collection's exact pause
  ;; as its GC time, and the collection names that pause.
  (let ((gc-cons-threshold most-positive-fixnum)
        (heap (make-list 2000000 nil)))
    (watchdog-gc--with-mode 1
      (run-at-time 0.01 nil (lambda () (garbage-collect) (watchdog-gc--spin 20)))
      (nidhogg-watchdog--test-idle-wait 0.3)
      (let* ((taken (nidhogg-watchdog--take))
             (with-gc (delq nil (mapcar (lambda (s) (and (> (plist-get s :gcs) 0) s))
                                        (car taken))))
             (cs (cdr taken)))
        (should (= (length with-gc) 1))
        (should (= (length cs) 1))
        (let ((s (car with-gc)) (c (car cs)))
          (should (= (plist-get s :gcs) 1))
          (should (> (plist-get s :gc-ms) 0))
          (should (>= (plist-get s :wall-ms) (+ (plist-get s :gc-ms) 20)))
          (should (< (abs (- (plist-get s :nongc-ms)
                             (- (plist-get s :wall-ms) (plist-get s :gc-ms))))
                     0.001))
          (should (< (abs (- (plist-get c :pause-ms) (plist-get s :gc-ms))) 0.001))
          (should (eq (plist-get c :explicit) t)))))
    (should (= (length heap) 2000000))))

;;; watchdog-gc-tests.el ends here
