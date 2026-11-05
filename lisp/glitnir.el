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
alist may hold `corner-radius' (a number of pixels) and `shadow' (a
plist of :blur, :offset (X . Y), :spread and :opacity, or nil for no
shadow).  A frame's own `corner-radius' and `shadow' parameters win
over it; `(corner-radius . 0)' and `(shadow . nil)' opt a frame out.
Root frames and decorated (not `undecorated') child frames get no
chrome.  The shadow colour is the foreground of the
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
                 (alist :key-type (choice (const corner-radius) (const shadow)
                                          (const transition) (const backdrop-blur))
                        :value-type sexp)
                 function)
  :group 'glitnir
  :version "31.1")

(defface child-frame-shadow
  '((t :foreground "black"))
  "Face whose foreground is the colour of a child frame's shadow.
Set it for one frame with `set-face-foreground' to change that frame's."
  :version "31.1"
  :group 'frames)

(defvar glitnir--shadow-colors (make-hash-table :test 'eq :weakness 'key)
  "Frame -> the shadow colour (R G B) last resolved for it.")

(defun glitnir--shadow-color (frame)
  "FRAME's shadow colour as (R G B), 0-255."
  (let* ((name (face-foreground 'child-frame-shadow frame t))
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
:opacity O :color (R G B))) in logical pixels, from FRAME's parameters
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
           (radius (cdr r))
           (shadow (and (cdr s) (glitnir--shadow (cdr s) frame))))
      (when radius
        (unless (and (numberp radius) (>= radius 0))
          (signal 'wrong-type-argument (list 'natnump radius))))
      (if shadow
          (puthash frame (plist-get shadow :color) glitnir--shadow-colors)
        (remhash frame glitnir--shadow-colors))
      (when (or shadow (and radius (> radius 0)))
        (list :radius (or radius 0) :shadow shadow)))))

