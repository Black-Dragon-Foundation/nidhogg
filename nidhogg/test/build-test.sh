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

run NIDHOGG_BUILD_NPROC=16 NIDHOGG_BUILD_MEMORY_KB=$((16*1024*1024)) -- --dry-run --container fedora-45 --prefix /opt/x
rcis 0; has "--memory 6g --memory-swap 6g"; has "make -j4"; has "registry.fedoraproject.org/fedora:45"
has "--prefix=\"\$P\""; has "P=/opt/x"; has "--with-nidhogg-wayland"; hasnt "pgtk"; has "--with-native-compilation=aot"; has "--with-tree-sitter"
has "nidhogg/deps/fedora-45.build"; hasnt "docker"
# Glitnir (the compositor): --with-glitnir by default on Linux and NS, --without-glitnir turns it off (and installs no Rust)
has "--with-glitnir"; hasnt "grep -vxE"
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n
rcis 0; has "--with-glitnir"; has "--with-ns"; hasnt "grep -vxE"; hasnt "--with-nidhogg-wayland"
# without Glitnir there is no shell, so Linux builds for the terminal
run -- --dry-run --container fedora-45 --without-glitnir
rcis 0; hasnt "--with-glitnir"; hasnt "--with-nidhogg-wayland"; has "--without-x"; has "grep -vxE 'cargo|rust|rustc'"
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n --without-glitnir
rcis 0; hasnt "--with-glitnir"; has "--with-ns"; hasnt "--without-x"
# the old switch still works, and it cannot be asked for without what it needs
run -- --dry-run --container fedora-45 --nidhogg-wayland
rcis 0; has "--with-nidhogg-wayland"
run -- --dry-run --container fedora-45 --nidhogg-wayland --without-glitnir
rcis 1; has "needs Glitnir"
for d in fedora-45 ubuntu-26.04 debian-13 arch; do
  ! grep -qi gtk nidhogg/deps/$d.build nidhogg/deps/$d.runtime || fail "$d's package lists still name GTK"
