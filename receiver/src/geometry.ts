// Where a touch lands on the computer's screen. The picture is shown with
// `object-fit: contain`, so it may have black bars on two sides.

export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface Spot {
  /** 0..1 across the computer's screen (clamped) */
  x: number;
  y: number;
  /** false when the finger is on a black bar */
  inside: boolean;
}

/** The rectangle the picture really fills inside `box`. */
export function pictureRect(box: Box, videoW: number, videoH: number): Box {
  if (videoW <= 0 || videoH <= 0 || box.width <= 0 || box.height <= 0) return { ...box };
  // The side that fills the box is taken as is, so edges stay exact.
  const wide = box.width / box.height > videoW / videoH;
  const width = wide ? (box.height * videoW) / videoH : box.width;
  const height = wide ? box.height : (box.width * videoH) / videoW;
  return {
    left: box.left + (box.width - width) / 2,
    top: box.top + (box.height - height) / 2,
    width,
    height,
  };
}

export function toScreen(clientX: number, clientY: number, box: Box, videoW: number, videoH: number): Spot {
  const r = pictureRect(box, videoW, videoH);
  const x = (clientX - r.left) / r.width;
  const y = (clientY - r.top) / r.height;
  // A hair of slack for rounding at the very edge.
  const e = 1e-9;
  const inside = x >= -e && x <= 1 + e && y >= -e && y <= 1 + e;
  const clamp = (v: number) => (Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0);
  return { x: clamp(x), y: clamp(y), inside };
}
