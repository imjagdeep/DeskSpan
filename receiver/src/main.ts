// The receiver: pair with DeskSpan on the computer, then show its screen
// full size and (if allowed there) send touch, pen and mouse back.

import "./style.css";
import { toScreen } from "./geometry";
import { guessDeviceName, parseInbound, type Auth, type InputEvent, type Outbound, type ScreenInfo } from "./protocol";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

// The computer to talk to: the page's own server, or (in the DeskSpan Screen
// app, whose page is local) the address it was opened with: ?host=ip:port.
const HOST = (() => {
  const h = new URLSearchParams(location.search).get("host");
  return h && /^[\w.:[\]-]+$/.test(h) ? h : location.host;
})();
const video = $<HTMLVideoElement>("screen");
const panel = $<HTMLElement>("panel");
const title = $<HTMLElement>("title");
const text = $<HTMLElement>("text");
const codeForm = $<HTMLFormElement>("code-form");
const codeInput = $<HTMLInputElement>("code");
const nameInput = $<HTMLInputElement>("name");
const actions = $<HTMLElement>("actions");
const retry = $<HTMLButtonElement>("retry");
const bar = $<HTMLElement>("bar");
const barHost = $<HTMLElement>("bar-host");
const soundBtn = $<HTMLButtonElement>("sound");
const fullBtn = $<HTMLButtonElement>("full");
const leaveBtn = $<HTMLButtonElement>("leave");
const fsPrompt = $<HTMLElement>("fs-prompt");
const fsGo = $<HTMLButtonElement>("fs-go");
const fsSkip = $<HTMLButtonElement>("fs-skip");
const home = $<HTMLElement>("home");
const goScreen = $<HTMLButtonElement>("go-screen");
const goSend = $<HTMLButtonElement>("go-send");
const sendPanel = $<HTMLElement>("send");
const filesInput = $<HTMLInputElement>("files");
const cameraInput = $<HTMLInputElement>("camera");
const sendList = $<HTMLUListElement>("send-list");
const sendBack = $<HTMLButtonElement>("send-back");
const homeBtn = $<HTMLButtonElement>("home-btn");

// ---------- saved device (browser storage; may be unavailable) ----------

interface Saved {
  id?: string;
  key?: string;
  name?: string;
}
// One pairing per computer: the app talks to several from one origin.
const STORE = HOST === location.host ? "deskspan-device" : `deskspan-device@${HOST}`;

function loadSaved(): Saved {
  try {
    const raw = localStorage.getItem(STORE);
    const v: unknown = raw ? JSON.parse(raw) : {};
    return typeof v === "object" && v !== null ? (v as Saved) : {};
  } catch {
    // Private window or blocked storage: pair each time.
    return {};
  }
}

function save(s: Saved) {
  try {
    localStorage.setItem(STORE, JSON.stringify(s));
  } catch {
    // Not remembered: the next visit asks for a code again.
  }
}

let saved = loadSaved();
const deviceName = () => saved.name || guessDeviceName(navigator.userAgent, navigator.maxTouchPoints || 0);

