import { describe, expect, it, vi } from "vitest";

// hub.ts imports the Tauri event API; nothing here talks to it.
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const { mmss, since } = await import("./hub");

describe("hub helpers", () => {
  it("formats the code countdown", () => {
    expect(mmss(600)).toBe("10:00");
    expect(mmss(61)).toBe("1:01");
    expect(mmss(-3)).toBe("0:00");
  });

  it("says when a device was last seen", () => {
    const now = 1_800_000_000_000;
    const t = now / 1000;
    expect(since(t - 30, now)).toBe("just now");
    expect(since(t - 600, now)).toBe("10 minutes ago");
    expect(since(t - 3600, now)).toBe("an hour ago");
    expect(since(t - 86400, now)).toBe("yesterday");
    expect(since(t - 5 * 86400, now)).toBe("5 days ago");
  });
});
