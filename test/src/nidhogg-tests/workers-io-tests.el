;;; workers-io-tests.el --- tests for workers: the process reader  -*- lexical-binding: t -*-

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

;; The reader (nh-io, src/nidhogg-io.c).  Every row runs in both of
;; `nidhogg-workers--mode''s modes with every fd kind's sub-switch on.
;; Waits are on events: process output, status changes and pause points.

;;; Code:

(require 'ert)
(require 'cl-lib)

;; FD_SETSIZE on GNU/Linux.
(defconst workers-io-tests--fd-setsize 1024)

(defmacro workers-io-tests--modes (&rest body)
  "Run BODY in each mode with the reader on for every fd kind."
  (declare (indent 0) (debug t))
  `(dolist (mode '(threads inline))
     (let ((nidhogg-workers--mode mode)
           (nidhogg-workers-process-reader t)
           (nidhogg-workers-reader-pipes t)
           (nidhogg-workers-reader-sockets t)
           (nidhogg-workers-reader-ptys t))
       (ert-info ((format "mode %s" mode) :prefix "")
         (unwind-protect
             (progn ,@body)
           (nidhogg-workers--test-release-on-wait nil)
           (nidhogg-workers--release)
           (nidhogg-workers--test-io-log nil)
           (nidhogg-workers--test-fd-reserve nil)
           (dolist (p (process-list))
             (when (string-prefix-p "wio-" (process-name p))
               (delete-process p))))))))

(defun workers-io-tests--until (pred &optional tries)
  "Wait for process output until PRED holds (TRIES waits of 20 ms at most)."
  (let ((i 0) (n (or tries 250)))
    (while (and (not (funcall pred)) (< i n))
      (accept-process-output nil 0.02)
      (setq i (1+ i)))
    (funcall pred)))

(defun workers-io-tests--info (proc key)
  (plist-get (nidhogg-workers--test-io-info proc) key))

(defun workers-io-tests--cat (name &rest props)
  "A `cat' process NAME (prefixed \"wio-\") whose filter collects its output.
The output, oldest first, is in the process property `out'."
  (let ((proc (apply #'make-process
                     :name (concat "wio-" name) :command '("cat")
                     :connection-type 'pipe :noquery t
                     :filter (lambda (p s)
                               (process-put p 'out (cons s (process-get p 'out))))
                     props)))
    proc))

(defun workers-io-tests--out (proc)
  (apply #'concat (reverse (process-get proc 'out))))

(defun workers-io-tests--echo (proc string)
  "Send STRING to the `cat' PROC and wait until it has come back."
  (let ((want (concat (workers-io-tests--out proc) string)))
    (process-send-string proc string)
    (workers-io-tests--until
     (lambda () (equal (workers-io-tests--out proc) want)))))

(defun workers-io-tests--fd-count ()
  (length (directory-files "/proc/self/fd" nil "\\`[0-9]")))

