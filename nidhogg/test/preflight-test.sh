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

# a dirty tree gets none
echo x > "$d/repo/f"; git -C "$d/repo" add f
pre "$d/ok" --out "$d/r4/report" > /dev/null; r=$?
[ "$r" = 1 ] && [ ! -e "$d/r4/report.stamp" ] || fail "a stamp for a dirty tree ($r)"

[ "$rc" = 0 ] && echo "preflight-test: ok"
exit $rc
