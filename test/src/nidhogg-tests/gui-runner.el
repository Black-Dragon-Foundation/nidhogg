;;; gui-runner.el --- run the *-gui-tests.el files on a Nidhogg GUI frame  -*- lexical-binding: t -*-

;; Loaded by nidhogg/test/gui.sh (`nidhogg -Q --fullscreen -l gui-runner.el', or, with --daemon, by the
;; daemon and started from `server-after-make-frame-hook').  Environment:
;;   GLITNIR_GUI_SCALE   the monitor scale the run asked for (default 1)
;;   GLITNIR_GUI_SIZE    WxH of the virtual monitor in device pixels (for the fractional-scale check)
;;   GLITNIR_GUI_TESTS   directory holding the *-gui-tests.el files (default: this file's directory)
;;   GLITNIR_GUI_SELECT  ERT selector, an Emacs regexp (default: every test)
;; Prints every result to `external-debugging-output', then `kill-emacs' 0 (all as expected) or 1.
;; The tests start only once the first frame is visible and has been redisplayed, never inside `command-line'.

(require 'ert)

(defconst glitnir-gui--dir (file-name-directory (or load-file-name buffer-file-name)))

(defun glitnir-gui--say (fmt &rest args)
  (let ((s (apply #'format fmt args)))
    (princ (concat s "\n") #'external-debugging-output)))

(defun glitnir-gui--scale ()
  (string-to-number (or (getenv "GLITNIR_GUI_SCALE") "1")))

(defun glitnir-gui--scale-ready-p ()
  "Non-nil once the selected frame sits at the asked scale (the monitor's scale applied)."
  (let ((s (glitnir-gui--scale)))
    ;; GTK 3 knows only integer scales: it reports ceil(S) and a monitor geometry of the logical size (WxH / S)
    ;; times ceil(S) (S = 1.25 on 2880x1800: 4608x2880), which tells S = 1.25 from S = 2.
    (and (= (frame-scale-factor) (ceiling s))
         (let* ((size (getenv "GLITNIR_GUI_SIZE"))
                (geo (alist-get 'geometry (car (display-monitor-attributes-list))))
                (m (and size (string-match "\\`\\([0-9]+\\)x\\([0-9]+\\)\\'" size))))
           (or (not m)
               (and (= (nth 2 geo) (* (ceiling s) (round (/ (string-to-number (match-string 1 size)) s))))
                    (= (nth 3 geo) (* (ceiling s) (round (/ (string-to-number (match-string 2 size)) s))))))))))

(defun glitnir-gui--wait-for-scale ()
  (let ((deadline (+ (float-time) 10)))
    (while (and (not (glitnir-gui--scale-ready-p)) (< (float-time) deadline))
      (sit-for 0.1))
    (unless (glitnir-gui--scale-ready-p)
      (glitnir-gui--say "gui-runner: scale %s not reached in 10 s (frame-scale-factor %s, monitors %S)"
                        (glitnir-gui--scale) (frame-scale-factor) (display-monitor-attributes-list))
      (kill-emacs 1))))

(defun glitnir-gui--load-tests ()
  (let ((dir (or (getenv "GLITNIR_GUI_TESTS") glitnir-gui--dir)))
    (dolist (f (directory-files dir t "-gui-tests\\.el\\'"))
      (load f nil t))))

(defun glitnir-gui--report (event-type &rest args)
  (pcase event-type
    ('test-ended
     (let* ((test (nth 1 args)) (result (nth 2 args))
            (expected (ert-test-result-expected-p test result)))
       (glitnir-gui--say "%s %s" (if expected "passed" "FAILED") (ert-test-name test))
       (unless expected
         (when (ert-test-result-with-condition-p result)
           (glitnir-gui--say "  %S" (ert-test-result-with-condition-condition result))))))))

(defun glitnir-gui--run ()
  (glitnir-gui--wait-for-scale)
  (glitnir-gui--load-tests)
  (let* ((stats (ert-run-tests (or (getenv "GLITNIR_GUI_SELECT") t) #'glitnir-gui--report))
         (n (ert-stats-total stats)) (bad (ert-stats-completed-unexpected stats)))
    (glitnir-gui--say "Ran %d tests, %d results as expected, %d unexpected" n (- n bad) bad)
    (kill-emacs (if (and (> n 0) (= bad 0)) 0 1))))

(defun glitnir-gui--start (&rest _)
  "Run the tests once, after the first frame is visible and redisplayed."
  (remove-hook 'server-after-make-frame-hook #'glitnir-gui--start)
  (remove-hook 'window-setup-hook #'glitnir-gui--start)
  (run-at-time 0 nil
               (lambda ()
                 (condition-case err
                     (progn (redisplay t) (sit-for 0.2) (glitnir-gui--run))
                   (error (glitnir-gui--say "gui-runner: %S" err) (kill-emacs 1))))))

;; `window-setup-hook' runs at the end of `command-line'; the timer fires in the command loop after it.
(if (daemonp)
    (add-hook 'server-after-make-frame-hook #'glitnir-gui--start)
  (add-hook 'window-setup-hook #'glitnir-gui--start))

;;; gui-runner.el ends here
