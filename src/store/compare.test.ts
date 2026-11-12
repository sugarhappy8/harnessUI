import { afterEach, describe, expect, it } from "vitest";
import { COMPARE_LIMIT, sameEntry, toCompareItem, useCompare } from "./compare";

describe("the comparison list (WP0 stub)", () => {
  afterEach(() => useCompare.getState().clear());

  it("adds up to four models, once each", () => {
    const { add } = useCompare.getState();
    expect(add({ catalogId: "a" })).toBe(true);
    expect(add({ catalogId: "a" })).toBe(false);
    expect(add({ installedPath: "/m/b.gguf" })).toBe(true);
    expect(add({ repo: "o/r", file: "x.gguf" })).toBe(true);
    expect(add({ repo: "o/r", file: "y.gguf" })).toBe(true);
    expect(useCompare.getState().entries).toHaveLength(COMPARE_LIMIT);
    expect(add({ catalogId: "e" })).toBe(false);
    useCompare.getState().remove({ catalogId: "a" });
    expect(useCompare.getState().entries).toHaveLength(3);
    expect(add({ catalogId: "e" })).toBe(true);
  });

  it("chooses a repository's file later", () => {
    const { add, setFile } = useCompare.getState();
    add({ repo: "o/r", file: null });
    expect(toCompareItem(useCompare.getState().entries[0]!)).toBeNull();
    setFile("o/r", null, "q4.gguf");
    expect(toCompareItem(useCompare.getState().entries[0]!)).toEqual({ repo: "o/r", file: "q4.gguf" });
    expect(toCompareItem({ installedPath: "/m.gguf" })).toEqual({ installedPath: "/m.gguf" });
    expect(sameEntry({ repo: "o/r", file: "a" }, { repo: "o/r", file: "b" })).toBe(false);
    expect(sameEntry({ catalogId: "x" }, { installedPath: "x" })).toBe(false);
  });
});
