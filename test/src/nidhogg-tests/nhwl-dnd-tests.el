;;; nhwl-dnd-tests.el --- tests for the shell's names and conversions in Lisp  -*- lexical-binding: t -*-

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

;; Batch tests of what C did before it moved to Lisp, pinned to C's
;; results: `x-begin-drag's action and result mapping and its
;; conversion of the XdndSelection value (lisp/term/nhwl-dnd.el), the
;; decoding of the `nhwl-popup' parameter (lisp/term/nhwl-win.el) and the
;; numbers of the title-bar buttons (lisp/term/nhwl-settings.el).  The
;; drag itself and the popups need a shell and are covered by the GUI
;; rows (nhwl-gui-w2-tests.el).

;;; Code:

(require 'ert)
(require 'nhwl-dnd)
(require 'nhwl-win)
(require 'nhwl-settings)

;;; The action and the result

(ert-deftest nhwl-dnd-actions-map-like-c ()
  "The offered bits and the preferred one for every kind of ACTION."
  (should (equal (nhwl-dnd--actions nil) '(1 . 1)))
  (should (equal (nhwl-dnd--actions 'XdndActionCopy) '(1 . 1)))
  (should (equal (nhwl-dnd--actions 'XdndActionPrivate) '(1 . 1)))
  (should (equal (nhwl-dnd--actions 'XdndActionLink) '(1 . 1)))
  (should (equal (nhwl-dnd--actions 'XdndActionMove) '(3 . 2)))
  (should (equal (nhwl-dnd--actions 'XdndActionAsk) '(7 . 4)))
  (should (equal (nhwl-dnd--actions '((XdndActionCopy . "Copy"))) '(5 . 4)))
  (should (equal (nhwl-dnd--actions '((XdndActionMove . "Move"))) '(6 . 4)))
  (should (equal (nhwl-dnd--actions '((XdndActionMove . "Move")
                                      (XdndActionCopy . "Copy")))
                 '(7 . 4))))

(ert-deftest nhwl-dnd-actions-signal-like-c ()
  "An action that is neither a symbol nor a list of (SYMBOL . STRING)."
  (should (equal (should-error (nhwl-dnd--actions 42))
                 '(error "Invalid drag-and-drop action" 42)))
  (should (equal (should-error (nhwl-dnd--actions '(42)))
                 '(wrong-type-argument consp 42)))
  (should (equal (should-error (nhwl-dnd--actions '((1 . "x"))))
                 '(wrong-type-argument symbolp 1)))
  (should (equal (should-error (nhwl-dnd--actions '((XdndActionCopy . 2))))
                 '(wrong-type-argument stringp 2))))

(ert-deftest nhwl-dnd-result-maps-like-c ()
  "The action bit the target performed as x-begin-drag's value."
  (should (eq (nhwl-dnd--result 1) 'XdndActionCopy))
  (should (eq (nhwl-dnd--result 2) 'XdndActionMove))
  (should (eq (nhwl-dnd--result 4) 'XdndActionPrivate))
  (should (null (nhwl-dnd--result 0)))
  (should (null (nhwl-dnd--result 3))))

;;; The conversion

(ert-deftest nhwl-dnd-convert-follows-the-converter-alist ()
  "A converter makes a unibyte string, or the target does not convert."
  (let ((selection-converter-alist
         `((STRING . ,(lambda (sel target value)
                        (should (eq sel 'XdndSelection))
                        (should (eq target 'STRING))
                        value))
           (TYPED . ,(lambda (_s _t value) (cons 'STRING value)))
           (GUARDED . (always . ,(lambda (_s _t value) value)))
           (UTF8_STRING . ,(lambda (_s _t _v) "hé"))
           (FAILS . ,(lambda (_s _t _v) (error "no")))
           (NOT-A-STRING . ,(lambda (_s _t _v) 42)))))
    (should (equal (nhwl-dnd--convert "abc" 'STRING) "abc"))
    (should (equal (nhwl-dnd--convert "abc" 'TYPED) "abc"))
    (should (equal (nhwl-dnd--convert "abc" 'GUARDED) "abc"))
    (let ((bytes (nhwl-dnd--convert "abc" 'UTF8_STRING)))
      (should (equal bytes "h\303\251"))
      (should-not (multibyte-string-p bytes)))
    (should (null (nhwl-dnd--convert "abc" 'FAILS)))
    (should (null (nhwl-dnd--convert "abc" 'NOT-A-STRING)))
    (should (null (nhwl-dnd--convert "abc" 'UNKNOWN)))))

(ert-deftest nhwl-dnd-convert-takes-the-text-property ()
  "A string value's text property named TARGET stands for it."
  (let ((selection-converter-alist
         `((STRING . ,(lambda (_s _t value) value)))))
    (should (equal (nhwl-dnd--convert (propertize "abc" 'STRING "from-prop")
                                      'STRING)
                   "from-prop"))))

(ert-deftest nhwl-dnd-items-keep-the-order-and-add-utf8 ()
  "Targets that convert, in order and once; utf-8 text follows a text target."
  (let ((selection-converter-alist
         `((STRING . ,(lambda (_s _t value) value))
           (UTF8_STRING . ,(lambda (_s _t value) value))
           (text/plain\;charset=utf-8 . ,(lambda (_s _t value) value))
           (text/uri-list . ,(lambda (_s _t _v) "file:///x")))))
    (should (equal (nhwl-dnd--items "abc" '("UTF8_STRING" "NOPE" "STRING"
                                            "UTF8_STRING"))
                   '(("UTF8_STRING" . "abc") ("STRING" . "abc")
                     ("text/plain;charset=utf-8" . "abc"))))
    (should (equal (nhwl-dnd--items "abc" '("text/plain;charset=utf-8" "STRING"))
                   '(("text/plain;charset=utf-8" . "abc") ("STRING" . "abc"))))
    (should (equal (nhwl-dnd--items "abc" '("text/uri-list"))
                   '(("text/uri-list" . "file:///x"))))
    (should (null (nhwl-dnd--items "abc" '("NOPE"))))))

;;; The popup parameter

(ert-deftest nhwl-popup-decode-defaults ()
  "Bottom-right gravity, every adjustment, no grab."
  (should (equal (nhwl-popup-decode '(10 20 30 40)) '(10 20 30 40 8 15 0))))

(ert-deftest nhwl-popup-decode-names ()
  "Every gravity and adjust name, the grab, and a negative size."
  (let ((names '(top bottom left right top-left bottom-left top-right
                     bottom-right))
        (n 0))
    (dolist (g names)
      (setq n (1+ n))
      (should (equal (nth 4 (nhwl-popup-decode (list 0 0 1 1 :gravity g))) n))))
  (should (equal (nhwl-popup-decode '(1 2 -3 -4 :gravity top :adjust (flip-x)
                                        :grab t))
                 '(1 2 0 0 1 4 1)))
  (should (equal (nth 5 (nhwl-popup-decode '(0 0 1 1 :adjust (slide-x slide-y))))
                 3))
  (should (equal (nth 5 (nhwl-popup-decode '(0 0 1 1 :adjust (flip-y flip-x))))
                 12))
  (should (equal (nth 5 (nhwl-popup-decode '(0 0 1 1 :adjust nil))) 0))
  (should (equal (nth 4 (nhwl-popup-decode '(0 0 1 1 :gravity nil))) 8)))

(ert-deftest nhwl-popup-decode-invalid ()
  "An invalid value signals `error' naming the value."
  (dolist (v '(nil (1 2 3) (1 2 3 x) (1 2 3 4 :gravity middle)
               (1 2 3 4 :gravity "top") (1 2 3 4 :adjust (bogus))
               (1 2 3 4 :adjust (flip-x 1)) (1073741824 0 1 1)
               (0 -1073741824 1 1)))
    (should (equal (should-error (nhwl-popup-decode v))
                   (list 'error "Invalid `nhwl-popup' parameter" v)))))

;;; The title-bar buttons

(ert-deftest nhwl-settings-button-kinds-number-the-buttons ()
  "The shell's numbers, in order; other elements are left out."
  (should (equal (nhwl-settings--button-kinds
                  '(appmenu close maximize minimize))
                 '(4 1 2 3)))
  (should (equal (nhwl-settings--button-kinds '(close icon nil "close" menu))
                 '(1)))
  (should (null (nhwl-settings--button-kinds nil))))

(provide 'nhwl-dnd-tests)

;;; nhwl-dnd-tests.el ends here
