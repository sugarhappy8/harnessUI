// Studio's gallery grid (0.1.0 Task B9; studio-design 5): every result as a tile (its
// thumbnail; a GIF, an SVG or a picture without one shown from its file; a video without one
// by its length and start frame), filtered by Pictures, Video, Starred, date and search, the
// next page on Load more; and several results selected to delete or save together.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { LOCAL, LOCAL_RUN, MODELS, RUN } from "./fixtures/models";
import type { Output, Run, RunsCursor, StudioEndpoint, StudioPrefs, Usage } from "./types";

// Radix positions menus with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));

const api = await import("./api");
const dialog = await import("@tauri-apps/plugin-dialog");
const { Grid } = await import("./Grid");
const { StudioView } = await import("./StudioView");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

const openrouter: StudioEndpoint = {
  providerId: 1,
  name: "OpenRouter",
  company: "openrouter",
  host: "openrouter.ai",
  cloud: true,
  hasKey: true,
  fetchedAt: new Date(2026, 10, 17, 9).getTime(),
  models: MODELS,
};

function prefs(): StudioPrefs {
  return {
    lastModel: {},
    advancedOpen: false,
    confirmEveryVideo: true,
    confirmAtUsd: 1,
    dailyLimitUsd: 10,
    setups: [],
    galleryDir: null,
    gpuPolicy: "unload",
    reloadChatAfter: false,
    suggestionDismissed: true,
    localNoticeAcceptedAt: null,
  };
}

function usage(): Usage {
  return { spentTodayUsd: 0, pendingUsd: 0, limitUsd: 10, folder: "/home/me/harnessUI/studio", bytes: 0, pictures: 0, videos: 0 };
}

/** One result of `runId`. */
function output(runId: string, patch: Partial<Output> = {}): Output {
  return {
    id: `${runId}-0`,
    runId,
    idx: 0,
    mime: "image/png",
    sizeBytes: 1000,
    width: 512,
    height: 512,
    durationMs: null,
    seed: null,
    starred: false,
    hasThumb: true,
    path: `/home/me/harnessUI/studio/2026-11/${runId}-0.png`,
    ...patch,
  };
}

const START_FRAME = "d".repeat(64);
/** An animated GIF, which has no thumbnail. */
const GIF: Run = {
  ...RUN,
  id: "gif-run",
  createdAt: RUN.createdAt - 1000,
  outputs: [output("gif-run", { id: "g1", mime: "image/gif", hasThumb: false })],
};
/** A video made from a start frame, its poster frame not saved yet. */
const VIDEO: Run = {
  ...RUN,
  id: "video-run",
  kind: "video",
  model: "google/veo-3.1",
  modelLabel: "Veo 3.1",
  prompt: "Waves on the rocks",
  createdAt: RUN.createdAt - 2000,
  refs: [{ sha256: START_FRAME, mime: "image/png", sizeBytes: 2000, role: "startFrame" }],
  outputs: [output("video-run", { id: "v1", mime: "video/mp4", hasThumb: false, durationMs: 8000 })],
};

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
  await flush();
}

/** Studio shown with the grid on, the gallery's first page `runs` (next page at `next`). */
async function showGrid(runs: Run[] = [RUN, LOCAL_RUN, GIF, VIDEO], next: RunsCursor | null = null) {
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [openrouter] });
  vi.mocked(api.studioLocalModels).mockResolvedValue(LOCAL);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs, next });
  vi.mocked(api.studioLive).mockResolvedValue([]);
  vi.mocked(api.studioGetPrefs).mockResolvedValue(prefs());
  vi.mocked(api.studioUsage).mockResolvedValue(usage());
  vi.mocked(api.studioEstimate).mockResolvedValue({
    usd: 0,
    reasons: [],
    confirmAtUsd: 1,
    limit: { limitUsd: 10, spentTodayUsd: 0, pendingUsd: 0, wouldPass: true },
    invalid: [],
    eta: null,
    local: null,
  });
  useStudio.getState().showStudio();
  await act(async () => {
    render(<StudioView />);
  });
  await flush();
  await click(screen.getByRole("button", { name: "Grid" }));
}

