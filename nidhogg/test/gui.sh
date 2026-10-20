#!/usr/bin/env bash
# Nidhogg's GUI test runner: a headless mutter (its own session bus, its own Wayland display), a Nidhogg pgtk frame
# on it, and the *-gui-tests.el files run by test/src/nidhogg-tests/gui-runner.el.  Never touches the caller's display,
# session bus or Emacs server: everything it starts is private and stopped on exit.
#
#   nidhogg/test/gui.sh --prefix DIR [--scale S] [--size WxH] [--monitors N] [--select REGEXP] [--tests DIR]
#                       [--daemon] [--container DISTRO] [--wayland-debug FILE] [--env K=V]... [--timeout SEC]
#
#   --prefix DIR        an unpacked Nidhogg (DIR/bin/nidhogg)
#   --scale S           the monitors' scale, integer or fractional (default 1)
#   --size WxH          each virtual monitor in device pixels (default 2880x1800)
#   --monitors N        virtual monitors, side by side (default 1)
#   --select REGEXP     ERT selector, an Emacs regexp (alternation is \|); default every test
#   --tests DIR         where the *-gui-tests.el files are (default: test/src/nidhogg-tests of this checkout)
#   --daemon            run in `nidhogg --fg-daemon' and open the frame with emacsclient -c
#   --container DISTRO  the same, inside a memory-capped podman container (fedora-45), image pinned by digest
#   --wayland-debug F   Emacs's stderr (WAYLAND_DEBUG=client protocol trace and the test output) goes to F
#   --env K=V           an extra environment variable for Nidhogg (repeatable)
#   --timeout SEC       kill the run after SEC seconds (default 300)
#
# Exit 0 all tests passed, 1 a failure, a timeout or a session that did not come up, 2 usage.
# The scale is applied with `gdctl set --logical-monitor --scale S' inside the session (mutter 50 sets fractional
# scales on a headless virtual monitor; no scale-monitor-framebuffer fallback was needed), after which the runner waits
# for (frame-scale-factor) = ceil(S) (GTK 3 knows only integer scales) and the monitor's GDK geometry = (WxH / S) * ceil(S).
# Temp files live under ${TMPDIR:-/tmp}/glitnir-gui.XXXXXX (HOME, XDG_*, caches), never in ~.
set -u

self=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")
checkout=$(cd "$(dirname "$self")/../.." && pwd)

# image pinned by digest (registry.fedoraproject.org/fedora:45 as of 2026-10-21)
IMAGE_fedora_45=registry.fedoraproject.org/fedora@sha256:6430abb162172c052b6ac9c9d6a219b1b21352508c04b1b0ff2eb9292943d881

usage() { sed -n '2,/^# Temp files/s/^# \{0,1\}//p' "$self" >&2; exit 2; }

prefix= scale=1 size=2880x1800 monitors=1 select= tests= daemon=0 container= wdebug= timeout=300
envs=()
while [ $# -gt 0 ]; do
  case $1 in
    --prefix) prefix=${2:-}; shift 2 ;;
    --scale) scale=${2:-}; shift 2 ;;
    --size) size=${2:-}; shift 2 ;;
    --monitors) monitors=${2:-}; shift 2 ;;
    --select) select=${2:-}; shift 2 ;;
    --tests) tests=${2:-}; shift 2 ;;
    --daemon) daemon=1; shift ;;
    --container) container=${2:-}; shift 2 ;;
    --wayland-debug) wdebug=${2:-}; shift 2 ;;
    --env) envs+=("${2:-}"); shift 2 ;;
    --timeout) timeout=${2:-}; shift 2 ;;
    --inner) shift ;;   # internal: the part that runs under dbus-run-session
    -h|--help) usage ;;
    *) echo "gui.sh: unknown argument $1" >&2; usage ;;
  esac
