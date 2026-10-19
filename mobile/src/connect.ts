// DeskSpan Screen: find the computer, then open the receiver page for it.
// The QR code holds the computer's link (http://<ip>:<port>/#t=<one-time
// token>); typed addresses connect without a token and ask for the code.

import "../../receiver/src/style.css";
import "./connect.css";
import { Format, checkPermissions, requestPermissions, scan } from "@tauri-apps/plugin-barcode-scanner";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const RECENT = "deskspan-recent";

/** "192.168.1.20:47800" from a link or a typed address; null if not one. */
export function hostOf(text: string): string | null {
  const t = text.trim();
  let host = t;
  try {
    if (/^https?:\/\//i.test(t)) host = new URL(t).host;
  } catch {
    return null;
  }
  host = host.replace(/\/.*$/, "");
  if (!/^[\w.-]+(:\d{1,5})?$/.test(host) && !/^\[[0-9a-f:]+\](:\d{1,5})?$/i.test(host)) return null;
  return host.includes(":") && !host.startsWith("[") ? host : `${host}:47800`;
}

function tokenOf(text: string): string | null {
  const m = /#t=([0-9a-f]{16,64})/.exec(text);
  return m ? m[1] : null;
}

function recent(): string[] {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(RECENT) ?? "[]");
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string").slice(0, 5) : [];
  } catch {
    return [];
  }
}

function remember(host: string) {
  try {
    localStorage.setItem(RECENT, JSON.stringify([host, ...recent().filter((h) => h !== host)].slice(0, 5)));
  } catch {
    // Not remembered; it still connects.
  }
}

function open(host: string, token: string | null) {
  remember(host);
  location.href = `screen.html?host=${encodeURIComponent(host)}${token ? `#t=${token}` : ""}`;
}

function showError(text: string) {
  const e = $("error");
  e.textContent = text;
  e.hidden = false;
}

$("scan").addEventListener("click", async () => {
  try {
    let perm = await checkPermissions();
    if (perm !== "granted") perm = await requestPermissions();
    if (perm !== "granted") {
      showError("DeskSpan needs the camera to read the QR code. You can type the address instead.");
      return;
    }
    const found = await scan({ formats: [Format.QRCode], windowed: false });
    const host = hostOf(found.content);
    if (!host) {
      showError("That isn't a DeskSpan QR code.");
      return;
    }
    open(host, tokenOf(found.content));
  } catch (e) {
    showError(`Couldn't scan: ${e instanceof Error ? e.message : String(e)}`);
  }
});

$<HTMLFormElement>("manual").addEventListener("submit", (ev) => {
  ev.preventDefault();
  const text = $<HTMLInputElement>("address").value;
  const host = hostOf(text);
  if (!host) {
    showError("Type the address DeskSpan shows, like 192.168.1.20:47800.");
    return;
  }
  open(host, tokenOf(text));
});

const list = recent();
if (list.length > 0) {
  $("recent-box").hidden = false;
  for (const host of list) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "tile";
    b.innerHTML = "<strong></strong><span>Connect again</span>";
    (b.querySelector("strong") as HTMLElement).textContent = host;
    b.addEventListener("click", () => open(host, null));
    $("recent").append(b);
  }
}
