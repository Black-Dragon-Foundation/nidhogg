;;; mouse-sensor-gui-tests.el --- GUI tests for mouse sensors  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/gui.sh (gui-runner.el loads every *-gui-tests.el).  `nidhogg--note-mouse' notes the pointer as a
;; shell does (note_mouse_highlight, or the leave handler); the sensor's changes are events, run by read_char.

(require 'ert)
(require 'cl-lib)

(defvar mouse-sensor-gui--log nil "What the sensors were called with: (NAME DIR POS), newest first.")

(defun mouse-sensor-gui--sensor (name)
  "A sensor function logging NAME."
  (lambda (_window pos dir) (push (list name dir pos) mouse-sensor-gui--log)))

(defun mouse-sensor-gui--run ()
  "Run the queued sensor events: read_char runs each as it fetches them, so one read with a short timeout runs all that
are pending (a read with no timeout left returns before it looks).  At most 20 reads: other input pending ends it too."
  (let ((input-pending-p-filter-events nil) (n 0))
    (while (and (< n 20) (input-pending-p))
      (read-event nil nil 0.01)
      (setq n (1+ n)))))

(defun mouse-sensor-gui--xy (pos)
  "Frame pixel coordinates of buffer position POS in the selected window."
  (pcase-let ((`(,x . ,y) (posn-x-y (posn-at-point pos)))
              (`(,left ,top . ,_) (window-inside-pixel-edges)))
    (cons (+ left x 1) (+ top y 1))))

(defun mouse-sensor-gui--at (pos)
  "Note the pointer on POS, then run what that queued; the log since, oldest first."
  (setq mouse-sensor-gui--log nil)
  (pcase-let ((`(,x . ,y) (mouse-sensor-gui--xy pos)))
    (nidhogg--note-mouse nil x y))
  (mouse-sensor-gui--run)
  (mapcar (lambda (e) (list (car e) (cadr e))) (reverse mouse-sensor-gui--log)))

