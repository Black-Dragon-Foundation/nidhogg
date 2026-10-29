#!/usr/bin/env bash
# The shell's budgets: Nidhogg's own Wayland shell with the presenter thread (A) and with presenting inline (C), in its
# own headless sway inside a memory-capped container, one plist per row, each judged on an absolute target.
#
#   nidhogg/test/budgets.sh --prefix-a SHELL [--compositor sway] [--scale S] [--size WxH] [--runs N] [--refresh 120]
#                           [--out FILE.eld] [--window SEC] [--resize-steps N] [--motion-frames N] [--motion-size WxH]
#                           [--config q|drgnx --drgnx DIR]
#   nidhogg/test/budgets.sh --self-test --prefix-a SHELL [--out FILE.eld]
#   nidhogg/test/budgets.sh --reference --prefix-a PGTK [--runs N] [--out FILE.eld]
#
#   --prefix-a        an unpacked Nidhogg build with --nidhogg-wayland.  Side C is the same prefix with NHWL_PRESENTER=0
#   --reference       the pgtk reference run: --prefix-a is a pgtk build, only the idle, typing, scroll and resize
#                     sessions run on it, and every row is :kind report.  Its cpu-scroll and pss-scroll at 1920x1080 x 1
#                     are frozen into REF_CPU_SCROLL and REF_PSS_SCROLL below
#   --scale, --size   the output of the row sessions (defaults 1, 1920x1080)
#   --runs N          sessions per side; a row's value is the median over them (default 5)
#   --window SEC      the idle, typing and scroll windows (default 60)
#   --resize-steps N  `swaymsg resize' steps (default 200)
#   --motion-frames N frames of the motion bench (default 600), on --motion-size (default 3840x2160) at scale 1.5 and
#                     120 Hz
#   --config C        q (default): both sides start with -Q.  drgnx: both sides start with drgnx's own configuration, the way
#                     bin/drgnx starts its Emacs (--init-directory DIR/init); --drgnx names the checkout.  It is mounted
#                     read-only at its own path, and the run's HOME is private, so drgnx's state lands in the run.  The
#                     targets that name a 1920x1080 x 1 cell (pss-idle, cpu-typing, cpu-scroll, pss-scroll) were set on -Q
#                     and are judged on --config q only; on drgnx they are report
#   --refresh N       the speed sessions' sway output runs at N Hz (default 120; the scroll bar is a 120 Hz bar)
#   --container fedora-45  accepted and ignored: the script always runs in its own container, and the slot wrappers
#                     (drgnx-heavy-slot.sh) only start the command on the host, not inside a dev container, when they see this
#   --out FILE        the plists, one per line (default budgets.eld in the current directory)
#   --self-test       --runs 1, 1280x720, 3 s windows, 10 resize steps, 30 motion frames; then every line must read
#                     back as a plist with :target, :kind and :verdict, none may be nil, and freeze-watchdog may not be
#                     skipped
#   BUDGETS_MEMORY    the container's memory cap (default 6g); BUDGETS_CPUSET pins it (podman --cpuset-cpus)
#
# The speed sessions (V2's three bars, C1, C4) run each side with WAYLAND_DEBUG=client and read the traces with
# wl-latency.el: key-to-commit and frame intervals.  They also run the stall watchdog (threshold 100 ms) and count its
# stalls over the typing and the scroll.  C only exists on a build with the presenter thread; on an older build the rows
# that need it are skipped.  Rows: typing-stall-max, typing-stall-p99, latency-typing, latency-typing-inline,
# scroll-120-dropped, scroll-120-dropped-inline, scroll-120-presenter-late, main-wait-max, freeze-watchdog,
# cpu-typing-inline; cpu-scroll is fixed work (CPU seconds).  The -inline rows compare A with C (:c), the rest are judged
# on the targets below.
# The workload runs in the shell's own configuration (-Q) unless --config drgnx says otherwise.
#
# idle-wakeups-blink repeats the idle window with the cursor blinking, so redisplays run.
# A row is (:row NAME :unit U :a (MEDIAN SD) [:c (MEDIAN SD)] :target T :kind gate|budget|report :source S
#   :verdict pass|fail|report|skip), lower is better.  gate: a V2 section 1 bar; budget: a number the plans gave; report:
#   no verdict.  A row that cannot be measured here is (:row NAME :skipped "REASON" ... :verdict skip): never nil, never
#   a made-up number.  scroll-120-dropped carries :presenter-late and :main-late, the split of its late frames per 1000
#   (the presenter-late row is the presenter's own count, or the proxy on a build without it; the rest is the main
#   thread's).
# Exit 0 when the rows were produced (verdicts are the caller's to read), 1 when a session did not come up, 2 usage.
set -u

self=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")
checkout=$(cd "$(dirname "$self")/../.." && pwd)
# the container image, pinned by digest, is gui.sh's
GUI_SH_SOURCE_ONLY=1; . "$(dirname "$self")/gui.sh"; GUI_SH_SOURCE_ONLY=
self=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")   # gui.sh set its own

