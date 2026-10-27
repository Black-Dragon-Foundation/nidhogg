;;; nhwl-settings-tests.el --- tests for the Wayland shell's desktop settings  -*- lexical-binding: t -*-

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

;; Batch tests of lisp/term/nhwl-settings.el.  The Settings portal is an
;; external service, so its answers, its errors and its `SettingChanged'
;; signal are simulated by replacing `dbus-call-method' and
;; `dbus-register-signal'.

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'dbus)
(require 'nhwl-settings)

(defmacro nhwl-settings-test--with-portal (answers &rest body)
  "Run BODY with a fresh settings state and a simulated portal.
ANSWERS maps (NAMESPACE . KEY) to a value; others signal `dbus-error'.
`handler' is bound to the function registered for `SettingChanged'."
  (declare (indent 1))
  `(let ((nhwl-settings--values (make-hash-table :test 'eq))
         (nhwl-settings--initialized nil)
         (nhwl-settings-changed-functions nil)
         (handler nil))
     (cl-letf (((symbol-function 'dbus-call-method)
                (lambda (&rest args)
                  (let ((hit (assoc (cons (nth 5 args) (nth 6 args))
                                    ,answers)))
                    (if (and hit (equal (nth 4 args) "ReadOne"))
                        (cdr hit)
                      (signal 'dbus-error '("no portal"))))))
               ((symbol-function 'dbus-register-signal)
                (lambda (_bus _svc _path _iface _member fn)
                  (setq handler fn))))
       ,@body)))

(ert-deftest nhwl-settings/layout-parses-gnome-syntax ()
  (cl-flet ((layout (s)
              (let ((nhwl-settings--values (make-hash-table :test 'eq)))
                (puthash 'button-layout s nhwl-settings--values)
                (nhwl-settings-button-layout))))
    (should (equal (layout "appmenu:minimize,maximize,close")
                   '((appmenu) . (minimize maximize close))))
    (should (equal (layout ":close") '(nil . (close))))
    (should (equal (layout "close,foo:") '((close) . nil)))))

(ert-deftest nhwl-settings/defaults-without-portal ()
  (nhwl-settings-test--with-portal nil
    (nhwl-settings-init)
    (should (equal (nhwl-settings-get 'button-layout) "appmenu:close"))
    (should (eq (nhwl-settings-get 'color-scheme) 0))
    (should (eq (nhwl-settings-get 'cursor-size) 24))
    (should (equal (nhwl-settings-get 'cursor-theme) "default"))
    (should-not (nhwl-settings-get 'accent-color))))

(ert-deftest nhwl-settings/reads-portal-values ()
  (nhwl-settings-test--with-portal
      '((("org.freedesktop.appearance" . "color-scheme") . 1)
        (("org.freedesktop.appearance" . "accent-color") . (0.2 0.4 0.6))
        (("org.gnome.desktop.interface" . "cursor-size") . 32))
    (nhwl-settings-init)
    (should (eq (nhwl-settings-get 'color-scheme) 1))
    (should (equal (nhwl-settings-get 'accent-color) '(0.2 0.4 0.6)))
    (should (eq (nhwl-settings-get 'cursor-size) 32))
    (should (equal (face-attribute 'nidhogg-title-bar :background nil t)
                   "#303030"))))

(ert-deftest nhwl-settings/changed-signal-updates-and-runs-hook ()
  (nhwl-settings-test--with-portal nil
    (let (calls)
      (add-hook 'nhwl-settings-changed-functions
                (lambda (key value) (push (cons key value) calls)))
      (nhwl-settings-init)
      (should handler)
      (funcall handler "org.freedesktop.appearance" "color-scheme" 1)
      (should (eq (nhwl-settings-get 'color-scheme) 1))
      (should (equal calls '((color-scheme . 1))))
      (should (equal (face-attribute 'nidhogg-title-bar :background nil t)
                     "#303030"))
      (should (equal (face-attribute 'nidhogg-title-bar-inactive :background
                                     nil t)
                     "#242424"))
      ;; An unrelated setting does nothing.
      (funcall handler "org.example" "color-scheme" 2)
      (should (equal calls '((color-scheme . 1))))
      (funcall handler "org.freedesktop.appearance" "color-scheme" 2)
      (should (equal (face-attribute 'nidhogg-title-bar :background nil t)
                     "#ebebeb")))))

(provide 'nhwl-settings-tests)

;;; nhwl-settings-tests.el ends here
