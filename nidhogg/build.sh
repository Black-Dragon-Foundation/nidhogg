#!/usr/bin/env bash
# Build Nidhogg in a capped podman container or on the host.  See README.md ("Building").
#   nidhogg/build.sh (--container DISTRO | --host) [--prefix DIR] [--jobs N] [--memory SIZE]
#                    [--build-dir DIR] [--check FILE] [--dry-run]
# Exit: 0 ok, 1 build/check failed or not enough memory, 2 usage.
# Test overrides: NIDHOGG_BUILD_UNAME, NIDHOGG_BUILD_NPROC, NIDHOGG_BUILD_MEMORY_KB, NIDHOGG_BUILD_PODMAN.
# bash 3.2-safe (macOS): none of the bash 4 features.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
VERSION=$(head -n 1 "$ROOT/nidhogg/version")
DISTROS="fedora-44 fedora-45 ubuntu-26.04"

usage() {
  cat >&2 <<USAGE
usage: nidhogg/build.sh (--container DISTRO | --host) [--prefix DIR] [--jobs N] [--memory SIZE]
                        [--build-dir DIR] [--check FILE] [--dry-run]
  DISTRO: $DISTROS
USAGE
  exit 2
}
die() { echo "build.sh: $*" >&2; exit 1; }

MODE="" DISTRO="" PREFIX="" JOBS="" MEMORY="" BUILD_DIR="" CHECK="" DRY=0
while [ $# -gt 0 ]; do
  case $1 in
    --container) [ $# -ge 2 ] || usage; MODE=container; DISTRO=$2; shift 2 ;;
    --host) MODE=host; shift ;;
    --prefix) [ $# -ge 2 ] || usage; PREFIX=$2; shift 2 ;;
    --jobs) [ $# -ge 2 ] || usage; JOBS=$2; shift 2 ;;
    --memory) [ $# -ge 2 ] || usage; MEMORY=$2; shift 2 ;;
    --build-dir) [ $# -ge 2 ] || usage; BUILD_DIR=$2; shift 2 ;;
    --check) [ $# -ge 2 ] || usage; CHECK=$2; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    *) echo "unknown option: $1" >&2; usage ;;
  esac
done
[ -n "$MODE" ] || usage

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
if [ -n "$CHECK" ] && [ ! -f "$CHECK" ] && [ "$DRY" = 0 ]; then die "--check file not found: $CHECK"; fi

mem_available_kb() {
  if [ -n "${NIDHOGG_BUILD_MEMORY_KB:-}" ]; then echo "$NIDHOGG_BUILD_MEMORY_KB"
  elif [ -r /proc/meminfo ]; then awk '/^MemAvailable:/ {print $2}' /proc/meminfo
  else # macOS: free + inactive pages
    vm_stat | awk '/page size of/ {ps=$8} /Pages (free|inactive)/ {n+=$NF} END {printf "%d", n*ps/1024}'
  fi
}
AVAIL_KB=$(mem_available_kb)
AVAIL_GIB=$((AVAIL_KB / 1024 / 1024))

# container cap: min(6, available - 1) GiB, refused below 3 GiB; an explicit --memory is taken as given
if [ "$MODE" = container ]; then
  if [ -z "$MEMORY" ]; then
    CAP=$((AVAIL_GIB - 1))
    [ "$CAP" -le 6 ] || CAP=6
    if [ "$CAP" -lt 3 ]; then
      echo "build.sh: a Nidhogg build needs 3 GiB of free memory for its container; only ${AVAIL_GIB} GiB are available" >&2
      exit 1
    fi
    MEMORY=${CAP}g
  fi
  case $MEMORY in *g) CAP=${MEMORY%g} ;; *) echo "--memory takes whole GiB, e.g. 6g" >&2; exit 2 ;; esac
  JOBS_FROM=$CAP
else
  JOBS_FROM=$AVAIL_GIB
fi
if [ -z "$JOBS" ]; then
  JOBS=$((JOBS_FROM * 10 / 15))   # one job per 1.5 GiB
  [ "$JOBS" -le "$NPROC" ] || JOBS=$NPROC
  [ "$JOBS" -ge 1 ] || JOBS=1
