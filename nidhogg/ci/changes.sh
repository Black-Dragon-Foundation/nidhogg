#!/usr/bin/env bash
# Which jobs a change can affect, from one path table (paths.txt beside this script).  The same script runs in Nidhogg
# (nidhogg/ci/) and drgnx (.github/ci/); only the tables differ.
#
#   changes.sh TABLE                  in a workflow: the environment below; prints GITHUB_OUTPUT lines
#   changes.sh TABLE --files LIST     the same, with the changed paths read from LIST (one per line), no API
#   changes.sh TABLE --self-test      the mechanism's bite cases, then every `test` line of TABLE
#
# The table: `jobs: ID...` (every job a path row may name; ALL means all of them), `list NAME: ID...` (an output NAME
# with the listed jobs that run), `deps ID: ID...` (the jobs ID takes artifacts from: they run whenever it does), rows `GLOB ID...|ALL|-` (first match wins; `*` matches across `/`; `-`: no job), and
# `test` lines (see --self-test).  A path no row matches runs everything.
#
# Environment: EVENT, REF, SHA, REPO, DEFAULT_BRANCH (github.*); BASE (a pull request's base commit); GATE (the name of
# this workflow's gate check); PUSH (the jobs this workflow runs on a push, which its gate needs); CAN (the jobs a
# dispatch may ask for, default PUSH); REQUEST and SELECTOR (a narrowed dispatch: the jobs to run, and a test selector
# inside them); GH_TOKEN.  CHANGES_GH replaces gh (the self-test's stand-in).
#
# Which runs are filtered: a pull request, a push to ci/** other than ci/joint/**, and a narrowed dispatch.  Every
# other run (the default branch, tags, schedules, a dispatch that names nothing, joint rounds) runs everything.  A
# filtered run diffs against a base whose own gate passed: the pull request's base, or the default branch's tip, which
# must be an ancestor of SHA.  The default branch only ever moves to a commit whose gate passed, so the jobs a change
# cannot affect have their result on the base.  Anything uncertain runs everything: no base, a base that is not an
# ancestor, a base without a passed GATE, an API error, 300 or more changed files, a path no row matches.
#
# Output: run_ID=true|false (ID with - as _) for PUSH and CAN, skip= (the PUSH jobs not run: the gate's allowed
# skips), partial=true|false (a narrowed run left out a job the change needs, or narrowed the tests: its gate is named
# partial and never satisfies branch protection), one line per `list`, why= (a sentence for the log).
# bash 3.2-safe.  Exit 0 always in workflow mode (errors run everything), 2 usage, 1 a failed self-test.
set -u

usage() { echo "usage: changes.sh TABLE [--files LIST | --self-test]" >&2; exit 2; }
[ $# -ge 1 ] || usage
TABLE=$1; shift
[ -f "$TABLE" ] || { echo "changes.sh: no table $TABLE" >&2; exit 2; }
GH=${CHANGES_GH:-gh}

# shellcheck disable=SC2086  # split on purpose, with globbing off
words() { set -f; set -- $1; set +f; echo "$*"; }   # word-split and squeeze
# read a table into JOBS, PATS/IDS (the rows), LISTN/LISTV and TESTS
parse() {
  local line n
  JOBS="" PATS=() IDS=() LISTN=() LISTV=() DEPN=() DEPV=() TESTS=()
  while IFS= read -r line || [ -n "$line" ]; do
    case $line in '#'*|'') continue ;; esac
    case $line in
      'jobs:'*) JOBS=$(words "${line#jobs:}") ;;
      'list '*) n=${line#list }; LISTN+=("${n%%:*}"); LISTV+=("$(words "${n#*:}")") ;;
      'deps '*) n=${line#deps }; DEPN+=("${n%%:*}"); DEPV+=("$(words "${n#*:}")") ;;
      'test '*) TESTS+=("${line#test }") ;;
      *) line=$(words "${line%%#*}"); [ "${line#* }" != "$line" ] || continue
         PATS+=("${line%% *}"); IDS+=("${line#* }") ;;
    esac
  done < "$1"
  [ -n "$JOBS" ] || { echo "changes.sh: $1 has no jobs: line" >&2; exit 2; }
}
parse "$TABLE"

has() { case " $2 " in *" $1 "*) return 0 ;; esac; return 1; }
union() { local out=$1 w; for w in $2; do has "$w" "$out" || out="$out $w"; done; words "$out"; }
inter() { local out="" w; for w in $1; do ! has "$w" "$2" || out="$out $w"; done; words "$out"; }
minus() { local out="" w; for w in $1; do has "$w" "$2" || out="$out $w"; done; words "$out"; }

