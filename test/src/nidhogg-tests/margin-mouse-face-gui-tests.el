;;; margin-mouse-face-gui-tests.el --- GUI tests: `mouse-face' on a margin string  -*- lexical-binding: t -*-

;; test-class: cond
;; test-pattern: handshake

;; Run by nidhogg/test/gui.sh (gui-runner.el loads every *-gui-tests.el).  Stock Emacs lights a string's `mouse-face'
;; in the mode, header and tab lines only; Nidhogg lights it in the window margins too (a debugger's breakpoint
;; column shows a faint dot under the pointer).  `nidhogg--note-mouse' notes the pointer as a shell does and answers
;; the lit run, (BEG-ROW BEG-COL END-ROW END-COL) in glyphs of the area, or nil.

(require 'ert)
(declare-function glitnir-gui--say "gui-runner" (fmt &rest args)) ; the lane's log, not *Messages*

(defun margin-mouse-face-gui--where (row)
  "Log what places the selected window's rows for ROW: its header and tab lines, start, edges and scale (round 6:
a run one row low at scale 1 on macOS)."
  (glitnir-gui--say "%s: header %S tab %S start %S edges %S inside %S scale %S line 2 posn %S" row
                    (window-header-line-height) (window-tab-line-height) (window-start) (window-pixel-edges)
                    (window-inside-pixel-edges) (frame-scale-factor)
                    (save-excursion (goto-char (point-min)) (forward-line 1) (posn-at-point (point)))))

(defun margin-mouse-face-gui--xy (line col)
  "Frame pixel coordinates inside column COL of the left margin on LINE (from 1) of the selected window."
  (save-excursion
    (goto-char (point-min)) (forward-line (1- line))
    (pcase-let ((`(,_ . ,y) (posn-x-y (posn-at-point (point))))
                (`(,left ,top . ,_) (window-inside-pixel-edges)))
      ;; the margin sits outside the fringe unless `fringes-outside-margins'
      (cons (+ (- left (* (car (window-margins)) (frame-char-width))
                  (if (nth 2 (window-fringes)) 0 (car (window-fringes))))
               (* col (frame-char-width)) 1)
            (+ top y 1)))))

(defun margin-mouse-face-gui--at (line col)
  "Note the pointer in the left margin's column COL on LINE; the lit run."
  (pcase-let ((`(,x . ,y) (margin-mouse-face-gui--xy line col)))
    ;; the point is in the margin (frame coordinates, as the note takes them)
    (let ((p (posn-at-x-y x y (selected-frame))))
      (should (equal (list (posn-area p) (posn-window p)) (list 'left-margin (selected-window)))))
    (nidhogg--note-mouse nil x y)))

(defmacro margin-mouse-face-gui--with (&rest body)
  "BODY in a buffer of 5 lines with a 2-column left margin: lines 2 and 4 show \"●\" in its second column with a
`mouse-face' and a `help-echo' (line 4's mouse face is another), line 3 a \"●\" without one."
  (declare (indent 0))
  `(let ((buf (generate-new-buffer "*margin-mouse-face*")))
     (unwind-protect
         (progn
           (switch-to-buffer buf)
           (setq left-margin-width 2)
           (set-window-buffer (selected-window) buf)
           (dotimes (i 5) (insert (format "line %d\n" (1+ i))))
           (dolist (spec '((2 highlight) (3 nil) (4 bold)))
             (goto-char (point-min)) (forward-line (1- (car spec)))
             (overlay-put (make-overlay (point) (point)) 'before-string
                          (propertize " " 'display
                                      (list '(margin left-margin)
                                            (concat " " (if (cadr spec)
                                                            (propertize "●" 'mouse-face (cadr spec)
                                                                        'help-echo "Click to add a breakpoint")
                                                          "●"))))))
           (redisplay t)
           (nidhogg--note-mouse nil 'leave nil)
           ,@body)
       (nidhogg--note-mouse nil 'leave nil)
       (kill-buffer buf))))

(ert-deftest margin-mouse-face-gui/the-build-says-it-lights-margins ()
  (should (featurep 'glitnir-margin-mouse-face)))

(ert-deftest margin-mouse-face-gui/the-pointer-lights-a-margin-string ()
  (margin-mouse-face-gui--with
    (margin-mouse-face-gui--where 'margin-mouse-face-gui/the-pointer-lights-a-margin-string)
    ;; line 2's dot, glyph 1 of the margin's row: lit
    (should (equal (margin-mouse-face-gui--at 2 1) '(1 1 1 2)))
    ;; the same run again: still lit, nothing redrawn
    (should (equal (margin-mouse-face-gui--at 2 1) '(1 1 1 2)))
    ;; the blank before it, and line 3's dot without a mouse face: nothing lit
    (should-not (margin-mouse-face-gui--at 2 0))
    (should-not (margin-mouse-face-gui--at 3 1))
    ;; from one lit dot to another
    (should (equal (margin-mouse-face-gui--at 4 1) '(3 1 3 2)))
    ;; into the text: the margin's run goes out
    (pcase-let ((`(,x . ,y) (margin-mouse-face-gui--xy 2 1)))
      (nidhogg--note-mouse nil (+ x (* 4 (frame-char-width))) y))
    (should-not (margin-mouse-face-gui--at 3 1))))

;;; margin-mouse-face-gui-tests.el ends here
