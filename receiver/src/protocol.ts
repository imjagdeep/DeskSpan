// Messages between the receiver page and DeskSpan on the computer
// (src-tauri/src/server.rs). JSON text over a WebSocket at /ws; video, sound
// and input then go over WebRTC.

export interface ScreenInfo {
  /** CSS pixels */
  w: number;
  h: number;
  /** device pixels per CSS pixel */
  dpr: number;
}

export type Auth =
  | { kind: "token"; token: string }
  | { kind: "code"; code: string }
  | { kind: "device"; id: string; key: string };

/** Receiver → computer */
export type Outbound =
  | { t: "hello"; name: string; screen: ScreenInfo; auth: Auth }
  | { t: "answer"; sdp: string }
  | { t: "ice"; c: RTCIceCandidateInit }
  | ({ t: "screen" } & ScreenInfo);

/** Computer → receiver */
export type Inbound =
  | { t: "need-code" }
  | { t: "wrong-code"; left: number }
  | { t: "wait" }
  | { t: "denied" }
  | { t: "welcome"; id: string; key?: string; host: string; control: boolean }
  | { t: "offer"; sdp: string }
  | { t: "ice"; c: RTCIceCandidateInit }
  | { t: "bye"; reason: string };

const KINDS = new Set(["need-code", "wrong-code", "wait", "denied", "welcome", "offer", "ice", "bye"]);

/** A message from the computer, or null if it isn't one we know. */
export function parseInbound(text: string): Inbound | null {
  let m: unknown;
  try {
    m = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof m !== "object" || m === null) return null;
  const t = (m as { t?: unknown }).t;
  if (typeof t !== "string" || !KINDS.has(t)) return null;
  const o = m as Record<string, unknown>;
  switch (t) {
    case "wrong-code":
      return typeof o.left === "number" ? (o as unknown as Inbound) : null;
    case "welcome":
      return typeof o.id === "string" && typeof o.host === "string" && typeof o.control === "boolean" && (o.key === undefined || typeof o.key === "string")
        ? (o as unknown as Inbound)
        : null;
    case "offer":
      return typeof o.sdp === "string" ? (o as unknown as Inbound) : null;
    case "ice":
      return typeof o.c === "object" && o.c !== null ? (o as unknown as Inbound) : null;
    case "bye":
      return typeof o.reason === "string" ? (o as unknown as Inbound) : null;
    default:
      return o as unknown as Inbound;
  }
}

/** Input sent back over the "input" data channel (only when allowed). */
export type InputEvent =
  /** pointer: d(own) m(ove) u(p) c(ancel); x/y 0..1 of the picture */
  | { k: "p"; e: "d" | "m" | "u" | "c"; id: number; pt: "t" | "p" | "m"; x: number; y: number; pr: number; tx: number; ty: number; b: number }
  /** wheel / two-finger scroll, in CSS pixels */
  | { k: "w"; x: number; y: number; dx: number; dy: number };

/** "iPad", "Android tablet", "Windows PC"… a starting name the user can change. */
export function guessDeviceName(ua: string, touchPoints: number): string {
  if (/iPad/.test(ua) || (/Macintosh/.test(ua) && touchPoints > 1)) return "iPad";
  if (/iPhone/.test(ua)) return "iPhone";
  if (/Android/.test(ua)) return /Mobile/.test(ua) ? "Android phone" : "Android tablet";
  if (/CrOS/.test(ua)) return "Chromebook";
  if (/Windows/.test(ua)) return "Windows PC";
  if (/Macintosh/.test(ua)) return "Mac";
  if (/Linux/.test(ua)) return "Linux computer";
  return "Browser";
}
