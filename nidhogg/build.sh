#!/usr/bin/env bash
# Build Nidhogg in a capped podman container or on the host.  See README.md ("Building").
#   nidhogg/build.sh (--container DISTRO | --host) [--prefix DIR] [--jobs N] [--memory SIZE]
#                    [--build-dir DIR] [--check FILE] [--out FILE] [--stage STAGE] [--dry-run]
# Linux builds Nidhogg's own Wayland shell (--with-nidhogg-wayland, which carries Glitnir), the one Linux path; macOS the NS app.
# --without-glitnir on Linux is a terminal-only build with no Rust: the shell needs Glitnir, so there is no window system.
# --nidhogg-wayland is the default and stays accepted; with --without-glitnir it is refused.
# --stage build|check|pack runs one part on an existing --build-dir (default: all three in one run): build = configure and make
# (and the compiled-Lisp cache), check = the --check targets in the built tree, pack = install and the tarball (--out).
# --dev (or NIDHOGG_BUILD_DEV=1) is the DEV BUILD, for agents' task builds and functional/GUI tests: it configures
# --with-native-compilation=yes instead of aot, so only the preloaded Lisp (the files dumped into emacs) is native-compiled and the
# other ~1,400 files stay bytecode; the build takes minutes where the AOT build takes the better part of an hour.  Skipped:
# the .eln of every non-preloaded library.  Also baked in: lisp/site-init.el (in the build copy, dumped into the image) sets
# native-comp-jit-compilation to nil, so no background compiles start while tests run.  Fine on a dev build: GUI and functional
# tests.  Needs the AOT build: performance runs (Gate A, budgets, A/B) and releases.  A dev build can never be mistaken for a
# release: --dev refuses --out and --stage pack (no tarball is made), and its compiled-Lisp cache stamp says "dev", so dev and
# AOT entries never mix (use a fresh --build-dir when switching modes).
# --native-preloaded (or NIDHOGG_BUILD_NATIVE_PRELOADED=1) is that native-compilation mode alone, for a build that is
# still packed (CI's scratch runs of the macOS app): the preloaded Lisp is native-compiled, the other files stay byte code,
# and JIT compilation is off in the image.  It is a tarball for tests, not a release: BUILDINFO's configure line says
# --with-native-compilation=yes, which tarball-test.sh reads to check what this mode promises.
# NIDHOGG_BUILD_LISP_CACHE_ELC=1 lets the compiled-Lisp cache give the byte-compiled part (not the .eln files) to a build
# of another native-compilation mode or window system (the NS app, Naglfar), which the entry's full stamp refuses.
# --naglfar builds Nidhogg's own macOS shell (--with-nidhogg-macos) instead of the NS app; on macOS only, except with
# --dry-config, which stops after configure (src/config.h in the build dir) and on Linux configures for a darwin host.
# --checking configures --enable-checking (dev builds only): Emacs's assertions, and the shell's and glue's test hooks
# (the macOS rows inject events and read counters through them).
# Exit: 0 ok, 1 build/check failed or not enough memory, 2 usage.
# Test overrides: NIDHOGG_BUILD_UNAME, NIDHOGG_BUILD_NPROC, NIDHOGG_BUILD_MEMORY_KB, NIDHOGG_BUILD_PODMAN,
# NIDHOGG_BUILD_VMSTAT (a vm_stat stand-in), NIDHOGG_BUILD_HEARTBEAT (seconds between "still running" lines, default 300).
# NIDHOGG_BUILD_PKG_CACHE=DIR keeps the container's downloaded packages (dnf, apt) in DIR between builds.
# NIDHOGG_BUILD_CCACHE=DIR compiles the C code through ccache with its cache in DIR.  In a container ccache is installed
# there and the compiler is checked by content, so a changed gcc never hits; on the host (macOS CI) ccache must be on
# PATH and the compiler is checked by its `-v` output (Xcode's cc is a shim whose content does not follow Xcode).
# NIDHOGG_BUILD_LISP_CACHE=DIR keeps the build's compiled Lisp (the generated files under lisp/ and leim/: .elc, loaddefs, ...,
# and obj/native-lisp: the .eln files) in DIR: written after `make`, restored (made newer than every source) before it.  The
# caller keys DIR on everything that determines the output (the Lisp and C sources, build.sh, the distro, the compiler): an
# entry is only ever reused for the exact same inputs; build.sh itself restores only an entry made by the same compiler,
# architecture and configure line (--prefix included: it is part of the .eln files' ABI hash).  DIR/obj may be missing:
# a caller may cache it under a key of its own (the dev CI builds key the byte-compiled part on the Lisp side alone,
# nidhogg/ci/lisp-key.sh --elc), and the .eln files are then compiled again.  Builds of one key may share DIR at once: a restore holds DIR/.lock
# shared and a save holds it exclusive (with flock; without it, as before).
# bash 3.2-safe (macOS): none of the bash 4 features.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
VERSION=$(head -n 1 "$ROOT/nidhogg/version")
DISTROS="fedora-45 ubuntu-26.04 debian-13 arch"

usage() {
  cat >&2 <<USAGE
usage: nidhogg/build.sh (--container DISTRO | --host) [--prefix DIR] [--jobs N] [--memory SIZE]
                        [--build-dir DIR] [--check FILE] [--out FILE] [--stage build|check|pack] [--without-glitnir] [--nidhogg-wayland] [--dev] [--chaos] [--without-nidhogg-watchdog] [--without-nidhogg-workers] [--without-nidhogg-history] [--naglfar] [--dry-config] [--dry-run]
  DISTRO: $DISTROS
USAGE
  exit 2
}
die() { echo "build.sh: $*" >&2; exit 1; }

