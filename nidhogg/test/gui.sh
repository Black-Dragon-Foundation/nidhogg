#!/usr/bin/env bash
# Nidhogg's GUI test runner: a headless compositor (headless sway by default; its own session bus, its own Wayland
# display), a Nidhogg pgtk frame on it, and the *-gui-tests.el files run by test/src/nidhogg-tests/gui-runner.el.
# Never touches the caller's display, session bus or Emacs server: everything it starts is private and stopped on exit.
#
#   nidhogg/test/gui.sh --prefix DIR [--compositor sway|mutter|recorder] [--recorder BIN]
#                       [--recorder-profile sway|kwin|mutter] [--scale S] [--size WxH] [--monitors N]
#                       [--select REGEXP] [--tests DIR] [--daemon] [--container DISTRO] [--wayland-debug FILE]
#                       [--env K=V]... [--force-csd] [--timeout SEC] [--smoke] [--files GLOB] [--tag TAG]
#
#   --prefix DIR        an unpacked Nidhogg (DIR/bin/nidhogg)
#   --compositor C      sway (default: headless sway, wlroots pixman renderer), mutter (headless mutter) or recorder
#                       (nh-recorder, our own recording compositor, one output)
#   --recorder BIN      the nh-recorder binary (default $REC); mounted at /opt/nh-recorder in a container
#   --recorder-profile  what the recorder imitates: sway (default), kwin or mutter
#   GLITNIR_GUI_DRY=1   print the commands a run would start and exit 0 (nothing is started)
#   --scale S           the monitors' scale, integer or fractional (default 1)
#   --size WxH          each virtual monitor in device pixels (default 2880x1800)
#   --monitors N        virtual monitors, side by side (default 1)
#   --select REGEXP     ERT selector, an Emacs regexp (alternation is \|); default every test
#   --tests DIR         where the *-gui-tests.el files are (default: test/src/nidhogg-tests of this checkout)
#   --daemon            run in `nidhogg --fg-daemon' and open the frame with emacsclient -c
#   --container DISTRO  the same, inside a memory-capped podman container (fedora-45), image pinned by digest
#   --wayland-debug F   Emacs's stderr (WAYLAND_DEBUG=client protocol trace and the test output) goes to F
#   --env K=V           an extra environment variable for Nidhogg (repeatable)
#   --force-csd         client-side decorations: GTK_CSD=1 for pgtk, NIDHOGG_SHELL_FORCE_CSD=1 for the shell, and
#                       GLITNIR_GUI_CSD=1 for the runner.  The shell then draws its own title bar on sway; GTK 3
#                       ignores GTK_CSD there (it asks sway's KDE server-decoration global for server mode)
#   --timeout SEC       kill the run after SEC seconds (default 300)
#   --smoke             the GNOME smoke tier: implies --compositor mutter; every *-gui-tests.el file that holds a
#                       gnome-smoke row runs in its own mutter (restarted per file) at scales 1.25 and 1.5, only its
#                       gnome-smoke rows, under GLITNIR_GUI_MEMORY (default 2g here); exit 1 if any run fails
#   --files GLOB        load only the test files matching GLOB (in --tests); also $GLITNIR_GUI_FILES
#   --tag TAG           run only the rows tagged TAG (selector (and REGEXP (tag TAG))); also $GLITNIR_GUI_TAG
#   Every run prints "gui.sh: COMPOSITOR peak RSS: N kB" (VmHWM, read at exit).
#
# Exit 0 all tests passed, 1 a failure, a timeout or a session that did not come up, 2 usage.
# Mutter: the scale is applied with `gdctl set --logical-monitor --scale S' inside the session (mutter 50 sets fractional
# scales on a headless virtual monitor; no scale-monitor-framebuffer fallback was needed).  Sway: `output NAME ... scale S'.
# Recorder: --output WxH@round(S*120).  The runner (gui-runner.el) then waits for the scale rule of the compositor.
# Temp files live under ${TMPDIR:-/tmp}/glitnir-gui.XXXXXX (HOME, XDG_*, caches), never in ~.
set -u

self=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")
checkout=$(cd "$(dirname "$self")/../.." && pwd)

