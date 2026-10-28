#!/usr/bin/env bash
# The series rules as a check.  Usage: tree-test.sh [BASE]   (BASE default: emacs-<upstream part of nidhogg/version>)
# Exit 0 when the tree keeps the rules, else 1 with "TREE FAIL: <why>" lines.
set -u
cd "$(git rev-parse --show-toplevel)" || exit 1
fails=0
fail() { echo "TREE FAIL: $*"; fails=1; }
# Without unifdef both sides of the C comparison are empty and every file passes unread: refuse instead.
command -v unifdef >/dev/null || { echo "TREE FAIL: unifdef is not installed (the C rule cannot run)"; exit 1; }

# Flag-group rule: for each group G with commits on HEAD (subject 'nidhogg: G:'), with the defines of every group that
# has commits in G's range undefined, the files G touched equal G's base.  Runs in the current repo.
# FLAG_GROUPS (overridable) = name:define:feature, space separated.  ${NAME}_BASE (upper case; GLITNIR_BASE keeps
# working, overridable) defaults to the parent of the oldest commit on HEAD whose subject starts 'nidhogg: <name>:'.
# Order-independent: either group may be on top.
FLAG_GROUPS_DEFAULT='glitnir:HAVE_GLITNIR:glitnir watchdog:HAVE_NIDHOGG_WATCHDOG:nidhogg-watchdog workers:HAVE_NIDHOGG_WORKERS:nidhogg-workers shell:HAVE_NHWL:nidhogg-wayland'
flag_rule() {
  local g name def ft b var first f sha body i j c n matched feat lowest
  local -a gname=() gdef=() gfeat=() gbase=() flags=() feats=()
  for g in ${FLAG_GROUPS:-$FLAG_GROUPS_DEFAULT}; do
    IFS=: read -r name def ft <<<"$g"
    var=${name^^}_BASE; var=${var//-/_}; b=${!var:-}
    if [ -z "$b" ]; then
      first=$(git log --reverse --format=%H --grep="^nidhogg: $name:" HEAD | head -n 1)
      [ -n "$first" ] || continue
      b=$(git rev-parse "$first^")
    fi
    gname+=("$name"); gdef+=("$def"); gfeat+=("$ft"); gbase+=("$b")
  done
  if [ ${#gname[@]} = 0 ]; then echo "tree: no flag-group commits"; return 0; fi
  for i in "${!gname[@]}"; do
    flags=(); feats=()
    for j in "${!gname[@]}"; do   # U: the defines of every group with commits in this group's range
      if [ "$i" = "$j" ] || [ -n "$(git rev-list -n 1 --grep="^nidhogg: ${gname[j]}:" "${gbase[i]}..HEAD")" ]; then
        flags+=("-U${gdef[j]}"); feats+=("${gfeat[j]}")
      fi
    done
    while IFS= read -r f; do
      git cat-file -e "${gbase[i]}:$f" 2>/dev/null || continue   # new files are the group's own
      case $f in
        # C sources only (src/Makefile.in is not preprocessed).  Both sides go through unifdef: it also folds unrelated
        # always-false conditions such as '#if defined X && 0', which would show as a diff against the raw base.
        src/*.[chm])
          cmp -s <(git show "HEAD:$f" | unifdef "${flags[@]}" 2>/dev/null) \
                 <(git show "${gbase[i]}:$f" | unifdef "${flags[@]}" 2>/dev/null) ||
            fail "$f changes code outside ${gdef[i]}" ;;
        lisp/*)
          body=$(git diff -U0 "${gbase[i]}" HEAD -- "$f" | grep -E '^[+-][^+-]')
          n=$(printf '%s\n' "$body" | wc -l); matched=0
          for feat in "${feats[@]}"; do
            c=$(printf '%s\n' "$body" | grep -cF "(featurep '$feat)")
            [ "$c" -le 1 ] || matched=-1000   # one line per group
            matched=$((matched + c))
          done
          { [ "$f" = lisp/loadup.el ] && [ "$matched" = "$n" ]; } ||
            fail "$f changed outside the groups' own files" ;;
      esac
    done < <(git diff --name-only "${gbase[i]}" HEAD -- src lisp)
  done
  lowest=$(git merge-base --octopus "${gbase[@]}")
  while IFS= read -r sha; do
    c=$(git log -1 --format=%s "$sha")
    for g in "${gname[@]}"; do case $c in "nidhogg: $g:"*) continue 2 ;; esac; done
    [ -z "$(git diff-tree --no-commit-id --name-only -r "$sha" -- src)" ] ||
      fail "$sha belongs to no flag group; it goes below the groups"
  done < <(git log --format=%H "$lowest..HEAD")
}

# TREE_TEST_SELF=1: the rule on temp repo fixtures.
if [ "${TREE_TEST_SELF:-}" = 1 ]; then
  command -v unifdef >/dev/null || { echo "tree self-test: unifdef is not installed"; exit 1; }
  self=$(mktemp -d) || exit 1
  trap 'rm -rf "$self"' EXIT
  GLITNIR_BASE= WATCHDOG_BASE= WORKERS_BASE=
  fixture() {   # a fresh repo with one upstream commit
    rm -rf "$self/r" && mkdir "$self/r" && cd "$self/r" || exit 1
    git init -q . && git config user.email t@t && git config user.name t && git config commit.gpgsign false
    mkdir src && printf 'int a;\n' > src/a.c && git add . && git commit -qm upstream
  }
  put() { printf '%b' "$2" > "$self/r/src/a.c" && git -C "$self/r" commit -qam "$1"; }
  expect() {   # expect NAME PATTERN ("" = no output)
    out=$(flag_rule 2>&1)
    if { [ -z "$2" ] && [ -n "$out" ]; } || { [ -n "$2" ] && ! printf '%s\n' "$out" | grep -q -- "$2"; }; then
      echo "tree self-test: case '$1' failed; output: ${out:-<none>}"; exit 1
    fi
  }
  in='int a;\n#ifdef HAVE_GLITNIR\nint g;\n#endif\n'
  fixture; put "nidhogg: glitnir: g" "${in}int out;\n"
  expect "a line outside the flag" "src/a.c changes code outside HAVE_GLITNIR"
  fixture; put "nidhogg: glitnir: g" "$in"
  expect "a line inside the flag" ""
  fixture; put "nidhogg: glitnir: g" "$in"; put "nidhogg: pgtk: p" "/* p */\n$in"
  expect "a pgtk commit above the group" "belongs to no flag group; it goes below the groups"
  fixture; put "nidhogg: pgtk: p" "/* p */\nint a;\n"; put "nidhogg: glitnir: g" "/* p */\n$in"
  expect "the same commit below the group" ""
  fixture; put "nidhogg: v0: v" "/* v */\nint a;\n"; put "nidhogg: glitnir: g" "/* v */\n$in"
  expect "a v0 commit before the group (the base moved)" ""
  wd='int a;\n#ifdef HAVE_NIDHOGG_WATCHDOG\nint w;\n#endif\n'
  both='int a;\n#ifdef HAVE_NIDHOGG_WATCHDOG\nint w;\n#endif\n#ifdef HAVE_GLITNIR\nint g;\n#endif\n'
  putf() {   # putf SUBJECT FILE CONTENT
    mkdir -p "$self/r/$(dirname "$2")" && printf '%b' "$3" > "$self/r/$2" && git -C "$self/r" add -A &&
      git -C "$self/r" commit -qm "$1"; }
  loadup() { fixture; putf "nidhogg: pgtk: l" lisp/loadup.el '(a)\n'; }
  fixture; put "nidhogg: watchdog: w" "${wd}int out;\n"
  expect "a watchdog line outside the flag" "src/a.c changes code outside HAVE_NIDHOGG_WATCHDOG"
  fixture; put "nidhogg: watchdog: w" "$wd"
  expect "a watchdog line inside the flag" ""
  fixture; put "nidhogg: watchdog: w" "$wd"; putf "nidhogg: glitnir: g" src/b.c '#ifdef HAVE_GLITNIR\nint g;\n#endif\n'
  expect "watchdog below glitnir, both inside" ""
  fixture; putf "nidhogg: glitnir: g" src/b.c '#ifdef HAVE_GLITNIR\nint g;\n#endif\n'; put "nidhogg: watchdog: w" "$wd"
  expect "watchdog above glitnir" ""
  fixture; put "nidhogg: glitnir: g" "$in"; put "nidhogg: watchdog: w" "$both"
  expect "one file touched by both, each in its own ifdef" ""
  loadup; putf "nidhogg: watchdog: w" lisp/loadup.el "(a)\n(featurep 'nidhogg-watchdog)\n"
  putf "nidhogg: glitnir: g" lisp/loadup.el "(a)\n(featurep 'nidhogg-watchdog)\n(featurep 'glitnir)\n"
  expect "loadup.el with both lines" ""
  fixture; put "nidhogg: watchdog: w" "$wd"; put "nidhogg: glitnir: g" "$both"; put "nidhogg: pgtk: p" "/* p */\n$both"
  expect "a pgtk commit above both groups" "belongs to no flag group; it goes below the groups"
  wk='int a;\n#ifdef HAVE_NIDHOGG_WORKERS\nint k;\n#endif\n'
  wdwk='int a;\n#ifdef HAVE_NIDHOGG_WATCHDOG\nint w;\n#endif\n#ifdef HAVE_NIDHOGG_WORKERS\nint k;\n#endif\n'
  all3='int a;\n#ifdef HAVE_NIDHOGG_WATCHDOG\nint w;\n#endif\n#ifdef HAVE_NIDHOGG_WORKERS\nint k;\n#endif\n#ifdef HAVE_GLITNIR\nint g;\n#endif\n'
  fixture; put "nidhogg: workers: k" "${wk}int out;\n"
  expect "a workers line outside the flag" "src/a.c changes code outside HAVE_NIDHOGG_WORKERS"
  fixture; put "nidhogg: workers: k" "$wk"
  expect "a workers line inside the flag" ""
  fixture; put "nidhogg: watchdog: w" "$wd"; put "nidhogg: workers: k" "$wdwk"; put "nidhogg: glitnir: g" "$all3"
  expect "watchdog, workers, glitnir stacked, one file, each inside its define" ""
  fixture; putf "nidhogg: glitnir: g" src/b.c '#ifdef HAVE_GLITNIR\nint g;\n#endif\n'; put "nidhogg: workers: k" "$wk"
  expect "workers above glitnir" ""
  loadup; putf "nidhogg: watchdog: w" lisp/loadup.el "(a)\n(featurep 'nidhogg-watchdog)\n"
  putf "nidhogg: workers: k" lisp/loadup.el "(a)\n(featurep 'nidhogg-watchdog)\n(featurep 'nidhogg-workers)\n"
  putf "nidhogg: glitnir: g" lisp/loadup.el "(a)\n(featurep 'nidhogg-watchdog)\n(featurep 'nidhogg-workers)\n(featurep 'glitnir)\n"
  expect "loadup.el with three lines" ""
  fixture; putf "nidhogg: workers: k" src/w.c '#ifdef HAVE_NIDHOGG_WORKERS\nint k;\n#endif\n'
  putf "nidhogg: glitnir: g" src/w.c '#ifdef HAVE_NIDHOGG_WORKERS\nint k;\n#endif\n#ifdef HAVE_GLITNIR\nint g;\n#endif\n'
  expect "a glitnir commit editing a file a workers commit created" ""
  fixture; putf "upstream jsonrpc" lisp/jsonrpc.el '(a)\n'; putf "nidhogg: workers: k" lisp/jsonrpc.el '(a)\n(b)\n'
  expect "a changed upstream jsonrpc.el in a workers commit" "lisp/jsonrpc.el changed outside the groups' own files"
  echo "tree self-test: 19 cases ok"
  exit 0
fi

if [ ! -f nidhogg/version ]; then
  fail "nidhogg/version is missing"
  ver=""
else
  ver=$(head -n 1 nidhogg/version)
  printf '%s\n' "$ver" | grep -Eq '^[0-9]+\.[0-9]+-nidhogg\.[0-9]+$' ||
    fail "nidhogg/version '$ver' does not match <emacs>-nidhogg.<N>"
fi

base=${1:-}
if [ -z "$base" ] && [ -n "$ver" ]; then base="emacs-${ver%%-nidhogg.*}"; fi
if [ -z "$base" ]; then
  fail "no base to compare with"
elif ! git rev-parse --verify --quiet "$base^{commit}" >/dev/null; then
  fail "base $base does not exist"
else
  git merge-base --is-ancestor "$base" HEAD || fail "$base is not an ancestor of HEAD"
  while IFS= read -r subject; do
    case $subject in
      "nidhogg: "*) ;;
      *) fail "commit subject does not start with 'nidhogg: ': $subject" ;;
    esac
  done < <(git log --format=%s "$base..HEAD")
fi

# agent rules: upstream tracks AGENTS.md and CLAUDE.md; Nidhogg replaces them with its own (never send anything to GNU)
grep -qxF '# AGENTS.md — Nidhogg — agent rules' AGENTS.md 2>/dev/null ||
  fail "AGENTS.md is not Nidhogg's version (marker line '# AGENTS.md — Nidhogg — agent rules' is missing)"
[ "$(cat CLAUDE.md 2>/dev/null)" = "@AGENTS.md" ] || fail "CLAUDE.md is not Nidhogg's version (it must be '@AGENTS.md')"

# README.md is Nidhogg's (upstream ships only README; a rebase keeps ours if upstream ever adds README.md)
[ "$(head -n 1 README.md 2>/dev/null)" = "# Nidhogg" ] || fail "README.md is not Nidhogg's version (first line must be '# Nidhogg')"

# local artifacts: the series may not add any other agent/plan file
bad=$(git ls-files | grep -E '^(\.claude/|\.superpowers/|docs/plans/)' |
  while IFS= read -r f; do
    if [ -n "${base:-}" ] && git cat-file -e "$base:$f" 2>/dev/null; then
      git diff --quiet "$base" -- "$f" || echo "$f"
    else
      echo "$f"
    fi
  done)
[ -z "$bad" ] || fail "local artifacts are tracked or changed: $(echo "$bad" | tr '\n' ' ')"

# the pattern is built from two halves so this file does not match itself
pat='/ho''me/|/Us''ers/'
hits=$(git ls-files nidhogg .github README.md ':!nidhogg/*/vendor/*' | while IFS= read -r f; do
  [ -f "$f" ] && grep -IEl "$pat" "$f"; done)
[ -z "$hits" ] || fail "absolute home path in: $(echo "$hits" | tr '\n' ' ')"

flag_rule

# the shell API and the glue name no backend: no 'wayland_' identifier in nhshell's source outside src/wayland/, nor
# in the glue's source
wl=$(git ls-files nidhogg/shell/src nidhogg/glue/src | grep -v '^nidhogg/shell/src/wayland/' |
  while IFS= read -r f; do # (the test api::no_wayland_names_in_ffi has the name in its own fn line)
    [ -f "$f" ] && grep 'wayland_' "$f" | grep -v 'fn no_' | grep -q . && echo "$f"; done)
[ -z "$wl" ] || fail "wayland_ named outside nidhogg/shell/src/wayland/: $(echo "$wl" | tr '\n' ' ')"

# Adapted-from rule: every file under nidhogg/ and src/ with an Adapted-from line (colon, then a source) names a '## <source>'
# entry of nidhogg/THIRD_PARTY.md, and every entry is named by at least one file.  (Pattern in two halves, as above.)
third=nidhogg/THIRD_PARTY.md
apat='Adapted''-from:'
entries=$(sed -n 's/^## *//p' "$third" 2>/dev/null | sed -E 's/[[:space:]]+$//')
used=""
while IFS= read -r f; do
  [ "$f" = "$third" ] && continue
  [ -f "$f" ] || continue
  while IFS= read -r src; do
    src=$(printf '%s' "$src" | sed -E "s/.*${apat}[[:space:]]*//; s/[[:space:]]*(\*\/|-->)?[[:space:]]*\$//")
    if printf '%s\n' "$entries" | grep -qxF -- "$src"; then
      used="$used
$src"
    else
      fail "$f: Adapted-from not in THIRD_PARTY.md"
    fi
  done < <(grep -Ia -- "$apat" "$f")
done < <(git ls-files nidhogg src)
while IFS= read -r e; do
  [ -n "$e" ] || continue
  printf '%s\n' "$used" | grep -qxF -- "$e" || fail "$third: entry '$e' is named by no file"
done <<< "$entries"

exit "$fails"