done
# low-memory-shrinks-or-refuses
run NIDHOGG_BUILD_NPROC=16 NIDHOGG_BUILD_MEMORY_KB=$((5*1024*1024)) -- --dry-run --container fedora-45
rcis 0; has "--memory 4g --memory-swap 4g"; has "make -j2"; has "/opt/nidhogg/31.1-nidhogg.6"
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
run -- --dry-run --container nosuch
rcis 2; has "fedora-45 ubuntu-26.04 debian-13 arch"
run -- --dry-run --container debian-13 --jobs 4
rcis 0; has "docker.io/library/debian:13"; has "apt-get install"
run -- --dry-run --container arch --jobs 4
rcis 0; has "docker.io/library/archlinux:latest"; has "pacman -Syu --noconfirm --needed"; hasnt "apt-get"
run NIDHOGG_BUILD_PKG_CACHE=/var/tmp/pc -- --dry-run --container arch
rcis 0; has "/var/tmp/pc/pacman:/var/cache/pacman/pkg"
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n
rcis 0; has "--with-ns"; has "/Nidhogg.app"; has "P=/tmp/n"; hasnt "podman"
run -- --dry-run --container fedora-45
rcis 0; has "cmp -s"
run -- --dry-run --container fedora-45 --check nidhogg/check-subset.txt
rcis 0; has "make -C test src/nidhogg-tests"; has "nidhogg/test/check-rerun.sh"; has "check_rerun"
run -- --dry-run --container fedora-45 --out dist/x.tar.xz
rcis 0; has "DESTDIR=/build/stage"; has "BUILDINFO"; has "/out:Z"; has "/src:ro,z"
has 'lic=$pk/share/nidhogg/licenses'; has 'cp -a "$S/nidhogg/licenses/." "$lic/"'; has 'cp "$S/nidhogg/THIRD_PARTY.md" "$lic/"'
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --out dist/x.tar.xz
rcis 0; has "Nidhogg.app"; has "BUILDINFO"
# the stall watchdog is built in on every target unless --without-nidhogg-watchdog
run -- --dry-run --container fedora-45
rcis 0; has "--with-nidhogg-watchdog"
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n
rcis 0; has "--with-nidhogg-watchdog"
run -- --dry-run --container fedora-45 --without-nidhogg-watchdog
rcis 0; hasnt "--with-nidhogg-watchdog"
# the worker threads likewise, unless --without-nidhogg-workers
run -- --dry-run --container fedora-45
rcis 0; has "--with-nidhogg-workers"
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n
rcis 0; has "--with-nidhogg-workers"
run -- --dry-run --container fedora-45 --without-nidhogg-workers
rcis 0; hasnt "--with-nidhogg-workers"; has "--with-nidhogg-watchdog"
# the edit history's hook likewise, wherever Glitnir is (it holds the rings), unless --without-nidhogg-history
run -- --dry-run --container fedora-45
rcis 0; has "--with-nidhogg-history"
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n
rcis 0; has "--with-nidhogg-history"
run -- --dry-run --container fedora-45 --without-nidhogg-history
rcis 0; hasnt "--with-nidhogg-history"; has "--with-nidhogg-workers"
run -- --dry-run --container fedora-45 --without-glitnir
rcis 0; hasnt "--with-nidhogg-history"
# --chaos: seeded delays in a dev build, never a release
run -- --dry-run --container fedora-45
rcis 0; hasnt "--with-nidhogg-chaos"
run -- --dry-run --container fedora-45 --dev --chaos
rcis 0; has "--with-nidhogg-chaos"
run -- --dry-run --container fedora-45 --chaos
rcis 1; has "dev builds only"
# --checking: --enable-checking (the test hooks) in a dev build, never a release
run -- --dry-run --container fedora-45
rcis 0; hasnt "--enable-checking"
run -- --dry-run --container fedora-45 --dev --checking
rcis 0; has "--enable-checking"
run -- --dry-run --container fedora-45 --checking
rcis 1; has "dev builds only"
# --dev: native-compilation=yes, no JIT in the image, never packed, cache stamp carries the mode
run -- --dry-run --container fedora-45 --dev
rcis 0; has "--with-native-compilation=yes"; hasnt "--with-native-compilation=aot"; has "site-init.el"; has "MODE_ID=yes"; has "--with-tree-sitter"
run NIDHOGG_BUILD_DEV=1 -- --dry-run --container fedora-45
rcis 0; has "--with-native-compilation=yes"
run -- --dry-run --container fedora-45 --dev --out dist/x.tar.xz
rcis 1; has "never packed"
# --native-preloaded: the same native-compilation mode and no JIT, but a build that is packed; its compiled-Lisp stamp
# names the mode, and a second stamp (without the mode and the window system) lets NIDHOGG_BUILD_LISP_CACHE_ELC take the .elc
run -- --dry-run --container fedora-45 --native-preloaded --out dist/x.tar.xz
rcis 0; has "--with-native-compilation=yes"; has "site-init.el"; has "MODE_ID=yes"; has "phase tarball"
run NIDHOGG_BUILD_NATIVE_PRELOADED=1 -- --dry-run --container fedora-45
rcis 0; has "--with-native-compilation=yes"; has "site-init.el"
run NIDHOGG_BUILD_LISP_CACHE=/var/tmp/lc NIDHOGG_BUILD_LISP_CACHE_ELC=1 -- --dry-run --container fedora-45
rcis 0; has "LC_ELC=1"; has "ELC_CONF_ID=--prefix"; case $(sed -n 's/^ELC_CONF_ID=//p' <<<"$out") in *native-compilation*) fail "the byte-compiled stamp names the native-compilation mode";; esac; has 'cc_elc_id > "$LC/stamp-elc"'; has "phase restore byte-compiled Lisp"
run NIDHOGG_BUILD_UNAME=Darwin NIDHOGG_BUILD_LISP_CACHE=/var/tmp/lc -- --dry-run --host --prefix /tmp/n
a1=$(sed -n 's/^ELC_CONF_ID=//p' <<<"$out")
run NIDHOGG_BUILD_UNAME=Darwin NIDHOGG_BUILD_LISP_CACHE=/var/tmp/lc -- --dry-run --host --prefix /tmp/n --native-preloaded
[ "$(sed -n 's/^ELC_CONF_ID=//p' <<<"$out")" = "$a1" ] || fail "the byte-compiled stamp changes with the native-compilation mode"
run -- --dry-run --container fedora-45 --dev --stage pack
rcis 1; has "never packed"
run -- --dry-run --container fedora-45
rcis 0; hasnt "site-init.el"; has "MODE_ID=aot"; hasnt "fsanitize"
# NIDHOGG_BUILD_SANITIZE reaches CFLAGS and LDFLAGS; dev builds only
run NIDHOGG_BUILD_SANITIZE=address,undefined -- --dry-run --container fedora-45 --dev
rcis 0; has "CFLAGS='-g -O1 -fno-omit-frame-pointer -fsanitize=address,undefined'"; has "LDFLAGS='-fsanitize=address,undefined'"; has "libasan"
run NIDHOGG_BUILD_SANITIZE=address,undefined -- --dry-run --container fedora-45
rcis 1; has "dev builds only"
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
rcis 0; has "-v /var/tmp/cc:/ccache:z"; has "CCACHE_COMPILERCHECK=content"; has 'CC="ccache gcc"'; has " ccache"
run NIDHOGG_BUILD_CCACHE=/var/tmp/cc -- --dry-run --container ubuntu-26.04
rcis 0; has "--no-install-recommends \$(sed"; has "ccache -s"
run -- --dry-run --container fedora-45
rcis 0; hasnt "ccache"
# on the host (macOS CI): the cache directory as given, the compiler checked by its -v output (not the cc shim's content)
run NIDHOGG_BUILD_CCACHE=/var/tmp/cc NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n
rcis 0; has "CCACHE_DIR=/var/tmp/cc"; has "CCACHE_COMPILERCHECK=%compiler%\\ -v"; has "CC=ccache\\ gcc"; has "ccache -s"; hasnt "CCACHE_COMPILERCHECK=content"
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n
rcis 0; hasnt "ccache"
# compiled-Lisp cache: opt-in, mounted with a shared label, restored before autogen under a shared lock, saved after make
# under an exclusive one, only for the same compiler
run NIDHOGG_BUILD_LISP_CACHE=/var/tmp/lc -- --dry-run --container fedora-45
rcis 0; has "-v /var/tmp/lc:/lispcache:z"; has "phase restore compiled Lisp"; has "phase save compiled Lisp"; has 'cc_id'
has "lc_lock -s"; has "lc_lock -x"; hasnt "/lispcache:Z"
# an entry without obj (the .eln files, cached apart by the dev CI builds) restores its byte-compiled part alone
has '[ ! -d "$LC/obj" ] || parts="src obj"'; has 'tar -C "$LC" -cf - $parts'
# ... and only for the same configure line: the prefix and the flags are part of the .eln files' ABI hash
run NIDHOGG_BUILD_LISP_CACHE=/var/tmp/lc -- --dry-run --container fedora-45 --prefix /tmp/p1
rcis 0; has "CONF_ID=--prefix=/tmp/p1\\ --with-native-compilation=aot"; has '"$MODE_ID" "$CONF_ID"'
run -- --dry-run --container fedora-45
rcis 0; hasnt "lispcache"; hasnt "compiled Lisp"
# stages: build = make only, check = the check targets in the built tree, pack = install and tarball; no prefix mount unless packing
run -- --dry-run --container fedora-45 --stage build --check nidhogg/check-subset.txt --out dist/x.tar.xz
rcis 0; has "phase make -j"; hasnt "phase check"; hasnt "phase tarball"; hasnt ":/out:Z"
run -- --dry-run --container fedora-45 --stage check --check nidhogg/check-subset.txt --out dist/x.tar.xz
rcis 0; has "phase check src/emacs-tests"; hasnt "phase make -j"; hasnt "phase autogen"; hasnt "phase tarball"; has 'cd "$B/obj"'
run -- --dry-run --container fedora-45 --stage pack --check nidhogg/check-subset.txt --out dist/x.tar.xz
rcis 0; has "phase tarball"; hasnt "phase make -j"; hasnt "phase check"; has ":/out:Z"
run -- --dry-run --container fedora-45 --stage bogus
rcis 2
# one progress line per phase
run -- --dry-run --container fedora-45 --check nidhogg/check-subset.txt --out dist/x.tar.xz
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
run NIDHOGG_BUILD_PODMAN="$T/podman" FAKE_LOG="$T/pm.log" NIDHOGG_BUILD_MEMORY_KB=$((16*1024*1024)) -- --container fedora-45 --prefix "$T/prefix"
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
  grep -q -- "-v $T/c/lisp:/lispcache:z" "$T/pm.log" || fail "stage $st: lisp cache not mounted"
