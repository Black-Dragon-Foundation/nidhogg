#!/usr/bin/env bash
# Naglfar's paired timing (design §7): the stock NS build (A) and Naglfar (B) run a timed row (by default
# nhmac-paired-key-to-commit) in turn, ABAB, on the same virtual display, each round writing its times (µs, one
# per line); then the
# i-th time of each B round is paired with the i-th of the A round before it.  Naglfar passes when it is not slower:
# the median of the differences B - A is at most 0, B's p95 is at most A's, and a one-sided sign test does not find
# B slower (fewer than the 5% critical count of pairs with B > A, normal approximation).
#
#   nidhogg/test/nhmac-pair.sh --ns FILE --naglfar FILE [--row NAME]... [--rounds N] [--within PCT] [--out DIR]
#
#   --ns FILE        the NS build's Emacs (Nidhogg.app/Contents/MacOS/Emacs of the release tarball)
#   --naglfar FILE   Naglfar's Emacs (a build tree's src/emacs)
#   --row NAME       a timed row, judged on its own; repeatable (default nhmac-paired-key-to-commit; any of the
#                    *nhmac-gui-tests.el files); it writes GLITNIR_GUI_OUT/pair-BUILD.txt
#   --rounds N       A/B rounds (default 3)
#   --within PCT     judge each row by a stated tolerance instead: Naglfar's median at most NS's median plus PCT
#                    percent (G1's main-thread CPU per key: --within 10)
#   --out DIR        where the logs and times go (default $TMPDIR/nhmac-pair)
#
# Exit 0 not slower (every row), 1 slower or a round that wrote no times, 2 usage.
set -u

self=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ns= naglfar= rounds=3 out= rows=() within=
while [ $# -gt 0 ]; do
  case $1 in
    --ns) ns=${2:-}; shift 2 ;;
    --naglfar) naglfar=${2:-}; shift 2 ;;
    --row) rows+=("${2:-}"); shift 2 ;;
    --rounds) rounds=${2:-}; shift 2 ;;
    --within) within=${2:-}; shift 2 ;;
    --out) out=${2:-}; shift 2 ;;
    -h|--help) sed -n '2,/^# Exit 0/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}" >&2; exit 2 ;;
    *) echo "nhmac-pair.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -n "$ns" ] && [ -n "$naglfar" ] || { echo "nhmac-pair.sh: --ns and --naglfar are required" >&2; exit 2; }
case $rounds in ''|*[!0-9]*|0) echo "nhmac-pair.sh: --rounds is a positive number" >&2; exit 2 ;; esac
case $within in ''|[0-9]|[0-9][0-9]|[0-9][0-9][0-9]) ;; *) echo "nhmac-pair.sh: --within is a whole percentage" >&2; exit 2 ;; esac
[ -n "$out" ] || out=${TMPDIR:-/tmp}/nhmac-pair
mkdir -p "$out" || exit 1
[ ${#rows[@]} -gt 0 ] || rows=(nhmac-paired-key-to-commit)

status=0
for row in "${rows[@]}"; do
o=$out/$row
mkdir -p "$o" || exit 1
for i in $(seq 1 "$rounds"); do
  for b in ns naglfar; do
    if [ "$b" = ns ]; then emacs=$ns; else emacs=$naglfar; fi
    d=$o/round$i-$b
    mkdir -p "$d"
    bash "$self/nhmac-gui.sh" --emacs "$emacs" --scale 2 --files '*nhmac-gui-tests.el' --select "$row" --out "$d" ||
      { echo "nhmac-pair.sh: $row: round $i, $b: the row failed"; exit 1; }
    [ -s "$d/pair-$b.txt" ] || { echo "nhmac-pair.sh: $row: round $i, $b wrote no times"; exit 1; }
  done
  paste "$o/round$i-ns/pair-ns.txt" "$o/round$i-naglfar/pair-naglfar.txt" >>"$o/pairs.tsv"
done
echo "nhmac-pair.sh: $row"

if [ -n "$within" ]; then
  # the stated tolerance: B's median at most A's median plus WITHIN percent
  awk -F'\t' -v within="$within" '
    NF == 2 && $1 != "" && $2 != "" { n++; a[n] = $1; b[n] = $2 }
    function sort(x, m,   i, j, t) { for (i = 2; i <= m; i++) { t = x[i]; for (j = i - 1; j > 0 && x[j] > t; j--) x[j + 1] = x[j]; x[j + 1] = t } }
    function rank(x, m, q,   k) { k = int(q * m + 0.999999); if (k < 1) k = 1; return x[k] }
    END {
      if (n == 0) { print "nhmac-pair.sh: no pairs"; exit 1 }
      sort(a, n); sort(b, n)
      ma = rank(a, n, 0.5); mb = rank(b, n, 0.5); bound = ma * (100 + within) / 100
      printf "nhmac-pair.sh: %d pairs: median NS %d, Naglfar %d; the bound (NS + %d%%) %.1f\n", n, ma, mb, within, bound
      if (mb > bound) { print "nhmac-pair.sh: Naglfar median is above the bound"; exit 1 }
    }' "$o/pairs.tsv" || status=1
  continue
fi
# median and p95 (nearest rank) of B - A, p95 of each side, and the sign test
awk -F'\t' '
  NF == 2 && $1 != "" && $2 != "" { n++; d[n] = $2 - $1; a[n] = $1; b[n] = $2; if ($2 > $1) slower++; else if ($2 < $1) faster++ }
  function sort(x, m,   i, j, t) { for (i = 2; i <= m; i++) { t = x[i]; for (j = i - 1; j > 0 && x[j] > t; j--) x[j + 1] = x[j]; x[j + 1] = t } }
  function rank(x, m, q,   k) { k = int(q * m + 0.999999); if (k < 1) k = 1; return x[k] }
  END {
    if (n == 0) { print "nhmac-pair.sh: no pairs"; exit 1 }
    sort(d, n); sort(a, n); sort(b, n)
    med = rank(d, 0 + n, 0.5); pa = rank(a, n, 0.95); pb = rank(b, n, 0.95)
    m = slower + faster
    crit = m / 2 + 1.645 * sqrt(m) / 2
    printf "nhmac-pair.sh: %d pairs: median B-A %d us; p95 NS %d us, Naglfar %d us; Naglfar slower in %d, faster in %d (critical %.1f)\n", n, med, pa, pb, slower, faster, crit
    bad = 0
    if (med > 0) { print "nhmac-pair.sh: the median difference is above 0"; bad = 1 }
    if (pb > pa) { print "nhmac-pair.sh: Naglfar p95 is above NS p95"; bad = 1 }
    if (m > 0 && slower > crit) { print "nhmac-pair.sh: the sign test finds Naglfar slower"; bad = 1 }
    exit bad
  }' "$o/pairs.tsv" || status=1
done
exit $status
