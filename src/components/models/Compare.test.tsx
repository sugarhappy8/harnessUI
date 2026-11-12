import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type {
  CatalogFacts,
  CatalogListEntry,
  CompareFacts,
  CompareItem,
  ErrorInfo,
  FileFacts,
  FitResult,
  HubFacts,
  RepoView,
  VariantView,
  Verdict,
} from "../../lib/types";

// Made-up names only: no model name appears in code or tests.

const ipcMock = vi.hoisted(() => ({
  isTauri: () => true,
  hubRepo: vi.fn<(repo: string, ctx: number | null) => Promise<RepoView>>(),
  compareFacts: vi.fn<(items: CompareItem[], ctx: number | null) => Promise<CompareFacts[]>>(),
  catalogList: vi.fn<(ctx: number | null) => Promise<CatalogListEntry[]>>(),
}));
vi.mock("../../lib/ipc", () => ipcMock);
const openUrl = vi.hoisted(() => vi.fn(async (url: string) => void url));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));
vi.mock("../../store/app", () => ({
  toErrorInfo: (e: unknown) =>
    e && typeof e === "object" && "code" in e
      ? e
      : { code: "unexpected", message: "Something went wrong.", nextStep: "Try again.", details: String(e) },
}));

const { Compare, CompareTabLabel, PROBLEM_WORDS, EMPTY_TEXT, NO_DESCRIPTION, SOURCE, SUGGESTED_SHOWN, fitSource, shown } =
  await import("./Compare");
const { useCompare } = await import("../../store/compare");

function fit(verdict: Verdict, sentence = "Needs about 4.4 GB; your GPU has 15.2 GB free."): FitResult {
  return { verdict, neededBytes: 4.4e9, availableBytes: 15.2e9, suggestedCtx: null, partialOffload: false, sentence };
}

function catalog(id: string, over: Partial<CatalogFacts> = {}): CatalogFacts {
  return {
    id,
    name: `Model ${id}`,
    summary: "A small model that answers questions.",
    source: "https://huggingface.co/example-org/alpha",
    goodAt: ["chat", "coding"],
    publisher: "Example Org",
    parameters: 9e9,
    activeParameters: null,
    languages: ["en", "fr"],
    released: "2026-03",
    capabilities: ["tools", "thinking"],
    license: { name: "apache-2.0", url: "https://example.org/license" },
    ...over,
  };
}

function hub(repo: string, over: Partial<HubFacts> = {}): HubFacts {
  return {
    repo,
    author: "uploader-one",
    pipelineTag: "text-generation",
    baseModel: null,
    languages: ["de"],
    license: { name: "mit", url: null },
    tags: ["gguf", "conversational"],
    downloads: 12345,
    likes: 6,
    lastModified: "2026-02-03T04:05:06Z",
    gated: false,
    cardUrl: `https://huggingface.co/${repo}`,
    ...over,
  };
}

function fileFacts(over: Partial<FileFacts> = {}): FileFacts {
  return {
    architecture: "sample",
    parameters: 4e9,
    contextLength: 32768,
    tools: false,
    thinking: true,
    vision: false,
    moe: false,
    quantization: "Q4_K_M",
    expertCount: null,
    expertUsedCount: null,
    activeParameters: null,
    ...over,
  };
}

function facts(item: CompareItem, ctx: number | null, over: Partial<CompareFacts> = {}): CompareFacts {
  return {
    item,
    title: "catalogId" in item ? `Model ${item.catalogId}` : "repo" in item ? item.repo : "local.gguf",
    file: "repo" in item ? item.file : "alpha-Q4_K_M.gguf",
    quant: "Q4_K_M",
    sizeBytes: 2.7e9,
    installedPath: "installedPath" in item ? item.installedPath : null,
    ctx: ctx ?? 8192,
    fit: fit("green"),
    catalog: "catalogId" in item ? catalog(item.catalogId) : null,
    baseCatalog: null,
    hub: "repo" in item ? hub(item.repo) : null,
    fileFacts: fileFacts(),
    results: [],
    problem: null,
    ...over,
  };
}

function listEntry(id: string): CatalogListEntry {
  return {
    catalog: catalog(id),
    repo: `example/${id}-GGUF`,
    file: `${id}-Q4_K_M.gguf`,
    quant: "Q4_K_M",
    sizeBytes: 2.7e9,
    fit: fit("green"),
    installedPath: null,
    results: [],
  };
}

