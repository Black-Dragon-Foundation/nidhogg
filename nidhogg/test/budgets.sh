#!/usr/bin/env bash
# The shell's budgets: Nidhogg's own Wayland shell (A) against pgtk (B), built from the same
# commit, each in its own headless sway inside a memory-capped container, one plist per row.
#
#   nidhogg/test/budgets.sh --prefix-a SHELL --prefix-b PGTK [--compositor sway] [--scale S] [--size WxH] [--runs N]
#                           [--out FILE.eld] [--window SEC] [--resize-steps N] [--motion-frames N] [--motion-size WxH]
#   nidhogg/test/budgets.sh --self-test --prefix-a SHELL --prefix-b PGTK [--out FILE.eld]
#
#   --prefix-a / -b   unpacked Nidhogg builds: A with --nidhogg-wayland, B (pgtk) without
#   --scale, --size   the output of the row sessions (defaults 1, 1920x1080)
#   --runs N          sessions per side; a row's value is the median over them (default 5)
#   --window SEC      the idle, typing and scroll windows (default 60)
#   --resize-steps N  `swaymsg resize' steps (default 200)
#   --motion-frames N frames of the motion bench (default 600), on --motion-size (default 3840x2160) at scale 1.5 and
#                     120 Hz; shell only (pgtk has no bench primitive)
#   --out FILE        the plists, one per line (default budgets.eld in the current directory)
#   --self-test       --runs 1, 1280x720, 3 s windows, 10 resize steps, 30 motion frames; then every line must read
#                     back as a plist and none may be nil
#   BUDGETS_MEMORY    the container's memory cap (default 6g); BUDGETS_CPUSET pins it (podman --cpuset-cpus)
#
# idle-wakeups-blink repeats the idle window with the cursor blinking, so redisplays run.
# A row is (:row NAME :unit U :a (MEDIAN SD) :b (MEDIAN SD) :verdict pass|fail|equal), lower is better: pass when A is
# not above B, equal when within the larger of 5 % or two SD of B (spec section 3), else fail.  A row that cannot be
# measured here is (:row NAME :skipped "REASON" ...): never nil, never a made-up number.  The motion rows have no B and
# their verdict is the absolute budget (compose p95 <= 8.33 ms; whole-system CPU under one core).
# Exit 0 when the rows were produced (verdicts are the caller's to read), 1 when a session did not come up, 2 usage.
set -u

self=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")
checkout=$(cd "$(dirname "$self")/../.." && pwd)
# the container image, pinned by digest, is gui.sh's
GUI_SH_SOURCE_ONLY=1; . "$(dirname "$self")/gui.sh"; GUI_SH_SOURCE_ONLY=
self=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")   # gui.sh set its own

pa= pb= scale=1 size=1920x1080 runs=5 out=budgets.eld window=60 steps=200 mframes=600 msize=3840x2160 selftest=0 mode=host
while [ $# -gt 0 ]; do
  case $1 in
    --prefix-a) pa=${2:-}; shift 2 ;;
    --prefix-b) pb=${2:-}; shift 2 ;;
    --compositor) [ "${2:-}" = sway ] || { echo "budgets.sh: only --compositor sway" >&2; exit 2; }; shift 2 ;;
    --scale) scale=${2:-}; shift 2 ;;
    --size) size=${2:-}; shift 2 ;;
    --runs) runs=${2:-}; shift 2 ;;
    --out) out=${2:-}; shift 2 ;;
    --window) window=${2:-}; shift 2 ;;
    --resize-steps) steps=${2:-}; shift 2 ;;
    --motion-frames) mframes=${2:-}; shift 2 ;;
    --motion-size) msize=${2:-}; shift 2 ;;
    --self-test) selftest=1; shift ;;
    --in-container) mode=container; shift ;;
    --session) mode=session; shift ;;
    -h|--help) sed -n '2,/^# Exit/s/^# \{0,1\}//p' "$self" >&2; exit 2 ;;
    *) echo "budgets.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
if [ "$selftest" = 1 ]; then runs=1 size=1280x720 window=3 steps=10 mframes=30 msize=1280x720; fi
[ -x "$pa/bin/nidhogg" ] && [ -x "$pb/bin/nidhogg" ] || { echo "budgets.sh: --prefix-a and --prefix-b must be unpacked Nidhoggs" >&2; exit 2; }
case $size$msize in *x*x*) ;; *) echo "budgets.sh: sizes are WxH" >&2; exit 2 ;; esac
sargs=(--prefix-a "$pa" --prefix-b "$pb" --scale "$scale" --size "$size" --runs "$runs" --window "$window"
       --resize-steps "$steps" --motion-frames "$mframes" --motion-size "$msize")

