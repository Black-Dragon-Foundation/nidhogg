#!/usr/bin/env bash
# Nidhogg's GUI test lane on macOS (the NS build today, Naglfar when it links): the built Nidhogg.app is run directly
# from this shell, never through `open', on a virtual display of the asked scale (mac-display.m), and
# test/src/nidhogg-tests/gui-runner.el runs nhmac-gui-tests.el in it.  Copied from the Glitnir P5 lane (ns-gui.sh,
# ns-display.m), renamed, with a row floor added.
#
#   nidhogg/test/nhmac-gui.sh --tarball FILE | --prefix DIR [--scale 1|2]... [--select REGEXP] [--files GLOB]
#                          [--timeout SEC] [--out DIR] [--floor N]
#
#   --tarball FILE   the macOS release tarball (nidhogg-VERSION-macos-ARCH.tar.xz); unpacked under $TMPDIR
#   --prefix DIR     an unpacked tarball (DIR/Nidhogg.app/Contents/MacOS/Emacs)
#   --scale S        the display scale, 1 or 2; repeatable (default: 1, then 2)
#   --select REGEXP  ERT selector, an Emacs regexp (alternation is \|); default every test of the files
#   --files GLOB     the test files, in test/src/nidhogg-tests (default nhmac-gui-tests.el)
#   --timeout SEC    kill a run after SEC seconds (default 900)
#   --floor N        the fewest rows a run may report (default 1): a run that reports 0 rows, or fewer than N, is red
#   --out DIR        where the logs go (default $TMPDIR/nhmac-gui); one nhmac-gui-scaleS.log per scale, and the pixels of a
#                    pixel row whose captures differ (GLITNIR_GUI_OUT, nhmac-gui-tests.el)
#
# The Screen Recording permission the pixel rows need is the runner's grant to /bin/bash, which the app inherits as a
# child of this script; `open' would make the app its own responsible process, without it.  Without the grant the pixel
# rows skip (glitnir-no-capture) and the rest run.  HOME is a temporary directory; nothing of the caller's is touched.
# Exit 0 all rows as expected, 1 a failure, too few rows, a timeout or a display that did not come up, 2 usage.
set -u

self=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
checkout=$(cd "$self/../.." && pwd)

tarball= prefix= scales=() select= files=nhmac-gui-tests.el timeout=900 out= floor=1
while [ $# -gt 0 ]; do
  case $1 in
    --tarball) tarball=${2:-}; shift 2 ;;
    --prefix) prefix=${2:-}; shift 2 ;;
    --scale) scales+=("${2:-}"); shift 2 ;;
    --select) select=${2:-}; shift 2 ;;
    --files) files=${2:-}; shift 2 ;;
    --timeout) timeout=${2:-}; shift 2 ;;
    --out) out=${2:-}; shift 2 ;;
    --floor) floor=${2:-}; shift 2 ;;
    -h|--help) sed -n '2,/^# Exit 0/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}" >&2; exit 2 ;;
    *) echo "nhmac-gui.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
case $floor in ''|*[!0-9]*) echo "nhmac-gui.sh: --floor is a number" >&2; exit 2 ;; esac
[ "$(uname -s)" = Darwin ] || { echo "nhmac-gui.sh: macOS only" >&2; exit 2; }
[ ${#scales[@]} -gt 0 ] || scales=(1 2)
for s in "${scales[@]}"; do case $s in 1|2) ;; *) echo "nhmac-gui.sh: --scale is 1 or 2" >&2; exit 2 ;; esac; done

work=$(mktemp -d "${TMPDIR:-/tmp}/nhmac-gui.XXXXXX") || exit 1
display_pid=
cleanup() {
  [ -z "$display_pid" ] || kill "$display_pid" 2>/dev/null
  rm -r "$work" 2>/dev/null
}
trap cleanup EXIT

if [ -n "$tarball" ]; then
  [ -f "$tarball" ] || { echo "nhmac-gui.sh: no tarball $tarball" >&2; exit 2; }
  mkdir "$work/app" && tar -C "$work/app" -xJf "$tarball" || { echo "nhmac-gui.sh: cannot unpack $tarball" >&2; exit 1; }
  prefix=$(echo "$work"/app/nidhogg-*)
fi
[ -n "$prefix" ] || { echo "nhmac-gui.sh: --tarball or --prefix is required" >&2; exit 2; }
emacs=$prefix/Nidhogg.app/Contents/MacOS/Emacs
[ -x "$emacs" ] || { echo "nhmac-gui.sh: $emacs not found" >&2; exit 2; }
[ -n "$out" ] || out=${TMPDIR:-/tmp}/nhmac-gui
mkdir -p "$out" || exit 1

clang -fobjc-arc -framework AppKit -framework CoreGraphics -o "$work/mac-display" "$self/mac-display.m" ||
  { echo "nhmac-gui.sh: cannot build mac-display.m" >&2; exit 1; }

# a run's time limit: macOS has no timeout(1); perl's alarm ends the run like a kill
limited() { local secs=$1; shift; perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$secs" "$@"; }

status=0
for scale in "${scales[@]}"; do
  log=$out/nhmac-gui-scale$scale.log
  ready=$work/display-$scale
  echo "nhmac-gui.sh: scale $scale"
  "$work/mac-display" --scale "$scale" --ready "$ready" >"$out/mac-display-scale$scale.log" 2>&1 &
  display_pid=$!
  for _ in $(seq 1 200); do
    [ -f "$ready" ] && break
    kill -0 "$display_pid" 2>/dev/null || break
    sleep 0.1
  done
  if [ ! -f "$ready" ]; then
    echo "nhmac-gui.sh: the scale $scale display did not come up:"; cat "$out/mac-display-scale$scale.log"
    status=1; kill "$display_pid" 2>/dev/null; display_pid=; continue
  fi
  read -r dx dy dw dh dscale <"$ready"
  echo "nhmac-gui.sh: display $dx,$dy ${dw}x${dh} scale $dscale"
  mkdir -p "$work/home-$scale"
  rc=0
  HOME="$work/home-$scale" \
  GLITNIR_GUI_SCALE=$scale GLITNIR_GUI_COMPOSITOR=windowserver \
  GLITNIR_GUI_TESTS=$checkout/test/src/nidhogg-tests GLITNIR_GUI_FILES=$files GLITNIR_GUI_SELECT=$select \
  GLITNIR_GUI_OUT=$out \
  GLITNIR_GUI_FRAME="$((dx + 20)) $((dy + 40)) 100 35" \
    limited "$timeout" "$emacs" -Q -l "$checkout/test/src/nidhogg-tests/gui-runner.el" >"$log" 2>&1 || rc=$?
  cat "$log"
  kill "$display_pid" 2>/dev/null; wait "$display_pid" 2>/dev/null; display_pid=
  if [ "$rc" != 0 ]; then echo "nhmac-gui.sh: scale $scale: exit $rc"; status=1; fi
  # every row run is counted: 0 rows, or fewer than the floor, is red even when the runner exited 0
  ran=$(sed -n 's/^.*Ran \([0-9][0-9]*\) tests.*$/\1/p' "$log" | tail -n 1)
  if [ "${ran:-0}" -lt "$floor" ]; then echo "nhmac-gui.sh: scale $scale: ${ran:-0} rows ran, the floor is $floor"; status=1; fi
done
exit $status
