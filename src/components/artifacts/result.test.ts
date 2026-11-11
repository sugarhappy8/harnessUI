import { describe, expect, it } from "vitest";
import { parseSavedLine } from "./result";

describe("the first line of a create or update result", () => {
  it("names the artifact, its kind and the version", () => {
    expect(parseSavedLine('Artifact #3 "Tip calculator" (html), version 2 saved.\nRendered without errors.')).toEqual({
      id: 3,
      title: "Tip calculator",
      kind: "html",
      version: 2,
    });
    expect(parseSavedLine('Artifact #12 "Plan" (markdown), version 1 saved.')?.kind).toBe("markdown");
    expect(parseSavedLine('Artifact #1 "D" (mermaid), version 40 saved.\r\nNot rendered yet: open it to check.')?.version).toBe(40);
  });

  it("keeps a title with quotes and brackets whole", () => {
    expect(parseSavedLine('Artifact #12 "Say "hi" (twice)" (mermaid), version 4 saved.')).toEqual({
      id: 12,
      title: 'Say "hi" (twice)',
      kind: "mermaid",
      version: 4,
    });
    // A title that imitates the rest of the line does not move the id or the version.
    expect(parseSavedLine('Artifact #5 "x" (html), version 9 saved." (svg), version 2 saved.')).toEqual({
      id: 5,
      title: 'x" (html), version 9 saved.',
      kind: "svg",
      version: 2,
    });
  });

  it("is null for anything else", () => {
    for (const text of [
      "",
      "No artifact #3 in this chat.",
      'old_text was found 2 times in artifact #1 (version 1)',
      'Artifact #3 "T" (pdf), version 1 saved.',
      'Artifact #0 "T" (html), version 1 saved.',
      'Artifact #3 "T" (html), version 0 saved.',
      'Artifact #abc "T" (html), version 1 saved.',
      'artifact #3 "T" (html), version 1 saved.',
      ' Artifact #3 "T" (html), version 1 saved.',
      'Artifact #3 "T" (html), version 1 saved. And more',
      'Something first\nArtifact #3 "T" (html), version 1 saved.',
      'Artifact #99999999999999999999 "T" (html), version 1 saved.',
    ]) {
      expect(parseSavedLine(text), text).toBeNull();
    }
  });
});
