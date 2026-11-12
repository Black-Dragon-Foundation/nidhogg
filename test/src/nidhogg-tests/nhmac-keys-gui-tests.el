;;; nhmac-keys-gui-tests.el --- Naglfar's modifier choice rows on macOS  -*- lexical-binding: t -*-

;; test-class: cond

;; Run by nidhogg/test/nhmac-gui.sh (its default glob takes every nhmac-*gui-tests.el).  Keys are posted as AppKit
;; events (`nhmac--test-post-key') and read as Emacs reads them.  The comparison with NS in the same run is not
;; available (the NS app is another build): the expectations are NS's documented behaviour.

(require 'ert)
(require 'cl-lib)

(defun nhmac-keys--naglfar-p ()
  (and (featurep 'nidhogg-macos) (fboundp 'nhmac--test-post-key)))

(defconst nhmac-keys--option #x80000 "`NSEventModifierFlagOption'.")
(defconst nhmac-keys--command #x100000 "`NSEventModifierFlagCommand'.")
(defconst nhmac-keys--keypad #x200000 "`NSEventModifierFlagNumericPad'.")
(defconst nhmac-keys--right-option #x40 "The right Option key's device bit.")

(defun nhmac-keys--read (char flags &optional ignoring)
  "Post CHAR with FLAGS and read the event Emacs gets."
  (while (read-event nil nil 0.1))
  (nhmac--test-post-key char (or ignoring char) flags)
  (read-event nil nil 2))

;; Task 25: keys
(ert-deftest nhmac-keys-like-ns ()
  "The `ns-*-modifier' variables choose what each modifier key is; the keypad has its own keys."
  (skip-unless (nhmac-keys--naglfar-p))
  (let ((ns-alternate-modifier ns-alternate-modifier)
        (ns-command-modifier ns-command-modifier)
        (ns-right-alternate-modifier ns-right-alternate-modifier))
    (unwind-protect
        (progn
          ;; defaults: Option is meta, Command is super
          (setq ns-alternate-modifier 'meta ns-command-modifier 'super ns-right-alternate-modifier 'left)
          (should (eq (nhmac-keys--read ?a nhmac-keys--option) ?\M-a))
          (should (eq (nhmac-keys--read ?a nhmac-keys--command) ?\s-a))
          ;; Option as super, Command as meta
          (setq ns-alternate-modifier 'super ns-command-modifier 'meta)
          (should (eq (nhmac-keys--read ?a nhmac-keys--option) ?\s-a))
          (should (eq (nhmac-keys--read ?a nhmac-keys--command) ?\M-a))
          ;; the right Option key on its own choice
          (setq ns-right-alternate-modifier 'hyper)
          (should (eq (nhmac-keys--read ?a (logior nhmac-keys--option nhmac-keys--right-option)) ?\H-a))
          (should (eq (nhmac-keys--read ?a nhmac-keys--option) ?\s-a))
          ;; the keypad
          (should (eq (nhmac-keys--read ?5 nhmac-keys--keypad) 'kp-5))
          (should (eq (nhmac-keys--read ?+ nhmac-keys--keypad) 'kp-add)))
      (setq ns-alternate-modifier 'meta ns-command-modifier 'super ns-right-alternate-modifier 'left))))

(ert-deftest nhmac-keys-option-none-composes ()
  "With `ns-alternate-modifier' none, Option gives the composed character."
  (skip-unless (nhmac-keys--naglfar-p))
  (let ((ns-alternate-modifier ns-alternate-modifier))
    (unwind-protect
        (progn
          (setq ns-alternate-modifier 'none)
          (should (eq (nhmac-keys--read ?é nhmac-keys--option ?e) ?é)))
      (setq ns-alternate-modifier 'meta))))

(ert-deftest nhmac-keys-command-shift ()
  "Command+Shift+letter arrives as the capital letter with Command alone (s-P, not S-s-p), as the NS port sent it
and drgnx's `drgnx-keys--mac-cmd' binds it; on a named key Shift stays (S-s-<right>)."
  (skip-unless (nhmac-keys--naglfar-p))
  (let ((ns-command-modifier ns-command-modifier)
        (shift #x20000)                 ; `NSEventModifierFlagShift'
        (arrow (logior #x800000 nhmac-keys--keypad))) ; an arrow's Function and NumericPad flags
    (unwind-protect
        (progn
          (setq ns-command-modifier 'super)
          ;; AppKit's charactersIgnoringModifiers keeps Shift: the key is P
          (should (eq (nhmac-keys--read ?P (logior nhmac-keys--command shift)) ?\s-P))
          (should (eq (nhmac-keys--read ?p nhmac-keys--command) ?\s-p))
          ;; NSRightArrowFunctionKey
          (should (eq (nhmac-keys--read #xF703 (logior nhmac-keys--command shift arrow)) 'S-s-right)))
      (setq ns-command-modifier 'super))))

;;; nhmac-keys-gui-tests.el ends here