# ---------------------------------------------------------------- the targets, all in one place
# The pgtk reference run's values (see --reference): empty until the gate fills them.  While empty the two rows are report.
REF_CPU_SCROLL=${BUDGETS_REF_CPU_SCROLL:-}   # CPU s for the 2000 fixed scroll steps at 1920x1080 x 1
REF_PSS_SCROLL=${BUDGETS_REF_PSS_SCROLL:-}   # kB after that work, at 1920x1080 x 1
# ROW|TARGET|KIND|SOURCE|CELL (CELL: the only cell the target applies to; other cells are report).  Units are the rows':
# ms, per 1000 frames, % of one core, kB (120 MB = 122880, 50 MB = 51200), counts.  A row not listed is report.
TARGETS="typing-stall-max|16|gate|V2 section 1 line 26: typing never stalls longer than ~16 ms
latency-typing|16|gate|V2 section 1 line 26 (key to commit p95)
scroll-120-presenter-late|0|gate|V2 section 1 line 25: no dropped frame at 120 Hz, presenter misses only
freeze-watchdog|0|gate|V2 section 1 lines 27 and 61: no freeze over 100 ms (stalls the watchdog counted)
typing-stall-p99|8.33|report|the budgets table; not a V2 bar
main-wait-max|1|budget|the budgets table
idle-wakeups-threads|0|budget|shells section 3: no wake-ups from the shell's threads
motion-compose-p95|8.33|budget|shells section 3, the budgets table
motion-interval-p95|9.2|budget|shells section 3, the budgets table
motion-cores|1|budget|the budgets table: under one core
resize-stuck|0|budget|shells section 3: no stuck resize
pss-compositor|51200|budget|shells section 3: the test compositor at 50 MB
pss-idle|122880|budget|pgtk gate run 2026-10-27, 120 MB|1920x1080@1
cpu-typing|8.59|budget|pgtk gate run 2026-10-27|1920x1080@1
cpu-scroll|$REF_CPU_SCROLL|budget|pgtk reference run|1920x1080@1
pss-scroll|$REF_PSS_SCROLL|budget|pgtk reference run|1920x1080@1"

pa= scale=1 size=1920x1080 runs=5 out=budgets.eld window=60 steps=200 mframes=600 msize=3840x2160 selftest=0 mode=host refresh=120 sscroll=10 fixed=2000 nh_env=() HAS_W3=0 reference=0 config=q drgnx=
while [ $# -gt 0 ]; do
  case $1 in
    --prefix-a) pa=${2:-}; shift 2 ;;
    --reference) reference=1; shift ;;
    --config) config=${2:-}; shift 2 ;;
    --drgnx) drgnx=${2:-}; shift 2 ;;
    --compositor) [ "${2:-}" = sway ] || { echo "budgets.sh: only --compositor sway" >&2; exit 2; }; shift 2 ;;
    --scale) scale=${2:-}; shift 2 ;;
    --size) size=${2:-}; shift 2 ;;
    --runs) runs=${2:-}; shift 2 ;;
    --out) out=${2:-}; shift 2 ;;
    --window) window=${2:-}; shift 2 ;;
    --resize-steps) steps=${2:-}; shift 2 ;;
    --motion-frames) mframes=${2:-}; shift 2 ;;
    --motion-size) msize=${2:-}; shift 2 ;;
    --refresh) refresh=${2:-}; shift 2 ;;
    --container) [ "${2:-}" = fedora-45 ] || { echo "budgets.sh: only --container fedora-45" >&2; exit 2; }; shift 2 ;;
    --self-test) selftest=1; shift ;;
    --in-container) mode=container; shift ;;
    --session) mode=session; shift ;;
    -h|--help) sed -n '2,/^# Exit/s/^# \{0,1\}//p' "$self" >&2; exit 2 ;;
    *) echo "budgets.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
if [ "$selftest" = 1 ]; then runs=1 size=1280x720 window=3 steps=10 mframes=30 msize=1280x720 sscroll=2 fixed=100; fi
case $refresh in ''|*[!0-9]*) echo "budgets.sh: --refresh is a whole number of Hz" >&2; exit 2 ;; esac
[ -x "$pa/bin/nidhogg" ] || { echo "budgets.sh: --prefix-a must be an unpacked Nidhogg" >&2; exit 2; }
[ "$reference$selftest" != 11 ] || { echo "budgets.sh: --reference and --self-test do not combine" >&2; exit 2; }
case $size$msize in *x*x*) ;; *) echo "budgets.sh: sizes are WxH" >&2; exit 2 ;; esac
case $config in
  q) nh_init=(-Q) ;;
  drgnx)
    [ -f "$drgnx/init/init.el" ] || { echo "budgets.sh: --config drgnx needs --drgnx DIR, a drgnx checkout" >&2; exit 2; }
    [ "$mode" != host ] || drgnx=$(cd "$drgnx" && pwd)
    nh_init=(--init-directory "$drgnx/init") ;;
  *) echo "budgets.sh: --config is q or drgnx" >&2; exit 2 ;;
esac
cfgargs=(--config "$config"); [ -z "$drgnx" ] || cfgargs+=(--drgnx "$drgnx")
sargs=(--prefix-a "$pa" --scale "$scale" --size "$size" --runs "$runs" --window "$window"
       --resize-steps "$steps" --motion-frames "$mframes" --motion-size "$msize" --refresh "$refresh" "${cfgargs[@]}")
[ "$reference" = 0 ] || sargs+=(--reference)

# ---------------------------------------------------------------- on the host: the container
if [ "$mode" = host ]; then
  command -v podman > /dev/null || { echo "budgets.sh: podman not found" >&2; exit 1; }
  outdir=$(cd "$(dirname "$out")" && pwd) && outfile=$(basename "$out")
  # a tarball runs at the prefix it was built for (BUILDINFO), so it is mounted there
  inpa=$(sed -n 's/^prefix=//p' "$pa/share/nidhogg/BUILDINFO" 2>/dev/null); [ -n "$inpa" ] || inpa=$(cd "$pa" && pwd)
  pa=$(cd "$pa" && pwd)
  sargs=(--prefix-a "$inpa" --scale "$scale" --size "$size" --runs "$runs" --window "$window"
         --resize-steps "$steps" --motion-frames "$mframes" --motion-size "$msize" --refresh "$refresh" "${cfgargs[@]}" --out "/out/$outfile")
  [ "$selftest" = 0 ] || sargs+=(--self-test)
  [ "$reference" = 0 ] || sargs+=(--reference)
  name=budgets-$$
  trap 'podman rm -f "$name" > /dev/null 2>&1' EXIT
  trap 'exit 1' INT TERM
  # the same container recipe as gui.sh, plus wtype (the typing) and procps
  podman run --rm --name "$name" --memory "${BUDGETS_MEMORY:-6g}" --memory-swap "${BUDGETS_MEMORY:-6g}" \
    ${BUDGETS_CPUSET:+--cpuset-cpus "$BUDGETS_CPUSET"} \
    ${drgnx:+-v "$drgnx:$drgnx:ro,z"} \
    -v "$checkout:/src:ro,z" -v "$pa:$inpa:ro,z" -v "$outdir:/out:Z" "$IMAGE_fedora_45" bash -c '
      set -eu
      pkgs=$(cat "'"$inpa"'/share/nidhogg/deps.runtime" 2>/dev/null | sed "s/#.*//" || true)
      dnf -y -q --setopt=install_weak_deps=False install sway wtype pixman libxkbcommon libcap dbus-daemon \
        mesa-dri-drivers mesa-libEGL mesa-libgbm dejavu-sans-mono-fonts procps-ng util-linux \
        $pkgs > /tmp/dnf.log 2>&1 || { cat /tmp/dnf.log; exit 1; }
      cp /usr/bin/sway /usr/local/bin/sway; setcap -r /usr/local/bin/sway 2>/dev/null || true
      printf "%s\n" "#!/bin/bash" "a=(); while [ \$# -gt 0 ]; do if [ \"\$1\" = --dev ]; then a+=(--dev-bind \"\$2\" \"\$2\"); shift 2; else a+=(\"\$1\"); shift; fi; done" \
        "exec /usr/bin/bwrap \"\${a[@]}\"" > /usr/local/bin/bwrap
      chmod +x /usr/local/bin/bwrap
      exec bash /src/nidhogg/test/budgets.sh --in-container "$@"' _ "${sargs[@]}"
  exit $?
