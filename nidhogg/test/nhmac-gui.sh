#!/usr/bin/env bash
# Nidhogg's GUI test lane on macOS (the NS build today, Naglfar when it links): the built Nidhogg.app is run directly
# from this shell, never through `open', on a virtual display of the asked scale (mac-display.m), and
# test/src/nidhogg-tests/gui-runner.el runs nhmac-gui-tests.el in it.  Copied from the Glitnir P5 lane (ns-gui.sh,
# ns-display.m), renamed, with a row floor added.
#
#   nidhogg/test/nhmac-gui.sh --tarball FILE | --prefix DIR | --emacs FILE [--scale 1|2]... [--select REGEXP] [--files GLOB]
#                          [--timeout SEC] [--out DIR] [--floor N] [--interpose] [--app DIR]
#                          [--drgnx DIR [--drgnx-strict]]
#
#   --tarball FILE   the macOS release tarball (nidhogg-VERSION-macos-ARCH.tar.xz); unpacked under $TMPDIR
#   --prefix DIR     an unpacked tarball (DIR/Nidhogg.app/Contents/MacOS/Emacs)
#   --emacs FILE     an Emacs binary run as it is: a Naglfar build tree's src/emacs (no bundle yet)
#   --app DIR        the installed Naglfar bundle (nextstep/Emacs.app after `make install'): the rows of nhmac-app-gui-tests.el
#                    start it (NHMAC_APP); without it they skip
#   --scale S        the display scale, 1 or 2; repeatable (default: 1, then 2)
#   --select REGEXP  ERT selector, an Emacs regexp (alternation is \|); default every test of the files
#   --files GLOB     the test files, in test/src/nidhogg-tests (default nhmac-*gui-tests.el)
#   --timeout SEC    kill a run after SEC seconds (default 900)
#   --floor N        the fewest rows a run may report (default 1): a run that reports 0 rows, or fewer than N, is red
#   --interpose      build nidhogg/test/mac-interpose (cargo, on the runner) and run Emacs with it inserted: the
#                    counters nidhogg/test/mac-counts.el reads (NHMAC_INTERPOSE_LIB names the dylib)
#   --drgnx DIR      drgnx's fit-check row instead of Nidhogg's rows: DIR is drgnx at the commit Nidhogg pins; its hub
#                    is built here (cargo build --locked --release, aarch64-apple-darwin, as drgnx's install builds
#                    it) and named by DRGNX_RATATOSKR_BIN, and DIR/test/ui/rows-fit-gui-test.el runs with drgnx's
#                    harness, core and wb on the load path (default scale 2).  Until that commit has the file, the
#                    row is reported skipped, with that reason; --drgnx-strict makes a missing file red (the combined
#                    gate pass)
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

tarball= prefix= emacs= scales=() select= files='nhmac-*gui-tests.el' timeout=900 out= floor=1 interpose=0 app= drgnx= dstrict=0
while [ $# -gt 0 ]; do
  case $1 in
    --tarball) tarball=${2:-}; shift 2 ;;
    --prefix) prefix=${2:-}; shift 2 ;;
    --emacs) emacs=${2:-}; shift 2 ;;
    --scale) scales+=("${2:-}"); shift 2 ;;
    --select) select=${2:-}; shift 2 ;;
    --files) files=${2:-}; shift 2 ;;
    --timeout) timeout=${2:-}; shift 2 ;;
    --out) out=${2:-}; shift 2 ;;
    --floor) floor=${2:-}; shift 2 ;;
    --interpose) interpose=1; shift ;;
    --drgnx) drgnx=${2:-}; shift 2 ;;
    --drgnx-strict) dstrict=1; shift ;;
    --app) app=${2:-}; shift 2 ;;
    -h|--help) sed -n '2,/^# Exit 0/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}" >&2; exit 2 ;;
    *) echo "nhmac-gui.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
