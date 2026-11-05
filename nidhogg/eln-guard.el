;;; eln-guard.el --- check restored .eln files against the Emacs just built  -*- lexical-binding: t -*-

;; nidhogg/build.sh runs this with the Emacs it built, after `make', whenever it restored a compiled-Lisp cache's
;; .eln files (README.md, "CI and releases").  The cache key holds the inputs of Emacs's own .eln check (src/comp.c,
;; hash_native_abi: ABI_VERSION, the version, the configuration and every primitive's name and arity) and more
;; (nidhogg/ci/lisp-key.sh --abi), but not the C bodies, so a restored set can only be trusted once the new binary
;; agrees with it:
;;  - the facts the entry was saved with (its .eln-abi file: the native-lisp directory name, the ABI hash, libgccjit's
;;    version, the native-comp speed and compiler and driver options, the configuration and its options) are this
;;    Emacs's, character for character;
;;  - every restored .eln sits in this Emacs's native-lisp directory (named after its ABI hash);
;;  - a sample of them loads: native-elisp-load refuses a file whose embedded ABI hash (freloc_hash) is not this
;;    Emacs's (native-lisp-file-inconsistent).  An error from running a file's top level is not the guard's concern.
;; Exit 0 when all hold, 1 (one "eln-guard: STALE: ..." line per reason) when one does not: build.sh then discards the
;; restored Lisp and builds it cold.
;;   emacs -Q --batch -l nidhogg/eln-guard.el -f nidhogg-eln-guard-facts            the facts, for the save
;;   emacs -Q --batch -l nidhogg/eln-guard.el -f nidhogg-eln-guard-check LIST OBJ FACTS N
;; LIST: the restored .eln files, one per line, relative to OBJ (the build's obj/); FACTS: the entry's .eln-abi;
;; N: how many to load.

;;; Code:

(require 'subr-x)
(require 'comp)   ; native-comp-speed and the option variables

(defun nidhogg-eln-guard--facts ()
  "What a set of .eln files must agree on with the Emacs that loads them, printed."
  (format "%S\n" (list comp-native-version-dir comp-abi-hash
                       (and (fboundp 'comp-libgccjit-version) (comp-libgccjit-version))
                       native-comp-speed native-comp-compiler-options native-comp-driver-options
                       system-configuration system-configuration-options)))

(defun nidhogg-eln-guard-facts ()
  "Print this Emacs's facts."
  (princ (nidhogg-eln-guard--facts)))

(defun nidhogg-eln-guard--lines (file)
  (with-temp-buffer
    (insert-file-contents file)
    (split-string (buffer-string) "\n" t)))

(defun nidhogg-eln-guard-check ()
  "Check the restored .eln files named on the command line; exit 1 when one is not this Emacs's."
  (let* ((args command-line-args-left)
         (files (sort (nidhogg-eln-guard--lines (nth 0 args)) #'string<))
         (obj (file-name-as-directory (nth 1 args)))
         (facts-file (nth 2 args))
         (n (string-to-number (nth 3 args)))
         (dir (concat "native-lisp/" comp-native-version-dir "/"))
         (stale nil)
         (loaded 0))
    (setq command-line-args-left nil)
    (cond ((not (file-exists-p facts-file))
           (push "the entry has no .eln-abi (an older build.sh made it)" stale))
          ((not (equal (with-temp-buffer (insert-file-contents facts-file) (buffer-string))
                       (nidhogg-eln-guard--facts)))
           (push (format "the entry was made for %s; this Emacs is %s"
                         (string-trim (with-temp-buffer (insert-file-contents facts-file) (buffer-string)))
                         (string-trim (nidhogg-eln-guard--facts)))
                 stale)))
    (dolist (f files)
      (unless (string-prefix-p dir f)
        (push (format "%s is not in this Emacs's %s" f dir) stale)))
    (unless files (push "no .eln files were restored" stale))
    ;; the sample: N files spread over the sorted list, a preloaded one (the dump holds those already) only when there
    ;; is nothing else (a dev build native-compiles the preloaded files alone)
    (let* ((cands (or (seq-remove (lambda (f) (string-match-p "/preloaded/" f)) files) files))
           (len (length cands))
           (picks (and (> len 0) (delete-dups (mapcar (lambda (i) (nth (/ (* i len) (min n len)) cands))
                                                      (number-sequence 0 (1- (min n len))))))))
      (dolist (f picks)
        (condition-case err
            (progn (native-elisp-load (concat obj f)) (setq loaded (1+ loaded)))
          ((native-lisp-file-inconsistent native-lisp-load-failed)
           (push (format "%s does not load into this Emacs: %S" f err) stale))
          (error (setq loaded (1+ loaded))))))
    (if stale
        (progn (dolist (s (nreverse stale)) (message "eln-guard: STALE: %s" s))
               (kill-emacs 1))
      (message "eln-guard: ok: %d restored .eln files in %s, %d of them loaded" (length files) dir loaded)
      (kill-emacs 0))))

;;; eln-guard.el ends here
