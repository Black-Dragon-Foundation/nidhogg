;;; nhmac-ax-gui-tests.el --- Naglfar's accessibility and Zoom rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).  `test-ax' calls the root
;; view's accessibility methods in-process, as an assistive application would through the accessibility server.

(require 'ert)
(require 'cl-lib)

(defun nhmac-ax--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--query)))

(defun nhmac-ax--state ()
  (nhmac--query 'test-ax (selected-frame)))

(defun nhmac-ax--focus ()
  "Give the selected frame the focus and wait (2 s at most) until Emacs has it: C pushes the cursor and the line
only for the focused frame."
  (select-frame-set-input-focus (selected-frame))
  (let ((deadline (+ (float-time) 2)))
    (while (and (not (eq (frame-focus-state) t)) (< (float-time) deadline))
      (sit-for 0.05)))
  (should (eq (frame-focus-state) t)))

(defmacro nhmac-ax--with-text (text &rest body)
  "Run BODY in a buffer showing TEXT in the selected window, the frame focused, after a redisplay."
  (declare (indent 1))
  `(let ((buf (generate-new-buffer " *nhmac-ax*")))
     (unwind-protect
         (save-window-excursion
           (switch-to-buffer buf)
           (insert ,text)
           (nhmac-ax--focus)
           (redisplay t)
           ,@body)
       (kill-buffer buf))))

(defun nhmac-ax--settle ()
  (redisplay t)
  (sit-for 0.05)
  (redisplay t))

(ert-deftest nhmac-ax-role ()
  "The root view is an accessibility element in the text area role."
  (skip-unless (nhmac-ax--naglfar-p))
  (let ((st (nhmac-ax--state)))
    (should (eq (plist-get st :element) t))
    (should (equal (plist-get st :role) "AXTextArea"))))

(ert-deftest nhmac-ax-value-and-selected-text ()
  "The value is the line around point (the pushed text, within its cap) and the selected text the region within it."
  (skip-unless (nhmac-ax--naglfar-p))
  (nhmac-ax--with-text "first line\nhello brave world\nlast"
    (goto-char (point-min))
    (forward-line 1)
    (forward-char 6)
    (push-mark (point) t t)
    (forward-char 5)
    (nhmac-ax--settle)
    (let ((st (nhmac-ax--state)))
      (should (equal (plist-get st :value) "hello brave world"))
      (should (equal (plist-get st :selected) "brave")))
    (deactivate-mark)
    (nhmac-ax--settle)
    (should (equal (plist-get (nhmac-ax--state) :selected) ""))))

(ert-deftest nhmac-ax-frame ()
  "`accessibilityFrame' is the cursor's rectangle on the screen: inside the window's frame, as tall as a line, and
`firstRectForCharacterRange:' of one character at point."
  (skip-unless (nhmac-ax--naglfar-p))
  (nhmac-ax--with-text "abc"
    (nhmac-ax--settle)
    (let* ((st (nhmac-ax--state)) (r (plist-get st :frame)) (win (plist-get st :window)))
      (should (equal r (plist-get (nhmac--query 'test-ime-state (selected-frame) 0 1) :rect)))
      (should (>= (nth 0 r) (nth 0 win)))
      (should (<= (+ (nth 0 r) (nth 2 r)) (+ (nth 0 win) (nth 2 win))))
      (should (> (nth 3 r) 0)))))

(ert-deftest nhmac-ax-zoom-follows-the-cursor ()
  "With Zoom on, a cursor move gives Zoom the cursor's rectangle once; a redisplay with the cursor where it was gives
none."
  (skip-unless (nhmac-ax--naglfar-p))
  (nhmac--call 'test-zoom-force 1)
  (unwind-protect
      (nhmac-ax--with-text "one two three"
        (goto-char (point-min))
        (nhmac-ax--settle)
        (let ((before (plist-get (nhmac-stats) :zoom-calls)))
          (forward-char 4)
          (nhmac-ax--settle)
          (should (= (- (plist-get (nhmac-stats) :zoom-calls) before) 1))
          (setq before (plist-get (nhmac-stats) :zoom-calls))
          (force-mode-line-update)
          (nhmac-ax--settle)
          (should (= (- (plist-get (nhmac-stats) :zoom-calls) before) 0))))
    (nhmac--call 'test-zoom-force 0)))

;;; nhmac-ax-gui-tests.el ends here
