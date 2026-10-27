;;; watchdog-modal-gui-tests.el --- GUI tests for the stall watchdog (menus and dialogs)  -*- lexical-binding: t -*-

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

;; The watchdog treats a menu or a dialog the user is looking at as waiting
;; (the GLib poll wrapper, nidhogg-watchdog-gtk.c), and a Lisp timer that
;; runs inside one as busy.  Uses `--start', the spans and `--take'.

(defmacro watchdog-modal--with-watchdog (&rest body)
  "Run BODY with the watchdog on at a 100 ms threshold; stop it after."
  (declare (indent 0))
  `(let ((hang-file (make-temp-file "nh-wd-hang")))
     (unwind-protect
         (progn
           (nidhogg-watchdog--start 100 600000 hang-file)
           ,@body)
       (ignore-errors (nidhogg-watchdog--stop))
       (ignore-errors (delete-file hang-file)))))

(defun watchdog-modal--stalls ()
  "The stalls the watchdog has recorded so far (the car of `--take')."
  (car (nidhogg-watchdog--take)))

(defun watchdog-modal--spin (ms)
  "Busy-loop for MS milliseconds without any wait."
  (let ((end (+ (float-time) (/ ms 1000.0))))
    (while (< (float-time) end))))

(defun watchdog-modal--describe-stalls (stalls)
  "One line per stall in STALLS: its wall time, span and top frames."
  (mapconcat
   (lambda (s)
     (let* ((samples (append (plist-get s :samples) nil))
            (frames (and samples (append (plist-get (car samples) :frames) nil))))
       (format "wall %s ms (nongc %s, late %s) span %s, %d samples, top frames %S"
               (plist-get s :wall-ms) (plist-get s :nongc-ms)
               (plist-get s :late-ms) (plist-get s :span) (length samples)
               (seq-take frames 6))))
   stalls "\n"))

(ert-deftest watchdog-gui/menu-open-is-waiting ()
  (watchdog-modal--with-watchdog
    (nidhogg-watchdog--test-popdown-after 1000)
    (condition-case nil
        (x-popup-menu (list '(10 10) (selected-window))
                      '("Menu" ("Pane" ("One" . one) ("Two" . two))))
      (quit nil))
    (should (null (watchdog-modal--stalls)))))

(ert-deftest watchdog-gui/timer-inside-a-dialog-is-busy ()
  (watchdog-modal--with-watchdog
    ;; A file chooser runs in `xg_dialog_run' (nh_wd_dialog_depth), the one
    ;; modal loop that runs Lisp timers; pgtk's popup dialogs do not.
    (let ((timer (run-with-timer 0.2 nil #'watchdog-modal--busy-timer)))
      (unwind-protect
          (progn
            (nidhogg-watchdog--test-dialog-response-after 1000)
            (condition-case nil (x-file-dialog "Pick" "/tmp/" nil t) (quit nil))
            ;; The chooser's own setup may stall on a slow machine (a real
            ;; stall in `x-file-dialog', before the dialog loop starts), so
            ;; the claim is about the timer's stall, not the stall count.
            (let* ((stalls (watchdog-modal--stalls))
                   (busy (seq-filter
                          (lambda (s)
                            ;; any sample: the first can land in the dialog
                            ;; loop's C side, before the timer's frames exist
                            (seq-some
                             (lambda (sample)
                               (memq 'watchdog-modal--busy-timer
                                     (append (plist-get sample :frames) nil)))
                             (plist-get s :samples)))
                          stalls)))
              (ert-info ((watchdog-modal--describe-stalls stalls) :prefix "stalls: ")
                (should (= (length busy) 1))
                (should (>= (plist-get (car busy) :wall-ms) 300)))))
        (cancel-timer timer)))))

(defun watchdog-modal--busy-timer ()
  (watchdog-modal--spin 300))

(ert-deftest watchdog-gui/map-wait-stays-busy ()
  (watchdog-modal--with-watchdog
    (let ((f (make-frame '((visibility . nil) (minibuffer . nil)
                           (parent-frame . nil)))))
      (unwind-protect
          (let ((child (make-frame `((parent-frame . ,f) (visibility . nil)
                                     (minibuffer . nil)
                                     (width . 10) (height . 5)))))
            (nidhogg-watchdog--stats t)
            (make-frame-visible child)
            (should (= (plist-get (nidhogg-watchdog--stats) :wrapper-edges) 0))
            (delete-frame child))
        (delete-frame f)))))

;;; watchdog-modal-gui-tests.el ends here
