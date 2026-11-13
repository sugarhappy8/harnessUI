import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { FitResult, RepoView, SearchPage, VariantView } from "../../lib/types";

// "Add to comparison" on Browse results and on a repository page (0.0.6 item 4). Made-up
// repository names only.

const ipcMock = vi.hoisted(() => ({
  isTauri: () => true,
  hubSearch: vi.fn<(text: string, sort: string, all: boolean) => Promise<SearchPage>>(),
  hubSearchNext: vi.fn(),
  hubRepo: vi.fn<(repo: string, ctx: number | null) => Promise<RepoView>>(),
  startDownload: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipcMock);
const openUrl = vi.hoisted(() => vi.fn(async (url: string) => void url));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));
vi.mock("../../store/app", () => ({
  toErrorInfo: (e: unknown) => ({ code: "unexpected", message: String(e), nextStep: "", details: "" }),
}));

const { Browse } = await import("./Browse");
const { RepoDetail } = await import("./RepoDetail");
const { useCompare, COMPARE_FULL } = await import("../../store/compare");

const fit: FitResult = {
  verdict: "green",
  neededBytes: 1,
  availableBytes: 2,
  suggestedCtx: null,
  partialOffload: false,
  device: "graphicsCard",
  sentence: "Fits easily.",
};

function variant(label: string, parts = 1): VariantView {
  const files = Array.from({ length: parts }, (_, i) => ({
    path: parts > 1 ? `${label}/alpha-${label}-0000${i + 1}-of-0000${parts}.gguf` : `alpha-${label}.gguf`,
    size: 1e9,
    sha256: "0".repeat(64),
  }));
  return { label, files, totalSize: parts * 1e9, bits: null, fit, installed: false, hasChecksum: true };
}

const VIEW: RepoView = {
  id: "someone/alpha-GGUF",
  commit: "a".repeat(40),
  gated: "no",
  license: "apache-2.0",
  architecture: "sample",
  architectureSupported: true,
  contextLength: 32768,
  parameters: 4e9,
  templateTools: true,
  templateThinking: false,
  projector: null,
  variants: [variant("Q4_K_M"), variant("Q8_0", 2)],
  incomplete: [],
  card: "A card.",
  fitContext: 8192,
};

beforeEach(() => {
  ipcMock.hubSearch.mockResolvedValue({
    items: [
      { id: "someone/alpha-GGUF", downloads: 10, likes: 1, gated: "no", license: null, lastModified: null, pipelineTag: null },
      { id: "someone/beta-GGUF", downloads: 5, likes: 0, gated: "no", license: null, lastModified: null, pipelineTag: null },
    ],
    next: null,
  });
  ipcMock.hubRepo.mockResolvedValue(VIEW);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  useCompare.setState({ entries: [], auto: [], ctx: null, repos: {} });
});

async function settle() {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 5));
  });
}

describe("Browse results", () => {
  it("adds a repository without a file, so the Compare tab chooses one", async () => {
    render(<Browse onStarted={() => undefined} />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Add to comparison: someone/alpha-GGUF" }));
    expect(useCompare.getState().entries).toEqual([{ repo: "someone/alpha-GGUF", file: null }]);
    // Adding did not open the repository or ask Hugging Face for anything.
    expect(ipcMock.hubRepo).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Remove from comparison: someone/alpha-GGUF" }));
    expect(useCompare.getState().entries).toEqual([]);
  });

  it("shows a repository as added when one of its files is, and the list as full", async () => {
    useCompare.setState({
      entries: [{ repo: "someone/alpha-GGUF", file: "alpha-Q8_0.gguf" }, { catalogId: "a" }, { catalogId: "b" }],
    });
    render(<Browse onStarted={() => undefined} />);
    await settle();
    expect(screen.getByRole("button", { name: "Remove from comparison: someone/alpha-GGUF" })).toBeTruthy();
    expect(screen.queryByText(COMPARE_FULL)).toBeNull();
    act(() => void useCompare.getState().add({ catalogId: "c" }));
    const add = screen.getByRole("button", { name: "Add to comparison: someone/beta-GGUF" }) as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    expect(add.title).toBe(COMPARE_FULL);
    expect(screen.getByText(COMPARE_FULL)).toBeTruthy();
  });

  it("still opens a repository from its name", async () => {
    render(<Browse onStarted={() => undefined} />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: /^someone\/beta-GGUF/ }));
    await settle();
    expect(ipcMock.hubRepo).toHaveBeenCalledWith("someone/beta-GGUF", null);
  });
});

