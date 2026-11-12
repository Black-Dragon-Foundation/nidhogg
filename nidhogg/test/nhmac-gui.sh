#!/usr/bin/env bash
# Nidhogg's GUI test lane on macOS (the NS build today, Naglfar when it links): the built Nidhogg.app is run directly
# from this shell, never through `open', on a virtual display of the asked scale (mac-display.m), and
# test/src/nidhogg-tests/gui-runner.el runs nhmac-gui-tests.el in it.  Copied from the Glitnir P5 lane (ns-gui.sh,
# ns-display.m), renamed, with a row floor added.
#
#   nidhogg/test/nhmac-gui.sh --tarball FILE | --prefix DIR | --emacs FILE [--scale 1|2]... [--select REGEXP] [--files GLOB]
#                          [--timeout SEC] [--row-timeout SEC] [--out DIR] [--floor N] [--interpose] [--app DIR]
#                          [--drgnx DIR [--drgnx-strict]]
#
#   --tarball FILE   the macOS release tarball (nidhogg-VERSION-macos-ARCH.tar.xz); unpacked under $TMPDIR
#   --prefix DIR     an unpacked tarball (DIR/Nidhogg.app/Contents/MacOS/Emacs)
#   --emacs FILE     an Emacs binary run as it is: a Naglfar build tree's src/emacs (no bundle yet)
#   --app DIR        the installed Naglfar bundle (nextstep/Emacs.app after `make install'): the rows of nhmac-app-gui-tests.el
#                    start it (NHMAC_APP); without it they skip
#   --scale S        the display scale, 1 or 2; repeatable (default: 1, then 2)
#   --select REGEXP  ERT selector, an Emacs regexp (alternation is \|); default every test of the files
#   --files GLOB     the test files, in test/src/nidhogg-tests: space-separated globs, run in that order (default: the
#                    fixed set and order of nidhogg/test/nhmac-lane.txt, its fresh rows first)
#   --timeout SEC    end a session after SEC seconds (default 3600; a backstop: --row-timeout bounds each row)
#   --row-timeout SEC  a row still running after SEC seconds (default 240) is ended: it is reported FAILED, with that
#                    reason, and a new session runs the rows not yet reported.  A row that ends Emacs (a crash) is one
#                    red row the same way.  So a hung row (a modal panel, a wait that never returns) fails itself,
#                    never the rest of the lane
#   --floor N        the fewest rows a run may report: a run that reports 0 rows, or fewer than N, is red.  The default is
#                    the lane's floor (`lane_floor' below) for a run of the whole lane (no --select, --files or --drgnx),
#                    else 1
#   --interpose      build nidhogg/test/mac-interpose (cargo, on the runner) and run Emacs with it inserted: the
#                    counters nidhogg/test/mac-counts.el reads (NHMAC_INTERPOSE_LIB names the dylib)
#   --drgnx DIR      drgnx's fit-check row instead of Nidhogg's rows: DIR is drgnx at the commit Nidhogg pins; its hub
#                    is built here (cargo build --locked --release, aarch64-apple-darwin, with drgnx's test hooks and
#                    ratatoskr-mock beside it, as drgnx's build-test-bins action builds them) and named by
#                    DRGNX_RATATOSKR_BIN, the fixture fonts (DIR/test/fixtures/fonts) are registered for each session
#                    (GLITNIR_GUI_FONTS: gui-runner, Core Text's process scope; nothing is installed), and
#                    DIR/test/ui/rows-fit-gui-test.el runs with drgnx's harness, core and wb on the load path
#                    (default scale 2).  Until that commit has the file, the
#                    row is reported skipped, with that reason; --drgnx-strict makes a missing file red (the combined
#                    gate pass)
#   --out DIR        where the logs go (default $TMPDIR/nhmac-gui); one nhmac-gui-scaleS.log per scale, and the pixels of a
#                    pixel row whose captures differ (GLITNIR_GUI_OUT, nhmac-gui-tests.el)
#
# The Screen Recording permission the pixel rows need is the runner's grant to /bin/bash, which the app inherits as a
# child of this script; `open' would make the app its own responsible process, without it.  Without the grant the pixel
# rows skip (glitnir-no-capture) and the rest run.  HOME is a temporary directory; nothing of the caller's is touched.
# The rows of nhmac-lane.txt's `fresh' lines run first, each alone in a fresh Emacs (the timing and heap rows: no row
# before them leaves anything live); then one session runs the files' rows, in the fixed order (gui-runner.el).
# Before any row, one Emacs opens a frame, redisplays and exits (bounded at 60 s): when it cannot, the lane stops at
# once with its output and the macOS crash report, never running sessions that each die before a row.  A session that
# ends on a signal prints the crash reports written meanwhile (~/Library/Logs/DiagnosticReports/*.ips, the caller's
# real home: ReportCrash ignores HOME), which name the faulting thread, its frames and an abort's message.
# Exit 0 all rows as expected, 1 a failure, too few rows, a timeout, a display that did not come up or a first frame
# that did not open, 2 usage.
set -u

