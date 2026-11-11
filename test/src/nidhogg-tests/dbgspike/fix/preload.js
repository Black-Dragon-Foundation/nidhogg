// drgnx's debug preload (spike): a child's inspector.open(0) takes the next free port of the session's block.
// DRGNX_DEBUG_PORTS: the block's ports for children; DRGNX_DEBUG_CLAIMS: a directory in the session tmp where each
// port is claimed by an O_EXCL file, so two children never take the same one.
"use strict";
const fs = require("fs");
const path = require("path");
const inspector = require("inspector");
const ports = (process.env.DRGNX_DEBUG_PORTS || "").split(",").map(Number).filter(Boolean);
const dir = process.env.DRGNX_DEBUG_CLAIMS;
const trace = (m) => process.env.DRGNX_DEBUG_TRACE && fs.writeSync(2, `drgnx-preload pid=${process.pid} ${m}\n`);
trace(`ppid=${process.ppid} execArgv=${JSON.stringify(process.execArgv)} node_options=${process.env.NODE_OPTIONS}`);
// a forked child inherits the parent's --inspect-brk=HOST:PORT through execArgv and would fight for that port: the
// child gets its inspector from js-debug's bootloader instead, on a block port (below)
process.execArgv = process.execArgv.filter((a) => !/^--inspect(-brk|-port|-wait)?(=|$)/.test(a));
function claim() {
  for (const p of ports) {
    try {
      fs.closeSync(fs.openSync(path.join(dir, "port-" + p), "wx"));
      return p;
    } catch (e) {
      if (e.code !== "EEXIST") throw e;
    }
  }
  throw new Error("drgnx: every debug port of the session's block is taken");
}
if (dir && ports.length) {
  const open = inspector.open;
  inspector.open = function (port, host, wait) {
    if (!port) {
      port = claim();
      host = "127.0.0.1";
      trace(`inspector.open(0) -> ${port}`);
    } else {
      trace(`inspector.open(${port})`);
    }
    return open.call(this, port, host, wait);
  };
}
