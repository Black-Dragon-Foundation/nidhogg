;;; workers-image-gui-tests.el --- GUI tests for workers: image  -*- lexical-binding: t -*-

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
(require 'cl-lib)

;; Run by nidhogg/test/gui.sh (gui-runner.el loads every *-gui-tests.el).

(defvar workers-image-gui--dir
  (file-name-as-directory
   (or (getenv "NH_WORKERS_FIXTURES")
       (expand-file-name "fixtures/images/"
                         (file-name-directory
                          (or load-file-name buffer-file-name)))))
  "The image corpus (the runner stages the tests apart from it).")

(defvar workers-image-gui--serial 0)

(defun workers-image-gui--read (name)
  (with-temp-buffer
    (insert-file-contents (expand-file-name name workers-image-gui--dir))
    (goto-char (point-min))
    (read (current-buffer))))

(defun workers-image-gui--type (file)
  (pcase (file-name-extension file)
    ("png" 'png) ("jpg" 'jpeg) ("gif" 'gif) ("webp" 'webp)))

(defun workers-image-gui--spec (file bg index)
  `(image :type ,(workers-image-gui--type file)
          :file ,(expand-file-name file workers-image-gui--dir)
          ,@(and bg (list :background bg))
          ,@(and index (list :index index))))

(defun workers-image-gui--digest (r)
  (list (plist-get r :width) (plist-get r :height)
        (and (plist-get r :pixels) (secure-hash 'sha1 (plist-get r :pixels)))
        (and (plist-get r :mask) (secure-hash 'sha1 (plist-get r :mask)))))

(defun workers-image-gui--params (file bg)
  "The core's params for what the loader reads from this frame."
  (let* ((type (workers-image-gui--type file))
         (fc (color-values (frame-parameter nil 'background-color)))
         (c (and bg (color-values bg)))
         (px (lambda (v) (logior (ash (ash (nth 0 v) -8) 16)
                                 (ash (ash (nth 1 v) -8) 8)
                                 (ash (nth 2 v) -8)))))
    (append '(:limit 10000)
            (and (memq type '(png webp)) (list :bg (or c fc)))
            (and (eq type 'gif)
                 (append (list :frame-bg (funcall px fc))
                         (and c (list :gif-bg (funcall px c))))))))

(defun workers-image-gui--load (spec legacy)
  "SPEC loaded by the loader (LEGACY) or the loader under the flag.
Value: (DIGEST LISP-DATA CORNERS FAILED LOG)."
  (let ((nidhogg-workers--test-image-legacy legacy)
        (max-image-size 10000)
        start r)
    ;; *Messages* folds a repeated message into "[2 times]": break the run.
    (message "-- load %s" (cl-incf workers-image-gui--serial))
    (with-current-buffer (get-buffer-create "*Messages*")
      (setq start (point-max)))
    (setq r (nidhogg-workers--test-image-pixels spec))
    (list (workers-image-gui--digest r) (plist-get r :lisp-data)
          (plist-get r :corners) (plist-get r :failed)
          (with-current-buffer "*Messages*"
            (string-replace workers-image-gui--dir "@DIR@/"
                            (string-trim (buffer-substring start (point-max))))))))

(ert-deftest workers-gui/image-oracle ()
  ;; Today's loaders, the synchronous loaders under the flag and the
  ;; core give identical pixels, masks, metadata and error text.
  (skip-unless (display-graphic-p))
  (let* ((cases (workers-image-gui--read "oracle-cases.eld"))
         (errors (workers-image-gui--read "oracle-errors.eld"))
         (fc (color-values (frame-parameter nil 'background-color)))
         legacy flagged results texts)
    (nidhogg-workers--test-image-cache-clear)
    (dolist (c cases)
      (push (workers-image-gui--load
             (workers-image-gui--spec (nth 0 c) (nth 1 c) (nth 2 c)) t)
            legacy))
    (setq legacy (nreverse legacy))
    (dolist (c cases)
      (push (workers-image-gui--load
             (workers-image-gui--spec (nth 0 c) (nth 1 c) (nth 2 c)) nil)
            flagged))
    (setq flagged (nreverse flagged))
    (nidhogg-workers--test-image-cache-clear)
    (cl-loop for c in cases for l in legacy for f in flagged
             do (should-not (nth 3 l))
             (should (equal (nth 0 l) (nth 0 f)))
             (should (equal (nth 1 l) (nth 1 f)))
             (should (equal (nth 2 l) (nth 2 f)))
             (let ((r (nidhogg-workers--test-image-decode
                       (workers-image-gui--spec (nth 0 c) nil (nth 2 c))
                       (workers-image-gui--params (nth 0 c) (nth 1 c)))))
               (should (equal (workers-image-gui--digest r) (nth 0 l))))
             (push (cons c (nth 0 l)) results))
    ;; Errors: the same text and a failed image under both.
    (dolist (e errors)
      (pcase-let ((`(,file ,bg ,index) (if (consp e) e (list e nil nil))))
        (let* ((spec (workers-image-gui--spec file bg index))
               (l (workers-image-gui--load spec t))
               (f (workers-image-gui--load spec nil)))
          (should (nth 3 l))
          (should (nth 3 f))
          (should (equal (nth 4 l) (nth 4 f)))
          (push (cons (list file bg index) (nth 4 l)) texts))))
    (when-let ((out (getenv "NH_RECORD_ORACLE")))
      (let ((golden
             (concat ";; -*- lexical-binding: t -*-\n"
                     ";; Recorded from the loaders (nidhogg-workers--test-image-legacy t) by\n"
                     ";; workers-gui/image-oracle with NH_RECORD_ORACLE set.\n"
                     (format "((frame-bg-color %S)\n (results\n" fc)
                     (mapconcat (lambda (r) (format "  %S\n" r))
                                (reverse results) "")
                     "  ))\n"))
            (errors-golden
             (concat ";; -*- lexical-binding: t -*-\n"
                     ";; Recorded from the loaders by workers-gui/image-oracle with\n"
                     ";; NH_RECORD_ORACLE set: ((FILE BG INDEX) . LOG-TEXT).\n(\n"
                     (mapconcat (lambda (r) (format " %S\n" r))
                                (reverse texts) "")
                     ")\n")))
        (if (equal out "stderr")
            ;; The container mounts the checkout read-only: print the files.
            (let ((standard-output #'external-debugging-output))
              (princ (concat "\n@@NH-GOLDEN-BEGIN oracle-golden.eld\n" golden
                             "@@NH-GOLDEN-END\n@@NH-GOLDEN-BEGIN oracle-errors-golden.eld\n"
                             errors-golden "@@NH-GOLDEN-END\n")))
          (with-temp-file (expand-file-name "oracle-golden.eld" out)
            (insert golden))
          (with-temp-file (expand-file-name "oracle-errors-golden.eld" out)
            (insert errors-golden)))))))

;;; workers-image-gui-tests.el ends here