MODE="" DISTRO="" PREFIX="" JOBS="" MEMORY="" BUILD_DIR="" CHECK="" OUT="" DRY=0 STAGE=all GLITNIR=--with-glitnir
DEV=0; [ "${NIDHOGG_BUILD_DEV:-}" != 1 ] || DEV=1
PRELOADED=0; [ "${NIDHOGG_BUILD_NATIVE_PRELOADED:-}" != 1 ] || PRELOADED=1
NHWL=0       # --nidhogg-wayland was asked for by name
WATCHDOG=1  # the stall watchdog is built in unless --without-nidhogg-watchdog
WORKERS=1    # the worker threads are built in unless --without-nidhogg-workers
HISTORY=1    # the edit history's change hook is built in with Glitnir (it holds the rings) unless --without-nidhogg-history
NAGLFAR=0    # --naglfar: the macOS shell (--with-nidhogg-macos), not the NS app
DRYCONF=0    # --dry-config: configure only
CHECKING=0   # --checking: --enable-checking, so the test hooks are built; dev builds only
CHAOS=0      # --chaos: seeded delays at the workers' sync points (src/nidhogg-chaos.h); dev builds only, so never in a release
while [ $# -gt 0 ]; do
  case $1 in
    --container) [ $# -ge 2 ] || usage; MODE=container; DISTRO=$2; shift 2 ;;
    --host) MODE=host; shift ;;
    --prefix) [ $# -ge 2 ] || usage; PREFIX=$2; shift 2 ;;
    --jobs) [ $# -ge 2 ] || usage; JOBS=$2; shift 2 ;;
    --memory) [ $# -ge 2 ] || usage; MEMORY=$2; shift 2 ;;
    --build-dir) [ $# -ge 2 ] || usage; BUILD_DIR=$2; shift 2 ;;
    --check) [ $# -ge 2 ] || usage; CHECK=$2; shift 2 ;;
    --out) [ $# -ge 2 ] || usage; OUT=$2; shift 2 ;;
    --stage) [ $# -ge 2 ] || usage; STAGE=$2; shift 2; case $STAGE in all|build|check|pack) ;; *) usage ;; esac ;;
    --without-glitnir) GLITNIR=""; shift ;;
    --nidhogg-wayland) NHWL=1; shift ;;
    --dev) DEV=1; shift ;;
    --native-preloaded) PRELOADED=1; shift ;;
    --chaos) CHAOS=1; shift ;;
    --checking) CHECKING=1; shift ;;
    --without-nidhogg-watchdog) WATCHDOG=0; shift ;;
    --without-nidhogg-workers) WORKERS=0; shift ;;
    --without-nidhogg-history) HISTORY=0; shift ;;
    --dry-run) DRY=1; shift ;;
    --naglfar) NAGLFAR=1; shift ;;
    --dry-config) DRYCONF=1; shift ;;
    *) echo "unknown option: $1" >&2; usage ;;
  esac
done
[ -n "$MODE" ] || usage
if [ "$DEV" = 1 ] && { [ -n "$OUT" ] || [ "$STAGE" = pack ]; }; then
  die "--dev builds are never packed (no --out, no --stage pack): a dev build is not a release; build without --dev"
fi
[ "$CHAOS" = 0 ] || [ "$DEV" = 1 ] || die "--chaos is for dev builds only: add --dev"
[ "$CHECKING" = 0 ] || [ "$DEV" = 1 ] || die "--checking is for dev builds only: add --dev"
[ "$NHWL" = 0 ] || [ -n "$GLITNIR" ] || die "--nidhogg-wayland needs Glitnir: leave out --without-glitnir"
NATIVE=aot; { [ "$DEV" = 0 ] && [ "$PRELOADED" = 0 ]; } || NATIVE=yes
# NIDHOGG_BUILD_SANITIZE=address,undefined adds -fsanitize=... to CFLAGS and LDFLAGS: dev builds only (the workers-asan CI job)
SANITIZE=${NIDHOGG_BUILD_SANITIZE:-}
if [ -n "$SANITIZE" ]; then
  case $SANITIZE in *[!a-z,]*) die "NIDHOGG_BUILD_SANITIZE takes a comma list such as address,undefined" ;; esac
  [ "$DEV" = 1 ] || die "NIDHOGG_BUILD_SANITIZE is for dev builds only: add --dev"
fi
LISP_CACHE=${NIDHOGG_BUILD_LISP_CACHE:-} LC="" LC_RESTORE=1
LC_ELC=0; [ "${NIDHOGG_BUILD_LISP_CACHE_ELC:-}" != 1 ] || LC_ELC=1
CONFIG_CACHE=${NIDHOGG_BUILD_CONFIG_CACHE:-} CFC=""
CARGO_TARGET=${NIDHOGG_BUILD_CARGO_TARGET:-} CT=""

UNAME=${NIDHOGG_BUILD_UNAME:-$(uname -s)}
PODMAN=${NIDHOGG_BUILD_PODMAN:-podman}
NPROC=${NIDHOGG_BUILD_NPROC:-$(nproc 2>/dev/null || sysctl -n hw.ncpu)}

if [ "$MODE" = container ]; then
  case " $DISTROS " in *" $DISTRO "*) ;; *) echo "unknown distro '$DISTRO'; one of: $DISTROS" >&2; exit 2 ;; esac
  case $DISTRO in
    fedora-45) IMAGE=registry.fedoraproject.org/fedora:45 ;;
    ubuntu-26.04) IMAGE=docker.io/library/ubuntu:26.04 ;;
    debian-13) IMAGE=docker.io/library/debian:13 ;;
    arch) IMAGE=docker.io/library/archlinux:latest ;;
  esac
  # NIDHOGG_BUILD_IMAGE=REF: run in REF instead, an image of the same distro with the packages installed (CI's
  # nidhogg-build-DISTRO, nidhogg/ci/image.sh); the script below then finds them all and runs no package manager
  IMAGE=${NIDHOGG_BUILD_IMAGE:-$IMAGE}
  [ "$UNAME" = Linux ] || [ "$DRY" = 1 ] || die "--container needs Linux (use --host on macOS)"
else
  case $UNAME in
    Darwin) DISTRO=macos ;;
    Linux) DISTRO=host ;;
    *) die "unsupported system: $UNAME" ;;
  esac
fi

