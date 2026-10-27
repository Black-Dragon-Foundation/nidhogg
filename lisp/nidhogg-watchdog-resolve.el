;;; nidhogg-watchdog-resolve.el --- Nidhogg's stall watchdog: names, files and packages  -*- lexical-binding: t -*-

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

;; Attribution, run at idle on the main thread.  A raw
;; record (from `nidhogg-watchdog--take') becomes a resolved one: every
;; function object gets a name, a file and a package (cached in a weak `eq'
;; table), then the record's samples give its entry, culprit, state, phase
;; shares, signature and merged stacks.  The load-history index that file
;; lookups need is built in 2 ms slices; a record whose symbols are not
;; indexed yet resolves to `:later' and waits for the next drain.

;;; Code:

(defvar nidhogg-watchdog--clock)
(defvar nidhogg-watchdog--resolve-function)
(defvar nidhogg-watchdog-package-functions)
(defvar package-alist)
(defvar lisp-directory)
(declare-function advice--p "nadvice" (object))
(declare-function advice--car "nadvice" (f))
(declare-function package-desc-dir "package" (desc))

(defvar nidhogg-watchdog--deadline nil
  "The drain's deadline in `nidhogg-watchdog--clock' time, nil for none.")

(defconst nidhogg-watchdog--slice 0.002
  "Seconds one index slice may run.")

;;; The load-history index

(defvar nidhogg-watchdog--index-started nil)
(defvar nidhogg-watchdog--index-syms nil
  "Symbol to the file of its `defun' or `defvar' (the newest definition).")
(defvar nidhogg-watchdog--index-bases nil
  "Source base name to an alist (KEY . FILE), one per distinct source file.")
(defvar nidhogg-watchdog--index-todo nil
  "The tail of `load-history' not yet indexed.")

(defun nidhogg-watchdog--file-base (file)
  "The source base name of FILE: no directory, no extension, no .eln hashes."
  (let ((name (file-name-nondirectory file)))
    (cond ((string-match "\\`\\(.+\\)-[0-9a-f]+-[0-9a-f]+\\.eln\\'" name)
           (match-string 1 name))
          ((string-match "\\`\\(.+?\\)\\.\\(?:el\\|elc\\|eln\\)\\(?:\\.gz\\)?\\'"
                         name)
           (match-string 1 name))
          (t name))))