# ---------------------------------------------------------------- on the host: the container
if [ "$mode" = host ]; then
  command -v podman > /dev/null || { echo "budgets.sh: podman not found" >&2; exit 1; }
  outdir=$(cd "$(dirname "$out")" && pwd) && outfile=$(basename "$out")
  # a tarball runs at the prefix it was built for (BUILDINFO), so each is mounted there
  inpa=$(sed -n 's/^prefix=//p' "$pa/share/nidhogg/BUILDINFO" 2>/dev/null); [ -n "$inpa" ] || inpa=$(cd "$pa" && pwd)
  inpb=$(sed -n 's/^prefix=//p' "$pb/share/nidhogg/BUILDINFO" 2>/dev/null); [ -n "$inpb" ] || inpb=$(cd "$pb" && pwd)
  pa=$(cd "$pa" && pwd) pb=$(cd "$pb" && pwd)
  sargs=(--prefix-a "$inpa" --prefix-b "$inpb" --scale "$scale" --size "$size" --runs "$runs" --window "$window"
         --resize-steps "$steps" --motion-frames "$mframes" --motion-size "$msize" --out "/out/$outfile")
  [ "$selftest" = 0 ] || sargs+=(--self-test)
  name=budgets-$$
  trap 'podman rm -f "$name" > /dev/null 2>&1' EXIT
  trap 'exit 1' INT TERM
  # the same container recipe as gui.sh, plus wtype (the typing) and procps
  podman run --rm --name "$name" --memory "${BUDGETS_MEMORY:-6g}" --memory-swap "${BUDGETS_MEMORY:-6g}" \
    ${BUDGETS_CPUSET:+--cpuset-cpus "$BUDGETS_CPUSET"} \
    -v "$checkout:/src:ro,z" -v "$pa:$inpa:ro,z" -v "$pb:$inpb:ro,z" -v "$outdir:/out:Z" "$IMAGE_fedora_45" bash -c '
      set -eu
      pkgs=$(cat "'"$inpa"'/share/nidhogg/deps.runtime" "'"$inpb"'/share/nidhogg/deps.runtime" 2>/dev/null | sed "s/#.*//" || true)
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
    # every line reads back as a plist with a verdict or a skip, and none is nil
    HOME="$run/home" "$pa/bin/nidhogg" -Q --batch --eval '(with-temp-buffer
      (insert-file-contents (car command-line-args-left)) (goto-char (point-min))
      (let ((n 0) (blink nil))
        (while (progn (skip-chars-forward " \n\t") (not (eobp)))
          (let ((r (read (current-buffer))))
            (unless (and (consp r) (eq (car r) :row) (symbolp (cadr r)) (cadr r)
                         (or (plist-get r :skipped) (plist-get r :verdict)))
              (error "bad row: %S" r))
            (when (eq (cadr r) (quote idle-wakeups-blink)) (setq blink t))
            (setq n (1+ n))))
        (unless blink (error "no idle-wakeups-blink row"))
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
  BUDGETS_SRV=$xdg/srv BUDGETS_FILE=$run/big.txt timeout --kill-after=10 3000 "$prefix/bin/nidhogg" -Q --fullscreen \
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
    for rr in cpu-typing power-typing latency-typing pixels-per-char; do rec "$rr" "$side" "SKIP $SKIP_TYPING"; done
  else
    win "$side" typing "$s1" "$s2"
    st=$(cl '(let ((s (glitnir-stats))) (if s (format "%s %s" (plist-get (or (plist-get s :key-to-commit-ms) (plist-get s :latency-ms)) :p95) (plist-get s :damage-px)) "none"))' | tr -d '"')
    case $st in
      none|"") rec latency-typing "$side" "SKIP no glitnir-stats record on this frame (the record is pgtk-only at this commit)"
               rec pixels-per-char "$side" "SKIP no glitnir-stats record on this frame (the record is pgtk-only at this commit)" ;;
      *) rec latency-typing "$side" "${st% *}"; rec pixels-per-char "$side" "${st#* }" ;;
    esac
  fi

  # scroll: scroll-up over the 10k-line file for one window
  s1=$(snap)
  cl "(let ((end (+ (float-time) $window))) (goto-char (point-min)) (while (< (float-time) end) (condition-case nil (scroll-up 5) (end-of-buffer (goto-char (point-min)))) (redisplay t)) t)" > /dev/null
  s2=$(snap)
  win "$side" scroll "$s1" "$s2"
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

