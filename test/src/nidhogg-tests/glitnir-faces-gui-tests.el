;;; glitnir-faces-gui-tests.el --- GUI rows for merging faces that set font attributes  -*- lexical-binding: t -*-

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

;;; Commentary:

;; Run by nidhogg/test/gui.sh.  A face with a font attribute of its own
;; (a float :height, a family, a weight) clears that attribute from the
;; font spec of the face it is merged into.  The first row counts the
;; vector cells a forced redisplay allocates for such faces; the second
;; compares the merged attributes and the realized fonts over a matrix
;; of faces with what the stock merge gave, recorded in
;; fixtures/face-merge-matrix.eld.  Each row drives redisplay in order,
;; so nothing in it depends on timing.

;;; Code:

(require 'ert)
(require 'seq)
(require 'face-remap)

(defconst glitnir-faces-gui--fixture
  (expand-file-name "fixtures/face-merge-matrix.eld"
                    (file-name-directory (or load-file-name buffer-file-name)))
  "The matrix's results on the stock merge: (CASE HASH) per case, HASH the SHA-256 of the printed result.")

(defconst glitnir-faces-gui--steps-fixture
  (expand-file-name "fixtures/face-remap-steps.eld"
                    (file-name-directory (or load-file-name buffer-file-name)))
  "The remap steps' results on the stock merge: (STEP HASH) per step, HASH the SHA-256 of the printed result.")

(defun glitnir-faces-gui--check (results fixture tag)
  "RESULTS, a list of (KEY . RESULT), are the ones recorded in FIXTURE, a list of (KEY HASH).
Without FIXTURE, print each result after TAG and fail."
  (let ((expected (and (file-readable-p fixture)
                       (with-temp-buffer
                         (insert-file-contents fixture)
                         (read (current-buffer))))))
    (unless expected
      (dolist (r results) (glitnir-gui--say "%s: %S" tag r))
      (ert-fail (list "no recorded results" fixture)))
    (should (= (length results) (length expected)))
    ;; A result that differs shows whole.
    (should-not (seq-remove (lambda (r)
                              (equal (cadr (assq (car r) expected))
                                     (secure-hash 'sha256 (prin1-to-string r))))
                            results))))

(defun glitnir-faces-gui--cells (fn)
  "The vector cells FN allocates."
  (let ((before (nth 2 (memory-use-counts))))
    (funcall fn)
    (- (nth 2 (memory-use-counts)) before)))

(defun glitnir-faces-gui--face (name &rest plist)
  "Face NAME with the attributes PLIST on every frame; return NAME."
  (make-face name)
  (apply #'set-face-attribute name nil plist)
  name)

(defun glitnir-faces-gui--redisplay-cells (face remap)
  "Vector cells one forced redisplay allocates for 40 lines in FACE, `default' remapped to REMAP (nil: not)."
  (let ((buf (generate-new-buffer " *glitnir-faces*")))
    (unwind-protect
        (save-window-excursion
          (delete-other-windows)
          (switch-to-buffer buf)
          (setq-local mode-line-format nil)
          (setq-local header-line-format nil)
          (dotimes (i 40)
            (insert (propertize (format "run %d " i) 'face face) "plain "
                    (propertize "tail" 'face face) "\n"))
          (goto-char (point-min))
          (when remap (face-remap-add-relative 'default remap))
          (let ((forced (lambda () (force-window-update (selected-window)) (redisplay t))))
            (funcall forced)            ; the faces are realized here
            (funcall forced)
            (glitnir-faces-gui--cells forced)))
      (kill-buffer buf))))

(ert-deftest glitnir-faces-gui/a-merge-copies-the-font-spec-once ()
  "Pattern D (pure): three font attributes of a face's own cost a redisplay what one costs.
Every attribute a merge cleared from the font spec copied the spec
again: up to three copies per face run, and as many again at every
face lookup in a buffer whose `default' is remapped.  The two buffers
lay out alike (the same height and the same font: the family is the
default's, the weight a synonym of the default's), so they look up as
many faces; only the copies differ."
  (let* ((family (copy-sequence (face-attribute 'default :family)))
         (weight (if (eq (face-attribute 'default :weight) 'normal) 'regular 'normal))
         (one (glitnir-faces-gui--face 'glitnir-faces-gui--one :height 0.889))
         (three (glitnir-faces-gui--face 'glitnir-faces-gui--three
                                         :height 0.889 :family family :weight weight))
         (remap-one (glitnir-faces-gui--face 'glitnir-faces-gui--remap-one :height 0.963))
         (remap-three (glitnir-faces-gui--face 'glitnir-faces-gui--remap-three
                                               :height 0.963 :family family :weight weight))
         (spec (glitnir-faces-gui--cells #'font-spec))
         (cells-one (glitnir-faces-gui--redisplay-cells one remap-one))
         (cells-three (glitnir-faces-gui--redisplay-cells three remap-three)))
    (glitnir-gui--say "face-merge cells: one %d, three %d, a font spec %d" cells-one cells-three spec)
    (should (> spec 0))
    (should (<= (- cells-three cells-one) (* 2 spec)))))

(defun glitnir-faces-gui--taller (height)
  "HEIGHT and 7 more: a :height function."
  (+ height 7))

(defun glitnir-faces-gui--cases ()
  "The matrix, a list of (HEIGHT FAMILY WEIGHT INHERIT REMAP)."
  (let (cases)
    (dolist (height '(130 0.889 glitnir-faces-gui--taller))
      (dolist (family '(nil "Monospace"))
        (dolist (weight '(nil bold))
          (dolist (inherit '(nil glitnir-faces-gui--parent))
            (dolist (remap '(nil glitnir-faces-gui--remap))
              (push (list height family weight inherit remap) cases))))))
    (nreverse cases)))

(defun glitnir-faces-gui--observe (face remap spec)
  "What merging FACE gives in a buffer whose `default' is remapped to REMAP (nil: not).
A list of three strings: the attribute vectors of SPEC with FACE merged
in and with `default' merged in, and the font a redisplay realizes for
text in FACE."
  (let ((buf (generate-new-buffer " *glitnir-faces-matrix*")))
    (unwind-protect
        (save-window-excursion
          (delete-other-windows)
          (switch-to-buffer buf)
          (when remap (face-remap-add-relative 'default remap))
          (insert (propertize "x" 'face face) "\n")
          (redisplay t)
          (list (prin1-to-string (face-attributes-as-vector (list :font spec :inherit face)))
                (prin1-to-string (face-attributes-as-vector (list :font spec :inherit 'default)))
                (prin1-to-string (font-at 1))))
      (kill-buffer buf))))

(ert-deftest glitnir-faces-gui/merged-faces-and-fonts-are-the-stock-ones ()
  "Pattern D (pure): the merge gives the stock merge's attributes and fonts.
A matrix of faces: :height an integer, a float or a function; a family
or none; a weight or none; inheriting a face or not; in a buffer with
`default' remapped or not.  Each case's merged attribute vector and
realized font equal the ones recorded on the stock merge, and the font
spec merged into stays as it was."
  (glitnir-faces-gui--face 'glitnir-faces-gui--parent :slant 'italic :height 1.1 :weight 'semibold)
  (glitnir-faces-gui--face 'glitnir-faces-gui--remap :family "Monospace" :height 0.963 :weight 'light)
  (let* ((spec (font-spec :name "Monospace-14" :family "Monospace" :size 14 :weight 'normal
                          :slant 'normal :width 'normal :registry 'iso10646-1 :spacing 100))
         (pristine (prin1-to-string spec))
         (n 0) results)
    (pcase-dolist (`(,height ,family ,weight ,inherit ,remap) (glitnir-faces-gui--cases))
      (let ((face (intern (format "glitnir-faces-gui--case-%d" n))))
        (apply #'glitnir-faces-gui--face face :height height
               (append (and family (list :family family))
                       (and weight (list :weight weight))
                       (and inherit (list :inherit inherit))))
        (push (cons n (glitnir-faces-gui--observe face remap spec)) results)
        (setq n (1+ n))))
    (should (equal (prin1-to-string spec) pristine))
    (glitnir-faces-gui--check (nreverse results) glitnir-faces-gui--fixture "face-merge-matrix")))

(ert-deftest glitnir-faces-gui/a-remapped-default-costs-what-the-default-does ()
  "Pattern D (pure): a buffer whose `default' is remapped to a face with font attributes redisplays for what it does
without the remap.  Every face lookup in a remapped buffer merged the remap into the default face again, copying the
font spec each time.  The remap face has the default's family, a synonym of its weight and :height 1.0, so the two
buffers lay out alike."
  (let* ((family (copy-sequence (face-attribute 'default :family)))
         (weight (if (eq (face-attribute 'default :weight) 'normal) 'regular 'normal))
         (remap (glitnir-faces-gui--face 'glitnir-faces-gui--remap-same
                                         :height 1.0 :family family :weight weight))
         (spec (glitnir-faces-gui--cells #'font-spec))
         (plain (glitnir-faces-gui--redisplay-cells nil nil))
         (remapped (glitnir-faces-gui--redisplay-cells nil remap)))
    (glitnir-gui--say "face-remap cells: plain %d, remapped %d, a font spec %d" plain remapped spec)
    (should (<= (- remapped plain) (* 2 spec)))))

(defun glitnir-faces-gui--fonts ()
  "Redisplay; the fonts of the plain text at 1 and of the face run at 3, printed."
  (redisplay t)
  (list (prin1-to-string (font-at 1)) (prin1-to-string (font-at 3))))

(ert-deftest glitnir-faces-gui/remap-changes-give-the-stock-fonts ()
  "Pattern D (pure): the fonts after each change of a `default' remap are the stock merge's.
The remap is added, redisplayed twice, its face's weight and height change, `text-scale-set' scales it and back, it
is removed, and a property-list remap comes and goes."
  (glitnir-faces-gui--face 'glitnir-faces-gui--step-remap :family "Monospace" :height 0.963 :weight 'light)
  (glitnir-faces-gui--face 'glitnir-faces-gui--step-run :height 0.889 :weight 'bold)
  (let ((buf (generate-new-buffer " *glitnir-faces-steps*"))
        steps)
    (unwind-protect
        (save-window-excursion
          (delete-other-windows)
          (switch-to-buffer buf)
          (insert "ab" (propertize "cd" 'face 'glitnir-faces-gui--step-run) "\n")
          (push (cons 'none (glitnir-faces-gui--fonts)) steps)
          (let ((cookie (face-remap-add-relative 'default 'glitnir-faces-gui--step-remap)))
            (push (cons 'added (glitnir-faces-gui--fonts)) steps)
            (push (cons 'again (glitnir-faces-gui--fonts)) steps)
            (set-face-attribute 'glitnir-faces-gui--step-remap nil :weight 'bold)
            (push (cons 'weight (glitnir-faces-gui--fonts)) steps)
            (set-face-attribute 'glitnir-faces-gui--step-remap nil :height 1.2)
            (push (cons 'height (glitnir-faces-gui--fonts)) steps)
            (text-scale-set 2)
            (push (cons 'scaled (glitnir-faces-gui--fonts)) steps)
            (text-scale-set 0)
            (push (cons 'unscaled (glitnir-faces-gui--fonts)) steps)
            (face-remap-remove-relative cookie))
          (push (cons 'removed (glitnir-faces-gui--fonts)) steps)
          (let ((cookie (face-remap-add-relative 'default :family "Monospace" :height 1.2 :weight 'semibold)))
            (push (cons 'plist (glitnir-faces-gui--fonts)) steps)
            (face-remap-remove-relative cookie))
          (push (cons 'removed-again (glitnir-faces-gui--fonts)) steps))
      (kill-buffer buf))
    (glitnir-faces-gui--check (nreverse steps) glitnir-faces-gui--steps-fixture "face-remap-steps")))

;;; glitnir-faces-gui-tests.el ends here
