// Dollars as Studio shows them (0.1.0 Task B2; Task B7 adds the rest of cost.ts).
import { describe, expect, it } from "vitest";
import { formatUsd } from "./cost";

describe("formatUsd", () => {
  it("shows cents, and less than a cent as such", () => {
    expect(formatUsd(0.16)).toBe("$0.16");
    expect(formatUsd(0.84)).toBe("$0.84");
    expect(formatUsd(12.5)).toBe("$12.50");
    expect(formatUsd(0.004)).toBe("< $0.01");
    expect(formatUsd(0)).toBe("$0.00");
  });
});
