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
  "Chrome (rounded corners and a shadow) for child frames.
Nil, an alist, or a function of the frame returning an alist.  The
alist may hold `corner-radius' (a number of pixels) and `shadow' (a
plist of :blur, :offset (X . Y), :spread and :opacity, or nil for no
shadow).  A frame's own `corner-radius' and `shadow' parameters win
over it; `(corner-radius . 0)' and `(shadow . nil)' opt a frame out.
Root frames and decorated (not `undecorated') child frames get no
chrome.  The shadow colour is the foreground of the
`child-frame-shadow' face."
  :type '(choice (const :tag "None" nil)
                 (alist :key-type (choice (const corner-radius) (const shadow))
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
(defun glitnir--theme-refresh (&rest _) (glitnir-refresh))
(add-hook 'enable-theme-functions #'glitnir--theme-refresh)
(add-hook 'disable-theme-functions #'glitnir--theme-refresh)
(add-variable-watcher
 'child-frame-chrome
 (lambda (&rest _) (run-at-time 0 nil #'glitnir-refresh)))

(provide 'glitnir-lisp)

;;; glitnir.el ends here
