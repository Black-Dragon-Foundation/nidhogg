;;; nhmac-driver-gui-tests.el --- Naglfar's font driver, images and drawing rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh on the macOS runner (nhmac-lane.txt).  Each row judges Naglfar against Core
;; Text's or AppKit's own answers (asked through `test-font-probe') or against a stated expected value, never against
;; the NS port.  Self-contained: the lane may run this file alone.

(require 'ert)
(require 'cl-lib)

(defun nhmac-driver--say (fmt &rest args)
  "Print FMT with ARGS as a line of the lane's log (stderr), not to *Messages*."
  (princ (concat (apply #'format fmt args) "\n") #'external-debugging-output))

(defun nhmac-driver--naglfar-p ()
  "Whether this Emacs is Naglfar (the rows test its driver)."
  (featurep 'nidhogg-macos))

(defun nhmac-driver--probe (&rest args)
  "Core Text's or AppKit's answer to ARGS (`test-font-probe', nidhogg/shell/src/macos/font_probe.rs)."
  (apply #'nhmac--query 'test-font-probe args))

;;; Fonts

(ert-deftest nhmac-font-lang-picks-the-language-s-family ()
  "A script spec's first font follows its `:lang' as Core Text's own cascade does: `list-fonts' of (:script S :lang
L) starts with the family Core Text lays S's representative characters out in for L, for kana and han in Japanese,
Simplified Chinese and Korean (`mac_font_create_preferred_family_for_attributes' lays them out with the spec's
first language)."
  (skip-unless (nhmac-driver--naglfar-p))
  (let ((asked 0))
    (dolist (script '(kana han))
      (let ((chars (append (cdr (assq script script-representative-chars)) nil)))
        (dolist (lang '(ja zh-Hans ko))
          (let* ((want (apply #'nhmac-driver--probe 'preferred (symbol-name lang) chars))
                 (first (car (list-fonts (font-spec :script script :lang lang))))
                 (got (and first (symbol-name (font-get first :family)))))
            (nhmac-driver--say "nhmac-font-lang: %s %s: Core Text %S, listed first %S" script lang want got)
            ;; Core Text has no one family for characters it lays out in several fonts: nothing to move first
            (when want
              (cl-incf asked)
              (should (equal got want)))))))
    ;; kana is one character, so Core Text always names its family
    (should (>= asked 3))))

(defconst nhmac-driver--probe-fonts
  '(("/System/Library/Fonts/Supplemental/Andale Mono.ttf" "Andale Mono" "Nhmac Probe")
    ("/System/Library/Fonts/Supplemental/Courier New.ttf" "Courier New" "Nhmac Probe"))
  "System fonts copied under a family no installed font has: (FILE FAMILY NEW), NEW as long as FAMILY.")

(defun nhmac-driver--renamed-font (file family new)
  "A copy of the font FILE in a temporary file with FAMILY renamed NEW (the same length) in its name table, in
ASCII and UTF-16BE, and its PostScript name (FAMILY without spaces) with it; the copy's name."
  (let ((out (make-temp-file "nhmac-probe" nil ".ttf"))
        (pairs (list (cons family new)
                     (cons (string-replace " " "" family) (string-replace " " "" new)))))
    (with-temp-buffer
      (set-buffer-multibyte nil)
      (insert-file-contents-literally file)
      (dolist (p pairs)
        (dolist (enc (list #'identity
                           (lambda (s) (mapconcat (lambda (c) (unibyte-string 0 c)) s ""))))
          (goto-char (point-min))
          (let ((from (funcall enc (car p))) (to (funcall enc (cdr p))))
            (while (search-forward from nil t)
              (replace-match to t t)))))
      (let ((coding-system-for-write 'no-conversion))
        (write-region nil nil out nil 'silent)))
    out))

(ert-deftest nhmac-font-list-sees-a-font-installed-mid-session ()
  "A font registered while Emacs runs is listed without a restart: after Core Text's registered-fonts notification
the driver lists its faces again (macfont drops its family cache then), so an unnamed `list-fonts' finds it once
`clear-font-cache' drops Emacs's own lists."
  (skip-unless (nhmac-driver--naglfar-p))
  (let* ((src (seq-find (lambda (f) (file-exists-p (car f))) nhmac-driver--probe-fonts))
         (new (nth 2 src))
         (spec (font-spec :spacing 'm :registry 'iso10646-1))
         (listed (lambda ()
                   (clear-font-cache)
                   (cl-some (lambda (e) (equal (symbol-name (font-get e :family)) new)) (list-fonts spec)))))
    (should src)
    (should-not (funcall listed))
    (let ((file (apply #'nhmac-driver--renamed-font src)))
      (unwind-protect
          (progn
            (should (eq (nhmac-driver--probe 'register file) t))
            (let ((deadline (+ (float-time) 5)) found)
              ;; the notification reaches the shell through the run loop
              (while (and (not (setq found (funcall listed))) (< (float-time) deadline))
                (sit-for 0.1))
              (nhmac-driver--say "nhmac-font-list-refresh: %s listed %s, %.1f s left" new (if found "yes" "no")
                                 (- deadline (float-time)))
              (should found)))
        (ignore-errors (delete-file file))))))

(ert-deftest nhmac-font-ultralight-weights-follow-appkit ()
  "Every listed face whose Core Text weight trait is -0.40 (ultra-light) is listed regular when AppKit weighs it 5,
its regular, and ultra-light otherwise (`mac_font_descriptor_get_adjusted_weight'); the answers are Core Text's
traits and AppKit's `weightOfFont:', asked of each face."
  (skip-unless (nhmac-driver--naglfar-p))
  (let ((faces (nhmac-driver--probe 'ultralight))
        (regular (font-get (font-spec :weight 'normal) :weight))
        (light (font-get (font-spec :weight 'ultra-light) :weight)))
    (nhmac-driver--say "nhmac-font-ultralight: %d faces at -0.40: %S" (length faces) faces)
    (dolist (f faces)
      (pcase-let* ((`(,id ,family ,ps ,appkit) f)
                   (entity (cl-find-if (lambda (e) (equal (car (font-get e :font-entity)) id))
                                       (list-fonts (font-spec :family family))))
                   (want (if (eql appkit 5) regular light)))
        (nhmac-driver--say "nhmac-font-ultralight: %s (AppKit %S): listed %S, want %S"
                           ps appkit (and entity (font-get entity :weight)) want)
        (should entity)
        (should (eq (font-get entity :weight) want))))))

(defconst nhmac-driver--sequences
  '(("Hiragino Mincho ProN" #x845B (#xE0100 #xE0101) t)
    ("Hiragino Sans" #x8FBB (#xE0100 #xE0101) nil)
    ("Apple Color Emoji" #x2764 (#xFE0E #xFE0F) nil)
    ("Apple Color Emoji" #x263A (#xFE0E #xFE0F) nil))
  "Variation sequences: (FAMILY BASE SELECTORS MUST), MUST when FAMILY has to have the first sequence (Hiragino's
Adobe-Japan1 ideographic variation sequences).")

(ert-deftest nhmac-font-variation-glyphs ()
  "`font-variation-glyphs' reads the font's own sequences (its cmap format-14 table, macfont_variation_glyphs): for an
ideographic variation sequence in Hiragino and VS15/VS16 on emoji, a sequence the driver lists has the glyph Core
Text lays BASE + SELECTOR out with in that font, and one it does not list is laid out with the base glyph."
  (skip-unless (nhmac-driver--naglfar-p))
  (let ((size 24) (checked 0))
    (pcase-dolist (`(,family ,base ,selectors ,must) nhmac-driver--sequences)
      (let* ((entity (car (list-fonts (font-spec :family family))))
             (font (and entity (open-font entity size)))
             (have (and font (font-variation-glyphs font base)))
             (plain (and font (aref (aref (font-get-glyphs font 0 1 (string base)) 0) 3))))
        (nhmac-driver--say "nhmac-font-variation: %s %X: %S (base glyph %S)" family base have plain)
        (when font
          (dolist (vs selectors)
            (let ((ct (nhmac-driver--probe 'variation family size base vs))
                  (mine (cdr (assq vs have))))
              (nhmac-driver--say "nhmac-font-variation: %s %X %X: Core Text %S, driver %S" family base vs ct mine)
              (when ct
                (cl-incf checked)
                (should (eql ct (or mine plain))))))
          (when must
            (should (assq (car selectors) have))))))
    (should (> checked 0))))

(provide 'nhmac-driver-gui-tests)
;;; nhmac-driver-gui-tests.el ends here
