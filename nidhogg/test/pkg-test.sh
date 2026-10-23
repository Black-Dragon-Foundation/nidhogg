#!/usr/bin/env bash
# The package suites with the workers on and off (engine C, §17.4, PD-8).
#   pkg-test.sh --container DISTRO --prefix DIR --mode on|off [--select GROUP] [--out DIR]
#       Runs the groups (default: all but image) in a fresh podman container of DISTRO against the installed Nidhogg at
#       DIR, with NIDHOGG_WORKERS=MODE and a temporary HOME.  Each test's outcome goes to DIR_OUT/pkg-GROUP-MODE.outcomes
#       (TEST<TAB>passed|failed|skipped), and one "PKG GROUP MODE: N passed, M failed, K skipped" line per group is
#       printed.  Exit 0 when the groups ran, whatever their outcomes: the verdict is --compare's.
#   pkg-test.sh --compare DIR [--expected FILE]
#       For each group, the tests that failed with the workers on must also have failed with them off, or be listed in
#       pkg-expected-failures.txt.  A failure only with the workers on is the gate failure (exit 1).
# Groups: jsonrpc treesit process image files.  image needs the GUI runner (gui.sh): it runs only locally.
# Third-party packages come from pkg-pins.txt into NIDHOGG_PKG_CACHE (default ~/.cache/nidhogg-pkg); servers and tools
# are installed inside the container, never on the host.  Containers are memory-capped (--memory, default 8g).
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
GROUPS_ALL="jsonrpc treesit process image files"

compare() {
  local dir=$1 expected=$2 rc=0 on g off t
  t=$(mktemp -d); trap 'rm -r "$t"' RETURN
  grep -v '^#' "$expected" 2>/dev/null | cut -f1 | sort -u > "$t/exp" || true
  local found=0
  for on in "$dir"/pkg-*-on.outcomes; do
    [ -f "$on" ] || continue
    found=1
    g=$(basename "$on" -on.outcomes); g=${g#pkg-}; off="$dir/pkg-$g-off.outcomes"
    if [ ! -f "$off" ]; then echo "COMPARE FAIL $g: no off-mode outcomes"; rc=1; continue; fi
    awk -F'\t' '$2=="failed"{print $1}' "$on" | sort -u > "$t/on"
    awk -F'\t' '$2=="failed"{print $1}' "$off" | sort -u > "$t/off"
    sort -u "$t/off" "$t/exp" > "$t/ok"
    comm -23 "$t/on" "$t/ok" > "$t/bad"
    if [ -s "$t/bad" ]; then
      echo "COMPARE FAIL $g: fails only with the workers on:"; sed 's/^/  /' "$t/bad"; rc=1
    else
      echo "COMPARE ok   $g"
    fi
  done
  [ $found = 1 ] || { echo "COMPARE FAIL: no on-mode outcome files in $dir"; rc=1; }
  return $rc
}

DISTRO="" PREFIX="" MODE="" SELECT="" OUT="" CMP="" EXPECTED="$HERE/pkg-expected-failures.txt"
while [ $# -gt 0 ]; do
  case $1 in
    --container) DISTRO=${2:?}; shift 2 ;;
    --prefix) PREFIX=${2:?}; shift 2 ;;
    --mode) MODE=${2:?}; shift 2 ;;
    --select) SELECT=${2:?}; shift 2 ;;
    --out) OUT=${2:?}; shift 2 ;;
    --compare) CMP=${2:?}; shift 2 ;;
    --expected) EXPECTED=${2:?}; shift 2 ;;
    *) echo "pkg-test: unknown option $1" >&2; exit 2 ;;
  esac
done
if [ -n "$CMP" ]; then compare "$CMP" "$EXPECTED"; exit $?; fi

[ -n "$DISTRO" ] && [ -n "$PREFIX" ] || { echo "usage: pkg-test.sh --container DISTRO --prefix DIR --mode on|off [--select GROUP] | --compare DIR" >&2; exit 2; }
case $MODE in on|off) ;; *) echo "pkg-test: --mode on|off" >&2; exit 2 ;; esac
case $DISTRO in
  fedora-44) IMAGE=registry.fedoraproject.org/fedora:44 ;;
  fedora-45) IMAGE=registry.fedoraproject.org/fedora:45 ;;
  ubuntu-26.04) IMAGE=docker.io/library/ubuntu:26.04 ;;
  *) echo "pkg-test: unknown distro $DISTRO" >&2; exit 2 ;;
