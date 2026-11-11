// Picture settings (src-tauri/src/quality.rs): stream size, frame rate and
// the battery saver. A default in Settings; each device may pick a preset.

export type PictureSize = "full" | "balanced" | "light";

export interface Picture {
  size: PictureSize;
  /** 30, 60 or 120 */
  fps: number;
  saver: boolean;
}

export const DEFAULT_PICTURE: Picture = { size: "balanced", fps: 30, saver: false };

export type PresetId = "default" | "sharp" | "smooth" | "smoothest" | "saver";

/** What the Devices page offers; "default" follows Settings. */
export const PRESETS: { id: PresetId; label: string; picture: Picture | null }[] = [
  { id: "default", label: "Picture: same as Settings", picture: null },
  { id: "sharp", label: "Sharpest text", picture: { size: "full", fps: 30, saver: false } },
  { id: "smooth", label: "Smooth · 60 fps", picture: { size: "balanced", fps: 60, saver: false } },
  { id: "smoothest", label: "Smoothest · 120 fps", picture: { size: "balanced", fps: 120, saver: false } },
  { id: "saver", label: "Battery saver", picture: { size: "light", fps: 30, saver: true } },
];

const same = (a: Picture, b: Picture) => a.size === b.size && a.fps === b.fps && a.saver === b.saver;

/** The preset a device's setting matches, or "custom". */
export function presetOf(p: Picture | null): PresetId | "custom" {
  if (p === null) return "default";
  return PRESETS.find((x) => x.picture && same(x.picture, p))?.id ?? "custom";
}

/** "Balanced · 30 fps", "Full size · 60 fps · battery saver" */
export function describe(p: Picture): string {
  const size = p.size === "full" ? "Full size" : p.size === "light" ? "Light" : "Balanced";
  return `${size} · ${p.fps} fps${p.saver ? " · battery saver" : ""}`;
}
