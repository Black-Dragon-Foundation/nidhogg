;;; nidhogg-test-peer.el --- scripted process peers for tests  -*- lexical-binding: t -*-

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

;; A test that needs "a process wrote something" should not ask the OS
;; for the order or the chunking of the reads.  A peer is a process
;; whose input the test writes itself: `nidhogg-test-peer-write' reaches
;; the peer's filter as exactly one call, and returns after the filter
;; ran.  No child process, no sleep, no timer.
;;
;; The peer is the receiving end of a connected pair of local sockets
;; inside this Emacs (`make-pipe-process' has no loopback: what one
;; writes to it goes to a pipe nobody reads).  A stream socket may split
;; or join writes, so the peer counts the bytes of a write and calls the
;; filter once they have all arrived.
;;
;; The deadline of a write bounds a failure only (a filter that never
;; runs); a passing write never waits on it.

;;; Code:

(require 'cl-lib)

(cl-defstruct (nidhogg-test-peer--state
               (:constructor nidhogg-test-peer--state-make))
  filter coding (want 0) (have 0) (pending nil) (calls 0)
  server client dir conn)

(defun nidhogg-test-peer--internal-filter (proc chunk)
  "Collect CHUNK of PROC; run the user's filter once the write is whole."
  (let ((st (process-get proc 'nidhogg-test-peer)))
    (push chunk (nidhogg-test-peer--state-pending st))
    (cl-incf (nidhogg-test-peer--state-have st) (length chunk))
    (when (and (> (nidhogg-test-peer--state-want st) 0)
               (>= (nidhogg-test-peer--state-have st)
                   (nidhogg-test-peer--state-want st)))
      (let* ((raw (apply #'concat
                         (reverse (nidhogg-test-peer--state-pending st))))
             (text (decode-coding-string
                    raw (nidhogg-test-peer--state-coding st)))
             (f (nidhogg-test-peer--state-filter st)))
        (setf (nidhogg-test-peer--state-pending st) nil
              (nidhogg-test-peer--state-have st) 0
              (nidhogg-test-peer--state-want st) 0)
        (cl-incf (nidhogg-test-peer--state-calls st))
        (when f (funcall f proc text))))))

(cl-defun nidhogg-test-peer-make (&key name filter sentinel (coding 'utf-8))
  "Return a scripted peer: the process whose input the test writes.
FILTER is called as (FILTER PEER STRING), once per
`nidhogg-test-peer-write'.  SENTINEL is the peer's process sentinel.
CODING is the coding system the written text passes through (default
utf-8)."
  (let* ((dir (make-temp-file "nhpeer" t))
         (path (expand-file-name "s" dir))
         (st (nidhogg-test-peer--state-make :filter filter :coding coding
                                            :dir dir))
         (server (make-network-process
                  :name (concat (or name "nidhogg-test-peer") "-srv")
                  :family 'local :service path :server t :noquery t
                  :coding 'no-conversion :plist (list 'nidhogg-test-peer st)
                  :filter #'nidhogg-test-peer--internal-filter
                  :sentinel sentinel
                  :log (lambda (_srv conn _msg)
                         (setf (nidhogg-test-peer--state-conn st) conn)))))
    (setf (nidhogg-test-peer--state-server st) server)
    (setf (nidhogg-test-peer--state-client st)
          (make-network-process
           :name (concat (or name "nidhogg-test-peer") "-wr")
           :family 'local :service path :noquery t :coding 'no-conversion
           :filter #'ignore))
    (let ((end (+ (float-time) 10)))
      (while (and (null (nidhogg-test-peer--state-conn st))
                  (< (float-time) end))
        (accept-process-output nil 0.05)))
    (unless (nidhogg-test-peer--state-conn st)
      (nidhogg-test-peer--cleanup st)
      (error "Peer %s: the connection was not accepted" name))
    (let ((conn (nidhogg-test-peer--state-conn st)))
      (set-process-query-on-exit-flag conn nil)
      (process-put conn 'nidhogg-test-peer st)
      conn)))

(defun nidhogg-test-peer--cleanup (st)
  "Delete the processes and the socket directory of ST."
  (dolist (p (list (nidhogg-test-peer--state-client st)
                   (nidhogg-test-peer--state-conn st)
                   (nidhogg-test-peer--state-server st)))
    (when (and p (process-live-p p)) (delete-process p)))
  (when (nidhogg-test-peer--state-dir st)
    (ignore-errors (delete-directory (nidhogg-test-peer--state-dir st) t))
    (setf (nidhogg-test-peer--state-dir st) nil)))

(defun nidhogg-test-peer-calls (peer)
  "Return how many times PEER's filter ran."
  (nidhogg-test-peer--state-calls (process-get peer 'nidhogg-test-peer)))

(defun nidhogg-test-peer-write (peer string &optional deadline)
  "Deliver STRING to PEER's filter as one call; return after it ran.
DEADLINE (seconds, default 10) only bounds a failure: it signals an error."
  (let* ((st (process-get peer 'nidhogg-test-peer))
         (raw (encode-coding-string string
                                    (nidhogg-test-peer--state-coding st)))
         (before (nidhogg-test-peer--state-calls st))
         (end (+ (float-time) (or deadline 10))))
    (when (= (length raw) 0)
      (error "Nothing to write"))
    (setf (nidhogg-test-peer--state-want st) (length raw))
    (process-send-string (nidhogg-test-peer--state-client st) raw)
    (while (and (= (nidhogg-test-peer--state-calls st) before)
                (process-live-p peer)
                (< (float-time) end))
      (accept-process-output peer 0.05))
    (unless (> (nidhogg-test-peer--state-calls st) before)
      (error "Peer %s: the filter did not run" (process-name peer)))
    nil))

(defun nidhogg-test-peer-close (peer)
  "Close PEER and its socket."
  (nidhogg-test-peer--cleanup (process-get peer 'nidhogg-test-peer)))

(provide 'nidhogg-test-peer)

;;; nidhogg-test-peer.el ends here
