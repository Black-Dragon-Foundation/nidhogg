;;; nidhogg-workers-treesit.el --- Nidhogg's worker threads: tree-sitter wrappers  -*- lexical-binding: t -*-

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

;; Loaded after `treesit' (nidhogg-workers.el).  The create start point
;; of tree-sitter parse jobs: when a major mode has finished, a job
;; parses each parser it created, so the first parse of a large file
;; overlaps the rest of `find-file'.  Task 13 adds the fontify-region
;; and pre-redisplay wrappers.

;;; Code:

(defvar nidhogg-workers-treesit)
(defvar nidhogg--redisplay-fontifying)
(declare-function nidhogg-workers--treesit-start-created "nidhogg-treesit-bg.c" ())

(defun nidhogg-workers-treesit--start-created ()
  "Start a parse job for each parser of this buffer that never parsed.
On `after-change-major-mode-hook'.  Nothing with `nidhogg-workers-treesit'
off."
  (when (and (bound-and-true-p nidhogg-workers-treesit)
             (fboundp 'nidhogg-workers--treesit-start-created))
    (nidhogg-workers--treesit-start-created)))

(add-hook 'after-change-major-mode-hook
          #'nidhogg-workers-treesit--start-created)

(defmacro nidhogg-workers--test-fontifying (buffer &rest body)
  "Run BODY as if redisplay were fontifying BUFFER; for tests.
Binds `nidhogg--redisplay-fontifying' to BUFFER, as redisplay does
around `fontification-functions'."
  (declare (indent 1) (debug t))
  `(let ((nidhogg--redisplay-fontifying ,buffer))
     ,@body))

(provide 'nidhogg-workers-treesit)

;;; nidhogg-workers-treesit.el ends here