# image pinned by digest (registry.fedoraproject.org/fedora:45 as of 2026-10-21)
IMAGE_fedora_45=registry.fedoraproject.org/fedora@sha256:6430abb162172c052b6ac9c9d6a219b1b21352508c04b1b0ff2eb9292943d881

# budgets.sh sources this file for the pinned image only: stop before any argument is read
[ "${GUI_SH_SOURCE_ONLY:-}" = 1 ] && return 0

usage() { sed -n '2,/^# Temp files/s/^# \{0,1\}//p' "$self" >&2; exit 2; }

prefix= scale=1 size=2880x1800 monitors=1 select= tests= daemon=0 container= wdebug= timeout=300
compositor=sway recorder=${REC:-} rprofile=sway
envs=() csd=0 smoke=0 files=${GLITNIR_GUI_FILES:-} tag=${GLITNIR_GUI_TAG:-}
while [ $# -gt 0 ]; do
  case $1 in
    --prefix) prefix=${2:-}; shift 2 ;;
    --compositor) compositor=${2:-}; shift 2 ;;
    --recorder) recorder=${2:-}; shift 2 ;;
    --recorder-profile) rprofile=${2:-}; shift 2 ;;
    --scale) scale=${2:-}; shift 2 ;;
    --size) size=${2:-}; shift 2 ;;
    --monitors) monitors=${2:-}; shift 2 ;;
    --select) select=${2:-}; shift 2 ;;
    --tests) tests=${2:-}; shift 2 ;;
    --daemon) daemon=1; shift ;;
    --container) container=${2:-}; shift 2 ;;
    --wayland-debug) wdebug=${2:-}; shift 2 ;;
    --env) envs+=("${2:-}"); shift 2 ;;
    --force-csd) csd=1; shift ;;
    --timeout) timeout=${2:-}; shift 2 ;;
    --smoke) smoke=1; shift ;;
    --files) files=${2:-}; shift 2 ;;
    --tag) tag=${2:-}; shift 2 ;;
    --inner) shift ;;   # internal: the part that runs under dbus-run-session
    -h|--help) usage ;;
    *) echo "gui.sh: unknown argument $1" >&2; usage ;;
  esac
done
[ "$smoke" = 0 ] || compositor=mutter
[ -n "$prefix" ] || { echo "gui.sh: --prefix is required" >&2; usage; }
prefix=$(cd "$prefix" 2>/dev/null && pwd) || { echo "gui.sh: no such prefix" >&2; exit 2; }
[ -x "$prefix/bin/nidhogg" ] || { echo "gui.sh: $prefix/bin/nidhogg not found" >&2; exit 2; }
case $size in [0-9]*x[0-9]*) ;; *) echo "gui.sh: --size is WxH" >&2; exit 2 ;; esac
[ -n "$tests" ] || tests=$checkout/test/src/nidhogg-tests
tests=$(cd "$tests" && pwd) || exit 2
W=${size%x*} H=${size#*x}
case $compositor in sway|mutter|recorder) ;; *) echo "gui.sh: --compositor is sway, mutter or recorder" >&2; exit 2 ;; esac
case $rprofile in sway|kwin|mutter) ;; *) echo "gui.sh: --recorder-profile is sway, kwin or mutter" >&2; exit 2 ;; esac
if [ "$compositor" = recorder ]; then
  [ -n "$recorder" ] && [ -x "$recorder" ] || { echo "gui.sh: nh-recorder not built (--recorder BIN or \$REC: '$recorder')" >&2; exit 2; }
  recorder=$(cd "$(dirname "$recorder")" && pwd)/$(basename "$recorder")
  [ "$monitors" = 1 ] || { echo "gui.sh: the recorder has one output (--monitors 1)" >&2; exit 2; }
fi
known=$tests/gui-known-differences.eld
# the recorder's scale is a 120th: round(S * 120)
s120=$(awk -v s="$scale" 'BEGIN { printf "%d", s * 120 + 0.5 }')

