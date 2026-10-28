#!/usr/bin/env bash
# The input-method matrix: Nidhogg's text-input-v3 path against real input methods, each combination in its own
# memory-capped (3 GiB) podman container, driven by nidhogg/test/gui.sh --ime and the rows of
# test/src/nidhogg-tests/nhwl-ime-tests.el.  Never touches the caller's display, session bus or input method.
#
#   nidhogg/test/ime-matrix.sh --prefix DIR [--only sway-fcitx5|kwin-fcitx5|gnome-ibus] [--out FILE.eld]
#                              [--timeout SEC] [--mem SIZE] [--container]   (--mem: every cell's cap; default 3g, KWin 4g)
#   nidhogg/test/ime-matrix.sh --smoke COMBO     the input method (and mutter, for gnome-ibus) starts in the container;
#                                                 prints the installed size and the cgroup's peak memory (no Nidhogg, no rows)
#   GLITNIR_GUI_DRY=1 nidhogg/test/ime-matrix.sh --prefix DIR   print the gui.sh command of each combination and exit
#
# Combinations (compositor + input method, pinyin profile, keys typed by wtype; mutter: RemoteDesktop over D-Bus):
#   sway-fcitx5   headless sway, fcitx5 as a client (input-method-v2)
#   kwin-fcitx5   KWin --virtual, fcitx5 launched by KWin (kwinrc)
#   gnome-ibus    headless mutter (GNOME's text-input and IBus path) with ibus-daemon; gnome-shell itself is not started
# --out FILE.eld gets one plist per row: (:combo C :row R :result pass|fail|skipped :reason S).  A combination that
# cannot be driven is `skipped' with the reason, never a failure.  Exit 0 when no row failed, 1 otherwise, 2 usage.
set -u

self=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")
gui=$(dirname "$self")/gui.sh
rows=(pinyin-commits-cjk preedit-overlay-shown cursor-rectangle-follows-point)
usage() { sed -n '2,/^# cannot be driven/s/^# \{0,1\}//p' "$self" >&2; exit 2; }

combo_compositor() { case $1 in sway-fcitx5) echo sway ;; kwin-fcitx5) echo kwin ;; gnome-ibus) echo mutter ;; esac; }
combo_ime() { case $1 in *-fcitx5) echo fcitx5 ;; gnome-ibus) echo ibus ;; esac; }

prefix= only= out= timeout=300 smoke= mem=
while [ $# -gt 0 ]; do
  case $1 in
    --prefix) prefix=${2:-}; shift 2 ;;
    --only) only=${2:-}; shift 2 ;;
    --out) out=${2:-}; shift 2 ;;
    --timeout) timeout=${2:-}; shift 2 ;;
    --smoke) smoke=${2:-}; shift 2 ;;
    --mem) mem=${2:-}; shift 2 ;;
    --container) shift ;;   # always on; the word tells the slot wrappers this command starts its own containers
    -h|--help) usage ;;
    *) echo "ime-matrix.sh: unknown argument $1" >&2; usage ;;
  esac
done
combos=(sway-fcitx5 kwin-fcitx5 gnome-ibus)
for c in "$only" "$smoke"; do
  [ -z "$c" ] || [ -n "$(combo_compositor "$c")" ] || { echo "ime-matrix.sh: unknown combination $c" >&2; exit 2; }
done
[ -z "$only" ] || combos=("$only")

