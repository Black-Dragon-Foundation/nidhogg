;;; nhmac-win-tests.el --- tests for Naglfar's Lisp half  -*- lexical-binding: t -*-

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

;; Batch tests of lisp/term/nhmac-win.el.  Tasks append under ";; Task N:".

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'menu-bar)
(require 'nhmac-win)

(defconst nhmac-win-test--dir
  (file-name-directory (or load-file-name buffer-file-name)))

;; Task 6: the menu flattener

(defun nhmac-win-test--rows (vec)
  "VEC, a flattened menu, as a list of (PATH KIND KEY ENABLED STATE).
KIND is coarse (NS says nothing of toggles and radios): 0 item, 1 submenu,
2 separator.  PATH is the labels from the title down, joined with \"/\"."
  (let ((w nhmac-menu-width) (stack nil) (rows nil))
    (dotimes (i (/ (length vec) w))
      (let* ((o (* i w)) (depth (aref vec o)) (kind (aref vec (+ o 1))))
        (setq stack (append (seq-take (reverse stack) depth) nil)
              stack (reverse stack))
        (push (aref vec (+ o 2)) stack)
        (push (list (mapconcat #'identity (reverse stack) "/")
                    (pcase kind (1 1) (2 2) (_ 0))
                    (substring-no-properties (aref vec (+ o 3)))
                    (aref vec (+ o 4)) (aref vec (+ o 5)))
              rows)))
    (nreverse rows)))

(defun nhmac-win-test--golden ()
  "The NS fixture as rows like `nhmac-win-test--rows'."
  (let ((stack nil))
    (mapcar (lambda (e)
              (setq stack (reverse (seq-take (reverse stack) (nth 0 e))))
              (push (nth 2 e) stack)
              (list (mapconcat #'identity (reverse stack) "/")
                    (nth 1 e) (nth 3 e) (nth 4 e) (nth 5 e)))
            (with-temp-buffer
              (insert-file-contents
               (expand-file-name "nhmac-win-resources/menu-bar-golden.eld"
                                 nhmac-win-test--dir))
              (read (current-buffer))))))

;; What a batch Emacs cannot give the way the NS session did.  The first two
;; lists are dropped from both sides, the others only have the named field
;; ignored.  Everything else in the fixture's 831 rows must match.
(defconst nhmac-win-test--drop
  '("Options/Multilingual Environment/Set Coding Systems/For X Selections/Clipboard"
    "Options/Multilingual Environment/Set Coding Systems/For Next X Selection"
    "Options/Show/Hide/Tool Bar"
    "Options/Use System Font")
  "Rows (and their descendants) that differ in kind between NS and this build, or exist in one only.
The X selection rows need an X display, the tool bar is a submenu of positions on
a toolkit that has them, and `Use System Font' exists when the build has
the `system-font-setting' feature (a GTK one), which NS does not.")

(defconst nhmac-win-test--drop-children
  '("Buffers")
  "Rows whose items are session state (the buffer list).")

(defconst nhmac-win-test--ignore
  '(("Edit/Paste" . enabled)
    ("Options/Multilingual Environment/Set Coding Systems/For Terminal" . enabled)
    ("Options/Show/Hide/Scroll Bar/On the Right" . state)
    ("Options/Customize Emacs/Top-level Emacs Customization Group" . key))
  "Rows whose enable, state or key hint depends on the session (selections, frame, the NS key tables).")

(defun nhmac-win-test--normalize (rows)
  "ROWS without the gaps above."
  (let ((out nil))
    (dolist (r rows)
      (let ((path (car r)))
        (unless (or (cl-some (lambda (d) (or (equal path d) (string-prefix-p (concat d "/") path)))
                             nhmac-win-test--drop)
                    (cl-some (lambda (d) (string-prefix-p (concat d "/") path))
                             nhmac-win-test--drop-children))
          (let ((ign (cdr (assoc path nhmac-win-test--ignore))))
            (push (list path (nth 1 r)
                        (if (eq ign 'key) :ignored (nth 2 r))
                        (if (eq ign 'enabled) :ignored (nth 3 r))
                        (if (eq ign 'state) :ignored (nth 4 r)))
                  out)))))
    (nreverse out)))

(defun nhmac-win-test--ns-edit-menu ()
  "`menu-bar-edit-menu' as it is built when `ns' is a feature."
  (let ((saved features) form)
    (with-temp-buffer
      (insert-file-contents (locate-library "menu-bar.el"))
      (goto-char (point-min))
      (re-search-forward "^(defvar menu-bar-edit-menu\n  (let")
      (goto-char (match-beginning 0))
      (setq form (read (current-buffer))))
    (unwind-protect
        (progn (setq features (cons 'ns features))
               (eval (nth 2 form) nil))
      (setq features saved))))

(defun nhmac-win-test--bind-ns-keys (map)
  "Install in MAP the global bindings `ns-win.el' makes."
  (with-temp-buffer
    (insert-file-contents (locate-library "term/ns-win.el"))
    (goto-char (point-min))
    (condition-case nil
        (while t
          (let ((f (read (current-buffer))))
            (when (and (eq (car-safe f) 'define-key) (eq (cadr f) 'global-map))
              (ignore-errors (eval `(define-key ',map ,@(cddr f)) t)))))
      (end-of-file nil))))

(defun nhmac-win-test--call-as-ns (fn)
  "Call FN in a *scratch* that looks like the NS session's, and return its value.
What NS had at startup is rebuilt: `ns-win.el's key bindings, the Edit menu
with `ns' a feature, a graphic display, transient-mark and blink-cursor on, a
buffer with undo history."
  (let* ((old (current-global-map))
         (new (copy-keymap old))
         (saved-features features)
         (saved-tmm (default-value 'transient-mark-mode))
         (saved-blink (default-value 'blink-cursor-mode)))
    (unwind-protect
        (progn
          (use-global-map new)
          (nhmac-win-test--bind-ns-keys new)
          (let ((bar (copy-keymap (lookup-key new [menu-bar]))))
            (define-key bar [edit] (cons "Edit" (nhmac-win-test--ns-edit-menu)))
            (define-key new [menu-bar] bar))
          (setq-default transient-mark-mode t blink-cursor-mode t)
          (setq features (cons 'ns features))
          (with-current-buffer (get-buffer-create "*scratch*")
            (buffer-enable-undo)
            (insert "a")
            (cl-letf (((symbol-function 'display-graphic-p) (lambda (&rest _) t))
                      ((symbol-function 'window-system) (lambda (&rest _) 'ns)))
              (funcall fn))))
      (setq features saved-features)
      (setq-default transient-mark-mode saved-tmm blink-cursor-mode saved-blink)
      (use-global-map old))))

(ert-deftest nhmac-win-menu-bar-matches-ns ()
  "The flattener on `emacs -Q's menu bar gives what NS built, field by field.
The NS session is rebuilt first (`nhmac-win-test--call-as-ns'); the rows
batch cannot reproduce are listed in `nhmac-win-test--ignore' and the lists beside it."
  (let* ((got (nhmac-win-test--normalize
               (nhmac-win-test--rows
                (nhmac-win-test--call-as-ns #'nhmac-menu-flatten-bar))))
         (want (nhmac-win-test--normalize (nhmac-win-test--golden))))
    (should (> (length want) 700))
    (should (= (length got) (length want)))
    (cl-mapc (lambda (g w) (should (equal g w))) got want)))

(defun nhmac-win-test--bar (&rest items)
  "A menu-bar keymap of ITEMS, each (KEY NAME . SUBMENU-ITEMS) or (KEY NAME COMMAND).
A submenu item is a binding as in a keymap."
  (let ((bar (make-sparse-keymap)))
    (dolist (item (reverse items))
      (let ((rest (cddr item)))
        (define-key bar (vector (car item))
          (if (and (null (cdr rest)) (symbolp (car rest)))
              (list 'menu-item (cadr item) (car rest))
            (cons (cadr item) (let ((m (make-sparse-keymap (cadr item))))
                                (dolist (b (reverse rest))
                                  (define-key m (vector (car b)) (cdr b)))
                                m))))))
    bar))

(defun nhmac-win-test--ref (vec)
  "VEC, flattened, as a list of (DEPTH KIND LABEL KEY ENABLED STATE INDEX)."
  (let ((w nhmac-menu-width))
    (cl-loop for o from 0 below (length vec) by w
             collect (cl-loop for j below w collect (aref vec (+ o j))))))

(ert-deftest nhmac-win-menu-filter-runs-once ()
  "A `:filter' runs once per walk, for an item in a submenu and for a bar item."
  (let ((n 0) (m 0))
    (let ((bar (nhmac-win-test--bar
                `(file "File"
                       (a menu-item "A" nhmac-win-test--cmd
                          :filter ,(lambda (d) (cl-incf n) d)))
                `(tools "Tools" (b "B" . nhmac-win-test--cmd)))))
      (define-key bar [filtered]
        `(menu-item "Filtered" ,(let ((s (make-sparse-keymap "Filtered")))
                                  (define-key s [c] '("C" . nhmac-win-test--cmd))
                                  s)
                    :filter ,(lambda (d) (cl-incf m) d)))
      (nhmac-menu--flatten bar))
    (should (= n 1))
    (should (= m 1))))

(ert-deftest nhmac-win-menu-enable-error-disables ()
  "An error in `:enable' disables the item; `:visible' that errors hides it."
  (let* ((bar (nhmac-win-test--bar
               `(file "File"
                      (a menu-item "A" nhmac-win-test--cmd :enable ,'(error "boom"))
                      (b menu-item "B" nhmac-win-test--cmd :visible ,'(error "boom"))
                      (c menu-item "C" nhmac-win-test--cmd))))
         (rows (nhmac-win-test--rows (nhmac-menu--flatten bar))))
    (should (equal (mapcar #'car rows) '("File" "File/A" "File/C")))
    (should (equal (mapcar (lambda (r) (nth 3 r)) rows) '(1 0 1)))))

(ert-deftest nhmac-win-menu-entry-fields-and-indexes ()
  "Kinds, toggle state, key hints, separators and the `menu_items' indexes."
  (let* ((on t)
         (bar (nhmac-win-test--bar
               `(file "File"
                      (a menu-item "A" nhmac-win-test--cmd :keys "C-x A")
                      (s "--")
                      (t1 menu-item "T" nhmac-win-test--cmd :button (:toggle . ,on))
                      (r1 menu-item "R" nhmac-win-test--cmd :button (:radio . nil))
                      (sub "Sub" . ,(let ((s (make-sparse-keymap "Sub")))
                                      (define-key s [z] '("Z" . nhmac-win-test--cmd))
                                      s)))
                `(edit "Edit" (q "Q" . nhmac-win-test--cmd))))
         (ref (nhmac-win-test--ref (nhmac-menu--flatten bar))))
    (should (equal ref
                   `((0 ,nhmac-menu-kind-submenu "File" "" 1 0 -1)
                     (1 ,nhmac-menu-kind-item "A" "C-x A" 1 0 3)
                     (1 ,nhmac-menu-kind-separator "" "" 0 0 11)
                     (1 ,nhmac-menu-kind-toggle "T" "" 1 1 19)
                     (1 ,nhmac-menu-kind-radio "R" "" 1 0 27)
                     (1 ,nhmac-menu-kind-submenu "Sub" "" 1 0 35)
                     (2 ,nhmac-menu-kind-item "Z" "" 1 0 47)
                     (0 ,nhmac-menu-kind-submenu "Edit" "" 1 0 -1)
                     (1 ,nhmac-menu-kind-item "Q" "" 1 0 59))))))

;; Task 7: the ns compatibility layer

(defun nhmac-win-test--compat ()
  "The fixture of the ns-* names NS defines, as a plist."
  (with-temp-buffer
    (insert-file-contents
     (expand-file-name "nhmac-win-resources/ns-compat.eld" nhmac-win-test--dir))
    (goto-char (point-min))
    (read (current-buffer))))

(ert-deftest nhmac-win-compat-complete ()
  "Every ns-* function and variable NS defines in C exists, with NS's arity and default."
  (let ((compat (nhmac-win-test--compat)) (missing nil))
    (should (= (length (plist-get compat :functions)) 34))
    (should (= (length (plist-get compat :variables)) 32))
    (dolist (f (plist-get compat :functions))
      (let ((arity (and (fboundp (car f)) (func-arity (car f)))))
        (unless (equal arity (cons (nth 1 f) (nth 2 f)))
          (push (list (car f) arity) missing))))
    (dolist (v (plist-get compat :variables))
      (unless (and (boundp (car v))
                   (if (eq (cdr v) :string)
                       (stringp (symbol-value (car v)))
                     (equal (default-value (car v)) (cdr v))))
        (push (list (car v) 'variable) missing)))
    (should (equal (nreverse missing) nil))))

(ert-deftest nhmac-win-ns-win-compiles-against-it ()
  "`ns-win.el' byte-compiles with no undefined ns-* function or free ns-* variable.
A child Emacs compiles it after loading `nhmac-win.el', as the shell's build does."
  (let* ((dir (make-temp-file "nhmac-win" t))
         (src (expand-file-name "ns-win.el" dir))
         (compat (nhmac-win-test--compat))
         (unknown nil))
    (unwind-protect
        (progn
          ;; the installed copy may be compressed; compile a plain one
          (with-temp-file src
            (insert-file-contents (locate-library "term/ns-win.el")))
          (with-temp-buffer
            (call-process "sh" nil t nil "-c" "exec \"$@\" 2>&1" "sh"
                          (expand-file-name invocation-name invocation-directory)
                          "-Q" "--batch"
                          "-L" (file-name-directory (locate-library "nhmac-win"))
                          "-l" "nhmac-win" "-f" "batch-byte-compile" src)
            (should (file-exists-p (expand-file-name "ns-win.elc" dir)))
            (goto-char (point-min))
            (while (re-search-forward
                    "\\(?:function\\|variable\\) [`‘]\\(ns-[^'’]*\\)['’]" nil t)
              (let ((name (intern (match-string 1))))
                (when (or (assq name (plist-get compat :functions))
                          (assq name (plist-get compat :variables)))
                  (push name unknown)))))
          (should (equal (nreverse unknown) nil)))
      (delete-directory dir t))))

(defconst nhmac-win-test--ns-win-events
  '(ns-power-off ns-open-file ns-open-temp-file ns-open-file-line ns-new-frame
    ns-toggle-toolbar ns-show-prefs ns-spi-service-call ns-drag-file ns-drag-color
    ns-drag-text ns-drag-operation-copy ns-drag-operation-generic ns-output-file ns-win
    ns-option-modifier ns-right-option-modifier)
  "The ns-* symbols `ns-win.el' uses that name events, drag operations, a local variable, the feature,
or an alias it makes inside a `when'.
Shell events (`make-non-key-event' codes) are the shell's to produce.")

(ert-deftest nhmac-win-ns-win-names-resolve ()
  "Every ns-* name `ns-win.el' uses is defined by it, by `nhmac-win.el', or named above."
  (let ((used nil) (defined nil))
    (with-temp-buffer
      (insert-file-contents (locate-library "term/ns-win.el"))
      (goto-char (point-min))
      (condition-case nil
          (while t
            (let ((form (read (current-buffer))))
              (when (and (memq (car-safe form)
                               '(defun defvar defvar-local defcustom defmacro defconst
                                  defface define-minor-mode cl-defmethod))
                         (symbolp (cadr form)))
                (push (cadr form) defined))
              (when (and (eq (car-safe form) 'defvaralias) (eq (car-safe (cadr form)) 'quote))
                (push (cadr (cadr form)) defined))
              (cl-labels ((walk (x)
                            (cond ((vectorp x) nil)
                                  ((consp x)
                                   (while (consp x) (walk (car x)) (setq x (cdr x)))
                                   (when x (walk x)))
                                  ((and (symbolp x) (string-prefix-p "ns-" (symbol-name x)))
                                   (cl-pushnew x used)))))
                (walk form))))
        (end-of-file nil)))
    (should (> (length used) 40))
    (should (equal (sort (cl-remove-if
                          (lambda (s) (or (memq s defined)
                                          (memq s nhmac-win-test--ns-win-events)
                                          (fboundp s) (boundp s)))
                          used)
                         #'string<)
                   nil))))

(ert-deftest nhmac-win-watchers-push-changes ()
  "A `set' of a variable the shell needs reaches `nhmac--call'; others do not."
  (let ((pushed nil)
        (old (list ns-confirm-quit ns-command-modifier ns-click-through)))
    (unwind-protect
        (cl-letf (((symbol-function 'nhmac--call)
                   (lambda (what &rest args) (push (cons what args) pushed))))
          (setq ns-confirm-quit t)
          (setq ns-command-modifier 'meta)
          (setq ns-click-through nil)
          (should (equal (reverse pushed)
                         '((variable ns-confirm-quit t)
                           (variable ns-command-modifier meta)))))
      (setq ns-confirm-quit (nth 0 old)
            ns-command-modifier (nth 1 old)
            ns-click-through (nth 2 old)))))

(ert-deftest nhmac-win-not-yet-signals ()
  (should-error (nhmac--not-yet) :type 'error)
  (should (equal (cadr (should-error (nhmac--not-yet)))
                 "Naglfar: not available yet")))

;; Task 27: menus built at idle

(defun nhmac-win-test--bar-with-sub ()
  (nhmac-win-test--bar
   `(file "File"
          (a menu-item "A" nhmac-win-test--cmd)
          (sub "Sub" . ,(let ((s (make-sparse-keymap "Sub")))
                          (define-key s [z] '("Z" . nhmac-win-test--cmd))
                          s)))
   `(edit "Edit" (q "Q" . nhmac-win-test--cmd))))

(ert-deftest nhmac-win-menu-paths-follow-the-places ()
  "Each item's place in the tree maps to the keys that lead to it (what the menu-bar events carry)."
  (let* ((r (let ((nhmac-menu--used 0))
              (cl-letf (((symbol-function 'nhmac-menu-bar-maps)
                         (lambda () (list (nhmac-win-test--bar-with-sub)))))
                (nhmac-menu-flatten-bar-with-paths))))
         (paths (cdr r)))
    (should (equal (cdr (assq 3 paths)) '(file a)))
    (should (equal (cdr (assq 11 paths)) '(file sub)))
    (should (equal (cdr (assq 23 paths)) '(file sub z)))
    (should (equal (cdr (assq 35 paths)) '(edit q)))))

(ert-deftest nhmac-win-menu-encode-has-seven-fields-a-record ()
  (let* ((vec (vector 0 nhmac-menu-kind-submenu "Fi\nle" "" 1 0 -1
                      1 nhmac-menu-kind-item "A" "C-x A" 1 0 3))
         (text (nhmac-menu-encode vec))
         (recs (split-string text "\C-^")))
    (should (= (length recs) 2))
    (should (equal (split-string (car recs) "\C-_" nil) '("0" "1" "Fi le" "" "1" "0" "-1")))
    (should (equal (split-string (cadr recs) "\C-_" nil) '("1" "0" "A" "C-x A" "1" "0" "3")))))

(ert-deftest nhmac-win-menu-one-pending-build ()
  "Triggers while a build is pending change nothing: one idle timer."
  (let ((nhmac-menu--timer nil))
    (unwind-protect
        (progn
          (nhmac-menu-invalidate)
          (let ((first nhmac-menu--timer))
            (should (timerp first))
            (nhmac-menu-invalidate)
            (nhmac-menu-invalidate)
            (should (eq nhmac-menu--timer first))))
      (when (timerp nhmac-menu--timer) (cancel-timer nhmac-menu--timer)))))

(ert-deftest nhmac-win-menu-build-sends-only-changes ()
  "A build whose tree equals the last one sends nothing; a changed tree is the next generation."
  (let ((calls nil) (bar (nhmac-win-test--bar-with-sub))
        (nhmac-menu--last nil) (nhmac-menu--timer nil) (nhmac-menu-generation 0) (nhmac-menu-builds 0))
    (cl-letf (((symbol-function 'nhmac--call) (lambda (&rest a) (push a calls)))
              ((symbol-function 'display-graphic-p) (lambda (&rest _) t))
              ((symbol-function 'nhmac-menu-bar-maps) (lambda () (list bar))))
      (nhmac-menu-build)
      (nhmac-menu-build)
      (should (= nhmac-menu-builds 2))
      (should (= (length calls) 1))
      (should (equal (list (car (car calls)) (cadr (car calls))) '(menu-bar 1)))
      (should (stringp (nth 2 (car calls))))
      (define-key bar [extra] '("Extra" . nhmac-win-test--cmd))
      (nhmac-menu-build)
      (should (= (length calls) 2))
      (should (= (cadr (car calls)) 2)))))

(ert-deftest nhmac-win-menu-popup-vector ()
  "A list menu: the records and what each chosen place returns; a nil value is disabled."
  (let* ((r (nhmac-menu--popup-vector '("T" ("P" ("a" . 1) ("--") ("c") ("b" . 2)))))
         (rows (nhmac-win-test--ref (car r))))
    (should (equal (mapcar #'cadr rows)
                   (list nhmac-menu-kind-item nhmac-menu-kind-separator
                         nhmac-menu-kind-item nhmac-menu-kind-item)))
    (should (equal (mapcar (lambda (x) (nth 4 x)) rows) '(1 0 0 1)))
    (should (equal (cdr r) '((0 . 1) (3 . 2))))))

(ert-deftest nhmac-win-menu-popup-vector-of-a-keymap ()
  "A keymap menu: a choice returns the keys that lead to the item."
  (let* ((map (make-sparse-keymap "M"))
         (sub (make-sparse-keymap "S")))
    (define-key sub [z] '("Z" . nhmac-win-test--cmd))
    (define-key map [s] (cons "Sub" sub))
    (define-key map [a] '("A" . nhmac-win-test--cmd))
    (let ((r (nhmac-menu--popup-vector map)))
      (should (member '(a) (mapcar #'cdr (cdr r))))
      (should (member '(s z) (mapcar #'cdr (cdr r)))))))

;; Task 31: application events

(ert-deftest nhmac-win-app-events-are-bound ()
  "Each application event's key path leads to its command, and the numbers match the shell's."
  (should (equal (mapcar #'car nhmac-app-paths) '(1 2 3 4)))
  (dolist (e nhmac-app-paths)
    (let ((cmd (car (cdr e))))
      (should (eq (lookup-key global-map (vector 'menu-bar cmd)) cmd))
      (should (commandp cmd)))))

(ert-deftest nhmac-win-quit-asks-when-confirming ()
  (let ((asked nil) (killed nil) (ns-confirm-quit t))
    (cl-letf (((symbol-function 'y-or-n-p) (lambda (&rest _) (setq asked t) nil))
              ((symbol-function 'save-buffers-kill-emacs) (lambda (&rest _) (setq killed t))))
      (nhmac-quit))
    (should asked)
    (should-not killed)
    (setq asked nil)
    (cl-letf (((symbol-function 'y-or-n-p) (lambda (&rest _) (setq asked t) t))
              ((symbol-function 'save-buffers-kill-emacs) (lambda (&rest _) (setq killed t))))
      (nhmac-quit))
    (should killed)
    (setq killed nil ns-confirm-quit nil asked nil)
    (cl-letf (((symbol-function 'save-buffers-kill-emacs) (lambda (&rest _) (setq killed t))))
      (nhmac-quit))
    (should killed)
    (should-not asked)))

(provide 'nhmac-win-tests)
;;; nhmac-win-tests.el ends here