[ -n "$PREFIX" ] || PREFIX=/opt/nidhogg/$VERSION
# the prefix is bind-mounted (and relabelled with :Z) in a container: refuse anything that is not a plain, absolute directory
case $PREFIX in /*) ;; *) die "--prefix must be an absolute path: $PREFIX" ;; esac
case /$PREFIX/ in */../*|*/./*) die "--prefix must not contain '.' or '..' components: $PREFIX" ;; esac
P_NORM=$(printf '%s' "$PREFIX" | sed 's|//*|/|g; s|/$||')
[ -n "$P_NORM" ] && [ "$P_NORM" != "${HOME%/}" ] || die "--prefix must not be / or your home directory: $PREFIX"
PREFIX=$P_NORM
if [ -n "$CHECK" ] && [ ! -f "$CHECK" ] && [ "$DRY" = 0 ]; then die "--check file not found: $CHECK"; fi

mem_available_kb() {
  if [ -n "${NIDHOGG_BUILD_MEMORY_KB:-}" ]; then echo "$NIDHOGG_BUILD_MEMORY_KB"
  elif [ "$UNAME" = Darwin ] && command -v "${NIDHOGG_BUILD_VMSTAT:-vm_stat}" >/dev/null 2>&1; then
    # macOS reclaims inactive, speculative and purgeable pages on demand, so they count as available
    "${NIDHOGG_BUILD_VMSTAT:-vm_stat}" |
      awk '/page size of/ {ps=$8} /^Pages (free|inactive|speculative|purgeable):/ {n+=$NF} END {printf "%d", n*ps/1024}'
  else awk '/^MemAvailable:/ {print $2}' /proc/meminfo
  fi
}
AVAIL_KB=$(mem_available_kb)
AVAIL_MIB=$((AVAIL_KB / 1024))

# memory floor, container and host alike: refused below 3 GiB (3072 MiB) available; the cap, and the jobs from it,
# are max(2, min(6, available - 1)) GiB.  An explicit --memory for a container is taken as given; on a host build an
# explicit --memory or --jobs means the caller takes responsibility: a warning, not a refusal.
CAP=$(((AVAIL_MIB - 1024) / 1024))
[ "$CAP" -le 6 ] || CAP=6
[ "$CAP" -ge 2 ] || CAP=2
if [ "$AVAIL_MIB" -lt 3072 ]; then
  if [ "$MODE" = host ] && { [ -n "$MEMORY" ] || [ -n "$JOBS" ]; }; then
    echo "build.sh: warning: a Nidhogg build needs 3072 MiB of free memory; only ${AVAIL_MIB} MiB are available (building anyway: --memory/--jobs given)" >&2
  elif [ "$MODE" = host ] || [ -z "$MEMORY" ]; then
    echo "build.sh: a Nidhogg build needs 3072 MiB (3 GiB) of free memory; only ${AVAIL_MIB} MiB are available" >&2
    exit 1
  fi
fi
if [ "$MODE" = container ]; then
  [ -n "$MEMORY" ] || MEMORY=${CAP}g
  case $MEMORY in *g) CAP=${MEMORY%g} ;; *) echo "--memory takes whole GiB, e.g. 6g" >&2; exit 2 ;; esac
  JOBS_FROM=$CAP
else
  JOBS_FROM=$CAP
fi
if [ -z "$JOBS" ]; then
  JOBS=$((JOBS_FROM * 10 / 15))   # one job per 1.5 GiB
  [ "$JOBS" -le "$NPROC" ] || JOBS=$NPROC
  [ "$JOBS" -ge 1 ] || JOBS=1
fi

if [ "$NAGLFAR" = 1 ]; then
  [ -n "$GLITNIR" ] || die "--naglfar needs Glitnir: leave out --without-glitnir"
  { [ "$MODE" = host ] && [ "$UNAME" = Darwin ]; } || [ "$DRYCONF" = 1 ] ||
    die "--naglfar builds on macOS (--host); elsewhere only --dry-config"
  CONF="--with-native-compilation=$NATIVE --with-tree-sitter --with-nidhogg-macos"
  # configure on Linux as for a darwin host (the result is config.h, never a binary)
  [ "$UNAME" = Darwin ] || CONF="$CONF --build=aarch64-apple-darwin24.6.0"
elif [ "$MODE" = host ] && [ "$UNAME" = Darwin ]; then
  CONF="--with-ns --with-native-compilation=$NATIVE --with-tree-sitter"
elif [ -n "$GLITNIR" ]; then
  CONF="--with-native-compilation=$NATIVE --with-tree-sitter --with-nidhogg-wayland"
else
  CONF="--with-native-compilation=$NATIVE --with-tree-sitter --without-x"
fi
# Glitnir (the compositor, Rust) is on by default on Linux and NS; --without-glitnir drops it and the Rust packages
CONF="$CONF${GLITNIR:+ $GLITNIR}"
PKGS="\$(sed \"s/#.*//\" \$S/nidhogg/deps/$DISTRO.build"
if [ -n "$GLITNIR" ]; then PKGS="$PKGS)"; else PKGS="$PKGS | grep -vxE 'cargo|rust|rustc')"; fi
[ "$WATCHDOG" = 0 ] || CONF="$CONF --with-nidhogg-watchdog"
[ "$WORKERS" = 0 ] || CONF="$CONF --with-nidhogg-workers"
[ "$HISTORY" = 0 ] || [ -z "$GLITNIR" ] || CONF="$CONF --with-nidhogg-history"
[ "$CHAOS" = 0 ] || CONF="$CONF --with-nidhogg-chaos"
[ "$CHECKING" = 0 ] || CONF="$CONF --enable-checking"
# the quotes are parsed by the build script, not here
[ -z "$SANITIZE" ] || CONF="$CONF CFLAGS='-g -O1 -fno-omit-frame-pointer -fsanitize=$SANITIZE' LDFLAGS='-fsanitize=$SANITIZE'"

TEMP_BUILD=0
if [ -z "$BUILD_DIR" ]; then
  TEMP_BUILD=1
  if [ "$DRY" = 1 ]; then BUILD_DIR=/tmp/nidhogg-build.XXXXXX; else BUILD_DIR=$(mktemp -d "${TMPDIR:-/tmp}/nidhogg-build.XXXXXX"); fi
elif [ "$DRY" = 0 ]; then
  mkdir -p "$BUILD_DIR"
  BUILD_DIR=$(cd "$BUILD_DIR" && pwd)
fi

if [ "$MODE" = container ]; then S=/src B=/build; else S=$ROOT B=$BUILD_DIR; fi
# The build links src/glitnir-target to the cargo target, and every later stage runs make (the pack stage's install
# runs cargo) through that link: a check or pack stage not given NIDHOGG_BUILD_CARGO_TARGET takes the one the build had.
CT_NOTE=$BUILD_DIR/.nidhogg-cargo-target
if [ -z "$CARGO_TARGET" ] && { [ "$STAGE" = check ] || [ "$STAGE" = pack ]; } && [ -f "$CT_NOTE" ]; then
  CARGO_TARGET=$(cat "$CT_NOTE")
fi
# the configure cache and the Glitnir cargo target, mounted in a container, as given on the host
for v in CONFIG_CACHE:CFC:/configcache CARGO_TARGET:CT:/cargotarget; do
  IFS=: read -r src dst mnt <<<"$v"
  [ -n "${!src}" ] || continue
  [ "$DRY" = 1 ] || { mkdir -p "${!src}"; printf -v "$src" '%s' "$(cd "${!src}" && pwd)"; }
  if [ "$MODE" = container ]; then printf -v "$dst" '%s' "$mnt"; else printf -v "$dst" '%s' "${!src}"; fi
done
[ -z "$CARGO_TARGET" ] || [ -n "$GLITNIR" ] || CT=""
if [ "$DRY" = 0 ] && { [ "$STAGE" = all ] || [ "$STAGE" = build ]; }; then
  if [ -n "$CT" ]; then printf '%s\n' "$CARGO_TARGET" > "$CT_NOTE"; else rm -f "$CT_NOTE"; fi
fi
if [ -n "$LISP_CACHE" ]; then
  if [ "$MODE" = container ]; then LC=/lispcache
  elif [ "$DRY" = 1 ]; then LC=$LISP_CACHE
  else mkdir -p "$LISP_CACHE"; LC=$(cd "$LISP_CACHE" && pwd); fi
  [ "$MODE" = host ] || { [ "$DRY" = 1 ] || { mkdir -p "$LISP_CACHE"; LISP_CACHE=$(cd "$LISP_CACHE" && pwd); }; }
fi

