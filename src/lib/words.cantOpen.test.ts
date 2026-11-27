import { describe, expect, it } from "vitest";
import { CANT_OPEN, cantOpenCardTip } from "./words";
import { APP_NAME } from "./app";

// The one set of words for a model whose kind this version of the app cannot open yet. Every
// screen that shows such a model (the search card, the repository page, Installed, Compare)
// takes its words from here, so the owner reads the same few true sentences everywhere.

/** Every plain string in the block, with the functions called on a sample. */
function everyPhrase(): string[] {
  const out: string[] = [];
  const walk = (value: unknown) => {
    if (typeof value === "string") out.push(value);
    else if (typeof value === "function") out.push(String((value as (a: string) => unknown)("sample")));
    else if (value && typeof value === "object") Object.values(value).forEach(walk);
  };
  walk(CANT_OPEN);
  out.push(cantOpenCardTip("sample"));
  return out;
}

describe("the words for a model this version can't open yet", () => {
  it("says what is wrong, and that nothing is wrong with the person's computer", () => {
    expect(CANT_OPEN.short).toBe("Can't open yet");
    expect(CANT_OPEN.title).toBe("This version can't open this model yet");
    expect(CANT_OPEN.body.before + "sample" + CANT_OPEN.body.after).toBe(
      `It is a “sample” model. The model runner in this version of ${APP_NAME} does not know that kind yet. Nothing is wrong with your computer.`,
    );
    expect(CANT_OPEN.noKind).toBe("This file does not say what kind of model it is, so this version can't open it.");
  });

  it("says what to do next, and promises only what is true", () => {
    expect(CANT_OPEN.next).toBe(
      `Newer versions of ${APP_NAME} come with a newer model runner, and the model may work then. Nothing is downloaded until it can open.`,
    );
    expect(CANT_OPEN.check).toBe("Check for updates");
    // "may work", never "will": nobody knows what a later version knows.
    for (const text of everyPhrase()) expect(text).not.toMatch(/\bwill (work|open|load)\b/i);
  });

  it("names no memory figure and offers no download for a model that cannot open", () => {
    expect(CANT_OPEN.memory).toBe("Memory use is worked out once this version can open the model.");
    expect(CANT_OPEN.compareFit).toBe("Not checked. This version can't open this model yet.");
    for (const text of everyPhrase()) expect(text).not.toMatch(/download anyway|fits|too large/i);
  });

  it("puts the kind's name, which is the file's own text, between two halves of a sentence", () => {
    expect(CANT_OPEN.row.before + "sample" + CANT_OPEN.row.after).toBe(
      `Can't open yet: this version of ${APP_NAME} does not know the “sample” kind of model. A newer version may open it.`,
    );
    expect(cantOpenCardTip("sample")).toBe(
      `From the listing: this version of ${APP_NAME} may not know this kind of model (“sample”). Open it to check.`,
    );
  });

  it("words what an update check found, for a model that still cannot open", () => {
    expect(CANT_OPEN.latest("0.2.1")).toBe(
      "You have the latest version (0.2.1). It can't open this model yet, so check again later.",
    );
    expect(CANT_OPEN.noUpdates).toBe(
      "This copy can't update itself. New versions are on the project's GitHub releases page.",
    );
    expect(CANT_OPEN.checking).toBe("Checking for updates…");
  });

  it("has no jargon, no exclamation mark and no raw identifier on screen", () => {
    for (const text of everyPhrase()) {
      expect(text.trim().length, text).toBeGreaterThan(0);
      expect(text, text).not.toMatch(/llama|architecture|gguf|engine|\bbuild\b|backend|_|!/i);
    }
  });
});
