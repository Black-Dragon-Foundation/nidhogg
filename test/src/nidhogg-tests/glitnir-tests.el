;;; glitnir-tests.el --- tests for Glitnir (the compositor)  -*- lexical-binding: t -*-
;; test-class: cond
;; test-pattern: handshake

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
(require 'loadhist)

(ert-deftest glitnir/feature ()
  (should (featurep 'glitnir)))

(ert-deftest glitnir/inactive-in-batch ()
  ;; The batch frame's type is t, like a tty frame.
  (should (eq (framep (selected-frame)) t))
  (should-not (glitnir-active-p))
  (should (memq glitnir-enable '(t nil))))

(ert-deftest glitnir/no-paint-times-in-batch ()
  ;; Pattern D (pure): the batch frame is never painted, so a perf probe that reads the present time sees none and
  ;; waits for nothing; the clock is the monotonic one and runs.
  (should-not (glitnir--last-paint))
  (let ((a (glitnir--now-ns)) (b (glitnir--now-ns)))
    (should (natnump a))
    (should (<= a b))))

(ert-deftest glitnir/in-the-configuration ()
  (should (string-search "--with-glitnir" system-configuration-options)))

(ert-deftest glitnir/lisp-preloaded-compiled ()
  ;; The dump loads glitnir.el compiled, like the rest of the preloaded
  ;; Lisp, not from source.
  (let ((file (feature-file 'glitnir-lisp)))
    (should file)
    (should-not (string-suffix-p ".el" file))))


;; The resolver.  The batch frame is a root tty frame; a child frame is stood in for by
;; mocking the two things the resolver asks of it.

(require 'cl-lib)

(defmacro glitnir-test--as-child (params &rest body)
  "Run BODY with the selected frame an undecorated child frame with parameters PARAMS."
  (declare (indent 1))
  `(cl-letf (((symbol-function 'frame-parent) (lambda (&optional _f) (selected-frame)))
             ((symbol-function 'frame-parameters)
              (lambda (&optional _f) (append ,params '((undecorated . t))))))
     ,@body))

(ert-deftest glitnir/resolve-merges-the-option ()
  (let ((child-frame-chrome '((corner-radius . 8) (shadow . (:blur 16 :offset (0 . 6) :opacity 0.5)))))
    (glitnir-test--as-child nil
      (let ((r (glitnir--resolve (selected-frame))))
        (should (= (plist-get r :radius) 8))
        (let ((s (plist-get r :shadow)))
          (should (= (plist-get s :blur) 16))
          (should (equal (plist-get s :offset) '(0 . 6)))
          (should (= (plist-get s :spread) 0))
          (should (= (plist-get s :opacity) 0.5))
          (should (= (length (plist-get s :color)) 3)))))))

(ert-deftest glitnir/frame-parameter-wins ()
  (let ((child-frame-chrome '((corner-radius . 8) (shadow . (:blur 16)))))
    (glitnir-test--as-child '((corner-radius . 3) (shadow . (:blur 4)))
      (let ((r (glitnir--resolve (selected-frame))))
        (should (= (plist-get r :radius) 3))
        (should (= (plist-get (plist-get r :shadow) :blur) 4))))))

(ert-deftest glitnir/zero-radius-opts-out ()
  (let ((child-frame-chrome '((corner-radius . 8) (shadow . (:blur 16)))))
    (glitnir-test--as-child '((corner-radius . 0) (shadow . nil))
      (should-not (glitnir--resolve (selected-frame))))
    ;; the radius alone opted out keeps the shadow
    (glitnir-test--as-child '((corner-radius . 0))
      (let ((r (glitnir--resolve (selected-frame))))
        (should (= (plist-get r :radius) 0))
        (should (plist-get r :shadow))))))

(ert-deftest glitnir/function-option ()
  (let ((child-frame-chrome (lambda (f) (should (framep f)) '((corner-radius . 5)))))
    (glitnir-test--as-child nil
      (let ((r (glitnir--resolve (selected-frame))))
        (should (= (plist-get r :radius) 5))
        (should-not (plist-get r :shadow))))))

(ert-deftest glitnir/bad-shadow-plist-signals ()
  (glitnir-test--as-child '((shadow . (:blurr 4)))
    (should-error (glitnir--resolve (selected-frame)) :type 'wrong-type-argument))
  (glitnir-test--as-child '((shadow . (:blur "x")))
    (should-error (glitnir--resolve (selected-frame)) :type 'wrong-type-argument))
  (glitnir-test--as-child '((corner-radius . "x"))
    (should-error (glitnir--resolve (selected-frame)) :type 'wrong-type-argument)))

(defmacro glitnir-test--as-root (params &rest body)
  "Run BODY with the selected frame a root frame with parameters PARAMS."
  (declare (indent 1))
  `(cl-letf (((symbol-function 'frame-parameters) (lambda (&optional _f) ,params)))
     ,@body))

(ert-deftest glitnir/toplevel-chrome-vector ()
  ;; Pattern D (pure): an undecorated root frame takes its corners and shadow from `toplevel-chrome', in slots 0-7;
  ;; it has no motion, no backdrop and no ring, and `child-frame-chrome' does not reach it.
  (let ((child-frame-chrome '((corner-radius . 3) (transition . pop))))
    (glitnir-test--as-root '((undecorated . t)
                             (toplevel-chrome . ((corner-radius . 12)
                                                 (shadow . (:blur 16 :offset (0 . 6) :opacity 0.35)))))
      (let ((v (glitnir--chrome-vector (selected-frame))))
        (should (= (length v) 26))
        (should (equal (append (cl-subseq v 0 7) nil) '(1 12 16 0 6 0 0.35)))
        (should (cl-every #'zerop (cl-subseq v 8 21)))
        (should (equal (list (aref v 21) (aref v 22) (aref v 23)) '(-1 -1 0)))))))

(ert-deftest glitnir/toplevel-chrome-needs-undecorated ()
  ;; Pattern D (pure): a decorated root, one without `toplevel-chrome' or one that opts out has none; a bad shadow
  ;; signals as a child frame's does.
  (glitnir-test--as-root '((undecorated . t))
    (should-not (glitnir--chrome-vector (selected-frame))))
  (glitnir-test--as-root '((toplevel-chrome . ((corner-radius . 12))))
    (should-not (glitnir--chrome-vector (selected-frame))))
  (glitnir-test--as-root '((undecorated . t) (toplevel-chrome . ((corner-radius . 0))))
    (should-not (glitnir--chrome-vector (selected-frame))))
  (glitnir-test--as-root '((undecorated . t) (toplevel-chrome . ((shadow . (:blurr 1)))))
    (should-error (glitnir--chrome-vector (selected-frame)) :type 'wrong-type-argument)))

(ert-deftest glitnir/root-frames-get-none ()
  (let ((child-frame-chrome '((corner-radius . 8) (shadow . (:blur 16)))))
    (should-not (frame-parent))
    (should-not (glitnir--resolve (selected-frame)))))

(ert-deftest glitnir/decorated-children-get-none ()
  ;; A decorated child frame is a plain one, its own chrome parameters included.
  (let ((child-frame-chrome '((corner-radius . 8) (shadow . (:blur 16)))))
    (glitnir-test--as-child '((undecorated . nil) (corner-radius . 4))
      (should-not (glitnir--resolve (selected-frame))))))

(ert-deftest glitnir/function-option-may-modify-parameters ()
  ;; A function option that sets parameters itself must not recurse into the resolver for good: the C side
  ;; guards that; here the resolver is simply called again from inside, and both calls return.
  (let* ((depth 0) (max-depth 0)
         (child-frame-chrome (lambda (f)
                               (setq depth (1+ depth) max-depth (max depth max-depth))
                               (unwind-protect
                                   (progn (when (= depth 1) (glitnir-refresh f))
                                          '((corner-radius . 6)))
                                 (setq depth (1- depth))))))
    (glitnir-test--as-child nil
      (should (= (plist-get (glitnir--resolve (selected-frame)) :radius) 6))
      (should (<= max-depth 2)))))

(ert-deftest glitnir/probe-rejects-short-bytes ()
  ;; RAW shorter than 4*W*H bytes is refused before anything reads it.
  (skip-unless (fboundp 'glitnir--probe))
  (should-error (glitnir--probe 'edges (list 4 4 (make-string 63 0)) '(0 0 4 4))
                :type 'args-out-of-range)
  (should-error (glitnir--probe 'edges (list 4 4 (make-string 64 0)) '(0 0 5 4))
                :type 'args-out-of-range)
  (should-error (glitnir--probe 'blur (list 4 4 (make-string 64 0)) '(0 0 4 4))))

;; PD's box keys (:corner-radius, :open, :label), taken over for the shell.
(ert-deftest glitnir/box-keys-accepted ()
  (skip-unless (featurep 'glitnir))
  (let ((face (make-symbol "glitnir-box-keys-face")))
    (custom-declare-face face nil "Test face.")
    (set-face-attribute face nil :box '(:line-width 2 :color "red" :corner-radius 6
                                        :open (top left) :label "x"))
    (should (= (plist-get (face-attribute face :box) :corner-radius) 6))
    (set-face-attribute face nil :box '(:corner-radius 2.5 :open nil))
    (set-face-attribute face nil :box '(:corner-radius 0 :open (top bottom left right)))))

(ert-deftest glitnir/box-keys-bad-values-signal ()
  (skip-unless (featurep 'glitnir))
  (dolist (box '((:corner-radius -1) (:corner-radius "6") (:open (diagonal)) (:open top)
                 (:label 5) (:label nil)))
    (should-error (set-face-attribute 'default nil :box box) :type 'error)
    (should (equal (cadr (should-error (set-face-attribute 'default nil :box box)))
                   "Invalid face box"))))

(ert-deftest glitnir/box-keys-ignored-on-tty-frame ()
  (skip-unless (featurep 'glitnir))
  (let ((face (make-symbol "glitnir-box-tty-face")))
    (custom-declare-face face nil "Test face.")
    (set-face-attribute face nil :box '(:corner-radius 6 :open (left) :label "x"))
    (should (stringp (format "%S" (face-attribute face :box nil t))))
    (with-temp-buffer
      (insert (propertize "abc" 'face face))
      (should (equal (buffer-string) "abc")))))

;; The chrome vector: Lisp resolves, C copies, the glue validates.

(ert-deftest glitnir-reduced-motion-auto-on-the-macos-shell ()
  ;; `auto' on the macOS shell asks it (`nhmac-reduced-motion-p', the system's Reduce motion), not the Linux settings
  (let ((glitnir-reduced-motion 'auto) (asked nil))
    (cl-letf (((symbol-function 'nhmac-reduced-motion-p) (lambda () (setq asked t) t))
              ((symbol-function 'nhwl-settings-get) (lambda (_k) 0)))
      (should (glitnir--reduced-motion-p))
      (should asked))
    (cl-letf (((symbol-function 'nhmac-reduced-motion-p) (lambda () nil))
              ((symbol-function 'nhwl-settings-get) (lambda (_k) 1)))
      (should-not (glitnir--reduced-motion-p)))))

(ert-deftest glitnir-chrome-vector-off ()
  ;; Glitnir off (design: off equals Glitnir off): a child asking for corners, a shadow and a blur gets the vector of
  ;; no chrome: slot 0 is 0, no radius, no shadow, no blur.
  (let ((child-frame-chrome '((corner-radius . 8) (shadow . (:blur 16 :offset (2 . 6) :opacity 0.5))
                              (backdrop-blur . 12)))
        (glitnir-enable nil))
    (glitnir-test--as-child '((alpha-background . 0.5))
      (let ((v (glitnir--chrome-vector (selected-frame))))
        (should (= (length v) (length glitnir--chrome-slots)))
        (should (cl-every #'zerop (cl-subseq v 0 8)))
        (should (= (aref v (alist-get 'backdrop glitnir--chrome-slots)) 0))))))

(ert-deftest glitnir-chrome-vector-layout ()
  (let ((child-frame-chrome '((corner-radius . 8) (shadow . (:blur 16 :offset (2 . 6) :spread -1 :opacity 0.5)))))
    (should-not (glitnir--chrome-vector (selected-frame)))
    (glitnir-test--as-child nil
      (let* ((f (selected-frame))
             (v (glitnir--chrome-vector f))
             (r (glitnir--resolve f))
             (s (plist-get r :shadow))
             (c (plist-get s :color)))
        (should (= (length v) 26))
        (should (= (length v) (length glitnir--chrome-slots)))
        (should (equal (list (aref v 0) (aref v 1) (aref v 2) (aref v 3) (aref v 4) (aref v 5) (aref v 6))
                       (list 1 (plist-get r :radius) (plist-get s :blur) 2 6 -1 (plist-get s :opacity))))
        (should (= (aref v 7) (+ (ash (nth 0 c) 16) (ash (nth 1 c) 8) (nth 2 c))))
        (should (cl-every #'zerop (cl-subseq v 8 21)))
        (should (= (aref v 21) -1))
        ;; a child frame has no ring
        (should (equal (list (aref v 22) (aref v 23)) '(-1 0)))))
    ;; a child frame without chrome still has a vector, off
    (let ((child-frame-chrome nil))
      (glitnir-test--as-child nil
        (let ((v (glitnir--chrome-vector (selected-frame))))
          (should (= (length v) 26))
          (should (= (aref v 0) 0)))))))

(ert-deftest glitnir-chrome-vector-clamps ()
  (let ((child-frame-chrome '((corner-radius . 4) (shadow . (:opacity 1.5 :blur -3)))))
    (glitnir-test--as-child nil
      (let ((v (glitnir--chrome-vector (selected-frame))))
        (should (= (aref v 6) 1))
        (should (= (aref v 2) 0))))))

(ert-deftest glitnir-chrome-vector-ring ()
  ;; a ring alone is chrome: slots 22-23 hold its colour and opacity (the batch terminal maps
  ;; colours to its own, so they are read from the spec)
  (cl-letf (((symbol-function 'color-values)
             (lambda (c &optional _f) (and (stringp c) (color-values-from-color-spec c)))))
    (let ((child-frame-chrome '((ring . ("#102030" . 0.45)))))
      (glitnir-test--as-child nil
        (let ((v (glitnir--chrome-vector (selected-frame))))
          (should (= (aref v 0) 1))
          (should (equal (list (aref v 22) (aref v 23)) (list #x102030 0.45)))))
      ;; the frame's parameter wins, and (ring . nil) opts out
      (glitnir-test--as-child '((ring . ("#ffffff" . 2)))
        (let ((v (glitnir--chrome-vector (selected-frame))))
          (should (equal (list (aref v 22) (aref v 23)) (list #xffffff 1)))))
      (glitnir-test--as-child '((ring . nil))
        (let ((v (glitnir--chrome-vector (selected-frame))))
          (should (= (aref v 0) 0))
          (should (equal (list (aref v 22) (aref v 23)) '(-1 0)))))))
  (glitnir-test--as-child '((ring . (3 . 0.5)))
    (should-error (glitnir--chrome-vector (selected-frame)) :type 'wrong-type-argument)))

;; Task 3: transition, backdrop-blur and reduced motion fill slots 8-20.

(defconst glitnir-test--ease-out '(0 0 0.58 1))
(defconst glitnir-test--ease-in '(0.42 0 1 1))

(defun glitnir-test--motion (v)
  "Slots 8-19 of vector V as a list."
  (cl-loop for i from 8 below 20 collect (aref v i)))

(defmacro glitnir-test--with-reduced-motion (value &rest body)
  "Run BODY with the desktop's reduced-motion setting VALUE."
  (declare (indent 1))
  `(cl-letf (((symbol-function 'nhwl-settings-get)
              (lambda (key) (and (eq key 'reduced-motion) ,value))))
     ,@body))

(ert-deftest glitnir-transition-pop-default ()
  (glitnir-test--as-child '((transition . pop))
    (let ((v (glitnir--chrome-vector (selected-frame))))
      (should (equal (glitnir-test--motion v)
                     (append '(2 120) glitnir-test--ease-out '(1 80) glitnir-test--ease-in)))
      (should (= (length v) 26)))))

(ert-deftest glitnir-transition-plist-and-curves ()
  (glitnir-test--as-child '((transition . (:show fade :duration 0.2 :curve ease-in-out
                                           :hide pop :hide-duration 0.05
                                           :hide-curve (cubic-bezier 0.1 2 0.9 -1))))
    (should (equal (glitnir-test--motion (glitnir--chrome-vector (selected-frame)))
                   '(1 200 0.42 0 0.58 1 2 50 0.1 2 0.9 -1))))
  ;; the option may come from child-frame-chrome; the frame's parameter wins
  (let ((child-frame-chrome '((transition . fade))))
    (glitnir-test--as-child nil
      (should (= (aref (glitnir--chrome-vector (selected-frame)) 8) 1)))
    (glitnir-test--as-child '((transition . nil))
      (should (= (aref (glitnir--chrome-vector (selected-frame)) 8) 0)))))

(ert-deftest glitnir-transition-invalid-signals ()
  (dolist (bad '(slide (:show spin) (:duration -1) (:curve bounce)
                       (:show fade :curve (cubic-bezier 2 0 1 1)) (:show) 3))
    (glitnir-test--as-child `((transition . ,bad))
      (should-error (glitnir--chrome-vector (selected-frame)) :type 'wrong-type-argument))))

(ert-deftest glitnir-backdrop-slot ()
  (glitnir-test--as-child '((backdrop-blur . 10) (alpha-background . 0.8))
    (should (= (aref (glitnir--chrome-vector (selected-frame)) 20) 10)))
  (glitnir-test--as-child '((backdrop-blur . 2.5) (alpha-background . 50))
    (should (= (aref (glitnir--chrome-vector (selected-frame)) 20) 2.5)))
  (glitnir-test--as-child '((alpha-background . 0.8))
    (should (= (aref (glitnir--chrome-vector (selected-frame)) 20) 0)))
  (dolist (opaque '(nil 1.0 100))
    (glitnir-test--as-child `((backdrop-blur . 10) (alpha-background . ,opaque))
      (should (= (aref (glitnir--chrome-vector (selected-frame)) 20) 0))))
  (dolist (bad '(-1 "10" t))
    (glitnir-test--as-child `((backdrop-blur . ,bad))
      (should-error (glitnir--chrome-vector (selected-frame)) :type 'wrong-type-argument))))

(ert-deftest glitnir-transition-off-without-param ()
  (glitnir-test--as-child nil
    (let ((v (glitnir--chrome-vector (selected-frame))))
      (should (cl-every #'zerop (glitnir-test--motion v)))
      (should (zerop (aref v 20))))))

(ert-deftest glitnir-reduced-motion-mid-transition ()
  (let (refreshed)
    (glitnir-test--as-child '((transition . pop))
      (glitnir-test--with-reduced-motion 0
        (should (= (aref (glitnir--chrome-vector (selected-frame)) 8) 2)))
      (glitnir-test--with-reduced-motion 1
        (let ((v (glitnir--chrome-vector (selected-frame))))
          (should (= (aref v 8) 0))
          (should (= (aref v 14) 0))))
      ;; the setting changing runs a refresh
      (cl-letf (((symbol-function 'glitnir-refresh) (lambda (&rest _) (setq refreshed t))))
        (glitnir--settings-changed 'color-scheme 1)
        (should-not refreshed)
        (run-hook-with-args 'nhwl-settings-changed-functions 'reduced-motion 1)
        (should refreshed)))))

;; test-pattern: D (pure)
(ert-deftest glitnir-reduced-motion-override ()
  (glitnir-test--as-child '((transition . pop))
    ;; t forces zero legs with the portal at 0
    (let ((glitnir-reduced-motion t))
      (glitnir-test--with-reduced-motion 0
        (let ((v (glitnir--chrome-vector (selected-frame))))
          (should (= (aref v 8) 0))
          (should (= (aref v 14) 0)))))
    ;; nil forces motion with the portal at 1
    (let ((glitnir-reduced-motion nil))
      (glitnir-test--with-reduced-motion 1
        (let ((v (glitnir--chrome-vector (selected-frame))))
          (should (= (aref v 8) 2))
          (should (= (aref v 14) 1)))))
    ;; auto follows the portal
    (let ((glitnir-reduced-motion 'auto))
      (glitnir-test--with-reduced-motion 1
        (should (= (aref (glitnir--chrome-vector (selected-frame)) 8) 0)))
      (glitnir-test--with-reduced-motion 0
        (should (= (aref (glitnir--chrome-vector (selected-frame)) 8) 2))))))

;; test-pattern: D (pure)
(ert-deftest glitnir-reduced-motion-p-public ()
  "The public predicate answers what the option and the desktop say."
  (dolist (case '((auto 0 nil) (auto 1 t) (t 0 t) (nil 1 nil)))
    (let ((glitnir-reduced-motion (nth 0 case)))
      (glitnir-test--with-reduced-motion (nth 1 case)
        (should (eq (and (glitnir-reduced-motion-p) t) (nth 2 case)))
        (should (eq (and (glitnir--reduced-motion-p) t) (nth 2 case)))))))

;; Window cards: the window parameters resolve into a vector C lists with the window.

(defmacro glitnir-test--with-window-params (params &rest body)
  "Run BODY with the selected window's parameters PARAMS set, then clear them and its card.
Colours are \"#RRGGBB\" specs: the batch terminal may define no colour names."
  (declare (indent 1))
  `(let ((w (selected-window))
         (divider (face-attribute 'window-divider :foreground (selected-frame))))
     (cl-letf (((symbol-function 'color-values)
                (lambda (c &optional _f) (and (stringp c) (color-values-from-color-spec c)))))
       (unwind-protect
           (progn
             (set-face-foreground 'window-divider "#123456" (selected-frame))
             (dolist (p ,params) (set-window-parameter w (car p) (cdr p)))
             ,@body)
         (set-face-attribute 'window-divider (selected-frame) :foreground divider)
         (dolist (k '(corner-radius shadow ring card-group glitnir--card glitnir--card-key))
           (set-window-parameter w k nil))
         (set-frame-parameter nil 'glitnir--cards nil)))))

(ert-deftest glitnir-card-vector-from-window-params ()
  (let ((w (selected-window)))
    (should-not (glitnir--card-vector w))
    (glitnir-test--with-window-params
        '((corner-radius . 10) (shadow . (:blur 3 :offset (0 . 1) :opacity 0.45))
          (ring . ("#ff0000" . 0.6)))
      (set-face-foreground 'child-frame-shadow "#0a0b0c" (selected-frame))
      (unwind-protect
          (let ((v (glitnir--card-vector w)))
            (should (= (length v) (length glitnir--chrome-slots)))
            (should (equal (list (aref v 0) (aref v 1) (aref v 2) (aref v 3) (aref v 4) (aref v 6))
                           '(1 10 3 0 1 0.45)))
            (should (= (aref v 7) #x0a0b0c))
            (should (cl-every #'zerop (cl-subseq v 8 21)))
            ;; the corners show the colour dividers are drawn in
            (should (= (aref v 21) #x123456))
            (should (= (aref v 22) #xff0000))
            (should (= (aref v 23) 0.6))
            ;; the hook keeps it in the window parameter C reads
            (glitnir--recheck-cards w)
            (should (equal (window-parameter w 'glitnir--card) v)))
        (set-face-attribute 'child-frame-shadow (selected-frame) :foreground 'unspecified)))
    ;; a radius alone is a card without shadow; a face colours the ring
    (glitnir-test--with-window-params '((corner-radius . 6) (ring . (window-divider . 2)))
      (let ((v (glitnir--card-vector w)))
        (should (equal (list (aref v 0) (aref v 1) (aref v 6)) '(1 6 0)))
        (should (= (aref v 22) #x123456))
        (should (= (aref v 23) 1))))
    ;; without a divider colour, the frame's background
    (glitnir-test--with-window-params '((corner-radius . 6))
      (set-face-attribute 'window-divider (selected-frame) :foreground 'unspecified)
      (cl-letf (((symbol-function 'frame-parameter)
                 (lambda (_f p) (and (eq p 'background-color) "#202122"))))
        (should (= (aref (glitnir--card-vector w) 21) #x202122))))
    (glitnir-test--with-window-params '((corner-radius . 0))
      (should-not (glitnir--card-vector w)))
    (dolist (bad '(((corner-radius . -1)) ((corner-radius . "8"))
                   ((ring . 3)) ((ring . ("#ff0000" . high)))
                   ((shadow . (:blur)))))
      (glitnir-test--with-window-params bad
        (should-error (glitnir--card-vector w) :type 'wrong-type-argument)))))

(ert-deftest glitnir-card-vector-card-group ()
  "`card-group' (NAME . FILL) gives the `group' slot a number per name and `fill-rgb' the fill's colour; without
one they are 0 and -1, and a group of another shape signals."
  (let ((w (selected-window)) (group (alist-get 'group glitnir--chrome-slots))
        (fill (alist-get 'fill-rgb glitnir--chrome-slots)))
    (glitnir-test--with-window-params '((corner-radius . 6))
      (let ((v (glitnir--card-vector w)))
        (should (= (aref v group) 0))
        (should (= (aref v fill) -1))))
    (glitnir-test--with-window-params '((corner-radius . 6) (card-group . (glitnir-test-a . "#102030")))
      (let ((v (glitnir--card-vector w)) (n nil))
        (should (> (setq n (aref v group)) 0))
        (should (= (aref v fill) #x102030))
        ;; the same name is the same number; another is another
        (should (= (aref (glitnir--card-vector w) group) n))
        (set-window-parameter w 'card-group '(glitnir-test-b . "#102030"))
        (should-not (= (aref (glitnir--card-vector w) group) n))))
    (glitnir-test--with-window-params '((corner-radius . 6) (card-group . (glitnir-test-a . window-divider)))
      (set-face-background 'window-divider "#405060" (selected-frame))
      (unwind-protect (should (= (aref (glitnir--card-vector w) fill) #x405060))
        (set-face-attribute 'window-divider (selected-frame) :background 'unspecified)))
    (glitnir-test--with-window-params '((corner-radius . 6) (card-group . 3))
      (should-error (glitnir--card-vector w) :type 'wrong-type-argument))))

(ert-deftest glitnir-card-vector-follows-theme ()
  (let ((w (selected-window)))
    (custom-declare-theme 'glitnir-test-red nil)
    (custom-declare-theme 'glitnir-test-blue nil)
    (custom-theme-set-faces 'glitnir-test-red '(window-divider ((t :foreground "#ff0000"))))
    (custom-theme-set-faces 'glitnir-test-blue '(window-divider ((t :foreground "#0000ff"))))
    (glitnir-test--with-window-params '((corner-radius . 8))
      ;; the theme's colour, not the frame's own
      (set-face-attribute 'window-divider (selected-frame) :foreground 'unspecified)
      (unwind-protect
          (progn
            (enable-theme 'glitnir-test-red)
            (glitnir--recheck-cards w)
            (should (= (aref (window-parameter w 'glitnir--card) 21) #xff0000))
            (disable-theme 'glitnir-test-red)
            (enable-theme 'glitnir-test-blue)
            ;; enabling the theme resolved the card again, before any redisplay
            (should (= (aref (window-parameter w 'glitnir--card) 21) #x0000ff)))
        (disable-theme 'glitnir-test-red)
        (disable-theme 'glitnir-test-blue)))))

(ert-deftest glitnir-cards-frame-flag-set-only-on-change ()
  (let ((sets 0)
        (set-frame-parameter (symbol-function 'set-frame-parameter))
        (w (selected-window))
        w2)
    (cl-letf (((symbol-function 'set-frame-parameter)
               (lambda (f p v)
                 (when (eq p 'glitnir--cards) (setq sets (1+ sets)))
                 (funcall set-frame-parameter f p v))))
      (unwind-protect
          (progn
            (set-window-parameter w 'corner-radius 8)
            (glitnir--recheck-cards w)
            (should (eq (frame-parameter nil 'glitnir--cards) t))
            (should (= sets 1))
            ;; nothing changed: nothing is set
            (glitnir--recheck-cards w)
            (glitnir--recheck-frame-cards w)
            (should (= sets 1))
            ;; a second card: the flag is t already
            (setq w2 (split-window w))
            (set-window-parameter w2 'shadow '(:blur 4))
            (glitnir--recheck-cards w2)
            (should (= sets 1))
            ;; the first loses its card, the second keeps the flag
            (set-window-parameter w 'corner-radius nil)
            (glitnir--recheck-cards w)
            (should-not (window-parameter w 'glitnir--card))
            (should (= sets 1))
            ;; the last one goes
            (set-window-parameter w2 'shadow nil)
            (glitnir--recheck-cards w2)
            (should-not (frame-parameter nil 'glitnir--cards))
            (should (= sets 2)))
        (when (window-live-p w2) (delete-window w2))
        (dolist (k '(corner-radius shadow glitnir--card glitnir--card-key))
          (set-window-parameter w k nil))
        (funcall set-frame-parameter nil 'glitnir--cards nil)))))

(defmacro glitnir-test--with-card-windows (n &rest body)
  "Run BODY with N windows in the selected frame, every one a card whose vector is resolved.
The windows are deleted and the card parameters and flag cleared after."
  (declare (indent 1))
  `(let ((ws (list (selected-window))) (window-min-height 1) (window-min-width 1))
     (unwind-protect
         (progn
           (clrhash glitnir--cards-seen)
           (while (< (length ws) ,n) (push (split-window (car ws) nil 'right) ws))
           (dolist (w ws)
             (set-window-parameter w 'corner-radius 8)
             (set-window-parameter w 'shadow '(:blur 3 :offset (0 . 1) :opacity 0.45))
             (set-window-parameter w 'ring '("#ff0000" . 0.6)))
           (cl-letf (((symbol-function 'color-values)
                      (lambda (c &optional _f) (and (stringp c) (color-values-from-color-spec c)))))
             (glitnir--recheck-frame-cards (frame-first-window))
             ,@body))
       (dolist (w ws)
         (when (window-live-p w)
           (dolist (k '(corner-radius shadow ring card-group glitnir--card glitnir--card-key))
             (set-window-parameter w k nil))))
       (delete-other-windows)
       (clrhash glitnir--cards-seen)
       (set-frame-parameter nil 'glitnir--cards nil))))

(ert-deftest glitnir-block-vector-from-spec ()
  "A block spec (KIND START END . PLIST) resolves into the vector C reads: the kind, the markers, the inset, the
radius, the ring's colour from a face's foreground and the colour beneath; a bad shape signals."
  (with-temp-buffer
    (insert "one\ntwo\nthree\n")
    (let* ((s (copy-marker 5)) (e (copy-marker 13))
           ;; colours the batch terminal has exactly (it rounds others to its palette)
           (v (glitnir--block-vector `(ring ,s ,e :x0 40 :x1 300 :radius 6 :ring ("#ff0000" . 1) :beneath "#0000ff")
                                     (selected-frame))))
      (should (equal (append v nil) (list 0 s e 40 300 6 #xff0000 1 #x0000ff)))
      (should (= (aref (glitnir--block-vector `(rule ,s ,e :ring ("#ff0000" . 0.5)) (selected-frame)) 0) 1))
      ;; a bar (a selected row's accent): kind 2, its x0..x1 the bar's width
      (should (equal (append (glitnir--block-vector `(bar ,s ,e :x1 3 :ring ("#ff0000" . 1)) (selected-frame)) nil)
                     (list 2 s e 0 3 0 #xff0000 1 (aref (glitnir--block-vector `(bar ,s ,e) (selected-frame)) 8))))
      ;; no ring: -1; no beneath: the frame's background
      (cl-letf (((symbol-function 'frame-parameter)
                 (lambda (_f p) (and (eq p 'background-color) "#00ff00"))))
        (let ((v (glitnir--block-vector `(ring ,s ,e) (selected-frame))))
          (should (= (aref v 6) -1))
          (should (= (aref v 8) #x00ff00))))
      (dolist (bad (list '(box 1 2) '(ring 1) 3 `(ring ,s ,e :ring 3)))
        (should-error (glitnir--block-vector bad (selected-frame)) :type 'wrong-type-argument)))))

(ert-deftest glitnir-blocks-set-only-on-change ()
  "The redisplay hook resolves a window's `glitnir-blocks' into `glitnir--blocks' once per change of the spec (or
the theme's generation), and the frame's `glitnir--blocks' flag follows whether any window has blocks."
  (let ((w (selected-window)) (sets 0)
        (set-window-parameter (symbol-function 'set-window-parameter)))
    (with-temp-buffer
      (insert "one\ntwo\n")
      (cl-letf (((symbol-function 'set-window-parameter)
                 (lambda (win p v)
                   (when (eq p 'glitnir--blocks) (setq sets (1+ sets)))
                   (funcall set-window-parameter win p v))))
        (unwind-protect
            (let ((spec `((ring ,(copy-marker 1) ,(copy-marker 5) :radius 6 :ring ("#405060" . 1) :beneath "#102030"))))
              (set-window-parameter w 'glitnir-blocks spec)
              (glitnir--recheck-frame-cards w)
              (should (vectorp (window-parameter w 'glitnir--blocks)))
              (should (eq (frame-parameter nil 'glitnir--blocks) t))
              (should (= sets 1))
              ;; nothing changed: nothing is set
              (glitnir--recheck-frame-cards w)
              (glitnir--recheck-frame-cards w)
              (should (= sets 1))
              ;; a theme change (a new generation) resolves it again; the same colours set nothing
              (setq glitnir--card-generation (1+ glitnir--card-generation))
              (glitnir--recheck-frame-cards w)
              (should (= sets 1))
              ;; a new spec: set once
              (set-window-parameter w 'glitnir-blocks (list (car spec) (list 'rule (copy-marker 5) (copy-marker 9))))
              (glitnir--recheck-frame-cards w)
              (should (= sets 2))
              (should (= (length (window-parameter w 'glitnir--blocks)) 2))
              ;; none: cleared, and the frame's flag with it
              (set-window-parameter w 'glitnir-blocks nil)
              (glitnir--recheck-frame-cards w)
              (should-not (window-parameter w 'glitnir--blocks))
              (should-not (frame-parameter nil 'glitnir--blocks))
              ;; no blocks left: no key, and a theme change leaves the window's parameters alone
              (should-not (window-parameter w 'glitnir--blocks-key))
              (setq glitnir--card-generation (1+ glitnir--card-generation))
              (glitnir--recheck-frame-cards w)
              (should-not (window-parameter w 'glitnir--blocks-key))
              ;; a window that never had blocks gets no key either
              (let ((other (split-window w)))
                (unwind-protect
                    (progn (glitnir--recheck-frame-cards other)
                           (should-not (assq 'glitnir--blocks-key (window-parameters other))))
                  (delete-window other))))
          (dolist (k '(glitnir-blocks glitnir--blocks glitnir--blocks-key))
            (set-window-parameter w k nil))
          (clrhash glitnir--blocks-seen)
          (set-frame-parameter nil 'glitnir--blocks nil))))))

(ert-deftest glitnir-block-ring-follows-theme ()
  "A ring given as a face takes the theme's colour again after a theme change."
  (let ((w (selected-window)))
    (custom-declare-theme 'glitnir-test-red nil)
    (custom-declare-theme 'glitnir-test-blue nil)
    (custom-theme-set-faces 'glitnir-test-red '(window-divider ((t :foreground "#ff0000"))))
    (custom-theme-set-faces 'glitnir-test-blue '(window-divider ((t :foreground "#0000ff"))))
    (with-temp-buffer
      (insert "one\n")
      (set-face-attribute 'window-divider (selected-frame) :foreground 'unspecified)
      (unwind-protect
          (progn
            (set-window-parameter w 'glitnir-blocks `((ring ,(copy-marker 1) ,(copy-marker 4) :ring window-divider)))
            (enable-theme 'glitnir-test-red)
            (glitnir--recheck-frame-cards w)
            (should (= (aref (aref (window-parameter w 'glitnir--blocks) 0) 6) #xff0000))
            (disable-theme 'glitnir-test-red)
            (enable-theme 'glitnir-test-blue)
            (glitnir--recheck-frame-cards w)
            (should (= (aref (aref (window-parameter w 'glitnir--blocks) 0) 6) #x0000ff)))
        (disable-theme 'glitnir-test-red)
        (disable-theme 'glitnir-test-blue)
        (dolist (k '(glitnir-blocks glitnir--blocks glitnir--blocks-key))
          (set-window-parameter w k nil))
        (clrhash glitnir--blocks-seen)
        (set-frame-parameter nil 'glitnir--blocks nil)))))

(ert-deftest glitnir-edge-vector-from-spec ()
  "An edge spec as the vector C reads: flags, colour, the number of points, then each (POS DX DY); a place is a
position or (POS DX DY); bends go between FROM and TO; more than 8 points or a bad place signal."
  (with-temp-buffer
    (insert "one\ntwo\nthree\n")
    (let* ((m (copy-marker 5))
           (v (glitnir--edge-vector `((1 3 4) ,m ("#ff0000" . 0.5) :points ((9 0 2)) :arrow t) (selected-frame))))
      (should (equal (append v nil) (list 1 #xff0000 0.5 3 1 3 4 9 0 2 m 0 0)))
      (should (= (aref (glitnir--edge-vector '(1 5 "#00ff00" :dash t) (selected-frame)) 0) 2))
      (should (= (aref (glitnir--edge-vector '(1 5 "#00ff00" :dash t :arrow t) (selected-frame)) 0) 3))
      (should-not (glitnir--edge-vector '(1 5 "no-such-colour") (selected-frame)))
      (should-error (glitnir--edge-vector `(1 5 "#00ff00" :points ,(make-list 7 2)) (selected-frame))
                    :type 'wrong-type-argument)
      (dolist (bad '(nil (1) (1 5) ("x" 5 "#00ff00") (1 (5 "a") "#00ff00")))
        (should-error (glitnir--edge-vector bad (selected-frame)) :type 'wrong-type-argument)))))

(ert-deftest glitnir-edges-resolve-only-on-change ()
  "A window's `glitnir-edges' is resolved into `glitnir--edges' once per change of the spec or the theme's
generation, and sets the frame's `glitnir--blocks' flag, so C lists the frame; edges gone, both go."
  (let ((w (selected-window)) (sets 0)
        (set-window-parameter (symbol-function 'set-window-parameter)))
    (with-temp-buffer
      (insert "one\ntwo\nthree\n")
      (cl-letf (((symbol-function 'set-window-parameter)
                 (lambda (win p v)
                   (when (eq p 'glitnir--edges) (setq sets (1+ sets)))
                   (funcall set-window-parameter win p v))))
        (unwind-protect
            (let ((spec '((1 5 ("#ff0000" . 1) :arrow t))))
              (set-window-parameter w 'glitnir-edges spec)
              (glitnir--recheck-frame-cards w)
              (should (equal (window-parameter w 'glitnir--edges) (vector (vector 1 #xff0000 1 2 1 0 0 5 0 0))))
              (should (eq (frame-parameter nil 'glitnir--blocks) t))
              (glitnir--recheck-frame-cards w)
              (setq glitnir--card-generation (1+ glitnir--card-generation))
              (glitnir--recheck-frame-cards w)
              (should (= sets 1))
              (set-window-parameter w 'glitnir-edges '((1 5 ("#ff0000" . 1))))
              (glitnir--recheck-frame-cards w)
              (should (= sets 2))
              (set-window-parameter w 'glitnir-edges nil)
              (glitnir--recheck-frame-cards w)
              (should-not (window-parameter w 'glitnir--edges))
              (should-not (frame-parameter nil 'glitnir--blocks)))
          (dolist (k '(glitnir-edges glitnir--edges glitnir--edges-key))
            (set-window-parameter w k nil))
          (clrhash glitnir--blocks-seen)
          (set-frame-parameter nil 'glitnir--blocks nil))))))

(ert-deftest glitnir-ruler-resolves-only-on-change ()
  "A window's `glitnir-ruler' is resolved into `glitnir--ruler' once per change of the spec or the theme's
generation, never per redisplay, and sets the frame's `glitnir--blocks' flag, so C lists the frame."
  (let ((w (selected-window)) (sets 0)
        (set-window-parameter (symbol-function 'set-window-parameter)))
    (with-temp-buffer
      (insert "one\ntwo\nthree\n")
      (cl-letf (((symbol-function 'set-window-parameter)
                 (lambda (win p v)
                   (when (eq p 'glitnir--ruler) (setq sets (1+ sets)))
                   (funcall set-window-parameter win p v))))
        (unwind-protect
            (let ((spec `(:width 12 :right 0 :height 2 :band ("#808080" . 0.25)
                                 :marks ((,(copy-marker 5) 3 9 ("#ff0000" . 1)) (9 0 3 ("#00ff00" . 0.5))))))
              (set-window-parameter w 'glitnir-ruler spec)
              (glitnir--recheck-frame-cards w)
              (should (equal (window-parameter w 'glitnir--ruler)
                             (vector 12 0 2 #x808080 0.25 (vector (car (nth 0 (plist-get spec :marks))) 3 9 #xff0000 1)
                                     (vector 9 0 3 #x00ff00 0.5))))
              (should (eq (frame-parameter nil 'glitnir--blocks) t))
              (should (= sets 1))
              (glitnir--recheck-frame-cards w)
              (glitnir--recheck-frame-cards w)
              (should (= sets 1))
              ;; a new generation with the same colours sets nothing; a new spec sets once
              (setq glitnir--card-generation (1+ glitnir--card-generation))
              (glitnir--recheck-frame-cards w)
              (should (= sets 1))
              (set-window-parameter w 'glitnir-ruler (plist-put (copy-sequence spec) :marks nil))
              (glitnir--recheck-frame-cards w)
              (should (= sets 2))
              (should (= (length (window-parameter w 'glitnir--ruler)) 5))
              (set-window-parameter w 'glitnir-ruler nil)
              (glitnir--recheck-frame-cards w)
              (should-not (window-parameter w 'glitnir--ruler))
              (should-not (frame-parameter nil 'glitnir--blocks)))
          (dolist (k '(glitnir-ruler glitnir--ruler glitnir--ruler-key))
            (set-window-parameter w k nil))
          (clrhash glitnir--blocks-seen)
          (set-frame-parameter nil 'glitnir--blocks nil))))))

(ert-deftest glitnir-ruler-faces-through-the-theme ()
  "A ruler mark given as a face takes the theme's colour again after a theme change."
  (let ((w (selected-window)))
    (custom-declare-theme 'glitnir-test-red nil)
    (custom-declare-theme 'glitnir-test-blue nil)
    (custom-theme-set-faces 'glitnir-test-red '(window-divider ((t :foreground "#ff0000"))))
    (custom-theme-set-faces 'glitnir-test-blue '(window-divider ((t :foreground "#0000ff"))))
    (with-temp-buffer
      (insert "one\n")
      (set-face-attribute 'window-divider (selected-frame) :foreground 'unspecified)
      (unwind-protect
          (progn
            (set-window-parameter w 'glitnir-ruler '(:width 12 :marks ((1 0 12 window-divider))))
            (enable-theme 'glitnir-test-red)
            (glitnir--recheck-frame-cards w)
            (should (= (aref (aref (window-parameter w 'glitnir--ruler) 5) 3) #xff0000))
            (disable-theme 'glitnir-test-red)
            (enable-theme 'glitnir-test-blue)
            (glitnir--recheck-frame-cards w)
            (should (= (aref (aref (window-parameter w 'glitnir--ruler) 5) 3) #x0000ff)))
        (disable-theme 'glitnir-test-red)
        (disable-theme 'glitnir-test-blue)
        (dolist (k '(glitnir-ruler glitnir--ruler glitnir--ruler-key))
          (set-window-parameter w k nil))
        (clrhash glitnir--blocks-seen)
        (set-frame-parameter nil 'glitnir--blocks nil)))))

(defun glitnir-test--recheck-n (n window)
  "Run the card hook N times on WINDOW (compiled by the test: an interpreted loop conses by itself)."
  (dotimes (_ n) (glitnir--recheck-frame-cards window)))

(ert-deftest glitnir-card-recheck-unchanged-redisplay-allocates-nothing ()
  "Pattern D (pure).  Over 100 passes of the redisplay hook with five card windows and nothing changed, no face
is looked up and (within a stated bound) nothing is consed: the hook compares the window parameters with
the generation it resolved them at."
  (glitnir-test--with-card-windows 5
    (should (= (length (window-list)) 5))
    (let ((looked-up 0) (face-foreground (symbol-function 'face-foreground)) before conses)
      (cl-letf (((symbol-function 'face-foreground)
                 (lambda (&rest args) (setq looked-up (1+ looked-up)) (apply face-foreground args))))
        (byte-compile 'glitnir-test--recheck-n)
        (setq before (car (memory-use-counts)))
        (glitnir-test--recheck-n 100 (frame-first-window))
        (setq conses (- (car (memory-use-counts)) before)))
      (should (= looked-up 0))
      (should (<= conses 20)))))               ; the 100 passes cost no conses; slack for the counter's own list

(ert-deftest glitnir-card-flag-clears-when-the-last-card-window-goes ()
  "Pattern D (pure).  Deleting the only card window leaves no card, so the next pass of the redisplay hook clears
the frame's `glitnir--cards' flag (C lists every window at every publish while it is set)."
  (glitnir-test--with-card-windows 2
    (should (eq (frame-parameter nil 'glitnir--cards) t))
    (let ((card (frame-first-window)))
      (set-window-parameter (next-window card nil nil) 'glitnir--card nil)
      (dolist (k '(corner-radius shadow ring))
        (set-window-parameter (next-window card nil nil) k nil))
      (glitnir--recheck-frame-cards (frame-first-window))
      (should (eq (frame-parameter nil 'glitnir--cards) t))
      (delete-window card)
      (glitnir--recheck-frame-cards (frame-first-window))
      (should-not (frame-parameter nil 'glitnir--cards)))))

(defmacro glitnir-test--with-leg-params (params &rest body)
  "Run BODY with the selected window's layout leg parameters PARAMS set, then clear them.
Colours are \"#RRGGBB\" specs: the batch terminal may define no colour names."
  (declare (indent 1))
  `(let ((w (selected-window)))
     (cl-letf (((symbol-function 'color-values)
                (lambda (c &optional _f) (and (stringp c) (color-values-from-color-spec c)))))
       (unwind-protect
           (progn
             (dolist (p ,params) (set-window-parameter w (car p) (cdr p)))
             ,@body)
         (dolist (k '(glitnir-layout-capture glitnir-layout-motion glitnir--leg-sent))
           (set-window-parameter w k nil))))))

(ert-deftest glitnir-leg-vector-from-params ()
  "The leg vector carries the capture, the text area and the motion's items."
  ;; pattern D: pure
  (glitnir-test--with-leg-params
      '((glitnir-layout-capture . t)
        (glitnir-layout-motion . (:moves ((a (1 2 3 4) (5 6 3 4))) :enters ((b (0 0 2 2)))
                                  :exits ((c (9 9 1 1))) :background "#102030"
                                  :duration 0.2 :curve linear :step t)))
    (let* ((w (selected-window))
           (edges (window-body-pixel-edges w))
           (v (glitnir--leg-vector w 3 t)))
      (should (= (length v) (+ 14 (* 3 10))))
      (should (equal (append (cl-subseq v 0 5) nil)
                     (list 3 (nth 0 edges) (nth 1 edges)
                           (- (nth 2 edges) (nth 0 edges)) (- (nth 3 edges) (nth 1 edges)))))
      (should (equal (append (cl-subseq v 5 14) nil) (list 3 #x102030 200 0 0 1 1 1 3)))
      (should (equal (append (cl-subseq v 14 24) nil)
                     (list 1 (glitnir--leg-id 'a) 1 2 3 4 5 6 3 4)))
      (should (equal (append (cl-subseq v 24 34) nil) (list 2 (glitnir--leg-id 'b) 0 0 0 0 0 0 2 2)))
      (should (equal (append (cl-subseq v 34 44) nil) (list 3 (glitnir--leg-id 'c) 9 9 1 1 0 0 0 0)))
      ;; a capture alone: no motion, no items
      (let ((c (glitnir--leg-vector w 4 nil)))
        (should (= (length c) 14))
        (should (equal (list (aref c 0) (aref c 5) (aref c 13)) '(4 0 0))))
      ;; the defaults: 0.16 s, ease-out, the default face's background
      (set-window-parameter w 'glitnir-layout-motion '(:moves ((a (1 2 3 4) (5 6 3 4)))))
      (let ((d (glitnir--leg-vector w 5 t)))
        (should (equal (append (cl-subseq d 7 13) nil) '(160 0 0 0.58 1 0))))
      (set-window-parameter w 'glitnir-layout-motion '(:moves ((a (1 2) (3 4)))))
      (should-error (glitnir--leg-vector w 6 t) :type 'wrong-type-argument))))

(ert-deftest glitnir-leg-vector-nil-under-reduced-motion ()
  "Under reduced motion no capture goes out, so the layout jumps."
  ;; pattern D: pure
  (glitnir-test--with-leg-params '((glitnir-layout-capture . t))
    (let ((w (selected-window)))
      (let ((glitnir-reduced-motion nil))
        (should (glitnir--leg-to-send w)))
      (let ((glitnir-reduced-motion t))
        (should-not (glitnir--leg-to-send w))
        (should-not (glitnir-layout-captured-p w)))
      ;; the batch frame has no presenter: never acknowledged
      (let ((glitnir-reduced-motion nil))
        (should-not (glitnir-layout-captured-p w))))))

;;; glitnir-tests.el ends here
