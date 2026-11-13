;;; nhmac-ime-gui-tests.el --- Naglfar's input method rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).  An input method talks to the
;; frame's view through `NSTextInputClient'; the test hooks call those methods as one does (`test-ime') and read what
;; the view answers (`test-ime-state'), which comes from the state C pushed at the last redisplay.

(require 'ert)
(declare-function glitnir-gui--say "gui-runner" (fmt &rest args)) ; the lane's log, not *Messages*
(require 'cl-lib)

(defun nhmac-ime--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--query)))

(defun nhmac-ime--state (&optional loc len)
  "What the selected frame's view answers an input method, after a redisplay; the substring asked is LOC, LEN."
  (redisplay t)
  (nhmac--query 'test-ime-state (selected-frame) (or loc 0) (or len 0)))

(defun nhmac-ime--events (secs)
  "The events that come within SECS seconds, oldest first (special events run as they come)."
  (let ((deadline (+ (float-time) secs)) evs ev)
    (while (and (< (float-time) deadline) (setq ev (read-event nil nil 0.3)))
      (push ev evs))
    (nreverse evs)))

(defvar ns-working-overlay)

(defun nhmac-ime--working ()
  "The marked text shown as NS shows it (`ns-insert-working-text'): the after-string of `ns-working-overlay' in the
current buffer, or nil.  It is no buffer text."
  (and (bound-and-true-p ns-working-overlay) (eq (overlay-buffer ns-working-overlay) (current-buffer))
       (substring-no-properties (or (overlay-get ns-working-overlay 'after-string) ""))))

(defmacro nhmac-ime--in-buffer (text &rest body)
  "Run BODY in a fresh buffer holding TEXT, shown in the selected window."
  (declare (indent 1))
  `(let ((old (window-buffer)) (b (generate-new-buffer " *nhmac-ime*")))
     (unwind-protect
         (progn
           (set-window-buffer nil b)
           (with-current-buffer b
             (insert ,text)
             (goto-char (point-min))
             ,@body))
       (set-window-buffer nil old)
       (kill-buffer b))))

;; Task 26: the input method
(ert-deftest nhmac-ime-marked-and-commit ()
  "Marked text shows as NS's working text, and a commit replaces it with the text inserted."
  (skip-unless (nhmac-ime--naglfar-p))
  (nhmac-ime--in-buffer ""
    ;; C pushes what the view answers only for the focused frame
    (glitnir-gui--say "nhmac-ime-marked-and-commit: focus %S" (frame-focus-state))
    (nhmac--call 'test-ime (selected-frame) 0 "か")
    (nhmac-ime--events 1)
    ;; the working text is an overlay's after-string (NS's since Bug#23412), the buffer still empty
    (should (equal (list (buffer-string) (nhmac-ime--working)) '("" "か")))
    (should (eql (cadr (plist-get (nhmac-ime--state) :marked)) 1))
    (nhmac--call 'test-ime (selected-frame) 1 "家")
    (dolist (ev (nhmac-ime--events 1))
      (when (characterp ev) (insert ev)))
    (should (equal (buffer-string) "家"))
    (should (null (nhmac-ime--working)))
    (should (null (plist-get (nhmac-ime--state) :marked)))))

(ert-deftest nhmac-ime-selected-range ()
  "`selectedRange' is point from the accessible start, or the region when the mark is active, in characters."
  (skip-unless (nhmac-ime--naglfar-p))
  (nhmac-ime--in-buffer "héllo wörld"
    (goto-char 4)
    (deactivate-mark)
    (should (equal (plist-get (nhmac-ime--state) :selected) '(3 0)))
    (set-mark 2)
    (goto-char 6)
    (activate-mark)
    (should (equal (plist-get (nhmac-ime--state) :selected) '(1 4)))
    (deactivate-mark)))

(ert-deftest nhmac-ime-substring ()
  "`attributedSubstringForProposedRange:' returns the text of the line around point, with its actual range."
  (skip-unless (nhmac-ime--naglfar-p))
  (nhmac-ime--in-buffer "first\nhéllo wörld\nlast"
    (goto-char 10)
    (let ((s (nhmac-ime--state 6 100)))
      (should (equal (plist-get s :substring) "héllo wörld"))
      (should (equal (plist-get s :actual) '(6 11))))))

(ert-deftest nhmac-ime-first-rect ()
  "`firstRectForCharacterRange:' is below the cursor, on the screen inside the window, and moves with point."
  (skip-unless (nhmac-ime--naglfar-p))
  (nhmac-ime--in-buffer "abcdefghij"
    (goto-char 2)
    (let* ((s1 (nhmac-ime--state)) (r1 (plist-get s1 :rect)) (win (plist-get s1 :window)))
      (glitnir-gui--say "nhmac-ime-first-rect: %S" s1)
      (should (> (nth 3 r1) 0))
      (should (<= (nth 0 win) (nth 0 r1) (+ (nth 0 win) (nth 2 win))))
      (should (<= (nth 1 win) (nth 1 r1) (+ (nth 1 win) (nth 3 win))))
      (goto-char 6)
      (let ((r2 (plist-get (nhmac-ime--state) :rect)))
        (should (> (nth 0 r2) (nth 0 r1)))
        (should (= (nth 1 r2) (nth 1 r1)))))))

(ert-deftest nhmac-ime-backspace-in-composition ()
  "A backspace the input method passes on (`doCommandBySelector:' with `deleteBackward:') is a delete, as on NS."
  (skip-unless (nhmac-ime--naglfar-p))
  (nhmac-ime--in-buffer ""
    (nhmac--call 'test-ime (selected-frame) 3 "deleteBackward:")
    (should (cl-some (lambda (e) (memq e '(127 backspace))) (nhmac-ime--events 1)))))

(ert-deftest nhmac-ime-dead-keys ()
  "With Option not a modifier (`ns-alternate-modifier' none), Option-e e is é, through the input system.
The keys go through the window server, whose keyboard layout makes the dead key, as typing does: keys the app posts
itself (`test-post-key') never composed in any round."
  (skip-unless (nhmac-ime--naglfar-p))
  (let ((access (nhmac--query 'test-post-access)))
    (unless (eq access t)
      (ert-skip (format "the runner's grant does not allow posting events (CGPreflightPostEventAccess %S)" access))))
  (let ((was ns-alternate-modifier))
    (unwind-protect
        (progn
          (setq ns-alternate-modifier 'none)
          (nhmac-ime--events 0.3)
          ;; key code 14 is e on the ANSI layout; Option is kCGEventFlagMaskAlternate (1 << 19)
          (nhmac--call 'test-real-key 14 (ash 1 19))
          (nhmac--call 'test-real-key 14 0)
          (should (memq ?é (nhmac-ime--events 1.5))))
      (setq ns-alternate-modifier was))))

;; A real input source: Japanese (Romaji) from Text Input Sources, selected for the session, typed through by AppKit
;; key events; the input method itself makes the marked text and the commit.
(defconst nhmac-ime--kana "com.apple.inputmethod.Kotoeri.RomajiTyping.Japanese"
  "The Japanese Romaji input source, which macOS installs.")

(ert-deftest nhmac-ime-real-input-source ()
  "With the Japanese Romaji input source selected (`TISSelectInputSource'), k a typed as AppKit key events is marked
か (the input method's own preedit), and Return commits it into the buffer; the earlier input source comes back."
  (skip-unless (nhmac-ime--naglfar-p))
  (let* ((before (nhmac--query 'test-input-source ""))
         (was (plist-get before :current)))
    (unwind-protect
        (let ((sel (nhmac--query 'test-input-source nhmac-ime--kana)))
          (glitnir-gui--say "nhmac-ime-real-input-source: before %S, select %S" before sel)
          (should (plist-get sel :found))
          (should (eql (plist-get sel :select) 0))
          (should (equal (plist-get sel :current) nhmac-ime--kana))
          (nhmac-ime--in-buffer ""
            (nhmac-ime--events 0.3)
            ;; ANSI key codes: k 40, a 0, Return 36
            (nhmac--call 'test-post-key (selected-frame) ?k ?k 0 40)
            (nhmac--call 'test-post-key (selected-frame) ?a ?a 0 0)
            (nhmac-ime--events 1)
            (let ((marked (plist-get (nhmac-ime--state) :marked)))
              (glitnir-gui--say "nhmac-ime-real-input-source: after k a: %S, working %S, marked %S"
                                (buffer-string) (nhmac-ime--working) marked)
              (should (equal (list (buffer-string) (nhmac-ime--working)) '("" "か")))
              (should marked))
            (nhmac--call 'test-post-key (selected-frame) ?\r ?\r 0 36)
            (dolist (ev (nhmac-ime--events 1))
              (when (characterp ev) (insert ev)))
            (glitnir-gui--say "nhmac-ime-real-input-source: after Return: %S" (buffer-string))
            (should (equal (buffer-string) "か"))
            (should (null (plist-get (nhmac-ime--state) :marked)))))
      (when was (nhmac--query 'test-input-source was)))
    (should (equal (plist-get (nhmac--query 'test-input-source "") :current) was))))

;;; nhmac-ime-gui-tests.el ends here
