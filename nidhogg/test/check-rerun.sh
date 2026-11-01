#!/usr/bin/env bash
# The rerun step of `build.sh --stage check`: a test file with unexpected results is run once more, and a test that failed
# and then passed fails the step unless nidhogg/test/flaky.txt lists it with its cause ("a flake without a cause is fixed,
# not listed").  Same semantics as drgnx's test/flaky.eld.
#   check-rerun.sh [--flaky FILE] [--src DIR] [--make "CMD"] LOGDIR [TARGET...]
# LOGDIR  the first runs' logs, TARGET.log (build.sh writes them to $B/check-logs: `make -C test TARGET` itself keeps none).
# TARGET  a test file as `make -C test` names it (src/nidhogg-tests/watchdog-tests); without targets every log in LOGDIR
#         with unexpected results is a target.
# --flaky FILE   rows `NAME # cause; stress run` (default: flaky.txt beside this script).
# --src DIR      the tree of the .el test files, to check that every listed name exists once (default: LOGDIR).
# --make CMD     how a target is run again: CMD TARGET (default: make -C LOGDIR).
# Prints `flaky (listed: CAUSE)` for a listed flake.  Exit 0: no unlisted flake, no red, no stale or ambiguous row.
set -u
here=$(cd "$(dirname "$0")" && pwd)
flaky=$here/flaky.txt src="" mk=""
while [ $# -gt 0 ]; do
  case $1 in
    --flaky) flaky=$2; shift 2 ;;
    --src) src=$2; shift 2 ;;
    --make) mk=$2; shift 2 ;;
    *) break ;;
  esac
done
[ $# -ge 1 ] || { echo "usage: check-rerun.sh [--flaky FILE] [--src DIR] [--make CMD] LOGDIR [TARGET...]" >&2; exit 2; }
logdir=$1; shift
[ -n "$src" ] || src=$logdir
[ -n "$mk" ] || mk="make -C $logdir"
rc=0
red() { echo "check-rerun: RED: $*"; rc=1; }

# the flaky list: NAME -> cause
declare -A cause
if [ -f "$flaky" ]; then
  while IFS= read -r line; do
    line=${line%%$'\r'}
    case $line in ''|'#'*) continue ;; esac
    name=${line%%#*}; name=${name%"${name##*[![:space:]]}"}; name=${name#"${name%%[![:space:]]*}"}
    c=${line#*#}; [ "$c" != "$line" ] || c=""
    c=${c#"${c%%[![:space:]]*}"}
    [ -n "$name" ] || continue
    [ -n "$c" ] || red "flaky row $name has no cause"
    cause[$name]=$c
  done < "$flaky"
fi

# every listed name must be defined in exactly one test file
for name in "${!cause[@]}"; do
  n=$(grep -rlF --include='*.el' -e "(ert-deftest $name " -e "(ert-deftest $name)" "$src" 2>/dev/null | sort -u | wc -l)
  [ "$n" -ge 1 ] || red "flaky row $name names no test (stale row)"
  [ "$n" -le 1 ] || red "flaky row $name names a test defined in $n files"
done

# the targets with unexpected results
targets=()
if [ $# -gt 0 ]; then
  for t in "$@"; do targets+=("$t"); done
else
  while IFS= read -r f; do
    grep -qE '[0-9]+ unexpected' "$f" 2>/dev/null || continue
    f=${f#"$logdir"/}; targets+=("${f%.log}")
  done < <(find "$logdir" -name '*.log' 2>/dev/null | sort)
fi

for t in "${targets[@]}"; do
  log=$logdir/$t.log
  # a target that passed has a log without unexpected results; one with none at all (it did not start) is red
  if [ -f "$log" ] && ! grep -qE '^Ran [0-9]+ tests' "$log"; then
    red "$t: no ERT summary in its log (the run ended early)"; continue
  fi
  if [ -f "$log" ] && grep -qE '^Ran [0-9]+ tests, [0-9]+ results as expected, 0 unexpected' "$log" ; then
    continue
  fi
  [ -f "$log" ] || { red "$t: no log"; continue; }
  failed=$(grep -E '^ +FAILED +' "$log" | sed -E 's/^ +FAILED +//; s/ +$//' | sort -u)
  echo "check-rerun: $t had unexpected results; running it once more"
  if $mk "$t" > "$logdir/.rerun.out" 2>&1; then
    if [ -z "$failed" ]; then red "$t: failed, then passed, and the log names no test to list"; continue; fi
    while IFS= read -r name; do
      if [ -n "${cause[$name]+x}" ]; then
        echo "flaky (listed: ${cause[$name]}): $name"
      else
        red "$name failed, then passed on rerun, and is not in flaky.txt (find the cause and fix it, or list it with the cause)"
      fi
    done <<< "$failed"
  else
    red "$t: failed twice"
    tail -n 40 "$logdir/.rerun.out"
  fi
done
exit $rc