// A one-time link from the QR code: #t=<token>. Taken out of the address bar
// so it isn't bookmarked or shared by accident.
let token: string | null = null;
{
  const m = /(?:^#|&)t=([0-9a-f]{16,64})/.exec(location.hash);
  if (m) token = m[1];
  if (location.hash) history.replaceState(null, "", location.pathname);
}

// ---------- screens of the panel ----------

type View = "connecting" | "code" | "wait" | "message" | "live" | "home" | "send";

function show(view: View, heading = "", body = "", canRetry = false) {
  const live = view === "live";
  if (live && !document.body.classList.contains("live")) offerFullScreen();
  if (!live) fsPrompt.hidden = true;
  document.body.classList.toggle("live", live);
  panel.hidden = live;
  codeForm.hidden = view !== "code";
  home.hidden = view !== "home";
  sendPanel.hidden = view !== "send";
  actions.hidden = !canRetry;
  title.textContent = heading;
  text.textContent = body;
  if (view === "code") {
    nameInput.value = deviceName();
    window.setTimeout(() => codeInput.focus(), 50);
  }
}

// ---------- connection ----------

let ws: WebSocket | null = null;
let pc: RTCPeerConnection | null = null;
let input: RTCDataChannel | null = null;
let control = false;
let leaving = false;
let welcomed = false;
let retryDelay = 1000;
let pendingAuth: Auth | null = null;
let host = "the computer";
/** This device's id and key, for uploads (also kept if storage is blocked). */
let creds: { id: string; key: string } | null = saved.id && saved.key ? { id: saved.id, key: saved.key } : null;

// The screen comes back by itself after a reconnect if it was in use.
const AUTO_SCREEN = HOST === location.host ? "deskspan-auto-screen" : `deskspan-auto-screen@${HOST}`;
function autoScreen(): boolean {
  try {
    return localStorage.getItem(AUTO_SCREEN) === "1";
  } catch {
    return false;
  }
}
function setAutoScreen(on: boolean) {
  try {
    if (on) localStorage.setItem(AUTO_SCREEN, "1");
    else localStorage.removeItem(AUTO_SCREEN);
  } catch {
    // Not remembered: the home page shows after a reconnect.
  }
}

function showHome() {
  show("home", `Connected to ${host}`, "What would you like to do?");
}

function startScreen() {
  setAutoScreen(true);
  send({ t: "screen-start" });
  show("message", `Connected to ${host}`, "Starting the screen…");
  requestWake();
}

function stopScreen() {
  setAutoScreen(false);
  send({ t: "screen-stop" });
  closePeer();
  showHome();
}

function screenInfo(): ScreenInfo {
  return { w: Math.round(window.innerWidth), h: Math.round(window.innerHeight), dpr: window.devicePixelRatio || 1 };
}

function send(m: Outbound) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(m));
}

// A device already paired uses its key even when opened with a pairing link
// (DeskSpan opens the page with one over the USB cable); the link is the
// fallback if the key stopped working.
function authNow(): Auth | null {
  if (pendingAuth) return pendingAuth;
  if (saved.id && saved.key) return { kind: "device", id: saved.id, key: saved.key };
  if (token) return { kind: "token", token };
  return null;
}

function connect() {
  leaving = false;
  welcomed = false;
  const auth = authNow();
  if (!auth) {
    show("code", "Connect to your computer", "Open DeskSpan on the computer and press Start sharing. Then type the code it shows.");
    return;
  }
  show("connecting", "Connecting…", "");
  const url = `${location.protocol === "https:" ? "wss" : "ws"}://${HOST}/ws`;
  const sock = new WebSocket(url);
  ws = sock;
  sock.onopen = () => {
    retryDelay = 1000;
    send({ t: "hello", name: deviceName(), screen: screenInfo(), auth });
  };
  sock.onmessage = (ev) => {
    if (typeof ev.data === "string") void onMessage(ev.data);
  };
  sock.onclose = () => {
    if (ws !== sock) return;
    ws = null;
    closePeer();
    if (leaving) return;
    if (welcomed) {
      // Dropped (Wi-Fi blip, computer asleep): keep trying quietly.
      show("message", "Connection lost", "Trying again…");
      window.setTimeout(connect, retryDelay);
      retryDelay = Math.min(retryDelay * 2, 15000);
    } else {
      show("message", "Can't reach the computer", "Check that DeskSpan is sharing and that this device is on the same network.", true);
    }
  };
}

