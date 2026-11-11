#!/bin/bash
# A throwaway spike (drgnx's graphical debugging plan, T13), on a macOS CI runner only, never merged: four debug
# adapters (debugpy, js-debug, delve, lldb-dap) with a fixture, bare and under Gleipnir's Seatbelt profile (generated
# by gleipnir at drgnx's public main), shared and isolated network, and isolated plus a candidate loopback rule.
# Now: the debug branch's loopback flag (loop.sb / noloop.sb, made by its gleipnir), probed, and the adapters under it.
# Prints "dbgspike: ..." lines and one JSON summary per run.
set -u
here=$(cd "$(dirname "$0")" && pwd)
B=/private/tmp/dbgspike
mkdir -p "$B/w" "$B/t" "$B/fix"
cd "$B" || exit 1
say() { echo "dbgspike: $*"; }
export PATH=/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH
dev=$(cd "$(xcode-select -p)" && pwd -P)
if [ ! -f setup-done ]; then
  t0=$(date +%s)
  brew install --quiet node go delve > brew.log 2>&1 || { say "brew failed"; tail -20 brew.log; }
  /opt/homebrew/bin/python3 -m venv "$B/venv" && "$B/venv/bin/pip" -q install debugpy==1.8.22 || say "pip failed"
  curl -sSfL -o js.tgz https://github.com/microsoft/vscode-js-debug/releases/download/v1.140.1/js-debug-dap-v1.140.1.tar.gz &&
    echo "c72fab31bcbfdce6ad1f42adfd28eff8d4681dd3b03b5e2e6f9e72e6f9927faf  js.tgz" | shasum -a 256 -c - && tar -xzf js.tgz
  cp "$here"/fix/* "$here/dapspike.py" "$B/fix/"
  (cd fix && go mod init spike > /dev/null 2>&1; go build -gcflags=all='-N -l' -o fixture-go fixture.go) || say "go build failed"
  clang -g -O0 -o fix/fixture-c fix/fixture.c || say "cc failed"
  touch setup-done
  say "setup $(( $(date +%s) - t0 )) s; developer dir $dev; $(sw_vers -productVersion) $(uname -m)"
fi
say "versions: $(node --version) $(go version) $(dlv version 2>&1 | sed -n 2p) $("$B/venv/bin/python" -c 'import debugpy,sys; print("debugpy", debugpy.__version__, sys.version.split()[0])')"
say "lldb-dap: $(xcrun -f lldb-dap 2>&1)"
say "DevToolsSecurity: $(DevToolsSecurity -status 2>&1)"
export JS_DEBUG=$B/js-debug
PATH=$B/venv/bin:$PATH:$dev/usr/bin
# The chosen design (spec §21): each session's fixed block of ports (47610-47617), its profile (block.sb: the debug
# branch's loopback flag with the block as mac_loopback_ports, plus Unix connect under the session tmp), debugpy as an
# attach with listen, js-debug attached to --inspect-brk with the port-0 preload. At each stop, lsof (outside the
# sandbox) lists every socket of the sandboxed tree: any TCP port outside the block is a finding.
app=${dev%/Contents/Developer}
for n in block noloop; do sed "s#/Applications/Xcode.app/Contents#$app/Contents#g" "$here/$n.sb" > "$B/$n.sb"; done
say "profiles: $(grep -c . "$B/block.sb") lines; app $app"
cp "$here/loopprobe.py" "$here/dapspike.py" "$here"/fix/* "$B/fix/"
"$B/venv/bin/python" -c 'import socket,time,sys,os
l=socket.socket(); l.bind(("127.0.0.1",0)); l.listen(5)
os.makedirs(sys.argv[1], exist_ok=True); u=socket.socket(socket.AF_UNIX); p=os.path.join(sys.argv[1],"s")
try: os.unlink(p)
except OSError: pass
u.bind(p); u.listen(5); print(l.getsockname()[1], flush=True); time.sleep(900)' "$B/hostsock" > "$B/host-port" &
hostpid=$!
sleep 1
hp=$(cat "$B/host-port")
# what holds the block's ports before anything starts (run 8: the probe's bind of 47610 was EADDRINUSE)
lsof -nP -iTCP:47610-47617 2>/dev/null | sed 's/^/dbgspike-held /'
for n in block noloop; do
  /usr/bin/env TMPDIR="$B/t/" HOME="$B/t" /usr/bin/sandbox-exec -f "$B/$n.sb" "$B/venv/bin/python" "$B/fix/loopprobe.py" 47610 "$hp" "$dev" "$B/t" "$B/w" 47620 "$B/hostsock/s" 2>&1 |
    sed "s/^dbgspike-probe /dbgspike-probe $n /"
done
"$B/venv/bin/python" "$B/fix/loopprobe.py" 47610 "$hp" "$dev" "$B/t" "$B/w" 47620 "$B/hostsock/s" 2>&1 | sed "s/^dbgspike-probe /dbgspike-probe bare /"
summ() { "$B/venv/bin/python" -c 'import json,sys
r=json.load(open(sys.argv[1]))
st={s["step"]:s for s in r.get("steps",[])}
print(json.dumps({"adapter":r.get("adapter"),"ok":r.get("ok"),"error":r.get("error"),
 "t_init":st.get("initialize",{}).get("t_ms"),"t_stop":st.get("stopped",{}).get("t_ms"),"t":r.get("t_total_ms"),
 "stop_conn":st.get("stopped",{}).get("conn"),"probes":len(r.get("probes",[])),"vars":st.get("model",{}).get("vars"),
 "reverse":[s.get("command") for s in r.get("steps",[]) if s["step"]=="reverse"],"stderr":(r.get("stderr_tail") or "")[-400:]}))' "$1" 2>&1; }
descend() {  # PID and every descendant
  local all out q p c
  all=$(ps -A -o pid=,ppid=); out=$1; q=$1
  while [ -n "$q" ]; do
    set -- $q; p=$1; shift; q="$*"
    for c in $(echo "$all" | awk -v p="$p" '$2==p{print $1}'); do out="$out $c"; q="$q $c"; done
  done
  echo "$out"
}
# a listener outside the block, or a connection with neither end in it (an outbound connect's own end is an ephemeral
# port: that is not a bind), for the processes PIDS
outside() {
  lsof -nP -a -p "$1" -i 2>/dev/null | awk 'NR>1{print $9, $10}' | awk '
    function inb(x) { n = split(x, a, ":"); p = a[n] + 0; return p >= 47610 && p <= 47617 }
    { split($1, e, "->"); if ($2 == "(LISTEN)" || e[2] == "") { if (!inb(e[1])) print $1 }
      else if (!inb(e[1]) && !inb(e[2])) print $1 }'
}
for a in debugpy-listen jsdebug-attach; do
  for run in 1 2; do
    rm -rf "$B/w/r.json" "$B/w/mark" "$B/t/claims"; mkdir -p "$B/t/claims"
    : > "$B/t/claims/port-47610"; : > "$B/t/claims/port-47611"
    (cd fix && perl -e 'alarm shift; exec @ARGV' 150 /usr/bin/env TMPDIR="$B/t/" HOME="$B/t" DBGSPIKE_MARK="$B/w/mark" DBGSPIKE_CLAIMS="$B/t/claims" /usr/bin/sandbox-exec -f "$B/block.sb" "$B/venv/bin/python" dapspike.py run "$a" "$B/fix" "$B/w/r.json" > /dev/null 2>"$B/w/se.txt") &
    top=$!
    : > "$B/w/sweep"
    for i in $(seq 1 600); do
      [ -f "$B/w/mark" ] && break; kill -0 $top 2>/dev/null || break
      # every second, whatever the run does: the tree's sockets outside the block (a run that never stops is seen too)
      if [ $((i % 5)) = 0 ]; then
        sp=$(descend "$(pgrep -P $top | head -1)" | tr ' ' '\n' | sed '/^$/d' | sort -un | paste -sd, -)
        [ -n "$sp" ] && { outside "$sp" >> "$B/w/sweep"; ps -o command= -p "$sp" | cut -c1-160 >> "$B/w/sweep-ps"; }
      fi
      sleep 0.2
    done
    if [ -f "$B/w/mark" ]; then
      pids=$(descend "$(pgrep -P $top | head -1)" | tr ' ' '\n' | sed '/^$/d' | sort -un | paste -sd, -)
      ps -o pid=,ppid=,command= -p "$pids" | cut -c1-200 | sed "s/^/dbgspike-ps $a/"
      lsof -nP -a -p "$pids" -i 2>/dev/null | sed "s/^/dbgspike-lsof $a /"
      lsof -nP -a -p "$pids" -U 2>/dev/null | awk 'NR>1 && $NF ~ /\//' | sed "s/^/dbgspike-unix $a /"
      out=$(outside "$pids" | sort -u | paste -sd' ' -)
      echo "dbgspike-block $a run$run outside=[${out}] claims=[$(ls "$B/t/claims" | paste -sd' ' -)]"
    else
      echo "dbgspike-block $a run$run never-stopped"
    fi
    wait $top
    echo "dbgspike-sweep $a run$run outside=[$(sort -u "$B/w/sweep" | paste -sd' ' -)]"
    sort -u "$B/w/sweep-ps" 2>/dev/null | sed "s/^/dbgspike-sweep-ps $a run$run /"; rm -f "$B/w/sweep-ps"
    grep -a "fixture-child" "$B/w/r.json.stderr" 2>/dev/null | cut -c1-400 | sed "s/^/dbgspike-child $a run$run /"
    grep -a "drgnx-preload" "$B/w/r.json.stderr" 2>/dev/null | cut -c1-300 | sed "s/^/dbgspike-preload $a run$run /"
    echo "dbgspike-result $a run$run rc=$? $(summ "$B/w/r.json") $(head -c 300 "$B/w/se.txt" 2>/dev/null)"
  done
done
kill $hostpid 2>/dev/null
say done
