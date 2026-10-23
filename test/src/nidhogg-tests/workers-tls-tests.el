;;; workers-tls-tests.el --- tests for workers: STARTTLS and the reader  -*- lexical-binding: t -*-

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

;; `gnutls-boot' and the reader (src/nidhogg-io.c, Task 20; the design's
;; §8.2 "A STARTTLS upgrade", MC-32).  Every row runs in both of
;; `nidhogg-workers--mode''s modes with every fd kind's sub-switch on.
;; The servers: `gnutls-serv' in echo mode (network-stream-tests's
;; certificate), and a mock SMTP or IMAP server in a child Emacs that
;; answers STARTTLS and then relays every byte to its own `gnutls-serv'
;; (a blocking handshake runs no Lisp, so the relay cannot be in this
;; Emacs).  Waits are on events: process output, status changes and
;; pause points.

;;; Code:

(require 'ert)
(require 'cl-lib)

(defconst workers-tls-tests--resources
  (expand-file-name "../../lisp/net/network-stream-resources/"
                    (file-name-directory (or load-file-name buffer-file-name)))
  "network-stream-tests's key and certificate.")

(defmacro workers-tls-tests--modes (&rest body)
  "Run BODY in each mode with the reader on for every fd kind."
  (declare (indent 0) (debug t))
  `(dolist (mode '(threads inline))
     (let ((nidhogg-workers--mode mode)
           (nidhogg-workers-process-reader t)
           (nidhogg-workers-reader-pipes t)
           (nidhogg-workers-reader-sockets t)
           (nidhogg-workers-reader-ptys t)
           (network-security-level 'low))
       (ert-info ((format "mode %s" mode) :prefix "")
         (unwind-protect
             (progn ,@body)
           (nidhogg-workers--test-release-on-wait nil)
           (nidhogg-workers--release)
           (nidhogg-workers--test-io-log nil)
           (dolist (p (process-list))
             (when (string-prefix-p "wtls-c-" (process-name p))
               (delete-process p))))))))

(defun workers-tls-tests--until (pred &optional tries)
  "Wait for process output until PRED holds (TRIES waits of 20 ms at most)."
  (let ((i 0) (n (or tries 250)))
    (while (and (not (funcall pred)) (< i n))
      (accept-process-output nil 0.02)
      (setq i (1+ i)))
    (funcall pred)))

(defun workers-tls-tests--info (proc key)
  (plist-get (nidhogg-workers--test-io-info proc) key))

(defun workers-tls-tests--event-ids (log event)
  (mapcar #'cadr (cl-remove-if-not (lambda (e) (eq (car e) event)) log)))

(defmacro workers-tls-tests--skip ()
  ;; A macro: `skip-unless' exists only inside an `ert-deftest' body.
  '(progn
     (skip-unless (and (fboundp 'gnutls-available-p) (gnutls-available-p)))
     (skip-unless (executable-find "gnutls-serv"))
     (require 'gnutls)
     (require 'network-stream)))

(defun workers-tls-tests--server ()
  "`gnutls-serv' in echo mode on a free port: (PROCESS . PORT)."
  (catch 'server
    (dotimes (_ 10)
      (let* ((port (+ 20000 (random 45535)))
             (out "")
             (proc (make-process
                    :name "wtls-serv" :noquery t :connection-type 'pty
                    :command (list "gnutls-serv"
                                   "--x509keyfile"
                                   (expand-file-name "key.pem"
                                                     workers-tls-tests--resources)
                                   "--x509certfile"
                                   (expand-file-name "cert.pem"
                                                     workers-tls-tests--resources)
                                   "--port" (number-to-string port))
                    :filter (lambda (_p s) (setq out (concat out s))))))
        (workers-tls-tests--until
         (lambda ()
           (or (not (process-live-p proc))
               (string-match-p (format "port %d\\.\\.\\." port) out)))
         1500)
        (if (string-match-p (format "port %d\\.\\.\\.done" port) out)
            (throw 'server (cons proc port))
          (delete-process proc))))
    (error "gnutls-serv did not start")))

(defconst workers-tls-tests--mock-program
  '(progn
     ;; WTLS_PROTO: `smtp' or `imap' (STARTTLS, then a relay to this
     ;; Emacs's own gnutls-serv), or `alert' (SMTP; after STARTTLS the
     ;; line "GO" makes it send a TLS alert record, nothing else).
     (defvar wtls-proto (intern (getenv "WTLS_PROTO")))
     (defvar wtls-serv nil)
     (defvar wtls-tls-port nil)
     (defvar wtls-alert (unibyte-string #x15 3 3 0 2 2 40))
     (add-hook 'kill-emacs-hook
               (lambda () (when wtls-serv (delete-process wtls-serv))))
     (defun wtls-tls-server ()
       (catch 'up
         (dotimes (_ 10)
           (let* ((port (+ 20000 (random 45535)))
                  (out "")
                  (proc (start-process "gnutls-serv" nil "gnutls-serv"
                                       "--x509keyfile" (getenv "WTLS_KEY")
                                       "--x509certfile" (getenv "WTLS_CERT")
                                       "--port" (number-to-string port))))
             (set-process-filter proc (lambda (_p s) (setq out (concat out s))))
             (while (and (process-live-p proc)
                         (not (string-match-p (format "port %d\\.\\.\\." port)
                                              out)))
               (accept-process-output proc 0.1))
             (if (string-match-p (format "port %d\\.\\.\\.done" port) out)
                 (progn (setq wtls-serv proc) (throw 'up port))
               (delete-process proc))))
         (kill-emacs 2)))
     (defun wtls-upgrade (conn rest)
       (let ((up (make-network-process
                  :name "up" :host "127.0.0.1" :service wtls-tls-port
                  :coding 'binary :noquery t
                  :filter (lambda (_p s)
                            (when (process-live-p conn)
                              (process-send-string conn s)))
                  :sentinel (lambda (_p _e)
                              (when (process-live-p conn)
                                (delete-process conn))))))
         (process-put conn 'up up)
         (unless (equal rest "")
           (process-send-string up rest))))
     (defun wtls-line (conn line rest)
       ;; Answer LINE; return what is left to parse, or nil.
       (cond
        ((process-get conn 'starttls)
         (if (not (string-prefix-p "GO" line))
             rest
           (process-put conn 'done t)
           (process-send-string conn wtls-alert)
           nil))
        ((string-match-p "STARTTLS" line)
         (process-send-string conn (if (eq wtls-proto 'imap)
                                       "1 OK Begin TLS\r\n"
                                     "220 2.0.0 Ready to start TLS\r\n"))
         (if (not wtls-tls-port)
             (progn (process-put conn 'starttls t) rest)
           (wtls-upgrade conn rest)
           nil))
        ((eq wtls-proto 'imap)
         (process-send-string
          conn (format "%s OK [CAPABILITY IMAP4rev1 STARTTLS] done\r\n"
                       (car (split-string line " "))))
         rest)
        (t (process-send-string conn "250-mock\r\n250 STARTTLS\r\n")
           rest)))
     (defun wtls-filter (conn s)
       (let ((up (process-get conn 'up)))
         (cond
          (up (process-send-string up s))
          ((process-get conn 'done))
          (t
           (let ((buf (concat (process-get conn 'buf) s)))
             (while (and buf (string-match "\n" buf))
               (let ((line (substring buf 0 (match-beginning 0))))
                 (setq buf (substring buf (match-end 0)))
                 (when (string-suffix-p "\r" line)
                   (setq line (substring line 0 -1)))
                 (setq buf (wtls-line conn line buf))))
             (process-put conn 'buf buf))))))
     (unless (eq wtls-proto 'alert)
       (setq wtls-tls-port (wtls-tls-server)))
     (let ((server (make-network-process
                    :name "mock" :server t :host 'local :service t
                    :family 'ipv4 :coding 'binary :noquery t
                    :filter #'wtls-filter
                    :log (lambda (_server conn _msg)
                           (process-send-string
                            conn (if (eq wtls-proto 'imap)
                                     "* OK mock ready\r\n"
                                   "220 mock ESMTP\r\n"))))))
       (message "port %d" (process-contact server :service))
       (while t
         (accept-process-output nil 1))))
  "The mock server's program, which a child Emacs loads.")

(defun workers-tls-tests--mock (proto)
  "Start the mock server for PROTO in a child Emacs: (PROCESS . PORT).
PROTO is `smtp', `imap' or `alert' (see `workers-tls-tests--mock-program')."
  (let* ((file (make-temp-file "wtls-mock" nil ".el"))
         (out "")
         (process-environment
          (append (list (concat "WTLS_PROTO=" (symbol-name proto))
                        (concat "WTLS_KEY=" (expand-file-name
                                             "key.pem" workers-tls-tests--resources))
                        (concat "WTLS_CERT=" (expand-file-name
                                              "cert.pem" workers-tls-tests--resources)))
                  process-environment))
         (proc nil))
    (with-temp-file file
      (insert ";; -*- lexical-binding: t -*-\n")
      (let ((print-length nil) (print-level nil))
        (prin1 workers-tls-tests--mock-program (current-buffer)))
      (insert "\n"))
    ;; Its only output is the port line, on stderr (unbuffered).
    (setq proc (make-process
                :name "wtls-mock" :connection-type 'pipe :noquery t
                :coding 'binary
                :command (list (expand-file-name invocation-name
                                                 invocation-directory)
                               "-Q" "--batch" "-l" file)
                :filter (lambda (_p s) (setq out (concat out s)))))
    (process-put proc 'file file)
    (unless (workers-tls-tests--until
             (lambda () (string-match-p "^port [0-9]+$" out)) 1500)
      (error "The mock server did not start: %S" out))
    (string-match "^port \\([0-9]+\\)$" out)
    (cons proc (string-to-number (match-string 1 out)))))

(defun workers-tls-tests--mock-stop (mock)
  "Stop MOCK: SIGTERM, so its `kill-emacs-hook' stops its gnutls-serv."
  (let ((proc (car mock)))
    (when (process-live-p proc)
      (signal-process proc 'TERM)
      (workers-tls-tests--until (lambda () (not (process-live-p proc)))))
    (delete-process proc)
    (delete-file (process-get proc 'file))))

(defmacro workers-tls-tests--with-mock (spec &rest body)
  "Bind (PORT PROTO) of SPEC to a mock server's port around BODY."
  (declare (indent 1) (debug ((symbolp form) body)))
  (let ((mock (make-symbol "mock")))
    `(let* ((,mock (workers-tls-tests--mock ,(cadr spec)))
            (,(car spec) (cdr ,mock)))
       (unwind-protect
           (progn ,@body)
         (workers-tls-tests--mock-stop ,mock)))))

(defun workers-tls-tests--starttls-params (proto)
  "The `open-network-stream' parameters smtpmail or nnimap (PROTO) use."
  (pcase proto
    ('smtp
     ;; smtpmail-via-smtp's, but for two: no client certificate (no
     ;; auth-source lookup), and a capability command whose echo ends
     ;; like an SMTP reply: after the upgrade, network-stream asks for
     ;; the capabilities again, and the TLS side is gnutls-serv's echo.
     (list :type 'starttls :return-list t
           :capability-command "250 EHLO localhost\r\n"
           :end-of-command "^[0-9]+ .*\r\n"
           :success "^2.*\n"
           :always-query-capabilities t
           :starttls-function (lambda (capabilities)
                                (and (string-match "[ -]STARTTLS" capabilities)
                                     "STARTTLS\r\n"))
           :use-starttls-if-possible t))
    ('imap
     ;; nnimap-open-connection-1's.
     (list :type 'starttls :warn-unless-encrypted t :return-list t
           :capability-command "1 CAPABILITY\r\n"
           :always-query-capabilities t
           :end-of-command "\r\n"
           :success " OK "
           :starttls-function (lambda (capabilities)
                                (when (string-match-p "STARTTLS" capabilities)
                                  "1 STARTTLS\r\n"))))))

(defconst workers-tls-tests--smtp-plaintext
  "220 mock ESMTP\r\n250-mock\r\n250 STARTTLS\r\n220 2.0.0 Ready to start TLS\r\n"
  "What the mock SMTP server says before the TLS handshake.")

(defun workers-tls-tests--smtp-to-starttls (proc seen)
  "Talk SMTP on PROC up to the STARTTLS reply; SEEN returns its output."
  (should (workers-tls-tests--until
           (lambda () (string-suffix-p "ESMTP\r\n" (funcall seen)))))
  (process-send-string proc "EHLO localhost\r\n")
  (should (workers-tls-tests--until
           (lambda () (string-suffix-p "250 STARTTLS\r\n" (funcall seen)))))
  (process-send-string proc "STARTTLS\r\n")
  (should (workers-tls-tests--until
           (lambda () (string-suffix-p "TLS\r\n" (funcall seen)))))
  (should (equal (funcall seen) workers-tls-tests--smtp-plaintext)))

(defun workers-tls-tests--boot (proc)
  (gnutls-boot proc 'gnutls-x509pki
               (gnutls-boot-parameters :type 'gnutls-x509pki
                                       :hostname "localhost")))

(defun workers-tls-tests--leftover-run (port)
  "STARTTLS against the alert mock on PORT with a record left unread.
Return (WHAT-THE-FILTER-SAW GNUTLS-BOOT-RESULT OWNED REFUSED RECLAIMED
PULLED PULL)."
  (let* ((out nil)
         (proc (make-network-process
                :name "wtls-c-alert" :host "127.0.0.1" :service port
                :coding 'binary :noquery t
                :filter (lambda (_p s) (push s out))))
         (seen (lambda () (apply #'concat (reverse out))))
         (owned (workers-tls-tests--info proc :owned))
         (threads (and owned (eq nidhogg-workers--mode 'threads))))
    (workers-tls-tests--smtp-to-starttls proc seen)
    ;; The server's first record (a TLS alert) follows the reply.  In
    ;; threads mode nh-io has read it and is parked before queueing it
    ;; when `gnutls-boot' runs; the hand-back's wait releases it, so the
    ;; record is queued and undelivered when the socket goes back.
    ;; Without the reader, and inline, it is still in the kernel.
    (when threads
      (nidhogg-workers--pause-at 'before-decode))
    (process-send-string proc "GO\r\n")
    (when threads
      (should (nidhogg-workers--test-await-paused 'before-decode 5000))
      (nidhogg-workers--test-release-on-wait 'before-decode))
    (let ((ret (workers-tls-tests--boot proc)))
      (prog1 (list (funcall seen) ret owned
                   (workers-tls-tests--info proc :refused)
                   (workers-tls-tests--info proc :tls-reclaimed)
                   (workers-tls-tests--info proc :tls-pulled)
                   (workers-tls-tests--info proc :tls-pull))
        (delete-process proc)))))

(ert-deftest workers/starttls ()
  ;; §17.2a B1: smtpmail's and nnimap's upgrades (their
  ;; `open-network-stream' parameters) complete over a reader-owned
  ;; stream, which `gnutls-boot' takes back and which is never handed
  ;; over again.  Bytes the reader had read and not delivered when
  ;; `gnutls-boot' ran reach GnuTLS through the pull function, never the
  ;; filter: the result is the reader-off run's.  (A real server sends
  ;; nothing between its STARTTLS reply and the ClientHello, so the
  ;; queued record here is a fatal alert: both runs fail the handshake
  ;; on it, and only GnuTLS ever sees it.)  Also an implicit-TLS stream
  ;; with `:tls-parameters' and `:nowait', never handed over.
  (workers-tls-tests--skip)
  (dolist (proto '(smtp imap))
    (workers-tls-tests--with-mock (port proto)
      (workers-tls-tests--modes
        (ert-info ((format "%s" proto) :prefix "")
          (nidhogg-workers--test-io-log t)
          (let* ((buf (generate-new-buffer " *wtls*"))
                 (res (apply #'open-network-stream "wtls-c-starttls" buf
                             "localhost" port
                             (workers-tls-tests--starttls-params proto)))
                 (stream (car res))
                 (log (nidhogg-workers--test-io-log nil)))
            (should (eq (plist-get (cdr res) :type) 'tls))
            (should (consp (gnutls-peer-status stream)))
            ;; Reader-owned while plain, handed back by `gnutls-boot'.
            (should (workers-tls-tests--event-ids log 'handover))
            (should-not (workers-tls-tests--event-ids log 'timeout))
            (should-not (workers-tls-tests--info stream :owned))
            (should (eq (workers-tls-tests--info stream :refused) 'tls))
            (should-not (workers-tls-tests--info stream :tls-pull))
            ;; Reading resumes: still not handed over.
            (set-process-filter stream t)
            (set-process-filter stream #'internal-default-process-filter)
            (should-not (workers-tls-tests--info stream :owned))
            ;; Data still flows through GnuTLS on the main thread.
            (with-current-buffer buf
              (let ((start (point-max)))
                (process-send-string stream "ping\r\n")
                (should (workers-tls-tests--until
                         (lambda ()
                           (equal (buffer-substring start (point-max))
                                  "ping\r\n"))))))
            (delete-process stream)
            (kill-buffer buf))))))
  (ert-info ("leftover" :prefix "")
    (workers-tls-tests--with-mock (port 'alert)
      (workers-tls-tests--modes
        (let ((off (let ((nidhogg-workers-process-reader nil))
                     (workers-tls-tests--leftover-run port)))
              (on (workers-tls-tests--leftover-run port)))
          (should-not (nth 2 off))
          (should (nth 2 on))
          ;; The filter saw the plaintext and nothing after it.
          (should (equal (nth 0 off) workers-tls-tests--smtp-plaintext))
          (should (equal (nth 0 on) workers-tls-tests--smtp-plaintext))
          ;; GnuTLS read the alert in both runs.
          (should (gnutls-errorp (nth 1 off)))
          (should (equal (nth 1 on) (nth 1 off)))
          (should (eq (nth 3 on) 'tls))
          (if (eq mode 'threads)
              (progn
                (should (= (nth 4 on) 7))
                (should (= (nth 5 on) 7))
                (should (nth 6 on)))
            (should (= (nth 4 on) 0))
            (should-not (nth 6 on)))))))
  (ert-info (":tls-parameters :nowait" :prefix "")
    (let ((s (workers-tls-tests--server)))
      (unwind-protect
          (workers-tls-tests--modes
            (let* ((buf (generate-new-buffer " *wtls*"))
                   (proc (make-network-process
                          :name "wtls-c-nowait" :buffer buf :host "localhost"
                          :service (cdr s) :family 'ipv4 :nowait t :noquery t
                          :tls-parameters
                          (cons 'gnutls-x509pki
                                (gnutls-boot-parameters :hostname "localhost")))))
              (should (workers-tls-tests--until
                       (lambda () (not (eq (process-status proc) 'connect)))))
              (should (eq (process-status proc) 'open))
              (should (consp (gnutls-peer-status proc)))
              (should-not (workers-tls-tests--info proc :owned))
              (should (eq (workers-tls-tests--info proc :refused) 'tls))
              (process-send-string proc "ping\n")
              (should (workers-tls-tests--until
                       (lambda ()
                         (equal (with-current-buffer buf (buffer-string))
                                "ping\n"))))
              (delete-process proc)
              (kill-buffer buf)))
        (delete-process (car s))))))

(ert-deftest workers/starttls-no-leftover ()
  ;; A STARTTLS upgrade with nothing queued (a real exchange: the
  ;; server waits for the ClientHello): the session gets no pull
  ;; function, GnuTLS reads the socket itself, the handshake completes
  ;; and data flows through it (§17.2a).
  (workers-tls-tests--skip)
  (workers-tls-tests--with-mock (port 'smtp)
    (workers-tls-tests--modes
      (let* ((out nil)
             (proc (make-network-process
                    :name "wtls-c-plain" :host "127.0.0.1" :service port
                    :coding 'binary :noquery t
                    :filter (lambda (_p s) (push s out))))
             (seen (lambda () (apply #'concat (reverse out)))))
        (should (workers-tls-tests--info proc :owned))
        (workers-tls-tests--smtp-to-starttls proc seen)
        (should (eq (workers-tls-tests--boot proc) t))
        (should (consp (gnutls-peer-status proc)))
        (should-not (workers-tls-tests--info proc :owned))
        (should (eq (workers-tls-tests--info proc :refused) 'tls))
        (should (= (workers-tls-tests--info proc :tls-reclaimed) 0))
        (should-not (workers-tls-tests--info proc :tls-pull))
        (process-send-string proc "ping\r\n")
        (should (workers-tls-tests--until
                 (lambda ()
                   (equal (funcall seen)
                          (concat workers-tls-tests--smtp-plaintext
                                  "ping\r\n")))))
        (delete-process proc)))))

(ert-deftest workers/tls-blocking-connect ()
  ;; A blocking `open-gnutls-stream' connects as a plain stream, which
  ;; the reader takes at connect, and `gnutls-negotiate' hands it straight
  ;; back (fd audit finding 11, decision 2): the handshake completes, as
  ;; network-stream-tests's TLS rows need, and the process is never handed
  ;; over again, also after `gnutls-deinit' clears its TLS state (finding
  ;; 9).
  (workers-tls-tests--skip)
  (let ((s (workers-tls-tests--server)))
    (unwind-protect
        (workers-tls-tests--modes
          (nidhogg-workers--test-io-log t)
          (let* ((buf (generate-new-buffer " *wtls*"))
                 (proc (open-gnutls-stream "wtls-c-blocking" buf "localhost"
                                           (cdr s) (list :nowait nil)))
                 (log (nidhogg-workers--test-io-log nil))
                 (resume (lambda ()
                           (set-process-filter proc t)
                           (set-process-filter
                            proc #'internal-default-process-filter))))
            (should (consp (gnutls-peer-status proc)))
            (should (workers-tls-tests--event-ids log 'handover))
            (when (eq mode 'threads)
              (should (workers-tls-tests--event-ids log 'ack)))
            (should-not (workers-tls-tests--event-ids log 'timeout))
            (should-not (workers-tls-tests--info proc :owned))
            (should (eq (workers-tls-tests--info proc :refused) 'tls))
            (should-not (workers-tls-tests--info proc :tls-pull))
            (process-send-string proc "ping\n")
            (should (workers-tls-tests--until
                     (lambda ()
                       (equal (with-current-buffer buf (buffer-string))
                              "ping\n"))))
            (funcall resume)
            (should-not (workers-tls-tests--info proc :owned))
            (gnutls-deinit proc)
            (funcall resume)
            (should-not (workers-tls-tests--info proc :owned))
            (should (eq (workers-tls-tests--info proc :refused) 'tls))
            (delete-process proc)
            (kill-buffer buf)))
      (delete-process (car s)))))

;;; workers-tls-tests.el ends here
