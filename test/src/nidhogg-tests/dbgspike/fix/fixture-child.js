// The js-debug attach spike's parent: forks fixture.js.  js-debug, attached to this process, puts its bootloader into
// NODE_OPTIONS (a Runtime.evaluate after the attach); a fork made before that has no debugger.  So the parent waits up
// to 5 s for the bootloader and says what it saw (the spike asks whether attach mode injects it at all).
const { fork } = require("child_process");
const say = (m) => process.stderr.write(`fixture-child ${m}\n`);
const t0 = Date.now();
function go() {
  const o = process.env.NODE_OPTIONS || "";
  if (!/bootloader/.test(o) && Date.now() - t0 < 5000) return setTimeout(go, 50);
  say(`after ${Date.now() - t0} ms NODE_OPTIONS=${o} VSCODE_INSPECTOR_OPTIONS=${process.env.VSCODE_INSPECTOR_OPTIONS || ""}`);
  const c = fork(require("path").join(__dirname, "fixture.js"));
  c.on("exit", (code) => process.exit(code ?? 0));
}
go();
