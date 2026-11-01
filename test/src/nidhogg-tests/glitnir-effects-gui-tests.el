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

(defun glitnir-fx--with-child (transition fn &rest params)
  "Call FN with a child frame of `glitnir-fx--colour' and nothing else, with TRANSITION and PARAMS, just shown.
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
             (setq c (apply #'nhwl-gui--child (cons 'transition transition)
                            (cons 'background-color glitnir-fx--colour)
                            '(left-fringe . 0) '(right-fringe . 0) '(internal-border-width . 0)
                            '(vertical-scroll-bars . nil) '(cursor-type . nil)
                            params))
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

;; Task 7: the backdrop.  A child with one shows the parent's pixels, blurred, through half its background; a row keeps
;; the parent still (its cursor does not blink) but for the change it makes.

(defconst glitnir-fx--blur '((alpha-background . 0.5) (backdrop-blur . 10))
  "The parameters of a child with a backdrop.")

(defun glitnir-fx--backdrop-px (c)
  "Backdrop pixels recomputed for child frame C since its record's last reset."
  (or (plist-get (glitnir-stats c) :backdrop-px) 0))

(defun glitnir-fx--rest (c)
  "Empty child frame C's record once what it last drew has reached the compositor."
  (gui-runner-wait-settled)
  (glitnir-stats c t))

(defun glitnir-fx--spot ()
  "(X . Y) in a grab of the corner of a child frame made as the rows make theirs, with square corners and opaque."
  (let (at)
    (glitnir-fx--with-child
     nil
     (lambda (_c _id _base)
       (setq at (glitnir-fx--wait #'glitnir-fx--found))
       (should at)))
    (gui-runner-wait-settled)
    at))

(defun glitnir-fx--with-text (fn)
  "Call FN with `*scratch*' full of numbered lines in one window, its top line first; it is emptied after."
  (let ((text (with-current-buffer (get-buffer-create "*scratch*") (buffer-string))))
    (unwind-protect
        (progn
          (delete-other-windows)
          (switch-to-buffer "*scratch*")
          (erase-buffer)
          (dotimes (i 400)
            (insert (format "line %03d of the blur rows, with a mark %s at a place of its own\n"
                            i (make-string (% (* i 7) 23) ?#))))
          (goto-char (point-min))
          (redisplay t)
          (funcall fn))
      (with-current-buffer "*scratch*"
        (erase-buffer)
        (insert text)))))

(ert-deftest glitnir-blur-follows-change-beneath ()
  "A change in the parent under the child shows in the child; on the recorder, in the one commit of the tree."
  :tags '(gnome-smoke)
  (skip-unless (glitnir-fx--live-p))
  (let ((at (glitnir-fx--spot)))
    (apply
     #'glitnir-fx--with-child
     nil
     (lambda (c id _base)
       (glitnir-fx--rest c)
       (let ((before (gui-runner-screen-grab))
             (mark (and id (nhwl-gui--mark))))
         (with-current-buffer "*scratch*"
           (goto-char (point-min))
           (dotimes (_ 12) (insert (make-string 60 ?#) "\n")))
         (redisplay t)
         (unwind-protect
             (progn
               (should (glitnir-fx--wait (lambda () (> (glitnir-fx--backdrop-px c) 0))))
               (gui-runner-wait-settled)
               (should (> (cdr (gui-runner-pixel-diff before (gui-runner-screen-grab)
                                                      (glitnir-fx--rect c at)))
                          0))
               (when id
                 (let* ((kid (car (glitnir-fx--buffers id mark)))
                        (by (or (nhwl-gui--get kid :applied_by) (plist-get kid :seq))))
                   (should kid)
                   (should (seq-find (lambda (r) (eql (plist-get r :seq) by)) (nhwl-gui--new mark))))))
           (with-current-buffer "*scratch*" (erase-buffer)))))
     glitnir-fx--blur)))

(ert-deftest glitnir-typing-in-blurred-child-no-reblur ()
  "50 keys typed into a child with a backdrop recompute none of it."
  (skip-unless (glitnir-fx--recorder-p))
  (apply
   #'glitnir-fx--with-child
   nil
   (lambda (c id _base)
     (glitnir-fx--rest c)
     (let ((mark (nhwl-gui--mark))
           (buf (window-buffer (frame-root-window c))))
       (dotimes (_ 50)
         (should (glitnir-fx--next-key))
         (with-current-buffer buf (insert "x"))
         (redisplay t)
         (gui-runner-wait-settled))
       (should (> (length (glitnir-fx--buffers id mark)) 0))
       (should (= (glitnir-fx--backdrop-px c) 0))))
   glitnir-fx--blur))

(ert-deftest glitnir-blur-off-identical ()
  "A child with an opaque background grabs the same with `backdrop-blur' as without, and recomputes nothing."
  (skip-unless (glitnir-fx--live-p))
  (let (with without)
    (glitnir-fx--with-child
     nil
     (lambda (c _id _base)
       (gui-runner-wait-settled)
       (setq with (gui-runner-screen-grab))
       (should (= (glitnir-fx--backdrop-px c) 0)))
     '(alpha-background . 1.0) '(backdrop-blur . 10))
    (gui-runner-wait-settled)
    (glitnir-fx--with-child
     nil
     (lambda (_c _id _base)
       (gui-runner-wait-settled)
       (setq without (gui-runner-screen-grab)))
     '(alpha-background . 1.0))
    (should (eql 0 (cdr (gui-runner-pixel-diff with without))))))

(ert-deftest glitnir-blur-corners ()
  "Rounded corners: the pixel at a corner is what lies beneath, the blur stops there."
  (skip-unless (glitnir-fx--live-p))
  (let ((at (glitnir-fx--spot)))
    (apply
     #'glitnir-fx--with-child
     nil
     (lambda (c _id base)
       (gui-runner-wait-settled)
       (let* ((grab (gui-runner-screen-grab))
              (right (+ (car at) (frame-pixel-width c) -1))
              (bottom (+ (cdr at) (frame-pixel-height c) -1)))
         (dolist (p (list (cons (car at) (cdr at)) (cons right (cdr at))
                          (cons (car at) bottom) (cons right bottom)))
           (should (eql 0 (cdr (gui-runner-pixel-diff base grab (list (car p) (cdr p) 1 1))))))))
     '(corner-radius . 8) glitnir-fx--blur)))

(ert-deftest glitnir-blur-over-scrolling-window ()
  "20 line scrolls of a promoted window beneath: the child follows each, and ends as a blur made afresh."
  (skip-unless (and (glitnir-fx--recorder-p) (fboundp 'nhwl--scroll-layers)
                    (boundp 'nhwl-scroll-layers) nhwl-scroll-layers))
  (let ((at (glitnir-fx--spot)) grab rect)
    (glitnir-fx--with-text
     (lambda ()
       (apply
        #'glitnir-fx--with-child
        nil
        (lambda (c id _base)
          (glitnir-fx--rest c)
          (let ((mark (nhwl-gui--mark)) (layers 0))
            (dotimes (_ 20)
              (scroll-up 1)
              (redisplay t)
              (setq layers (max layers (nhwl--scroll-layers)))
              (gui-runner-wait-settled))
            (should (= layers 1))
            (should (>= (length (glitnir-fx--buffers id mark)) 20))
            (should (> (glitnir-fx--backdrop-px c) 0))
            (setq grab (gui-runner-screen-grab)
                  rect (glitnir-fx--rect c at))))
        glitnir-fx--blur)
       (gui-runner-wait-settled)
       (apply
        #'glitnir-fx--with-child
        nil
        (lambda (_c _id _base)
          (gui-runner-wait-settled)
          (should (eql 0 (cdr (gui-runner-pixel-diff grab (gui-runner-screen-grab) rect)))))
        glitnir-fx--blur)))))

(ert-deftest glitnir-blur-scale-change ()
  "From scale 1 to 1.25 with the child open: it recomputes, at the new scale, and nothing breaks on the way."
  (skip-unless (glitnir-fx--recorder-p))
  (apply
   #'glitnir-fx--with-child
   nil
   (lambda (c _id _base)
     (glitnir-fx--rest c)
     (unwind-protect
         (progn
           (gui-runner-recorder '(:cmd "scale" :value120 150))
           (should (nhwl-gui--wait (lambda () (nhwl-gui--pump) (redisplay t) (= (frame-scale-factor) 1.25)) 5))
           (should (glitnir-fx--wait (lambda () (redisplay t) (> (glitnir-fx--backdrop-px c) 0))))
           (gui-runner-wait-settled t)
           (should (glitnir-fx--connected-p)))
       (gui-runner-recorder '(:cmd "scale" :value120 120))
       (nhwl-gui--wait (lambda () (nhwl-gui--pump) (redisplay t) (= (frame-scale-factor) 1.0)) 5)
       (gui-runner-wait-settled t)))
   glitnir-fx--blur))

(ert-deftest glitnir-fade-with-blur ()
  "A child with a backdrop fades out and leaves nothing; the blur fades with it and is not recomputed."
  (skip-unless (glitnir-fx--recorder-p))
  (let ((at (glitnir-fx--spot)))
    (apply
     #'glitnir-fx--with-child
     'pop
     (lambda (c id base)
       (should (glitnir-fx--wait (lambda () (> (glitnir-fx--backdrop-px c) 0))))
       (should (glitnir-fx--wait (lambda () (> (length (glitnir-fx--buffers id 0)) 1))))
       (glitnir-fx--rest c)
       (make-frame-invisible c t)
       (should (nhwl-gui--wait (lambda () (glitnir-fx--unmapped-p id)) 3))
       (gui-runner-wait-settled)
       (should (= (glitnir-fx--backdrop-px c) 0))
       (should (glitnir-fx--clean base (glitnir-fx--rect c at)))
       (should (glitnir-fx--connected-p)))
     glitnir-fx--blur)))

;;; glitnir-effects-gui-tests.el ends here
