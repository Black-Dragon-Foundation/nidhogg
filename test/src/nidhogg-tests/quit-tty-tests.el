;;; quit-tty-tests.el --- C-g stops a synchronous program, on any tty  -*- lexical-binding: t -*-
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

;; A child Nidhogg (-nw on a pty, its controlling terminal) runs a
;; 30 s program through `call-process' from a command, and gets C-g
;; while it waits in waitpid.  On the controlling tty C-g is SIGINT,
;; whose handler sets `quit-flag'.  On any other tty (an emacsclient
;; frame, `make-frame-on-tty') C-g is a plain byte: SIGIO interrupts
;; waitpid, `maybe_quit' processes the pending signal, and
;; `gobble_input' reads the C-g and sets `quit-flag'.  `probably_quit'
;; (src/eval.c) used to check the flag only before it processed the
;; signals, so the wait went on until another signal came (a child's
;; SIGCHLD, more input) or the program ended.
;;
;; The order is forced: the key goes in once the child's frame is up,
;; C-g once the program has started and the child's main thread sleeps
;; in the kernel's wait (/proc/PID/wchan), and nothing else signals
;; the child, so without the fix the row is red every run.  The bound
;; on the wait is 10 s for a program that sleeps 30 s.

;;; Code:

(require 'ert)
(require 'seq)

(defun quit-tty-tests--read (file)
  "FILE's contents, or nil when it cannot be read."
  (ignore-errors
    (with-temp-buffer
      (insert-file-contents file)
      (buffer-string))))

(defun quit-tty-tests--wait (pred secs)
  "Wait until PRED returns non-nil, at most SECS seconds; return its value."
  (let ((end (+ (float-time) secs))
        v)
    (while (and (not (setq v (funcall pred))) (< (float-time) end))
      (accept-process-output nil 0.02))
    v))

(defun quit-tty-tests--usable-p ()
  "Whether this system has what the rows need: /proc's wchan and xterm's terminfo."
  (and (eq system-type 'gnu/linux)
       (file-readable-p "/proc/self/wchan")
       (seq-some (lambda (d) (file-exists-p (expand-file-name "x/xterm" d)))
                 '("/usr/share/terminfo" "/lib/terminfo" "/etc/terminfo"))))

(defun quit-tty-tests--row (other-tty)
  "Run the row; return what the child's command wrote: \"quit\" or \"returned\", or nil.
With OTHER-TTY the key and C-g go to a second tty, a frame the child
opens with `make-frame-on-tty'; else to the child's controlling tty."
  (let* ((dir (make-temp-file "nh-quit" t))
         (started (expand-file-name "started" dir))
         (ready (expand-file-name "ready" dir))
         (result (expand-file-name "result" dir))
         (process-environment (cons "TERM=xterm" process-environment))
         ;; the second tty: a pty whose slave a `sleep' holds open
         (holder (and other-tty
                      (make-process :name "nh-quit-tty" :buffer nil :noquery t
                                    :connection-type 'pty :filter #'ignore
                                    :command '("sleep" "300"))))
         (child nil))
    (unwind-protect
        (progn
          (setq child
                (make-process
                 :name "nh-quit-child" :buffer nil :noquery t
                 :connection-type 'pty :filter #'ignore
                 :command
                 (list (expand-file-name invocation-name invocation-directory)
                       "-nw" "-Q" "--eval"
                       (prin1-to-string
                        `(progn
                           (defun nhq-run ()
                             (interactive)
                             (write-region
                              (condition-case nil
                                  (progn
                                    (call-process
                                     "/bin/sh" nil nil nil "-c"
                                     ,(format "echo $$ > %s; exec sleep 30"
                                              (shell-quote-argument started)))
                                    "returned")
                                (quit "quit"))
                              nil ,result nil 'silent))
                           ;; x runs it, ahead of every other keymap
                           (push (list (cons t (let ((m (make-sparse-keymap)))
                                                 (define-key m "x" #'nhq-run)
                                                 m)))
                                 emulation-mode-map-alists)
                           ,@(and other-tty
                                  `((select-frame-set-input-focus
                                     (make-frame-on-tty ,(process-tty-name holder)
                                                        "xterm"))))
                           (write-region "" nil ,ready nil 'silent))))))
          (let ((tty (or holder child))
                (pid (process-id child)))
            (unless (quit-tty-tests--wait (lambda () (file-exists-p ready)) 30)
              (error "The child's frame never came up"))
            (process-send-string tty "x")
            (unless (quit-tty-tests--wait (lambda () (file-exists-p started)) 30)
              (error "The program never started"))
            ;; the child's main thread is in waitpid
            (unless (quit-tty-tests--wait
                     (lambda ()
                       (equal (quit-tty-tests--read (format "/proc/%d/wchan" pid))
                              "do_wait"))
                     30)
              (error "The child never waited for the program: wchan %S"
                     (quit-tty-tests--read (format "/proc/%d/wchan" pid))))
            (process-send-string tty "\C-g")
            (quit-tty-tests--wait (lambda () (quit-tty-tests--read result)) 10)))
      (when child
        (delete-process child))
      (when holder
        (delete-process holder))
      (let ((pid (string-to-number (or (quit-tty-tests--read started) ""))))
        (when (> pid 0)
          (signal-process pid 'KILL)))
      (delete-directory dir t))))

(ert-deftest nidhogg-quit/c-g-on-the-controlling-tty-stops-call-process ()
  ;; C-g as SIGINT: the handler sets `quit-flag', the wait quits
  (skip-unless (quit-tty-tests--usable-p))
  (should (equal (quit-tty-tests--row nil) "quit")))

(ert-deftest nidhogg-quit/c-g-on-another-tty-stops-call-process ()
  ;; C-g as a byte on a frame's tty that is not the controlling one: read
  ;; by `maybe_quit''s signal processing, which must quit then too
  (skip-unless (quit-tty-tests--usable-p))
  (should (equal (quit-tty-tests--row t) "quit")))

;;; quit-tty-tests.el ends here
