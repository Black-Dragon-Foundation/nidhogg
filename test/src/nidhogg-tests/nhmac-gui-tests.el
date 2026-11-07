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

;;; nhmac-gui-tests.el ends here
