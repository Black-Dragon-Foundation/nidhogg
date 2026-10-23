;;; workers-timing-gui-tests.el --- GUI tests for workers: timing  -*- lexical-binding: t -*-

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
;; Loaded first, so the row's `let' binds `treesit-auto-install-grammar' and
;; `c-ts-mode-emacs-sources-support' dynamically (see workers-ts-gui-tests.el).
(require 'c-ts-mode)

;; Run by nidhogg/test/gui.sh (gui-runner.el loads every *-gui-tests.el).
;; Local only (review M4): wall-clock assertions with wide margins.

(defconst workers-timing-gui--dir
  (file-name-directory (or load-file-name buffer-file-name "")))

(ert-deftest workers-gui/ts-open-no-wait ()
  ;; §17.5: opening a 2 MB C file, the first redisplay draws the text
  ;; uncoloured within one frame (asserted with a wide margin: under
  ;; 1 s, where it takes at most the 8 ms budget and the drawing), and
  ;; the coloured one follows when the parse lands.  Needs the C grammar
  ;; (NIDHOGG_TEST_GRAMMARS or fixtures/grammars-cache/ beside this file).
  (let* ((gdir (or (getenv "NIDHOGG_TEST_GRAMMARS")
                   (expand-file-name "fixtures/grammars-cache"
                                     workers-timing-gui--dir)))
         (chunk "int f (int a, int b) { if (a > b) return a * 2; else return b + a; }\n")
         (file nil)
         (buf nil))
    (skip-unless (and (display-graphic-p) (featurep 'nidhogg-workers)
                      (file-exists-p
                       (expand-file-name "libtree-sitter-c.so" gdir))))
    (let ((treesit-extra-load-path (list gdir))
          (auto-mode-alist (cons '("\\.c\\'" . c-ts-mode) auto-mode-alist))
          (c-ts-mode-emacs-sources-support nil)
          (treesit-auto-install-grammar 'never)
          (nidhogg-workers--mode 'threads)
          (nidhogg-workers-treesit t)
          (nidhogg-treesit-defer t))
      (unwind-protect
          (progn
            (setq file (make-temp-file
                        "workers-ts-timing" nil ".c"
                        (apply #'concat
                               (make-list (/ 2000000 (length chunk)) chunk))))
            (setq buf (find-file-noselect file))
            (delete-other-windows)
            (switch-to-buffer buf)
            (goto-char (point-min))
            (let* ((t0 (float-time))
                   (_ (redisplay t))
                   (dt (- (float-time) t0))
                   (end (window-end nil t)))
              (message "workers-gui/ts-open-no-wait: first redisplay %.1f ms"
                       (* dt 1000))
              (should (< dt 1.0))
              (should (nidhogg-workers--test-await-landed 'nh-parse 30000))
              (redisplay t)
              (should-not (buffer-local-value 'nidhogg-workers-treesit--pending
                                              buf))
              (should (text-property-any (point-min) end
                                         'face 'font-lock-keyword-face))))
        (nidhogg-workers--test-await-landed 'nh-parse 30000)
        (when buf
          (kill-buffer buf))
        (when file
          (delete-file file))
        (redisplay t)))))

;;; workers-timing-gui-tests.el ends here
