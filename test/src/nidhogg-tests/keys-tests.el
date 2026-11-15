;;; keys-tests.el --- The define-key redirect  -*- lexical-binding: t -*-
;; test-class: det

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

;; `--with-nidhogg-keys' adds `nidhogg-define-key-redirect-function'
;; (`src/keymap.c'): `define-key' calls it before the prefix walk and
;; defines the key in the keymap it returns, if any.  drgnx uses it to
;; keep definitions out of its composed global map: native-compiled
;; callers reach `define-key' without going through advice.
;;
;; Pure (pattern D): no time is waited for, no threads, no processes.
;; The rows skip on a build configured without the flag.

;;; Code:

(require 'ert)

(defvar nidhogg-define-key-redirect-function)

(defun keys-tests--built-p ()
  "Whether this Emacs was configured with the define-key redirect."
  (string-search "--with-nidhogg-keys" system-configuration-options))

(defmacro keys-tests--with-redirect (fn &rest body)
  "Run BODY with FN as the redirect function."
  (declare (indent 1))
  `(let ((nidhogg-define-key-redirect-function ,fn)) ,@body))

(ert-deftest keys-tests/built-in-p ()
  "A build with the flag has the variable, nil by default."
  (skip-unless (keys-tests--built-p))
  (should (boundp 'nidhogg-define-key-redirect-function))
  (should-not (default-value 'nidhogg-define-key-redirect-function)))

(ert-deftest keys-tests/redirects-to-the-returned-map ()
  "A keymap returned gets the definition; the asked-for map stays as it was."
  (skip-unless (keys-tests--built-p))
  (let* ((asked (make-sparse-keymap)) (to (make-sparse-keymap)) seen)
    (keys-tests--with-redirect
        (lambda (map key def remove) (push (list map key def remove) seen) (and (eq map asked) to))
      (define-key asked (kbd "C-c z") #'ignore))
    (should (equal seen (list (list asked (kbd "C-c z") #'ignore nil))))
    (should (equal asked '(keymap)))
    ;; The prefix walk runs in the returned map: it gets its own C-c prefix.
    (should (eq (lookup-key to (kbd "C-c z")) #'ignore))
    (should (keymapp (lookup-key to (kbd "C-c"))))))

(ert-deftest keys-tests/nil-leaves-the-definition-alone ()
  "nil from the function defines the key where it was asked for."
  (skip-unless (keys-tests--built-p))
  (let ((m (make-sparse-keymap)) (n 0))
    (keys-tests--with-redirect (lambda (&rest _) (setq n (1+ n)) nil)
      (define-key m "a" #'ignore)
      (define-key m "a" nil t))
    (should (= n 2))
    (should (equal m '(keymap)))))

(ert-deftest keys-tests/remove-is-passed ()
  "REMOVE reaches the function, and the removal lands in the returned map."
  (skip-unless (keys-tests--built-p))
  (let ((asked (make-sparse-keymap)) (to (make-sparse-keymap)) removes)
    (define-key to "a" #'ignore)
    (keys-tests--with-redirect (lambda (_m _k _d remove) (push remove removes) to)
      (define-key asked "a" nil t))
    (should (equal removes '(t)))
    (should (equal to '(keymap)))))

(ert-deftest keys-tests/every-caller-is-seen ()
  "`keymap-set', `global-set-key', `local-set-key', `funcall' and `apply' all reach it."
  (skip-unless (keys-tests--built-p))
  (let ((g (make-sparse-keymap)) (l (make-sparse-keymap)) (to (make-sparse-keymap)) keys)
    (with-temp-buffer
      (let ((old (current-global-map)))
        (unwind-protect
            (progn
              (use-global-map g)
              (use-local-map l)
              (keys-tests--with-redirect (lambda (_m key _d _r) (push (key-description key) keys) to)
                (keymap-set g "C-c a" #'ignore)
                (global-set-key (kbd "C-c b") #'ignore)
                (local-set-key (kbd "C-c c") #'ignore)
                (funcall #'define-key g (kbd "C-c d") #'ignore)
                (apply #'define-key (list g (kbd "C-c e") #'ignore))))
          (use-global-map old))))
    (should (equal (sort keys #'string<) '("C-c a" "C-c b" "C-c c" "C-c d" "C-c e")))
    (should (equal g '(keymap)))
    (should (equal l '(keymap)))
    (dolist (k '("a" "b" "c" "d" "e"))
      (should (eq (lookup-key to (kbd (concat "C-c " k))) #'ignore)))))

(ert-deftest keys-tests/the-callers-are-native ()
  "The callers `every-caller-is-seen' goes through are native-compiled in this build, so that row shows native code
reaching the redirect (no advice could see it with subr trampolines off)."
  (skip-unless (keys-tests--built-p))
  (skip-unless (and (fboundp 'native-comp-available-p) (native-comp-available-p)))
  (dolist (f '(keymap-set global-set-key local-set-key))
    (should (native-comp-function-p (symbol-function f)))))

(ert-deftest keys-tests/a-composed-map-write-goes-to-the-first-map ()
  "drgnx's shape: a write into (keymap USER PROFILE . PARENT) under PROFILE's prefix
lands in USER, never in PROFILE's prefix map."
  (skip-unless (keys-tests--built-p))
  (let* ((user (make-sparse-keymap)) (profile (make-sparse-keymap)) (parent (make-sparse-keymap))
         (composed (make-composed-keymap (list user profile) parent)))
    (define-key profile (kbd "C-k r") #'ignore)
    (keys-tests--with-redirect (lambda (map &rest _) (and (eq map composed) user))
      (define-key composed (kbd "C-k x") #'forward-char))
    (should (eq (lookup-key user (kbd "C-k x")) #'forward-char))
    (should-not (lookup-key profile (kbd "C-k x")))
    (should (eq (lookup-key composed (kbd "C-k x")) #'forward-char))
    (should (eq (lookup-key composed (kbd "C-k r")) #'ignore))))

(ert-deftest keys-tests/its-own-writes-are-not-redirected ()
  "The variable is nil while the function runs and restored after, also on an error."
  (skip-unless (keys-tests--built-p))
  (let* ((m (make-sparse-keymap)) (side (make-sparse-keymap)) inside
         (fn (lambda (_map key def _r)
               (setq inside nidhogg-define-key-redirect-function)
               (define-key side key def)
               nil)))
    (keys-tests--with-redirect fn
      (define-key m "a" #'ignore)
      (should (eq nidhogg-define-key-redirect-function fn)))
    (should-not inside)
    (should (eq (lookup-key side "a") #'ignore))
    (should (eq (lookup-key m "a") #'ignore))
    (let ((fail (lambda (&rest _) (error "Refused"))))
      (keys-tests--with-redirect fail
        (should-error (define-key m "b" #'ignore))
        (should (eq nidhogg-define-key-redirect-function fail)))
      (should-not (lookup-key m "b")))))

(ert-deftest keys-tests/a-non-keymap-is-an-error ()
  "A value that is neither nil nor a keymap signals, and nothing is defined."
  (skip-unless (keys-tests--built-p))
  (let ((m (make-sparse-keymap)))
    (keys-tests--with-redirect (lambda (&rest _) 'not-a-map)
      (should-error (define-key m "a" #'ignore) :type 'wrong-type-argument))
    (should (equal m '(keymap)))))

;;; keys-tests.el ends here
