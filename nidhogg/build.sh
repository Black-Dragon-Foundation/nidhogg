#!/usr/bin/env bash
# Build Nidhogg in a capped podman container or on the host.  See README.md ("Building").
#   nidhogg/build.sh (--container DISTRO | --host) [--prefix DIR] [--jobs N] [--memory SIZE]
#                    [--build-dir DIR] [--check FILE] [--out FILE] [--stage STAGE] [--dry-run]
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
# Exit: 0 ok, 1 build/check failed or not enough memory, 2 usage.
# Test overrides: NIDHOGG_BUILD_UNAME, NIDHOGG_BUILD_NPROC, NIDHOGG_BUILD_MEMORY_KB, NIDHOGG_BUILD_PODMAN,
# NIDHOGG_BUILD_VMSTAT (a vm_stat stand-in), NIDHOGG_BUILD_HEARTBEAT (seconds between "still running" lines, default 300).
# NIDHOGG_BUILD_PKG_CACHE=DIR keeps the container's downloaded packages (dnf, apt) in DIR between builds.
# NIDHOGG_BUILD_CCACHE=DIR (container builds) compiles the C code through ccache with its cache in DIR (ccache is installed
# in the container; the compiler is checked by content, so a changed gcc never hits).
# NIDHOGG_BUILD_LISP_CACHE=DIR keeps the build's compiled Lisp (the generated files under lisp/ and leim/: .elc, loaddefs, ...,
# and obj/native-lisp: the .eln files) in DIR: written after `make`, restored (made newer than every source) before it.  The
# caller keys DIR on everything that determines the output (the Lisp and C sources, build.sh, the distro, the compiler): an
# entry is only ever reused for the exact same inputs.
# bash 3.2-safe (macOS): none of the bash 4 features.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
VERSION=$(head -n 1 "$ROOT/nidhogg/version")
DISTROS="fedora-44 fedora-45 ubuntu-26.04"

usage() {
  cat >&2 <<USAGE
usage: nidhogg/build.sh (--container DISTRO | --host) [--prefix DIR] [--jobs N] [--memory SIZE]
                        [--build-dir DIR] [--check FILE] [--out FILE] [--stage build|check|pack] [--dev] [--dry-run]
  DISTRO: $DISTROS
USAGE
  exit 2
}
die() { echo "build.sh: $*" >&2; exit 1; }

MODE="" DISTRO="" PREFIX="" JOBS="" MEMORY="" BUILD_DIR="" CHECK="" OUT="" DRY=0 STAGE=all
DEV=0; [ "${NIDHOGG_BUILD_DEV:-}" != 1 ] || DEV=1
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
    --dev) DEV=1; shift ;;
    --dry-run) DRY=1; shift ;;
    *) echo "unknown option: $1" >&2; usage ;;
  esac
done
[ -n "$MODE" ] || usage
if [ "$DEV" = 1 ] && { [ -n "$OUT" ] || [ "$STAGE" = pack ]; }; then
  die "--dev builds are never packed (no --out, no --stage pack): a dev build is not a release; build without --dev"
fi
NATIVE=aot; [ "$DEV" = 0 ] || NATIVE=yes
LISP_CACHE=${NIDHOGG_BUILD_LISP_CACHE:-} LC=""

UNAME=${NIDHOGG_BUILD_UNAME:-$(uname -s)}
PODMAN=${NIDHOGG_BUILD_PODMAN:-podman}
NPROC=${NIDHOGG_BUILD_NPROC:-$(nproc 2>/dev/null || sysctl -n hw.ncpu)}

if [ "$MODE" = container ]; then
  case " $DISTROS " in *" $DISTRO "*) ;; *) echo "unknown distro '$DISTRO'; one of: $DISTROS" >&2; exit 2 ;; esac
  case $DISTRO in
    fedora-44) IMAGE=registry.fedoraproject.org/fedora:44 ;;
    fedora-45) IMAGE=registry.fedoraproject.org/fedora:45 ;;
    ubuntu-26.04) IMAGE=docker.io/library/ubuntu:26.04 ;;
  esac
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

if [ "$MODE" = host ] && [ "$UNAME" = Darwin ]; then
  CONF="--with-ns --with-native-compilation=$NATIVE --with-tree-sitter"
else
  CONF="--with-native-compilation=$NATIVE --with-tree-sitter --with-pgtk"
fi

TEMP_BUILD=0
if [ -z "$BUILD_DIR" ]; then
  TEMP_BUILD=1
  if [ "$DRY" = 1 ]; then BUILD_DIR=/tmp/nidhogg-build.XXXXXX; else BUILD_DIR=$(mktemp -d "${TMPDIR:-/tmp}/nidhogg-build.XXXXXX"); fi
elif [ "$DRY" = 0 ]; then
  mkdir -p "$BUILD_DIR"
  BUILD_DIR=$(cd "$BUILD_DIR" && pwd)
fi