function variant(label: string, size: number, verdict: Verdict): VariantView {
  return {
    label,
    files: [{ path: `alpha-${label}.gguf`, size, sha256: null }],
    totalSize: size,
    bits: null,
    fit: fit(verdict),
    installed: false,
    hasChecksum: true,
  };
}

function repoView(id: string, ctx: number, variants: VariantView[]): RepoView {
  return {
    id,
    commit: "a".repeat(40),
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
    fitContext: ctx,
  };
}

/** Facts per item as the backend would give them; override per test. */
let factsFor: (item: CompareItem, ctx: number | null) => CompareFacts;

beforeEach(() => {
  factsFor = (item, ctx) => facts(item, ctx);
  ipcMock.compareFacts.mockImplementation(async (items, ctx) => items.map((i) => factsFor(i, ctx)));
  ipcMock.catalogList.mockImplementation(async () => [listEntry("one"), listEntry("two")]);
  ipcMock.hubRepo.mockImplementation(async (repo, ctx) =>
    repoView(repo, ctx ?? 8192, [variant("Q2_K", 2e9, "green"), variant("Q4_K_M", 4e9, "green"), variant("Q8_0", 8e9, "red")]),
  );
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  useCompare.setState({ entries: [], auto: [], ctx: null, repos: {} });
});

async function show(onOpen?: (tab: "installed" | "browse") => void) {
  await act(async () => {
    render(<Compare onOpen={onOpen} />);
  });
  // Let chained requests (file choice, then facts) settle.
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
}

function cell(question: string, column = 0): HTMLElement {
  const row = screen.getByRole("rowheader", { name: question }).closest("tr")!;
  return row.querySelectorAll("td")[column] as HTMLElement;
}

