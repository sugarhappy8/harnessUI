import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  GOOD_AT_KEYS,
  GOOD_AT_WORDS,
  PIPELINE_WORDS,
  TAG_WORDS,
  capabilityAnswer,
  fileWords,
  goodAtSentence,
  goodAtWords,
  languageWords,
  licenseWords,
  parameterWords,
  pipelineWords,
  quantFromFile,
  quantWords,
  sizeAnswer,
  tagWords,
} from "./abilities";

describe("what a model is good at, in plain words", () => {
  it("has a phrase for every key of the catalog's vocabulary", () => {
    expect(GOOD_AT_KEYS).toEqual([
      "chat",
      "writing",
      "coding",
      "reasoning",
      "math",
      "tools",
      "images",
      "long-documents",
      "languages",
      "speed",
    ]);
    for (const key of GOOD_AT_KEYS) {
      const phrase = GOOD_AT_WORDS[key];
      expect(phrase.length).toBeGreaterThan(2);
      expect(phrase).not.toContain("-");
      expect(phrase).not.toMatch(/!/);
    }
  });

  it("drops unknown keys and repeats instead of showing them raw", () => {
    expect(goodAtWords(["coding", "jailbreaks", "chat", "coding"])).toEqual(["coding", "everyday chat"]);
    expect(goodAtWords(["<b>x</b>"])).toEqual([]);
  });

  it("makes one sentence", () => {
    expect(goodAtSentence(["chat"])).toBe("Good at everyday chat.");
    expect(goodAtSentence(["chat", "coding"])).toBe("Good at everyday chat and coding.");
    expect(goodAtSentence(["chat", "coding", "long-documents"])).toBe(
      "Good at everyday chat, coding and long documents.",
    );
    expect(goodAtSentence(["nothing-known"])).toBeNull();
    expect(goodAtSentence([])).toBeNull();
  });
});

describe("what the uploader lists, in plain words", () => {
  it("maps pipeline tags and drops unknown ones", () => {
    expect(pipelineWords("text-generation")).toBe("writes text");
    expect(pipelineWords("image-text-to-text")).toBe("reads images and writes text");
    expect(pipelineWords("not-a-pipeline")).toBeNull();
    expect(pipelineWords(null)).toBeNull();
    for (const phrase of Object.values(PIPELINE_WORDS)) expect(phrase).toBe(phrase.toLowerCase());
  });

  it("maps tags that say what a model does; format tags and unknown tags say nothing", () => {
    expect(tagWords(["gguf", "conversational", "imatrix", "code", "region:us", "conversational"])).toEqual([
      "made for chat",
      "coding",
    ]);
    expect(tagWords(["gguf", "quantized", "llama.cpp", "base_model:quantized:x/y"])).toEqual([]);
    expect(tagWords(["\u202Eevil"])).toEqual([]);
    for (const phrase of Object.values(TAG_WORDS)) expect(phrase).toBe(phrase.toLowerCase());
  });
});

describe("file names in plain words", () => {
  it("names every common quantisation", () => {
    expect(quantWords("Q4_K_M")).toBe("smaller file, good quality");
    expect(quantWords("UD-Q4_K_XL")).toBe("smaller file, good quality");
    expect(quantWords("Q8_0")).toBe("large file, almost the original quality");
    expect(quantWords("Q6_K")).toBe("large file, very close to the original quality");
    expect(quantWords("Q5_K_S")).toBe("medium file, very good quality");
    expect(quantWords("IQ3_XXS")).toBe("small file, lower quality");
    expect(quantWords("Q2_K")).toBe("very small file, noticeably lower quality");
    expect(quantWords("IQ1_S")).toBe("tiny file, much lower quality");
    expect(quantWords("BF16")).toBe("full size, the original quality");
    expect(quantWords("MXFP4")).toBe("smaller file, good quality");
    expect(quantWords("WHATEVER")).toBeNull();
    expect(quantWords(null)).toBeNull();
  });

  it("finds the quantisation in a file name, the last one winning", () => {
    expect(quantFromFile("Alpha-8B-Q4_K_M.gguf")).toBe("Q4_K_M");
    expect(quantFromFile("sub/Alpha-8B-UD-Q4_K_XL.gguf")).toBe("UD-Q4_K_XL");
    expect(quantFromFile("Alpha-Q2_K-then-Q8_0.gguf")).toBe("Q8_0");
    expect(quantFromFile("Alpha-BF16-00001-of-00003.gguf")).toBe("BF16");
    expect(quantFromFile("alpha-fp16.gguf")).toBe("F16");
    expect(quantFromFile("alpha.gguf")).toBeNull();
    expect(quantFromFile(null)).toBeNull();
  });

  it("puts the name first, as the column shows it", () => {
    expect(fileWords("Q4_K_M")).toBe("Q4_K_M: smaller file, good quality");
    expect(fileWords("ODD")).toBe("ODD");
    expect(fileWords(null)).toBeNull();
  });
});

