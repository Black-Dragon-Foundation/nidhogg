;;; xdisp-tests.el --- tests for xdisp.c functions -*- lexical-binding: t -*-

;; Copyright (C) 2020-2026 Free Software Foundation, Inc.

;; This file is part of GNU Emacs.

;; GNU Emacs is free software: you can redistribute it and/or modify
;; it under the terms of the GNU General Public License as published by
;; the Free Software Foundation, either version 3 of the License, or
;; (at your option) any later version.

;; GNU Emacs is distributed in the hope that it will be useful,
;; but WITHOUT ANY WARRANTY; without even the implied warranty of
;; MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
;; GNU General Public License for more details.

;; You should have received a copy of the GNU General Public License
;; along with GNU Emacs.  If not, see <https://www.gnu.org/licenses/>.

;;; Code:

(require 'ert)
(require 'ert-x)

(defmacro xdisp-tests--in-minibuffer (&rest body)
  (declare (debug t) (indent 0))
  `(catch 'result
     (minibuffer-with-setup-hook
         (lambda ()
           (let ((redisplay-skip-initial-frame nil)
                 (executing-kbd-macro nil)) ;Don't skip redisplay
             (throw 'result (progn . ,body))))
       (let ((executing-kbd-macro t)) ;Force real minibuffer in `read-string'.
         (read-string "toto: ")))))

