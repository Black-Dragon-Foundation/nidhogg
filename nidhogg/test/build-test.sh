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
  out=$(env NIDHOGG_BUILD_UNAME=Linux NIDHOGG_BUILD_PODMAN=true NIDHOGG_BUILD_MEMORY_KB=$((16*1024*1024)) ${envs[@]+"${envs[@]}"} bash nidhogg/build.sh "$@" 2>&1)
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
# the floor is 3 GiB available (MiB precision); cap and jobs are max(2, min(6, available - 1)) GiB
run NIDHOGG_BUILD_NPROC=16 NIDHOGG_BUILD_MEMORY_KB=$((2969*1024)) -- --dry-run --container fedora-45   # 2.9 GiB
rcis 1; has "needs 3072 MiB"; has "only 2969 MiB are available"
run NIDHOGG_BUILD_NPROC=16 NIDHOGG_BUILD_MEMORY_KB=$((3*1024*1024)) -- --dry-run --container fedora-45   # 3.0 GiB
rcis 0; has "--memory 2g --memory-swap 2g"; has "make -j1"
run NIDHOGG_BUILD_NPROC=16 NIDHOGG_BUILD_MEMORY_KB=$((3584*1024)) -- --dry-run --container fedora-45   # 3.5 GiB
rcis 0; has "--memory 2g --memory-swap 2g"; has "make -j1"
run NIDHOGG_BUILD_NPROC=16 NIDHOGG_BUILD_MEMORY_KB=$((4*1024*1024)) -- --dry-run --container fedora-45   # 4.0 GiB
rcis 0; has "--memory 3g --memory-swap 3g"; has "make -j2"
# a host build with an explicit --jobs or --memory warns instead of refusing
run NIDHOGG_BUILD_UNAME=Darwin NIDHOGG_BUILD_NPROC=16 NIDHOGG_BUILD_MEMORY_KB=$((2969*1024)) -- --dry-run --host --prefix /tmp/n
rcis 1; has "only 2969 MiB are available"
run NIDHOGG_BUILD_UNAME=Darwin NIDHOGG_BUILD_NPROC=16 NIDHOGG_BUILD_MEMORY_KB=$((2969*1024)) -- --dry-run --host --prefix /tmp/n --jobs 3
rcis 0; has "warning:"; has "make -j3"
run NIDHOGG_BUILD_UNAME=Darwin NIDHOGG_BUILD_NPROC=16 NIDHOGG_BUILD_MEMORY_KB=$((2969*1024)) -- --dry-run --host --prefix /tmp/n --memory 2g
rcis 0; has "warning:"
run -- --dry-run --container ubuntu-26.04 --memory 12g --jobs 4
rcis 0; has "docker.io/library/ubuntu:26.04"; has "--memory 12g"; has "make -j4"
run -- --dry-run --container arch
rcis 2; has "fedora-44 fedora-45 ubuntu-26.04"
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n
rcis 0; has "--with-ns"; has "/Nidhogg.app"; has "P=/tmp/n"; hasnt "podman"
run -- --dry-run --container fedora-44 --check nidhogg/check-subset.txt
rcis 0; has "make -C test src/nidhogg-tests"
run -- --dry-run --container fedora-44 --out dist/x.tar.xz
rcis 0; has "DESTDIR=/build/stage"; has "BUILDINFO"; has "/out:Z"
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --out dist/x.tar.xz
rcis 0; has "Nidhogg.app"; has "BUILDINFO"
# the stall watchdog is built in on every target unless --without-nidhogg-watchdog
run -- --dry-run --container fedora-45
rcis 0; has "--with-nidhogg-watchdog"
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n
rcis 0; has "--with-nidhogg-watchdog"
run -- --dry-run --container fedora-45 --without-nidhogg-watchdog
rcis 0; hasnt "--with-nidhogg-watchdog"
# --dev: native-compilation=yes, no JIT in the image, never packed, cache stamp carries the mode
run -- --dry-run --container fedora-44 --dev
rcis 0; has "--with-native-compilation=yes"; hasnt "--with-native-compilation=aot"; has "site-init.el"; has "MODE_ID=yes"; has "--with-tree-sitter"
run NIDHOGG_BUILD_DEV=1 -- --dry-run --container fedora-44
rcis 0; has "--with-native-compilation=yes"
run -- --dry-run --container fedora-44 --dev --out dist/x.tar.xz
rcis 1; has "never packed"
run -- --dry-run --container fedora-44 --dev --stage pack
rcis 1; has "never packed"
run -- --dry-run --container fedora-44
rcis 0; hasnt "site-init.el"; has "MODE_ID=aot"
# macOS counts free + inactive + speculative + purgeable pages (a fake vm_stat: 16 KiB pages, 262144 each = 4 GiB each)
T=$(mktemp -d "${TMPDIR:-/tmp}/nidhogg-build-test.XXXXXX")
trap 'rm -rf "$T"' EXIT
cat > "$T/vm_stat" <<'VM'
#!/bin/sh
echo "Mach Virtual Memory Statistics: (page size of 16384 bytes)"
echo "Pages free:                               ${FREE:-10000}."
echo "Pages active:                             900000."
echo "Pages inactive:                           ${INACT:-0}."
echo "Pages speculative:                        ${SPEC:-0}."
echo "Pages purgeable:                          ${PURG:-0}."
VM
chmod +x "$T/vm_stat"
run NIDHOGG_BUILD_MEMORY_KB= NIDHOGG_BUILD_UNAME=Darwin NIDHOGG_BUILD_VMSTAT="$T/vm_stat" -- --dry-run --host --prefix /tmp/n
rcis 1; has "MiB are available"
for c in "190054 1" "196608 0" "229376 0" "262144 0"; do   # free pages = 2.9, 3.0, 3.5, 4.0 GiB
  run NIDHOGG_BUILD_MEMORY_KB= NIDHOGG_BUILD_UNAME=Darwin NIDHOGG_BUILD_VMSTAT="$T/vm_stat" FREE=${c% *} -- --dry-run --host --prefix /tmp/n
  rcis "${c#* }"
