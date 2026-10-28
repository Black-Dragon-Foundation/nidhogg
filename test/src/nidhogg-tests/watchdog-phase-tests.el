;;; watchdog-phase-tests.el --- tests for the stall watchdog (phase)  -*- lexical-binding: t -*-

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

;; Phases, felt input and typing commands.  Phases are read
;; with `nidhogg-watchdog--test-phase' (the top of the phase stack) and,
;; where a sample must show the phase and the Lisp caller, by setting
;; `nidhogg-watchdog--test-sample-on-phase': the push of that phase takes
;; a synchronous sample into `nidhogg-watchdog--test-last-sample'.  The
;; felt and typing rows run on the virtual clock.  No monitor thread
;; runs (`:no-monitor').

;;; Code:

(require 'ert)

(defmacro wdph-running (&rest body)
  "Run BODY with the watchdog started without a monitor thread."
  (declare (indent 0) (debug t))
  `(progn
     (setq nidhogg-watchdog--test-last-sample nil)
     (nidhogg-watchdog--start 100 2000 nil :no-monitor)
     (unwind-protect
         (progn ,@body)
       (setq nidhogg-watchdog--test-sample-on-phase nil)
       (nidhogg-watchdog--set-typing-commands nil)
       (nidhogg-watchdog--test-clock nil)
       (nidhogg-watchdog--stop))))

(defconst wdph--t0 1000000000000
  "The virtual clock's origin, in ns (1000 s).")

(defun wdph--clock-at (ms)
  "Set the virtual clock to its origin plus MS milliseconds."
  (nidhogg-watchdog--test-clock (+ wdph--t0 (round (* ms 1000000)))))

(defun wdph--idle ()
  "One short responsive wait: it ends the span in progress."
  (nidhogg-watchdog--test-idle-wait 0.001))

(defun wdph--align ()
  "Put the virtual clock at its origin, end the span there, drop records."
  (wdph--clock-at 0)
  (wdph--idle)
  (nidhogg-watchdog--take))

(defun wdph--stall-ending-at (ms)
  "Move the virtual clock to MS, end the span; return its one stall."
  (wdph--clock-at ms)
  (wdph--idle)
  (let ((stalls (car (nidhogg-watchdog--take))))
    (should (= (length stalls) 1))
    (car stalls)))

(defun wdph--stat (key)
  (plist-get (nidhogg-watchdog--stats) key))

;;;; Phases

(defun wdph--search () (re-search-forward "b+" nil t))
(defun wdph--looking () (looking-at "a+"))
(defun wdph--match () (string-match "b+" "abc"))
(defun wdph--json-parse () (json-parse-string "[1]"))
(defun wdph--json-serialize () (json-serialize [1]))
(defun wdph--call-process () (call-process "true"))

(defun wdph--check (phase caller)
  "Call CALLER with the push of PHASE sampling; check that sample.
Its phase is PHASE and its innermost Lisp frame is CALLER (the frame of
the primitive itself, when it has one, comes first)."
  (setq nidhogg-watchdog--test-last-sample nil)
  (let ((nidhogg-watchdog--test-sample-on-phase phase))
    (funcall caller))
  (let* ((s nidhogg-watchdog--test-last-sample)
         (frames (plist-get s :frames)))
    (should s)
    (should (eq (plist-get s :phase) phase))
    (should (>= (length frames) 2))
    (should (or (eq (aref frames 0) caller) (eq (aref frames 1) caller))))
  (should (eq (nidhogg-watchdog--test-phase) 'lisp)))

(defun wdph--run-process (filter sentinel)
  "Run `echo x' with FILTER and SENTINEL until the sentinel ran (5 s at most)."
  (let* ((done nil)
         (proc (make-process
                :name "wdph" :command '("echo" "x") :connection-type 'pipe
                :noquery t :filter filter
                :sentinel (lambda (p e)
                            (when sentinel (funcall sentinel p e))
                            (setq done t)))))
    (unwind-protect
        (dotimes (_ 50)
          (unless done (accept-process-output nil 0.1)))
      (delete-process proc))
    done))

(ert-deftest watchdog/phases ()
  (wdph-running
    (with-temp-buffer
      (insert "abc")
      (goto-char (point-min))
      (wdph--check 'regex #'wdph--search)
      (goto-char (point-min))
      (wdph--check 'regex #'wdph--looking))
    (wdph--check 'regex #'wdph--match)
    (wdph--check 'json #'wdph--json-parse)
    (wdph--check 'json #'wdph--json-serialize)
    (wdph--check 'call-process #'wdph--call-process)
    ;; A process filter and a sentinel.
    (let ((seen nil))
      (setq nidhogg-watchdog--test-last-sample nil)
      (setq nidhogg-watchdog--test-sample-on-phase 'filter)
      (should (wdph--run-process
               (lambda (_p _o) (push (cons 'filter (nidhogg-watchdog--test-phase))
                                     seen))
               (lambda (_p _e) (push (cons 'sentinel (nidhogg-watchdog--test-phase))
                                     seen))))
      (setq nidhogg-watchdog--test-sample-on-phase nil)
      (should (eq (cdr (assq 'filter seen)) 'filter))
      (should (eq (cdr (assq 'sentinel seen)) 'sentinel))
      (should (eq (plist-get nidhogg-watchdog--test-last-sample :phase) 'filter)))
    (should (eq (nidhogg-watchdog--test-phase) 'lisp))))

(ert-deftest watchdog/process-wait-phase ()
  ;; A wait that is not responsive (here: under `inhibit-redisplay') is
  ;; busy and labelled `process-wait'; after it returns, the rest of the
  ;; span is not.
  (wdph-running
    (setq nidhogg-watchdog--test-last-sample nil)
    (let ((nidhogg-watchdog--test-sample-on-phase 'process-wait)
          (inhibit-redisplay t))
      (nidhogg-watchdog--test-idle-wait 0.01))
    (should (eq (plist-get nidhogg-watchdog--test-last-sample :phase)
                'process-wait))
    (should (eq (nidhogg-watchdog--test-phase) 'lisp))))

(ert-deftest watchdog/phase-restored-after-error ()
  ;; A non-local exit pops the phases pushed inside its catch target
  ;; (`unwind_to_catch'): an error out of `json-parse-string', a `throw' out of a
  ;; filter.
  (wdph-running
    (should (eq (condition-case nil (json-parse-string "{") (error 'caught))
                'caught))
    (should (eq (nidhogg-watchdog--test-phase) 'lisp))
    (should (eq (catch 'wdph--out
                  (wdph--run-process
                   (lambda (_p _o) (throw 'wdph--out 'thrown)) nil)
                  'never)
                'thrown))
    (should (eq (nidhogg-watchdog--test-phase) 'lisp))))

(ert-deftest watchdog/phase-kept-across-an-inner-catch ()
  ;; A catch established inside the phase and thrown to inside it, and
  ;; an error caught inside it, leave the phase alone.
  (wdph-running
    (let ((seen nil))
      (should (wdph--run-process
               (lambda (_p _o)
                 (catch 'wdph--in (throw 'wdph--in nil))
                 (push (nidhogg-watchdog--test-phase) seen)
                 (condition-case nil (json-parse-string "{") (error nil))
                 (push (nidhogg-watchdog--test-phase) seen))
               nil))
      (should (equal seen '(filter filter))))
    (should (eq (nidhogg-watchdog--test-phase) 'lisp))))

;;;; Felt input

(ert-deftest watchdog/felt-from-stored-input ()
  (wdph-running
    (wdph--align)
    ;; Stored at 20 ms of a 300 ms span: felt, waited 280 ms.
    (wdph--clock-at 20)
    (nidhogg-watchdog--test-input-stored 'key nil)
    (let ((s (wdph--stall-ending-at 300)))
      (should (eq (plist-get s :felt) t))
      (should (= (plist-get s :input-wait-ms) 280.0))
      (should (eq (plist-get s :kind) 'other)))
    ;; The event's own timestamp (CLOCK_MONOTONIC ms) wins over the
    ;; store time: it arrived 10 ms into the span.
    (wdph--clock-at 320)
    (nidhogg-watchdog--test-input-stored 'button (+ (/ wdph--t0 1000000) 310))
    (let ((s (wdph--stall-ending-at 600)))
      (should (eq (plist-get s :felt) t))
      (should (= (plist-get s :input-wait-ms) 290.0)))
    ;; No input, or an event that is not input: not felt.
    (wdph--clock-at 650)
    (nidhogg-watchdog--test-input-stored 'other nil)
    (let ((s (wdph--stall-ending-at 800)))
      (should-not (plist-get s :felt))
      (should-not (plist-get s :input-wait-ms)))
    (should (eql (wdph--stat :requests) 0))))

(ert-deftest watchdog/command-entry-is-felt ()
  ;; A stall that ran a command is felt: its input waits for the result.
  (wdph-running
    (wdph--align)
    (nidhogg-watchdog--test-command 'forward-char)
    (let ((s (wdph--stall-ending-at 150)))
      (should (eq (plist-get s :felt) t))
      (should (eq (plist-get s :kind) 'command))
      (should (eql (plist-get s :commands) 1))
      (should (= (plist-get s :input-wait-ms) 150.0))
      (should-not (plist-get s :input-queued)))
    (should (eql (wdph--stat :requests) 0))))

;;;; Commands and typing

(ert-deftest watchdog/typing-histogram ()
  ;; Two typing commands of 10 and 20 ms: buckets <16 and <33.  The
  ;; first ends when the next command starts, the second at the wait.  A
  ;; command outside the typing set adds nothing.
  (wdph-running
    (should (eql (nidhogg-watchdog--set-typing-commands
                  '(self-insert-command delete-backward-char))
                 2))
    (wdph--align)
    (nidhogg-watchdog--stats t)
    (nidhogg-watchdog--test-command 'self-insert-command)
    (wdph--clock-at 10)
    (nidhogg-watchdog--test-command 'delete-backward-char)
    (wdph--clock-at 30)
    (wdph--idle)
    (nidhogg-watchdog--test-command 'forward-char)
    (wdph--clock-at 80)
    (wdph--idle)
    (should (equal (plist-get (wdph--stat :histograms) :typing)
                   [0 0 1 1 0 0 0 0 0]))
    (should-error (nidhogg-watchdog--set-typing-commands (make-list 17 'x)))))

(ert-deftest watchdog/merged-span-counts-commands ()
  ;; Three commands in one span (keys queued behind each other).
  (wdph-running
    (wdph--align)
    (nidhogg-watchdog--test-command 'next-line)
    (wdph--clock-at 50)
    (nidhogg-watchdog--test-command 'next-line)
    (wdph--clock-at 100)
    (nidhogg-watchdog--test-command 'next-line)
    (let ((s (wdph--stall-ending-at 150)))
      (should (eql (plist-get s :commands) 3))
      (should (eq (plist-get s :input-queued) t))
      (should (eq (plist-get s :kind) 'command)))))

;;; watchdog-phase-tests.el ends here
