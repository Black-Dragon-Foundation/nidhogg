;;; watchdog-resolve-tests.el --- tests for the stall watchdog (resolve)  -*- lexical-binding: t -*-

;; Copyright (C) 2026 Black Dragon Foundation contributors

;; This file is part of Nidhogg.

;; Nidhogg is free software: you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or
;; (at your option) any later version.

;; Nidhogg is distributed in the hope that it will be useful,
;; but WITHOUT ANY WARRANTY; without even the implied warranty of
;; MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
;; GNU General Public License for more details.

;; You should have received a copy of the GNU General Public License
;; along with Nidhogg.  If not, see <https://www.gnu.org/licenses/>.

;;; Commentary:

;; Attribution.  The fixture rows take real samples inside the
;; `wdfix' package (watchdog-resources/wdfix); the rule rows resolve
;; hand-built records whose function infos are seeded into the cache, so
;; they need neither the index nor a watchdog thread.

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'package)
;; `native-comp-speed' is bound below: load its definition first, or the
;; binding is lexical and comp.el's `defcustom' signals when compiling loads it.
(when (and (fboundp 'native-comp-available-p) (native-comp-available-p))
  (require 'comp))
(defvar native-comp-speed)

(defvar lisp-directory)
(defvar package-alist)
(defvar nidhogg-watchdog--test-last-sample)
(defvar nidhogg-watchdog--test-sample-on-phase)

(defconst wdres--dir
  (file-name-directory (or load-file-name buffer-file-name))
  "This file's directory.")

(defmacro wdres--fresh (&rest body)
  "Run BODY with an empty index, function cache and package map."
  (declare (indent 0) (debug t))
  `(let ((nidhogg-watchdog--index-started nil)
         (nidhogg-watchdog--index-syms nil)
         (nidhogg-watchdog--index-bases nil)
         (nidhogg-watchdog--index-todo nil)
         (nidhogg-watchdog--fn-cache (make-hash-table :test 'eq :weakness 'key))
         (nidhogg-watchdog--pkg-map nil)
         (nidhogg-watchdog-package-functions nil))
     (skip-unless (fboundp 'nidhogg-watchdog-resolve))
     ,@body))

(defmacro wdres--with-fixture (&rest body)
  "Run BODY with the wdfix package copied to a temp `package-alist' entry.
`pkgdir' is the package's directory."
  (declare (indent 0) (debug t))
  `(let* ((tmp (make-temp-file "wdfix" t))
          (pkgdir (expand-file-name "wdfix-1.0" tmp))
          (package-alist nil)
          (load-path (cons pkgdir load-path))
          (load-history load-history))
     (unwind-protect
         (progn
           (copy-directory
            (expand-file-name "watchdog-resources/wdfix/wdfix-1.0" wdres--dir)
            pkgdir nil t t)
           (package-load-descriptor pkgdir)
           ,@body)
       (delete-directory tmp t))))

(defmacro wdres--sampling (&rest body)
  "Run BODY with the watchdog started (no monitor thread)."
  (declare (indent 0) (debug t))
  `(progn
     (setq nidhogg-watchdog--test-last-sample nil)
     (nidhogg-watchdog--start 100 2000 nil :no-monitor)
     (unwind-protect (progn ,@body)
       (setq nidhogg-watchdog--test-sample-on-phase nil)
       (nidhogg-watchdog--stop))))

(defun wdres--s (&rest kv)
  "A sample plist with defaults, overridden by KV."
  (let ((s (list :t-ms 1.0 :phase 'lisp :reason nil :gc nil :frames []
                 :outer [] :skipped 0 :hooks [] :lisp-thread nil)))
    (while kv (setq s (plist-put s (pop kv) (pop kv))))
    s))

(defun wdres--rec (samples &rest kv)
  "A raw record with SAMPLES (a list), overridden by KV."
  (let ((r (list :wall-ms 100.0 :gc-ms 0.0 :cpu-ms 90.0
                 :samples (vconcat samples))))
    (while kv (setq r (plist-put r (pop kv) (pop kv))))
    r))

(defun wdres--seed (&rest specs)
  "Seed the cache: each of SPECS is a symbol or (SYMBOL . PACKAGE)."
  (dolist (s specs)
    (let ((sym (if (consp s) (car s) s)))
      (puthash sym (list :name (symbol-name sym)
                         :pkg (and (consp s) (cdr s)))
               nidhogg-watchdog--fn-cache))))

(defun wdres--stack (r)
  "The frame names of the first merged stack of the record R."
  (plist-get (car (plist-get r :stacks)) :frames))

(ert-deftest watchdog/attribution-native-and-bytecode ()
  ;; A sample taken inside the wdfix package, byte-compiled and then
  ;; native-compiled at speed 2: the culprit's package is `wdfix' both times.
  (skip-unless (and (fboundp 'native-comp-available-p)
                    (native-comp-available-p)))
  (wdres--fresh
    (wdres--with-fixture
      (let* ((core (expand-file-name "wdfix-core.el" pkgdir))
             (native-comp-eln-load-path
              (list (expand-file-name "eln/" tmp)))
             (native-comp-speed 2)
             (native-comp-jit-compilation nil))
        (should (byte-compile-file core))
        (load (concat core "c") nil t)
        (wdres--sampling
          (let ((r (nidhogg-watchdog-resolve
                    (wdres--rec (list (wdfix-work))))))
            (should (equal (plist-get (plist-get r :culprit) :fn) "wdfix-work"))
            (should (equal (plist-get (plist-get r :culprit) :pkg) "wdfix"))))
        ;; Native: a new index and cache, the same package.
        (should (stringp (native-compile core)))
        (load (car (directory-files-recursively tmp "wdfix-core.*\\.eln\\'"))
              nil t)
        (should (subr-native-elisp-p (symbol-function 'wdfix-work)))
        (let ((nidhogg-watchdog--index-started nil)
              (nidhogg-watchdog--fn-cache
               (make-hash-table :test 'eq :weakness 'key)))
          (wdres--sampling
            (let ((r (nidhogg-watchdog-resolve
                      (wdres--rec (list (wdfix-work))))))
              (should (equal (plist-get (plist-get r :culprit) :fn)
                             "wdfix-work"))
              (should (equal (plist-get (plist-get r :culprit) :pkg) "wdfix")))
            ;; An anonymous native function is `<lambda in wdfix-core>'.
            (let ((r (nidhogg-watchdog-resolve
                      (wdres--rec (list (funcall (wdfix-make-lambda)))))))
              (should (member "<lambda in wdfix-core>" (wdres--stack r))))))))))

(ert-deftest watchdog/signature-same-byte-and-native ()
  ;; A regexp search in wdfix from byte code and from native code: one
  ;; signature, phase `regex'.
  (skip-unless (and (fboundp 'native-comp-available-p)
                    (native-comp-available-p)))
  (wdres--fresh
    (wdres--with-fixture
      (let* ((core (expand-file-name "wdfix-core.el" pkgdir))
             (native-comp-eln-load-path
              (list (expand-file-name "eln/" tmp)))
             (native-comp-speed 2)
             (native-comp-jit-compilation nil)
             (take (lambda ()
                     (setq nidhogg-watchdog--test-last-sample nil)
                     (let ((nidhogg-watchdog--test-sample-on-phase 'regex))
                       (wdfix-hot-loop))
                     (nidhogg-watchdog-resolve
                      (wdres--rec (list nidhogg-watchdog--test-last-sample)))))
             sig-byte sig-native)
        (should (byte-compile-file core))
        (load (concat core "c") nil t)
        (wdres--sampling
          (setq sig-byte (plist-get (funcall take) :signature)))
        (should (stringp (native-compile core)))
        (load (car (directory-files-recursively tmp "wdfix-core.*\\.eln\\'"))
              nil t)
        (let ((nidhogg-watchdog--index-started nil)
              (nidhogg-watchdog--fn-cache
               (make-hash-table :test 'eq :weakness 'key)))
          (wdres--sampling
            (setq sig-native (plist-get (funcall take) :signature))))
        (should (equal sig-byte sig-native))
        (should (string-suffix-p "/regex" sig-byte))
        (should (string-match-p ">wdfix-hot-loop/" sig-byte))))))

(ert-deftest watchdog/advice-and-closure ()
  (wdres--fresh
    (wdres--seed '(wdres-named . "ext"))
    (defun wdres--adv-target () 1)
    (defun wdres--adv-fn (orig) (funcall orig))
    (advice-add 'wdres--adv-target :around #'wdres--adv-fn)
    (unwind-protect
        (let* ((closure (let ((x 1)) (lambda () x)))
               ;; A closure is `<lambda>', credited to the named frame
               ;; below it; so is `<gone>'.
               (r (nidhogg-watchdog-resolve
                   (wdres--rec (list (wdres--s :frames (vector closure 'wdres-named))
                                     (wdres--s :frames
                                               (vector 'nidhogg-watchdog--gone
                                                       'wdres-named))))))
               (adv (symbol-function 'wdres--adv-target))
               (r2 (nidhogg-watchdog-resolve
                    (wdres--rec (list (wdres--s :frames (vector adv)))))))
          (should (equal (wdres--stack r) '("<lambda>" "wdres-named")))
          (should (member '(:n 1 :frames ("<gone>" "wdres-named"))
                          (plist-get r :stacks)))
          (should (equal (plist-get (plist-get r :culprit) :fn) "wdres-named"))
          (should (equal (plist-get (plist-get r :culprit) :pkg) "ext"))
          (should (equal (plist-get (plist-get r2 :culprit) :fn)
                         "advice: wdres--adv-fn")))
      (advice-remove 'wdres--adv-target #'wdres--adv-fn))))

(ert-deftest watchdog/index-slices ()
  ;; A load-history of 3,000 files is indexed in slices of at most 2 ms of
  ;; virtual time, and the lookups agree with `symbol-file'.
  (wdres--fresh
    (let* ((now 0.0)
           (nidhogg-watchdog--clock (lambda () (setq now (+ now 0.00001))))
           (load-history
            (cl-loop for i from 0 below 3000
                     collect (list (format "/wds/f%d.el" i)
                                   (cons 'defun (intern (format "wds%d-fn" i)))
                                   (intern (format "wds%d-var" i))
                                   (cons 'defvar (intern (format "wds%d-dv" i))))))
           slices)
      (unwind-protect
          (progn
            (while (not (nidhogg-watchdog--index-done-p))
              (let ((t0 now))
                (nidhogg-watchdog--index-step (+ now 0.002))
                (push (- now t0) slices)))
            (should (> (length slices) 1))
            (dolist (s slices)
              (should (<= s (+ 0.002 0.00003))))
            (dolist (i '(0 1 1499 2998 2999))
              (let ((fn (intern (format "wds%d-fn" i)))
                    (dv (intern (format "wds%d-dv" i))))
                (should (equal (nidhogg-watchdog--index-lookup fn)
                               (symbol-file fn 'defun)))
                (should (equal (nidhogg-watchdog--index-lookup dv)
                               (symbol-file dv 'defvar)))
                (should (equal (nidhogg-watchdog--index-lookup
                                (intern (format "wds%d-var" i)))
                               (format "/wds/f%d.el" i))))))
        (remove-hook 'after-load-functions
                     #'nidhogg-watchdog--index-after-load)))))

(ert-deftest watchdog/entry-and-culprit ()
  (wdres--fresh
    (wdres--seed 'wdres-cmd 'wdres-timerfn 'wdres-filter 'wdres-hookfn
                 'wdres-inner '(timer-event-handler . "emacs")
                 '(wdres-em . "emacs") '(wdres-ext . "ext"))
    (cl-flet ((entry (&rest kv)
                (let ((r (nidhogg-watchdog-resolve
                          (wdres--rec (list (apply #'wdres--s kv))))))
                  (plist-get r :entry))))
      ;; A command: the frame after command-execute and call-interactively.
      (let ((e (entry :frames [wdres-inner wdres-cmd]
                      :outer [call-interactively command-execute])))
        (should (eq (plist-get e :kind) 'command))
        (should (equal (plist-get e :fn) "wdres-cmd")))
      ;; A timer: the frame after timer-event-handler, past `apply'.
      (let ((e (entry :frames [wdres-inner wdres-timerfn apply
                                           timer-event-handler])))
        (should (eq (plist-get e :kind) 'timer))
        (should (equal (plist-get e :fn) "wdres-timerfn")))
      ;; A filter, from the phase: the frame after the wait.
      (let ((e (entry :phase 'filter
                      :frames [wdres-inner wdres-filter accept-process-output
                                           wdres-cmd])))
        (should (eq (plist-get e :kind) 'filter))
        (should (equal (plist-get e :fn) "wdres-filter")))
      (let ((e (entry :phase 'sentinel :frames [wdres-filter])))
        (should (eq (plist-get e :kind) 'sentinel)))
      ;; A hook: its name and the function it runs.
      (let ((e (entry :frames [wdres-hookfn run-hooks]
                      :hooks [nil wdres-hook])))
        (should (eq (plist-get e :kind) 'hook))
        (should (equal (plist-get e :fn) "wdres-hook"))
        (should (equal (plist-get e :function) "wdres-hookfn")))
      ;; Redisplay and gc have no Lisp frame; a Lisp thread names itself.
      (should (eq (plist-get (entry :phase 'redisplay) :kind) 'redisplay))
      (should (eq (plist-get (entry :gc t) :kind) 'gc))
      (let ((e (entry :lisp-thread "worker")))
        (should (eq (plist-get e :kind) 'lisp-thread))
        (should (equal (plist-get e :fn) "worker"))))
    ;; The culprit: the innermost non-emacs frame in at least half the
    ;; non-GC samples, an emacs frame innermost of it notwithstanding.
    (let* ((s (wdres--s :frames [wdres-em wdres-ext wdres-inner]))
           (r (nidhogg-watchdog-resolve (wdres--rec (list s s))))
           (c (plist-get r :culprit)))
      (should (equal (plist-get c :fn) "wdres-ext"))
      (should (equal (plist-get c :pkg) "ext"))
      (should (= (plist-get c :share) 1.0))
      (should (eq (plist-get r :state) 'cpu)))
    ;; In under half the samples: the innermost emacs frame instead.
    (let* ((a (wdres--s :frames [wdres-em wdres-ext]))
           (b (wdres--s :frames [wdres-em]))
           (r (nidhogg-watchdog-resolve (wdres--rec (list a b b))))
           (c (plist-get r :culprit)))
      (should (equal (plist-get c :fn) "wdres-em"))
      (should (equal (plist-get c :pkg) "emacs")))
    ;; GC samples are not counted towards the culprit.
    (let* ((a (wdres--s :frames [wdres-ext]))
           (g (wdres--s :gc t :frames [wdres-em]))
           (r (nidhogg-watchdog-resolve (wdres--rec (list a g g g))))
           (c (plist-get r :culprit)))
      (should (equal (plist-get c :fn) "wdres-ext"))
      (should (= (plist-get c :share) 1.0))
      (should (> (plist-get (plist-get r :phases) :gc) 0.5)))))

(ert-deftest watchdog/state-rules ()
  (wdres--fresh
    (wdres--seed 'wdres-inner)
    (cl-flet ((state (&rest kv)
                (plist-get (nidhogg-watchdog-resolve
                            (apply #'wdres--rec
                                   (list (wdres--s :frames [wdres-inner]))
                                   kv))
                           :state)))
      (should (eq (state :gc-ms 60.0 :cpu-ms 40.0) 'gc))
      (should (eq (state :cpu-ms 80.0) 'cpu))
      (should (eq (state :cpu-ms 19.0) 'blocked))
      (should (eq (state :cpu-ms 50.0) 'mixed))
      (should (eq (state :cpu-ms nil) 'unsampled)))
    ;; Samples waiting for a worker count as blocked, whatever the CPU says.
    (let* ((s (wdres--s :phase 'worker-wait :reason 'display
                        :frames [wdres-inner]))
           (r (nidhogg-watchdog-resolve (wdres--rec (list s s) :cpu-ms 95.0))))
      (should (eq (plist-get r :state) 'blocked))
      (should (string-suffix-p "/worker-wait:display" (plist-get r :signature)))
      (should (= (plist-get (plist-get r :phases)
                            (intern ":worker-wait:display"))
                 1.0)))
    ;; No sample: the CPU clock alone, and `unsampled' entry and culprit.
    (let ((r (nidhogg-watchdog-resolve (wdres--rec nil :cpu-ms 5.0))))
      (should (eq (plist-get r :state) 'blocked))
      (should (eq (plist-get (plist-get r :entry) :kind) 'unsampled))
      (should (equal (plist-get (plist-get r :culprit) :fn) "unsampled")))))

(ert-deftest watchdog/package-map ()
  (wdres--fresh
    (wdres--with-fixture
      (let* ((lisp (file-name-as-directory (make-temp-file "wdlisp" t)))
             (lisp-directory lisp))
        (unwind-protect
            (progn
              ;; An installed package's file.
              (should (equal (nidhogg-watchdog--package-of
                              (expand-file-name "wdfix-core.el" pkgdir))
                             "wdfix"))
              ;; An Emacs library: package `emacs', detail `emacs/LIB'.
              (let ((f (expand-file-name "jit-lock.el" lisp)))
                (should (equal (nidhogg-watchdog--package-of f) "emacs"))
                (should (equal (nidhogg-watchdog--package-detail f)
                               "emacs/jit-lock")))
              (should (equal (nidhogg-watchdog--package-of "C") "emacs"))
              ;; Unknown.
              (should-not (nidhogg-watchdog--package-of "/nonexistent/foo.el"))
              ;; A package function goes first.
              (let ((nidhogg-watchdog-package-functions
                     (list (lambda (f) (and (string-suffix-p "foo.el" f)
                                            "custom")))))
                (should (equal (nidhogg-watchdog--package-of
                                "/nonexistent/foo.el")
                               "custom"))))
          (delete-directory lisp t))))))

;;; watchdog-resolve-tests.el ends here
