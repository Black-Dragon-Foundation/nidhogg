;;; workers-framing-tests.el --- tests for workers: process framing  -*- lexical-binding: t -*-

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

;; Framed processes (Task 24, src/nidhogg-io.c): `nh-io' frames a
;; process's output into messages (content-length, lines, records),
;; tokenizes them, and the main thread delivers batches to a batch
;; function.  Every row runs in both of `nidhogg-workers--mode''s modes
;; unless it needs nh-io's thread.  Waits are on events: process output,
;; status changes and pause points.

;;; Code:

(require 'ert)
(require 'cl-lib)

(defvar nidhogg-workers--test-size-scale
  (let ((s (getenv "NIDHOGG_WORKERS_TEST_SIZE_SCALE")))
    (if s (string-to-number s) 1))
  "How much of their size the large rows use (CI's sanitizer jobs: 0.1).")

(defconst workers-fr-tests--dir
  (expand-file-name "fixtures/"
                    (file-name-directory (or load-file-name buffer-file-name))))

(defun workers-fr-tests--fixture (name)
  "The bytes of fixtures/NAME, a unibyte string."
  (with-temp-buffer
    (set-buffer-multibyte nil)
    (insert-file-contents-literally (expand-file-name name workers-fr-tests--dir))
    (buffer-string)))

(defmacro workers-fr-tests--modes (&rest body)
  "Run BODY in each mode with the reader on for every fd kind."
  (declare (indent 0) (debug t))
  `(dolist (mode '(threads inline))
     (let ((nidhogg-workers--mode mode)
           (nidhogg-workers-process-reader t)
           (nidhogg-workers-reader-pipes t)
           (nidhogg-workers-reader-sockets t)
           (nidhogg-workers-reader-ptys t)
           (nidhogg-workers--test-deliver-budget nil))
       (ert-info ((format "mode %s" mode) :prefix "")
         (unwind-protect
             (progn ,@body)
           (nidhogg-workers--test-release-on-wait nil)
           (nidhogg-workers--release)
           (nidhogg-workers--test-io-log nil)
           (dolist (p (process-list))
             (when (string-prefix-p "wfr-" (process-name p))
               (delete-process p))))))))

(defun workers-fr-tests--until (pred &optional secs)
  "Wait for process output until PRED holds (SECS seconds at most, default 5)."
  (let ((end (+ (float-time) (or secs 5))))
    (while (and (not (funcall pred)) (< (float-time) end))
      (accept-process-output nil 0.02))
    (funcall pred)))

(defun workers-fr-tests--busy-until (pred &optional tries)
  "Keep the main thread busy until PRED holds (TRIES at most, default 5000).
Each try runs `true' through `call-process', which serves no process
output: process output waits meanwhile, while a SIGCHLD still reaches
its handler."
  (let ((i 0) (n (or tries 5000)))
    (while (and (not (funcall pred)) (< i n))
      (call-process "true")
      (setq i (1+ i)))
    (funcall pred)))

(defun workers-fr-tests--info (proc key)
  (plist-get (nidhogg-workers--test-io-info proc) key))

(defun workers-fr-tests--collect (proc batch)
  "A batch function: PROC's entries go to its property `got' (newest
first), each batch's length to `batches'."
  (process-put proc 'batches (cons (length batch) (process-get proc 'batches)))
  (dolist (e batch)
    (process-put proc 'got (cons e (process-get proc 'got)))))

