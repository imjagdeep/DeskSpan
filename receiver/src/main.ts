// The receiver: pair with DeskSpan on the computer, then show its screen
// full size and (if allowed there) send touch, pen and mouse back.

import "./style.css";
import { toScreen } from "./geometry";
import { guessDeviceName, parseInbound, type Auth, type InputEvent, type Outbound, type ScreenInfo } from "./protocol";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
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

// ---------- saved device (browser storage; may be unavailable) ----------

interface Saved {
  id?: string;
  key?: string;
  name?: string;
}
const STORE = "deskspan-device";

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

type View = "connecting" | "code" | "wait" | "message" | "live";

function show(view: View, heading = "", body = "", canRetry = false) {
  const live = view === "live";
  if (live && !document.body.classList.contains("live")) offerFullScreen();
  if (!live) fsPrompt.hidden = true;
  document.body.classList.toggle("live", live);
  panel.hidden = live;
  codeForm.hidden = view !== "code";
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

function screenInfo(): ScreenInfo {
  return { w: Math.round(window.innerWidth), h: Math.round(window.innerHeight), dpr: window.devicePixelRatio || 1 };
}

function send(m: Outbound) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(m));
}

function authNow(): Auth | null {
  if (pendingAuth) return pendingAuth;
  if (token) return { kind: "token", token };
  if (saved.id && saved.key) return { kind: "device", id: saved.id, key: saved.key };
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
  const url = `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`;
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
    case "need-code":
      // Saved key no longer valid, or the link expired: start over with a code.
      token = null;
      pendingAuth = null;
      saved = { name: saved.name };
      save(saved);
      show("code", "Enter the code", "Open DeskSpan on the computer: the code is under the QR code.");
      break;
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
      }
      barHost.textContent = m.host;
      show("message", `Connected to ${m.host}`, "Waiting for the picture…");
      requestWake();
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