# ---------------------------------------------------------------- in the session (private bus)
if [ "${GLITNIR_GUI_INNER:-}" = 1 ]; then
  display=glitnir-gui-$GLITNIR_GUI_PID
  mpid= epid=
  cleanup() {
    # the compositor's peak resident set (VmHWM) while it is still alive
    [ -z "$mpid" ] || \
      echo "gui.sh: $compositor peak RSS: $(sed -n 's/^VmHWM:[[:space:]]*\(.*\)$/\1/p' "/proc/$mpid/status" 2>/dev/null) (scale $scale)"
    for p in $epid $mpid; do kill "$p" 2>/dev/null; done
    for p in $epid $mpid; do wait "$p" 2>/dev/null; done
    # the sockets and files the run left in its XDG dir (dbus makes an empty dbus-1/services), listed after the compositor is gone and before the outer trap removes it
    [ -z "${GLITNIR_GUI_LEFTOVER:-}" ] || find "$XDG_RUNTIME_DIR" -mindepth 1 -not -type d > "$GLITNIR_GUI_LEFTOVER" 2>&1
  }
  trap cleanup EXIT
  trap 'exit 1' INT TERM

  clog=$GLITNIR_GUI_RUN/$compositor.log
  lw=$(awk -v w="$W" -v s="$scale" 'BEGIN { printf "%d", w / s + 0.5 }')
  ctl=
  case $compositor in
    mutter)
      vm=()
      for ((i = 0; i < monitors; i++)); do vm+=(--virtual-monitor "$size"); done
      mutter --headless --wayland --no-x11 --wayland-display "$display" "${vm[@]}" > "$clog" 2>&1 &
      mpid=$!
      ;;
    sway)
      # no bar, no borders, no XWayland (it would take an X display in the shared /tmp), every client floating
      {
        echo "output HEADLESS-1 mode ${size} scale $scale position 0 0"
        echo "xwayland disable"
        echo "default_border none"
        echo "default_floating_border none"
        echo 'for_window [app_id=".*"] floating enable'
      } > "$GLITNIR_GUI_RUN/sway.conf"
      WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 SWAYSOCK="$XDG_RUNTIME_DIR/sway-ipc.sock" \
        sway -c "$GLITNIR_GUI_RUN/sway.conf" > "$clog" 2>&1 &
      mpid=$!
      ;;
    recorder)
      ctl=$XDG_RUNTIME_DIR/nh-recorder.ctl
      # the recorder takes a profile file; they sit next to its source (the checkout is /src in a container)
      "$recorder" --output "${size}@${s120}" --profile "$(dirname "$self")/recorder/profiles/$rprofile.json" --socket "$display" --control "$ctl" > "$clog" 2>&1 &
      mpid=$!
      ;;
  esac
  for _ in $(seq 150); do
    if [ "$compositor" = sway ]; then
      sock=$(compgen -G "$XDG_RUNTIME_DIR/wayland-*" | grep -v '\.lock$' | head -n 1)
      [ -n "$sock" ] && [ -S "$sock" ] && { display=${sock##*/}; break; }
    elif [ "$compositor" = recorder ]; then
      grep -q '^READY' "$clog" 2>/dev/null && [ -S "$XDG_RUNTIME_DIR/$display" ] && break
    else
      [ -S "$XDG_RUNTIME_DIR/$display" ] && break
    fi
    kill -0 "$mpid" 2>/dev/null || { echo "gui.sh: $compositor exited early:"; cat "$clog"; exit 1; }
    sleep 0.1
  done
  [ -S "$XDG_RUNTIME_DIR/$display" ] || { echo "gui.sh: $compositor has no display"; cat "$clog"; exit 1; }

  # the scale (WAYLAND_DISPLAY is set only for the commands below)
  export WAYLAND_DISPLAY=$display
  if [ "$compositor" = mutter ]; then
    # gdctl on the session's own mutter
    names=$(gdctl show | grep -o 'Monitor Meta-[0-9]*' | sed 's/Monitor //')
    args=() x=0 first=--primary
    for n in $names; do
      args+=(--logical-monitor $first --monitor "$n" --scale "$scale" --x "$x" --y 0)
      first=; x=$((x + lw))
    done
    gdctl set "${args[@]}" || { echo "gui.sh: gdctl could not set scale $scale"; exit 1; }
  elif [ "$compositor" = sway ]; then
    export SWAYSOCK=$XDG_RUNTIME_DIR/sway-ipc.sock
    for ((i = 2; i <= monitors; i++)); do
      swaymsg create_output > /dev/null || { echo "gui.sh: swaymsg could not create output $i"; exit 1; }
      swaymsg "output HEADLESS-$i mode $size scale $scale position $(((i - 1) * lw)) 0" > /dev/null \
        || { echo "gui.sh: swaymsg could not set output $i"; exit 1; }
    done
  fi

  export GLITNIR_GUI_SCALE=$scale GLITNIR_GUI_SIZE=$size GLITNIR_GUI_TESTS=$tests GLITNIR_GUI_SELECT=$select
  export GLITNIR_GUI_COMPOSITOR=$compositor GLITNIR_GUI_KNOWN=$known GLITNIR_GUI_FILES=$files GLITNIR_GUI_TAG=$tag
  [ -z "$ctl" ] || export GLITNIR_GUI_RECORDER=$ctl
  export GDK_BACKEND=wayland NO_AT_BRIDGE=1
  unset DISPLAY EMACS_SOCKET_NAME
  for kv in ${envs[@]+"${envs[@]}"}; do export "$kv"; done
  [ "$csd" = 0 ] || export GTK_CSD=1 NIDHOGG_SHELL_FORCE_CSD=1 GLITNIR_GUI_CSD=1
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
  [ "$rc" = 0 ] || { echo "gui.sh: nidhogg exited with $rc; the $compositor log:"; tail -n 20 "$clog"; }
  [ "$rc" = 0 ] && exit 0 || exit 1
fi

# ---------------------------------------------------------------- the smoke tier: one mutter per tagged file and scale
if [ "$smoke" = 1 ]; then
  smoked=$(grep -l 'gnome-smoke' "$tests"/*-gui-tests.el 2>/dev/null)
  [ -n "$smoked" ] || { echo "gui.sh: no *-gui-tests.el file in $tests holds a gnome-smoke row" >&2; exit 1; }
  export GLITNIR_GUI_MEMORY=${GLITNIR_GUI_MEMORY:-2g}
  smoke_rc=0
  for f in $smoked; do
    for s in 1.25 1.5; do
      echo "gui.sh: smoke: $(basename "$f") at scale $s"
      bash "$self" --prefix "$prefix" --compositor mutter --scale "$s" --size "$size" --monitors "$monitors" \
        --tests "$tests" --timeout "$timeout" --files "$(basename "$f")" --tag gnome-smoke \
        ${select:+--select "$select"} ${container:+--container "$container"} \
        $([ "$daemon" = 0 ] || echo --daemon) $([ "$csd" = 0 ] || echo --force-csd) \
        $(for kv in ${envs[@]+"${envs[@]}"}; do echo --env "$kv"; done) || smoke_rc=1
    done
  done
  exit $smoke_rc
fi

# ---------------------------------------------------------------- dry run: what would start
if [ "${GLITNIR_GUI_DRY:-}" = 1 ]; then
  case $compositor in
    sway) echo "sway -c RUN/sway.conf   (output HEADLESS-1 mode $size scale $scale; $monitors output(s))" ;;
    mutter) echo "mutter --headless --wayland --no-x11 --wayland-display NAME ($monitors x --virtual-monitor $size; scale $scale)" ;;
    recorder) echo "$recorder --output ${size}@${s120} --profile $(dirname "$self")/recorder/profiles/$rprofile.json --socket NAME --control CTL" ;;
  esac
  echo "$prefix/bin/nidhogg -Q --fullscreen -l $checkout/test/src/nidhogg-tests/gui-runner.el"
  exit 0
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
  args=(--prefix "$inprefix" --compositor "$compositor" --recorder-profile "$rprofile" --scale "$scale" --size "$size"
        --monitors "$monitors" --tests /src/test/src/nidhogg-tests --timeout "$timeout")
  [ "$compositor" != recorder ] || args+=(--recorder /opt/nh-recorder)
  [ -z "$select" ] || args+=(--select "$select")
  [ "$daemon" = 0 ] || args+=(--daemon)
  [ "$csd" = 0 ] || args+=(--force-csd)
  [ -z "$files" ] || args+=(--files "$files")
  [ -z "$tag" ] || args+=(--tag "$tag")
  [ -z "$wdebug" ] || args+=(--wayland-debug /out/wayland-debug.txt)
  for kv in ${envs[@]+"${envs[@]}"}; do args+=(--env "$kv"); done
  out=
  mounts=(-v "$checkout:/src:ro,Z" -v "$prefix:$inprefix:ro,Z")
  [ "$compositor" != recorder ] || mounts+=(-v "$recorder:/opt/nh-recorder:ro,Z")
  # a caller's GLITNIR_GUI_SOCKDIR (a host directory) is where the run's XDG_RUNTIME_DIR is made, so the caller can see it
  sockenv=()
  if [ -n "${GLITNIR_GUI_SOCKDIR:-}" ]; then
    mounts+=(-v "$GLITNIR_GUI_SOCKDIR:$GLITNIR_GUI_SOCKDIR:Z"); sockenv=(-e "GLITNIR_GUI_SOCKDIR=$GLITNIR_GUI_SOCKDIR")
    [ -z "${GLITNIR_GUI_LEFTOVER:-}" ] || sockenv+=(-e "GLITNIR_GUI_LEFTOVER=$GLITNIR_GUI_LEFTOVER")
  fi
  [ "$tests" = "$checkout/test/src/nidhogg-tests" ] || mounts+=(-v "$tests:/src/test/src/nidhogg-tests:ro,Z")
  if [ -n "$wdebug" ]; then
    out=$(mktemp -d "${TMPDIR:-/tmp}/glitnir-gui-out.XXXXXX")
    mounts+=(-v "$out:/out:Z")
  fi
  # deps.runtime of the tarball, mutter (the compositor and gdctl), dbus-daemon, software GL, one monospace font
  podman run --rm --name "$name" --memory "${GLITNIR_GUI_MEMORY:-6g}" --memory-swap "${GLITNIR_GUI_MEMORY:-6g}" \
    ${sockenv[@]+"${sockenv[@]}"} "${mounts[@]}" "$image" bash -c '
      set -eu
      pkgs=$(sed "s/#.*//" "'"$inprefix"'/share/nidhogg/deps.runtime" 2>/dev/null || true)
      dnf -y -q --setopt=install_weak_deps=False install mutter sway grim pixman libxkbcommon libcap dbus-daemon \
        mesa-dri-drivers mesa-libEGL mesa-libgbm python3-gobject-base dejavu-sans-mono-fonts procps-ng util-linux \
        $(case " $* " in *" mutter "*) echo pipewire wireplumber gstreamer1-plugins-base pipewire-gstreamer pipewire-utils ;; esac) \
        $pkgs > /tmp/dnf.log 2>&1 || { cat /tmp/dnf.log; exit 1; }
      # sway ships with file capabilities a container refuses to exec: a copy without them
      cp /usr/bin/sway /usr/local/bin/sway; setcap -r /usr/local/bin/sway 2>/dev/null || true
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
case $compositor in
  sway) need="sway swaymsg dbus-run-session" ;;
  mutter) need="mutter gdctl dbus-run-session" ;;
  *) need="dbus-run-session" ;;
esac
for t in $need; do
  command -v "$t" > /dev/null || { echo "gui.sh: $t not found (the host needs $need; or use --container)" >&2; exit 1; }
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
  dbus-run-session -- bash "$self" --prefix "$prefix" --compositor "$compositor" --recorder-profile "$rprofile" ${recorder:+--recorder "$recorder"} --scale "$scale" --size "$size" --monitors "$monitors" \
    --tests "$tests" --timeout "$timeout" ${select:+--select "$select"} ${wdebug:+--wayland-debug "$wdebug"} \
    ${files:+--files "$files"} ${tag:+--tag "$tag"} \
    $([ "$daemon" = 0 ] || echo --daemon) $([ "$csd" = 0 ] || echo --force-csd) $(for kv in ${envs[@]+"${envs[@]}"}; do echo --env "$kv"; done)
exit $?
