;;; wdfix-core.el --- Fixture for the stall watchdog's attribution  -*- lexical-binding: t -*-

;; Copyright (C) 2026 Black Dragon Foundation contributors

;; Version: 1.0
;; This file is part of Nidhogg (GPL-3.0-or-later).

;;; Commentary:

;; Functions that take a synchronous watchdog sample from inside, so the
;; sample's frames name this package.

;;; Code:

(declare-function nidhogg-watchdog--test-sample "nidhogg-watchdog.c" ())

(defun wdfix-work ()
  "Return a sample taken from inside this function."
  (nidhogg-watchdog--test-sample))

(defun wdfix-make-lambda ()
  "Return an anonymous function that takes a sample."
  (lambda () (nidhogg-watchdog--test-sample)))

(defun wdfix-hot-loop ()
  "Search a buffer, so a sample on the `regex' phase is taken inside."
  (with-temp-buffer
    (insert (make-string 2000 ?a))
    (goto-char (point-min))
    (re-search-forward "zzz" nil t)))

(provide 'wdfix-core)
;;; wdfix-core.el ends here
