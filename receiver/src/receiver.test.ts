import { describe, expect, it } from "vitest";
import { pictureRect, toScreen } from "./geometry";
import { guessDeviceName, parseInbound } from "./protocol";

describe("geometry", () => {
  const box = { left: 0, top: 0, width: 1000, height: 1000 };

  it("letterboxes a wide picture in a square box", () => {
    expect(pictureRect(box, 1920, 1080)).toEqual({ left: 0, top: 218.75, width: 1000, height: 562.5 });
  });

  it("maps corners and the middle", () => {
    expect(toScreen(0, 218.75, box, 1920, 1080)).toEqual({ x: 0, y: 0, inside: true });
    expect(toScreen(500, 500, box, 1920, 1080)).toEqual({ x: 0.5, y: 0.5, inside: true });
    expect(toScreen(1000, 781.25, box, 1920, 1080)).toEqual({ x: 1, y: 1, inside: true });
  });

  it("clamps touches on the black bars", () => {
    expect(toScreen(500, 10, box, 1920, 1080)).toEqual({ x: 0.5, y: 0, inside: false });
  });

  it("copes with no picture yet", () => {
    const s = toScreen(10, 10, box, 0, 0);
    expect(s.inside).toBe(true);
    expect(s.x).toBeCloseTo(0.01);
  });
});

describe("protocol", () => {
  it("accepts known messages and rejects the rest", () => {
    expect(parseInbound('{"t":"wait"}')).toEqual({ t: "wait" });
    expect(parseInbound('{"t":"welcome","id":"a","host":"pc","control":false}')).toMatchObject({ t: "welcome", host: "pc" });
    expect(parseInbound('{"t":"welcome","id":"a","host":"pc"}')).toBeNull();
    expect(parseInbound('{"t":"wrong-code","left":"3"}')).toBeNull();
    expect(parseInbound('{"t":"steal-keys"}')).toBeNull();
    expect(parseInbound("not json")).toBeNull();
    expect(parseInbound("null")).toBeNull();
  });

  it("guesses a device name", () => {
    expect(guessDeviceName("Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X)", 5)).toBe("iPad");
    // iPadOS asks for the desktop site and says Macintosh.
    expect(guessDeviceName("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)", 5)).toBe("iPad");
    expect(guessDeviceName("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)", 0)).toBe("Mac");
    expect(guessDeviceName("Mozilla/5.0 (Linux; Android 14; Pixel 8) Mobile", 5)).toBe("Android phone");
    expect(guessDeviceName("Mozilla/5.0 (Linux; Android 14; SM-X710)", 5)).toBe("Android tablet");
    expect(guessDeviceName("Mozilla/5.0 (Windows NT 10.0; Win64; x64)", 0)).toBe("Windows PC");
  });
});
