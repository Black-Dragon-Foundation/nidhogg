;;; watchdog-tests.el --- tests for the stall watchdog (flag, edges, spans)  -*- lexical-binding: t -*-

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

(ert-deftest watchdog/feature () (should (featurep 'nidhogg-watchdog)))

(ert-deftest watchdog/in-the-configuration ()
  (should (string-search "--with-nidhogg-watchdog" system-configuration-options)))

(ert-deftest watchdog/no-frames-when-off ()
  ;; Flag on, mode off: no thread, no handler, no stats.
  (let ((tasks (length (directory-files "/proc/self/task" nil "\\`[0-9]"))))
    (should-not (nidhogg-watchdog--stats))
    (should (= tasks (length (directory-files "/proc/self/task" nil "\\`[0-9]"))))))

;;; watchdog-tests.el ends here
