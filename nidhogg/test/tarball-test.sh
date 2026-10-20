#!/usr/bin/env bash
# A release tarball runs at its recorded prefix and elsewhere.
#   nidhogg/test/tarball-test.sh TARBALL DISTRO      (DISTRO: fedora-44 | fedora-45 | ubuntu-26.04 | macos)
# Linux: in a fresh --memory 2g podman container of DISTRO.  macOS: on the host, under $TMPDIR.
# Exit 0 when all checks pass, 1 with "TARBALL FAIL: <why>".
set -u
[ $# -eq 2 ] || { echo "usage: tarball-test.sh TARBALL DISTRO" >&2; exit 2; }
TARBALL=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
DISTRO=$2
PODMAN=${NIDHOGG_BUILD_PODMAN:-podman}
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
[ -f "$TARBALL" ] || { echo "TARBALL FAIL: $TARBALL does not exist"; exit 1; }

# the checks, as one script; $1 = unpacked tree root, $2 = expected nidhogg-version
cat > "${TMPDIR:-/tmp}/nidhogg-tarball-checks.$$" <<'CHECKS'
root=$1; want=$2
if [ "$(uname -s)" = Darwin ]; then bin=$root/Nidhogg.app/Contents/MacOS/Emacs; else bin=$root/bin/nidhogg; fi
# Linux: every shared library of every program in bin/ and libexec/ is found, all the missing ones named at once (the
# loader stops at the first); a library missing here needs its package in nidhogg/deps/<distro>.runtime
if [ "$(uname -s)" != Darwin ]; then
  command -v ldd >/dev/null || { echo "TARBALL FAIL: no ldd to check the libraries with"; exit 1; }
  missing=$(for f in "$root"/bin/* "$root"/libexec/emacs/*/*/*; do
              if [ -f "$f" ] && [ -x "$f" ]; then ldd "$f" 2>/dev/null; fi
            done | awk '/=> not found/ {print $1}' | sort -u | tr '\n' ' ')
  [ -z "$missing" ] || { echo "TARBALL FAIL: libraries not found (their packages belong in deps.runtime): $missing"; exit 1; }
fi
WANT=$want ROOT=$root "$bin" --batch -Q --eval '(progn
  (unless (equal nidhogg-version (getenv "WANT")) (error "nidhogg-version %s" nidhogg-version))
  (unless (featurep (quote nidhogg)) (error "no nidhogg feature"))
  (unless (and (native-comp-available-p) (native-compile (quote (lambda () 1)))) (error "native compilation"))
  (unless (treesit-available-p) (error "tree-sitter"))
  (unless (featurep (if (eq system-type (quote darwin)) (quote ns) (quote pgtk))) (error "not the graphical build"))
  (require (quote org))
  (unless (file-in-directory-p data-directory (getenv "ROOT")) (error "data-directory %s" data-directory))
  (princ "TARBALL OK\n"))' || { echo "TARBALL FAIL: checks failed for $root"; exit 1; }
if [ "$(uname -s)" != Darwin ]; then "$root/bin/nidhoggclient" --version >/dev/null || { echo "TARBALL FAIL: nidhoggclient"; exit 1; }; fi
CHECKS
CHECKS_FILE="${TMPDIR:-/tmp}/nidhogg-tarball-checks.$$"
trap 'rm -f "$CHECKS_FILE"' EXIT

VERSION=$(head -n 1 "$HERE/nidhogg/version")

if [ "$DISTRO" = macos ]; then
  W=$(mktemp -d "${TMPDIR:-/tmp}/nidhogg-tarball.XXXXXX")
  tar -C "$W" -xJf "$TARBALL" || { echo "TARBALL FAIL: unpack"; exit 1; }
  bash "$CHECKS_FILE" "$W/nidhogg-$VERSION" "$VERSION" || exit 1
  rm -rf "$W"
  exit 0
fi

# xz and tar first (a bare image may lack xz), then the runtime packages the tarball lists
case $DISTRO in
  ubuntu-*) INNER='set -eu
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends xz-utils tar > /dev/null
tar -xJOf /t.tar.xz "nidhogg-$V/share/nidhogg/deps.runtime" > /deps.runtime
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends $(sed "s/#.*//" /deps.runtime) > /dev/null' ;;
  *) INNER='set -eu
dnf -y -q --setopt=install_weak_deps=False install xz tar > /dev/null
tar -xJOf /t.tar.xz "nidhogg-$V/share/nidhogg/deps.runtime" > /deps.runtime
dnf -y -q --setopt=install_weak_deps=False install $(sed "s/#.*//" /deps.runtime) > /dev/null' ;;
esac
INNER="$INNER"'
prefix=$(tar -xJOf /t.tar.xz "nidhogg-$V/share/nidhogg/BUILDINFO" | sed -n "s/^prefix=//p")
echo "== at the recorded prefix: $prefix"
mkdir -p "$prefix" && tar -C "$prefix" --strip-components=1 -xJf /t.tar.xz
bash /checks.sh "$prefix" "$V" || exit 1
echo "== elsewhere: /tmp/elsewhere (env.sh)"
mkdir -p /tmp/elsewhere && tar -C /tmp/elsewhere --strip-components=1 -xJf /t.tar.xz
rm -rf "$prefix"
NIDHOGG_ROOT=/tmp/elsewhere . /tmp/elsewhere/share/nidhogg/env.sh
bash /checks.sh /tmp/elsewhere "$V" || exit 1'

IMAGE=""
case $DISTRO in
  fedora-44) IMAGE=registry.fedoraproject.org/fedora:44 ;;
  fedora-45) IMAGE=registry.fedoraproject.org/fedora:45 ;;
  ubuntu-26.04) IMAGE=docker.io/library/ubuntu:26.04 ;;
  *) echo "TARBALL FAIL: unknown distro $DISTRO"; exit 1 ;;
esac
"$PODMAN" run --rm --memory 2g --memory-swap 2g -e "V=$VERSION" \
  -v "$TARBALL:/t.tar.xz:ro,Z" -v "$CHECKS_FILE:/checks.sh:ro,Z" "$IMAGE" bash -c "$INNER" ||
  { echo "TARBALL FAIL: $DISTRO container run"; exit 1; }
