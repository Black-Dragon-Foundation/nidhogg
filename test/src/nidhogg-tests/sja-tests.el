;;; sja-tests.el --- tests for Sjá's Lisp door  -*- lexical-binding: t -*-
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

;; Every test is pattern D (pure): Sjá answers synchronously, with no clock
;; and no thread; each test uses its own view names and closes its handles.

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'nidhogg-views)

(defconst sja-tests--map
  (concat
   "{\"ops\": [{\"reset\": {\"descriptor\": {\"scheme\": \"links\", \"chain_kinds\": [\"came-from\"],"
   " \"match_fields\": [\"label\"], \"lists\": [\"team\"], \"classes\": [\"selected\", \"chain\", \"dim\"],"
   " \"start\": \"team\"},"
   " \"nodes\": ["
   "{\"id\": \"plan:1\", \"kind\": \"plan\", \"nav\": {\"down\": \"agent:a\"}, \"fields\": [[\"label\", \"Plän ✓\"]]},"
   "{\"id\": \"agent:a\", \"kind\": \"agent\", \"nav\": {\"up\": \"plan:1\", \"right\": \"agent:b\"},"
   " \"fields\": [[\"label\", \"A\"], [\"minutes\", 7]]},"
   "{\"id\": \"agent:b\", \"kind\": \"agent\", \"nav\": {\"left\": \"agent:a\"}}],"
   " \"edges\": [[\"plan:1\", \"agent:a\", \"came-from\"]],"
   " \"lists\": [{\"name\": \"team\", \"ids\": [\"agent:a\", \"agent:b\"]}]}}]}")
  "A small links model: a plan and two agents, one came from the plan.")

(defun sja-tests--line (view base version reset model)
  "A hub's `view-model' line for VIEW: MODEL (a JSON object's text) from BASE to VERSION."
  (format "{\"event\":\"view-model\",\"data\":{\"base\":%d,\"hub\":7,\"model\":%s,%s\"version\":%d,\"view\":%S}}"
          base model (if reset "\"reset\":true," "") version view))