# the tarball: nidhogg-<version>-<distro>-<arch>.tar.xz (+ .sha256); OUT is its path, its basename is only a hint
UPSTREAM_VERSION=${VERSION%%-nidhogg.*}
COMMIT=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)
ARCH=$(uname -m)
DISTRO_TAG=$DISTRO
if [ "$MODE" = host ] && [ "$UNAME" = Linux ] && [ -r /etc/os-release ]; then
  DISTRO_TAG=$(. /etc/os-release; if [ "$ID" = arch ]; then echo arch; else echo "${ID}-${VERSION_ID}"; fi)
fi
if [ -n "$OUT" ]; then
  OUT_DIR=$(dirname "$OUT")
  [ "$DRY" = 1 ] || { mkdir -p "$OUT_DIR"; OUT_DIR=$(cd "$OUT_DIR" && pwd); }
  OUT_BASE=$(basename "$OUT")
  if [ "$MODE" = container ]; then OUT_IN=/out/$OUT_BASE; else OUT_IN=$OUT_DIR/$OUT_BASE; fi
fi

# the install into a staging tree and the tarball
out_steps() {
  printf 'OUTF=%q DISTRO_TAG=%q ARCH=%q COMMIT=%q J=%q CONF=%q\n' "$OUT_IN" "$DISTRO_TAG" "$ARCH" "$COMMIT" "$JOBS" "$CONF"
  if [ "$MODE" = host ] && [ "$UNAME" = Darwin ]; then
    echo 'phase install'
    echo 'logged "$B/install.log" make install'
    echo 'pk=$B/pkg/nidhogg-$V; rm -rf "$B/pkg"; mkdir -p "$pk"; mv nextstep/Emacs.app "$pk/Nidhogg.app"; info=$pk/BUILDINFO; rt=$pk/deps.runtime; lic=$pk/licenses'
  else
    echo 'phase install'
    echo 'rm -rf "$B/stage"'
    echo "logged \"\$B/install.log\" make install DESTDIR=$B/stage"
    echo 'pk=$B/pkg/nidhogg-$V; rm -rf "$B/pkg"; mkdir -p "$B/pkg"; cp -a "$B/stage$P" "$pk"; dumplinks "$pk"; mkdir -p "$pk/share/nidhogg"; info=$pk/share/nidhogg/BUILDINFO; rt=$pk/share/nidhogg/deps.runtime; lic=$pk/share/nidhogg/licenses'
    echo 'cp "$S/nidhogg/env.sh" "$pk/share/nidhogg/env.sh"'
  fi
  cat <<'BODY'
cp "$S/nidhogg/deps/$DISTRO_TAG.runtime" "$rt"
mkdir -p "$lic" && cp -a "$S/nidhogg/licenses/." "$lic/" && cp "$S/nidhogg/THIRD_PARTY.md" "$lic/"
printf 'version=%s\ncommit=%s\ndistro=%s\narch=%s\nprefix=%s\nconfigure=%s\ndate=%s\n' \
  "$V" "$COMMIT" "$DISTRO_TAG" "$ARCH" "$P" "$CONF" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$info"
phase tarball
tar -C "$B/pkg" -cJf "$OUTF" "nidhogg-$V"
(cd "$(dirname "$OUTF")" && { sha256sum "$(basename "$OUTF")" 2>/dev/null || shasum -a 256 "$(basename "$OUTF")"; } > "$OUTF.sha256")
BODY
}

# --- the build script: the same steps in a container and on the host --------------------------------------
script() {
  printf 'S=%q B=%q P=%q V=%q UPV=%q HB=%q\n' "$S" "$B" "$PREFIX" "$VERSION" "$UPSTREAM_VERSION" "${NIDHOGG_BUILD_HEARTBEAT:-300}"
  [ -z "$LISP_CACHE" ] || printf 'LC=%q LC_RESTORE=%q LC_ELC=%q\n' "$LC" "$LC_RESTORE" "$LC_ELC"
  [ -z "$CFC" ] || printf 'CFC=%q\n' "$CFC"
  [ -z "$CT" ] || printf 'CT=%q COMMIT=%q\n' "$CT" "$COMMIT"
  printf 'MODE_ID=%q\n' "$NATIVE${SANITIZE:+-$SANITIZE}"
  printf 'CONF_ID=%q\n' "--prefix=$PREFIX $CONF"
  # CONF_ID without what the byte-compiled Lisp does not depend on: the native-compilation mode and the window system
  elc_conf=""
  for w in --prefix=$PREFIX $CONF; do
    case $w in --with-native-compilation=*|--with-ns|--with-nidhogg-macos|--enable-checking) ;; *) elc_conf="$elc_conf $w" ;; esac
  done
  printf 'ELC_CONF_ID=%q\n' "${elc_conf# }"
  # leak reports from configure's and the dump's short-lived programs are noise.  detect_stack_use_after_return=0 as
  # etc/DEBUG says: the runtime defaults it on (GCC 16's does), which moves main's locals to a heap "fake stack",
  # so stack_bottom (&stack_bottom_variable) is not on the stack and the GC's mark_c_stack scans from the real stack
  # into unmapped memory: temacs segfaulted at its first GC while dumping bootstrap-emacs.pdmp
  [ -z "$SANITIZE" ] || echo 'export ASAN_OPTIONS=detect_leaks=0:abort_on_error=0:detect_stack_use_after_return=0 UBSAN_OPTIONS=print_stacktrace=1'
  # the 100 MB-class workers rows read this to size themselves down under the sanitizers (nidhogg-workers--test-size-scale)
  [ -z "${NIDHOGG_WORKERS_TEST_SIZE_SCALE:-}" ] || printf 'export NIDHOGG_WORKERS_TEST_SIZE_SCALE=%q\n' "$NIDHOGG_WORKERS_TEST_SIZE_SCALE"
  # where the test grammars are, inside the container (the pre-flight mounts them): the treesit rows skip without them
  [ -z "${NIDHOGG_TEST_GRAMMARS:-}" ] || printf 'export NIDHOGG_TEST_GRAMMARS=%q\n' "$NIDHOGG_TEST_GRAMMARS"
  # a chaos build's seed (the pre-flight's P3 runs seeds 1..N): without it in the container every seed ran as seed 0
  [ -z "${NIDHOGG_TEST_SEED:-}" ] || printf 'export NIDHOGG_TEST_SEED=%q\n' "$NIDHOGG_TEST_SEED"
  if [ "$MODE" = container ]; then
    # each install is skipped when its packages are all installed already (a prebuilt image: NIDHOGG_BUILD_IMAGE,
    # NIDHOGG_BUILD_IMAGE_CACHE), so such a build runs no package manager and downloads nothing.  rpm answers for a
    # name another package provides; dpkg does not (Ubuntu 26.04's libselinux1-dev), so an apt distro's image is known by
    # the copy of this list it was built from (nidhogg/ci/images/build-*.Containerfile)
    case $DISTRO in
      ubuntu-*|debian-*)
        echo "if ! { cmp -s /usr/share/nidhogg-image/$DISTRO.build \$S/nidhogg/deps/$DISTRO.build && dpkg -s ccache >/dev/null 2>&1; }; then"
        [ -z "${NIDHOGG_BUILD_PKG_CACHE:-}" ] || echo 'rm -f /etc/apt/apt.conf.d/docker-clean; echo "Binary::apt::APT::Keep-Downloaded-Packages \"true\";" > /etc/apt/apt.conf.d/keep'
        echo "apt-get update"
        echo "DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends $PKGS${NIDHOGG_BUILD_CCACHE:+ ccache}"
        echo "fi" ;;
      arch)
        echo "pacman -Syu --noconfirm --needed $PKGS${NIDHOGG_BUILD_CCACHE:+ ccache}" ;;
      *) echo "rpm -q --quiet --whatprovides $PKGS${NIDHOGG_BUILD_CCACHE:+ ccache} 2>/dev/null || dnf -y --setopt=install_weak_deps=False ${NIDHOGG_BUILD_PKG_CACHE:+--setopt=keepcache=1 }install $PKGS${NIDHOGG_BUILD_CCACHE:+ ccache}" ;;
    esac
    # Fedora ships the sanitizer runtimes apart from gcc
    if [ -n "$SANITIZE" ]; then case $DISTRO in ubuntu-*|debian-*|arch) ;; *) echo "rpm -q --quiet libasan libubsan libtsan || dnf -y --setopt=install_weak_deps=False install libasan libubsan libtsan" ;; esac; fi
    # the pre-flight, Fedora only: packages only its tests need (gnutls-serv), and rr for P5; NIDHOGG_CHECK_RR is the
    # recorder's command, the prefix of each test run.  Not in deps/: the builds' caches are keyed on that file
    [ -z "${NIDHOGG_CHECK_PACKAGES:-}${NIDHOGG_CHECK_RR:-}" ] || echo "rpm -q --quiet --whatprovides ${NIDHOGG_CHECK_PACKAGES:-} ${NIDHOGG_CHECK_RR:+rr} 2>/dev/null || dnf -y --setopt=install_weak_deps=False install ${NIDHOGG_CHECK_PACKAGES:-} ${NIDHOGG_CHECK_RR:+rr}"
    [ -z "${NIDHOGG_BUILD_CCACHE:-}" ] || echo 'export CCACHE_DIR=/ccache CCACHE_COMPILERCHECK=content CCACHE_MAXSIZE=500M CC="ccache gcc"; ccache -z >/dev/null'
  elif [ -n "${NIDHOGG_BUILD_CCACHE:-}" ]; then
    printf 'export CCACHE_DIR=%q CCACHE_COMPILERCHECK=%q CCACHE_MAXSIZE=500M CC=%q; ccache -z >/dev/null\n' \
      "$NIDHOGG_BUILD_CCACHE" '%compiler% -v' "ccache ${CC:-gcc}"
  fi
  cat <<'BODY'
