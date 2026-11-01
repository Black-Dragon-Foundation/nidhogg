;;; glitnir-effects-gui-tests.el --- GUI rows for child-frame motion, blur and window cards  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/gui.sh with `--files glitnir-effects-gui-tests.el': on the recorder with its sway profile, again
;; with `--recorder-profile mutter' (no alpha modifier) and `--recorder-profile kwin' (the modifier); the `gnome-smoke'
;; rows also on real headless mutter.  `glitnir-pop-in-lisp-busy' runs again with `--env NHWL_PRESENTER=0'.
;;
;; Every row waits on what the recorder, the frame or a grab reports; a deadline bounds a failure and never decides a
;; pass.  "Mid-leg" means the leg's first buffer is in the recorder's log: never a number of milliseconds.  A row that
;; must act inside a leg gives the leg a long duration, so a slow machine cannot end it first.  Sway discards the
;; feedback of a root commit that only carried a child, so legs there run on the real clock, which ends them all the
;; same: no row counts a leg's frames exactly.

(require 'ert)
(require 'cl-lib)
(require 'seq)
(require 'subr-x)
(require 'nhwl-gui-tests
         (expand-file-name "nhwl-gui-tests" (file-name-directory (or load-file-name buffer-file-name))))

(defconst glitnir-fx--colour "#2040a0"
  "A child frame's background in the rows.")

(defconst glitnir-fx--bgr '(160 64 32)
  "`glitnir-fx--colour' as a grab holds it: B, G, R.")

(defconst glitnir-fx--leg-frames 9
  "The most frames a default leg takes on the recorder's 60 Hz: 120 ms of them, and one at rest.")

(defun glitnir-fx--live-p ()
  "Non-nil on the native shell with the chrome vector."
  (and (nhwl-gui--live-p) (fboundp 'glitnir--chrome-vector)))

(defun glitnir-fx--recorder-p ()
  "Non-nil when the row can read the recorder too."
  (and (glitnir-fx--live-p) (nhwl-gui--recorder-p)))

(defun glitnir-fx--with-child (transition fn)
  "Call FN with a child frame of `glitnir-fx--colour' and nothing else, with TRANSITION, just shown.
FN gets the frame, its surface's id on the recorder (else nil) and a grab from before the frame was made.  On the
recorder the leg's first buffer is in the log by then.  The frame goes after, and the parent's cursor does not blink."
  (nhwl-gui--with-blink
   nil
   (lambda ()
     (switch-to-buffer "*scratch*")
     (message nil)
     (redisplay t)
     (gui-runner-wait-settled)
     (let ((base (gui-runner-screen-grab))
           (before (and (nhwl-gui--recorder-p) (nhwl-gui--sub-ids)))
           (buf (generate-new-buffer " *glitnir-fx*"))
           (c nil))
       (unwind-protect
           (progn
             (with-current-buffer buf (setq mode-line-format nil))
             (setq c (nhwl-gui--child (cons 'transition transition)
                                      (cons 'background-color glitnir-fx--colour)
                                      '(left-fringe . 0) '(right-fringe . 0) '(internal-border-width . 0)
                                      '(vertical-scroll-bars . nil) '(cursor-type . nil)))
             (set-window-buffer (frame-root-window c) buf)
             (redisplay t)
             (funcall fn c (and (nhwl-gui--recorder-p) (plist-get (nhwl-gui--child-node before) :id)) base))
         (when (frame-live-p c) (delete-frame c t))
         (kill-buffer buf))))))

(defun glitnir-fx--ticks (c)
  "Motion frames of child frame C so far."
  (or (plist-get (glitnir-stats c) :ticks) 0))

(defun glitnir-fx--wait (pred &optional seconds)
  "Read events until PRED returns non-nil, SECONDS (default 10) at most; return its value."
  (nhwl-gui--wait (lambda () (nhwl-gui--pump) (funcall pred)) (or seconds 10)))

(defun glitnir-fx--found ()
  "(X . Y) in a grab of the child's top-left corner while the child is whole and opaque, else nil.
A leg's frames multiply the colour, so a block of it holds at rest only."
  (gui-runner-grab-find-block (gui-runner-screen-grab) glitnir-fx--bgr 20 20))

(defun glitnir-fx--rect (c at)
  "The area child frame C covers in a grab, its corner AT."
  (list (car at) (cdr at) (frame-pixel-width c) (frame-pixel-height c)))

(defun glitnir-fx--clean (base rect)
  "Non-nil when a new grab equals the grab BASE over RECT."
  (eql 0 (cdr (gui-runner-pixel-diff base (gui-runner-screen-grab) rect))))

(defun glitnir-fx--buffers (id mark)
  "Surface ID's commits with a buffer since MARK."
  (seq-filter (lambda (c) (nhwl-gui--get c :buffer)) (nhwl-gui--surface-commits id mark)))

(defun glitnir-fx--unmapped-p (id)
  "Non-nil when surface ID shows no buffer."
  (not (nhwl-gui--get (nhwl-gui--node id) :buffer)))

(defun glitnir-fx--connected-p ()
  "Non-nil while the recorder has a client: a protocol error would have dropped it."
  (plist-get (gui-runner-recorder '(:cmd "clients")) :clients))

(defun glitnir-fx--presenter (key)
  "Counter KEY of `nhwl-presenter-stats'."
  (plist-get (nhwl-presenter-stats) key))

(defun glitnir-fx--presenter-wakes ()
  "The times the presenter's thread has gone to sleep, which its stats do not count; nil with no thread."
  (when-let* ((task (seq-find (lambda (id)
                                (with-temp-buffer
                                  (insert-file-contents (format "/proc/self/task/%s/comm" id))
                                  (equal (string-trim (buffer-string)) "nh-present")))
                              (directory-files "/proc/self/task" nil "\\`[0-9]"))))
    (with-temp-buffer
      (insert-file-contents (format "/proc/self/task/%s/status" task))
      (re-search-forward "^voluntary_ctxt_switches:[ \t]*\\([0-9]+\\)")
      (string-to-number (match-string 1)))))

(defun glitnir-fx--next-key ()
  "Type a key on the focused window; non-nil once Lisp has read it."
  (let ((got nil))
    (gui-runner-recorder '(:cmd "key" :text "a"))
    (nhwl-gui--wait (lambda ()
                      (gui-runner-read-all (lambda (e) (when (eq e ?a) (setq got t))))
                      got)
                    5)))

;; Task 5: motion.

(ert-deftest glitnir-pop-in-ends-opaque ()
  "A pop-in runs frames of its own and ends in the child's own colour."
  :tags '(gnome-smoke)
  (skip-unless (glitnir-fx--live-p))
  (glitnir-fx--with-child
   'pop
   (lambda (c _id _base)
     (should (glitnir-fx--wait (lambda () (> (glitnir-fx--ticks c) 0))))
     (should (glitnir-fx--wait #'glitnir-fx--found)))))

(ert-deftest glitnir-fade-out-no-residue ()
  "A fade-out runs frames of its own, and the area then equals the grab from before the child."
  :tags '(gnome-smoke)
  (skip-unless (glitnir-fx--live-p))
  (glitnir-fx--with-child
   'pop
   (lambda (c _id base)
     (let* ((at (glitnir-fx--wait #'glitnir-fx--found))
            (rect (and at (glitnir-fx--rect c at)))
            (ticks (glitnir-fx--ticks c)))
       (should at)
       (make-frame-invisible c t)
       (should (glitnir-fx--wait (lambda () (glitnir-fx--clean base rect))))
       (should (> (glitnir-fx--ticks c) ticks))))))

(ert-deftest glitnir-hide-show-one-command-no-motion ()
  "A hide and a show in one command run no leg: the presenter's thread may wake between them, and the hide waits."
  (skip-unless (glitnir-fx--recorder-p))
  (glitnir-fx--with-child
   'pop
   (lambda (c id _base)
     (gui-runner-wait-settled)
     (let ((ticks (glitnir-fx--ticks c)))
       ;; no collection between the two calls
       (let ((gc-cons-threshold most-positive-fixnum))
         (make-frame-invisible c t)
         (make-frame-visible c))
       (redisplay t)
       (gui-runner-wait-settled)
       (should (= (glitnir-fx--ticks c) ticks))
       (should (nhwl-gui--get (nhwl-gui--node id) :mapped))))))

(ert-deftest glitnir-churn-no-pile ()
  "The corfu pattern fifty times: a hide from a timer, a show on the next key.  Half the shows come mid-leg.
No leg piles up on another: the child's buffers stay within its content and one leg per churn, and nothing is left."
  (skip-unless (glitnir-fx--recorder-p))
  (glitnir-fx--with-child
   'pop
   (lambda (c id base)
     (let* ((at (glitnir-fx--wait #'glitnir-fx--found))
            (rect (and at (glitnir-fx--rect c at)))
            (buf (window-buffer (frame-root-window c)))
            (mark nil) (packets nil))
       (should at)
       (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))
       (gui-runner-wait-settled)
       (setq mark (nhwl-gui--mark) packets (glitnir-fx--presenter :packets))
       (dotimes (i 50)
         (let ((hidden nil) (ticks (glitnir-fx--ticks c)))
           (run-at-time 0 nil (lambda () (make-frame-invisible c t) (setq hidden t)))
           (should (gui-runner-wait (lambda () hidden)))
           (when (cl-oddp i)
             (should (glitnir-fx--wait (lambda () (> (glitnir-fx--ticks c) ticks)) 3))))
         (should (glitnir-fx--next-key))
         (with-current-buffer buf (insert "x"))
         (make-frame-visible c)
         (redisplay t))
       (gui-runner-wait-settled)
       (should (<= (length (glitnir-fx--buffers id mark))
                   (+ (- (glitnir-fx--presenter :packets) packets) (* 50 glitnir-fx--leg-frames))))
       (make-frame-invisible c t)
       (should (glitnir-fx--wait (lambda () (glitnir-fx--clean base rect))))))))

(ert-deftest glitnir-delete-during-fade ()
  "A child deleted mid-leg finishes the fade on a surface of the shell's, then the surface goes."
  (skip-unless (glitnir-fx--recorder-p))
  (glitnir-fx--with-child
   '(:show pop :hide fade :hide-duration 1)
   (lambda (c id _base)
     (gui-runner-wait-settled)
     (let ((mark (nhwl-gui--mark)))
       (make-frame-invisible c t)
       (should (nhwl-gui--wait (lambda () (glitnir-fx--buffers id mark)) 5))
       (delete-frame c t)
       (let ((gone (nhwl-gui--mark)))
         (should (memq id (nhwl-gui--sub-ids)))
         (should (nhwl-gui--wait (lambda () (glitnir-fx--buffers id gone)) 5))
         (should (nhwl-gui--wait (lambda () (not (memq id (nhwl-gui--sub-ids)))) 5)))
       (should (glitnir-fx--connected-p))))))

(ert-deftest glitnir-parent-deleted-during-fade ()
  "A parent deleted during its child's fade takes the fading surface with it, long before the fade would end."
  (skip-unless (glitnir-fx--recorder-p))
  (let ((old (selected-frame)) (before (nhwl-gui--sub-ids)) (p nil))
    (unwind-protect
        (progn
          (setq p (make-frame '((width . 60) (height . 20))))
          (select-frame p)
          (let* ((c (nhwl-gui--child '(transition . (:show pop :hide fade :hide-duration 5))))
                 (id (progn (redisplay t) (plist-get (nhwl-gui--child-node before) :id))))
            (should id)
            (gui-runner-wait-settled)
            (let ((mark (nhwl-gui--mark)))
              (make-frame-invisible c t)
              (should (nhwl-gui--wait (lambda () (glitnir-fx--buffers id mark)) 5))
              (select-frame old)
              (delete-frame p t)
              (should (nhwl-gui--wait (lambda () (not (memq id (nhwl-gui--sub-ids)))) 2))
              (should (glitnir-fx--connected-p)))))
      (select-frame old)
      (when (frame-live-p p) (delete-frame p t)))))

(ert-deftest glitnir-reduced-motion ()
  "With the desktop's reduced motion on, a child with a transition shows and hides with no frame of motion.
The fake portal answers every read with NotFound, so the row hands the shell's handler the signal itself."
  (skip-unless (and (glitnir-fx--recorder-p) (fboundp 'nhwl-settings--signal)))
  (unwind-protect
      (progn
        (nhwl-settings--signal "org.freedesktop.appearance" "reduced-motion" 1)
        (glitnir-fx--with-child
         'pop
         (lambda (c id _base)
           (gui-runner-wait-settled)
           (let ((mark (nhwl-gui--mark)))
             (make-frame-invisible c t)
             (should (nhwl-gui--wait (lambda () (glitnir-fx--unmapped-p id)) 3))
             (should-not (glitnir-fx--buffers id mark))
             (should (= (glitnir-fx--ticks c) 0))))))
    (nhwl-settings--signal "org.freedesktop.appearance" "reduced-motion" 0)))

(ert-deftest glitnir-idle-after-motion ()
  "After a leg's end nothing commits and the presenter's thread sleeps.
A front dropped after its quiet period is the timer's one wake-up, and counts for itself."
  (skip-unless (glitnir-fx--recorder-p))
  (glitnir-fx--with-child
   'pop
   (lambda (c id _base)
     (gui-runner-wait-settled)
     (make-frame-invisible c t)
     (should (nhwl-gui--wait (lambda () (glitnir-fx--unmapped-p id)) 5))
     (gui-runner-wait-settled)
     (let ((mark (nhwl-gui--mark))
           (wakes (glitnir-fx--presenter-wakes))
           (trims (glitnir-fx--presenter :front-trims)))
       ;; the span in which nothing may happen
       (nhwl-gui--observe 1)
       (should-not (nhwl-gui--log-since mark))
       (when wakes
         (should (<= (- (glitnir-fx--presenter-wakes) wakes)
                     (- (glitnir-fx--presenter :front-trims) trims))))))))

(ert-deftest glitnir-input-during-pop-final-geometry ()
  "A click mid-leg, 2 px inside the final edge, lands in the child: a pop scales the picture, not the surface."
  (skip-unless (glitnir-fx--recorder-p))
  (glitnir-fx--with-child
   '(:show pop :duration 1 :hide fade)
   (lambda (c id _base)
     (let* ((s (frame-scale-factor c))
            (w (frame-pixel-width c)) (h (frame-pixel-height c))
            (x (- (floor (/ w s)) 2)) (y (- (floor (/ h s)) 2))
            (events nil))
       (unwind-protect
           (progn
             (gui-runner-recorder (list :cmd "focus" :surface id))
             (gui-runner-recorder (list :cmd "pointer" :pos (vector x y)))
             (gui-runner-recorder '(:cmd "button" :button 272 :state "press"))
             (gui-runner-recorder '(:cmd "button" :button 272 :state "release"))
             (let* ((click (nhwl-gui--wait
                            (lambda ()
                              (gui-runner-read-all (lambda (e) (push e events)))
                              (seq-find (lambda (e) (and (consp e) (memq (car e) '(down-mouse-1 mouse-1)))) events))
                            5))
                    (posn (and click (event-start click)))
                    (win (and posn (posn-window posn))))
               (should click)
               (should (eq (if (framep win) win (window-frame win)) c))
               (should (>= (car (posn-x-y posn)) (- w (ceiling (* 3 s)))))
               (should (>= (cdr (posn-x-y posn)) (- h (ceiling (* 3 s)))))))
         (ignore-errors (gui-runner-recorder '(:cmd "focus" :surface "toplevel"))))))))

(ert-deftest glitnir-pop-in-lisp-busy ()
  "Lisp busy for 0.2 s during a pop-in.
Presenter thread: the leg's buffers all go during the stall, before a commit made right after it.
Inline: the leg's time passes unseen, and at most two commits jump to the end."
  (skip-unless (glitnir-fx--recorder-p))
  (glitnir-fx--with-child
   'pop
   (lambda (_c id _base)
     (let ((done nil) (mark nil))
       (run-at-time 0 nil (lambda () (gui-runner-stimulus 0.2) (setq done t)))
       (should (gui-runner-wait (lambda () done)))
       (setq mark (nhwl-gui--mark))
       (insert "x")
       (redisplay t)
       (should (nhwl-gui--wait (lambda () (nhwl-gui--new mark)) 5))
       (gui-runner-wait-settled)
       (if (eql (glitnir-fx--presenter :thread-on) 1)
           (let ((fence (plist-get (car (nhwl-gui--new mark)) :seq)))
             (dolist (b (glitnir-fx--buffers id 0))
               (should (< (or (nhwl-gui--get b :applied_by) (plist-get b :seq)) fence))))
         (should (<= (length (glitnir-fx--buffers id mark)) 2))
         (should (glitnir-fx--wait #'glitnir-fx--found)))))))

(ert-deftest glitnir-motion-off-parity ()
  "With no `transition' the child has no motion: no ticks, and its buffers are its content's."
  (skip-unless (glitnir-fx--recorder-p))
  (glitnir-fx--with-child
   nil
   (lambda (c id _base)
     (gui-runner-wait-settled)
     (let ((mark (nhwl-gui--mark))
           (packets (glitnir-fx--presenter :packets))
           (buf (window-buffer (frame-root-window c))))
       (dotimes (_ 3)
         (with-current-buffer buf (insert "x"))
         (redisplay t)
         (gui-runner-wait-settled))
       (make-frame-invisible c t)
       (should (nhwl-gui--wait (lambda () (glitnir-fx--unmapped-p id)) 3))
       (make-frame-visible c)
       (redisplay t)
       (gui-runner-wait-settled)
       (should (= (glitnir-fx--ticks c) 0))
       (should (<= (length (glitnir-fx--buffers id mark))
                   (- (glitnir-fx--presenter :packets) packets)))))))

;;; glitnir-effects-gui-tests.el ends here
