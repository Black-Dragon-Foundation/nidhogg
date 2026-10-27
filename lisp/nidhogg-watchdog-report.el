;;; nidhogg-watchdog-report.el --- Nidhogg's stall watchdog: ranking and the report  -*- lexical-binding: t -*-

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

;; The ranking of stalls by package and signature, and the report buffer
;; (design 6.3, 7.1).  Both read the day files (JSON Lines) when asked for,
;; never on a timer.  This file is preloaded, so it takes no cl-lib and
;; loads tabulated-list and iso8601 only when a report is asked for.

;;; Code:

(eval-when-compile (require 'tabulated-list))

(defvar nidhogg-watchdog-threshold)
(defvar nidhogg-watchdog-log-directory)
(declare-function nidhogg-watchdog-gc-row "nidhogg-watchdog-gc")

;;; Reading the day files

(defun nidhogg-watchdog--rep-time (at)
  "The time of the ISO-8601 string AT (milliseconds dropped), or nil."
  (require 'iso8601)
  (condition-case nil
      (encode-time (iso8601-parse (concat (substring at 0 19)
                                          (substring at -6))))
    (error nil)))

(defun nidhogg-watchdog--report-lines (since)
  "The parsed lines of the day files from SINCE (a time, or nil for all).
Objects are alists; JSON false and null are nil, arrays vectors."
  (let ((first (and since (format-time-string "%F" since)))
        (dir nidhogg-watchdog-log-directory)
        out)
    (when (file-directory-p dir)
      (dolist (f (sort (directory-files
                        dir t "\\`[0-9]\\{4\\}-[0-9][0-9]-[0-9][0-9]\\.jsonl\\'")
                       #'string<))
        (when (or (null first) (string-greaterp (file-name-base f) first)
                  (string= (file-name-base f) first))
          (with-temp-buffer
            (let ((coding-system-for-read 'utf-8))
              (insert-file-contents f))
            (goto-char (point-min))
            (while (not (eobp))
              (let ((l (condition-case nil
                           (json-parse-string
                            (buffer-substring (line-beginning-position)
                                              (line-end-position))
                            :object-type 'alist :null-object nil
                            :false-object nil)
                         (error nil))))
                (when (and (consp l)
                           (or (null since)
                               (let ((tm (and (stringp (alist-get 'at l))
                                              (nidhogg-watchdog--rep-time
                                               (alist-get 'at l)))))
                                 (and tm (not (time-less-p tm since))))))
                  (push l out)))
              (forward-line 1))))))
    (nreverse out)))

;;; Ranking

(defun nidhogg-watchdog--rep-num (line key)
  "The number under KEY in LINE, or 0."
  (let ((v (alist-get key line)))
    (if (numberp v) v 0)))

(defun nidhogg-watchdog--rep-tree-add (root frames n)
  "Add FRAMES (root first) with weight N under the tree ROOT, in place.
A node is (NAME COUNT . CHILDREN)."
  (let ((node root))
    (setcar (cdr node) (+ (cadr node) n))
    (dolist (f frames)
      (let ((child (assoc f (cddr node))))
        (unless child
          (setq child (list f 0))
          (setcdr (cdr node) (nconc (cddr node) (list child))))
        (setcar (cdr child) (+ (cadr child) n))
        (setq node child)))))

(defun nidhogg-watchdog--rep-tree-sort (node)
  "Sort NODE's children by count, biggest first, recursively; return NODE."
  (setcdr (cdr node)
          (sort (cddr node) (lambda (a b) (> (cadr a) (cadr b)))))
  (dolist (c (cddr node)) (nidhogg-watchdog--rep-tree-sort c))
  node)

(defun nidhogg-watchdog--rep-tree-string (node &optional depth)
  "NODE (a call tree) as indented text, one `COUNT NAME' line per frame."
  (let ((depth (or depth 0)))
    (concat (unless (= depth 0)
              (format "%s%d %s\n" (make-string (* 2 (1- depth)) ?\s)
                      (cadr node) (car node)))
            (mapconcat (lambda (c) (nidhogg-watchdog--rep-tree-string c (1+ depth)))
                       (cddr node) ""))))

(defun nidhogg-watchdog--rep-mix (alist)
  "ALIST sorted by value, biggest first."
  (sort alist (lambda (a b) (> (cdr a) (cdr b)))))

(defun nidhogg-watchdog--rep-row (name lines thr)
  "The ranking row NAME over the stall LINES, with threshold THR (ms)."
  (let ((total 0.0) (maxms 0.0) (score 0.0) (felt 0) (nongcs nil)
        (states nil) (phases nil) (phase-weight 0.0)
        (tree (list "root" 0)) first last)
    (dolist (l lines)
      (let ((ms (nidhogg-watchdog--rep-num l 'nongc_ms))
            (at (alist-get 'at l)))
        (setq total (+ total ms)
              maxms (max maxms ms)
              score (+ score (- ms thr))
              nongcs (cons ms nongcs))
        (when (alist-get 'felt l) (setq felt (1+ felt)))
        (when (stringp at)
          (when (or (null first) (string< at first)) (setq first at))
          (when (or (null last) (string< last at)) (setq last at)))
        (let* ((s (or (alist-get 'state l) "unknown"))
               (cell (assoc s states)))
          (if cell (setcdr cell (1+ (cdr cell)))
            (push (cons s 1) states)))
        (let ((w (max ms 1.0)))
          (dolist (p (alist-get 'phases l))
            (when (numberp (cdr p))
              (let* ((k (symbol-name (car p))) (cell (assoc k phases)))
                (if cell (setcdr cell (+ (cdr cell) (* w (cdr p))))
                  (push (cons k (* w (cdr p))) phases)))))
          (setq phase-weight (+ phase-weight w)))
        (mapc (lambda (s)
                (let ((frames (alist-get 'frames s)) (n (alist-get 'n s)))
                  (when (and (sequencep frames) (> (length frames) 0))
                    (nidhogg-watchdog--rep-tree-add
                     tree (reverse (append frames nil))
                     (if (numberp n) n 1)))))
              (alist-get 'stacks l))))
    (setq nongcs (sort nongcs #'<))
    (list :name name
          :count (length lines) :felt felt
          :total total :max maxms :score score
          :p50 (if nongcs (nth (/ (length nongcs) 2) nongcs) 0.0)
          :states (nidhogg-watchdog--rep-mix states)
          :phases (nidhogg-watchdog--rep-mix
                   (mapcar (lambda (c) (cons (car c) (/ (cdr c) phase-weight)))
                           phases))
          :first first :last last
          :tree (nidhogg-watchdog--rep-tree-sort tree))))

(defun nidhogg-watchdog--rep-gc-row (gcs)
  "The GC row over the collection lines GCS."
  (let ((pauses (sort (mapcar (lambda (l) (nidhogg-watchdog--rep-num l 'pause_ms))
                              gcs)
                      #'<)))
    (list :gc t :name "GC" :count (length gcs)
          :felt (let ((n 0)) (dolist (l gcs n) (when (alist-get 'felt l) (setq n (1+ n)))))
          :total (apply #'+ 0.0 pauses)
          :max (if pauses (apply #'max pauses) 0.0)
          :p50 (if pauses (nth (/ (length pauses) 2) pauses) 0.0)
          :collections gcs
          :detail (and (fboundp 'nidhogg-watchdog-gc-row)
                       (condition-case nil (nidhogg-watchdog-gc-row) (error nil))))))

(defun nidhogg-watchdog--rank (lines felt-only)
  "The ranking over the parsed LINES; with FELT-ONLY only felt stalls and GCs."
  (let ((thr (or (bound-and-true-p nidhogg-watchdog-threshold) 100))
        (pkgs (make-hash-table :test 'equal))
        (order nil) gcs)
    (dolist (l lines)
      (let ((type (alist-get 'type l)))
        (cond
         ((and (equal type "gc") (or (not felt-only) (alist-get 'felt l)))
          (push l gcs))
         ((and (equal type "stall")
               (or (not felt-only) (alist-get 'felt l))
               (>= (nidhogg-watchdog--rep-num l 'nongc_ms) thr))
          (let* ((culprit (alist-get 'culprit l))
                 (pkg (or (and (consp culprit) (alist-get 'pkg culprit))
                          "unsampled"))
                 (sig (or (alist-get 'signature l) "unsampled"))
                 (sigs (or (gethash pkg pkgs)
                           (progn (push pkg order)
                                  (puthash pkg (list nil) pkgs))))
                 (cell (assoc sig (car sigs))))
            (if cell (setcdr cell (cons l (cdr cell)))
              (setcar sigs (cons (list sig l) (car sigs)))))))))
    (let (rows)
      (dolist (pkg order)
        (let* ((sigs (mapcar (lambda (s)
                               (nidhogg-watchdog--rep-row
                                (car s) (reverse (cdr s)) thr))
                             (car (gethash pkg pkgs))))
               (all (apply #'append
                           (mapcar (lambda (s) (cdr s)) (car (gethash pkg pkgs)))))
               (row (nidhogg-watchdog--rep-row pkg all thr)))
          (push (append (list :package pkg
                              :signatures
                              (sort sigs (lambda (a b) (> (plist-get a :score)
                                                          (plist-get b :score)))))
                        row)
                rows)))
      (cons (nidhogg-watchdog--rep-gc-row (nreverse gcs))
            (sort rows (lambda (a b) (> (plist-get a :score)
                                        (plist-get b :score))))))))

(defun nidhogg-watchdog-ranking (&rest args)
  "The ranking of stalls: the GC row first, then packages by score.
ARGS are keywords: `:since' (a time; default all day files kept) and
`:felt-only' (non-nil: only felt stalls and collections).

Each package row is a plist with `:package', `:score' (the sum over its
stalls of non-GC ms minus the threshold, for every stall whose non-GC time
reaches the threshold), `:count', `:felt', `:total', `:max', `:p50' (ms),
`:states' and `:phases' (alists, biggest first; phase shares are weighted by
non-GC ms), `:first', `:last', `:tree' (a call tree, (NAME COUNT . CHILDREN),
stacks being innermost first in the files) and `:signatures', rows of the
same shape named by signature.  The GC row has `:gc' t and `:collections'."
  (nidhogg-watchdog--rank
   (nidhogg-watchdog--report-lines (plist-get args :since))
   (plist-get args :felt-only)))

;;; The report buffer

(defun nidhogg-watchdog--report-header (lines)
  "The day's header over the parsed LINES."
  (let ((stalls 0) (felt 0) (longest 0.0) (gcs 0) (gcmax 0.0)
        (typing (make-vector 9 0)))
    (dolist (l lines)
      (let ((type (alist-get 'type l)))
        (cond
         ((equal type "stall")
          (setq stalls (1+ stalls)
                longest (max longest (nidhogg-watchdog--rep-num l 'wall_ms)))
          (when (alist-get 'felt l) (setq felt (1+ felt))))
         ((equal type "gc")
          (setq gcs (1+ gcs)
                gcmax (max gcmax (nidhogg-watchdog--rep-num l 'pause_ms))))
         ((equal type "summary")
          (let ((h (alist-get 'typing (alist-get 'histograms l))))
            (when (and (vectorp h) (= (length h) 9))
              (dotimes (i 9) (aset typing i (+ (aref typing i) (aref h i))))))))))
    (let ((all (apply #'+ (append typing nil)))
          (slow (apply #'+ (nthcdr 3 (append typing nil)))))
      (format "%d stalls over %s ms, %d felt, longest %d ms; GC: %d collections, max %d ms; typing ≥ 16 ms: %s"
              stalls (or (bound-and-true-p nidhogg-watchdog-threshold) 100)
              felt (truncate longest) gcs (truncate gcmax)
              (if (zerop all) "n/a" (format "%.1f %%" (* 100.0 (/ slow (float all)))))))))

(defun nidhogg-watchdog--rep-top (alist)
  "The first cell of the mix ALIST as a short string."
  (if alist (format "%s %s" (caar alist)
                    (if (integerp (cdar alist)) (cdar alist)
                      (format "%.0f%%" (* 100 (cdar alist)))))
    ""))

(defun nidhogg-watchdog--rep-entry (row indent)
  "The tabulated-list entry for the ranking ROW, its label indented INDENT."
  (list row
        (vector (concat (make-string indent ?\s) (plist-get row :name))
                (number-to-string (plist-get row :count))
                (number-to-string (plist-get row :felt))
                (format "%.0f" (plist-get row :total))
                (format "%.0f" (plist-get row :max))
                (format "%.0f" (plist-get row :p50))
                (nidhogg-watchdog--rep-top (plist-get row :states))
                (nidhogg-watchdog--rep-top (plist-get row :phases)))))

(defun nidhogg-watchdog--rep-num-sort (i)
  "A sort predicate for the numeric column I."
  (lambda (a b) (< (string-to-number (aref (cadr a) i))
                   (string-to-number (aref (cadr b) i)))))

(defvar-local nidhogg-watchdog--report-felt-only nil)

(define-derived-mode nidhogg-watchdog-report-mode tabulated-list-mode
  "Watchdog"
  "Major mode of `nidhogg-watchdog-report'.
\\{nidhogg-watchdog-report-mode-map}"
  (setq tabulated-list-use-header-line nil
        tabulated-list-format
        (vector '("Row" 44 t)
                (list "Count" 6 (nidhogg-watchdog--rep-num-sort 1))
                (list "Felt" 5 (nidhogg-watchdog--rep-num-sort 2))
                (list "Total ms" 9 (nidhogg-watchdog--rep-num-sort 3))
                (list "Max ms" 7 (nidhogg-watchdog--rep-num-sort 4))
                (list "p50 ms" 7 (nidhogg-watchdog--rep-num-sort 5))
                '("State" 12 t) '("Phase" 20 t)))
  (tabulated-list-init-header)
  (setq-local revert-buffer-function
              (lambda (&rest _) (nidhogg-watchdog--report-fill))))

(keymap-set nidhogg-watchdog-report-mode-map "RET" #'nidhogg-watchdog-report-open)

(defun nidhogg-watchdog--rep-midnight ()
  "Midnight of today, as a time."
  (let ((d (decode-time)))
    (encode-time (list 0 0 0 (nth 3 d) (nth 4 d) (nth 5 d) nil -1 nil))))

(defun nidhogg-watchdog--report-fill ()
  "Fill the current report buffer from today's day files."
  (let* ((lines (nidhogg-watchdog--report-lines (nidhogg-watchdog--rep-midnight)))
         (rows (nidhogg-watchdog--rank lines nidhogg-watchdog--report-felt-only))
         entries)
    (dolist (r rows)
      (push (nidhogg-watchdog--rep-entry r 0) entries)
      (dolist (s (plist-get r :signatures))
        (push (nidhogg-watchdog--rep-entry s 2) entries)))
    (setq header-line-format
          (concat " " (replace-regexp-in-string
                       "%" "%%" (nidhogg-watchdog--report-header lines)))
          tabulated-list-entries (nreverse entries))
    (tabulated-list-print t)))

;;;###autoload
(defun nidhogg-watchdog-report (&optional felt-only)
  "Show today's stalls ranked by package, the GC row on top.
RET on a row opens its call tree, on the GC row the day's collections.
With a prefix argument FELT-ONLY, only felt stalls and collections."
  (interactive "P")
  (require 'tabulated-list)
  (with-current-buffer (get-buffer-create "*Nidhogg Watchdog*")
    (nidhogg-watchdog-report-mode)
    (setq nidhogg-watchdog--report-felt-only felt-only)
    (nidhogg-watchdog--report-fill)
    (pop-to-buffer (current-buffer))))

(defun nidhogg-watchdog-report-open ()
  "Open the call tree of the row at point, or the day's collections for GC."
  (interactive)
  (let ((row (tabulated-list-get-id)))
    (unless row (user-error "No row here"))
    (if (plist-get row :gc)
        (with-current-buffer (get-buffer-create "*Nidhogg Watchdog GC*")
          (let ((inhibit-read-only t))
            (erase-buffer)
            (dolist (l (plist-get row :collections))
              (insert (format "%s  %7.1f ms  %-8s%s  heap %s MB\n"
                              (alist-get 'at l)
                              (nidhogg-watchdog--rep-num l 'pause_ms)
                              (or (alist-get 'origin l) "")
                              (if (alist-get 'felt l) "  felt" "")
                              (or (alist-get 'live_mb (alist-get 'heap l)) "?"))))
            (goto-char (point-min))
            (special-mode))
          (pop-to-buffer (current-buffer)))
      (with-current-buffer (get-buffer-create "*Nidhogg Watchdog Tree*")
        (let ((inhibit-read-only t))
          (erase-buffer)
          (insert (format "%s: %d stalls, %d felt, %.0f ms\n\n"
                          (plist-get row :name) (plist-get row :count)
                          (plist-get row :felt) (plist-get row :total)))
          (dolist (s (plist-get row :signatures))
            (insert (format "  %s  (%d)\n" (plist-get s :name) (plist-get s :count))))
          (insert "\n" (nidhogg-watchdog--rep-tree-string (plist-get row :tree)))
          (goto-char (point-min))
          (special-mode))
        (pop-to-buffer (current-buffer))))))


(provide 'nidhogg-watchdog-report)

;;; nidhogg-watchdog-report.el ends here