done
run NIDHOGG_BUILD_MEMORY_KB= NIDHOGG_BUILD_UNAME=Darwin NIDHOGG_BUILD_VMSTAT="$T/vm_stat" INACT=262144 SPEC=131072 PURG=131072 -- --dry-run --host --prefix /tmp/n
rcis 0; has "make -j"
run NIDHOGG_BUILD_MEMORY_KB= NIDHOGG_BUILD_UNAME=Darwin NIDHOGG_BUILD_VMSTAT="$T/vm_stat" INACT=262144 -- --dry-run --host --prefix /tmp/n
rcis 0; has "make -j2"
# ccache: opt-in, the cache directory mounted, ccache installed, the compiler checked by content
run NIDHOGG_BUILD_CCACHE=/var/tmp/cc -- --dry-run --container fedora-45
rcis 0; has "-v /var/tmp/cc:/ccache:Z"; has "CCACHE_COMPILERCHECK=content"; has 'CC="ccache gcc"'; has " ccache"
run NIDHOGG_BUILD_CCACHE=/var/tmp/cc -- --dry-run --container ubuntu-26.04
rcis 0; has "--no-install-recommends \$(sed"; has "ccache -s"
run -- --dry-run --container fedora-45
rcis 0; hasnt "ccache"
# compiled-Lisp cache: opt-in, mounted, restored before autogen, saved after make, only for the same compiler
run NIDHOGG_BUILD_LISP_CACHE=/var/tmp/lc -- --dry-run --container fedora-45
rcis 0; has "-v /var/tmp/lc:/lispcache:Z"; has "phase restore compiled Lisp"; has "phase save compiled Lisp"; has 'cc_id'
run -- --dry-run --container fedora-45
rcis 0; hasnt "lispcache"; hasnt "compiled Lisp"
# stages: build = make only, check = the check targets in the built tree, pack = install and tarball; no prefix mount unless packing
run -- --dry-run --container fedora-44 --stage build --check nidhogg/check-subset.txt --out dist/x.tar.xz
rcis 0; has "phase make -j"; hasnt "phase check"; hasnt "phase tarball"; hasnt ":/out:Z"
run -- --dry-run --container fedora-44 --stage check --check nidhogg/check-subset.txt --out dist/x.tar.xz
rcis 0; has "phase check src/emacs-tests"; hasnt "phase make -j"; hasnt "phase autogen"; hasnt "phase tarball"; has 'cd "$B/obj"'
run -- --dry-run --container fedora-44 --stage pack --check nidhogg/check-subset.txt --out dist/x.tar.xz
rcis 0; has "phase tarball"; hasnt "phase make -j"; hasnt "phase check"; has ":/out:Z"
run -- --dry-run --container fedora-44 --stage bogus
rcis 2
# one progress line per phase
run -- --dry-run --container fedora-44 --check nidhogg/check-subset.txt --out dist/x.tar.xz
for ph in "phase autogen" "phase configure" "phase make -j" "phase check src/nidhogg-tests" "phase install" "phase tarball"; do has "$ph"; done
has "install_weak_deps=False"
run -- --dry-run --container ubuntu-26.04
has "--no-install-recommends"
# the heartbeat and the log tail on failure (logged() lifted out of the generated script)
run -- --dry-run --host --prefix /tmp/n
sed -n '/^phase()/p;/^logged()/,/^}/p' <<<"$out" > "$T/fns.sh"
hb=$(bash -c 'HB=1; PHASE=make; . "$1"; logged "$2" sleep 2.5' _ "$T/fns.sh" "$T/ok.log" 2>&1)
case $hb in *"... make still running, 0 min"*) ;; *) fail "no heartbeat: $hb";; esac
hb=$(bash -c 'HB=300; PHASE=make; . "$1"; seq 1 300 > "$2.in"; logged "$2" sh -c "cat $2.in; exit 3"' _ "$T/fns.sh" "$T/bad.log" 2>&1); frc=$?
case $hb in *"FAILED"*) ;; *) fail "no failure line: $hb";; esac
case $hb in *$'\n'300$'\n'*|*$'\n'201*) ;; *) fail "no log tail: $hb";; esac
case $hb in *$'\n'200$'\n'*) fail "tail longer than 100 lines";; esac
# the container is named and removed on exit and on TERM (a fake podman)
cat > "$T/podman" <<'PM'
#!/bin/sh
echo "$@" >> "$FAKE_LOG"
[ "$1" = run ] && [ -n "${FAKE_HANG:-}" ] && exec sleep 30
exit 0
PM
chmod +x "$T/podman"
: > "$T/pm.log"
run NIDHOGG_BUILD_PODMAN="$T/podman" FAKE_LOG="$T/pm.log" NIDHOGG_BUILD_MEMORY_KB=$((16*1024*1024)) -- --container fedora-44 --prefix "$T/prefix"
rcis 0
name=$(sed -n 's/^run .*--name \(nidhogg-build-[0-9]*\) .*/\1/p' "$T/pm.log")
[ -n "$name" ] || fail "container not named: $(cat "$T/pm.log")"
grep -q "^stop .*$name\$" "$T/pm.log" && grep -q "^rm -f $name\$" "$T/pm.log" || fail "container not stopped and removed: $(cat "$T/pm.log")"
# the build and check stages (no extra mount) with the three cache dirs: no empty word in podman's argv (CI: "container directory cannot be empty")
for st in build check pack; do
  : > "$T/pm.log"
  run NIDHOGG_BUILD_PODMAN="$T/podman" FAKE_LOG="$T/pm.log" NIDHOGG_BUILD_PKG_CACHE="$T/c/pkg" NIDHOGG_BUILD_CCACHE="$T/c/cc" \
    NIDHOGG_BUILD_LISP_CACHE="$T/c/lisp" -- --container fedora-45 --stage $st --build-dir "$T/b-$st" --prefix "$T/prefix"
  rcis 0
  # the fake podman logs "$@" joined by spaces: an empty argument shows as a double space or a trailing/leading one
  case $(grep '^run ' "$T/pm.log") in *"  "*|*" ") fail "stage $st: empty podman argument: $(cat "$T/pm.log")";; esac
  grep -q -- "-v $T/c/lisp:/lispcache:Z" "$T/pm.log" || fail "stage $st: lisp cache not mounted"