async function onMessage(raw: string) {
  const m = parseInbound(raw);
  if (!m) return;
  switch (m.t) {
    case "need-code": {
      // Saved key no longer valid, or the link expired.
      const hadKey = !!(saved.id && saved.key);
      pendingAuth = null;
      saved = { name: saved.name };
      creds = null;
      save(saved);
      if (hadKey && token) {
        // Try the pairing link this page was opened with.
        send({ t: "hello", name: deviceName(), screen: screenInfo(), auth: { kind: "token", token } });
        break;
      }
      token = null;
      show("code", "Enter the code", "Open DeskSpan on the computer: the code is under the QR code.");
      break;
    }
    case "wrong-code":
      pendingAuth = null;
      show("code", "That code didn't match", m.left > 0 ? `${m.left} tries left before the computer makes a new code.` : "The computer made a new code. Type the new one.");
      break;
    case "wait":
      token = null;
      pendingAuth = null;
      show("wait", "Allow this device on the computer", `DeskSpan on the computer is asking whether "${deviceName()}" may connect.`);
      break;
    case "denied":
      show("message", "Not allowed", "The computer said no. You can ask again.", true);
      leaving = true;
      break;
    case "welcome":
      welcomed = true;
      control = m.control;
      if (m.key) {
        saved = { id: m.id, key: m.key, name: deviceName() };
        save(saved);
        creds = { id: m.id, key: m.key };
      }
      host = m.host;
      barHost.textContent = m.host;
      if (autoScreen()) startScreen();
      else showHome();
      break;
    case "offer":
      await onOffer(m.sdp);
      break;
    case "ice":
      try {
        await pc?.addIceCandidate(m.c);
      } catch (e) {
        console.warn("ice", e);
      }
      break;
    case "control":
      // Turned on or off on the computer while connected.
      control = m.on;
      break;
    case "notice":
      // E.g. why there's no picture; shown until the picture arrives.
      if (!document.body.classList.contains("live")) show("message", "No picture yet", m.text, true);
      break;
    case "bye":
      leaving = true;
      closePeer();
      show("message", "Disconnected", m.reason, true);
      break;
  }
}

async function onOffer(sdp: string) {
  closePeer();
  // Same network only: no STUN/TURN servers, nothing leaves the network.
  const peer = new RTCPeerConnection({ iceServers: [] });
  pc = peer;
  peer.onicecandidate = (ev) => {
    if (ev.candidate) send({ t: "ice", c: ev.candidate.toJSON() });
  };
  peer.ontrack = (ev) => {
    const stream = ev.streams[0] ?? new MediaStream([ev.track]);
    if (video.srcObject !== stream) video.srcObject = stream;
    void video.play().catch(() => undefined);
    show("live");
  };
  peer.ondatachannel = (ev) => {
    if (ev.channel.label === "input") input = ev.channel;
  };
  peer.onconnectionstatechange = () => {
    if (peer.connectionState === "failed") show("message", "The picture stopped", "Reconnecting…");
  };
  try {
    await peer.setRemoteDescription({ type: "offer", sdp });
    const answer = await peer.createAnswer();
    await peer.setLocalDescription(answer);
    send({ t: "answer", sdp: peer.localDescription?.sdp ?? answer.sdp ?? "" });
  } catch (e) {
    console.warn("offer", e);
    show("message", "This browser can't show the picture", "Try a current version of Safari, Chrome, Edge or Firefox.", true);
  }
}

function closePeer() {
  input = null;
  if (pc) {
    pc.onicecandidate = null;
    pc.ontrack = null;
    pc.close();
    pc = null;
  }
  video.srcObject = null;
}

// ---------- the code form and buttons ----------

codeForm.addEventListener("submit", (ev) => {
  ev.preventDefault();
  const code = codeInput.value.replace(/\D/g, "");
  if (code.length !== 6) {
    codeInput.focus();
    return;
  }
  saved = { ...saved, name: nameInput.value.trim().slice(0, 60) || deviceName() };
  save(saved);
  codeInput.value = "";
  pendingAuth = { kind: "code", code };
  if (ws && ws.readyState === WebSocket.OPEN) {
    show("connecting", "Checking the code…");
    send({ t: "hello", name: deviceName(), screen: screenInfo(), auth: pendingAuth });
  } else {
    connect();
  }
});

