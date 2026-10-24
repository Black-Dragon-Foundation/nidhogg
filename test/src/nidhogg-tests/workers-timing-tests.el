;;; workers-timing-tests.el --- tests for workers: the timing group (wide margins)  -*- lexical-binding: t -*-
;; test-class: timing

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
(require 'treesit)
(require 'nidhogg-workers-treesit)

(defconst workers-timing-tests--dir
  (file-name-directory (or load-file-name buffer-file-name "")))

(ert-deftest workers/wait-deadline ()
  ;; Review Focus 1, the deadline variant: a wait on a job that never
  ;; finishes, under a storm of SIGURG and SIGCHLD, returns 0 at its
  ;; 100 ms deadline (not before; within 1 s, a wide margin).
  (let ((nidhogg-workers--mode 'threads))
    (unwind-protect
        (progn
          (nidhogg-workers--pause-at 'before-done)
          (let ((j (nidhogg-workers--test-job 'nh-blk 0 :class 'file
                                              :pause 'before-done)))
            (should (nidhogg-workers--test-await-paused 'before-done 5000))
            (nidhogg-workers--test-signal-storm 300 nil)
            (let* ((t0 (float-time))
                   (r (nidhogg-workers--test-wait j 100))
                   (dt (- (float-time) t0)))
              (should (eql r 0))
              (should (>= dt 0.1))
              (should (< dt 1.0)))))
      (nidhogg-workers--release))))

(ert-deftest workers/ts-attempt-clock ()
  ;; §17.3 S1 against the real clock (`workers/ts-start-points' fixes the
  ;; bound): in a buffer that redisplay fontifies, a one-character edit's
  ;; reparse stays on the main thread (its estimate is under the inline
  ;; base, or its 16 ms attempt finishes), and a 500 KB yank's goes to a
  ;; job.  A one-character reparse costs in proportion to the
  ;; siblings of the edited repetition, with the workers off too (desktop
  ;; dev build, 2026-10-23: 2 MB of one-line functions 19 ms, 2 MB of
  ;; 50-statement functions 0.6 ms, 200 KB of those 0.1 ms): the wide
  ;; margin is that last buffer, far under the base.  Needs the C
  ;; grammar (fixtures/grammars.sh).
  (let* ((dir (or (getenv "NIDHOGG_TEST_GRAMMARS")
                  (expand-file-name "fixtures/grammars-cache"
                                    workers-timing-tests--dir)))
         (chunk (concat "int f (int a, int b) {\n"
                        (apply #'concat
                               (make-list
                                50 "  if (a > b) a = a * 2; else b = b + a;\n"))
                        "  return a;\n}\n"))
         (text (lambda (bytes)
                 (apply #'concat (make-list (/ bytes (length chunk)) chunk))))
         (stat (lambda (key) (plist-get (nidhogg-workers--test-ts-stats) key))))
    (skip-unless (file-exists-p (expand-file-name "libtree-sitter-c.so" dir)))
    (let ((treesit-extra-load-path (list dir)))
      (dolist (mode '(threads inline))
        (let ((nidhogg-workers--mode mode)
              (nidhogg-workers-treesit t))
          (ert-info ((format "mode %s" mode) :prefix "")
            (with-temp-buffer
              (insert (funcall text 200000))
              (let ((p (treesit-parser-create 'c nil t))
                    (demand
                     (lambda (p)
                       (let ((t0 (float-time))
                             (can (funcall stat :started-demand)))
                         (nidhogg-workers--test-fontifying (current-buffer)
                           (treesit-parser-changed-regions p))
                         (message "workers/ts-attempt-clock: %s: %.0f us"
                                  mode (* 1e6 (- (float-time) t0)))
                         (- (funcall stat :started-demand) can)))))
                (treesit-parser-root-node p)
                (ert-info ("one character" :prefix "")
                  (goto-char (/ (point-max) 2))
                  ;; Before a `;': the text stays valid C.
                  (skip-chars-forward "^;")
                  (insert " ")
                  ;; (Inline mode runs every offload decision as a job, done
                  ;; at its start.)
                  (should (= (funcall demand p) (if (eq mode 'threads) 0 1)))
                  (should (nidhogg-treesit-parser-ready-p p)))
                (ert-info ("a 500 KB yank" :prefix "")
                  ;; Its estimate: the edited bytes at a seeded rate.
                  (nidhogg-workers--test-ts-cost p 100000 100)
                  (goto-char (/ (point-max) 2))
                  (forward-line 0)
                  (insert-for-yank (funcall text 500000))
                  (should (= (funcall demand p) 1))
                  ;; Inline, the job was done at its start and published.
                  (when (eq mode 'threads)
                    (should (eql (nidhogg-workers--test-ts-wait p 120000)
                                 1))))))))))
    (should (nidhogg-workers--test-ts-await-idle 60000))))

(ert-deftest workers/autosave-snapshot ()
  ;; The auto-save snapshot's copy of 50 MB: into fresh memory, then into
  ;; the same memory.  The numbers go to the log; the copy is
  ;; fault-bound when the second takes at most half of the first.
  (let* ((r (nidhogg-workers--test-copy-bench (* 50 1024 1024)))
         (fresh (nth 0 r))
         (again (nth 2 r)))
    (message "workers/autosave-snapshot: fresh %.1f ms, %d faults; same %.1f ms, %d faults; %s"
             (/ fresh 1e6) (nth 1 r) (/ again 1e6) (nth 3 r)
             (if (<= (* 2 again) fresh) "fault-bound" "copy-bound"))
    (should (> fresh 0))
    (should (< (nth 3 r) (nth 1 r)))))

(defun workers-timing-tests--big-message (bytes)
  "A Content-Length framed notification `big' of about BYTES, unibyte.
Its params hold completion-shaped items, as a large completion answer."
  (let* ((item "{\"label\":\"workers_timing_item_%06d\",\"kind\":6,\"detail\":\"int (int a, int b)\",\"sortText\":\"%06d\",\"insertTextFormat\":1}")
         (n (/ bytes (length (format item 0 0))))
         (body (concat "{\"jsonrpc\":\"2.0\",\"method\":\"big\",\"params\":{\"items\":["
                       (mapconcat (lambda (i) (format item i i)) (number-sequence 1 n) ",")
                       "]}}")))
    (encode-coding-string
     (format "Content-Length: %d\r\n\r\n%s" (string-bytes body) body) 'utf-8-unix)))

(ert-deftest workers/lsp-burst-latency ()
  ;; §17.5: typing 100 characters (one every 20 ms, from a timer) while
  ;; a mock server sends a 3 MB message every 200 ms to a framed
  ;; jsonrpc connection: no stall of 100 ms or more has a sample in the
  ;; `json' or `filter' phase (the materializer's slices and the batch
  ;; function), every character is typed and every message dispatched.
  ;; The watchdog's 100 ms threshold against slices of 4 ms is the wide
  ;; margin.  Threads only: inline mode delivers every message unsliced.
  (skip-unless (and (featurep 'nidhogg-watchdog) (eq system-type 'gnu/linux)))
  (require 'cl-lib)
  (require 'jsonrpc)
  (require 'nidhogg-workers-jsonrpc)
  (let* ((nidhogg-workers--mode 'threads)
         (nidhogg-workers-process-reader t)
         (nidhogg-workers-reader-pipes t)
         (nidhogg-workers-framing t)
         (messages 10)
         (file (make-temp-file "workers-lsp-burst" nil ".bin"))
         (got 0) (typed 0) (stalls nil) (phases nil) conn)
    (unwind-protect
        (progn
          (let ((coding-system-for-write 'no-conversion))
            (write-region (workers-timing-tests--big-message 3000000) nil file nil 'silent))
          (setq conn
                (make-instance
                 'jsonrpc-process-connection
                 :name "wtt-lsp-burst" :events-buffer-config '(:size 0)
                 :process (lambda ()
                            (make-process
                             :name "wtt-lsp-burst" :connection-type 'pipe
                             :noquery t :coding 'utf-8-emacs-unix
                             :command (list "sh" "-c"
                                            (format "read go; for i in $(seq %d); do cat %s; sleep 0.2; done; cat"
                                                    messages (shell-quote-argument file)))))
                 :notification-dispatcher (lambda (_c _m _p) (setq got (1+ got)))
                 :request-dispatcher #'ignore))
          (should (eq (nidhogg-process-framing (jsonrpc--process conn))
                      'content-length))
          (with-temp-buffer
            (nidhogg-watchdog--start 100 600000 nil)
            (unwind-protect
                (let ((timer nil)
                      (buf (current-buffer))
                      (end (+ (float-time) 60)))
                  (nidhogg-watchdog--test-idle-wait 0.01)
                  (nidhogg-watchdog--take)
                  (nidhogg-workers-stats t)
                  (setq timer (run-at-time 0 0.02
                                           (lambda ()
                                             (when (< typed 100)
                                               (with-current-buffer buf
                                                 (insert ?x))
                                               (setq typed (1+ typed))))))
                  (unwind-protect
                      (progn
                        (process-send-string (jsonrpc--process conn) "go\n")
                        ;; Idle waits, as the command loop's: each ends
                        ;; the watchdog's span (`accept-process-output'
                        ;; would not), so a stall is one stretch of work.
                        (while (and (< (float-time) end)
                                    (or (< got messages) (< typed 100)))
                          (nidhogg-watchdog--test-idle-wait 0.02)))
                    (cancel-timer timer))
                  (nidhogg-watchdog--test-idle-wait 0.02)
                  (setq stalls (car (nidhogg-watchdog--take))
                        phases (plist-get (nidhogg-workers-stats) :phase-ms)))
              (nidhogg-watchdog--stop))
            (should (= typed 100))
            (should (= (buffer-size) 100)))
          (should (= got messages))
          (let ((from-json-or-filter
                 (cl-remove-if-not
                  (lambda (st)
                    (cl-some (lambda (s) (memq (plist-get s :phase) '(json filter)))
                             (plist-get st :samples)))
                  stalls)))
            (message "workers/lsp-burst-latency: %d stalls >= 100 ms (%s), %d from json/filter; json %.1f ms, filter %.1f ms"
                     (length stalls)
                     (mapconcat (lambda (st) (format "%.0f" (plist-get st :wall-ms)))
                                stalls " ")
                     (length from-json-or-filter)
                     (plist-get phases 'json) (plist-get phases 'filter))
            ;; The messages were built in the main thread's `json' slices.
            (should (> (plist-get phases 'json) 0))
            (should-not from-json-or-filter)))
      (when conn
        (let ((p (jsonrpc--process conn)))
          (set-process-sentinel p #'ignore)
          (delete-process p)
          (dolist (b (list (process-buffer p) (jsonrpc-stderr-buffer conn)))
            (when (buffer-live-p b) (kill-buffer b)))))
      (delete-file file))))

;;; workers-timing-tests.el ends here
