;;; nidhogg-views.el --- Sjá, the native view model: its Lisp functions  -*- lexical-binding: t -*-

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

;; Sjá (nidhogg/sja, in the Rust library Glitnir carries) holds the model of
;; each hub-fed view as it is drawn, and each instance's user state:
;; selection, form, search, lit list, expanded and ticked sets.  A key
;; moves the selection by following a link or walking a list the hub sent,
;; with no round trip.  These are thin functions over the one C call,
;; `nidhogg-view--call'; the verbs and every rule are Sjá's, every word of
;; a model (links, lists, fields, classes) is the hub's.
;;
;; A view is named by a string.  An instance is a fixnum handle from
;; `nidhogg-view-open'.  Ids, links, lists and field names are strings.

;;; Code:

(eval-and-compile
  (defconst nidhogg-view--ops
    '((apply . 1) (open . 2) (close . 3) (follow . 4) (walk . 5) (walk-to . 6)
      (set . 7) (get . 8) (hub-lost . 9) (stats . 10) (push . 11) (drain . 12)
      (named . 13) (due . 14) (test-panic . 99))
    "Sjá's op numbers (nidhogg/sja/include/sja.h)."))

(defvar nidhogg-view-more nil
  "The direction (-1 or 1) of the last walk that answered `more'.")

(defconst nidhogg-view--statuses [applied dropped resync too-large broken]
  "What `nidhogg-view-apply' answers, by Sjá's status number.")

(defmacro nidhogg-view--op (name)
  "The op number of NAME, at compile time."
  (or (alist-get name nidhogg-view--ops) (error "No Sjá op %s" name)))

(defsubst nidhogg-view-apply (view hub version model)
  "Apply MODEL, one model patch (a JSON string) from HUB at VERSION, to VIEW.
Return `applied', `dropped' (not newer than what is drawn), `resync' (ask
the hub for a reset), `too-large' or `broken'."
  (aref nidhogg-view--statuses
        (nidhogg-view--call (nidhogg-view--op apply) view hub version model)))

(defconst nidhogg-view--decisions [queue reset drop resync none]
  "What `nidhogg-view-push' decides, by Sjá's number.")

(defsubst nidhogg-view-push (line)
  "Hand LINE, a hub's whole `view-model' line, to Sjá, unparsed.
Sjá reads its envelope, decides and queues its model for the draw.
Return (VIEW VERSION DECISION), DECISION `queue' (a diff queued),
`reset' (a reset queued: a reset asked for has come), `drop', `resync'
\(a gap or a cap: ask the hub for a reset) or `none' (no instance of
VIEW is open); nil when LINE is not a `view-model' line."
  (let ((r (nidhogg-view--call (nidhogg-view--op push) line)))
    (when r (setcar (nthcdr 2 r) (aref nidhogg-view--decisions (nth 2 r))))
    r))

(defsubst nidhogg-view--drained (r)
  "R, a drain's answer, with its status as a symbol."
  (when r (setcar (cdr r) (aref nidhogg-view--statuses (nth 1 r))))
  r)

(defsubst nidhogg-view-drain (view &optional upto)
  "Apply VIEW's queued models of a version up to UPTO (all when nil), oldest first.
Return (APPLIED LAST VERSION): how many applied, the last status (as
`nidhogg-view-apply' answers; `resync' or `too-large' dropped the queue
and its stream) and the newest version the stream queued (nil when it
has none); nil when no instance of VIEW is open."
  (nidhogg-view--drained (nidhogg-view--call (nidhogg-view--op drain) view upto)))

(defsubst nidhogg-view-named (view n)
  "A row patch of VIEW named model version N: it waits for that draw."
  (nidhogg-view--call (nidhogg-view--op named) view n))

(defsubst nidhogg-view-due (view)
  "Apply VIEW's queued models when the oldest is one no row patch named.
Return as `nidhogg-view-drain', or nil when none was due."
  (nidhogg-view--drained (nidhogg-view--call (nidhogg-view--op due) view)))

(defsubst nidhogg-view-open (view &optional group)
  "Open an instance of VIEW and return its handle.
Instances in the same GROUP (a string) share one selection."
  (nidhogg-view--call (nidhogg-view--op open) view group))

(defsubst nidhogg-view-close (h)
  "Close instance H; the view goes with its last instance."
  (nidhogg-view--call (nidhogg-view--op close) h))

(defsubst nidhogg-view--step (answer)
  "ANSWER of a walk: an id or nil, or (ID . more) (and the direction in
`nidhogg-view-more') when a window must move first."
  (if (consp answer)
      (progn (setq nidhogg-view-more (cdr answer)) (cons (car answer) 'more))
    answer))

(defsubst nidhogg-view-follow (h link)
  "Follow LINK (\"up\", \"left\", ...) from H's selection; return the selection."
  (nidhogg-view--call (nidhogg-view--op follow) h link))

(defsubst nidhogg-view-walk (h list step)
  "Step STEP entries along LIST from H's selection, wrapping; return the
selection, or (ID . more) when LIST is a window that must move first."
  (nidhogg-view--step (nidhogg-view--call (nidhogg-view--op walk) h list step)))

(defsubst nidhogg-view-walk-to (h list n)
  "Jump to position N (0-based) of LIST; as `nidhogg-view-walk'."
  (nidhogg-view--step (nidhogg-view--call (nidhogg-view--op walk-to) h list n)))

(defsubst nidhogg-view-set (h what value)
  "Set WHAT of instance H to VALUE.
WHAT is `select' (an id or nil; returns the selection), `search', `form'
or `lit' (a string or nil), `fold' or `tick' (an id to toggle; returns
non-nil when it is now in the set), or `prefix' (a tree: select the
visible sibling whose label starts with VALUE; returns the selection)."
  (let ((r (nidhogg-view--call (nidhogg-view--op set) h (symbol-name what) value)))
    (if (memq what '(fold tick)) (eql r 1) r)))

(defsubst nidhogg-view-get (h what &optional arg arg2)
  "Read WHAT of instance H.
WHAT is `selection', `kind' (of ARG, an id, or the selection), `field'
\(ARG a field name, ARG2 an id or the selection), `flag', `parent',
`rows', `list' (ARG a list name), `finished' (the id that went from under
the selection, once), `class' (of ARG), `classes' (a vector [ID CLASS
...] of the nodes whose class changed since the last read), `reclass',
`class-names', `expanded', `open' (of ARG: a tree node is open), `ticked', `version', `stats' or `health' (0 ok,
1 no model, 2 broken, 3 stale)."
  (nidhogg-view--call (nidhogg-view--op get) h (symbol-name what) arg arg2))

(defsubst nidhogg-view-hub-lost ()
  "The hub connection went: every view is stale; keys keep working."
  (nidhogg-view--call (nidhogg-view--op hub-lost)))

(defsubst nidhogg-view-stats ()
  "Sjá's counts: (VIEWS INSTANCES BYTES PANICS BROKEN)."
  (nidhogg-view--call (nidhogg-view--op stats)))

(provide 'nidhogg-views)

;;; nidhogg-views.el ends here
