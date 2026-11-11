const { fork } = require("child_process");
const c = fork(require("path").join(__dirname, "fixture.js"));
c.on("exit", (code) => process.exit(code ?? 0));
