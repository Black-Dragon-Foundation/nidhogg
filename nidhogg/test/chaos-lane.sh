#!/usr/bin/env bash
# CI's workers-chaos job: the files of chaos-files.txt on a chaos build (`build.sh --dev --chaos`: seeded delays at the
# workers' sync points, src/nidhogg-chaos.h), all of them in one check container per seed, NIDHOGG_TEST_SEED=SEED.
#   chaos-lane.sh --build-dir DIR [--seeds "1 2 3 4 5"] -- BUILD...
# BUILD is the build command without --build-dir, --stage and --check (CI: `bash nidhogg/build.sh --container fedora-45
# --dev --chaos --memory 12g --jobs 4`); its `--stage build` made DIR.  Run from the tree's root.
# No rerun lets anything off: a red under chaos is a race, not a flake.  A seed is red when the check stage fails, when
# a file had an unexpected result in its first run (check-rerun.sh's second run and a flaky.txt row do not count), or
# when a file left no ERT summary from this run or ran no test.  Each red names its seed and the line that replays it.
# Exit 0 green, 1 red, 2 usage.
set -u
usage() { sed -n '4p' "$0" >&2; exit 2; }
build="" seeds="1 2 3 4 5" list=nidhogg/test/chaos-files.txt
while [ $# -gt 0 ]; do
  case $1 in
    --build-dir) build=${2:?}; shift 2 ;;
    --seeds) seeds=${2:?}; shift 2 ;;
    --) shift; break ;;
    *) usage ;;
  esac
done
[ -n "$build" ] && [ $# -gt 0 ] || usage
[ -f "$list" ] || { echo "chaos-lane: no $list: run from the tree's root" >&2; exit 2; }
mkdir -p "$build"
mark=$(mktemp); trap 'rm -f "$mark"' EXIT
targets=$(sed 's/#.*//' "$list" | awk 'NF')
reds=""
say_red() { # SEED WHY
  if [ "${GITHUB_ACTIONS:-}" = true ]; then echo "::error title=Chaos seed $1::$2"; else echo "chaos-lane: RED seed $1: $2"; fi
}
for s in $seeds; do
  log=$build/chaos-seed-$s.log red=0
  # this run's logs only: the last seed's are removed, and a log must be newer than the run's start (file times sit on
  # coarse ticks, so the mark is a second early)
  for t in $targets; do rm -f "$build/check-logs/$t.log"; done
  touch -d "@$(( $(date +%s) - 1 ))" "$mark"
  echo "== chaos seed $s"
  NIDHOGG_TEST_SEED=$s "$@" --build-dir "$build" --stage check --check "$list" > "$log" 2>&1; rc=$?
  replay="NIDHOGG_TEST_SEED=$s $* --build-dir $build --stage check --check $list"
  if [ "$rc" != 0 ]; then red=1; say_red "$s" "the check stage failed (exit $rc); replay: $replay"; fi
  for t in $targets; do
    tlog=$build/check-logs/$t.log sum=""
    [ -f "$tlog" ] && [ "$tlog" -nt "$mark" ] && sum=$(grep -E '^Ran [0-9]+ tests' "$tlog" | tail -n 1)
    if [ -z "$sum" ]; then red=1; say_red "$s" "$t: no ERT summary from this run; replay: $replay"
    elif [[ $sum != *" 0 unexpected"* ]]; then red=1; say_red "$s" "$t: $sum; replay: $replay"
    elif [[ $sum == "Ran 0 tests"* ]]; then red=1; say_red "$s" "$t ran no test; replay: $replay"
    else echo "chaos-lane: seed $s $t green: $sum"; fi
  done
  # a second run that passed: check-rerun.sh lets a listed flake off, chaos does not
  if grep -qE '^check-rerun: |^flaky \(listed' "$log"; then
    red=1; say_red "$s" "a file failed its first run ($(grep -E '^check-rerun: |^flaky \(listed' "$log" | head -n 3 | tr '\n' ' ')); replay: $replay"
  fi
  if [ "$red" = 1 ]; then reds="$reds $s"; echo "--- seed $s: the check stage's last lines"; tail -n 60 "$log"; fi
done
[ -z "$reds" ] || { echo "chaos-lane: red under seed(s)$reds"; exit 1; }
echo "chaos-lane: green under seed(s) $seeds"
