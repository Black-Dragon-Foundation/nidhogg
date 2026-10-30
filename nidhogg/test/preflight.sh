#!/usr/bin/env bash
# The pre-flight: the passes that find ordering bugs on the desktop, before a CI round.
#   preflight.sh --build-dir D --out REPORT [--prefix P] [--seeds N] [--runs N] [--files FILE] [--passes P2,P3,..]
#                [--stamp-dir DIR] [--sched-modules "M..."] [--tsan-modules "M..."]
# Run it from the nidhogg worktree root on the laptop, not itself in a slot: every build run takes a heavy slot of its own
# (drgnx-heavy-slot.sh nidhogg-build-local.sh, the way nh-verify.sh does; git does not work inside a slot's jail on the
# desktop, so a script run there cannot call the build wrapper).  Passes:
#   P1  the D files (nidhogg/test/preflight-d.txt) twice, unpinned and pinned to 1 CPU: the same results.  An empty list
#       skips with a note (P1:skipped in the stamp).
#   P2  the files of preflight-files.txt (workers, process, I/O, watchdog) pinned to 1 CPU, to 2 CPUs and under a
#       podman --cpus=0.5 (cgroup cpu.max 50000 100000), --runs (3) each, through NIDHOGG_BUILD_PODMAN_ARGS.  A run whose test file only passed on
#       check-rerun.sh's second run counts as a red: the pre-flight is where a flake is found.  The report has a GROUP line
#       per file per run (green, RED or NOT RUN, with its ERT summary) and a `P2 GROUP: N green, M red of K` line per group.
#   P3  the same files on the chaos build (D-chaos, built with `build.sh --chaos` when missing), --seeds (200) seeds
#       each, NIDHOGG_TEST_SEED=1..N.  A red prints its seed and the replay line.
#   P4  nidhogg/test/workers-harness.sh --tsan and --sched.
#   P5  P2's and P3's reds again under `rr record --chaos`, when rr exists (otherwise a note; the recording inside the
#       build container is not wired, so P5 is recorded as skipped).
# REPORT gets the log.  Only when every run passed, REPORT.stamp holds one line `<commit> <passes> <date>` (a skipped pass
# is written `P1:skipped`) and, with --stamp-dir, a copy goes to DIR/<commit>.stamp (what tooling/ci-round.sh reads).
# Exit 0 only with a stamp.  A stamp is written last, to a temp name and renamed; an interrupt or a kill never leaves one,
# and a dirty tree gets none.
# Test hook: PREFLIGHT_RUNNER=EXE replaces the build runs: EXE PASS TARGET WRAP SEED (WRAP none|cpu1|cpu2|quota; SEED a
# number or -) exits 0 when green; PREFLIGHT_HARNESS=EXE replaces workers-harness.sh.  Exit: 0 ok, 1 a red, 2 usage.
set -u
here=$(cd "$(dirname "$0")" && pwd)
root=${PREFLIGHT_ROOT:-$(cd "$here/../.." && pwd)}   # PREFLIGHT_ROOT: the tree whose commit is stamped (the test hook)
T=${PREFLIGHT_TMP:-/home/upmanyu/workspace/black-dragon-foundation/.claude/tmp}
build="" out="" prefix="" seeds=200 runs=3 files="$here/preflight-files.txt" stampdir="" want="P1,P2,P3,P4,P5"
sched="" tsan="queue wake io"
slot=${PREFLIGHT_SLOT:-$T/drgnx-heavy-slot.sh}
usage() { sed -n '2,5p' "$0" >&2; exit 2; }
while [ $# -gt 0 ]; do
  case $1 in
    --build-dir) build=${2:?}; shift 2 ;;
    --out) out=${2:?}; shift 2 ;;
    --prefix) prefix=${2:?}; shift 2 ;;
    --seeds) seeds=${2:?}; shift 2 ;;
    --runs) runs=${2:?}; shift 2 ;;
    --files) files=${2:?}; shift 2 ;;
    --passes) want=${2:?}; shift 2 ;;
    --stamp-dir) stampdir=${2:?}; shift 2 ;;
    --sched-modules) sched=${2:?}; shift 2 ;;
    --tsan-modules) tsan=${2:?}; shift 2 ;;
    *) usage ;;
  esac
done
[ -n "$build" ] && [ -n "$out" ] || usage
mkdir -p "$(dirname "$out")"
rm -f "$out.stamp"          # an earlier run's stamp never stands for this one
: > "$out"
commit=$(git -C "$root" rev-parse HEAD 2>/dev/null || echo unknown)
dirty=$(git -C "$root" status --porcelain --untracked-files=no 2>/dev/null | head -1)
say() { echo "$*" | tee -a "$out"; }
reds=0
declare -a done_passes=()       # P2, P3:skipped, ...
redlist=()                      # "PASS|TARGET|WRAP|SEED" for P5
runlst="" runlog="" runmark=""   # the current group's temporary files; a kill must not leave the list in the tree
trap 'rm -f "$out.stamp" "$out.stamp.tmp" "$runlst" "$runlog" "$runmark"; say "preflight: interrupted: no stamp"; exit 130' INT TERM HUP
declare -A group_green=() group_red=()   # "PASS WRAP" -> files; the summary line of each group
run_no=0                                 # which of the group's --runs this is
group_add() { # PASS WRAP green|red
  local k="$1 $2"
  [ "$3" = green ] && group_green[$k]=$((${group_green[$k]:-0} + 1)) || group_red[$k]=$((${group_red[$k]:-0} + 1))
}
group_summary() { # PASS WRAP...: one line per group, what it ran, not only what failed
  local p=$1 w g r; shift
  for w in "$@"; do
    g=${group_green["$p $w"]:-0} r=${group_red["$p $w"]:-0}
    say "$p $w: $g green, $r red of $((g + r))"
  done
}

