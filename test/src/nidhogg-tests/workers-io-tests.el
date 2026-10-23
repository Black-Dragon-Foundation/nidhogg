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

;;; Task 17: drains, flow control, caps and the waits.

(defun workers-io-tests--busy-until (pred &optional tries)
  "Keep the main thread busy until PRED holds (TRIES at most, default 5000).
Each try runs `true' through `call-process', which serves no process
output and runs no select over process fds: process output waits
meanwhile, while a SIGCHLD still reaches its handler and
`process-status' reports what the handler recorded."
  (let ((i 0) (n (or tries 5000)))
    (while (and (not (funcall pred)) (< i n))
      (call-process "true")
      (setq i (1+ i)))
    (funcall pred)))

(defun workers-io-tests--io-stat (key)
  (plist-get (plist-get (nidhogg-workers-stats) :nh-io) key))

(ert-deftest workers/raw-sentinel-order ()
  ;; `seq' exits while the main thread is busy: every line reaches the
  ;; filter before the sentinel runs, and a grandchild that holds the
  ;; pipe open does not delay the sentinel (§17.2a B6).  In threads
  ;; mode nh-io is parked after a poll the whole time, so every byte is
  ;; still in the pipe when the child exits: the drain hands the process
  ;; back and reads the real fd itself.  (10000 lines, not 100000: the
  ;; output must fit the pipe for `seq' to exit while nothing reads it.)
  (workers-io-tests--modes
    (let* ((count 10000) (out nil) (at-sentinel nil) (gpid nil)
           (proc nil))
      (when (eq mode 'threads)
        (nidhogg-workers--pause-at 'in-poll))
      (setq proc (make-process
                  :name "wio-seq" :connection-type 'pipe :noquery t
                  :coding 'binary
                  :command (list "sh" "-c"
                                 (format "sleep 60 & echo $!; seq 1 %d" count))
                  :filter (lambda (_p s) (push s out))
                  :sentinel (lambda (_p _e)
                              (unless at-sentinel
                                (setq at-sentinel
                                      (apply #'concat (reverse out)))))))
      (unwind-protect
          (progn
            (should (workers-io-tests--info proc :owned))
            (when (eq mode 'threads)
              (should (nidhogg-workers--test-await-paused 'in-poll 5000))
              ;; The drain's hand-back un-parks nh-io.
              (nidhogg-workers--test-release-on-wait 'in-poll))
            (should (workers-io-tests--busy-until
                     (lambda () (eq (process-status proc) 'exit))))
            (should (workers-io-tests--until (lambda () at-sentinel)))
            (let ((lines (split-string at-sentinel "\n" t)))
              (setq gpid (string-to-number (car lines)))
              (should (> gpid 0))
              ;; The grandchild still holds the pipe: no EOF was awaited.
              (should (eq (signal-process gpid 0) 0))
              (should (equal (cdr lines)
                             (mapcar #'number-to-string
                                     (number-sequence 1 count))))))
        (when (and gpid (> gpid 0))
          (signal-process gpid 'KILL))))))

(ert-deftest workers/sentinel-after-stderr-eof ()
  ;; Upstream's process-test-stderr-filter, made deterministic: a child
  ;; writes to stdout and to a `:stderr' pipe process and exits.  Its
  ;; sentinel runs only once no process has output waiting, as today, so
  ;; the stderr pipe's output and EOF (its sentinel) come first.  In
  ;; threads mode nh-io is held in the middle of a read when the exit is
  ;; noticed: the wait for what it is reading releases it.
  (workers-io-tests--modes
    (let* ((err-out nil) (err-sentinel nil) (out nil) (at-sentinel nil)
           (stderr (make-pipe-process
                    :name "wio-stderr" :noquery t
                    :filter (lambda (_p s) (push s err-out))
                    :sentinel (lambda (_p _e) (setq err-sentinel t))))
           (proc nil))
      (should (workers-io-tests--info stderr :owned))
      (when (eq mode 'threads)
        (nidhogg-workers--pause-at 'before-decode))
      (setq proc (make-process
                  :name "wio-stderr-main" :noquery t :connection-type 'pipe
                  :command '("sh" "-c" "echo out; echo err >&2; exit 3")
                  :stderr stderr
                  :filter (lambda (_p s) (push s out))
                  :sentinel (lambda (_p _e)
                              (setq at-sentinel
                                    (apply #'concat (reverse err-out))))))
      (when (eq mode 'threads)
        (should (nidhogg-workers--test-await-paused 'before-decode 5000)))
      (should (workers-io-tests--busy-until
               (lambda () (eq (process-status proc) 'exit))))
      (when (eq mode 'threads)
        (nidhogg-workers--test-release-on-wait 'before-decode))
      (should (workers-io-tests--until (lambda () at-sentinel)))
      ;; The stderr pipe's EOF was read in the same wait: its sentinel
      ;; ran in the same status_notify, after this one (newer first).
      (should err-sentinel)
      (should (equal (apply #'concat (reverse out)) "out\n"))
      (should (equal at-sentinel "err\n")))))

(ert-deftest workers/raw-flow-control ()
  ;; A stream far past the queue cap (MC-22, max (4 × readmax, 256 KB)):
  ;; the queue never holds more than the cap, nh-io stops reading at it
  ;; and goes on when delivery takes chunks.  Stopping a subprocess only
  ;; stops the producer (fd audit finding 12: this is the cap's row, not
  ;; the pause's); `continue-process' lets it finish; `set-process-filter'
  ;; t pauses the reader, a filter again resumes it.  The producer is
  ;; stopped with SIGSTOP, not `stop-process': that sends SIGTSTP, and a
  ;; subprocess leads its own session (setsid), so its process group is
  ;; orphaned and Linux discards SIGTSTP; the status never becomes `stop'.
  ;; (32 MB, not 500 MB: the cap is 256 KB; the row needs a stream that
  ;; outruns it, not a long one.)
  (workers-io-tests--modes
    (let* ((total (* 32 1024 1024)) (n 0) (over nil) (proc nil)
           (filter (lambda (p s)
                     (setq n (+ n (length s)))
                     (let ((q (or (workers-io-tests--info p :queued) 0))
                           (cap (workers-io-tests--info p :cap)))
                       (when (and cap (> q cap))
                         (setq over (list q cap)))))))
      (nidhogg-workers-stats t)
      (when (eq mode 'threads)
        (nidhogg-workers--pause-at 'after-read))
      (setq proc (make-process
                  :name "wio-flow" :connection-type 'pipe :noquery t
                  :coding 'binary :filter filter
                  :command (list "head" "-c" (number-to-string total)
                                 "/dev/zero")))
      (should (workers-io-tests--info proc :owned))
      (when (eq mode 'threads)
        ;; Nothing is delivered yet: nh-io fills the queue to its cap,
        ;; exactly, and parks there.
        (should (nidhogg-workers--test-await-paused 'after-read 5000))
        (should (= (workers-io-tests--info proc :queued)
                   (workers-io-tests--info proc :cap)))
        (should (> (workers-io-tests--io-stat :backpressure) 0)))
      (signal-process proc 'STOP)
      (nidhogg-workers--release 'after-read)
      (should (workers-io-tests--until
               (lambda () (eq (process-status proc) 'stop))))
      (should (< n total))
      (continue-process proc)
      (should (workers-io-tests--until (lambda () (> n (/ total 2))) 2500))
      ;; Filter t pauses the reader (it stays the owner); a filter resumes.
      (set-process-filter proc t)
      (should (workers-io-tests--info proc :owned))
      (should (workers-io-tests--info proc :paused))
      (set-process-filter proc filter)
      (should-not (workers-io-tests--info proc :paused))
      (should (workers-io-tests--until (lambda () (= n total)) 2500))
      (should (workers-io-tests--until
               (lambda () (eq (process-status proc) 'exit))))
      (should-not over))))

(ert-deftest workers/flow-control-signal-safe ()
  ;; A SIGCHLD that arrives while the main thread holds nh-io's queue
  ;; mutex pauses the reader through the atomic flag and one byte to the
  ;; command fd: the handler takes no lock (a worker mutex taken in signal
  ;; context aborts) and nothing deadlocks (§17.2 S17).  The output
  ;; still reaches the filter before the sentinel.
  (workers-io-tests--modes
    (let* ((out nil) (at-sentinel nil)
           (proc (make-process
                  :name "wio-sig" :connection-type 'pipe :noquery t
                  :command '("sh" "-c" "read x; echo \"$x done\"")
                  :filter (lambda (_p s) (push s out))
                  :sentinel (lambda (_p _e)
                              (setq at-sentinel
                                    (apply #'concat (reverse out)))))))
      (should (workers-io-tests--info proc :owned))
      (should-not (workers-io-tests--info proc :paused))
      (nidhogg-workers--test-io-hold-lock t)
      (unwind-protect
          (progn
            (process-send-string proc "go\n")
            (workers-io-tests--busy-until
             (lambda () (not (eq (process-status proc) 'run)))))
        (nidhogg-workers--test-io-hold-lock nil))
      (should (eq (process-status proc) 'exit))
      (should (workers-io-tests--info proc :paused))
      (should (workers-io-tests--until (lambda () at-sentinel)))
      (should (equal at-sentinel "go done\n")))))

(ert-deftest workers/accept-process-output-raw ()
  ;; `accept-process-output' on a reader-owned process (§5.4): non-nil
  ;; once PROC's output was delivered, nil on a timeout without any;
  ;; JUST-THIS-ONE selects only PROC's wake fd (another process's ready
  ;; output waits); a filter that calls it on its own process waits
  ;; without a spin.
  (workers-io-tests--modes
    (let ((a (workers-io-tests--cat "apo-a"))
          (b (workers-io-tests--cat "apo-b")))
      (should (workers-io-tests--info a :owned))
      (should (workers-io-tests--info b :owned))
      (process-send-string a "one\n")
      (should (accept-process-output a 5))
      (should (equal (workers-io-tests--out a) "one\n"))
      (should-not (accept-process-output a 0.1))
      ;; JUST-THIS-ONE.
      (process-send-string b "bee\n")
      (when (eq mode 'threads)
        (should (workers-io-tests--busy-until
                 (lambda () (> (workers-io-tests--info b :queued) 0)))))
      (process-send-string a "two\n")
      (should (accept-process-output a 5 nil t))
      (should (equal (workers-io-tests--out a) "one\ntwo\n"))
      (should (equal (workers-io-tests--out b) ""))
      (should (workers-io-tests--until
               (lambda () (equal (workers-io-tests--out b) "bee\n"))))
      ;; Recursion from the filter, with nothing more to come.
      (let ((ret 'unset) (cpu nil))
        (set-process-filter
         a (lambda (p s)
             (process-put p 'out (cons s (process-get p 'out)))
             (when (equal s "rec\n")
               (let ((t0 (current-cpu-time)))
                 (setq ret (accept-process-output p 1 nil t))
                 (let ((t1 (current-cpu-time)))
                   (setq cpu (/ (float (- (car t1) (car t0))) (cdr t1))))))))
        (process-send-string a "rec\n")
        (should (workers-io-tests--until (lambda () cpu)))
        (should-not ret)
        (should (< cpu 0.05))))))

(ert-deftest workers/lisp-thread-lock ()
  ;; A reader-owned process locked to a `make-thread' thread is delivered
  ;; only on that thread, though the main thread waits for output too:
  ;; the lock covers the wake fd (fd audit finding 2).
  (skip-unless (featurep 'threads))
  (workers-io-tests--modes
    (let* ((callers nil) (done nil)
           (proc (workers-io-tests--cat "lock"))
           (worker nil))
      (set-process-filter proc (lambda (p s)
                                 (push (current-thread) callers)
                                 (process-put p 'out
                                              (cons s (process-get p 'out)))))
      ;; No yield between creating WORKER and locking: it never runs
      ;; before the lock is its own.
      (setq worker (make-thread (lambda ()
                                  (while (not done)
                                    (accept-process-output proc 0.05)))
                                "wio-lock"))
      (set-process-thread proc worker)
      (should (workers-io-tests--info proc :owned))
      (unwind-protect
          (progn
            (process-send-string proc "locked\n")
            (should (workers-io-tests--until
                     (lambda () (equal (workers-io-tests--out proc)
                                       "locked\n"))))
            (should callers)
            (should (cl-every (lambda (th) (eq th worker)) callers)))
        (setq done t)
        (thread-join worker)))))

(defun workers-io-tests--numbered (n)
  "N numbered lines, 7 bytes each."
  (mapconcat (lambda (i) (format "%06d\n" i)) (number-sequence 1 n) ""))

(defun workers-io-tests--small-cat (name filter)
  "A `cat' NAME with FILTER, read 4 KB at a time (many chunks)."
  (let ((read-process-output-max 4096))
    (make-process :name (concat "wio-" name) :command '("cat")
                  :connection-type 'pipe :noquery t :coding 'binary
                  :filter filter)))

(ert-deftest workers/reconfigure-inside-own-filter ()
  ;; Inside its own filter a process deletes itself, changes its filter,
  ;; stops itself, changes its buffer, or deletes the next process: no
  ;; delivery to a deleted process after `delete-process' returns, and
  ;; every undelivered byte reaches the new filter, in order (Review
  ;; Focus 5; ASan in the `workers-asan' job).
  (workers-io-tests--modes
    (let ((data (workers-io-tests--numbered 5000)))
      (ert-info ("delete-process" :prefix "")
        (let* ((deleted nil) (after 0)
               (p (workers-io-tests--small-cat
                   "re-del" (lambda (p _s)
                              (if deleted
                                  (setq after (1+ after))
                                (delete-process p)
                                (setq deleted t))))))
          (process-send-string p data)
          (should (workers-io-tests--until (lambda () deleted)))
          (workers-io-tests--until #'ignore 5)
          (should (= after 0))))
      (ert-info ("set-process-filter" :prefix "")
        (let* ((got nil)
               (second (lambda (_p s) (push s got)))
               (p (workers-io-tests--small-cat
                   "re-filter" (lambda (p s)
                                 (push s got)
                                 (set-process-filter p second)))))
          (process-send-string p data)
          (should (workers-io-tests--until
                   (lambda () (equal (apply #'concat (reverse got)) data))))))
      (ert-info ("stop-process" :prefix "")
        (let* ((got nil)
               (p (workers-io-tests--small-cat
                   "re-stop" (lambda (p s)
                               (unless got
                                 (stop-process p))
                               (push s got)))))
          (process-send-string p data)
          (should (workers-io-tests--until (lambda () got)))
          (continue-process p)
          (should (workers-io-tests--until
                   (lambda () (equal (apply #'concat (reverse got)) data))))))
      (ert-info ("set-process-buffer" :prefix "")
        (with-temp-buffer
          (let* ((buf (current-buffer)) (got nil)
                 (p (workers-io-tests--small-cat
                     "re-buffer" (lambda (p s)
                                   (unless got
                                     (set-process-buffer p buf))
                                   (push s got)))))
            (process-send-string p data)
            (should (workers-io-tests--until
                     (lambda () (equal (apply #'concat (reverse got)) data))))
            (should (eq (process-buffer p) buf))
            (set-process-buffer p nil))))
      (ert-info ("delete the next process" :prefix "")
        (let* ((b-deleted nil) (b-after 0)
               (b (workers-io-tests--small-cat
                   "re-next-b" (lambda (_p _s)
                                 (when b-deleted
                                   (setq b-after (1+ b-after))))))
               (a (workers-io-tests--small-cat
                   "re-next-a" (lambda (_p _s)
                                 (unless b-deleted
                                   (delete-process b)
                                   (setq b-deleted t))))))
          (process-send-string b data)
          (process-send-string a data)
          (should (workers-io-tests--until (lambda () b-deleted)))
          (workers-io-tests--until #'ignore 5)
          (should (= b-after 0))
          (should-not (process-live-p b))
          (should (process-live-p a)))))))

(ert-deftest workers/pty-eio ()
  ;; A pty child's exit: the reader gives -1 with EIO once its queue is
  ;; empty (fd audit P85), and Lisp sees today's output, status, exit
  ;; code and sentinel message (the reader-off run's).
  (workers-io-tests--modes
    (cl-flet ((run ()
                (let* ((out nil) (events nil)
                       (p (make-process
                           :name "wio-pty" :connection-type 'pty :noquery t
                           :command '("sh" "-c" "printf 'pty out\\n'; exit 3")
                           :filter (lambda (_p s) (push s out))
                           :sentinel (lambda (_p e) (push e events))))
                       (owned (workers-io-tests--info p :owned)))
                  (should (workers-io-tests--until (lambda () events)))
                  (list (apply #'concat (reverse out)) (process-status p)
                        (process-exit-status p) (reverse events) owned))))
      (let ((off (let ((nidhogg-workers-process-reader nil)) (run)))
            (on (run)))
        (should-not (nth 4 off))
        (should (nth 4 on))
        (should (equal (butlast on) (butlast off)))
        (should (eq (nth 1 on) 'exit))
        (should (= (nth 2 on) 3))))))

;;; Task 20: ptys, comint and TRAMP.

(defconst workers-io-tests--pty-script
  "n=0
trap 'echo \"winch $(stty size)\"' WINCH
trap 'echo int; n=1' INT
echo \"tty $(tty)\"
read a; echo \"got $a\"
while [ $n = 0 ]; do sleep 0.05; done
echo \"after int\"
read b || echo eof
exit 3"
  "A pty child that reports a line, a resize, an interrupt and EOF.")

(ert-deftest workers/pty-semantics ()
  ;; A pty child under the reader (§17.2a): writes, `process-tty-name',
  ;; `set-process-window-size' (the child sees SIGWINCH and the new
  ;; size), `interrupt-process', `process-send-eof' (C-d) and the child's
  ;; exit (EIO) give the reader-off run's output, status and sentinel
  ;; messages: they all use the real fd or the tty name.
  (workers-io-tests--modes
    (cl-flet ((run ()
                (let* ((out nil) (events nil)
                       (p (make-process
                           :name "wio-ptysem" :connection-type 'pty :noquery t
                           :command (list "sh" "-c" workers-io-tests--pty-script)
                           :filter (lambda (_p s) (push s out))
                           :sentinel (lambda (_p e) (push e events))))
                       (owned (workers-io-tests--info p :owned))
                       (text (lambda () (apply #'concat (reverse out))))
                       (seen (lambda (re)
                               (workers-io-tests--until
                                (lambda () (string-match-p re (funcall text)))))))
                  (should (funcall seen "^tty /.*\n"))
                  (let ((s (funcall text)))
                    (string-match "^tty \\(/[^\r\n]*\\)" s)
                    (should (equal (match-string 1 s) (process-tty-name p))))
                  (process-send-string p "hello\n")
                  (should (funcall seen "^got hello"))
                  (should (set-process-window-size p 33 111))
                  (should (funcall seen "^winch 33 111"))
                  (interrupt-process p)
                  (should (funcall seen "^after int"))
                  (process-send-eof p)
                  (should (workers-io-tests--until (lambda () events)))
                  (list (replace-regexp-in-string "^tty .*\n" "" (funcall text))
                        (process-status p) (process-exit-status p)
                        (reverse events) owned))))
      (let ((off (let ((nidhogg-workers-process-reader nil)) (run)))
            (on (run)))
        (should-not (nth 4 off))
        (should (nth 4 on))
        (should (equal (butlast on) (butlast off)))
        (should (string-match-p "^int" (car on)))
        (should (string-match-p "^eof" (car on)))
        (should (eq (nth 1 on) 'exit))
        (should (= (nth 2 on) 3))))))

(defun workers-io-tests--comint ()
  "A `shell-mode' session: a command, then a `comint-redirect-send-command'.
Return the buffer's text, the last prompt, the redirected output, and
whether the reader owned the shell."
  (require 'shell)
  (let* ((bash (executable-find "bash"))
         (explicit-shell-file-name (or bash "sh"))
         (explicit-bash-args '("--noediting" "--norc" "-i"))
         ;; Sets before unsets (Emacs drops NAME=… after a bare NAME).
         (process-environment (append '("PS1=wio$ " "ENV" "HISTFILE")
                                      process-environment))
         (buf (get-buffer-create "*wio-comint*"))
         (redirect (get-buffer-create " *wio-redirect*"))
         (proc nil))
    (unwind-protect
        (with-current-buffer buf
          (shell buf)
          (setq proc (get-buffer-process buf))
          (should (workers-io-tests--until
                   (lambda () (string-suffix-p "wio$ " (buffer-string)))))
          (goto-char (point-max))
          (insert "echo hi")
          (comint-send-input)
          (should (workers-io-tests--until
                   (lambda () (string-match-p "^hi\r?\nwio\\$ \\'"
                                              (buffer-string)))))
          (comint-redirect-send-command "echo redirected" redirect nil t)
          (should (workers-io-tests--until (lambda () comint-redirect-completed)))
          (list (buffer-substring-no-properties (point-min) (point-max))
                (and comint-last-prompt
                     (buffer-substring-no-properties (car comint-last-prompt)
                                                     (cdr comint-last-prompt)))
                (with-current-buffer redirect (buffer-string))
                (workers-io-tests--info proc :owned)))
      (when (process-live-p proc)
        (delete-process proc))
      (kill-buffer buf)
      (kill-buffer redirect))))

(defun workers-io-tests--tramp ()
  "Write, read, list and run on TRAMP's connection; return the results.
The connection is sshx to `REMOTE_TEMPORARY_FILE_DIRECTORY' when that
names one (pkg-test.sh's sshd), else TRAMP's mock method (as
tramp-tests.el: a local `sh -i' over a pty, the same tramp-sh code)."
  (require 'tramp)
  (require 'tramp-cache)
  (let* ((env (getenv "REMOTE_TEMPORARY_FILE_DIRECTORY"))
         (tramp-verbose 0)
         (tramp-persistency-file-name nil)
         (tramp-methods
          (cons `("mock"
                  (tramp-login-program ,tramp-encoding-shell)
                  (tramp-login-args (("-i")))
                  (tramp-direct-async ("-c"))
                  (tramp-tmpdir ,temporary-file-directory)
                  (tramp-remote-shell ,tramp-default-remote-shell)
                  (tramp-remote-shell-args ("-c"))
                  (tramp-connection-timeout 10))
                tramp-methods))
         (tramp-default-host-alist
          (cons `("\\`mock\\'" nil ,(system-name)) tramp-default-host-alist))
         (dir (file-name-as-directory
               (if (and env (string-prefix-p "/sshx:" env))
                   env
                 (format "/mock::%s" temporary-file-directory))))
         (file (expand-file-name (make-temp-name "wio-tramp") dir))
         (text "line one\nline two\n"))
    (tramp-cleanup-all-connections)
    (unwind-protect
        (progn
          (write-region text nil file nil 'nomessage)
          (list (with-temp-buffer
                  (insert-file-contents file)
                  (buffer-string))
                (file-exists-p file)
                (file-attribute-size (file-attributes file))
                (and (member (file-name-nondirectory file) (directory-files dir))
                     t)
                (let ((default-directory dir))
                  (with-temp-buffer
                    (list (process-file "sh" nil t nil "-c"
                                        "printf 'remote %s\\n' ok; exit 4")
                          (buffer-string))))
                (let ((default-directory dir))
                  (shell-command-to-string "echo via shell"))
                (let ((p (tramp-get-connection-process
                          (tramp-dissect-file-name dir))))
                  (and p (workers-io-tests--info p :owned)))))
      (ignore-errors (delete-file file))
      (tramp-cleanup-all-connections))))

(ert-deftest workers/comint-and-tramp ()
  ;; §17.2a: `shell-mode''s prompt tracking and
  ;; `comint-redirect-send-command', and TRAMP's connection (its waits
  ;; use JUST-THIS-ONE, which selects only that process's wake fd): the
  ;; same results with the reader on and off.
  (workers-io-tests--modes
    (ert-info ("comint" :prefix "")
      (let ((off (let ((nidhogg-workers-process-reader nil))
                   (workers-io-tests--comint)))
            (on (workers-io-tests--comint)))
        (should-not (car (last off)))
        (should (car (last on)))
        (should (equal (butlast on) (butlast off)))
        (should (equal (nth 1 on) "wio$ "))
        (should (string-match-p "redirected" (nth 2 on)))))
    (ert-info ("tramp" :prefix "")
      (let ((off (let ((nidhogg-workers-process-reader nil))
                   (workers-io-tests--tramp)))
            (on (workers-io-tests--tramp)))
        (should-not (car (last off)))
        (should (car (last on)))
        (should (equal (butlast on) (butlast off)))
        (should (equal (car on) "line one\nline two\n"))
        (should (equal (nth 4 on) '(4 "remote ok\n")))))))

;;; workers-io-tests.el ends here
