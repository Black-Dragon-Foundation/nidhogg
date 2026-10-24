;;; nidhogg-workers.el --- Nidhogg's worker threads: the switches and NIDHOGG_WORKERS  -*- lexical-binding: t -*-

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

;; Preloaded when Nidhogg is built with --with-nidhogg-workers (loadup.el's
;; one `(featurep 'nidhogg-workers)' line).  The C side provides the feature
;; `nidhogg-workers'; this file provides `nidhogg-workers-lisp'.  It holds
;; the switches, the NIDHOGG_WORKERS environment variable (read at startup,
;; never at preload time) and the hooks that load the area files.

;;; Code:

(defgroup nidhogg-workers nil
  "Work off the main thread."
  :group 'processes
  :prefix "nidhogg-workers-")

(defmacro nidhogg-workers--defswitch (name default doc)
  "Define the switch NAME, a boolean `defcustom' defaulting to DEFAULT."
  `(defcustom ,name ,default ,doc :type 'boolean :group 'nidhogg-workers))

(nidhogg-workers--defswitch nidhogg-workers-process-reader t
  "Non-nil means a reader thread may own process output.
The sub-switches gate each kind of process.")
(nidhogg-workers--defswitch nidhogg-workers-reader-pipes t
  "Non-nil means the reader owns pipe processes.")
(nidhogg-workers--defswitch nidhogg-workers-reader-sockets t
  "Non-nil means the reader owns network sockets.")
(nidhogg-workers--defswitch nidhogg-workers-reader-ptys nil
  "Non-nil means the reader owns pty processes.")
(nidhogg-workers--defswitch nidhogg-workers-framing nil
  "Non-nil means processes with a framing are read and framed off the main thread.")
(nidhogg-workers--defswitch nidhogg-workers-treesit nil
  "Non-nil means tree-sitter parses on a worker thread.")
(nidhogg-workers--defswitch nidhogg-treesit-defer nil
  "Non-nil means tree-sitter colour may land a moment after the text.")
(nidhogg-workers--defswitch nidhogg-workers-images nil
  "Non-nil means images decode on a worker thread.")
(nidhogg-workers--defswitch nidhogg-image-defer nil
  "Non-nil means an image may appear a moment after the text.")
(nidhogg-workers--defswitch nidhogg-workers-autosave nil
  "Non-nil means auto-saving writes on a worker thread.")
(nidhogg-workers--defswitch nidhogg-workers-dns nil
  "Non-nil means host-name lookups run on a worker thread.")

(defcustom nidhogg-redisplay-wait-budget 1
  "Milliseconds a redisplay may wait for a nearly finished worker job."
  :type 'integer
  :group 'nidhogg-workers)

(defconst nidhogg-workers--switches
  '(nidhogg-workers-process-reader nidhogg-workers-reader-pipes
    nidhogg-workers-reader-sockets nidhogg-workers-reader-ptys
    nidhogg-workers-framing nidhogg-workers-treesit nidhogg-treesit-defer
    nidhogg-workers-images nidhogg-image-defer nidhogg-workers-autosave
    nidhogg-workers-dns)
  "Every switch, in the order of the README.")

(defconst nidhogg-workers--defaults
  (mapcar (lambda (s) (cons s (default-value s))) nidhogg-workers--switches)
  "The build's defaults: (SWITCH . VALUE) for every switch.")

(defun nidhogg-workers-set-all (on)
  "Set every switch to t if ON is non-nil, else to nil."
  (dolist (s nidhogg-workers--switches)
    (set-default s (and on t))))

(defun nidhogg-workers--reset ()
  "Set every switch back to the build's default."
  (dolist (d nidhogg-workers--defaults)
    (set-default (car d) (cdr d))))

(defun nidhogg-workers--split (spec)
  "Split SPEC, a string of +SWITCH and -SWITCH items, into the items.
The split is before each \"+nidhogg-\" and \"-nidhogg-\", since the
switch names contain `-'."
  (let ((pos 0) items)
    (while (string-match "[-+]nidhogg-" spec (1+ pos))
      (push (substring spec pos (match-beginning 0)) items)
      (setq pos (match-beginning 0)))
    (push (substring spec pos) items)
    (nreverse items)))

(defun nidhogg-workers-apply (spec)
  "Apply SPEC to the switches.
SPEC is nil, \"\" or \"default\" (the build's defaults), \"on\" (every
switch t), \"off\" (every switch nil), \"inline\" (every switch t, in
inline mode), or a list of +SWITCH and -SWITCH items applied to the
defaults.  It affects processes created, parsers demanded and images
looked up afterwards."
  (cond
   ((member spec '(nil "" "default")) (nidhogg-workers--reset))
   ((equal spec "on") (nidhogg-workers-set-all t))
   ((equal spec "off") (nidhogg-workers-set-all nil))
   ((equal spec "inline")
    (nidhogg-workers-set-all t)
    (when (boundp 'nidhogg-workers--mode)
      (set 'nidhogg-workers--mode 'inline)))
   (t
    (nidhogg-workers--reset)
    (dolist (item (nidhogg-workers--split spec))
      (let ((sym (intern-soft (substring item 1))))
        (unless (memq sym nidhogg-workers--switches)
          (error "Unknown worker switch: %s" item))
        (set-default sym (eq (aref item 0) ?+)))))))

(defun nidhogg-workers--init ()
  "Apply the NIDHOGG_WORKERS environment variable, if set."
  (let ((env (getenv "NIDHOGG_WORKERS")))
    (when env
      (nidhogg-workers-apply env))))

;; Read at startup, after the dump is loaded, never at preload time.
(add-hook 'before-init-hook #'nidhogg-workers--init)

(with-eval-after-load 'jsonrpc (require 'nidhogg-workers-jsonrpc))
(with-eval-after-load 'treesit (require 'nidhogg-workers-treesit))

(provide 'nidhogg-workers-lisp)

;;; nidhogg-workers.el ends here