(defun nidhogg-watchdog--index-file (entry &optional force)
  "Add the load-history ENTRY to the index.
The first definition seen wins, load-history being newest first; with FORCE
a newly loaded file overrides.  Variables are the bare symbols, functions
`(defun . SYM)': the entries `symbol-file' reads, so the two agree."
  (let ((file (car entry))
        (syms nidhogg-watchdog--index-syms))
    (when (stringp file)
      (dolist (e (cdr entry))
        (let ((sym (cond ((and e (symbolp e) (not (eq e t))) e)
                         ((and (consp e) (memq (car e) '(defun t))
                               (symbolp (cdr e)))
                          (cdr e)))))
          (when (and sym (or force (not (gethash sym syms))))
            (puthash sym file syms))))
      (unless (string-suffix-p ".eln" file)
        (let* ((base (nidhogg-watchdog--file-base file))
               (key (file-name-sans-extension (if (string-suffix-p ".gz" file)
                                                         (substring file 0 -3)
                                                       file)))
               (cur (gethash base nidhogg-watchdog--index-bases)))
          (unless (assoc key cur)
            (puthash base (cons (cons key file) cur)
                     nidhogg-watchdog--index-bases)))))))

(defun nidhogg-watchdog--index-after-load (_file)
  "`after-load-functions': index the file that was just loaded."
  (when (and nidhogg-watchdog--index-started load-history)
    (nidhogg-watchdog--index-file (car load-history) t)))

(defun nidhogg-watchdog--index-done-p ()
  (and nidhogg-watchdog--index-started (null nidhogg-watchdog--index-todo)))

(defun nidhogg-watchdog--index-step (&optional deadline)
  "Index `load-history' until DEADLINE (nil: to the end); non-nil when done."
  (unless nidhogg-watchdog--index-started
    (setq nidhogg-watchdog--index-syms (make-hash-table :test 'eq :size 8192)
          nidhogg-watchdog--index-bases (make-hash-table :test 'equal)
          nidhogg-watchdog--index-todo load-history
          nidhogg-watchdog--index-started t)
    (add-hook 'after-load-functions #'nidhogg-watchdog--index-after-load))
  (let ((clock nidhogg-watchdog--clock))
    (while (and nidhogg-watchdog--index-todo
                (or (null deadline) (< (funcall clock) deadline)))
      (nidhogg-watchdog--index-file (pop nidhogg-watchdog--index-todo))))
  (null nidhogg-watchdog--index-todo))

(defun nidhogg-watchdog--index-lookup (sym)
  "The file that defines SYM, nil when none, `:later' when not indexed yet."
  (let ((file (and nidhogg-watchdog--index-syms
                   (gethash sym nidhogg-watchdog--index-syms))))
    (cond (file file)
          ((nidhogg-watchdog--index-done-p) nil)
          (t :later))))

(defun nidhogg-watchdog--base-lookup (base)
  "The (KEY . FILE) alist of source files named BASE, or `:later'."
  (if (nidhogg-watchdog--index-done-p)
      (gethash base nidhogg-watchdog--index-bases)
    :later))

;;; Files and packages

(defvar nidhogg-watchdog--pkg-map nil
  "(PACKAGE-ALIST LENGTH . DIRS): the directory map of that `package-alist'.")

(defun nidhogg-watchdog--pkg-dirs ()
  "An alist (DIRECTORY . NAME) of the installed packages."
  (let ((alist (bound-and-true-p package-alist)))
    (unless (and nidhogg-watchdog--pkg-map
                 (eq (car nidhogg-watchdog--pkg-map) alist)
                 (eql (cadr nidhogg-watchdog--pkg-map) (length alist)))
      (let (dirs)
        (dolist (e alist)
          (let ((dir (ignore-errors (package-desc-dir (cadr e)))))
            (when (stringp dir)
              (push (cons (file-name-as-directory (expand-file-name dir))
                          (symbol-name (car e)))
                    dirs))))
        (setq nidhogg-watchdog--pkg-map
              (cons alist (cons (length alist) dirs)))))
    (cddr nidhogg-watchdog--pkg-map)))

(defun nidhogg-watchdog--lisp-file-p (file)
  (and (bound-and-true-p lisp-directory)
       (string-prefix-p (file-name-as-directory
                         (expand-file-name lisp-directory))
                        (expand-file-name file))))

(defun nidhogg-watchdog--pkg-dir-file-p (file)
  "The name of the installed package whose directory holds FILE, or nil."
  (let ((abs (expand-file-name file)) name)
    (dolist (d (nidhogg-watchdog--pkg-dirs))
      (when (and (not name) (string-prefix-p (car d) abs))
        (setq name (cdr d))))
    name))

(defun nidhogg-watchdog--default-package (file)
  "The package of FILE: an installed package, or `emacs'; else nil."
  (cond ((equal file "C") "emacs")
        ((nidhogg-watchdog--pkg-dir-file-p file))
        ((nidhogg-watchdog--lisp-file-p file) "emacs")))

(defun nidhogg-watchdog--package-of (file)
  "The package name of FILE, or nil.
`nidhogg-watchdog-package-functions' are tried first, each with FILE."
  (when (stringp file)
    (let ((p (condition-case nil
                 (run-hook-with-args-until-success
                  'nidhogg-watchdog-package-functions file)
               (error nil))))
      (cond ((stringp p) p)
            ((and p (symbolp p)) (symbol-name p))
            (t (nidhogg-watchdog--default-package file))))))

(defun nidhogg-watchdog--package-detail (file)
  "The library detail for an Emacs file (\"emacs/jit-lock\"), else nil."
  (and (stringp file) (not (equal file "C"))
       (not (nidhogg-watchdog--pkg-dir-file-p file))
       (nidhogg-watchdog--lisp-file-p file)
       (concat "emacs/" (nidhogg-watchdog--file-base file))))

;;; Function objects

(defvar nidhogg-watchdog--speed3-function nil
  "Function taking an .eln file name, non-nil when its unit was built at speed 3.
Emacs has no API for a unit's speed; a build that knows sets this.")

(defvar nidhogg-watchdog--fn-cache (make-hash-table :test 'eq :weakness 'key)
  "Function object to its info plist.")

(defun nidhogg-watchdog--info (name file &rest more)
  "An info plist for NAME defined in FILE."
  (append (list :name name :file file
                :pkg (nidhogg-watchdog--package-of file)
                :detail (nidhogg-watchdog--package-detail file))
          more))

(defun nidhogg-watchdog--native-p (fn)
  (and (subrp fn) (fboundp 'subr-native-elisp-p) (subr-native-elisp-p fn)))

(defun nidhogg-watchdog--native-info (name fn)
  "Info for the native function FN; NAME is its symbol name or nil (anonymous)."
  (let* ((eln (native-comp-unit-file (subr-native-comp-unit fn)))
         (base (nidhogg-watchdog--file-base eln))
         (srcs (nidhogg-watchdog--base-lookup base)))
    (if (eq srcs :later)
        :later
      (let* ((ambiguous (cdr srcs))
             (src (and srcs (not ambiguous) (cdar srcs)))
             (extra (list :speed3 (and nidhogg-watchdog--speed3-function
                                       (funcall nidhogg-watchdog--speed3-function eln)
                                       t)
                          :native t)))
        (apply #'nidhogg-watchdog--info
               (or name (format "<lambda in %s>" base))
               (or src eln)
               :anon (null name)
               :ambiguous (and ambiguous (null name))
               extra)))))

(defun nidhogg-watchdog--symbol-info (sym)
  (let ((def (and (fboundp sym) (symbol-function sym)))
        (name (symbol-name sym)))
    (when (and def (not (symbolp def)) (fboundp 'advice--p) (advice--p def)
               (fboundp 'advice--cd*r))
      (setq def (advice--cd*r def)))
    (cond ((nidhogg-watchdog--native-p def)
           (nidhogg-watchdog--native-info name def))
          ((subrp def) (nidhogg-watchdog--info name "C" :prim t))
          (t (let ((file (or (let ((h (car-safe (get sym 'function-history))))
                               (and (stringp h) h))
                             (nidhogg-watchdog--index-lookup sym))))
               (if (eq file :later) :later
                 (nidhogg-watchdog--info name file)))))))

(defun nidhogg-watchdog--subr-info (fn)
  (let* ((name (subr-name fn))
         (sym (intern-soft name)))
    (cond ((and sym (fboundp sym) (eq (symbol-function sym) fn))
           (nidhogg-watchdog--symbol-info sym))
          ((nidhogg-watchdog--native-p fn)
           (nidhogg-watchdog--native-info nil fn))
          (t (nidhogg-watchdog--info name "C" :prim t)))))

(defun nidhogg-watchdog--compute-info (fn)
  "The info plist of FN, or `:later'."
  (condition-case nil
      (cond
       ((eq fn 'nidhogg-watchdog--gone) (list :name "<gone>" :anon t))
       ((and (not (symbolp fn)) (fboundp 'advice--p) (advice--p fn))
        (let* ((car (advice--car fn))
               (sub (or (gethash car nidhogg-watchdog--fn-cache)
                        (nidhogg-watchdog--compute-info car))))
          (if (eq sub :later) :later
            (nidhogg-watchdog--info
             (concat "advice: " (plist-get sub :name))
             (plist-get sub :file) :advice t))))
       ((symbolp fn) (nidhogg-watchdog--symbol-info fn))
       ((subrp fn) (nidhogg-watchdog--subr-info fn))
       (t (list :name "<lambda>" :anon t)))
    (error (list :name "<unknown>" :anon t))))

(defun nidhogg-watchdog--info-for (fn &optional deadline)
  "The cached info plist of FN; `:later' while the index is incomplete.
A slice of the index runs up to DEADLINE (nil: to the end) on a miss."
  (or (gethash fn nidhogg-watchdog--fn-cache)
      (let ((info (nidhogg-watchdog--compute-info fn)))
        (when (eq info :later)
          (nidhogg-watchdog--index-step deadline)
          (setq info (nidhogg-watchdog--compute-info fn)))
        (unless (eq info :later)
          (puthash fn info nidhogg-watchdog--fn-cache))
        info)))

;;; Samples: entry, culprit, phases, stacks

(defun nidhogg-watchdog--sample-seq (sample)
  "SAMPLE's frames innermost first as a list of (FUNCTION . HOOK-NAME).
A skipped stretch shows as (:gap)."
  (let* ((frames (plist-get sample :frames))
         (outer (plist-get sample :outer))
         (hooks (plist-get sample :hooks))
         (ni (length frames))
         (no (length outer))
         (nh (length hooks))
         seq)
    (dotimes (i ni)
      (push (cons (aref frames i) (and (< i nh) (aref hooks i))) seq))
    (when (> (or (plist-get sample :skipped) 0) 0)
      (push (list :gap) seq))
    (dotimes (j no)
      (push (cons (aref outer j) (and (< (+ ni j) nh) (aref hooks (+ ni j))))
            seq))
    (nreverse seq)))

(defun nidhogg-watchdog--seq-name (info)
  (plist-get info :name))

(defconst nidhogg-watchdog--command-skip
  '(command-execute call-interactively funcall-interactively))
(defconst nidhogg-watchdog--timer-skip '(apply funcall))
(defconst nidhogg-watchdog--wait-fns
  '(accept-process-output sit-for sleep-for read-event read-char read-key))

(defun nidhogg-watchdog--after (rest skip)
  "The first function in REST (a list of (FN . HOOK)) not in SKIP, or nil."
  (while (and rest (memq (caar rest) skip))
    (setq rest (cdr rest)))
  (and rest (not (eq (caar rest) :gap)) (caar rest)))

(defun nidhogg-watchdog--entry-of (fn kind infos &optional extra)
  "An entry plist of KIND for the function FN."
  (let ((info (and fn (gethash fn infos))))
    (append (list :kind kind
                  :fn (and info (nidhogg-watchdog--seq-name info))
                  :pkg (and info (plist-get info :pkg)))
            extra)))

(defun nidhogg-watchdog--sample-entry (sample infos)
  "The entry of SAMPLE: the outermost meaningful frame."
  (let* ((outward (reverse (nidhogg-watchdog--sample-seq sample)))
         (phase (plist-get sample :phase))
         (thread (plist-get sample :lisp-thread))
         entry)
    (cond
     (thread
      (list :kind 'lisp-thread :fn (if (stringp thread) thread "thread")))
     ((memq phase '(filter sentinel))
      (let ((from outward) (rest outward))
        (while rest
          (when (memq (caar rest) nidhogg-watchdog--wait-fns)
            (setq from (cdr rest)))
          (setq rest (cdr rest)))
        (nidhogg-watchdog--entry-of (nidhogg-watchdog--after from nil)
                                    phase infos)))
     (t
      (let ((rest outward))
        (while (and rest (not entry))
          (let ((fn (caar rest)) (hook (cdar rest)))
            (cond ((memq fn '(command-execute call-interactively))
                   (setq entry (nidhogg-watchdog--entry-of
                                (nidhogg-watchdog--after
                                 (cdr rest) nidhogg-watchdog--command-skip)
                                'command infos)))
                  ((eq fn 'timer-event-handler)
                   (setq entry (nidhogg-watchdog--entry-of
                                (nidhogg-watchdog--after
                                 (cdr rest) nidhogg-watchdog--timer-skip)
                                'timer infos)))
                  (hook
                   (setq entry
                         (list :kind 'hook
                               :fn (symbol-name hook)
                               :function
                               (let* ((f (nidhogg-watchdog--after (cdr rest) nil))
                                      (i (and f (gethash f infos))))
                                 (and i (nidhogg-watchdog--seq-name i)))
                               :pkg
                               (let* ((f (nidhogg-watchdog--after (cdr rest) nil))
                                      (i (and f (gethash f infos))))
                                 (and i (plist-get i :pkg))))))))
          (setq rest (cdr rest))))
      (or entry
          (cond ((plist-get sample :gc) (list :kind 'gc))
                ((eq phase 'redisplay) (list :kind 'redisplay))
                (t (let ((fn (nidhogg-watchdog--after outward nil)))
                     (nidhogg-watchdog--entry-of fn 'lisp infos)))))))))

(defun nidhogg-watchdog--attributed (sample infos)
  "SAMPLE's culprit candidates, innermost first: a list of infos.
A primitive is skipped; an anonymous function is attributed to the nearest
named frame outward of it."
  (let ((seq (nidhogg-watchdog--sample-seq sample))
        out)
    (while seq
      (let* ((fn (caar seq))
             (info (and (not (eq fn :gap)) (gethash fn infos))))
        (cond ((null info))
              ((plist-get info :prim))
              ((plist-get info :anon)
               (let ((rest (cdr seq)) found)
                 (while (and rest (not found))
                   (let ((i (and (not (eq (caar rest) :gap))
                                 (gethash (caar rest) infos))))
                     (cond ((null i) (setq rest nil))
                           ((or (plist-get i :prim) (plist-get i :anon))
                            (setq rest (cdr rest)))
                           (t (setq found i)))))
                 (when found (push found out))))
              (t (push info out))))
      (setq seq (cdr seq)))
    (nreverse out)))

(defun nidhogg-watchdog--phase-key (sample)
  "The phase of SAMPLE as a string: `gc', `phase' or `phase:reason'."
  (cond ((plist-get sample :gc) "gc")
        (t (let ((phase (plist-get sample :phase))
                 (reason (plist-get sample :reason)))
             (concat (if phase (symbol-name phase) "lisp")
                     (and reason (concat ":" (symbol-name reason))))))))

(defun nidhogg-watchdog--mode-of (items)
  "The most frequent element of ITEMS (`equal'; the first on a tie)."
  (let ((counts (make-hash-table :test 'equal)) best (n 0))
    (dolist (i items) (puthash i (1+ (gethash i counts 0)) counts))
    (dolist (i items)
      (when (> (gethash i counts) n)
        (setq best i n (gethash i counts))))
    best))

(defun nidhogg-watchdog--culprit (nongc infos phase)
  "The culprit over the non-GC samples NONGC, or nil.
PHASE is the dominant phase string."
  (let ((total (length nongc))
        (table (make-hash-table :test 'equal))
        order)
    (dolist (sample nongc)
      (let ((i 0) (seen nil))
        (dolist (info (nidhogg-watchdog--attributed sample infos))
          (let ((name (plist-get info :name)))
            (unless (member name seen)
              (push name seen)
              (let ((e (gethash name table)))
                (unless e (setq e (list info 0 0)) (push name order)
                        (puthash name e table))
                (setcar (cdr e) (1+ (cadr e)))
                (setcar (cddr e) (+ (nth 2 e) i))))
            (setq i (1+ i))))))
    (let (best)
      (dolist (pass '(non-emacs emacs))
        (unless best
          (dolist (name (reverse order))
            (let* ((e (gethash name table))
                   (info (car e))
                   (emacs (equal (plist-get info :pkg) "emacs")))
              (when (and (eq emacs (eq pass 'emacs))
                         (>= (* 2 (cadr e)) total)
                         (or (null best)
                             (< (/ (float (nth 2 e)) (cadr e))
                                (/ (float (nth 2 best)) (cadr best)))))
                (setq best (cons info (cdr e))))))))
      (when best
        (let ((info (car best)))
          (list :fn (plist-get info :name)
                :pkg (plist-get info :pkg)
                :file (plist-get info :file)
                :share (/ (float (nth 1 best)) total)
                :phase phase))))))

(defun nidhogg-watchdog--state (record nongc)
  "The state of RECORD; NONGC are its non-GC samples."
  (let ((wall (plist-get record :wall-ms))
        (gc (plist-get record :gc-ms))
        (cpu (plist-get record :cpu-ms)))
    (cond
     ((not (and (numberp wall) (> wall 0))) 'unsampled)
     ((and (numberp gc) (>= (* 2 gc) wall)) 'gc)
     ((and nongc
           (let ((n 0))
             (dolist (s nongc)
               (when (memq (plist-get s :phase) '(treesit-wait worker-wait))
                 (setq n (1+ n))))
             (>= (* 2 n) (length nongc))))
      'blocked)
     ((not (numberp cpu)) 'unsampled)
     ((>= (/ cpu wall) 0.8) 'cpu)
     ((< (/ cpu wall) 0.2) 'blocked)
     (t 'mixed))))

(defun nidhogg-watchdog--stacks (samples infos)
  "SAMPLES merged by identical frame names: at most 8 of 32 names."
  (let ((table (make-hash-table :test 'equal)) keys)
    (dolist (sample samples)
      (let ((names (mapcar (lambda (f)
                             (if (eq (car f) :gap) "…"
                               (plist-get (gethash (car f) infos) :name)))
                           (nidhogg-watchdog--sample-seq sample))))
        (when names
          (setq names (take 32 names))
          (unless (gethash names table) (push names keys))
          (puthash names (1+ (gethash names table 0)) table))))
    (mapcar (lambda (k) (list :n (gethash k table) :frames k))
            (take 8 (sort (nreverse keys)
                          (lambda (a b) (> (gethash a table) (gethash b table))))))))

(defun nidhogg-watchdog--phase-shares (samples)
  "The share of SAMPLES in each phase, as a plist."
  (let ((counts nil) (total (length samples)))
    (dolist (s samples)
      (let* ((key (nidhogg-watchdog--phase-key s))
             (cell (assoc key counts)))
        (if cell (setcdr cell (1+ (cdr cell)))
          (push (cons key 1) counts))))
    (let (out)
      (dolist (c (sort (nreverse counts) (lambda (a b) (> (cdr a) (cdr b)))))
        (push (intern (concat ":" (car c))) out)
        (push (/ (float (cdr c)) total) out))
      (nreverse out))))

(defun nidhogg-watchdog--signature (entry culprit phase)
  "ENTRY.KIND:ENTRY.FN>CULPRIT.FN/PHASE, the ranking key."
  (if (eq (plist-get entry :kind) 'unsampled)
      "unsampled"
    (concat (symbol-name (plist-get entry :kind))
            (and (plist-get entry :fn) (concat ":" (plist-get entry :fn)))
            (and (plist-get culprit :fn) (concat ">" (plist-get culprit :fn)))
            (and phase (concat "/" phase)))))

;;; The resolver

(defun nidhogg-watchdog--sample-fns (sample)
  "The distinct function objects in SAMPLE."
  (let (out)
    (dolist (v (list (plist-get sample :frames) (plist-get sample :outer)))
      (mapc (lambda (f) (unless (memq f out) (push f out))) v))
    out))

(defun nidhogg-watchdog-resolve (record &optional deadline)
  "Resolve the raw RECORD: add its entry, culprit, state, phases, signature
and stacks.  Return `:later' when the load-history index is not ready by
DEADLINE (a time in `nidhogg-watchdog--clock' units; nil: no limit)."
  (let ((samples (append (plist-get record :samples) nil))
        (infos (make-hash-table :test 'eq))
        later)
    (dolist (s samples)
      (dolist (fn (nidhogg-watchdog--sample-fns s))
        (unless (or later (gethash fn infos))
          (let ((info (nidhogg-watchdog--info-for fn deadline)))
            (if (eq info :later) (setq later t) (puthash fn info infos))))))
    (if later
        :later
      (let* ((nongc (let (l) (dolist (s samples) (unless (plist-get s :gc) (push s l)))
                      (nreverse l)))
             (state (nidhogg-watchdog--state record nongc))
             (phase (and nongc (nidhogg-watchdog--mode-of
                                (mapcar #'nidhogg-watchdog--phase-key nongc))))
             (entry (cond (samples
                           (nidhogg-watchdog--mode-of
                            (mapcar (lambda (s)
                                      (nidhogg-watchdog--sample-entry s infos))
                                    samples)))
                          ((eq state 'gc) (list :kind 'gc))
                          (t (list :kind 'unsampled))))
             (culprit (if nongc
                          (nidhogg-watchdog--culprit nongc infos phase)
                        nil))
             (culprit (or culprit
                          (list :fn (if samples nil "unsampled")
                                :phase phase)))
             (res (copy-sequence record)))
        (setq res (plist-put res :state state)
              res (plist-put res :entry entry)
              res (plist-put res :culprit culprit)
              res (plist-put res :phases (and samples
                                              (nidhogg-watchdog--phase-shares
                                               samples)))
              res (plist-put res :signature
                             (nidhogg-watchdog--signature
                              entry culprit
                              (or phase (and (eq state 'gc) "gc"))))
              res (plist-put res :stacks
                             (nidhogg-watchdog--stacks samples infos)))
        res))))

(defun nidhogg-watchdog--resolve-record (raw)
  "The drain's resolver: RAW resolved by the drain's deadline, or `:later'.
A failure leaves the record raw."
  (condition-case nil
      (nidhogg-watchdog-resolve raw nidhogg-watchdog--deadline)
    (error raw)))

(setq nidhogg-watchdog--resolve-function #'nidhogg-watchdog--resolve-record)

(provide 'nidhogg-watchdog-resolve)

;;; nidhogg-watchdog-resolve.el ends here
