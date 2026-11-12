import { describe, expect, it } from "vitest";
import type { BackendKind, LoadStage } from "./types";
import {
  aboutWords,
  BACKEND_WORDS,
  backendWords,
  GPU_USE_WORDS,
  loadStageWords,
  THINKING_WORDS,
  tokensWithWords,
} from "./words";

describe("plain words (WP0 skeleton)", () => {
  it("gives token counts with words, as the specification writes them", () => {
    expect(tokensWithWords(8192)).toBe("8,192 tokens, about 6,000 words");
    expect(tokensWithWords(24_576)).toBe("24,576 tokens, about 18,000 words");
    expect(tokensWithWords(32_768)).toBe("32,768 tokens, about 24,000 words");
    expect(aboutWords(1000)).toBe(700);
    expect(aboutWords(0)).toBe(0);
  });

  it("names every backend without its identifier alone", () => {
    const kinds: BackendKind[] = ["cpu", "vulkan", "cuda", "rocm", "metal"];
    for (const k of kinds) {
      expect(BACKEND_WORDS[k]).toBeTruthy();
      expect(backendWords(k)).not.toBe(k);
    }
    expect(backendWords("vulkan")).toBe("Graphics card (Vulkan)");
    expect(backendWords("cpu")).toBe("Processor only (slower)");
    expect(GPU_USE_WORDS.fit).toBe("As much as fits");
    expect(THINKING_WORDS.noLimit).toBe("Default");
  });

  it("has a phrase for every load stage", () => {
    const stages: LoadStage[] = [
      { kind: "starting" },
      { kind: "readingModel" },
      { kind: "readingImageReader" },
      { kind: "loading" },
      { kind: "restarting", attempt: 1, max: 3 },
      { kind: "retryingWithoutSaveMemory", backend: "vulkan" },
      { kind: "retryingWithLessGpu", backend: "vulkan" },
      { kind: "steppingDown", from: "vulkan", to: "cpu", reason: { code: "out_of_memory" } },
    ];
    for (const s of stages) {
      const words = loadStageWords(s);
      expect(words).toBeTruthy();
      expect(words).not.toMatch(/_|vulkan\b|Vulkan0/);
    }
    expect(loadStageWords({ kind: "restarting", attempt: 2, max: 3 })).toContain("2 of 3");
  });
});
