#!/usr/bin/env bash
# Tests check-rerun.sh with fixture logs and a fake runner that stands in for `make -C test` (no Emacs needed): a test that
# failed in its first run (the log the test writes) and passes on the rerun (the runner).
# Usage: nidhogg/test/check-rerun-test.sh   Exit 0 ok, 1 with "FAIL: <why>".
set -u
here=$(cd "$(dirname "$0")" && pwd)
rr=$here/check-rerun.sh
rc=0
fail() { echo "FAIL: $*"; rc=1; }
d=$(mktemp -d) || exit 1
trap 'rm -rf "$d"' EXIT
mkdir -p "$d/t/src" "$d/s/src"
cat > "$d/s/src/a-tests.el" <<'EOF'
(ert-deftest a/sometimes ()
  (should t))
EOF
red_log() {
  mkdir -p "$d/t/src"
  printf 'Running 1 tests\nRan 1 tests, 0 results as expected, 1 unexpected\n\n1 unexpected results:\n   FAILED  %s\n' "$1" > "$d/t/src/a-tests.log"
}
# the fake `make -C test TARGET`: passes (or, with RUNNER_FAIL=1, fails again)
cat > "$d/runner" <<'EOF'
#!/usr/bin/env bash
if [ "${RUNNER_FAIL:-0}" = 1 ]; then
  printf 'Ran 1 tests, 0 results as expected, 1 unexpected\n   FAILED  a/sometimes\n' > "$(dirname "$0")/t/$1.log"; exit 1
fi
printf 'Ran 1 tests, 1 results as expected, 0 unexpected\n' > "$(dirname "$0")/t/$1.log"; exit 0
EOF
chmod +x "$d/runner"
run() { bash "$rr" --flaky "$d/flaky.txt" --src "$d/s" --make "$d/runner" "$d/t" "$@"; }

# unlisted flake: red
: > "$d/flaky.txt"; red_log a/sometimes
out=$(run src/a-tests); r=$?
[ "$r" = 1 ] && grep -q 'not in flaky.txt' <<<"$out" || fail "an unlisted flake was not red: $r $out"

# listed with a cause: green, and it says so
printf 'a/sometimes # a race in the fixture; stress run 40/40\n' > "$d/flaky.txt"; red_log a/sometimes
out=$(run src/a-tests); r=$?
[ "$r" = 0 ] && grep -q 'flaky (listed: a race in the fixture' <<<"$out" || fail "a listed flake was not green: $r $out"

# no targets given: found from the logs
red_log a/sometimes
out=$(run); r=$?
[ "$r" = 0 ] && grep -q 'flaky (listed' <<<"$out" || fail "targets not found from the logs: $r $out"

# a stale row: red even with nothing to rerun
printf 'a/sometimes # c; s\na/gone # c; s\n' > "$d/flaky.txt"
printf 'Ran 1 tests, 1 results as expected, 0 unexpected\n' > "$d/t/src/a-tests.log"
out=$(run src/a-tests); r=$?
[ "$r" = 1 ] && grep -q 'a/gone names no test' <<<"$out" || fail "a stale row was not red: $r $out"

# a row without a cause is red
printf 'a/sometimes\n' > "$d/flaky.txt"
out=$(run src/a-tests); r=$?
[ "$r" = 1 ] && grep -q 'has no cause' <<<"$out" || fail "a row without a cause was not red: $r $out"

# the same name in two files: red
printf 'a/sometimes # c; s\n' > "$d/flaky.txt"
cp "$d/s/src/a-tests.el" "$d/s/src/b-tests.el"
out=$(run src/a-tests); r=$?
[ "$r" = 1 ] && grep -q 'defined in 2 files' <<<"$out" || fail "a duplicate name was not red: $r $out"
rm "$d/s/src/b-tests.el"

