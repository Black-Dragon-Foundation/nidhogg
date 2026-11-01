#!/usr/bin/env bash
# The timing lane (report-only): the timing test files, judged against the runner's own speed.
#   timing-lane.sh (--build-dir D | --prefix P) --report F [--rows FILE] [--samples N] [--container DISTRO] [--runner EXE]
# --build-dir D: a sample is `build.sh --stage check` on that build dir (the desktop); --prefix P: an installed Nidhogg
# (P/bin/nidhogg) runs the test file in a batch Emacs of its own (the CI job, in a container on Linux, natively on macOS).
#   timing-lane.sh --self-test
# Each row of timing-rows.txt (`TEST-FILE BUDGET-RATIO`, a file under test/src/nidhogg-tests/) takes N (20) samples,
# one run of the file each, and is judged on the MEDIAN of the samples' times divided by the calibration: the time
# timing-calibration-tests.el (a fixed ~1 s workload) takes on this runner, run before and after the row.  Rules:
#   - the two calibrations differ by over 25 %: the attempt is void and retried once; void twice prints `void`, never red;
#   - a median over BUDGET-RATIO, or over a quarter of the samples failing their own assertions, is a red;
#   - a red reruns once with fresh calibrations; a pass on the rerun is `flaky`, two reds are `red`.
# F gets one line per row: `ROW VERDICT median=X ratio=R calibration=C fails=K/N (note)`, VERDICT pass|flaky|red|void.
# Exit 0 always while the lane is report-only (2 on a usage error); the verdict is in F.
# A sample is EXE TARGET (default: build.sh --stage check on the build dir, the time read from the file's ERT log), which
# prints the seconds the run took on stdout and exits 0 when the file passed.
set -u
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
rows="$here/timing-rows.txt" samples=20 distro=fedora-45 runner="" build="" report="" self=0 prefix=""
while [ $# -gt 0 ]; do
  case $1 in
    --build-dir) build=${2:?}; shift 2 ;;
    --prefix) prefix=${2:?}; shift 2 ;;
    --report) report=${2:?}; shift 2 ;;
    --rows) rows=${2:?}; shift 2 ;;
    --samples) samples=${2:?}; shift 2 ;;
    --container) distro=${2:?}; shift 2 ;;
    --runner) runner=${2:?}; shift 2 ;;
    --self-test) self=1; shift ;;
    *) sed -n '2,4p' "$0" >&2; exit 2 ;;
  esac
done

CALIB=timing-calibration-tests

# one sample of TARGET (a file name without .el): prints seconds, returns the file's status
default_sample() {
  local t=$1 lst rc log secs
  if [ -n "$prefix" ]; then   # an installed Nidhogg (the CI job): the file in a batch Emacs of its own
    local bin=$prefix/bin/nidhogg
    [ "$(uname -s)" != Darwin ] || bin=$prefix/Nidhogg.app/Contents/MacOS/Emacs
    log=$(mktemp); "$bin" --batch -Q -L "$root/test/src" -l ert -l "$root/test/src/nidhogg-tests/$t.el" \
      --eval '(ert-run-tests-batch-and-exit (quote (not (tag :unstable))))' > "$log" 2>&1; rc=$?
    secs=$(sed -nE 's/^Ran [0-9]+ tests.*, ([0-9.]+) sec\).*/\1/p' "$log" | tail -n 1)
    rm -f "$log"; echo "${secs:-0}"; return "$rc"
  fi
  lst=$(mktemp "$root/timing-chk.XXXXXX.txt"); echo "src/nidhogg-tests/$t" > "$lst"    # untracked: the desktop sync carries it
  "$root/nidhogg/build.sh" --container "$distro" --stage check --build-dir "$build" --check "${lst#"$root"/}" > /dev/null 2>&1; rc=$?
  rm -f "$lst"
  log=$build/check-logs/src/nidhogg-tests/$t.log
  secs=$(sed -nE 's/^Ran [0-9]+ tests.*, ([0-9.]+) sec\).*/\1/p' "$log" 2>/dev/null | tail -n 1)
  echo "${secs:-0}"
  return "$rc"
}
sample() { if [ -n "$runner" ]; then "$runner" "$1"; else default_sample "$1"; fi; }

median() { sort -g | awk '{a[NR]=$1} END {if (NR == 0) {print 0} else if (NR % 2) {print a[(NR+1)/2]} else {printf "%.6f\n", (a[NR/2]+a[NR/2+1])/2}}'; }
calib() { sample "$CALIB" 2>/dev/null | tail -n 1; }

# one attempt at a row: prints "VERDICT MEDIAN RATIO CAL FAILS" (VERDICT pass|red|void)
attempt() { # ROW BUDGET
  local row=$1 budget=$2 c1 c2 cal i s rc fails=0 vals="" med ratio dev
  c1=$(calib)
  for ((i = 0; i < samples; i++)); do
    s=$(sample "$row" 2>/dev/null); rc=$?
    s=$(tail -n 1 <<<"$s")
    [ "$rc" = 0 ] || fails=$((fails + 1))
    vals+="${s:-0}"$'\n'
  done
  c2=$(calib)
  dev=$(awk -v a="$c1" -v b="$c2" 'BEGIN {m = (a < b) ? a : b; if (m <= 0) print 99; else printf "%.4f", ((a > b ? a : b) - m) / m}')
  cal=$(awk -v a="$c1" -v b="$c2" 'BEGIN {printf "%.4f", (a + b) / 2}')
  med=$(printf '%s' "$vals" | median)
  ratio=$(awk -v m="$med" -v c="$cal" 'BEGIN {if (c <= 0) print 0; else printf "%.3f", m / c}')
  if awk -v d="$dev" 'BEGIN {exit !(d > 0.25)}'; then echo "void $med $ratio $cal $fails"; return; fi
  if awk -v r="$ratio" -v b="$budget" 'BEGIN {exit !(r > b)}' || [ $((fails * 4)) -gt "$samples" ]; then echo "red $med $ratio $cal $fails"
  else echo "pass $med $ratio $cal $fails"; fi
}

