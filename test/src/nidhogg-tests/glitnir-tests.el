;;; glitnir-tests.el --- tests for Glitnir (the compositor)  -*- lexical-binding: t -*-

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
(require 'loadhist)

(ert-deftest glitnir/feature ()
  (should (featurep 'glitnir)))

(ert-deftest glitnir/inactive-in-batch ()
  ;; The batch frame's type is t, like a tty frame.
  (should (eq (framep (selected-frame)) t))
  (should-not (glitnir-active-p))
  (should (memq glitnir-enable '(t nil))))

(ert-deftest glitnir/in-the-configuration ()
  (should (string-search "--with-glitnir" system-configuration-options)))

(ert-deftest glitnir/lisp-preloaded-compiled ()
  ;; The dump loads glitnir.el compiled, like the rest of the preloaded
  ;; Lisp, not from source.
  (let ((file (feature-file 'glitnir-lisp)))
    (should file)
    (should-not (string-suffix-p ".el" file))))


;; The resolver (Task 33).  The batch frame is a root tty frame; a child frame is stood in for by
;; mocking the two things the resolver asks of it.

(require 'cl-lib)

(defmacro glitnir-test--as-child (params &rest body)
  "Run BODY with the selected frame an undecorated child frame with parameters PARAMS."
  (declare (indent 1))
  `(cl-letf (((symbol-function 'frame-parent) (lambda (&optional _f) (selected-frame)))
             ((symbol-function 'frame-parameters)
              (lambda (&optional _f) (append ,params '((undecorated . t))))))
     ,@body))

(ert-deftest glitnir/resolve-merges-the-option ()
  (let ((child-frame-chrome '((corner-radius . 8) (shadow . (:blur 16 :offset (0 . 6) :opacity 0.5)))))
    (glitnir-test--as-child nil
      (let ((r (glitnir--resolve (selected-frame))))
        (should (= (plist-get r :radius) 8))
        (let ((s (plist-get r :shadow)))
          (should (= (plist-get s :blur) 16))
          (should (equal (plist-get s :offset) '(0 . 6)))
          (should (= (plist-get s :spread) 0))
          (should (= (plist-get s :opacity) 0.5))
          (should (= (length (plist-get s :color)) 3)))))))

(ert-deftest glitnir/frame-parameter-wins ()
  (let ((child-frame-chrome '((corner-radius . 8) (shadow . (:blur 16)))))
    (glitnir-test--as-child '((corner-radius . 3) (shadow . (:blur 4)))
      (let ((r (glitnir--resolve (selected-frame))))
        (should (= (plist-get r :radius) 3))
        (should (= (plist-get (plist-get r :shadow) :blur) 4))))))

(ert-deftest glitnir/zero-radius-opts-out ()
  (let ((child-frame-chrome '((corner-radius . 8) (shadow . (:blur 16)))))
    (glitnir-test--as-child '((corner-radius . 0) (shadow . nil))
      (should-not (glitnir--resolve (selected-frame))))
    ;; the radius alone opted out keeps the shadow
    (glitnir-test--as-child '((corner-radius . 0))
      (let ((r (glitnir--resolve (selected-frame))))
        (should (= (plist-get r :radius) 0))
        (should (plist-get r :shadow))))))

(ert-deftest glitnir/function-option ()
  (let ((child-frame-chrome (lambda (f) (should (framep f)) '((corner-radius . 5)))))
    (glitnir-test--as-child nil
      (let ((r (glitnir--resolve (selected-frame))))
        (should (= (plist-get r :radius) 5))
        (should-not (plist-get r :shadow))))))

(ert-deftest glitnir/bad-shadow-plist-signals ()
  (glitnir-test--as-child '((shadow . (:blurr 4)))
    (should-error (glitnir--resolve (selected-frame)) :type 'wrong-type-argument))
  (glitnir-test--as-child '((shadow . (:blur "x")))
    (should-error (glitnir--resolve (selected-frame)) :type 'wrong-type-argument))
  (glitnir-test--as-child '((corner-radius . "x"))
    (should-error (glitnir--resolve (selected-frame)) :type 'wrong-type-argument)))

(ert-deftest glitnir/root-frames-get-none ()
  (let ((child-frame-chrome '((corner-radius . 8) (shadow . (:blur 16)))))
    (should-not (frame-parent))
    (should-not (glitnir--resolve (selected-frame)))))

(ert-deftest glitnir/decorated-children-get-none ()
  ;; Design §3.4: a decorated child frame is a plain one, its own chrome parameters included.
  (let ((child-frame-chrome '((corner-radius . 8) (shadow . (:blur 16)))))
    (glitnir-test--as-child '((undecorated . nil) (corner-radius . 4))
      (should-not (glitnir--resolve (selected-frame))))))

(ert-deftest glitnir/function-option-may-modify-parameters ()
  ;; A function option that sets parameters itself must not recurse into the resolver for good: the C side
  ;; guards that; here the resolver is simply called again from inside, and both calls return.
  (let* ((depth 0) (max-depth 0)
         (child-frame-chrome (lambda (f)
                               (setq depth (1+ depth) max-depth (max depth max-depth))
                               (unwind-protect
                                   (progn (when (= depth 1) (glitnir-refresh f))
                                          '((corner-radius . 6)))
                                 (setq depth (1- depth))))))
    (glitnir-test--as-child nil
      (should (= (plist-get (glitnir--resolve (selected-frame)) :radius) 6))
      (should (<= max-depth 2)))))

(ert-deftest glitnir/probe-rejects-short-bytes ()
  ;; RAW shorter than 4*W*H bytes is refused before anything reads it.
  (skip-unless (fboundp 'glitnir--probe))
  (should-error (glitnir--probe 'edges (list 4 4 (make-string 63 0)) '(0 0 4 4))
                :type 'args-out-of-range)
  (should-error (glitnir--probe 'edges (list 4 4 (make-string 64 0)) '(0 0 5 4))
                :type 'args-out-of-range)
  (should-error (glitnir--probe 'blur (list 4 4 (make-string 64 0)) '(0 0 4 4))))

;;; glitnir-tests.el ends here
