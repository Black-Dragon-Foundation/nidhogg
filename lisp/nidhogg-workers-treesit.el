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
;; overlaps the rest of `find-file'.
;;
;; Deferral at redisplay (`nidhogg-treesit-defer'): while redisplay
;; fontifies a buffer whose parse runs on a worker, the region keeps its
;; old faces and is recorded; when the parse lands, the recorded regions
;; are marked unfontified and redisplay colours them with the new tree.
;; Two buffer-local wrappers, installed after `treesit-major-mode-setup':
;; one around `font-lock-fontify-region-function', one in place of
;; `treesit--pre-redisplay' in `pre-redisplay-functions'.  Only those
;; defer: `font-lock-ensure', `syntax-ppss', outline search, display code
;; called from Lisp, indentation, navigation and every node function
;; still end with a tree.

;;; Code:

(require 'treesit)

(defvar nidhogg-workers-treesit)
(defvar nidhogg-treesit-defer)
(defvar nidhogg--redisplay-fontifying)
(declare-function nidhogg-workers--treesit-start-created "nidhogg-treesit-bg.c" ())
(declare-function nidhogg-workers--treesit-await "nidhogg-treesit-bg.c"
                  (parser &optional count))
(declare-function nidhogg-workers--treesit-busy-p "nidhogg-treesit-bg.c"
                  (buffer))
(declare-function nidhogg-treesit-parser-ready-p "nidhogg-treesit-bg.c"
                  (parser))

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

;;; Deferral at redisplay.

(defvar-local nidhogg-workers-treesit--pending nil
  "The regions whose colour redisplay deferred: (BEG . END) markers.")

(defvar nidhogg-workers-treesit--pending-buffers nil
  "The buffers with deferred regions.")

(defvar nidhogg-workers-treesit--hook-entry nil
  "Non-nil while the pre-redisplay wrapper calls `treesit--pre-redisplay'.")

(defun nidhogg-workers-treesit--on ()
  "Non-nil when redisplay may defer tree-sitter colour."
  (and (bound-and-true-p nidhogg-workers-treesit)
       (bound-and-true-p nidhogg-treesit-defer)
       (fboundp 'nidhogg-workers--treesit-await)))

(defun nidhogg-workers-treesit--only-hook-defers (orig &rest args)
  "Call ORIG, `treesit--pre-redisplay', with ARGS; only the hook entry defers.
`syntax-ppss' (through `treesit--pre-syntax-ppss') and
`treesit-outline-search' call the function itself: there the demand
ends with a tree, even while redisplay fontifies the buffer."
  (if nidhogg-workers-treesit--hook-entry
      (let ((nidhogg-workers-treesit--hook-entry nil))
        (apply orig args))
    (let ((nidhogg--redisplay-fontifying nil))
      (apply orig args))))

(advice-add 'treesit--pre-redisplay :around
            #'nidhogg-workers-treesit--only-hook-defers)

(defun nidhogg-workers-treesit--demand-primary (args)
  "Call `treesit--pre-redisplay' with ARGS as redisplay's demand.
Its demand of the primary parser may parse for 1 ms, then hand the parse
to a job and answer \"not ready\".  Then wait for that job within the
redisplay's budget: in time, call `treesit--pre-redisplay' again, which
publishes the tree and marks the changed ranges; otherwise leave
`treesit--pre-redisplay-tick' as it was, so the next redisplay asks
again."
  (let ((tick treesit--pre-redisplay-tick)
        (parser treesit-primary-parser)
        (nidhogg--redisplay-fontifying (current-buffer))
        (nidhogg-workers-treesit--hook-entry t))
    (apply #'treesit--pre-redisplay args)
    (when (and (treesit-parser-p parser)
               (not (nidhogg-treesit-parser-ready-p parser)))
      (setq treesit--pre-redisplay-tick tick)
      (when (nidhogg-workers--treesit-await parser)
        (apply #'treesit--pre-redisplay args)))))

(defun nidhogg-workers-treesit--pre-redisplay (&rest args)
  "In place of `treesit--pre-redisplay' in `pre-redisplay-functions'.
ARGS are the hook's.  With deferral on, see
`nidhogg-workers-treesit--demand-primary'; then land this buffer's
deferred regions if no parse job runs for it any more."
  (if (nidhogg-workers-treesit--on)
      (nidhogg-workers-treesit--demand-primary args)
    (apply #'treesit--pre-redisplay args))
  (when nidhogg-workers-treesit--pending
    (nidhogg-workers-treesit--maybe-land)))

(defun nidhogg-workers-treesit--ready-p (beg end)
  "Non-nil if the parsers that fontify BEG..END give their trees now.
The primary parser and the local parsers of the region make redisplay's
demand (a 1 ms parse, else a job); then, like every other parser of the
buffer, each waits for its job within the redisplay's budget.  A parser
whose job still runs makes the answer nil and counts a deferral."
  (condition-case nil
      (let ((primary treesit-primary-parser)
            (ready t))
        (when (treesit-parser-p primary)
          (nidhogg-workers-treesit--demand-primary nil)
          (setq ready (nidhogg-workers--treesit-await primary t)))
        (dolist (p (and ready (treesit-local-parsers-on beg end)))
          (when (and ready (not (nidhogg-treesit-parser-ready-p p)))
            (treesit-parser-changed-regions p)
            (setq ready (nidhogg-workers--treesit-await p t))))
        (dolist (p (and ready (treesit-parser-list)))
          (when (and ready (not (eq p primary)))
            (setq ready (nidhogg-workers--treesit-await p t))))
        ready)
    ;; Whatever failed fails again in the original, as it would today.
    (error t)))

(defun nidhogg-workers-treesit--defer (beg end)
  "Record BEG..END for the landing; return nil, fontifying nothing.
The region keeps the faces it has, and jit-lock marks it fontified."
  (push (cons (copy-marker beg) (copy-marker end t))
        nidhogg-workers-treesit--pending)
  (unless (memq (current-buffer) nidhogg-workers-treesit--pending-buffers)
    (push (current-buffer) nidhogg-workers-treesit--pending-buffers))
  nil)

(defun nidhogg-workers-treesit--fontify-region (orig beg end &optional loudly)
  "Around `font-lock-fontify-region-function', ORIG, called with BEG END LOUDLY.
While redisplay fontifies this very buffer with deferral on, and a
parser's job does not finish within the redisplay's budget, return
without calling ORIG, so nothing is unfontified, and record the region
for the landing.  Anything else calls ORIG: `font-lock-ensure' (the
variable is nil, or names another buffer, as when Org fontifies a
source block in a temporary buffer), display code called from Lisp."
  (if (and (eq (current-buffer) nidhogg--redisplay-fontifying)
           (nidhogg-workers-treesit--on)
           (not (nidhogg-workers-treesit--ready-p beg end)))
      (nidhogg-workers-treesit--defer beg end)
    (funcall orig beg end loudly)))

(defun nidhogg-workers-treesit--maybe-land ()
  "Land this buffer's deferred regions unless a parse job runs for it.
Mark them unfontified and ask every window showing the buffer to
redisplay: jit-lock fontifies them again, and its demand publishes the
tree."
  (when (and nidhogg-workers-treesit--pending
             (not (nidhogg-workers--treesit-busy-p (current-buffer))))
    (let ((regions nidhogg-workers-treesit--pending))
      (setq nidhogg-workers-treesit--pending nil)
      (setq nidhogg-workers-treesit--pending-buffers
            (delq (current-buffer) nidhogg-workers-treesit--pending-buffers))
      (save-restriction
        (widen)
        (with-silent-modifications
          (dolist (r regions)
            (when (< (car r) (cdr r))
              (put-text-property (car r) (cdr r) 'fontified nil))
            (set-marker (car r) nil)
            (set-marker (cdr r) nil))))
      (force-window-update (current-buffer)))))

(defun nidhogg-workers-treesit--land ()
  "Land the deferred regions of every buffer no parse job runs for.
The drainer calls this when a job started with deferral on lands."
  (dolist (buf (copy-sequence nidhogg-workers-treesit--pending-buffers))
    (if (buffer-live-p buf)
        (with-current-buffer buf
          (nidhogg-workers-treesit--maybe-land))
      (setq nidhogg-workers-treesit--pending-buffers
            (delq buf nidhogg-workers-treesit--pending-buffers)))))

(defun nidhogg-workers-treesit--setup (&rest _)
  "Install the deferral wrappers in this buffer.
The `:after' advice of `treesit-major-mode-setup'.  The wrappers act only
while `nidhogg-workers-treesit' and `nidhogg-treesit-defer' are on."
  (when treesit-font-lock-settings
    (add-function :around (local 'font-lock-fontify-region-function)
                  #'nidhogg-workers-treesit--fontify-region)
    (when (memq #'treesit--pre-redisplay pre-redisplay-functions)
      (remove-hook 'pre-redisplay-functions #'treesit--pre-redisplay t)
      (add-hook 'pre-redisplay-functions
                #'nidhogg-workers-treesit--pre-redisplay 0 t))))

(advice-add 'treesit-major-mode-setup :after
            #'nidhogg-workers-treesit--setup)

(provide 'nidhogg-workers-treesit)

;;; nidhogg-workers-treesit.el ends here