targets() { sed 's/#.*//' "$1" 2>/dev/null | awk 'NF'; }

# --- one run of one test file -----------------------------------------------------------------------------------------
replay_line() { # TARGET WRAP SEED
  local pre=""
  [ "$3" = - ] || pre="NIDHOGG_TEST_SEED=$3 "
  case $2 in cpu1) pre="${pre}NIDHOGG_BUILD_PODMAN_ARGS=--cpuset-cpus=0 " ;; cpu2) pre="${pre}NIDHOGG_BUILD_PODMAN_ARGS=--cpuset-cpus=0,1 " ;;
    quota) pre="${pre}NIDHOGG_BUILD_PODMAN_ARGS=--cpus=0.5 " ;; esac
  local dir=$build; [ "$3" = - ] || dir=$build-chaos
  echo "${pre}$T/nidhogg-build-local.sh ${prefix:+--prefix $prefix }--build-dir $dir --stage check --check <a file listing $1>"
}
note_red() { # PASS TARGET WRAP SEED LOG
  say "RED $1 $2 wrap=$3 seed=$4"
  say "  replay: $(replay_line "$2" "$3" "$4")"
  { echo "--- $1 $2 wrap=$3 seed=$4 ---"; tail -n 40 "$5"; } >> "$out"
  redlist+=("$1|$2|$3|$4")
  reds=$((reds + 1))
}
run_one() { # PASS TARGET WRAP SEED -> 0 green  (the stub runner of the test hook: one file a run)
  local log rc
  log=$(mktemp "${out}.run.XXXXXX")
  "$PREFLIGHT_RUNNER" "$1" "$2" "$3" "$4" > "$log" 2>&1; rc=$?
  if [ "$rc" = 0 ]; then say "GROUP $1 $3 run=$run_no seed=$4 $2 green"; group_add "$1" "$3" green
  else say "GROUP $1 $3 run=$run_no seed=$4 $2 RED"; group_add "$1" "$3" red; note_red "$1" "$2" "$3" "$4" "$log"; fi
  rm -f "$log"
  return "$rc"
}
# All the files of the list in one container run (the container's package install costs more than a short test file):
# a file is red when its target is named in the run's FAILED lines or in check-rerun.sh's lines (a failure, or a pass only
# on the second run: that is a find here), and the whole run is red when it failed without naming one.  Every file gets a
# GROUP line with its ERT summary.  A file with no check log newer than this run's start was not tested (check-logs/ keeps
# older runs' logs, and a run that dies early names nothing): it is red, or its last result would pass for this run's.
run_group() { # PASS WRAP SEED [LIST]
  local pass=$1 wrap=$2 seed=$3 list=${4:-$files} t log rc dir=$build lst named=0 pargs="" tlog sum red
  if [ -n "${PREFLIGHT_RUNNER:-}" ]; then
    while IFS= read -r t; do run_one "$pass" "$t" "$wrap" "$seed"; done < <(targets "$list")
    return 0
  fi
  [ "$seed" = - ] || dir=$build-chaos
  log=$(mktemp "${out}.run.XXXXXX"); runlog=$log
  runmark=$(mktemp "${out}.mark.XXXXXX")
  lst=$(mktemp "$root/preflight-chk.XXXXXX.txt"); runlst=$lst; targets "$list" > "$lst"      # untracked, so the desktop sync carries it
  # the pin is a podman option of the check container (taskset on the podman client does not reach the container)
  case $wrap in cpu1) pargs="--cpuset-cpus=0" ;; cpu2) pargs="--cpuset-cpus=0,1" ;; quota) pargs="--cpus=0.5" ;; esac
  local cmd=("$slot" "$T/nidhogg-build-local.sh" ${prefix:+--prefix "$prefix"} --build-dir "$dir" --stage check --check "${lst#"$root"/}")
  if [ "$seed" = - ]; then NIDHOGG_BUILD_PODMAN_ARGS=$pargs "${cmd[@]}" > "$log" 2>&1; rc=$?
  else NIDHOGG_BUILD_PODMAN_ARGS=$pargs NIDHOGG_TEST_SEED=$seed "${cmd[@]}" > "$log" 2>&1; rc=$?; fi
  rm -f "$lst"; runlst=""
  while IFS= read -r t; do
    tlog=$dir/check-logs/$t.log red=0 sum=""
    if [ -f "$tlog" ] && [ "$tlog" -nt "$runmark" ]; then sum=$(sed -nE 's/^(Ran [0-9]+ tests.*) \(.*, ([0-9.]+) sec\).*/\1, \2 s/p' "$tlog" | tail -n 1); fi
    if grep -qF -e "make -C test $t " -e "check-rerun: $t " -e "check-rerun: RED: $t" "$log"; then
      red=1; say "GROUP $pass $wrap run=$run_no seed=$seed $t RED: ${sum:-no ERT summary}"
    elif [ -z "$sum" ]; then
      red=1; say "GROUP $pass $wrap run=$run_no seed=$seed $t NOT RUN: no ERT summary from this run"
    else
      say "GROUP $pass $wrap run=$run_no seed=$seed $t green: $sum"
    fi
    if [ "$red" = 1 ]; then group_add "$pass" "$wrap" red; note_red "$pass" "$t" "$wrap" "$seed" "$log"; named=1
    else group_add "$pass" "$wrap" green; fi
  done < <(targets "$list")
  if [ "$named" = 0 ] && { [ "$rc" != 0 ] || grep -qE '^flaky \(listed' "$log"; }; then
    note_red "$pass" "(whole run)" "$wrap" "$seed" "$log"
  fi
  rm -f "$log" "$runmark"; runlog="" runmark=""
}
want_pass() { case ",$want," in *",$1,"*) return 0 ;; *) return 1 ;; esac; }
finish_pass() { # PASS NOTE  (a pass counts when it ran with no red; skipped is recorded as such)
  done_passes+=("$1")
}

