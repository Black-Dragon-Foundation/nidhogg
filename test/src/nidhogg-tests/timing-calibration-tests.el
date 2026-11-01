;;; timing-calibration-tests.el --- the timing lane's reference workload  -*- lexical-binding: t -*-
;; test-class: timing

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

;; A fixed workload of about a second (a batch of `string-to-number' and
;; a buffer insert loop).  nidhogg/test/timing-lane.sh runs it before and
;; after the timing rows, on the same runner, and judges each row against
;; the time this takes there.  The work is fixed: never tune it to a
;; machine.

;;; Code:

(require 'ert)

(ert-deftest timing-calibration/reference ()
  (let ((n 0.0))
    (dotimes (_ 1500000)
      (setq n (+ n (string-to-number "12345.678"))))
    (with-temp-buffer
      (dotimes (i 300000)
        (insert (number-to-string i) "\n"))
      (should (= (count-lines (point-min) (point-max)) 300000)))
    (should (> n 0.0))))

(provide 'timing-calibration-tests)

;;; timing-calibration-tests.el ends here
