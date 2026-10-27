;;; watchdog-sample-tests.el --- tests for the stall watchdog (sample)  -*- lexical-binding: t -*-

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

;; The signal handler, the backtrace copy, the raw ring and the
;; validation of raw words.  Samples are synchronous: the test
;; primitives raise the signal on the calling thread with
;; `pthread_kill (pthread_self ())', so the handler runs inside a known
;; Lisp stack.  No monitor thread runs (`:no-monitor').

;;; Code:

(require 'ert)
(require 'cl-lib)

(defmacro wdtest-sampling (&rest body)
  "Run BODY with the watchdog started without a monitor thread."
  (declare (indent 0) (debug t))
  `(progn
     (setq wdtest--now nil)
     (nidhogg-watchdog--start 100 2000 nil :no-monitor)
     (unwind-protect
         (progn ,@body)
       (nidhogg-watchdog--test-set-unstable nil)
       (nidhogg-watchdog--test-force-lisp-thread nil)
       (nidhogg-watchdog--deep 0)
       (nidhogg-watchdog--test-clock nil)
       (nidhogg-watchdog--stop))))

(defun wdtest--stale ()
  (plist-get (nidhogg-watchdog--stats) :stale))

(defun wdtest--deeper (n thunk)
  "Call THUNK N frames deeper, so a sample has outermost frames."
  (if (> n 0) (wdtest--deeper (1- n) thunk) (funcall thunk)))

(defun wdtest--recurse (n)
  (if (> n 0) (wdtest--recurse (1- n)) (nidhogg-watchdog--test-sample)))

(defconst wdtest--t0 1000000000000
  "The virtual clock's origin, in ns (1000 s).")

(defvar wdtest--now nil
  "The virtual clock's current value, in ns; nil before the first span.")

(defun wdtest--span (ms thunk)
  "Run THUNK in a fresh busy span that lasts MS on the virtual clock.
Return the stalls taken after the span ended."
  (setq wdtest--now (or wdtest--now wdtest--t0))
  (nidhogg-watchdog--test-clock wdtest--now)
  (nidhogg-watchdog--test-idle-wait 0.001)  ; end the span in progress
  (nidhogg-watchdog--take)                  ; and drop what it left
  (funcall thunk)
  (setq wdtest--now (+ wdtest--now (* ms 1000000)))
  (nidhogg-watchdog--test-clock wdtest--now)
  (nidhogg-watchdog--test-idle-wait 0.001)  ; the span ends at MS
  (car (nidhogg-watchdog--take)))

(defvar wdtest-hook nil)
(defvar wdtest--hook-sample nil)

(defun wdtest--hook-fn ()
  (setq wdtest--hook-sample (nidhogg-watchdog--test-sample)))

(defun wdtest--ert-frame-p (f)
  (and (symbolp f) (string-prefix-p "ert-" (symbol-name f))))

(ert-deftest watchdog/stats-count-samples ()
  ;; `--stats' counts the samples the handler kept (`:samples'), so a
  ;; reader can set `:missed' against them.
  (wdtest-sampling
    (let ((before (plist-get (nidhogg-watchdog--stats) :samples)))
      (should (natnump before))
      (should (nidhogg-watchdog--test-sample))
      (should (= (1+ before) (plist-get (nidhogg-watchdog--stats) :samples))))))

(ert-deftest watchdog/sample-attribution ()
  ;; Interpreted, then byte-compiled: the frames are the same symbols.
  (eval '(progn
           (defun wdtest-inner () (nidhogg-watchdog--test-sample))
           (defun wdtest-outer () (wdtest-inner)))
        t)
  (wdtest-sampling
    (dolist (compiled '(nil t))
      (when compiled
        (byte-compile 'wdtest-inner)
        (byte-compile 'wdtest-outer))
      (should (eq (byte-code-function-p (symbol-function 'wdtest-inner))
                  compiled))
      (let* ((s (wdtest--deeper 30 #'wdtest-outer))
             (frames (append (plist-get s :frames) nil))
             (outer (append (plist-get s :outer) nil)))
        (should (= 24 (length frames)))
        (should (eq (nth 0 frames) 'nidhogg-watchdog--test-sample))
        (should (memq 'wdtest-inner frames))
        (should (< (cl-position 'wdtest-inner frames)
                   (cl-position 'wdtest-outer frames)))
        (should (<= 1 (length outer) 8))
        (should (cl-some #'wdtest--ert-frame-p outer))
        (should (< 0 (plist-get s :skipped)))
        (should (floatp (plist-get s :t-ms)))
        (should (eq 'lisp (plist-get s :phase)))
        (should-not (plist-get s :reason))
        (should-not (plist-get s :gc))
        (should-not (plist-get s :lisp-thread))))
    ;; A hook runner below the innermost frame records the hook's name.
    (setq wdtest--hook-sample nil)
    (add-hook 'wdtest-hook #'wdtest--hook-fn)
    (unwind-protect
        (run-hooks 'wdtest-hook)
      (remove-hook 'wdtest-hook #'wdtest--hook-fn))
    (let* ((s wdtest--hook-sample)
           (frames (append (plist-get s :frames) nil))
           (hooks (plist-get s :hooks))
           (i (cl-position 'run-hooks frames)))
      (should i)
      (should (eq 'wdtest-hook (aref hooks i)))
      (should (= (length hooks)
                 (+ (length frames) (length (plist-get s :outer)))))
      (should (= 1 (cl-count-if #'identity hooks))))))

(ert-deftest watchdog/guards ()
  (wdtest-sampling
    ;; The specpdl is being reallocated: no walk, the metadata is kept.
    (nidhogg-watchdog--test-set-unstable t)
    (let ((s (unwind-protect (nidhogg-watchdog--test-sample)
               (nidhogg-watchdog--test-set-unstable nil))))
      (should s)
      (should (= 0 (length (plist-get s :frames))))
      (should (= 0 (length (plist-get s :outer))))
      (should (floatp (plist-get s :t-ms)))
      (should (eq 'lisp (plist-get s :phase)))
      (should-not (plist-get s :lisp-thread)))
    ;; A Lisp thread holds the lock: no walk, the sample says so.
    (nidhogg-watchdog--test-force-lisp-thread t)
    (let ((s (unwind-protect (nidhogg-watchdog--test-sample)
               (nidhogg-watchdog--test-force-lisp-thread nil))))
      (should (plist-get s :lisp-thread))
      (should (= 0 (length (plist-get s :frames)))))
    (should (plist-get (nidhogg-watchdog--test-accumulator) :lisp-thread))
    ;; Both guards off again: frames come back.
    (should (< 0 (length (plist-get (nidhogg-watchdog--test-sample)
                                    :frames))))))

(ert-deftest watchdog/raw-word-validation ()
  (wdtest-sampling
    (let* ((live (let ((n 0)) (lambda () (setq n (1+ n)))))
           (planted (list #x5a5a5a5a5a5a5a58  ; a symbol-tagged pattern
                          (nidhogg-watchdog--test-word :c-function)
                          (nidhogg-watchdog--test-word :text-vectorlike)
                          (nidhogg-watchdog--test-freed-word)
                          (nidhogg-watchdog--test-word live)
                          (nidhogg-watchdog--test-word 'car)
                          (nidhogg-watchdog--test-word (symbol-function 'car)))))
      (should (closurep live))
      (nidhogg-watchdog--test-plant-words planted)
      ;; Nothing in the ring is a GC root: a collection between the
      ;; plant and the take neither marks nor frees through it.
      (garbage-collect)
      (let* ((samples (plist-get (nidhogg-watchdog--test-accumulator) :samples))
             (frames (plist-get (aref samples (1- (length samples))) :frames)))
        (should (= 7 (length frames)))
        (dotimes (i 4)
          (should (eq 'nidhogg-watchdog--gone (aref frames i))))
        (should (eq live (aref frames 4)))
        (should (eq 'car (aref frames 5)))
        (should (eq (symbol-function 'car) (aref frames 6)))))))

(ert-deftest watchdog/thinning ()
  ;; 256 requests: thinned at 64 and 128 kept, every 4th request kept.
  (wdtest-sampling
    (should (= 256 (nidhogg-watchdog--test-raw-samples 256)))
    (let* ((acc (nidhogg-watchdog--test-accumulator))
           (samples (append (plist-get acc :samples) nil))
           (times (mapcar (lambda (s) (plist-get s :t-ms)) samples)))
      (should (= 64 (length samples)))
      (should (= 4 (plist-get acc :stride)))
      (should (= 64 (plist-get acc :thinned)))
      (should (equal (number-sequence 1 253 4)
                     (append (plist-get acc :seq) nil)))
      (should (cl-every #'<= times (cdr times)))))
  ;; 200 requests: 50 kept at stride 4.
  (wdtest-sampling
    (nidhogg-watchdog--test-raw-samples 200)
    (let ((acc (nidhogg-watchdog--test-accumulator)))
      (should (= 50 (length (plist-get acc :samples))))
      (should (= 4 (plist-get acc :stride)))
      (should (equal (number-sequence 1 197 4)
                     (append (plist-get acc :seq) nil))))))

(ert-deftest watchdog/deep-mode ()
  (wdtest-sampling
    (nidhogg-watchdog--deep 1)
    (let ((s (wdtest--recurse 60)))
      (should (= 56 (length (plist-get s :frames))))
      (should (= 8 (length (plist-get s :outer))))
      (should (< 0 (plist-get s :skipped))))
    (should (plist-get (nidhogg-watchdog--test-accumulator) :deep))
    ;; A deep stall keeps 32 samples, not 64.
    (nidhogg-watchdog--test-raw-samples 100)
    (should (<= (length (plist-get (nidhogg-watchdog--test-accumulator)
                                   :samples))
                32))))

(ert-deftest watchdog/span-isolation ()
  ;; A 70 ms span with 70 samples (thinned once: stride 2) ends below
  ;; the threshold; the next span's 150 ms stall holds only its own 3
  ;; samples, taken at stride 1.
  (wdtest-sampling
    (should-not (wdtest--span 70 (lambda ()
                                   (nidhogg-watchdog--test-raw-samples 70))))
    (let* ((stalls (wdtest--span 150 (lambda ()
                                       (nidhogg-watchdog--test-raw-samples 3))))
           (rec (car stalls)))
      (should (= 1 (length stalls)))
      (should (= 3 (length (plist-get rec :samples))))
      (should (= 0 (plist-get rec :thinned))))
    ;; A 256-sample stall's record carries the thinning.
    (let ((rec (car (wdtest--span 150 (lambda ()
                                        (nidhogg-watchdog--test-raw-samples 256))))))
      (should (= 64 (length (plist-get rec :samples))))
      (should (= 64 (plist-get rec :thinned))))))

(ert-deftest watchdog/late-request-is-stale ()
  ;; A request raised for a span that has ended lands in the wait.
  (wdtest-sampling
    (nidhogg-watchdog--test-sample)     ; the span has an accumulator
    (let ((stale (wdtest--stale))
          (n (length (plist-get (nidhogg-watchdog--test-accumulator)
                                :samples))))
      (should (= 0 (nidhogg-watchdog--test-forge-late-request)))
      (should (= (1+ stale) (wdtest--stale)))
      (should (= n (length (plist-get (nidhogg-watchdog--test-accumulator)
                                      :samples)))))))

(ert-deftest watchdog/signal-is-ignored-elsewhere ()
  ;; A SIGURG sent to the process, with no request pending.
  (wdtest-sampling
    (nidhogg-watchdog--test-sample)
    (let ((stale (wdtest--stale))
          (acc (nidhogg-watchdog--test-accumulator)))
      (signal-process (emacs-pid) 'SIGURG)
      (should (= (1+ stale) (wdtest--stale)))
      (should (= (length (plist-get acc :samples))
                 (length (plist-get (nidhogg-watchdog--test-accumulator)
                                    :samples)))))))

(ert-deftest watchdog/handler-sa-restart ()
  (wdtest-sampling
    (let ((sa-restart (nidhogg-watchdog--test-handler-flags t)))
      (should (< 0 sa-restart))
      (should (/= 0 (logand (nidhogg-watchdog--test-handler-flags)
                            sa-restart))))))

(ert-deftest watchdog/handler-allocates-nothing ()
  (wdtest-sampling
    (let* ((a (memory-use-counts))
           (b (progn (identity 1000) (memory-use-counts)))
           (c (progn (nidhogg-watchdog--test-raw-samples 1000)
                     (memory-use-counts))))
      (should (equal (cl-mapcar #'- b a) (cl-mapcar #'- c b))))))

;;; watchdog-sample-tests.el ends here
