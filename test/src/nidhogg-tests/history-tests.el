;;; history-tests.el --- The edit history's change hook  -*- lexical-binding: t -*-
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

;; `--with-nidhogg-history' records every change to a tracked buffer's
;; text at the ten places the text changes (`src/nidhogg-history.c'):
;; `nidhogg-history-drain' answers them as (START OLD-LEN INSERTED TIME
;; ORIGIN), oldest first, or `resync'.  One row per site: each replays
;; the drained changes onto the text the buffer had when it was tracked
;; and asserts the result is the buffer's text, so a change the hook
;; missed, or recorded wrong, fails the row.
;;
;; Pure (pattern D): no time is waited for, no threads, no processes.
;; The rows skip on a build configured without the flag.

;;; Code:

(require 'ert)

(defvar nidhogg-history-origin)

(defun history-tests--built-p ()
  "Whether this Emacs was configured with the edit history's hook."
  (string-search "--with-nidhogg-history" system-configuration-options))

(defun history-tests--replay (old changes multibyte)
  "OLD with CHANGES applied in order, in a buffer that is MULTIBYTE or not."
  (with-temp-buffer
    (set-buffer-multibyte multibyte)
    (insert old)
    (dolist (c changes)
      (let ((start (nth 0 c)) (old-len (nth 1 c)) (inserted (nth 2 c)))
        (should (stringp inserted))
        (should (<= 1 start (+ start old-len) (point-max)))
        (goto-char start)
        (delete-region start (+ start old-len))
        (insert inserted)))
    (buffer-substring-no-properties (point-min) (point-max))))

(defun history-tests--whole (buffer)
  "BUFFER's whole text, without properties and ignoring narrowing."
  (with-current-buffer buffer
    (save-restriction
      (widen)
      (buffer-substring-no-properties (point-min) (point-max)))))

(defun history-tests--check (buffer old)
  "Drain BUFFER and assert its changes take OLD to its text; the changes."
  (let ((changes (nidhogg-history-drain buffer)))
    (should (listp changes))
    (dolist (c changes)
      (should (= (length c) 5))
      (should (floatp (nth 3 c))))
    (should (equal (history-tests--replay
                    old changes
                    (buffer-local-value 'enable-multibyte-characters buffer))
                   (history-tests--whole buffer)))
    changes))

(defmacro history-tests--tracked (spec &rest body)
  "Run BODY with (VAR TEXT) of SPEC a fresh buffer holding TEXT, tracked.
After BODY the buffer is drained and checked against TEXT; the value is
the changes.  The buffer is untracked and killed after."
  (declare (indent 1) (debug ((symbolp form) body)))
  (let ((var (car spec)) (old (make-symbol "old")))
    `(let ((,var (generate-new-buffer " *history-tests*"))
           (,old ,(cadr spec)))
       (unwind-protect
           (progn
             (with-current-buffer ,var (insert ,old))
             (should (nidhogg-history-track ,var t))
             (with-current-buffer ,var ,@body)
             (history-tests--check ,var ,old))
         (when (buffer-live-p ,var)
           (nidhogg-history-track ,var nil)
           (kill-buffer ,var))))))

(ert-deftest history/insert-records ()
  "`insert', deletion, `erase-buffer' and narrowing; the origin is recorded."
  (skip-unless (history-tests--built-p))
  (let ((changes
         (history-tests--tracked (b "héllo wörld\n")
           (goto-char 3)
           (insert "XY")
           (insert-before-markers "🐉")
           (insert-char ?é 3)
           (delete-region 1 2)
           (let ((nidhogg-history-origin 'agent))
             (goto-char (point-max))
             (insert "agent line\n"))
           (narrow-to-region 4 8)
           (goto-char (point-min))
           (insert "[n]")
           (delete-char 1)
           (widen)
           (let ((nidhogg-history-origin "external:git"))
             (erase-buffer)
             (insert "after\n")))))
    (should (>= (length changes) 9))
    (should-not (nth 4 (car changes)))
    (should (equal (delq nil (mapcar (lambda (c)
                                       (and (equal (nth 2 c) "agent line\n")
                                            (list (nth 4 c))))
                                     changes))
                   '((agent))))
    (should (equal (mapcar (lambda (c) (nth 4 c)) (last changes 2))
                   '("external:git" "external:git")))))

(ert-deftest history/yank-records ()
  "`yank' and `insert-buffer-substring', text properties dropped."
  (skip-unless (history-tests--built-p))
  (let ((src (generate-new-buffer " *history-src*")))
    (unwind-protect
        (progn
          (with-current-buffer src
            (insert (propertize "from another buffer" 'face 'bold)))
          (history-tests--tracked (b "one\ntwo\n")
            (let ((kill-ring nil) (kill-ring-yank-pointer nil)
                  (interprogram-paste-function nil))
              (kill-new (propertize "yanked ✓" 'face 'italic))
              (goto-char 5)
              (yank))
            (goto-char (point-max))
            (insert-buffer-substring src 6 13)))
      (kill-buffer src))))

(ert-deftest history/insert-file-contents-records ()
  "`insert-file-contents' into the middle, decoded, and with REPLACE."
  (skip-unless (history-tests--built-p))
  (let ((file (make-temp-file "history-tests" nil ".txt")))
    (unwind-protect
        (progn
          (let ((coding-system-for-write 'utf-8-unix))
            (write-region "file line ü\nsecond line\n" nil file nil 'silent))
          (history-tests--tracked (b "before\nafter\n")
            (goto-char 8)
            (let ((coding-system-for-read 'utf-8-unix))
              (insert-file-contents file)))
          (history-tests--tracked (b "file line u\nsecond line changed\n")
            (let ((coding-system-for-read 'utf-8-unix))
              (insert-file-contents file nil nil nil t))))
      (delete-file file))))

(ert-deftest history/replace-buffer-contents-records ()
  "`replace-buffer-contents' (its diff hunks) and `replace-region-contents'."
  (skip-unless (history-tests--built-p))
  (let ((src (generate-new-buffer " *history-src*")))
    (unwind-protect
        (progn
          (with-current-buffer src
            (insert "alpha\nBETA\ngamma\ndelta é\nepsilon\n"))
          (history-tests--tracked (b "alpha\nbeta\ngamma\ndelta\n")
            (replace-buffer-contents src))
          (history-tests--tracked (b "keep (old) keep\n")
            (replace-region-contents 7 10 (lambda () "nouveau"))))
      (kill-buffer src))))

(ert-deftest history/casify-records ()
  "Case changes in place, one that changes the length, and a unibyte buffer."
  (skip-unless (history-tests--built-p))
  (history-tests--tracked (b "straße hello world ǆ\n")
    (upcase-region 1 7)
    (capitalize-region (point-min) (point-max))
    (downcase-region 1 4))
  (let ((b (generate-new-buffer " *history-unibyte*")))
    (unwind-protect
        (with-current-buffer b
          (set-buffer-multibyte nil)
          (insert "raw \377 bytes")
          (should (nidhogg-history-track b t))
          (upcase-region 1 5)
          (history-tests--check b "raw \377 bytes"))
      (nidhogg-history-track b nil)
      (kill-buffer b))))

(ert-deftest history/subst-char-records ()
  "`subst-char-in-region', with and without NOUNDO."
  (skip-unless (history-tests--built-p))
  (history-tests--tracked (b "a-b-c-d é-ü\n")
    (subst-char-in-region 1 6 ?- ?_)
    (subst-char-in-region (point-min) (point-max) ?é ?è t)))

(ert-deftest history/translate-region-records ()
  "`translate-region': a same-length change in place, and one that grows."
  (skip-unless (history-tests--built-p))
  (let ((table (make-char-table 'translation-table)))
    (aset table ?b ?c)
    (aset table ?a ?é)
    (history-tests--tracked (b "abba cab\n")
      (translate-region (point-min) (point-max) table))))

(ert-deftest history/transpose-regions-records ()
  "`transpose-regions' of equal and unequal lengths, multibyte."
  (skip-unless (history-tests--built-p))
  (history-tests--tracked (b "one two three four\n")
    (transpose-regions 1 4 5 8))
  (history-tests--tracked (b "αβ and a longer γδε part\n")
    (transpose-regions 1 3 8 16)))

(ert-deftest history/inhibit-modification-hooks-still-records ()
  "Changes made with `inhibit-modification-hooks' are recorded too."
  (skip-unless (history-tests--built-p))
  (let ((changes
         (history-tests--tracked (b "quiet\n")
           (let ((inhibit-modification-hooks t))
             (goto-char 1)
             (insert "shh ")
             (delete-region 5 7)))))
    (should (= (length changes) 2))))

(ert-deftest history/swap-text-forces-resync ()
  "`buffer-swap-text' and `set-buffer-multibyte' answer `resync', then record again."
  (skip-unless (history-tests--built-p))
  (let ((a (generate-new-buffer " *history-a*"))
        (b (generate-new-buffer " *history-b*")))
    (unwind-protect
        (progn
          (with-current-buffer a (insert "text of a"))
          (with-current-buffer b (insert "text of b, longer"))
          (should (nidhogg-history-track a t))
          (with-current-buffer a (insert "!"))
          (with-current-buffer a (buffer-swap-text b))
          (should (nidhogg-history-pending-p))
          (should (eq (nidhogg-history-drain a) 'resync))
          (should-not (nidhogg-history-pending-p))
          (let ((now (history-tests--whole a)))
            (should (equal now "text of b, longer"))
            (with-current-buffer a (goto-char 1) (insert ">"))
            (history-tests--check a now))
          (with-current-buffer a (set-buffer-multibyte nil))
          (should (eq (nidhogg-history-drain a) 'resync))
          (let ((now (history-tests--whole a)))
            (with-current-buffer a (goto-char (point-max)) (insert "\377"))
            (history-tests--check a now)))
      (nidhogg-history-track a nil)
      (kill-buffer a)
      (kill-buffer b))))

(ert-deftest history/untracked-buffer-costs-nothing ()
  "With nothing tracked the hook is never entered; an untracked buffer reaches no ring."
  (skip-unless (history-tests--built-p))
  (let ((b (generate-new-buffer " *history-untracked*"))
        (other (generate-new-buffer " *history-other*")))
    (unwind-protect
        (let ((s0 (nidhogg-history-stats)))
          (should (= (plist-get s0 :tracked) 0))
          (with-current-buffer b (insert "x") (delete-char -1))
          (let ((s1 (nidhogg-history-stats)))
            (should (= (plist-get s1 :lookups) (plist-get s0 :lookups)))
            (should (= (plist-get s1 :changes) (plist-get s0 :changes))))
          (should (nidhogg-history-track other t))
          (with-current-buffer b (insert "y"))
          (let ((s2 (nidhogg-history-stats)))
            (should (= (plist-get s2 :tracked) 1))
            (should (= (plist-get s2 :changes) (plist-get s0 :changes))))
          (should-not (nidhogg-history-pending-p))
          (should-not (nidhogg-history-drain b))
          ;; a killed buffer's text is forgotten
          (kill-buffer other)
          (should (= (plist-get (nidhogg-history-stats) :tracked) 0)))
      (kill-buffer b)
      (when (buffer-live-p other) (kill-buffer other)))))

(ert-deftest history/indirect-buffer-records-once ()
  "An indirect buffer shares its base's text: one change, one record."
  (skip-unless (history-tests--built-p))
  (let ((base (generate-new-buffer " *history-base*"))
        ind)
    (unwind-protect
        (progn
          (with-current-buffer base (insert "shared text\n"))
          (setq ind (make-indirect-buffer base " *history-indirect*"))
          (should (nidhogg-history-track base t))
          (should (nidhogg-history-track ind t))
          (should (= (plist-get (nidhogg-history-stats) :tracked) 1))
          (with-current-buffer ind (goto-char 1) (insert "via indirect: "))
          (let ((changes (nidhogg-history-drain base)))
            (should (= (length changes) 1))
            (should (equal (history-tests--replay "shared text\n" changes t)
                           (history-tests--whole base))))
          (should-not (nidhogg-history-drain ind)))
      (when ind (kill-buffer ind))
      (nidhogg-history-track base nil)
      (kill-buffer base))))

(ert-deftest history/huge-insert-one-change ()
  "An 8 MB insert is one change, not a resync."
  (skip-unless (history-tests--built-p))
  (let* ((big (make-string (* 8 1024 1024) ?z))
         (changes (history-tests--tracked (b "head\n")
                    (goto-char 3)
                    (insert big))))
    (should (= (length changes) 1))
    (should (= (length (nth 2 (car changes))) (length big)))))

;;; history-tests.el ends here
