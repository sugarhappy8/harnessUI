// What is kept of an error that stopped part of a screen from being drawn (0.2.1).
import { describe, expect, it } from "vitest";
import { cleanMessage, crashSummary, MESSAGE_LIMIT } from "./crash";

describe("cleanMessage", () => {
  it("keeps a plain message as it is", () => {
    const tt = "Failed to set the 'innerHTML' property on 'Element': This document requires 'TrustedHTML' assignment.";
    expect(cleanMessage(tt)).toBe(tt);
    expect(cleanMessage("Cannot read properties of undefined (reading 'length')")).toBe(
      "Cannot read properties of undefined (reading 'length')",
    );
    // A repository's name is public, and not a path.
    expect(cleanMessage("chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized not found")).toBe(
      "chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized not found",
    );
  });

  it("leaves out every kind of path, which can name the home folder", () => {
    for (const path of [
      "/home/alice/.local/share/harnessUI/models/a.gguf",
      "/Users/alice/Library/Application Support/x",
      "C:\\Users\\alice\\AppData\\Roaming\\harnessUI\\a.gguf",
      "C:/Users/alice/a.gguf",
      "\\\\server\\share\\alice",
      "~/Pictures/harnessUI",
    ]) {
      const out = cleanMessage(`could not open ${path} now`);
      expect(out, path).not.toMatch(/alice|Users|home|Pictures/);
      expect(out, path).toMatch(/^could not open … ?.*now$/);
    }
  });

  it("leaves out addresses and anything shaped like a key", () => {
    const out = cleanMessage(
      "GET https://huggingface.co/api/models?token=abc failed; key sk-or-v1-0123456789abcdef, hf_AbCdEfGhIjKlMnOp, ghp_0123456789abcdefghij, a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6",
    );
    expect(out).not.toMatch(/huggingface|token|sk-or|hf_|ghp_|a1b2c3/);
  });

  it("cuts a long message", () => {
    const out = cleanMessage("word ".repeat(200));
    expect(out.length).toBe(MESSAGE_LIMIT);
    expect(out.endsWith("…")).toBe(true);
  });
});

describe("crashSummary", () => {
  it("names the error by its type, and anything else thrown as an Error", () => {
    expect(crashSummary(new TypeError("x"))).toEqual({ name: "TypeError", message: "x" });
    expect(crashSummary("thrown text")).toEqual({ name: "Error", message: "thrown text" });
    expect(crashSummary(42)).toEqual({ name: "Error", message: "42" });
    const odd = new Error("y");
    odd.name = "/home/alice/evil name";
    expect(crashSummary(odd).name).toBe("Error");
  });
});
