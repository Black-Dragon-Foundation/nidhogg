;;; workers-terminal-tests.el --- tests for workers: terminal emulators  -*- lexical-binding: t -*-

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

;; `workers/terminal-emulators' (§17.2a): eat and vterm (when its
;; module is built) render a recorded `htop' and `ls --color' stream,
;; written through a pty by `cat', to identical screens with the
;; process reader off and on (in both of `nidhogg-workers--mode''s
;; modes).  The screen is the buffer's text and its faces.  The
;; packages come from pkg-test.sh's cache: its `process' group puts
;; them on `load-path'.  Without them the row skips.

;;; Code:

(require 'ert)
(require 'cl-lib)

(defvar eat-terminal)
(defvar vterm-shell)
(defvar vterm-kill-buffer-on-exit)
(defvar vterm-timer-delay)
(defvar vterm-exit-functions)
(defvar vterm--process)
(declare-function eat-mode "eat")
(declare-function eat-exec "eat")
(declare-function vterm-mode "vterm")
(declare-function vterm--delayed-redraw "vterm")

(defun workers-terminal-tests--ls ()
  "A recorded `ls --color=always -l' stream, as UTF-8 bytes.
Directories, executables, links, archives and pictures in their
default colours, names with accents and wide characters."
  (let ((kinds '(("01;34" "src" "drwxr-xr-x.") ("01;32" "build.sh" "-rwxr-xr-x.")
                 ("01;36" "latest" "lrwxrwxrwx.") ("01;31" "dist.tar.gz" "-rw-r--r--.")
                 ("01;35" "shot.png" "-rw-r--r--.") (nil "README.md" "-rw-r--r--.")))
        (names '("naïve-über" "日本語のファイル" "done-✓" "plain")))
    (with-temp-buffer
      (insert "total 1928\n")
      (dotimes (i 240)
        (let ((k (nth (% i 6) kinds))
              (name (format "%s-%03d-%s" (nth (% (/ i 6) 4) names) i
                            (nth 1 (nth (% i 6) kinds)))))
          (insert (format "%s %d user user %8d Oct %2d %02d:%02d "
                          (nth 2 k) (1+ (% i 3)) (* 4096 (1+ (% (* i 7919) 97)))
                          (1+ (% i 28)) (% i 24) (% (* i 7) 60))
                  (if (car k) (format "\e[0m\e[%sm%s\e[0m" (car k) name) name)
                  (if (equal (car k) "01;36") " -> \e[01;34msrc\e[0m" "")
                  "\n")))
      (encode-coding-string (buffer-string) 'utf-8))))

(defun workers-terminal-tests--htop ()
  "A recorded `htop' stream, as UTF-8 bytes: 40 frames on the alternate screen.
Cursor addressing, SGR colours, erasures, a scroll region with a deleted
line, tree glyphs and wide characters."
  (let ((commands '("emacs --daemon" "htop" "sh -c make -j8" "日本語 worker"
                    "cc1 -O2 nidhogg.c")))
    (with-temp-buffer
      (insert "\e[?1049h\e[22;0;0t\e[1;24r\e(B\e[m\e[4l\e[?7h\e[?1h\e=\e[?25l"
              "\e[39;49m\e[H\e[2J")
      (dotimes (f 40)
        (dotimes (cpu 4)
          (let ((bars (% (+ (* f 7) (* cpu 13)) 30)))
            (insert (format "\e[%d;3H\e[1;36m%d\e[m\e[1m[\e[32m%s\e[31m%s\e[m%s\e[1;30m%5.1f%%\e[m\e[1m]\e[K"
                            (1+ cpu) cpu (make-string (/ bars 2) ?|)
                            (make-string (- bars (/ bars 2)) ?|)
                            (make-string (- 30 bars) ?\s) (* bars 3.3)))))
        (let ((mem (% (+ 9 (* f 5)) 28)))
          (insert (format "\e[5;3H\e[1;36mMem\e[m\e[1m[\e[32m%s\e[34m%s\e[33m%s\e[m%s\e[1;30m%dM/15.5G\e[m\e[1m]\e[K"
                          (make-string (/ mem 2) ?|) (make-string (/ mem 4) ?|)
                          (make-string (- mem (/ mem 2) (/ mem 4)) ?|)
                          (make-string (- 28 mem) ?\s) (* 512 mem))))
        (insert (format "\e[1;45HTasks: \e[1m%d\e[m, \e[1;32m%d\e[m thr; \e[1;32m%d\e[m running\e[K"
                        (+ 120 f) (* 3 f) (1+ (% f 4)))
                (format "\e[2;45HLoad average: \e[1m%.2f %.2f %.2f\e[K"
                        (/ (% (* f 37) 400) 100.0) (/ (% (* f 23) 300) 100.0)
                        (/ (% (* f 11) 200) 100.0))
                "\e[7;1H\e[30;42m    PID USER      PRI  NI  VIRT   RES S CPU% MEM%   TIME+  Command\e[K\e[m")
        (dotimes (r 16)
          (insert (format "\e[%d;1H%s%7d user       20   0 %4dM %4dM %s %4.1f %4.1f %3d:%02d.%02d %s%s\e[K\e[m"
                          (+ 8 r) (if (= r (% f 16)) "\e[30;46m" "")
                          (+ 1000 (* 37 (% (+ r f) 23))) (+ 100 (* 13 r)) (+ 20 (* 7 r))
                          (if (zerop (% (+ r f) 3)) "R" "S")
                          (/ (% (* (+ r f) 71) 1000) 10.0) (/ (% (* r 43) 500) 10.0)
                          (% (+ r f) 60) (% (* r 7) 60) (% (* f 13) 100)
                          (if (> r 0) "├─ " "") (nth (% (+ r f) 5) commands))))
        (when (zerop (% f 5))
          (insert "\e[8;23r\e[8;1H\e[M\e[1;24r"))
        (insert "\e[24;1H\e[30;46mF1\e[39;49mHelp  \e[30;46mF2\e[39;49mSetup \e[30;46mF10\e[39;49mQuit\e[K"))
      (insert "\e[24;1H\e[?25h")
      (encode-coding-string (buffer-string) 'utf-8))))

(defun workers-terminal-tests--until (pred)
  "Wait for process output until PRED holds (10 s at most)."
  (let ((i 0))
    (while (and (not (funcall pred)) (< i 500))
      (accept-process-output nil 0.02)
      (setq i (1+ i)))
    (funcall pred)))

(defun workers-terminal-tests--screen (buffer)
  "BUFFER's screen: its text, in runs of one face, as (TEXT FACE)."
  (with-current-buffer buffer
    (let ((pos (point-min)) runs)
      (while (< pos (point-max))
        (let ((next (next-single-property-change pos 'face nil (point-max))))
          (push (list (buffer-substring-no-properties pos next)
                      (get-text-property pos 'face))
                runs)
          (setq pos next)))
      (nreverse runs))))

(defun workers-terminal-tests--owned (proc)
  (plist-get (nidhogg-workers--test-io-info proc) :owned))

(defun workers-terminal-tests--eat (file)
  "Render FILE with `cat' in an eat terminal until the process ends.
Return the screen and whether the reader owned the process."
  (let ((buf (generate-new-buffer "wterm-eat")))
    (unwind-protect
        (with-current-buffer buf
          (eat-mode)
          (eat-exec buf "wterm-eat" "cat" nil (list file))
          (let* ((proc (get-buffer-process buf))
                 (owned (workers-terminal-tests--owned proc)))
            (set-process-query-on-exit-flag proc nil)
            ;; eat's sentinel processes the queued output, then drops
            ;; the terminal.
            (should (workers-terminal-tests--until
                     (lambda () (null (buffer-local-value 'eat-terminal buf)))))
            (list (workers-terminal-tests--screen buf) owned)))
      (let ((kill-buffer-query-functions nil))
        (kill-buffer buf)))))

(defun workers-terminal-tests--vterm (file)
  "Render FILE with `cat' in a vterm until the process ends.
Return the screen and whether the reader owned the process."
  (let* ((done nil)
         (vterm-shell (concat "cat " (shell-quote-argument file)))
         (vterm-kill-buffer-on-exit nil)
         (vterm-timer-delay nil)
         (vterm-exit-functions (list (lambda (_buf _event) (setq done t))))
         (locale-coding-system 'utf-8-unix)
         (buf (generate-new-buffer "wterm-vterm")))
    (unwind-protect
        (with-current-buffer buf
          (vterm-mode)
          (let* ((proc vterm--process)
                 (owned (workers-terminal-tests--owned proc)))
            (set-process-query-on-exit-flag proc nil)
            ;; The sentinel runs after every byte reached the filter.
            (should (workers-terminal-tests--until (lambda () done)))
            (vterm--delayed-redraw buf)
            (list (workers-terminal-tests--screen buf) owned)))
      (let ((kill-buffer-query-functions nil))
        (kill-buffer buf)))))

(defun workers-terminal-tests--compare (render file want)
  "RENDER FILE with the reader off, then on in each mode: the same screen.
WANT is a string the screen shows."
  (let ((off (let ((nidhogg-workers-process-reader nil))
               (funcall render file))))
    (should-not (cadr off))
    (should (string-search want (mapconcat #'car (car off) "")))
    (dolist (mode '(threads inline))
      (ert-info ((format "mode %s" mode) :prefix "")
        (let* ((on (let ((nidhogg-workers--mode mode)
                         (nidhogg-workers-process-reader t)
                         (nidhogg-workers-reader-pipes t)
                         (nidhogg-workers-reader-ptys t))
                     (funcall render file))))
          (should (cadr on))
          (should (equal (car on) (car off))))))))

(ert-deftest workers/terminal-emulators ()
  ;; eat and vterm (when built) draw a recorded `htop' and `ls --color'
  ;; stream to the same screen with the reader on and off (§17.2a).
  (skip-unless (featurep 'nidhogg-workers))
  (let ((emulators
         (delq nil
               (list (and (require 'eat nil t)
                          (cons 'eat #'workers-terminal-tests--eat))
                     (and (locate-library "vterm-module")
                          (require 'vterm nil t)
                          (cons 'vterm #'workers-terminal-tests--vterm))))))
    (skip-unless emulators)
    (dolist (stream (list (list 'htop (workers-terminal-tests--htop) "Load average")
                          (list 'ls (workers-terminal-tests--ls) "README.md")))
      (let ((file (make-temp-file "wterm-")))
        (unwind-protect
            (progn
              (let ((coding-system-for-write 'no-conversion))
                (write-region (nth 1 stream) nil file nil 'silent))
              (dolist (e emulators)
                (ert-info ((format "%s, %s" (car e) (car stream)) :prefix "")
                  (workers-terminal-tests--compare (cdr e) file (nth 2 stream)))))
          (delete-file file))))))

;;; workers-terminal-tests.el ends here
