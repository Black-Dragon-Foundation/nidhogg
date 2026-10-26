;;; gui-known-differences.eld --- tests that are expected to fail on one compositor  -*- lisp-data -*-

;; Read by `gui-runner-known-differences' (gui-runner.el) with `read'.  An alist: the compositor
;; (GLITNIR_GUI_COMPOSITOR: "sway", "mutter" or "recorder") and the ERT tests that fail there for a reason that is
;; the compositor's, not Nidhogg's.  Each such test gets :failed as its expected result, so a run is green while the
;; difference stands and red (an unexpected pass) the day it goes away.  Every entry says why, next to it.
;;
;;   ("sway" pgtk-gui/some-test)   ; why
(("sway")
 ("mutter")
 ("recorder"))