done
# the .eln guard's verdict (exit 75, nidhogg/eln-guard.el): the build tree goes and one more run builds cold (nothing
# restored); a second 75 is a failure, not a third run
cat > "$T/podman75" <<'PM'
#!/bin/sh
[ "$1" = run ] || exit 0
for a; do last=$a; done
case $last in
  *"LC_RESTORE=1"*) echo restore >> "$FAKE_LOG"; exit 75 ;;
  *"LC_RESTORE=0"*) echo cold >> "$FAKE_LOG"; exit "${FAKE_RC:-0}" ;;
esac
echo none >> "$FAKE_LOG"
PM
chmod +x "$T/podman75"
mkdir -p "$T/b75/obj" "$T/b75/src"; : > "$T/b75/obj/stale"; : > "$T/pm.log"
run NIDHOGG_BUILD_PODMAN="$T/podman75" FAKE_LOG="$T/pm.log" NIDHOGG_BUILD_LISP_CACHE="$T/c/lisp" -- --container fedora-45 --stage build --build-dir "$T/b75" --prefix "$T/prefix"
rcis 0; has "building again without the restored compiled Lisp"
[ "$(tr '\n' ' ' < "$T/pm.log")" = "restore cold " ] || fail "the guard's 75: runs $(tr '\n' ' ' < "$T/pm.log"), wanted a restoring one, then a cold one"
[ ! -e "$T/b75/obj/stale" ] || fail "the guard's 75: the build tree was kept for the cold run"
: > "$T/pm.log"
run NIDHOGG_BUILD_PODMAN="$T/podman75" FAKE_LOG="$T/pm.log" FAKE_RC=75 NIDHOGG_BUILD_LISP_CACHE="$T/c/lisp" -- --container fedora-45 --stage build --build-dir "$T/b75" --prefix "$T/prefix"
rcis 1; [ "$(wc -l < "$T/pm.log")" = 2 ] || fail "the guard's 75 twice: $(wc -l < "$T/pm.log") runs, wanted 2"
# the image a caller names (CI's prebuilt one) replaces the distro's; its packages are checked before any install
run NIDHOGG_BUILD_IMAGE=ghcr.io/x/nidhogg-build-fedora-45@sha256:abc -- --dry-run --container fedora-45 --prefix /opt/x
rcis 0; has "ghcr.io/x/nidhogg-build-fedora-45@sha256:abc"; hasnt "registry.fedoraproject.org"; has "rpm -q --quiet --whatprovides"
run -- --dry-run --container ubuntu-26.04 --prefix /opt/x
rcis 0; has 'if ! { cmp -s /usr/share/nidhogg-image/ubuntu-26.04.build $S/nidhogg/deps/ubuntu-26.04.build'
: > "$T/pm.log"
NIDHOGG_BUILD_UNAME=Linux NIDHOGG_BUILD_PODMAN="$T/podman" FAKE_LOG="$T/pm.log" FAKE_HANG=1 NIDHOGG_BUILD_MEMORY_KB=$((16*1024*1024)) \
  bash nidhogg/build.sh --container fedora-45 --prefix "$T/prefix" >/dev/null 2>&1 &
