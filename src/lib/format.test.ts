import { describe, expect, it } from "vitest";
import { dayGroup, formatBytes, formatCount, formatEta, formatParams } from "./format";

describe("format", () => {
  it("formats sizes in decimal units", () => {
    expect(formatBytes(2_740_937_888)).toBe("2.7 GB");
    expect(formatBytes(672_423_616)).toBe("672 MB");
    expect(formatBytes(null)).toBe("—");
  });
  it("formats counts and params", () => {
    expect(formatCount(1_234_567)).toBe("1.2M");
    expect(formatCount(999)).toBe("999");
    expect(formatParams(27_320_697_856)).toBe("27B params");
    expect(formatParams(4_000_000_000)).toBe("4.0B params");
    expect(formatParams(null)).toBeNull();
  });
  it("estimates time left", () => {
    expect(formatEta(600e6, 10e6)).toBe("1 min left");
    expect(formatEta(30e6, 1e6)).toBe("30 s left");
    expect(formatEta(1, 0)).toBeNull();
  });
});

describe("dayGroup", () => {
  const now = new Date(2026, 10, 11, 15, 30).getTime();
  const at = (y: number, m: number, d: number, h = 12) => new Date(y, m, d, h).getTime();
  it("puts a chat under the day it was last used", () => {
    expect(dayGroup(at(2026, 10, 11, 0), now)).toBe("Today");
    expect(dayGroup(now, now)).toBe("Today");
    expect(dayGroup(at(2026, 10, 10, 23), now)).toBe("Yesterday");
    expect(dayGroup(at(2026, 10, 10, 0), now)).toBe("Yesterday");
    expect(dayGroup(at(2026, 10, 9, 23), now)).toBe("This week");
    expect(dayGroup(at(2026, 10, 5, 1), now)).toBe("This week");
    expect(dayGroup(at(2026, 10, 4, 23), now)).toBe("Earlier");
    expect(dayGroup(0, now)).toBe("Earlier");
  });
});