esac
GROUPS_RUN=${SELECT:-jsonrpc treesit process files}
for g in $GROUPS_RUN; do case " $GROUPS_ALL " in *" $g "*) ;; *) echo "pkg-test: unknown group $g" >&2; exit 2 ;; esac; done
PREFIX=$(cd "$PREFIX" && pwd)
OUT=${OUT:-$PWD/pkg-out}; mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
CACHE=${NIDHOGG_PKG_CACHE:-$HOME/.cache/nidhogg-pkg}; mkdir -p "$CACHE"
PODMAN=${NIDHOGG_BUILD_PODMAN:-podman}

# the in-container script is single-quoted on purpose: it expands there, not here
# shellcheck disable=SC2016
INNER='set -u
case $DISTRO in
  ubuntu-*) export DEBIAN_FRONTEND=noninteractive; apt-get update -qq
    apt-get install -y -qq --no-install-recommends $(sed "s/#.*//" /src/nidhogg/deps/$DISTRO.runtime) git make gcc cmake libtool-bin \
      clangd python3-pylsp gnutls-bin openssh-server openssh-client >/dev/null ;;
  *) dnf -y -q --setopt=install_weak_deps=False install $(sed "s/#.*//" /src/nidhogg/deps/$DISTRO.runtime) git make gcc cmake libtool \
      clang-tools-extra python3-lsp-server gnutls-utils openssh-server openssh-clients findutils >/dev/null ;;
esac
EMACS=$PREFIX/bin/nidhogg; [ -x "$EMACS" ] || EMACS=$PREFIX/bin/emacs
export HOME=$(mktemp -d) NIDHOGG_WORKERS=$MODE EMACS
T=/src/test
# fetch the pinned packages once
grep -v "^#" /src/nidhogg/test/pkg-pins.txt | while read -r n url sha; do
  [ -n "$n" ] || continue
  d=/cache/$n
  [ -d "$d/.git" ] || git clone -q "$url" "$d"
  git -C "$d" cat-file -e "$sha^{commit}" 2>/dev/null || git -C "$d" fetch -q origin "$sha"
  git -C "$d" checkout -q "$sha"
done
LP=""; for n in compat llama with-editor cond-let dash transient websocket; do LP="$LP -L /cache/$n -L /cache/$n/lisp"; done

