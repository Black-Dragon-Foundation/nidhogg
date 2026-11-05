#!/usr/bin/env bash
# Give every tracked file the time of the last commit that changed it (a checkout stamps every file with "now"), so that
# what a restored cache built from files this change did not touch stays fresh: cargo judges its targets by mtime, and
# build.sh's restored Lisp is made newer than every source.  Run in the checkout, after the caches are restored.
#   set-mtimes.sh [--since FILE]...
# A shallow checkout is first deepened to the upstream release tag the series sits on (git fetch --shallow-exclude, about
# as many commits as the series has); a file the series never changed gets the time of the oldest fetched commit that
# has it, which stays the same from run to run.  In a full clone the tag's own time stands in for those files.
# --since FILE: FILE names the commit a restored cache was built from (written when it was saved).  Every file that
# differs between that commit and HEAD then gets the current time, so a change committed before the cache was built is
# still newer than the cache; FILE missing or empty, or its commit not in the history, gives every file the current time.
# Untracked and generated files are never touched, so they stay newer than any cache.  bash 3.2-safe (macOS).
set -euo pipefail
since=()
while [ $# -gt 0 ]; do
  case $1 in
    --since) since+=("${2:?--since FILE}"); shift 2 ;;
    *) echo "usage: set-mtimes.sh [--since FILE]..." >&2; exit 2 ;;
  esac
done
cd "$(git rev-parse --show-toplevel)"
base=emacs-$(sed -n '1s/-nidhogg.*//p' nidhogg/version)
if [ "$(git rev-parse --is-shallow-repository)" = true ]; then
  git fetch -q --no-tags --shallow-exclude="refs/tags/$base" origin "$(git rev-parse HEAD)" \
    || echo "set-mtimes.sh: could not deepen the checkout to $base; the files the fetched history does not change get its oldest commit's time" >&2
fi
range=(HEAD) def=""
if git rev-parse -q --verify "refs/tags/$base^{commit}" >/dev/null; then
  range=(HEAD "^refs/tags/$base"); def=$(git log -1 --format=%ct "refs/tags/$base^{commit}")
fi
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
git -c core.quotePath=false ls-files > "$t/files"
# the history newest first: "@TIME", then the paths the commit changed (a shallow history's oldest commits list their
# whole tree); each tracked file takes its newest time
git -c core.quotePath=false log --format=@%ct --name-only --no-renames "${range[@]}" > "$t/log"
awk -v def="$def" '
  FNR == NR { want[$0] = 1; next }
  /^@[0-9]+$/ { ct = substr($0, 2) + 0; if (min == "" || ct < min) min = ct; next }
  $0 == "" || !($0 in want) { next }
  !($0 in tm) || ct > tm[$0] { tm[$0] = ct }
  END { if (def == "") def = min; for (f in want) print ((f in tm) ? tm[f] : def) "\t" f }' "$t/files" "$t/log" | sort -n > "$t/times"
stamp_of() { date -u -d "@$1" +%Y%m%d%H%M.%S 2>/dev/null || date -u -r "$1" +%Y%m%d%H%M.%S; }
flush() {
  [ -s "$t/batch" ] || return 0
  TZ=UTC xargs -0 touch -h -t "$(stamp_of "$cur")" < "$t/batch"
  : > "$t/batch"
}
cur="" tab=$(printf '\t')
: > "$t/batch"
while IFS=$tab read -r ct f; do
  [ "$ct" = "$cur" ] || { flush; cur=$ct; }
  printf '%s\0' "$f" >> "$t/batch"
done < "$t/times"
flush
# what changed since a restored cache was built: newer than the cache
for s in ${since[@]+"${since[@]}"}; do
  c=$(cat "$s" 2>/dev/null || true)
  if [ -n "$c" ] && git cat-file -e "$c^{commit}" 2>/dev/null; then
    git -c core.quotePath=false diff --name-only --no-renames "$c" HEAD > "$t/changed"
    echo "set-mtimes.sh: $(wc -l < "$t/changed" | tr -d ' ') file(s) changed since ${c:0:12} ($s): stamped now"
  else
    cp "$t/files" "$t/changed"
    echo "set-mtimes.sh: no commit from $s in the history: every file stamped now"
  fi
  # a path the diff names may be gone in HEAD
  while IFS= read -r f; do [ -e "$f" ] || [ -L "$f" ] && printf '%s\0' "$f"; done < "$t/changed" | xargs -0 touch -h 2>/dev/null || true
done