bpid=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do grep -q '^run ' "$T/pm.log" && break; sleep 0.5; done
kill -TERM "$bpid"; wait "$bpid"; trc=$?
[ "$trc" = 143 ] || fail "TERM: exit $trc"
grep -q '^stop ' "$T/pm.log" && grep -q '^rm -f nidhogg-build-' "$T/pm.log" || fail "TERM: container not removed: $(cat "$T/pm.log")"
# dumplinks links the installed binary's own dump, not the first one a reused prefix holds, even while the links still
# name an earlier build's dump (the binary loads a dump before it reads any option, and refuses one not built for it)
dl=$T/dl; mkdir -p "$dl/bin" "$dl/libexec/emacs/31.1/x86_64"
: > "$dl/libexec/emacs/31.1/x86_64/emacs-aaa.pdmp"; : > "$dl/libexec/emacs/31.1/x86_64/emacs-bbb.pdmp"
ln -s ../libexec/emacs/31.1/x86_64/emacs-aaa.pdmp "$dl/bin/emacs-31.1.pdmp"
printf '#!/bin/sh\ncase "$1" in --dump-file=*/emacs-bbb.pdmp) echo bbb ;; *) exit 1 ;; esac\n' > "$dl/bin/emacs-31.1"
chmod +x "$dl/bin/emacs-31.1"
( eval "$(sed -n '/^dumplinks() {/,/^}/p' nidhogg/build.sh)"; UPV=31.1; dumplinks "$dl" )
[ "$(readlink "$dl/bin/nidhogg.pdmp")" = ../libexec/emacs/31.1/x86_64/emacs-bbb.pdmp ] ||
  fail "dumplinks: nidhogg.pdmp -> $(readlink "$dl/bin/nidhogg.pdmp"), wanted the binary's emacs-bbb.pdmp"