retry.addEventListener("click", () => {
  ws?.close();
  ws = null;
  connect();
});

goScreen.addEventListener("click", startScreen);
homeBtn.addEventListener("click", stopScreen);
goSend.addEventListener("click", () => show("send", `Send to ${host}`, "Files go to its Downloads folder."));
sendBack.addEventListener("click", showHome);
filesInput.addEventListener("change", () => void sendFiles(filesInput));
cameraInput.addEventListener("change", () => void sendFiles(cameraInput));

// ---------- send files (DeskLab Link) ----------

/** One file to /api/send, with progress. Resolves with the saved name. */
function upload(file: File, bar: HTMLProgressElement): Promise<string> {
  return new Promise((resolve, reject) => {
    if (!creds) {
      reject(new Error("This device isn't paired any more. Reconnect with a code."));
      return;
    }
    const xhr = new XMLHttpRequest();
    xhr.open("POST", `http://${HOST}/api/send?name=${encodeURIComponent(file.name || "photo.jpg")}`);
    xhr.setRequestHeader("X-DeskSpan-Device", creds.id);
    xhr.setRequestHeader("X-DeskSpan-Key", creds.key);
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable) bar.value = e.loaded / e.total;
    };
    xhr.onload = () => {
      if (xhr.status === 200) {
        try {
          resolve((JSON.parse(xhr.responseText) as { saved?: string }).saved ?? file.name);
        } catch {
          resolve(file.name);
        }
      } else reject(new Error(xhr.responseText || `failed (${xhr.status})`));
    };
    xhr.onerror = () => reject(new Error("the connection broke off"));
    xhr.send(file);
  });
}

let sending = false;
async function sendFiles(picker: HTMLInputElement) {
  const files = Array.from(picker.files ?? []);
  picker.value = "";
  if (files.length === 0 || sending) return;
  sending = true;
  let sent = 0;
  for (const file of files) {
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.className = "name";
    name.textContent = file.name || "photo";
    const bar = document.createElement("progress");
    bar.max = 1;
    bar.value = 0;
    li.append(name, bar);
    sendList.prepend(li);
    try {
      const saved = await upload(file, bar);
      bar.remove();
      name.textContent = `${saved} ✓`;
      name.classList.add("done");
      sent++;
    } catch (e) {
      bar.remove();
      name.textContent = `${file.name}: ${e instanceof Error ? e.message : "not sent"}`;
      name.classList.add("failed");
    }
  }
  sending = false;
  if (sent > 0) send({ t: "sent", count: sent });
}

leaveBtn.addEventListener("click", () => {
  leaving = true;
  ws?.close();
  ws = null;
  closePeer();
  show("message", "Disconnected", "This device is still allowed: tap Try again to reconnect.", true);
});

soundBtn.addEventListener("click", () => {
  // Browsers only start sound after a tap.
  video.muted = !video.muted;
  soundBtn.textContent = video.muted ? "Sound on" : "Sound off";
  void video.play().catch(() => undefined);
});

// ---------- full screen ----------

type FsElement = HTMLElement & { webkitRequestFullscreen?: () => void };
const canFullScreen = () => {
  const el = document.documentElement as FsElement;
  return !!(el.requestFullscreen || el.webkitRequestFullscreen);
};
const inFullScreen = () =>
  !!document.fullscreenElement || window.matchMedia("(display-mode: fullscreen), (display-mode: standalone)").matches;

function enterFullScreen() {
  const el = document.documentElement as FsElement;
  if (el.requestFullscreen) void el.requestFullscreen({ navigationUI: "hide" }).catch(() => undefined);
  else el.webkitRequestFullscreen?.();
}

/** Browsers only go full screen after a tap, so offer a big button. */
let fsSkipped = false;
function offerFullScreen() {
  fsPrompt.hidden = fsSkipped || !canFullScreen() || inFullScreen();
}