# ---- inputs: the driver, the 10k-line file
cat > "$run/driver.el" <<'EOF'
;;; budgets driver: no timers, so idle really is idle.
(blink-cursor-mode 0)
(setq inhibit-startup-screen t server-name (getenv "BUDGETS_SRV"))
(find-file (getenv "BUDGETS_FILE"))
(setq-default cursor-in-non-selected-windows nil)
(require 'server)
(server-start)
EOF
awk 'BEGIN { for (i = 1; i <= 10000; i++) printf "line %05d the quick brown fox jumps over the lazy dog, again and again\n", i }' > "$run/big.txt"

for ((ri = 1; ri <= runs; ri++)); do
  # alternate which side goes first, so a drifting machine does not favour one
  if [ $((ri % 2)) = 1 ]; then order="A B"; else order="B A"; fi
  for side in $order; do
    if [ "$side" = A ]; then p=$pa; else p=$pb; fi
    measure "$side" "$p" || { echo "budgets.sh: the $side session failed"; exit 1; }
  done
  motion || { echo "budgets.sh: the motion session failed"; exit 1; }
done

# ---- aggregate: median and SD per row and side, then the verdict
awk -F'|' '
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
  function fmt(x) { return sprintf("%.4g", x) }
  function pair(row, side) { return "(" fmt(med(row, side)) " " fmt(sd(row, side)) ")" }
  {
    row = $1; side = $2; v = $3
    if (!(row in seen)) { seen[row] = 1; order[++nrows] = row }
    if (v ~ /^SKIP /) { if (!((row, side) in why)) why[row, side] = substr(v, 6); skip[row] = 1; next }
    cnt[row, side]++; val[row, side, cnt[row, side]] = v + 0
  }
  END {
    unit["idle-wakeups"] = "switches/s"; unit["idle-wakeups-blink"] = "switches/s (cursor blinking, 0.1 s)"; unit["idle-wakeups-threads"] = "switches/s (all but the main thread)"
    unit["pixels-per-char"] = "device px"; unit["resize-stuck"] = "steps"
    unit["latency-typing"] = "ms p95"; unit["motion-compose-p95"] = "ms"; unit["motion-interval-p95"] = "ms"
    unit["motion-cores"] = "cores"
    for (k = 1; k <= nrows; k++) {
      row = order[k]; u = unit[row]
      if (u == "") u = (row ~ /^cpu-/) ? "% of one core" : (row ~ /^power-/) ? "W" : (row ~ /^pss-/) ? "kB" : "-"
      line = "(:row " row " :unit \"" u "\""
      if (skip[row]) {
        reason = ""
        if ((row, "A") in why) reason = "A: " why[row, "A"]
        if ((row, "B") in why) reason = reason (reason == "" ? "" : "; ") "B: " why[row, "B"]
        gsub(/"/, "\047", reason)
        line = line " :skipped \"" reason "\""
        if (cnt[row, "A"]) line = line " :a " pair(row, "A")
        if (cnt[row, "B"]) line = line " :b " pair(row, "B")
        print line ")"; continue
      }
      if (row ~ /^motion-/) {
        line = line " :a " pair(row, "A")
        budget = (row == "motion-compose-p95") ? 8.33 : (row == "motion-cores") ? 1 : -1
        if (budget < 0) verdict = "info"
        else verdict = (med(row, "A") <= budget) ? "pass" : "fail"
        print line " :budget " (budget < 0 ? "nil" : budget) " :verdict " verdict ")"; continue
      }
      a = med(row, "A"); b = med(row, "B")
      tol = 0.05 * b; t2 = 2 * (sd(row, "A") > sd(row, "B") ? sd(row, "A") : sd(row, "B")); if (t2 > tol) tol = t2
      verdict = (a <= b) ? "pass" : (a - b <= tol) ? "equal" : "fail"
      print line " :a " pair(row, "A") " :b " pair(row, "B") " :verdict " verdict ")"
    }
  }' "$raw" > "$out"
cat "$out"
exit 0
