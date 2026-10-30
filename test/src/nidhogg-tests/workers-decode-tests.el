;;; workers-decode-tests.el --- tests for workers: process decoding  -*- lexical-binding: t -*-

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

;; nh-io's stateless decoders (src/nidhogg-decode.c) and the coding
;; generation (src/nidhogg-io.c), Task 18.  The decoder row compares
;; nh-io's decoder with the main thread's chunk for chunk; it is a pure
;; function, the same in both of `nidhogg-workers--mode''s modes.  The
;; process rows run each case with the reader off, then on in both
;; modes.  Waits are on events: process output and pause points.

;;; Code:

(require 'ert)
(require 'cl-lib)

;;;; The decoders

(defconst workers-decode-tests--systems
  '(utf-8-unix utf-8-dos utf-8-mac raw-text-unix raw-text-dos raw-text-mac
    no-conversion binary)
  "The systems nh-io decodes.")

(defconst workers-decode-tests--pieces
  ["a" "Z" " " "\n" "\r" "\r\n" "\r\r\n" "\n\r" "\0" "\e[0m"
   ;; Valid UTF-8 of each length, the largest character, Emacs's
   ;; extended range (four and five bytes) and a five-byte raw byte.
   "\303\251" "\342\202\254" "\360\237\230\200" "\364\217\277\277"
   "\364\220\200\200" "\367\277\277\277" "\370\210\200\200\200"
   "\370\217\277\276\277"
   ;; Overlong forms, a surrogate, stray continuation bytes, bytes never
   ;; valid and truncated sequences.
   "\300\200" "\301\277" "\340\200\200" "\355\240\200" "\360\200\200\200"
   "\200" "\277" "\376" "\377" "\302" "\342\202" "\360\237\230"
   "\370\210\200"]
  "Byte runs the random strings are made of.")

(defun workers-decode-tests--random-bytes (pieces)
  "A unibyte string of up to PIECES runs, most from the table, some random."
  (let ((parts nil))
    (dotimes (_ (random (1+ pieces)))
      (push (if (< (random 10) 7)
                (aref workers-decode-tests--pieces
                      (random (length workers-decode-tests--pieces)))
              (unibyte-string (random 256)))
            parts))
    (apply #'concat "" parts)))

(defun workers-decode-tests--every-byte (s)
  "S cut at every boundary: one chunk per byte."
  (mapcar #'unibyte-string (append s nil)))

(defun workers-decode-tests--random-cuts (s &optional most)
  "S cut at random, into chunks of at most MOST (default 6) bytes."
  (let ((chunks nil) (i 0) (n (length s)))
    (while (< i n)
      (let ((k (min n (+ i 1 (random (or most 6))))))
        (push (substring s i k) chunks)
        (setq i k)))
    (nreverse chunks)))

(defun workers-decode-tests--mismatch (system chunks)
  "Nil if nh-io decodes CHUNKS under SYSTEM as the main thread does.
Else what differs.  The two must agree chunk for chunk (the flush at
the end included), in multibyteness too, and nh-io's character count
must be its string's."
  (let ((mine (nidhogg-workers--test-decode system chunks))
        (off (nidhogg-workers--test-decode system chunks t)))
    (unless (and mine
                 (= (length mine) (length off))
                 (cl-every
                  (lambda (m o)
                    (and (equal (car m) (car o))
                         (or (equal (car m) "")
                             (eq (multibyte-string-p (car m))
                                 (multibyte-string-p (car o))))
                         (= (cdr m) (length (car m)))))
                  mine off))
      (list system chunks mine off))))

(defun workers-decode-tests--whole-mismatch (system s chunks)
  "Nil if nh-io's decode of S in CHUNKS joins up to `decode-coding-string''s.
Not for utf-8-dos: Emacs's own decoder drops a byte read after a CR and
found invalid, so where a stream is cut changes its result."
  (unless (eq system 'utf-8-dos)
    (let ((mine (apply #'concat
                       (mapcar #'car (nidhogg-workers--test-decode
                                      system chunks)))))
      (unless (equal mine (decode-coding-string s system))
        (list system s mine)))))

