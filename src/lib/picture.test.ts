import { describe as suite, expect, it } from "vitest";
import { DEFAULT_PICTURE, PRESETS, describe, presetOf } from "./picture";

suite("picture presets", () => {
  it("matches a device's setting to its preset", () => {
    expect(presetOf(null)).toBe("default");
    for (const p of PRESETS) if (p.picture) expect(presetOf({ ...p.picture })).toBe(p.id);
    expect(presetOf({ size: "light", fps: 120, saver: false })).toBe("custom");
  });

  it("describes a setting in words", () => {
    expect(describe(DEFAULT_PICTURE)).toBe("Balanced · 30 fps");
    expect(describe({ size: "full", fps: 60, saver: true })).toBe("Full size · 60 fps · battery saver");
  });
});
