;;; stock-fix-tests.el --- tests for fixes to stock Emacs code  -*- lexical-binding: t -*-

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

;;; Code:

(require 'ert)
(require 'seq)

(ert-deftest nidhogg-mouse-drag-line-map-ends-on-a-fringe-and-prints-small ()
  "`mouse-drag-line''s transient map ends the drag on a release over a fringe,
and the commands it binds still print small.  The map is a prefix of itself
for each area a release may land on; each such prefix written into the map
itself multiplies the printed size of a command that holds the map (about ten
times each: with ten of them, printing the release's command, as a command
recorder or `view-lossage' prints `this-command', made 700 MB and the session
was killed for its memory).  So the prefixes reach the map through a symbol."
  (skip-unless (featurep 'glitnir))     ; the fix is Glitnir's Lisp (lisp/glitnir.el); mouse.el is upstream's
  (let ((overriding-terminal-local-map nil)
        (track-mouse nil)
        (pre-command-hook nil)
        (left (selected-window)))
    (split-window-right)
    (unwind-protect
        (progn
          (mouse-drag-line (list 'down-mouse-1 (list left 'vertical-line '(0 . 0) 0)) 'vertical)
          (let* ((map (cadr overriding-terminal-local-map))
                 (release (lookup-key map [drag-mouse-1])))
            (should (keymapp map))
            (should (functionp release))
            ;; a release over either fringe ends the drag, as one over the divider does
            (should (eq (lookup-key map [right-fringe drag-mouse-1]) release))
            (should (eq (lookup-key map [left-fringe drag-mouse-1]) release))
            ;; so do releases looked up with the other areas' prefixes
            (dolist (area '(mode-line header-line tab-line vertical-line right-divider
                            bottom-divider left-margin right-margin))
              (should (eq (lookup-key map (vector area 'drag-mouse-1)) release)))
            ;; no prefix is written into the map itself, so the release prints small
            ;; (stock Emacs's eight such prefixes print it at 150 MB)
            (should (= (seq-count (lambda (b) (and (consp b) (eq (cdr b) map))) (cdr map)) 0))
            (should (< (length (format "%s" release)) 1000000))
            (funcall release)
            (should-not overriding-terminal-local-map)))
      (setq overriding-terminal-local-map nil)
      (delete-other-windows left))))

(provide 'stock-fix-tests)

;;; stock-fix-tests.el ends here
