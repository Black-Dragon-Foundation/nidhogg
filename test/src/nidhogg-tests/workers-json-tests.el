;;; workers-json-tests.el --- tests for workers: the JSON tape  -*- lexical-binding: t -*-

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

;;; Code:

(require 'ert)
(require 'cl-lib)

;; The tape's tokenizer and materializer must give what json.c gives:
;; the same value (`equal', hash tables compared entry by entry in
;; order), the same error symbol and data, and for `buffer' mode the
;; same bytes consumed.

(defun workers-json--norm (v &optional depth)
  "V with hash tables turned into (ht (KEY . VALUE)...) lists, in order.
Below 100 levels the value is replaced by `deep' (Lisp's own depth limit)."
  (let ((d (1+ (or depth 0))))
    (cond ((> d 100) 'deep)
          ((hash-table-p v)
           (let (acc)
             (maphash (lambda (k x) (push (cons k (workers-json--norm x d)) acc)) v)
             (cons 'ht (nreverse acc))))
          ((vectorp v) (vconcat (mapcar (lambda (x) (workers-json--norm x d)) v)))
          ((proper-list-p v) (mapcar (lambda (x) (workers-json--norm x d)) v))
          ((consp v) (cons (workers-json--norm (car v) d)
                           (workers-json--norm (cdr v) d)))
          (t v))))

(defmacro workers-json--outcome (&rest body)
  "The normalized value of BODY, or (err ERROR-DATA)."
  `(condition-case e (workers-json--norm (progn ,@body))
     (error (list 'err e))))

(defun workers-json--string-ref (doc opts)
  (workers-json--outcome (apply #'json-parse-string doc opts)))

(defun workers-json--string-tape (doc opts)
  (workers-json--outcome
   (apply #'nidhogg-workers--test-tokenize doc 'string opts)))

(defun workers-json--buffer-ref (doc opts gap)
  "json-parse-buffer on a narrowed unibyte buffer; (VALUE POINT-MOVE)."
  (with-temp-buffer
    (set-buffer-multibyte nil)
    (insert "xx" doc "yy")
    (narrow-to-region 3 (+ 3 (length doc)))
    (when gap
      (goto-char (+ 3 (/ (length doc) 2)))
      (insert "z")
      (delete-char -1))
    (goto-char (point-min))
    (let ((out (workers-json--outcome (apply #'json-parse-buffer opts))))
      (if (eq (car-safe out) 'err)
          out
        (list out (- (point) 3))))))

(defun workers-json--buffer-tape (doc opts)
  (condition-case e
      (let ((r (apply #'nidhogg-workers--test-tokenize doc 'buffer opts)))
        (list (workers-json--norm (car r)) (cdr r)))
    (error (list 'err e))))

(defun workers-json--check (doc opts &optional buffer gap)
  "Compare json.c and the tape on DOC; return nil or a description."
  (let ((a (workers-json--string-ref doc opts))
        (b (workers-json--string-tape doc opts)))
    (cond ((not (equal a b)) (list 'string doc opts a b))
          ((and buffer (not (multibyte-string-p doc))
                (not (equal (workers-json--buffer-ref doc opts gap)
                            (workers-json--buffer-tape doc opts))))
           (list 'buffer doc opts
                 (workers-json--buffer-ref doc opts gap)
                 (workers-json--buffer-tape doc opts))))))

(defconst workers-json--options
  '(nil
    (:object-type alist)
    (:object-type plist :array-type list)
    (:array-type list :null-object nil :false-object nil)
    (:null-object :nul :false-object :fals)
    (:object-type alist :array-type array :null-object nil)))

(defun workers-json--u (&rest bytes) (apply #'unibyte-string bytes))

(defconst workers-json--corpus
  (list
   ;; y_: accepted
   "[]" "{}" "[1]" "[\"\"]" "[[]]" "[true,false,null]" "{\"a\":1}" "{\"a\":{\"b\":[1,2]}}"
   " [ 1 , 2 ] " "\t\n\r[\t\n\r]\t\n\r" "[0]" "[-0]" "[-1]" "[1.5]" "[1E22]" "[1e-2]" "[1e+2]"
   "[123e65]" "[0e+1]" "[-0.0]" "[-0e0]" "[1.0E+2]" "[4.9e-324]" "[1.7976931348623157e308]"
   "\"\\u0060\\u012a\\u12AB\"" "\"\\uD801\\udc37\"" "\"\\ud83d\\ude39\\ud83d\\udc8d\""
   "\"\\\\u0000\"" "\"\\\"\"" "\"\\u0000\"" "\"\\u0022\"" "\"\\b\\f\\n\\r\\t\\/\\\\\"" "\"a/*b*/c\""
   "{\"\":0}" "{\"a\":1,\"a\":2}" "{\"a\":1,\"b\":2,\"a\":3}" "{\"\\u0061\":1,\"a\":2}"
   "[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70]"
   "9223372036854775807" "9223372036854775808" "-9223372036854775808" "-9223372036854775809"
   "18446744073709551615" "18446744073709551616" "123456789012345678901234567890"
   "-123456789012345678901234567890" "[0.1e1]" "[1e1]" "[1E1]" "[2.5e0]"
   "1" "-1" "0" "\"x\"" "true" "false" "null" "1e400" "-1e400" "1e-400" "[1e400]" "[-1e999]"
   "123" "0.000001" "12.5e3"
   "{\"a\":[1,{\"b\":null},true],\"c\":false}"
   (encode-coding-string "[\"é\",\"日本語\",\"😀\"]" 'utf-8)
   (encode-coding-string "{\"é\":\"é\",\"日本語\":1}" 'utf-8)
   "[\"\u00e9\",\"\u65e5\u672c\u8a9e\"]"
   (workers-json--u ?\" #xdf #xbf ?\") (workers-json--u ?\" #xef #xbf #xbf ?\")
   (workers-json--u ?\" #xf4 #x8f #xbf #xbf ?\") (workers-json--u ?\" #xe2 #x82 #xac ?\")
   ;; n_: rejected
   "" " " "[" "]" "{" "}" "[1" "[1," "[1,]" "[,1]" "[1 2]" "[1:2]" "{\"a\"" "{\"a\":" "{\"a\":1"
   "{\"a\":1,}" "{,\"a\":1}" "{\"a\" 1}" "{a:1}" "{'a':1}" "{1:1}" "{\"a\":1 \"b\":2}"
   "['a']" "[\"a\"" "\"a" "\"\\" "\"\\u" "\"\\u12" "\"\\u12G4\"" "\"\\x41\"" "\"\\a\"" "\"\\'\""
   "\"\\ud800\"" "\"\\udc00\"" "\"\\ud800\\u0041\"" "\"\\ud800\\ud800\"" "\"\\ud800x\"" "\"\\udc00\\ud800\""
   "[tru]" "[truE]" "[nul]" "[fals]" "[nulll]" "[truee]" "[True]" "[NULL]" "[-]" "[-a]" "[+1]" "[.5]"
   "[1.]" "[1.e1]" "[1e]" "[1e+]" "[1e-]" "[01]" "[-01]" "[00]" "[0x1]" "[1_0]" "[Infinity]" "[-Infinity]"
   "[NaN]" "[--1]" "[1.2.3]" "[1e1.5]" "[0.3e]" "[0.e1]" "[2.e+3]" "[1+2]" "[-.1]"
   "\"\t\"" "\"\n\"" "\"\x01\"" "\"\x1f\"" "[\"\x00\"]" "[\"a\"\"b\"]"
   "truetrue" "true true" "1 2" "[] []" "[]]" "{}}" "[1]x" "nullx" "null1" "1x" "0 1" "01" "1.5.5"
   "[1,2" "[[[[[[[[[[" "{\"a\":[}" "[{]" "[}" "{]"
   (workers-json--u ?\" #xff ?\") (workers-json--u ?\" #x80 ?\") (workers-json--u ?\" #xc0 #x80 ?\")
   (workers-json--u ?\" #xc1 #xbf ?\") (workers-json--u ?\" #xe0 #x80 #x80 ?\")
   (workers-json--u ?\" #xed #xa0 #x80 ?\") (workers-json--u ?\" #xf0 #x80 #x80 #x80 ?\")
   (workers-json--u ?\" #xf4 #x90 #x80 #x80 ?\") (workers-json--u ?\" #xf8 #x88 #x80 #x80 #x80 ?\")
   (workers-json--u ?\" #xc3 ?\") (workers-json--u ?\" #xe2 #x82 ?\") (workers-json--u ?\" #xe2 #x82)
   (workers-json--u ?\" #xf0 #x9f #x98) (workers-json--u ?\" #xc3 #x28 ?\")
   (workers-json--u ?\" #xe2 #x28 #xa1 ?\") (workers-json--u ?\" #xf0 #x28 #x8c #xbc ?\")
   (workers-json--u #xef #xbb #xbf ?\[ ?\])
   ;; i_: implementation-defined
   "[1E-2]" "[123456789012345678901234567890]" "[-123456789012345678901234567890.5]"
   "[1E400]" "[0e1]" "[1e-400]" "[\"\\ud800\\udc00\"]" "[\"\\uDBFF\\uDFFF\"]" "[\"\\u0000\"]"
   "[\"\\uFFFE\"]" "[\"\\uFFFF\"]" "{\"\\ud800\":1}" "[\"\\uD800\\uD800\\uD800\\uD800\"]"
   "\x0c[]" "[1]\x0c" " \x0b[]" "[\x00]"
   ;; whitespace, literals at the end of the input, depth edges
   "[true" "[false" "[null" "t" "tr" "tru" "f" "fals" "n" "nul" "-" "-0" "0e" "1."
   (concat (make-string 9999 ?\[) (make-string 9999 ?\]))
   (concat (make-string 10000 ?\[) (make-string 10000 ?\]))
   (concat (make-string 10001 ?\[) (make-string 10001 ?\]))
   (concat (make-string 10001 ?\[))
   (concat (apply #'concat (make-list 5000 "{\"a\":[")) "1" (apply #'concat (make-list 5000 "]}")))
   (concat (apply #'concat (make-list 5001 "{\"a\":[")) "1" (apply #'concat (make-list 5001 "]}")))
   (concat (make-string 10000 ?\[) "]")
   (concat "[" (make-string 10000 ?\[) (make-string 10000 ?\]) "]")
   (concat (make-string 30 ?\[) (make-string 30 ?\]))
   "[[],{},[[]],[{}],{\"a\":[]},{\"a\":{}}]"))

;; A random generator of JSON text and of byte-level damage to it.

(defun workers-json--rand-elt (list) (nth (random (length list)) list))

(defconst workers-json--string-bits
  (list "a" "b" " " "key" "\\n" "\\t" "\\\\" "\\\"" "\\/" "\\u0041" "\\u00e9" "\\u20ac" "\\ud83d\\ude00"
        "\\ud800" "\\udc00" "\\u12" "\\x" (workers-json--u #xc3 #xa9) (workers-json--u #xe2 #x82 #xac)
        (workers-json--u #xf0 #x9f #x98 #x80) (workers-json--u #xc0 #x80) (workers-json--u #xed #xa0 #x80)
        (workers-json--u #xff) (workers-json--u 1) "\\u0000" "\\ud801\\udc37"))

(defconst workers-json--number-bits
  '("0" "-0" "1" "-1" "12" "1234567890" "9223372036854775807" "9223372036854775808"
    "-9223372036854775808" "-9223372036854775809" "18446744073709551615" "18446744073709551616"
    "99999999999999999999999999" "-99999999999999999999999999" "0.5" "-0.5" "1.25e3" "1E-2" "1e+2"
    "1e400" "-1e400" "1e-400" "123.456e-7" "0e0" "5e-324" "01" "1." ".5" "1e" "-" "+1" "1e+"))

(defun workers-json--gen-ws ()
  (if (zerop (random 3)) (workers-json--rand-elt '(" " "\n" "\t" "\r" "  " "\r\n")) ""))

(defun workers-json--gen-string ()
  (let ((n (random 5)) (parts (list "\"")))
    (dotimes (_ n) (push (workers-json--rand-elt workers-json--string-bits) parts))
    (push "\"" parts)
    (apply #'concat (nreverse parts))))

(defun workers-json--gen (depth)
  "Random, mostly valid, JSON text (unibyte)."
  (let ((k (random (if (> depth 5) 6 9))))
    (concat
     (workers-json--gen-ws)
     (pcase k
       (0 (workers-json--rand-elt '("true" "false" "null" "tru" "nul" "truex")))
       ((or 1 2) (workers-json--rand-elt workers-json--number-bits))
       ((or 3 4 5) (workers-json--gen-string))
       ((or 6 7)
        (let ((n (random 5)) (parts nil))
          (dotimes (i n)
            (when (> i 0) (push (if (zerop (random 40)) "" ",") parts))
            (push (workers-json--gen (1+ depth)) parts))
          (concat "[" (apply #'concat (nreverse parts)) "]")))
       (_
        (let ((n (random 5)) (parts nil))
          (dotimes (i n)
            (when (> i 0) (push "," parts))
            (push (concat (workers-json--gen-ws)
                          (if (zerop (random 3)) (workers-json--rand-elt '("\"a\"" "\"b\"" "\"k\""))
                            (workers-json--gen-string))
                          (workers-json--gen-ws) (if (zerop (random 40)) "" ":")
                          (workers-json--gen (1+ depth)))
                  parts))
          (concat "{" (apply #'concat (nreverse parts)) "}"))))
     (workers-json--gen-ws))))

(defun workers-json--mutate (doc)
  "DOC with a few byte-level edits."
  (let ((s (string-to-list (if (multibyte-string-p doc) (string-to-unibyte doc) doc)))
        (structural (append "{}[]\",:\\ -+.eE0123456789tfnu\n" nil)))
    (dotimes (_ (1+ (random 3)))
      (let ((n (length s)))
        (pcase (random 5)
          (0 (when (> n 0) (setf (nth (random n) s) (workers-json--rand-elt structural))))
          (1 (when (> n 0) (setf (nth (random n) s) (random 256))))
          (2 (when (> n 0) (let ((i (random n))) (setq s (append (cl-subseq s 0 i) (cl-subseq s (1+ i)))))))
          (3 (let ((i (random (1+ n))))
               (setq s (append (cl-subseq s 0 i) (list (if (zerop (random 2)) (random 256)
                                                         (workers-json--rand-elt structural)))
                               (cl-subseq s i)))))
          (_ (when (> n 0) (setq s (cl-subseq s 0 (random n))))))))
    (apply #'unibyte-string s)))

(ert-deftest workers/json-differential ()
  "The tape gives what json-parse-string and json-parse-buffer give."
  (let ((failures nil) (n 0))
    (cl-flet ((try (doc opts &optional buffer gap)
                (cl-incf n)
                (let ((f (workers-json--check doc opts buffer gap)))
                  (when (and f (< (length failures) 5)) (push f failures)))))
      ;; The corpus under every option set, both modes, with and without a gap.
      (dolist (doc workers-json--corpus)
        (dolist (opts workers-json--options)
          (try doc opts t nil)
          (try doc opts t t)))
      ;; Seeded random and mutated documents.
      (random "workers-json-differential")
      (dotimes (i 100000)
        (let* ((doc (encode-coding-string (workers-json--gen 0) 'raw-text))
               (doc (if (zerop (% i 2)) doc (workers-json--mutate doc)))
               (opts (nth (% i (length workers-json--options)) workers-json--options)))
          (try doc opts (zerop (% i 5)) (zerop (% i 10)))
          (when (zerop (% i 7))
            ;; The same bytes as a multibyte string: positions count characters.
            (let ((m (decode-coding-string doc 'utf-8)))
              (when (multibyte-string-p m) (try m opts))))))
      ;; Depth 10,000 parses; compare the shape by walking, not `equal'.
      (let ((doc (concat (make-string 10000 ?\[) (make-string 10000 ?\])))
            (depth (lambda (v) (let ((d 0)) (while (and (vectorp v) (> (length v) 0))
                                              (setq v (aref v 0)) (cl-incf d))
                                 d))))
        (should (= 9999 (funcall depth (json-parse-string doc))))
        (should (= 9999 (funcall depth (nidhogg-workers--test-tokenize doc 'string))))))
    (should (equal nil failures))
    (should (> n 100000))))

(ert-deftest workers/json-locale ()
  "Floats parse as in the C locale whatever LC_NUMERIC is."
  (let* ((docs '("[1.5,-0.25,1e3,2.5E-2,123456.789]" "1.5" "[0.1]"))
         (want (mapcar (lambda (d) (workers-json--string-ref d nil)) docs))
         (old (nidhogg-workers--test-set-numeric-locale nil))
         (set (cl-some #'nidhogg-workers--test-set-numeric-locale
                       '("de_DE.UTF-8" "de_DE.utf8" "de_DE" "fr_FR.UTF-8" "fr_FR.utf8" "ru_RU.UTF-8"))))
    (unwind-protect
        (progn
          (unless set (ert-skip "no locale with a decimal comma installed"))
          (should (equal want (mapcar (lambda (d) (workers-json--string-tape d nil)) docs)))
          (should (equal '(0.1) (append (nidhogg-workers--test-tokenize "[0.1]" 'string) nil))))
      (nidhogg-workers--test-set-numeric-locale (or old "C")))))

;; Slicing (Task 23): the materializer's partial state lives in a rooted
;; Lisp vector, so GC may run between slices; with one entry per slice
;; (`nidhogg-workers--test-slice-entries' 1, a budget of 0) the tests
;; call `garbage-collect' between slices.

(defvar nidhogg-workers--test-size-scale
  (let ((s (getenv "NIDHOGG_WORKERS_TEST_SIZE_SCALE")))
    (if s (string-to-number s) 1))
  "How much of their size the large rows use (CI's sanitizer jobs: 0.1).")

(defun workers-json--big-doc (bytes)
  "An array of LSP-like completion items, about BYTES long."
  (with-temp-buffer
    (insert "[")
    (let ((i 0) (pad (make-string 40 ?x)))
      (while (< (buffer-size) bytes)
        (when (> i 0) (insert ","))
        (insert (format "{\"label\":\"item-%d\",\"kind\":%d,\"detail\":\"%s\",\"sortText\":\"%08d\",\
\"deprecated\":%s,\"score\":%d.5,\"data\":null,\"range\":{\"start\":{\"line\":%d,\"character\":%d},\
\"end\":{\"line\":%d,\"character\":%d}},\"tags\":[1,2,%d]}"
                        i (% i 25) pad i (if (zerop (% i 3)) "true" "false") i i (% i 80) i
                        (+ 3 (% i 80)) i))
        (setq i (1+ i))))
    (insert "]")
    (buffer-string)))

(defun workers-json--sliced (gc-every)
  "Step the test materialization one entry at a time to its value.
Call `garbage-collect' every GC-EVERY slices; check that nothing is
delivered before the last slice.  Return (VALUE SLICES)."
  (let ((slices 0) r)
    (while (not (setq r (nidhogg-workers--test-json-step 0)))
      (setq slices (1+ slices))
      (unless (eql (nidhogg-workers--test-json-in-flight) 1)
        (error "Not in flight after slice %d" slices))
      (when (zerop (% slices gc-every))
        (garbage-collect)))
    (list (car r) slices)))

(ert-deftest workers/json-slicing ()
  "Sliced materialization with GC between slices equals the unsliced."
  (let ((nidhogg-workers--test-slice-entries 1))
    ;; Small documents under every option set, GC after every slice; the
    ;; :null-object is a fresh object only the materialization holds.
    (dolist (doc (list "{\"a\":[1,{\"b\":null},true],\"c\":false,\"a\":\"x\",\"d\":{}}"
                       (encode-coding-string
                        "[\"日本語\",{\"k\":[123456789012345678901234567890,-0.5,[],null]},\"é\"]"
                        'utf-8)))
      (dolist (opts workers-json--options)
        (apply #'nidhogg-workers--test-json-start doc 'string opts)
        (pcase-let ((`(,v ,slices) (workers-json--sliced 1)))
          (should (> slices 10))
          (should (eql (nidhogg-workers--test-json-in-flight) 0))
          (should (equal (workers-json--norm v) (workers-json--string-ref doc opts)))))
      (nidhogg-workers--test-json-start doc 'string :null-object (list (make-string 3 ?n)))
      (should (equal (workers-json--norm (car (workers-json--sliced 1)))
                     (workers-json--string-ref doc '(:null-object ("nnn"))))))
    ;; 20 MB, one entry per slice, GC every 65,536 slices; the reference
    ;; parse comes after, so the GCs see only the materialization.
    (let* ((doc (workers-json--big-doc (round (* 20e6 nidhogg-workers--test-size-scale))))
           (got (progn (nidhogg-workers--test-json-start doc 'string :object-type 'alist)
                       (workers-json--sliced 65536))))
      (should (> (nth 1 got) 1000))
      (should (equal (car got) (json-parse-string doc :object-type 'alist))))
    ;; A `memory-full' in a slice unlinks the materialization, which is
    ;; an error record from then on.
    (nidhogg-workers--test-json-start "[1,[2,3],{\"a\":4},5]" 'string)
    (should-not (nidhogg-workers--test-json-step 0))
    (should-not (nidhogg-workers--test-json-step 0))
    (should (eql (nidhogg-workers--test-json-in-flight) 1))
    (let ((nidhogg-workers--test-json-fail-at 5))
      (should (equal (condition-case e
                         (progn (while (not (nidhogg-workers--test-json-step 0))) 'completed)
                       (error e))
                     memory-signal-data)))
    (should (eql (nidhogg-workers--test-json-in-flight) 0))
    (should (eq (car (should-error (nidhogg-workers--test-json-step 0)))
                'json-out-of-memory))))

(ert-deftest workers/json-key-dedup ()
  "10,000 repeats of 20 keys intern 20 symbols; hash tables intern none."
  (let* ((keys (mapcar (lambda (i) (format "dedup-key-%02d" i)) (number-sequence 1 20)))
         (obj (concat "{" (mapconcat (lambda (k) (format "\"%s\":1" k)) keys ",") "}"))
         (doc (concat "[" (mapconcat #'identity (make-list 10000 obj) ",") "]")))
    (dolist (opts '((:object-type alist) (:object-type plist :array-type list)))
      (setq nidhogg-workers--test-intern-count 0)
      (let ((v (apply #'nidhogg-workers--test-tokenize doc 'string opts)))
        (should (eql nidhogg-workers--test-intern-count 20))
        (should (equal v (apply #'json-parse-string doc opts)))))
    (setq nidhogg-workers--test-intern-count 0)
    (should (eql (length (nidhogg-workers--test-tokenize doc 'string)) 10000))
    (should (eql nidhogg-workers--test-intern-count 0))))

(ert-deftest workers/json-phase ()
  "A sample taken during a slice carries the phase `json'."
  (skip-unless (and (featurep 'nidhogg-watchdog) (eq system-type 'gnu/linux)))
  (set 'nidhogg-watchdog--test-last-sample nil)
  (nidhogg-watchdog--start 100 2000 nil :no-monitor)
  (unwind-protect
      (progn
        (set 'nidhogg-watchdog--test-sample-on-phase 'json)
        (should (equal (nidhogg-workers--test-tokenize "[1,2]" 'string) [1 2])))
    (set 'nidhogg-watchdog--test-sample-on-phase nil)
    (nidhogg-watchdog--stop))
  (let ((s (symbol-value 'nidhogg-watchdog--test-last-sample)))
    (should (eq (plist-get s :phase) 'json))
    (should (memq 'nidhogg-workers--test-tokenize (append (plist-get s :frames) nil))))
  (should (eq (funcall 'nidhogg-watchdog--test-phase) 'lisp)))

;;; workers-json-tests.el ends here