# a failure that repeats: red, listed or not
red_log a/sometimes
out=$(RUNNER_FAIL=1 run src/a-tests); r=$?
[ "$r" = 1 ] && grep -q 'failed twice' <<<"$out" || fail "a repeated failure was not red: $r $out"

# a log with no summary (the run ended early): red
printf 'Running 1 tests\n' > "$d/t/src/a-tests.log"
out=$(run src/a-tests); r=$?
[ "$r" = 1 ] && grep -q 'no ERT summary' <<<"$out" || fail "a log without a summary was not red: $r $out"

# class D: a failing D test is not rerun and stays red (the runner would pass, and must not be called)
cat > "$d/s/src/d-tests.el" <<'EOF'
;; test-class: det
(ert-deftest d/plain ()
  (should t))
(ert-deftest d/loose ()
  :tags '(:cond)
  (should t))
EOF
cat > "$d/s/src/m-tests.el" <<'EOF'
;; test-class: cond
(ert-deftest m/waits ()
  (should t))
(ert-deftest m/exact ()
  :tags '(:det)
  (should t))
EOF
: > "$d/flaky.txt"
red_d_log() {
  printf 'Ran 2 tests, 0 results as expected, 2 unexpected\n\n2 unexpected results:\n' > "$d/t/src/$1.log"
  shift; for n in "$@"; do printf '   FAILED  %s\n' "$n" >> "$d/t/src/$LOGFILE.log"; done
}
LOGFILE=d-tests; red_d_log d-tests d/plain
rm -f "$d/ran"; printf '#!/usr/bin/env bash\n: > "%s/ran"; exit 0\n' "$d" > "$d/runner2"; chmod +x "$d/runner2"
out=$(bash "$rr" --flaky "$d/flaky.txt" --src "$d/s" --make "$d/runner2" "$d/t" src/d-tests); r=$?
[ "$r" = 1 ] && grep -q 'd/plain is a deterministic test' <<<"$out" && [ ! -e "$d/ran" ] || fail "a failing D test was rerun or not red: $r $out"

# a test's own tag beats its file's class: :cond in a D file still reruns, :det in a C file does not
LOGFILE=d-tests; red_d_log d-tests d/loose
out=$(run src/d-tests); r=$?
[ "$r" = 1 ] && grep -q 'not in flaky.txt' <<<"$out" || fail "a :cond test in a D file was not rerun: $r $out"
LOGFILE=m-tests; red_d_log m-tests m/exact
rm -f "$d/ran"
out=$(bash "$rr" --flaky "$d/flaky.txt" --src "$d/s" --make "$d/runner2" "$d/t" src/m-tests); r=$?
[ "$r" = 1 ] && grep -q 'm/exact is a deterministic test' <<<"$out" && [ ! -e "$d/ran" ] || fail "a :det test in a C file was rerun: $r $out"

# a C test in a mixed file still reruns; the D one in it stays red
LOGFILE=m-tests; red_d_log m-tests m/exact m/waits
rm -f "$d/ran"
out=$(bash "$rr" --flaky "$d/flaky.txt" --src "$d/s" --make "$d/runner2" "$d/t" src/m-tests); r=$?
[ "$r" = 1 ] && grep -q 'm/exact is a deterministic test' <<<"$out" && grep -q 'm/waits failed, then passed' <<<"$out" && [ -e "$d/ran" ] \
  || fail "a C test in a mixed file was not rerun: $r $out"
rm -f "$d/t/src/m-tests.log"

# a D test on flaky.txt fails the check, even with nothing to rerun
printf 'd/plain # c; s\n' > "$d/flaky.txt"
printf 'Ran 1 tests, 1 results as expected, 0 unexpected\n' > "$d/t/src/d-tests.log"
out=$(run src/d-tests); r=$?
[ "$r" = 1 ] && grep -q 'd/plain names a deterministic test' <<<"$out" || fail "a D test on flaky.txt was not red: $r $out"

[ "$rc" = 0 ] && echo "check-rerun-test: ok"
exit $rc