# a set with the jobs its jobs take artifacts from (`deps` lines), until nothing is added
closure() {
  local set=$1 before="" i
  while [ "$set" != "$before" ]; do
    before=$set i=0
    while [ "$i" -lt "${#DEPN[@]}" ]; do
      ! has "${DEPN[$i]}" "$set" || set=$(union "$set" "${DEPV[$i]}")
      i=$((i + 1))
    done
  done
  echo "$set"
}

# the jobs one path can affect: a row's ids, "" for `-`, ALL when no row matches
affects() {
  local i=0
  while [ "$i" -lt "${#PATS[@]}" ]; do
    # shellcheck disable=SC2053  # the pattern is a glob on purpose
    if [[ $1 == ${PATS[$i]} ]]; then
      case ${IDS[$i]} in -) echo "" ;; ALL) echo ALL ;; *) echo "${IDS[$i]}" ;; esac
      return
    fi
    i=$((i + 1))
  done
  echo ALL
}

# NEED from the changed paths on stdin: the union of what each can affect (ALL short-circuits)
need_of() {
  local need="" f a
  while IFS= read -r f || [ -n "$f" ]; do
    [ -n "$f" ] || continue
    a=$(affects "$f")
    if [ "$a" = ALL ]; then echo "ALL $f"; return; fi
    need=$(union "$need" "$a")
  done
  echo "SET $need"
}

# the changed paths between BASE and SHA through the API, or a reason to run everything (prefixed "!")
api_paths() {
  local base=$1 out status n
  if ! out=$("$GH" api "repos/$REPO/compare/$base...$SHA" --jq '.status, (.files | length), (.files[] | .filename, (.previous_filename // empty))' 2>/dev/null); then
    echo "!the compare API failed"; return
  fi
  status=$(printf '%s\n' "$out" | sed -n 1p); n=$(printf '%s\n' "$out" | sed -n 2p)
  case $status in ahead|identical) ;; *) echo "!the base is not an ancestor ($status)"; return ;; esac
  case $n in ''|*[!0-9]*) echo "!no file count"; return ;; esac
  [ "$n" -lt 300 ] || { echo "!$n files changed (the API lists at most 300)"; return; }
  printf '%s\n' "$out" | sed 1,2d
}

# the base a filtered run diffs against, after its gate passed, or "!" and why not
base_of() {
  local base=${BASE:-} ok
  if [ -z "$base" ]; then
    base=$("$GH" api "repos/$REPO/branches/$DEFAULT_BRANCH" --jq .commit.sha 2>/dev/null) || { echo "!no default branch tip"; return; }
  fi
  case $base in *[!0-9a-f]*|'') echo "!no base commit"; return ;; esac
  ok=$("$GH" api --method GET "repos/$REPO/commits/$base/check-runs" -f check_name="$GATE" -f filter=all \
         --jq '[.check_runs[] | select(.conclusion == "success")] | length' 2>/dev/null) || { echo "!the checks API failed"; return; }
  [ "${ok:-0}" -gt 0 ] 2>/dev/null || { echo "!the base ${base:0:12} has no passed $GATE"; return; }
  echo "$base"
}

