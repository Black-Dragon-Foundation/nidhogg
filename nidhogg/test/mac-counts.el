;;; mac-counts.el --- Surface-lock and run-loop counters for the macOS GUI lane  -*- lexical-binding: t -*-

;; Test-only.  Reads the counters of nidhogg/test/mac-interpose (a DYLD_INSERT_LIBRARIES dylib that also is an Emacs
;; module): IOSurfaceLock calls, the bytes locked for writing, and run-loop iterations, split into those inside and
;; outside a redisplay.  nhmac-gui.sh --interpose loads the dylib and sets NHMAC_INTERPOSE_LIB.  Without the dylib
;; every counter reads 0 and no dump is written, so a row that needs the interposer calls `mac-counts-require' first
;; and fails saying so, not on a count of 0 or a missing file.
;;
;; The inside/outside split: NS emits no signposts, so `pre-redisplay-functions' marks the start of a redisplay and
;; `post-command-hook' its end; `mac-counts-redisplay' marks both around a (redisplay) the row calls itself.

(declare-function mac-counts--read-raw "mac-interpose")
(declare-function mac-counts--mark "mac-interpose")

(defvar mac-counts--loaded nil "Non-nil once the interposer's module is loaded.")

(defun mac-counts--load ()
  "Load the interposer's module (NHMAC_INTERPOSE_LIB) once; return non-nil when its functions are there."
  (unless mac-counts--loaded
    (let ((lib (getenv "NHMAC_INTERPOSE_LIB")))
      (when (and lib (not (string-empty-p lib)) (fboundp 'module-load))
        (module-load lib)
        (setq mac-counts--loaded t))))
  mac-counts--loaded)

(defun mac-counts-require ()
  "Signal an error unless the counting interposer is loaded: the row cannot measure without it."
  (unless (mac-counts--load)
    (error "No interposer inserted (run with --interpose)")))

(defun mac-counts-read ()
  "The counters as a plist (:surface-locks :surface-bytes :spins :spins-in-redisplay)."
  (let ((v (if (mac-counts--load) (mac-counts--read-raw) '(0 0 0 0))))
    (list :surface-locks (nth 0 v) :surface-bytes (nth 1 v) :spins (nth 2 v) :spins-in-redisplay (nth 3 v))))

(defun mac-counts--in (&rest _) (when (mac-counts--load) (mac-counts--mark 1)))
(defun mac-counts--out (&rest _) (when (mac-counts--load) (mac-counts--mark 0)))

(defun mac-counts-redisplay (&optional force)
  "Run (redisplay FORCE) with the run-loop spins it causes counted as inside a redisplay."
  (mac-counts--in)
  (unwind-protect (redisplay force) (mac-counts--out)))

(defun mac-counts-install ()
  "Mark the command loop's redisplays too: in at `pre-redisplay-functions', out at `post-command-hook'."
  (add-hook 'pre-redisplay-functions #'mac-counts--in)
  (add-hook 'post-command-hook #'mac-counts--out))

(provide 'mac-counts)
;;; mac-counts.el ends here