done
[ -n "$prefix" ] || { echo "gui.sh: --prefix is required" >&2; usage; }
prefix=$(cd "$prefix" 2>/dev/null && pwd) || { echo "gui.sh: no such prefix" >&2; exit 2; }
[ -x "$prefix/bin/nidhogg" ] || { echo "gui.sh: $prefix/bin/nidhogg not found" >&2; exit 2; }
case $size in [0-9]*x[0-9]*) ;; *) echo "gui.sh: --size is WxH" >&2; exit 2 ;; esac
[ -n "$tests" ] || tests=$checkout/test/src/nidhogg-tests
tests=$(cd "$tests" && pwd) || exit 2
W=${size%x*} H=${size#*x}

# ---------------------------------------------------------------- in the session (private bus)
if [ "${GLITNIR_GUI_INNER:-}" = 1 ]; then
  display=glitnir-gui-$GLITNIR_GUI_PID
  mpid= epid=
  cleanup() {
    for p in $epid $mpid; do kill "$p" 2>/dev/null; done
    for p in $epid $mpid; do wait "$p" 2>/dev/null; done
  }
  trap cleanup EXIT
  trap 'exit 1' INT TERM

  vm=()
  for ((i = 0; i < monitors; i++)); do vm+=(--virtual-monitor "$size"); done
  mutter --headless --wayland --no-x11 --wayland-display "$display" "${vm[@]}" \
    > "$GLITNIR_GUI_RUN/mutter.log" 2>&1 &
  mpid=$!
  for _ in $(seq 100); do
    [ -S "$XDG_RUNTIME_DIR/$display" ] && break
    kill -0 "$mpid" 2>/dev/null || { echo "gui.sh: mutter exited early:"; cat "$GLITNIR_GUI_RUN/mutter.log"; exit 1; }
    sleep 0.1
  done
  [ -S "$XDG_RUNTIME_DIR/$display" ] || { echo "gui.sh: mutter has no display $display"; exit 1; }

  # the scale: gdctl on the session's own mutter (WAYLAND_DISPLAY is set only for the commands below)
  export WAYLAND_DISPLAY=$display
  names=$(gdctl show | grep -o 'Monitor Meta-[0-9]*' | sed 's/Monitor //')
  args=() x=0 first=--primary
  lw=$(awk -v w="$W" -v s="$scale" 'BEGIN { printf "%d", w / s + 0.5 }')
  for n in $names; do
    args+=(--logical-monitor $first --monitor "$n" --scale "$scale" --x "$x" --y 0)
    first=; x=$((x + lw))
  done
  gdctl set "${args[@]}" || { echo "gui.sh: gdctl could not set scale $scale"; exit 1; }

  export GLITNIR_GUI_SCALE=$scale GLITNIR_GUI_SIZE=$size GLITNIR_GUI_TESTS=$tests GLITNIR_GUI_SELECT=$select
  export GDK_BACKEND=wayland NO_AT_BRIDGE=1
  unset DISPLAY EMACS_SOCKET_NAME
  for kv in ${envs[@]+"${envs[@]}"}; do export "$kv"; done
  [ -z "$wdebug" ] || export WAYLAND_DEBUG=client
  runner=$checkout/test/src/nidhogg-tests/gui-runner.el
  err=/dev/stderr; [ -z "$wdebug" ] || err=$wdebug

  if [ "$daemon" = 1 ]; then
    sock=$XDG_RUNTIME_DIR/emacs/glitnir-gui
    timeout --kill-after=10 "$timeout" "$prefix/bin/nidhogg" -Q --fg-daemon="$sock" -l "$runner" 2> "$err" &
    epid=$!
    for _ in $(seq 100); do
      [ -S "$sock" ] && break
      kill -0 "$epid" 2>/dev/null || break
      sleep 0.1
    done
    [ -S "$sock" ] || { echo "gui.sh: the daemon has no socket"; wait "$epid"; exit 1; }
    # the frame, which starts the tests; WAYLAND_DEBUG is for the daemon only
    env -u WAYLAND_DEBUG timeout --kill-after=10 "$timeout" "$prefix/bin/nidhoggclient" -s "$sock" -c \
      -F '((fullscreen . fullboth))' > /dev/null 2>&1 &
    cpid=$!
    wait "$epid"; rc=$?
    kill "$cpid" 2>/dev/null; wait "$cpid" 2>/dev/null
    epid=
  else
    timeout --kill-after=10 "$timeout" "$prefix/bin/nidhogg" -Q --fullscreen -l "$runner" 2> "$err" &
    epid=$!
    wait "$epid"; rc=$?
    epid=
  fi
  [ "$rc" = 0 ] || echo "gui.sh: nidhogg exited with $rc"
  [ "$rc" = 0 ] && exit 0 || exit 1
fi

# ---------------------------------------------------------------- in a container
if [ -n "$container" ]; then
  case $container in
    fedora-45) image=$IMAGE_fedora_45 ;;
    *) echo "gui.sh: --container supports fedora-45" >&2; exit 2 ;;
  esac
  name=${GLITNIR_GUI_NAME:-glitnir-gui-$$}
  trap 'podman rm -f "$name" > /dev/null 2>&1' EXIT
  trap 'exit 1' INT TERM
  # a tarball runs at the prefix it was built for (BUILDINFO), so it is mounted there
  inprefix=$(sed -n 's/^prefix=//p' "$prefix/share/nidhogg/BUILDINFO" 2>/dev/null)
  [ -n "$inprefix" ] || inprefix=$prefix
  args=(--prefix "$inprefix" --scale "$scale" --size "$size" --monitors "$monitors" --tests /src/test/src/nidhogg-tests --timeout "$timeout")
  [ -z "$select" ] || args+=(--select "$select")
  [ "$daemon" = 0 ] || args+=(--daemon)
  [ -z "$wdebug" ] || args+=(--wayland-debug /out/wayland-debug.txt)
  for kv in ${envs[@]+"${envs[@]}"}; do args+=(--env "$kv"); done
  out=
  mounts=(-v "$checkout:/src:ro,Z" -v "$prefix:$inprefix:ro,Z")
  [ "$tests" = "$checkout/test/src/nidhogg-tests" ] || mounts+=(-v "$tests:/src/test/src/nidhogg-tests:ro,Z")
  if [ -n "$wdebug" ]; then
    out=$(mktemp -d "${TMPDIR:-/tmp}/glitnir-gui-out.XXXXXX")
    mounts+=(-v "$out:/out:Z")
  fi
  # deps.runtime of the tarball, mutter (the compositor and gdctl), dbus-daemon, software GL, one monospace font
  podman run --rm --name "$name" --memory "${GLITNIR_GUI_MEMORY:-6g}" --memory-swap "${GLITNIR_GUI_MEMORY:-6g}" \
    "${mounts[@]}" "$image" bash -c '
      set -eu
      pkgs=$(sed "s/#.*//" "'"$inprefix"'/share/nidhogg/deps.runtime" 2>/dev/null || true)
      dnf -y -q --setopt=install_weak_deps=False install mutter dbus-daemon mesa-dri-drivers mesa-libEGL \
        mesa-libgbm python3-gobject-base dejavu-sans-mono-fonts procps-ng util-linux $pkgs > /tmp/dnf.log 2>&1 \
        || { cat /tmp/dnf.log; exit 1; }
      # glycin (GTK 3 loads its icons through it) sandboxes its image loaders with bwrap --dev /dev, which a podman
      # container refuses (it cannot mount a devpts); the loaders get the container'"'"'s own /dev instead
      printf "%s\n" "#!/bin/bash" "a=(); while [ \$# -gt 0 ]; do if [ \"\$1\" = --dev ]; then a+=(--dev-bind \"\$2\" \"\$2\"); shift 2; else a+=(\"\$1\"); shift; fi; done" \
        "exec /usr/bin/bwrap \"\${a[@]}\"" > /usr/local/bin/bwrap
      chmod +x /usr/local/bin/bwrap
      export XDG_RUNTIME_DIR=/tmp/xdg-container
      exec bash /src/nidhogg/test/gui.sh "$@"' _ "${args[@]}"
  rc=$?
  if [ -n "$out" ]; then cp "$out/wayland-debug.txt" "$wdebug" 2>/dev/null; rm -r "$out"; fi
  exit $rc
