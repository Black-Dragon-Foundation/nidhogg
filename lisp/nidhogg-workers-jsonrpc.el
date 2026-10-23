;;; nidhogg-workers-jsonrpc.el --- Nidhogg's worker threads: jsonrpc adoption  -*- lexical-binding: t -*-

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

;; Loaded after `jsonrpc' (nidhogg-workers.el).  With
;; `nidhogg-workers-framing' on, every `jsonrpc-process-connection'
;; whose process the reader thread reads gets the `content-length'
;; framing: the reader thread finds the messages and tokenizes them, and
;; the main thread gets them in batches.  The batch function runs
;; jsonrpc's filter tail unchanged (jsonrpc.el:879-897): it pushes each
;; message onto `jsonrpc-mqueue' with `:jsonrpc-json', then arms one
;; timer per message with one shared time, in reverse push order, so the
;; messages are dispatched in arrival order as today.
;;
;; Fallbacks, all to `jsonrpc--process-filter': a process that cannot be
;; framed (`nidhogg-framing-unsupported'), advice on `jsonrpc--json-read'
;; or `jsonrpc--process-filter' (checked when the connection starts and
;; at every batch, so advice added later, such as eglot-booster's, still
;; sees every message), a framing error (junk before a header: the filter
;; skips it as today), and a replaced filter (the reader turns framing
;; off by itself).  jsonrpc.el itself is not changed.

;;; Code:

(require 'cl-lib)
(require 'jsonrpc)

(defvar nidhogg-workers-framing)
(declare-function nidhogg-set-process-framing "nidhogg-io.c")

(defconst nidhogg-workers-jsonrpc--filter-sha256
  "a59d560ebea62737190185428970bc1808b51ffbc9684baf0886266bc5f43ccc"
  "SHA-256 of `jsonrpc--process-filter's source in lisp/jsonrpc.el.
From its opening parenthesis to its closing one, as the batch function
below copies its tail (jsonrpc 1.0.29).  When jsonrpc.el changes, the
test `workers/jsonrpc-tail-unchanged' fails: compare the copy with the
new filter first, then record the new hash.")

(defun nidhogg-workers-jsonrpc--advised-p ()
  "Non-nil if `jsonrpc--json-read' or `jsonrpc--process-filter' has advice."
  (or (advice--p (advice--symbol-function 'jsonrpc--json-read))
      (advice--p (advice--symbol-function 'jsonrpc--process-filter))))

(defun nidhogg-workers-jsonrpc--fall-back (proc batch)
  "Turn PROC's framing off and give BATCH's bytes to jsonrpc's filter.
The bytes framing has not delivered yet follow as raw output."
  (nidhogg-set-process-framing proc nil)
  (dolist (entry batch)
    (let ((raw (nth 1 entry)))
      (jsonrpc--process-filter
       proc (concat "Content-Length: " (number-to-string (length raw))
                    "\r\n\r\n" raw)))))

(defun nidhogg-workers-jsonrpc--batch (proc batch)
  "Queue BATCH's messages for PROC's connection, as jsonrpc's filter does.
BATCH is a list of (MESSAGE RAW ERROR) in arrival order (see
`nidhogg-set-process-framing')."
  (if (nidhogg-workers-jsonrpc--advised-p)
      (nidhogg-workers-jsonrpc--fall-back proc batch)
    (let ((conn (process-get proc 'jsonrpc-connection))
          (failed nil))
      (unwind-protect
          (dolist (entry batch)
            (let ((message (nth 0 entry))
                  (raw (nth 1 entry))
                  (err (nth 2 entry)))
              (if err
                  (jsonrpc--warn "Invalid JSON: %s %s" (cdr err) raw)
                (when message
                  ;; As jsonrpc's filter, which signals here for a
                  ;; message that is no object; the batch's later
                  ;; messages still go on.
                  (condition-case oops
                      (push (plist-put message :jsonrpc-json raw)
                            (process-get proc 'jsonrpc-mqueue))
                    (error (unless failed (setq failed oops))))))))
        ;; jsonrpc.el:883-897, unchanged.
        (cl-loop
         ;; `timer-activate' orders timers by time, which is an
         ;; very expensive operation when jsonrpc-mqueue is large,
         ;; therefore the time object is reused for each timer
         ;; created.
         with time = (current-time)
         for msg = (pop (process-get proc 'jsonrpc-mqueue)) while msg
         do (let ((timer (timer-create)))
              (timer-set-time timer time)
              (timer-set-function timer
                                  (lambda (conn msg)
                                    (with-temp-buffer
                                      (jsonrpc-connection-receive conn msg)))
                                  (list conn msg))
              (timer-activate timer))))
      (when failed
        (signal (car failed) (cdr failed))))))

(cl-defmethod initialize-instance :around ((conn jsonrpc-process-connection)
                                           _slots)
  "Frame CONN's process when `nidhogg-workers-framing' is on.
Not when the process cannot be framed or jsonrpc's reader has advice:
then jsonrpc's filter reads it."
  (prog1 (cl-call-next-method)
    (when (and (bound-and-true-p nidhogg-workers-framing)
               (fboundp 'nidhogg-set-process-framing)
               (not (nidhogg-workers-jsonrpc--advised-p)))
      (condition-case nil
          (nidhogg-set-process-framing
           (jsonrpc--process conn) 'content-length
           :parse 'buffer :object-type 'plist :null-object nil
           :false-object :json-false
           :batch-function #'nidhogg-workers-jsonrpc--batch)
        (nidhogg-framing-unsupported nil)))))

(provide 'nidhogg-workers-jsonrpc)

;;; nidhogg-workers-jsonrpc.el ends here
