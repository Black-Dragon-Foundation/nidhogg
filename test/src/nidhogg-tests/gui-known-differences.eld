;;; gui-known-differences.eld --- tests that are expected to fail on one compositor  -*- lisp-data -*-

;; Read by `gui-runner-known-differences' (gui-runner.el) with `read'.  A list of entries
;;
;;   (TEST-NAME COMPOSITOR REASON . OPTIONS)
;;
;; COMPOSITOR is GLITNIR_GUI_COMPOSITOR: "sway", "mutter" or "recorder".  The test fails there for REASON (a string),
;; which is the compositor's, not Nidhogg's: it gets an expected result of "a failed assertion", so a run is green
;; while the difference stands and red (an unexpected pass) the day it goes away.  An error in the row is not the
;; known failure.  OPTIONS is a plist: `:either t' also accepts a pass (a row that lands on both sides there).  drgnx's test/ui-compositor-known.eld has the same
;; first three elements.
(("glitnir-gui/hide-from-timer-not-late" "sway"
  "GTK's frame clock waits for sway's frame callback, which headless sway's pixman renderer sends after a software render of the whole 2880x1800 output: the first paint after a timer hide came 20-64 ms after the call (S0 gate, scales 1-2, 7 runs, one under the row's one 60 Hz tick + 4 ms); mutter's came after 10.6-11.3 ms.")
 ("glitnir-gui/child-churn-paints-the-child-only" "sway"
  "An expected failure everywhere (GL-POPUP-CHURN-FULL-1: GTK restyles the root on a child's show/hide); on sway GTK sometimes skips that root invalidation (painted px p50 1224 of 4780800 at scale 2 in 1 of 4 runs; 306 of 4982400 at scale 1 in the fix round's run), so the row passes there now and then, at either scale.  The cause of the skip was not probed."
  :either t))
