;;; nhmac-gui-tests.el --- Naglfar's GUI rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh on the macOS runner.  Rows are appended under ";; Task N:" headings.

(require 'ert)
(load (expand-file-name "../../../nidhogg/test/mac-counts.el" (file-name-directory (or load-file-name buffer-file-name))) nil t)

(defun nhmac-gui--ns-build-p ()
  "The stock NS port (not Naglfar): the baselines are measured here."
  (and (eq window-system 'ns) (not (featurep 'nidhogg-macos))))

;; Task 2: the interposer and the counters harness
(ert-deftest nhmac-interposer-counts-ns ()
  "20 keys into a buffer lock a surface and spin the run loop inside a redisplay (NS baseline: the counts are logged).
NS coalesces: a run measured 16 locks per 20 keys at scale 1 and 10 at scale 2, so the bound is only that both happen."
  (skip-unless (nhmac-gui--ns-build-p))
  (switch-to-buffer (generate-new-buffer " *mac-counts*"))
  (mac-counts-install)
  (mac-counts-redisplay t)
  (let ((before (mac-counts-read)))
    (dotimes (_ 20)
      (execute-kbd-macro "x")
      (mac-counts-redisplay t))
    (let* ((after (mac-counts-read))
           (d (lambda (k) (- (plist-get after k) (plist-get before k)))))
      (message "nhmac-counts: 20 keys: locks %d bytes %d spins %d spins-in-redisplay %d"
               (funcall d :surface-locks) (funcall d :surface-bytes) (funcall d :spins) (funcall d :spins-in-redisplay))
      (should (>= (funcall d :surface-locks) 1))
      (should (>= (funcall d :spins-in-redisplay) 1)))))

(ert-deftest nhmac-ns-menu-dump ()
  "NS's real menu bar, built through menu.c, is dumped to a file: the fixture Naglfar's flattener is checked against."
  (skip-unless (nhmac-gui--ns-build-p))
  (let ((file (expand-file-name "nhmac-ns-menu.tsv" (or (getenv "GLITNIR_GUI_OUT") temporary-file-directory))))
    (when (file-exists-p file) (delete-file file))
    (mac-counts-menu-dump file)
    (should (file-exists-p file))
    (with-temp-buffer
      (insert-file-contents file)
      (should (> (count-lines (point-min) (point-max)) 20))
      (should (re-search-forward "^1\t[^\t]*\t" nil t)))))

;; The window system's names: Naglfar reports NS's, so ns-win.el and packages take their macOS branches
(ert-deftest nhmac-window-system-is-ns ()
  "`window-system', `initial-window-system' and the terminal's type read `ns', and the shell provides
`nidhogg-macos' and `ns'."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (should (featurep 'nidhogg-macos))
  (should (featurep 'ns))
  (should (eq window-system 'ns))
  (should (eq initial-window-system 'ns))
  (should (eq (terminal-live-p nil) 'ns)))

;; Frames: an NSWindow per frame, its root view hosting the shell's layer
(ert-deftest nhmac-frame-opens ()
  "`make-frame' gives a visible NSWindow whose root view hosts the shell's layer (no subviews), sized in device
pixels; the display's width is the main screen's."
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((f (make-frame '((width . 60) (height . 20)))))
    (unwind-protect
        (let ((info (progn (redisplay t) (sit-for 0.2) (nhmac--window-info f))))
          (should (frame-visible-p f))
          (should (eq (plist-get info :visible) t))
          (should (eq (plist-get info :layer) t))
          (should (eq (plist-get info :wants-layer) t))
          (should (= (plist-get info :subviews) 0))
          (should (= (frame-native-width f) (plist-get info :device-width)))
          (should (= (display-pixel-width) (plist-get info :screen-device-width))))
      (delete-frame f))))

;; The guard: an Objective-C exception in a pump poisons the display, as a panic does, and Emacs goes on
(ert-deftest nhmac-objc-exception-poisons ()
  "An NSException raised inside the AppKit pump is caught; the display is poisoned and Emacs does not abort."
  :tags '(runs-last)
  (skip-unless (not (nhmac-gui--ns-build-p)))
  (let ((before (length (nhwl-glue-panics))))
    (nhmac--test-raise-in-pump)
    (sit-for 0.5)
    (should (> (length (nhwl-glue-panics)) before))
    (should (string-match-p "Objective-C exception" (car (nhwl-glue-panics))))
    ;; Emacs is still here and still evaluates
    (should (equal (eval '(+ 1 1) t) 2))))

;;; nhmac-gui-tests.el ends here
