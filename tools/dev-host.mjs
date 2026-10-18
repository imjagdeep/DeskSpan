// A stand-in for the app while the Rust media engine is being built:
// serves the built receiver page and speaks the same pairing/signalling
// protocol as src-tauri/src/server.rs, relaying WebRTC to a fake host page
// (tools/fake-host.html) that streams a test picture from a <canvas>.
//
//   npm run build && node tools/dev-host.mjs [port]
//   receiver:  http://127.0.0.1:8130/#t=<token printed below>
//   fake host: http://127.0.0.1:8130/fake-host
//
// New devices are allowed automatically (DEVHOST_ASK=1 asks on the console
// instead). Not part of the app; testing only, listens on 127.0.0.1.

import { createServer } from "node:http";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { randomBytes, randomInt, createHash, timingSafeEqual } from "node:crypto";
import { extname, join, normalize } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { WebSocketServer } from "ws";

const PORT = Number(process.argv[2] || 8130);
// Files "sent from the phone" land here (DEVHOST_INBOX overrides).
const INBOX = process.env.DEVHOST_INBOX || join(fileURLToPath(new URL("..", import.meta.url)), "tools", ".inbox");
const ROOT = fileURLToPath(new URL("..", import.meta.url));
const RECEIVER = join(ROOT, "dist", "receiver");
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".png": "image/png", ".webmanifest": "application/manifest+json" };

let pairing = newPairing();
const devices = new Map(); // id -> { name, keyHash, control }
const sessions = new Map(); // session -> ws
let nextSession = 1;
let host = null; // the fake host page's socket

function newPairing() {
  const p = { token: randomBytes(16).toString("hex"), code: String(randomInt(0, 1_000_000)).padStart(6, "0"), wrong: 0 };
  console.log(`pairing: http://127.0.0.1:${PORT}/#t=${p.token}  code ${p.code}`);
  return p;
}
const sha = (s) => createHash("sha256").update(s).digest("hex");
const same = (a, b) => a.length === b.length && timingSafeEqual(Buffer.from(a), Buffer.from(b));

const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://x");
  const path = url.pathname;
  if (path === "/api/send" && req.method === "POST") return receive(req, res, url);
  let file;
  if (path === "/" || path === "/index.html") file = join(RECEIVER, "index.html");
  else if (path === "/fake-host") file = join(ROOT, "tools", "fake-host.html");
  else if (path === "/fake-host.js") file = join(ROOT, "tools", "fake-host.js");
  else if (/^\/(assets\/[\w.-]+|favicon\.png|manifest\.webmanifest)$/.test(path)) file = normalize(join(RECEIVER, path));
  if (!file) {
    res.writeHead(404).end();
    return;
  }
  try {
    const body = await readFile(file);
    const headers = { "Content-Type": TYPES[extname(file)] ?? "application/octet-stream", "X-Content-Type-Options": "nosniff" };
    // Same policy as server.rs gives the receiver page.
    if (file.endsWith("index.html"))
      headers["Content-Security-Policy"] =
        `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; media-src 'self' blob:; connect-src 'self' ws://${req.headers.host}; frame-ancestors 'none'; base-uri 'none'; form-action 'none'`;
    res.writeHead(200, headers).end(body);
  } catch {
    res.writeHead(404).end();
  }
});

