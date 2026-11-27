import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { RepoSummary, SearchPage } from "../../lib/types";

// A search card for a model this version can't open yet. The owner opened the page of such a
// model to find out; the listing already says the kind, so the card says it first, as a hint:
// it comes from the repository's first file, and the page reads the files.

const ipcMock = vi.hoisted(() => ({
  isTauri: () => true,
  hubSearch: vi.fn<(text: string, sort: string, all: boolean) => Promise<SearchPage>>(),
  hubSearchNext: vi.fn(),
  hubRepo: vi.fn(),
  startDownload: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("../../store/app", () => ({
  toErrorInfo: (e: unknown) => ({ code: "unexpected", message: String(e), nextStep: "", details: "" }),
}));

const { Browse } = await import("./Browse");
const { useCompare } = await import("../../store/compare");

/** A search hit as the backend sends it (src-tauri `RepoSummary`, camelCase). */
function hit(id: string, patch: Partial<RepoSummary> = {}): RepoSummary {
  return {
    id,
    downloads: 39_177,
    likes: 44,
    gated: "no",
    license: "apache-2.0",
    lastModified: "2026-11-24T02:39:11.000Z",
    pipelineTag: "image-text-to-text",
    ...patch,
  };
}

const NEW = "someone/inkling-next-GGUF";
const OLD = "someone/qwen-ok-GGUF";

async function results(items: RepoSummary[]) {
  ipcMock.hubSearch.mockResolvedValue({ items, next: null });
  render(<Browse onStarted={() => undefined} />);
  await act(async () => {
    await new Promise((r) => setTimeout(r, 5));
  });
}

function card(id: string): HTMLElement {
  const li = screen.getByText(id).closest("li");
  if (!li) throw new Error(`no card for ${id}`);
  return li;
}

beforeEach(() => {
  ipcMock.hubRepo.mockResolvedValue(null);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  useCompare.setState({ entries: [], auto: [], ctx: null, repos: {} });
});

describe("a search card for a model this version can't open yet", () => {
  it("says so on the card, as a hint, so the person need not open it to find out", async () => {
    await results([hit(NEW, { unknownArchitecture: "futurearch" }), hit(OLD)]);
    const badge = within(card(NEW)).getByText("Can't open yet").closest("[data-hint]") as HTMLElement;
    expect(badge.getAttribute("data-hint")).toBe("listing");
    // A hint is a guess from the listing: its tooltip says "may", and the page checks.
    const tip =
      "From the listing: this version of harnessUI may not know this kind of model (“futurearch”). Open it to check.";
    expect(badge.getAttribute("title")).toBe(tip);
    // A screen reader gets the same sentence, which a tooltip would not give it.
    expect(badge.textContent).toContain(tip);
    // The other card says nothing.
    expect(within(card(OLD)).queryByText("Can't open yet")).toBeNull();
  });

  it("says nothing when the listing names no kind, or one that is known", async () => {
    await results([hit(NEW, { unknownArchitecture: null }), hit(OLD, { unknownArchitecture: undefined })]);
    expect(screen.queryByText("Can't open yet")).toBeNull();
  });

  it("still opens the page, and keeps Add to comparison", async () => {
    await results([hit(NEW, { unknownArchitecture: "futurearch" })]);
    const li = card(NEW);
    fireEvent.click(within(li).getByRole("button", { name: "Add to comparison: someone/inkling-next-GGUF" }));
    expect(useCompare.getState().entries).toEqual([{ repo: NEW, file: null }]);
    fireEvent.click(within(li).getByRole("button", { name: new RegExp(`^${NEW}`) }));
    expect(ipcMock.hubRepo).toHaveBeenCalledWith(NEW, null);
  });

  it("shows the kind's own text with its invisible characters made visible", async () => {
    await results([hit(NEW, { unknownArchitecture: "fu‮ture" })]);
    expect(within(card(NEW)).getByText("Can't open yet").closest("[data-hint]")?.getAttribute("title")).toContain(
      "(“fu[U+202E]ture”)",
    );
  });

  it("does not claim more than the listing knows: no 'not supported', no 'will'", async () => {
    await results([hit(NEW, { unknownArchitecture: "futurearch" })]);
    expect(card(NEW).textContent).not.toMatch(/not supported|cannot load|llama|architecture|\bwill\b/i);
  });
});

describe("a long repository name on a card", () => {
  /** The owner's own photo cut the name off ("...Brilliance-50X-V2-AGI...") with no way to read it. */
  it("can be read in full: the name carries itself as its tooltip", async () => {
    const long = "DavidAU/Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-GGUF";
    await results([hit(long)]);
    expect(screen.getByText(long).getAttribute("title")).toBe(long);
  });
});
