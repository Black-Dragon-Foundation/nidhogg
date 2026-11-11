;;; nhmac-dbgspike-gui-tests.el --- a throwaway spike row, never merged  -*- lexical-binding: t -*-

;; drgnx's graphical debugging plan, T13: debug adapters under Gleipnir's Seatbelt profile on a macOS runner.  The
;; work is dbgspike/run-mac.sh; this row only runs it and prints its lines.

(require 'ert)

(defconst nhmac-dbgspike--dir (file-name-directory (or load-file-name buffer-file-name)))

(ert-deftest nhmac-dbgspike/adapters-under-seatbelt ()
  (let ((out (with-temp-buffer
               (call-process "/bin/bash" nil t nil (expand-file-name "dbgspike/run-mac.sh" nhmac-dbgspike--dir))
               (buffer-string))))
    (dolist (line (split-string out "\n" t))
      (when (string-prefix-p "dbgspike" line)
        (let ((coding-system-for-write 'utf-8-unix))
          (write-region (concat line "\n") nil "/dev/stderr" t 'silent))))
    (should (string-match-p "dbgspike: done" out))))

;;; nhmac-dbgspike-gui-tests.el ends here
