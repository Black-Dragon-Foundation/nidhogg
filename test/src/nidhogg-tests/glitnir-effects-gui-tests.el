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

;; Task 11: window cards.  Two windows side by side, the left a card, a gap of 6 logical px between them in the
;; divider colour.  Each buffer has a colour of its own, no mode line, no cursor and no fringes, so the left window's
;; first block of its colour in a grab is its top row right of the top-left corner square: the card's corner is there
;; minus the radius.  The rows run at the lane's scale (1, 2 and 1.25).

(defconst glitnir-fx--card-bg "#3070c0" "The card window's background.")
(defconst glitnir-fx--card-bgr '(192 112 48) "`glitnir-fx--card-bg' as a grab holds it.")
(defconst glitnir-fx--other-bg "#c07030" "The other window's background.")
(defconst glitnir-fx--other-bgr '(48 112 192) "`glitnir-fx--other-bg' as a grab holds it.")
(defconst glitnir-fx--gap "#20a040" "The divider colour: the gap, and what the card's corners show.")
(defconst glitnir-fx--gap-bgr '(64 160 32) "`glitnir-fx--gap' as a grab holds it.")
(defconst glitnir-fx--radius 10 "The card's corner radius, logical px.")

(defun glitnir-fx--card-r ()
  "The card's corner radius in device px at the frame's scale, as the glue rounds it."
  (floor (+ 0.5 (* glitnir-fx--radius (frame-scale-factor)))))

(defun glitnir-fx--redraw (&rest windows)
  "Draw WINDOWS again (a parameter change draws nothing by itself) and wait until it reached the compositor."
  (dolist (w windows) (force-window-update w))
  (redisplay t)
  (gui-runner-wait-settled))

(defun glitnir-fx--card-box (w)
  "Card window W in a grab, (X Y WIDTH HEIGHT): found from its colour, sized from its edges without dividers."
  (let ((at (gui-runner-grab-find-block (gui-runner-screen-grab) glitnir-fx--card-bgr 20 20))
        (r (if (window-parameter w 'corner-radius) (glitnir-fx--card-r) 0)))
    (should at)
    (list (- (car at) r) (cdr at)
          (- (window-pixel-width w) (window-right-divider-width w))
          (- (window-pixel-height w) (window-bottom-divider-width w)))))

(defun glitnir-fx--corners (box r)
  "The four R x R corner squares of BOX, each (X Y R R)."
  (pcase-let ((`(,x ,y ,w ,h) box))
    (list (list x y r r) (list (+ x w (- r)) y r r)
          (list x (+ y h (- r)) r r) (list (+ x w (- r)) (+ y h (- r)) r r))))

(defun glitnir-fx--pixel (grab x y)
  "Pixel (X . Y) of GRAB as (B G R)."
  (let ((o (* 4 (+ x (* y (nth 0 grab))))) (b (nth 2 grab)))
    (list (aref b o) (aref b (+ o 1)) (aref b (+ o 2)))))

(defun glitnir-fx--near (px want)
  "Non-nil when every channel of PX is within 2 of WANT's."
  (cl-every (lambda (a b) (<= (abs (- a b)) 2)) px want))

(defun glitnir-fx--with-cards (params fn)
  "Call FN with the left and right windows of the rows' layout, the left with window PARAMS, drawn and settled.
Everything goes back after: one window on `*scratch*', the frame's divider width and colour."
  (nhwl-gui--with-blink
   nil
   (lambda ()
     (let* ((f (selected-frame))
            (gap (frame-parameter f 'right-divider-width))
            (divider (face-attribute 'window-divider :foreground f))
            (a (get-buffer-create " *glitnir-card-a*"))
            (b (get-buffer-create " *glitnir-card-b*"))
            left right)
       (unwind-protect
           (progn
             (delete-other-windows)
             (set-face-foreground 'window-divider glitnir-fx--gap f)
             (set-frame-parameter f 'right-divider-width (round (* 6 (frame-scale-factor))))
             (dolist (x (list (cons a glitnir-fx--card-bg) (cons b glitnir-fx--other-bg)))
               (with-current-buffer (car x)
                 (setq mode-line-format nil cursor-type nil)
                 (face-remap-add-relative 'default :background (cdr x))))
             (setq left (selected-window) right (split-window-right))
             (set-window-buffer left a)
             (set-window-buffer right b)
             (dolist (w (list left right)) (set-window-fringes w 0 0))
             (dolist (p params) (set-window-parameter left (car p) (cdr p)))
             (message nil)
             (glitnir-fx--redraw left right)
             (funcall fn left right))
         (when (window-live-p left)
           (dolist (k '(corner-radius shadow ring)) (set-window-parameter left k nil)))
         (delete-other-windows)
         (switch-to-buffer "*scratch*")
         (set-frame-parameter f 'right-divider-width gap)
         (set-face-attribute 'window-divider f :foreground divider)
         (kill-buffer a)
         (kill-buffer b)
         (glitnir-fx--redraw (selected-window)))))))

(defconst glitnir-fx--card `((corner-radius . ,glitnir-fx--radius))
  "A card with corners only.")

(defconst glitnir-fx--card-look
  `((corner-radius . ,glitnir-fx--radius) (shadow . (:blur 3 :offset (0 . 1) :opacity 0.45))
    (ring . (,glitnir-fx--gap . 0.6)))
  "The look's card: corners, a soft shadow one px down and the edge ring.")

(ert-deftest glitnir-card-corner-shows-gap ()
  "A card window's corner pixel is the gap's colour; the same window without a card shows its own there."
  :tags '(gnome-smoke)
  (skip-unless (glitnir-fx--live-p))
  (glitnir-fx--with-cards
   glitnir-fx--card
   (lambda (left _right)
     (should (window-parameter left 'glitnir--card))
     (let* ((box (glitnir-fx--card-box left))
            (grab (gui-runner-screen-grab)))
       (dolist (sq (glitnir-fx--corners box (glitnir-fx--card-r)))
         (should (glitnir-fx--near (glitnir-fx--pixel grab (nth 0 sq) (nth 1 sq)) glitnir-fx--gap-bgr)))
       ;; a pixel on the top edge past the corner square is the window's
       (should (glitnir-fx--near (glitnir-fx--pixel grab (+ (nth 0 box) (glitnir-fx--card-r)) (nth 1 box))
                                 glitnir-fx--card-bgr))
       (set-window-parameter left 'corner-radius nil)
       (glitnir-fx--redraw left)
       (should (glitnir-fx--near (glitnir-fx--pixel (gui-runner-screen-grab) (nth 0 box) (nth 1 box))
                                 glitnir-fx--card-bgr))))))

(ert-deftest glitnir-card-shadow-only-in-gap ()
  "The look's card leaves the other window's pixels as they are without it; the gap beside the card is darker."
  (skip-unless (glitnir-fx--live-p))
  (glitnir-fx--with-cards
   glitnir-fx--card-look
   (lambda (left right)
     (let* ((box (glitnir-fx--card-box left))
            (with (gui-runner-screen-grab))
            (other (gui-runner-grab-find-block with glitnir-fx--other-bgr 20 20))
            (other-rect (list (car other) (cdr other)
                              (- (window-pixel-width right) 2) (- (window-pixel-height right) 2)))
            (gap (list (+ (nth 0 box) (nth 2 box)) (+ (nth 1 box) 20)
                       (window-right-divider-width left) 20)))
       (should other)
       (dolist (k '(corner-radius shadow ring)) (set-window-parameter left k nil))
       (glitnir-fx--redraw left right)
       (let ((without (gui-runner-screen-grab)))
         (should (eql 0 (cdr (gui-runner-pixel-diff with without other-rect))))
         (should (> (cdr (gui-runner-pixel-diff with without gap)) 0)))))))

(ert-deftest glitnir-card-typing-damage ()
  "50 keys typed in a card window: no commit's damage reaches a corner square, the cursor being in the middle."
  (skip-unless (glitnir-fx--recorder-p))
  (glitnir-fx--with-cards
   glitnir-fx--card
   (lambda (left _right)
     (let* ((box (glitnir-fx--card-box left))
            (squares (glitnir-fx--corners box (glitnir-fx--card-r)))
            (buf (window-buffer left))
            (meets (lambda (a b)
                     (pcase-let ((`(,ax ,ay ,aw ,ah) a) (`(,bx ,by ,bw ,bh) b))
                       (and (< ax (+ bx bw)) (< bx (+ ax aw)) (< ay (+ by bh)) (< by (+ ay ah)))))))
       (with-current-buffer buf
         (dotimes (_ 12) (insert "\n"))
         (forward-line -6)
         (insert "  "))
       (set-window-point left (with-current-buffer buf (point)))
       (glitnir-fx--redraw left)
       (let ((mark (nhwl-gui--mark)))
         (dotimes (_ 50)
           (should (glitnir-fx--next-key))
           (with-current-buffer buf (insert "x"))
           (redisplay t)
           (gui-runner-wait-settled))
         (let ((commits (seq-filter #'nhwl-gui--damage (nhwl-gui--toplevel-all-commits mark))))
           (should commits)
           (dolist (c commits)
             (dolist (d (nhwl-gui--damage c))
               (should-not (seq-find (lambda (sq) (funcall meets d sq)) squares))))))))))

(ert-deftest glitnir-card-removed-no-residue ()
  "A card that goes leaves the frame as it was before it came."
  (skip-unless (glitnir-fx--live-p))
  (glitnir-fx--with-cards
   nil
   (lambda (left _right)
     (let ((base (gui-runner-screen-grab)))
       (dolist (p glitnir-fx--card-look) (set-window-parameter left (car p) (cdr p)))
       (glitnir-fx--redraw left)
       (should (> (cdr (gui-runner-pixel-diff base (gui-runner-screen-grab))) 0))
       (dolist (p glitnir-fx--card-look) (set-window-parameter left (car p) nil))
       (glitnir-fx--redraw left)
       (should (eql 0 (cdr (gui-runner-pixel-diff base (gui-runner-screen-grab)))))))))

(defun glitnir-fx--outputs ()
  "How many of the frame's windows are their own outputs: `nhwl--scroll-layers' counts each card too."
  (- (nhwl--scroll-layers)
     (seq-count (lambda (w) (window-parameter w 'glitnir--card)) (window-list nil t))))

(defun glitnir-fx--fill (w)
  "Fill window W's buffer with numbered lines, its top line first, after the box was found."
  (with-current-buffer (window-buffer w)
    (dotimes (i 300)
      (insert (format "line %03d of the card rows %s\n" i (make-string (% (* i 7) 23) ?#))))
    (goto-char (point-min)))
  (set-window-start w 1)
  (set-window-point w 1))

(defun glitnir-fx--scroll-corners (w box steps)
  "Scroll window W one line STEPS times from its top; after each, the grab of its four corner squares in BOX."
  (set-window-start w 1)
  (set-window-point w 1)
  (glitnir-fx--redraw w)
  (let (out (most 0))
    (dotimes (_ steps)
      (with-selected-window w (scroll-up 1))
      (redisplay t)
      (setq most (max most (glitnir-fx--outputs)))
      (gui-runner-wait-settled)
      (let ((grab (gui-runner-screen-grab)))
        (push (cons grab (glitnir-fx--corners box (glitnir-fx--card-r))) out)))
    (cons most (nreverse out))))

(defun glitnir-fx--same-corners (a b)
  "Non-nil when every corner square of steps A equals B's, step by step."
  (cl-every (lambda (x y)
              (cl-every (lambda (sq) (eql 0 (cdr (gui-runner-pixel-diff (car x) (car y) sq)))) (cdr x)))
            a b))

(defmacro glitnir-fx--with-scroll-layers (value &rest body)
  "Run BODY with `nhwl-scroll-layers' VALUE, restored after."
  (declare (indent 1))
  `(let ((old nhwl-scroll-layers))
     (unwind-protect (progn (setq nhwl-scroll-layers ,value) ,@body)
       (setq nhwl-scroll-layers old))))

(ert-deftest glitnir-card-promoted-scroll-corners ()
  "A card window's full-window scrolls promote it; after each step its corners grab the same as without promotion."
  :tags '(gnome-smoke)
  (skip-unless (and (glitnir-fx--live-p) (fboundp 'nhwl--scroll-layers) (boundp 'nhwl-scroll-layers)))
  (glitnir-fx--with-cards
   glitnir-fx--card-look
   (lambda (left _right)
     (let ((box (glitnir-fx--card-box left)) promoted plain)
       (glitnir-fx--fill left)
       (glitnir-fx--with-scroll-layers t
         (setq promoted (glitnir-fx--scroll-corners left box 10)))
       (should (= (car promoted) 1))
       (when (glitnir-fx--recorder-p)
         (should (string-search "content" (format "%S" (gui-runner-recorder '(:cmd "tree"))))))
       (glitnir-fx--with-scroll-layers nil
         (glitnir-fx--redraw left)
         (should (= (glitnir-fx--outputs) 0))
         (setq plain (glitnir-fx--scroll-corners left box 10)))
       (should (= (car plain) 0))
       (should (glitnir-fx--same-corners (cdr promoted) (cdr plain)))))))

(ert-deftest glitnir-card-promoted-demote-no-residue ()
  "After the idle demotion of a promoted card window, the window grabs as it did while promoted."
  (skip-unless (and (glitnir-fx--live-p) (fboundp 'nhwl--scroll-layers) (boundp 'nhwl-scroll-layers)))
  (glitnir-fx--with-cards
   glitnir-fx--card-look
   (lambda (left _right)
     (let ((box (glitnir-fx--card-box left)))
       (glitnir-fx--fill left)
       (glitnir-fx--with-scroll-layers t
         (should (= (car (glitnir-fx--scroll-corners left box 3)) 1))
         (let ((before (gui-runner-screen-grab)))
           ;; demotion runs at a publish two seconds after the last scroll: the echo area draws, the window does not
           (should (gui-runner-wait (lambda ()
                                      (message "%s" (random 1000))
                                      (redisplay t)
                                      (= (glitnir-fx--outputs) 0))
                                    6))
           (message nil)
           (redisplay t)
           (gui-runner-wait-settled)
           (should (eql 0 (cdr (gui-runner-pixel-diff before (gui-runner-screen-grab) box))))))))))

(ert-deftest glitnir-card-theme-switch ()
  "A theme that changes the divider colour changes what the corners show, promoted or not, and leaves nothing after."
  (skip-unless (glitnir-fx--live-p))
  (custom-declare-theme 'glitnir-fx-card nil)
  (custom-theme-set-faces 'glitnir-fx-card '(window-divider ((t :foreground "#a02060"))))
  (glitnir-fx--with-cards
   glitnir-fx--card
   (lambda (left _right)
     (let* ((box (glitnir-fx--card-box left))
            (corner (lambda () (glitnir-fx--pixel (gui-runner-screen-grab) (nth 0 box) (nth 1 box))))
            (base (gui-runner-screen-grab)))
       (unwind-protect
           (progn
             ;; the theme's colour wins over none set for the frame
             (set-face-attribute 'window-divider (selected-frame) :foreground 'unspecified)
             (enable-theme 'glitnir-fx-card)
             (glitnir-fx--redraw left)
             (should (glitnir-fx--near (funcall corner) '(96 32 160)))
             (when (boundp 'nhwl-scroll-layers)
               (glitnir-fx--fill left)
               (glitnir-fx--with-scroll-layers t
                 (should (= (car (glitnir-fx--scroll-corners left box 3)) 1))
                 (disable-theme 'glitnir-fx-card)
                 (set-face-foreground 'window-divider glitnir-fx--gap (selected-frame))
                 (glitnir-fx--redraw left)
                 (should (glitnir-fx--near (funcall corner) glitnir-fx--gap-bgr))
                 (enable-theme 'glitnir-fx-card)
                 (set-face-attribute 'window-divider (selected-frame) :foreground 'unspecified)
                 (glitnir-fx--redraw left)
                 (should (glitnir-fx--near (funcall corner) '(96 32 160))))))
         (disable-theme 'glitnir-fx-card)
         (set-face-foreground 'window-divider glitnir-fx--gap (selected-frame))
         (with-current-buffer (window-buffer left) (erase-buffer))
         (glitnir-fx--redraw left)
         (should (eql 0 (cdr (gui-runner-pixel-diff base (gui-runner-screen-grab) box)))))))))

(ert-deftest glitnir-cards-off-parity ()
  "Card parameters that ask for no card (radius 0) change no pixel and give the frame no cards."
  (skip-unless (glitnir-fx--live-p))
  (glitnir-fx--with-cards
   nil
   (lambda (left _right)
     (let ((base (gui-runner-screen-grab)))
       (set-window-parameter left 'corner-radius 0)
       (glitnir-fx--redraw left)
       (should-not (window-parameter left 'glitnir--card))
       (should-not (frame-parameter nil 'glitnir--cards))
       (should (eql 0 (cdr (gui-runner-pixel-diff base (gui-runner-screen-grab)))))))))

;;; glitnir-effects-gui-tests.el ends here