done
: > "$T/pm.log"
NIDHOGG_BUILD_UNAME=Linux NIDHOGG_BUILD_PODMAN="$T/podman" FAKE_LOG="$T/pm.log" FAKE_HANG=1 NIDHOGG_BUILD_MEMORY_KB=$((16*1024*1024)) \
  bash nidhogg/build.sh --container fedora-44 --prefix "$T/prefix" >/dev/null 2>&1 &
bpid=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do grep -q '^run ' "$T/pm.log" && break; sleep 0.5; done
kill -TERM "$bpid"; wait "$bpid"; trc=$?
[ "$trc" = 143 ] || fail "TERM: exit $trc"
grep -q '^stop ' "$T/pm.log" && grep -q '^rm -f nidhogg-build-' "$T/pm.log" || fail "TERM: container not removed: $(cat "$T/pm.log")"
# every distro has both lists; build.sh has no bash-4 constructs
for d in fedora-44 fedora-45 ubuntu-26.04 macos; do
  { test -s nidhogg/deps/$d.build && test -s nidhogg/deps/$d.runtime; } || fail "deps lists missing for $d"
done
if grep -nE 'declare -A|mapfile|readarray|\$\{[a-zA-Z_]+(,,|\^\^)\}|;&|\|&' nidhogg/build.sh; then fail "bash 4 construct in build.sh"; fi
if command -v shellcheck >/dev/null; then shellcheck --severity=warning nidhogg/build.sh nidhogg/test/*.sh || fail shellcheck; fi
exit "$fails"
