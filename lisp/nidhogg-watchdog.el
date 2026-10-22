;;; nidhogg-watchdog.el --- Nidhogg's stall watchdog: the mode, the drain and the log  -*- lexical-binding: t -*-

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

;; Preloaded when Nidhogg is built with --with-nidhogg-watchdog (loadup.el's one
;; `(featurep 'nidhogg-watchdog)' line).  The C side provides the feature
;; `nidhogg-watchdog'; this file provides `nidhogg-watchdog-lisp'.
;;
;; `nidhogg-watchdog-mode' starts the C monitor and one repeating idle timer.
;; The timer drains the C rings (take at once, resolve within 2 ms) and appends
;; JSON Lines, one file per day.  Names, files and packages come from
;; nidhogg-watchdog-resolve.el (Task 15), the GC row from -gc.el (Task 16),
;; ranking from -report.el (Task 17); hang files arrive in Task 18.

;;; Code:

;; `load', not `require': loadup refuses a `require' that loads a file.
(load "nidhogg-watchdog-resolve")
(load "nidhogg-watchdog-gc")
(load "nidhogg-watchdog-report")

(defgroup nidhogg-watchdog nil
  "Nidhogg's stall watchdog."
  :group 'processes
  :prefix "nidhogg-watchdog-")

(defvar nidhogg-watchdog-mode nil
  "Non-nil while the stall watchdog runs.  Use `nidhogg-watchdog-mode'.")

(defcustom nidhogg-watchdog-threshold 100
  "A busy span of at least this many milliseconds is a stall.
Read when `nidhogg-watchdog-mode' is turned on."
  :type 'number)

(defcustom nidhogg-watchdog-hang-ms 2000
  "A busy span of at least this many milliseconds is a hang.
Read when `nidhogg-watchdog-mode' is turned on."
  :type 'number)

(defcustom nidhogg-watchdog-log-directory (locate-user-emacs-file "watchdog/")
  "The directory of the day files (`YYYY-MM-DD.jsonl').
It is created when the first line is written."
  :type 'directory
  :initialize #'custom-initialize-delay)

(defun nidhogg-watchdog--typing-commands-setter (symbol value)
  "Set SYMBOL to VALUE and mirror it into the C side while the mode is on."
  (set-default symbol value)
  (when (and (bound-and-true-p nidhogg-watchdog-mode)
             (fboundp 'nidhogg-watchdog--set-typing-commands))
    (nidhogg-watchdog--set-typing-commands value)))

(defcustom nidhogg-watchdog-typing-commands nil
  "Commands whose latency (arrival to end) counts as typing latency.
A list of up to 16 symbols, mirrored into the C side."
  :type '(repeat symbol)
  :set #'nidhogg-watchdog--typing-commands-setter)

(defcustom nidhogg-watchdog-record-functions nil
  "Abnormal hook run with each resolved stall record (a plist)."
  :type 'hook)

(defcustom nidhogg-watchdog-package-functions nil
  "Functions run with a function name until one returns its package."
  :type 'hook)

(defvar nidhogg-watchdog-gc-origin nil
  "Callers bind this around `garbage-collect': idle, gap, audit or explicit.
Nil means an automatic collection.")

(defconst nidhogg-watchdog--days-kept 14
  "Day files older than this many days are deleted.")

(defvar nidhogg-watchdog--stall-cap 5000
  "Stall lines written per day; past that a day only counts.")

(defvar nidhogg-watchdog--summary-interval 600
  "Seconds between two summary lines at most.")

(defvar nidhogg-watchdog--drain-budget 0.002
  "Seconds one drain may spend resolving records.")

(defvar nidhogg-watchdog--clock #'float-time
  "Function returning the drain's time in seconds (tests bind a virtual one).")

(defvar nidhogg-watchdog--resolve-function nil
  "Function turning a raw record into a resolved one (set by Task 15).
Nil leaves the raw record as it is.")

(defvar nidhogg-watchdog--collections-function nil
  "Function called with the list of taken collections (set by Task 16).
Nil drops them.")

(defvar nidhogg-watchdog--timer nil)
(defvar nidhogg-watchdog--queue nil
  "Taken, not yet resolved stalls (Lisp data; GC keeps them alive).")
(defvar nidhogg-watchdog--draining nil)
(defvar nidhogg-watchdog--drain-hist (make-vector 9 0)
  "Drain durations by the buckets of the C histograms (ms).")
(defvar nidhogg-watchdog--day nil "The day the counters below belong to.")
(defvar nidhogg-watchdog--day-dir nil "The log directory of that day's counters.")
(defvar nidhogg-watchdog--day-stalls 0 "Stall lines in today's file.")
(defvar nidhogg-watchdog--over-cap 0 "Stalls not written because of the cap.")
(defvar nidhogg-watchdog--last-summary nil)
(defvar nidhogg-watchdog--errors 0 "Errors the timer function caught.")

(defconst nidhogg-watchdog--bucket-edges [4 8 16 33 50 100 250 1000]
  "Upper edges (ms) of the histogram buckets; the last bucket is open.")

;;; The log

(defun nidhogg-watchdog--bool (x)
  "X as a JSON boolean."
  (if x t :false))

(defun nidhogg-watchdog--key (k)
  "The JSON key for the keyword K: underscores for dashes."
  (intern (concat ":" (string-replace "-" "_" (substring (symbol-name k) 1)))))

(defun nidhogg-watchdog--jsonable (x)
  "X in a shape `json-serialize' takes.
A list whose first element is a keyword is an object, any other list an
array, a symbol its name, nil null (write booleans with `--bool')."
  (cond ((null x) :null)
        ((memq x '(t :false :null)) x)
        ((keywordp x) (substring (symbol-name x) 1))
        ((symbolp x) (symbol-name x))
        ((floatp x) (/ (round (* x 1000)) 1000.0))
        ((consp x)
         (if (keywordp (car x))
             (let (out)
               (while x
                 (push (nidhogg-watchdog--key (pop x)) out)
                 (push (nidhogg-watchdog--jsonable (pop x)) out))
               (nreverse out))
           (vconcat (mapcar #'nidhogg-watchdog--jsonable x))))
        ((vectorp x) (vconcat (mapcar #'nidhogg-watchdog--jsonable x)))
        (t x)))

(defun nidhogg-watchdog--file (day)
  "The day file for DAY (\"YYYY-MM-DD\")."
  (expand-file-name (concat day ".jsonl") nidhogg-watchdog-log-directory))

(defun nidhogg-watchdog--prune ()
  "Delete day files older than `nidhogg-watchdog--days-kept' days."
  (when (file-directory-p nidhogg-watchdog-log-directory)
    (let ((limit (time-subtract nil (* nidhogg-watchdog--days-kept 86400))))
      (dolist (f (directory-files nidhogg-watchdog-log-directory t
                                  "\\`[0-9]\\{4\\}-[0-9][0-9]-[0-9][0-9]\\.jsonl\\'"))
        (when (time-less-p (file-attribute-modification-time
                            (file-attributes f))
                           limit)
          (ignore-errors (delete-file f)))))))

(defun nidhogg-watchdog--enter-day (day)
  "Reset the per-day counters for DAY from what is already in its file."
  (setq nidhogg-watchdog--day day
        nidhogg-watchdog--day-dir nidhogg-watchdog-log-directory
        nidhogg-watchdog--day-stalls 0
        nidhogg-watchdog--over-cap 0)
  (nidhogg-watchdog--prune)
  (let ((file (nidhogg-watchdog--file day)))
    (when (file-readable-p file)
      (with-temp-buffer
        (insert-file-contents file)
        (setq nidhogg-watchdog--day-stalls
              (count-matches "\"type\":\"stall\"" (point-min) (point-max)))))))

(defun nidhogg-watchdog--log-write (type plist &optional time)
  "Append one line of TYPE (stall, gc or summary) with the fields PLIST.
PLIST's keys are keywords (dashes become underscores in the file).  TIME
is the line's time (default now).  Return non-nil when a line was written;
a stall past the day's cap is only counted."
  (let* ((time (or time (current-time)))
         (day (format-time-string "%F" time)))
    (unless (and (equal day nidhogg-watchdog--day)
                 (equal nidhogg-watchdog-log-directory nidhogg-watchdog--day-dir))
      (nidhogg-watchdog--enter-day day))
    (if (and (eq type 'stall)
             (>= nidhogg-watchdog--day-stalls nidhogg-watchdog--stall-cap))
        (progn (setq nidhogg-watchdog--over-cap (1+ nidhogg-watchdog--over-cap))
               nil)
      (let ((line (json-serialize
                   (nidhogg-watchdog--jsonable
                    (append (list :v 1 :type (symbol-name type)
                                  :at (format-time-string "%FT%T.%3N%:z" time)
                                  :pid (emacs-pid)
                                  :nidhogg (bound-and-true-p nidhogg-version))
                            plist))
                   :null-object :null :false-object :false))
            (file (nidhogg-watchdog--file day))
            (coding-system-for-write 'binary)
            (inhibit-message t))
        (make-directory nidhogg-watchdog-log-directory t)
        (write-region (concat line "\n") nil file t 'silent)
        (when (eq type 'stall)
          (setq nidhogg-watchdog--day-stalls (1+ nidhogg-watchdog--day-stalls)))
        t))))

;;; Histograms and stats

(defun nidhogg-watchdog--bucket (ms)
  "The bucket index of MS milliseconds."
  (let ((i 0))
    (while (and (< i 8) (>= ms (aref nidhogg-watchdog--bucket-edges i)))
      (setq i (1+ i)))
    i))

(defun nidhogg-watchdog--p95 (hist)
  "The upper edge (ms) of the bucket holding the 95th percentile of HIST."
  (let ((total (apply #'+ (append hist nil))) (acc 0) (i 0))
    (if (zerop total)
        0
      (while (and (< i 8)
                  (< (* 20 (setq acc (+ acc (aref hist i)))) (* 19 total)))
        (setq i (1+ i)))
      (aref nidhogg-watchdog--bucket-edges (min i 7)))))

(defun nidhogg-watchdog-stats (&optional reset)
  "The C counters and histograms, plus the drain's.
Adds `:drain' (a histogram), `:drain-p95-ms' and `:stalls-over-cap'.
Nil when the watchdog is off.  With RESET, zero them all."
  (let ((c (nidhogg-watchdog--stats reset)))
    (when c
      (let* ((h (copy-sequence nidhogg-watchdog--drain-hist))
             (r (append c (list :drain h
                                :drain-p95-ms (nidhogg-watchdog--p95 h)
                                :stalls-over-cap nidhogg-watchdog--over-cap))))
        (when reset
          (fillarray nidhogg-watchdog--drain-hist 0)
          (setq nidhogg-watchdog--over-cap 0))
        r))))

(defun nidhogg-watchdog--write-summary (&optional final)
  "Write a summary line with the deltas since the last one; FINAL at mode off."
  (let ((s (nidhogg-watchdog-stats t))
        (now (funcall nidhogg-watchdog--clock)))
    (setq nidhogg-watchdog--last-summary now)
    (when s
      (let ((h (plist-get s :histograms)))
        (nidhogg-watchdog--log-write
         'summary
         (list :final (nidhogg-watchdog--bool final)
               :spans (plist-get s :spans)
               :stalls (plist-get s :stalls)
               :stalls-over-cap (plist-get s :stalls-over-cap)
               :missed (plist-get s :missed)
               :stale (plist-get s :stale)
               :ring-overflows (plist-get s :dropped)
               :gc-dropped (plist-get s :gc-dropped)
               :monitor-cpu-ms (plist-get s :monitor-cpu-ms)
               :drain-p95-ms (plist-get s :drain-p95-ms)
               :histograms (list :command (plist-get h :command)
                                 :other (plist-get h :other)
                                 :typing (plist-get h :typing)
                                 :collections (plist-get h :collections)
                                 :drain (plist-get s :drain))))))))

(defun nidhogg-watchdog--maybe-summary ()
  "Write a summary line when `nidhogg-watchdog--summary-interval' has passed."
  (let ((now (funcall nidhogg-watchdog--clock)))
    (if (null nidhogg-watchdog--last-summary)
        (setq nidhogg-watchdog--last-summary now)
      (when (>= (- now nidhogg-watchdog--last-summary)
                nidhogg-watchdog--summary-interval)
        (nidhogg-watchdog--write-summary)))))

;;; The drain

(defun nidhogg-watchdog--emit (record)
  "Run the record hook on RECORD and write its stall line."
  (dolist (f nidhogg-watchdog-record-functions)
    (condition-case nil (funcall f record) (error nil)))
  (nidhogg-watchdog--log-write
   'stall
   (list :wall-ms (plist-get record :wall-ms)
         :gc-ms (plist-get record :gc-ms)
         :gcs (plist-get record :gcs)
         :nongc-ms (plist-get record :nongc-ms)
         :cpu-ms (plist-get record :cpu-ms)
         :state (plist-get record :state)
         :felt (nidhogg-watchdog--bool (plist-get record :felt))
         :input-wait-ms (plist-get record :input-wait-ms)
         :commands (plist-get record :commands)
         :input-queued (nidhogg-watchdog--bool (plist-get record :input-queued))
         :hang (nidhogg-watchdog--bool (plist-get record :hang))
         :killed (nidhogg-watchdog--bool (plist-get record :killed))
         :entry (plist-get record :entry)
         :culprit (plist-get record :culprit)
         :phases (plist-get record :phases)
         :signature (plist-get record :signature)
         :stacks (plist-get record :stacks)
         :samples (plist-get record :samples-taken)
         :missed (plist-get record :missed)
         :late-ms (plist-get record :late-ms))
   (let ((start (plist-get record :start)))
     (and (numberp start) (seconds-to-time start)))))

(defun nidhogg-watchdog--drain ()
  "Take what is pending and resolve it within the budget; return the count done.
Allocates nothing when nothing waits (no parameters, so not even an
interpreted call binds anything)."
  (when (and (not nidhogg-watchdog--draining)
             (or nidhogg-watchdog--queue
                 (not (eq 0 (nidhogg-watchdog--pending)))))
    (nidhogg-watchdog--drain-1 nil)))

(defun nidhogg-watchdog--drain-1 (unlimited)
  "Take, queue and resolve; with UNLIMITED, resolve everything."
  (progn
    (let* ((nidhogg-watchdog--draining t)
           (clock nidhogg-watchdog--clock)
           (t0 (funcall clock))
           (done 0)
           (taken (nidhogg-watchdog--take)))
      (setq nidhogg-watchdog--queue (nconc nidhogg-watchdog--queue (car taken)))
      (when (and (cdr taken) nidhogg-watchdog--collections-function)
        (funcall nidhogg-watchdog--collections-function (cdr taken)))
      (while (and nidhogg-watchdog--queue
                  (or unlimited
                      (< (- (funcall clock) t0) nidhogg-watchdog--drain-budget)))
        (let ((raw (pop nidhogg-watchdog--queue)))
          (nidhogg-watchdog--emit
           (if nidhogg-watchdog--resolve-function
               (funcall nidhogg-watchdog--resolve-function raw)
             raw))
          (setq done (1+ done))))
      (let ((i (nidhogg-watchdog--bucket (* 1000 (- (funcall clock) t0)))))
        (aset nidhogg-watchdog--drain-hist i
              (1+ (aref nidhogg-watchdog--drain-hist i))))
      done)))

(defun nidhogg-watchdog-drain-now ()
  "Drain everything now, without the budget (perf and tests)."
  (unless nidhogg-watchdog--draining
    (nidhogg-watchdog--drain-1 t)))

(defun nidhogg-watchdog--tick ()
  "The idle timer: drain, then maybe a summary.  Never signals."
  (condition-case err
      (progn (nidhogg-watchdog--drain)
             (nidhogg-watchdog--maybe-summary))
    (error
     (when (= 1 (setq nidhogg-watchdog--errors (1+ nidhogg-watchdog--errors)))
       (message "nidhogg-watchdog: drain failed: %S" err)))))

;;; The mode

(defun nidhogg-watchdog--at-exit ()
  "Drain and write the final summary when Emacs exits."
  (when nidhogg-watchdog-mode
    (ignore-errors (nidhogg-watchdog-mode -1))))

(defun nidhogg-watchdog--enable ()
  "Start the C side, the drain timer and the housekeeping."
  (unless (fboundp 'nidhogg-watchdog--start)
    (user-error "This Emacs was built without the stall watchdog"))
  (nidhogg-watchdog--start nidhogg-watchdog-threshold nidhogg-watchdog-hang-ms
                           nil)
  (when (fboundp 'nidhogg-watchdog--set-typing-commands)
    (nidhogg-watchdog--set-typing-commands nidhogg-watchdog-typing-commands))
  (setq nidhogg-watchdog--queue nil
        nidhogg-watchdog--errors 0
        nidhogg-watchdog--day nil
        nidhogg-watchdog--last-summary (funcall nidhogg-watchdog--clock))
  (fillarray nidhogg-watchdog--drain-hist 0)
  (nidhogg-watchdog--prune)
  (setq nidhogg-watchdog--timer
        (run-with-idle-timer 1 t #'nidhogg-watchdog--tick))
  (add-hook 'kill-emacs-hook #'nidhogg-watchdog--at-exit))

(defun nidhogg-watchdog--disable ()
  "Stop the timer, take what is left, write the final summary, stop the C side."
  (when (timerp nidhogg-watchdog--timer)
    (cancel-timer nidhogg-watchdog--timer))
  (setq nidhogg-watchdog--timer nil)
  (remove-hook 'kill-emacs-hook #'nidhogg-watchdog--at-exit)
  ;; Builtin subrs are named only while the C side runs: take before stopping.
  (unwind-protect
      (progn (nidhogg-watchdog-drain-now)
             (nidhogg-watchdog--write-summary t))
    (nidhogg-watchdog--stop)))

;; Written by hand rather than with `define-minor-mode': this file is
;; preloaded, and easy-mmode is not.
(defun nidhogg-watchdog-mode (&optional arg)
  "Watch for stalls: busy spans of `nidhogg-watchdog-threshold' ms or more.
A C monitor times every busy span; at idle a timer resolves what it
found and appends JSON Lines to a file per day in
`nidhogg-watchdog-log-directory'.  A global mode: with ARG as for a
minor mode (nil or positive on, `toggle' toggles, other numbers off)."
  (interactive (list (or current-prefix-arg 'toggle)))
  (let ((on (cond ((eq arg 'toggle) (not nidhogg-watchdog-mode))
                  ((null arg) t)
                  (t (> (prefix-numeric-value arg) 0)))))
    (cond ((and on (not nidhogg-watchdog-mode))
           (nidhogg-watchdog--enable)
           (setq nidhogg-watchdog-mode t))
          ((and (not on) nidhogg-watchdog-mode)
           (setq nidhogg-watchdog-mode nil)
           (nidhogg-watchdog--disable)))
    nidhogg-watchdog-mode))

(provide 'nidhogg-watchdog-lisp)

;;; nidhogg-watchdog.el ends here
