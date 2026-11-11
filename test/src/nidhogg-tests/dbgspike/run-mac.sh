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
# The debug branch's profiles (gleipnir's sbpl with the loopback flag; generated off the runner, see loop.sb), the
# stand-in developer dir replaced by this runner's. loop: the flag, own port 47613; noloop: the plain isolated tier.
app=${dev%/Contents/Developer}
for n in loop noloop; do sed "s#/Applications/Xcode.app/Contents#$app/Contents#g" "$here/$n.sb" > "$B/$n.sb"; done
say "profiles: $(grep -c . "$B/loop.sb") lines; app $app"
cp "$here/loopprobe.py" "$here/dapspike.py" "$B/fix/"
"$B/venv/bin/python" -c 'import socket,time,sys
l=socket.socket(); l.bind(("127.0.0.1",0)); l.listen(5); print(l.getsockname()[1], flush=True); time.sleep(900)' > "$B/host-port" &
hostpid=$!
sleep 1
hp=$(cat "$B/host-port")
for n in loop noloop; do
  /usr/bin/env TMPDIR="$B/t/" HOME="$B/t" /usr/bin/sandbox-exec -f "$B/$n.sb" "$B/venv/bin/python" "$B/fix/loopprobe.py" 47613 "$hp" "$dev" "$B/t" "$B/w" 2>&1 |
    sed "s/^dbgspike-probe /dbgspike-probe $n /"
done
"$B/venv/bin/python" "$B/fix/loopprobe.py" 47613 "$hp" "$dev" "$B/t" "$B/w" 2>&1 | sed "s/^dbgspike-probe /dbgspike-probe bare /"
kill $hostpid 2>/dev/null
summ() { "$B/venv/bin/python" -c 'import json,sys
r=json.load(open(sys.argv[1]))
st={s["step"]:s for s in r.get("steps",[])}
print(json.dumps({"adapter":r.get("adapter"),"shim":r.get("shim"),"ok":r.get("ok"),"error":r.get("error"),
 "t_init":st.get("initialize",{}).get("t_ms"),"t_stop":st.get("stopped",{}).get("t_ms"),"t":r.get("t_total_ms"),
 "probes":len(r.get("probes",[])),"vars":st.get("model",{}).get("vars"),"stderr":(r.get("stderr_tail") or "")[-300:]}))' "$1" 2>&1; }
for a in debugpy jsdebug lldb; do
  for mode in loop; do
    for shim in "" --shim; do
      [ -n "$shim" ] && [ "$a" != jsdebug ] && continue
      rm -f "$B/w/r.json"
      if [ "$mode" = bare ]; then
        (cd fix && perl -e 'alarm shift; exec @ARGV' 120 python3 dapspike.py run "$a" "$B/fix" "$B/w/r.json" $shim > /dev/null 2>&1)
      else
        (cd fix && perl -e 'alarm shift; exec @ARGV' 120 /usr/bin/env TMPDIR="$B/t/" HOME="$B/t" DBGSPIKE_PORT=47613 /usr/bin/sandbox-exec -f "$B/$mode.sb" "$B/venv/bin/python" dapspike.py run "$a" "$B/fix" "$B/w/r.json" $shim > /dev/null 2>"$B/w/se.txt")
      fi
      echo "dbgspike-result $a${shim:+-shim} $mode rc=$? $(summ "$B/w/r.json") $(head -c 300 "$B/w/se.txt" 2>/dev/null)"
      rm -f "$B/w/se.txt"
    done
  done
done
say done
