;;; nidhogg-watchdog.el --- Nidhogg's stall watchdog: the mode, the drain and the log  -*- lexical-binding: t -*-

;; Copyright (C) 2026 Black Dragon Foundation contributors.

;; This file is part of Nidhogg.

;; Nidhogg is free software: you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or (at
;; your option) any later version.

;; Nidhogg is distributed in the hope that it will be useful,
;; but WITHOUT ANY WARRANTY; without even the implied warranty of
;; MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
;; GNU General Public License for more details.

;; You should have received a copy of the GNU General Public License
;; along with Nidhogg.  If not, see <https://www.gnu.org/licenses/>.

;;; Commentary:

;; Preloaded when Nidhogg is built with --with-nidhogg-watchdog (loadup.el's one
;; `(featurep 'nidhogg-watchdog)' line).  The C side provides the feature
;; `nidhogg-watchdog'; this file provides `nidhogg-watchdog-lisp'.  Stub (Task 4):
;; the mode, drain and log arrive in Task 14, hang files in Task 18.

;;; Code:

(require 'nidhogg-watchdog-resolve)
(require 'nidhogg-watchdog-gc)
(require 'nidhogg-watchdog-report)

(provide 'nidhogg-watchdog-lisp)

;;; nidhogg-watchdog.el ends here
