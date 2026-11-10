;;; workers-hash-tests.el --- tests for workers: the SHA-256 of a buffer's text  -*- lexical-binding: t -*-
;; test-class: cond
;; test-pattern: handshake

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

;; `nidhogg-buffer-sha256' and `nidhogg-buffer-sha256-async' hash exactly the
;; bytes `secure-hash' hashes of a buffer with `coding-system-for-write' bound
;; to `utf-8-unix': the edit history sends that hash with every batch and the
;; hub compares it with its own.  The async one copies the text at the call
;; and hashes the copy on nh-save; the job parks at `before-done' while a row
;; edits the buffer.

;;; Code:

(require 'ert)
(require 'cl-lib)

(defun workers-hash-tests--secure (buffer)
  "What the edit history hashed before: `secure-hash' of BUFFER's whole text."
  (with-current-buffer buffer
    (save-restriction
      (widen)
      (let ((coding-system-for-write 'utf-8-unix))
        (secure-hash 'sha256 (current-buffer))))))

(defun workers-hash-tests--buffers ()
  "Fresh buffers whose text is hard to hash alike: (NAME . BUFFER), to be killed by the caller."
  (let (out)
    (cl-flet ((add (name multibyte fn)
                (let ((b (generate-new-buffer (format " *hash-%s*" name) t)))
                  (with-current-buffer b
                    (set-buffer-multibyte multibyte)
                    (funcall fn))
                  (push (cons name b) out))))
      (add "empty" t #'ignore)
      (add "ascii" t (lambda () (insert "hello\nworld\n")))
      (add "unicode" t (lambda () (insert "café € \U0001F600 \u0000 end")))
      (add "eight-bit" t (lambda ()
                           (insert "a" (decode-char 'eight-bit #x80)
                                   (decode-char 'eight-bit #xff) "b"
                                   (decode-char 'eight-bit #xc3) (decode-char 'eight-bit #xa9))))
      (add "non-utf-8-sequences" t (lambda ()
                                     ;; the bytes of a truncated and an overlong UTF-8 sequence, as raw bytes
                                     (insert (string-to-multibyte "\xe2\x82") "x"
                                             (string-to-multibyte "\xc0\xaf") "y"
                                             (string-to-multibyte "\xff\xfe"))))
      (add "beyond-unicode" t (lambda () (insert (string #x110000) (string #x3fff7f) (string #xd800))))
      (add "crlf" t (lambda () (insert "one\r\ntwo\r\n\r\nthree\r\n")))
      (add "crlf-unibyte" nil (lambda () (insert "one\r\ntwo\r\n")))
      (add "unibyte-high-bytes" nil (lambda () (insert (unibyte-string 97 #x80 #xc3 #xa9 #xff 98))))
      (add "narrowed" t (lambda ()
                          (insert "before\né middle\nafter\n")
                          (narrow-to-region 8 15)))
      (add "gap-in-the-middle" t (lambda ()
                                   (insert (make-string 5000 ?a))
                                   (goto-char 2500)
                                   (insert "€" (decode-char 'eight-bit #x90) "\r\n")
                                   (delete-region 100 120)
                                   (goto-char 3000))))
    (nreverse out)))

(ert-deftest workers/hash-sync-is-secure-hash ()
  (let ((bufs (workers-hash-tests--buffers)))
    (unwind-protect
        (dolist (nb bufs)
          (should (equal (nidhogg-buffer-sha256 (cdr nb))
                         (workers-hash-tests--secure (cdr nb)))))
      (mapc (lambda (nb) (kill-buffer (cdr nb))) bufs))))

(defun workers-hash-tests--async (buffer)
  "The hash `nidhogg-buffer-sha256-async' hands BUFFER's callback, once its job has landed."
  (let (got)
    (nidhogg-buffer-sha256-async buffer (lambda (hash) (push hash got)))
    (should (nidhogg-workers--test-await-landed 'nh-save 10000))
    (should (= (length got) 1))
    (car got)))

(ert-deftest workers/hash-async-is-secure-hash ()
  (dolist (mode '(threads inline))
    (let ((nidhogg-workers--mode mode)
          (bufs (workers-hash-tests--buffers)))
      (unwind-protect
          (dolist (nb bufs)
            (should (equal (workers-hash-tests--async (cdr nb))
                           (workers-hash-tests--secure (cdr nb)))))
        (mapc (lambda (nb) (kill-buffer (cdr nb))) bufs)))))

(ert-deftest workers/hash-async-is-the-text-at-the-call ()
  ;; the job parks at before-done; the buffer changes (and dies) meanwhile; the hash is the text it had
  (let ((nidhogg-workers--mode 'threads)
        (buf (generate-new-buffer " *hash-edit*" t))
        got want)
    (unwind-protect
        (progn
          (with-current-buffer buf (insert (make-string 100000 ?x) "é\r\n"))
          (setq want (workers-hash-tests--secure buf))
          (nidhogg-workers--pause-at 'before-done)
          (nidhogg-buffer-sha256-async buf (lambda (hash) (push hash got)))
          (should (nidhogg-workers--test-await-paused 'before-done 5000))
          (with-current-buffer buf
            (goto-char 50)
            (insert "changed")
            (erase-buffer))
          (kill-buffer buf)
          (should (null got))
          (nidhogg-workers--release 'before-done)
          (should (nidhogg-workers--test-await-landed 'nh-save 10000))
          (should (equal got (list want))))
      (nidhogg-workers--release)
      (when (buffer-live-p buf) (kill-buffer buf)))))

(provide 'workers-hash-tests)
;;; workers-hash-tests.el ends here
