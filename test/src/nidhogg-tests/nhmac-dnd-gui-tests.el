;;; nhmac-dnd-gui-tests.el --- Naglfar's drag and drop rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).  A drag needs AppKit's own
;; dragging session; these rows wait for a synthetic one (a second window as the source or target).

(require 'ert)

(defun nhmac-dnd--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--query)))

;; Task 29: drag and drop in and out
(ert-deftest nhmac-dnd-drop-file ()
  "A file dropped on a frame arrives as a `drag-n-drop' event, exactly."
  (skip-unless (nhmac-dnd--naglfar-p))
  (ert-skip "pending: a synthetic dragging session (the drop target's code is in `macos/dnd.rs')"))

(ert-deftest nhmac-dnd-drop-text ()
  "Text dropped on a frame arrives as a `drag-n-drop' event."
  (skip-unless (nhmac-dnd--naglfar-p))
  (ert-skip "pending: a synthetic dragging session"))

(ert-deftest nhmac-dnd-drag-out ()
  "`x-begin-drag' to a test drop target in a second window; the nested loop holds `:overlapping-takes' at 0."
  (skip-unless (nhmac-dnd--naglfar-p))
  (ert-skip "pending: a synthetic dragging session"))

;;; nhmac-dnd-gui-tests.el ends here
