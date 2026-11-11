// Run 11's js-debug parent: forks fixture.js on its first line, before js-debug's bootloader could reach this
// process unless configurationDone waits for the attach (the hub's hold).
const c = require("child_process").fork(require("path").join(__dirname, "fixture.js"));
c.on("exit", (code) => process.exit(code ?? 0));
