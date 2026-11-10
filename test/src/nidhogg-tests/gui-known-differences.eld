;;; gui-known-differences.eld --- tests that are expected to fail on one compositor  -*- lisp-data -*-

;; Read by `gui-runner-known-differences' (gui-runner.el) with `read'.  A list of entries
;;
;;   (TEST-NAME COMPOSITOR REASON . OPTIONS)
;;
;; COMPOSITOR is GLITNIR_GUI_COMPOSITOR: "sway", "mutter", "kwin", "labwc", "recorder" or
;; "windowserver" (macOS, nhmac-gui.sh).  The test fails there for REASON (a string),
;; which is the compositor's, not Nidhogg's: it gets an expected result of "a failed assertion", so a run is green
;; while the difference stands and red (an unexpected pass) the day it goes away.  An error in the row is not the
;; known failure.  OPTIONS is a plist: `:either t' also accepts a pass (a row that lands on both sides there).  drgnx's test/ui-compositor-known.eld has the same
;; first three elements.
(("nhwl-gui/live-sway-resize-burst-ends-at-the-last-size" "sway"
  "sway loses every resize of a burst after the first: they wait in its pending transaction while it waits for the
window's answer to the first, and that answer (a floating window's commit of the first size) resizes the container to
it and overwrites them (view_update_size, transaction_commit_dirty_client), so no configure of the last size is ever
sent.  Any client that answers after the burst's later requests came loses them; the trace shows the first size's
configure answered in 9 ms and none after it.  A race, so either side: with client-side decorations (gui.sh
--force-csd) the row passes, the frame and sway's rectangle at the last size"
  :either t)
 ("glitnir-nhmac/card-layer-follows-the-window" "windowserver"
  "Naglfar: after glitnir-nhmac/card-corner-shows-gap the root frame's picture freezes (its backing keeps the card
layout through full redraws), so the row starts on a dirty root and its clean-root check fails.  Nidhogg's own bug,
parked as DEFERRED NAGLFAR-CARD-ROOT-FREEZE-1 until before the M1 gate; NS skips the row")
 ("glitnir-nhmac/card-removed-no-residue" "windowserver"
  "Naglfar: after glitnir-nhmac/card-corner-shows-gap the root frame's picture freezes (its backing keeps the card
layout through full redraws), so the row starts on a dirty root and its clean-root check fails.  Nidhogg's own bug,
parked as DEFERRED NAGLFAR-CARD-ROOT-FREEZE-1 until before the M1 gate; NS skips the row")
 ("glitnir-nhmac/card-shadow-only-in-gap" "windowserver"
  "Naglfar: after glitnir-nhmac/card-corner-shows-gap the root frame's picture freezes (its backing keeps the card
layout through full redraws), so the row starts on a dirty root and its clean-root check fails.  Nidhogg's own bug,
parked as DEFERRED NAGLFAR-CARD-ROOT-FREEZE-1 until before the M1 gate; NS skips the row")
 ("glitnir-nhmac/corner-shows-what-is-beneath" "windowserver"
  "Naglfar: after glitnir-nhmac/card-corner-shows-gap the root frame's picture freezes (its backing keeps the card
layout through full redraws), so the row starts on a dirty root and its clean-root check fails.  Nidhogg's own bug,
parked as DEFERRED NAGLFAR-CARD-ROOT-FREEZE-1 until before the M1 gate; NS skips the row")
 ("glitnir-nhmac/shadow-darkens-below" "windowserver"
  "Naglfar: after glitnir-nhmac/card-corner-shows-gap the root frame's picture freezes (its backing keeps the card
layout through full redraws), so the row starts on a dirty root and its clean-root check fails.  Nidhogg's own bug,
parked as DEFERRED NAGLFAR-CARD-ROOT-FREEZE-1 until before the M1 gate; NS skips the row")
 ("glitnir-nhmac/text-identical-to-off" "windowserver"
  "Naglfar: after glitnir-nhmac/card-corner-shows-gap the root frame's picture freezes (its backing keeps the card
layout through full redraws), so the row starts on a dirty root and its clean-root check fails.  Nidhogg's own bug,
parked as DEFERRED NAGLFAR-CARD-ROOT-FREEZE-1 until before the M1 gate; NS skips the row")
 ("nhmac-cards-theme-switch" "windowserver"
  "Naglfar: after glitnir-nhmac/card-corner-shows-gap the root frame's picture freezes (its backing keeps the card
layout through full redraws), so the row starts on a dirty root and its clean-root check fails.  Nidhogg's own bug,
parked as DEFERRED NAGLFAR-CARD-ROOT-FREEZE-1 until before the M1 gate; NS skips the row")
 ("nhmac-cards-typing-no-change" "windowserver"
  "Naglfar: after glitnir-nhmac/card-corner-shows-gap the root frame's picture freezes (its backing keeps the card
layout through full redraws), so the row starts on a dirty root and its clean-root check fails.  Nidhogg's own bug,
parked as DEFERRED NAGLFAR-CARD-ROOT-FREEZE-1 until before the M1 gate; NS skips the row")
 ("nhmac-glitnir-off" "windowserver"
  "Naglfar: after glitnir-nhmac/card-corner-shows-gap the root frame's picture freezes (its backing keeps the card
layout through full redraws), so the row starts on a dirty root and its clean-root check fails.  Nidhogg's own bug,
parked as DEFERRED NAGLFAR-CARD-ROOT-FREEZE-1 until before the M1 gate; NS skips the row"))
