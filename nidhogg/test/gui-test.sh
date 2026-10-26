#!/usr/bin/env bash
# Checks of nidhogg/test/gui.sh itself.  The first two run on the host and start no compositor (GLITNIR_GUI_DRY=1);
# the third runs a real run in a container.  Needs GUI_TEST_PREFIX, an unpacked Nidhogg (the third also needs podman).
#   GUI_TEST_PREFIX=DIR bash nidhogg/test/gui-test.sh [--container]
# (--container is ignored: it tells the slot scripts that this script starts its own container.)
set -u
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
gui=$here/gui.sh
prefix=${GUI_TEST_PREFIX:-}
[ -x "$prefix/bin/nidhogg" ] || { echo "gui-test.sh: set GUI_TEST_PREFIX to an unpacked Nidhogg" >&2; exit 2; }
fail=0
ok() { echo "ok $1"; }
bad() { echo "FAIL $1: $2"; fail=1; }

# gui-sh-default-is-sway
out=$(GLITNIR_GUI_DRY=1 bash "$gui" --prefix "$prefix" 2>&1); rc=$?
if [ $rc = 0 ] && grep -q '^sway -c ' <<< "$out"; then ok gui-sh-default-is-sway; else bad gui-sh-default-is-sway "rc=$rc: $out"; fi

# gui-sh-recorder-needs-binary
out=$(GLITNIR_GUI_DRY=1 REC= bash "$gui" --prefix "$prefix" --compositor recorder --recorder /nonexistent/nh-recorder 2>&1); rc=$?
if [ $rc = 2 ] && grep -q 'nh-recorder not built' <<< "$out"; then ok gui-sh-recorder-needs-binary; else bad gui-sh-recorder-needs-binary "rc=$rc: $out"; fi

# gui-sh-cleans-up-after-a-killed-client: SIGKILL Nidhogg in a sway run; gui.sh exits 1 within 10 s and leaves no
# socket under the run's XDG_RUNTIME_DIR (GLITNIR_GUI_SOCKDIR, a host directory mounted into the container).  The
# compositor is waited for by gui.sh's own cleanup before it exits, so no sway outlives the run.
name=gui-test-kill-$$
work=$(mktemp -d "${TMPDIR:-/tmp}/gui-test.XXXXXX")
mkdir "$work/tests" "$work/xdg"
cat > "$work/tests/sleep-gui-tests.el" <<'EL'
;;; sleep-gui-tests.el --- keep Nidhogg alive for gui-test.sh  -*- lexical-binding: t -*-
(require 'ert)
(ert-deftest sleep/forever () (sleep-for 600))
EL
GLITNIR_GUI_NAME=$name GLITNIR_GUI_SOCKDIR=$work/xdg bash "$gui" --prefix "$prefix" --container fedora-45 \
  --compositor sway --tests "$work/tests" > "$work/out.log" 2>&1 &
gpid=$!
up=0
for _ in $(seq 600); do   # the container installs packages first: up to 5 min
  if podman exec "$name" sh -c 'pgrep -x sway > /dev/null && pgrep -x nidhogg > /dev/null' 2> /dev/null; then up=1; break; fi
  kill -0 $gpid 2> /dev/null || break
  sleep 0.5
done
if [ $up = 0 ]; then
  bad gui-sh-cleans-up-after-a-killed-client "sway and nidhogg did not come up: $(tail -n 15 "$work/out.log")"
  kill $gpid 2> /dev/null; podman rm -f "$name" > /dev/null 2>&1
else
  sleep 2
  podman exec "$name" pkill -9 -x nidhogg
  t0=$SECONDS
  while kill -0 $gpid 2> /dev/null && [ $((SECONDS - t0)) -le 10 ]; do sleep 0.2; done
  if kill -0 $gpid 2> /dev/null; then
    bad gui-sh-cleans-up-after-a-killed-client "gui.sh still running 10 s after the kill"
    kill $gpid 2> /dev/null; podman rm -f "$name" > /dev/null 2>&1
  else
    wait $gpid; rc=$?
    left=$(find "$work/xdg" -mindepth 1 2> /dev/null)
    if [ $rc = 1 ] && [ -z "$left" ]; then ok gui-sh-cleans-up-after-a-killed-client
    else bad gui-sh-cleans-up-after-a-killed-client "rc=$rc, left: $left"; fi
  fi
fi
rm -r "$work"
exit $fail