describe("Compare: nothing chosen", () => {
  it("says how to start, and offers the catalog's suggested models", async () => {
    const onOpen = vi.fn();
    await show(onOpen);
    expect(screen.getByText(EMPTY_TEXT)).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Suggested models" })).toBeTruthy();
    expect(screen.getByText("Model one")).toBeTruthy();
    expect(screen.getAllByText(/Q4_K_M: smaller file, good quality/).length).toBe(2);
    expect(screen.getAllByText("Good at everyday chat and coding. (from the publisher's model card)").length).toBe(2);
    expect(ipcMock.catalogList).toHaveBeenCalledWith(null);
    expect(ipcMock.compareFacts).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Open Browse" }));
    expect(onOpen).toHaveBeenCalledWith("browse");
  });

  it("adds a suggestion as a column and stops suggesting it", async () => {
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Add to comparison: Model one" }));
    });
    expect(useCompare.getState().entries).toEqual([{ catalogId: "one" }]);
    expect(ipcMock.compareFacts).toHaveBeenCalledWith([{ catalogId: "one" }], null);
    expect(screen.getByRole("table", { name: "Model comparison" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Add to comparison: Model one" })).toBeNull();
    expect(screen.getByRole("button", { name: "Add to comparison: Model two" })).toBeTruthy();
  });

  it("shows the count on the tab", async () => {
    useCompare.setState({ entries: [{ catalogId: "one" }, { catalogId: "two" }] });
    render(<CompareTabLabel />);
    expect(document.body.textContent).toContain("Compare");
    expect(document.body.textContent).toContain("(2 chosen)");
  });
});

describe("Compare: rows and where each answer comes from", () => {
  it("asks every question of the specification, in order", async () => {
    useCompare.setState({ entries: [{ catalogId: "one" }] });
    await show();
    const questions = screen.getAllByRole("rowheader").map((h) => h.textContent);
    expect(questions).toEqual([
      "What is it good at?",
      "Who made it?",
      "How big is it?",
      "Will it fit on this computer?",
      "Download size",
      "Can it use tools?",
      "Can it read images?",
      "Does it think before answering?",
      "Longest conversation it was made for",
      "Languages",
      "Made from",
      "Released or last updated",
      "License",
      "Downloads on Hugging Face",
      "Your test results",
    ]);
  });

  it("labels catalog facts as the publisher's model card and the app's list", async () => {
    useCompare.setState({ entries: [{ catalogId: "one" }] });
    await show();
    const good = cell("What is it good at?");
    expect(good.textContent).toContain("“A small model that answers questions.”");
    expect(good.textContent).toContain("Good at everyday chat and coding.");
    expect(within(good).getByText(SOURCE.card)).toBeTruthy();
    expect(cell("Who made it?").textContent).toBe(`Example Org${SOURCE.card}`);
    expect(cell("How big is it?").textContent).toBe(
      `9 billion parameters. Bigger models are usually smarter but slower.${SOURCE.card}`,
    );
    expect(cell("Will it fit on this computer?").textContent).toContain("Fits.");
    expect(within(cell("Will it fit on this computer?")).getByText(fitSource(8192))).toBeTruthy();
    expect(fitSource(8192)).toBe("Estimate for this computer at 8,192 tokens, about 6,000 words");
    expect(cell("Download size").textContent).toBe(`2.7 GB${SOURCE.catalog}`);
    // The catalog's capabilities win over the file's guess.
    expect(cell("Can it use tools?").textContent).toBe(`Yes${SOURCE.catalog}`);
    expect(cell("Can it read images?").textContent).toBe(`No${SOURCE.catalog}`);
    expect(cell("Does it think before answering?").textContent).toBe(`Yes${SOURCE.catalog}`);
    expect(cell("Longest conversation it was made for").textContent).toBe(
      `32,768 tokens, about 24,000 words${SOURCE.file}`,
    );
    expect(cell("Languages").textContent).toBe(`English and French${SOURCE.card}`);
    expect(cell("Made from").textContent).toBe(`example-org/alpha${SOURCE.catalog}`);
    expect(cell("Released or last updated").textContent).toBe(`Released March 2026${SOURCE.card}`);
    // A license id in words.
    expect(cell("License").textContent).toBe(`Apache 2.0 Read the license${SOURCE.catalog}`);
    // Missing data.
    expect(cell("Downloads on Hugging Face").textContent).toBe("Not known");
    expect(cell("Your test results").textContent).toContain("No tests on this computer yet.");
  });

  it("labels Hugging Face facts as what the uploader lists", async () => {
    useCompare.setState({ entries: [{ repo: "someone/alpha-GGUF", file: "alpha-Q4_K_M.gguf" }] });
    await show();
    const good = cell("What is it good at?");
    expect(good.textContent).toBe(`The uploader lists: writes text, made for chat.${SOURCE.uploader}`);
    expect(cell("Who made it?").textContent).toBe(`uploader-one uploaded it to Hugging Face.${SOURCE.hub}`);
    expect(cell("How big is it?").textContent).toContain(SOURCE.file);
    expect(cell("Can it use tools?").textContent).toBe(`No${SOURCE.file}`);
    expect(cell("Languages").textContent).toBe(`German${SOURCE.uploader}`);
    expect(cell("Released or last updated").textContent).toBe(`Last updated February 3, 2026${SOURCE.hub}`);
    expect(cell("License").textContent).toBe(`MIT${SOURCE.uploader}`);
    expect(cell("Downloads on Hugging Face").textContent).toBe(`12,345 in the last 30 days, 6 likes${SOURCE.hub}`);
    expect(cell("Made from").textContent).toBe("Not known");
    expect(screen.getByText("Q4_K_M: smaller file, good quality")).toBeTruthy();
  });

  it("says there is no description when only format tags exist, with a link to the card", async () => {
    factsFor = (item, ctx) => facts(item, ctx, { hub: hub("someone/beta-GGUF", { pipelineTag: null, tags: ["gguf", "imatrix"] }) });
    useCompare.setState({ entries: [{ repo: "someone/beta-GGUF", file: "beta-Q4_K_M.gguf" }] });
    await show();
    const good = cell("What is it good at?");
    expect(good.textContent).toBe(`${NO_DESCRIPTION} Read the model card.`);
    expect(within(good).getByRole("button", { name: "Read the model card." })).toBeTruthy();
  });

  it("shows a base model's catalog text only under the uploader's claim, never its capabilities", async () => {
    factsFor = (item, ctx) =>
      facts(item, ctx, {
        baseCatalog: { baseModel: "example-org/alpha", catalog: catalog("one", { capabilities: ["tools", "vision"] }) },
        hub: hub("someone/alpha-GGUF", { baseModel: "example-org/alpha", pipelineTag: null, tags: [] }),
      });
    useCompare.setState({ entries: [{ repo: "someone/alpha-GGUF", file: "alpha-Q4_K_M.gguf" }] });
    await show();
    const good = cell("What is it good at?");
    expect(good.textContent).toContain(
      "The uploader says this is made from example-org/alpha. That model's publisher says:",
    );
    expect(good.textContent).toContain("“A small model that answers questions.”");
    // Capabilities, size and publisher come from the file and Hugging Face, not the catalog.
    expect(cell("Can it use tools?").textContent).toBe(`No${SOURCE.file}`);
    expect(cell("Can it read images?").textContent).toBe(`No${SOURCE.file}`);
    expect(cell("Who made it?").textContent).toContain(SOURCE.hub);
    expect(cell("How big is it?").textContent).toContain("4 billion");
    expect(cell("Made from").textContent).toBe(`example-org/alpha${SOURCE.uploader}`);
  });

  it("explains a mixture of experts and lists the user's own results", async () => {
    factsFor = (item, ctx) =>
      facts(item, ctx, {
        catalog: null,
        fileFacts: fileFacts({ parameters: 35e9, moe: true, activeParameters: 3e9 }),
        results: [
          { kind: "modelCheck", id: "p1", at: Date.UTC(2026, 2, 12), backend: "vulkan", passed: 3, total: 3 },
          { kind: "benchmark", id: "b1", at: Date.UTC(2026, 2, 14), backend: "cpu", passed: 7, total: 11 },
        ],
      });
    useCompare.setState({ entries: [{ installedPath: "/models/local.gguf" }] });
    await show();
    expect(cell("How big is it?").textContent).toBe(
      `35 billion, using 3 billion at a time, so it runs faster than its size suggests.${SOURCE.file}`,
    );
    const results = cell("Your test results").textContent;
    expect(results).toContain("Coding benchmark: 7 of 11 tasks done · Processor only (slower) · March 14, 2026");
    expect(results).toContain("Model check: 3 of 3 checks passed · Graphics card (Vulkan) · March 12, 2026");
    expect(results!.indexOf("Coding benchmark")).toBeLessThan(results!.indexOf("Model check"));
    expect(results).toContain(SOURCE.results);
    expect(cell("Download size").textContent).toBe(`2.7 GB${SOURCE.disk}`);
    expect(within(screen.getAllByRole("columnheader")[1]!).getByText("On this computer")).toBeTruthy();
  });
});

describe("Compare: model card links go through the destination-confirm dialog", () => {
  it("shows the host before opening the card in the browser", async () => {
    useCompare.setState({ entries: [{ repo: "someone/alpha-GGUF", file: "alpha-Q4_K_M.gguf" }] });
    await show();
    fireEvent.click(screen.getByRole("button", { name: "Show model card" }));
    expect(openUrl).not.toHaveBeenCalled();
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Open this link?")).toBeTruthy();
    expect(within(dialog).getByText("huggingface.co")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Open in browser" }));
    expect(openUrl).toHaveBeenCalledWith("https://huggingface.co/someone/alpha-GGUF");
  });
});

describe("Compare: a repository's file", () => {
  it("chooses the largest file that fits for a Browse result, and Change picks another", async () => {
    useCompare.setState({ entries: [{ repo: "someone/alpha-GGUF", file: null }] });
    await show();
    expect(ipcMock.hubRepo).toHaveBeenCalledWith("someone/alpha-GGUF", null);
    expect(useCompare.getState().entries).toEqual([{ repo: "someone/alpha-GGUF", file: "alpha-Q4_K_M.gguf" }]);
    expect(ipcMock.compareFacts).toHaveBeenCalledWith([{ repo: "someone/alpha-GGUF", file: "alpha-Q4_K_M.gguf" }], null);
    expect(screen.getByText("Picked for you: the largest file that fits.")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Change" }));
    const files = screen.getByRole("group", { name: "Files in this repository" });
    const options = within(files).getAllByRole("button").map((b) => b.textContent);
    expect(options).toEqual([
      "Q2_K: very small file, noticeably lower quality · 2.0 GB · Fits",
      "Q4_K_M: smaller file, good quality · 4.0 GB · Fits",
      "Q8_0: large file, almost the original quality · 8.0 GB · Too large",
    ]);
    // The page was fetched once, for the choice; Change needed no new request.
    expect(ipcMock.hubRepo).toHaveBeenCalledTimes(1);
    await act(async () => {
      fireEvent.click(within(files).getByRole("button", { name: /^Q8_0/ }));
    });
    expect(useCompare.getState().entries).toEqual([{ repo: "someone/alpha-GGUF", file: "alpha-Q8_0.gguf" }]);
    expect(ipcMock.compareFacts).toHaveBeenLastCalledWith([{ repo: "someone/alpha-GGUF", file: "alpha-Q8_0.gguf" }], null);
    expect(screen.queryByText(/Picked for you/)).toBeNull();
  });

  it("takes the smallest file when none fits", async () => {
    ipcMock.hubRepo.mockImplementation(async (repo, ctx) =>
      repoView(repo, ctx ?? 8192, [variant("Q8_0", 8e9, "red"), variant("Q4_K_M", 4e9, "yellow")]),
    );
    useCompare.setState({ entries: [{ repo: "someone/alpha-GGUF", file: null }] });
    await show();
    expect(useCompare.getState().entries).toEqual([{ repo: "someone/alpha-GGUF", file: "alpha-Q4_K_M.gguf" }]);
    expect(screen.getByText("Picked for you: the smallest file.")).toBeTruthy();
  });

  it("chooses again at a new comparison size, but keeps the user's own pick", async () => {
    ipcMock.hubRepo.mockImplementation(async (repo, ctx) =>
      repoView(
        repo,
        ctx ?? 8192,
        (ctx ?? 8192) > 16384
          ? [variant("Q2_K", 2e9, "green"), variant("Q4_K_M", 4e9, "red")]
          : [variant("Q2_K", 2e9, "green"), variant("Q4_K_M", 4e9, "green")],
      ),
    );
    useCompare.setState({
      entries: [
        { repo: "someone/alpha-GGUF", file: null },
        { repo: "someone/beta-GGUF", file: "beta-Q8_0.gguf" },
      ],
    });
    await show();
    expect(useCompare.getState().entries[0]).toEqual({ repo: "someone/alpha-GGUF", file: "alpha-Q4_K_M.gguf" });
    await act(async () => {
      fireEvent.change(screen.getByLabelText("Compare at"), { target: { value: "32768" } });
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(ipcMock.hubRepo).toHaveBeenLastCalledWith("someone/alpha-GGUF", 32768);
    expect(useCompare.getState().entries).toEqual([
      { repo: "someone/alpha-GGUF", file: "alpha-Q2_K.gguf" },
      { repo: "someone/beta-GGUF", file: "beta-Q8_0.gguf" },
    ]);
    expect(ipcMock.hubRepo).not.toHaveBeenCalledWith("someone/beta-GGUF", expect.anything());
    // The user's pick is asked about at once; the app's choice only once it is made again,
    // so no request is spent on the file it no longer uses.
    expect(ipcMock.compareFacts).toHaveBeenCalledWith([{ repo: "someone/beta-GGUF", file: "beta-Q8_0.gguf" }], 32768);
    expect(ipcMock.compareFacts).toHaveBeenCalledWith([{ repo: "someone/alpha-GGUF", file: "alpha-Q2_K.gguf" }], 32768);
    expect(ipcMock.compareFacts).not.toHaveBeenCalledWith(
      expect.arrayContaining([{ repo: "someone/alpha-GGUF", file: "alpha-Q4_K_M.gguf" }]),
      32768,
    );
    expect(ipcMock.catalogList).toHaveBeenLastCalledWith(32768);
    expect(within(cell("Will it fit on this computer?")).getByText(fitSource(32768))).toBeTruthy();
  });

  it("says when a repository has no model files", async () => {
    ipcMock.hubRepo.mockImplementation(async (repo) => repoView(repo, 8192, []));
    useCompare.setState({ entries: [{ repo: "someone/empty", file: null }] });
    await show();
    expect(screen.getByText(PROBLEM_WORDS.noFiles)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
    expect(ipcMock.compareFacts).not.toHaveBeenCalled();
  });
});

describe("Compare: the comparison size", () => {
  it("shows the defaults' size once known, and offers fixed sizes with words", async () => {
    useCompare.setState({ entries: [{ catalogId: "one" }] });
    await show();
    const select = screen.getByLabelText("Compare at") as HTMLSelectElement;
    const labels = [...select.options].map((o) => o.textContent);
    expect(labels[0]).toBe("Same as your default (8,192 tokens, about 6,000 words)");
    expect(labels).toContain("32,768 tokens, about 24,000 words");
    expect(select.value).toBe("");
  });
});

describe("Compare: problems", () => {
  const net: ErrorInfo = {
    code: "network",
    message: "Hugging Face could not be reached.",
    nextStep: "Check your connection.",
    details: "",
  };

  it("says offline facts need the internet, and Try again asks again", async () => {
    ipcMock.hubRepo.mockRejectedValueOnce(net);
    useCompare.setState({ entries: [{ repo: "someone/alpha-GGUF", file: null }] });
    await show();
    expect(screen.getByText(PROBLEM_WORDS.offline)).toBeTruthy();
    expect(PROBLEM_WORDS.offline).toBe("These facts need the internet. They are saved once the model is downloaded.");
    expect(cell("What is it good at?").textContent).toBe("Not known");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(ipcMock.hubRepo).toHaveBeenCalledTimes(2);
    expect(screen.queryByText(PROBLEM_WORDS.offline)).toBeNull();
    expect(useCompare.getState().entries[0]).toEqual({ repo: "someone/alpha-GGUF", file: "alpha-Q4_K_M.gguf" });
  });

  it("shows each problem the backend reports, beside the facts it still has", async () => {
    const problems = { a: "offline", b: "rateLimited", c: "gated" } as const;
    factsFor = (item, ctx) =>
      facts(item, ctx, { hub: null, problem: problems[("catalogId" in item ? item.catalogId : "a") as "a" | "b" | "c"] });
    useCompare.setState({ entries: [{ catalogId: "a" }, { catalogId: "b" }, { catalogId: "c" }] });
    await show();
    expect(screen.getByText(PROBLEM_WORDS.offline)).toBeTruthy();
    expect(screen.getByText("Hugging Face is busy. Try again in a minute.")).toBeTruthy();
    expect(screen.getByText("This model needs a Hugging Face token. Add one in Settings → Hugging Face.")).toBeTruthy();
    // The catalog facts are still shown.
    expect(cell("Who made it?", 1).textContent).toContain("Example Org");
  });

  it("maps a failed request to the same words", async () => {
    ipcMock.compareFacts.mockRejectedValueOnce({ ...net, code: "hf_rate_limited" });
    useCompare.setState({ entries: [{ catalogId: "one" }] });
    await show();
    expect(screen.getByText(PROBLEM_WORDS.rateLimited)).toBeTruthy();
  });
});

describe("Compare: four models", () => {
  it("stops suggesting when the list is full, and Remove makes room", async () => {
    useCompare.setState({ entries: ["a", "b", "c", "d"].map((id) => ({ catalogId: id })) });
    await show();
    expect(screen.queryByRole("heading", { name: "Suggested models" })).toBeNull();
    expect(ipcMock.catalogList).not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Remove Model b from comparison" }));
    });
    expect(useCompare.getState().entries.map((e) => ("catalogId" in e ? e.catalogId : ""))).toEqual(["a", "c", "d"]);
    expect(await screen.findByRole("heading", { name: "Suggested models" })).toBeTruthy();
  });
});

describe("Compare: uploader text is shown as text", () => {
  it("renders markup as text and makes a right-to-left override visible", async () => {
    factsFor = (item, ctx) =>
      facts(item, ctx, {
        title: "someone/alpha-GGUF",
        hub: hub("someone/alpha-GGUF", {
          author: '<img src=x onerror="alert(1)">',
          baseModel: "evil\u202Efdp.exe",
          license: { name: "<b>bold</b>", url: "javascript:alert(1)" },
        }),
      });
    useCompare.setState({ entries: [{ repo: "someone/alpha-GGUF", file: "alpha-Q4_K_M.gguf" }] });
    await show();
    const table = screen.getByRole("table");
    expect(table.querySelector("img")).toBeNull();
    expect(table.querySelector("b")).toBeNull();
    expect(cell("Who made it?").textContent).toContain('<img src=x onerror="alert(1)">');
    expect(cell("Made from").textContent).toContain("evil[U+202E]fdp.exe");
    expect(cell("Made from").textContent).not.toContain("\u202E");
    // A javascript: link is not a link.
    expect(within(cell("License")).queryByRole("button")).toBeNull();
    expect(shown("a\u200Bb\u2066c", 300)).toBe("a[U+200B]b[U+2066]c");
    expect(shown("x".repeat(400))).toHaveLength(300);
  });

  it("makes every invisible character the approval cards escape visible, and never cuts a character in half", () => {
    // The same set as the approval cards' sanitiser (crates/policy/src/sanitize.rs):
    // hidden tag characters, variation selectors, fillers, interlinear annotations.
    const hidden = String.fromCodePoint(0xe0041, 0xe0042);
    expect(shown(`a${hidden}b`)).toBe("a[U+E0041][U+E0042]b");
    expect(shown("a\uFE0Fb\u3164c\uFFF9d\u180Be\u115Ff")).toBe("a[U+FE0F]b[U+3164]c[U+FFF9]d[U+180B]e[U+115F]f");
    expect(shown(`x${String.fromCodePoint(0xe0100)}`)).toBe("x[U+E0100]");
    // Tabs and new lines stay as they are.
    expect(shown("a\tb\nc")).toBe("a\tb\nc");
    // The cap counts characters, so a character outside the basic plane is kept whole.
    const capped = shown(`${"x".repeat(298)}😀😀😀`, 300);
    expect(capped).toBe(`${"x".repeat(298)}😀…`);
    expect(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])/.test(capped)).toBe(false);
  });
});

describe("Compare: columns as the backend fills them", () => {
  it("shows a catalog model's facts without the internet, and says what is missing and why", async () => {
    // What compare_facts returns for a catalog model that is not downloaded, offline: the
    // catalog's facts, no file facts, no fit, no Hugging Face facts.
    factsFor = (item, ctx) =>
      facts(item, ctx, {
        file: "one-Q4_K_M.gguf",
        sizeBytes: 3413361504,
        ctx: 32768,
        fit: null,
        hub: null,
        fileFacts: null,
        problem: "offline",
        catalog: catalog("one", {
          source: "https://huggingface.co/example-org/one",
          languages: ["multilingual"],
          released: "2026-08-25",
          capabilities: ["tools", "vision", "thinking"],
          license: { name: "apache-2.0", url: null },
        }),
      });
    useCompare.setState({ entries: [{ catalogId: "one" }] });
    await show();
    expect(screen.getByRole("status").textContent).toContain(PROBLEM_WORDS.offline);
    expect(within(screen.getByRole("status")).getByRole("button", { name: "Try again" })).toBeTruthy();
    expect(cell("What is it good at?").textContent).toBe(
      `“A small model that answers questions.”Good at everyday chat and coding.${SOURCE.card}`,
    );
    expect(cell("Who made it?").textContent).toBe(`Example Org${SOURCE.card}`);
    expect(cell("How big is it?").textContent).toBe(
      `9 billion parameters. Bigger models are usually smarter but slower.${SOURCE.card}`,
    );
    expect(cell("Will it fit on this computer?").textContent).toBe("Not known");
    expect(cell("Download size").textContent).toBe(`3.4 GB${SOURCE.catalog}`);
    expect(cell("Can it read images?").textContent).toBe(`Yes${SOURCE.catalog}`);
    expect(cell("Longest conversation it was made for").textContent).toBe("Not known");
    expect(cell("Languages").textContent).toBe(`Many languages${SOURCE.card}`);
    expect(cell("Made from").textContent).toBe(`example-org/one${SOURCE.catalog}`);
    expect(cell("Released or last updated").textContent).toBe(`Released August 25, 2026${SOURCE.card}`);
    expect(cell("License").textContent).toBe(`Apache 2.0${SOURCE.catalog}`);
    expect(cell("Downloads on Hugging Face").textContent).toBe("Not known");
    const head = screen.getAllByRole("columnheader")[1]!;
    expect(within(head).getByText("Model one")).toBeTruthy();
    expect(within(head).getByText("Q4_K_M: smaller file, good quality")).toBeTruthy();
    expect(within(head).queryByText("On this computer")).toBeNull();
    expect(within(head).getByRole("button", { name: "Show model card" })).toBeTruthy();
  });

  it("shows an installed file's own facts, and nothing it does not know", async () => {
    factsFor = (item, ctx) =>
      facts(item, ctx, {
        title: "local-Q8_0.gguf",
        file: "local-Q8_0.gguf",
        quant: "Q8_0",
        sizeBytes: 639446688,
        ctx: 32768,
        catalog: null,
        hub: null,
        fileFacts: fileFacts({ parameters: 6e8, contextLength: 40960, tools: true, thinking: true, quantization: "Q8_0" }),
        fit: fit("green", "Needs about 5.0 GB; your computer has 11.4 GB free. Fits."),
      });
    useCompare.setState({ entries: [{ installedPath: "/models/local-Q8_0.gguf" }] });
    await show();
    const head = screen.getAllByRole("columnheader")[1]!;
    expect(within(head).getByText("local-Q8_0.gguf")).toBeTruthy();
    expect(within(head).getByText("Q8_0: large file, almost the original quality")).toBeTruthy();
    expect(within(head).getByText("On this computer")).toBeTruthy();
    // No card to show: an imported file has no Hugging Face page and no catalog entry.
    expect(within(head).queryByRole("button", { name: "Show model card" })).toBeNull();
    expect(screen.queryByRole("status")).toBeNull();
    expect(cell("What is it good at?").textContent).toBe(NO_DESCRIPTION);
    expect(cell("Who made it?").textContent).toBe("Not known");
    expect(cell("How big is it?").textContent).toBe(
      `600 million parameters. Bigger models are usually smarter but slower.${SOURCE.file}`,
    );
    expect(cell("Will it fit on this computer?").textContent).toContain("your computer has 11.4 GB free");
    expect(within(cell("Will it fit on this computer?")).getByText(fitSource(32768))).toBeTruthy();
    expect(cell("Download size").textContent).toBe(`639 MB${SOURCE.disk}`);
    expect(cell("Can it use tools?").textContent).toBe(`Yes${SOURCE.file}`);
    expect(cell("Can it read images?").textContent).toBe(`No${SOURCE.file}`);
    expect(cell("Does it think before answering?").textContent).toBe(`Yes${SOURCE.file}`);
    expect(cell("Longest conversation it was made for").textContent).toBe(
      `40,960 tokens, about 30,000 words${SOURCE.file}`,
    );
    for (const q of ["Languages", "Made from", "Released or last updated", "License", "Downloads on Hugging Face"]) {
      expect(cell(q).textContent).toBe("Not known");
    }
    // Nothing on this screen downloads or loads a model.
    expect(screen.queryByRole("button", { name: /Download|Load|Use/ })).toBeNull();
  });

  it("names the catalog's publisher model as what a matched file is made from, over the uploader's claim", async () => {
    factsFor = (item, ctx) =>
      facts(item, ctx, {
        catalog: catalog("one", { source: "https://huggingface.co/example-org/one" }),
        hub: hub("someone/one-GGUF", { baseModel: "someone-else/other" }),
      });
    useCompare.setState({ entries: [{ repo: "someone/one-GGUF", file: "one-Q4_K_M.gguf" }] });
    await show();
    expect(cell("Made from").textContent).toBe(`example-org/one${SOURCE.catalog}`);
  });

  it("opens a Hugging Face column's own page, and a catalog model's publisher card", async () => {
    factsFor = (item, ctx) =>
      facts(item, ctx, {
        catalog: catalog("one", { source: "https://huggingface.co/example-org/one" }),
        hub: hub("someone/one-GGUF"),
      });
    useCompare.setState({ entries: [{ repo: "someone/one-GGUF", file: "one-Q4_K_M.gguf" }, { catalogId: "one" }] });
    await show();
    const heads = screen.getAllByRole("columnheader");
    for (const [i, url] of [
      [1, "https://huggingface.co/someone/one-GGUF"],
      [2, "https://huggingface.co/example-org/one"],
    ] as const) {
      fireEvent.click(within(heads[i]!).getByRole("button", { name: "Show model card" }));
      const dialog = await screen.findByRole("alertdialog");
      expect(within(dialog).getByText(url)).toBeTruthy();
      fireEvent.click(within(dialog).getByRole("button", { name: "Open in browser" }));
      expect(openUrl).toHaveBeenLastCalledWith(url);
    }
  });
});

describe("Compare: suggested models", () => {
  it("shows the best four first and the rest on request", async () => {
    ipcMock.catalogList.mockImplementation(async () => ["a", "b", "c", "d", "e", "f"].map(listEntry));
    await show();
    const offered = () =>
      screen.getAllByRole("button", { name: /^Add to comparison: / }).map((b) => b.getAttribute("aria-label"));
    expect(SUGGESTED_SHOWN).toBe(4);
    expect(offered()).toEqual(["a", "b", "c", "d"].map((id) => `Add to comparison: Model ${id}`));
    fireEvent.click(screen.getByRole("button", { name: "Show 2 more suggested models" }));
    expect(offered()).toHaveLength(6);
    fireEvent.click(screen.getByRole("button", { name: "Show fewer" }));
    expect(offered()).toHaveLength(4);
    // A chosen model leaves the list, and the next one moves up.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Add to comparison: Model b" }));
    });
    expect(offered()).toEqual(["a", "c", "d", "e"].map((id) => `Add to comparison: Model ${id}`));
    expect(screen.getByRole("button", { name: "Show 1 more suggested model" })).toBeTruthy();
  });

  it("says which suggestions are already on this computer, with their fit", async () => {
    ipcMock.catalogList.mockImplementation(async () => [
      { ...listEntry("a"), installedPath: "/models/a.gguf", fit: fit("yellow", "Needs a shorter memory here.") },
      { ...listEntry("b"), fit: null },
    ]);
    await show();
    const items = screen.getAllByRole("listitem");
    expect(items[0]!.textContent).toContain("Q4_K_M: smaller file, good quality · 2.7 GB download · On this computer");
    expect(items[0]!.textContent).toContain("Fits with limits.");
    expect(items[1]!.textContent).not.toContain("On this computer");
    expect(items[1]!.textContent).not.toContain("Fits");
  });
});