function tile(outputId: string): HTMLElement {
  const found = document.querySelector<HTMLElement>(`[data-output-tile="${outputId}"]`);
  if (!found) throw new Error(`no tile for ${outputId}`);
  return found;
}

async function selectTiles(...ids: string[]) {
  for (const id of ids) await click(within(tile(id)).getByRole("checkbox"));
}

async function confirmDialog() {
  await click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete" }));
}

function typeSearch(text: string) {
  fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), { target: { value: text } });
}

function pickDate(label: string) {
  const select = screen.getByRole("combobox", { name: "When" }) as HTMLSelectElement;
  const option = [...select.options].find((o) => o.text === label);
  if (!option) throw new Error(`no date choice ${label}`);
  fireEvent.change(select, { target: { value: option.value } });
}

const original = { create: URL.createObjectURL, revoke: URL.revokeObjectURL };

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  URL.createObjectURL = vi.fn(() => "blob:tile");
  URL.revokeObjectURL = vi.fn();
  vi.mocked(api.studioOutputBytes).mockResolvedValue(new ArrayBuffer(8));
  vi.mocked(api.studioRefBytes).mockResolvedValue(new ArrayBuffer(8));
});

afterEach(() => {
  cleanup();
  URL.createObjectURL = original.create;
  URL.revokeObjectURL = original.revoke;
});