(defun workers-io-tests--event-ids (log event)
  (mapcar #'cadr (cl-remove-if-not (lambda (e) (eq (car e) event)) log)))

(ert-deftest workers/fd-reuse ()
  ;; A reader-owned process killed during a burst while nh-io is parked
  ;; after a poll: the hand-back's acknowledgement (nh-io closed its
  ;; copy) comes before the real fd closes, and nh-io reads nothing for
  ;; that handle afterwards, though the next process and files may take
  ;; its fd numbers (§17.2 B2).
  (let ((file (make-temp-file "wio-file" nil nil "the file's own text\n")))
    (unwind-protect
        (workers-io-tests--modes
          (nidhogg-workers--test-io-log t)
          ;; The burst: a megabyte through `cat', still coming back.
          (let* ((burst (make-process :name "wio-burst" :command '("cat")
                                      :connection-type 'pipe :noquery t
                                      :filter (lambda (p s)
                                                (process-put
                                                 p 'n (+ (or (process-get p 'n) 0)
                                                         (length s))))))
                 (id nil))
            (should (workers-io-tests--info burst :owned))
            (process-send-string burst (make-string (* 1024 1024) ?b))
            (should (workers-io-tests--until
                     (lambda () (> (or (process-get burst 'n) 0) 0))))
            (when (eq mode 'threads)
              (nidhogg-workers--pause-at 'in-poll)
              ;; The point is after a poll returns, and nh-io may have
              ;; read the whole burst already: more output makes its
              ;; poll return.
              (process-send-string burst "more\n")
              (should (nidhogg-workers--test-await-paused 'in-poll 5000))
              (nidhogg-workers--test-release-on-wait 'in-poll))
            (delete-process burst)
            ;; At once: another process and files, on the freed numbers.
            (let ((next (workers-io-tests--cat "next")))
              (dotimes (_ 5)
                (should (equal (with-temp-buffer
                                 (insert-file-contents file)
                                 (buffer-string))
                               "the file's own text\n")))
              (should (workers-io-tests--echo next "after the kill\n"))
              (should (equal (workers-io-tests--out next) "after the kill\n"))
              (let* ((log (nidhogg-workers--test-io-log nil))
                     (closes (workers-io-tests--event-ids log 'close)))
                (setq id (car closes))
                (should id)
                (should-not (workers-io-tests--event-ids log 'timeout))
                (let ((tail (cdr (cl-member-if
                                  (lambda (e) (and (eq (car e) 'close)
                                                   (eql (cadr e) id)))
                                  log))))
                  ;; Nothing read for the killed process after its close.
                  (should-not (cl-find-if (lambda (e) (and (eq (car e) 'read)
                                                           (eql (cadr e) id)))
                                          tail)))
                (when (eq mode 'threads)
                  ;; Its acknowledgement precedes its close.
                  (let ((ack (cl-position-if (lambda (e) (and (eq (car e) 'ack)
                                                              (eql (cadr e) id)))
                                             log))
                        (close (cl-position-if (lambda (e) (and (eq (car e) 'close)
                                                                (eql (cadr e) id)))
                                               log)))
                    (should ack)
                    (should (< ack close))))))))
      (delete-file file))))

(ert-deftest workers/children-inherit-no-worker-fds ()
  ;; A child started while ten reader-owned processes exist inherits
  ;; none of the reader's fds; a `cat' still sees EOF when its stdin
  ;; pipe closes (Review Focus 3).
  (workers-io-tests--modes
    (let* ((procs (cl-loop for i below 10
                           collect (workers-io-tests--cat (format "inh-%d" i))))
           (reader-fds
            (cl-loop for p in procs
                     do (should (workers-io-tests--info p :owned))
                     append (delq nil (list (workers-io-tests--info p :copy)
                                            (workers-io-tests--info p :wake)))))
           (child-fds
            (with-temp-buffer
              (call-process "sh" nil t nil "-c" "ls /proc/$$/fd; :")
              (mapcar #'string-to-number (split-string (buffer-string))))))
      (should (= (length reader-fds) (if (eq mode 'threads) 20 10)))
      (should (memq 0 child-fds))
      (should-not (cl-intersection reader-fds child-fds))
      (let* ((async-fds nil)
             (lister (make-process :name "wio-lister"
                                   :command '("sh" "-c" "ls /proc/$$/fd; read x")
                                   :connection-type 'pipe :noquery t
                                   :filter (lambda (_p s)
                                             (setq async-fds (concat async-fds s))))))
        (should (workers-io-tests--until
                 (lambda () (and async-fds (string-suffix-p "\n" async-fds)))))
        (should-not (cl-intersection
                     reader-fds (mapcar #'string-to-number (split-string async-fds))))
        (delete-process lister))
      (should (workers-io-tests--echo (car procs) "x\n"))
      (process-send-eof (car procs))
      (should (workers-io-tests--until
               (lambda () (eq (process-status (car procs)) 'exit)))))))

(ert-deftest workers/fd-budget ()
  ;; Near FD_SETSIZE every process still starts; the ones whose two
  ;; reader fds would come within 64 of it stay on today's path; a
  ;; reader-owned process holds exactly two more fds (one inline,
  ;; where the select set holds the copy itself) (§17.2a M1).
  (workers-io-tests--modes
    ;; Two more fds per process, against the same process unowned.
    (let ((warm (workers-io-tests--cat "warm")))
      (should (workers-io-tests--info warm :owned))
      (let* ((n0 (workers-io-tests--fd-count))
             (owned (workers-io-tests--cat "owned"))
             (n1 (workers-io-tests--fd-count)))
        (should (workers-io-tests--info owned :owned))
        (delete-process owned)
        (let* ((n2 (workers-io-tests--fd-count))
               (plain (let ((nidhogg-workers-reader-pipes nil))
                        (workers-io-tests--cat "plain")))
               (n3 (workers-io-tests--fd-count)))
          (should-not (workers-io-tests--info plain :owned))
          (should (eq (workers-io-tests--info plain :refused) 'switch))
          (should (= (- (- n1 n0) (- n3 n2)) (if (eq mode 'threads) 2 1)))
          (should (= (workers-io-tests--info warm :fds)
                     (if (eq mode 'threads) 2 1)))
          (delete-process plain)))
      (delete-process warm))
    ;; Fill the fds to 64 + 16 short of FD_SETSIZE, then start processes.
    (should (> (nidhogg-workers--test-fd-reserve 80) 0))
    (let ((procs (cl-loop for i below 16
                          collect (workers-io-tests--cat (format "budget-%d" i))))
          (limit (- workers-io-tests--fd-setsize 64)))
      (dolist (p procs)
        (should (process-live-p p))
        (should (workers-io-tests--echo p "ok\n"))
        (if (workers-io-tests--info p :owned)
            (progn
              (should (< (workers-io-tests--info p :copy) limit))
              (should (< (workers-io-tests--info p :select) limit)))
          (should (eq (workers-io-tests--info p :refused) 'fd-budget))))
      (should (cl-some (lambda (p) (workers-io-tests--info p :owned)) procs))
      (should (cl-some (lambda (p) (not (workers-io-tests--info p :owned))) procs)))))

(ert-deftest workers/ineligible ()
  ;; Never handed over: TLS, datagram, servers, serial, a `:nowait'
  ;; stream before it connects, filter t, an opted-out process.  A
  ;; `:nowait' stream is handed over at connect, so is a connection a
  ;; server accepts, and a filter-t process when its filter changes.
  (workers-io-tests--modes
    (let* ((server (make-network-process :name "wio-server" :server t
                                         :host 'local :service t :family 'ipv4
                                         :noquery t :filter #'ignore))
           (port (process-contact server :service)))
      (should-not (workers-io-tests--info server :owned))
      (should (eq (workers-io-tests--info server :refused) 'server))
      ;; :nowait: not before the connect, then handed over.
      (let ((client (make-network-process :name "wio-nowait" :host "127.0.0.1"
                                          :service port :nowait t :noquery t
                                          :filter #'ignore)))
        (should (eq (process-status client) 'connect))
        (should-not (workers-io-tests--info client :owned))
        (should (workers-io-tests--until
                 (lambda () (eq (process-status client) 'open))))
        (should (workers-io-tests--info client :owned))
        ;; The connection the server accepted is a plain stream too.
        (let ((accepted nil))
          (should (workers-io-tests--until
                   (lambda ()
                     (setq accepted
                           (cl-find-if (lambda (p)
                                         (and (not (eq p server))
                                              (string-prefix-p "wio-server"
                                                               (process-name p))))
                                       (process-list))))))
          (should (workers-io-tests--info accepted :owned))))
      ;; TLS: refused at connect.
      (when (and (fboundp 'gnutls-available-p) (gnutls-available-p))
        (require 'gnutls)
        (let ((tls (make-network-process
                    :name "wio-tls" :host "127.0.0.1" :service port :nowait t
                    :noquery t :filter #'ignore
                    ;; `gnutls-boot' needs a :hostname, or the
                    ;; process fails before it connects.
                    :tls-parameters (cons 'gnutls-anon
                                          (gnutls-boot-parameters
                                           :type 'gnutls-anon
                                           :hostname "127.0.0.1")))))
          (should (workers-io-tests--until
                   (lambda () (eq (workers-io-tests--info tls :refused) 'tls))))
          (should-not (workers-io-tests--info tls :owned)))))
    ;; Datagrams.
    (let ((dgram (make-network-process :name "wio-dgram" :type 'datagram
                                       :server t :host 'local :service t
                                       :family 'ipv4 :noquery t)))
      (should-not (workers-io-tests--info dgram :owned))
      (should (eq (workers-io-tests--info dgram :refused) 'datagram)))
    ;; Serial: a pty's tty, opened as a serial port.
    (let* ((host (make-process :name "wio-ptyhost" :command '("sleep" "60")
                               :connection-type 'pty :noquery t))
           (serial (make-serial-process :name "wio-serial"
                                        :port (process-tty-name host)
                                        :speed nil :noquery t)))
      (should (workers-io-tests--info host :owned))
      (should-not (workers-io-tests--info serial :owned))
      (should (eq (workers-io-tests--info serial :refused) 'serial)))
    ;; Filter t: handed over when the filter changes.
    (let ((pipe (make-pipe-process :name "wio-filter-t" :filter t :noquery t)))
      (should-not (workers-io-tests--info pipe :owned))
      (set-process-filter pipe #'ignore)
      (should (workers-io-tests--info pipe :owned))
      (set-process-filter pipe t)
      (should (workers-io-tests--info pipe :owned)))
    ;; Opting out hands back; it sticks until opted in again.
    (let ((cat (workers-io-tests--cat "opt")))
      (should (workers-io-tests--info cat :owned))
      (should (workers-io-tests--echo cat "one\n"))
      (should-not (nidhogg-set-process-reader cat nil))
      (should-not (workers-io-tests--info cat :owned))
      (should (workers-io-tests--echo cat "two\n"))
      (set-process-filter cat t)
      (set-process-filter cat (lambda (p s)
                                (process-put p 'out (cons s (process-get p 'out)))))
      (should-not (workers-io-tests--info cat :owned))
      (should (eq (workers-io-tests--info cat :refused) 'opt-out))
      (should (workers-io-tests--echo cat "three\n"))
      (should (nidhogg-set-process-reader cat t))
      (should (workers-io-tests--info cat :owned))
      (should (workers-io-tests--echo cat "four\n"))
      (should (equal (workers-io-tests--out cat) "one\ntwo\nthree\nfour\n")))))

(defconst workers-io-tests--recorded
  (append
   ;; `seq', in groups of lines.
   (cl-loop for i from 1 to 2000 by 97
            collect (mapconcat #'number-to-string
                               (number-sequence i (+ i 96)) "\n"))
   ;; A `git log' with non-ASCII text, a multibyte character split
   ;; across two chunks, CRLF, and bytes that are not UTF-8.
   (list (encode-coding-string
          "commit 4b825dc642cb6eb9a060e54bf8d69288fbee4904\nAuthor: Zoë Ångström <z@example.org>\n\n    Fix → the arrow\n"
          'utf-8)
         "Date:   Thu Oct 23 2026 \342\206"
         "\222 split\r\n"
         "binary \377\376\000 bytes\n"))
  "Chunks of a recorded stream, each a unibyte string.")

(defun workers-io-tests--replay (coding)
  "Replay the recorded chunks through `cat' with CODING, one at a time.
Return the filter's arguments in order, and whether the reader owned it."
  (let* ((args nil)
         (proc (make-process :name "wio-replay" :command '("cat")
                             :connection-type 'pipe :noquery t :coding coding
                             :filter (lambda (_p s) (push s args))))
         (owned (workers-io-tests--info proc :owned)))
    (unwind-protect
        (dolist (chunk workers-io-tests--recorded)
          (let ((n (length args))
                (bytes (apply #'+ (mapcar #'string-bytes args))))
            (process-send-string proc chunk)
            ;; Each chunk comes back whole, as one read.
            (workers-io-tests--until
             (lambda () (and (> (length args) n)
                             (or (memq coding '(utf-8))
                                 (>= (apply #'+ (mapcar #'string-bytes args))
                                     (+ bytes (length chunk)))))))))
      (delete-process proc))
    (list (nreverse args) owned)))

(ert-deftest workers/raw-bytes-unchanged ()
  ;; A recorded `seq' and `git log' stream through a pipe: the filter's
  ;; arguments are `equal', call for call, multibyteness included, to
  ;; the reader-off run's.
  (workers-io-tests--modes
    (dolist (coding '(binary utf-8))
      (ert-info ((format "coding %s" coding) :prefix "")
        (let ((off (let ((nidhogg-workers-process-reader nil))
                     (workers-io-tests--replay coding)))
              (on (workers-io-tests--replay coding)))
          (should-not (cadr off))
          (should (cadr on))
          (should (= (length (car on)) (length workers-io-tests--recorded)))
          (should (equal (car on) (car off)))
          (should (equal (mapcar #'multibyte-string-p (car on))
                         (mapcar #'multibyte-string-p (car off)))))))))

;;; workers-io-tests.el ends here