fi

# ---------------------------------------------------------------- in the container: a private session bus
if [ "$mode" = container ]; then
  run=$(mktemp -d /tmp/budgets.XXXXXX)
  mkdir -p "$run/home" "$run/cache" "$run/data" "$run/config"
  env -u WAYLAND_DISPLAY -u DISPLAY -u DBUS_SESSION_BUS_ADDRESS -u EMACS_SOCKET_NAME \
    HOME="$run/home" XDG_CACHE_HOME="$run/cache" XDG_DATA_HOME="$run/data" XDG_CONFIG_HOME="$run/config" \
    XDG_SESSION_TYPE=wayland GIO_USE_VFS=local GSETTINGS_BACKEND=memory BUDGETS_RUN="$run" \
    dbus-run-session -- bash "$self" --session "${sargs[@]}" --out "$out" $([ "$selftest" = 0 ] || echo --self-test)
  rc=$?
  if [ "$rc" = 0 ] && [ "$selftest" = 1 ]; then
    # every line reads back as a plist with :target, :kind and :verdict, none is nil, and the freeze row is measured
    HOME="$run/home" "$pa/bin/nidhogg" -Q --batch --eval '(with-temp-buffer
      (insert-file-contents (car command-line-args-left)) (goto-char (point-min))
      (let ((n 0) (blink nil) (seen nil))
        (while (progn (skip-chars-forward " \n\t") (not (eobp)))
          (let ((r (read (current-buffer))))
            (unless (and (consp r) (eq (car r) :row) (symbolp (cadr r)) (cadr r)
                         (plist-member r :target) (memq (plist-get r :kind) (quote (gate budget report)))
                         (memq (plist-get r :verdict) (quote (pass fail report skip)))
                         (or (plist-get r :skipped) (plist-get r :a)))
              (error "bad row: %S" r))
            (when (and (eq (cadr r) (quote freeze-watchdog)) (plist-get r :skipped))
              (error "freeze-watchdog skipped: %S" r))
            (when (eq (cadr r) (quote idle-wakeups-blink)) (setq blink t))
            (push (cadr r) seen)
            (setq n (1+ n))))
        (unless blink (error "no idle-wakeups-blink row"))
        (dolist (want (quote (typing-stall-max typing-stall-p99 latency-typing scroll-120-dropped
                              scroll-120-presenter-late main-wait-max cpu-scroll latency-typing-inline
                              freeze-watchdog pss-compositor)))
          (unless (memq want seen) (error "no %s row" want)))
        (when (< n 10) (error "only %d rows" n))
        (princ (format "budgets self-test: %d rows read back\n" n))))' "$out" || rc=1
  fi
  rm -r "$run"
  exit $rc
fi

# ---------------------------------------------------------------- the session: measure, aggregate
run=$BUDGETS_RUN
raw=$run/raw; : > "$raw"
clk=$(getconf CLK_TCK)
energy_f=$(ls /sys/class/powercap/intel-rapl:0/energy_uj /sys/class/powercap/intel-rapl/intel-rapl:0/energy_uj 2>/dev/null | head -n 1)
[ -n "$energy_f" ] && cat "$energy_f" > /dev/null 2>&1 || energy_f=
mpid= epid= xdg= prefix=
SKIP_TYPING= SKIP_ENERGY=

# ROW SIDE VALUE...: one measurement; VALUE is a number or "SKIP reason"
rec() { local v=$3; [ -n "${v// /}" ] || v="SKIP the measurement came back empty"; printf "%s|%s|%s\n" "$1" "$2" "$v" >> "$raw"; }

stop_all() {
  for p in $epid $mpid; do kill "$p" 2>/dev/null; done
  for p in $epid $mpid; do wait "$p" 2>/dev/null; done
  epid= mpid=; [ -z "$xdg" ] || rm -r "$xdg" 2>/dev/null
  xdg=
}
trap stop_all EXIT
trap 'exit 1' INT TERM

