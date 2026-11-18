// How long a local run takes, in words (0.1.0): one figure, a first guess labelled as one.
import { describe, expect, it } from "vitest";
import { etaText } from "./eta";
import type { Eta } from "./types";

function eta(lowMs: number, highMs: number, measured = true): Eta {
  return { lowMs, highMs, measured, leftMs: null };
}

describe("etaText", () => {
  it("gives the middle of the range, rounded as people read it", () => {
    expect(etaText(eta(180_000, 300_000))).toBe("about 4 min");
    expect(etaText(eta(1_200_000, 1_800_000))).toBe("about 25 min");
    expect(etaText(eta(80_000, 100_000))).toBe("about 1 min 30 s");
    expect(etaText(eta(90_000, 120_000))).toBe("about 2 min");
    expect(etaText(eta(10_000, 15_000))).toBe("about 15 s");
    expect(etaText(eta(3_600_000, 4_200_000))).toBe("about 1 h 5 min");
    expect(etaText(eta(0, 1_000))).toBe("about 5 s");
  });

  it("labels a first guess, and says nothing without an estimate", () => {
    expect(etaText(eta(20_000, 30_000, false))).toBe("about 25 s (first guess)");
    expect(etaText(eta(180_000, 300_000, false), "a picture")).toBe("about 4 min a picture (first guess)");
    expect(etaText(eta(180_000, 300_000), "for a 2 s clip")).toBe("about 4 min for a 2 s clip");
    expect(etaText(null)).toBeNull();
  });
});
