#!/usr/bin/env bash
# Checks of nidhogg/test/gui.sh itself.  The first three run on the host and start no compositor (GLITNIR_GUI_DRY=1);
# the last runs a real run in a container.  Needs GUI_TEST_PREFIX, an unpacked Nidhogg (the third also needs podman).
#   GUI_TEST_PREFIX=DIR [GUI_TEST_RECORDER=nh-recorder] bash nidhogg/test/gui-test.sh [--container]
# GUI_TEST_RECORDER (a built nh-recorder) adds the recorder's kill case.
# (--container is ignored: it tells the slot scripts that this script starts its own container.)
set -u
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
gui=$here/gui.sh
prefix=${GUI_TEST_PREFIX:-}
[ -x "$prefix/bin/nidhogg" ] || { echo "gui-test.sh: set GUI_TEST_PREFIX to an unpacked Nidhogg" >&2; exit 2; }
fail=0
works=() names=()
# a run that is interrupted or fails leaves no work directory and no container
trap 'for n in ${names[@]+"${names[@]}"}; do podman rm -f "$n" > /dev/null 2>&1; done; for w in ${works[@]+"${works[@]}"}; do rm -r "$w" 2> /dev/null; done' EXIT
trap 'exit 1' INT TERM
ok() { echo "ok $1"; }
bad() { echo "FAIL $1: $2"; fail=1; }

# gui-sh-default-is-sway
out=$(GLITNIR_GUI_DRY=1 bash "$gui" --prefix "$prefix" 2>&1); rc=$?
if [ $rc = 0 ] && grep -q '^sway -c ' <<< "$out"; then ok gui-sh-default-is-sway; else bad gui-sh-default-is-sway "rc=$rc: $out"; fi

# gui-sh-recorder-needs-binary
out=$(GLITNIR_GUI_DRY=1 REC= bash "$gui" --prefix "$prefix" --compositor recorder --recorder /nonexistent/nh-recorder 2>&1); rc=$?
if [ $rc = 2 ] && grep -q 'nh-recorder not built' <<< "$out"; then ok gui-sh-recorder-needs-binary; else bad gui-sh-recorder-needs-binary "rc=$rc: $out"; fi

# gui-sh-smoke-restarts-per-file: a dry --smoke run starts one mutter per gnome-smoke-tagged file and per scale (1.25, 1.5)
tests=$(cd "$here/../.." && pwd)/test/src/nidhogg-tests
nfiles=$(grep -l gnome-smoke "$tests"/*-gui-tests.el 2>/dev/null | wc -l)
out=$(GLITNIR_GUI_DRY=1 bash "$gui" --prefix "$prefix" --smoke 2>&1); rc=$?
nstart=$(grep -c '^mutter --headless' <<< "$out")
if [ $rc = 0 ] && [ "$nfiles" -ge 2 ] && [ "$nstart" = $((nfiles * 2)) ] && grep -q 'scale 1.25)' <<< "$out" \
   && grep -q 'scale 1.5)' <<< "$out" && ! grep -q '^sway ' <<< "$out"; then
  ok gui-sh-smoke-restarts-per-file
else bad gui-sh-smoke-restarts-per-file "rc=$rc files=$nfiles starts=$nstart: $out"; fi

# kill_case TEST COMPOSITOR PROCESS [GUI.SH ARGS]: SIGKILL Nidhogg in a run on COMPOSITOR; gui.sh exits 1 within 10 s and
# leaves no socket under the run's XDG_RUNTIME_DIR (GLITNIR_GUI_SOCKDIR, a host directory mounted into the container).
# The compositor (PROCESS) is waited for by gui.sh's own cleanup before it exits, so none outlives the run.  gui.sh's
# own trap removes the XDG directory afterwards, so its cleanup lists what is in the directory first (GLITNIR_GUI_LEFTOVER, a file
# next to it); the case reads that list, and fails when there is none.
kill_case() {
  local test=$1 comp=$2 proc=$3; shift 3
  local name=gui-test-kill-$comp-$$ work gpid up=0 t0 rc left
  work=$(mktemp -d "${TMPDIR:-/tmp}/gui-test.XXXXXX")
  works+=("$work"); names+=("$name")
  mkdir "$work/tests" "$work/xdg"
  cat > "$work/tests/sleep-gui-tests.el" <<'EL'
;;; sleep-gui-tests.el --- keep Nidhogg alive for gui-test.sh  -*- lexical-binding: t -*-
(require 'ert)
(ert-deftest sleep/forever () (sleep-for 600))
EL
  GLITNIR_GUI_NAME=$name GLITNIR_GUI_SOCKDIR=$work/xdg GLITNIR_GUI_LEFTOVER=$work/xdg/leftover.list \
    bash "$gui" --prefix "$prefix" --container fedora-45 --compositor "$comp" "$@" --tests "$work/tests" \
    > "$work/out.log" 2>&1 &
  gpid=$!
  for _ in $(seq 600); do   # the container installs packages first: up to 5 min
    if podman exec "$name" sh -c "pgrep -x $proc > /dev/null && pgrep -x nidhogg > /dev/null" 2> /dev/null; then up=1; break; fi
    kill -0 $gpid 2> /dev/null || break
    sleep 0.5
  done
  if [ $up = 0 ]; then
    bad "$test" "$comp and nidhogg did not come up: $(tail -n 15 "$work/out.log")"
    kill $gpid 2> /dev/null; podman rm -f "$name" > /dev/null 2>&1
    return
  fi
  sleep 2
  podman exec "$name" pkill -9 -x nidhogg
  t0=$SECONDS
  while kill -0 $gpid 2> /dev/null && [ $((SECONDS - t0)) -le 10 ]; do sleep 0.2; done
  if kill -0 $gpid 2> /dev/null; then
    bad "$test" "gui.sh still running 10 s after the kill"
    kill $gpid 2> /dev/null; podman rm -f "$name" > /dev/null 2>&1
    return
  fi
  wait $gpid; rc=$?
  if [ ! -f "$work/xdg/leftover.list" ]; then bad "$test" "rc=$rc: gui.sh wrote no list of what was left"; return; fi
  left=$(cat "$work/xdg/leftover.list")
  if [ $rc = 1 ] && [ -z "$left" ]; then ok "$test"
  else bad "$test" "rc=$rc, left: $left"; fi
}

kill_case gui-sh-cleans-up-after-a-killed-client sway sway
if [ -x "${GUI_TEST_RECORDER:-}" ]; then
  kill_case gui-sh-cleans-up-after-a-killed-recorder-client recorder nh-recorder --recorder "$GUI_TEST_RECORDER"
else
  echo "skipped gui-sh-cleans-up-after-a-killed-recorder-client (GUI_TEST_RECORDER is not a built nh-recorder)"
fi
exit $fail