# start_sway WxH SCALE HZ: a headless sway on a fresh runtime dir; sets xdg, mpid, WAYLAND_DISPLAY, SWAYSOCK
start_sway() {
  local sz=$1 sc=$2 hz=$3 sock i
  xdg=$(mktemp -d /tmp/bs.XXXXXX)
  {
    echo "output HEADLESS-1 mode ${sz}${hz:+@${hz}Hz} scale $sc position 0 0"
    echo "xwayland disable"
    echo "default_border none"
    echo "default_floating_border none"
    echo 'for_window [app_id=".*"] floating enable'
  } > "$run/sway.conf"
  XDG_RUNTIME_DIR=$xdg WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
    SWAYSOCK="$xdg/sway-ipc.sock" sway -c "$run/sway.conf" > "$run/sway.log" 2>&1 &
  mpid=$!
  for i in $(seq 150); do
    sock=$(compgen -G "$xdg/wayland-*" | grep -v '\.lock$' | head -n 1)
    [ -n "$sock" ] && [ -S "$sock" ] && break
    kill -0 "$mpid" 2>/dev/null || { echo "budgets.sh: sway exited early:"; cat "$run/sway.log"; return 1; }
    sleep 0.1
  done
  [ -S "${sock:-}" ] || { echo "budgets.sh: sway has no display"; return 1; }
  export XDG_RUNTIME_DIR=$xdg WAYLAND_DISPLAY=${sock##*/} SWAYSOCK=$xdg/sway-ipc.sock
  export GDK_BACKEND=wayland NO_AT_BRIDGE=1
  unset DISPLAY EMACS_SOCKET_NAME
}

cl() { timeout 600 "$prefix/bin/nidhoggclient" -s "$xdg/srv" -e "$1" 2> /dev/null | tr "\n" " "; }

# start_nidhogg PREFIX: the frame, fullscreen, with the 10k-line buffer and a server (no timers: the idle rows count)
start_nidhogg() {
  prefix=$1
  env ${nh_env[@]+"${nh_env[@]}"} BUDGETS_SRV=$xdg/srv BUDGETS_FILE=$run/big.txt \
    timeout --kill-after=10 3000 "$prefix/bin/nidhogg" "${nh_init[@]}" --fullscreen \
    -l "$run/driver.el" 2> "$run/nh.err" &
  epid=$!
  for _ in $(seq 300); do
    [ -S "$xdg/srv" ] && break
    kill -0 "$epid" 2>/dev/null || { echo "budgets.sh: nidhogg exited early:"; tail -n 20 "$run/nh.err"; return 1; }
    sleep 0.1
  done
  [ -S "$xdg/srv" ] || { echo "budgets.sh: nidhogg has no server socket"; tail -n 20 "$run/nh.err"; return 1; }
  nhpid=$(cl '(emacs-pid)' | tr -d ' ')
  [ -n "$nhpid" ] && [ -d "/proc/$nhpid" ] || { echo "budgets.sh: no nidhogg pid"; return 1; }
  sleep 2
}

# snap: ticks of nidhogg and sway, energy_uj (-1 unreadable), wall seconds, context switches (all threads; all but the main)
snap() {
  local t1 t2 e w s1 s2
  t1=$(awk '{print $14 + $15}' "/proc/$nhpid/stat"); t2=$(awk '{print $14 + $15}' "/proc/$mpid/stat")
  e=-1; [ -z "$energy_f" ] || e=$(cat "$energy_f" 2> /dev/null || echo -1)
  w=$(date +%s.%N)
  s1=$(cat /proc/"$nhpid"/task/*/status | awk '/ctxt_switches/ {s += $2} END {print s + 0}')
  s2=$(for t in /proc/"$nhpid"/task/*; do [ "${t##*/}" = "$nhpid" ] || cat "$t/status"; done | awk '/ctxt_switches/ {s += $2} END {print s + 0}')
  echo "$t1 $t2 $e $w $s1 $s2"
}

pss() { # nidhogg + sway, kB
  awk '/^Pss:/ {s += $2} END {print s + 0}' "/proc/$nhpid/smaps_rollup" "/proc/$mpid/smaps_rollup"
}

# win SIDE NAME S1 S2: whole-system CPU (percent of one core) and package power (W) over one window
win() {
  awk -v ck="$clk" -v side="$1" -v name="$2" -v ef="$energy_f" -v s1="$3" -v s2="$4" 'BEGIN {
      split(s1, a, " "); split(s2, b, " "); dw = b[4] - a[4]
      printf "cpu-%s|%s|%.4f\n", name, side, ((b[1] + b[2]) - (a[1] + a[2])) / ck / dw * 100
      if (ef == "" || a[3] < 0 || b[3] < a[3]) printf "power-%s|%s|SKIP energy_uj unreadable here (RAPL needs root on the host)\n", name, side
      else printf "power-%s|%s|%.4f\n", name, side, (b[3] - a[3]) / 1e6 / dw
    }' >> "$raw"
}

# rate SIDE NAME FIELD S1 S2: the per-second change of one snap field
rate() {
  rec "$2" "$1" "$(awk -v f="$3" -v s1="$4" -v s2="$5" 'BEGIN { split(s1, a, " "); split(s2, b, " "); printf "%.4f", (b[f] - a[f]) / (b[4] - a[4]) }')"
}

