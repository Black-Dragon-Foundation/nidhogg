;;; nhmac-app-gui-tests.el --- Naglfar's app bundle rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el) when --app names the
;; installed bundle (`make install' in a --naglfar build tree); without it the rows skip.  Rows are appended under
;; ";; Task N:" headings.

(require 'ert)
(require 'cl-lib)

(defun nhmac-app--say (fmt &rest args)
  "Print FMT with ARGS as a line of the lane's log (stderr), not to *Messages*."
  (princ (concat (apply #'format fmt args) "\n") #'external-debugging-output))

(defun nhmac-app--dir ()
  "The bundle the lane was given (NHMAC_APP), or nil."
  (let ((app (getenv "NHMAC_APP")))
    (and app (not (string-empty-p app)) (file-name-as-directory app))))

(defun nhmac-app--emacs ()
  (expand-file-name "Contents/MacOS/Emacs" (nhmac-app--dir)))

(defun nhmac-app--run (program args secs)
  "Run PROGRAM with ARGS for at most SECS seconds; (EXIT-STATUS . OUTPUT), EXIT-STATUS `timeout' when it ran over."
  (let* ((buf (generate-new-buffer " *nhmac-app*"))
         (done nil)
         ;; the sentinel runs after the last output is read; `process-live-p' turns nil at the exit, before that
         (proc (make-process :name "nhmac-app" :buffer buf :command (cons program args)
                             :connection-type 'pipe :noquery t :sentinel (lambda (_p _e) (setq done t))))
         (deadline (+ (float-time) secs)))
    (unwind-protect
        (progn
          (while (and (not done) (< (float-time) deadline))
            (accept-process-output proc 0.2))
          (let ((live (not done)))
            (when live (delete-process proc))
            (cons (if live 'timeout (process-exit-status proc))
                  (with-current-buffer buf (buffer-string)))))
      (kill-buffer buf))))

(defun nhmac-app--without-locale ()
  "`process-environment' with no locale variable: a launch from Finder has none."
  (cl-remove-if (lambda (e) (string-match-p "\\`\\(LANG\\|LC_[A-Z]+\\)=" e)) process-environment))

;; Task 21: the bundle, startup and the daemon
(ert-deftest nhmac-bundle-open ()
  "`open -a' on the bundle starts Emacs, and `invocation-directory' and `data-directory' point into the bundle."
  (skip-unless (nhmac-app--dir))
  (let* ((app (file-truename (nhmac-app--dir)))
         (out (make-temp-file "nhmac-open"))
         (form (format "(progn (write-region (prin1-to-string (list invocation-directory data-directory window-system)) nil %S) (kill-emacs))" out))
         (r (nhmac-app--run "open" (list "-n" "-W" "-a" app "--args" "-Q" "--eval" form) 90)))
    (nhmac-app--say "nhmac-bundle-open: %S" r)
    (should (eq (car r) 0))
    (let ((got (read (with-temp-buffer (insert-file-contents out) (buffer-string)))))
      (nhmac-app--say "nhmac-bundle-open: %S" got)
      (should (string-prefix-p app (file-truename (nth 0 got))))
      (should (string-prefix-p app (file-truename (nth 1 got))))
      (should (eq (nth 2 got) 'ns)))))

(ert-deftest nhmac-bundle-open-file ()
  "A file opened through an Apple event (`open -a APP FILE') arrives as a visited buffer."
  (skip-unless (nhmac-app--dir))
  (let* ((app (file-truename (nhmac-app--dir)))
         (file (make-temp-file "nhmac-open-file" nil ".txt" "hello\n"))
         (out (make-temp-file "nhmac-open-file-out"))
         (form (format "(run-at-time 3 nil (lambda () (write-region (prin1-to-string (mapcar #'buffer-file-name (buffer-list))) nil %S) (kill-emacs)))" out))
         (r (nhmac-app--run "open" (list "-n" "-W" "-a" app file "--args" "-Q" "--eval" form) 90)))
    (should (eq (car r) 0))
    (should (member (file-truename file)
                    (mapcar (lambda (f) (and f (file-truename f)))
                            (read (with-temp-buffer (insert-file-contents out) (buffer-string))))))))

(ert-deftest nhmac-daemon-client ()
  "`emacs --daemon' starts (it execs itself, as CoreFoundation fails in a forked child), then `emacsclient -c' opens a frame."
  (skip-unless (nhmac-app--dir))
  (let* ((sock (make-temp-name (expand-file-name "nhd" temporary-file-directory)))
         (client (expand-file-name "Contents/MacOS/bin/emacsclient" (nhmac-app--dir)))
         (started (nhmac-app--run (nhmac-app--emacs) (list (concat "--daemon=" sock)) 90)))
    ;; on stderr (the lane's log), not *Messages*: batch 2's client answered "" with exit 0
    (nhmac-app--say "nhmac-daemon-client: daemon %S" started)
    (unwind-protect
        (progn
          (should (eq (car started) 0))
          (let ((r (nhmac-app--run client (list "-s" sock "-c" "-n" "-e" "(list window-system (length (frame-list)))") 60)))
            (nhmac-app--say "nhmac-daemon-client: client %S" r)
            (unless (string-match-p "\\`(ns [2-9]" (string-trim (cdr r)))
              ;; what the daemon says it has: its frames and the window system it would make one on
              (nhmac-app--say "nhmac-daemon-client: daemon's view %S"
                                (nhmac-app--run client (list "-s" sock "-e" "(list (frame-list) (mapcar (lambda (f) (framep f)) (frame-list)) initial-window-system (featurep 'ns) (featurep 'nidhogg-macos) (daemonp))") 30)))
            (should (eq (car r) 0))
            (should (string-match-p "\\`(ns [2-9]" (string-trim (cdr r))))))
      (nhmac-app--run client (list "-s" sock "-e" "(kill-emacs)") 30))))

(ert-deftest nhmac-locale-from-system ()
  "Started with no locale in the environment, Emacs runs with LANG taken from the system's locale."
  (skip-unless (nhmac-app--dir))
  (let* ((process-environment (nhmac-app--without-locale))
         (r (nhmac-app--run (nhmac-app--emacs) (list "--batch" "--eval" "(princ (getenv \"LANG\"))") 60)))
    (nhmac-app--say "nhmac-locale-from-system: %S" r)
    (should (eq (car r) 0))
    (should (string-match-p "[a-z]+\\(_[A-Za-z]+\\)?\\.UTF-8\\'" (string-trim (cdr r))))))

;;; nhmac-app-gui-tests.el ends here
