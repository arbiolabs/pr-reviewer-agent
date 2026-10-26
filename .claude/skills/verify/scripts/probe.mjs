#!/usr/bin/env node
// Read-only protocol v1 probe: hello, print the welcome; with `snapshot`, also print every PR, run and finding.
// usage: probe.mjs SOCKET [snapshot]
import { connect } from "node:net";

const [socket, what] = process.argv.slice(2);
if (!socket) {
  console.error("usage: probe.mjs SOCKET [snapshot]");
  process.exit(2);
}
const s = connect(socket);
const timer = setTimeout(() => fail("no answer in 10 s"), 10_000);
const send = (m) => s.write(`${JSON.stringify(m)}\n`);
function fail(why) {
  console.error(`probe: ${why}`);
  process.exit(1);
}

let buf = "";
s.on("error", (e) => fail(e.message));
s.on("connect", () => send({ type: "hello", requestId: "verify-hello", protocolVersion: 1, client: "verify-probe", capabilities: ["snapshots"] }));
s.on("data", (chunk) => {
  buf += chunk;
  let i;
  while ((i = buf.indexOf("\n")) >= 0) {
    const m = JSON.parse(buf.slice(0, i));
    buf = buf.slice(i + 1);
    if (m.type === "error") fail(`${m.code}: ${m.message}`);
    if (m.type === "welcome") {
      const { mode, readOnlyReason, capabilities, serverVersion, streamEpoch, latestSequence } = m;
      console.log(JSON.stringify({ welcome: { serverVersion, mode, readOnlyReason, capabilities, streamEpoch, latestSequence } }));
      if (what !== "snapshot") return done();
      send({ type: "snapshot.get", requestId: "verify-snapshot" });
    }
    if (m.type === "snapshot") {
      console.log(JSON.stringify({ prs: m.prs.map(({ id, title, headSha, lifecycle }) => ({ id, title, headSha, lifecycle })) }));
      console.log(JSON.stringify({ runs: m.runs.map(({ id, prId, phase, status, model }) => ({ id, prId, phase, status, model })) }));
      console.log(JSON.stringify({ findings: m.findings.map(({ id, prId, severity, status, title }) => ({ id, prId, severity, status, title })) }));
      return done();
    }
  }
});
function done() {
  clearTimeout(timer);
  s.end();
}