self=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
checkout=$(cd "$self/../.." && pwd)

# The fewest rows the whole lane reports per scale (a skipped row is reported too).  Counted on 2026-11-12 as the
# `(ert-deftest' forms at the start of a line in the files nhmac-lane.txt lists (nidhogg/test/ci-test.sh counts them
# the same way and fails when this is above the count): 232 then, so 200 leaves room for a file that loses rows and
# is far above a pass that ran one file, or none.
lane_floor=200

tarball= prefix= emacs= scales=() select= files= timeout=3600 rtimeout=240 out= floor= interpose=0 app= drgnx= dstrict=0
while [ $# -gt 0 ]; do
  case $1 in
    --tarball) tarball=${2:-}; shift 2 ;;
    --prefix) prefix=${2:-}; shift 2 ;;
    --emacs) emacs=${2:-}; shift 2 ;;
    --scale) scales+=("${2:-}"); shift 2 ;;
    --select) select=${2:-}; shift 2 ;;
    --files) files=${2:-}; shift 2 ;;
    --timeout) timeout=${2:-}; shift 2 ;;
    --row-timeout) rtimeout=${2:-}; shift 2 ;;
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
if [ -z "$floor" ]; then
  if [ -z "$select$files$drgnx" ]; then floor=$lane_floor; else floor=1; fi
fi
case $floor in ''|*[!0-9]*) echo "nhmac-gui.sh: --floor is a number" >&2; exit 2 ;; esac
case $rtimeout in ''|*[!0-9]*) echo "nhmac-gui.sh: --row-timeout is a number" >&2; exit 2 ;; esac
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
# the fixed file set and order, and the rows that run first in fresh sessions (unless --files names the files)
fresh=()
if [ -z "$files" ]; then
  while read -r kind rest; do
    case $kind in
      fresh) fresh+=("$rest") ;;
      files) files="${files:+$files }$rest" ;;
    esac
  done < <(sed 's/#.*//' "$self/nhmac-lane.txt")
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

# the names of the rows a log reports (passed, FAILED or skipped), one per line
reported() {
  sed -nE 's/^(passed|FAILED|skipped) ([^ ]+).*$/\2/p' "$1"
}

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


if [ "$interpose" = 1 ]; then
  cargo build --release --manifest-path "$checkout/nidhogg/test/mac-interpose/Cargo.toml" --target-dir "$work/interpose" ||
    { echo "nhmac-gui.sh: cannot build the interposer" >&2; exit 1; }
  export NHMAC_INTERPOSE_LIB=$work/interpose/release/libmac_interpose.dylib
  [ -f "$NHMAC_INTERPOSE_LIB" ] || { echo "nhmac-gui.sh: no $NHMAC_INTERPOSE_LIB" >&2; exit 1; }
fi