# every distro has both lists; build.sh has no bash-4 constructs
for d in fedora-45 ubuntu-26.04 debian-13 arch macos; do
  { test -s nidhogg/deps/$d.build && test -s nidhogg/deps/$d.runtime; } || fail "deps lists missing for $d"
done
# the Rust toolchain for Glitnir is in every build list (Homebrew: rust)
for d in fedora-45; do grep -qx cargo nidhogg/deps/$d.build && grep -qx rust nidhogg/deps/$d.build || fail "$d.build lacks cargo/rust"; done
grep -qx cargo nidhogg/deps/ubuntu-26.04.build && grep -qx rustc nidhogg/deps/ubuntu-26.04.build || fail "ubuntu-26.04.build lacks cargo/rustc"
# without vendor/ cargo fetches the locked crates over HTTPS: the apt lists, installed without recommends, name the CA
# certificates (Fedora's and Arch's bases have them)
for d in ubuntu-26.04 debian-13; do grep -qx ca-certificates nidhogg/deps/$d.build || fail "$d.build lacks ca-certificates"; done
# Debian's Rust is below the floor: the build fetches the pinned one, for which it needs curl and nothing of Debian's
grep -qx curl nidhogg/deps/debian-13.build && ! grep -qxE 'cargo|rustc' nidhogg/deps/debian-13.build || fail "debian-13.build: curl, and no distro Rust"
run -- --dry-run --container debian-13 --jobs 4
rcis 0; has "RUST_PIN=1.93.1"; has "sha256sum -c"
run -- --dry-run --container debian-13 --jobs 4 --without-glitnir
rcis 0; hasnt "RUST_PIN"
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n
rcis 0; hasnt "RUST_PIN"
pin=$(sed -n 's/.*RUST_PIN=1\.\([0-9]*\)\..*/\1/p' nidhogg/build.sh | head -1)
floor=$(sed -n 's/.*whether rustc is 1\.\([0-9]*\) or newer.*/\1/p' configure.ac | head -1)
recent=$(sed -n 's/.*"${v#\* }" -ge \([0-9]*\).*/\1/p' nidhogg/build.sh | head -1)
[ -n "$pin" ] && [ "$pin" -ge "$floor" ] && [ "$recent" = "$floor" ] || fail "build.sh's Rust (pin 1.$pin, check 1.$recent) and configure's floor 1.$floor disagree"
grep -qx rust nidhogg/deps/arch.build || fail "arch.build lacks rust"
grep -qx rust nidhogg/deps/macos.build || fail "macos.build lacks rust"
# a stage after the build mounts the cargo target the build linked src/glitnir-target to, without being given it again
# (the pack stage's make install runs cargo through that link)
ctb=$(mktemp -d); printf '%s\n' /tmp/ct-host > "$ctb/.nidhogg-cargo-target"
run -- --dry-run --container ubuntu-26.04 --stage pack --build-dir "$ctb" --out "$ctb/n.tar.xz"
rcis 0; has "-v /tmp/ct-host:/cargotarget:z"
run -- --dry-run --container ubuntu-26.04 --stage check --build-dir "$ctb"
rcis 0; has "-v /tmp/ct-host:/cargotarget:z"
run NIDHOGG_BUILD_CARGO_TARGET=/tmp/ct-other -- --dry-run --container ubuntu-26.04 --stage pack --build-dir "$ctb" --out "$ctb/n.tar.xz"
rcis 0; has "-v /tmp/ct-other:/cargotarget:z"
rm -r "$ctb"
# naglfar: --naglfar is the macOS shell (--with-nidhogg-macos, not --with-ns); elsewhere it only configures (--dry-config)
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n --naglfar
rcis 0; has "--with-nidhogg-macos"; hasnt "--with-ns "; hasnt "--with-nidhogg-wayland"; hasnt "--build="
run -- --dry-run --container fedora-45 --naglfar
rcis 1; has "elsewhere only --dry-config"
run -- --dry-run --container fedora-45 --naglfar --dry-config
rcis 0; has "--with-nidhogg-macos"; has "--build=aarch64-apple-darwin"; has 'dry config: $B/obj/src/config.h'
run NIDHOGG_BUILD_UNAME=Darwin -- --dry-run --host --prefix /tmp/n --naglfar --without-glitnir
rcis 1; has "--naglfar needs Glitnir"
# naglfar-config (build-test.sh --config; real configures in podman, through the heavy slot): the Wayland configure
# defines NHWL_WAYLAND; the macOS one (configured on Linux for a darwin host) the shared C and HAVE_NHMAC, and none of
# Wayland, Cairo, fontconfig, FreeType, HarfBuzz or an image library; the workers and Glitnir stay on
if [ "${1:-}" = --config ]; then
  def() { grep -Eq "^#define $2( |\$)" "$1"; }
  for w in wayland naglfar; do
    cb=$(mktemp -d "${TMPDIR:-/tmp}/nh-config.XXXXXX")
    if [ $w = wayland ]; then a=--nidhogg-wayland; else a="--naglfar --dry-config"; fi
    # shellcheck disable=SC2086
    bash nidhogg/build.sh --container fedora-45 --dev --stage build --dry-config --build-dir "$cb" --prefix /opt/nh-config $a > "$cb.log" 2>&1 ||
      fail "the $w configure failed: $(tail -n 5 "$cb.log")"
    h=$cb/obj/src/config.h
    if [ ! -f "$h" ]; then fail "no config.h from the $w configure"
    elif [ $w = wayland ]; then
      for d in HAVE_NHWL NHWL_WAYLAND; do def "$h" $d || fail "wayland config.h lacks $d"; done
      ! def "$h" HAVE_NHMAC || fail "wayland config.h defines HAVE_NHMAC"
    else
      for d in HAVE_NHWL HAVE_NHMAC HAVE_NIDHOGG_WORKERS HAVE_GLITNIR HAVE_WINDOW_SYSTEM HAVE_NATIVE_IMAGE_API DAEMON_MUST_EXEC; do
        def "$h" $d || fail "naglfar config.h lacks $d"
      done
      for d in NHWL_WAYLAND HAVE_NS USE_CAIRO HAVE_FONTCONFIG HAVE_FREETYPE HAVE_HARFBUZZ HAVE_PNG HAVE_JPEG HAVE_GIF \
               HAVE_TIFF HAVE_WEBP HAVE_RSVG HAVE_XPM HAVE_IMAGEMAGICK HAVE_LIBXML2_IMAGES; do
        ! def "$h" $d || fail "naglfar config.h defines $d"
      done
    fi
    echo "build-test: $w configure checked ($cb)"
  done
else
  echo "build-test: the configure cases (naglfar-config) run with --config"
fi
if grep -nE 'declare -A|mapfile|readarray|\$\{[a-zA-Z_]+(,,|\^\^)\}|;&|\|&' nidhogg/build.sh; then fail "bash 4 construct in build.sh"; fi
if command -v shellcheck >/dev/null; then shellcheck --severity=warning nidhogg/build.sh nidhogg/test/*.sh || fail shellcheck; fi
exit "$fails"