fi

# ---------------------------------------------------------------- on the host: a run dir, a private session bus
for t in mutter gdctl dbus-run-session; do
  command -v "$t" > /dev/null || { echo "gui.sh: $t not found (the host needs mutter and dbus; or use --container)" >&2; exit 1; }
done
run=$(mktemp -d "${TMPDIR:-/tmp}/glitnir-gui.XXXXXX")
# XDG_RUNTIME_DIR holds only sockets, and a Unix socket path is limited to 107 bytes: it sits in a short directory.
xdg=$(mktemp -d "${GLITNIR_GUI_SOCKDIR:-/tmp}/glg.XXXXXX")
trap 'rm -r "$run" "$xdg"' EXIT
trap 'exit 1' INT TERM
mkdir -p "$run/home" "$run/cache" "$run/data" "$run/config"
# Nothing of the caller's session reaches the run: no display, no bus, no Emacs server, no agent, a private HOME and XDG.
env -u WAYLAND_DISPLAY -u DISPLAY -u DBUS_SESSION_BUS_ADDRESS -u EMACS_SOCKET_NAME -u SSH_AUTH_SOCK \
  HOME="$run/home" XDG_RUNTIME_DIR="$xdg" XDG_CACHE_HOME="$run/cache" XDG_DATA_HOME="$run/data" \
  XDG_CONFIG_HOME="$run/config" XDG_SESSION_TYPE=wayland GIO_USE_VFS=local GSETTINGS_BACKEND=memory \
  GLITNIR_GUI_INNER=1 GLITNIR_GUI_PID=$$ GLITNIR_GUI_RUN="$run" \
  dbus-run-session -- bash "$self" --prefix "$prefix" --scale "$scale" --size "$size" --monitors "$monitors" \
    --tests "$tests" --timeout "$timeout" ${select:+--select "$select"} ${wdebug:+--wayland-debug "$wdebug"} \
    $([ "$daemon" = 0 ] || echo --daemon) $(for kv in ${envs[@]+"${envs[@]}"}; do echo --env "$kv"; done)
exit $?
