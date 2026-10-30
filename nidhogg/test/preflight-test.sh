#!/usr/bin/env bash
# Tests preflight.sh with stub runners (no build): the stamp's form, and no stamp on a red, on a kill, or on a dirty tree.
# Usage: nidhogg/test/preflight-test.sh   Exit 0 ok, 1 with "FAIL: <why>".
set -u
here=$(cd "$(dirname "$0")" && pwd)
pf=$here/preflight.sh
rc=0
fail() { echo "FAIL: $*"; rc=1; }
d=$(mktemp -d) || exit 1
trap 'rm -rf "$d"' EXIT
# a tree to stamp
git init -q "$d/repo" && git -C "$d/repo" -c user.name=t -c user.email=t@t commit -q --allow-empty -m x
sha=$(git -C "$d/repo" rev-parse HEAD)
printf 'src/a-tests\nsrc/b-tests\n' > "$d/files.txt"
cat > "$d/ok" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
cat > "$d/failp3" <<'EOF'
#!/usr/bin/env bash
# PASS TARGET WRAP SEED: P3 of b-tests with seed 2 fails
[ "$1" = P3 ] && [ "$2" = src/b-tests ] && [ "$4" = 2 ] && { echo "boom"; exit 1; }
exit 0
EOF
cat > "$d/slow" <<'EOF'
#!/usr/bin/env bash
# P3 hangs (to be killed); the others pass
[ "$1" = P3 ] && { sleep 30; }
exit 0
EOF
cat > "$d/harness" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "$d/ok" "$d/failp3" "$d/slow" "$d/harness"
pre() { PREFLIGHT_ROOT="$d/repo" PREFLIGHT_RUNNER="$1" PREFLIGHT_HARNESS="$d/harness" PREFLIGHT_TMP="$d" \
  bash "$pf" --build-dir "$d/b" --files "$d/files.txt" --seeds 3 --runs 1 "${@:2}"; }

# all green: a stamp in the form tooling/ci-round.sh reads, and a copy in --stamp-dir
pre "$d/ok" --out "$d/r1/report" --stamp-dir "$d/stamps" > /dev/null; r=$?
[ "$r" = 0 ] || fail "green run exited $r"
[ "$(cat "$d/r1/report.stamp" 2>/dev/null | cut -d' ' -f1,2)" = "$sha P1:skipped,P2,P3,P4,P5:skipped" ] \
  || fail "stamp form: $(cat "$d/r1/report.stamp" 2>/dev/null)"
[ "$(awk '{print NF}' "$d/r1/report.stamp")" = 3 ] || fail "stamp is not <commit> <passes> <date>"
cmp -s "$d/r1/report.stamp" "$d/stamps/$sha.stamp" || fail "no copy in --stamp-dir"

# no-stamp-on-failure: P3 red on one seed; the report names the seed and the replay line
mkdir -p "$d/r2"; echo "old" > "$d/r2/report.stamp"
pre "$d/failp3" --out "$d/r2/report" --stamp-dir "$d/stamps2" > /dev/null; r=$?
[ "$r" = 1 ] || fail "a red run exited $r"
[ ! -e "$d/r2/report.stamp" ] || fail "a stamp after a red P3 (or an older one left)"
[ ! -e "$d/stamps2/$sha.stamp" ] || fail "a stamp copy after a red"
grep -q 'RED P3 src/b-tests wrap=none seed=2' "$d/r2/report" || fail "no red line with its seed"
grep -q 'NIDHOGG_TEST_SEED=2 ' "$d/r2/report" || fail "no replay line with the seed"

# no-stamp-on-failure: killed mid-run (TERM, then KILL)
for sig in TERM KILL; do
  mkdir -p "$d/r3$sig"
  pre "$d/slow" --out "$d/r3$sig/report" --stamp-dir "$d/stamps3" > /dev/null 2>&1 &
  pid=$!
  sleep 1.5; pkill -"$sig" -P "$pid" sleep 2>/dev/null; kill -"$sig" "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  [ ! -e "$d/r3$sig/report.stamp" ] || fail "a stamp after $sig mid-run"
  [ ! -e "$d/stamps3/$sha.stamp" ] || fail "a stamp copy after $sig mid-run"
