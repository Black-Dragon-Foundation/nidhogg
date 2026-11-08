;;; nhmac-menu-gui-tests.el --- Naglfar's menu rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).  The native menu bar is built
;; from trees Lisp sends at idle; these rows read what the shell holds (`nhmac--query' 'test-menu-dump) and drive the
;; popup's tracking loop with a timer.

(require 'ert)
(require 'cl-lib)
(require 'subr-x)

(defun nhmac-menu-gui--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--query)))

(defun nhmac-menu-gui--dump ()
  "The shell's main menu as a list of (DEPTH TITLE TAG ENABLED)."
  (mapcar (lambda (l) (let ((f (split-string l "\t")))
                        (list (string-to-number (nth 0 f)) (nth 1 f) (string-to-number (nth 2 f))
                              (string-to-number (nth 3 f)))))
          (split-string (nhmac--query 'test-menu-dump) "\n" t)))

(defun nhmac-menu-gui--settle ()
  "Let the pending idle build run and the shell take it."
  (sit-for 1.0))

;; Task 27: menus built at idle
(ert-deftest nhmac-menu-builds-per-key ()
  "At most one deep build runs per idle period while typing."
  (skip-unless (nhmac-menu-gui--naglfar-p))
  (nhmac-menu-gui--settle)
  (switch-to-buffer (generate-new-buffer " *nhmac-menu-keys*"))
  (let ((before nhmac-menu-builds))
    (dotimes (_ 20)
      (execute-kbd-macro "x"))
    (nhmac-menu-gui--settle)
    (message "nhmac-menu-builds-per-key: %d builds" (- nhmac-menu-builds before))
    (should (<= (- nhmac-menu-builds before) 1))))

(ert-deftest nhmac-menu-bar-holds-the-tree ()
  "After a build the shell's menu bar has the File menu with items that carry their places."
  (skip-unless (nhmac-menu-gui--naglfar-p))
  (nhmac-menu-invalidate)
  (nhmac-menu-gui--settle)
  (let ((d (nhmac-menu-gui--dump)))
    (should (cl-find-if (lambda (e) (and (= (car e) 1) (equal (cadr e) "File"))) d))
    (should (cl-find-if (lambda (e) (and (= (car e) 2) (>= (nth 2 e) 0))) d))))

(ert-deftest nhmac-menu-no-lisp-on-open ()
  "AppKit updating the menu as it opens runs no Lisp: no `menu-bar-update-hook' and no `:filter'."
  (skip-unless (nhmac-menu-gui--naglfar-p))
  (nhmac-menu-gui--settle)
  (let ((hooked 0) (hook (lambda () (cl-incf hooked))))
    (add-hook 'menu-bar-update-hook hook)
    (add-hook 'activate-menubar-hook hook)
    (unwind-protect
        (progn
          (dotimes (_ 5) (nhmac--call 'test-menu-update))
          (should (= hooked 0)))
      (remove-hook 'menu-bar-update-hook hook)
      (remove-hook 'activate-menubar-hook hook))))

(ert-deftest nhmac-menu-buffers-after-filter ()
  "During a long idle, a process filter creates a buffer, and within 0.5 s the Buffers menu lists it."
  (skip-unless (nhmac-menu-gui--naglfar-p))
  (nhmac-menu-gui--settle)
  (let* ((name "nhmac-menu-late-buffer")
         (proc (make-process :name "nhmac-menu-late" :command '("sh" "-c" "sleep 3; echo go")
                             :connection-type 'pipe :noquery t
                             :filter (lambda (_p _s) (get-buffer-create name))))
         (deadline (+ (float-time) 6))
         listed)
    (unwind-protect
        (progn
          (while (and (not (get-buffer name)) (< (float-time) deadline))
            (accept-process-output proc 0.2))
          (should (get-buffer name))
          (let ((t0 (float-time)))
            (while (and (not listed) (< (- (float-time) t0) 1.5))
              (sit-for 0.1)
              (setq listed (cl-find-if (lambda (e) (string-match-p name (cadr e)))
                                       (nhmac-menu-gui--dump))))
            (message "nhmac-menu-buffers-after-filter: listed after %.2f s" (- (float-time) t0))
            (should listed)
            (should (< (- (float-time) t0) 0.5))))
      (delete-process proc)
      (kill-buffer name))))

(ert-deftest nhmac-menu-stale-generation-dropped ()
  "A choice from an older tree is dropped; one from the current tree becomes the menu-bar events of its path."
  (skip-unless (nhmac-menu-gui--naglfar-p))
  (let ((nhmac-menu-generation 99)
        (nhmac-menu-paths '((3 . (nhmac-test-item)))))
    (nhmac--call 'test-menu-choose 98 3)
    (should-not (read-event nil nil 0.5))
    (nhmac--call 'test-menu-choose 99 3)
    (should (eq (read-event nil nil 2) 'menu-bar))
    (should (eq (read-event nil nil 2) 'nhmac-test-item))))

(ert-deftest nhmac-popup-menu ()
  "`x-popup-menu' runs the shell's tracking loop in one call: a cancelled popup returns nil and a chosen item its value,
and neither overlaps a take."
  (skip-unless (nhmac-menu-gui--naglfar-p))
  (let ((menu '("T" ("P" ("a" . first) ("b" . second))))
        (before (nhmac-stats)))
    (nhmac--call 'test-popup-end 0.5)
    (should (null (x-popup-menu t menu)))
    (nhmac--call 'test-popup-end 0.5 1)
    (should (eq (x-popup-menu t menu) 'second))
    (should (= (- (plist-get (nhmac-stats) :overlapping-takes) (plist-get before :overlapping-takes)) 0))))

(ert-deftest nhmac-dialog ()
  "`x-popup-dialog' (a menu of the buttons) returns the chosen button's value."
  (skip-unless (nhmac-menu-gui--naglfar-p))
  (nhmac--call 'test-popup-end 0.5 0)
  (should (eq (x-popup-dialog t '("Sure?" ("Yes" . yes) ("No" . no))) 'yes)))

(ert-deftest nhmac-dock-menu ()
  "The Dock's menu is built from the same records: the delegate serves NS's New Frame, and choosing it makes a frame."
  (skip-unless (nhmac-menu-gui--naglfar-p))
  (nhmac-menu-build)
  (let ((dump (nhmac--query 'test-dock-menu -1)))
    (message "nhmac-dock-menu: %S" dump)
    (should (string-match-p "\\`0\tNew Frame\t5\t1\n" dump)))
  (let ((before (length (frame-list))))
    (nhmac--query 'test-dock-menu 0)
    (let ((cmd (key-binding (vector (read-event nil nil 2) (read-event nil nil 2)))))
      (should (eq cmd 'nhmac-dock-new-frame))
      (let ((f (call-interactively cmd)))
        (unwind-protect
            (should (= (length (frame-list)) (1+ before)))
          (when (frame-live-p f) (delete-frame f t)))))))

;;; nhmac-menu-gui-tests.el ends here