# ---------------------------------------------------------------- smoke: the daemons start inside the 3 GiB cap
if [ -n "$smoke" ]; then
  GUI_SH_SOURCE_ONLY=1 source "$gui"
  case $smoke in
    sway-fcitx5) pkgs="sway dbus-daemon procps-ng util-linux fcitx5 fcitx5-chinese-addons wtype" ;;
    kwin-fcitx5) pkgs="kwin kscreen dbus-daemon procps-ng util-linux fcitx5 fcitx5-chinese-addons wtype" ;;
    gnome-ibus) pkgs="mutter dbus-daemon procps-ng util-linux ibus ibus-libpinyin" ;;
  esac
  podman run --rm --name "ime-smoke-$$" --memory 3g --memory-swap 3g "$IMAGE_fedora_45" bash -c '
    combo=$1; pkgs=$2
    dnf -y -q --setopt=install_weak_deps=False install $pkgs > /tmp/dnf.log 2>&1 || { tail -n 30 /tmp/dnf.log; exit 1; }
    echo "smoke $combo: installed, / is $(du -sxm / 2>/dev/null | cut -f1) MiB (base image included)"
    export XDG_RUNTIME_DIR=/tmp/xdg HOME=/tmp/home; mkdir -p $XDG_RUNTIME_DIR $HOME; chmod 700 $XDG_RUNTIME_DIR
    case $combo in
      sway-fcitx5|kwin-fcitx5)
        fcitx5 --version; command -v wtype
        ls /usr/share/applications | grep -i fcitx || echo "no fcitx5 desktop file"
        ls /usr/lib64/fcitx5 | grep -i pinyin || echo "no pinyin addon"
        dbus-run-session -- bash -c "fcitx5 --disable=wayland > /tmp/fcitx5.log 2>&1 & sleep 3; pgrep -x fcitx5 && echo \"smoke $combo: fcitx5 is running\" || { cat /tmp/fcitx5.log; exit 1; }" ;;
      gnome-ibus)
        ibus version; ibus-daemon --help | grep -e --address -e --panel -e --config
        ls /usr/share/ibus/component | grep -i pinyin
        dbus-run-session -- bash -c "
          export IBUS_ADDRESS=unix:path=\$XDG_RUNTIME_DIR/ibus.sock
          ibus-daemon --address \$IBUS_ADDRESS --panel disable > /tmp/ibus.log 2>&1 &
          sleep 2
          mutter --headless --wayland --no-x11 --wayland-display ime-smoke --virtual-monitor 2880x1800 > /tmp/mutter.log 2>&1 &
          sleep 5
          export WAYLAND_DISPLAY=ime-smoke
          export GSETTINGS_BACKEND=memory
          for i in 1 2 3; do ibus engine libpinyin && break; done; echo \"smoke $combo: ibus engine now: \$(ibus engine)\"
          pgrep -x ibus-daemon && pgrep -x mutter && echo \"smoke $combo: ibus-daemon and mutter are running\"
          tail -n 5 /tmp/ibus.log" ;;
    esac
    echo "smoke $combo: cgroup memory peak $(( $(cat /sys/fs/cgroup/memory.peak 2>/dev/null || echo 0) / 1048576 )) MiB of the 3072 MiB cap"
  ' _ "$smoke" "$pkgs"
  exit $?
fi

# ---------------------------------------------------------------- the matrix
[ -n "$prefix" ] || { echo "ime-matrix.sh: --prefix is required" >&2; usage; }
[ -z "$out" ] || : > "$out" || exit 2
work=$(mktemp -d "${TMPDIR:-/tmp}/ime-matrix.XXXXXX")
trap 'rm -r "$work"' EXIT
failed=0

# put ROW's result in OUT: parsed from the gui runner's log LOG
record() {   # combo row result reason
  local line
  line=$(printf '(:combo "%s" :row "nhwl-ime/%s" :result %s :reason "%s")' "$1" "$2" "$3" "${4//\"/\'}")
  echo "$line"
  [ -z "$out" ] || echo "$line" >> "$out"
}

for combo in "${combos[@]}"; do
  log=$work/$combo.log
  echo "ime-matrix: $combo ($(combo_compositor "$combo") + $(combo_ime "$combo"))"
  GLITNIR_GUI_MEMORY=${mem:-$([ "$combo" = kwin-fcitx5 ] && echo 4g || echo 3g)} bash "$gui" --prefix "$prefix" --container fedora-45 --compositor "$(combo_compositor "$combo")" \
    --ime "$(combo_ime "$combo")" --files nhwl-ime-tests.el --select 'nhwl-ime/' --timeout "$timeout" \
    --wayland-debug "$log" --env "NHWL_IME_COMBO=$combo" --env NHWL_IME_TRACE=/out/wayland-debug.txt
  rc=$?
  [ "${GLITNIR_GUI_DRY:-}" != 1 ] || continue
  for row in "${rows[@]}"; do
    name=nhwl-ime/$row
    verdict=$(grep -E "^(passed|skipped|FAILED) $name\$" "$log" 2>/dev/null | head -n 1 | cut -d' ' -f1)
    case $verdict in
      passed) record "$combo" "$row" pass "" ;;
      skipped) record "$combo" "$row" skipped "$(sed -n "s/^nhwl-ime-skip $name: //p" "$log" | head -n 1)" ;;
      FAILED) record "$combo" "$row" fail "$(grep -A1 "^FAILED $name\$" "$log" | sed -n 2p | cut -c1-200)"; failed=1 ;;
      *)
        # no verdict: the container, the compositor or Nidhogg did not come up.  Whole-combination reason from the log.
        reason=$(grep -m1 -E '^gui.sh: |dnf|Error' "$log" 2>/dev/null | cut -c1-200)
        record "$combo" "$row" skipped "the combination did not run (rc $rc): ${reason:-no log}" ;;
    esac
  done
done
exit $failed
