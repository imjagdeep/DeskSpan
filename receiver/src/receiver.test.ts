import { describe, expect, it } from "vitest";
import { pictureRect, toScreen } from "./geometry";
import { guessDeviceName, parseInbound } from "./protocol";
import { label, summarize } from "./stats";

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
    expect(parseInbound('{"t":"notice","text":"No encoder"}')).toEqual({ t: "notice", text: "No encoder" });
    expect(parseInbound('{"t":"notice"}')).toBeNull();
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

describe("stats", () => {
  const pair = { type: "candidate-pair", nominated: true, state: "succeeded", currentRoundTripTime: 0.01 };
  const v1 = { type: "inbound-rtp", kind: "video", framesPerSecond: 29.6, jitterBufferDelay: 2, jitterBufferEmittedCount: 100, framesDecoded: 100, totalDecodeTime: 0.5 };
  const v2 = { ...v1, jitterBufferDelay: 3.2, jitterBufferEmittedCount: 130, framesDecoded: 130, totalDecodeTime: 0.62 };

  it("adds network, buffer and decoding since the last report", () => {
    const first = summarize([pair, v1], null);
    const { summary } = summarize([pair, v2], first.video);
    // 5 ms network + 1.2 s / 30 frames = 40 ms buffer + 0.12 s / 30 = 4 ms decoding
    expect(summary).toEqual({ fps: 30, delay: 49 });
    expect(label(summary)).toBe("30 fps · 49 ms");
  });

  it("says nothing before there is video", () => {
    expect(summarize([pair], null).summary).toEqual({ fps: null, delay: null });
    expect(label({ fps: null, delay: null })).toBe("");
  });
});
