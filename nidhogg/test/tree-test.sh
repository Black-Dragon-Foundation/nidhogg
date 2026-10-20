#!/usr/bin/env bash
# The series rules as a check.  Usage: tree-test.sh [BASE]   (BASE default: emacs-<upstream part of nidhogg/version>)
# Exit 0 when the tree keeps the rules, else 1 with "TREE FAIL: <why>" lines.
set -u
cd "$(git rev-parse --show-toplevel)" || exit 1
fails=0
fail() { echo "TREE FAIL: $*"; fails=1; }

# Glitnir rule: with HAVE_GLITNIR off, the files the glitnir commits touched equal upstream's.  Runs in the current repo.
# GLITNIR_BASE (overridable) defaults to the parent of the oldest commit on HEAD whose subject starts 'nidhogg: glitnir:'.
glitnir_rule() {
  local gbase=${GLITNIR_BASE:-} first f sha body
  if [ -z "$gbase" ]; then
    first=$(git log --reverse --format=%H --grep='^nidhogg: glitnir:' HEAD | head -n 1)
    if [ -z "$first" ]; then echo "tree: no glitnir commits"; return 0; fi
    gbase=$(git rev-parse "$first^")
  fi
  while IFS= read -r f; do
    git cat-file -e "$gbase:$f" 2>/dev/null || continue   # new files are Glitnir's own
    case $f in
      # C sources only (src/Makefile.in is not preprocessed).  Both sides go through unifdef: it also folds unrelated
      # always-false conditions such as '#if defined X && 0', which would show as a diff against the raw base.
      src/*.[chm])
        cmp -s <(git show "HEAD:$f" | unifdef -UHAVE_GLITNIR 2>/dev/null) \
               <(git show "$gbase:$f" | unifdef -UHAVE_GLITNIR 2>/dev/null) ||
          fail "$f changes code outside HAVE_GLITNIR" ;;
      lisp/*)
        body=$(git diff -U0 "$gbase" HEAD -- "$f" | grep -E '^[+-][^+-]')
        { [ "$f" = lisp/loadup.el ] && [ "$(printf '%s\n' "$body" | wc -l)" = 1 ] &&
          printf '%s\n' "$body" | grep -qF "(featurep 'glitnir)"; } ||
          fail "$f changed outside Glitnir's own file" ;;
    esac
  done < <(git diff --name-only "$gbase" HEAD -- src lisp)
  while IFS= read -r sha; do
    case $(git log -1 --format=%s "$sha") in "nidhogg: glitnir:"*) continue ;; esac
    [ -z "$(git diff-tree --no-commit-id --name-only -r "$sha" -- src)" ] ||
      fail "$sha is not a glitnir commit; it goes below the group"
  done < <(git log --format=%H "$gbase..HEAD")
}

# TREE_TEST_SELF=1: the rule on temp repo fixtures.
if [ "${TREE_TEST_SELF:-}" = 1 ]; then
  command -v unifdef >/dev/null || { echo "tree self-test: unifdef is not installed"; exit 1; }
  self=$(mktemp -d) || exit 1
  trap 'rm -rf "$self"' EXIT
  GLITNIR_BASE=
  fixture() {   # a fresh repo with one upstream commit
    rm -rf "$self/r" && mkdir "$self/r" && cd "$self/r" || exit 1
    git init -q . && git config user.email t@t && git config user.name t && git config commit.gpgsign false
    mkdir src && printf 'int a;\n' > src/a.c && git add . && git commit -qm upstream
  }
  put() { printf '%b' "$2" > "$self/r/src/a.c" && git -C "$self/r" commit -qam "$1"; }
  expect() {   # expect NAME PATTERN ("" = no output)
    out=$(glitnir_rule 2>&1)
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
  expect "a pgtk commit above the group" "is not a glitnir commit; it goes below the group"
  fixture; put "nidhogg: pgtk: p" "/* p */\nint a;\n"; put "nidhogg: glitnir: g" "/* p */\n$in"
  expect "the same commit below the group" ""
  fixture; put "nidhogg: v0: v" "/* v */\nint a;\n"; put "nidhogg: glitnir: g" "/* v */\n$in"
  expect "a v0 commit before the group (the base moved)" ""
  echo "tree self-test: 5 cases ok"
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
hits=$(git ls-files nidhogg .github README.md | while IFS= read -r f; do
  [ -f "$f" ] && grep -IEl "$pat" "$f"; done)
[ -z "$hits" ] || fail "absolute home path in: $(echo "$hits" | tr '\n' ' ')"

glitnir_rule

exit "$fails"