(defmacro mouse-sensor-gui--with (&rest body)
  "BODY in a buffer \"aaaa bbbb cccc\" shown in the selected window: aaaa senses A, bbbb B (an overlay), cccc nothing."
  (declare (indent 0))
  `(let ((buf (generate-new-buffer "*mouse-sensor*"))
         (mouse-sensor-enabled t))
     (unwind-protect
         (progn
           (switch-to-buffer buf)
           (insert (propertize "aaaa" 'mouse-sensor-functions (list (mouse-sensor-gui--sensor 'A))) " bbbb cccc")
           (overlay-put (make-overlay 6 10) 'mouse-sensor-functions (list (mouse-sensor-gui--sensor 'B)))
           (redisplay t)
           ;; start off every sensor, with nothing queued: what an earlier test or the setup's redisplay noted is gone
           (nidhogg--note-mouse nil 'leave nil)
           (mouse-sensor-gui--run)
           (setq mouse-sensor-gui--log nil)
           ,@body)
       ;; off every sensor before the buffer goes
       (nidhogg--note-mouse nil 'leave nil)
       (mouse-sensor-gui--run)
       (kill-buffer buf))))

(ert-deftest mouse-sensor-gui/entered-and-left-in-order ()
  (mouse-sensor-gui--with
    (should (equal (mouse-sensor-gui--at 2) '((A entered))))
    ;; moving inside one run, or a redisplay under a still pointer, is no change
    (should (equal (mouse-sensor-gui--at 3) nil))
    (redisplay t)
    (should (equal (mouse-sensor-gui--at 3) nil))
    (should (equal (mouse-sensor-gui--at 7) '((A left) (B entered))))
    (should (equal (mouse-sensor-gui--at 12) '((B left))))
    (should (equal (mouse-sensor-gui--at 2) '((A entered))))))

(ert-deftest mouse-sensor-gui/leaving-the-frame-or-the-text-leaves ()
  (mouse-sensor-gui--with
    (should (equal (mouse-sensor-gui--at 2) '((A entered))))
    (setq mouse-sensor-gui--log nil)
    (nidhogg--note-mouse nil 'leave nil)
    (mouse-sensor-gui--run)
    (should (equal (mapcar #'cadr mouse-sensor-gui--log) '(left)))
    (should (equal (mouse-sensor-gui--at 2) '((A entered))))
    ;; onto the mode line: off the text
    (setq mouse-sensor-gui--log nil)
    (pcase-let ((`(,left ,_ ,_ ,bottom) (window-pixel-edges)))
      (nidhogg--note-mouse nil (+ left 4) (- bottom 2)))
    (mouse-sensor-gui--run)
    (should (equal (mapcar #'cadr mouse-sensor-gui--log) '(left)))))

(ert-deftest mouse-sensor-gui/off-means-no-event ()
  (mouse-sensor-gui--with
    (let ((mouse-sensor-enabled nil))
      (should (equal (mouse-sensor-gui--at 2) nil))
      (should (equal (mouse-sensor-gui--at 7) nil)))))

(ert-deftest mouse-sensor-gui/a-key-sequence-goes-on-across-a-sensor ()
  ;; C-x is read, then the sensor's event, then o: the sequence is C-x o, and the sensor ran between
  (mouse-sensor-gui--with
    (let* ((ran nil)
           (sensor (lambda (_w _p dir) (when (eq dir 'entered) (setq ran t) (push ?o unread-command-events)))))
      (put-text-property 11 15 'mouse-sensor-functions (list sensor))
      (redisplay t)
      (pcase-let ((`(,x . ,y) (mouse-sensor-gui--xy 12)))
        (nidhogg--note-mouse nil x y))
      (let ((unread-command-events (list ?\C-x))
            (keys nil))
        ;; the deadline only bounds a failure (no sensor event: nothing would give the o)
        (setq keys (with-timeout (10 (ert-fail (list "no sensor event" :log mouse-sensor-gui--log)))
                     (read-key-sequence nil)))
        (should ran)
        (should (equal keys (kbd "C-x o")))
        ;; nothing of it is input: not in the recent keys
        (should-not (seq-find (lambda (e) (eq (car-safe e) 'mouse-sensor)) (recent-keys)))))))

(ert-deftest mouse-sensor-gui/an-error-is-reported-not-raised ()
  (mouse-sensor-gui--with
    (put-text-property 11 15 'mouse-sensor-functions (list (lambda (&rest _) (error "Sensor broke"))))
    (redisplay t)
    (should (equal (mouse-sensor-gui--at 12) nil))
    (should (string-match-p "Error in mouse-sensor-functions: (error \"Sensor broke\")"
                            (with-current-buffer "*Messages*" (buffer-string))))))

(ert-deftest mouse-sensor-gui/motion-cost ()
  "The pointer's cost in note_mouse_highlight: sensors off (no lookup), on over text with none, on across two runs."
  :tags '(:timing)
  (mouse-sensor-gui--with
    (let* ((xy (list (mouse-sensor-gui--xy 2) (mouse-sensor-gui--xy 12) (mouse-sensor-gui--xy 7)))
           (n 20000)
           (time (lambda (enabled a b)
                   (let ((mouse-sensor-enabled enabled))
                     (garbage-collect)
                     (let ((t0 (float-time)))
                       (dotimes (i n)
                         (let ((p (if (cl-evenp i) a b))) (nidhogg--note-mouse nil (car p) (cdr p))))
                       (mouse-sensor-gui--run)
                       (/ (* 1e6 (- (float-time) t0)) n))))))
      (let ((off (funcall time nil (nth 0 xy) (nth 2 xy)))
            (none (funcall time t (nth 1 xy) (nth 1 xy)))
            (cross (funcall time t (nth 0 xy) (nth 2 xy))))
        (message "MOUSE-SENSOR-BENCH us/motion: off %.2f; on, no sensor %.2f; on, crossing runs %.2f" off none cross)
        (should (> off 0))))))

;;; mouse-sensor-gui-tests.el ends here