# --- the passes ------------------------------------------------------------------------------------------------------
if want_pass P1; then
  d="$here/preflight-d.txt"
  if [ -z "$(targets "$d")" ]; then
    say "P1 skipped: no D files yet (nidhogg/test/preflight-d.txt is empty)"; done_passes+=("P1:skipped")
  else
    before=$reds
    run_group P1 none - "$d"; run_group P1 none - "$d"; run_group P1 cpu1 - "$d"
    [ "$reds" = "$before" ] && done_passes+=(P1) || say "P1: reds"
  fi
fi
if want_pass P2; then
  say "P2: pinned runs ($runs each)"; before=$reds
  for w in cpu1 cpu2 quota; do for ((run_no = 1; run_no <= runs; run_no++)); do run_group P2 "$w" -; done; done
  group_summary P2 cpu1 cpu2 quota
  [ "$reds" = "$before" ] && done_passes+=(P2) || say "P2: reds"
fi
if want_pass P3; then
  before=$reds
  if [ -z "${PREFLIGHT_RUNNER:-}" ] && [ ! -d "$build-chaos" ]; then
    say "P3: building the chaos build in $build-chaos"
    "$slot" "$T/nidhogg-build-local.sh" ${prefix:+--prefix "$prefix"} --chaos --build-dir "$build-chaos" --stage build >> "$out" 2>&1 \
      || { say "P3: the chaos build failed"; reds=$((reds + 1)); }
  fi
  say "P3: $seeds seeds per file on the chaos build"
  for ((s = 1; s <= seeds; s++)); do run_group P3 none "$s"; done
  [ "$reds" = "$before" ] && done_passes+=(P3) || say "P3: reds"
fi
if want_pass P4; then
  say "P4: workers-harness --tsan and --sched"; before=$reds
  h=${PREFLIGHT_HARNESS:-$here/workers-harness.sh}
  # shellcheck disable=SC2086
  "$h" --tsan $tsan >> "$out" 2>&1 || { say "RED P4 --tsan $tsan"; reds=$((reds + 1)); }
  # shellcheck disable=SC2086
  "$h" --sched $sched --seeds "$seeds" >> "$out" 2>&1 || { say "RED P4 --sched $sched --seeds $seeds"; reds=$((reds + 1)); }
  [ "$reds" = "$before" ] && done_passes+=(P4) || say "P4: reds"
fi
if want_pass P5; then
  if [ "${#redlist[@]}" = 0 ]; then say "P5 skipped: no reds to record"
  elif ! command -v rr >/dev/null 2>&1; then say "P5 skipped: rr is not installed (${#redlist[@]} reds not recorded)"
  else say "P5 skipped: rr is installed, but recording inside the build container is not wired (${#redlist[@]} reds not recorded)"; fi
  done_passes+=("P5:skipped")
fi

# --- the verdict -----------------------------------------------------------------------------------------------------
if [ "$reds" != 0 ]; then say "preflight: $reds red; no stamp"; exit 1; fi
if [ -n "$dirty" ]; then say "preflight: green, but the tree has uncommitted changes; no stamp"; exit 1; fi
[ "$commit" != unknown ] || { say "preflight: no commit; no stamp"; exit 1; }
passes=$(IFS=,; echo "${done_passes[*]}")
echo "$commit $passes $(date +%Y-%m-%d)" > "$out.stamp.tmp" && mv "$out.stamp.tmp" "$out.stamp"
if [ -n "$stampdir" ]; then mkdir -p "$stampdir" && cp "$out.stamp" "$stampdir/$commit.stamp"; fi
say "preflight: green; stamp $(cat "$out.stamp")"
exit 0
