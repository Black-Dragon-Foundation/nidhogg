#!/bin/bash
# A throwaway spike (drgnx's graphical debugging plan, T13), on a macOS CI runner only, never merged: four debug
# adapters (debugpy, js-debug, delve, lldb-dap) with a fixture, bare and under Gleipnir's Seatbelt profile (generated
# by gleipnir at drgnx's public main), shared and isolated network, and isolated plus a candidate loopback rule.
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
  git clone -q --depth 1 https://github.com/Black-Dragon-Foundation/drgnx.git drgnx || say "clone failed"
  mkdir -p prof/src
  cp "$here/prof-main.rs" prof/src/main.rs
  printf '[package]\nname = "prof"\nversion = "0.1.0"\nedition = "2024"\n[dependencies]\ngleipnir = { path = "../drgnx/runtime/crates/gleipnir" }\nserde_json = "1"\n[workspace]\n' > prof/Cargo.toml
  (cd prof && cargo build -q --release > ../cargo.log 2>&1) || { say "prof build failed"; tail -30 cargo.log; }
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
spec() {   # spec NET > file
  cat <<EOF
{"read": ["$B/venv", "$B/fix", "$B/js-debug"],
 "write": ["$B/w", "$B/t"],
 "env": [["PATH", "$PATH"], ["HOME", "$B/t"], ["TMPDIR", "$B/t"], ["JS_DEBUG", "$B/js-debug"],
         ["PYTHONDONTWRITEBYTECODE", "1"]],
 "chdir": "$B/fix", "net": $1, "tmp": "$B/t", "layers": {"landlock_min_abi": 0}}
EOF
}
spec '"shared"' > spec-shared.json
spec '{"isolated": {"relay": [], "mac_tcp_ports": []}}' > spec-iso.json
for n in shared iso; do
  out=$(prof/target/release/prof spec-$n.json "$dev" $n.sb 2>&1) || say "profile ($n) refused: $(echo "$out" | tr '\n' ' ' | head -c 600)"
done
[ -x prof/target/release/prof ] || say "no prof binary: $(tail -c 600 cargo.log | tr '\n' ' ')"
{ cat iso.sb; printf '\n; candidate Q17 rule: loopback bind, inbound and outbound\n(allow network-bind (local ip "localhost:*"))\n(allow network-inbound (local ip "localhost:*"))\n(allow network-outbound (remote ip "localhost:*"))\n'; } > iso-loop.sb
summ() { "$B/venv/bin/python" -c 'import json,sys
r=json.load(open(sys.argv[1]))
st={s["step"]:s for s in r.get("steps",[])}
print(json.dumps({"adapter":r.get("adapter"),"shim":r.get("shim"),"ok":r.get("ok"),"error":r.get("error"),
 "t_init":st.get("initialize",{}).get("t_ms"),"t_stop":st.get("stopped",{}).get("t_ms"),"t":r.get("t_total_ms"),
 "probes":len(r.get("probes",[])),"vars":st.get("model",{}).get("vars"),"stderr":(r.get("stderr_tail") or "")[-300:]}))' "$1" 2>&1; }
for a in debugpy jsdebug delve lldb; do
  for mode in bare shared iso iso-loop; do
    for shim in "" --shim; do
      [ -n "$shim" ] && [ "$a" != jsdebug ] && continue
      rm -f "$B/w/r.json"
      if [ "$mode" = bare ]; then
        (cd fix && perl -e 'alarm shift; exec @ARGV' 120 python3 dapspike.py run "$a" "$B/fix" "$B/w/r.json" $shim > /dev/null 2>&1)
      else
        (cd fix && perl -e 'alarm shift; exec @ARGV' 120 /usr/bin/sandbox-exec -f "$B/$mode.sb" "$B/venv/bin/python" dapspike.py run "$a" "$B/fix" "$B/w/r.json" $shim > /dev/null 2>"$B/w/se.txt")
      fi
      echo "dbgspike-result $a${shim:+-shim} $mode rc=$? $(summ "$B/w/r.json") $(head -c 300 "$B/w/se.txt" 2>/dev/null)"
      rm -f "$B/w/se.txt"
    done
  done
done
say "fork start pieces: $(cd "$B" && rm -rf proj && mkdir proj && cd proj && git init -q && for d in $(seq 0 49); do mkdir -p pkg$d; for f in $(seq 0 99); do printf 'def f():\n    return %d\n' "$f" > pkg$d/m$f.py; done; done && git add -A && git -c user.email=s@x -c user.name=s commit -qm base && for i in 1 2 3; do s=$(python3 -c 'import time;print(time.monotonic())'); git worktree add -q --detach ../wt$i; e=$(python3 -c 'import time;print(time.monotonic())'); printf 'worktree-add %s ms; ' "$(python3 -c "print(round(($e-$s)*1000))")"; done)"
for i in 1 2 3; do
  s=$(python3 -c 'import time;print(time.monotonic())'); /usr/bin/sandbox-exec -f "$B/iso.sb" /usr/bin/true
  say "sandbox-exec start+exit run $i: $(python3 -c "import time;print(round((time.monotonic()-$s)*1000))") ms"
done
say done
