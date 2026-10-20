#!/usr/bin/env bash
# build.sh dry-run assertions.  Run from anywhere; exit 0 when all hold.
cd "$(git rev-parse --show-toplevel)" || exit 1
fails=0
out="" rc=0
fail() { echo "BUILD FAIL: $*"; fails=1; }
# run [VAR=val ...] -- ARGS...   (Linux, podman "true" unless overridden)
run() {
  local envs=()
  while [ "$1" != "--" ]; do envs+=("$1"); shift; done
  shift
  out=$(env NIDHOGG_BUILD_UNAME=Linux NIDHOGG_BUILD_PODMAN=true ${envs[@]+"${envs[@]}"} bash nidhogg/build.sh "$@" 2>&1)
  rc=$?
}
has() { case $out in *"$1"*) ;; *) fail "output lacks '$1' (args: $*)";; esac; }
hasnt() { case $out in *"$1"*) fail "output has '$1'";; esac; }
rcis() { [ "$rc" = "$1" ] || fail "exit $rc, wanted $1: $out"; }

run NIDHOGG_BUILD_NPROC=16 NIDHOGG_BUILD_MEMORY_KB=$((16*1024*1024)) -- --dry-run --container fedora-44 --prefix /opt/x
rcis 0; has "--memory 6g --memory-swap 6g"; has "make -j4"; has "registry.fedoraproject.org/fedora:44"
has "--prefix=\"\$P\""; has "P=/opt/x"; has "--with-pgtk"; has "--with-native-compilation=aot"; has "--with-tree-sitter"
has "nidhogg/deps/fedora-44.build"; hasnt "docker"
# low-memory-shrinks-or-refuses
run NIDHOGG_BUILD_NPROC=16 NIDHOGG_BUILD_MEMORY_KB=$((5*1024*1024)) -- --dry-run --container fedora-45
rcis 0; has "--memory 4g --memory-swap 4g"; has "make -j2"; has "/opt/nidhogg/31.1-nidhogg.1"
run NIDHOGG_BUILD_MEMORY_KB=$((3*1024*1024)) -- --dry-run --container fedora-45
rcis 1; has "needs 3 GiB of free memory"
run -- --dry-run --container ubuntu-26.04 --memory 12g --jobs 4
rcis 0; has "docker.io/library/ubuntu:26.04"; has "--memory 12g"; has "make -j4"
run -- --dry-run --container arch
rcis 2; has "fedora-44 fedora-45 ubuntu-26.04"
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n
rcis 0; has "--with-ns"; has "/Nidhogg.app"; has "P=/tmp/n"; hasnt "podman"
run -- --dry-run --container fedora-44 --check nidhogg/check-subset.txt
rcis 0; has "make -C test src/nidhogg-tests"
# every distro has both lists; build.sh has no bash-4 constructs
for d in fedora-44 fedora-45 ubuntu-26.04 macos; do
  { test -s nidhogg/deps/$d.build && test -s nidhogg/deps/$d.runtime; } || fail "deps lists missing for $d"
done
if grep -nE 'declare -A|mapfile|readarray|\$\{[a-zA-Z_]+(,,|\^\^)\}|;&|\|&' nidhogg/build.sh; then fail "bash 4 construct in build.sh"; fi
if command -v shellcheck >/dev/null; then shellcheck --severity=warning nidhogg/build.sh nidhogg/test/*.sh || fail shellcheck; fi
exit "$fails"
