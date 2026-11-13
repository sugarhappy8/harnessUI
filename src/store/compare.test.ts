import { afterEach, describe, expect, it } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";
import type { FitResult, RepoView, VariantView, Verdict } from "../lib/types";
import {
  COMPARE_LIMIT,
  chooseFile,
  entryKey,
  isAutoFile,
  sameEntry,
  toCompareItem,
  useAddToComparison,
  useCompare,
  useRepoInComparison,
} from "./compare";

function fit(verdict: Verdict): FitResult {
  return { verdict, neededBytes: 1, availableBytes: 2, suggestedCtx: null, partialOffload: false, device: "graphicsCard", computerBytes: 0, sentence: "" };
}

function variant(label: string, size: number, verdict: Verdict | null, parts = 1): VariantView {
  const files = Array.from({ length: parts }, (_, i) => ({
    path: parts > 1 ? `${label}/m-${label}-0000${i + 1}-of-0000${parts}.gguf` : `m-${label}.gguf`,
    size: size / parts,
    sha256: null,
  }));
  return { label, files, totalSize: size, bits: null, fit: verdict ? fit(verdict) : null, installed: false, hasChecksum: false };
}

function view(repo: string, variants: VariantView[]): RepoView {
  return {
    id: repo,
    commit: "c".repeat(40),
    gated: "no",
    license: null,
    architecture: null,
    architectureSupported: null,
    contextLength: null,
    parameters: null,
    templateTools: false,
    templateThinking: false,
    projector: null,
    variants,
    incomplete: [],
    card: null,
    fitContext: 8192,
  };
}

const reset = () => useCompare.setState({ entries: [], auto: [], ctx: null, repos: {} });

describe("the comparison list", () => {
  afterEach(() => {
    cleanup();
    reset();
  });

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

  it("sends only the item's own fields to the backend", () => {
    expect(toCompareItem({ catalogId: "x" })).toEqual({ catalogId: "x" });
    expect(Object.keys(toCompareItem({ repo: "o/r", file: "f.gguf" })!)).toEqual(["repo", "file"]);
  });

  it("merges two entries that end up naming the same file", () => {
    const { add, setFile, autoFile } = useCompare.getState();
    add({ repo: "o/r", file: "a.gguf" });
    add({ repo: "o/r", file: null });
    add({ catalogId: "c" });
    autoFile("o/r", null, "a.gguf");
    expect(useCompare.getState().entries).toEqual([{ repo: "o/r", file: "a.gguf" }, { catalogId: "c" }]);
    // The kept entry was the user's pick, so it is not marked as the app's choice.
    expect(isAutoFile(useCompare.getState(), { repo: "o/r", file: "a.gguf" })).toBe(false);
    add({ repo: "o/r", file: "b.gguf" });
    setFile("o/r", "b.gguf", "a.gguf");
    expect(useCompare.getState().entries).toHaveLength(2);
  });

  it("remembers which files the app chose, until the user changes them", () => {
    const { add, autoFile, setFile, remove } = useCompare.getState();
    add({ repo: "o/r", file: null });
    autoFile("o/r", null, "a.gguf");
    expect(isAutoFile(useCompare.getState(), { repo: "o/r", file: "a.gguf" })).toBe(true);
    autoFile("o/r", "a.gguf", "b.gguf");
    expect(isAutoFile(useCompare.getState(), { repo: "o/r", file: "b.gguf" })).toBe(true);
    expect(useCompare.getState().auto).toEqual([entryKey({ repo: "o/r", file: "b.gguf" })]);
    setFile("o/r", "b.gguf", "c.gguf");
    expect(isAutoFile(useCompare.getState(), { repo: "o/r", file: "c.gguf" })).toBe(false);
    expect(useCompare.getState().auto).toEqual([]);
    autoFile("o/r", "c.gguf", "d.gguf");
    remove({ repo: "o/r", file: "d.gguf" });
    expect(useCompare.getState().auto).toEqual([]);
  });

  it("keeps the comparison size and the repository pages it has seen", () => {
    const { setCtx, rememberRepo } = useCompare.getState();
    setCtx(32768);
    rememberRepo(view("o/r", []), 32768);
    expect(useCompare.getState().ctx).toBe(32768);
    expect(useCompare.getState().repos["o/r"]?.ctx).toBe(32768);
    useCompare.getState().clear();
    expect(useCompare.getState().entries).toEqual([]);
    expect(useCompare.getState().ctx).toBe(32768);
  });

  it("gives every entry a distinct key", () => {
    const keys = [
      { catalogId: "x" },
      { installedPath: "x" },
      { repo: "x", file: null },
      { repo: "x", file: "y" },
    ].map(entryKey);
    expect(new Set(keys).size).toBe(4);
  });
});

describe("the file a repository entry uses", () => {
  it("takes the largest file marked as fitting", () => {
    const vs = [variant("Q2_K", 2e9, "green"), variant("Q4_K_M", 4e9, "green"), variant("Q8_0", 8e9, "red")];
    expect(chooseFile(vs)).toBe("m-Q4_K_M.gguf");
  });

  it("does not count Fits with limits as fitting", () => {
    const vs = [variant("Q2_K", 2e9, "green"), variant("Q4_K_M", 4e9, "yellow")];
    expect(chooseFile(vs)).toBe("m-Q2_K.gguf");
  });

  it("takes the smallest when nothing fits or nothing is known", () => {
    expect(chooseFile([variant("Q8_0", 8e9, "red"), variant("Q4_K_M", 4e9, "yellow")])).toBe("m-Q4_K_M.gguf");
    expect(chooseFile([variant("Q8_0", 8e9, null), variant("Q3_K_M", 3e9, null)])).toBe("m-Q3_K_M.gguf");
  });

  it("names the first part of a split model", () => {
    expect(chooseFile([variant("Q8_0", 8e9, "green", 2)])).toBe("Q8_0/m-Q8_0-00001-of-00002.gguf");
  });

  it("has nothing to choose in a repository without model files", () => {
    expect(chooseFile([])).toBeNull();
  });
});

describe("Add to comparison buttons", () => {
  afterEach(() => {
    cleanup();
    reset();
  });

  it("knows when an entry is in the list and when the list is full", () => {
    const { result } = renderHook(() => useAddToComparison({ installedPath: "/m/a.gguf" }));
    expect(result.current).toMatchObject({ added: false, full: false });
    act(() => void result.current.add());
    expect(result.current).toMatchObject({ added: true, full: false });
    act(() => {
      for (const id of ["b", "c", "d"]) useCompare.getState().add({ catalogId: id });
    });
    // In the list: never "full" for its own button.
    expect(result.current.full).toBe(false);
    act(() => result.current.remove());
    expect(result.current).toMatchObject({ added: false, full: false });
    act(() => void useCompare.getState().add({ catalogId: "e" }));
    expect(result.current).toMatchObject({ added: false, full: true });
    let ok = true;
    act(() => {
      ok = result.current.add();
    });
    expect(ok).toBe(false);
  });

  it("treats a repository as added when any of its files is", () => {
    const { result } = renderHook(() => useRepoInComparison("o/r"));
    expect(result.current.added).toBe(false);
    act(() => void useCompare.getState().add({ repo: "o/r", file: "a.gguf" }));
    expect(result.current.added).toBe(true);
    act(() => void useCompare.getState().add({ repo: "o/r", file: "b.gguf" }));
    act(() => result.current.remove());
    expect(useCompare.getState().entries).toEqual([]);
    act(() => void result.current.add());
    expect(useCompare.getState().entries).toEqual([{ repo: "o/r", file: null }]);
  });
});
