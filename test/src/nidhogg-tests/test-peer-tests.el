;;; test-peer-tests.el --- tests for the scripted peers  -*- lexical-binding: t -*-

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

;; `nidhogg-test-peer': one write is one filter call, whatever the OS
;; does with the bytes.

;;; Code:

(require 'ert)
(require 'nidhogg-test-peer)

(defmacro test-peer-tests--modes (&rest body)
  "Run BODY in both worker modes, when the workers exist."
  (declare (indent 0) (debug t))
  `(dolist (mode (if (boundp 'nidhogg-workers--mode) '(threads inline) '(nil)))
     (let ((nidhogg-workers--mode mode))
       (ert-info ((format "mode %s" mode) :prefix "")
         ,@body))))

(ert-deftest test-peer/one-write-one-filter-call ()
  (test-peer-tests--modes
    (let* ((got nil)
           (peer (nidhogg-test-peer-make
                  :name "tp" :filter (lambda (_p s) (push s got)))))
      (unwind-protect
          (progn
            (nidhogg-test-peer-write peer "x")
            (should (equal got '("x")))
            (setq got nil)
            (let ((big (make-string 65536 ?y)))
              (nidhogg-test-peer-write peer big)
              (should (equal got (list big))))
            (setq got nil)
            (nidhogg-test-peer-write peer "one")
            (nidhogg-test-peer-write peer "two")
            (should (equal (reverse got) '("one" "two")))
            (should (= (nidhogg-test-peer-calls peer) 4)))
        (nidhogg-test-peer-close peer)))))

(ert-deftest test-peer/multibyte-stays-whole ()
  (let* ((got nil)
         (peer (nidhogg-test-peer-make
                :name "tp-mb" :filter (lambda (_p s) (push s got)))))
    (unwind-protect
        (progn
          (nidhogg-test-peer-write peer "héllo 世")
          (should (equal got '("héllo 世"))))
      (nidhogg-test-peer-close peer))))

(provide 'test-peer-tests)

;;; test-peer-tests.el ends here