if [ -n "$drgnx" ]; then
  cargo build --locked --release --target aarch64-apple-darwin --manifest-path "$drgnx/runtime/Cargo.toml" \
        -p drgnx-ratatoskr -p domr --features drgnx-ratatoskr/test-hooks --bin ratatoskr --bin ratatoskr-mock \
        --target-dir "$work/hub" || { echo "nhmac-gui.sh: cannot build drgnx's hub" >&2; exit 1; }
  export DRGNX_RATATOSKR_BIN=$work/hub/aarch64-apple-darwin/release/ratatoskr
  for b in "$DRGNX_RATATOSKR_BIN" "${DRGNX_RATATOSKR_BIN%/*}/ratatoskr-mock"; do
    [ -x "$b" ] || { echo "nhmac-gui.sh: no $b" >&2; exit 1; }
  done
  # the fixture fonts, registered for each session by gui-runner (Core Text's process scope; nothing is installed)
  GLITNIR_GUI_FONTS=$(ls "$drgnx"/test/fixtures/fonts/*.ttf "$drgnx"/test/fixtures/fonts/*.otf 2>/dev/null | paste -sd: -)
  [ -n "$GLITNIR_GUI_FONTS" ] || { echo "nhmac-gui.sh: no fixture fonts in $drgnx/test/fixtures/fonts" >&2; exit 1; }
  export GLITNIR_GUI_FONTS
  # drgnx's test helpers, then Emacs's own directories (the empty element)
  export EMACSLOADPATH=$drgnx/harness:$drgnx/core:$drgnx/wb:
fi

# the crash reports macOS wrote after marker file $1 (at most 2, each cut at 40 KB), waiting up to 10 s for the first
reports_dir="$HOME/Library/Logs/DiagnosticReports"
crash_reports() {
  local found= _
  for _ in $(seq 1 20); do
    found=$(find "$reports_dir" -maxdepth 1 -name '*.ips' -newer "$1" 2>/dev/null | head -n 2)
    [ -n "$found" ] && break
    sleep 0.5
  done
  [ -n "$found" ] || { echo "nhmac-gui.sh: no crash report in $reports_dir"; return; }
  while IFS= read -r f; do
    echo "nhmac-gui.sh: crash report $f:"
    head -c 40000 "$f"; echo
  done <<<"$found"
}

status=0
smoked=0
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
  : >"$log"
  # the first frame, once per lane: a build that cannot open one stops everything now, with the reason
  if [ "$smoked" = 0 ]; then
    smoked=1
    : >"$work/crash-mark"
    env HOME="$work/home-$scale" RUST_BACKTRACE=1 \
      "$emacs" -Q --eval '(progn (redisplay t) (sit-for 0.5) (kill-emacs 0))' >"$work/first-frame.log" 2>&1 &
    pid=$! rc=
    for _ in $(seq 1 60); do kill -0 "$pid" 2>/dev/null || break; sleep 1; done
    if kill -0 "$pid" 2>/dev/null; then
      kill -KILL "$pid" 2>/dev/null; wait "$pid" 2>/dev/null; rc=timeout
    else
      rc=0; wait "$pid" 2>/dev/null || rc=$?
    fi
    if [ "$rc" != 0 ]; then
      echo "nhmac-gui.sh: the first frame did not open (exit $rc); the lane stops:" | tee -a "$log"
      tee -a "$log" <"$work/first-frame.log"
      crash_reports "$work/crash-mark" | tee -a "$log"
      kill "$display_pid" 2>/dev/null; wait "$display_pid" 2>/dev/null
      exit 1
    fi
    echo "nhmac-gui.sh: the first frame opened"
  fi
  done_file=$work/done-$scale
  : >"$done_file"
  # the fresh rows first, each in its own session; then the main session, again after a row that hung or ended Emacs
  queue=(${fresh[@]+"${fresh[@]}"} "")
  sessions=0
  while [ ${#queue[@]} -gt 0 ]; do
    only=${queue[0]}
    sessions=$((sessions + 1))
    if [ "$sessions" -gt 40 ]; then echo "nhmac-gui.sh: scale $scale: 40 sessions, the lane stops" | tee -a "$log"; status=1; break; fi
    # the main session leaves out the fresh rows: they are reported, or ran in their own session
    [ -n "$only" ] || [ ${#fresh[@]} = 0 ] || printf '%s\n' "${fresh[@]}" >>"$done_file"
    slog=$work/session.log
    : >"$slog"
    : >"$work/crash-mark"
    # perl execs Emacs, so $! is Emacs's pid (perl is a system binary: dyld strips DYLD_* from its environment, so
    # DYLD_INSERT_LIBRARIES is set here, after it started).  The time limits are the watch loop's below, never an
    # alarm: Emacs handles SIGALRM itself (its atimers), so an alarm inherited through exec never ends it
    env HOME="$work/home-$scale" RUST_BACKTRACE=1 \
      GLITNIR_GUI_SCALE=$scale GLITNIR_GUI_COMPOSITOR=windowserver \
      GLITNIR_GUI_KNOWN=$checkout/test/src/nidhogg-tests/gui-known-differences.eld \
      GLITNIR_GUI_TESTS=$tests GLITNIR_GUI_FILES="$files" GLITNIR_GUI_SELECT=$select \
      GLITNIR_GUI_ONLY="$only" GLITNIR_GUI_DONE="$done_file" \
      GLITNIR_GUI_OUT=$out \
      NHMAC_APP=$app \
      GLITNIR_GUI_FRAME="$((dx + 20)) $((dy + 40)) 100 35" \
      perl -e '$ENV{DYLD_INSERT_LIBRARIES} = $ENV{NHMAC_INTERPOSE_LIB} if $ENV{NHMAC_INTERPOSE_LIB}; exec @ARGV or die "exec: $!"' \
        "$emacs" -Q -l "$checkout/test/src/nidhogg-tests/gui-runner.el" >"$slog" 2>&1 &
    pid=$!
    # the bounds: the open row (started, not reported) for longer than --row-timeout, or the session for longer than
    # --timeout, ends the session; the open row is the red one
    open= since=$SECONDS hung= began=$SECONDS why=
    while kill -0 "$pid" 2>/dev/null; do
      sleep 1
      cur=$(sed -n 's/^started //p' "$slog" | tail -n 1)
      if [ $((SECONDS - began)) -ge "$timeout" ]; then
        why="the session ran past the lane's bound of $timeout s"
        if [ -n "$cur" ] && ! reported "$slog" | grep -qxF -- "$cur"; then hung=$cur; else hung=-; fi
        kill -TERM "$pid" 2>/dev/null; sleep 2; kill -KILL "$pid" 2>/dev/null
        break
      fi
      if [ -n "$cur" ] && ! reported "$slog" | grep -qxF -- "$cur"; then
        [ "$cur" = "$open" ] || { open=$cur since=$SECONDS; }
        if [ $((SECONDS - since)) -ge "$rtimeout" ]; then
          hung=$cur why="the row ran past the lane's bound of $rtimeout s"
          kill -TERM "$pid" 2>/dev/null; sleep 2; kill -KILL "$pid" 2>/dev/null
          break
        fi
      else
        open=
      fi
    done
    rc=0
    wait "$pid" 2>/dev/null || rc=$?
    if [ "$hung" = - ]; then
      echo "nhmac-gui.sh: scale $scale: $why, between rows" >>"$slog"; status=1
    elif [ -n "$hung" ]; then
      echo "FAILED $hung" >>"$slog"
      echo "  ($why: ended, and the lane goes on in a new session)" >>"$slog"
    fi
    # a session that ended inside a row (a crash, an abort): that row is one red row
    if ! grep -q '^Ran [0-9]* tests' "$slog"; then
      last=$(sed -n 's/^started //p' "$slog" | tail -n 1)
      if [ -n "$last" ] && ! reported "$slog" | grep -qxF -- "$last"; then
        echo "FAILED $last" >>"$slog"
        echo "  (Emacs ended inside the row: exit $rc)" >>"$slog"
      fi
    fi
    # a session that ended on a signal (exit 128 + N, not the lane's own kill): what macOS recorded
    [ "$rc" -gt 128 ] && [ -z "$hung" ] && crash_reports "$work/crash-mark" >>"$slog"
    cat "$slog" | tee -a "$log"
    reported "$slog" >>"$done_file"
    # the session's own end: the next fresh row or the main session; an unfinished main session runs again
    if grep -q '^Ran [0-9]* tests' "$slog" || [ -n "$only" ] || [ -z "$(sed -n 's/^started //p' "$slog")" ]; then
      queue=(${queue[@]+"${queue[@]:1}"})
    fi
    if [ "$rc" != 0 ] && [ -z "$hung" ] && ! grep -q '^Ran [0-9]* tests' "$slog" && [ -z "$(sed -n 's/^started //p' "$slog")" ]; then
      echo "nhmac-gui.sh: scale $scale: a session ended before any row: exit $rc"; status=1
    fi
  done
  kill "$display_pid" 2>/dev/null; wait "$display_pid" 2>/dev/null; display_pid=
  ran=$(reported "$log" | wc -l | tr -d ' ')
  bad=$(grep -c '^FAILED ' "$log")
  echo "nhmac-gui.sh: scale $scale: $ran rows in $sessions sessions, $bad unexpected" | tee -a "$log"
  [ "$bad" = 0 ] || status=1
  if [ "${ran:-0}" -lt "$floor" ]; then echo "nhmac-gui.sh: scale $scale: ${ran:-0} rows ran, the floor is $floor"; status=1; fi
done
exit $status
