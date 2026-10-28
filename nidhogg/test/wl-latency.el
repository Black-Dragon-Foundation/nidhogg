;;; wl-latency.el --- key-to-commit latency and frame intervals from a WAYLAND_DEBUG=client trace  -*- lexical-binding: t -*-

;; budgets.sh measures the shell (A, C) and pgtk (B) alike: both clients run with WAYLAND_DEBUG=client, and this file
;; reads the same events on each side at the trace's microsecond resolution.
;;
;;   key-to-commit  the dispatch of `wl_keyboard.key' (pressed) -> the next `wl_surface.commit' on the toplevel tree
;;                  (an xdg_toplevel's surface and its subsurfaces) that carries damage
;;   interval       the time between consecutive damaged commits of the tree
;;
;;   nidhogg -Q --batch -l wl-latency.el -f wl-latency-batch TRACE KIND FROM TO DROP PERIOD-MS
;;
;; KIND is `typing' (prints the key-to-commit statistics) or `scroll' (the frame intervals).  FROM and TO are byte
;; offsets into TRACE: only events inside count (the object tables are built from the whole file, so a window can
;; start after the toplevel was made).  DROP is how many leading keys to ignore; PERIOD-MS is one refresh period.
;; The result is one plist on stdout:
;;   typing  (:n N :p50 X :p95 X :p99 X :max X)                  milliseconds
;;   scroll  (:frames N :late L :per1000 X :max X :period P)    L = intervals above 1.5 periods
;; Exit 0, or 2 when the window holds nothing to measure.

;;; Code:

(require 'cl-lib)
(eval-and-compile
  (add-to-list 'load-path (file-name-directory (or load-file-name buffer-file-name default-directory))))
(require 'wl-trace-check)

(defun wl-latency--tree-p (surface toplevels parents)
  "Non-nil when SURFACE is a toplevel's surface, or a subsurface (at any depth) of one."
  (let ((s surface) (n 0) found)
    (while (and s (not found) (< n 16))
      (if (gethash s toplevels) (setq found t) (setq s (gethash s parents) n (1+ n))))
    found))

(defun wl-latency-analyze (file &optional from to)
  "Read FILE; return (KEYS . COMMITS) for the events between byte offsets FROM and TO.
KEYS is the key-to-commit latencies in ms, in key order; COMMITS the times (ms of trace time) of the tree's damaged
commits.  A key whose commit never came is not in KEYS."
  (let ((xdg-surface (make-hash-table)) (toplevels (make-hash-table)) (parents (make-hash-table))
        (damage (make-hash-table)) pending keys commits)
    (with-temp-buffer
      (insert-file-contents-literally file)
      (goto-char (point-min))
      (while (re-search-forward wl-trace-check--line-re nil t)
        (let* ((pos (1- (match-beginning 0)))
               (in (and (or (null from) (>= pos from)) (or (null to) (<= pos to))))
               (request (match-beginning 2))
               (iface (match-string 3)) (id (string-to-number (match-string 4)))
               (msg (match-string 5)) (args (match-string 6))
               (time (wl-trace-check--ms (match-string 1))))
          (cond
           ((and request (equal iface "xdg_wm_base") (equal msg "get_xdg_surface"))
            (let ((refs (wl-trace-check--refs args)))
              (puthash (cdr (nth 0 refs)) (cdr (nth 1 refs)) xdg-surface)))
           ((and request (equal iface "xdg_surface") (equal msg "get_toplevel"))
            (let ((s (gethash id xdg-surface))) (when s (puthash s t toplevels))))
           ((and request (equal iface "wl_subcompositor") (equal msg "get_subsurface"))
            (let ((refs (wl-trace-check--refs args)))
              (puthash (cdr (nth 1 refs)) (cdr (nth 2 refs)) parents)))
           ((and request (equal iface "wl_surface") (member msg '("damage" "damage_buffer")))
            (puthash id t damage))
           ((and request (equal iface "wl_surface") (equal msg "commit"))
            (when (and (gethash id damage) (wl-latency--tree-p id toplevels parents))
              (when in (push time commits))
              (dolist (k pending) (push (- time k) keys))
              (setq pending nil))
            (remhash id damage))
           ((and (not request) (equal iface "wl_keyboard") (equal msg "key"))
            (when (and in (string-suffix-p ", 1" args)) (setq pending (append pending (list time)))))))))
    (cons (nreverse keys) (nreverse commits))))

(defun wl-latency--percentile (sorted p)
  "The P (0..1) nearest-rank percentile of the sorted list SORTED."
  (nth (max 0 (1- (ceiling (* p (length sorted))))) sorted))

(defun wl-latency-key-to-commit (file &optional from to drop)
  "The plist (:n :p50 :p95 :p99 :max) of FILE's key-to-commit latencies, ignoring the first DROP keys; nil if none."
  (let ((lat (nthcdr (or drop 0) (car (wl-latency-analyze file from to)))))
    (when lat
      (let ((s (sort (copy-sequence lat) #'<)))
        (list :n (length s) :p50 (wl-latency--percentile s 0.5) :p95 (wl-latency--percentile s 0.95)
              :p99 (wl-latency--percentile s 0.99) :max (car (last s)))))))

(defun wl-latency-commit-intervals (file &optional from to period)
  "The plist (:frames :late :per1000 :max :period) of FILE's damaged-commit intervals; nil if fewer than two commits.
An interval above 1.5 PERIOD (ms, default 8.333) is late."
  (let* ((period (or period 8.333)) (c (cdr (wl-latency-analyze file from to))) iv)
    (while (cdr c) (push (- (cadr c) (car c)) iv) (setq c (cdr c)))
    (when iv
      (let ((late (cl-count-if (lambda (d) (> d (* 1.5 period))) iv)))
        (list :frames (length iv) :late late :per1000 (/ (* 1000.0 late) (length iv))
              :max (apply #'max iv) :period period)))))

(defun wl-latency-batch ()
  "Batch entry: TRACE KIND FROM TO DROP PERIOD-MS from the command line; print the plist."
  (let* ((a command-line-args-left) (file (nth 0 a)) (kind (nth 1 a))
         (from (and (nth 2 a) (string-to-number (nth 2 a)))) (to (and (nth 3 a) (string-to-number (nth 3 a))))
         (drop (string-to-number (or (nth 4 a) "0"))) (period (string-to-number (or (nth 5 a) "8.333")))
         (r (cond ((equal kind "typing") (wl-latency-key-to-commit file from to drop))
                  ((equal kind "scroll") (wl-latency-commit-intervals file from to period)))))
    (setq command-line-args-left nil)
    (princ (format "%S\n" r))
    (kill-emacs (if r 0 2))))

(ert-deftest wl-latency/key-to-commit ()
  (let ((file (make-temp-file "wl-latency")))
    (unwind-protect
        (progn
          (with-temp-file file
            (insert
             "[1000.000] {Default Queue}  -> xdg_wm_base#6.get_xdg_surface(new id xdg_surface#14, wl_surface#12)\n"
             "[1000.001] {Default Queue}  -> xdg_surface#14.get_toplevel(new id xdg_toplevel#15)\n"
             "[1000.002] {Default Queue}  -> wl_subcompositor#5.get_subsurface(new id wl_subsurface#20, wl_surface#13, wl_surface#12)\n"
             "[1100.000] {Default Queue} wl_keyboard#7.key(1, 1100, 30, 1)\n"
             "[1100.500] {Default Queue} wl_keyboard#7.key(2, 1100, 30, 0)\n"
             ;; a commit with no damage, and one on an unrelated surface: neither counts
             "[1101.000] {Default Queue}  -> wl_surface#12.commit()\n"
             "[1101.500] {Default Queue}  -> wl_surface#99.damage_buffer(0, 0, 4, 4)\n"
             "[1101.600] {Default Queue}  -> wl_surface#99.commit()\n"
             ;; the subsurface's damaged commit does
             "[1104.250] {Default Queue}  -> wl_surface#13.damage_buffer(0, 0, 4, 4)\n"
             "[1104.500] {Default Queue}  -> wl_surface#13.commit()\n"
             "garbage that is not a trace line\n"
             "[1200.000] {Default Queue} wl_keyboard#7.key(3, 1200, 31, 1)\n"
             "[1203.000] {Default Queue}  -> wl_surface#12.damage(0, 0, 4, 4)\n"
             "[1203.125] {Default Queue}  -> wl_surface#12.commit()\n"))
          (let ((r (wl-latency-analyze file)))
            (should (equal (mapcar (lambda (x) (/ (round (* x 1000)) 1000.0)) (car r)) '(4.5 3.125))))
          (let ((r (wl-latency-key-to-commit file nil nil 1)))
            (should (= (plist-get r :n) 1))
            (should (= (plist-get r :max) 3.125)))
          ;; a window that starts after the first key leaves one
          (let* ((text (with-temp-buffer (insert-file-contents-literally file) (buffer-string)))
                 (r (wl-latency-analyze file (string-search "[1200.000]" text))))
            (should (= (length (car r)) 1))))
      (delete-file file))))

(ert-deftest wl-latency/commit-intervals ()
  (let ((file (make-temp-file "wl-latency")))
    (unwind-protect
        (progn
          (with-temp-file file
            (insert
             "[1000.000]  -> xdg_wm_base#6.get_xdg_surface(new id xdg_surface#14, wl_surface#12)\n"
             "[1000.001]  -> xdg_surface#14.get_toplevel(new id xdg_toplevel#15)\n")
            ;; commits at 0, 8, 16, 32 (a 16 ms gap), 40 ms: intervals 8 8 16 8
            (dolist (ms '(1100.0 1108.0 1116.0 1132.0 1140.0))
              (insert (format "[%.3f]  -> wl_surface#12.damage_buffer(0, 0, 4, 4)\n[%.3f]  -> wl_surface#12.commit()\n"
                              ms (+ ms 0.1)))))
          (let ((r (wl-latency-commit-intervals file nil nil 8.333)))
            (should (= (plist-get r :frames) 4))
            (should (= (plist-get r :late) 1))
            (should (= (round (plist-get r :per1000)) 250))
            (should (= (round (plist-get r :max)) 16))))
      (delete-file file))))

(provide 'wl-latency)
;;; wl-latency.el ends here