(ert-deftest xdisp-tests--minibuffer-resizing () ;; bug#43519
  (should
   (equal
    t
    (xdisp-tests--in-minibuffer
      (insert "hello")
      (let ((ol (make-overlay (point) (point)))
            (max-mini-window-height 1)
            (text (copy-sequence "askdjfhaklsjdfhlkasjdfhklasdhflkasdhflkajsdhflkashdfkljahsdlfkjahsdlfkjhasldkfhalskdjfhalskdfhlaksdhfklasdhflkasdhflkasdhflkajsdhklajsdgh")))
        ;; (save-excursion (insert text))
        ;; (sit-for 2)
        ;; (delete-region (point) (point-max))
        (put-text-property 0 1 'cursor t text)
        (overlay-put ol 'after-string text)
        (redisplay 'force)
        ;; Make sure we do the see "hello" text.
        (prog1 (equal (window-start) (point-min))
          ;; (list (window-start) (window-end) (window-width))
          (delete-overlay ol)))))))

(ert-deftest xdisp-tests--minibuffer-scroll () ;; bug#44070
  (let ((posns
         (xdisp-tests--in-minibuffer
           (let ((max-mini-window-height 4))
             (dotimes (_ 80) (insert "\nhello"))
             (goto-char (point-min))
             (redisplay 'force)
             (goto-char (point-max))
             ;; A simple edit like removing the last `o' shouldn't cause
             ;; the rest of the minibuffer's text to move.
             (list
              (progn (redisplay 'force) (window-start))
              (progn (delete-char -1)
                     (redisplay 'force) (window-start))
              (progn (goto-char (point-min)) (redisplay 'force)
                     (goto-char (point-max)) (redisplay 'force)
                     (window-start)))))))
    (should (equal (nth 0 posns) (nth 1 posns)))
    (should (equal (nth 1 posns) (nth 2 posns)))))

(ert-deftest xdisp-tests--window-text-pixel-size () ;; bug#45748
  (with-temp-buffer
    (insert "xxx")
    (switch-to-buffer (current-buffer))
    (let* ((char-width (frame-char-width))
           (size (window-text-pixel-size nil t t))
           (width-in-chars (/ (car size) char-width)))
      (should (equal width-in-chars 3)))))

(ert-deftest xdisp-tests--window-text-pixel-size-leading-space () ;; bug#45748
  (with-temp-buffer
    (insert " xx")
    (switch-to-buffer (current-buffer))
    (let* ((char-width (frame-char-width))
           (size (window-text-pixel-size nil t t))
           (width-in-chars (/ (car size) char-width)))
      (should (equal width-in-chars 3)))))

(ert-deftest xdisp-tests--window-text-pixel-size-trailing-space () ;; bug#45748
  (with-temp-buffer
    (insert "xx ")
    (switch-to-buffer (current-buffer))
    (let* ((char-width (frame-char-width))
           (size (window-text-pixel-size nil t t))
           (width-in-chars (/ (car size) char-width)))
      (should (equal width-in-chars 3)))))

(ert-deftest xdisp-tests--find-directional-overrides-case-1 ()
  (with-temp-buffer
    (insert "\
int main() {
  bool isAdmin = false;
  /*‮ }⁦if (isAdmin)⁩ ⁦ begin admins only */
  printf(\"You are an admin.\\n\");
  /* end admins only ‮ { ⁦*/
  return 0;
}")
    (goto-char (point-min))
    (should (eq (bidi-find-overridden-directionality (point-min) (point-max)
                                                     nil)
                46))))

(ert-deftest xdisp-tests--find-directional-overrides-case-2 ()
  (with-temp-buffer
    (insert "\
#define is_restricted_user(user)			\\
  !strcmp (user, \"root\") ? 0 :			\\
  !strcmp (user, \"admin\") ? 0 :			\\
  !strcmp (user, \"superuser‮⁦? 0 : 1⁩ ⁦\")⁩‬

int main () {
  printf (\"root: %d\\n\", is_restricted_user (\"root\"));
  printf (\"admin: %d\\n\", is_restricted_user (\"admin\"));
  printf (\"superuser: %d\\n\", is_restricted_user (\"superuser\"));
  printf (\"luser: %d\\n\", is_restricted_user (\"luser\"));
  printf (\"nobody: %d\\n\", is_restricted_user (\"nobody\"));
}")
    (goto-char (point-min))
    (should (eq (bidi-find-overridden-directionality (point-min) (point-max)
                                                     nil)
                138))))

(ert-deftest xdisp-tests--find-directional-overrides-case-3 ()
  (with-temp-buffer
    (insert "\
#define is_restricted_user(user)			\\
  !strcmp (user, \"root\") ? 0 :			\\
  !strcmp (user, \"admin\") ? 0 :			\\
  !strcmp (user, \"superuser‮⁦? '#' : '!'⁩ ⁦\")⁩‬

int main () {
  printf (\"root: %d\\n\", is_restricted_user (\"root\"));
  printf (\"admin: %d\\n\", is_restricted_user (\"admin\"));
  printf (\"superuser: %d\\n\", is_restricted_user (\"superuser\"));
  printf (\"luser: %d\\n\", is_restricted_user (\"luser\"));
  printf (\"nobody: %d\\n\", is_restricted_user (\"nobody\"));
}")
    (goto-char (point-min))
    (should (eq (bidi-find-overridden-directionality (point-min) (point-max)
                                                     nil)
                138))))

(ert-deftest test-get-display-property ()
  (with-temp-buffer
    (insert (propertize "foo" 'face 'bold 'display '(height 2.0)))
    (should (equal (get-display-property 2 'height) 2.0)))
  (with-temp-buffer
    (insert (propertize "foo" 'face 'bold 'display '((height 2.0)
                                                     (space-width 2.0))))
    (should (equal (get-display-property 2 'height) 2.0))
    (should (equal (get-display-property 2 'space-width) 2.0)))
  (with-temp-buffer
    (insert (propertize "foo bar" 'face 'bold
                        'display '[(height 2.0)
                                   (space-width 20)]))
    (should (equal (get-display-property 2 'height) 2.0))
    (should (equal (get-display-property 2 'space-width) 20))))

(ert-deftest test-messages-buffer-name ()
  (should
   (equal
    (let ((messages-buffer-name "test-message"))
      (message "foo")
      (with-current-buffer messages-buffer-name
        (buffer-string)))
    "foo\n")))

(ert-deftest xdisp-test-format-mode-line ()
  ;; 'format-mode-line' returns an empty string with no properties in
  ;; noninteractive sessions.
  (skip-when noninteractive)
  (with-temp-buffer
    (insert (format-mode-line " " t))
    (should (equal (buffer-string) #(" " 0 1 (face mode-line-active)))))
  (with-temp-buffer
    (insert (format-mode-line
             (propertize "x" 'face 'bold-italic)
             1200000000000000000000000000))
    (should (null (get-text-property 1 'face))))
  (should
   (equal
    (text-properties-at
     0
     (format-mode-line '((:propertize "Hello!" face bold)) 'mode-line))
    (list 'face '(bold mode-line) 'mode-line-elt-no 3)))
  (with-temp-buffer
    ;; This test is due to Markus Triska <triska@metalevel.at>.
    (let ((m1 (format-mode-line mode-line-format nil))
	  (m2 (format-mode-line mode-line-format 'default))
	  s1 s2)
      (font-lock-mode 0)
      (insert "\n")
      (insert m1)
      (setq s1 (buffer-text-pixel-size))
      (erase-buffer)
      (insert "\n")
      (insert m2)
      (setq s2 (buffer-text-pixel-size))
      (should (equal m1 m2))
      (should (equal s1 s2)))))

(defun xdisp-tests--clip-child (out)
  "The form the child Emacs of `xdisp-tests--clip-restored-after-insert' runs.
It writes to OUT how many lines three redisplays laid out: after
`force-window-update', after an insertion, and after an insertion
followed by a narrowing undone."
  `(progn
     (defvar xdisp-tests--n 0)
     (defun xdisp-tests--count () (setq xdisp-tests--n (1+ xdisp-tests--n)) nil)
     (defun xdisp-tests--laid-out (f)
       ;; Each line's display property counts when the display
       ;; engine reaches it.
       (redisplay t)
       (setq xdisp-tests--n 0)
       (funcall f)
       (redisplay t)
       xdisp-tests--n)
     (run-at-time
      0 nil
      (lambda ()
        (switch-to-buffer "t")
        ;; Reordering looks ahead for display strings and would
        ;; count lines that are not laid out.
        (setq bidi-display-reordering nil)
        (dotimes (i 40)
          (insert (propertize "l" 'display '(when (xdisp-tests--count) . "L"))
                  (format "ine %d\n" i)))
        (goto-char (point-min))
        (forward-line 10)
        (end-of-line)
        (let ((counts
               (list
                (xdisp-tests--laid-out #'force-window-update)
                (xdisp-tests--laid-out (lambda () (insert "x")))
                ;; What `blink-matching-open' does after `)'.
                (xdisp-tests--laid-out
                 (lambda ()
                   (insert ")")
                   (save-restriction
                     (narrow-to-region (line-beginning-position) (point))))))))
          (with-temp-file ,out (prin1 counts (current-buffer))))
        (kill-emacs 0)))))

(ert-deftest xdisp-tests--clip-restored-after-insert ()
  "A narrowing undone in the command that inserted text redisplays one line.
The clip-changed mark it leaves is dropped when the restriction still
ends as far from Z as at the last redisplay; it used to be dropped only
for an unchanged ZV, which the insertion moved, so the whole window was
redisplayed."
  ;; The window-based optimizations need a terminal that can insert
  ;; and delete lines: a child Emacs on a pty.
  (ert-with-temp-directory dir
    (let* ((out (expand-file-name "out" dir))
           (process-environment
            (append (list "TERM=screen" (concat "HOME=" dir))
                    process-environment))
           (buf (generate-new-buffer " *xdisp-child*"))
           (proc (make-process
                  :name "xdisp-child" :buffer buf :connection-type 'pty
                  :command (list (expand-file-name invocation-name
                                                   invocation-directory)
                                 "-nw" "-Q" "--eval"
                                 (prin1-to-string
                                  (xdisp-tests--clip-child out)))))
           (deadline (+ (float-time) 60)))
      (unwind-protect
          (progn
            (while (and (process-live-p proc) (< (float-time) deadline))
              (accept-process-output proc 0.1))
            (unless (file-exists-p out)
              (ert-fail (list "no counts from the child"
                              (with-current-buffer buf (buffer-string)))))
            (pcase-let ((`(,whole ,insert ,narrowed)
                         (with-temp-buffer
                           (insert-file-contents out)
                           (read (current-buffer)))))
              (message "lines laid out: whole %d, insert %d, narrowed %d"
                       whole insert narrowed)
              (ert-info ((format "lines laid out: whole %d, insert %d, narrowed %d"
                                 whole insert narrowed))
                (should (>= whole 20))
                (should (< (* 2 insert) whole))
                (should (< (* 2 narrowed) whole)))))
        (delete-process proc)
        (kill-buffer buf)))))

;;; xdisp-tests.el ends here