// Same checks as server.rs: an allowed device's id + key, a cleaned name.
async function receive(req, res, url) {
  const d = devices.get(req.headers["x-deskspan-device"] ?? "");
  if (!d || !same(d.keyHash, sha(String(req.headers["x-deskspan-key"] ?? "")))) {
    res.writeHead(401).end("this device isn't allowed");
    return;
  }
  const raw = url.searchParams.get("name") ?? "";
  const name = raw.split(/[\\/]/).pop().replace(/[\u0000-\u001f<>:"|?*]/g, "").replace(/^\.+/, "").trim() || "file from phone";
  const chunks = [];
  for await (const c of req) chunks.push(c);
  await mkdir(INBOX, { recursive: true });
  await writeFile(join(INBOX, name), Buffer.concat(chunks));
  console.log(`received ${name} (${Buffer.concat(chunks).length} bytes)`);
  res.writeHead(200, { "Content-Type": "application/json" }).end(JSON.stringify({ saved: name }));
}

const rl = process.env.DEVHOST_ASK ? createInterface({ input: process.stdin }) : null;
const ask = (name) =>
  rl
    ? new Promise((ok) => rl.question(`Allow "${name}"? [y/N/c=with control] `, (a) => ok({ allow: /^[yc]/i.test(a), control: /^c/i.test(a) })))
    : new Promise((ok) => setTimeout(() => ok({ allow: true, control: true }), 400));

const wss = new WebSocketServer({ noServer: true, maxPayload: 64 * 1024 });
server.on("upgrade", (req, socket, head) => {
  const path = new URL(req.url, "http://x").pathname;
  if (path !== "/ws" && path !== "/host") return socket.destroy();
  if (req.headers.origin && req.headers.origin !== `http://${req.headers.host}`) return socket.destroy();
  wss.handleUpgrade(req, socket, head, (ws) => (path === "/host" ? onHost(ws) : onReceiver(ws)));
});

function onHost(ws) {
  host = ws;
  console.log("fake host connected");
  ws.on("message", (raw) => {
    const m = JSON.parse(String(raw));
    const r = sessions.get(m.session);
    if (r && (m.t === "offer" || m.t === "ice")) r.send(JSON.stringify(m.t === "offer" ? { t: "offer", sdp: m.sdp } : { t: "ice", c: m.c }));
  });
  for (const id of sessions.keys()) ws.send(JSON.stringify({ t: "join", session: id }));
}

function onReceiver(ws) {
  let session = 0;
  const say = (m) => ws.send(JSON.stringify(m));
  ws.on("message", async (raw) => {
    let m;
    try {
      m = JSON.parse(String(raw));
    } catch {
      return;
    }
    if (m.t === "hello" && !session) {
      const a = m.auth ?? {};
      let id, key, control;
      if (a.kind === "device") {
        const d = devices.get(a.id);
        if (!d || !same(d.keyHash, sha(a.key ?? ""))) return say({ t: "need-code" });
        id = a.id;
        control = d.control;
      } else {
        const ok = a.kind === "token" ? same(a.token ?? "", pairing.token) : a.kind === "code" && same(String(a.code ?? "").trim(), pairing.code);
        if (!ok) {
          if (a.kind === "token") return say({ t: "need-code" });
          pairing.wrong++;
          const left = Math.max(0, 5 - pairing.wrong);
          if (!left) pairing = newPairing();
          return say({ t: "wrong-code", left });
        }
        pairing = newPairing();
        say({ t: "wait" });
        const d = await ask(m.name);
        if (!d.allow) {
          say({ t: "denied" });
          say({ t: "bye", reason: "Not allowed on the computer." });
          return ws.close();
        }
        id = randomBytes(8).toString("hex");
        key = randomBytes(32).toString("hex");
        control = d.control;
        devices.set(id, { name: m.name, keyHash: sha(key), control });
      }
      session = nextSession++;
      sessions.set(session, ws);
      say({ t: "welcome", id, key, host: "dev-host", control });
      console.log(`welcome ${m.name} (session ${session}, screen ${JSON.stringify(m.screen)})`);
      return;
    }
    // The picture starts and stops on request (DeskLab Link home page).
    if (session && m.t === "screen-start") host?.send(JSON.stringify({ t: "join", session }));
    if (session && m.t === "screen-stop") host?.send(JSON.stringify({ t: "leave", session }));
    if (session && m.t === "sent") console.log(`sent batch: ${m.count} files`);
    if (session && (m.t === "answer" || m.t === "ice")) host?.send(JSON.stringify({ ...m, session }));
    if (session && m.t === "screen") console.log(`session ${session} screen ${m.w}x${m.h}@${m.dpr}`);
  });
  ws.on("close", () => {
    if (!session) return;
    sessions.delete(session);
    host?.send(JSON.stringify({ t: "leave", session }));
  });
}

server.listen(PORT, "127.0.0.1", () => console.log(`dev host on http://127.0.0.1:${PORT}/`));
