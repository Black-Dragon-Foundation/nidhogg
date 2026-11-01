;;; gui-known-differences.eld --- tests that are expected to fail on one compositor  -*- lisp-data -*-

;; Read by `gui-runner-known-differences' (gui-runner.el) with `read'.  A list of entries
;;
;;   (TEST-NAME COMPOSITOR REASON . OPTIONS)
;;
;; COMPOSITOR is GLITNIR_GUI_COMPOSITOR: "sway", "mutter", "kwin", "labwc" or "recorder".  The test fails there for REASON (a string),
;; which is the compositor's, not Nidhogg's: it gets an expected result of "a failed assertion", so a run is green
;; while the difference stands and red (an unexpected pass) the day it goes away.  An error in the row is not the
;; known failure.  OPTIONS is a plist: `:either t' also accepts a pass (a row that lands on both sides there).  drgnx's test/ui-compositor-known.eld has the same
;; first three elements.
()
