;;; workers-ts-gui-tests.el --- GUI tests for workers: ts  -*- lexical-binding: t -*-

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

;; Run by nidhogg/test/gui.sh (gui-runner.el loads every *-gui-tests.el).

;; The rows need the C grammar: NIDHOGG_TEST_GRAMMARS, or
;; fixtures/grammars-cache/ beside this file (fixtures/grammars.sh builds
;; it); without one they skip.  They wait on events (pause points,
;; `--test-await-landed'), never on time.

(defconst workers-ts-gui--dir
  (file-name-directory (or load-file-name buffer-file-name "")))

(defun workers-ts-gui--grammar-dir ()
  "The directory holding libtree-sitter-c.so, or nil."
  (let* ((dir (or (getenv "NIDHOGG_TEST_GRAMMARS")
                  (expand-file-name "fixtures/grammars-cache"
                                    workers-ts-gui--dir)))
         (lib (expand-file-name "libtree-sitter-c.so" dir)))
    (and (file-exists-p lib) dir)))

(defun workers-ts-gui--c-text (bytes)
  "A C source text of about BYTES bytes."
  (let ((chunk "int f (int a, int b) { if (a > b) return a * 2; else return b + a; }\n"))
    (apply #'concat (make-list (/ bytes (length chunk)) chunk))))

(defun workers-ts-gui--positions (face beg end)
  "The positions between BEG and END whose `face' includes FACE."
  (let (out)
    (dotimes (i (- end beg))
      (when (memq face (ensure-list (get-text-property (+ beg i) 'face)))
        (push (+ beg i) out)))
    (nreverse out)))

(defmacro workers-ts-gui--with-file (spec &rest body)
  "Bind (FILE BYTES) in SPEC to a new C file of BYTES bytes, run BODY.
With the C grammar on the load path, `c-ts-mode' for .c files,
`nidhogg-workers-treesit' and `nidhogg-treesit-defer' on, in threads
mode.  Afterwards every parse lands and the file and its buffer go."
  (declare (indent 1))
  (let ((file (car spec)) (bytes (cadr spec)))
    `(let* ((dir (workers-ts-gui--grammar-dir))
            (treesit-extra-load-path (list dir))
            (auto-mode-alist (cons '("\\.c\\'" . c-ts-mode) auto-mode-alist))
            (c-ts-mode-emacs-sources-support nil)
            (treesit-auto-install-grammar 'never)
            (nidhogg-workers--mode 'threads)
            (nidhogg-workers-treesit t)
            (nidhogg-treesit-defer t)
            (,file (make-temp-file "workers-ts-gui" nil ".c"
                                   (workers-ts-gui--c-text ,bytes))))
       (unwind-protect
           (progn ,@body)
         (nidhogg-workers--release)
         (nidhogg-workers--test-await-landed 'nh-parse 30000)
         (let ((buf (get-file-buffer ,file)))
           (when buf
             (with-current-buffer buf
               (set-buffer-modified-p nil))
             (kill-buffer buf)))
         (delete-file ,file)
         (redisplay t)))))

(defun workers-ts-gui--sync-keywords (file end)
  "The keyword positions below END of FILE fontified synchronously."
  (with-temp-buffer
    (insert-file-contents file)
    (let ((nidhogg-treesit-defer nil))
      (c-ts-mode)
      (font-lock-ensure (point-min) (min (point-max) (+ end 2000)))
      (workers-ts-gui--positions 'font-lock-keyword-face (point-min) end))))

(ert-deftest workers-gui/ts-open-defers ()
  ;; Open a 2 MB C file with nh-parse parked at `before-done': the first
  ;; redisplay draws the text uncoloured and records it; release, await
  ;; the landing, redisplay: the visible region has
  ;; `font-lock-keyword-face' exactly where a synchronous run has it.
  (skip-unless (and (display-graphic-p) (featurep 'nidhogg-workers)
                    (workers-ts-gui--grammar-dir)))
  (workers-ts-gui--with-file (file 2000000)
    (nidhogg-workers--pause-at 'before-done)
    (let ((buf (find-file-noselect file)))
      (should (eq (buffer-local-value 'major-mode buf) 'c-ts-mode))
      (delete-other-windows)
      (switch-to-buffer buf)
      (goto-char (point-min))
      ;; The create start's job, parked.
      (should (nidhogg-workers--test-await-paused 'before-done 30000))
      (redisplay t)
      (let ((end (window-end nil t)))
        (should (> end (point-min)))
        (should (buffer-local-value 'nidhogg-workers-treesit--pending buf))
        (should-not (workers-ts-gui--positions 'font-lock-keyword-face
                                               (point-min) end))
        (nidhogg-workers--release 'before-done)
        (should (nidhogg-workers--test-await-landed 'nh-parse 30000))
        (redisplay t)
        (should-not (buffer-local-value 'nidhogg-workers-treesit--pending buf))
        (let ((got (workers-ts-gui--positions 'font-lock-keyword-face
                                              (point-min) end)))
          (should got)
          (should (equal got (workers-ts-gui--sync-keywords file end))))))))

;;; workers-ts-gui-tests.el ends here
