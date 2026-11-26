// What is kept of an error that stopped part of a screen from being drawn (0.2.1).
import { describe, expect, it, vi } from "vitest";
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

describe("the safety net never throws itself", () => {
  it("names anything thrown, even a value that cannot be made a string", () => {
    const bare = Object.create(null) as object;
    const hostile = { toString: () => { throw new Error("no"); } };
    expect(crashSummary(bare)).toEqual({ name: "Error", message: "Unknown error" });
    expect(crashSummary(hostile)).toEqual({ name: "Error", message: "Unknown error" });
    const odd = new Error("x");
    (odd as unknown as { message: unknown }).message = { code: 3 };
    expect(() => crashSummary(odd)).not.toThrow();
    const odder = new Error("x");
    Object.defineProperty(odder, "message", { get: () => { throw new Error("no"); } });
    expect(crashSummary(odder)).toEqual({ name: "Error", message: "" });
    (odder as unknown as { name: unknown }).name = 5;
    expect(crashSummary(odder).name).toBe("Error");
  });

  it("leaves out a long quoted run, which can be the person's own text", () => {
    const out = cleanMessage(`Unexpected token 'h', "here is my private prompt about my family" is not valid JSON`);
    expect(out).toBe(`Unexpected token 'h', "…" is not valid JSON`);
  });

  it("reads only the start of a very long message", () => {
    const long = "a-".repeat(200_000);
    const started = performance.now();
    const out = cleanMessage(long);
    expect(performance.now() - started).toBeLessThan(200);
    expect(out.length).toBeLessThanOrEqual(MESSAGE_LIMIT);
  });
});

describe("the root's error handlers", () => {
  it("report only through reportCrash, and draw a plain notice if even the root notice fails", async () => {
    const { createRoot } = await import("react-dom/client");
    const { createElement } = await import("react");
    const { rootErrorHandlers } = await import("./crash");
    const { CRASH } = await import("./words");
    const logged: string[] = [];
    const spy = vi.spyOn(console, "error").mockImplementation((...a: unknown[]) => void logged.push(a.map(String).join(" ")));
    const container = document.body.appendChild(document.createElement("div"));
    function Broken(): never {
      throw new TypeError("could not open /home/alice/secret");
    }
    try {
      const root = createRoot(container, rootErrorHandlers(container));
      // Outside act, which would throw the error back here instead of handing it to the root.
      root.render(createElement(Broken));
      await vi.waitFor(() => expect(container.textContent).toContain(CRASH.title));
      expect(container.querySelector("button")!.textContent).toBe(CRASH.reload);
      expect(logged).toEqual(["[crash] App could not be drawn: TypeError: could not open …"]);
    } finally {
      spy.mockRestore();
      container.remove();
    }
  });
});