(defun workers-fr-tests--got (proc)
  "PROC's batch entries, oldest first."
  (reverse (process-get proc 'got)))

(defun workers-fr-tests--raw-filter (proc string)
  (process-put proc 'raw (cons string (process-get proc 'raw))))

(defun workers-fr-tests--raw (proc)
  "What PROC's filter got, joined."
  (apply #'concat (reverse (process-get proc 'raw))))

(defun workers-fr-tests--joined (proc)
  "PROC's `lines' messages, each with its newline, then its raw output."
  (concat (mapconcat (lambda (e) (concat (nth 1 e) "\n"))
                     (workers-fr-tests--got proc) "")
          (workers-fr-tests--raw proc)))

(defconst workers-fr-tests--json-args '(:object-type alist :array-type list)
  "JSON options whose values `equal' compares.")

(defun workers-fr-tests--cat (name framing &rest args)
  "A binary `cat' NAME (prefixed \"wfr-\") framed with FRAMING and ARGS.
Batches go to `workers-fr-tests--collect', raw output to the property
`raw'."
  (let ((proc (make-process :name (concat "wfr-" name) :command '("cat")
                            :connection-type 'pipe :noquery t :coding 'binary
                            :filter #'workers-fr-tests--raw-filter)))
    (should (workers-fr-tests--info proc :owned))
    (should (eq (apply #'nidhogg-set-process-framing proc framing
                       :batch-function #'workers-fr-tests--collect
                       (append args (unless (eq framing 'records)
                                      workers-fr-tests--json-args)))
                framing))
    (should (eq (nidhogg-process-framing proc) framing))
    proc))

(defun workers-fr-tests--lines (from n)
  "N JSON lines numbered from FROM."
  (mapconcat (lambda (i) (format "{\"n\":%d}\n" i))
             (number-sequence from (+ from n -1)) ""))

(defun workers-fr-tests--oracle (raw &optional parse)
  "The batch entry json-parse-* make of RAW (PARSE `buffer' or `string')."
  (condition-case err
      (list (if (eq parse 'buffer)
                (with-temp-buffer
                  (set-buffer-multibyte nil)
                  (insert raw)
                  (goto-char (point-min))
                  (apply #'json-parse-buffer workers-fr-tests--json-args))
              (apply #'json-parse-string raw workers-fr-tests--json-args))
            raw nil)
    (json-error (list nil raw err))))

(defun workers-fr-tests--frame (chunks framing &rest args)
  "What the reader's framer makes of CHUNKS, then the end of the stream."
  (apply #'nidhogg-workers--test-frame chunks t framing
         (append args workers-fr-tests--json-args)))

(defun workers-fr-tests--cut (s cuts)
  "S cut at the rising positions CUTS."
  (let ((at 0) (out nil))
    (dolist (c cuts)
      (push (substring s at c) out)
      (setq at c))
    (nreverse (cons (substring s at) out))))

(defconst workers-fr-tests--streams
  '(("lsp/stream.bin" content-length 6) ("ndjson/stream.ndjson" lines 6))
  "The recorded streams: file, framing, messages.")

(ert-deftest workers/framing-chunk-invariance ()
  ;; A recorded LSP stream (header names in any case and order, one
  ;; invalid body) and an NDJSON stream (CRLF, empty lines, one invalid
  ;; line), framed whole, split at every point and in 1,000 seeded
  ;; random splits: the same messages and RAW, each what
  ;; json-parse-string makes of RAW (§17.2).  Through a process, in both
  ;; modes, the batches hold the same entries.
  (pcase-dolist (`(,file ,framing ,count) workers-fr-tests--streams)
    (ert-info (file :prefix "stream ")
      (let* ((s (workers-fr-tests--fixture file))
             (whole (workers-fr-tests--frame (list s) framing)))
        (should (= (length (car whole)) count))
        (should (equal (cdr whole) '("" nil)))
        (dolist (e (car whole))
          (should (equal e (workers-fr-tests--oracle (nth 1 e)))))
        (should (= (cl-count-if #'caddr (car whole)) 1))
        ;; `buffer' parsing: the same values, the same error symbols.
        (let ((buf (workers-fr-tests--frame (list s) framing :parse 'buffer)))
          (should (equal (mapcar #'car (car buf)) (mapcar #'car (car whole))))
          (should (equal (mapcar (lambda (e) (car (nth 2 e))) (car buf))
                         (mapcar (lambda (e) (car (nth 2 e))) (car whole)))))
        (dotimes (i (1+ (length s)))
          (should (equal (workers-fr-tests--frame
                          (workers-fr-tests--cut s (list i)) framing)
                         whole)))
        (random "workers/framing-chunk-invariance")
        (dotimes (_ 1000)
          (let ((cuts (sort (cl-loop repeat (1+ (random 30))
                                     collect (random (1+ (length s))))
                            #'<)))
            (should (equal (workers-fr-tests--frame
                            (workers-fr-tests--cut s cuts) framing)
                           whole))))
        (workers-fr-tests--modes
          (let ((p (workers-fr-tests--cat "inv" framing)))
            (process-send-string p s)
            (should (workers-fr-tests--until
                     (lambda () (= (length (process-get p 'got)) count))))
            (should (equal (workers-fr-tests--got p) (car whole)))
            (should (equal (workers-fr-tests--raw p) ""))))))))

(ert-deftest workers/sentinel-after-last-message ()
  ;; 50 messages, then the process exits, or exits leaving a grandchild
  ;; that holds its stdout open: the sentinel runs after message 50
  ;; (B6).  In threads mode nh-io is parked after a poll the whole time,
  ;; so every message is still in the pipe at the exit and the drain
  ;; delivers them; with a delivery budget of 0 (one message per delivery
  ;; elsewhere) they come in one batch: never sliced inside the drain.
  (workers-fr-tests--modes
    (dolist (grandchild '(nil t))
      (ert-info ((if grandchild "grandchild" "exit") :prefix "")
        (let* ((pidfile (make-temp-file "wfr-pid"))
               (nidhogg-workers--test-deliver-budget 0)
               (at-sentinel 'none)
               (proc (make-process
                      :name "wfr-fifty" :connection-type 'pipe :noquery t
                      :coding 'binary :filter #'workers-fr-tests--raw-filter
                      :command
                      (list "sh" "-c"
                            (concat
                             "read x; "
                             (if grandchild
                                 (format "sleep 60 & echo $! > %s; "
                                         (shell-quote-argument pidfile))
                               "")
                             "i=1; while [ $i -le 50 ]; do "
                             "printf 'Content-Length: 9\\r\\n\\r\\n{\"n\":%3d}' $i; "
                             "i=$((i+1)); done"))
                      :sentinel (lambda (p _e)
                                  (when (eq at-sentinel 'none)
                                    (setq at-sentinel (workers-fr-tests--got p)))))))
          (unwind-protect
              (progn
                (should (workers-fr-tests--info proc :owned))
                (nidhogg-set-process-framing proc 'content-length
                                             :batch-function
                                             #'workers-fr-tests--collect)
                (when (eq mode 'threads)
                  (nidhogg-workers--pause-at 'in-poll))
                (process-send-string proc "go\n")
                (when (eq mode 'threads)
                  (should (nidhogg-workers--test-await-paused 'in-poll 5000))
                  ;; The drain's hand-back un-parks nh-io.
                  (nidhogg-workers--test-release-on-wait 'in-poll))
                (should (workers-fr-tests--busy-until
                         (lambda () (eq (process-status proc) 'exit))))
                (should (workers-fr-tests--until
                         (lambda () (not (eq at-sentinel 'none)))))
                (should (equal (mapcar (lambda (e) (gethash "n" (car e)))
                                       at-sentinel)
                               (number-sequence 1 50)))
                (when (eq mode 'threads)
                  (should (equal (process-get proc 'batches) '(50))))
                (should (equal (workers-fr-tests--raw proc) "")))
            (let ((pid (with-temp-buffer
                         (insert-file-contents pidfile)
                         (string-to-number (buffer-string)))))
              (when (> pid 0)
                (signal-process pid 'KILL)))
            (delete-file pidfile)))))))

(ert-deftest workers/accept-process-output ()
  ;; `accept-process-output' on a framed process (§5.4): non-nil once a
  ;; batch from PROC was delivered, nil on a timeout with only an
  ;; incomplete message (its bytes are not "received" yet);
  ;; JUST-THIS-ONE serves only PROC; a batch function that calls it on
  ;; its own process, or on another, waits without a spin.
  (workers-fr-tests--modes
    (let ((a (workers-fr-tests--cat "apo-a" 'lines))
          (b (workers-fr-tests--cat "apo-b" 'lines)))
      (process-send-string a "{\"n\":1}\n")
      (should (accept-process-output a 5))
      (should (equal (mapcar #'car (workers-fr-tests--got a)) '(((n . 1)))))
      (should-not (accept-process-output a 0.1))
      ;; An incomplete message.
      (process-send-string a "{\"n\":")
      (should-not (accept-process-output a 0.3))
      (should (= (length (workers-fr-tests--got a)) 1))
      (process-send-string a "2}\n")
      (should (accept-process-output a 5))
      (should (equal (mapcar #'car (workers-fr-tests--got a))
                     '(((n . 1)) ((n . 2)))))
      ;; JUST-THIS-ONE.
      (process-send-string b "{\"b\":1}\n")
      (when (eq mode 'threads)
        (should (workers-fr-tests--busy-until
                 (lambda () (> (workers-fr-tests--info b :messages) 0)))))
      (process-send-string a "{\"n\":3}\n")
      (should (accept-process-output a 5 nil t))
      (should (= (length (workers-fr-tests--got a)) 3))
      (should-not (workers-fr-tests--got b))
      (should (workers-fr-tests--until (lambda () (workers-fr-tests--got b))))
      ;; Recursion from the batch function, with nothing more to come:
      ;; on its own process, then on another.
      (dolist (other '(nil t))
        (let ((ret 'unset) (cpu nil))
          (nidhogg-set-process-framing
           a 'lines :object-type 'alist
           :batch-function
           (lambda (p batch)
             (workers-fr-tests--collect p batch)
             (let ((t0 (current-cpu-time)))
               (setq ret (accept-process-output (if other b p) 0.5 nil t))
               (let ((t1 (current-cpu-time)))
                 (setq cpu (/ (float (- (car t1) (car t0))) (cdr t1)))))))
          (process-send-string a "{\"rec\":1}\n")
          (should (workers-fr-tests--until (lambda () cpu)))
          (should-not ret)
          (should (< cpu 0.05)))))))

(ert-deftest workers/filter-replaced ()
  ;; Framed output under way, then `set-process-filter', or
  ;; `add-function' :around on the filter: framing is off and the
  ;; (new) filter gets every byte not delivered as messages, in order
  ;; (§5.2).  With a delivery budget of 0, most messages are still
  ;; queued when the filter changes.  (Inline, the bytes framing left
  ;; wait for the copy to be readable again: the EOF.)
  (workers-fr-tests--modes
    (let ((data (workers-fr-tests--lines 1 30000))
          (nidhogg-workers--test-deliver-budget 0))
      (dolist (how '(set-process-filter add-function))
        (ert-info ((symbol-name how) :prefix "")
          (let ((p (workers-fr-tests--cat (format "repl-%s" how) 'lines)))
            (process-send-string p data)
            (should (workers-fr-tests--until
                     (lambda () (workers-fr-tests--got p))))
            (if (eq how 'set-process-filter)
                (set-process-filter p #'workers-fr-tests--raw-filter)
              (add-function :around (process-filter p)
                            (lambda (orig proc s)
                              (process-put proc 'advised t)
                              (funcall orig proc s))))
            (should-not (nidhogg-process-framing p))
            (process-send-eof p)
            (should (workers-fr-tests--until
                     (lambda () (equal (workers-fr-tests--joined p) data))
                     10))
            ;; Threads: one message per delivery, so most went raw.
            ;; (Inline delivers every complete message at once.)
            (when (eq mode 'threads)
              (should (< (length (workers-fr-tests--got p)) 30000)))
            (when (eq how 'add-function)
              (should (process-get p 'advised)))))))))

(ert-deftest workers/caps ()
  ;; S7.  A writer that never stops while Emacs never waits for output:
  ;; at most 64 MB of complete messages queue (plus the read that
  ;; crossed the cap), and nh-io stops reading there.  A 100 MB message
  ;; completes.  A message over `:max-message' is a framing error: the
  ;; bytes from it on go to the filter, raw, and framing is off.
  (workers-fr-tests--modes
    (when (eq mode 'threads)
      (ert-info ("cap" :prefix "")
        (let ((proc (make-process
                     :name "wfr-yes" :connection-type 'pipe :noquery t
                     :coding 'binary :filter #'ignore
                     :command (list "sh" "-c"
                                    "yes \"$(head -c 1023 /dev/zero | tr '\\0' x)\""))))
          (nidhogg-set-process-framing proc 'lines :batch-function #'ignore)
          (nidhogg-workers--pause-at 'after-read)
          (should (nidhogg-workers--test-await-paused 'after-read 60000))
          (let ((queued (workers-fr-tests--info proc :queued))
                (cap (workers-fr-tests--info proc :cap)))
            (should (= cap (* 64 1024 1024)))
            (should (>= queued cap))
            (should (< queued (+ cap (* 1024 1024))))
            (should (> (workers-fr-tests--info proc :messages) 60000)))
          (nidhogg-workers--release 'after-read)
          (delete-process proc))))
    (ert-info ("100 MB message" :prefix "")
      (let* ((m (max 1000 (round (* 100 1024 1024 nidhogg-workers--test-size-scale))))
             (proc (make-process
                    :name "wfr-big" :connection-type 'pipe :noquery t
                    :coding 'binary :filter #'workers-fr-tests--raw-filter
                    :command
                    (list "sh" "-c"
                          (format "read x; printf 'Content-Length: %d\\r\\n\\r\\n\"'; head -c %d /dev/zero | tr '\\0' a; printf '\"'; read x"
                                  (+ m 2) m)))))
        (nidhogg-set-process-framing proc 'content-length
                                     :batch-function #'workers-fr-tests--collect)
        (process-send-string proc "go\n")
        (should (workers-fr-tests--until
                 (lambda () (workers-fr-tests--got proc)) 120))
        (let ((e (car (workers-fr-tests--got proc))))
          (should (= (length (nth 1 e)) (+ m 2)))
          (should (stringp (car e)))
          (should (= (length (car e)) m))
          (should-not (nth 2 e)))
        (should (equal (workers-fr-tests--raw proc) ""))
        (delete-process proc)))
    (ert-info (":max-message" :prefix "")
      (let* ((small "Content-Length: 7\r\n\r\n{\"a\":1}")
             (big (concat "Content-Length: 2002\r\n\r\n\""
                          (make-string 2000 ?b) "\""))
             (after "Content-Length: 7\r\n\r\n{\"c\":3}")
             (p (workers-fr-tests--cat "max" 'content-length :max-message 1000)))
        (process-send-string p (concat small big after))
        (process-send-eof p)
        (should (workers-fr-tests--until
                 (lambda () (equal (workers-fr-tests--raw p)
                                   (concat big after)))))
        (should (equal (mapcar #'car (workers-fr-tests--got p)) '(((a . 1)))))
        (should-not (nidhogg-process-framing p))))))

(ert-deftest workers/ineligible-and-turn-off ()
  ;; Not framed (`nidhogg-framing-unsupported' with the reason): TLS, a
  ;; process that decodes other than `binary', servers, datagrams,
  ;; serial ports, a `:nowait' stream before it connects.  `records'
  ;; takes any stateless coding.  `set-process-coding-system' turns
  ;; framing off: the bytes not delivered go to the filter.
  (workers-fr-tests--modes
    (cl-flet ((why (proc framing &rest args)
                (nth 2 (should-error
                        (apply #'nidhogg-set-process-framing proc framing
                               :batch-function #'ignore args)
                        :type 'nidhogg-framing-unsupported))))
      (let* ((server (make-network-process :name "wfr-server" :server t
                                           :host 'local :service t :family 'ipv4
                                           :noquery t :filter #'ignore))
             (port (process-contact server :service)))
        (should (eq (why server 'lines) 'server))
        (let ((client (make-network-process :name "wfr-nowait" :host "127.0.0.1"
                                            :service port :nowait t :noquery t
                                            :filter #'ignore)))
          (should (eq (process-status client) 'connect))
          (should (memq (why client 'lines) '(connecting not-read))))
        (when (and (fboundp 'gnutls-available-p) (gnutls-available-p))
          (require 'gnutls)
          (let ((tls (make-network-process
                      :name "wfr-tls" :host "127.0.0.1" :service port :nowait t
                      :noquery t :filter #'ignore
                      :tls-parameters (cons 'gnutls-anon
                                            (gnutls-boot-parameters
                                             :type 'gnutls-anon
                                             :hostname "127.0.0.1")))))
            (should (workers-fr-tests--until
                     (lambda () (eq (workers-fr-tests--info tls :refused) 'tls))))
            (should (eq (why tls 'content-length) 'tls)))))
      (let ((dgram (make-network-process :name "wfr-dgram" :type 'datagram
                                         :server t :host 'local :service t
                                         :family 'ipv4 :noquery t)))
        (should (eq (why dgram 'lines) 'datagram)))
      (let* ((host (make-process :name "wfr-ptyhost" :command '("sleep" "60")
                                 :connection-type 'pty :noquery t))
             (serial (make-serial-process :name "wfr-serial"
                                          :port (process-tty-name host)
                                          :speed nil :noquery t)))
        (should (eq (why serial 'records) 'serial)))
      ;; Decoding: `binary' for the JSON framings, stateless for records.
      (let ((text (make-process :name "wfr-utf8" :command '("cat")
                                :connection-type 'pipe :noquery t
                                :coding 'utf-8-unix
                                :filter #'workers-fr-tests--raw-filter)))
        (should (workers-fr-tests--info text :owned))
        (should (eq (why text 'lines) 'coding))
        (should (eq (why text 'content-length) 'coding))
        (should-not (nidhogg-process-framing text))
        (should (eq (why text 'records :coding 'undecided) 'coding))
        (should (eq (nidhogg-set-process-framing text 'records
                                                 :batch-function #'ignore)
                    'records))
        (should-not (nidhogg-set-process-framing text nil))
        (should-not (nidhogg-process-framing text)))
      ;; `set-process-coding-system' turns framing off.
      (let ((p (workers-fr-tests--cat "coding" 'lines)))
        (process-send-string p "{\"a\":1}\n{\"b\":")
        (should (workers-fr-tests--until (lambda () (workers-fr-tests--got p))))
        (set-process-coding-system p 'binary 'binary)
        (should-not (nidhogg-process-framing p))
        (process-send-string p "2}\n")
        (process-send-eof p)
        (should (workers-fr-tests--until
                 (lambda () (equal (workers-fr-tests--raw p) "{\"b\":2}\n"))))
        (should (equal (mapcar #'car (workers-fr-tests--got p)) '(((a . 1)))))))))

(ert-deftest workers/lisp-thread-lock-framed ()
  ;; A framed process locked to a `make-thread' thread gets its batches
  ;; only on that thread, though the main thread waits for output too.
  (skip-unless (featurep 'threads))
  (workers-fr-tests--modes
    (let* ((callers nil) (done nil)
           (proc (workers-fr-tests--cat "lock" 'lines))
           (worker nil))
      (nidhogg-set-process-framing proc 'lines
                                   :batch-function
                                   (lambda (p batch)
                                     (push (current-thread) callers)
                                     (workers-fr-tests--collect p batch)))
      ;; No yield between creating WORKER and locking: it never runs
      ;; before the lock is its own.
      (setq worker (make-thread (lambda ()
                                  (while (not done)
                                    (accept-process-output proc 0.05)))
                                "wfr-lock"))
      (set-process-thread proc worker)
      (unwind-protect
          (progn
            (process-send-string proc (workers-fr-tests--lines 1 20))
            (should (workers-fr-tests--until
                     (lambda () (= (length (workers-fr-tests--got proc)) 20))))
            (should callers)
            (should (cl-every (lambda (th) (eq th worker)) callers)))
        (setq done t)
        (thread-join worker)))))

(defun workers-fr-tests--repo (n)
  "A git repository with N files (and names with a newline and invalid
UTF-8), committed, then changed.  Return its directory."
  (let ((default-directory (file-name-as-directory
                            (make-temp-file "wfr-repo" t))))
    (should (eq 0 (call-process
                   "sh" nil nil nil "-c"
                   (concat
                    "git init -q . && mkdir d && "
                    (format "seq -f 'd/f%%.0f' 1 %d | xargs touch && " n)
                    "touch \"$(printf 'new\\nline')\" \"$(printf 'bad\\377name')\" && "
                    "git add -A && "
                    "git -c user.name=t -c user.email=t@example.org commit -qm init && "
                    "echo x > d/f1 && echo y > \"$(printf 'new\\nline')\" && "
                    "touch \"$(printf 'untracked\\377')\""))))
    default-directory))

(defun workers-fr-tests--split-output (dir args)
  "The output of git ARGS in DIR, split at NULs."
  (let ((default-directory dir))
    (with-temp-buffer
      (set-buffer-multibyte nil)
      (let ((coding-system-for-read 'binary))
        (should (eq 0 (apply #'call-process "git" nil t nil args))))
      (split-string (buffer-string) "\0" t))))

(ert-deftest workers/records-framing ()
  ;; §17.2: `git ls-files -z' and `git status --porcelain=v2 -z' over a
  ;; repository of 100k files, names with a newline and invalid UTF-8:
  ;; the records are the output split at NUL, with `:coding' their
  ;; decode; with a delivery budget of 0 each batch outside a drain is
  ;; one record (threads; inline delivers every complete one at once);
  ;; the sentinel runs after the last batch.
  (skip-unless (executable-find "git"))
  (let* ((n (max 100 (round (* 100000 nidhogg-workers--test-size-scale))))
         (dir (workers-fr-tests--repo n)))
    (unwind-protect
        (workers-fr-tests--modes
          (let ((default-directory dir)
                (nidhogg-workers--test-deliver-budget 0))
            (dolist (args '(("ls-files" "-z") ("status" "--porcelain=v2" "-z")))
              (dolist (coding '(nil utf-8-unix))
                (ert-info ((format "%s %s" (car args) coding) :prefix "")
                  (let* ((want (workers-fr-tests--split-output dir args))
                         (p (make-process
                             :name "wfr-git" :connection-type 'pipe :noquery t
                             :coding 'binary :filter #'workers-fr-tests--raw-filter
                             :command (list "sh" "-c"
                                            (concat "git "
                                                    (mapconcat #'identity args " ")
                                                    "; read x")))))
                    (if (equal (car args) "ls-files")
                        (should (>= (length want) (+ n 2)))
                      (should (>= (length want) 3)))
                    (apply #'nidhogg-set-process-framing p 'records
                           :separator 0 :batch-function #'workers-fr-tests--collect
                           (and coding (list :coding coding)))
                    (should (workers-fr-tests--until
                             (lambda () (>= (length (process-get p 'got))
                                            (length want)))
                             120))
                    (let ((got (workers-fr-tests--got p)))
                      (should (equal (mapcar #'cadr got) want))
                      (should (equal (mapcar #'car got)
                                     (if coding
                                         (mapcar (lambda (r)
                                                   (decode-coding-string r coding))
                                                 want)
                                       want)))
                      (should-not (delq nil (mapcar #'caddr got))))
                    (when (eq mode 'threads)
                      (should (cl-every (lambda (k) (= k 1))
                                        (process-get p 'batches))))
                    (should (equal (workers-fr-tests--raw p) ""))
                    (process-send-string p "\n")
                    (should (workers-fr-tests--until
                             (lambda () (eq (process-status p) 'exit))))))))
            ;; The sentinel after the last batch.
            (let* ((want (workers-fr-tests--split-output dir '("ls-files" "-z")))
                   (at-sentinel 'none)
                   (p (make-process
                       :name "wfr-git-exit" :connection-type 'pipe :noquery t
                       :coding 'binary :command '("git" "ls-files" "-z")
                       :filter #'workers-fr-tests--raw-filter
                       :sentinel (lambda (p _e)
                                   (when (eq at-sentinel 'none)
                                     (setq at-sentinel
                                           (mapcar #'cadr (workers-fr-tests--got p))))))))
              (nidhogg-set-process-framing p 'records
                                           :batch-function #'workers-fr-tests--collect)
              (should (workers-fr-tests--until
                       (lambda () (not (eq at-sentinel 'none))) 120))
              (should (equal at-sentinel want)))))
      (delete-directory dir t))))

(ert-deftest workers/fd-reuse-large-message ()
  ;; S16: `delete-process' while nh-io is about to tokenize a 200 MB
  ;; message (parked at `before-tokenize', released when the hand-back
  ;; waits): the tokenize stops at its first 64 KB check, the message's
  ;; bytes stay for the main thread (the log's `stopped', every byte of
  ;; the message), and only then nh-io acknowledges: no timeout.
  (workers-fr-tests--modes
    (when (eq mode 'threads)
      (let* ((m (max 100000 (round (* 200 1024 1024 nidhogg-workers--test-size-scale))))
             (header (format "Content-Length: %d\r\n\r\n" (+ m 2)))
             (proc (make-process
                    :name "wfr-huge" :connection-type 'pipe :noquery t
                    :coding 'binary :filter #'ignore
                    :command
                    (list "sh" "-c"
                          (format "read x; printf 'Content-Length: %d\\r\\n\\r\\n\"'; head -c %d /dev/zero | tr '\\0' a; printf '\"'; read x"
                                  (+ m 2) m)))))
        (nidhogg-workers--test-io-log t)
        (nidhogg-set-process-framing proc 'content-length :batch-function #'ignore)
        (nidhogg-workers--pause-at 'before-tokenize)
        (process-send-string proc "go\n")
        (should (nidhogg-workers--test-await-paused 'before-tokenize 120000))
        (nidhogg-workers--test-release-on-wait 'before-tokenize)
        (delete-process proc)
        (let* ((log (nidhogg-workers--test-io-log nil))
               (stopped (cl-member-if (lambda (e) (eq (car e) 'stopped)) log)))
          (should stopped)
          (should (= (nth 3 (car stopped)) (+ (length header) m 2)))
          (let ((id (nth 1 (car stopped))))
            (should (cl-find-if (lambda (e) (and (eq (car e) 'ack) (eql (nth 1 e) id)))
                                (cdr stopped)))
            (should-not (cl-find-if (lambda (e) (eq (car e) 'timeout)) log))))))))

(ert-deftest workers/reconfigure-inside-own-batch ()
  ;; Review Focus 5, framed: inside its own batch a process deletes
  ;; itself, changes its filter, changes its coding system, stops
  ;; itself, changes its buffer, or deletes the next process.  No batch
  ;; reaches a deleted process after `delete-process' returns; a new
  ;; filter or coding system turns framing off and the filter gets every
  ;; byte not delivered, in order; the others keep it framed.  (A
  ;; delivery budget of 0 keeps messages queued at each batch.)
  (workers-fr-tests--modes
    (let ((data (workers-fr-tests--lines 1 3000))
          (nidhogg-workers--test-deliver-budget 0))
      (cl-flet ((framed (name fn)
                  (let ((p (workers-fr-tests--cat name 'lines)))
                    (nidhogg-set-process-framing
                     p 'lines :object-type 'alist :batch-function fn)
                    p))
                ;; Every byte delivered, PROCESS still running: its close
                ;; would drop the framing (it goes with the process).
                (finish (p)
                  (should (workers-fr-tests--until
                           (lambda () (equal (workers-fr-tests--joined p) data))
                           10))))
        (ert-info ("delete-process" :prefix "")
          (let* ((deleted nil) (after 0)
                 (p (framed "re-del" (lambda (p _b)
                                       (if deleted
                                           (setq after (1+ after))
                                         (delete-process p)
                                         (setq deleted t))))))
            (process-send-string p data)
            (should (workers-fr-tests--until (lambda () deleted)))
            (workers-fr-tests--until #'ignore 0.1)
            (should (= after 0))))
        (ert-info ("set-process-filter" :prefix "")
          (let ((p (framed "re-filter"
                           (lambda (p b)
                             (workers-fr-tests--collect p b)
                             (set-process-filter p #'workers-fr-tests--raw-filter)))))
            (process-send-string p data)
            (finish p)
            (should-not (nidhogg-process-framing p))
            (should (= (length (process-get p 'batches)) 1))))
        (ert-info ("set-process-coding-system" :prefix "")
          (let ((p (framed "re-coding"
                           (lambda (p b)
                             (workers-fr-tests--collect p b)
                             (set-process-coding-system p 'binary 'binary)))))
            (process-send-string p data)
            (finish p)
            (should-not (nidhogg-process-framing p))
            (should (= (length (process-get p 'batches)) 1))))
        (ert-info ("stop-process" :prefix "")
          (let ((p (framed "re-stop"
                           (lambda (p b)
                             (unless (process-get p 'got)
                               (stop-process p))
                             (workers-fr-tests--collect p b)))))
            (process-send-string p data)
            (should (workers-fr-tests--until (lambda () (process-get p 'got))))
            (continue-process p)
            (finish p)
            (should (eq (nidhogg-process-framing p) 'lines))
            (should (equal (workers-fr-tests--raw p) ""))))
        (ert-info ("set-process-buffer" :prefix "")
          (with-temp-buffer
            (let* ((buf (current-buffer))
                   (p (framed "re-buffer"
                              (lambda (p b)
                                (unless (process-get p 'got)
                                  (set-process-buffer p buf))
                                (workers-fr-tests--collect p b)))))
              (process-send-string p data)
              (finish p)
              (should (eq (process-buffer p) buf))
              (should (eq (nidhogg-process-framing p) 'lines))
              (should (equal (workers-fr-tests--raw p) ""))
              (set-process-buffer p nil))))
        (ert-info ("delete the next process" :prefix "")
          (let* ((b-deleted nil) (b-after 0)
                 (b (framed "re-next-b" (lambda (_p _b)
                                          (when b-deleted
                                            (setq b-after (1+ b-after))))))
                 (a (framed "re-next-a" (lambda (_p _b)
                                          (unless b-deleted
                                            (delete-process b)
                                            (setq b-deleted t))))))
            (process-send-string b data)
            (process-send-string a data)
            (should (workers-fr-tests--until (lambda () b-deleted)))
            (workers-fr-tests--until #'ignore 0.1)
            (should (= b-after 0))
            (should-not (process-live-p b))
            (should (process-live-p a))))))))

(ert-deftest workers/flow-control-framed ()
  ;; §17.2 S9, review M5, on a framed connection: `stop-process' pauses
  ;; reading within one read and no batch comes while it is stopped;
  ;; `continue-process' resumes it.  `set-process-filter' t pauses it
  ;; too and turns framing off; the next filter gets the bytes, raw, in
  ;; order.
  (workers-fr-tests--modes
    (let* ((conns nil)
           (server (make-network-process
                    :name "wfr-srv" :server t :host 'local :service t
                    :family 'ipv4 :noquery t :coding 'binary
                    :log (lambda (_s c _m) (push c conns))))
           (client (make-network-process
                    :name "wfr-cli" :host "127.0.0.1"
                    :service (process-contact server :service) :family 'ipv4
                    :noquery t :coding 'binary
                    :filter #'workers-fr-tests--raw-filter)))
      (should (workers-fr-tests--until (lambda () conns)))
      (should (workers-fr-tests--info client :owned))
      (nidhogg-set-process-framing client 'lines :object-type 'alist
                                   :batch-function #'workers-fr-tests--collect)
      (let ((conn (car conns)))
        (process-send-string conn (workers-fr-tests--lines 1 10))
        (should (workers-fr-tests--until
                 (lambda () (= (length (workers-fr-tests--got client)) 10))))
        (stop-process client)
        (should (workers-fr-tests--info client :paused))
        (process-send-string conn (workers-fr-tests--lines 11 10))
        (workers-fr-tests--until #'ignore 0.3)
        (should (= (length (workers-fr-tests--got client)) 10))
        (should (<= (workers-fr-tests--info client :queued) (* 64 1024)))
        (continue-process client)
        (should-not (workers-fr-tests--info client :paused))
        (should (workers-fr-tests--until
                 (lambda () (= (length (workers-fr-tests--got client)) 20))))
        (should (equal (mapcar (lambda (e) (alist-get 'n (car e)))
                               (workers-fr-tests--got client))
                       (number-sequence 1 20)))
        ;; Filter t: paused, and framing is off.
        (set-process-filter client t)
        (should (workers-fr-tests--info client :paused))
        (should-not (nidhogg-process-framing client))
        (process-send-string conn (workers-fr-tests--lines 21 5))
        (workers-fr-tests--until #'ignore 0.3)
        (should (equal (workers-fr-tests--raw client) ""))
        (set-process-filter client #'workers-fr-tests--raw-filter)
        (should (workers-fr-tests--until
                 (lambda () (equal (workers-fr-tests--raw client)
                                   (workers-fr-tests--lines 21 5)))))
        (should (= (length (workers-fr-tests--got client)) 20))))))

;;; workers-framing-tests.el ends here
