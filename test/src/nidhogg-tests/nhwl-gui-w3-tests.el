;;; nhwl-gui-w3-tests.el --- GUI rows for the native shell's W3 (the presenter)  -*- lexical-binding: t -*-

;; Run by nidhogg/test/gui.sh with the W1 rows (nhwl-gui-tests.el, loaded first: its helpers are used here).  Rows
;; that drive the compositor need `--compositor recorder'; rows named `nhwl-gui/live-*' run on real compositors and
;; are judged by the trace checker.  Each W3 task appends its rows under its own heading.

(require 'ert)
(require 'nhwl-gui-tests)
(require 'nhwl-gui-w2-tests)                ; nhwl-gui--connected-p

;; W3 Task 8: the presenter thread.  With `nhwl-presenter-thread' on, a thread of the shell composes and commits each
;; tree when its pacer opens; the main thread only publishes.  `(nhwl-presenter-stats)' carries `:main-wait-max-ns',
;; the longest the main thread waited for the presenter.

(defun nhwl-gui--presenter-on-p ()
  "Non-nil when this Emacs presents on the shell's thread."
  (and (boundp 'nhwl-presenter-thread) nhwl-presenter-thread (fboundp 'nhwl-presenter-stats)
       (eql (plist-get (nhwl-presenter-stats) :thread-on) 1)))

(defun nhwl-gui--damaged-toplevel-commits (mark)
  "The toplevel commits since MARK that carry damage."
  (seq-filter (lambda (c) (and (equal (plist-get c :role) "toplevel") (plist-get c :damage)))
              (nhwl-gui--log-since mark)))

(defun nhwl-gui--presenter-switches ()
  "The context switches of this Emacs's `nh-present' thread so far (nil when it has none)."
  (let ((dir (format "/proc/%d/task" (emacs-pid))))
    (seq-some (lambda (tid)
                (let ((comm (expand-file-name (concat tid "/comm") dir)))
                  (when (and (file-readable-p comm)
                             (equal (string-trim (with-temp-buffer
                                                   (insert-file-contents comm) (buffer-string)))
                                    "nh-present"))
                    (with-temp-buffer
                      (insert-file-contents (expand-file-name (concat tid "/status") dir))
                      (let ((n 0))
                        (goto-char (point-min))
                        (while (re-search-forward "ctxt_switches:[ \t]+\\([0-9]+\\)" nil t)
                          (setq n (+ n (string-to-number (match-string 1)))))
                        n)))))
              (directory-files dir nil "\\`[0-9]+\\'"))))

(ert-deftest nhwl-gui/commit-while-lisp-busy ()
  "Review Focus 3: a redisplay publishes while the pacer waits for its frame callback, and Lisp then runs for 200 ms
without returning to the command loop.  The callback comes during that time, and the presenter commits the picture
before the Lisp ends: the commit does not wait for the main loop."
  (skip-unless (and (nhwl-gui--recorder-p) (nhwl-gui--presenter-on-p)))
  (nhwl-gui--with-blink
   nil
   (lambda ()
     (let ((buf (generate-new-buffer " *nhwl-busy*")))
       (unwind-protect
           (progn
             (switch-to-buffer buf)
             (nhwl-gui--pump)
             (let ((mark (nhwl-gui--mark)))
               ;; the first picture commits at once and leaves the pacer waiting; the second waits for the callback
               (insert "a")
               (redisplay t)
               (insert "b")
               (redisplay t)
               (let ((end (+ (float-time) 0.2)))
                 (while (< (float-time) end)))
               ;; still inside the busy Lisp: nothing has run the command loop since the second redisplay
               (should (>= (length (nhwl-gui--damaged-toplevel-commits mark)) 2))))
         (kill-buffer buf))))))

(ert-deftest nhwl-gui/delete-frame-storm-while-presenting ()
  "Review Focus 4: 200 cycles of making and deleting a frame with a child frame and a tooltip while typing, with the
presenter composing and committing all along.  No hang (the row's own deadline is 120 s), the shell stays connected,
and the main thread never waited 1 ms for the presenter.  Under gui.sh `--daemon' too."
  (skip-unless (and (nhwl-gui--recorder-p) (nhwl-gui--presenter-on-p)))
  (let ((t0 (float-time)) (buf (generate-new-buffer " *nhwl-storm*")))
    (unwind-protect
        (with-current-buffer buf
          (dotimes (i 200)
            (let* ((f (make-frame '((width . 40) (height . 10))))
                   (c (make-frame `((parent-frame . ,f) (width . 10) (height . 3) (minibuffer . nil)))))
              (with-selected-frame f
                (switch-to-buffer buf)
                (insert (format "%d " i))
                (redisplay t)
                (x-show-tip "tip" f nil 1)
                (insert "x")
                (redisplay t))
              (x-hide-tip)
              (when (frame-live-p c) (delete-frame c))
              (delete-frame f))
            (when (zerop (% i 20)) (nhwl-gui--pump 0.05))
            (should (< (- (float-time) t0) 120)))
          (nhwl-gui--pump)
          (should (nhwl-gui--connected-p))
          (should (< (plist-get (nhwl-presenter-stats) :main-wait-max-ns) 1000000)))
      (kill-buffer buf))))

(ert-deftest nhwl-gui/presenter-idle-has-no-wakeups ()
  "An idle frame's presenter sleeps: once the quiet period dropped the fronts, its thread is not woken for 3 s."
  (skip-unless (and (nhwl-gui--recorder-p) (nhwl-gui--presenter-on-p)))
  (nhwl-gui--with-blink
   nil
   (lambda ()
     (let ((trims (plist-get (nhwl-presenter-stats) :front-trims)))
       (redisplay t)
       ;; the quiet period (2 s) and the trim's own wake
       (should (nhwl-gui--wait (lambda ()
                                 (> (plist-get (nhwl-presenter-stats) :front-trims) trims))
                               5))
       (let ((before (nhwl-gui--presenter-switches)))
         (should before)
         (sleep-for 3)
         (should (equal (nhwl-gui--presenter-switches) before)))))))

(provide 'nhwl-gui-w3-tests)
;;; nhwl-gui-w3-tests.el ends here
