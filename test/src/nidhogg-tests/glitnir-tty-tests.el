;;; glitnir-tty-tests.el --- The overview column and fold margin on a terminal  -*- lexical-binding: t -*-
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

;; A child Nidhogg runs with -nw inside a `term' buffer (TERM
;; eterm-direct: 24-bit colour, which term.el decodes into faces), and
;; the row reads the screen term.el keeps.  The child's window has a
;; `glitnir-ruler' and a one-column right margin: C puts the ruler's
;; column in the frame's desired matrix (glitnir_tty_columns), so the
;; screen shows it, and after each scroll it shows the column the
;; window's new start and end give, with no Lisp resolving the ruler;
;; text added below the window, which redraws none of its rows, moves
;; the cells too.
;; Then a line model with a hidden fold and `glitnir-fold-arrows': a
;; `▸' in the left margin on the folded line, gone once the fold opens.
;;
;; The order is forced: each step is one key the child's command map
;; runs, after which the child writes its window's numbers to a file;
;; the row then waits (at most 10 s) until the screen shows what those
;; numbers give.

;;; Code:

(require 'ert)
(require 'term)
(require 'seq)

(defun glitnir-tty-tests--wait (pred secs)
  "Wait until PRED returns non-nil, at most SECS seconds; return its value."
  (let ((end (+ (float-time) secs))
        v)
    (while (and (not (setq v (funcall pred))) (< (float-time) end))
      (accept-process-output nil 0.02))
    v))

(defun glitnir-tty-tests--read (file)
  "FILE's contents read as Lisp, or nil when it cannot be read."
  (ignore-errors
    (with-temp-buffer
      (insert-file-contents file)
      (read (current-buffer)))))

(defun glitnir-tty-tests--fg (pos)
  "The foreground term.el gave the character at POS, as (R G B) 0-255, or nil."
  (let ((f (get-text-property pos 'font-lock-face)) fg)
    (dolist (x (if (keywordp (car-safe f)) (list f) (ensure-list f)))
      (when (and (consp x) (keywordp (car x)) (plist-get x :foreground))
        (setq fg (plist-get x :foreground))))
    (and (stringp fg) (string-match "\\`#\\([[:xdigit:]]\\{6\\}\\)\\'" fg)
         (let ((n (string-to-number (match-string 1 fg) 16)))
           (list (ash n -16) (logand (ash n -8) 255) (logand n 255))))))

(defun glitnir-tty-tests--cell (buf row col)
  "The character at screen ROW and COL of term buffer BUF and its colour, as (CHAR . RGB), or nil."
  (with-current-buffer buf
    (save-excursion
      (goto-char term-home-marker)
      (and (zerop (forward-line row))
           (= (move-to-column col) col)
           (not (eolp))
           (cons (char-after) (glitnir-tty-tests--fg (point)))))))

(defun glitnir-tty-tests--text (buf row)
  "Screen ROW of term buffer BUF, as text."
  (with-current-buffer buf
    (save-excursion
      (goto-char term-home-marker)
      (forward-line row)
      (buffer-substring-no-properties (point) (line-end-position)))))

(defun glitnir-tty-tests--kind (cell)
  "CELL's kind: `track', `thumb' (green `┃'), `red' or `blue' (a `▐'), else the cell."
  (pcase cell
    (`(?│ . ,_) 'track)
    (`(?┃ ,r ,g ,b) (if (and (> g 200) (< r 60) (< b 60)) 'thumb cell))
    (`(?▐ ,r ,g ,b) (cond ((and (> r 200) (< g 60) (< b 60)) 'red)
                          ((and (> b 200) (< r 60) (< g 60)) 'blue)
                          (t cell)))
    (_ cell)))

(defun glitnir-tty-tests--expected (state)
  "The column STATE (the child's (START END Z N RESOLVES)) gives, as kinds by row.
As C makes it: a row's share of the buffer is by characters (no line
model), the thumb from START's row to END's (at least one row), the
marks (100 red, 1500 blue) over it."
  (pcase-let* ((`(,start ,end ,z ,n . ,_) state)
               (row (lambda (pos) (floor (* (/ (float (- pos 1)) (- z 1)) n))))
               (y0 (min (funcall row start) (1- n)))
               (y1 (min (max (1+ y0) (funcall row end)) n))
               (v (make-vector n 'track)))
    (dotimes (y n)
      (when (and (>= y y0) (< y y1)) (aset v y 'thumb)))
    (aset v (min (funcall row 100) (1- n)) 'red)
    (aset v (min (funcall row 1500) (1- n)) 'blue)
    (append v nil)))

(defun glitnir-tty-tests--column (buf n)
  "The kinds of screen column 79's first N rows in term buffer BUF."
  (mapcar (lambda (y) (glitnir-tty-tests--kind (glitnir-tty-tests--cell buf y 79)))
          (number-sequence 0 (1- n))))

(ert-deftest glitnir-tty/the-ruler-is-a-margin-column-and-a-fold-a-margin-arrow ()
  "On a terminal the ruler is the right margin's last column, kept right by every scroll with no Lisp,
and a folded line gets a `▸' in the left margin."
  (skip-unless (featurep 'glitnir-ruler))
  (skip-unless (fboundp 'nidhogg-lines-set-hidden))
  (skip-unless (file-exists-p (expand-file-name "e/eterm-direct" data-directory)))
  (let* ((dir (make-temp-file "nh-gtty" t))
         (ready (expand-file-name "ready" dir))
         (state (expand-file-name "state" dir))
         (step 0)
         (form
          `(progn
             (setq display-hourglass nil)
             (menu-bar-mode -1)
             (set-terminal-coding-system 'utf-8)
             (switch-to-buffer (get-buffer-create "t"))
             (dotimes (i 200) (insert (format "line %03d\n" i)))
             (goto-char 1)
             (set-window-margins nil 1 1)
             (set-window-parameter nil 'glitnir-ruler
                                   '(:band "#00ff00" :marks ((100 0 1 "#ff0000") (1500 0 1 "#0000ff"))))
             (defvar gtty-resolves 0)
             (advice-add 'glitnir--ruler-vector :before
                         (lambda (&rest _) (setq gtty-resolves (1+ gtty-resolves))))
             (defvar gtty-step 0)
             (defun gtty-report ()
               (redisplay)
               (setq gtty-step (1+ gtty-step))
               (write-region (prin1-to-string (list (window-start) (window-end nil t) (point-max)
                                                    (window-body-height) gtty-resolves gtty-step))
                             nil ,state nil 'silent))
             (let ((m (make-sparse-keymap)))
               (define-key m "s" (lambda () (interactive) (gtty-report)))
               (define-key m "x" (lambda () (interactive) (scroll-up 7) (gtty-report)))
               ;; text added below the window: no row of it is drawn again, every cell's share changes
               (define-key m "g" (lambda () (interactive)
                                   (save-excursion
                                     (goto-char (point-max))
                                     (dotimes (i 200) (insert (format "more %03d\n" i))))
                                   (gtty-report)))
               (define-key m "f" (lambda () (interactive)
                                   (goto-char 1)
                                   (set-window-start nil 1)
                                   (nidhogg-lines-enable nil t)
                                   ;; line 10 (91..99) folds lines 11 to 15
                                   (nidhogg-lines-set-folds nil 0 '((91 145 1)))
                                   (nidhogg-lines-set-hidden nil '((99 . 144)))
                                   (overlay-put (make-overlay 99 144) 'invisible t)
                                   (set-window-parameter nil 'glitnir-fold-arrows
                                                         '(:mode always :folded "#ff00ff"))
                                   (gtty-report)))
               (define-key m "u" (lambda () (interactive)
                                   (remove-overlays)
                                   (nidhogg-lines-set-hidden nil nil)
                                   (gtty-report)))
               (push (list (cons t m)) emulation-mode-map-alists))
             (run-with-timer 0.5 nil (lambda () (write-region "t" nil ,ready nil 'silent)))))
         (buf (generate-new-buffer " *glitnir-tty*"))
         (locale-coding-system 'utf-8))
    (unwind-protect
        (let ((press
               (lambda (key)
                 (setq step (1+ step))
                 (with-current-buffer buf (term-send-raw-string key))
                 (or (glitnir-tty-tests--wait
                      (lambda () (let ((s (glitnir-tty-tests--read state))) (and (eql (nth 5 s) step) s)))
                      10)
                     (error "The child did not answer key %S" key)))))
          (with-current-buffer buf
            (term-mode)
            (setq-local term-height 24 term-width 80)
            (let ((term-term-name "eterm-direct"))
              (term-exec buf "glitnir-tty" (expand-file-name invocation-name invocation-directory) nil
                         (list "-nw" "-Q" "--eval" (prin1-to-string form))))
            (term-char-mode))
          (unless (glitnir-tty-tests--wait (lambda () (file-exists-p ready)) 30)
            (error "The child's frame did not come up"))
          ;; the column at the start, after each of four 7-line scrolls, and after text added below
          (let* ((s0 (funcall press "s"))
                 (n (nth 3 s0)))
            (should (= n 22))
            (dolist (key '("s" "x" "x" "x" "x" "g"))
              (let* ((s (if (equal key "s") s0 (funcall press key)))
                     (want (glitnir-tty-tests--expected s)))
                (should (equal (glitnir-tty-tests--wait
                                (lambda () (let ((c (glitnir-tty-tests--column buf n)))
                                             (and (equal c want) c)))
                                10)
                               want))
                ;; the text keeps its place: the window's first line is at the top, after the left margin
                (should (string-prefix-p (format " line %03d" (/ (1- (car s)) 9))
                                         (glitnir-tty-tests--text buf 0)))
                ;; a scroll resolved no ruler
                (should (= (nth 4 s) (nth 4 s0)))))
            (should (memq 'thumb (glitnir-tty-tests--column buf n)))
            (should-not (eq (car (glitnir-tty-tests--column buf n)) 'thumb))
            ;; a folded line: `▸' in the folded colour at its row's left margin
            (funcall press "f")
            (let ((arrow (lambda (row)
                           (pcase (glitnir-tty-tests--cell buf row 0)
                             (`(?▸ ,r ,g ,b) (and (> r 200) (> b 200) (< g 60)))))))
              (should (glitnir-tty-tests--wait
                       (lambda () (string-prefix-p " line 016" (glitnir-tty-tests--text buf 11)))
                       10))
              (should (funcall arrow 10))
              (should (string-prefix-p "▸line 010" (glitnir-tty-tests--text buf 10)))
              (should-not (seq-some arrow (delete 10 (number-sequence 0 (1- n)))))
              ;; opened: no arrow, the lines back
              (funcall press "u")
              (should (glitnir-tty-tests--wait
                       (lambda () (string-prefix-p " line 011" (glitnir-tty-tests--text buf 11)))
                       10))
              (should (string-prefix-p " line 010" (glitnir-tty-tests--text buf 10)))
              (should-not (seq-some arrow (number-sequence 0 (1- n)))))))
      (let ((p (get-buffer-process buf)))
        (when p (delete-process p)))
      (kill-buffer buf)
      (delete-directory dir t))))

;; A ruler on the left (`:left'): the left margin's first column, the fold arrow in its second.
(ert-deftest glitnir-tty/a-left-ruler-is-the-left-margins-first-column ()
  "With `:left' the ruler is the left margin's first column (its thumb and track there, none at the right edge), and a
folded line's `▸' moves to the margin's second column."
  (skip-unless (featurep 'glitnir-ruler))
  (skip-unless (fboundp 'nidhogg-lines-set-hidden))
  (skip-unless (file-exists-p (expand-file-name "e/eterm-direct" data-directory)))
  (let* ((dir (make-temp-file "nh-gtty" t))
         (ready (expand-file-name "ready" dir))
         (state (expand-file-name "state" dir))
         (step 0)
         (form
          `(progn
             (setq display-hourglass nil)
             (menu-bar-mode -1)
             (set-terminal-coding-system 'utf-8)
             (switch-to-buffer (get-buffer-create "t"))
             (dotimes (i 200) (insert (format "line %03d\n" i)))
             (goto-char 1)
             (set-window-margins nil 2 0)
             (set-window-parameter nil 'glitnir-ruler '(:left t :band "#00ff00"))
             (defvar gtty-step 0)
             (defun gtty-report ()
               (redisplay)
               (setq gtty-step (1+ gtty-step))
               (write-region (prin1-to-string (list (window-start) (window-end nil t) (point-max)
                                                    (window-body-height) 0 gtty-step))
                             nil ,state nil 'silent))
             (let ((m (make-sparse-keymap)))
               (define-key m "s" (lambda () (interactive) (gtty-report)))
               (define-key m "f" (lambda () (interactive)
                                   (nidhogg-lines-enable nil t)
                                   (nidhogg-lines-set-folds nil 0 '((91 145 1)))
                                   (nidhogg-lines-set-hidden nil '((99 . 144)))
                                   (overlay-put (make-overlay 99 144) 'invisible t)
                                   (set-window-parameter nil 'glitnir-fold-arrows
                                                         '(:mode always :folded "#ff00ff"))
                                   (gtty-report)))
               (push (list (cons t m)) emulation-mode-map-alists))
             (run-with-timer 0.5 nil (lambda () (write-region "t" nil ,ready nil 'silent)))))
         (buf (generate-new-buffer " *glitnir-tty*"))
         (locale-coding-system 'utf-8))
    (unwind-protect
        (let ((press
               (lambda (key)
                 (setq step (1+ step))
                 (with-current-buffer buf (term-send-raw-string key))
                 (or (glitnir-tty-tests--wait
                      (lambda () (let ((s (glitnir-tty-tests--read state))) (and (eql (nth 5 s) step) s)))
                      10)
                     (error "The child did not answer key %S" key)))))
          (with-current-buffer buf
            (term-mode)
            (setq-local term-height 24 term-width 80)
            (let ((term-term-name "eterm-direct"))
              (term-exec buf "glitnir-tty" (expand-file-name invocation-name invocation-directory) nil
                         (list "-nw" "-Q" "--eval" (prin1-to-string form))))
            (term-char-mode))
          (unless (glitnir-tty-tests--wait (lambda () (file-exists-p ready)) 30)
            (error "The child's frame did not come up"))
          (let* ((n (nth 3 (funcall press "s")))
                 (col (lambda (x) (mapcar (lambda (y) (glitnir-tty-tests--kind (glitnir-tty-tests--cell buf y x)))
                                          (number-sequence 0 (1- n))))))
            (should (glitnir-tty-tests--wait (lambda () (memq 'thumb (funcall col 0))) 10))
            (should (memq 'track (funcall col 0)))
            (should-not (seq-some (lambda (k) (memq k '(thumb track))) (funcall col 79)))
            ;; the text after the two margin columns
            (should (string-match-p "\\`..line 000" (glitnir-tty-tests--text buf 0)))
            (funcall press "f")
            (should (glitnir-tty-tests--wait
                     (lambda () (pcase (glitnir-tty-tests--cell buf 10 1)
                                  (`(?▸ ,r ,g ,b) (and (> r 200) (> b 200) (< g 60)))))
                     10))
            ;; the ruler keeps its column on the folded row
            (should (memq (glitnir-tty-tests--kind (glitnir-tty-tests--cell buf 10 0)) '(thumb track)))))
      (let ((p (get-buffer-process buf)))
        (when p (delete-process p)))
      (kill-buffer buf)
      (delete-directory dir t))))

;;; glitnir-tty-tests.el ends here
