#!/usr/bin/env bash
# Nidhogg's GUI test lane on macOS (NS): the built Nidhogg.app is run directly from this shell, never through `open', on
# a virtual display of the asked scale (ns-display.m), and test/src/nidhogg-tests/gui-runner.el runs
# glitnir-ns-gui-tests.el in it.
#
#   nidhogg/test/ns-gui.sh --tarball FILE | --prefix DIR [--scale 1|2]... [--select REGEXP] [--files GLOB]
#                          [--timeout SEC] [--out DIR]
#
#   --tarball FILE   the macOS release tarball (nidhogg-VERSION-macos-ARCH.tar.xz); unpacked under $TMPDIR
#   --prefix DIR     an unpacked tarball (DIR/Nidhogg.app/Contents/MacOS/Emacs)
#   --scale S        the display scale, 1 or 2; repeatable (default: 1, then 2)
#   --select REGEXP  ERT selector, an Emacs regexp (alternation is \|); default every test of the files
#   --files GLOB     the test files, in test/src/nidhogg-tests (default glitnir-ns-gui-tests.el)
#   --timeout SEC    kill a run after SEC seconds (default 900)
#   --out DIR        where the logs go (default $TMPDIR/ns-gui); one ns-gui-scaleS.log per scale, and the pixels of a
#                    pixel row whose captures differ (GLITNIR_GUI_OUT, glitnir-ns-gui-tests.el)
#
# The Screen Recording permission the pixel rows need is the runner's grant to /bin/bash, which the app inherits as a
# child of this script; `open' would make the app its own responsible process, without it.  Without the grant the pixel
# rows skip (glitnir-no-capture) and the rest run.  HOME is a temporary directory; nothing of the caller's is touched.
# Exit 0 all rows as expected, 1 a failure, a timeout or a display that did not come up, 2 usage.
set -u

self=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
checkout=$(cd "$self/../.." && pwd)

tarball= prefix= scales=() select= files=glitnir-ns-gui-tests.el timeout=900 out=
while [ $# -gt 0 ]; do
  case $1 in
    --tarball) tarball=${2:-}; shift 2 ;;
    --prefix) prefix=${2:-}; shift 2 ;;
    --scale) scales+=("${2:-}"); shift 2 ;;
    --select) select=${2:-}; shift 2 ;;
    --files) files=${2:-}; shift 2 ;;
    --timeout) timeout=${2:-}; shift 2 ;;
    --out) out=${2:-}; shift 2 ;;
    -h|--help) sed -n '2,/^# Exit 0/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}" >&2; exit 2 ;;
    *) echo "ns-gui.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ "$(uname -s)" = Darwin ] || { echo "ns-gui.sh: macOS only" >&2; exit 2; }
[ ${#scales[@]} -gt 0 ] || scales=(1 2)
for s in "${scales[@]}"; do case $s in 1|2) ;; *) echo "ns-gui.sh: --scale is 1 or 2" >&2; exit 2 ;; esac; done

work=$(mktemp -d "${TMPDIR:-/tmp}/ns-gui.XXXXXX") || exit 1
display_pid=
cleanup() {
  [ -z "$display_pid" ] || kill "$display_pid" 2>/dev/null
  rm -r "$work" 2>/dev/null
}
trap cleanup EXIT

if [ -n "$tarball" ]; then
  [ -f "$tarball" ] || { echo "ns-gui.sh: no tarball $tarball" >&2; exit 2; }
  mkdir "$work/app" && tar -C "$work/app" -xJf "$tarball" || { echo "ns-gui.sh: cannot unpack $tarball" >&2; exit 1; }
  prefix=$(echo "$work"/app/nidhogg-*)
fi
[ -n "$prefix" ] || { echo "ns-gui.sh: --tarball or --prefix is required" >&2; exit 2; }
emacs=$prefix/Nidhogg.app/Contents/MacOS/Emacs
[ -x "$emacs" ] || { echo "ns-gui.sh: $emacs not found" >&2; exit 2; }
[ -n "$out" ] || out=${TMPDIR:-/tmp}/ns-gui
mkdir -p "$out" || exit 1

clang -fobjc-arc -framework AppKit -framework CoreGraphics -o "$work/ns-display" "$self/ns-display.m" ||
  { echo "ns-gui.sh: cannot build ns-display.m" >&2; exit 1; }

# a run's time limit: macOS has no timeout(1); perl's alarm ends the run like a kill
limited() { local secs=$1; shift; perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$secs" "$@"; }

status=0
for scale in "${scales[@]}"; do
  log=$out/ns-gui-scale$scale.log
  ready=$work/display-$scale
  echo "ns-gui.sh: scale $scale"
  "$work/ns-display" --scale "$scale" --ready "$ready" >"$out/ns-display-scale$scale.log" 2>&1 &
  display_pid=$!
  for _ in $(seq 1 200); do
    [ -f "$ready" ] && break
    kill -0 "$display_pid" 2>/dev/null || break
    sleep 0.1
  done
  if [ ! -f "$ready" ]; then
    echo "ns-gui.sh: the scale $scale display did not come up:"; cat "$out/ns-display-scale$scale.log"
    status=1; kill "$display_pid" 2>/dev/null; display_pid=; continue
  fi
  read -r dx dy dw dh dscale <"$ready"
  echo "ns-gui.sh: display $dx,$dy ${dw}x${dh} scale $dscale"
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
  if [ "$rc" != 0 ]; then echo "ns-gui.sh: scale $scale: exit $rc"; status=1; fi
done
exit $status
