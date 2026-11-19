;;; fixes-tests.el --- Fixes to upstream Emacs bugs  -*- lexical-binding: t -*-
;; test-class: cond

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

;; `--with-nidhogg-fixes' (README.md, "Fixes to upstream bugs"): bugs of
;; upstream Emacs that touch no Nidhogg feature.  One row per fix.
;;
;; Conditions only: a child Emacs is waited for by its output and its
;; exit, never by the clock.

;;; Code:

(require 'ert)
(require 'seq)

(defconst fixes-tests--reloads 400
  "How many times each Emacs loads the shared .eln again.")

(defun fixes-tests--reload-form (eln n)
  "A form that loads ELN once, says it is ready, waits for a line on standard input, then loads ELN again N
times and prints how many of those loads signalled, and the first error."
  `(let ((errors 0) (first nil))
     (native-elisp-load ,eln)
     (princ "ready\n")
     (read-from-minibuffer "")
     (dotimes (_ ,n)
       (condition-case err (native-elisp-load ,eln)
         (error (setq errors (1+ errors)) (unless first (setq first err)))))
     (princ (format "errors %d %S\n" errors first))))

(ert-deftest fixes/two-emacs-reloading-one-eln-never-lose-it ()
  "Two Emacs processes load one .eln of a shared eln-cache again (a module turned off and on, a package reloaded
in a daemon and in a second Emacs).  Upstream, a load of a file this session loaded before renamed the shared file
aside, opened it and renamed it back, so the other process found it missing (`file-missing' \"Renaming\", or a load
that failed).  Each process now opens a private copy, and neither signals."
  (skip-unless (and (fboundp 'native-comp-available-p) (native-comp-available-p)))
  (let* ((dir (make-temp-file "fixes-eln" t))
         (src (expand-file-name "fixes-reload.el" dir))
         (eln (expand-file-name "fixes-reload.eln" dir))
         (out "") (proc nil))
    (unwind-protect
        (progn
          (with-temp-file src
            (insert ";;; fixes-reload.el  -*- lexical-binding: t -*-\n(defvar fixes-reload--n 0)\n"
                    "(defun fixes-reload-step () (setq fixes-reload--n (1+ fixes-reload--n)))\n"))
          (native-compile src eln)
          (should (file-exists-p eln))
          (setq proc (make-process
                      :name "fixes-reload" :connection-type 'pipe :noquery t
                      :command (list (expand-file-name invocation-name invocation-directory) "-Q" "--batch" "--eval"
                                     (prin1-to-string (fixes-tests--reload-form eln fixes-tests--reloads)))
                      :filter (lambda (_p s) (setq out (concat out s)))))
          (while (and (process-live-p proc) (not (string-search "ready\n" out)))
            (accept-process-output proc))
          (should (string-search "ready\n" out))
          ;; both start reloading together
          (process-send-string proc "go\n")
          (let ((errors 0) (first nil))
            (native-elisp-load eln)
            (dotimes (_ fixes-tests--reloads)
              (condition-case err (native-elisp-load eln)
                (error (setq errors (1+ errors)) (unless first (setq first err)))))
            (while (process-live-p proc) (accept-process-output proc))
            (should (equal (list :here errors first :child (string-trim (substring out (or (string-search "errors" out) 0))))
                           (list :here 0 nil :child "errors 0 nil"))))
          (should (file-exists-p eln)))
      (when (process-live-p proc) (delete-process proc))
      (delete-directory dir t))))

(defun fixes-tests--read (file)
  "FILE's contents read as a Lisp object, or nil when there is none yet."
  (ignore-errors (with-temp-buffer (insert-file-contents file) (read (current-buffer)))))

(defun fixes-tests--until (pred child)
  "Wait until PRED returns non-nil or CHILD ends, at most 30 s; return PRED's value."
  (let ((end (+ (float-time) 30)) v)
    (while (and (not (setq v (funcall pred))) (process-live-p child) (< (float-time) end))
      (accept-process-output nil 0.02))
    (or v (funcall pred))))

(defun fixes-tests--tty-report (child file lines cols)
  "Make CHILD's terminal LINES x COLS and return its report: (WIDTH LINES-USED OUTSIDE), or (:ended STATUS)."
  (set-process-window-size child lines cols)
  (or (fixes-tests--until
       (lambda ()
         ;; y reports; the size is the terminal's once the child has seen SIGWINCH
         (let ((r (fixes-tests--read file)))
           (if (and r (= (car r) cols))
               r
             (when (file-exists-p file) (delete-file file))
             (when (process-live-p child) (process-send-string child "y"))
             (accept-process-output nil 0.1)
             nil)))
       child)
      (and (not (process-live-p child)) (list :ended (process-status child) (process-exit-status child)))
      (fixes-tests--read file)))

(defun fixes-tests--xterm-p ()
  "Whether xterm's terminfo is installed, for a child Emacs on a pty."
  (seq-some (lambda (d) (file-exists-p (expand-file-name "x/xterm" d)))
            '("/usr/share/terminfo" "/lib/terminfo" "/etc/terminfo")))

(defun fixes-tests--shrink (layout sizes)
  "Run a child `emacs -nw -Q' on a 40x120 pty, make LAYOUT's windows, then resize its terminal to each of SIZES.
LAYOUT is a form the child evaluates.  SIZES is a list of (LINES COLS).  Return (WINDOWS REPORT...): the number of
windows LAYOUT made, then a `fixes-tests--tty-report' per size."
  (let*((dir (make-temp-file "fixes-tty" t))
         (ready (expand-file-name "ready" dir))
         (report (expand-file-name "report" dir))
         (built (expand-file-name "built" dir))
         (process-environment (cons "TERM=xterm" process-environment))
         (child nil))
    (unwind-protect
        (progn
          (setq child
                (make-process
                 :name "fixes-tty" :buffer nil :noquery t :connection-type 'pty :filter #'ignore
                 :command
                 (list (expand-file-name invocation-name invocation-directory) "-nw" "-Q" "--eval"
                       (prin1-to-string
                        `(progn
                           (defun fixes-tty-report ()
                             (interactive)
                             (redisplay t)
                             (let* ((f (selected-frame))
                                    (cols (frame-width f))
                                    (used (nth 3 (window-edges (minibuffer-window f))))
                                    (outside nil))
                               (dolist (w (window-list f t))
                                 (let ((e (window-edges w)))
                                   (when (or (> (nth 2 e) cols) (> (nth 3 e) used))
                                     (push e outside))))
                               (write-region (prin1-to-string (list cols used outside)) nil ,report nil 'silent)))
                           (defun fixes-tty-build ()
                             (interactive)
                             ,layout
                             (write-region (format "%d" (length (window-list nil 'nomini))) nil ,built nil 'silent))
                           (push (list (cons t (let ((m (make-sparse-keymap)))
                                                 (define-key m "x" #'fixes-tty-build)
                                                 (define-key m "y" #'fixes-tty-report)
                                                 m)))
                                 emulation-mode-map-alists)
                           (write-region "t" nil ,ready nil 'silent))))))
          (should (fixes-tests--until (lambda () (file-exists-p ready)) child))
          (should (equal (fixes-tests--tty-report child report 40 120) '(120 40 nil)))
          (process-send-string child "x")
          (cons (fixes-tests--until (lambda () (fixes-tests--read built)) child)
                (mapcar (lambda (size) (fixes-tests--tty-report child report (car size) (cadr size))) sizes)))
      (when child (delete-process child))
      (delete-directory dir t))))

(ert-deftest fixes/a-shrunk-terminal-keeps-every-window-on-it ()
  "A terminal made smaller than its windows' minimum sizes: 36 windows, each with a tab line, a header line and a
mode line, in a 30x8 terminal.  Upstream, `resize_frame_windows' tried normal and then `ignore' minimum sizes, both
of which count those lines, so both failed and the windows kept their old sizes, larger than the frame: redisplay
wrote past it and Emacs aborted in `cmcheckmagic'.  The windows now take their safe sizes (one line or two columns
of text, mode, header and tab lines no longer counted), fit the terminal, and come back when it grows."
  (skip-unless (fixes-tests--xterm-p))
  (should (equal (fixes-tests--shrink
                  ;; six rows of six windows
                  '(progn (let ((window-min-height 1) (window-min-width 1))
                            (dotimes (_ 5) (split-window-below))
                            (dolist (w (window-list nil 'nomini))
                              (with-selected-window w (dotimes (_ 5) (split-window-right))))
                            (balance-windows))
                          (setq-default tab-line-format '("tabs") header-line-format '("head"))
                          (balance-windows))
                  '((8 30) (40 120)))
                 '(36 (30 8 nil) (120 40 nil)))))

(ert-deftest fixes/below-the-safe-sizes-the-frame-stays-at-them ()
  "A terminal smaller than even its windows' safe sizes: 12 windows above one another in an 8-line terminal.  As
upstream, a terminal frame never gets smaller than those sizes (`frame-windows-min-size' with `safe'): the frame
stays 14 lines (the menu bar, 12 one-line windows, the echo area), larger than the terminal, and every window
fits the frame.  No crash, and the windows come back when the terminal grows."
  (skip-unless (fixes-tests--xterm-p))
  (should (equal (fixes-tests--shrink
                  '(let ((window-min-height 1))
                     (dotimes (_ 11) (split-window (get-largest-window) nil 'below))
                     (balance-windows))
                  '((8 30) (40 120)))
                 '(12 (30 14 nil) (120 40 nil)))))

;;; fixes-tests.el ends here
