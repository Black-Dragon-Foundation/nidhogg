#!/usr/bin/env bash
# The series rules as a check.  Usage: tree-test.sh [BASE]   (BASE default: emacs-<upstream part of nidhogg/version>)
# Exit 0 when the tree keeps the rules, else 1 with "TREE FAIL: <why>" lines.
set -u
cd "$(git rev-parse --show-toplevel)" || exit 1
fails=0
fail() { echo "TREE FAIL: $*"; fails=1; }

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

exit "$fails"
