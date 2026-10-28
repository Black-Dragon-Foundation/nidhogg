;;; nhwl-settings.el --- desktop settings for Nidhogg's Wayland shell  -*- lexical-binding: t -*-

;; Copyright (C) 2026 Black Dragon Foundation contributors.

;; This file is part of Nidhogg.

;; Nidhogg is free software: you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or (at
;; your option) any later version.

;; Nidhogg is distributed in the hope that it will be useful,
;; but WITHOUT ANY WARRANTY; without even the implied warranty of
;; MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
;; GNU General Public License for more details.

;; You should have received a copy of the GNU General Public License
;; along with Nidhogg.  If not, see <https://www.gnu.org/licenses/>.

;;; Commentary:

;; Without GTK nothing iterates GLib's main context for GSettings
;; change signals, so the shell reads the desktop's settings from the
;; Settings portal (org.freedesktop.portal.Settings) over D-Bus and
;; follows its `SettingChanged' signal.  The values are the colour
;; scheme, the accent colour, the title-bar button layout and the
;; cursor theme and size.  Without a portal the defaults apply.
;;
;; The settings are read once, when the first graphical frame is made,
;; never when this file is loaded.

;;; Code:

(declare-function dbus-call-method "dbus")
(declare-function dbus-register-signal "dbus")

(defgroup nhwl-settings nil
  "Desktop settings of Nidhogg's Wayland shell."
  :group 'frames
  :version "31.1")

(defcustom nhwl-selection-timeout 1.0
  "Seconds to wait for another program to hand over a selection.
After that the read gives nil and Emacs goes on."
  :type 'number
  :version "31.1"
  :group 'nhwl-settings)

(defface nidhogg-title-bar
  '((t :background "#ebebeb" :foreground "#2e3436"))
  "Face whose colours are those of an active window's title bar.
Its default follows the desktop's colour scheme; a theme overrides it."
  :version "31.1"
  :group 'nhwl-settings)

(defface nidhogg-title-bar-inactive
  '((t :background "#fafafa" :foreground "#919191"))
  "Face whose colours are those of an inactive window's title bar.
Its default follows the desktop's colour scheme; a theme overrides it."
  :version "31.1"
  :group 'nhwl-settings)

(defvar nhwl-settings-changed-functions nil
  "Abnormal hook run when a desktop setting changes.
Each function is called with the key (a symbol) and the new value.")

(defconst nhwl-settings--keys
  '((color-scheme "org.freedesktop.appearance" "color-scheme" 0)
    (accent-color "org.freedesktop.appearance" "accent-color" nil)
    (button-layout "org.gnome.desktop.wm.preferences" "button-layout"
                   "appmenu:close")
    (cursor-theme "org.gnome.desktop.interface" "cursor-theme" "default")
    (cursor-size "org.gnome.desktop.interface" "cursor-size" 24))
  "Each setting: (KEY NAMESPACE PORTAL-KEY DEFAULT).")

(defvar nhwl-settings--values (make-hash-table :test 'eq)
  "KEY -> the value read from the portal.  A missing key has its default.")

(defvar nhwl-settings--initialized nil
  "Non-nil once the portal has been read.")

(defun nhwl-settings-get (key)
  "Return the desktop setting KEY, or its default.
KEY is `color-scheme' (0 none, 1 dark, 2 light), `accent-color' (a
list (R G B) of floats, or nil), `button-layout' (a string),
`cursor-theme' or `cursor-size'."
  (let ((entry (assq key nhwl-settings--keys)))
    (unless entry (error "Unknown desktop setting: %S" key))
    (gethash key nhwl-settings--values (nth 3 entry))))

(defun nhwl-settings--normalize (key value)
  "Return the portal's VALUE for KEY as Lisp, or nil if it is unusable."
  ;; dbus.el hands a variant over as its content, but `Read' on an old
  ;; portal wraps it in a second variant, which shows as a 1-list.
  (while (and (consp value) (null (cdr value)) (consp (car value)))
    (setq value (car value)))
  ;; `cond', not `pcase': this file is loaded while Emacs is dumped, before `pcase'.
  (cond
   ((eq key 'accent-color)
    (and (consp value) (= (length value) 3) (seq-every-p #'numberp value)
         (seq-every-p (lambda (c) (<= 0.0 c 1.0)) value)
         (mapcar #'float value)))
   ((eq key 'cursor-size)
    (when (and (consp value) (null (cdr value))) (setq value (car value)))
    (and (integerp value) (> value 0) value))
   ((eq key 'color-scheme)
    (when (and (consp value) (null (cdr value))) (setq value (car value)))
    (and (memq value '(0 1 2)) value))
   (t
    (when (and (consp value) (null (cdr value))) (setq value (car value)))
    (and (stringp value) value))))

(defun nhwl-settings--read (namespace name)
  "Read NAME of NAMESPACE from the portal, or return nil."
  (let ((args (list :session "org.freedesktop.portal.Desktop"
                    "/org/freedesktop/portal/desktop"
                    "org.freedesktop.portal.Settings")))
    (condition-case nil
        (apply #'dbus-call-method
               (append args (list "ReadOne" namespace name)))
      (dbus-error
       ;; Portals older than version 2 only have `Read'.
       (condition-case nil
           (apply #'dbus-call-method
                  (append args (list "Read" namespace name)))
         (dbus-error nil))))))