if [ "$MODE" = container ]; then S=/src B=/build; else S=$ROOT B=$BUILD_DIR; fi
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
  DISTRO_TAG=$(. /etc/os-release; echo "${ID}-${VERSION_ID}")
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
    echo 'pk=$B/pkg/nidhogg-$V; rm -rf "$B/pkg"; mkdir -p "$pk"; mv nextstep/Emacs.app "$pk/Nidhogg.app"; info=$pk/BUILDINFO; rt=$pk/deps.runtime'
  else
    echo 'phase install'
    echo 'rm -rf "$B/stage"'
    echo "logged \"\$B/install.log\" make install DESTDIR=$B/stage"
    echo 'pk=$B/pkg/nidhogg-$V; rm -rf "$B/pkg"; mkdir -p "$B/pkg"; cp -a "$B/stage$P" "$pk"; dumplinks "$pk"; mkdir -p "$pk/share/nidhogg"; info=$pk/share/nidhogg/BUILDINFO; rt=$pk/share/nidhogg/deps.runtime'
    echo 'cp "$S/nidhogg/env.sh" "$pk/share/nidhogg/env.sh"'
  fi
  cat <<'BODY'
cp "$S/nidhogg/deps/$DISTRO_TAG.runtime" "$rt"
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
  [ -z "$LISP_CACHE" ] || printf 'LC=%q\n' "$LC"
  printf 'MODE_ID=%q\n' "$NATIVE"
  if [ "$MODE" = container ]; then
    case $DISTRO in
      ubuntu-*)
        [ -z "${NIDHOGG_BUILD_PKG_CACHE:-}" ] || echo 'rm -f /etc/apt/apt.conf.d/docker-clean; echo "Binary::apt::APT::Keep-Downloaded-Packages \"true\";" > /etc/apt/apt.conf.d/keep'
        echo "apt-get update"
        echo "DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \$(sed \"s/#.*//\" \$S/nidhogg/deps/$DISTRO.build)${NIDHOGG_BUILD_CCACHE:+ ccache}" ;;
      *) echo "dnf -y --setopt=install_weak_deps=False ${NIDHOGG_BUILD_PKG_CACHE:+--setopt=keepcache=1 }install \$(sed \"s/#.*//\" \$S/nidhogg/deps/$DISTRO.build)${NIDHOGG_BUILD_CCACHE:+ ccache}" ;;
    esac
    [ -z "${NIDHOGG_BUILD_CCACHE:-}" ] || echo 'export CCACHE_DIR=/ccache CCACHE_COMPILERCHECK=content CCACHE_MAXSIZE=500M CC="ccache gcc"; ccache -z >/dev/null'
  fi
  cat <<'BODY'
# relative links so that a relocated binary finds its dump beside itself
dumplinks() {
  root=$1
  set -- "$root"/libexec/emacs/*/*/emacs-*.pdmp
  [ -e "$1" ] || return 0
  rel=${1#"$root"/}
  for n in nidhogg emacs "emacs-$UPV"; do ln -sf "../$rel" "$root/bin/$n.pdmp"; done
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
mkdir -p "$B/src" "$B/obj"
BODY
  case $STAGE in all|build) ;; *) echo 'cd "$B/obj"' ;; esac
  if [ "$STAGE" = all ] || [ "$STAGE" = build ]; then cat <<'BODY'
rm -f "$B"/*.log
# GNU cp's -u (copy only newer) keeps an incremental build dir's mtimes; BSD cp (macOS) has no -u: copy everything
if cp -u /dev/null "$B/.cp-u-probe" 2>/dev/null; then CP_UPDATE="cp -a -u"; else CP_UPDATE="cp -pR"; fi; rm -f "$B/.cp-u-probe"
for e in "$S"/* "$S"/.[!.]*; do [ -e "$e" ] || continue; [ "${e##*/}" = .git ] || $CP_UPDATE "$e" "$B/src/"; done
cd "$B/src"
BODY
  if [ -n "$LISP_CACHE" ]; then cat <<'BODY'
# the compiled-Lisp cache (used only when made by the same compiler and architecture): restored files all get one timestamp, newer than every source (and equal among themselves, so
# no .eln is older than its .elc), so that make sees them as up to date
cc_id() { printf '%s %s %s\n' "$(gcc --version | head -n 1)" "$(uname -m)" "$MODE_ID"; }
if [ -f "$LC/stamp" ] && [ "$(cat "$LC/stamp")" = "$(cc_id)" ]; then
  phase restore compiled Lisp
  tar -C "$LC" -cf - src obj | tar -C "$B" -xf -
  touch "$B/.restored"
  (cd "$LC/src" && find . -type f) | while IFS= read -r f; do touch -r "$B/.restored" "$B/src/$f"; done
  (cd "$LC/obj" && find . -type f) | while IFS= read -r f; do touch -r "$B/.restored" "$B/obj/$f"; done
fi
BODY
  fi
  cat <<'BODY'