# relative links so that a relocated binary finds its dump beside itself; the installed binary's own dump (a reused
# prefix keeps the dumps of earlier builds): the one it loads when given it, since it refuses a dump not built for it
dumplinks() {
  root=$1
  for f in "$root"/libexec/emacs/*/*/emacs-*.pdmp; do
    "$root/bin/emacs-$UPV" --dump-file="$f" --fingerprint >/dev/null 2>&1 || continue
    rel=${f#"$root"/}
    for n in nidhogg emacs "emacs-$UPV"; do ln -sf "../$rel" "$root/bin/$n.pdmp"; done
    return 0
  done
}
phase() { PHASE=$1; echo "== $*"; }
# run a logged step; a heartbeat line every HB seconds while it runs; on failure the last 100 lines of its log
logged() {
  log=$1; shift
  "$@" >> "$log" 2>&1 & cmd=$!
  t=0
  while kill -0 "$cmd" 2>/dev/null; do
    sleep 1; t=$((t + 1))
    if [ $((t % HB)) -eq 0 ]; then echo "... $PHASE still running, $((t / 60)) min"; fi
  done
  rc=0; wait "$cmd" || rc=$?
  if [ "$rc" != 0 ]; then
    echo "FAILED: $* (log: $log); last 100 lines:" >&2
    tail -n 100 "$log" >&2
    exit 1
  fi
}
# a check target (its last argument): a failure is not the end of the run, check_rerun judges every target's log
CHECK_ALL=""
checked() {
  all=$1; shift
  tlog="$B/check-logs/${!#}.log"
  mkdir -p "$(dirname "$tlog")"; : > "$tlog"
  ( logged "$tlog" "$@" ) || true
  cat "$tlog" >> "$all"
  CHECK_ALL="$CHECK_ALL ${!#}"
}
# one more run of each failed test file; a test that then passes must be listed in nidhogg/test/flaky.txt with its cause
# (also checks that the list names no test that is gone); the first runs' logs are $B/check-logs/TARGET.log
check_rerun() {
  # shellcheck disable=SC2086
  bash "$S/nidhogg/test/check-rerun.sh" --flaky "$S/nidhogg/test/flaky.txt" --src "$B/src/test" \
    --make "make -C $B/obj/test" "$B/check-logs" $CHECK_ALL || { echo "FAILED: check (see above)" >&2; exit 1; }
}
mkdir -p "$B/src" "$B/obj"
BODY
  # Glitnir's floor is configure's; a distro that carries an older Rust (Debian 13) builds with a pinned private toolchain
  # in the build directory: network once, never root.  rustup-init comes from the versioned archive (the unversioned file
  # changes under a pinned hash), one sha256 per target, the same pins as drgnx's install.sh
  if [ -n "$GLITNIR" ] && [ "$UNAME" = Linux ]; then cat <<'BODY'
R="$B/rust" RUST_PIN=1.93.1 RUSTUP_VERSION=1.28.2
rust_recent() {
  v=$(rustc --version 2>/dev/null | sed -n 's/^rustc \([0-9]*\)\.\([0-9]*\).*/\1 \2/p')
  [ -n "$v" ] && { [ "${v% *}" -gt 1 ] || [ "${v#* }" -ge 93 ]; }
}
if [ -x "$R/cargo/bin/rustc" ]; then export RUSTUP_HOME="$R/home" CARGO_HOME="$R/cargo" PATH="$R/cargo/bin:$PATH"; fi
if ! rust_recent; then
  phase "Rust $RUST_PIN (private, in the build directory)"
  case $(uname -m) in
    x86_64) sha=20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c ;;
    aarch64) sha=e3853c5a252fca15252d07cb23a1bdd9377a8c6f3efa01531109281ae47f841c ;;
    *) echo "build.sh: no pinned Rust for $(uname -m): install Rust 1.93 or newer" >&2; exit 1 ;;
  esac
  mkdir -p "$R"
  curl -fsSL -o "$R/rustup-init" "https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/$(uname -m)-unknown-linux-gnu/rustup-init" ||
    { echo "build.sh: could not download rustup-init" >&2; exit 1; }
  echo "$sha  $R/rustup-init" | sha256sum -c --quiet ||
    { echo "build.sh: rustup-init is not the pinned file: nothing was run" >&2; rm -f "$R/rustup-init"; exit 1; }
  chmod +x "$R/rustup-init"
  export RUSTUP_HOME="$R/home" CARGO_HOME="$R/cargo" PATH="$R/cargo/bin:$PATH"
  logged "$B/rust.log" "$R/rustup-init" -y --no-modify-path --profile minimal --default-toolchain "$RUST_PIN"
  rm -f "$R/rustup-init"