(defconst glitnir--chrome-slots
  '((on . 0) (radius . 1) (blur . 2) (ox . 3) (oy . 4) (spread . 5) (opacity . 6) (rgb . 7)
    (show-kind . 8) (show-ms . 9) (show-x1 . 10) (show-y1 . 11) (show-x2 . 12) (show-y2 . 13)
    (hide-kind . 14) (hide-ms . 15) (hide-x1 . 16) (hide-y1 . 17) (hide-x2 . 18) (hide-y2 . 19)
    (backdrop . 20) (beneath-rgb . 21) (ring-rgb . 22) (ring-opacity . 23))
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
  "Colour NAME on FRAME as the number 0xRRGGBB, or nil when it names none."
  (when-let* (((stringp name))
              (v (color-values name frame)))
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

(defun glitnir--chrome-vector (frame)
  "Return FRAME's chrome as a vector of numbers, or nil for a root frame.
The slots are `glitnir--chrome-slots'.  Slots 0-7 come from `glitnir--resolve',
clamped; 8-19 from the `transition' option, which the desktop's reduced-motion
setting turns off; 20 from `backdrop-blur', 0 while the frame's
`alpha-background' is opaque; `beneath-rgb' and `ring-rgb' are -1.  A child
frame with no chrome has a vector whose first slot is 0."
  (when (frame-parent frame)
    (let* ((res (glitnir--resolve frame))
           (tr (glitnir--transition (glitnir--option frame 'transition)))
           (blur (glitnir--option frame 'backdrop-blur))
           (v (make-vector (length glitnir--chrome-slots) 0)))
      (unless (or (null blur) (and (numberp blur) (>= blur 0)))
        (signal 'wrong-type-argument (list 'glitnir-backdrop-blur blur)))
      (when (and (fboundp 'nhwl-settings-get) (eql (nhwl-settings-get 'reduced-motion) 1))
        (setcar tr 0)
        (setcar (nthcdr 6 tr) 0))
      (dotimes (i 12) (aset v (+ 8 i) (nth i tr)))
      (aset v (alist-get 'backdrop glitnir--chrome-slots)
            (if (and blur (glitnir--translucent-p frame)) blur 0))
      (aset v (alist-get 'beneath-rgb glitnir--chrome-slots) -1)
      (aset v (alist-get 'ring-rgb glitnir--chrome-slots) -1)
      (when res
        (glitnir--fill-shadow v (plist-get res :radius) (plist-get res :shadow)))
      v)))

;; Window cards: a window with the parameters `corner-radius', `shadow'
;; or `ring' is drawn as a card.  Its corners show the colour beneath the
;; window, its shadow and ring lie outside it, in the gap.

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
0-7 and 22-23.  `beneath-rgb', what the corners show, is the foreground of
the `window-divider' face on WINDOW's frame (the colour dividers are drawn
in), else the frame's background.  The motion and backdrop slots are 0."
  (let* ((frame (window-frame window))
         (radius (window-parameter window 'corner-radius))
         (spec (window-parameter window 'shadow))
         (sh (and spec (glitnir--shadow spec frame)))
         (ring-spec (window-parameter window 'ring))
         (ring (and ring-spec (glitnir--ring ring-spec frame))))
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
        v))))

(defvar glitnir--card-generation 0
  "Bumped when every window's card is resolved again (a theme changed).")

(defun glitnir--card-key (window)
  "What WINDOW's card is resolved from, or nil when it asks for none."
  (let ((radius (window-parameter window 'corner-radius))
        (shadow (window-parameter window 'shadow))
        (ring (window-parameter window 'ring)))
    (when (or radius shadow ring)
      (let ((f (window-frame window)))
        (list radius shadow ring glitnir--card-generation
              (face-foreground 'window-divider f t)
              (frame-parameter f 'background-color)
              (and shadow (face-foreground 'child-frame-shadow f t))
              (and (consp ring) (facep (car ring)) (face-foreground (car ring) f t)))))))

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
  (let ((key (glitnir--card-key window)))
    (unless (equal key (window-parameter window 'glitnir--card-key))
      ;; the key first: a parameter that signals does so once
      (set-window-parameter window 'glitnir--card-key key)
      (set-window-parameter window 'glitnir--card (and key (glitnir--card-vector window)))
      (glitnir--cards-flag (window-frame window)))))

(defun glitnir--recheck-all-cards ()
  "Resolve every window's card again."
  (setq glitnir--card-generation (1+ glitnir--card-generation))
  (dolist (f (frame-list))
    (dolist (w (window-list f t))
      (glitnir--recheck-cards w))))

(defun glitnir--recheck-frame-cards (window)
  "Recheck the cards of every window on WINDOW's frame, before redisplay.
Only for the frame's first window and the selected window, the windows
redisplay passes, so a frame's windows are checked at most twice."
  (let ((f (window-frame window)))
    (when (or (eq window (frame-first-window f)) (eq window (selected-window)))
      (dolist (w (window-list f t))
        (glitnir--recheck-cards w)))))

(add-hook 'pre-redisplay-functions #'glitnir--recheck-frame-cards)

(defun glitnir-refresh (&optional frame)
  "Resolve the chrome of FRAME (default: every child frame) again."
  (dolist (f (if frame (list frame) (frame-list)))
    (when (and (frame-live-p f) (frame-parent f))
      (glitnir--rechrome f))))

(defun glitnir--recheck-shadow (window)
  "Refresh WINDOW's frame when its `child-frame-shadow' colour changed.
Only for visible chromed child frames, once per frame per redisplay (at
its first window)."
  (let ((f (window-frame window)))
    (when (and (frame-parent f)
               (eq window (frame-first-window f))
               (frame-visible-p f))
      (let ((cached (gethash f glitnir--shadow-colors)))
        (when (and cached (not (equal cached (glitnir--shadow-color f))))
          (glitnir-refresh f))))))

(add-hook 'pre-redisplay-functions #'glitnir--recheck-shadow)
(defun glitnir--settings-changed (key _value)
  "Resolve every child frame again when the desktop's reduced-motion KEY changed."
  (when (eq key 'reduced-motion) (glitnir-refresh)))
;; nhwl-settings.el loads after this file; `add-hook' makes the hook variable here.
(add-hook 'nhwl-settings-changed-functions #'glitnir--settings-changed)
(defun glitnir--theme-refresh (&rest _)
  (glitnir-refresh)
  (glitnir--recheck-all-cards))
(add-hook 'enable-theme-functions #'glitnir--theme-refresh)
(add-hook 'disable-theme-functions #'glitnir--theme-refresh)
(add-variable-watcher
 'child-frame-chrome
 (lambda (&rest _) (run-at-time 0 nil #'glitnir-refresh)))

(provide 'glitnir-lisp)

;;; glitnir.el ends here
