;;; glitnir.el --- Glitnir, Nidhogg's compositor: its Lisp side  -*- lexical-binding: t -*-

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

;; Glitnir's own Lisp, preloaded when Nidhogg is built with Glitnir
;; (loadup.el's one `(featurep 'glitnir)' line).  Glitnir changes no
;; other Lisp file: its options, faces and resolver live here.

;;; Code:

(defface glitnir-debug-damage
  '((t :background "red"))
  "Face whose background flashes the rectangles Glitnir committed.
Used when `glitnir-debug' is `damage'."
  :version "31.1"
  :group 'basic-faces)

(defgroup glitnir nil
  "Glitnir, Nidhogg's compositor."
  :group 'frames
  :version "31.1")

(defcustom child-frame-chrome nil
  "Chrome (rounded corners, a shadow, motion) for child frames.
Nil, an alist, or a function of the frame returning an alist.  The
alist may hold `corner-radius' (a number of pixels), `shadow' (a
plist of :blur, :offset (X . Y), :spread and :opacity, or nil for no
shadow) and `ring' ((COLOR . OPACITY), COLOR a face, whose foreground
is used, or a colour name: a line 1 pixel wide just outside the frame,
as a window card's).  A frame's own `corner-radius', `shadow' and
`ring' parameters win over it; `(corner-radius . 0)', `(shadow . nil)'
and `(ring . nil)' opt a frame out.
Decorated (not `undecorated') child frames get no chrome, and root
frames none from this option: an undecorated root frame takes its
corners and shadow from its own `toplevel-chrome' parameter, an alist
of the same `corner-radius' and `shadow', drawn in the margins it keeps
where the compositor offers no server-side decorations
\(`nhwl-toplevel-chrome-p').  The shadow colour is the foreground of the
`child-frame-shadow' face.

It may also hold `transition', how the frame appears and disappears,
and `backdrop-blur', a blur radius in pixels for what lies behind the
frame (it shows only with a translucent `alpha-background').  Both are
frame parameters too, and the parameter wins.  `transition' is nil,
`fade', `pop' (a pop in, a fade out), or a plist (:show KIND :hide KIND
:duration S :curve C :hide-duration S :hide-curve C).  KIND is nil,
`fade' or `pop'; C is `linear', `ease', `ease-in', `ease-out',
`ease-in-out' or (cubic-bezier X1 Y1 X2 Y2).  The shorthands and the
plist's defaults show over 0.12 s with `ease-out' and hide over 0.08 s
with `ease-in'.  When the desktop asks for reduced motion there is no
transition."
  :type '(choice (const :tag "None" nil)
                 (alist :key-type (choice (const corner-radius) (const shadow) (const ring)
                                          (const transition) (const backdrop-blur))
                        :value-type sexp)
                 function)
  :group 'glitnir
  :version "31.1")

(defface child-frame-shadow
  '((t :foreground "black"))
  "Face whose foreground is the colour of a child frame's shadow.
Set it for one frame with `set-face-foreground' to change that frame's.
A theme's colour shows at once; one set outside a theme (with
`set-face-foreground', `set-face-attribute' or Customize) shows after
`glitnir-refresh'."
  :version "31.1"
  :group 'frames)

(defun glitnir--shadow-color (frame)
  "FRAME's shadow colour as (R G B), 0-255.
A frame still being made has no faces yet: the colour for new frames
then."
  (let* ((name (face-foreground
                'child-frame-shadow
                (if (gethash 'child-frame-shadow (frame--face-hash-table frame)) frame t)
                t))
         (v (and (stringp name) (color-values name frame))))
    (if v (mapcar (lambda (c) (/ c 257)) v) (list 0 0 0))))

(defun glitnir--shadow (spec frame)
  "Validate shadow plist SPEC and complete it for FRAME."
  (unless (and (proper-list-p spec) (zerop (% (length spec) 2)))
    (signal 'wrong-type-argument (list 'plistp spec)))
  (let ((p spec))
    (while p
      (unless (memq (car p) '(:blur :offset :spread :opacity))
        (signal 'wrong-type-argument (list 'glitnir-shadow-key (car p))))
      (setq p (cddr p))))
  (let ((blur (or (plist-get spec :blur) 0))
        (off (or (plist-get spec :offset) '(0 . 0)))
        (spread (or (plist-get spec :spread) 0))
        (opacity (or (plist-get spec :opacity) 0.35)))
    (dolist (v (list blur spread opacity))
      (unless (numberp v) (signal 'wrong-type-argument (list 'numberp v))))
    (unless (and (consp off) (numberp (car off)) (numberp (cdr off)))
      (signal 'wrong-type-argument (list 'glitnir-shadow-offset off)))
    (list :blur blur :offset off :spread spread :opacity opacity
          :color (glitnir--shadow-color frame))))

(defun glitnir--resolve (frame)
  "Return FRAME's chrome, or nil.
The value is (:radius R :shadow (:blur B :offset (X . Y) :spread S
:opacity O :color (R G B)) :ring (RGB . OPACITY)) in logical pixels,
the ring as `glitnir--ring' gives it, from FRAME's parameters
and `child-frame-chrome'.  Root frames and decorated (not
`undecorated') child frames have none."
  (when (frame-parent frame)
    (let* ((params (frame-parameters frame))
           (chromed (cdr (assq 'undecorated params)))
           (opt (and chromed
                     (if (functionp child-frame-chrome)
                         (funcall child-frame-chrome frame)
                       child-frame-chrome)))
           (r (and chromed (or (assq 'corner-radius params) (assq 'corner-radius opt))))
           (s (and chromed (or (assq 'shadow params) (assq 'shadow opt))))
           (g (and chromed (or (assq 'ring params) (assq 'ring opt))))
           (radius (cdr r))
           (shadow (and (cdr s) (glitnir--shadow (cdr s) frame)))
           (ring (and (cdr g) (glitnir--ring (cdr g) frame))))
      (when radius
        (unless (and (numberp radius) (>= radius 0))
          (signal 'wrong-type-argument (list 'natnump radius))))
      (when (or shadow ring (and radius (> radius 0)))
        (list :radius (or radius 0) :shadow shadow :ring ring)))))

(defconst glitnir--chrome-slots
  '((on . 0) (radius . 1) (blur . 2) (ox . 3) (oy . 4) (spread . 5) (opacity . 6) (rgb . 7)
    (show-kind . 8) (show-ms . 9) (show-x1 . 10) (show-y1 . 11) (show-x2 . 12) (show-y2 . 13)
    (hide-kind . 14) (hide-ms . 15) (hide-x1 . 16) (hide-y1 . 17) (hide-x2 . 18) (hide-y2 . 19)
    (backdrop . 20) (beneath-rgb . 21) (ring-rgb . 22) (ring-opacity . 23) (group . 24) (fill-rgb . 25))
  "Slot names of `glitnir--chrome-vector' and their indices.
The same numbers are `NHG_CHROME_*' in nhglue.h and `SLOT_*' in the glue's chrome.rs.")

(defconst glitnir--curves
  '((linear 0 0 1 1) (ease 0.25 0.1 0.25 1) (ease-in 0.42 0 1 1)
    (ease-out 0 0 0.58 1) (ease-in-out 0.42 0 0.58 1))
  "The CSS easing names and their control points X1 Y1 X2 Y2.")

(defun glitnir--curve (spec)
  "Return the control points (X1 Y1 X2 Y2) of curve SPEC.
SPEC is a name in `glitnir--curves' or (cubic-bezier X1 Y1 X2 Y2) with X1
and X2 in 0..1.  Signal `wrong-type-argument' otherwise."
  (cond
   ((and (symbolp spec) (cdr (assq spec glitnir--curves))))
   ((and (proper-list-p spec) (eq (car spec) 'cubic-bezier) (= (length spec) 5)
         (not (memq nil (mapcar #'numberp (cdr spec))))
         (<= 0 (nth 1 spec) 1) (<= 0 (nth 3 spec) 1))
    (cdr spec))
   (t (signal 'wrong-type-argument (list 'glitnir-curve spec)))))

(defun glitnir--leg (kind secs curve)
  "Return one transition leg as a list (KIND MS X1 Y1 X2 Y2).
KIND is nil or `none', `fade' or `pop'; SECS a non-negative number of seconds;
CURVE as for `glitnir--curve'.  Both are checked even when KIND is none; a leg
with no motion is all zeros."
  (let ((k (cdr (assq kind '((nil . 0) (none . 0) (fade . 1) (pop . 2))))))
    (unless k (signal 'wrong-type-argument (list 'glitnir-transition-kind kind)))
    (unless (and (numberp secs) (>= secs 0))
      (signal 'wrong-type-argument (list 'glitnir-transition-duration secs)))
    (let ((points (glitnir--curve curve)))
      (if (zerop k)
          (make-list 6 0)
        (append (list k (round (* 1000 secs))) points)))))

(defun glitnir--transition (spec)
  "Return transition SPEC as the slots 8-19: show leg, then hide leg.
SPEC is nil, `fade' (fade in, fade out), `pop' (pop in, fade out) or a
plist (:show KIND :hide KIND :duration S :curve C :hide-duration S
:hide-curve C).  The shorthands show over 0.12 s, `ease-out', and hide
over 0.08 s, `ease-in'; so do the plist's defaults, and a missing :show
or :hide is no motion for that leg."
  (let (show hide dur curve hdur hcurve)
    (cond
     ((null spec) (setq show nil hide nil))
     ((memq spec '(fade pop)) (setq show spec hide 'fade))
     ((and (proper-list-p spec) (zerop (% (length spec) 2)))
      (let ((p spec))
        (while p
          (unless (memq (car p) '(:show :hide :duration :curve :hide-duration :hide-curve))
            (signal 'wrong-type-argument (list 'glitnir-transition-key (car p))))
          (setq p (cddr p))))
      (setq show (plist-get spec :show) hide (plist-get spec :hide)
            dur (plist-get spec :duration) curve (plist-get spec :curve)
            hdur (plist-get spec :hide-duration) hcurve (plist-get spec :hide-curve)))
     (t (signal 'wrong-type-argument (list 'glitnir-transition spec))))
    (append (glitnir--leg show (or dur 0.12) (or curve 'ease-out))
            (glitnir--leg hide (or hdur 0.08) (or hcurve 'ease-in)))))

(defun glitnir--option (frame key)
  "Return child frame FRAME's option KEY: its parameter, else `child-frame-chrome''s.
Only undecorated child frames have one."
  (let ((params (frame-parameters frame)))
    (when (cdr (assq 'undecorated params))
      (let ((opt (if (functionp child-frame-chrome)
                     (funcall child-frame-chrome frame)
                   child-frame-chrome)))
        (cdr (or (assq key params) (assq key opt)))))))

(defun glitnir--translucent-p (frame)
  "Non-nil when FRAME's `alpha-background' lets what lies behind it show."
  (when-let* ((a (cdr (assq 'alpha-background (frame-parameters frame)))))
    (< a (if (floatp a) 1.0 100))))

(defun glitnir--rgb (rgb)
  "RGB, a list (R G B) of 0-255 values, as the number 0xRRGGBB."
  (let ((n 0))
    (dotimes (i 3)
      (let ((c (nth i rgb)))
        (setq n (logior (ash n 8) (if (natnump c) (min c 255) 0)))))
    n))

(defun glitnir--color-rgb (name frame)
  "Colour NAME on FRAME as the number 0xRRGGBB, or nil when it names none.
A colour spec (\"#808080\", \"rgb:...\") is taken exactly: on a frame
that is not graphical (batch, a terminal) `color-values' gives the
nearest of the terminal's colours instead, and Glitnir's numbers are
24-bit colours for the GPU, never a terminal's."
  (when-let* (((stringp name))
              (v (or (color-values-from-color-spec name)
                     (color-values name frame))))
    (glitnir--rgb (mapcar (lambda (c) (/ c 257)) v))))

(defun glitnir--fill-shadow (v radius sh)
  "Fill slots 0-7 of chrome vector V: on, RADIUS and shadow SH, clamped.
SH is what `glitnir--shadow' gives, or nil."
  (aset v 0 1)
  (aset v 1 (max 0 radius))
  (when sh
    (let ((off (plist-get sh :offset)))
      (aset v 2 (max 0 (plist-get sh :blur)))
      (aset v 3 (car off))
      (aset v 4 (cdr off))
      (aset v 5 (plist-get sh :spread))
      (aset v 6 (min 1 (max 0 (plist-get sh :opacity))))
      (aset v 7 (glitnir--rgb (plist-get sh :color))))))

(defcustom glitnir-reduced-motion 'auto
  "Whether child-frame transitions are turned off.
`auto' follows the desktop's reduced-motion setting; t always turns
them off, nil never does."
  :type '(choice (const :tag "Follow the desktop" auto)
                 (const :tag "Always reduce" t)
                 (const :tag "Never reduce" nil))
  :version "31.1"
  :group 'glitnir)

(declare-function nhmac-reduced-motion-p "term/nhmac-win" ())

(defun glitnir--reduced-motion-p ()
  "Non-nil when transitions are off, by `glitnir-reduced-motion'.
`auto' asks the macOS shell (the system's Reduce motion) there, and the
desktop's settings on Linux."
  (if (eq glitnir-reduced-motion 'auto)
      (if (fboundp 'nhmac-reduced-motion-p)
          (nhmac-reduced-motion-p)
        (and (fboundp 'nhwl-settings-get)
             (eql (nhwl-settings-get 'reduced-motion) 1)))
    glitnir-reduced-motion))

(defun glitnir-reduced-motion-p ()
  "Non-nil when Glitnir turns motion off.
That is when `glitnir-reduced-motion' is t, or `auto' while the
desktop asks for reduced motion.  Code that animates its own frames
can follow it."
  (glitnir--reduced-motion-p))

(defun glitnir--toplevel-vector (frame)
  "Return root frame FRAME's chrome as a vector of numbers, or nil.
Only an `undecorated' frame whose `toplevel-chrome' asks for corners or a
shadow has one: slots 0-7 from it, clamped, as `glitnir--resolve' takes
a child frame's; no motion, no backdrop, `beneath-rgb' and `ring-rgb' -1."
  (let* ((params (frame-parameters frame))
         (opt (and (cdr (assq 'undecorated params)) (cdr (assq 'toplevel-chrome params))))
         (radius (cdr (assq 'corner-radius opt)))
         (sh (and (cdr (assq 'shadow opt)) (glitnir--shadow (cdr (assq 'shadow opt)) frame))))
    (when radius
      (unless (and (numberp radius) (>= radius 0))
        (signal 'wrong-type-argument (list 'natnump radius))))
    (when (or sh (and radius (> radius 0)))
      (let ((v (make-vector (length glitnir--chrome-slots) 0)))
        (aset v (alist-get 'beneath-rgb glitnir--chrome-slots) -1)
        (aset v (alist-get 'ring-rgb glitnir--chrome-slots) -1)
        (glitnir--fill-shadow v (or radius 0) sh)
        v))))

(defun glitnir--chrome-vector (frame)
  "Return FRAME's chrome as a vector of numbers, or nil.
The slots are `glitnir--chrome-slots'.  Slots 0-7 come from `glitnir--resolve',
clamped; 8-19 from the `transition' option, which the desktop's reduced-motion
setting turns off; 20 from `backdrop-blur', 0 while the frame's
`alpha-background' is opaque; 22-23 from `ring' (-1 and 0 without one);
`beneath-rgb' and `fill-rgb' are -1.  A child
frame with no chrome, or any child frame while `glitnir-enable' is nil, has a vector
whose first slot is 0 and no blur.  A root frame's is
`glitnir--toplevel-vector'."
  (if (not (frame-parent frame))
      (glitnir--toplevel-vector frame)
    (let* ((on (or (not (boundp 'glitnir-enable)) glitnir-enable))
           (res (and on (glitnir--resolve frame)))
           (tr (glitnir--transition (glitnir--option frame 'transition)))
           (blur (glitnir--option frame 'backdrop-blur))
           (v (make-vector (length glitnir--chrome-slots) 0)))
      (unless (or (null blur) (and (numberp blur) (>= blur 0)))
        (signal 'wrong-type-argument (list 'glitnir-backdrop-blur blur)))
      (when (glitnir--reduced-motion-p)
        (setcar tr 0)
        (setcar (nthcdr 6 tr) 0))
      (dotimes (i 12) (aset v (+ 8 i) (nth i tr)))
      (aset v (alist-get 'backdrop glitnir--chrome-slots)
            (if (and on blur (glitnir--translucent-p frame)) blur 0))
      (aset v (alist-get 'beneath-rgb glitnir--chrome-slots) -1)
      (aset v (alist-get 'ring-rgb glitnir--chrome-slots) -1)
      (aset v (alist-get 'fill-rgb glitnir--chrome-slots) -1)
      (when res
        (glitnir--fill-shadow v (plist-get res :radius) (plist-get res :shadow))
        (when-let* ((ring (plist-get res :ring)))
          (aset v (alist-get 'ring-rgb glitnir--chrome-slots) (car ring))
          (aset v (alist-get 'ring-opacity glitnir--chrome-slots) (cdr ring))))
      v)))

;; Window cards: a window with the parameters `corner-radius', `shadow'
;; or `ring' is drawn as a card.  Its corners show the colour beneath the
;; window, its shadow and ring lie outside it, in the gap.  Windows with
;; the same `card-group' that tile a rectangle (one column or one row) are
;; one card: its corners, shadow and ring go around them all, and the
;; dividers between them take the group's fill.

(defvar glitnir--card-groups (make-hash-table :test 'equal)
  "Card group names (the car of a window's `card-group') and their numbers.
A session uses a few names; they are kept for its life.")

(defun glitnir--card-group (spec frame)
  "Return card group SPEC, (NAME . FILL), as (NUMBER . RGB) for FRAME.
NAME is any object; windows with `equal' names are one group.  FILL is a
face, whose background is used, or a colour name: what the dividers
between the windows show.  One that names no colour is the frame's
background.  Signal `wrong-type-argument' for another shape."
  (unless (and (consp spec) (or (facep (cdr spec)) (stringp (cdr spec))))
    (signal 'wrong-type-argument (list 'glitnir-card-group spec)))
  (cons (or (gethash (car spec) glitnir--card-groups)
            (puthash (car spec) (1+ (hash-table-count glitnir--card-groups)) glitnir--card-groups))
        (or (glitnir--color-rgb (if (facep (cdr spec)) (face-background (cdr spec) frame t) (cdr spec)) frame)
            (glitnir--color-rgb (frame-parameter frame 'background-color) frame)
            0)))

(defun glitnir--ring (spec frame)
  "Return ring SPEC, (COLOR . OPACITY), as (RGB . OPACITY) for FRAME, or nil.
COLOR is a face, whose foreground is used, or a colour name; OPACITY is
0 to 1.  A colour that names none is no ring.  Signal
`wrong-type-argument' for another shape."
  (unless (and (consp spec) (or (facep (car spec)) (stringp (car spec)))
               (numberp (cdr spec)))
    (signal 'wrong-type-argument (list 'glitnir-ring spec)))
  (let ((rgb (glitnir--color-rgb (if (facep (car spec))
                                     (face-foreground (car spec) frame t)
                                   (car spec))
                                 frame)))
    (and rgb (cons rgb (min 1 (max 0 (cdr spec)))))))

(defun glitnir--card-vector (window)
  "Return WINDOW's card as a vector of numbers, or nil when it has none.
The slots are `glitnir--chrome-slots'.  The window parameters
`corner-radius' (pixels), `shadow' (a plist as in `child-frame-chrome',
coloured by the `child-frame-shadow' face) and `ring' ((COLOR . OPACITY),
COLOR a face or a colour name: a line just outside the card) give slots
0-7 and 22-23; `card-group' ((NAME . FILL), `glitnir--card-group') gives
`group' and `fill-rgb' (0 and -1 without one).  `beneath-rgb', what the corners show, is the foreground of
the `window-divider' face on WINDOW's frame (the colour dividers are drawn
in), else the frame's background.  The motion and backdrop slots are 0."
  (let* ((frame (window-frame window))
         (radius (window-parameter window 'corner-radius))
         (spec (window-parameter window 'shadow))
         (sh (and spec (glitnir--shadow spec frame)))
         (ring-spec (window-parameter window 'ring))
         (ring (and ring-spec (glitnir--ring ring-spec frame)))
         (group-spec (window-parameter window 'card-group))
         (group (and group-spec (glitnir--card-group group-spec frame))))
    (when radius
      (unless (and (numberp radius) (>= radius 0))
        (signal 'wrong-type-argument (list 'natnump radius))))
    (when (or sh ring (and radius (> radius 0)))
      (let ((v (make-vector (length glitnir--chrome-slots) 0)))
        (glitnir--fill-shadow v (or radius 0) sh)
        (aset v (alist-get 'beneath-rgb glitnir--chrome-slots)
              (or (glitnir--color-rgb (face-foreground 'window-divider frame t) frame)
                  (glitnir--color-rgb (frame-parameter frame 'background-color) frame)
                  0))
        (aset v (alist-get 'ring-rgb glitnir--chrome-slots) (if ring (car ring) -1))
        (aset v (alist-get 'ring-opacity glitnir--chrome-slots) (if ring (cdr ring) 0))
        (aset v (alist-get 'group glitnir--chrome-slots) (if group (car group) 0))
        (aset v (alist-get 'fill-rgb glitnir--chrome-slots) (if group (cdr group) -1))
        v))))

(defvar glitnir--card-generation 0
  "Bumped when every window's card is resolved again (a theme changed).")

(defun glitnir--card-key-same-p (old radius shadow ring group)
  "Non-nil when the key OLD still stands for the parameters RADIUS, SHADOW, RING and GROUP.
The key is (RADIUS SHADOW RING GENERATION GROUP) from the pass that resolved the card, or nil for a window that
asked for none.  Nothing is allocated, and no face is looked up: colours are resolved with the card, and again when
`glitnir--card-generation' changes."
  (if old
      (and (eq (nth 3 old) glitnir--card-generation)
           (equal (nth 0 old) radius) (equal (nth 1 old) shadow) (equal (nth 2 old) ring)
           (equal (nth 4 old) group))
    (not (or radius shadow ring))))

(defun glitnir--cards-flag (frame)
  "Set FRAME's parameter `glitnir--cards' to whether a window has a card.
Only when that changed."
  (let (any)
    (dolist (w (window-list frame t))
      (when (window-parameter w 'glitnir--card) (setq any t)))
    (unless (eq any (frame-parameter frame 'glitnir--cards))
      (set-frame-parameter frame 'glitnir--cards any))))

(defun glitnir--recheck-cards (window)
  "Resolve WINDOW's card again when what it comes from changed.
The vector goes into the window parameter `glitnir--card', which C lists
with the window when the frame publishes."
  (let ((radius (window-parameter window 'corner-radius))
        (shadow (window-parameter window 'shadow))
        (ring (window-parameter window 'ring))
        (group (window-parameter window 'card-group)))
    (unless (glitnir--card-key-same-p (window-parameter window 'glitnir--card-key) radius shadow ring group)
      ;; the key first: a parameter that signals does so once
      (set-window-parameter window 'glitnir--card-key
                            (and (or radius shadow ring)
                                 (list radius shadow ring glitnir--card-generation group)))
      (let ((v (and (or radius shadow ring) (glitnir--card-vector window))))
        (unless (equal v (window-parameter window 'glitnir--card))
          (set-window-parameter window 'glitnir--card v)
          ;; the window draws, so the frame publishes and C lists the new card
          (force-window-update window)))
      (glitnir--cards-flag (window-frame window)))))

(defun glitnir--recheck-all-cards ()
  "Resolve every window's card again."
  (setq glitnir--card-generation (1+ glitnir--card-generation))
  (dolist (f (frame-list))
    (dolist (w (window-list f t))
      (glitnir--recheck-cards w))))

;; Blocks: a window parameter `glitnir-blocks' draws rings (rounded, with
;; their corners outside the rounded path in the colour beneath), rules
;; (a bottom hairline) or bars (a filled rectangle: a selected row's
;; accent) around runs of the window's rows, over its text.
;; The rows give their own fill.  It is resolved into `glitnir--blocks'
;; only when it, or the theme, changed; C resolves each block's rows
;; against the window's matrix when the frame publishes.

(defun glitnir--block-vector (spec frame)
  "Return block SPEC as the vector C reads, for FRAME.
SPEC is (KIND START END . PLIST): KIND `ring', `rule' or `bar' (the
rectangle filled in the line's colour, square); START and END
markers (or positions) of the window's buffer, END exclusive; PLIST
`:x0' and `:x1' (pixels from the text area's left edge, default 0 and
its width), `:radius' (pixels, a ring's outer corners), `:ring' (a face,
whose foreground is the line's colour, or (FACE-OR-COLOUR . OPACITY))
and `:beneath' (a colour name or a face, whose background is used: what
a ring's corners show outside its rounded path, the colour under the
block).  The vector is [KIND START END X0 X1 RADIUS RING-RGB
RING-OPACITY BENEATH-RGB], KIND 0 a ring, 1 a rule and 2 a bar, RING-RGB
-1 for no line.  Signal `wrong-type-argument' for another shape."
  (unless (and (consp spec) (memq (car spec) '(ring rule bar)) (consp (cdr spec)) (consp (cddr spec)))
    (signal 'wrong-type-argument (list 'glitnir-block spec)))
  (let* ((plist (nthcdr 3 spec))
         (ring-spec (plist-get plist :ring))
         (ring (cond ((null ring-spec) nil)
                     ((consp ring-spec) (glitnir--ring ring-spec frame))
                     (t (glitnir--ring (cons ring-spec 1) frame))))
         (beneath (plist-get plist :beneath)))
    (vector (pcase (car spec) ('rule 1) ('bar 2) (_ 0)) (nth 1 spec) (nth 2 spec)
            (or (plist-get plist :x0) 0) (or (plist-get plist :x1) most-positive-fixnum)
            (or (plist-get plist :radius) 0)
            (if ring (car ring) -1) (if ring (cdr ring) 0)
            (or (glitnir--color-rgb (if (facep beneath) (face-background beneath frame t) beneath) frame)
                (glitnir--color-rgb (frame-parameter frame 'background-color) frame)
                0))))

(defvar glitnir--blocks-any nil
  "Whether a window of the frame has blocks.
Bound by `glitnir--recheck-frame-cards' around its walk.")

(defun glitnir--recheck-blocks (window)
  "Resolve WINDOW's blocks when its `glitnir-blocks' or the theme changed.
The vector of block vectors goes into the window parameter
`glitnir--blocks', which C reads when the frame publishes.  A window
that never had blocks gets no parameter of them (its key neither): the
theme's generation would change it on every theme change."
  (let ((spec (window-parameter window 'glitnir-blocks))
        (key (window-parameter window 'glitnir--blocks-key)))
    (unless (or (and (eq (car key) spec) (eq (cdr key) glitnir--card-generation))
                (and (null spec) (null key)))
      ;; the key first: a parameter that signals does so once; blocks gone,
      ;; no key (nil is an absent parameter)
      (set-window-parameter window 'glitnir--blocks-key
                            (and spec (cons spec glitnir--card-generation)))
      (let* ((frame (window-frame window))
             (v (and spec (vconcat (mapcar (lambda (b) (glitnir--block-vector b frame)) spec)))))
        (unless (equal v (window-parameter window 'glitnir--blocks))
          (set-window-parameter window 'glitnir--blocks v)
          (force-window-update window))))
    (when (window-parameter window 'glitnir--blocks)
      (setq glitnir--blocks-any t))))

(defun glitnir--ruler-colour (spec frame)
  "Ruler colour SPEC, a face or colour or (FACE-OR-COLOUR . OPACITY), as (RGB . OPACITY) for FRAME, or nil."
  (and spec (glitnir--ring (if (consp spec) spec (cons spec 1)) frame)))

(defun glitnir--ruler-vector (spec frame)
  "Return ruler SPEC as the vector C reads, for FRAME.
SPEC is a plist: `:width' (the strip's width), `:right' (its gap from
the window's right inner edge), `:height' (a mark's least height), all
device pixels; `:band' (the colour of the band over the part of the
buffer the window shows: a face, whose foreground is used, a colour
name, or (FACE-OR-COLOUR . OPACITY)); `:marks', a list of
\(POS X0 X1 COLOUR): a buffer position (a marker or an integer), the x
range of its lane in the strip and its colour, as for `:band'.  The
vector is [WIDTH RIGHT HEIGHT BAND-RGB BAND-OPACITY MARK...], each MARK
[POS X0 X1 RGB OPACITY], RGB -1 for none."
  (let ((band (glitnir--ruler-colour (plist-get spec :band) frame)))
    (vconcat (vector (or (plist-get spec :width) 0) (or (plist-get spec :right) 0) (or (plist-get spec :height) 1)
                     (if band (car band) -1) (if band (cdr band) 0))
             (mapcar (lambda (m)
                       (let ((c (glitnir--ruler-colour (nth 3 m) frame)))
                         (vector (nth 0 m) (nth 1 m) (nth 2 m) (if c (car c) -1) (if c (cdr c) 0))))
                     (plist-get spec :marks)))))

(defun glitnir--recheck-ruler (window)
  "Resolve WINDOW's overview ruler when its `glitnir-ruler' or the theme changed.
The vector goes into the window parameter `glitnir--ruler', which C
reads when the frame publishes: the strip at the window's right inner
edge, its marks at their position's fraction of the buffer (characters,
as the scroll bar measures) and the band over the part shown.  Nothing
is done per redisplay: a scroll moves the band in C."
  (let ((spec (window-parameter window 'glitnir-ruler))
        (key (window-parameter window 'glitnir--ruler-key)))
    (unless (or (and (eq (car key) spec) (eq (cdr key) glitnir--card-generation))
                (and (null spec) (null key)))
      (set-window-parameter window 'glitnir--ruler-key (and spec (cons spec glitnir--card-generation)))
      (let ((v (and spec (glitnir--ruler-vector spec (window-frame window)))))
        (unless (equal v (window-parameter window 'glitnir--ruler))
          (set-window-parameter window 'glitnir--ruler v)
          (force-window-update window))))
    (when (window-parameter window 'glitnir--ruler)
      (setq glitnir--blocks-any t))))

;; Edges: a window parameter `glitnir-edges' draws thin lines over the
;; window's text from one buffer position to another, with rounded bends,
;; an arrowhead or dashes (the debugger's heap graph: a reference from a
;; card's field to the card it points at).  Like blocks, it is resolved
;; into `glitnir--edges' only when it, or the theme, changed; C resolves
;; each point against the window's matrix when the frame publishes, and
;; the line's width, bends, arrow and dashes come from the frame's scale.

(defconst glitnir--edge-points 8
  "The most points an edge has: its start, up to 6 bends and its end.")

(defun glitnir--edge-place (place)
  "PLACE, a position or (POS DX DY), as the list (POS DX DY)."
  (cond ((or (integerp place) (markerp place)) (list place 0 0))
        ((and (consp place) (or (integerp (car place)) (markerp (car place)))
              (integerp (or (nth 1 place) 0)) (integerp (or (nth 2 place) 0)))
         (list (car place) (or (nth 1 place) 0) (or (nth 2 place) 0)))
        (t (signal 'wrong-type-argument (list 'glitnir-edge-place place)))))

(defun glitnir--edge-vector (spec frame)
  "Return edge SPEC as the vector C reads, for FRAME, or nil for no colour.
SPEC is (FROM TO COLOUR . PLIST): FROM and TO places, each a buffer
position (a marker or an integer) or (POS DX DY), DX and DY pixels from
the left of POS's glyph and the top of its row; COLOUR a face, whose
foreground is used, a colour name or (FACE-OR-COLOUR . OPACITY); PLIST
`:points' (the bends between, places, at most 6), `:arrow' (an
arrowhead at TO) and `:dash' (dashed).  The vector is [FLAGS RGB
OPACITY NPOINTS POS DX DY...], FLAGS 1 for the arrow and 2 for dashes.
Signal `wrong-type-argument' for another shape."
  (unless (and (consp spec) (consp (cdr spec)) (consp (cddr spec)))
    (signal 'wrong-type-argument (list 'glitnir-edge spec)))
  (let* ((plist (nthcdr 3 spec))
         (places (mapcar #'glitnir--edge-place
                         (append (list (nth 0 spec)) (plist-get plist :points) (list (nth 1 spec)))))
         (c (nth 2 spec))
         (colour (glitnir--ring (if (and (consp c) (numberp (cdr c))) c (cons c 1)) frame)))
    (when (> (length places) glitnir--edge-points)
      (signal 'wrong-type-argument (list 'glitnir-edge-points spec)))
    (and colour
         (apply #'vector
                (logior (if (plist-get plist :arrow) 1 0) (if (plist-get plist :dash) 2 0))
                (car colour) (cdr colour) (length places)
                (apply #'append places)))))

(defun glitnir--recheck-edges (window)
  "Resolve WINDOW's edges when its `glitnir-edges' or the theme changed.
The vector of edge vectors goes into the window parameter
`glitnir--edges', which C reads when the frame publishes; it sets the
frame's flag that blocks set, so C lists the frame.  A window that
never had edges gets no parameter of them."
  (let ((spec (window-parameter window 'glitnir-edges))
        (key (window-parameter window 'glitnir--edges-key)))
    (unless (or (and (eq (car key) spec) (eq (cdr key) glitnir--card-generation))
                (and (null spec) (null key)))
      (set-window-parameter window 'glitnir--edges-key (and spec (cons spec glitnir--card-generation)))
      (let* ((frame (window-frame window))
             (v (and spec (vconcat (delq nil (mapcar (lambda (e) (glitnir--edge-vector e frame)) spec))))))
        (when (equal v []) (setq v nil))
        (unless (equal v (window-parameter window 'glitnir--edges))
          (set-window-parameter window 'glitnir--edges v)
          (force-window-update window))))
    (when (window-parameter window 'glitnir--edges)
      (setq glitnir--blocks-any t))))

(defun glitnir-ruler-position (posn)
  "The buffer position at the y of POSN, a click on a window's overview ruler.
The strip stands for the whole buffer, top to bottom, by characters, as
C draws it; nil when POSN's window has no ruler."
  (let ((w (posn-window posn)))
    (when (and (windowp w) (window-parameter w 'glitnir--ruler))
      (pcase-let* ((`(,_ ,top ,_ ,_) (window-pixel-edges w))
                   (`(,_ ,btop ,_ ,bbot) (window-inside-pixel-edges w))
                   (y (- (+ top (cdr (posn-x-y posn))) btop))
                   (frac (min 1.0 (max 0.0 (/ (float y) (max 1 (- bbot btop)))))))
        (with-current-buffer (window-buffer w)
          (+ (point-min) (round (* frac (- (point-max) (point-min))))))))))

(defvar glitnir--blocks-seen (make-hash-table :test 'eq :weakness 'key)
  "Per frame, the last value of its `glitnir--blocks' parameter.
As `glitnir--recheck-frame-cards' gave it.")

(defvar glitnir--cards-seen (make-hash-table :test 'eq :weakness 'key)
  "Per frame, the value `glitnir--recheck-frame-cards' last gave its `glitnir--cards' parameter.")

(defun glitnir--recheck-tree (window)
  "Recheck the card of every leaf window under WINDOW and its siblings; non-nil when one has a card.
This runs on every redisplay, so it walks the tree rather than building a window list."
  (let (any)
    (while window
      (if (window-child window)
          (when (glitnir--recheck-tree (window-child window)) (setq any t))
        (glitnir--recheck-cards window)
        (glitnir--recheck-blocks window)
        (glitnir--recheck-ruler window)
        (glitnir--recheck-edges window)
        (when (window-parameter window 'glitnir--card) (setq any t)))
      (setq window (window-next-sibling window)))
    any))

(defun glitnir--recheck-frame-cards (window)
  "Recheck the cards of every window on WINDOW's frame, before redisplay.
Only for the frame's first window and the selected window, the windows
redisplay passes, so a frame's windows are checked at most twice.  A pass
that finds nothing changed allocates nothing and looks up no face; the frame's
flag `glitnir--cards' follows the windows, also when the last card window was
deleted."
  (let ((f (window-frame window)))
    (when (or (eq window (frame-first-window f)) (eq window (selected-window)))
      (let* ((glitnir--blocks-any nil)
             (any (glitnir--recheck-tree (frame-root-window f))))
        ;; `frame-parameter' conses the frame's whole parameter list, so the last value is kept beside it
        (unless (eq any (gethash f glitnir--cards-seen))
          (puthash f any glitnir--cards-seen)
          (unless (eq any (frame-parameter f 'glitnir--cards))
            (set-frame-parameter f 'glitnir--cards any)))
        ;; the same for blocks: C lists a frame's blocks while its `glitnir--blocks' is set
        (unless (eq glitnir--blocks-any (gethash f glitnir--blocks-seen))
          (puthash f glitnir--blocks-any glitnir--blocks-seen)
          (unless (eq glitnir--blocks-any (frame-parameter f 'glitnir--blocks))
            (set-frame-parameter f 'glitnir--blocks glitnir--blocks-any)))))))

(add-hook 'pre-redisplay-functions #'glitnir--recheck-frame-cards)

(defun glitnir-refresh (&optional frame)
  "Resolve the chrome of FRAME (default: every child and toplevel-chromed frame).
Without FRAME the cards of every window are resolved again too: a card's
colours are read when it is resolved, so a face changed outside a theme (a
theme change does this itself) shows after this call."
  (dolist (f (if frame (list frame) (frame-list)))
    (when (and (frame-live-p f) (or (frame-parent f) (frame-parameter f 'toplevel-chrome)))
      (glitnir--rechrome f)))
  (unless frame
    (glitnir--recheck-all-cards)
    ;; a theme or reduced motion changed: a layout leg jumps to its end
    (glitnir--end-legs)))

(defun glitnir--settings-changed (key _value)
  "Resolve every child frame again when the desktop's reduced-motion KEY changed."
  (when (eq key 'reduced-motion) (glitnir-refresh)))
;; nhwl-settings.el loads after this file; `add-hook' makes the hook variable here.
(add-hook 'nhwl-settings-changed-functions #'glitnir--settings-changed)
(defun glitnir--theme-refresh (&rest _)
  (glitnir-refresh))
(add-hook 'enable-theme-functions #'glitnir--theme-refresh)
(add-hook 'disable-theme-functions #'glitnir--theme-refresh)
(add-variable-watcher
 'glitnir-reduced-motion
 (lambda (&rest _) (run-at-time 0 nil #'glitnir-refresh)))
(add-variable-watcher
 'child-frame-chrome
 (lambda (&rest _) (run-at-time 0 nil #'glitnir-refresh)))

;; Layout legs: a window's layout moves in the presenter.  Lisp asks
;; for a picture of the window's text area before it draws a new layout
;; (the window parameter `glitnir-layout-capture'), waits for
;; `glitnir-layout-captured-p', then draws the new layout and says what
;; moved (`glitnir-layout-motion').  The presenter moves the old
;; pictures to their new places over the next frames.

(defconst glitnir--leg-slots
  '((capture . 0) (x . 1) (y . 2) (w . 3) (h . 4) (motion . 5) (background-rgb . 6)
    (ms . 7) (x1 . 8) (y1 . 9) (x2 . 10) (y2 . 11) (step . 12) (count . 13))
  "Slot names of `glitnir--leg-vector' and their indices; the items follow.
Each item is 10 slots: its kind (1 move, 2 enter, 3 exit), its id, its old
rectangle X Y W H and its new one, zeros for the one it lacks.  The same
numbers are `SLOT_*' in the glue's legs.rs.")

(defvar glitnir--leg-serial 0
  "The newest capture number given to a window.")

(defun glitnir--leg-id (id)
  "ID, any object, as an item's number (ids that are `equal' have the same)."
  (logand (sxhash-equal id) #xffffff))

(defun glitnir--leg-rect (r)
  "R, a list (X Y W H) of integers with W and H not negative, as a list.
Signal `wrong-type-argument' otherwise."
  (unless (and (proper-list-p r) (= (length r) 4) (not (memq nil (mapcar #'integerp r)))
               (>= (nth 2 r) 0) (>= (nth 3 r) 0))
    (signal 'wrong-type-argument (list 'glitnir-layout-rect r)))
  r)

(defun glitnir--leg-items (motion)
  "The item slots of plist MOTION's :moves, :enters and :exits, as a list."
  (let ((none '(0 0 0 0)) out)
    (dolist (m (plist-get motion :moves))
      (push (append (list 1 (glitnir--leg-id (car m)))
                    (glitnir--leg-rect (nth 1 m)) (glitnir--leg-rect (nth 2 m)))
            out))
    (dolist (e (plist-get motion :enters))
      (push (append (list 2 (glitnir--leg-id (car e))) none (glitnir--leg-rect (nth 1 e))) out))
    (dolist (x (plist-get motion :exits))
      (push (append (list 3 (glitnir--leg-id (car x))) (glitnir--leg-rect (nth 1 x)) none) out))
    (nreverse out)))

(defun glitnir--leg-background (spec frame)
  "Background SPEC (a colour name or a face, default `default') as 0xRRGGBB on FRAME."
  (let ((spec (or spec 'default)))
    (or (glitnir--color-rgb (if (facep spec) (face-background spec frame t) spec) frame)
        (glitnir--color-rgb (frame-parameter frame 'background-color) frame)
        0)))

(defun glitnir--leg-vector (window serial motion-p)
  "Return WINDOW's layout leg as a vector of numbers: capture SERIAL of its text area.
With MOTION-P, also the leg from that capture that the window parameter
`glitnir-layout-motion' describes.  The slots are `glitnir--leg-slots'."
  (let* ((edges (window-body-pixel-edges window))
         (motion (and motion-p (window-parameter window 'glitnir-layout-motion)))
         (items (and motion (glitnir--leg-items motion)))
         (v (make-vector (+ 14 (* 10 (length items))) 0)))
    (aset v 0 serial)
    (aset v 1 (nth 0 edges))
    (aset v 2 (nth 1 edges))
    (aset v 3 (- (nth 2 edges) (nth 0 edges)))
    (aset v 4 (- (nth 3 edges) (nth 1 edges)))
    (when motion
      (let ((secs (or (plist-get motion :duration) 0.16))
            (points (glitnir--curve (or (plist-get motion :curve) 'ease-out))))
        (unless (and (numberp secs) (>= secs 0))
          (signal 'wrong-type-argument (list 'glitnir-layout-duration secs)))
        (aset v 5 serial)
        (aset v 6 (glitnir--leg-background (plist-get motion :background) (window-frame window)))
        (aset v 7 (round (* 1000 secs)))
        (dotimes (i 4) (aset v (+ 8 i) (nth i points)))
        (aset v 12 (if (plist-get motion :step) 1 0))
        (aset v 13 (length items))
        (let ((i 14))
          (dolist (it items)
            (dolist (n it) (aset v i n) (setq i (1+ i)))))))
    v))

(defun glitnir--leg-to-send (window)
  "WINDOW's `glitnir-layout-capture', or nil under reduced motion (the layout jumps)."
  ;; the parameter first: most windows have none, and redisplay asks for each
  (when-let* ((cap (window-parameter window 'glitnir-layout-capture)))
    (and (not (glitnir--reduced-motion-p)) cap)))

(defun glitnir--leg-send (window sent)
  "Hand WINDOW's leg vector to the shell and remember SENT, (CAPTURE MOTION SERIAL EDGES MOVED VECTOR).
SERIAL 0 ends the leg: the vector is nil."
  (glitnir--layout-leg window (nth 5 sent))
  (set-window-parameter window 'glitnir--leg-sent sent))

(defun glitnir--recheck-leg (window)
  "Hand WINDOW's capture and leg to the shell when its parameters changed.
A new `glitnir-layout-capture' value gets the next capture number; a new
`glitnir-layout-motion' value goes with that capture, or with a new one
when a leg already went with it.  Either parameter nil, or a text area
that moved, ends the leg.  Runs before each window's redisplay."
  (let ((sent (window-parameter window 'glitnir--leg-sent))
        (cap (glitnir--leg-to-send window)))
    (when (or cap sent)
      (let ((mot (window-parameter window 'glitnir-layout-motion))
            (edges (window-body-pixel-edges window)))
        (pcase-let ((`(,scap ,smot ,serial ,sedges ,moved ,_) sent))
          (let ((new (lambda (motion-p)
                       (let ((n (setq glitnir--leg-serial (1+ glitnir--leg-serial))))
                         (glitnir--leg-send
                          window (list cap mot n edges motion-p
                                       (glitnir--leg-vector window n motion-p))))))
                (end (lambda ()
                       (if (and (null cap) (null mot))
                           (set-window-parameter window 'glitnir--leg-sent nil)
                         (set-window-parameter window 'glitnir--leg-sent
                                               (list cap mot 0 edges nil nil)))
                       (when (and serial (> serial 0))
                         (glitnir--layout-leg window nil)))))
            (cond
             ((null cap) (unless (and (eql serial 0) (eq mot smot)) (funcall end)))
             ((not (and sent (eq cap scap))) (funcall new (and mot (not (eq mot smot)))))
             ((not (equal edges sedges)) (when (> serial 0) (funcall end)))
             ((eq mot smot))
             ((null mot) (if moved (funcall end) (setcar (cdr sent) nil)))
             ((and (> serial 0) (not moved))
              (glitnir--leg-send window (list cap mot serial edges t
                                              (glitnir--leg-vector window serial t))))
             (t (funcall new t)))))))))

(add-hook 'pre-redisplay-functions #'glitnir--recheck-leg)

(defun glitnir-layout-captured-p (&optional window)
  "Non-nil when the presenter holds the capture WINDOW's `glitnir-layout-capture' asked for.
WINDOW defaults to the selected window.  Always nil under reduced motion,
off the native shell and in batch: the layout then jumps."
  (let ((window (window-normalize-window window t)))
    (glitnir--recheck-leg window)
    (pcase-let ((`(,_ ,_ ,serial ,_ ,_ ,vector) (window-parameter window 'glitnir--leg-sent)))
      (and (glitnir--leg-to-send window) serial (> serial 0)
           (eql (glitnir--layout-leg window vector) serial)))))

(defun glitnir--end-legs ()
  "End every window's layout leg at once: each jumps to its new layout."
  (dolist (f (frame-list))
    (dolist (w (window-list f t))
      (let ((sent (window-parameter w 'glitnir--leg-sent)))
        (when (and sent (> (nth 2 sent) 0))
          (set-window-parameter w 'glitnir--leg-sent
                                (list (nth 0 sent) (nth 1 sent) 0 (nth 3 sent) nil nil))
          (glitnir--layout-leg w nil))))))

;;;; mouse-drag-line's transient map
;; A stock bug, fixed here so that mouse.el stays as upstream has it.  `mouse-drag-line''s transient map is a prefix of
;; itself for each area a release may land on (stock: eight, none for a fringe, so a drag released over one is
;; "<right-fringe> <drag-mouse-1> is undefined").  Each such prefix written into the map multiplies the printed size
;; of a command that holds the map about tenfold: stock's eight print the release command at 150 MB, and adding the
;; two fringes made it over 700 MB, which ran a session out of memory when a command recorder or `view-lossage'
;; printed `this-command'.  While `mouse-drag-line' runs, the map `set-transient-map' is given gets all ten prefixes
;; through one symbol instead.

(defvar glitnir--drag-line-p nil
  "Non-nil while `mouse-drag-line' runs (`glitnir--drag-line-map' reads it).")

(defun glitnir--drag-line-around (orig &rest args)
  "`mouse-drag-line' (ORIG with ARGS), noting that its transient map is on the way."
  (let ((glitnir--drag-line-p t)) (apply orig args)))

(defun glitnir--drag-line-map (args)
  "`set-transient-map's :filter-args, from `mouse-drag-line': the map's area prefixes through a symbol."
  (let ((map (car args)))
    (when (and glitnir--drag-line-p (keymapp map))
      (let ((prefix (make-symbol "mouse-drag-line-map")))
        (fset prefix map)
        (dolist (area '(mode-line header-line tab-line vertical-line right-divider bottom-divider
                        left-margin right-margin left-fringe right-fringe))
          (define-key map (vector area) prefix))))
    args))

(advice-add 'mouse-drag-line :around #'glitnir--drag-line-around)
(advice-add 'set-transient-map :filter-args #'glitnir--drag-line-map)

(provide 'glitnir-lisp)

;; The first frame born with a child-frame minibuffer when early-init.el asks for one (preloaded with this file: loadup
;; has one line per group).
(load "nidhogg-frame")

;;; glitnir.el ends here