fi
BODY
  fi
  case $STAGE in all|build) ;; *) echo 'cd "$B/obj"' ;; esac
  if [ "$STAGE" = all ] || [ "$STAGE" = build ]; then cat <<'BODY'
rm -f "$B"/*.log
# GNU cp's -u (copy only newer) keeps an incremental build dir's mtimes; BSD cp (macOS) has no -u: copy everything
if cp -u /dev/null "$B/.cp-u-probe" 2>/dev/null; then CP_UPDATE="cp -a -u"; else CP_UPDATE="cp -pR"; fi; rm -f "$B/.cp-u-probe"
for e in "$S"/* "$S"/.[!.]*; do [ -e "$e" ] || continue; [ "${e##*/}" = .git ] || $CP_UPDATE "$e" "$B/src/"; done
# a source whose content differs from the build dir's copy is copied with a fresh mtime: cp -u skips a file that is not newer
# (an edit that kept an older mtime, e.g. after a checkout of another commit), and make then keeps the stale .elc/.o
for d in lisp leim src lib lib-src lwlib; do
  [ -d "$S/$d" ] || continue
  (cd "$S" && find "$d" -type f) | while IFS= read -r f; do [ -e "$B/src/$f" ] && ! cmp -s "$S/$f" "$B/src/$f" && cp "$S/$f" "$B/src/$f"; done || true
done
cd "$B/src"
BODY
  if [ -n "$LISP_CACHE" ]; then cat <<'BODY'
# the compiled-Lisp cache (used only when made by the same compiler, architecture and configure line: the .eln files sit
# under native-lisp/VERSION-HASH/, a hash of the primitives and the configure options, --prefix included, so another
# configuration's .eln are never loaded): restored files all get one timestamp, newer than every source (and equal among themselves, so
# no .eln is older than its .elc), so that make sees them as up to date
cc_id() { printf '%s %s %s %s\n' "$(gcc --version | head -n 1)" "$(uname -m)" "$MODE_ID" "$CONF_ID"; }
# builds of the same key share DIR: a restore holds DIR/.lock shared, a save exclusive (where flock exists)
lc_lock() { if command -v flock >/dev/null; then exec 9>"$LC/.lock"; flock "$1" 9; fi; }
lc_unlock() { if command -v flock >/dev/null; then flock -u 9; exec 9>&-; fi; }
# the same without the native-compilation mode and the window system: what the byte-compiled part alone depends on
cc_elc_id() { printf '%s %s %s\n' "$(gcc --version | head -n 1)" "$(uname -m)" "$ELC_CONF_ID"; }
lc_lock -s
rm -f "$B/eln-restored.list"
if [ "$LC_RESTORE" = 1 ] && [ -f "$LC/stamp" ] && [ "$(cat "$LC/stamp")" = "$(cc_id)" ]; then
  phase restore compiled Lisp
  # obj (the .eln files) may be missing: a caller that keys them apart restores only the byte-compiled part
  parts=src; [ ! -d "$LC/obj" ] || parts="src obj"
  tar -C "$LC" -cf - $parts | tar -C "$B" -xf -
  touch "$B/.restored"
  (cd "$LC/src" && find . -type f) | while IFS= read -r f; do touch -r "$B/.restored" "$B/src/$f"; done
  [ ! -d "$LC/obj" ] || (cd "$LC/obj" && find . -type f) | while IFS= read -r f; do touch -r "$B/.restored" "$B/obj/$f"; done
  # the restored .eln files, which the guard checks against the new binary after make
  [ ! -d "$LC/obj/native-lisp" ] || (cd "$LC/obj" && find native-lisp -name '*.eln') > "$B/eln-restored.list"
elif [ "$LC_RESTORE" = 1 ] && [ "$LC_ELC" = 1 ] && [ -f "$LC/stamp-elc" ] && [ "$(cat "$LC/stamp-elc")" = "$(cc_elc_id)" ]; then
  # NIDHOGG_BUILD_LISP_CACHE_ELC=1: the byte-compiled part of an entry another mode or window system made (the .eln files,
  # which depend on both, are not taken)
  phase restore byte-compiled Lisp
  tar -C "$LC" -cf - src | tar -C "$B" -xf -
  touch "$B/.restored"
  (cd "$LC/src" && find . -type f) | while IFS= read -r f; do touch -r "$B/.restored" "$B/src/$f"; done
fi
lc_unlock
BODY
  fi
  cat <<'BODY'
phase autogen
logged "$B/autogen.log" ./autogen.sh
cd "$B/obj"
BODY
  if [ "$DEV" = 1 ] || [ "$PRELOADED" = 1 ]; then
    # loadup.el loads lisp/site-init.el when present, and the dump keeps what it sets (the build copy only, never the source tree)
    echo "printf '%s\\n' ';;; dev build (nidhogg/build.sh --dev): no background native compilation' '(setq native-comp-jit-compilation nil)' > \"\$B/src/lisp/site-init.el\""
  fi
  if [ -n "$CT" ]; then cat <<'BODY'
# The crates cargo fetches (no vendor/ unpacked; Cargo.lock pins them) are kept with the cargo target: the registry's
# index and downloaded .crate files only, linked into the build's cargo home, never the home itself (no unpacked
# sources, tools or credentials).
mkdir -p "$CT/registry/index" "$CT/registry/cache" "$B/obj/src/glitnir-cargo-home/registry"
ln -sfn "$CT/registry/index" "$B/obj/src/glitnir-cargo-home/registry/index"
ln -sfn "$CT/registry/cache" "$B/obj/src/glitnir-cargo-home/registry/cache"
BODY
  fi
  echo "phase configure"
  if [ -n "$CONFIG_CACHE" ]; then
    # autoconf's cache of its checks, through a site file (a --cache-file option would join the configure line that
    # system-configuration-options and so the .eln ABI hash record).  A configure that fails with it runs again cold.
    cat <<'BODY'
