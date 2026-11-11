// "30 fps · 42 ms": how smooth and how far behind the picture is, from the
// browser's own WebRTC statistics. The delay is the part this device can
// measure: half the network round trip, the jitter buffer, and decoding
// (capture and encoding on the computer add a little more).

export interface Summary {
  fps: number | null;
  /** milliseconds */
  delay: number | null;
}

export type Stat = Record<string, unknown> & { type: string };

const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

/** `prev` is the last report's video entry, for per-second jitter/decoding figures. */
export function summarize(stats: Iterable<Stat>, prev: Stat | null): { summary: Summary; video: Stat | null } {
  let video: Stat | null = null;
  let rtt: number | null = null;
  for (const s of stats) {
    if (s.type === "inbound-rtp" && s.kind === "video") video = s;
    if (s.type === "candidate-pair" && (s.nominated === true || s.selected === true) && s.state === "succeeded") {
      rtt = num(s.currentRoundTripTime) ?? rtt;
    }
  }
  if (!video) return { summary: { fps: null, delay: null }, video: null };

  // Differences since the last report, so the figure follows what's happening now.
  const delta = (key: string) => {
    const now = num(video?.[key]);
    const before = prev ? num(prev[key]) : 0;
    return now === null || before === null ? null : now - before;
  };
  const emitted = delta("jitterBufferEmittedCount");
  const buffer = delta("jitterBufferDelay");
  const decoded = delta("framesDecoded");
  const decodeTime = delta("totalDecodeTime");

  let delay = 0;
  let known = false;
  if (rtt !== null) {
    delay += (rtt / 2) * 1000;
    known = true;
  }
  if (emitted && buffer !== null && emitted > 0) {
    delay += (buffer / emitted) * 1000;
    known = true;
  }
  if (decoded && decodeTime !== null && decoded > 0) {
    delay += (decodeTime / decoded) * 1000;
    known = true;
  }
  const fps = num(video.framesPerSecond);
  return {
    summary: { fps: fps === null ? null : Math.round(fps), delay: known ? Math.round(delay) : null },
    video,
  };
}

export function label(s: Summary): string {
  const parts: string[] = [];
  if (s.fps !== null) parts.push(`${s.fps} fps`);
  if (s.delay !== null) parts.push(`${s.delay} ms`);
  return parts.join(" · ");
}
