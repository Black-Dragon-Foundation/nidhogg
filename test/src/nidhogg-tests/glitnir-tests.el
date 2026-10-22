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

;;; glitnir-tests.el ends here