printf 'cache_file=%s\n' "$B/config.cache" > "$B/config.site"
rm -f "$B/config.cache"
if [ -f "$CFC/config.cache" ]; then cp "$CFC/config.cache" "$B/config.cache"; echo "configure: with the cached checks"; fi
BODY
    echo "if ! ( CONFIG_SITE=\"\$B/config.site\" logged \"\$B/configure.log\" \"\$B/src/configure\" --prefix=\"\$P\" $CONF ); then"
    echo '  [ -f "$B/config.cache" ] || exit 1'
    echo '  echo "configure failed with the cached checks: running it again without them" >&2; rm -f "$B/config.cache"; : > "$B/configure.log"'
    echo "  CONFIG_SITE=\"\$B/config.site\" logged \"\$B/configure.log\" \"\$B/src/configure\" --prefix=\"\$P\" $CONF || { echo \"configure failed; the build packages are listed in nidhogg/deps/$DISTRO.build\" >&2; exit 1; }"
    echo "fi"
    echo 'cp "$B/config.cache" "$CFC/config.cache.new" && mv "$CFC/config.cache.new" "$CFC/config.cache"'
  else
    echo "logged \"\$B/configure.log\" \"\$B/src/configure\" --prefix=\"\$P\" $CONF || { echo \"configure failed; the build packages are listed in nidhogg/deps/$DISTRO.build\" >&2; exit 1; }"
  fi
  cat <<'BODY'
grep -Eq 'native lisp compiler\? +yes' "$B/configure.log" || { echo "configure found no native compilation (libgccjit); see $B/configure.log" >&2; exit 1; }
BODY
  if [ "$DRYCONF" = 1 ]; then echo 'echo "dry config: $B/obj/src/config.h"; exit 0'; fi
  if [ -n "$CT" ]; then cat <<'BODY'
# Glitnir's cargo target directory from NIDHOGG_BUILD_CARGO_TARGET (a cache): src/glitnir-target is a link to it.  Cargo
# judges each restored artifact by its sources' times (the caller gives the files their commits' times, and the files
# changed since the commit the target names, .nidhogg-source-commit, the current time).  The libraries make links are
# dated 1970, so make always runs cargo (its prerequisites list only two directory levels of sources), which then
# rebuilds what is stale and nothing else.
mkdir -p "$B/obj/src"
[ -L "$B/obj/src/glitnir-target" ] || { rm -rf "$B/obj/src/glitnir-target"; ln -s "$CT" "$B/obj/src/glitnir-target"; }
for a in "$CT"/release/lib*.a; do [ ! -f "$a" ] || touch -t 197001020000 "$a"; done
BODY
  fi
  echo "phase make -j$JOBS"
  echo "logged \"\$B/make.log\" make -j$JOBS"
  # the commit the target was built from, only after a make that passed (a failed one may have left a crate unbuilt)
  [ -z "$CT" ] || echo 'printf "%s\n" "$COMMIT" > "$CT/.nidhogg-source-commit"'
  if [ -n "$LISP_CACHE" ]; then cat <<'BODY'
# the guard (nidhogg/eln-guard.el): restored .eln files must be this binary's (its ABI facts, its native-lisp
# directory, a sample loads); otherwise exit 75, and build.sh discards the restored Lisp and builds again, cold
if [ -s "$B/eln-restored.list" ]; then
  phase check the restored .eln files
  to=""; if command -v timeout >/dev/null; then to="timeout 900"; fi
  if $to "$B/obj/src/emacs" -Q --batch -l "$S/nidhogg/eln-guard.el" -f nidhogg-eln-guard-check \
       "$B/eln-restored.list" "$B/obj" "$LC/obj/.eln-abi" 16 > "$B/eln-guard.log" 2>&1; then
    tail -n 1 "$B/eln-guard.log"
  else
    cat "$B/eln-guard.log" >&2
    echo "build.sh: the restored compiled Lisp is not this build's: discarding it" >&2
    exit 75
  fi
fi
phase save compiled Lisp
lc_lock -x
rm -rf "$LC/stamp" "$LC/stamp-elc" "$LC/src" "$LC/obj"
mkdir -p "$LC/src" "$LC/obj"
# the files make generated under lisp/ and leim/ (those the sources do not have), and the .eln files with the facts
# the guard compares (.eln-abi)
(cd "$B/src" && find lisp leim -type f | while IFS= read -r f; do [ -e "$S/$f" ] || echo "$f"; done > "$B/lisp-cache.list" && tar -cf - -T "$B/lisp-cache.list") | tar -C "$LC/src" -xf -
if [ -d "$B/obj/native-lisp" ]; then
  tar -C "$B/obj" -cf - native-lisp | tar -C "$LC/obj" -xf -
  "$B/obj/src/emacs" -Q --batch -l "$S/nidhogg/eln-guard.el" -f nidhogg-eln-guard-facts > "$LC/obj/.eln-abi"
fi
cc_id > "$LC/stamp"
cc_elc_id > "$LC/stamp-elc"
lc_unlock
BODY
  fi
  fi
  if [ -n "${NIDHOGG_BUILD_CCACHE:-}" ] && [ "$STAGE" != pack ]; then echo 'ccache -s || true'; fi
  if [ -n "$CHECK" ] && { [ "$STAGE" = all ] || [ "$STAGE" = check ]; }; then
    # test/Makefile runs each test as $(GDB) ... emacs: the recorder goes there, its traces under the build dir
    rr_arg=""
    if [ -n "${NIDHOGG_CHECK_RR:-}" ]; then
      echo 'export _RR_TRACE_DIR="$B/rr-traces"; mkdir -p "$_RR_TRACE_DIR"'
      rr_arg="$(printf '%q' "GDB=$NIDHOGG_CHECK_RR") "
    fi
    for t in $(sed "s/#.*//" "$CHECK" 2>/dev/null || echo "<targets of $CHECK>"); do
      echo "phase check $t"
      echo "checked \"\$B/check.log\" make -C test $rr_arg$t"
    done
    # each container installs its packages afresh: a trace replays in a later one only with its files packed in
    # and the trace's name, in a log so that it comes back from a remote runner
    [ -z "$rr_arg" ] || echo 'rr pack "$_RR_TRACE_DIR/latest-trace" || echo "build.sh: rr pack failed"; readlink -f "$_RR_TRACE_DIR/latest-trace" > "$B/check-logs/rr-trace.log"'
    echo "check_rerun"
  fi
  if [ "$STAGE" = build ] || [ "$STAGE" = check ]; then :
  elif [ -n "$OUT" ]; then
    out_steps
  elif [ "$MODE" = host ] && [ "$UNAME" = Darwin ]; then
    echo "phase install"
    echo "logged \"\$B/install.log\" make install"
    echo "rm -rf \"\$P/Nidhogg.app\" && mkdir -p \"\$P\" && mv nextstep/Emacs.app \"\$P/Nidhogg.app\""
  else
    echo "phase install"
    echo "logged \"\$B/install.log\" make install"
    echo 'dumplinks "$P"'
  fi
}

