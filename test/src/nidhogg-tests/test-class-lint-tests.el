;;; test-class-lint-tests.el --- every test file has a class, and a lint for each class  -*- lexical-binding: t -*-

;; test-class: det

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

;; A test file's class is the header comment line `;; test-class: det'
;; (or `cond', or `timing'), exact, within the first 15 lines.  A test's
;; class is its own tag (:det, :cond, :timing) when it has one, else
;; the file's.  The files are read as data with `read', never loaded, so
;; this works on any Emacs and needs no `require' of what they require.
;;
;; A lint applies to a file as soon as it has a class line.  Files that
;; are not tests are listed in nidhogg/test/class-exempt.txt
;; (`FILE # reason', FILE relative to test/src/nidhogg-tests/).
;;
;; The rules:
;;   D (det)    no real time: no sleep-for, sit-for, asynchronous process,
;;              connection or timer creation, no with-timeout, no
;;              accept-process-output with a time (seconds or milliseconds),
;;              no clock read (float-time, current-time, or a time function
;;              given nil for now).  A synchronous `call-process' is
;;              allowed.  Peer and clock helpers are allowed.
;;   C (cond)   no fixed pause: no sleep-for, no sit-for above 0.
;;   D and C    no child script that writes, sleeps and writes again
;;              (`echo a; sleep 0.3; echo b'): such a test relies on the
;;              OS handing it the two writes as two reads, at the child's
;;              pace.  Every string inside the call that starts the
;;              child counts, quoted or not, and a helper's it calls.
;;   D and C    no `should' over a comparison of a clock read.  Only T
;;              (timing) tests assert the clock.
;; A test that calls a same-file function or macro is judged by what that
;; function does too, except for the clock-assertion rule, which reads
;; only the test's own text (a wait helper reads the clock by design).

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'seq)
(require 'subr-x)

(defconst test-class-lint--dir
  (expand-file-name "test/src/nidhogg-tests/" source-directory)
  "The directory of Nidhogg's own test files.")

(defconst test-class-lint--exempt-file
  (expand-file-name "nidhogg/test/class-exempt.txt" source-directory)
  "The list of files that are not tests and carry no class.")

(defvar test-class-lint--all-classified t
  "When non-nil, a test file with neither a class nor an exempt row fails.
Every test file has a class, so it is set; nil would make
`test-class/every-test-has-a-class' only report the count.")

(defconst test-class-lint--classes '(det cond timing))

(defconst test-class-lint--real-time
  '(make-process start-process start-file-process
    make-network-process open-network-stream
    run-at-time run-with-timer run-with-idle-timer timer-activate with-timeout
    float-time current-time time-since)
  "Symbols a D test must not use outside the clock helpers.
The fixed pauses (`sleep-for', `sit-for' above 0) are barred too, see
`test-class-lint--fixed-pause', and so are the reads of
`test-class-lint--now-args'.")

(defconst test-class-lint--clock-reads '(float-time current-time time-since)
  "Functions whose value is a clock read.")

(defconst test-class-lint--now-args
  '((time-add 1 2) (time-subtract 1 2) (time-less-p 1 2) (time-equal-p 1 2)
    (time-convert 1) (time-to-seconds 1) (decode-time 1)
    (current-time-string 1) (format-time-string 2))
  "Functions that read the clock when a time argument is nil or left out.
Each row is the function and the positions of its time arguments in a
call (1 is the first argument).")

(defconst test-class-lint--comparisons
  '(< > <= >= = /= time-less-p time-equal-p)
  "Functions that compare.")

(defconst test-class-lint--paced-writes
  "\\b\\(?:echo\\|printf\\)\\b\\(?:.\\|\n\\)*\\bsleep[ \t]+[0-9.]+\\(?:.\\|\n\\)*\\b\\(?:echo\\|printf\\)\\b"
  "A shell script that writes, sleeps, then writes again.")

(defconst test-class-lint--allowed-prefix
  "\\`\\(?:nidhogg-test-peer-\\|nidhogg-test-clock-\\|nidhogg-watchdog--test-clock\\'\\)"
  "Calls whose whole subtree the real-time rule ignores.")

;;; Reading

(defun test-class-lint--read-forms (string)
  "All top-level forms of STRING, read as data."
  (with-temp-buffer
    (insert string)
    (goto-char (point-min))
    (let (forms)
      (condition-case nil
          (while t (push (read (current-buffer)) forms))
        (end-of-file nil))
      (nreverse forms))))

(defun test-class-lint--file-class (string)
  "The class symbol of the header line in STRING, `none', or `bad'."
  (let ((lines (seq-take (split-string string "\n") 15))
        (class 'none))
    (while (and lines (eq class 'none))
      (let ((line (pop lines)))
        (when (string-prefix-p ";; test-class:" line)
          (setq class
                (if (string-match "\\`;; test-class: \\(det\\|cond\\|timing\\)\\'" line)
                    (intern (match-string 1 line))
                  'bad)))))
    class))

(defun test-class-lint--parse-test (form)
  "Split an `ert-deftest' FORM into (NAME TAGS BODY)."
  (let ((rest (nthcdr 3 form)) tags)
    (when (stringp (car rest)) (pop rest))
    (while (keywordp (car rest))
      (when (eq (car rest) :tags)
        (let ((v (cadr rest)))
          (setq tags (if (eq (car-safe v) 'quote) (cadr v) v))))
      (setq rest (cddr rest)))
    (list (nth 1 form) tags rest)))

(defun test-class-lint--test-class (file-class tags)
  "The class of a test with TAGS in a file of FILE-CLASS."
  (or (seq-find (lambda (c) (memq (intern (concat ":" (symbol-name c))) tags))
                test-class-lint--classes)
      (and (memq file-class test-class-lint--classes) file-class)))

(defun test-class-lint--helpers (forms)
  "A table of the same-file functions and macros in FORMS: name to body."
  (let ((table (make-hash-table :test 'eq)))
    (dolist (f forms)
      (when (and (memq (car-safe f) '(defun defmacro defsubst cl-defun cl-defmacro))
                 (symbolp (nth 1 f)))
        (puthash (nth 1 f) (nthcdr 3 f) table)))
    table))

;;; Walking

(defun test-class-lint--nodes (body helpers)
  "Collect what BODY uses, following calls into HELPERS.
Returns (CALLS . SYMS): every list whose head is a symbol, and every
symbol that is not a head.  Quoted data and calls into the allowed
helpers are skipped."
  (let ((calls nil) (syms nil) (seen (make-hash-table :test 'eq)))
    (cl-labels
        ((visit (x)
           (cond
            ((symbolp x)
             (when x (push x syms) (follow x)))
            ((vectorp x) (mapc #'visit x))
            ((consp x)
             (let ((head (car x)))
               (cond
                ((eq head 'quote))
                ((and (symbolp head)
                      (string-match-p test-class-lint--allowed-prefix (symbol-name head))))
                (t
                 (when (symbolp head) (push x calls) (follow head))
                 (let ((tail (if (symbolp head) (cdr x) x)))
                   (while (consp tail) (visit (pop tail)))
                   (when tail (visit tail)))))))))
         (follow (name)
           (let ((hb (gethash name helpers 'none)))
             (unless (or (eq hb 'none) (gethash name seen))
               (puthash name t seen)
               (mapc #'visit hb)))))
      (mapc #'visit body))
    (cons calls syms)))

(defun test-class-lint--uses-p (nodes names)
  "Non-nil if NODES (from `test-class-lint--nodes') mention a symbol of NAMES."
  (or (seq-some (lambda (c) (memq (car c) names)) (car nodes))
      (seq-some (lambda (s) (memq s names)) (cdr nodes))))

(defun test-class-lint--zero-p (x)
  "Non-nil if X is a literal zero."
  (and (numberp x) (zerop x)))

(defun test-class-lint--now-reads (nodes)
  "The calls in NODES that read the clock through a nil time, as strings."
  (let (found)
    (dolist (c (car nodes))
      (let ((pos (cdr (assq (car c) test-class-lint--now-args))))
        (when (and pos (proper-list-p c)
                   (seq-some (lambda (p) (null (nth p c))) pos))
          (push (format "%s of now" (car c)) found))))
    (delete-dups (nreverse found))))

(defun test-class-lint--reads-clock-p (nodes)
  "Non-nil if NODES read the clock."
  (or (test-class-lint--uses-p nodes test-class-lint--clock-reads)
      (test-class-lint--now-reads nodes)))

;;; The rules

(defun test-class-lint--real-time (nodes)
  "The D rule: what in NODES uses real time."
  (let ((found (reverse (test-class-lint--fixed-pause nodes))))
    (dolist (s test-class-lint--real-time)
      (when (test-class-lint--uses-p nodes (list s)) (push (symbol-name s) found)))
    (dolist (r (test-class-lint--now-reads nodes)) (push r found))
    (dolist (c (car nodes))
      (when (and (eq (car c) 'accept-process-output)
                 (seq-some (lambda (a) (and a (not (test-class-lint--zero-p a))))
                           (list (nth 2 c) (nth 3 c))))
        (push "accept-process-output with a time" found)))
    (nreverse found)))

(defun test-class-lint--fixed-pause (nodes)
  "The C rule: what in NODES is a fixed pause."
  (let (found)
    (when (test-class-lint--uses-p nodes '(sleep-for)) (push "sleep-for" found))
    (dolist (c (car nodes))
      (when (and (eq (car c) 'sit-for) (not (test-class-lint--zero-p (nth 1 c))))
        (push "sit-for above 0" found)))
    (when (seq-some (lambda (s) (eq s 'sit-for)) (cdr nodes))
      (push "sit-for passed on" found))
    (nreverse found)))

(defconst test-class-lint--child-starts
  '(make-process start-process start-file-process
    start-process-shell-command start-file-process-shell-command)
  "Functions that start an asynchronous child, whose output arrives in reads.")

(defun test-class-lint--child-strings (body helpers)
  "The strings of the children BODY starts: every string, quoted data
included, inside a call of `test-class-lint--child-starts', following calls
into HELPERS (from the body, and from inside such a call)."
  (let (out (seen (make-hash-table :test 'equal)))
    (cl-labels
        ((visit (x in)
           (cond
            ((stringp x) (when in (push x out)))
            ((symbolp x)
             (let ((hb (gethash x helpers 'none)))
               (unless (or (eq hb 'none) (gethash (cons x in) seen))
                 (puthash (cons x in) t seen)
                 (dolist (f hb) (visit f in)))))
            ((vectorp x) (mapc (lambda (y) (visit y in)) x))
            ((consp x)
             (let ((in (or in (and (memq (car x) test-class-lint--child-starts) t))))
               (while (consp x) (visit (pop x) in))
               (when x (visit x in)))))))
      (dolist (f body) (visit f nil)))
    out))

(defun test-class-lint--paced-child (body helpers)
  "The D and C rule over BODY: a child script that writes, sleeps and writes."
  (when (seq-some (lambda (s) (string-match-p test-class-lint--paced-writes s))
                  (test-class-lint--child-strings body helpers))
    (list "a child that writes, sleeps and writes again")))

(defun test-class-lint--tainted (body helpers)
  "The variables bound in BODY, directly or not, from a clock read."
  (let ((tainted nil) (changed t)
        (all (car (test-class-lint--nodes body helpers))))
    (while changed
      (setq changed nil)
      (let ((clocky (lambda (init)
                      (let ((n (test-class-lint--nodes (list init) helpers)))
                        (or (test-class-lint--reads-clock-p n)
                            (test-class-lint--uses-p n tainted))))))
        (dolist (c all)
          (cond
           ((memq (car c) '(let let*))
            (dolist (b (cadr c))
              (when (and (consp b) (symbolp (car b)) (cdr b)
                         (not (memq (car b) tainted))
                         (funcall clocky (cadr b)))
                (push (car b) tainted) (setq changed t))))
           ((eq (car c) 'setq)
            (let ((p (cdr c)))
              (while (cdr p)
                (when (and (symbolp (car p)) (not (memq (car p) tainted))
                           (funcall clocky (cadr p)))
                  (push (car p) tainted) (setq changed t))
                (setq p (cddr p)))))))))
    tainted))

(defun test-class-lint--asserts-clock (body helpers)
  "The D and C rule over BODY: `should' forms that compare a clock read."
  (let ((tainted (test-class-lint--tainted body helpers)) found)
    (dolist (c (car (test-class-lint--nodes body helpers)))
      (when (memq (car c) '(should should-not))
        (let ((n (test-class-lint--nodes (cdr c) helpers)))
          (when (and (test-class-lint--uses-p n test-class-lint--comparisons)
                     (or (test-class-lint--reads-clock-p n)
                         (test-class-lint--uses-p n tainted)))
            (push "should compares a clock read" found)))))
    (delete-dups found)))

(defun test-class-lint--violations (string)
  "Every lint violation in the test file text STRING, as (RULE TEST DETAIL).
RULE is `det-real-time', `cond-pause' or `clock-assert'.  A file without a
class line has none."
  (let* ((file-class (test-class-lint--file-class string))
         (forms (test-class-lint--read-forms string))
         (helpers (test-class-lint--helpers forms))
         out)
    (dolist (f forms)
      (when (eq (car-safe f) 'ert-deftest)
        (pcase-let* ((`(,name ,tags ,body) (test-class-lint--parse-test f))
                     (class (test-class-lint--test-class file-class tags))
                     (nodes (test-class-lint--nodes body helpers)))
          (when (eq class 'det)
            (dolist (d (append (test-class-lint--real-time nodes)
                               (test-class-lint--paced-child body helpers)))
              (push (list 'det-real-time name d) out)))
          (when (eq class 'cond)
            (dolist (d (append (test-class-lint--fixed-pause nodes)
                               (test-class-lint--paced-child body helpers)))
              (push (list 'cond-pause name d) out)))
          (when (memq class '(det cond))
            ;; No helpers followed: a `should' over a wait helper (whose own loop reads the
            ;; clock) compared with something else is not an assertion on the clock.
            (dolist (d (test-class-lint--asserts-clock body (make-hash-table :test 'eq)))
              (push (list 'clock-assert name d) out))))))
    (nreverse out)))

;;; Where the tags sit

(defconst test-class-lint--tag-lines 8
  "How many lines of a test nidhogg/test/check-rerun.sh reads for its :tags.")

(defun test-class-lint--late-tags (string)
  "The names of tests in STRING whose :tags sit past the line check-rerun.sh reads."
  (let (late)
    (with-temp-buffer
      (with-syntax-table emacs-lisp-mode-syntax-table
        (insert string)
        (goto-char (point-min))
        (condition-case nil
            (while t
              (forward-comment (buffer-size))
              (let* ((start (point))
                     (form (read (current-buffer)))
                     (end (point)))
                (when (and (eq (car-safe form) 'ert-deftest)
                           (nth 1 (test-class-lint--parse-test form)))
                  (goto-char start)
                  (search-forward ":tags" end t)
                  (when (> (count-lines start (point)) test-class-lint--tag-lines)
                    (push (nth 1 form) late))
                  (goto-char end))))
          (end-of-file nil))))
    (nreverse late)))

;;; The files

(defun test-class-lint--test-files ()
  "Nidhogg's own test files."
  (directory-files test-class-lint--dir t "-tests\\.el\\'"))

(defun test-class-lint--read-file (file)
  "The text of FILE."
  (with-temp-buffer
    (insert-file-contents file)
    (buffer-string)))

(defun test-class-lint--exempt ()
  "The files in the exempt list, relative to the test directory."
  (when (file-readable-p test-class-lint--exempt-file)
    (let (rows)
      (dolist (line (split-string (test-class-lint--read-file test-class-lint--exempt-file) "\n"))
        (unless (or (string-empty-p (string-trim line)) (string-prefix-p "#" line))
          (push (string-trim (car (split-string line " # "))) rows)))
      (nreverse rows))))

(defun test-class-lint--real-violations (rule)
  "The violations of RULE over every classified test file, as strings."
  (let (out)
    (dolist (file (test-class-lint--test-files))
      (dolist (v (test-class-lint--violations (test-class-lint--read-file file)))
        (when (eq (car v) rule)
          (push (format "%s: %s: %s" (file-name-nondirectory file) (nth 1 v) (nth 2 v)) out))))
    (nreverse out)))

;;; The tests

(ert-deftest test-class/det-uses-no-real-time ()
  (should-not (test-class-lint--real-violations 'det-real-time)))

(ert-deftest test-class/cond-has-no-fixed-pause ()
  (should-not (test-class-lint--real-violations 'cond-pause)))

(ert-deftest test-class/only-timing-asserts-the-clock ()
  (should-not (test-class-lint--real-violations 'clock-assert)))

(ert-deftest test-class/tags-sit-where-check-rerun-reads ()
  (should-not (let (out)
                (dolist (file (test-class-lint--test-files))
                  (dolist (n (test-class-lint--late-tags (test-class-lint--read-file file)))
                    (push (format "%s: %s" (file-name-nondirectory file) n) out)))
                out))
  ;; The bite: tags on line 8 pass, on line 9 or later fail.
  (let ((pad (lambda (n) (concat "(ert-deftest my/a ()\n" (make-string n ?\n)))))
    (should-not (test-class-lint--late-tags
                 (concat (funcall pad 6) "  :tags '(:det)\n  (should t))\n")))
    (should (equal '(my/a)
                   (test-class-lint--late-tags
                    (concat (funcall pad 8) "  :tags '(:det)\n  (should t))\n"))))
    (should-not (test-class-lint--late-tags
                 (concat (funcall pad 9) "  (should t))\n")))))

(defconst test-class-lint--header-det ";; test-class: det\n")
(defconst test-class-lint--header-cond ";; test-class: cond\n")

(ert-deftest test-class/sees-through-a-local-helper ()
  (let ((v (test-class-lint--violations
            (concat test-class-lint--header-det
                    "(defun my--wait () (sleep-for 1))\n"
                    "(defun my--outer () (my--wait))\n"
                    "(ert-deftest my/a () (my--outer) (should t))\n"))))
    (should (equal v '((det-real-time my/a "sleep-for")))))
  ;; A macro body counts too.
  (should (test-class-lint--violations
           (concat test-class-lint--header-det
                   "(defmacro my--m (&rest b) `(progn (run-at-time 1 nil #'ignore) ,@b))\n"
                   "(ert-deftest my/a () (my--m (should t)))\n")))
  ;; A helper nobody calls does not matter.
  (should-not (test-class-lint--violations
               (concat test-class-lint--header-det
                       "(defun my--wait () (sleep-for 1))\n"
                       "(ert-deftest my/a () (should t))\n"))))

(ert-deftest test-class/the-lints-bite ()
  (let ((det test-class-lint--header-det)
        (cnd test-class-lint--header-cond))
    (dolist (case '(("(sleep-for 0.1)" "sleep-for")
                    ("(sit-for 0.1)" "sit-for above 0")
                    ("(make-process :name \"x\" :command '(\"true\"))" "make-process")
                    ("(start-process \"x\" nil \"true\")" "start-process")
                    ("(run-at-time 1 nil #'ignore)" "run-at-time")
                    ("(accept-process-output p 1)" "accept-process-output with a time")
                    ("(accept-process-output p nil 50)" "accept-process-output with a time")
                    ("(accept-process-output p 0 50)" "accept-process-output with a time")
                    ("(while (< (float-time) 5) nil)" "float-time")
                    ("(let ((t0 (current-time))) t0)" "current-time")
                    ("(with-timeout (1) (ignore))" "with-timeout")
                    ("(make-network-process :name \"x\" :service 1)" "make-network-process")
                    ("(open-network-stream \"x\" nil \"localhost\" 1)" "open-network-stream")
                    ("(timer-activate tm)" "timer-activate")
                    ("(while (time-less-p nil end) nil)" "time-less-p of now")
                    ("(setq end (time-add nil 2))" "time-add of now")
                    ("(format-time-string \"%F.jsonl\")" "format-time-string of now")))
      (let ((v (test-class-lint--violations
                (concat det "(ert-deftest my/a () " (car case) ")\n"))))
        (should (member (list 'det-real-time 'my/a (cadr case)) v))))
    ;; What a D test may do.
    (should-not (test-class-lint--violations
                 (concat det "(ert-deftest my/a () (sit-for 0) (call-process \"true\") (accept-process-output p)"
                         " (accept-process-output p 0) (accept-process-output p 0 0)"
                         " (accept-process-output p nil nil t) (format-time-string \"%F\" 0)"
                         " (time-add t0 2) (time-less-p t0 t1)"
                         " (nidhogg-test-peer-make (sleep-for 1))"
                         " (nidhogg-watchdog--test-clock 5) '(sleep-for 1))\n")))
    ;; C: a fixed pause.
    (should (equal '((cond-pause my/a "sleep-for"))
                   (test-class-lint--violations
                    (concat cnd "(ert-deftest my/a () (sleep-for 0.1))\n"))))
    (should (equal '((cond-pause my/a "sit-for above 0"))
                   (test-class-lint--violations
                    (concat cnd "(ert-deftest my/a () (sit-for 1))\n"))))
    (should-not (test-class-lint--violations
                 (concat cnd "(ert-deftest my/a () (sit-for 0) (make-process :name \"x\"))\n")))
    ;; D and C: a child paced by a sleep between two writes, quoted or not, or
    ;; in a same-file helper.  A child kept alive by a sleep is fine.
    (should (equal '((cond-pause my/a "a child that writes, sleeps and writes again"))
                   (test-class-lint--violations
                    (concat cnd "(ert-deftest my/a () (make-process :name \"x\""
                            " :command '(\"sh\" \"-c\" \"echo a; sleep 0.3; echo b\")))\n"))))
    (should (equal '((cond-pause my/a "a child that writes, sleeps and writes again"))
                   (test-class-lint--violations
                    (concat cnd "(defun my--cmd () (list \"sh\" \"-c\" \"printf a\nsleep 1\nprintf b\"))\n"
                            "(ert-deftest my/a () (make-process :name \"x\" :command (my--cmd)))\n"))))
    (should (member '(det-real-time my/a "a child that writes, sleeps and writes again")
                    (test-class-lint--violations
                     (concat det "(ert-deftest my/a () (make-process :name \"x\""
                             " :command '(\"sh\" \"-c\" \"echo a; sleep 0.3; echo b\")))\n"))))
    (should-not (test-class-lint--violations
                 (concat cnd "(ert-deftest my/a () (make-process :name \"x\""
                         " :command '(\"sh\" \"-c\" \"echo $$; exec sleep 60\"))"
                         " (make-process :name \"y\" :command '(\"sh\" \"-c\" \"sleep 60 & echo $!\")))\n")))
    ;; Clock assertions, direct and through a variable.
    (should (equal '((clock-assert my/a "should compares a clock read"))
                   (test-class-lint--violations
                    (concat cnd "(ert-deftest my/a () (should (< (- (float-time) 1) 5)))\n"))))
    (should (equal '((clock-assert my/a "should compares a clock read"))
                   (test-class-lint--violations
                    (concat cnd "(ert-deftest my/a ()"
                            " (let* ((t0 (float-time)) (dt (- (float-time) t0)))"
                            " (should (< dt 2))))\n"))))
    (should (equal '((clock-assert my/a "should compares a clock read"))
                   (test-class-lint--violations
                    (concat cnd "(ert-deftest my/a () (should (time-less-p nil end)))\n"))))
    (should (equal '((clock-assert my/a "should compares a clock read"))
                   (test-class-lint--violations
                    (concat cnd "(ert-deftest my/a () (let ((e (time-add nil 2)))"
                            " (should (time-less-p x e))))\n"))))
    (should-not (test-class-lint--violations
                 (concat cnd "(ert-deftest my/a () (let ((n 3)) (should (< n 5))))\n")))
    ;; A wait helper that reads the clock is not an assertion on it.
    (should-not (test-class-lint--violations
                 (concat cnd "(defun my--until (f) (let ((e (+ (float-time) 5))) (while (and (not (funcall f)) (< (float-time) e)))))\n"
                         "(ert-deftest my/a () (let ((ok (my--until (lambda () t)))) (should ok) (should (my--until (lambda () (= 1 1))))))\n")))
    ;; T asserts the clock; a tag moves one test; no class line means no lint.
    (should-not (test-class-lint--violations
                 (concat ";; test-class: timing\n"
                         "(ert-deftest my/a () (sleep-for 1) (should (< (float-time) 5)))\n")))
    (should-not (test-class-lint--violations
                 (concat det "(ert-deftest my/a () :tags '(:timing) (sleep-for 1))\n")))
    (should (test-class-lint--violations
             (concat cnd "(ert-deftest my/a () :tags '(:det) (sleep-for 1))\n")))
    (should-not (test-class-lint--violations "(ert-deftest my/a () (sleep-for 1))\n"))))

(ert-deftest test-class/the-class-line-is-exact ()
  (should (eq 'det (test-class-lint--file-class ";; test-class: det\n")))
  (should (eq 'timing (test-class-lint--file-class "a\n;; test-class: timing\n")))
  (should (eq 'none (test-class-lint--file-class "a\n")))
  (should (eq 'bad (test-class-lint--file-class ";; test-class: fast\n")))
  (should (eq 'bad (test-class-lint--file-class ";; test-class: det \n")))
  (should (eq 'none (test-class-lint--file-class
                     (concat (make-string 15 ?\n) ";; test-class: det\n")))))

(ert-deftest test-class/every-test-has-a-class ()
  (let* ((exempt (test-class-lint--exempt))
         (stale (seq-remove (lambda (f)
                              (file-exists-p (expand-file-name f test-class-lint--dir)))
                            exempt))
         bad unclassified (ntests 0))
    (dolist (file (test-class-lint--test-files))
      (let* ((name (file-name-nondirectory file))
             (text (test-class-lint--read-file file))
             (class (test-class-lint--file-class text)))
        (cond ((eq class 'bad) (push name bad))
              ((and (eq class 'none) (not (member name exempt)))
               (push name unclassified)
               (cl-incf ntests
                        (seq-count (lambda (f) (eq (car-safe f) 'ert-deftest))
                                   (test-class-lint--read-forms text)))))))
    (message "test-class: %d test files without a class (%d tests): %s"
             (length unclassified) ntests (string-join (reverse unclassified) " "))
    (should-not stale)
    (should-not bad)
    (when test-class-lint--all-classified
      (should-not unclassified))))

;;; Flaky list and thread masks

(defconst test-class-lint--flaky-file
  (expand-file-name "nidhogg/test/flaky.txt" source-directory)
  "The list of tests that may fail once and pass on the rerun.")

(defun test-class-lint--flaky-names (text)
  "The test names the rows of flaky list TEXT carry.
A row is NAME # cause: the name ends at the first `#', as in check-rerun.sh."
  (let (rows)
    (dolist (line (split-string text "\n"))
      (unless (or (string-empty-p (string-trim line)) (string-prefix-p "#" line))
        (push (string-trim (car (split-string line "#"))) rows)))
    (nreverse rows)))

(defun test-class-lint--flaky-rows ()
  "The test names that rows of the flaky list carry."
  (when (file-readable-p test-class-lint--flaky-file)
    (test-class-lint--flaky-names (test-class-lint--read-file test-class-lint--flaky-file))))

(ert-deftest test-class/flaky-rows-split-where-check-rerun-does ()
  (should (equal '("a" "b" "c" "d")
                 (test-class-lint--flaky-names
                  "# comment\na # why\nb #why\nc# why\n  d  #why # more\n\n"))))

(defun test-class-lint--own-test-names ()
  "The names of the tests Nidhogg's own test files define."
  (let (names)
    (dolist (file (test-class-lint--test-files))
      (dolist (form (test-class-lint--read-forms (test-class-lint--read-file file)))
        (when (eq (car-safe form) 'ert-deftest)
          (push (symbol-name (cadr form)) names))))
    names))

(ert-deftest test-class/flaky-txt-takes-no-rows ()
  ;; A test of ours that fails once is a bug in the test or the code,
  ;; found and fixed.  The list stays for GNU's own test files, whose
  ;; tests we do not own; a row naming one of ours fails.
  (let ((own (test-class-lint--own-test-names)))
    (should-not (seq-filter (lambda (name) (member name own))
                            (test-class-lint--flaky-rows)))))

(defun test-class-lint--thread-violations (text)
  "Line numbers in C source TEXT where a thread starts with signals unblocked.
Each `pthread_create' must sit in a function that, before it, blocks or
sets a mask built by `sigfillset' (or `nh_wk_worker_sigmask') -- the fill
before the mask is set, and no other signal-mask call in between -- and
that, after it, sets the old mask back with `pthread_sigmask'."
  (let* ((lines (vconcat (split-string text "\n")))
         (n (length lines))
         (start 0) bad)
    (dotimes (i n)
      (let ((line (aref lines i)))
        (cond
         ((string-prefix-p "}" line) (setq start (1+ i)))
         ((string-match-p "\\bpthread_create *(" line)
          (let ((var nil) (mask-line nil) (filled nil) (restored nil) (end i))
            (while (and (< end n) (not (string-prefix-p "}" (aref lines end))))
              (setq end (1+ end)))
            ;; The nearest signal-mask call before the create must set the
            ;; whole mask: an unblock (or any other call) after it undoes it.
            (cl-loop for j from (1- i) downto start
                     for l = (aref lines j)
                     when (string-match-p "\\b\\(?:pthread_sigmask\\|sigprocmask\\) *(" l)
                     do (when (string-match
                               "pthread_sigmask *(SIG_\\(?:BLOCK\\|SETMASK\\), *&\\([a-z_]+\\)" l)
                          (setq var (match-string 1 l) mask-line j))
                     and return nil)
            (when var
              (setq filled
                    (cl-loop for j from start below mask-line
                             thereis (string-match-p
                                      (concat "\\(?:sigfillset\\|nh_wk_worker_sigmask\\) *(&"
                                              (regexp-quote var) ")")
                                      (aref lines j)))))
            (setq restored
                  (cl-loop for j from (1+ i) below end
                           thereis (string-match-p
                                    "pthread_sigmask *(SIG_SETMASK, *&[a-z_]+, *\\(?:NULL\\|0\\))"
                                    (aref lines j))))
            (unless (and var filled restored)
              (push (1+ i) bad)))))))
    (nreverse bad)))

(ert-deftest test-class/the-thread-lint-bites ()
  (let ((good "static void\nf (void)\n{\n  sigset_t all, old;\n  sigfillset (&all);\n  pthread_sigmask (SIG_SETMASK, &all, &old);\n  pthread_create (&t, NULL, g, NULL);\n  pthread_sigmask (SIG_SETMASK, &old, NULL);\n}\n"))
    (should-not (test-class-lint--thread-violations good))
    ;; No mask at all.
    (should (test-class-lint--thread-violations
             "void\nf (void)\n{\n  pthread_create (&t, NULL, g, NULL);\n}\n"))
    ;; The mask is set after the thread starts.
    (should (test-class-lint--thread-violations
             "void\nf (void)\n{\n  sigfillset (&all);\n  pthread_create (&t, NULL, g, NULL);\n  pthread_sigmask (SIG_SETMASK, &all, &old);\n  pthread_sigmask (SIG_SETMASK, &old, NULL);\n}\n"))
    ;; A mask that is not the full set.
    (should (test-class-lint--thread-violations
             "void\nf (void)\n{\n  sigemptyset (&all);\n  pthread_sigmask (SIG_SETMASK, &all, &old);\n  pthread_create (&t, NULL, g, NULL);\n  pthread_sigmask (SIG_SETMASK, &old, NULL);\n}\n"))
    ;; Never restored.
    (should (test-class-lint--thread-violations
             "void\nf (void)\n{\n  sigfillset (&all);\n  pthread_sigmask (SIG_SETMASK, &all, &old);\n  pthread_create (&t, NULL, g, NULL);\n}\n"))
    ;; A create with no space before its parenthesis is still a create.
    (should (test-class-lint--thread-violations
             "void\nf (void)\n{\n  pthread_create(&t, NULL, g, NULL);\n}\n"))
    ;; An unblock between the mask and the create undoes the mask.
    (should (test-class-lint--thread-violations
             "void\nf (void)\n{\n  sigfillset (&all);\n  pthread_sigmask (SIG_SETMASK, &all, &old);\n  pthread_sigmask (SIG_UNBLOCK, &all, NULL);\n  pthread_create (&t, NULL, g, NULL);\n  pthread_sigmask (SIG_SETMASK, &old, NULL);\n}\n"))
    ;; The set is filled only after the mask was taken from it.
    (should (test-class-lint--thread-violations
             "void\nf (void)\n{\n  pthread_sigmask (SIG_SETMASK, &all, &old);\n  sigfillset (&all);\n  pthread_create (&t, NULL, g, NULL);\n  pthread_sigmask (SIG_SETMASK, &old, NULL);\n}\n"))
    ;; The mask of an earlier function does not count.
    (should (test-class-lint--thread-violations
             (concat good "void\nh (void)\n{\n  pthread_create (&t, NULL, g, NULL);\n  pthread_sigmask (SIG_SETMASK, &old, NULL);\n}\n")))))

(ert-deftest test-class/every-thread-starts-with-signals-blocked ()
  ;; A thread inherits its creator's signal mask: a thread started with
  ;; signals open takes SIGCHLD, SIGALRM and the rest, and the handler
  ;; runs where Emacs does not expect it.  android.c is GNU's and exempt.
  (let ((files (directory-files (expand-file-name "src/" source-directory) t
                                "\\`nidhogg-.*\\.c\\'"))
        (creates 0) bad)
    (dolist (file files)
      (let ((text (test-class-lint--read-file file)))
        (cl-incf creates (cl-count-if (lambda (l) (string-match-p "\\bpthread_create *(" l))
                                      (split-string text "\n")))
        (dolist (line (test-class-lint--thread-violations text))
          (push (format "%s:%d" (file-name-nondirectory file) line) bad))))
    (should (> creates 0))
    (should-not bad)))

;;; The test-pattern line
;;
;; A `cond' file names how its tests keep a race out on a `;; test-pattern: handshake' line, an exact line
;; within its first 15 lines, beside the class line.  `handshake': the test parks the job or the child at a
;; named point (`nidhogg-workers--pause', `--await', `--release') and goes on from what is announced, never from a
;; sleep, a liveness poll or a timer used as a marker.  A wait that polls with a deadline for what the other side
;; announces (`accept-process-output' with a time) is how the park is awaited, so it is allowed.  The `cond' files without the line are listed in
;; nidhogg/test/pattern-pending.txt (`FILE # reason'); the list only shrinks, so every new `cond' file carries it.
;; The same file lists the handshake rule's exemptions, `exempt FILE TEST FORM # reason': TEST of FILE may call FORM (one
;; of the banned calls, or a fixed pause's `sit-for' or `sleep-for') because that call is the stimulus the test is about,
;; never a marker or a wait.  An exemption whose test no longer calls the form fails, as a stale pending row does.

(defconst test-pattern-lint--pending-file
  (expand-file-name "nidhogg/test/pattern-pending.txt" source-directory)
  "The `cond' files that have no `;; test-pattern:' line yet.")

(defconst test-pattern-lint--handshake-banned
  '(process-live-p run-at-time run-with-timer run-with-idle-timer)
  "What a handshake test never calls, besides a fixed pause.")

(defun test-pattern-lint--file-patterns (string)
  "The patterns of the `;; test-pattern: P…' line in STRING, as symbols; `bad' for a malformed line."
  (let (patterns)
    (dolist (line (seq-take (split-string string "\n") 15))
      (when (string-prefix-p ";; test-pattern:" line)
        (setq patterns
              (if (string-match "\\`;; test-pattern: \\(handshake\\)\\'" line)
                  (list (intern (match-string 1 line)))
                'bad))))
    patterns))

(defun test-pattern-lint--handshake-violations (string &optional exempt)
  "Every handshake-rule violation in the test file text STRING, as (TEST DETAIL).
EXEMPT: (TEST FORM) pairs that are not violations."
  (let* ((forms (test-class-lint--read-forms string))
         (helpers (test-class-lint--helpers forms))
         out)
    (dolist (f forms)
      (when (eq (car-safe f) 'ert-deftest)
        (pcase-let* ((`(,name ,_tags ,body) (test-class-lint--parse-test f))
                     (nodes (test-class-lint--nodes body helpers)))
          (dolist (d (append (test-class-lint--fixed-pause nodes)
                             (seq-filter (lambda (s) (test-class-lint--uses-p nodes (list s)))
                                         test-pattern-lint--handshake-banned)))
            ;; a fixed pause's detail names its call first ("sit-for above 0")
            (unless (member (list name (intern (car (split-string (format "%s" d))))) exempt)
              (push (list name (format "%s" d)) out))))))
    (nreverse out)))

(defun test-pattern-lint--pending ()
  "The files of the pending list, relative to the test directory."
  (let (rows)
    (when (file-readable-p test-pattern-lint--pending-file)
      (dolist (line (split-string (test-class-lint--read-file test-pattern-lint--pending-file) "\n"))
        (unless (or (string-empty-p (string-trim line)) (string-prefix-p "#" line) (string-prefix-p "exempt " line))
          (push (string-trim (car (split-string line " # "))) rows))))
    (nreverse rows)))

(defun test-pattern-lint--exemptions (&optional text)
  "The handshake exemptions of the pending list (or of TEXT, its contents): (FILE TEST FORM), TEST and FORM symbols."
  (let (rows)
    (dolist (line (split-string (or text (and (file-readable-p test-pattern-lint--pending-file)
                                              (test-class-lint--read-file test-pattern-lint--pending-file))
                                    "")
                                "\n"))
      (when (string-prefix-p "exempt " line)
        (pcase-let ((`(,_ ,file ,test ,form) (split-string (car (split-string line " # ")))))
          (push (list file (intern test) (intern form)) rows))))
    (nreverse rows)))

(defun test-pattern-lint--file-exemptions (name)
  "The (TEST FORM) exemptions of the file NAME."
  (mapcar #'cdr (seq-filter (lambda (e) (equal (car e) name)) (test-pattern-lint--exemptions))))

(defun test-pattern-lint--cond-files ()
  "The `cond' test files: (NAME . TEXT)."
  (let (out)
    (dolist (file (test-class-lint--test-files))
      (let ((text (test-class-lint--read-file file)))
        (when (eq (test-class-lint--file-class text) 'cond)
          (push (cons (file-name-nondirectory file) text) out))))
    (nreverse out)))

(ert-deftest test-pattern/every-cond-file-has-a-pattern-or-is-pending ()
  (let ((pending (test-pattern-lint--pending)) missing malformed)
    (pcase-dolist (`(,name . ,text) (test-pattern-lint--cond-files))
      (let ((p (test-pattern-lint--file-patterns text)))
        (cond ((eq p 'bad) (push name malformed))
              ((and (null p) (not (member name pending))) (push name missing)))))
    (should-not malformed)
    (should-not missing)))

(ert-deftest test-pattern/pending-rows-only-shrink ()
  (let ((files (test-pattern-lint--cond-files)) stale)
    (dolist (row (test-pattern-lint--pending))
      (let ((text (cdr (assoc row files))))
        (when (or (null text) (test-pattern-lint--file-patterns text))
          (push row stale))))
    (should-not stale)))

(ert-deftest test-pattern/handshake-files-park-and-announce ()
  (let (bad)
    (pcase-dolist (`(,name . ,text) (test-pattern-lint--cond-files))
      (when (memq 'handshake (ensure-list (test-pattern-lint--file-patterns text)))
        (dolist (v (test-pattern-lint--handshake-violations text (test-pattern-lint--file-exemptions name)))
          (push (format "%s: %s: %s" name (car v) (cadr v)) bad))))
    (should-not (nreverse bad))))

(ert-deftest test-pattern/exemptions-are-not-stale ()
  "Every exemption names a handshake file's test that calls its form."
  (let ((files (test-pattern-lint--cond-files)) stale)
    (pcase-dolist ((and e `(,file ,test ,form)) (test-pattern-lint--exemptions))
      (let ((text (cdr (assoc file files))))
        (unless (and text (memq 'handshake (ensure-list (test-pattern-lint--file-patterns text)))
                     (member (list (symbol-name test) (symbol-name form))
                             (mapcar (lambda (v) (list (symbol-name (car v)) (car (split-string (cadr v)))))
                                     (test-pattern-lint--handshake-violations text))))
          (push e stale))))
    (should-not stale)))

(ert-deftest test-pattern/the-pattern-lints-bite ()
  (should (equal '(handshake) (test-pattern-lint--file-patterns ";; test-pattern: handshake\n")))
  (should-not (test-pattern-lint--file-patterns "a\n"))
  (should (eq 'bad (test-pattern-lint--file-patterns ";; test-pattern: gated\n")))
  (should (eq 'bad (test-pattern-lint--file-patterns ";; test-pattern: handshake \n")))
  (should-not (test-pattern-lint--file-patterns (concat (make-string 15 ?\n) ";; test-pattern: handshake\n")))
  (should-not (test-pattern-lint--handshake-violations
               "(ert-deftest my/a () (nidhogg-workers--release 'x) (accept-process-output p 1) (sit-for 0))\n"))
  (dolist (body '("(process-live-p p)" "(sleep-for 1)" "(sit-for 1)" "(run-at-time 1 nil #'ignore)"
                  "(run-with-timer 1 nil #'ignore)"))
    (should (test-pattern-lint--handshake-violations
             (format "(ert-deftest my/a () %s)\n" body))))
  ;; a helper's call counts
  (should (test-pattern-lint--handshake-violations
           "(defun my--wait (p) (process-live-p p))\n(ert-deftest my/a () (my--wait 1))\n"))
  ;; an exemption covers its test and form only
  (let ((two "(ert-deftest my/a () (run-at-time 1 nil #'ignore) (process-live-p p))\n(ert-deftest my/b () (run-at-time 1 nil #'ignore))\n"))
    (should (equal (test-pattern-lint--handshake-violations two '((my/a run-at-time)))
                   '((my/a "process-live-p") (my/b "run-at-time")))))
  ;; a fixed pause by its call
  (should-not (test-pattern-lint--handshake-violations "(ert-deftest my/a () (sit-for 1))\n" '((my/a sit-for))))
  (should (equal (test-pattern-lint--exemptions "x.el # pending\nexempt f.el my/a run-at-time # why\n")
                 '(("f.el" my/a run-at-time)))))

(provide 'test-class-lint-tests)
;;; test-class-lint-tests.el ends here