case $floor in ''|*[!0-9]*) echo "nhmac-gui.sh: --floor is a number" >&2; exit 2 ;; esac
[ "$(uname -s)" = Darwin ] || { echo "nhmac-gui.sh: macOS only" >&2; exit 2; }
tests=$checkout/test/src/nidhogg-tests
if [ -n "$drgnx" ]; then
  [ -d "$drgnx" ] || { echo "nhmac-gui.sh: no drgnx checkout $drgnx" >&2; exit 2; }
  if [ ! -f "$drgnx/test/ui/rows-fit-gui-test.el" ]; then
    echo "nhmac-gui.sh: drgnx's fit-check row: skipped: $drgnx/test/ui/rows-fit-gui-test.el is missing (the pinned drgnx commit predates it)"
    exit "$dstrict"
  fi
  tests=$drgnx/test/ui files=rows-fit-gui-test.el
  [ ${#scales[@]} -gt 0 ] || scales=(2)
fi
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
if [ -z "$emacs" ]; then
  [ -n "$prefix" ] || { echo "nhmac-gui.sh: --tarball, --prefix or --emacs is required" >&2; exit 2; }
  emacs=$prefix/Nidhogg.app/Contents/MacOS/Emacs
fi
[ -x "$emacs" ] || { echo "nhmac-gui.sh: $emacs not found" >&2; exit 2; }
[ -n "$out" ] || out=${TMPDIR:-/tmp}/nhmac-gui
mkdir -p "$out" || exit 1

clang -fobjc-arc -framework AppKit -framework CoreGraphics -o "$work/mac-display" "$self/mac-display.m" ||
  { echo "nhmac-gui.sh: cannot build mac-display.m" >&2; exit 1; }

# a run's time limit: macOS has no timeout(1); perl's alarm ends the run like a kill
# (perl is a system binary: dyld strips DYLD_* from its environment, so DYLD_INSERT_LIBRARIES is set here, after it started)
limited() {
  local secs=$1; shift
  perl -e '$ENV{DYLD_INSERT_LIBRARIES} = $ENV{NHMAC_INTERPOSE_LIB} if $ENV{NHMAC_INTERPOSE_LIB}; alarm shift; exec @ARGV or die "exec: $!"' "$secs" "$@"
}

if [ "$interpose" = 1 ]; then
  cargo build --release --manifest-path "$checkout/nidhogg/test/mac-interpose/Cargo.toml" --target-dir "$work/interpose" ||
    { echo "nhmac-gui.sh: cannot build the interposer" >&2; exit 1; }
  export NHMAC_INTERPOSE_LIB=$work/interpose/release/libmac_interpose.dylib
  [ -f "$NHMAC_INTERPOSE_LIB" ] || { echo "nhmac-gui.sh: no $NHMAC_INTERPOSE_LIB" >&2; exit 1; }
fi

if [ -n "$drgnx" ]; then
  cargo build --locked --release --target aarch64-apple-darwin --manifest-path "$drgnx/runtime/Cargo.toml" \
        -p drgnx-ratatoskr --target-dir "$work/hub" || { echo "nhmac-gui.sh: cannot build drgnx's hub" >&2; exit 1; }
  export DRGNX_RATATOSKR_BIN=$work/hub/aarch64-apple-darwin/release/ratatoskr
  [ -x "$DRGNX_RATATOSKR_BIN" ] || { echo "nhmac-gui.sh: no $DRGNX_RATATOSKR_BIN" >&2; exit 1; }
  # drgnx's test helpers, then Emacs's own directories (the empty element)
  export EMACSLOADPATH=$drgnx/harness:$drgnx/core:$drgnx/wb:
fi

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
  GLITNIR_GUI_KNOWN=$checkout/test/src/nidhogg-tests/gui-known-differences.eld \
  GLITNIR_GUI_TESTS=$tests GLITNIR_GUI_FILES=$files GLITNIR_GUI_SELECT=$select \
  GLITNIR_GUI_OUT=$out \
  NHMAC_APP=$app \
  GLITNIR_GUI_FRAME="$((dx + 20)) $((dy + 40)) 100 35" \
    limited "$timeout" "$emacs" -Q -l "$checkout/test/src/nidhogg-tests/gui-runner.el" >"$log" 2>&1 || rc=$?
  cat "$log"
  kill "$display_pid" 2>/dev/null; wait "$display_pid" 2>/dev/null; display_pid=
  if [ "$rc" != 0 ]; then echo "nhmac-gui.sh: scale $scale: exit $rc"; status=1; fi
  # every row run is counted: 0 rows, or fewer than the floor, is red even when the runner exited 0
  ran=$(sed -n 's/^.*Ran \([0-9][0-9]*\) tests.*$/\1/p' "$log" | tail -n 1)
  # Emacs ended inside a row (no summary): the rows that reported count, and the row it ended in is one red row
  if [ -z "$ran" ]; then
    ran=$(grep -cE '^(passed|FAILED|skipped) ' "$log")
    open=$(sed -n 's/^started //p' "$log" | tail -n 1)
    if [ -n "$open" ] && ! grep -qE "^(passed|FAILED|skipped) $open\$" "$log"; then
      echo "FAILED $open" | tee -a "$log"
      echo "  (Emacs ended inside the row: exit $rc)" | tee -a "$log"
      ran=$((ran + 1))
    fi
    echo "nhmac-gui.sh: scale $scale: no summary; $ran rows reported"
  fi
  if [ "${ran:-0}" -lt "$floor" ]; then echo "nhmac-gui.sh: scale $scale: ${ran:-0} rows ran, the floor is $floor"; status=1; fi
done
exit $status
