#!/usr/bin/env bash
# Diagnostic (wip/kwin-diag only): does KWin's virtual backend composite on this runner, and why not.
#   kwin-probe.sh LABEL PODMAN-DEVICE-ARGS...      (on the runner, after vgem is loaded)
set -u
label=$1; shift
img=registry.fedoraproject.org/fedora@sha256:6430abb162172c052b6ac9c9d6a219b1b21352508c04b1b0ff2eb9292943d881
echo "=== probe $label: devices $*"
podman run --rm "$@" -e LABEL="$label" "$img" bash -c '
  dnf -y -q --setopt=install_weak_deps=False install kwin kscreen dbus-tools dbus-daemon mesa-dri-drivers mesa-libEGL \
    mesa-libgbm procps-ng util-linux libdrm >/tmp/dnf.log 2>&1 || { tail -20 /tmp/dnf.log; exit 1; }
  for c in kwin_wayland kwin_wayland_wrapper; do cp /usr/bin/$c /usr/local/bin/$c; setcap -r /usr/local/bin/$c 2>/dev/null || true; done
  ls -l /dev/dri
  export XDG_RUNTIME_DIR=/tmp/xdg; mkdir -p -m 700 $XDG_RUNTIME_DIR
  export QT_LOGGING_RULES="kwin_*.debug=true;kwin_*.info=true" EGL_LOG_LEVEL=debug LIBGL_DEBUG=verbose
  dbus-run-session bash -c "
    kwin_wayland_wrapper --virtual --width 1280 --height 800 --scale 1 --no-lockscreen --no-global-shortcuts --no-kactivities > /tmp/kwin.log 2>&1 &
    for i in \$(seq 100); do compgen -G \"\$XDG_RUNTIME_DIR/wayland-*\" >/dev/null && break; sleep 0.1; done
    sleep 3
    dbus-send --session --print-reply=literal --dest=org.kde.KWin /KWin org.kde.KWin.supportInformation 2>&1 \
      | grep -E \"^(Compositing is|Compositing Type|OpenGL|EGL|Platform|GPU|Driver|Mesa)\" | sed \"s/^/[\$LABEL] info: /\"
    grep -i -E \"egl|gbm|drm|opengl|render|composit|libEGL|MESA|failed|error|warning\" /tmp/kwin.log | grep -v dbus-daemon | head -60 | sed \"s/^/[\$LABEL] log: /\"
  "'