# decide: sets RUN, PARTIAL, WHY from the environment and, with $1, the paths in that file
decide() {
  local files=${1:-} push=${PUSH:-$JOBS} can=${CAN:-${PUSH:-$JOBS}} req sel=${SELECTOR:-} filtered=0 base r need
  req=$(words "${REQUEST:-}")
  case ${EVENT:-} in
    pull_request) filtered=1 ;;
    push) case ${REF:-} in refs/heads/ci/joint/*) ;; refs/heads/ci/*) filtered=1 ;; esac ;;
    workflow_dispatch) [ -z "$req$sel" ] || filtered=1 ;;
  esac
  NEED=$push WHY="a full run (${EVENT:-no event} on ${REF:-no ref})"
  if [ "$filtered" = 1 ]; then
    if [ -n "$files" ]; then r=$(need_of < "$files")
    else
      base=$(base_of)
      case $base in
        '!'*) r="ALL" WHY="everything: ${base#!}" ;;
        *) paths=$(api_paths "$base")
           case $paths in '!'*) r="ALL" WHY="everything: ${paths#!}" ;; *) r=$(printf '%s\n' "$paths" | need_of) ;; esac ;;
      esac
    fi
    case $r in
      ALL\ *) WHY="everything: ${r#ALL } is in no row or is a row that runs everything" ;;
      SET*) need=$(closure "$(words "${r#SET}")"); NEED=$(inter "$push" "$need"); WHY="filtered: the change can affect: ${NEED:-no job}" ;;
    esac
  fi
  RUN=$NEED PARTIAL=false
  if [ -n "$req" ]; then
    RUN=$(inter "$(union "$push" "$can")" "$(closure "$req")")
    [ -z "$(minus "$req" "$RUN")" ] || WHY="$WHY; not jobs here: $(minus "$req" "$RUN")"
  fi
  if [ -n "$req$sel" ]; then
    if [ -n "$(minus "$NEED" "$RUN")" ] || [ -n "$sel" ]; then PARTIAL=true; WHY="$WHY; narrowed to: ${RUN:-no job}${sel:+, tests: $sel}"; fi
  fi
}

emit() {
  local push=${PUSH:-$JOBS} can=${CAN:-${PUSH:-$JOBS}} j i=0
  for j in $(union "$push" "$can"); do
    if has "$j" "$RUN"; then echo "run_$(echo "$j" | tr - _)=true"; else echo "run_$(echo "$j" | tr - _)=false"; fi
  done
  echo "skip=$(minus "$push" "$RUN")"
  echo "partial=$PARTIAL"
  while [ "$i" -lt "${#LISTN[@]}" ]; do echo "${LISTN[$i]}=$(inter "${LISTV[$i]}" "$RUN")"; i=$((i + 1)); done
  echo "why=$WHY"
}

if [ "${1:-}" = --self-test ]; then
  rc=0 d=$(mktemp -d "${TMPDIR:-/tmp}/changes-test.XXXXXX")
  trap 'rm -r "$d"' EXIT
  bad() { echo "changes.sh self-test FAIL: $*"; rc=1; }
  # run one case: env assignments then expected RUN and PARTIAL
  case_() {
    local want_run=$1 want_partial=$2; shift 2
    ( for kv in "$@"; do export "${kv?}"; done; decide "${FILES:-}"; echo "$RUN|$PARTIAL|$WHY" ) > "$d/out"
    got=$(cat "$d/out")
    [ "${got%%|*}" = "$want_run" ] || bad "$* => run '${got%%|*}', want '$want_run' (${got##*|})"
    p=${got#*|}; p=${p%%|*}
    [ "$p" = "$want_partial" ] || bad "$* => partial $p, want $want_partial"
  }
  # the mechanism, on a table of its own and a stand-in gh
  T="$d/t.txt"; printf '%s\n' 'jobs: a b c' 'docs/* -' 'src/b/* b' 'src/* a b' '.github/* ALL' > "$T"
  parse "$T"
  # deps: asking for c brings b (its artifact) along; a path that needs c needs b too
  printf '%s\n' 'jobs: a b c' 'deps c: b' 'docs/* -' 'src/c/* c' > "$d/deps.txt"; parse "$d/deps.txt"
  printf '%s\n' src/c/k.c > "$d/c"
  case_ "b c" false EVENT=pull_request FILES="$d/c"
  case_ "b c" true EVENT=workflow_dispatch REQUEST=c SELECTOR=t FILES="$d/c"
  parse "$T"
  printf '%s\n' docs/x.md > "$d/docs"; printf '%s\n' src/b/y.c > "$d/b"; printf '%s\n' src/q.c docs/z > "$d/a"
  printf '%s\n' weird/path > "$d/unknown"; printf '%s\n' .github/w.yml > "$d/wf"
  case_ "" false EVENT=pull_request FILES="$d/docs"
  case_ "b" false EVENT=pull_request FILES="$d/b"
  case_ "a b" false EVENT=pull_request FILES="$d/a"
  case_ "a b c" false EVENT=pull_request FILES="$d/unknown"
  case_ "a b c" false EVENT=pull_request FILES="$d/wf"
  case_ "a b c" false EVENT=push REF=refs/heads/main FILES="$d/docs"
  case_ "a b c" false EVENT=push REF=refs/tags/v1 FILES="$d/docs"
  case_ "a b c" false EVENT=schedule REF=refs/heads/main FILES="$d/docs"
  case_ "a b c" false EVENT=push REF=refs/heads/ci/joint/r1 FILES="$d/docs"
  case_ "a b c" false EVENT=workflow_dispatch REF=refs/heads/ci/x FILES="$d/docs"
  case_ "" false EVENT=push REF=refs/heads/ci/x FILES="$d/docs"
  # narrowed: a job the change needs left out, or a selector, is partial; asking for what it needs is not
  case_ "b" true EVENT=workflow_dispatch REF=refs/heads/ci/x REQUEST=b FILES="$d/a"
  case_ "a b" false EVENT=workflow_dispatch REF=refs/heads/ci/x REQUEST="a b" FILES="$d/a"
  case_ "a b" true EVENT=workflow_dispatch REF=refs/heads/ci/x SELECTOR=foo FILES="$d/a"
  case_ "c" false EVENT=workflow_dispatch REF=refs/heads/ci/x REQUEST=c FILES="$d/docs"
  case_ "a" true EVENT=workflow_dispatch REF=refs/heads/ci/x REQUEST="a zz" FILES="$d/wf"
  # the API path: a stand-in gh that answers from files; every failure runs everything
  cat > "$d/gh" <<'GH'
#!/usr/bin/env bash
case "$*" in
  *branches/*) [ -f "$CT/tip" ] && cat "$CT/tip" || exit 1 ;;
  *check-runs*) [ -f "$CT/gate" ] && cat "$CT/gate" || exit 1 ;;
  *compare/*) [ -f "$CT/compare" ] && cat "$CT/compare" || exit 1 ;;
  *) exit 1 ;;
esac
GH
  chmod +x "$d/gh"; export CT=$d CHANGES_GH=$d/gh; GH=$d/gh
  E=(EVENT=push REF=refs/heads/ci/x REPO=o/r SHA=feed DEFAULT_BRANCH=main GATE=gate)
  echo 0123abcd > "$d/tip"; echo 1 > "$d/gate"; printf 'ahead\n1\ndocs/x.md\n' > "$d/compare"
  case_ "" false "${E[@]}"
  printf 'ahead\n2\nsrc/b/new.c\nsrc/b/old.c\n' > "$d/compare"; case_ "b" false "${E[@]}"
  printf 'diverged\n1\ndocs/x.md\n' > "$d/compare"; case_ "a b c" false "${E[@]}"
  printf 'ahead\n300\ndocs/x.md\n' > "$d/compare"; case_ "a b c" false "${E[@]}"
  printf 'ahead\n1\ndocs/x.md\n' > "$d/compare"; echo 0 > "$d/gate"; case_ "a b c" false "${E[@]}"
  rm "$d/gate"; case_ "a b c" false "${E[@]}"
  echo 1 > "$d/gate"; rm "$d/tip"; case_ "a b c" false "${E[@]}"
  echo 0123abcd > "$d/tip"; rm "$d/compare"; case_ "a b c" false "${E[@]}"
  printf 'ahead\n1\ndocs/x.md\n' > "$d/compare"; case_ "" false "${E[@]}" BASE=4567abcd EVENT=pull_request
  echo 'not-a-sha' > "$d/tip"; case_ "a b c" false "${E[@]}"
  unset CT CHANGES_GH; GH=gh
  # the repository's own table: `test PATHS | REQUEST | SELECTOR | RUN | PARTIAL` (PATHS comma-separated, - for none;
  # RUN: ids, ALL for the table's jobs, - for none), each as a filtered run (a ci/* push or a narrowed dispatch)
  parse "$TABLE"
  [ "${#TESTS[@]}" -ge 6 ] || bad "$TABLE has ${#TESTS[@]} test lines (at least 6: the bite cases)"
  for t in "${TESTS[@]}"; do
    IFS='|' read -r tp tr ts trun tpart <<<"$t"
    tp=$(words "$tp"); tr=$(words "$tr"); ts=$(words "$ts"); trun=$(words "$trun"); tpart=$(words "$tpart")
    [ "$tp" != - ] || tp=""; [ "$tr" != - ] || tr=""; [ "$ts" != - ] || ts=""
    case $trun in ALL) trun=$JOBS ;; -) trun="" ;; esac
    printf '%s\n' "$tp" | tr , '\n' > "$d/paths"
    ev=push; [ -z "$tr$ts" ] || ev=workflow_dispatch
    case_ "$trun" "$tpart" EVENT=$ev REF=refs/heads/ci/x REQUEST="$tr" SELECTOR="$ts" FILES="$d/paths"
  done
  [ "$rc" = 0 ] && echo "changes.sh self-test: ok (${#TESTS[@]} table cases)"
  exit $rc
fi

if [ "${1:-}" = --files ]; then
  [ $# -eq 2 ] && [ -f "$2" ] || usage
  decide "$2"; emit; exit 0
fi
[ $# -eq 0 ] || usage
decide; emit
exit 0
