#!/usr/bin/env bash
# Throwaway-repo checks for Nidhogg's own AGENTS.md and README.md: tree-test.sh insists on it, and the documented rebase
# resolution (git checkout --theirs -- AGENTS.md CLAUDE.md README.md) keeps Nidhogg's version when upstream edits the files.
set -u
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d) || exit 1
trap 'rm -rf "$tmp"' EXIT
fails=0
check() { if "$@"; then echo "ok: $*"; else echo "FAIL: $*"; fails=1; fi; }
g() { git -C "$tmp/r" -c user.name=t -c user.email=t@example.invalid -c commit.gpgsign=false "$@"; }

mkdir -p "$tmp/r/nidhogg/test" && git init -q -b main "$tmp/r"
cp "$here/tree-test.sh" "$tmp/r/nidhogg/test/"
printf 'upstream agents v1\n' > "$tmp/r/AGENTS.md"; printf '@AGENTS.md\n' > "$tmp/r/CLAUDE.md"
echo 31.1 > "$tmp/r/emacs.txt"
g add -A && g commit -q -m "upstream 31.1" && g tag emacs-31.1
echo 31.1-nidhogg.3 > "$tmp/r/nidhogg/version"
printf '# AGENTS.md — Nidhogg — agent rules\n' > "$tmp/r/AGENTS.md"
printf '# Nidhogg\n' > "$tmp/r/README.md"
g add -A && g commit -q -m "nidhogg: tree"
tt() { (cd "$tmp/r" && bash nidhogg/test/tree-test.sh "${base:-emacs-31.1}" 2>&1); }
tt_fails() { [ -n "$(tt)" ]; }

check test -z "$(tt)"
printf 'upstream agents v1\n' > "$tmp/r/AGENTS.md"
check tt_fails
printf '# AGENTS.md — Nidhogg — agent rules\n' > "$tmp/r/AGENTS.md"
printf 'something else\n' > "$tmp/r/CLAUDE.md"
check tt_fails
printf '@AGENTS.md\n' > "$tmp/r/CLAUDE.md"
check test -z "$(tt)"

# upstream 31.2 edits both files; rebase the series onto it
g checkout -q -b up emacs-31.1
printf 'upstream agents v2\n' > "$tmp/r/AGENTS.md"; printf '@AGENTS.md\nmore\n' > "$tmp/r/CLAUDE.md"
g commit -q -am "upstream 31.2" && g tag emacs-31.2
g checkout -q main
g rebase --onto emacs-31.2 emacs-31.1 main >/dev/null 2>&1
# upstream edited AGENTS.md (conflicts) and CLAUDE.md (applies silently, the series never touched it)
check test "$(g diff --name-only --diff-filter=U | tr '\n' ' ')" = "AGENTS.md "
g checkout --theirs -- AGENTS.md CLAUDE.md && g add AGENTS.md CLAUDE.md
GIT_EDITOR=true g rebase --continue >/dev/null 2>&1
check grep -qxF '# AGENTS.md — Nidhogg — agent rules' "$tmp/r/AGENTS.md"
check tt_fails   # CLAUDE.md now carries upstream's edit
printf '@AGENTS.md\n' > "$tmp/r/CLAUDE.md"; base=emacs-31.2
g commit -q -a --amend --no-edit
check test -z "$(tt | grep -v '^$')"
check g merge-base --is-ancestor emacs-31.2 HEAD

# upstream 31.3 adds README.md (it has only README today) and edits AGENTS.md: both conflict, ours is kept
g checkout -q -b up2 emacs-31.2
printf 'upstream readme\n' > "$tmp/r/README.md"; printf 'upstream agents v3\n' > "$tmp/r/AGENTS.md"
g add -A && g commit -q -m "upstream 31.3" && g tag emacs-31.3
g checkout -q main
g rebase --onto emacs-31.3 emacs-31.2 main >/dev/null 2>&1
check test "$(g diff --name-only --diff-filter=U | tr '\n' ' ')" = "AGENTS.md README.md "
g checkout --theirs -- AGENTS.md README.md && g add AGENTS.md README.md
GIT_EDITOR=true g rebase --continue >/dev/null 2>&1
check grep -qxF '# Nidhogg' "$tmp/r/README.md"
check test -z "$(base=emacs-31.3 tt | grep -v '^$')"
# a README.md that is not ours fails the tree check
printf 'only README.md\n' > "$tmp/r/README.md"
check tt_fails
exit "$fails"