(ert-deftest workers/decode-differential ()
  ;; §17.2a: 100,000 seeded byte strings (valid and invalid UTF-8, raw
  ;; bytes, CR/LF mixes, a lone CR at chunk ends), each cut at every
  ;; boundary or at random, under each stateless system in turn: nh-io's
  ;; chunks are the reader-off run's, an incomplete tail at the end is
  ;; flushed as CODING_MODE_LAST_BLOCK flushes it, and the chunks join
  ;; up to `decode-coding-string' of the whole.  Then long strings, cut
  ;; where decode_coding's character buffer fills (0x4000 characters).
  (random "workers-decode-differential")
  (let ((n 100000)
        (nsys (length workers-decode-tests--systems))
        (failure nil))
    (dotimes (i n)
      (unless failure
        (let* ((system (nth (% i nsys) workers-decode-tests--systems))
               (s (workers-decode-tests--random-bytes 8))
               (chunks (if (cl-evenp (/ i nsys))
                           (workers-decode-tests--every-byte s)
                         (workers-decode-tests--random-cuts s))))
          (setq failure
                (or (workers-decode-tests--mismatch system chunks)
                    (workers-decode-tests--whole-mismatch system s chunks))))))
    (should-not failure)
    ;; A CR, then an invalid byte, on either side of the point where the
    ;; character buffer fills; one chunk, and two cut there.
    (dolist (k (number-sequence (- #x4000 4) (+ #x4000 2)))
      (let ((s (concat (make-string k ?a) "\r\342A\r\342\202"
                       (make-string 40 ?b) "\r")))
        (dolist (system '(utf-8-dos utf-8-unix raw-text-dos))
          (should-not (workers-decode-tests--mismatch system (list s)))
          (should-not (workers-decode-tests--mismatch
                       system (list (substring s 0 (1+ k))
                                    (substring s (1+ k))))))))
    ;; Long random strings in a few long chunks, as process reads come.
    (dotimes (_ 24)
      (let* ((s (apply #'concat
                       (cl-loop repeat 12000
                                collect (workers-decode-tests--random-bytes 3))))
             (chunks (workers-decode-tests--random-cuts s 30000)))
        (dolist (system workers-decode-tests--systems)
          (should-not (workers-decode-tests--mismatch system chunks))
          (should-not (workers-decode-tests--whole-mismatch
                       system s chunks)))))
    ;; Only the stateless systems.
    (dolist (system '(utf-8 raw-text undecided utf-8-emacs-unix
                      utf-8-with-signature-unix latin-1-unix))
      (should-not (nidhogg-workers--test-decode system '("abc"))))))

;;;; Processes

(defmacro workers-decode-tests--runs (&rest body)
  "Run BODY with the reader off, then on in each mode.
RUN is `off', `threads' or `inline' in BODY."
  (declare (indent 0) (debug t))
  `(dolist (run '(off threads inline))
     (let ((nidhogg-workers--mode (if (eq run 'inline) 'inline 'threads))
           (nidhogg-workers-process-reader (not (eq run 'off)))
           (nidhogg-workers-reader-pipes t)
           (nidhogg-workers-reader-sockets t)
           (nidhogg-workers-reader-ptys t))
       (ert-info ((format "run %s" run) :prefix "")
         (unwind-protect
             (progn ,@body)
           (nidhogg-workers--release)
           (dolist (p (process-list))
             (when (string-prefix-p "wdc-" (process-name p))
               (delete-process p))))))))

(defun workers-decode-tests--until (pred &optional tries)
  "Wait for process output until PRED holds (TRIES waits of 20 ms at most)."
  (let ((i 0) (n (or tries 250)))
    (while (and (not (funcall pred)) (< i n))
      (accept-process-output nil 0.02)
      (setq i (1+ i)))
    (funcall pred)))

(defun workers-decode-tests--info (proc key)
  (plist-get (nidhogg-workers--test-io-info proc) key))

(defun workers-decode-tests--record (proc string)
  "A filter: keep STRING, its multibyteness and `last-coding-system-used'."
  (process-put proc 'calls
               (cons (list string (multibyte-string-p string)
                           last-coding-system-used)
                     (process-get proc 'calls))))

(defun workers-decode-tests--calls (proc)
  (reverse (process-get proc 'calls)))

(defun workers-decode-tests--await-calls (proc n)
  "Wait until PROC's filter has had N calls."
  (workers-decode-tests--until
   (lambda () (>= (length (process-get proc 'calls)) n))))

(defun workers-decode-tests--cat (name decoding &rest props)
  "A `cat' NAME decoding with DECODING, encoding UTF-8, recording its calls.
PROPS come first, so they override these."
  (apply #'make-process
         (append props
                 (list :name (concat "wdc-" name) :command '("cat")
                       :connection-type 'pipe :noquery t
                       :coding (cons decoding 'utf-8-unix)
                       :filter #'workers-decode-tests--record))))

(defun workers-decode-tests--u8 (string)
  (encode-coding-string string 'utf-8-unix))

(defun workers-decode-tests--during-decode (run proc string change)
  "Send STRING to PROC and call CHANGE while its echo is read, not decoded.
In RUN `threads' nh-io holds the chunk at `before-decode', its coding
generation already taken, until CHANGE returns; otherwise the bytes are
still in the kernel.  Return non-nil if nh-io parked (or did not need
to)."
  (if (not (eq run 'threads))
      (progn (process-send-string proc string)
             (funcall change)
             t)
    (nidhogg-workers--pause-at 'before-decode)
    (process-send-string proc string)
    (let ((parked (nidhogg-workers--test-await-paused 'before-decode 5000)))
      (funcall change)
      (nidhogg-workers--release 'before-decode)
      parked)))

(defun workers-decode-tests--counts (run proc)
  "PROC's (DECODED REDECODED) with the reader on in RUN; nil with it off."
  (unless (eq run 'off)
    (list (workers-decode-tests--info proc :decoded)
          (workers-decode-tests--info proc :redecoded))))

(defun workers-decode-tests--moves (run proc thunk)
  "Call THUNK; with the reader on in RUN, PROC's coding generation must move."
  (let ((g0 (and (not (eq run 'off))
                 (workers-decode-tests--info proc :coding-generation))))
    (funcall thunk)
    (when g0
      (should (> (workers-decode-tests--info proc :coding-generation) g0)))))

(ert-deftest workers/coding-change ()
  ;; §17.2a S3: with a chunk read and not yet decoded (threads: nh-io
  ;; parked at `before-decode' with the old generation), each coding
  ;; change makes it decode as with the reader off, where the bytes were
  ;; still in the kernel; the generation moves on every
  ;; setup_process_coding_systems.  `:decoded' counts the deliveries
  ;; that took nh-io's decode, `:redecoded' those the main thread had to
  ;; decode again.
  (workers-decode-tests--runs
    ;; set-process-coding-system.
    (let ((p (workers-decode-tests--cat "coding" 'utf-8-unix)))
      (unless (eq run 'off)
        (should (workers-decode-tests--info p :owned))
        (should (eq (workers-decode-tests--info p :decoder) 'utf-8-unix)))
      (process-send-string p "fïrst\n")
      (should (workers-decode-tests--await-calls p 1))
      (workers-decode-tests--moves
       run p
       (lambda ()
         (should (workers-decode-tests--during-decode
                  run p "sëcond\n"
                  (lambda ()
                    (set-process-coding-system p 'raw-text-unix
                                               'utf-8-unix))))))
      (should (workers-decode-tests--await-calls p 2))
      (should (equal (workers-decode-tests--calls p)
                     `(("fïrst\n" t utf-8-unix)
                       (,(workers-decode-tests--u8 "sëcond\n")
                        nil raw-text-unix))))
      (should (equal (workers-decode-tests--counts run p)
                     (pcase run
                       ('threads '(1 1)) ('inline '(2 0)) (_ nil)))))

    ;; set-process-buffer to a unibyte buffer under the default filter:
    ;; raw text from then on.
    (let* ((mb (generate-new-buffer " wdc-mb"))
           (ub (generate-new-buffer " wdc-ub"))
           (p (make-process :name "wdc-buffer" :command '("cat")
                            :connection-type 'pipe :noquery t
                            :coding '(utf-8-unix . utf-8-unix) :buffer mb)))
      (unwind-protect
          (progn
            (with-current-buffer ub (set-buffer-multibyte nil))
            (process-send-string p "ä\n")
            (should (workers-decode-tests--until
                     (lambda () (equal (with-current-buffer mb
                                         (buffer-string))
                                       "ä\n"))))
            (workers-decode-tests--moves
             run p
             (lambda ()
               (should (workers-decode-tests--during-decode
                        run p "ünï\n"
                        (lambda () (set-process-buffer p ub))))))
            (let ((want (workers-decode-tests--u8 "ünï\n")))
              (should (workers-decode-tests--until
                       (lambda () (= (buffer-size ub) (length want)))))
              (should (equal (with-current-buffer ub (buffer-string))
                             want))))
        (delete-process p)
        (kill-buffer mb)
        (kill-buffer ub)))

    ;; set-process-filter: the new filter gets the chunk, decoded.
    (let ((p (workers-decode-tests--cat "filter" 'utf-8-unix)))
      (workers-decode-tests--moves
       run p
       (lambda ()
         (should (workers-decode-tests--during-decode
                  run p "ñ\n"
                  (lambda ()
                    (set-process-filter
                     p (lambda (proc s)
                         (process-put proc 'new
                                      (cons (list s (multibyte-string-p s))
                                            (process-get proc 'new))))))))))
      (should (workers-decode-tests--until (lambda () (process-get p 'new))))
      (should (equal (process-get p 'new) '(("ñ\n" t))))
      (should (equal (workers-decode-tests--counts run p)
                     (pcase run
                       ('threads '(0 1)) ('inline '(1 0)) (_ nil)))))

    ;; A translation table: the main thread decodes every chunk.
    (let ((p (workers-decode-tests--cat "table" 'utf-8-unix))
          (standard-translation-table-for-decode
           (make-translation-table '((?a . ?b)))))
      (process-send-string p "abc\n")
      (should (workers-decode-tests--await-calls p 1))
      (should (equal (workers-decode-tests--calls p)
                     '(("bbc\n" t utf-8-unix))))
      (unless (eq run 'off)
        (should (equal (workers-decode-tests--counts run p) '(0 1)))))

    ;; Inside the filter (Review Focus 5): on its first call the filter
    ;; sends more and changes the coding while that is read.
    (let ((p (workers-decode-tests--cat
              "inside" 'utf-8-unix
              :filter (lambda (proc s)
                        (unless (process-get proc 'calls)
                          (process-put
                           proc 'parked
                           (workers-decode-tests--during-decode
                            run proc "sëcond\n"
                            (lambda ()
                              (set-process-coding-system
                               proc 'raw-text-unix 'utf-8-unix)))))
                        (workers-decode-tests--record proc s)))))
      (process-send-string p "fïrst\n")
      (should (workers-decode-tests--await-calls p 2))
      (should (process-get p 'parked))
      (should (equal (workers-decode-tests--calls p)
                     `(("fïrst\n" t utf-8-unix)
                       (,(workers-decode-tests--u8 "sëcond\n")
                        nil raw-text-unix))))
      (when (eq run 'threads)
        (should (equal (workers-decode-tests--counts run p) '(1 1)))))

    ;; `undecided' settling on utf-8-unix: nh-io decodes from the next
    ;; chunk on.
    (with-coding-priority '(utf-8)
      (let ((p (workers-decode-tests--cat "undecided" 'undecided)))
        (unless (eq run 'off)
          (should-not (workers-decode-tests--info p :decoder)))
        (process-send-string p "héllo\n")
        (should (workers-decode-tests--await-calls p 1))
        (unless (eq run 'off)
          (should (eq (workers-decode-tests--info p :decoder) 'utf-8-unix)))
        (process-send-string p "wörld\n")
        (should (workers-decode-tests--await-calls p 2))
        (should (equal (workers-decode-tests--calls p)
                       '(("héllo\n" t utf-8-unix) ("wörld\n" t utf-8-unix))))
        (unless (eq run 'off)
          (should (equal (workers-decode-tests--counts run p) '(1 0))))))

    ;; A process that takes the fd numbers of a dead one (its wake fd
    ;; among them) decodes with its own struct.
    (let ((a (workers-decode-tests--cat "a" 'utf-8-unix)))
      (process-send-string a "ä\n")
      (should (workers-decode-tests--await-calls a 1))
      (delete-process a)
      (let ((b (workers-decode-tests--cat "b" 'raw-text-unix)))
        (unless (eq run 'off)
          (should (eq (workers-decode-tests--info b :decoder) 'raw-text-unix)))
        (process-send-string b "ä\n")
        (should (workers-decode-tests--await-calls b 1))
        (should (equal (workers-decode-tests--calls b)
                       `((,(workers-decode-tests--u8 "ä\n")
                          nil raw-text-unix))))))))

;;;; Recorded sessions

(defun workers-decode-tests--streams ()
  "Sessions of the kinds §17.2a names: (NAME . UNIBYTE-STRING).
Each starts with a short line, so the end of line is detected the same
way whatever the first read's size, and ends in a marker."
  (random "workers-decode-streams")
  (let ((u8 #'workers-decode-tests--u8)
        (end "\n__END__\n"))
    (mapcar
     (lambda (stream) (cons (car stream) (concat "start\n" (cdr stream))))
     (list
     (cons 'seq (concat (mapconcat #'number-to-string
                                   (number-sequence 1 10000) "\n")
                        end))
     (cons 'compile
           (concat
            (funcall u8 (mapconcat
                         (lambda (i)
                           (format "src/fïle%d.c:%d:%d: \e[01;35mwarning:\e[0m \
‘x%d’ unused — see «doc» [-Wunused]\n" (% i 37) i (% i 80) i))
                         (number-sequence 1 600) ""))
            end))
     (cons 'rg-json
           (concat
            (funcall u8 (mapconcat
                         (lambda (i)
                           (format "{\"type\":\"match\",\"data\":{\"path\":\
{\"text\":\"dïr/ü%d.txt\"},\"lines\":{\"text\":\"naïve ✓ %d 日本語\\n\"},\
\"line_number\":%d}}\n" i i i))
                         (number-sequence 1 400) ""))
            end))
     (cons 'git-log
           (concat
            (funcall u8 (mapconcat
                         (lambda (i)
                           (format "commit %08x%08x%08x%08x%08x\nAuthor: Jörg \
Müller <jm@example.org>\nDate:   Thu Oct 22 12:%02d:00 2026 +0200\n\n    \
Fix ✓ handling of 日本語 (#%d)\n\n"
                                   (random 65536) (random 65536) (random 65536)
                                   (random 65536) i (% i 60) i))
                         (number-sequence 1 300) ""))
            end))
     (cons 'websocket
           (concat
            (apply #'concat
                   (cl-loop
                    for i from 1 to 400
                    collect
                    (if (cl-evenp i)
                        (let ((text (funcall u8 (format "{\"op\":\"frame\",\
\"n\":%d,\"s\":\"é✓\"}" i))))
                          (concat (unibyte-string #x81 (length text)) text))
                      ;; No CR in the binary frames: bare CRs next to a
                      ;; pty's CR LFs settle the stream on utf-8-dos,
                      ;; where Emacs's own decode of an invalid byte
                      ;; after a CR depends on the read sizes.
                      (let ((bytes (apply #'unibyte-string
                                          (cl-loop repeat (+ 14 (random 120))
                                                   collect (let ((b (random 256)))
                                                             (if (= b ?\r) ?\n b))))))
                        (concat (unibyte-string #x82 (length bytes)) bytes)))))
            end))
     (cons 'shell
           (concat
            (funcall u8 (mapconcat
                         (lambda (i)
                           (format "$ ls --color\r\n\e[01;34mdïr\e[0m  \
fïle%d.txt\r\n$ echo ünï\r\nünï\r\n\r[%3d%%] ████\r[100%%] █████\r\n"
                                   i (% i 100)))
                         (number-sequence 1 400) ""))
            end))))))

(defun workers-decode-tests--recorder (proc string)
  "A filter: record the call and keep the last bytes seen."
  (workers-decode-tests--record proc string)
  (let ((tail (concat (process-get proc 'tail) string)))
    (process-put proc 'tail (substring tail (max 0 (- (length tail) 16))))))

(defun workers-decode-tests--replay (file system connection)
  "Replay FILE through `cat' over CONNECTION, decoding with SYSTEM.
Return the filter's calls and, with the reader on, its info.  The
process stays alive until the marker is in, so no output waits on its
exit."
  (let* ((read-process-output-max 4096)
         (p (make-process :name "wdc-replay"
                          :command (list "sh" "-c" "cat \"$0\"; exec sleep 60"
                                         file)
                          :connection-type connection :noquery t
                          :coding (cons system system)
                          :filter #'workers-decode-tests--recorder)))
    (unwind-protect
        (progn
          (should (workers-decode-tests--until
                   (lambda ()
                     (let ((tail (process-get p 'tail)))
                       (and tail (string-match-p "__END__\r?\n\\'" tail))))
                   1500))
          (list (workers-decode-tests--calls p)
                (and nidhogg-workers-process-reader
                     (nidhogg-workers--test-io-info p))))
      (delete-process p))))

(defun workers-decode-tests--difference (off on)
  "Nil if the filter calls ON are equivalent to the reader-off calls OFF.
Each call of ON has OFF's multibyteness (the same for every call) and
one of OFF's `last-coding-system-used', the last calls agree on it, and
the arguments joined are `equal': with one multibyteness throughout,
that is the arguments re-split at OFF's call boundaries being `equal'
call for call."
  (let ((mbs (delete-dups (mapcar #'cadr off)))
        (codings (delete-dups (mapcar #'caddr off)))
        (text (lambda (calls) (apply #'concat (mapcar #'car calls)))))
    (cond
     ((cdr mbs) (list 'off-mixes-multibyteness mbs))
     ((cl-find-if-not (lambda (c) (eq (cadr c) (car mbs))) on)
      (list 'multibyteness (car mbs)))
     ((cl-find-if-not (lambda (c) (memq (caddr c) codings)) on)
      (list 'coding codings (delete-dups (mapcar #'caddr on))))
     ((not (eq (caddr (car (last on))) (caddr (car (last off)))))
      (list 'last-coding (caddr (car (last off))) (caddr (car (last on)))))
     ((not (equal (funcall text on) (funcall text off)))
      (list 'text)))))

(ert-deftest workers/raw-filter-equivalence ()
  ;; §17.2a B2: recorded `seq', `compile', `rg --json', `git log',
  ;; websocket and pty shell sessions, replayed through a pipe and a pty
  ;; under utf-8, raw-text, no-conversion and binary: every filter call
  ;; has the reader-off run's multibyteness and coding, and the text is
  ;; the same.  Inline, nh-io's decode reaches the filter (`:decoded');
  ;; with threads that depends on when detection settles, so it is not
  ;; asserted there.
  (let ((files nil))
    (unwind-protect
        (progn
          (dolist (stream (workers-decode-tests--streams))
            (let ((file (make-temp-file (format "wdc-%s" (car stream))))
                  (coding-system-for-write 'no-conversion))
              (write-region (cdr stream) nil file nil 'silent)
              (push (cons (car stream) file) files)))
          (dolist (connection '(pipe pty))
            (dolist (system '(utf-8 raw-text no-conversion binary))
              (dolist (file files)
                (ert-info ((format "%s %s %s" (car file) connection system)
                           :prefix "")
                  (let ((off (car (let ((nidhogg-workers-process-reader nil))
                                    (workers-decode-tests--replay
                                     (cdr file) system connection)))))
                    (should off)
                    (dolist (mode '(threads inline))
                      (ert-info ((format "mode %s" mode) :prefix "")
                        (let* ((nidhogg-workers--mode mode)
                               (nidhogg-workers-process-reader t)
                               (nidhogg-workers-reader-pipes t)
                               (nidhogg-workers-reader-ptys t)
                               (on (workers-decode-tests--replay
                                    (cdr file) system connection)))
                          (should (plist-get (cadr on) :owned))
                          (should-not (workers-decode-tests--difference
                                       off (car on)))
                          (when (eq mode 'inline)
                            (should (> (plist-get (cadr on) :decoded)
                                       0))))))))))))
      (dolist (file files)
        (delete-file (cdr file))))))

;;; workers-decode-tests.el ends here