judge_row() { # ROW BUDGET -> one report line
  local row=$1 budget=$2 v med ratio cal fails note=""
  read -r v med ratio cal fails < <(attempt "$row" "$budget")
  if [ "$v" = void ]; then
    note="void once: calibrations over 25% apart, retried"
    read -r v med ratio cal fails < <(attempt "$row" "$budget")
    if [ "$v" = void ]; then echo "$row void median=$med ratio=$ratio calibration=$cal fails=$fails/$samples (void twice: the runner's speed moved over 25% during the run; not a failure)"; return; fi
  fi
  if [ "$v" = red ]; then
    note="${note:+$note; }red once, rerun with fresh calibration"
    read -r v med ratio cal fails < <(attempt "$row" "$budget")
    case $v in
      pass) v=flaky ;;
      void) note="$note; the rerun was void"; v=red ;;
    esac
  fi
  echo "$row $v median=$med ratio=$ratio calibration=$cal fails=$fails/$samples (budget $budget${note:+; $note})"
}

run_lane() {
  : > "$report"
  while read -r row budget _; do
    [ -n "$row" ] || continue
    judge_row "$row" "$budget" >> "$report"
  done < <(sed 's/#.*//' "$rows" | awk 'NF')
  cat "$report"
}

# --- self-test: fake samples ---------------------------------------------------------------------------------------------
self_test() {
  local d rc=0 out
  d=$(mktemp -d) || return 1
  trap 'rm -rf "$d"' RETURN
  # the fake runner reads its script from $d/script: lines `TARGET SECONDS STATUS`, consumed in order per call
  cat > "$d/run" <<'EOF'
#!/usr/bin/env bash
# the script has one line per call, in order: SECONDS STATUS
d=$(dirname "$0"); n=$(cat "$d/n" 2>/dev/null || echo 1)
line=$(sed -n "${n}p" "$d/script"); echo $((n + 1)) > "$d/n"
[ -n "$line" ] || line="1.0 0"
set -- $line; echo "$1"; exit "$2"
EOF
  chmod +x "$d/run"
  printf 'row-a 2.0\n' > "$d/rows"
  try() { # name expected-verdict ; script on stdin
    cat > "$d/script"; rm -f "$d/n"
    out=$(bash "$0" --runner "$d/run" --rows "$d/rows" --samples 4 --report "$d/rep"); local r=$?
    if [ "$r" != 0 ] || ! grep -q "^row-a $2 " <<<"$out"; then echo "FAIL: $1: wanted $2, got (exit $r): $out"; rc=1; fi
  }
  # a calibration is 1 call, then 4 samples, then a calibration: 6 calls an attempt
  # pass: calibrations 1.0 and 1.1, samples near 1.5 (ratio < 2.0)
  try pass pass <<'EOF'
1.0 0
1.5 0
1.5 0
1.4 0
1.6 0
1.1 0
EOF
  # void twice: calibrations 1.0 and 2.0 both attempts
  try void-twice void <<'EOF'
1.0 0
1.5 0
1.5 0
1.5 0
1.5 0
2.0 0
1.0 0
1.5 0
1.5 0
1.5 0
1.5 0
2.0 0
EOF
  # void, then a clean pass on the retry
  try void-then-pass pass <<'EOF'
1.0 0
1.5 0
1.5 0
1.5 0
1.5 0
2.0 0
1.0 0
1.5 0
1.5 0
1.5 0
1.5 0
1.0 0
EOF
  # red, then pass on the rerun: flaky
  try red-then-pass flaky <<'EOF'
1.0 0
3.0 0
3.0 0
3.0 0
3.0 0
1.0 0
1.0 0
1.5 0
1.5 0
1.5 0
1.5 0
1.0 0
EOF
  # red twice
  try red-twice red <<'EOF'
1.0 0
3.0 0
3.0 0
3.0 0
3.0 0
1.0 0
1.0 0
3.0 0
3.0 0
3.0 0
3.0 0
1.0 0
EOF
  # a median in budget but two of four samples failing their own assertions: red
  try failing-samples red <<'EOF'
1.0 0
1.5 1
1.5 1
1.5 0
1.5 0
1.0 0
1.0 0
1.5 1
1.5 1
1.5 0
1.5 0
1.0 0
EOF
  [ "$rc" != 0 ] || echo "timing-lane self-test: ok"
  return "$rc"
}

if [ "$self" = 1 ]; then self_test; exit $?; fi
[ -n "$report" ] || { sed -n '2,4p' "$0" >&2; exit 2; }
[ -n "$runner" ] || [ -n "$build" ] || [ -n "$prefix" ] || { echo "timing-lane: --build-dir, --prefix or --runner needed" >&2; exit 2; }
run_lane
exit 0