# ert LABEL ARGS...: one Emacs batch run; every test result line becomes an outcome line (LABEL/test)
ert() {
  label=$1; shift
  timeout 1500 "$EMACS" -Q --batch $LP "$@" -l ert --eval "(ert-run-tests-batch-and-exit ${SEL:-t})" 2>&1 \
    | tee -a "$O/pkg-$G-$MODE.log" \
    | awk -v l="$label" '"'"'$1 ~ /^(passed|PASSED|FAILED|failed|SKIPPED|skipped|XPASSED|xfailed)$/ && $2 ~ /^[0-9]+\/[0-9]+$/ {
        s = tolower($1); if (s == "xpassed") s = "failed"; if (s == "xfailed") s = "passed"
        printf "%s/%s\t%s\n", l, $3, s }'"'"' >> "$O/pkg-$G-$MODE.outcomes" || true
}
tests_of() { ls "/cache/$1"/{test,tests,t}/*test*.el 2>/dev/null | grep -v "/\(test-helper\|.*-helpers\)\.el$" || true; }
lf() { for f in "$@"; do [ -f "$f" ] && printf -- "-l %s " "$f"; done; }

for G in $GROUPS_RUN; do
  : > "$O/pkg-$G-$MODE.outcomes"; : > "$O/pkg-$G-$MODE.log"
  case $G in
    jsonrpc)
      ert jsonrpc -L $T/lisp -l $T/lisp/jsonrpc-tests.el
      ert eglot -L $T/lisp -L $T/lisp/progmodes -l $T/lisp/progmodes/eglot-tests.el
      # framing on/off/advised and the mcp.el-style filter replacement
      ert framing -L $T/src/nidhogg-tests $(lf $T/src/nidhogg-tests/workers-framing-tests.el)
      for p in dape mcp; do
        fs=$(tests_of $p | sed "s/^/-l /" | tr "\n" " ")
        ert $p -L /cache/$p $fs
      done ;;
    treesit)
      # deferral forced to "always wait": the packages must not see a half-parsed tree
      WAIT=(--eval "(when (boundp (quote nidhogg-treesit-defer)) (setq nidhogg-treesit-defer nil))")
      ert treesit -L $T/src "${WAIT[@]}" -l $T/src/treesit-tests.el
      for f in $T/lisp/progmodes/*-ts-mode-tests.el; do
        [ -f "$f" ] && ert "$(basename "$f" .el)" -L $T/lisp -L $T/lisp/progmodes "${WAIT[@]}" -l "$f"
      done ;;
    process)
      ert process -L $T/src -l $T/src/process-tests.el
      ert compile -L $T/lisp -L $T/lisp/progmodes -l $T/lisp/progmodes/compile-tests.el
      ert comint -L $T/lisp -l $T/lisp/comint-tests.el
      ert tramp -L $T/lisp -L $T/lisp/net -l $T/lisp/net/tramp-tests.el
      ert network-stream -L $T/lisp -L $T/lisp/net -l $T/lisp/net/network-stream-tests.el
      # TRAMP over ssh (sshx) against an sshd in this container
      ssh-keygen -A >/dev/null 2>&1; ssh-keygen -q -t ed25519 -N "" -f $HOME/.ssh_key; mkdir -p $HOME/.ssh /run/sshd
      cat $HOME/.ssh_key.pub >> $HOME/.ssh/authorized_keys; chmod 600 $HOME/.ssh/authorized_keys
      printf "Host *\n  IdentityFile %s\n  StrictHostKeyChecking no\n  UserKnownHostsFile /dev/null\n  Port 2222\n" "$HOME/.ssh_key" > $HOME/.ssh/config
      /usr/sbin/sshd -p 2222 -o PermitRootLogin=yes -o AuthorizedKeysFile=$HOME/.ssh/authorized_keys -o StrictModes=no
      REMOTE_TEMPORARY_FILE_DIRECTORY=/sshx:127.0.0.1:/tmp ert tramp-sshx -L $T/lisp -L $T/lisp/net -l $T/lisp/net/tramp-tests.el
      for p in magit eat vterm; do
        [ $p != vterm ] || (cd /cache/vterm && cmake -S . -B build >/dev/null && make -C build -j2 >/dev/null 2>&1) || true
        fs=$(tests_of $p | sed "s/^/-l /" | tr "\n" " ")
        ert $p -L /cache/$p -L /cache/$p/lisp -L /cache/$p/build $fs
      done ;;
    image) echo "image: needs the GUI runner; run it locally with gui.sh" >&2 ;;
    files)
      ert fileio -L $T/src -l $T/src/fileio-tests.el
      SEL="\"auto-save\\\\|recover\"" ert files -L $T/lisp -l $T/lisp/files-tests.el
      ert dired -L $T/lisp -l $T/lisp/dired-tests.el ;;
  esac
  p=$(grep -c "	passed$" "$O/pkg-$G-$MODE.outcomes" || true); f=$(grep -c "	failed$" "$O/pkg-$G-$MODE.outcomes" || true)
  s=$(grep -c "	skipped$" "$O/pkg-$G-$MODE.outcomes" || true)
  echo "PKG $G $MODE: $p passed, $f failed, $s skipped"
done'

exec "$PODMAN" run --rm --memory "${NIDHOGG_PKG_MEMORY:-8g}" --memory-swap "${NIDHOGG_PKG_MEMORY:-8g}" \
  -e DISTRO="$DISTRO" -e PREFIX="$PREFIX" -e MODE="$MODE" -e GROUPS_RUN="$GROUPS_RUN" -e O=/out \
  -v "$ROOT:/src:ro,Z" -v "$PREFIX:$PREFIX:ro,Z" -v "$CACHE:/cache:Z" -v "$OUT:/out:Z" "$IMAGE" bash -c "$INNER"
