#!/usr/bin/env bash
# Tests nhmac-gui.sh's sessions with stand-ins (no Mac, no Emacs): uname says Darwin, clang builds a display that only
# writes its ready file, and the Emacs is a script that "runs" rows named by a fixture.  It checks that the fresh rows
# run first, each alone; that a row that hangs is ended at the row bound and fails alone; that a row that ends Emacs
# fails alone; and that the lane goes on with the rows not yet reported, none run twice.
# Usage: nidhogg/test/nhmac-gui-test.sh [LANE]   (LANE: the script to test, default nhmac-gui.sh next to this one)
# Exit 0 ok, 1 with "FAIL: <why>".
set -u
here=$(cd "$(dirname "$0")" && pwd)
lane=${1:-$here/nhmac-gui.sh}
rc=0
fail() { echo "FAIL: $*"; rc=1; }
d=$(mktemp -d) || exit 1
trap 'rm -rf "$d"' EXIT
mkdir -p "$d/bin" "$d/co/nidhogg/test" "$d/co/test/src/nidhogg-tests"
cp "$lane" "$d/co/nidhogg/test/nhmac-gui.sh"
touch "$d/co/nidhogg/test/mac-display.m" "$d/co/test/src/nidhogg-tests/gui-runner.el"
cat > "$d/co/nidhogg/test/nhmac-lane.txt" <<'EOF'
# fixture
fresh p1
fresh p2
files a.el
EOF
cat > "$d/bin/uname" <<'EOF'
#!/bin/sh
echo Darwin
EOF
# clang -o OUT ...: OUT is the display, which writes its ready file and waits
cat > "$d/bin/clang" <<'EOF'
#!/bin/sh
while [ $# -gt 0 ]; do [ "$1" = -o ] && out=$2; shift; done
printf '#!/bin/sh\nwhile [ $# -gt 0 ]; do [ "$1" = --ready ] && r=$2; shift; done\necho 0 0 800 600 1 > "$r"\nexec sleep 600\n' > "$out"
chmod +x "$out"
EOF
# the Emacs: the rows p1 p2 a b c d e, skipping those GLITNIR_GUI_DONE lists; c hangs, e ends Emacs; every row it
# starts is logged to $d/ran
cat > "$d/emacs" <<EOF
#!/bin/bash
rows="p1 p2 a b c d e f"
[ -z "\$GLITNIR_GUI_ONLY" ] || rows=\$GLITNIR_GUI_ONLY
n=0
for r in \$rows; do
  grep -qx "\$r" "\$GLITNIR_GUI_DONE" 2>/dev/null && continue
  echo "\$r \${GLITNIR_GUI_ONLY:-main}" >> "$d/ran"
  echo "started \$r" >&2
  case \$r in c) exec sleep 600 ;; e) exit 6 ;; esac
  echo "passed \$r (0.1 s)" >&2
  n=\$((n + 1))
done
echo "Ran \$n tests, \$n as expected (0 skipped), 0 unexpected" >&2
EOF
chmod +x "$d/bin/uname" "$d/bin/clang" "$d/emacs"

PATH=$d/bin:$PATH TMPDIR=$d timeout 120 bash "$d/co/nidhogg/test/nhmac-gui.sh" --emacs "$d/emacs" --scale 1 \
  --row-timeout 3 --out "$d/out" > "$d/lane.log" 2>&1
st=$?
[ "$st" = 1 ] || fail "the lane's exit is $st, not 1 (two red rows)"
[ "$(cat "$d/ran" 2>/dev/null | tr '\n' ' ')" = "p1 p1 p2 p2 a main b main c main d main e main f main " ] ||
  fail "the rows ran as: $(cat "$d/ran" 2>/dev/null | tr '\n' ' ') (want p1 and p2 alone, then a to f once each)"
grep -qx 'FAILED c' "$d/lane.log" || fail "the hung row c is not FAILED"
grep -q "c.*row ran past the lane's bound of 3 s\|row ran past the lane's bound of 3 s" "$d/lane.log" ||
  fail "the hung row's reason is missing"
grep -qx 'FAILED e' "$d/lane.log" || fail "the row e that ended Emacs is not FAILED"
[ "$(grep -c '^FAILED ' "$d/lane.log")" = 2 ] || fail "not exactly two FAILED rows"
grep -q 'scale 1: 8 rows in 5 sessions, 2 unexpected' "$d/lane.log" || fail "the summary line is wrong"
[ "$rc" = 0 ] || { echo "--- lane log"; cat "$d/lane.log"; }
[ "$rc" = 0 ] && echo "nhmac-gui-test.sh: ok"
exit "$rc"
