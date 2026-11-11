#!/usr/bin/env python3
"""DAP spike driver (throwaway, plan T13): run one adapter against a fixture and record what works.

  dapspike.py run ADAPTER FIXDIR OUT.json [--shim]   ADAPTER: debugpy | jsdebug | delve | lldb
  dapspike.py shim PORT                              stdio <-> TCP 127.0.0.1:PORT, several connections multiplexed by a
                                                     `Drgnx-Conn: N` header (the js-debug stdio shim's shape)

The scenario: initialize; launch; on `initialized` a breakpoint after a loop and a probe (a logpoint with the
drgnx marker) inside it; configurationDone; the stop; threads, stackTrace, scopes, variables; continue; the end.
Child sessions (`startDebugging`, js-debug) get a new connection to the same server (or a new shim connection).
"""
import json, os, socket, subprocess, sys, threading, queue, time

MARK = "\u001edrgnx:"
T0 = time.monotonic()


def now():
    return round((time.monotonic() - T0) * 1000, 1)


def read_msg(rf):
    """One DAP message from RF: (headers dict, body dict) or None at EOF."""
    hdr = b""
    while not hdr.endswith(b"\r\n\r\n"):
        c = rf.read(1)
        if not c:
            return None
        hdr += c
    h = {}
    for line in hdr.decode().split("\r\n"):
        if ":" in line:
            k, v = line.split(":", 1)
            h[k.strip().lower()] = v.strip()
    n = int(h["content-length"])
    body = b""
    while len(body) < n:
        c = rf.read(n - len(body))
        if not c:
            return None
        body += c
    return h, json.loads(body)


def frame(body, extra=None):
    b = json.dumps(body).encode()
    h = "".join(f"{k}: {v}\r\n" for k, v in (extra or {}).items())
    return f"Content-Length: {len(b)}\r\n{h}\r\n".encode() + b


class Conn:
    def __init__(self, name, rf, wf, log, on_reverse):
        self.name, self.rf, self.wf, self.log, self.on_reverse = name, rf, wf, log, on_reverse
        self.seq, self.q, self.events, self.lock = 0, queue.Queue(), [], threading.Lock()
        threading.Thread(target=self._reader, daemon=True).start()

    def _reader(self):
        while True:
            try:
                m = read_msg(self.rf)
            except Exception as e:  # noqa: BLE001
                self.log.append([now(), self.name, "read-error", str(e)])
                m = None
            if m is None:
                self.q.put(None)
                return
            body = m[1]
            if body.get("type") == "request":
                threading.Thread(target=self.on_reverse, args=(self, body), daemon=True).start()
            else:
                self.q.put(body)

    def send(self, body):
        with self.lock:
            self.wf.write(frame(body))
            self.wf.flush()

    def request(self, cmd, args=None):
        self.seq += 1
        s = self.seq
        self.send({"seq": s, "type": "request", "command": cmd, "arguments": args or {}})
        return s

    def answer(self, req, body=None, ok=True, msg=None):
        self.seq += 1
        r = {"seq": self.seq, "type": "response", "request_seq": req["seq"], "command": req["command"],
             "success": ok, "body": body or {}}
        if msg:
            r["message"] = msg
        self.send(r)

    def wait(self, pred, timeout):
        end = time.monotonic() + timeout
        for e in self.events:
            if pred(e):
                self.events.remove(e)
                return e
        while True:
            left = end - time.monotonic()
            if left <= 0:
                raise TimeoutError(f"{self.name}: timed out")
            try:
                m = self.q.get(timeout=left)
            except queue.Empty:
                raise TimeoutError(f"{self.name}: timed out") from None
            if m is None:
                raise EOFError(f"{self.name}: adapter closed the connection")
            if pred(m):
                return m
            self.events.append(m)

    def call(self, cmd, args=None, timeout=20):
        s = self.request(cmd, args)
        r = self.wait(lambda m: m.get("type") == "response" and m.get("request_seq") == s, timeout)
        if not r.get("success"):
            raise RuntimeError(f"{cmd} failed: {r.get('message')} {json.dumps(r.get('body'))[:300]}")
        return r.get("body") or {}


class ShimClient:
    """The hub's side of the shim: one stdio pipe to `dapspike.py shim PORT`, connections told apart by a header."""

    def __init__(self, port, log):
        self.p = subprocess.Popen([sys.executable, os.path.abspath(__file__), "shim", str(port)],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.pipes, self.n, self.lock = {}, 0, threading.Lock()
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self):
        while True:
            m = read_msg(self.p.stdout)
            if m is None:
                for r, _ in self.pipes.values():
                    os.close(r)
                return
            h, body = m
            w = self.pipes[int(h["drgnx-conn"])][1]
            os.write(w, frame(body))

    def open(self):
        """A (rf, wf) pair for a new connection: messages written go to the shim with this connection's id."""
        with self.lock:
            cid = self.n
            self.n += 1
        r_in, w_in = os.pipe()
        self.pipes[cid] = (r_in, w_in)
        shim = self

        class W:
            def write(self, data):
                m = read_msg(_Bytes(data))
                with shim.lock:
                    shim.p.stdin.write(frame(m[1], {"Drgnx-Conn": cid}))
                    shim.p.stdin.flush()

            def flush(self):
                pass

        return os.fdopen(r_in, "rb", buffering=0), W()