(defun nhwl-settings--title-bar-colors (scheme)
  "Apply the default title-bar colours for colour SCHEME (1 is dark)."
  (let ((dark (eql scheme 1)))
    (face-spec-set
     'nidhogg-title-bar
     `((t :background ,(if dark "#303030" "#ebebeb")
          :foreground ,(if dark "#ffffff" "#2e3436")))
     'face-defface-spec)
    (face-spec-set
     'nidhogg-title-bar-inactive
     `((t :background ,(if dark "#242424" "#fafafa")
          :foreground "#919191"))
     'face-defface-spec)))

(defun nhwl-settings--set (key value)
  "Store VALUE (nil means unusable) for KEY; run the hook if it changed."
  (let ((old (nhwl-settings-get key)))
    (if value
        (puthash key value nhwl-settings--values)
      (remhash key nhwl-settings--values))
    (let ((new (nhwl-settings-get key)))
      (when (eq key 'color-scheme)
        (nhwl-settings--title-bar-colors new))
      (unless (equal old new)
        (run-hook-with-args 'nhwl-settings-changed-functions key new)))))

(defun nhwl-settings--signal (namespace name value &rest _)
  "Handle the portal's `SettingChanged' for NAMESPACE, NAME and VALUE."
  (let ((entry (seq-find (lambda (e) (and (equal (nth 1 e) namespace)
                                            (equal (nth 2 e) name)))
                           nhwl-settings--keys)))
    (when entry
      (nhwl-settings--set (car entry)
                          (nhwl-settings--normalize (car entry) value)))))

;; The shell draws the pointer from this theme where the compositor has no cursor-shape protocol.
(defun nhwl-settings--apply-cursor ()
  "Give the shell the desktop's cursor theme and size."
  (when (fboundp 'nhwl-set-cursor-theme)
    (nhwl-set-cursor-theme (nhwl-settings-get 'cursor-theme)
                           (nhwl-settings-get 'cursor-size))))

(defun nhwl-settings--cursor-changed (key _value)
  "Apply the cursor theme when the setting KEY is one of its two."
  (when (memq key '(cursor-theme cursor-size))
    (nhwl-settings--apply-cursor)))

(add-hook 'nhwl-settings-changed-functions #'nhwl-settings--cursor-changed)

(defun nhwl-settings-init ()
  "Read the desktop settings from the portal and follow their changes.
Without a portal the defaults stay.  Does nothing the second time."
  (unless nhwl-settings--initialized
    (setq nhwl-settings--initialized t)
    (when (require 'dbus nil t)
      (dolist (entry nhwl-settings--keys)
        (let ((value (condition-case nil
                         (nhwl-settings--read (nth 1 entry) (nth 2 entry))
                       (error nil))))
          (nhwl-settings--set (car entry)
                              (nhwl-settings--normalize (car entry) value))))
      (nhwl-settings--apply-cursor)
      (condition-case nil
          (dbus-register-signal
           :session "org.freedesktop.portal.Desktop"
           "/org/freedesktop/portal/desktop"
           "org.freedesktop.portal.Settings" "SettingChanged"
           #'nhwl-settings--signal)
        (error nil)))))

(defun nhwl-settings--first-frame (frame)
  "Read the settings at the first graphical FRAME, then stop listening."
  (when (display-graphic-p frame)
    (remove-hook 'after-make-frame-functions #'nhwl-settings--first-frame)
    (nhwl-settings-init)))

(defun nhwl-settings--parse-side (string)
  "Return the known button names in STRING (comma separated) as symbols."
  (let (names)
    (dolist (name (split-string string "," t "[ \t]+"))
      (let ((sym (intern name)))
        (when (memq sym '(close maximize minimize appmenu))
          (push sym names))))
    (nreverse names)))

(defun nhwl-settings-button-layout ()
  "Return the title-bar button layout as (LEFT . RIGHT).
Each side is a list of `close', `maximize', `minimize' and `appmenu',
from the `button-layout' setting (GNOME's \"appmenu:minimize,maximize,close\"
syntax); unknown names are dropped."
  (let* ((layout (nhwl-settings-get 'button-layout))
         (colon (string-search ":" layout)))
    (if colon
        (cons (nhwl-settings--parse-side (substring layout 0 colon))
              (nhwl-settings--parse-side (substring layout (1+ colon))))
      (cons (nhwl-settings--parse-side layout) nil))))

(add-hook 'after-make-frame-functions #'nhwl-settings--first-frame)

(provide 'nhwl-settings)

;;; nhwl-settings.el ends here