# one session of one side: idle, typing, scroll, resize
measure() {
  local side=$1 pfx=$2 s1 s2 n str got stuck last cur i w h
  start_sway "$size" "$scale" "" && start_nidhogg "$pfx" || return 1
  # idle: nothing running but the compositor and Nidhogg; the server socket is the only fd Emacs waits on besides input
  s1=$(snap); sleep "$window"; s2=$(snap)
  rate "$side" idle-wakeups 5 "$s1" "$s2"
  rate "$side" idle-wakeups-threads 6 "$s1" "$s2"
  win "$side" idle "$s1" "$s2"
  rec pss-idle "$side" "$(pss)"
  rec pss-compositor "$side" "$(awk '/^Pss:/ {s += $2} END {print s + 0}' "/proc/$mpid/smaps_rollup")"

  # idle with the timers on: the cursor blinks (0.1 s), so redisplays run; one that draws nothing must cost nothing
  cl '(progn (setq blink-cursor-interval 0.1 blink-cursor-delay 0) (blink-cursor-mode 1) t)' > /dev/null
  sleep 1
  s1=$(snap); sleep "$window"; s2=$(snap)
  rate "$side" idle-wakeups-blink 5 "$s1" "$s2"
  cl '(progn (blink-cursor-mode 0) t)' > /dev/null

  # typing: 10 chars/s through zwp_virtual_keyboard
  n=$((window * 10))
  str=$(awk -v n="$n" 'BEGIN { for (i = 0; i < n; i++) printf "%c", 97 + i % 26 }')
  # emacsclient -e runs in the server buffer, not the one the keys go to: read the selected window's buffer
  cl '(with-current-buffer (window-buffer (selected-window)) (goto-char (point-min)) (setq budgets--n (buffer-size)) (glitnir-stats nil t) t)' > /dev/null
  s1=$(snap)
  wtype -d 100 -- "$str" 2> "$run/wtype.err"; local wrc=$?
  s2=$(snap)
  got=$(cl '(with-current-buffer (window-buffer (selected-window)) (- (buffer-size) budgets--n))')
  if [ "$wrc" != 0 ] || [ "${got:-0}" -lt $((n / 2)) ] 2> /dev/null; then
    SKIP_TYPING="wtype did not reach the frame (exit $wrc, $got of $n chars typed): $(head -c 120 "$run/wtype.err" | tr '"\n' "' ")"
    for rr in cpu-typing power-typing latency-typing-stats pixels-per-char; do rec "$rr" "$side" "SKIP $SKIP_TYPING"; done
  else
    win "$side" typing "$s1" "$s2"
    st=$(cl '(let ((s (glitnir-stats))) (if s (format "%s %s" (plist-get (or (plist-get s :key-to-commit-ms) (plist-get s :latency-ms)) :p95) (plist-get s :damage-px)) "none"))' | tr -d '"')
    case $st in
      none|"") rec latency-typing-stats "$side" "SKIP no glitnir-stats record on this frame (the record is pgtk-only at this commit)"
               rec pixels-per-char "$side" "SKIP no glitnir-stats record on this frame (the record is pgtk-only at this commit)" ;;
      *) rec latency-typing-stats "$side" "${st% *}"; rec pixels-per-char "$side" "${st#* }" ;;
    esac
  fi

  # scroll (C4, fixed work): the same $fixed scroll steps over the 10k-line file on every side; the cost is the CPU
  # seconds of Nidhogg and the compositor together, and the energy
  s1=$(snap)
  cl "(budgets--scroll-fixed $fixed)" > /dev/null
  s2=$(snap)
  rec cpu-scroll "$side" "$(awk -v ck="$clk" -v s1="$s1" -v s2="$s2" 'BEGIN { split(s1, a, " "); split(s2, b, " "); printf "%.4f", ((b[1] + b[2]) - (a[1] + a[2])) / ck }')"
  case $s1$s2 in
    *" -1 "*) rec power-scroll "$side" "SKIP energy_uj unreadable here (RAPL needs root on the host)" ;;
    *) rec power-scroll "$side" "$(awk -v s1="$s1" -v s2="$s2" 'BEGIN { split(s1, a, " "); split(s2, b, " "); printf "%.4f", (b[3] - a[3]) / 1e6 }')" ;;
  esac
  rec pss-scroll "$side" "$(pss)"

  # resize: each step must change the frame's size (a step that leaves it alone is a stale frame)
  if [ "$steps" -gt 0 ]; then
    cl "(set-frame-parameter nil 'fullscreen nil)" > /dev/null; sleep 1
    last=$(cl '(format "%d %d" (frame-pixel-width) (frame-pixel-height))' | tr -d '"')
    stuck=0
    for ((i = 0; i < steps; i++)); do
      w=$((500 + (i % 2) * 160 + (i / 2 % 5) * 12)); h=$((400 + (i % 2) * 100))
      swaymsg "resize set width $w px height $h px" > /dev/null 2>&1
      for _ in $(seq 20); do
        cur=$(cl '(format "%d %d" (frame-pixel-width) (frame-pixel-height))' | tr -d '"')
        [ "$cur" != "$last" ] && break
        sleep 0.05
      done
      [ "$cur" != "$last" ] || stuck=$((stuck + 1))
      last=$cur
    done
    rec resize-stuck "$side" "$stuck"
  fi
  stop_all
}

# the motion bench: shell only
motion() {
  local s1 s2 r
  start_sway "$msize" 1.5 120 && start_nidhogg "$pa" || return 1
  s1=$(snap)
  r=$(cl "(nhwl--bench-full-redraw $mframes)")
  s2=$(snap)
  case $r in
    "(:frames "*)
      rec motion-compose-p95 A "$(echo "$r" | awk '{for (i = 1; i <= NF; i++) if ($i == ":compose-p95") {gsub(/[()]/, "", $(i + 1)); print $(i + 1)}}')"
      rec motion-interval-p95 A "$(echo "$r" | awk '{for (i = 1; i <= NF; i++) if ($i == ":present-interval-p95") {gsub(/[()]/, "", $(i + 1)); print $(i + 1)}}')"
      rec motion-cores A "$(awk -v ck="$clk" -v s1="$s1" -v s2="$s2" 'BEGIN { split(s1, a, " "); split(s2, b, " "); printf "%.4f", ((b[1] + b[2]) - (a[1] + a[2])) / ck / (b[4] - a[4]) }')" ;;
    *) for x in motion-compose-p95 motion-interval-p95 motion-cores; do rec "$x" A "SKIP the bench did not run: $(echo "$r" | head -c 100 | tr '"\n' "' ")"; done ;;
  esac
  stop_all
}

# do_typing: wtype window*10 characters (10 per second) into the frame; sets ts1 ts2 (snaps around it), twrc, tgot
do_typing() {
  local n=$((window * 10)) str
  str=$(awk -v n="$n" 'BEGIN { for (i = 0; i < n; i++) printf "%c", 97 + i % 26 }')
  cl '(with-current-buffer (window-buffer (selected-window)) (goto-char (point-min)) (setq budgets--n (buffer-size)) t)' > /dev/null
  ts1=$(snap)
  wtype -d 100 -- "$str" 2> "$run/wtype.err"; twrc=$?
  ts2=$(snap)
  tgot=$(cl '(with-current-buffer (window-buffer (selected-window)) (- (buffer-size) budgets--n))')
  [ "$twrc" = 0 ] && [ "${tgot:-0}" -ge $((n / 2)) ] 2> /dev/null
}