(defun sja-tests--put (view version model &optional reset)
  "MODEL (a JSON object's text) for VIEW from VERSION - 1 to VERSION, pushed as the hub's line and drained.
RESET marks it a reset.  Return the drain's status, or the push's decision when nothing was queued."
  (let ((d (nth 2 (nidhogg-view-push (sja-tests--line view (1- version) version reset model)))))
    (if (memq d '(queue reset)) (nth 1 (nidhogg-view-drain view)) d)))

(defmacro sja-tests--with (view h &rest body)
  "Run BODY with H an instance of VIEW holding the small map; close H after."
  (declare (indent 2))
  `(let ((,h (nidhogg-view-open ,view)))
     (unwind-protect
         (progn
           (should (eq (sja-tests--put ,view 1 sja-tests--map t) 'applied))
           ,@body)
       (nidhogg-view-close ,h))))

(ert-deftest sja/door-is-there ()
  (should (fboundp 'nidhogg-view--call))
  (should (subrp (symbol-function 'nidhogg-view--call))))

(ert-deftest sja/the-string-apply-is-gone ()
  "A model comes only as the hub's line (`nidhogg-view-push'): op 1, the string apply, is refused."
  (should-not (fboundp 'nidhogg-view-apply))
  (should-error (nidhogg-view--call 1 "t-gone" "h1" 1 "{\"ops\": []}") :type 'args-out-of-range))

(ert-deftest sja/follow-off-a-window-answers-more ()
  "A follow that stays at the edge of a windowed list answers (ID . more) and sets `nidhogg-view-more';
`window' tells the list's (FROM LENGTH TOTAL), nil for a whole list."
  (let ((h (nidhogg-view-open "t-window")))
    (unwind-protect
        (progn
          (should (eq (sja-tests--put
                       "t-window" 1
                       (concat "{\"ops\": [{\"reset\": {\"descriptor\": {\"scheme\": \"links\", \"lists\": [\"steps\"],"
                               " \"classes\": [\"selected\"], \"start\": \"steps\"},"
                               " \"nodes\": [{\"id\": \"s3\", \"kind\": \"step\", \"nav\": {\"down\": \"s4\"}},"
                               " {\"id\": \"s4\", \"kind\": \"step\", \"nav\": {\"up\": \"s3\"}}], \"edges\": [],"
                               " \"lists\": [{\"name\": \"steps\", \"ids\": [\"s3\", \"s4\"], \"total\": 9, \"from\": 3}]}}]}")
                       t)
                      'applied))
          (should (equal (nidhogg-view-get h 'window "steps") '(3 2 9)))
          (nidhogg-view-set h 'select "s3")
          (setq nidhogg-view-more nil)
          (should (equal (nidhogg-view-follow h "up") '("s3" . more)))
          (should (eql nidhogg-view-more -1))
          (should (equal (nidhogg-view-follow h "down") "s4"))
          (should (equal (nidhogg-view-follow h "down") '("s4" . more)))
          (should (eql nidhogg-view-more 1)))
      (nidhogg-view-close h)))
  (sja-tests--with "t-window-whole" h
    (should-not (nidhogg-view-get h 'window "team"))))

(ert-deftest sja/apply-then-follow ()
  (sja-tests--with "t-follow" h
    (should (equal (nidhogg-view-follow h "right") "agent:a"))
    (should (equal (nidhogg-view-follow h "right") "agent:b"))
    (should (equal (nidhogg-view-follow h "left") "agent:a"))
    (should (equal (nidhogg-view-follow h "up") "plan:1"))
    (should (equal (nidhogg-view-walk h "team" 1) "agent:a"))
    (should (equal (nidhogg-view-get h 'kind) "agent"))
    (should (eql (nidhogg-view-get h 'field "minutes") 7))
    (should (equal (nidhogg-view-set h 'select nil) nil))
    (should (eq (sja-tests--put "t-follow" 1 "{\"ops\": []}") 'drop))           ; a diff not newer
    (should (eq (sja-tests--put "t-follow" 2 "{\"ops\": [{\"remove\": 3}]}") 'resync))))

(ert-deftest sja/classes-are-face-indices ()
  (sja-tests--with "t-classes" h
    (should (equal (nidhogg-view-get h 'class-names) '("selected" "chain" "dim")))
    (nidhogg-view-set h 'select "agent:a")
    (should (equal (nidhogg-view-get h 'classes) ["agent:a" 0 "agent:b" 2 "plan:1" 1]))
    (should (equal (nidhogg-view-get h 'classes) []))
    (nidhogg-view-set h 'select nil)
    (should (equal (nidhogg-view-get h 'classes) ["agent:a" nil "agent:b" nil "plan:1" nil]))))

(ert-deftest sja/bad-args-signal ()
  (should-error (nidhogg-view--call 12345) :type 'args-out-of-range)
  (should-error (nidhogg-view--call 4 1.5) :type 'args-out-of-range)
  (should-error (nidhogg-view-follow 987654 "up") :type 'args-out-of-range)
  (should-error (nidhogg-view--call 'apply) :type 'wrong-type-argument)
  (should-error (apply #'nidhogg-view--call 10 (make-list 17 nil)) :type 'args-out-of-range)
  ;; a unibyte string that is not UTF-8
  (should-error (nidhogg-view-open (unibyte-string #xff #xfe)) :type 'args-out-of-range))

(ert-deftest sja/answer-strings-are-multibyte ()
  (sja-tests--with "t-strings" h
    (let ((label (nidhogg-view-get h 'field "label" "plan:1")))
      (should (equal label "Plän ✓"))
      (should (multibyte-string-p label)))))

(defun sja-tests--test-hooks-p ()
  "Whether this build's Sjá has its test hooks (an --enable-checking Wayland build)."
  (condition-case nil (progn (nidhogg-view--call 99 "t-probe-hooks") t)
    (args-out-of-range nil)))

(ert-deftest sja/panic-answers-resync-then-broken ()
  (skip-unless (sja-tests--test-hooks-p))
  (let ((h (nidhogg-view-open "t-panic")))
    (unwind-protect
        (progn
          (should (eq (sja-tests--put "t-panic" 1 sja-tests--map t) 'applied))
          (nidhogg-view-set h 'select "agent:b")
          (should (eql (nidhogg-view--call 99 "t-panic") 2))
          (should (eql (nidhogg-view-get h 'health) 1))
          (should (eq (sja-tests--put "t-panic" 2 sja-tests--map t) 'applied))
          (should (equal (nidhogg-view-get h 'selection) "agent:b"))
          (should (eql (nidhogg-view--call 99 "t-panic") 4))
          (should (eq (sja-tests--put "t-panic" 3 sja-tests--map t) 'drop))       ; a broken view drops its lines
          (should (eql (nidhogg-view-get h 'health) 2))
          (should-not (nidhogg-view-follow h "up")))
      (nidhogg-view-close h))
    ;; reopened, the view is whole again
    (let ((h (nidhogg-view-open "t-panic")))
      (unwind-protect (should (eql (nidhogg-view-get h 'health) 1))
        (nidhogg-view-close h)))))

(ert-deftest sja/no-lisp-allocation-beyond-the-answer ()
  (sja-tests--with "t-alloc" h
    (nidhogg-view-set h 'select "agent:a")
    (nidhogg-view-follow h "right")
    ;; the loop compiled: the test file runs interpreted, and the interpreter's own conses are not the door's
    (let* ((moves (byte-compile `(lambda ()
                                   (dotimes (_ 100)
                                     (nidhogg-view-follow ,h "left") (nidhogg-view-follow ,h "right")))))
           (before (memory-use-counts))
           (_ (funcall moves))
           (after (memory-use-counts))
           (conses (- (nth 0 after) (nth 0 before)))
           (strings (- (nth 6 after) (nth 6 before))))
      ;; 200 moves: one id string each, and no conses beyond the loop's own
      (should (<= strings 200))
      (should (< conses 50)))))

;; drgnx's perf row `view-select-dim' (2026-11-13): 6,400 rows dimmed and undimmed a key, each answer 6,400 fresh id
;; strings, about 460 KB a key, enough for a 40 ms GC to land on a keystroke
(defun sja-tests--many (n)
  "A tree model of N nodes n0.., labelled by their ids, with `dim' among its classes."
  (concat
   "{\"ops\": [{\"reset\": {\"descriptor\": {\"scheme\": \"tree\", \"lists\": [\"order\"],"
   " \"match_fields\": [\"label\"], \"classes\": [\"selected\", \"match\", \"dim\"], \"start\": \"order\"},"
   " \"nodes\": ["
   (mapconcat (lambda (i) (format "{\"id\": \"n%d\", \"kind\": \"var\", \"fields\": [[\"label\", \"n%d\"]]}" i i))
              (number-sequence 0 (1- n)) ",")
   "], \"edges\": [], \"lists\": [{\"name\": \"order\", \"ids\": ["
   (mapconcat (lambda (i) (format "\"n%d\"" i)) (number-sequence 0 (1- n)) ",")
   "]}]}}]}"))

(ert-deftest sja/the-same-ids-come-back-as-the-same-strings ()
  (let ((h (nidhogg-view-open "t-ids")))
    (unwind-protect
        (progn
          (should (eq (sja-tests--put "t-ids" 1 (sja-tests--many 200) t) 'applied))
          (nidhogg-view-set h 'search "n0")
          (let ((first (nidhogg-view-get h 'classes)))
            (should (> (length first) 300))         ; the premise: most nodes changed
            (nidhogg-view-set h 'search nil)
            (let ((again (nidhogg-view-get h 'classes)))
              (should (= (length first) (length again)))
              ;; the same ids (in any order): the same strings
              (let ((seen (make-hash-table :test #'equal)))
                (mapc (lambda (x) (when (stringp x) (puthash x x seen))) first)
                (should (cl-every (lambda (x) (or (not (stringp x)) (eq x (gethash x seen)))) again)))))
          (let* ((keys (byte-compile `(lambda ()
                                        (dotimes (_ 50)
                                          (nidhogg-view-set ,h 'search "n0") (nidhogg-view-get ,h 'classes)
                                          (nidhogg-view-set ,h 'search nil) (nidhogg-view-get ,h 'classes)))))
                 (before (memory-use-counts))
                 (_ (funcall keys))
                 (strings (- (nth 6 (memory-use-counts)) (nth 6 before))))
            ;; 100 answers of ~200 ids each: no string per id
            (should (< strings 100))))
      (nidhogg-view-close h))))

(ert-deftest sja/an-id-lisp-changed-is-not-given-again ()
  (sja-tests--with "t-ids-changed" h
    (nidhogg-view-set h 'select "agent:a")
    (let ((id (nidhogg-view-get h 'selection)))
      (put-text-property 0 1 'face 'bold id)
      (should (equal (nidhogg-view-get h 'selection) "agent:a"))
      (should-not (text-properties-at 0 (nidhogg-view-get h 'selection)))
      (aset id 0 ?b)
      (should (equal (nidhogg-view-get h 'selection) "agent:a")))))

(defconst sja-tests--tree
  (concat
   "{\"ops\": [{\"reset\": {\"descriptor\": {\"scheme\": \"tree\", \"lists\": [\"order\"],"
   " \"classes\": [\"selected\"], \"flags\": [\"expanded\"], \"start\": \"order\"},"
   " \"nodes\": ["
   "{\"id\": \"a\", \"kind\": \"dir\", \"flags\": 1, \"fields\": [[\"label\", \"alpha\"]]},"
   "{\"id\": \"a/x\", \"kind\": \"file\", \"parent\": \"a\", \"fields\": [[\"label\", \"xray\"]]},"
   "{\"id\": \"b\", \"kind\": \"dir\", \"fields\": [[\"label\", \"bravo\"]]},"
   "{\"id\": \"b/z\", \"kind\": \"file\", \"parent\": \"b\", \"fields\": [[\"label\", \"zulu\"]]}],"
   " \"edges\": [],"
   " \"lists\": [{\"name\": \"order\", \"ids\": [\"a\", \"a/x\", \"b\", \"b/z\"]}]}}]}")
  "A small tree: a/ open over x, b/ closed over z.")

(ert-deftest sja/tree-through-the-door ()
  "The tree scheme's verbs through the door: the visible list, a fold, `open', `prefix' and a move."
  (let ((h (nidhogg-view-open "t-tree")))
    (unwind-protect
        (progn
          (should (eq (sja-tests--put "t-tree" 1 sja-tests--tree t) 'applied))
          (should (equal (nidhogg-view-get h 'list "visible") '("a" "a/x" "b")))
          (should (eql (nidhogg-view-get h 'open "a") 1))
          (nidhogg-view-set h 'fold "b")
          (should (eql (nidhogg-view-get h 'open "b") 1))
          (should (equal (nidhogg-view-get h 'list "visible") '("a" "a/x" "b" "b/z")))
          (should (equal (nidhogg-view-set h 'prefix "B") "b"))
          (should (equal (nidhogg-view-follow h "right") "b/z"))
          (should (equal (nidhogg-view-follow h "up") "b")))
      (nidhogg-view-close h))))

(ert-deftest sja/push-a-line-then-drain ()
  "A whole `view-model' line goes to Sjá unparsed (a unibyte string, as nh-io delivers it); it waits for the draw."
  (let ((h (nidhogg-view-open "t-push")))
    (unwind-protect
        (progn
          (should (equal (nidhogg-view-push (encode-coding-string
                                             (sja-tests--line "t-push" 0 1 t sja-tests--map) 'utf-8))
                         '("t-push" 1 reset)))
          (should (equal (nidhogg-view-push (sja-tests--line "t-push" 1 2 nil
                                                             (concat "{\"ops\": [{\"list\": {\"name\": \"team\","
                                                                     " \"ids\": [\"agent:a\"]}}]}")))
                         '("t-push" 2 queue)))
          (should (equal (nidhogg-view-push (sja-tests--line "t-push" 5 6 nil "{\"ops\": []}"))
                         '("t-push" 6 resync)))
          (should (eql (nidhogg-view-get h 'version) 0))
          (nidhogg-view-named "t-push" 1)
          (should-not (nidhogg-view-due "t-push"))
          (should (equal (nidhogg-view-drain "t-push" 1) '(1 applied 2)))
          (should (equal (nidhogg-view-get h 'list "team") '("agent:a" "agent:b")))
          (should (equal (nidhogg-view-drain "t-push") '(1 applied 2)))
          (should (equal (nidhogg-view-get h 'list "team") '("agent:a")))
          (should (equal (nidhogg-view-push (sja-tests--line "t-nobody" 0 1 t sja-tests--map))
                         '("t-nobody" 1 none)))
          (should-not (nidhogg-view-push "{\"event\":\"view-patch\",\"data\":{}}")))
      (nidhogg-view-close h))))

(defun sja-tests--rows-line (view base version ops)
  "A hub's `view-model' line carrying a row patch of VIEW from BASE to VERSION with OPS (JSON text)."
  (format (concat "{\"event\":\"view-model\",\"data\":{\"view\":%S,\"rows\":{\"view\":%S,\"class\":\"c\","
                  "\"hub\":5,\"base\":%d,\"version\":%d,\"env_epoch\":0,\"ops\":%s}}}")
          view view base version ops))

(defun sja-tests--draw-rows (h)
  "Draw H's rows into the current buffer, which is the whole region, and confirm the batch."
  (let ((d (nidhogg-view-rows h 'draw (- (point-max) (point-min)))))
    (seq-doseq (e (nth 2 d))
      (goto-char (+ (point-min) (nth 0 e)))
      (delete-char (nth 1 e))
      (insert (nth 2 e)))
    (nidhogg-view-rows h 'drawn (nth 0 d))))

(ert-deftest sja/rows-through-the-door ()
  "A row patch on a `view-model' line goes to the view's row store; a draw's edits turn the buffer into the rows,
touching only the rows that changed."
  (let ((h (nidhogg-view-open "t-rows")))
    (unwind-protect
        (with-temp-buffer
          ;; no store: the line is not this view's to draw
          (should-not (nidhogg-view-push (sja-tests--rows-line "t-rows" 0 1 "[]")))
          (nidhogg-view-rows h 'on 1)
          (should (equal (nidhogg-view-push
                          (sja-tests--rows-line
                           "t-rows" 0 1
                           "[{\"reset\": [{\"key\": \"a\", \"text\": \"ä\"}, {\"key\": \"b\", \"text\": \"b\"}]}]"))
                         '("t-rows" 1 apply "c")))
          (should (eql (sja-tests--draw-rows h) 1))
          (should (equal (buffer-string) "ä\nb\n"))
          (should (equal (nidhogg-view-rows h 'place 2) '("b" . 0)))
          (put-text-property 1 2 'sja-tests-mark t)
          (should (equal (nidhogg-view-push
                          (sja-tests--rows-line
                           "t-rows" 1 2 "[{\"replace\": [{\"key\": \"b\", \"text\": \"bee\"}]}]"))
                         '("t-rows" 2 apply "c")))
          (sja-tests--draw-rows h)
          (should (equal (buffer-string) "ä\nbee\n"))
          (should (get-text-property 1 'sja-tests-mark))
          ;; a gap asks for a resync, once
          (should (equal (nth 2 (nidhogg-view-push (sja-tests--rows-line "t-rows" 5 6 "[]"))) 'resync))
          (should (equal (nth 2 (nidhogg-view-push (sja-tests--rows-line "t-rows" 7 8 "[]"))) 'quiet)))
      (nidhogg-view-close h))))

(ert-deftest sja/a-bar-is-laid-out ()
  "A bar's segments that fit and its branch's cut, through the door: eol and lsp go, the branch keeps 9 characters."
  (should (equal (nidhogg-view-bar
                  (concat "1 200 8 4 2 2 6 56 0 2 3  -1 100 1 12 11 8 8 8 8 8 8 8 8 8 8 8  -1 30 0"
                          "  3 40 0  -1 50 0  0 20 0"))
                 '((0 1) (1) (0 9 1 90) 50)))
  (should-error (nidhogg-view-bar "2 0") :type 'args-out-of-range)
  ;; a title row's search box, centred between the runs
  (should (equal (nidhogg-view-bar "2 800 0 0 0 0 0 0 0 2 1  -1 60 0  -1 40 0  -1 90 0  200 10 150")
                 '((0 1) (0) nil 90 (1 175 235)))))

(ert-deftest sja/a-local-list-is-picked ()
  "A list loaded once, picked from by input through the door: the recent
command first, then the other, then the file; a closed list is refused."
  (let ((h (nidhogg-view-pick 'load (concat "0\t-1\tView: Split Editor Down\n"
                                            "0\t0\tWindow: Split Right\n"
                                            "1\t-1\tFile: split.rs\n"))))
    (should (equal (nidhogg-view-pick 'top h "split" 10) '(1 0 2)))
    (should (equal (nidhogg-view-pick 'top h "split down" 10) '(0)))
    (nidhogg-view-pick 'close h)
    (should-error (nidhogg-view-pick 'top h "" 1) :type 'args-out-of-range)))

(provide 'sja-tests)

;;; sja-tests.el ends here