fsGo.addEventListener("click", () => {
  fsPrompt.hidden = true;
  enterFullScreen();
});
fsSkip.addEventListener("click", () => {
  fsSkipped = true;
  fsPrompt.hidden = true;
});
fsPrompt.addEventListener("pointerdown", (e) => e.stopPropagation());
// Left full screen (swipe, back button): offer it again.
document.addEventListener("fullscreenchange", () => {
  if (document.body.classList.contains("live") && !document.fullscreenElement) offerFullScreen();
});

fullBtn.addEventListener("click", () => {
  if (document.fullscreenElement) void document.exitFullscreen().catch(() => undefined);
  else enterFullScreen();
});

// The toolbar shows on a three-finger tap or a mouse move to the top edge,
// so normal touches go to the computer.
let barTimer = 0;
function flashBar() {
  bar.hidden = false;
  window.clearTimeout(barTimer);
  barTimer = window.setTimeout(() => (bar.hidden = true), 4000);
}
bar.addEventListener("pointerdown", (e) => e.stopPropagation());

// ---------- screen size, wake lock ----------

let resizeTimer = 0;
window.addEventListener("resize", () => {
  window.clearTimeout(resizeTimer);
  resizeTimer = window.setTimeout(() => send({ t: "screen", ...screenInfo() }), 300);
});

type WakeLockLike = { release: () => Promise<void> };
let wake: WakeLockLike | null = null;
function requestWake() {
  const nav = navigator as Navigator & { wakeLock?: { request: (t: "screen") => Promise<WakeLockLike> } };
  if (!nav.wakeLock || wake) return;
  nav.wakeLock
    .request("screen")
    .then((l) => (wake = l))
    .catch(() => undefined);
}
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") {
    wake = null;
    if (welcomed) requestWake();
  }
});

// ---------- input back to the computer ----------

function sendInput(ev: InputEvent) {
  if (control && input && input.readyState === "open") input.send(JSON.stringify(ev));
}

const POINTER: Record<string, "t" | "p" | "m"> = { touch: "t", pen: "p", mouse: "m" };
const touches = new Set<number>();

function pointer(e: PointerEvent, kind: "d" | "m" | "u" | "c") {
  const spot = toScreen(e.clientX, e.clientY, video.getBoundingClientRect(), video.videoWidth, video.videoHeight);
  if (kind === "d" && !spot.inside) return;
  sendInput({
    k: "p",
    e: kind,
    id: e.pointerId,
    pt: POINTER[e.pointerType] ?? "m",
    x: spot.x,
    y: spot.y,
    pr: e.pressure,
    tx: e.tiltX || 0,
    ty: e.tiltY || 0,
    b: e.buttons,
  });
}

video.addEventListener("pointerdown", (e) => {
  if (e.pointerType === "touch") {
    touches.add(e.pointerId);
    if (touches.size >= 3) {
      flashBar();
      return;
    }
  }
  video.setPointerCapture(e.pointerId);
  pointer(e, "d");
});
video.addEventListener("pointermove", (e) => {
  if (e.pointerType === "mouse" && e.clientY < 8) flashBar();
  if (e.buttons || e.pointerType !== "touch") pointer(e, "m");
});
video.addEventListener("pointerup", (e) => {
  touches.delete(e.pointerId);
  pointer(e, "u");
});
video.addEventListener("pointercancel", (e) => {
  touches.delete(e.pointerId);
  pointer(e, "c");
});
video.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    const spot = toScreen(e.clientX, e.clientY, video.getBoundingClientRect(), video.videoWidth, video.videoHeight);
    sendInput({ k: "w", x: spot.x, y: spot.y, dx: e.deltaX, dy: e.deltaY });
  },
  { passive: false },
);
video.addEventListener("contextmenu", (e) => e.preventDefault());

if (!("RTCPeerConnection" in window)) {
  show("message", "This browser can't be a screen", "Open this page in a current version of Safari, Chrome, Edge or Firefox.");
} else {
  connect();
}
