import { describe, expect, it } from "vitest";
import { fileName, when } from "./when";

describe("when", () => {
  const now = Date.UTC(2026, 10, 11, 12, 0, 0);
  it("says how long ago in a few words", () => {
    expect(when(now, now)).toBe("just now");
    expect(when(now - 30_000, now)).toBe("just now");
    expect(when(now - 5 * 60_000, now)).toBe("5 min ago");
    expect(when(now - 3 * 3_600_000, now)).toBe("3 h ago");
    expect(when(now - 26 * 3_600_000, now)).toBe("yesterday");
    expect(when(now - 3 * 86_400_000, now)).toBe("3 days ago");
    expect(when(now - 30 * 86_400_000, now)).toMatch(/2026/);
    // A clock that went back does not give a negative time.
    expect(when(now + 60_000, now)).toBe("just now");
  });
});

describe("fileName", () => {
  it("makes a name a file system accepts", () => {
    expect(fileName("Tip calculator", "html")).toBe("Tip calculator.html");
    expect(fileName('a/b\\c:d*e?f"g<h>i|j', "md")).toBe("a b c d e f g h i j.md");
    expect(fileName("  trailing dots... ", "svg")).toBe("trailing dots.svg");
    expect(fileName("\u0000\n\t", "mmd")).toBe("artifact.mmd");
    expect(fileName("", "md")).toBe("artifact.md");
    expect(fileName("x".repeat(300), "md")).toBe(`${"x".repeat(80)}.md`);
    expect(fileName("../../etc/passwd", "md")).toBe(".. .. etc passwd.md");
  });
});
