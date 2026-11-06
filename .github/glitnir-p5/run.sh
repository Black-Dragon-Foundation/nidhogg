#!/bin/bash
# Glitnir P5 spike: run one probe under a time limit.  The probe appends to $OUT/results.tsv line by line, so a crash
# or the limit keeps what it found; the exit status is recorded as the probe's last row.
set -u
p=$1
limit=${2:-600}
out=${OUT:-out}
mkdir -p "$out/logs"
if [ ! -x "$out/bin/$p" ]; then
  printf '%s\trun\tnot built\n' "$p" >> "$out/results.tsv"
  exit 1
fi
rc=0
# perl's alarm: macOS has no timeout(1); SIGALRM ends the probe like a kill
perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$limit" "$out/bin/$p" > "$out/logs/$p.log" 2>&1 || rc=$?
cat "$out/logs/$p.log"
case $rc in
  0) ;;
  142) printf '%s\trun\tKILLED after %s s\n' "$p" "$limit" >> "$out/results.tsv" ;;
  *) printf '%s\trun\texit %s\n' "$p" "$rc" >> "$out/results.tsv" ;;
esac
# a crash report names the line
for f in "$HOME"/Library/Logs/DiagnosticReports/"$p"*; do
  [ -e "$f" ] && cp "$f" "$out/logs/"
done
exit "$rc"
