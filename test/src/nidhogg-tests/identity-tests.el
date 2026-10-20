;;; identity-tests.el --- tests for Nidhogg's identity  -*- lexical-binding: t -*-

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

;;; Code:

(require 'ert)

(ert-deftest nidhogg-version-is-set ()
  (should (stringp nidhogg-version))
  (should (string-match-p "\\`[0-9]+\\.[0-9]+-nidhogg\\.[0-9]+\\'" nidhogg-version))
  (should (string-prefix-p (format "%d.%d-" emacs-major-version emacs-minor-version) nidhogg-version)))

(ert-deftest nidhogg-version-matches-the-file ()
  (let ((f (expand-file-name "nidhogg/version" source-directory)))
    (skip-unless (file-readable-p f))
    (should (equal nidhogg-version
                   (string-trim (with-temp-buffer (insert-file-contents f) (buffer-string)))))))

(ert-deftest nidhogg-feature ()
  (should (featurep 'nidhogg)))

(ert-deftest nidhogg-in-the-configuration ()
  (should (string-search (concat "--with-nidhogg=" nidhogg-version) system-configuration-options))
  (should (member "NIDHOGG" (split-string system-configuration-features))))

(ert-deftest nidhogg-emacs-version-names-it ()
  (should (string-suffix-p (format " (Nidhogg %s)" nidhogg-version) (emacs-version))))

(ert-deftest nidhogg-emacs-version-stays-gnus ()
  (should-not (string-match-p "nidhogg" emacs-version))
  (should (version-to-list emacs-version)))

;;; identity-tests.el ends here
