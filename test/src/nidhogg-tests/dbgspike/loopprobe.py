# The loopback flag's checks under a Seatbelt profile: one "dbgspike-probe NAME ok|ERRNO[ step]" line each.
# loopprobe.py PORT HOSTPORT DEV TMP W
import errno, os, socket, sys

port, host_port, dev, tmp, w = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3], sys.argv[4], sys.argv[5]
fw = os.path.join(os.path.dirname(dev), "SharedFrameworks")


def say(name, fn):
    try:
        fn()
        print(f"dbgspike-probe {name} ok", flush=True)
    except OSError as e:
        print(f"dbgspike-probe {name} {errno.errorcode.get(e.errno, e.errno)} {getattr(e, 'step', '')}", flush=True)


def stepped(step, fn):
    try:
        return fn()
    except OSError as e:
        e.step = step
        raise


def tcp_self(p):
    def go():
        l = socket.socket()
        stepped("bind", lambda: l.bind(("127.0.0.1", p)))
        stepped("listen", lambda: l.listen(1))
        c = socket.socket()
        stepped("connect", lambda: c.connect(l.getsockname()))
        stepped("accept", lambda: l.accept()[0].close())
        c.close(); l.close()
    return go


def connect(addr):
    def go():
        s = socket.socket(); s.settimeout(3)
        s.connect(addr); s.close()
    return go


def unix_self(path):
    def go():
        try: os.unlink(path)
        except OSError: pass
        l = socket.socket(socket.AF_UNIX)
        stepped("bind", lambda: l.bind(path))
        stepped("listen", lambda: l.listen(1))
        c = socket.socket(socket.AF_UNIX)
        stepped("connect", lambda: c.connect(path))
        stepped("accept", lambda: l.accept()[0].close())
        c.close(); l.close(); os.unlink(path)
    return go


def write(path):
    def go():
        with open(path, "w") as f: f.write("x")
        os.unlink(path)
    return go


say("own-port", tcp_self(port))
say("other-port", tcp_self(port + 1))
say("ephemeral-port", tcp_self(0))
say("host-listener", connect(("127.0.0.1", host_port)))
say("non-loopback", connect(("1.1.1.1", 443)))
say("unix-in-tmp", unix_self(os.path.join(tmp, "s")))
say("unix-in-w", unix_self(os.path.join(w, "s")))
say("frameworks-list", lambda: os.listdir(fw))
say("lldb-framework-read", lambda: open(os.path.join(fw, "LLDB.framework/LLDB"), "rb").read(16))
say("dev-write", write(os.path.join(dev, "dbgspike-probe")))
say("frameworks-write", write(os.path.join(fw, "dbgspike-probe")))
