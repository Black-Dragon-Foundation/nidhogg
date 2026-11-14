;;; nhmac-pasteboard-gui-tests.el --- Naglfar's clipboard, dialog and Services rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond
;; test-pattern: handshake

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).

(require 'ert)
(declare-function glitnir-gui--say "gui-runner" (fmt &rest args)) ; the lane's log, not *Messages*
(require 'cl-lib)

(defun nhmac-pb--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--query)))

;; Task 28: clipboard, file dialogs, Services
(ert-deftest nhmac-clipboard-roundtrip ()
  "Text, UTF-8 text and a file's URL go through the general pasteboard."
  (skip-unless (nhmac-pb--naglfar-p))
  (dolist (text '("plain text" "héllo ☃ 日本語" "line one\nline \"two\" (f x) \\"))
    (gui-set-selection 'CLIPBOARD text)
    (should (ns-selection-owner-p 'CLIPBOARD))
    (should (ns-selection-exists-p 'CLIPBOARD))
    (should (equal (ns-get-selection 'CLIPBOARD 'STRING) text)))
  (should-not (ns-selection-owner-p 'PRIMARY))
  (let ((file (make-temp-file "nhmac-clip")))
    (nhmac--call 'test-pasteboard-file file)
    (should-not (ns-selection-owner-p 'CLIPBOARD))
    (let ((uri (ns-get-selection 'CLIPBOARD 'text/uri-list)))
      (should (string-prefix-p "file://" uri))
      (should (string-suffix-p (file-name-nondirectory file) uri)))
    (ns-disown-selection-internal 'CLIPBOARD)))

(ert-deftest nhmac-file-dialog ()
  "An open panel and a save panel run modally, answered from inside their loop, and neither overlaps a take."
  (skip-unless (nhmac-pb--naglfar-p))
  (let* ((dir (file-name-as-directory (make-temp-file "nhmac-dlg" t)))
         (before (nhmac-stats)))
    (nhmac--call 'test-dialog-ok 1 dir)
    (should (equal (file-truename (ns-read-file-name "Pick" temporary-file-directory t nil t))
                   (file-truename (directory-file-name dir))))
    (nhmac--call 'test-dialog-ok 1 (concat dir "saved.txt"))
    (should (equal (file-truename (ns-read-file-name "Save" dir nil "x.txt"))
                   (file-truename (concat dir "saved.txt"))))
    (should (= (- (plist-get (nhmac-stats) :overlapping-takes)
                  (plist-get before :overlapping-takes))
               0))))

(ert-deftest nhmac-services-request ()
  "A Services request returns text."
  (skip-unless (nhmac-pb--naglfar-p))
  (ert-skip "pending: no service is guaranteed on the runner; the requestor is `ns-perform-service'"))

(ert-deftest nhmac-services-provide ()
  "Emacs answers another application's Services request: New Buffer Containing Selection makes a buffer of the text."
  (skip-unless (nhmac-pb--naglfar-p))
  (let ((before (buffer-list)) (text "nhmac services text"))
    (nhmac--call 'test-service-request "open-selection" text)
    (let* ((keys (vector (read-event nil nil 2) (read-event nil nil 2)))
           (cmd (key-binding keys)))
      (glitnir-gui--say "nhmac-services-provide: read %S" keys)
      (should (eq cmd 'nhmac-service))
      (save-window-excursion (call-interactively cmd)))
    (let ((made (seq-filter (lambda (b) (not (memq b before))) (buffer-list))))
      (unwind-protect
          (should (seq-some (lambda (b) (with-current-buffer b (equal (buffer-string) text))) made))
        (mapc #'kill-buffer made)))))

;;; nhmac-pasteboard-gui-tests.el ends here