# cputype SIDE PFX: typing alone, no trace (the trace costs CPU); cpu-typing-inline, power-typing-inline.  SIDE is A or C.
cputype() {
  local side=$1
  nh_env=(); [ "$side" != C ] || nh_env=(NHWL_PRESENTER=0)
  start_sway "$size" "$scale" "" && start_nidhogg "$2" || { nh_env=(); return 1; }
  nh_env=()
  if do_typing; then win "$side" typing-inline "$ts1" "$ts2"
  else for rr in cpu-typing-inline power-typing-inline; do rec "$rr" "$side" "SKIP wtype did not reach the frame (exit $twrc)"; done; fi
  stop_all
}

# plval KEY: the number after KEY in a printed plist on stdin
plval() { awk -v k="$1" '{ for (i = 1; i < NF; i++) if ($i == k) { v = $(i + 1); gsub(/[()]/, "", v); print v } }'; }

# rep ARGS...: wl-latency.el over this session's trace
rep() { "$prefix/bin/nidhogg" -Q --batch -l "$checkout/nidhogg/test/wl-latency.el" -f wl-latency-batch "$run/nh.err" "$@" 2> /dev/null; }

# speed SIDE PFX: one session with WAYLAND_DEBUG=client: typing, then 120 Hz scrolling of a fontified src/xdisp.c.  Side
# C is the shell with the presenter thread off; it records only the -inline rows, side A their A halves.
speed() {
  local side=$1 pfx=$2 o1 o2 r period sfx="" v mw pl wd st1=0 st2=0
  period=$(awk -v hz="$refresh" 'BEGIN { printf "%.4f", 1000 / hz }')
  nh_env=(WAYLAND_DEBUG=client); [ "$side" != C ] || nh_env+=(NHWL_PRESENTER=0)
  start_sway "$size" "$scale" "$refresh" && start_nidhogg "$pfx" || { nh_env=(); return 1; }
  nh_env=()
  [ "$side" != C ] || sfx=-inline
  # the stall watchdog (V2's freeze bar): threshold 100 ms, its log in the run directory; the counters start at zero
  wd=$(cl "(if (fboundp 'nidhogg-watchdog-mode)
             (progn (setq nidhogg-watchdog-log-directory \"$run/wd/\" nidhogg-watchdog-threshold 100)
                    (nidhogg-watchdog-mode 1) (nidhogg-watchdog-stats t) (format \"%s\" (and nidhogg-watchdog-mode t)))
           \"none\")" | tr -d '" ')
  # typing: key to commit, from the trace, the first 5 keys dropped
  o1=$(stat -c %s "$run/nh.err")
  if do_typing; then
    o2=$(stat -c %s "$run/nh.err")
    r=$(rep typing "$o1" "$o2" 5)
    case $r in
      "(:n "*)
        if [ "$side" = C ]; then rec latency-typing-inline C "$(plval :p95 <<< "$r")"
        else
          rec typing-stall-max "$side" "$(plval :max <<< "$r")"
          rec typing-stall-p99 "$side" "$(plval :p99 <<< "$r")"
          rec latency-typing "$side" "$(plval :p95 <<< "$r")"
          [ "$HAS_W3$side" != 1A ] || rec latency-typing-inline A "$(plval :p95 <<< "$r")"
        fi ;;
      *) for rr in typing-stall-max typing-stall-p99 latency-typing; do rec "$rr" "$side" "SKIP no key-to-commit pair in the trace"; done ;;
    esac
  else
    for rr in typing-stall-max typing-stall-p99 latency-typing; do rec "$rr" "$side" "SKIP wtype did not reach the frame (exit $twrc)"; done
  fi
  [ "$wd" != t ] || st1=$(cl '(format "%s" (plist-get (nidhogg-watchdog-stats t) :stalls))' | tr -d '" ')
  # scroll: 120 steps a second for sscroll seconds, intervals between damaged commits from the trace
  cl "(budgets--show \"$checkout/src/xdisp.c\")" > /dev/null
  sleep 1
  [ "$wd" != t ] || cl '(progn (nidhogg-watchdog-stats t) t)' > /dev/null   # fontifying the file is setup, not scrolling
  [ "$HAS_W3$side" != 1A ] || cl '(progn (glitnir-stats nil t) t)' > /dev/null
  # the presenter's own late and frame counts (since open) before the scroll
  pt='(let ((s (nhwl-presenter-stats))) (format "%s %s" (plist-get s :presenter-late) (plist-get s :presenter-frames)))'
  [ "$HAS_W3$side" != 1A ] || pt1=$(cl "$pt" | tr -d '"')
  o1=$(stat -c %s "$run/nh.err")
  cl "(budgets--scroll-paced $sscroll $refresh)" > /dev/null
  o2=$(stat -c %s "$run/nh.err")
  if [ "$wd" = t ]; then
    st2=$(cl '(format "%s" (plist-get (nidhogg-watchdog-stats t) :stalls))' | tr -d '" ')
    case $st1$st2 in
      ''|*[!0-9]*) rec freeze-watchdog "$side" "SKIP the watchdog counters did not read back ($st1 $st2)" ;;
      *) rec freeze-watchdog "$side" "$((st1 + st2))" ;;
    esac
  else rec freeze-watchdog "$side" "SKIP this build has no nidhogg-watchdog-mode ($wd)"; fi
  r=$(rep scroll "$o1" "$o2" 0 "$period")
  case $r in
    "(:frames "*)
      v=$(plval :per1000 <<< "$r")
      rec "scroll-120-dropped$sfx" "$side" "$v"
      [ "$HAS_W3$side" != 1A ] || rec scroll-120-dropped-inline A "$v" ;;
    *) rec "scroll-120-dropped$sfx" "$side" "SKIP fewer than two damaged commits in the trace" ;;
  esac
  # the presenter's own numbers (the shell with the thread only).  main-wait-max is the ns the host thread waited at
  # most on the tree mutex or the mailbox, over the whole session.  presenter-late: the presenter counts its own misses
  # (`:presenter-late' of `:presenter-frames': a root buffer committed more than one refresh period after its tree
  # could commit it, the later of its first publish and the frame callback), per 1000 of its frames during the scroll.
  # A build without those keys falls back to the proxy: the dropped-interval count when the presenter's
  # publish-to-commit time ever reached 2 ms, else 0.
  if [ "$HAS_W3$side" = 1A ]; then
    mw=$(cl '(format "%s" (plist-get (nhwl-presenter-stats) :main-wait-max-ns))' | tr -d '" ')
    case $mw in
      ''|*[!0-9]*) rec main-wait-max A "SKIP nhwl-presenter-stats has no :main-wait-max-ns ($mw)" ;;
      *) rec main-wait-max A "$(awk -v n="$mw" 'BEGIN { printf "%.4f", n / 1e6 }')" ;;
    esac
    pt2=$(cl "$pt" | tr -d '"')
    if [[ "$pt1 $pt2" =~ ^[0-9]+\ [0-9]+\ [0-9]+\ [0-9]+$ ]]; then
      rec scroll-120-presenter-late A "$(awk -v a="$pt1" -v b="$pt2" 'BEGIN { split(a, x, " "); split(b, y, " ");
        f = y[2] - x[2]; printf "%.2f", f > 0 ? (y[1] - x[1]) * 1000 / f : 0 }')"
    else
      pl=$(cl '(format "%s" (plist-get (plist-get (glitnir-stats) :publish-to-commit-ms) :max))' | tr -d '" ')
      case $pl in
        ''|*[!0-9.]*) rec scroll-120-presenter-late A "SKIP glitnir-stats has no :publish-to-commit-ms ($pl)" ;;
        *) rec scroll-120-presenter-late A "$(awk -v p="$pl" -v late="$(plval :late <<< "$r")" 'BEGIN { print (p >= 2 ? late + 0 : 0) }')" ;;
      esac
    fi
  fi
  stop_all
}

