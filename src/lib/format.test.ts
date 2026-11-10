import { describe, expect, it } from "vitest";
import { formatBytes, formatCount, formatEta, formatParams } from "./format";

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