# --- run --------------------------------------------------------------------------------------------------
if [ "$MODE" = container ]; then
  if [ "$STAGE" = build ] || [ "$STAGE" = check ]; then MOUNT=""
  elif [ -n "$OUT" ]; then MOUNT="$OUT_DIR:/out:Z"; else MOUNT="$PREFIX:$PREFIX:Z"; [ "$DRY" = 1 ] || mkdir -p "$PREFIX"; fi
  # NIDHOGG_BUILD_IMAGE_CACHE=1 (Fedora): the packages go into a local image once, keyed on the base image and the package
  # list, and every run starts from it: the script's own install then finds everything there.  A pre-flight seed (one
  # container, tests only) otherwise reinstalls some 350 packages and spends minutes on it.
  if [ -n "${NIDHOGG_BUILD_IMAGE_CACHE:-}" ] && [ "$DRY" = 0 ] && [ "${DISTRO#fedora-}" != "$DISTRO" ]; then
    pk=$(sed 's/#.*//' "$ROOT/nidhogg/deps/$DISTRO.build")
    [ -n "$GLITNIR" ] || pk=$(printf '%s\n' "$pk" | grep -vxE 'cargo|rust|rustc')
    pk="$(echo $pk) ${NIDHOGG_BUILD_CCACHE:+ccache} ${NIDHOGG_CHECK_PACKAGES:-} ${NIDHOGG_CHECK_RR:+rr}"
    ihash=$(printf '%s\n%s\n' "$IMAGE" "$pk" | sha256sum | cut -c1-12)
    CIMAGE=localhost/nidhogg-build:$DISTRO-$ihash
    if ! "$PODMAN" image exists "$CIMAGE"; then
      echo "build.sh: making the package image $CIMAGE (once)"
      ctx=$(mktemp -d)
      printf 'FROM %s\nRUN dnf -y --setopt=install_weak_deps=False install %s && dnf clean all\n' "$IMAGE" "$pk" > "$ctx/Containerfile"
      "$PODMAN" build --memory 4g -t "$CIMAGE" -f "$ctx/Containerfile" "$ctx" >&2 || { rm -rf "$ctx"; die "the package image failed"; }
      rm -rf "$ctx"
    fi
    IMAGE=$CIMAGE
  fi
  NAME=nidhogg-build-$$
  CACHE_ARGS=""
  # the caches are shared by builds running at once: :z (a shared label), since :Z relabels a directory for one container
  # and a second build's :Z locks the first one out of it mid-build; the read-only source mount is :z for the same reason
  # (builds, GUI runs and crate checks of one checkout run at once)
  if [ -n "${NIDHOGG_BUILD_PKG_CACHE:-}" ]; then
    case $DISTRO in ubuntu-*|debian-*) CACHE_ARGS="-v $NIDHOGG_BUILD_PKG_CACHE/apt:/var/cache/apt/archives:z" ;;
      arch) CACHE_ARGS="-v $NIDHOGG_BUILD_PKG_CACHE/pacman:/var/cache/pacman/pkg:z" ;;
      *) CACHE_ARGS="-v $NIDHOGG_BUILD_PKG_CACHE/dnf:/var/cache/libdnf5:z" ;; esac
  fi
  [ -z "${NIDHOGG_BUILD_CCACHE:-}" ] || CACHE_ARGS="$CACHE_ARGS -v $NIDHOGG_BUILD_CCACHE:/ccache:z"
  [ -z "$LISP_CACHE" ] || CACHE_ARGS="$CACHE_ARGS -v $LISP_CACHE:/lispcache:z"
  [ -z "$CFC" ] || CACHE_ARGS="$CACHE_ARGS -v $CONFIG_CACHE:/configcache:z"
  [ -z "$CT" ] || CACHE_ARGS="$CACHE_ARGS -v $CARGO_TARGET:/cargotarget:z"
  # extra podman options (words, no quoting): nidhogg/test/preflight.sh pins a check run with --cpuset-cpus=0 or --cpus=0.5
  [ -z "${NIDHOGG_BUILD_PODMAN_ARGS:-}" ] || CACHE_ARGS="$CACHE_ARGS $NIDHOGG_BUILD_PODMAN_ARGS"
  CMD_HEAD="$PODMAN run --rm --name $NAME --memory $MEMORY --memory-swap $MEMORY -v $ROOT:/src:ro,z -v $BUILD_DIR:/build:Z ${MOUNT:+-v $MOUNT} $CACHE_ARGS $IMAGE"
  if [ "$DRY" = 1 ]; then
    echo "$CMD_HEAD bash -euo pipefail -c '"
    script
    echo "'"
    exit 0
  fi
  command -v "$PODMAN" >/dev/null 2>&1 || die "podman not found (podman only; no docker)"
  echo "+ $CMD_HEAD bash -euo pipefail -c <script>"
  [ -z "${NIDHOGG_BUILD_CCACHE:-}" ] || mkdir -p "$NIDHOGG_BUILD_CCACHE"
  [ -z "$LISP_CACHE" ] || mkdir -p "$LISP_CACHE"
  [ -z "${NIDHOGG_BUILD_PKG_CACHE:-}" ] || mkdir -p "$NIDHOGG_BUILD_PKG_CACHE/apt" "$NIDHOGG_BUILD_PKG_CACHE/dnf" "$NIDHOGG_BUILD_PKG_CACHE/pacman"
  # stop and remove our own container (and only it) however we end
  cleanup() {
    [ -z "${RUN_PID:-}" ] || kill "$RUN_PID" 2>/dev/null || true
    "$PODMAN" stop -t 10 "$NAME" >/dev/null 2>&1 || true
    "$PODMAN" rm -f "$NAME" >/dev/null 2>&1 || true
  }
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  run() {
    # in the background so that a signal reaches the trap at once
    # shellcheck disable=SC2086
    "$PODMAN" run --rm --name "$NAME" --memory "$MEMORY" --memory-swap "$MEMORY" \
      -v "$ROOT:/src:ro,z" -v "$BUILD_DIR:/build:Z" ${MOUNT:+-v} ${MOUNT:+"$MOUNT"} $CACHE_ARGS "$IMAGE" \
      bash -euo pipefail -c "$(script)" &
    RUN_PID=$!
    rc=0; wait "$RUN_PID" || rc=$?
    RUN_PID=""
  }
else
  if [ "$DRY" = 1 ]; then script; exit 0; fi
  echo "+ host build in $BUILD_DIR (make -j$JOBS)"
  run() { rc=0; bash -euo pipefail -c "$(script)" || rc=$?; }
fi
run
# 75: the guard found the restored compiled Lisp is not this build's (eln-guard.el).  Once, cold: the build tree goes
# (ccache keeps the C side quick) and nothing is restored; the new Lisp is saved over the stale entry
if [ "$rc" = 75 ]; then
  echo "build.sh: building again without the restored compiled Lisp (the C side from ccache, the Lisp cold)" >&2
  [ -z "${GITHUB_ACTIONS:-}" ] || echo "::warning::the restored compiled Lisp failed the .eln guard (eln-guard.log): built cold; its cache key misses an input"
  rm -rf "$BUILD_DIR/src" "$BUILD_DIR/obj"
  LC_RESTORE=0
  run
fi
if [ "$rc" = 0 ]; then
  [ "$TEMP_BUILD" = 0 ] || rm -rf "$BUILD_DIR"
else
  echo "build.sh: the build failed; logs are in $BUILD_DIR" >&2
  exit 1
fi
