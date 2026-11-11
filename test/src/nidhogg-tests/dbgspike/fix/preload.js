// drgnx's debug preload (spike): a child's inspector.open(0) takes the next free port of the session's block.
// DRGNX_DEBUG_PORTS: the block's ports for children; DRGNX_DEBUG_CLAIMS: a directory in the session tmp where each
// port is claimed by an O_EXCL file, so two children never take the same one.
"use strict";
const fs = require("fs");
const path = require("path");
const inspector = require("inspector");
const ports = (process.env.DRGNX_DEBUG_PORTS || "").split(",").map(Number).filter(Boolean);
const dir = process.env.DRGNX_DEBUG_CLAIMS;
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
    }
    return open.call(this, port, host, wait);
  };
}