done

# the real group path, through a fake slot: every group's results are in the report, not only the reds
cat > "$d/fakeslot" <<'EOF'
#!/usr/bin/env bash
# stands for the heavy slot and the build: a check log per listed target, except $FAKE_SKIP; $FAKE_HANG: never returns
b="" l=""
while [ $# -gt 0 ]; do case $1 in --build-dir) b=$2; shift 2 ;; --check) l=$2; shift 2 ;; *) shift ;; esac; done
if [ -n "${FAKE_HANG:-}" ]; then echo $$ > "$FAKE_HANG"; exec sleep 30; fi
while IFS= read -r t; do
  [ "$t" != "${FAKE_SKIP:-}" ] || continue
  mkdir -p "$b/check-logs/$(dirname "$t")"
  echo "Ran 7 tests, 7 results as expected, 0 unexpected (2026-10-30 11:49:44+0000, 1.5 sec)" > "$b/check-logs/$t.log"
done < "$FAKE_ROOT/$l"
EOF
chmod +x "$d/fakeslot"
grp() { PREFLIGHT_SLOT="$d/fakeslot" FAKE_ROOT="$d/repo" pre "" --passes P2 "$@"; }
grp --out "$d/r5/report" > /dev/null; r=$?
[ "$r" = 0 ] && [ -e "$d/r5/report.stamp" ] || fail "a green group run exited $r"
for w in cpu1 cpu2 quota; do
  for t in src/a-tests src/b-tests; do
    grep -q "^GROUP P2 $w run=1 seed=- $t green: Ran 7 tests, 7 results as expected, 0 unexpected" "$d/r5/report" \
      || fail "no green result of $t in group $w"
  done
  grep -q "^P2 $w: 2 green, 0 red of 2$" "$d/r5/report" || fail "no summary of group $w"
done
# a file with no log from this run was not tested: its older log (r5's) must not pass for it
FAKE_SKIP=src/b-tests grp --out "$d/r7/report" > /dev/null; r=$?
[ "$r" = 1 ] && [ ! -e "$d/r7/report.stamp" ] || fail "a file that did not run still got a stamp ($r)"
grep -q "^GROUP P2 cpu1 run=1 seed=- src/b-tests NOT RUN" "$d/r7/report" || fail "no NOT RUN line"
grep -q "^P2 cpu1: 1 green, 1 red of 2$" "$d/r7/report" || fail "a file not run counted as green"
# a killed group leaves no list file in the tree (the list is untracked, for the desktop sync)
mkdir -p "$d/r8"
FAKE_HANG=$d/hang.pid grp --out "$d/r8/report" > /dev/null 2>&1 &
pid=$!
for _ in $(seq 1 50); do [ -s "$d/hang.pid" ] && break; sleep 0.1; done
ls "$d/repo"/preflight-chk.* > /dev/null 2>&1 || fail "the hanging group has no list file to clean up"
# bash runs the trap when its foreground child returns: signal preflight.sh itself, then end the fake slot
pkill -TERM -f "$d/r8/report"; kill "$(cat "$d/hang.pid")" 2>/dev/null; wait "$pid" 2>/dev/null
ls "$d/repo"/preflight-chk.* > /dev/null 2>&1 && fail "a killed run left its list file"
ls "$d/r8"/report.* > /dev/null 2>&1 && fail "a killed run left its temporary files"

# P5: a red of P2 goes through the recorder once, however many runs were red, in its own pin, with the options rr needs
cat > "$d/fakeslot-rr" <<'EOF'
#!/usr/bin/env bash
b="" l=""
while [ $# -gt 0 ]; do case $1 in --build-dir) b=$2; shift 2 ;; --check) l=$2; shift 2 ;; *) shift ;; esac; done
if [ -n "${NIDHOGG_CHECK_RR:-}" ]; then
  echo "$NIDHOGG_CHECK_RR|$NIDHOGG_BUILD_PODMAN_ARGS|$(cat "$FAKE_ROOT/$l")" >> "$FAKE_RR"
  t=$(cat "$FAKE_ROOT/$l"); mkdir -p "$b/check-logs/$(dirname "$t")"
  echo "Ran 7 tests, 7 results as expected, 0 unexpected (2026-10-30 11:49:44+0000, 1.5 sec)" > "$b/check-logs/$t.log"
  [ -z "${FAKE_NO_TRACE:-}" ] || exit 0
  [ -z "${FAKE_RR_ABORT:-}" ] || echo "[FATAL src/record_syscall.cc:6758:rec_process_syscall_arch()]" >> "$b/check-logs/$t.log"
  echo /build/rr-traces/emacs-0 > "$b/check-logs/rr-trace.log"     # what build.sh writes
  exit 0
fi
while IFS= read -r t; do
  mkdir -p "$b/check-logs/$(dirname "$t")"
  echo "Ran 7 tests, 7 results as expected, 0 unexpected (2026-10-30 11:49:44+0000, 1.5 sec)" > "$b/check-logs/$t.log"
done < "$FAKE_ROOT/$l"
[ "${NIDHOGG_BUILD_PODMAN_ARGS:-}" != --cpuset-cpus=0 ] || echo "check-rerun: RED: src/b-tests"
exit 0
EOF
chmod +x "$d/fakeslot-rr"
PREFLIGHT_SLOT="$d/fakeslot-rr" FAKE_ROOT="$d/repo" FAKE_RR="$d/rr.calls" pre "" --passes P2,P5 --runs 2 --out "$d/r9/report" > /dev/null; r=$?
[ "$r" = 1 ] && [ ! -e "$d/r9/report.stamp" ] || fail "a red with P5 run exited $r, or got a stamp"
[ "$(wc -l < "$d/rr.calls" 2>/dev/null)" = 1 ] || fail "not one recording of the red: $(cat "$d/rr.calls" 2>/dev/null)"
[ "$(cat "$d/rr.calls" 2>/dev/null)" = "rr record --chaos -n|--cpuset-cpus=0 --cap-add=SYS_PTRACE --security-opt seccomp=unconfined --security-opt label=disable|src/b-tests" ] \
  || fail "the recording's command or options: $(cat "$d/rr.calls" 2>/dev/null)"
grep -q "^P5 src/b-tests wrap=cpu1 seed=-: trace $d/b/rr-traces/emacs-0$" "$d/r9/report" || fail "no trace line in the report"
# an older run's trace name never stands for this one; a recorder that aborted is said so
rm -f "$d/rr.calls"
PREFLIGHT_SLOT="$d/fakeslot-rr" FAKE_ROOT="$d/repo" FAKE_RR="$d/rr.calls" FAKE_NO_TRACE=1 pre "" --passes P2,P5 --runs 1 --out "$d/r10/report" > /dev/null
grep -q "^P5 src/b-tests wrap=cpu1 seed=-: no trace" "$d/r10/report" || fail "an old trace name passed for this run's"
PREFLIGHT_SLOT="$d/fakeslot-rr" FAKE_ROOT="$d/repo" FAKE_RR="$d/rr.calls" FAKE_RR_ABORT=1 pre "" --passes P2,P5 --runs 1 --out "$d/r11/report" > /dev/null
grep -q "^P5 src/b-tests wrap=cpu1 seed=-: trace .*rr aborted" "$d/r11/report" || fail "an aborted recording not said"

# a dirty tree gets none
echo x > "$d/repo/f"; git -C "$d/repo" add f
pre "$d/ok" --out "$d/r4/report" > /dev/null; r=$?
[ "$r" = 1 ] && [ ! -e "$d/r4/report.stamp" ] || fail "a stamp for a dirty tree ($r)"

[ "$rc" = 0 ] && echo "preflight-test: ok"
exit $rc
