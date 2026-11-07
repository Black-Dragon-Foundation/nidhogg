;;; sja-tests.el --- tests for Sjá's Lisp door  -*- lexical-binding: t -*-
;; test-class: cond

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

(defmacro sja-tests--with (view h &rest body)
  "Run BODY with H an instance of VIEW holding the small map; close H after."
  (declare (indent 2))
  `(let ((,h (nidhogg-view-open ,view)))
     (unwind-protect
         (progn
           (should (eq (nidhogg-view-apply ,view "h1" 1 sja-tests--map) 'applied))
           ,@body)
       (nidhogg-view-close ,h))))

(ert-deftest sja/door-is-there ()
  (should (fboundp 'nidhogg-view--call))
  (should (subrp (symbol-function 'nidhogg-view--call))))

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
    (should (eq (nidhogg-view-apply "t-follow" "h1" 1 sja-tests--map) 'dropped))
    (should (eq (nidhogg-view-apply "t-follow" "h1" 2 "{\"ops\": [{\"remove\": 3}]}") 'resync))))

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
          (should (eq (nidhogg-view-apply "t-panic" "h1" 1 sja-tests--map) 'applied))
          (nidhogg-view-set h 'select "agent:b")
          (should (eql (nidhogg-view--call 99 "t-panic") 2))
          (should (eql (nidhogg-view-get h 'health) 1))
          (should (eq (nidhogg-view-apply "t-panic" "h1" 2 sja-tests--map) 'applied))
          (should (equal (nidhogg-view-get h 'selection) "agent:b"))
          (should (eql (nidhogg-view--call 99 "t-panic") 4))
          (should (eq (nidhogg-view-apply "t-panic" "h1" 3 sja-tests--map) 'broken))
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

(provide 'sja-tests)

;;; sja-tests.el ends here
