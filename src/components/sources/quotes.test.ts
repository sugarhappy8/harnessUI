import { describe, expect, it } from "vitest";
import { checkQuotes, citedIn, quoteInText, quotedPassages, quotesFor } from "./quotes";

const EXCERPT =
  "Runs the provided closure on a thread where blocking is acceptable.\n\n" +
  "In general, issuing a blocking call or performing a lot of compute in a future without yielding is problematic,\n" +
  "as it may prevent the executor from driving other futures forward.\n\n…\n\n" +
  "Closures spawned using spawn_blocking cannot be cancelled abruptly; they are run to completion.";

describe("quotedPassages", () => {
  it("finds text in double quotes of 20 characters or more", () => {
    const text = 'The docs say "runs the provided closure on a thread" and call it "fine". Also “blocking is acceptable there” [1].';
    expect(quotedPassages(text).map((q) => q.quote)).toEqual([
      "runs the provided closure on a thread",
      "blocking is acceptable there",
    ]);
  });

  it("knows the quotation marks other languages use", () => {
    expect(quotedPassages("Dort steht „der Abschluss läuft auf einem Thread“ [1].").map((q) => q.quote)).toEqual([
      "der Abschluss läuft auf einem Thread",
    ]);
    expect(quotedPassages("Il dit «la fermeture est exécutée ailleurs» [2].").map((q) => q.quote)).toEqual([
      "la fermeture est exécutée ailleurs",
    ]);
  });

  it("ignores code, short quotes and quotes that never close", () => {
    const text = [
      'Call `print("this is a string of some length")` to see it.',
      "```js",
      'const s = "another string that is long enough";',
      "```",
      'It says "too short" and "this one never closes because the paragraph ends',
      "",
      'A new paragraph "with a quote that is long enough" [1].',
    ].join("\n");
    expect(quotedPassages(text).map((q) => q.quote)).toEqual(["with a quote that is long enough"]);
  });
});

describe("quotesFor", () => {
  it("gives a quote to the citation that follows it", () => {
    const text =
      'The docs say "runs the provided closure on a thread where blocking is acceptable" [1]. ' +
      'A post adds "it worked well for hashing passwords" [2].';
    expect(quotesFor(text, 1)).toEqual(["runs the provided closure on a thread where blocking is acceptable"]);
    expect(quotesFor(text, 2)).toEqual(["it worked well for hashing passwords"]);
    expect(quotesFor(text, 3)).toEqual([]);
  });

  it("falls back to the citation before it, then to the nearest one in the paragraph", () => {
    const before = 'According to the documentation [1], "closures spawned this way cannot be cancelled".';
    expect(quotesFor(before, 1)).toEqual(["closures spawned this way cannot be cancelled"]);
    const shared = '"The first quote is long enough" and "the second one is long as well" [1][2].';
    expect(quotesFor(shared, 1)).toEqual(["The first quote is long enough", "the second one is long as well"]);
    expect(quotesFor(shared, 2)).toEqual(["The first quote is long enough", "the second one is long as well"]);
    const list = '- "the first item quotes this text" [1]\n- "the second item quotes that text" [2]';
    expect(quotesFor(list, 1)).toEqual(["the first item quotes this text"]);
    expect(quotesFor(list, 2)).toEqual(["the second item quotes that text"]);
  });

  it("does not carry a citation from one paragraph into another", () => {
    const text = 'Said in general: "a quote without any source near it".\n\nAnother paragraph cites its source [1].';
    expect(quotesFor(text, 1)).toEqual([]);
  });

  it("reads a list in one bracket as its citations", () => {
    const text = 'Both agree that "blocking is acceptable on that thread" [1, 2].';
    expect(quotesFor(text, 1)).toHaveLength(1);
    expect(quotesFor(text, 2)).toHaveLength(1);
    expect(citedIn(text, 2)).toBe(true);
    expect(citedIn(text, 3)).toBe(false);
    expect(citedIn("No citation, but code `a[2]`.", 2)).toBe(false);
  });
});

describe("quoteInText", () => {
  it("finds a quote whatever its letter case and white space", () => {
    expect(quoteInText("runs the provided closure on a thread", EXCERPT)).toBe(true);
    expect(quoteInText("RUNS  the provided\nclosure on a   thread", EXCERPT)).toBe(true);
    // Across the line break of the retrieved text.
    expect(quoteInText("without yielding is problematic, as it may prevent the executor", EXCERPT)).toBe(true);
  });

  it("does not find what the text does not say", () => {
    expect(quoteInText("runs the provided closure on the main thread", EXCERPT)).toBe(false);
    expect(quoteInText("blocking is never acceptable", EXCERPT)).toBe(false);
    expect(quoteInText("", EXCERPT)).toBe(false);
    expect(quoteInText("anything at all, really", "")).toBe(false);
    // Not across text that was left out between two passages.
    expect(quoteInText("driving other futures forward. Closures spawned using spawn_blocking", EXCERPT)).toBe(false);
  });

  it("allows for punctuation and emphasis the answer added", () => {
    expect(quoteInText("blocking is acceptable.", EXCERPT)).toBe(true);
    expect(quoteInText("…blocking is acceptable…", EXCERPT)).toBe(true);
    expect(quoteInText("a thread where **blocking** is _acceptable_", EXCERPT)).toBe(true);
    expect(quoteInText("Closures spawned using `spawn_blocking` cannot be cancelled abruptly", EXCERPT)).toBe(true);
    expect(quoteInText("can’t be found", "It can't be found here.")).toBe(true);
    expect(quoteInText("a long–dash here", "a long-dash here")).toBe(true);
  });

  it("checks each part of a quote that leaves something out, in order", () => {
    expect(quoteInText("Runs the provided closure ... where blocking is acceptable", EXCERPT)).toBe(true);
    expect(quoteInText("issuing a blocking call […] is problematic", EXCERPT)).toBe(true);
    expect(quoteInText("where blocking is acceptable ... Runs the provided closure", EXCERPT)).toBe(false);
    expect(quoteInText("Runs the provided closure ... on the main thread", EXCERPT)).toBe(false);
    expect(quoteInText("... …", EXCERPT)).toBe(false);
  });
});

describe("checkQuotes", () => {
  const answers = [
    { id: 10, text: 'The docs say "runs the provided closure on a thread where blocking is acceptable" [1].' },
    { id: 11, text: 'They also say "the closure always runs on the main thread" [1], and [2] agrees.' },
    { id: 12, text: "No sources here." },
  ];

  it("says for each quote in an answer that cites the source whether the text has it", () => {
    const result = checkQuotes(answers, 1, EXCERPT);
    expect(result.cited).toBe(2);
    expect(result.quotes).toEqual([
      { messageId: 10, quote: "runs the provided closure on a thread where blocking is acceptable", found: true },
      { messageId: 11, quote: "the closure always runs on the main thread", found: false },
    ]);
  });

  it("knows a source that is cited without a quote, and one that is not cited", () => {
    expect(checkQuotes(answers, 2, "anything")).toEqual({ cited: 1, quotes: [] });
    expect(checkQuotes(answers, 3, "anything")).toEqual({ cited: 0, quotes: [] });
    expect(checkQuotes([], 1, EXCERPT)).toEqual({ cited: 0, quotes: [] });
  });

  it("lists a quote once however often an answer repeats it", () => {
    const twice = [{ id: 1, text: '"blocking is acceptable on that thread" [1]. Again: "blocking is acceptable on that thread" [1].' }];
    expect(checkQuotes(twice, 1, EXCERPT).quotes).toHaveLength(1);
  });
});
