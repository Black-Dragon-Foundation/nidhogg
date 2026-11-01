#!/usr/bin/env bash
# One lane of the native shell's GUI rows, the way CI runs them (README.md, "CI and releases").  gui.sh does the runs; this
# picks the compositor and the files, and fails a lane whose rows did not run: on a pgtk build, or with a recorder the
# run could not reach, every nhwl-gui row skips and the lane would be green having tested nothing.
#
#   nidhogg/test/native-gui.sh LANE --prefix DIR --recorder BIN [gui.sh args...]
#
#   recorder          nh-recorder, sway's globals
#   recorder-mutter   nh-recorder, mutter's globals
#   recorder-inline   nh-recorder, no presenter thread (the kill switch)
#   mutter            headless mutter, its protocol trace checked: mutter is the compositor that disconnects a client
#                     for what the recorder only logs
#   sway              headless sway
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
[ -n "$prefix" ] || usage

here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -r "$work"' EXIT
rec=(--compositor recorder --recorder "$recorder")
nhwl=(--files 'nhwl-gui*-tests.el')
decor=(--files 'glitnir-decor-gui-tests.el')
trace=()
sliced=0
case $lane in
  recorder)        min=150; sliced=1; args=("${rec[@]}" "${nhwl[@]}") ;;
  recorder-mutter) min=150; sliced=1; args=("${rec[@]}" "${nhwl[@]}" --recorder-profile mutter) ;;
  recorder-inline) min=150; sliced=1; args=("${rec[@]}" "${nhwl[@]}" --env NHWL_PRESENTER=0) ;;
  mutter)          min=8;   args=(--compositor mutter "${nhwl[@]}")
                   trace=(--wayland-debug "$work/trace" --trace-check "errors,acks,unconfigured,popup,grab,popup-order") ;;
  sway)            min=8;   args=(--compositor sway "${nhwl[@]}") ;;
  decor-recorder)  min=15;  args=("${rec[@]}" "${decor[@]}") ;;
  decor-1)         min=15;  args=(--compositor sway "${decor[@]}" --scale 1) ;;
  decor-2)         min=15;  args=(--compositor sway "${decor[@]}" --scale 2) ;;
  *) usage ;;
esac
case $lane in
  *recorder*) [ -n "$recorder" ] || usage ;;
esac

fail=0
ran=0
# One Emacs: gui.sh with the lane's arguments and the selector $1, if any.
run() {
  : > "$work/log"
  bash "$here/gui.sh" --prefix "$prefix" --timeout 1200 "${args[@]}" "${trace[@]}" "${extra[@]}" ${1:+--select "$1"} 2>&1 | tee "$work/log"
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
  run ""
fi
[ "$fail" = 0 ] || exit 1
if [ "$ran" -lt "$min" ]; then
  echo "native-gui.sh: lane $lane ran $ran rows, expected at least $min" >&2
  exit 1
fi
echo "native-gui.sh: lane $lane ran $ran rows"