describe("Grid", () => {
  it("shows every result of every kind as a tile, newest first", async () => {
    await showGrid();
    const tiles = [...document.querySelectorAll("[data-output-tile]")].map((t) => t.getAttribute("data-output-tile"));
    expect(tiles).toEqual(["l1", "l2", "o1", "o2", "o3", "o4", "g1", "v1"]);
    // The card stream gives way to the grid.
    expect(document.querySelector("[data-run-card]")).toBeNull();
  });

  it("a tile uses the thumbnail, else the file (GIF), else for a video its length and start frame", async () => {
    await showGrid();
    expect(api.studioOutputBytes).toHaveBeenCalledWith("o1", true);
    expect(api.studioOutputBytes).toHaveBeenCalledWith("g1", false);
    expect(api.studioOutputBytes).not.toHaveBeenCalledWith("v1", expect.anything());
    expect(api.studioRefBytes).toHaveBeenCalledWith(START_FRAME);
    within(tile("v1")).getByText("0:08");
  });

  it("clicking a tile opens it", async () => {
    const onOpen = vi.fn();
    useStudio.setState({ runs: [RUN] });
    await act(async () => {
      render(<Grid onOpen={onOpen} />);
    });
    await flush();
    await click(within(tile("o2")).getByRole("button", { name: /^Open picture 2 of 4/ }));
    expect(onOpen).toHaveBeenCalledWith("o2");
  });

  it("in Studio a tile opens the viewer", async () => {
    await showGrid();
    await click(within(tile("o3")).getByRole("button", { name: /^Open picture 3 of 4/ }));
    expect(useStudio.getState().viewer).toBe("o3");
    screen.getByText("3 / 4");
  });

  it("the grid filters by Pictures, Video, Starred, date and search", async () => {
    await showGrid();
    await click(screen.getByRole("button", { name: "Video" }));
    expect(api.studioRuns).toHaveBeenLastCalledWith(expect.objectContaining({ kind: "video" }), null, 60);
    typeSearch("waves");
    await waitFor(() =>
      expect(api.studioRuns).toHaveBeenLastCalledWith(expect.objectContaining({ text: "waves" }), null, 60),
    );
    pickDate("Last 7 days");
    await flush();
    expect(api.studioRuns).toHaveBeenLastCalledWith(
      expect.objectContaining({ kind: "video", text: "waves", since: expect.any(Number) }),
      null,
      60,
    );
    const since = useStudio.getState().filter.since!;
    const weekAgo = new Date();
    weekAgo.setHours(0, 0, 0, 0);
    weekAgo.setDate(weekAgo.getDate() - 6);
    expect(since).toBe(weekAgo.getTime());
    await click(screen.getByRole("button", { name: "Starred" }));
    expect(api.studioRuns).toHaveBeenLastCalledWith(expect.objectContaining({ starred: true }), null, 60);
    await click(screen.getByRole("button", { name: "All" }));
    expect(useStudio.getState().filter.kind).toBeNull();
  });

  it("leaving the grid puts its Pictures or Video filter away, so the cards follow the tab", async () => {
    await showGrid();
    await click(screen.getByRole("button", { name: "Video" }));
    expect(useStudio.getState().filter.kind).toBe("video");
    await click(screen.getByRole("button", { name: "Grid" }));
    expect(useStudio.getState().filter.kind).toBeNull();
    expect(api.studioRuns).toHaveBeenLastCalledWith(expect.objectContaining({ kind: null }), null, 60);
  });

  it("Load more reads the next page from where the last one ended", async () => {
    const next: RunsCursor = { createdAt: VIDEO.createdAt, id: VIDEO.id };
    await showGrid([RUN, VIDEO], next);
    const older: Run = { ...GIF, createdAt: VIDEO.createdAt - 1000 };
    vi.mocked(api.studioRuns).mockResolvedValue({ runs: [older], next: null });
    await click(screen.getByRole("button", { name: "Load more" }));
    expect(api.studioRuns).toHaveBeenLastCalledWith({}, next, 60);
    tile("g1");
    expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
  });

  it("several can be selected and deleted together", async () => {
    await showGrid();
    vi.mocked(api.studioDelete).mockResolvedValue(undefined);
    await selectTiles("o1", "o2");
    await click(screen.getByRole("button", { name: "Delete 2" }));
    screen.getByText(
      "The audit log keeps a line saying a run happened (model and cost), but not your prompt. It still counts toward today's spending.",
    );
    await confirmDialog();
    expect(api.studioDelete).toHaveBeenCalledWith(["o1", "o2"]);
    expect(document.querySelector('[data-output-tile="o1"]')).toBeNull();
    expect(document.querySelector('[data-output-tile="o2"]')).toBeNull();
    tile("o3");
    expect(screen.queryByRole("button", { name: /^Delete \d/ })).toBeNull();
  });

  it("deleting only local results says only what the audit log keeps", async () => {
    await showGrid();
    await selectTiles("l1", "l2");
    await click(screen.getByRole("button", { name: "Delete 2" }));
    screen.getByText("The audit log keeps a line saying a run happened (model and cost), but not your prompt.");
    expect(screen.queryByText(/today's spending/)).toBeNull();
  });

  it("several can be saved together into one folder", async () => {
    await showGrid();
    vi.mocked(api.studioExport).mockImplementation(async (_id, path) => path);
    await selectTiles("o1", "o2");
    vi.mocked(dialog.open).mockResolvedValue("/tmp/out");
    await click(screen.getByRole("button", { name: "Save 2…" }));
    await waitFor(() => expect(api.studioExport).toHaveBeenCalledTimes(2));
    expect(dialog.open).toHaveBeenCalledWith(expect.objectContaining({ directory: true }));
    expect(api.studioExport).toHaveBeenCalledWith("o1", expect.stringMatching(/^\/tmp\/out\//), false);
    expect(api.studioExport).toHaveBeenCalledWith("o1", "/tmp/out/cloud-run-0.png", false);
    expect(api.studioExport).toHaveBeenCalledWith("o2", "/tmp/out/cloud-run-1.png", false);
  });

  it("a folder picker that is cancelled saves nothing", async () => {
    await showGrid();
    await selectTiles("o1");
    vi.mocked(dialog.open).mockResolvedValue(null);
    await click(screen.getByRole("button", { name: "Save 1…" }));
    expect(api.studioExport).not.toHaveBeenCalled();
  });

  it("an SVG tile is an img element only", async () => {
    const svg: Run = { ...GIF, id: "svg-run", outputs: [output("svg-run", { id: "s1", mime: "image/svg+xml", hasThumb: false })] };
    await showGrid([svg]);
    expect(api.studioOutputBytes).toHaveBeenCalledWith("s1", false);
    expect(tile("s1").querySelector("object, iframe, embed")).toBeNull();
    expect(tile("s1").querySelector("img")).not.toBeNull();
  });
});
