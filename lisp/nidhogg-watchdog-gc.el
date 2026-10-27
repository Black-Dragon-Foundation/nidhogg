;;; nidhogg-watchdog-gc.el --- Nidhogg's stall watchdog: the GC row  -*- lexical-binding: t -*-

;; Copyright (C) 2026 Black Dragon Foundation contributors.

;; This file is part of Nidhogg.

;; Nidhogg is free software: you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or (at
;; your option) any later version.

;; Nidhogg is distributed in the hope that it will be useful,
;; but WITHOUT ANY WARRANTY; without even the implied warranty of
;; MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
;; GNU General Public License for more details.

;; You should have received a copy of the GNU General Public License
;; along with Nidhogg.  If not, see <https://www.gnu.org/licenses/>.

;;; Commentary:

;; The GC row: every collection with its live heap, judged against its own
;; target (GC pauses are not stalls against the 100 ms goal).
;;
;; Two sides meet at drain time.  While `nidhogg-watchdog-mode' is on, a
;; `post-gc-hook' function records numbers only into a fixed ring of notes:
;; `gcs-done', `garbage-collect-heapsize', the origin (the binding of
;; `nidhogg-watchdog-gc-origin') and the threshold.  The C side's GC ring
;; holds each collection's exact pause and span.  The drain hands the taken
;; collections to `nidhogg-watchdog-gc--collections', which joins the two
;; on `gcs-done' (never by order: either ring may have dropped entries),
;; adds the span's felt, wall and non-GC time when that span is a stall,
;; and writes one collection line each.  An entry without a partner is
;; written with the fields it has and counted.
;;
;; `nidhogg-watchdog-gc-row' reads the collection lines back from the day
;; files (drgnx's GC census writes the same lines) and derives the numbers.
;; It reads only the files its rows need, and each of those only when it
;; changed since the last call.

;;; Code:

(defvar nidhogg-watchdog-mode)
(defvar nidhogg-watchdog-gc-origin)
(defvar nidhogg-watchdog-log-directory)
(defvar nidhogg-watchdog--queue)
(defvar nidhogg-watchdog--bucket-edges)
(defvar nidhogg-watchdog--collections-function)
(declare-function nidhogg-watchdog--log-write "nidhogg-watchdog"
                  (type plist &optional time))
(declare-function nidhogg-watchdog--bool "nidhogg-watchdog" (x))
(declare-function nidhogg-watchdog--bucket "nidhogg-watchdog" (ms &optional edges))

(defcustom nidhogg-watchdog-gc-target-p95 50
  "The GC row's target: the 95th percentile pause (ms) over 7 days.
It judges only when the 7 days hold at least 20 collections."
  :type 'number
  :group 'nidhogg-watchdog)

(defcustom nidhogg-watchdog-gc-max 250
  "A day with a collection of this many milliseconds or more misses."
  :type 'number
  :group 'nidhogg-watchdog)

;;; The notes (the hook's side)

(defconst nidhogg-watchdog-gc--ring-size 64
  "Notes the ring holds between two drains; more are counted as dropped.")

(defun nidhogg-watchdog-gc--make-ring ()
  "A ring of empty slots, [GCS HEAP ORIGIN THRESHOLD PERCENTAGE] each."
  (let ((ring (make-vector nidhogg-watchdog-gc--ring-size nil)))
    (dotimes (i nidhogg-watchdog-gc--ring-size)
      (aset ring i (make-vector 5 nil)))
    ring))

(defvar nidhogg-watchdog-gc--ring (nidhogg-watchdog-gc--make-ring)
  "The notes, made once so that a note fills a slot and allocates nothing.")
(defvar nidhogg-watchdog-gc--start 0 "The index of the oldest note.")
(defvar nidhogg-watchdog-gc--count 0 "How many notes the ring holds.")
(defvar nidhogg-watchdog-gc--dropped 0 "Notes not kept: the ring was full.")
(defvar nidhogg-watchdog-gc--c-only 0
  "Collections written without a note (no heap, origin from the C side).")
(defvar nidhogg-watchdog-gc--lisp-only 0
  "Notes written without a collection (no pause): the C ring dropped it.")

(defun nidhogg-watchdog-gc--reset ()
  "Empty the ring and zero the counters."
  (dotimes (i nidhogg-watchdog-gc--ring-size)
    (fillarray (aref nidhogg-watchdog-gc--ring i) nil))
  (setq nidhogg-watchdog-gc--start 0
        nidhogg-watchdog-gc--count 0
        nidhogg-watchdog-gc--dropped 0
        nidhogg-watchdog-gc--c-only 0
        nidhogg-watchdog-gc--lisp-only 0))

(defun nidhogg-watchdog-gc--push (gcs heap origin threshold percentage)
  "Note collection GCS with HEAP, ORIGIN, THRESHOLD and PERCENTAGE.
A full ring keeps what it has and counts the new note as dropped."
  (if (>= nidhogg-watchdog-gc--count nidhogg-watchdog-gc--ring-size)
      (setq nidhogg-watchdog-gc--dropped (1+ nidhogg-watchdog-gc--dropped))
    (let ((slot (aref nidhogg-watchdog-gc--ring
                      (% (+ nidhogg-watchdog-gc--start nidhogg-watchdog-gc--count)
                         nidhogg-watchdog-gc--ring-size))))
      (aset slot 0 gcs)
      (aset slot 1 heap)
      (aset slot 2 origin)
      (aset slot 3 threshold)
      (aset slot 4 percentage)
      (setq nidhogg-watchdog-gc--count (1+ nidhogg-watchdog-gc--count)))))

(defun nidhogg-watchdog-gc--note ()
  "The `post-gc-hook' function: note this collection in the ring.
Numbers only, for it runs inside every collection: no file I/O, no
`process-attributes', no message.  It allocates only the small list of
`garbage-collect-heapsize'."
  (nidhogg-watchdog-gc--push gcs-done (garbage-collect-heapsize)
                             nidhogg-watchdog-gc-origin
                             gc-cons-threshold gc-cons-percentage))

(defun nidhogg-watchdog-gc--take-notes (newest)
  "Take the notes of collections up to NEWEST (a `gcs-done') out of the ring.
Return them oldest first as (GCS . (:heap H :origin O :threshold T
:percentage P)).  Newer notes stay: their collections are not taken yet."
  (let ((k 0) (out nil))
    (while (and (< k nidhogg-watchdog-gc--count)
                (<= (aref (aref nidhogg-watchdog-gc--ring
                                (% (+ nidhogg-watchdog-gc--start k)
                                   nidhogg-watchdog-gc--ring-size))
                          0)
                    newest))
      (setq k (1+ k)))
    (dotimes (i k)
      (let ((s (aref nidhogg-watchdog-gc--ring
                     (% (+ nidhogg-watchdog-gc--start i)
                        nidhogg-watchdog-gc--ring-size))))
        (push (cons (aref s 0)
                    (list :heap (aref s 1) :origin (aref s 2)
                          :threshold (aref s 3) :percentage (aref s 4)))
              out)))
    ;; A collection meanwhile only appends behind the K taken slots.
    (dotimes (i k)
      (aset (aref nidhogg-watchdog-gc--ring
                  (% (+ nidhogg-watchdog-gc--start i)
                     nidhogg-watchdog-gc--ring-size))
            1 nil))
    (setq nidhogg-watchdog-gc--start (% (+ nidhogg-watchdog-gc--start k)
                                        nidhogg-watchdog-gc--ring-size)
          nidhogg-watchdog-gc--count (- nidhogg-watchdog-gc--count k))
    (nreverse out)))

;;; The collection lines (the drain's side)

(defconst nidhogg-watchdog-gc--heap-types
  '((conses . :conses-mb) (strings . :strings-mb)
    (string-bytes . :string-bytes-mb) (vectors . :vectors-mb)
    (vector-slots . :vectors-mb) (symbols . :symbols-mb)
    (intervals . :intervals-mb) (floats . :floats-mb)
    (buffers . :buffers-mb))
  "`garbage-collect-heapsize' types and the line's heap keys.
Vector slots count with vectors; the `heap' entry (malloc's arena, not
the Lisp heap) is left out.")

(defun nidhogg-watchdog-gc--mb (bytes)
  "BYTES in MB (MiB), to 0.1."
  (/ (fround (/ bytes 104857.6)) 10.0))

(defun nidhogg-watchdog-gc--heap (heapsize)
  "HEAPSIZE (`garbage-collect-heapsize') as (LIVE-BYTES . HEAP).
HEAP is a plist of MB: :live-mb and :free-mb first, then by type.  Live
bytes are SIZE × USED, free ones SIZE × FREE."
  (let ((acc (list (cons :conses-mb 0) (cons :strings-mb 0)
                   (cons :string-bytes-mb 0) (cons :vectors-mb 0)
                   (cons :symbols-mb 0) (cons :intervals-mb 0)
                   (cons :floats-mb 0) (cons :buffers-mb 0)))
        (live 0) (free 0) (heap nil))
    (dolist (e heapsize)
      (let ((key (cdr (assq (car e) nidhogg-watchdog-gc--heap-types))))
        (when key
          (let ((size (nth 1 e)) (used (or (nth 2 e) 0)) (unused (or (nth 3 e) 0)))
            (setcdr (assq key acc) (+ (cdr (assq key acc)) (* size used)))
            (setq live (+ live (* size used))
                  free (+ free (* size unused)))))))
    (dolist (c acc)
      (setq heap (cons (nidhogg-watchdog-gc--mb (cdr c)) (cons (car c) heap))))
    (cons live (append (list :live-mb (nidhogg-watchdog-gc--mb live)
                             :free-mb (nidhogg-watchdog-gc--mb free))
                       (nreverse heap)))))

(defun nidhogg-watchdog-gc--threshold-mb (note live)
  "The threshold in effect at NOTE's collection, in MB, LIVE bytes live.
That is `gc-cons-percentage' of the live heap when it is more than
`gc-cons-threshold'."
  (let ((threshold (plist-get note :threshold))
        (percentage (plist-get note :percentage)))
    (and (numberp threshold)
         (nidhogg-watchdog-gc--mb
          (max threshold (if (numberp percentage) (* percentage live) 0))))))

(defun nidhogg-watchdog-gc--rss-mb ()
  "This Emacs's resident size in MB, or nil where the system does not tell."
  (let* ((default-directory "/")
         (file-name-handler-alist nil)
         (kb (cdr (assq 'rss (process-attributes (emacs-pid))))))
    (and (numberp kb) (round (/ kb 1024.0)))))

(defun nidhogg-watchdog-gc--stall (span)
  "The waiting stall of SPAN, or nil (its span made no stall, or not yet)."
  (let ((q nidhogg-watchdog--queue) (found nil))
    (while (and q (not found))
      (when (eql (plist-get (car q) :span) span)
        (setq found (car q)))
      (setq q (cdr q)))
    found))

(defun nidhogg-watchdog-gc--write (gcs c note rss)
  "Write collection GCS's line from C (the C ring's) and NOTE (the hook's).
Either may be nil; RSS is the process's MB now."
  (let* ((span (and c (plist-get c :span)))
         (stall (and span (nidhogg-watchdog-gc--stall span)))
         (heap (and note (plist-get note :heap)
                    (nidhogg-watchdog-gc--heap (plist-get note :heap))))
         (origin (or (and note (plist-get note :origin))
                     (and c (if (plist-get c :explicit) 'explicit 'auto)))))
    (nidhogg-watchdog--log-write
     'gc
     (list :pause-ms (and c (plist-get c :pause-ms))
           :origin origin
           :felt (and stall (nidhogg-watchdog--bool (plist-get stall :felt)))
           :span-wall-ms (and stall (plist-get stall :wall-ms))
           :span-nongc-ms (and stall (plist-get stall :nongc-ms))
           :gcs-done gcs
           :heap (cdr heap)
           :rss-mb rss
           :threshold-mb (and note heap
                              (nidhogg-watchdog-gc--threshold-mb note (car heap))))
     (and c (numberp (plist-get c :start))
          (seconds-to-time (plist-get c :start))))))

(defun nidhogg-watchdog-gc--collections (collections)
  "Join the taken COLLECTIONS with the notes on `gcs-done' and write them.
The drain calls it (`nidhogg-watchdog--collections-function') after it
has queued the stalls of the same take.  A note older than the newest
collection whose collection is missing was dropped by the C ring: it is
written without a pause.  Lines go out in `gcs-done' order."
  (let ((newest 0) (entries nil) (rss (nidhogg-watchdog-gc--rss-mb)))
    (dolist (c collections)
      (setq newest (max newest (plist-get c :gcs-done))))
    (let ((notes (nidhogg-watchdog-gc--take-notes newest)))
      (dolist (c collections)
        (let* ((gcs (plist-get c :gcs-done))
               (n (assq gcs notes)))
          (if n
              (setq notes (delq n notes))
            (setq nidhogg-watchdog-gc--c-only (1+ nidhogg-watchdog-gc--c-only)))
          (push (list gcs c (cdr n)) entries)))
      (dolist (n notes)
        (setq nidhogg-watchdog-gc--lisp-only (1+ nidhogg-watchdog-gc--lisp-only))
        (push (list (car n) nil (cdr n)) entries)))
    (dolist (e (sort entries (lambda (a b) (< (car a) (car b)))))
      (nidhogg-watchdog-gc--write (nth 0 e) (nth 1 e) (nth 2 e) rss))))

(defun nidhogg-watchdog-gc--flush ()
  "Write the notes left in the ring, without a pause (the C side is off)."
  (when (> nidhogg-watchdog-gc--count 0)
    (let ((rss (nidhogg-watchdog-gc--rss-mb)))
      (dolist (n (nidhogg-watchdog-gc--take-notes most-positive-fixnum))
        (setq nidhogg-watchdog-gc--lisp-only (1+ nidhogg-watchdog-gc--lisp-only))
        (nidhogg-watchdog-gc--write (car n) nil (cdr n) rss)))))

(defun nidhogg-watchdog-gc--mode-changed ()
  "On `nidhogg-watchdog-mode-hook': the GC hook follows the mode."
  (if (bound-and-true-p nidhogg-watchdog-mode)
      (unless (memq #'nidhogg-watchdog-gc--note (default-value 'post-gc-hook))
        (nidhogg-watchdog-gc--reset)
        (add-hook 'post-gc-hook #'nidhogg-watchdog-gc--note))
    (remove-hook 'post-gc-hook #'nidhogg-watchdog-gc--note)
    (ignore-errors (nidhogg-watchdog-gc--flush))))

;; This file is loaded before nidhogg-watchdog.el (loadup.el), whose
;; `defvar' then keeps this value.
(setq nidhogg-watchdog--collections-function #'nidhogg-watchdog-gc--collections)
(add-hook 'nidhogg-watchdog-mode-hook #'nidhogg-watchdog-gc--mode-changed)

;;; The row (reading the day files back)

(defconst nidhogg-watchdog-gc--day-re
  "\\`\\([0-9]\\{4\\}\\)-\\([0-9][0-9]\\)-\\([0-9][0-9]\\)\\.jsonl\\'"
  "A day file's name.")

(defun nidhogg-watchdog-gc--day-number (date)
  "DATE (\"YYYY-MM-DD\") as a count of days."
  (/ (time-convert (encode-time (list 0 0 12
                                      (string-to-number (substring date 8 10))
                                      (string-to-number (substring date 5 7))
                                      (string-to-number (substring date 0 4))
                                      nil nil t))
                   'integer)
     86400))

(defun nidhogg-watchdog-gc--read-day (file)
  "FILE's collection lines, oldest first, as plists; other lines are skipped."
  (let ((out nil) (coding-system-for-read 'utf-8))
    (with-temp-buffer
      (insert-file-contents file)
      (goto-char (point-min))
      (while (search-forward "\"type\":\"gc\"" nil t)
        (let ((o (ignore-errors
                   (json-parse-string
                    (buffer-substring-no-properties (line-beginning-position)
                                                    (line-end-position))
                    :object-type 'plist :null-object nil :false-object nil))))
          (when (equal (plist-get o :type) "gc")
            (push o out)))
        (forward-line 1)))
    (nreverse out)))

(defun nidhogg-watchdog-gc--percentile (counts q top)
  "The Q quantile (0 to 1) of the pauses counted in the bucket vector COUNTS.
Linear inside its bucket and at most TOP (the open last bucket ends at
TOP).  Nil when COUNTS is empty."
  (let* ((edges nidhogg-watchdog--bucket-edges)
         (n (apply #'+ (append counts nil)))
         (rank (* q n)) (cum 0) (i 0) (result nil))
    (when (> n 0)
      (while (not result)
        (let ((c (aref counts i)))
          (if (and (> c 0) (>= (+ cum c) rank))
              (let ((lo (if (= i 0) 0 (aref edges (1- i))))
                    (hi (if (< i (length edges)) (aref edges i) top)))
                (setq result (min top (+ lo (* (- hi lo)
                                               (/ (- rank cum) (float c)))))))
            (setq cum (+ cum c) i (1+ i)))))
      (float result))))

;; A day file is read into a digest, small and enough for every number of
;; the row: the pause histogram, count, total, max, felt, the last live
;; heap, and a running least-squares fit of pause against live heap
;; ([N MEAN-X MEAN-Y M2-X C-XY], merged exactly, so a heap that never
;; changes has no slope).  Digests are kept per file and made again only
;; when the file's size or time changed: a closed day is parsed once.

(defun nidhogg-watchdog-gc--fit-add (fit x y)
  "Add the point X, Y to FIT (a vector [N MX MY M2X CXY]); return FIT."
  (let* ((n (1+ (aref fit 0)))
         (dx (- x (aref fit 1)))
         (mx (+ (aref fit 1) (/ dx n)))
         (my (+ (aref fit 2) (/ (- y (aref fit 2)) n))))
    (aset fit 3 (+ (aref fit 3) (* dx (- x mx))))
    (aset fit 4 (+ (aref fit 4) (* dx (- y my))))
    (aset fit 0 n) (aset fit 1 mx) (aset fit 2 my)
    fit))

(defun nidhogg-watchdog-gc--fit-merge (a b)
  "The fit of the points of fits A and B together (a new vector)."
  (let ((na (aref a 0)) (nb (aref b 0)))
    (cond ((= na 0) (copy-sequence b))
          ((= nb 0) (copy-sequence a))
          (t (let* ((n (+ na nb))
                    (dx (- (aref b 1) (aref a 1)))
                    (dy (- (aref b 2) (aref a 2)))
                    (w (/ (* na nb) (float n))))
               (vector n
                       (+ (aref a 1) (/ (* dx nb) (float n)))
                       (+ (aref a 2) (/ (* dy nb) (float n)))
                       (+ (aref a 3) (aref b 3) (* dx dx w))
                       (+ (aref a 4) (aref b 4) (* dx dy w))))))))

(defun nidhogg-watchdog-gc--digest (lines)
  "LINES (collection lines, oldest first) as a digest plist.
\(:counts V :count N :no-pause N :total MS :max MS :felt N :heap H :fit F)"
  (let ((counts (make-vector (1+ (length nidhogg-watchdog--bucket-edges)) 0))
        (n 0) (no-pause 0) (top nil) (total 0.0) (felt 0) (heap nil)
        (fit (vector 0 0.0 0.0 0.0 0.0)))
    (dolist (l lines)
      (let ((ms (plist-get l :pause_ms))
            (h (plist-get l :heap)))
        (if (not (numberp ms))
            (setq no-pause (1+ no-pause))
          (let ((b (nidhogg-watchdog--bucket ms)))
            (aset counts b (1+ (aref counts b))))
          (setq n (1+ n)
                total (+ total ms)
                top (if top (max top ms) ms)))
        (when (eq (plist-get l :felt) t)
          (setq felt (1+ felt)))
        (when (numberp (plist-get h :live_mb))
          (setq heap h)
          (when (numberp ms)
            (nidhogg-watchdog-gc--fit-add fit (plist-get h :live_mb) ms)))))
    (list :counts counts :count n :no-pause no-pause :total total :max top
          :felt felt :heap heap :fit fit)))

(defun nidhogg-watchdog-gc--merge (digests)
  "DIGESTS (oldest first) as one digest."
  (let ((counts (make-vector (1+ (length nidhogg-watchdog--bucket-edges)) 0))
        (n 0) (no-pause 0) (top nil) (total 0.0) (felt 0) (heap nil)
        (fit (vector 0 0.0 0.0 0.0 0.0)))
    (dolist (d digests)
      (let ((c (plist-get d :counts)) (m (plist-get d :max)))
        (dotimes (i (length counts))
          (aset counts i (+ (aref counts i) (aref c i))))
        (setq n (+ n (plist-get d :count))
              no-pause (+ no-pause (plist-get d :no-pause))
              total (+ total (plist-get d :total))
              felt (+ felt (plist-get d :felt))
              top (if (and top m) (max top m) (or top m))
              heap (or (plist-get d :heap) heap)
              fit (nidhogg-watchdog-gc--fit-merge fit (plist-get d :fit)))))
    (list :counts counts :count n :no-pause no-pause :total total :max top
          :felt felt :heap heap :fit fit)))

(defun nidhogg-watchdog-gc--summary (digest)
  "DIGEST summed up: a plist (:count :no-pause :total-s :max :p50 :p95 :p99
:felt :heap).  :count is the collections with a pause, :no-pause the
others; :heap is the last live heap by type."
  (let ((counts (plist-get digest :counts))
        (top (plist-get digest :max)))
    (list :count (plist-get digest :count)
          :no-pause (plist-get digest :no-pause)
          :total-s (/ (plist-get digest :total) 1000.0)
          :max (and top (float top))
          :p50 (and top (nidhogg-watchdog-gc--percentile counts 0.5 top))
          :p95 (and top (nidhogg-watchdog-gc--percentile counts 0.95 top))
          :p99 (and top (nidhogg-watchdog-gc--percentile counts 0.99 top))
          :felt (plist-get digest :felt)
          :heap (plist-get digest :heap))))

(defvar nidhogg-watchdog-gc--digests (make-hash-table :test 'equal)
  "FILE -> ((SIZE . MTIME) . DIGEST) for the day files read last.")

(defun nidhogg-watchdog-gc--day-digest (file)
  "FILE's digest: the kept one while FILE's size and time are unchanged."
  (let* ((attrs (file-attributes file 'integer))
         (key (and attrs (cons (file-attribute-size attrs)
                               (file-attribute-modification-time attrs))))
         (hit (gethash file nidhogg-watchdog-gc--digests)))
    (if (and key (equal (car hit) key))
        (cdr hit)
      (let ((d (nidhogg-watchdog-gc--digest (nidhogg-watchdog-gc--read-day file))))
        (when key
          (puthash file (cons key d) nidhogg-watchdog-gc--digests))
        d))))

(defun nidhogg-watchdog-gc--fit (points)
  "The least-squares slope of POINTS ((X . Y) …), or nil.
Nil under two points or when X never changes."
  (let ((n (length points)))
    (when (>= n 2)
      (let ((mx (/ (apply #'+ (mapcar #'car points)) (float n)))
            (my (/ (apply #'+ (mapcar #'cdr points)) (float n)))
            (sxx 0.0) (sxy 0.0))
        (dolist (p points)
          (setq sxx (+ sxx (* (- (car p) mx) (- (car p) mx)))
                sxy (+ sxy (* (- (car p) mx) (- (cdr p) my)))))
        (and (> sxx 0) (/ sxy sxx))))))

(defun nidhogg-watchdog-gc--meets (p95 top)
  "Whether P95 (7 days, or nil when unknown) and a day's maximum TOP meet."
  (and (or (null p95) (<= p95 nidhogg-watchdog-gc-target-p95))
       (or (null top) (< top nidhogg-watchdog-gc-max))))

(defun nidhogg-watchdog-gc--week (all day)
  "The rolling 7 days of ALL (day entries) that end on day number DAY."
  (let ((window nil))
    (dolist (d all)
      (when (and (<= (nth 1 d) day) (> (nth 1 d) (- day 7)))
        (push d window)))
    (setq window (nreverse window))
    (let* ((m (nidhogg-watchdog-gc--merge (mapcar (lambda (d) (nth 2 d)) window)))
           (s (nidhogg-watchdog-gc--summary m))
           (fit (plist-get m :fit))
           (heaps nil))
      (dolist (d window)
        (let ((h (plist-get (nth 2 d) :heap)))
          (when h
            (push (cons (nth 1 d) (plist-get h :live_mb)) heaps))))
      (list :count (plist-get s :count)
            :total-s (plist-get s :total-s)
            :max (plist-get s :max)
            :p95 (and (>= (plist-get s :count) 20) (plist-get s :p95))
            :felt (plist-get s :felt)
            :slope (and (>= (aref fit 0) 2) (> (aref fit 3) 0)
                        (* 100 (/ (aref fit 4) (aref fit 3))))
            :points (aref fit 0)
            :heap-trend (nidhogg-watchdog-gc--fit heaps)
            :heap (plist-get s :heap)))))

(defun nidhogg-watchdog-gc-row (&rest args)
  "The GC row from the collection lines of the day files: a plist.
ARGS may give :days, how many of the newest day files to report (7).

:days, one plist per day file, oldest first: :date, :count (collections
with a pause), :no-pause, :total-s, :max, :p50 :p95 :p99 (ms, from the
histogram buckets), :felt (count), :heap (the live heap by type after its
last collection, MB) and :meets (that day's verdict: the p95 of the 7
days ending on it, when they hold at least 20 collections, at most
`nidhogg-watchdog-gc-target-p95', and no collection of
`nidhogg-watchdog-gc-max' or more in the day).
:week, the 7 days ending on the newest day file: :count, :total-s, :max,
:p95 (nil under 20 collections), :felt, :slope (pause against live heap,
ms per 100 MB, least squares; nil under two points), :points,
:heap-trend (MB per day, from each day's last live heap) and :heap.
:meets, the newest day's verdict (nil when there is no day).
:target and :gc-max, the two numbers it was judged by; :session, this
session's notes dropped and entries written without a partner.

\(fn &key DAYS)"
  (let* ((days (or (plist-get args :days) 7))
         (dir nidhogg-watchdog-log-directory)
         ;; The newest DAYS files and the 6 before them hold every 7-day
         ;; window the rows need; older files are not read.
         (files (and dir (file-directory-p dir)
                     (last (directory-files dir t nidhogg-watchdog-gc--day-re)
                           (+ days 6))))
         (all (mapcar (lambda (f)
                        (let ((date (file-name-base f)))
                          (list date (nidhogg-watchdog-gc--day-number date)
                                (nidhogg-watchdog-gc--day-digest f))))
                      files))
         (rows nil))
    ;; Keep digests only for the files of this row.
    (let (gone)
      (maphash (lambda (f _) (unless (member f files) (push f gone)))
               nidhogg-watchdog-gc--digests)
      (dolist (f gone) (remhash f nidhogg-watchdog-gc--digests)))
    (dolist (d (last all days))
      (let ((s (nidhogg-watchdog-gc--summary (nth 2 d)))
            (week (nidhogg-watchdog-gc--week all (nth 1 d))))
        (push (append (list :date (nth 0 d)) s
                      (list :meets (nidhogg-watchdog-gc--meets
                                    (plist-get week :p95) (plist-get s :max))))
              rows)))
    (setq rows (nreverse rows))
    (list :days rows
          :week (and all (nidhogg-watchdog-gc--week all (nth 1 (car (last all)))))
          :meets (and rows (plist-get (car (last rows)) :meets))
          :target nidhogg-watchdog-gc-target-p95
          :gc-max nidhogg-watchdog-gc-max
          :session (list :dropped nidhogg-watchdog-gc--dropped
                         :c-only nidhogg-watchdog-gc--c-only
                         :lisp-only nidhogg-watchdog-gc--lisp-only))))

(provide 'nidhogg-watchdog-gc)

;;; nidhogg-watchdog-gc.el ends here