fi

if [ "$MODE" = host ] && [ "$UNAME" = Darwin ]; then
  CONF="--with-ns --with-native-compilation=aot --with-tree-sitter"
else
  CONF="--with-native-compilation=aot --with-tree-sitter --with-pgtk"
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

# --- the build script: the same steps in a container and on the host --------------------------------------
script() {
  printf 'S=%q B=%q P=%q\n' "$S" "$B" "$PREFIX"
  if [ "$MODE" = container ]; then
    case $DISTRO in
      ubuntu-*)
        echo "apt-get update"
        echo "DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \$(sed \"s/#.*//\" \$S/nidhogg/deps/$DISTRO.build)" ;;
      *) echo "dnf -y install \$(sed \"s/#.*//\" \$S/nidhogg/deps/$DISTRO.build)" ;;
    esac
  fi
  cat <<'BODY'
logged() { log=$1; shift; "$@" >> "$log" 2>&1 || { echo "FAILED: $* (log: $log)" >&2; exit 1; }; }
mkdir -p "$B/src" "$B/obj"
rm -f "$B"/*.log
# GNU cp's -u (copy only newer) keeps an incremental build dir's mtimes; BSD cp (macOS) has no -u: copy everything
if cp -u /dev/null "$B/.cp-u-probe" 2>/dev/null; then CP_UPDATE="cp -a -u"; else CP_UPDATE="cp -pR"; fi; rm -f "$B/.cp-u-probe"
for e in "$S"/* "$S"/.[!.]*; do [ -e "$e" ] || continue; [ "${e##*/}" = .git ] || $CP_UPDATE "$e" "$B/src/"; done
cd "$B/src"
logged "$B/autogen.log" ./autogen.sh
cd "$B/obj"
BODY
  echo "logged \"\$B/configure.log\" \"\$B/src/configure\" --prefix=\"\$P\" $CONF || { echo \"configure failed; the build packages are listed in nidhogg/deps/$DISTRO.build\" >&2; exit 1; }"
  cat <<'BODY'
grep -Eq 'native lisp compiler\? +yes' "$B/configure.log" || { echo "configure found no native compilation (libgccjit); see $B/configure.log" >&2; exit 1; }
BODY
  echo "logged \"\$B/make.log\" make -j$JOBS"
  if [ -n "$CHECK" ]; then
    for t in $(sed "s/#.*//" "$CHECK" 2>/dev/null || echo "<targets of $CHECK>"); do
      echo "logged \"\$B/check.log\" make -C test $t"
    done
  fi
  if [ "$MODE" = host ] && [ "$UNAME" = Darwin ]; then
    echo "logged \"\$B/install.log\" make install"
    echo "rm -rf \"\$P/Nidhogg.app\" && mkdir -p \"\$P\" && mv nextstep/Emacs.app \"\$P/Nidhogg.app\""
  else
    echo "logged \"\$B/install.log\" make install"
  fi
}

# --- run --------------------------------------------------------------------------------------------------
if [ "$MODE" = container ]; then
  mkdir -p "$PREFIX" 2>/dev/null || true
  CMD_HEAD="$PODMAN run --rm --memory $MEMORY --memory-swap $MEMORY -v $ROOT:/src:ro,Z -v $BUILD_DIR:/build:Z -v $PREFIX:$PREFIX:Z $IMAGE"
  if [ "$DRY" = 1 ]; then
    echo "$CMD_HEAD bash -euo pipefail -c '"
    script
    echo "'"
    exit 0
  fi
  command -v "$PODMAN" >/dev/null 2>&1 || die "podman not found (podman only; no docker)"
  echo "+ $CMD_HEAD bash -euo pipefail -c <script>"
  if "$PODMAN" run --rm --memory "$MEMORY" --memory-swap "$MEMORY" \
       -v "$ROOT:/src:ro,Z" -v "$BUILD_DIR:/build:Z" -v "$PREFIX:$PREFIX:Z" "$IMAGE" \
       bash -euo pipefail -c "$(script)"; then
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