phase autogen
logged "$B/autogen.log" ./autogen.sh
cd "$B/obj"
BODY
  if [ "$DEV" = 1 ]; then
    # loadup.el loads lisp/site-init.el when present, and the dump keeps what it sets (the build copy only, never the source tree)
    echo "printf '%s\\n' ';;; dev build (nidhogg/build.sh --dev): no background native compilation' '(setq native-comp-jit-compilation nil)' > \"\$B/src/lisp/site-init.el\""
  fi
  echo "phase configure"
  echo "logged \"\$B/configure.log\" \"\$B/src/configure\" --prefix=\"\$P\" $CONF || { echo \"configure failed; the build packages are listed in nidhogg/deps/$DISTRO.build\" >&2; exit 1; }"
  cat <<'BODY'
grep -Eq 'native lisp compiler\? +yes' "$B/configure.log" || { echo "configure found no native compilation (libgccjit); see $B/configure.log" >&2; exit 1; }
BODY
  echo "phase make -j$JOBS"
  echo "logged \"\$B/make.log\" make -j$JOBS"
  if [ -n "$LISP_CACHE" ]; then cat <<'BODY'
phase save compiled Lisp
rm -rf "$LC/stamp" "$LC/src" "$LC/obj"
mkdir -p "$LC/src" "$LC/obj"
# the files make generated under lisp/ and leim/ (those the sources do not have), and the .eln files
(cd "$B/src" && find lisp leim -type f | while IFS= read -r f; do [ -e "$S/$f" ] || echo "$f"; done > "$B/lisp-cache.list" && tar -cf - -T "$B/lisp-cache.list") | tar -C "$LC/src" -xf -
[ ! -d "$B/obj/native-lisp" ] || tar -C "$B/obj" -cf - native-lisp | tar -C "$LC/obj" -xf -
cc_id > "$LC/stamp"
BODY
  fi
  fi
  if [ "$MODE" = container ] && [ -n "${NIDHOGG_BUILD_CCACHE:-}" ] && [ "$STAGE" != pack ]; then echo 'ccache -s || true'; fi
  if [ -n "$CHECK" ] && { [ "$STAGE" = all ] || [ "$STAGE" = check ]; }; then
    for t in $(sed "s/#.*//" "$CHECK" 2>/dev/null || echo "<targets of $CHECK>"); do
      echo "phase check $t"
      echo "logged \"\$B/check.log\" make -C test $t"
    done
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
  NAME=nidhogg-build-$$
  CACHE_ARGS=""
  if [ -n "${NIDHOGG_BUILD_PKG_CACHE:-}" ]; then
    case $DISTRO in ubuntu-*) CACHE_ARGS="-v $NIDHOGG_BUILD_PKG_CACHE/apt:/var/cache/apt/archives:Z" ;;
      *) CACHE_ARGS="-v $NIDHOGG_BUILD_PKG_CACHE/dnf:/var/cache/libdnf5:Z" ;; esac
  fi
  [ -z "${NIDHOGG_BUILD_CCACHE:-}" ] || CACHE_ARGS="$CACHE_ARGS -v $NIDHOGG_BUILD_CCACHE:/ccache:Z"
  [ -z "$LISP_CACHE" ] || CACHE_ARGS="$CACHE_ARGS -v $LISP_CACHE:/lispcache:Z"
  CMD_HEAD="$PODMAN run --rm --name $NAME --memory $MEMORY --memory-swap $MEMORY -v $ROOT:/src:ro,Z -v $BUILD_DIR:/build:Z ${MOUNT:+-v $MOUNT} $CACHE_ARGS $IMAGE"
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
  [ -z "${NIDHOGG_BUILD_PKG_CACHE:-}" ] || mkdir -p "$NIDHOGG_BUILD_PKG_CACHE/apt" "$NIDHOGG_BUILD_PKG_CACHE/dnf"
  # stop and remove our own container (and only it) however we end
  cleanup() {
    [ -z "${RUN_PID:-}" ] || kill "$RUN_PID" 2>/dev/null || true
    "$PODMAN" stop -t 10 "$NAME" >/dev/null 2>&1 || true
    "$PODMAN" rm -f "$NAME" >/dev/null 2>&1 || true
  }
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  # in the background so that a signal reaches the trap at once
  # shellcheck disable=SC2086
  "$PODMAN" run --rm --name "$NAME" --memory "$MEMORY" --memory-swap "$MEMORY" \
    -v "$ROOT:/src:ro,Z" -v "$BUILD_DIR:/build:Z" ${MOUNT:+-v} ${MOUNT:+"$MOUNT"} $CACHE_ARGS "$IMAGE" \
    bash -euo pipefail -c "$(script)" &
  RUN_PID=$!
  if wait "$RUN_PID"; then
    RUN_PID=""
    [ "$TEMP_BUILD" = 0 ] || rm -rf "$BUILD_DIR"
  else
    echo "build.sh: the build failed; logs are in $BUILD_DIR" >&2
    exit 1
  fi
else
  if [ "$DRY" = 1 ]; then script; exit 0; fi
  echo "+ host build in $BUILD_DIR (make -j$JOBS)"
  if bash -euo pipefail -c "$(script)"; then
    [ "$TEMP_BUILD" = 0 ] || rm -rf "$BUILD_DIR"
  else
    echo "build.sh: the build failed; logs are in $BUILD_DIR" >&2
    exit 1
  fi
fi