# ---- inputs: the driver, the 10k-line file
cat > "$run/driver.el" <<'EOF'
;;; budgets driver: no timers, so idle really is idle.
(blink-cursor-mode 0)
(setq inhibit-startup-screen t server-name (getenv "BUDGETS_SRV"))
(find-file (getenv "BUDGETS_FILE"))
(setq-default cursor-in-non-selected-windows nil)
(require 'server)
(server-start)

;; the workloads of the speed rows; emacsclient -e runs in the server buffer, so they act on the selected window's
(defun budgets--scroll-fixed (n)
  "N scroll steps (5 lines each) with a redisplay after each, as fast as the build goes: the C4 fixed work."
  (with-current-buffer (window-buffer (selected-window))
    (goto-char (point-min))
    (dotimes (_ n)
      (condition-case nil (scroll-up 5) (end-of-buffer (goto-char (point-min))))
      (redisplay t)))
  t)
(defun budgets--show (file)
  "Show FILE fontified in the selected window."
  (set-window-buffer (selected-window) (find-file-noselect file))
  (with-current-buffer (window-buffer (selected-window))
    (font-lock-mode 1) (font-lock-ensure) (goto-char (point-min)))
  (redisplay t)
  t)
(defun budgets--scroll-paced (secs hz)
  "One line a step, HZ steps a second for SECS seconds, each with a forced redisplay."
  (with-current-buffer (window-buffer (selected-window))
    (goto-char (point-min))
    (let ((per (/ 1.0 hz)) (end (+ (float-time) secs)) (next (float-time)) d)
      (while (< (float-time) end)
        (condition-case nil (scroll-up 1) (end-of-buffer (goto-char (point-min))))
        (redisplay t)
        (setq next (+ next per) d (- next (float-time)))
        (if (> d 0) (sleep-for d) (setq next (float-time))))))
  t)
EOF
awk 'BEGIN { for (i = 1; i <= 10000; i++) printf "line %05d the quick brown fox jumps over the lazy dog, again and again\n", i }' > "$run/big.txt"

# the presenter thread (and so side C, and the rows that read its numbers) exists only on a build that has its stats
[ "$("$pa/bin/nidhogg" -Q --batch --eval '(princ (if (fboundp (quote nhwl-presenter-stats)) 1 0))' 2> /dev/null)" != 1 ] || HAS_W3=1
if [ "$HAS_W3" = 0 ] && [ "$reference" = 0 ]; then
  for rr in latency-typing-inline scroll-120-dropped-inline cpu-typing-inline scroll-120-presenter-late main-wait-max; do
    rec "$rr" A "SKIP needs the presenter thread : this build has no nhwl-presenter-stats"
  done
fi

for ((ri = 1; ri <= runs; ri++)); do
  measure A "$pa" || { echo "budgets.sh: the A session failed"; exit 1; }
  [ "$reference" = 0 ] || continue   # the pgtk reference run: the idle, typing, scroll and resize sessions only
  speed A "$pa" || { echo "budgets.sh: the A speed session failed"; exit 1; }
  if [ "$HAS_W3" = 1 ]; then
    # alternate which of A and C goes first, so a drifting machine does not favour one
    if [ $((ri % 2)) = 1 ]; then order="A C"; else order="C A"; fi
    for side in $order; do
      cputype "$side" "$pa" || { echo "budgets.sh: the $side typing session failed"; exit 1; }
    done
    speed C "$pa" || { echo "budgets.sh: the C speed session failed"; exit 1; }
  fi
  motion || { echo "budgets.sh: the motion session failed"; exit 1; }
done

# ---- aggregate: median and SD per row and side, then the verdict against the row's target
cellid=${size}@${scale}; [ "$config" = q ] || cellid=$config:$cellid   # the cell a target names is a -Q cell
TBL=$TARGETS awk -F'|' -v cell="$cellid" -v ref="$reference" '
  function med(row, side,   n, i, j, t, v) {
    n = cnt[row, side]
    for (i = 1; i <= n; i++) v[i] = val[row, side, i]
    for (i = 2; i <= n; i++) { t = v[i]; for (j = i - 1; j >= 1 && v[j] > t; j--) v[j + 1] = v[j]; v[j + 1] = t }
    return n % 2 ? v[(n + 1) / 2] : (v[n / 2] + v[n / 2 + 1]) / 2
  }
  function sd(row, side,   n, i, m, s) {
    n = cnt[row, side]; if (n < 2) return 0
    m = 0; for (i = 1; i <= n; i++) m += val[row, side, i]; m /= n
    for (i = 1; i <= n; i++) s += (val[row, side, i] - m) ^ 2
    return sqrt(s / (n - 1))
  }
  function fmt(x) { return sprintf("%.6g", x) }
  function pair(row, side) { return "(" fmt(med(row, side)) " " fmt(sd(row, side)) ")" }
  function quote(s) { gsub(/"/, "\047", s); return "\"" s "\"" }
  # TARGETS: ROW|TARGET|KIND|SOURCE[|CELL]
  function load(   n, i, f, L) {
    n = split(ENVIRON["TBL"], L, "\n")
    for (i = 1; i <= n; i++) {
      if (L[i] == "") continue
      split(L[i], f, "|")
      tg[f[1]] = f[2]; kd[f[1]] = f[3]; sc[f[1]] = f[4]; tc[f[1]] = f[5]
    }
  }
  {
    row = $1; side = $2; v = $3
    if (!(row in seen)) { seen[row] = 1; order[++nrows] = row }
    if (v ~ /^SKIP /) { if (!((row, side) in why)) why[row, side] = substr(v, 6); skip[row] = 1; next }
    cnt[row, side]++; val[row, side, cnt[row, side]] = v + 0
  }
  END {
    load()
    unit["idle-wakeups"] = "switches/s"; unit["idle-wakeups-blink"] = "switches/s (cursor blinking, 0.1 s)"; unit["idle-wakeups-threads"] = "switches/s (all but the main thread)"
    unit["pixels-per-char"] = "device px"; unit["resize-stuck"] = "steps"
    unit["latency-typing"] = "ms p95 (WAYLAND_DEBUG trace)"; unit["latency-typing-inline"] = "ms p95 (A thread, C inline)"
    unit["latency-typing-stats"] = "ms p95 (glitnir-stats)"
    unit["typing-stall-max"] = "ms (key to commit)"; unit["typing-stall-p99"] = "ms (key to commit)"
    unit["scroll-120-dropped"] = "late frames per 1000"; unit["scroll-120-dropped-inline"] = "late frames per 1000 (A thread, C inline)"
    unit["scroll-120-presenter-late"] = "late frames per 1000, presenter at fault"; unit["main-wait-max"] = "ms"
    unit["cpu-scroll"] = "CPU s, fixed work"; unit["power-scroll"] = "J, fixed work"; unit["freeze-watchdog"] = "stalls over 100 ms"
    unit["cpu-typing-inline"] = "% of one core (A thread, C inline)"
    unit["motion-compose-p95"] = "ms"; unit["motion-interval-p95"] = "ms"; unit["motion-cores"] = "cores"
    for (k = 1; k <= nrows; k++) {
      row = order[k]; u = unit[row]
      if (u == "") u = (row ~ /^cpu-/) ? "% of one core" : (row ~ /^power-/) ? "W" : (row ~ /^pss-/) ? "kB" : "-"
      # the target: a row judged at one cell only is report elsewhere; a budget with no value yet is report; -inline rows
      # compare the shell with itself (A against C); the reference run judges nothing
      kind = (row in kd) ? kd[row] : "report"; target = tg[row]; source = sc[row]
      if (tc[row] != "" && tc[row] != cell) { kind = "report"; target = ""; source = source " (judged at " tc[row] " only)" }
      if (kind == "budget" && target == "") kind = "report"
      if (row ~ /-inline$/) { kind = cnt[row, "C"] ? "budget" : "report"; source = "A <= C: the shell against itself"; target = cnt[row, "C"] ? med(row, "C") : "" }
      if (ref) { kind = "report"; target = "" }
      if (kind == "report") target = ""
      line = "(:row " row " :unit " quote(u)
      if (skip[row]) {
        reason = ""
        if ((row, "A") in why) reason = "A: " why[row, "A"]
        if ((row, "C") in why) reason = reason (reason == "" ? "" : "; ") "C: " why[row, "C"]
        line = line " :skipped " quote(reason)
        if (cnt[row, "A"]) line = line " :a " pair(row, "A")
        if (cnt[row, "C"]) line = line " :c " pair(row, "C")
        print line " :target " (target == "" ? "nil" : target) " :kind " kind " :source " quote(source) " :verdict skip)"; continue
      }
      if (row ~ /^(typing-stall|latency-typing|scroll-120|main-wait|cpu-typing-inline)/) line = line " :cell " quote(cell)
      a = med(row, "A")
      line = line " :a " pair(row, "A")
      if (cnt[row, "C"]) line = line " :c " pair(row, "C")
      if (row == "scroll-120-dropped") {
        # the late frames per 1000 split: the presenter-late row is the proxy for the presenter, the rest is the main thread
        pl = cnt["scroll-120-presenter-late", "A"] ? med("scroll-120-presenter-late", "A") : -1
        if (pl >= 0) line = line " :presenter-late " fmt(pl) " :main-late " fmt(a > pl ? a - pl : 0)
      }
      if (kind == "report") verdict = "report"
      else if (row ~ /-inline$/) {
        b = med(row, "C"); tol = 0.05 * b; t2 = 2 * (sd(row, "A") > sd(row, "C") ? sd(row, "A") : sd(row, "C")); if (t2 > tol) tol = t2
        verdict = (a - b <= tol) ? "pass" : "fail"
      } else verdict = (a <= target) ? "pass" : "fail"
      print line " :target " (target == "" ? "nil" : target) " :kind " kind " :source " quote(source) " :verdict " verdict ")"
    }
  }' "$raw" > "$out"
cat "$out"
exit 0