describe("how big a model is", () => {
  it("says parameters in words", () => {
    expect(parameterWords(9_000_000_000)).toBe("9 billion");
    expect(parameterWords(8_030_000_000)).toBe("8 billion");
    expect(parameterWords(1_540_000_000)).toBe("1.5 billion");
    expect(parameterWords(35_400_000_000)).toBe("35 billion");
    expect(parameterWords(596_000_000)).toBe("596 million");
    expect(parameterWords(1_200_000_000_000)).toBe("1.2 trillion");
    expect(parameterWords(12_345)).toBe("12,345");
  });

  it("uses the exact sentences of the specification", () => {
    expect(sizeAnswer(9e9, null, false)).toBe("9 billion parameters. Bigger models are usually smarter but slower.");
    expect(sizeAnswer(35e9, 3e9, true)).toBe(
      "35 billion, using 3 billion at a time, so it runs faster than its size suggests.",
    );
    expect(sizeAnswer(35e9, null, true)).toBe(
      "35 billion parameters, using only part of them at a time, so it runs faster than its size suggests.",
    );
    // Known active parameters mean a mixture of experts even when the file is not known.
    expect(sizeAnswer(35e9, 3e9, false)).toContain("using 3 billion at a time");
    expect(sizeAnswer(null, 3e9, true)).toBeNull();
  });
});

describe("capabilities and languages", () => {
  it("answers yes or no", () => {
    expect(capabilityAnswer(true)).toBe("Yes");
    expect(capabilityAnswer(false)).toBe("No");
  });

  it("names languages and leaves out codes it does not know", () => {
    expect(languageWords(["en", "fr"])).toEqual(["English", "French"]);
    expect(languageWords(["multilingual", "en", "en"])).toEqual(["Many languages", "English"]);
    expect(languageWords(["not a language", "zz-qq-1234567"])).toEqual([]);
  });
});

describe("licenses", () => {
  it("names common license ids and keeps anything else as it is written", () => {
    expect(licenseWords("apache-2.0")).toBe("Apache 2.0");
    expect(licenseWords("MIT")).toBe("MIT");
    expect(licenseWords(" cc-by-nc-4.0 ")).toBe("Creative Commons Attribution-NonCommercial 4.0 (no commercial use)");
    expect(licenseWords("other")).toBe("Another license (read the model card)");
    expect(licenseWords("some-family-license")).toBe("some-family-license");
    // Object keys are not license names.
    expect(licenseWords("constructor")).toBe("constructor");
    expect(licenseWords("__proto__")).toBe("__proto__");
  });
});

describe("the tags the backend keeps", () => {
  it("has a phrase for exactly the tags the models crate keeps (KNOWN_TAGS)", () => {
    // Uploader tags reach the screen only when crates/models/src/hf.rs keeps them; a tag
    // kept there without a phrase here would never be shown, one here only would be dead.
    const source = readFileSync(resolve(process.cwd(), "crates/models/src/hf.rs"), "utf8");
    const block = /pub const KNOWN_TAGS: \[&str; \d+\] = \[([^\]]*)\]/.exec(source)?.[1] ?? "";
    const kept = [...block.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
    expect(kept.length).toBeGreaterThan(10);
    expect([...kept].sort()).toEqual(Object.keys(TAG_WORDS).sort());
  });
});
