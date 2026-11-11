import { describe, expect, it } from "vitest";
import { splitCitations, type Piece } from "./citations";

const known = (...numbers: number[]) => {
  const set = new Set(numbers);
  return (n: number) => set.has(n);
};

/** The pieces as short text: plain text as it is, a citation as `<n>`. */
function shape(pieces: Piece[]): string {
  return pieces.map((p) => (typeof p === "string" ? p : `<${p.n}>`)).join("");
}

describe("splitCitations", () => {
  it("finds [n] in running text", () => {
    expect(shape(splitCitations("Tokio is a runtime [1]. It is fast [12].", known(1, 12)))).toBe(
      "Tokio is a runtime <1>. It is fast <12>.",
    );
    expect(splitCitations("Fast [3].", known(3))).toEqual(["Fast ", { n: 3, raw: "[3]" }, "."]);
  });

  it("finds citations next to each other and right after a word", () => {
    expect(shape(splitCitations("Both say so [1][2].", known(1, 2)))).toBe("Both say so <1><2>.");
    expect(shape(splitCitations("It is fast[1].", known(1)))).toBe("It is fast<1>.");
    expect(shape(splitCitations("[1] at the start and at the end [2]", known(1, 2)))).toBe(
      "<1> at the start and at the end <2>",
    );
  });

  it("keeps a number that is no source as a citation, so that it can be marked", () => {
    // Whether a source exists is decided where it is shown.
    expect(shape(splitCitations("Made up [9].", known(1)))).toBe("Made up <9>.");
    expect(shape(splitCitations("Made up [9].", known()))).toBe("Made up <9>.");
  });

  it("takes a list in one bracket only when every number is a source", () => {
    expect(shape(splitCitations("Both say so [1, 2].", known(1, 2)))).toBe("Both say so <1><2>.");
    expect(shape(splitCitations("All three [1,2 , 3]", known(1, 2, 3)))).toBe("All three <1><2><3>");
    // An array in prose is not a list of citations.
    expect(shape(splitCitations("The list [1, 2, 3] has three items.", known(1)))).toBe(
      "The list [1, 2, 3] has three items.",
    );
    expect(shape(splitCitations("The pair [4, 5].", known()))).toBe("The pair [4, 5].");
  });

  it("leaves everything else alone", () => {
    for (const text of [
      "",
      "No citation here.",
      "[0] is not a source number",
      "[01] and [007] are not written like one",
      "[1234] is too long",
      "[a] [1a] [ 1 ] [-1] [1.5]",
      "[] [[]] ]1[",
      "an unclosed [1",
      "x[i] and y[n+1]",
    ]) {
      expect(splitCitations(text, known(1, 2, 3)), text).toEqual(text === "" ? [] : [text]);
    }
  });

  it("gives back the whole text, in order", () => {
    const text = "A [1] b [2][3] c [x] d [4, 5] e [6,7].";
    const pieces = splitCitations(text, known(1, 2, 3, 6, 7));
    const back = pieces.map((p) => (typeof p === "string" ? p : p.raw)).join("");
    // The list that was taken is written as its single citations.
    expect(back).toBe("A [1] b [2][3] c [x] d [4, 5] e [6][7].");
    expect(pieces.filter((p) => typeof p !== "string").map((p) => (p as { n: number }).n)).toEqual([1, 2, 3, 6, 7]);
  });
});