describe("A repository page", () => {
  it("adds the file picked on the page, and remembers the page for the Change menu", async () => {
    render(<RepoDetail repoId="someone/alpha-GGUF" onBack={() => undefined} onStarted={() => undefined} />);
    await settle();
    const rows = screen.getAllByRole("listitem");
    fireEvent.click(within(rows[1]!).getByRole("button", { name: "Add to comparison: Q8_0" }));
    expect(useCompare.getState().entries).toEqual([
      { repo: "someone/alpha-GGUF", file: "Q8_0/alpha-Q8_0-00001-of-00002.gguf" },
    ]);
    expect(useCompare.getState().repos["someone/alpha-GGUF"]).toEqual({ ctx: null, view: VIEW });
    expect(within(rows[1]!).getByRole("button", { name: "Remove from comparison: Q8_0" })).toBeTruthy();
    expect(within(rows[0]!).getByRole("button", { name: "Add to comparison: Q4_K_M" })).toBeTruthy();
  });

  it("disables adding while the list is full and says why", async () => {
    useCompare.setState({ entries: ["a", "b", "c", "d"].map((id) => ({ catalogId: id })) });
    render(<RepoDetail repoId="someone/alpha-GGUF" onBack={() => undefined} onStarted={() => undefined} />);
    await settle();
    const add = screen.getByRole("button", { name: "Add to comparison: Q4_K_M" }) as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    expect(screen.getByText(COMPARE_FULL)).toBeTruthy();
  });

  it("opens the model page only after the destination-confirm dialog", async () => {
    render(<RepoDetail repoId="someone/alpha-GGUF" onBack={() => undefined} onStarted={() => undefined} />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Open on Hugging Face" }));
    expect(openUrl).not.toHaveBeenCalled();
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("huggingface.co")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Open in browser" }));
    expect(openUrl).toHaveBeenCalledWith("https://huggingface.co/someone/alpha-GGUF");
  });
});

describe("Uploader text on Browse and a repository page", () => {
  it("names a license in words and makes hidden characters in it visible", async () => {
    ipcMock.hubSearch.mockResolvedValue({
      items: [
        { id: "someone/alpha-GGUF", downloads: 10, likes: 1, gated: "no", license: "apache-2.0", lastModified: null, pipelineTag: null },
        { id: "someone/beta-GGUF", downloads: 5, likes: 0, gated: "no", license: "free\u202Eesu", lastModified: null, pipelineTag: null },
      ],
      next: null,
    });
    render(<Browse onStarted={() => undefined} />);
    await settle();
    const rows = screen.getAllByRole("listitem");
    expect(rows[0]!.textContent).toContain("Apache 2.0");
    expect(rows[1]!.textContent).toContain("free[U+202E]esu");
    expect(document.body.textContent).not.toContain("\u202E");
  });

  it("shows the model card and the file's fields as text, with direction overrides visible", async () => {
    ipcMock.hubRepo.mockResolvedValue({
      ...VIEW,
      architecture: "arch\u200Bname",
      license: "other",
      variants: [variant("Q4\u202EK_M")],
      card: '<b>bold</b> <img src=x onerror="alert(1)"> safe\u202Etxt.exe ok \u{1F680}\uFE0F done',
    });
    render(<RepoDetail repoId="someone/alpha-GGUF" onBack={() => undefined} onStarted={() => undefined} />);
    await settle();
    expect(screen.getByText("arch[U+200B]name")).toBeTruthy();
    expect(screen.getByText("Another license (read the model card)")).toBeTruthy();
    expect(screen.getByText("Q4[U+202E]K_M")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add to comparison: Q4[U+202E]K_M" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Show model card" }));
    const card = document.querySelector("pre")!;
    expect(card.querySelector("b, img")).toBeNull();
    expect(card.textContent).toContain('<b>bold</b> <img src=x onerror="alert(1)">');
    expect(card.textContent).toContain("safe[U+202E]txt.exe");
    // An emoji keeps its presentation selector in a card.
    expect(card.textContent).toContain("\u{1F680}\uFE0F done");
    expect(document.body.textContent).not.toContain("\u202E");
    expect(document.body.textContent).not.toContain("\u200B");
  });

  it("shows a card written with Windows line endings without codes at line ends, and hides nothing in joiners", async () => {
    ipcMock.hubRepo.mockResolvedValue({
      ...VIEW,
      card: "# Title\r\nFirst line.\r\nSecond\u200D\u200C\uFE0E\uFE0F line \u{1F469}\u200D\u{1F4BB}\r\n",
    });
    render(<RepoDetail repoId="someone/alpha-GGUF" onBack={() => undefined} onStarted={() => undefined} />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Show model card" }));
    const card = document.querySelector("pre")!;
    expect(card.textContent).toBe(
      "# Title\nFirst line.\nSecond[U+200D][U+200C][U+FE0E][U+FE0F] line \u{1F469}\u200D\u{1F4BB}\n",
    );
  });

  it("says token counts with words", async () => {
    render(<RepoDetail repoId="someone/alpha-GGUF" onBack={() => undefined} onStarted={() => undefined} />);
    await settle();
    expect(screen.getByText("remembers up to 32,768 tokens, about 24,000 words")).toBeTruthy();
    expect(screen.getByText("Memory estimates assume the model remembers 8,192 tokens, about 6,000 words.")).toBeTruthy();
  });
});
