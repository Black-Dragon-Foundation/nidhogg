;;; gui-known-differences.eld --- tests that are expected to fail on one compositor  -*- lisp-data -*-

;; Read by `gui-runner-known-differences' (gui-runner.el) with `read'.  An alist: the compositor
;; (GLITNIR_GUI_COMPOSITOR: "sway", "mutter" or "recorder") and the ERT tests that fail there for a reason that is
;; the compositor's, not Nidhogg's.  Each such test gets :failed as its expected result, so a run is green while the
;; difference stands and red (an unexpected pass) the day it goes away.  A row that lands on both sides there is
;; listed as (either NAME) and accepts either result.  Every entry says why, next to it.
;;
;;   ("sway" pgtk-gui/some-test (either pgtk-gui/other-test))   ; why
(("sway"
  ;; GTK's frame clock waits for sway's frame callback, which headless sway's pixman renderer sends after a software
  ;; render of the whole 2880x1800 output: the first paint after a timer hide came 20-64 ms after the call (S0 gate,
  ;; scales 1-2, 7 runs, one under the row's one 60 Hz tick + 4 ms); mutter's came after 10.6-11.3 ms.
  (either glitnir-gui/hide-from-timer-not-late)
  ;; An expected failure everywhere (GL-POPUP-CHURN-FULL-1: GTK restyles the root on a child's show/hide); on sway at
  ;; scale 2 GTK sometimes skips that root invalidation (painted px p50 1224 of 4780800 in 1 of 4 runs), so the row
  ;; passes there now and then.
  (either glitnir-gui/child-churn-paints-the-child-only))
 ("mutter")
 ("recorder"))