class _Bytes:
    def __init__(self, b):
        self.b, self.i = b, 0

    def read(self, n):
        out = self.b[self.i:self.i + n]
        self.i += n
        return out


def shim(port):
    """stdin/stdout <-> TCP connections to 127.0.0.1:PORT, one per Drgnx-Conn id, opened on its first message."""
    socks, out_lock = {}, threading.Lock()
    stdin, stdout = sys.stdin.buffer, sys.stdout.buffer

    def back(cid, s):
        f = s.makefile("rb", buffering=0)
        while True:
            m = read_msg(f)
            if m is None:
                return
            with out_lock:
                stdout.write(frame(m[1], {"Drgnx-Conn": cid}))
                stdout.flush()

    while True:
        m = read_msg(stdin)
        if m is None:
            return
        h, body = m
        cid = int(h["drgnx-conn"])
        if cid not in socks:
            s = connect(port, 10)
            socks[cid] = s
            threading.Thread(target=back, args=(cid, s), daemon=True).start()
        socks[cid].sendall(frame(body))


def connect(port, wait_s):
    end = time.monotonic() + wait_s
    while True:
        try:
            return socket.create_connection(("127.0.0.1", port))
        except OSError:
            if time.monotonic() > end:
                raise
            time.sleep(0.05)


def free_port():
    if os.environ.get("DBGSPIKE_PORT"):
        return int(os.environ["DBGSPIKE_PORT"])
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def run(adapter, fix, out_path, use_shim):
    res = {"adapter": adapter, "shim": use_shim, "steps": [], "log": [], "ok": False}
    procs = []

    def step(name, **kw):
        res["steps"].append(dict(t_ms=now(), step=name, **kw))

    fix = os.path.abspath(fix)
    port = None
    try:
        if adapter == "debugpy":
            cmd = [sys.executable, "-m", "debugpy.adapter"]
            prog = os.path.join(fix, "fixture.py")
            launch = {"type": "debugpy", "request": "launch", "program": prog, "console": "internalConsole",
                      "justMyCode": True, "cwd": fix}
            bp_file, bp_line, probe_line, probe_expr = prog, 9, 7, "{total}"
        elif adapter == "jsdebug":
            port = free_port()
            cmd = ["node", os.path.join(os.environ.get("JS_DEBUG", "/opt/js-debug"), "src/dapDebugServer.js"), str(port), "127.0.0.1"]
            prog = os.path.join(fix, "fixture.js")
            launch = {"type": "pwa-node", "request": "launch", "program": prog, "cwd": fix,
                      "console": "internalConsole", "outputCapture": "std"}
            bp_file, bp_line, probe_line, probe_expr = prog, 7, 5, "{total}"
        elif adapter == "delve":
            port = free_port()
            cmd = ["dlv", "dap", f"--listen=127.0.0.1:{port}"]
            prog = os.path.join(fix, "fixture-go")
            launch = {"type": "go", "request": "launch", "mode": "exec", "program": prog, "cwd": fix}
            bp_file, bp_line, probe_line, probe_expr = os.path.join(fix, "fixture.go"), 11, 9, "{total}"
        elif adapter == "lldb":
            cmd = ["lldb-dap"]
            prog = os.path.join(fix, "fixture-c")
            launch = {"type": "lldb-dap", "request": "launch", "program": prog, "cwd": fix, "disableASLR": False}
            bp_file, bp_line, probe_line, probe_expr = os.path.join(fix, "fixture.c"), 9, 7, "{total}"
        else:
            raise SystemExit(f"unknown adapter {adapter}")

        stderr_path = out_path + ".stderr"
        errf = open(stderr_path, "wb")
        step("spawn", cmd=cmd)
        if port is None:
            p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errf, cwd=fix)
            procs.append(p)
            opener = None
            rf, wf = p.stdout, p.stdin
        else:
            p = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=errf, stderr=errf, cwd=fix)
            procs.append(p)
            if use_shim:
                sc = ShimClient(port, res["log"])
                procs.append(sc.p)
                opener = sc.open
            else:
                def opener():
                    s = connect(port, 15)
                    return s.makefile("rb", buffering=0), s.makefile("wb", buffering=0)
            rf, wf = opener()
        step("connected")

        probes, outputs, children = [], [], []
        state = {"active": None}

        def on_output(conn):
            pass

        def configure(c):
            bps = [{"line": bp_line}, {"line": probe_line, "logMessage": f"{MARK}p1:{probe_expr}"}]
            r = c.call("setBreakpoints", {"source": {"path": bp_file}, "breakpoints": bps})
            step("setBreakpoints", conn=c.name,
                 answer=[{k: b.get(k) for k in ("verified", "line", "message")} for b in r.get("breakpoints", [])])
            try:
                c.call("setExceptionBreakpoints", {"filters": []})
            except Exception as e:  # noqa: BLE001
                step("setExceptionBreakpoints-failed", error=str(e))
            c.call("configurationDone")
            step("configurationDone", conn=c.name)

        def on_reverse(c, req):
            cmdn = req["command"]
            step("reverse", conn=c.name, command=cmdn)
            if cmdn == "startDebugging":
                c.answer(req)
                crf, cwf = opener()
                child = Conn(f"child{len(children)}", crf, cwf, res["log"], on_reverse)
                children.append(child)
                child.call("initialize", init_args())
                cfg = req["arguments"]["configuration"]
                child.request(req["arguments"].get("request", "launch"), cfg)
                child.wait(lambda m: m.get("event") == "initialized", 20)
                configure(child)
                state["active"] = child
            elif cmdn == "runInTerminal":
                a = req["arguments"]
                tp = subprocess.Popen(a["args"], cwd=a.get("cwd") or fix,
                                      env={**os.environ, **{k: v for k, v in (a.get("env") or {}).items() if v is not None}})
                procs.append(tp)
                c.answer(req, {"processId": tp.pid})
            else:
                c.answer(req, ok=False, msg="not supported by the spike")

        c = Conn("main", rf, wf, res["log"], on_reverse)
        state["active"] = c
        caps = c.call("initialize", init_args(), timeout=30)
        step("initialize", filters=[f.get("filter") for f in caps.get("exceptionBreakpointFilters", [])],
             caps=sorted(k for k, v in caps.items() if v is True))
        c.request(launch["request"], launch)
        c.wait(lambda m: m.get("event") == "initialized", 30)
        step("initialized")
        if adapter != "jsdebug":
            configure(c)
        else:
            c.call("configurationDone")
            step("configurationDone", conn="main")

        # wait for the stop on whichever connection is active (js-debug: the child)
        deadline = time.monotonic() + 60
        stopped = None
        while time.monotonic() < deadline and stopped is None:
            a = state["active"]
            try:
                m = a.wait(lambda m: m.get("event") in ("stopped", "terminated", "exited", "output"), 0.5)
            except TimeoutError:
                continue
            if m["event"] == "output":
                o = m["body"].get("output", "")
                (probes if MARK in o else outputs).append(o.strip()[:200])
            elif m["event"] == "stopped":
                stopped = m
            else:
                raise RuntimeError(f"ended before the stop: {m['event']}")
        if stopped is None:
            raise TimeoutError("no stop")
        a = state["active"]
        step("stopped", conn=a.name, reason=stopped["body"].get("reason"))
        th = a.call("threads")["threads"]
        tid = stopped["body"].get("threadId") or th[0]["id"]
        st = a.call("stackTrace", {"threadId": tid, "startFrame": 0, "levels": 20})
        top = st["stackFrames"][0]
        sc = a.call("scopes", {"frameId": top["id"]})["scopes"]
        vs = a.call("variables", {"variablesReference": sc[0]["variablesReference"]})["variables"]
        step("model", threads=len(th), top=[top.get("name"), top.get("line")], scopes=[s["name"] for s in sc],
             vars={v["name"]: v["value"] for v in vs[:12]})
        a.call("continue", {"threadId": tid})
        step("continued")
        try:
            while True:
                m = a.wait(lambda m: m.get("event") in ("terminated", "exited", "output"), 30)
                if m["event"] == "output":
                    o = m["body"].get("output", "")
                    (probes if MARK in o else outputs).append(o.strip()[:200])
                else:
                    step(m["event"], body=m.get("body"))
                    if m["event"] == "terminated":
                        break
        except (TimeoutError, EOFError) as e:
            step("end-wait", error=str(e))
        res["probes"] = probes
        res["outputs"] = outputs[:20]
        try:
            c.call("disconnect", {"terminateDebuggee": True}, timeout=5)
        except Exception as e:  # noqa: BLE001
            step("disconnect", error=str(e))
        res["ok"] = True
    except Exception as e:  # noqa: BLE001
        res["error"] = f"{type(e).__name__}: {e}"
    finally:
        for p in procs:
            try:
                p.terminate()
                p.wait(2)
            except Exception:  # noqa: BLE001
                p.kill()
        res["t_total_ms"] = now()
        try:
            res["stderr_tail"] = open(out_path + ".stderr", "rb").read()[-1500:].decode("utf-8", "replace")
        except OSError:
            pass
        with open(out_path, "w") as f:
            json.dump(res, f, indent=1)
        print(json.dumps({k: res.get(k) for k in ("adapter", "shim", "ok", "error", "t_total_ms")}))


def init_args():
    return {"clientID": "drgnx-spike", "adapterID": "spike", "linesStartAt1": True, "columnsStartAt1": True,
            "pathFormat": "path", "supportsVariableType": True, "supportsVariablePaging": True,
            "supportsRunInTerminalRequest": True, "supportsStartDebuggingRequest": True}


if __name__ == "__main__":
    if sys.argv[1] == "shim":
        shim(int(sys.argv[2]))
    else:
        run(sys.argv[2], sys.argv[3], sys.argv[4], "--shim" in sys.argv)
