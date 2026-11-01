#!/usr/bin/env bash
# One lane of the GUI rows, the way CI runs them (README.md, "CI and releases").  gui.sh does the runs; this picks the
# compositor and the files, and fails a lane whose rows did not run: a build without the shell, a recorder the run could
# not reach or a compositor that never came up would otherwise be green having tested nothing.
#
#   nidhogg/test/native-gui.sh LANE --prefix DIR --recorder BIN [gui.sh args...]
#
#   gui-1, gui-2      every *-gui-tests.el file on headless sway at scale 1 and 2
#   recorder          nh-recorder, sway's globals
#   recorder-mutter   nh-recorder, mutter's globals
#   recorder-kwin     nh-recorder, KWin's globals
#   recorder-xdg-v2   nh-recorder, xdg_wm_base at version 2
#   recorder-inline   nh-recorder, no presenter thread (the kill switch)
#   recorder-120hz    nh-recorder at 120 Hz
#   recorder-125      nh-recorder at scale 1.25
#   recorder-csd      nh-recorder, client-side decorations
#   recorder-daemon   nh-recorder, rows in a daemon with its frame from nidhoggclient
#   portal-fake       nh-recorder with the scripted portal: the file dialog's rows
#   mutter            headless mutter, its protocol trace checked: mutter is the compositor that disconnects a client
#                     for what the recorder only logs
#   mutter-125        the same at scale 1.25
#   kwin              KWin's virtual backend, its protocol trace checked
#   sway              headless sway
#   sway-csd          headless sway, client-side decorations
#   sway-daemon       headless sway, rows in a daemon
#   decor-recorder    the decoration rows on nh-recorder
#   decor-1, decor-2  the decoration rows on headless sway at scale 1 and 2
#
# The recorder lanes run their rows SLICE at a time, each slice in a new Emacs and compositor: rows leave windows' layers,
# frames and focus behind them, and these lanes test the shell, not one row after another's leftovers.
#
# Exit 0 the lane passed and ran its rows, 1 a failure or too few rows, 2 usage.
set -u

SLICE=16
usage() { sed -n '/^#   nidhogg/p' "$0" >&2; exit 2; }
[ $# -ge 1 ] || usage
lane=$1; shift
prefix="" recorder="" extra=()
while [ $# -gt 0 ]; do
  case $1 in
    --prefix) [ $# -ge 2 ] || usage; prefix=$2; shift 2 ;;
    --recorder) [ $# -ge 2 ] || usage; recorder=$2; shift 2 ;;
    *) extra+=("$1"); shift ;;
  esac
done
[ -n "$prefix" ] && [ -n "$recorder" ] || usage

here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -r "$work"' EXIT
rec=(--compositor recorder --recorder "$recorder")
# on sway the recorder binary serves the rows a virtual pointer (gui.sh, NHWL_GUI_VPOINTER)
sway=(--compositor sway --recorder "$recorder")
nhwl=(--files 'nhwl-gui*-tests.el')
decor=(--files 'glitnir-decor-gui-tests.el')
rules=errors,acks,unconfigured,popup,grab,popup-order
trace=()
sliced=0 only="" limit=1200
case $lane in
  gui-1)           min=103; limit=1800; args=("${sway[@]}" --scale 1) ;;
  gui-2)           min=102; limit=1800; args=("${sway[@]}" --scale 2) ;;
  recorder)        min=150; sliced=1; args=("${rec[@]}" "${nhwl[@]}") ;;
  recorder-mutter) min=150; sliced=1; args=("${rec[@]}" "${nhwl[@]}" --recorder-profile mutter) ;;
  recorder-kwin)   min=150; sliced=1; args=("${rec[@]}" "${nhwl[@]}" --recorder-profile kwin) ;;
  recorder-xdg-v2) min=150; sliced=1; args=("${rec[@]}" "${nhwl[@]}" --recorder-profile xdg-v2) ;;
  recorder-inline) min=150; sliced=1; args=("${rec[@]}" "${nhwl[@]}" --env NHWL_PRESENTER=0) ;;
  recorder-120hz)  min=150; sliced=1; args=("${rec[@]}" "${nhwl[@]}" --recorder-arg --refresh-mhz=120000) ;;
  recorder-125)    min=150; sliced=1; args=("${rec[@]}" "${nhwl[@]}" --scale 1.25) ;;
  recorder-csd)    min=150; sliced=1; args=("${rec[@]}" "${nhwl[@]}" --force-csd) ;;
  recorder-daemon) min=150; sliced=1; args=("${rec[@]}" "${nhwl[@]}" --daemon) ;;
  portal-fake)     min=3;   only='^nhwl-gui/file-dialog'; args=("${rec[@]}" "${nhwl[@]}" --portal fake) ;;
  mutter)          min=12;  args=(--compositor mutter "${nhwl[@]}")
                   trace=(--wayland-debug "$work/trace" --trace-check "$rules") ;;
  mutter-125)      min=12;  args=(--compositor mutter "${nhwl[@]}" --scale 1.25)
                   trace=(--wayland-debug "$work/trace" --trace-check "$rules") ;;
  kwin)            min=12;  args=(--compositor kwin "${nhwl[@]}")
                   trace=(--wayland-debug "$work/trace" --trace-check "$rules") ;;
  sway)            min=12;  args=("${sway[@]}" "${nhwl[@]}") ;;
  sway-csd)        min=12;  args=("${sway[@]}" "${nhwl[@]}" --force-csd) ;;
  sway-daemon)     min=12;  args=("${sway[@]}" "${nhwl[@]}" --daemon) ;;
  decor-recorder)  min=18;  args=("${rec[@]}" "${decor[@]}") ;;
  decor-1)         min=18;  args=("${sway[@]}" "${decor[@]}" --scale 1) ;;
  decor-2)         min=18;  args=("${sway[@]}" "${decor[@]}" --scale 2) ;;
  *) usage ;;
esac

fail=0
ran=0
# One Emacs: gui.sh with the lane's arguments and the selector $1, if any.
run() {
  : > "$work/log"
  bash "$here/gui.sh" --prefix "$prefix" --timeout "$limit" "${args[@]}" "${trace[@]}" "${extra[@]}" ${1:+--select "$1"} 2>&1 | tee "$work/log"
  [ "${PIPESTATUS[0]}" = 0 ] || fail=1
  # Ran 223 tests, 223 as expected (125 skipped), 0 unexpected
  local line
  line=$(cat "$work/log" "$work/trace" 2>/dev/null | sed -nE 's/^Ran ([0-9]+) tests?,.*\(([0-9]+) skipped\).*/\1 \2/p' | tail -1)
  [ -z "$line" ] || ran=$(( ran + ${line% *} - ${line#* } ))
}

if [ "$sliced" = 1 ]; then
  mapfile -t rows < <(sed -nE 's/^\(ert-deftest (nhwl-gui\/[^ ]+).*/\1/p' "$here"/../../test/src/nidhogg-tests/nhwl-gui*-tests.el | sort)
  for ((i = 0; i < ${#rows[@]}; i += SLICE)); do
    sel=$(printf '%s\\|' "${rows[@]:i:SLICE}")
    run "^\\(${sel%\\|}\\)\$"
  done
else
  run "$only"
fi
[ "$fail" = 0 ] || exit 1
if [ "$ran" -lt "$min" ]; then
  echo "native-gui.sh: lane $lane ran $ran rows, expected at least $min" >&2
  exit 1
fi
echo "native-gui.sh: lane $lane ran $ran rows"
