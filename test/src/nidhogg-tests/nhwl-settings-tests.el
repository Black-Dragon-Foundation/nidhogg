;;; nhwl-settings-tests.el --- tests for the Wayland shell's desktop settings  -*- lexical-binding: t -*-
;; test-class: cond

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
;; signal are simulated by replacing `dbus-call-method-asynchronously'
;; and `dbus-register-signal'; the asynchronous read itself runs against a
;; private session bus and a child Emacs that plays the portal.

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'dbus)
(require 'nhwl-settings)

(defmacro nhwl-settings-test--with-portal (answers &rest body)
  "Run BODY with a fresh settings state and a simulated portal.
ANSWERS maps (NAMESPACE . KEY) to a value; others get no answer.
`handler' is bound to the function registered for `SettingChanged' and
`timeouts' to the :timeout of every read."
  (declare (indent 1))
  `(let ((nhwl-settings--values (make-hash-table :test 'eq))
         (nhwl-settings--initialized nil)
         (nhwl-settings-changed-functions nil)
         (handler nil)
         (timeouts nil))
     (cl-letf (((symbol-function 'dbus-call-method-asynchronously)
                (lambda (_bus _svc _path _iface method fn &rest args)
                  (when (eq (car args) :timeout)
                    (push (cadr args) timeouts)
                    (setq args (cddr args)))
                  (let ((hit (assoc (cons (nth 0 args) (nth 1 args))
                                    ,answers)))
                    (when (and hit (equal method "ReadOne"))
                      (funcall fn (cdr hit))))))
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
    (should (equal (delete-dups timeouts) '(500)))
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

(ert-deftest nhwl-settings/reduced-motion-from-portal ()
  (dolist (case '((0 . 0) (1 . 1) (7 . 0)))
    (nhwl-settings-test--with-portal
        `((("org.freedesktop.appearance" . "reduced-motion") . ,(car case)))
      (nhwl-settings-init)
      (should (eq (nhwl-settings-get 'reduced-motion) (cdr case)))))
  (nhwl-settings-test--with-portal nil
    (let (calls)
      (add-hook 'nhwl-settings-changed-functions
                (lambda (key value) (push (cons key value) calls)))
      (nhwl-settings-init)
      (should (eq (nhwl-settings-get 'reduced-motion) 0))
      (funcall handler "org.freedesktop.appearance" "reduced-motion" 1)
      (should (eq (nhwl-settings-get 'reduced-motion) 1))
      (should (equal calls '((reduced-motion . 1)))))))

;; test-pattern: D (pure)
(ert-deftest nhwl-settings/contrast-and-fonts-normalise ()
  (dolist (case '((0 . 0) (1 . 1) (7 . 0)))
    (nhwl-settings-test--with-portal
        `((("org.freedesktop.appearance" . "contrast") . ,(car case)))
      (nhwl-settings-init)
      (should (eq (nhwl-settings-get 'contrast) (cdr case)))))
  (nhwl-settings-test--with-portal
      '((("org.gnome.desktop.interface" . "font-name") . "Adwaita Sans 11")
        (("org.gnome.desktop.interface" . "monospace-font-name") . 12))
    (nhwl-settings-init)
    (should (equal (nhwl-settings-get 'font-name) "Adwaita Sans 11"))
    (should-not (nhwl-settings-get 'monospace-font-name)))
  (nhwl-settings-test--with-portal nil
    (nhwl-settings-init)
    (should (eq (nhwl-settings-get 'contrast) 0))
    (should-not (nhwl-settings-get 'font-name))
    (should-not (nhwl-settings-get 'monospace-font-name))))

;; test-pattern: C (handshake): a real child that announces and parks

(defconst nhwl-settings-test--portal-child
  '(progn
     (require 'dbus)
     (let ((release-file (getenv "NHWL_TEST_RELEASE")))
       (dbus-register-service :session "org.freedesktop.portal.Desktop")
       (dbus-register-method
        :session "org.freedesktop.portal.Desktop"
        "/org/freedesktop/portal/desktop" "org.freedesktop.portal.Settings"
        "ReadOne"
        (lambda (_namespace key)
          ;; parked here until the test creates the release file
          (while (not (file-exists-p release-file))
            (accept-process-output nil 0.05))
          (cond ((equal key "color-scheme") 1)
                ((equal key "contrast") 1)
                ((equal key "reduced-motion") 7)
                ((equal key "cursor-size") 32)
                ((equal key "font-name") "Adwaita Sans 11")
                ((equal key "monospace-font-name") "JetBrains Mono 10")
                (t '(:error "org.freedesktop.portal.Error.NotFound" "none")))))
       ;; `message' goes to stderr, unbuffered; stdout is held back on a pipe
       (message "ready")
       ;; D-Bus events are special events: `read-event' runs them
       (while t (read-event nil nil 1))))
  "The child that plays the portal: it answers once the release file exists.")

(defvar nhwl-settings-test--no-portal nil
  "Non-nil: `nhwl-settings-test--with-parked-portal' starts no child.")

(defmacro nhwl-settings-test--with-parked-portal (&rest body)
  "Run BODY with a private session bus and a parked portal child.
`release' is a function that lets the child answer.  The settings state
is fresh and `nhwl-settings--bus' names the private bus."
  (declare (indent 0))
  `(let ((release-file (make-temp-name
                        (expand-file-name "nhwl-release" temporary-file-directory)))
         bus pid child)
     (with-temp-buffer
       (let ((status (condition-case err
                         (call-process "dbus-daemon" nil t nil "--fork"
                                       "--session" "--print-address"
                                       "--print-pid")
                       (error err))))
         (unless (eql status 0)
           (message "nhwl-settings: no private bus: %S %S" status
                    (buffer-string)))
         (skip-unless (eql status 0)))
       (goto-char (point-min))
       (setq bus (buffer-substring (point) (line-end-position)))
       (forward-line)
       (setq pid (string-to-number
                  (buffer-substring (point) (line-end-position)))))
     (unwind-protect
         (let* ((ready nil) (output "")
                (nhwl-settings--bus bus)
                (nhwl-settings--values (make-hash-table :test 'eq))
                (nhwl-settings--initialized nil)
                (nhwl-settings-changed-functions nil))
           (should (dbus-init-bus bus))
           ;; Only the child sees the bus as its session bus: with the
           ;; address in this Emacs's environment, dbus.el would take the
           ;; address for :session and look in the real environment.
           (unless nhwl-settings-test--no-portal
            (setq child
                 (let ((process-environment
                        (append (list (concat "DBUS_SESSION_BUS_ADDRESS=" bus)
                                      (concat "NHWL_TEST_RELEASE="
                                              release-file))
                                process-environment)))
                   (make-process
                    :name "nhwl-settings-portal" :connection-type 'pipe
                    :noquery t
                    :command (list (expand-file-name invocation-name
                                                     invocation-directory)
                                   "-Q" "--batch" "--eval"
                                   (prin1-to-string
                                    nhwl-settings-test--portal-child))
                    :filter (lambda (_p s)
                              (setq output (concat output s))
                              (when (string-search "ready" s)
                                (setq ready t))))))
            (with-timeout (60 (error "portal child never announced: %S"
                                     output))
              (while (not ready) (accept-process-output child 0.1))))
           (cl-flet ((release () (write-region "" nil release-file nil 'silent)))
             ,@body))
       (when (file-exists-p release-file) (delete-file release-file))
       (when (process-live-p child) (delete-process child))
       (when pid (signal-process pid 'TERM)))))

(ert-deftest nhwl-settings/async-read-never-blocks ()
  (nhwl-settings-test--with-parked-portal
    (let (calls)
      (add-hook 'nhwl-settings-changed-functions
                (lambda (key value) (push (cons key value) calls)))
      ;; The portal is parked: init returns with the defaults at once.
      (nhwl-settings-init)
      (should (eq (nhwl-settings-get 'color-scheme) 0))
      (should (eq (nhwl-settings-get 'contrast) 0))
      (should-not calls)
      (release)
      (with-timeout (60 (error "the answers never came: %S" calls))
        ;; D-Bus events are special events: only `read-event' runs them
        (while (< (length calls) 5) (read-event nil nil 0.1)))
      ;; One call per changed key; reduced-motion's 7 is the default.
      (should (equal (sort (mapcar #'car calls) #'string<)
                     '(color-scheme contrast cursor-size font-name
                       monospace-font-name)))
      (should (eq (nhwl-settings-get 'color-scheme) 1))
      (should (eq (nhwl-settings-get 'contrast) 1))
      (should (equal (nhwl-settings-get 'font-name) "Adwaita Sans 11"))
      (should (eq (nhwl-settings-get 'reduced-motion) 0)))))

(ert-deftest nhwl-settings/no-portal-on-the-bus-keeps-defaults ()
  (let ((nhwl-settings-test--no-portal t))
    (nhwl-settings-test--with-parked-portal
      (let ((calls nil) (errors 0)
            (count (length nhwl-settings--keys)))
        (add-hook 'nhwl-settings-changed-functions
                  (lambda (key value) (push (cons key value) calls)))
        (let ((count-error (lambda (_event _err) (setq errors (1+ errors)))))
          (add-hook 'dbus-event-error-functions count-error)
          (unwind-protect
              (progn
                (nhwl-settings-init)
                ;; Nobody owns the portal's name: the bus answers every
                ;; read with an error.
                (with-timeout (60 (error "the reads never ended: %S" errors))
                  (while (< errors count) (read-event nil nil 0.1))))
            (remove-hook 'dbus-event-error-functions count-error)))
        (should-not calls)
        (should (eq (nhwl-settings-get 'color-scheme) 0))
        (should (equal (nhwl-settings-get 'button-layout) "appmenu:close"))))))

(provide 'nhwl-settings-tests)

;;; nhwl-settings-tests.el ends here
