// Get models, Studio's full view (0.1.0 Task B13; 0.2.0 Task U5, design 10.3): it opens at its
// top without the composer or the side panel, says what this computer has as chips, searches
// Hugging Face with Pictures · Video · LoRAs and *Fits this computer* (off by default), and
// without a query lists Recommended for this computer, Popular on Hugging Face (asked once as it
// opens) and On this computer; result cards say what Studio can do with each in plain words.
// The engine's line and the content notice are ported from 0.1.x's view. A result card, or a
// catalog card's Download, opens the detail panel (Task U6, tested in ModelDetail.test.tsx).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { formatBytes } from "../../lib/format";
import type { DownloadGroup } from "../../lib/types";
import { LOCAL_MODELS } from "../../lib/words";
import { LOCAL } from "./fixtures/models";
import type {
  FileLabel,
  InstalledLocal,
  Licence,
  LocalEntry,
  LocalModels,
  LocalRepo,
  LocalSearchHit,
  Lora,
  Part,
  PlannedSlot,
  SetupPlan,
  Verdict,
} from "./types";

const ipc = vi.hoisted(() => ({
  isTauri: () => true,
  listDownloads: vi.fn<() => Promise<DownloadGroup[]>>(),
  onDownloadProgress: vi.fn<(handler: (g: DownloadGroup) => void) => Promise<() => void>>(),
  hubTokenStatus: vi.fn<() => Promise<{ saved: boolean; account: string | null }>>(),
  pauseDownload: vi.fn<(groupId: string) => Promise<void>>(),
  resumeDownload: vi.fn<(groupId: string) => Promise<void>>(),
  cancelDownload: vi.fn<(groupId: string) => Promise<void>>(),
}));
vi.mock("../../lib/ipc", () => ipc);
vi.mock("./api");

const api = await import("./api");
const { click, flush, showLocal } = await import("./testing/local");
const { useDownloads } = await import("../../store/downloads");
const { catalogShown, hardwareChips, resultShown } = await import("./GetModels");
const { gateProblem, licenceBadge } = await import("./ModelCard");
const { activeDownload } = await import("./DownloadState");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();
const GIB = 1024 ** 3;
const DAY = 86_400_000;

const klein = LOCAL.recommended.find((e) => e.id === "flux2-klein-4b-q4")!;
const zimage = LOCAL.recommended.find((e) => e.id === "z-image-turbo-q4")!;
const wan = LOCAL.recommended.find((e) => e.id === "wan22-ti2v-5b-fast-q6")!;
const apache = zimage.licence;

/** A licence of another kind than Apache-2.0's. */
function licence(patch: Partial<Licence>): Licence {
  return { ...apache, ...patch };
}

/** `entry` as one that may not fit this computer. */
function tooBig(entry: LocalEntry, reason = "needs 8 GB of graphics memory"): LocalEntry {
  return { ...entry, fit: { ...entry.fit, word: "notHere", verdict: "red", reason } };
}

/** The searched Z-Image GGUF's file: its three parts, the prompt reader already here from
 *  another model. */
const parts: Part[] = [
  { ...zimage.parts[0]!, repo: "someone/Z-Image-Turbo-GGUF", path: "z-image-turbo-Q5_K_M.gguf", sizeBytes: Math.round(4.3 * GIB) },
  { ...zimage.parts[1]!, sizeBytes: Math.round(2.5 * GIB), alreadyHere: true },
  { ...zimage.parts[2]!, sizeBytes: Math.round(0.3 * GIB) },
];

const slots: PlannedSlot[] = parts.map((part, i) => ({
  role: part.role,
  required: true,
  source: i === 0 ? "main" : part.alreadyHere ? "installed" : "companion",
  part,
}));

function plan(patch: Partial<SetupPlan> = {}): SetupPlan {
  return {
    recipe: { id: "z-image-turbo", name: "Z-Image Turbo", kind: "picture" },
    recipes: [{ id: "z-image-turbo", name: "Z-Image Turbo", kind: "picture" }],
    slots,
    licences: [apache],
    totalBytes: parts.reduce((n, p) => n + p.sizeBytes, 0),
    toFetchBytes: parts[0]!.sizeBytes + parts[2]!.sizeBytes,
    fit: { ...zimage.fit, verdict: "green" },
    canDownload: true,
    lowDisk: false,
    needsEngine: false,
    untested: false,
    ...patch,
  };
}

const works: FileLabel = {
  works: { version: "z-image", recipe: "z-image-turbo", kind: "picture", part: null, parameters: 6_150_000_000, folder: false },
};

function supported(patch: Partial<LocalRepo> = {}): LocalRepo {
  return {
    repo: "someone/Z-Image-Turbo-GGUF",
    commit: "c".repeat(40),
    author: "someone",
    summary: null,
    verdict: { works: { version: "z-image", kind: "picture", recipe: null } },
    files: [{ file: "z-image-turbo-Q5_K_M.gguf", quant: "Q5_K_M", label: works, plan: plan(), best: true }],
    loras: [],
    fileViews: [],
    licence: apache,
    licences: [apache],
    labelMismatch: null,
    gate: { mode: "none", collects: [] },
    readFiles: 1,
    unreadFiles: 0,
    byHandRecipes: [],
    ...patch,
  };
}

/** A search result for `repo`, offering what `verdict` says (from its name unless `exact`). */
function hit(repo: string, verdict: Verdict = "unknown", patch: Partial<LocalSearchHit> = {}): LocalSearchHit {
  return {
    repo,
    author: repo.split("/")[0]!,
    downloads: 12_345,
    likes: 10,
    updatedAt: null,
    gated: "none",
    licenseLabel: "apache-2.0",
    verdict,
    exact: false,
    ...patch,
  };
}

const sdxlHit = hit("stabilityai/stable-diffusion-xl-base-1.0", { works: { version: "sdxl", kind: "picture", recipe: null } }, {
  downloads: 1_912_440,
  likes: 6_804,
  updatedAt: new Date(Date.now() - 3 * DAY).toISOString(),
  exact: true,
});
const loraHit = hit("XLabs-AI/flux-lora-collection", { lora: { base: "flux" } }, { exact: true });
const luminaHit = hit("Alpha-VLLM/Lumina-Image-2.0", { cannotRun: { reason: "lumina2", version: null } }, { exact: true });

function typeIn(el: HTMLElement, text: string) {
  fireEvent.change(el, { target: { value: text } });
}

function searchBox(): HTMLElement {
  return screen.getByRole("searchbox", { name: "Search Hugging Face" });
}

/** Types `words` in the search and presses Enter (Hugging Face answers `found`). */
async function searchFor(words: string, found: LocalSearchHit[]) {
  vi.mocked(api.studioLocalSearch).mockResolvedValue({ items: found });
  typeIn(searchBox(), words);
  await act(async () => {
    fireEvent.submit(searchBox().closest("form")!);
  });
  await flush();
}

/** Searches Hugging Face for `repo`'s name, finds it, and opens it (`studioLocalRepo` gives
 *  `result`). Returns the slide-over it opens in. */
async function openResult(repo: string, result: LocalRepo = supported({ repo })) {
  vi.mocked(api.studioLocalRepo).mockResolvedValue(result);
  await searchFor(repo.split("/")[1]!, [hit(repo)]);
  await click(screen.getByRole("button", { name: repo }));
  expect(api.studioLocalRepo).toHaveBeenCalledWith(repo);
  return screen.getByRole("dialog", { name: repo });
}

function card(name: RegExp | string): HTMLElement {
  return screen.getByRole("article", { name });
}

function section(name: string): HTMLElement {
  return screen.getByRole("region", { name });
}

/** What the download manager's `download-progress` event tells the view, once it listens. */
let progress: (g: DownloadGroup) => void = () => undefined;

/** Z-Image Turbo's download, as the download manager reports it: a third done. */
function group(patch: Partial<DownloadGroup> = {}): DownloadGroup {
  return {
    id: "group-1",
    repo: "leejet/Z-Image-Turbo-GGUF",
    label: "Z-Image Turbo",
    fileCount: 3,
    bytesDone: 2 * GIB,
    bytesTotal: 6 * GIB,
    status: "downloading",
    error: null,
    bytesPerSecond: null,
    unverified: false,
    checking: false,
    generatorKind: "image",
    genModelId: "z-image-turbo-q4",
    ...patch,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  useDownloads.setState({ groups: [] });
  ipc.listDownloads.mockResolvedValue([]);
  ipc.hubTokenStatus.mockResolvedValue({ saved: false, account: null });
  ipc.onDownloadProgress.mockImplementation((handler) => {
    progress = handler;
    return Promise.resolve(() => undefined);
  });
});

afterEach(cleanup);

describe("the pieces", () => {
  it("this computer as chips: the graphics card and its memory, the computer's memory, free disk", () => {
    // Memory as the computer counts it (and the fit's reason says it); the free disk in the
    // cards' size format, since downloads fill it (412 GiB is 442.4 GB).
    expect(hardwareChips(LOCAL.hardware).map((c) => c.text)).toEqual([
      "Radeon RX 7600 · 8 GB graphics memory",
      "32 GB memory",
      "442.4 GB free",
    ]);
    const mac = { device: "Apple M5", graphicsBytes: Math.round(10.7 * GIB), sharedMemory: true, ramBytes: 16 * GIB, freeDiskBytes: 180e9 };
    // On shared memory, the computer's memory and never the graphics working set.
    expect(hardwareChips(mac).map((c) => c.text)).toEqual(["Apple M5 · 16 GB shared memory", "180.0 GB free"]);
    expect(hardwareChips({ ...mac, ramBytes: null }).map((c) => c.text)).toEqual(["Apple M5", "180.0 GB free"]);
    const cpu = { device: "Processor", graphicsBytes: null, sharedMemory: false, ramBytes: 16 * GIB, freeDiskBytes: null };
    expect(hardwareChips(cpu).map((c) => c.text)).toEqual(["Processor only", "16 GB memory"]);
  });

  it("the licence badge says what a licence allows in plain words, with its colour", () => {
    expect(licenceBadge(apache)).toEqual({ tone: "green", text: "Free for any use, including commercial" });
    expect(licenceBadge(licence({ badge: "amber", commercial: "underRevenue", revenueLimitUsd: 5_000_000 }))).toEqual({
      tone: "amber",
      text: "Commercial use only under $5M revenue",
    });
    expect(licenceBadge({ ...apache, excludedRegions: ["EU"] })).toEqual({ tone: "amber", text: "Commercial use in permitted regions" });
    expect(licenceBadge(licence({ badge: "amber", commercial: "withUseRules" }))).toEqual({
      tone: "amber",
      text: "Commercial use allowed, with use rules",
    });
    expect(licenceBadge(licence({ badge: "red", commercial: "no" }))).toEqual({ tone: "red", text: "Non-commercial" });
    // The backend sends `commercial` as a string: a newer one may send a word not known here.
    expect(licenceBadge(licence({ badge: "red", commercial: "somethingNew" as Licence["commercial"] }))).toEqual({
      tone: "red",
      text: "Check the licence",
    });
  });

  it("a model's download is the one running, not one done or cancelled", () => {
    for (const status of ["queued", "downloading", "paused", "verifying", "error", "cancelling"] as const) {
      expect(activeDownload([group({ status })], "z-image-turbo-q4")).toEqual(group({ status }));
    }
    expect(activeDownload([group({ status: "done" })], "z-image-turbo-q4")).toBeNull();
    expect(activeDownload([group({ status: "cancelled" })], "z-image-turbo-q4")).toBeNull();
    expect(activeDownload([group({ genModelId: null, generatorKind: null })], "z-image-turbo-q4")).toBeNull();
  });

  it("the catalog shown: its picks first, then while Fits this computer is off those that may not fit", () => {
    const local = { ...LOCAL, notHere: [tooBig({ ...wan, id: "big-video" })] };
    expect(catalogShown(local, "image", false)).toEqual([zimage, klein]);
    expect(catalogShown(local, "video", false)).toEqual([wan, ...local.notHere]);
    expect(catalogShown(local, "video", true)).toEqual([wan]);
  });

  it("Fits this computer hides an opened result with no file for this computer, never one not opened", () => {
    const none = supported({ files: supported().files.map((f) => ({ ...f, best: false })) });
    const h = hit(none.repo);
    expect(resultShown(h, {}, true)).toBe(true);
    expect(resultShown(h, { [none.repo]: none }, false)).toBe(true);
    expect(resultShown(h, { [none.repo]: none }, true)).toBe(false);
    expect(resultShown(h, { [none.repo]: supported() }, true)).toBe(true);
  });

  it("a gated repository whose files could not be read needs a token, or its terms", () => {
    const unread = (reason: "needsToken" | "needsTerms" | "budget"): LocalRepo =>
      supported({ files: [], fileViews: [{ path: "m.gguf", sizeBytes: 1, quant: null, label: { notRead: { reason } } }] });
    expect(gateProblem(unread("needsToken"))).toBe("hf_needs_token");
    expect(gateProblem(unread("needsTerms"))).toBe("hf_needs_terms");
    expect(gateProblem(unread("budget"))).toBeNull();
    expect(gateProblem(supported())).toBeNull();
  });

});

describe("Get models", () => {
  it("Get models opens as a full view at its top, without the composer or the side panel", async () => {
    const width = Object.getOwnPropertyDescriptor(window, "innerWidth");
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 1440 });
    try {
      await showLocal(LOCAL, { open: false });
      act(() => useStudio.setState({ settingsOpen: true }));
      screen.getByRole("textbox", { name: "Prompt" });
      expect(document.querySelector("[data-studio-panel]")).not.toBeNull();
      // The stream is scrolled to its newest card, far down.
      const main = document.querySelector<HTMLElement>("[data-studio-view] main")!;
      main.scrollTop = 900;
      await click(within(screen.getByRole("banner")).getByRole("button", { name: "Get models" }));
      expect(useStudio.getState().localView).toBe(true);
      expect(document.querySelector("[data-studio-view] main")).toBe(main);
      expect(main.scrollTop).toBe(0);
      expect(screen.queryByRole("textbox", { name: "Prompt" })).toBeNull();
      expect(screen.queryByRole("region", { name: "Make a picture" })).toBeNull();
      expect(document.querySelector("[data-studio-panel]")).toBeNull();
      expect(screen.getByRole("heading", { level: 2, name: "Get models" }).className).toContain("text-view-title");
      // The search and its filters are at the top, under the header.
      searchBox();
      expect(within(screen.getByRole("group", { name: "Show" })).getByRole("button", { name: "Pictures" }).getAttribute("aria-pressed")).toBe("true");
      // The saved choice stays: the panel is only hidden.
      expect(useStudio.getState().settingsOpen).toBe(true);
    } finally {
      if (width) Object.defineProperty(window, "innerWidth", width);
    }
  });

  it("Back returns to the results and the composer", async () => {
    await showLocal();
    expect(screen.queryByRole("textbox", { name: "Prompt" })).toBeNull();
    await click(screen.getByRole("button", { name: "Back" }));
    expect(useStudio.getState().localView).toBe(false);
    expect(document.querySelector("[data-card-stream]")).not.toBeNull();
    screen.getByRole("textbox", { name: "Prompt" });
    expect(screen.queryByRole("searchbox", { name: "Search Hugging Face" })).toBeNull();
    screen.getByRole("button", { name: "Grid" });
    expect(screen.getAllByRole("tablist")).toHaveLength(1);
    // Focus goes to the top bar's Get models, not lost with the Back button.
    expect(document.activeElement).toBe(within(screen.getByRole("banner")).getByRole("button", { name: "Get models" }));
  });

  it("without a query it shows Recommended, Popular on Hugging Face and On this computer, and asks for the popular list once", async () => {
    await showLocal(LOCAL, { popular: [sdxlHit, luminaHit] });
    expect(api.studioLocalSearch).toHaveBeenCalledTimes(1);
    expect(api.studioLocalSearch).toHaveBeenCalledWith("", "pictures");
    const sections = [...document.querySelectorAll("[data-get-models] > section[aria-labelledby]")];
    expect(sections.map((s) => document.getElementById(s.getAttribute("aria-labelledby")!)?.textContent)).toEqual([
      "Recommended for this computer",
      "Popular on Hugging Face",
      "On this computer",
    ]);
    // Recommended: the picture models of Studio's list, its pick first, each in its card.
    const recommended = section("Recommended for this computer");
    expect(within(recommended).getAllByRole("article").map((a) => a.getAttribute("aria-label"))).toEqual([
      "Z-Image Turbo",
      "FLUX.2 klein 4B",
    ]);
    within(within(recommended).getByRole("article", { name: "Z-Image Turbo" })).getByText("Top pick");
    // Popular: the cards of Hugging Face's most downloaded.
    within(section("Popular on Hugging Face")).getByRole("article", { name: sdxlHit.repo });
    within(section("Popular on Hugging Face")).getByRole("article", { name: luminaHit.repo });
    // On this computer: the picture models installed, and the LoRAs.
    const here = section("On this computer");
    within(here).getByRole("article", { name: "FLUX.2 klein 4B" });
    within(here).getByRole("article", { name: "sd_xl_base_1.0" });
    within(here).getByRole("article", { name: "pixel-art-xl" });
    expect(within(here).queryByRole("article", { name: "Wan 2.2 5B, fast" })).toBeNull();
    // Re-rendering asks nothing more; the Video chip asks for its own list once.
    act(() => useStudio.setState({ fitsOnly: true }));
    act(() => useStudio.setState({ fitsOnly: false }));
    await flush();
    expect(api.studioLocalSearch).toHaveBeenCalledTimes(1);
    await click(screen.getByRole("button", { name: "Video" }));
    expect(api.studioLocalSearch).toHaveBeenLastCalledWith("", "video");
    await click(screen.getByRole("button", { name: "Pictures" }));
    expect(api.studioLocalSearch).toHaveBeenCalledTimes(2);
    within(section("Popular on Hugging Face")).getByRole("article", { name: sdxlHit.repo });
  });

  it("each opening asks for the popular list again, with the filter from the tab", async () => {
    await showLocal();
    await click(screen.getByRole("button", { name: "Back" }));
    act(() => useStudio.getState().setTab("video"));
    await click(within(screen.getByRole("banner")).getByRole("button", { name: "Get models" }));
    expect(useStudio.getState().searchFilter).toBe("video");
    expect(api.studioLocalSearch).toHaveBeenNthCalledWith(2, "", "video");
    expect(api.studioLocalSearch).toHaveBeenCalledTimes(2);
    within(section("Recommended for this computer")).getByRole("article", { name: "Wan 2.2 5B, fast" });
    within(section("Recommended for this computer")).getByText("Local video is slower and simpler than cloud video, but free and private.");
  });

  it("the popular list shows two rows, and Show all shows the rest", async () => {
    const many = Array.from({ length: 8 }, (_, i) => hit(`someone/model-${i}`));
    await showLocal(LOCAL, { popular: many });
    const popular = section("Popular on Hugging Face");
    expect(within(popular).getAllByRole("article")).toHaveLength(6);
    await click(within(popular).getByRole("button", { name: "Show all 8" }));
    expect(within(popular).getAllByRole("article")).toHaveLength(8);
    within(popular).getByRole("button", { name: "Show fewer" });
  });

  it("the popular list that could not be read says why, and Try again asks again", async () => {
    vi.mocked(api.studioLocalSearch).mockReset();
    await showLocal();
    vi.mocked(api.studioLocalSearch).mockRejectedValueOnce({
      code: "hf_offline",
      message: "Hugging Face could not be reached.",
      nextStep: "Check your connection.",
      details: "",
    });
    await click(screen.getByRole("button", { name: "Video" }));
    within(section("Popular on Hugging Face")).getByText("Hugging Face could not be reached.");
    vi.mocked(api.studioLocalSearch).mockResolvedValue({ items: [hit("QuantStack/Wan2.2-TI2V-5B-GGUF")] });
    await click(within(section("Popular on Hugging Face")).getByRole("button", { name: "Try again" }));
    within(section("Popular on Hugging Face")).getByRole("article", { name: "QuantStack/Wan2.2-TI2V-5B-GGUF" });
  });

  describe("the popular list always ends in its list, its error with Try again, or a request on its way", () => {
    const offline = {
      code: "hf_offline",
      message: "Hugging Face could not be reached.",
      nextStep: "Check your connection.",
      details: "",
    };

    /** "Searching…" in the Popular section: shown only while its request is on its way. */
    function searching(): HTMLElement | null {
      return within(section("Popular on Hugging Face")).queryByRole("status");
    }

    it("(a) a failure dismissed asks again, rather than reading Searching… with nothing asked", async () => {
      await showLocal(LOCAL, { open: false });
      vi.mocked(api.studioLocalSearch).mockRejectedValueOnce(offline);
      await click(within(screen.getByRole("banner")).getByRole("button", { name: "Get models" }));
      within(section("Popular on Hugging Face")).getByText("Hugging Face could not be reached.");
      expect(api.studioLocalSearch).toHaveBeenCalledTimes(1);
      let answer: (page: { items: LocalSearchHit[] }) => void = () => undefined;
      vi.mocked(api.studioLocalSearch).mockImplementationOnce(() => new Promise((resolve) => (answer = resolve)));
      await click(within(section("Popular on Hugging Face")).getByRole("button", { name: "Dismiss" }));
      // Searching… is true now: a request for the list is on its way.
      expect(api.studioLocalSearch).toHaveBeenCalledTimes(2);
      expect(api.studioLocalSearch).toHaveBeenLastCalledWith("", "pictures");
      expect(searching()?.textContent).toBe("Searching…");
      await act(async () => answer({ items: [sdxlHit] }));
      await flush();
      expect(searching()).toBeNull();
      within(section("Popular on Hugging Face")).getByRole("article", { name: sdxlHit.repo });
    });

    it("(b) offline, each filter keeps its own failure with Try again, and going back asks nothing more", async () => {
      await showLocal(LOCAL, { open: false });
      vi.mocked(api.studioLocalSearch).mockRejectedValue(offline);
      await click(within(screen.getByRole("banner")).getByRole("button", { name: "Get models" }));
      within(section("Popular on Hugging Face")).getByRole("button", { name: "Try again" });
      await click(screen.getByRole("button", { name: "Video" }));
      within(section("Popular on Hugging Face")).getByRole("button", { name: "Try again" });
      await click(screen.getByRole("button", { name: "Pictures" }));
      expect(searching()).toBeNull();
      within(section("Popular on Hugging Face")).getByText("Hugging Face could not be reached.");
      within(section("Popular on Hugging Face")).getByRole("button", { name: "Try again" });
      // Asked once per filter while the view is open: Pictures, then Video.
      expect(vi.mocked(api.studioLocalSearch).mock.calls).toEqual([
        ["", "pictures"],
        ["", "video"],
      ]);
      // Back online, Try again gets the list.
      vi.mocked(api.studioLocalSearch).mockResolvedValue({ items: [sdxlHit] });
      await click(within(section("Popular on Hugging Face")).getByRole("button", { name: "Try again" }));
      within(section("Popular on Hugging Face")).getByRole("article", { name: sdxlHit.repo });
      await click(screen.getByRole("button", { name: "Video" }));
      within(section("Popular on Hugging Face")).getByRole("button", { name: "Try again" });
    });

    it("(c) a slow list whose answer is dropped for a search, after the field was emptied, is asked again", async () => {
      await showLocal(LOCAL, { open: false });
      let answerPopular: (page: { items: LocalSearchHit[] }) => void = () => undefined;
      vi.mocked(api.studioLocalSearch).mockImplementationOnce(() => new Promise((resolve) => (answerPopular = resolve)));
      await click(within(screen.getByRole("banner")).getByRole("button", { name: "Get models" }));
      expect(searching()?.textContent).toBe("Searching…");
      // A search asked meanwhile, then the field emptied again.
      await searchFor("x", [loraHit]);
      expect(api.studioLocalSearch).toHaveBeenLastCalledWith("x", "pictures");
      typeIn(searchBox(), "");
      await flush();
      // The slow list answers now: the store drops it, as a search was asked after it.
      vi.mocked(api.studioLocalSearch).mockResolvedValue({ items: [sdxlHit] });
      await act(async () => answerPopular({ items: [luminaHit] }));
      await flush();
      expect(vi.mocked(api.studioLocalSearch).mock.calls).toEqual([
        ["", "pictures"],
        ["x", "pictures"],
        ["", "pictures"],
      ]);
      expect(searching()).toBeNull();
      within(section("Popular on Hugging Face")).getByRole("article", { name: sdxlHit.repo });
    });
  });

  it("the header shows this computer as chips", async () => {
    await showLocal();
    const header = screen.getByRole("heading", { level: 2, name: "Get models" }).closest<HTMLElement>("[data-get-models-header]")!;
    const chips = within(header).getByRole("list", { name: "This computer" });
    expect(within(chips).getAllByRole("listitem").map((c) => c.textContent)).toEqual([
      "Radeon RX 7600 · 8 GB graphics memory",
      "32 GB memory",
      "442.4 GB free",
    ]);
    within(header).getByRole("button", { name: "Add a model file" });
    within(header).getByRole("button", { name: "Add a LoRA file" });
    within(header).getByRole("button", { name: "Back" });
  });

  it("Add a model file and Add a LoRA file open the dialog for each", async () => {
    await showLocal();
    vi.mocked(api.studioImportInspect).mockReturnValue(new Promise(() => undefined));
    await click(screen.getByRole("button", { name: "Add a LoRA file" }));
    expect(useStudio.getState().importOpen).toBe("lora");
    act(() => useStudio.getState().openImport(null));
    await click(screen.getByRole("button", { name: "Add a model file" }));
    expect(useStudio.getState().importOpen).toBe("model");
  });

  it("the LoRAs chip searches LoRAs", async () => {
    await showLocal();
    await click(screen.getByRole("button", { name: "LoRAs" }));
    expect(useStudio.getState().searchFilter).toBe("loras");
    expect(api.studioLocalSearch).toHaveBeenLastCalledWith("", "loras");
    // No catalog of LoRAs; this computer's LoRAs, and no models.
    expect(screen.queryByRole("region", { name: "Recommended for this computer" })).toBeNull();
    const here = section("On this computer");
    expect(within(here).getAllByRole("article").map((a) => a.getAttribute("aria-label"))).toEqual(["pixel-art-xl"]);
    // A search asks for LoRAs too, and the chip changes what an asked search looks for.
    await searchFor("pixel", [loraHit]);
    expect(api.studioLocalSearch).toHaveBeenLastCalledWith("pixel", "loras");
    await click(screen.getByRole("button", { name: "Pictures" }));
    expect(api.studioLocalSearch).toHaveBeenLastCalledWith("pixel", "pictures");
  });

  it("a result card shows author, downloads, likes, when it was updated and its verdict in words", async () => {
    await showLocal(LOCAL, { popular: [sdxlHit, loraHit, luminaHit] });
    const sdxl = card(sdxlHit.repo);
    // Its open control is the card's heading button, named by the repo.
    const open = within(sdxl).getByRole("button", { name: sdxlHit.repo });
    expect(open.closest("h3")).not.toBeNull();
    within(sdxl).getByText("stable-diffusion-xl-base-1.0");
    within(sdxl).getByText("stabilityai");
    within(sdxl).getByText("1.9M downloads");
    within(sdxl).getByText("6.8k likes");
    within(sdxl).getByText("updated 3 days ago");
    within(sdxl).getByText("Works in Studio · SDXL");
    within(card(loraHit.repo)).getByText("LoRA for FLUX.1");
    within(card(luminaHit.repo)).getByText("Can't run: Lumina 2");
    // No date known: no line for it.
    expect(within(card(loraHit.repo)).queryByText(/^updated/)).toBeNull();
  });

  it("a hint becomes exact once the result was opened", async () => {
    const hinted = hit("someone/Z-Image-Turbo-GGUF", { works: { version: "z-image", kind: "picture", recipe: null } });
    vi.mocked(api.studioLocalRepo).mockResolvedValue(supported());
    await showLocal(LOCAL, { popular: [hinted] });
    const badge = within(card(hinted.repo)).getByText("Works in Studio · Z-Image").closest("[data-verdict]")!;
    expect(badge.getAttribute("data-exact")).toBe("false");
    expect(badge.className).toContain("border-dashed");
    // Its tooltip, and its words for a screen reader, say where the hint comes from.
    expect(badge.textContent).toContain("From the name: opening it reads the files");
    await click(within(card(hinted.repo)).getByRole("button", { name: hinted.repo }));
    expect(api.studioLocalRepo).toHaveBeenCalledWith(hinted.repo);
    screen.getByRole("dialog", { name: hinted.repo });
    await click(screen.getByRole("button", { name: "Close" }));
    // Focus is back on the card's heading button.
    await waitFor(() => expect(document.activeElement).toBe(within(card(hinted.repo)).getByRole("button", { name: hinted.repo })));
    const exact = within(card(hinted.repo)).getByText("Works in Studio · Z-Image").closest("[data-verdict]")!;
    expect(exact.getAttribute("data-exact")).toBe("true");
    expect(exact.className).not.toContain("border-dashed");
    // A name that says nothing reads "Open to check".
    cleanup();
    useStudio.setState(INITIAL, true);
    await showLocal(LOCAL, { popular: [hit("someone/mystery")] });
    within(card("someone/mystery")).getByText("Open to check");
  });

  it("a result whose files all lack a part Studio can't find says so on its card once it was opened, never Works", async () => {
    const hinted = hit("someone/Z-Image-Turbo-GGUF", { works: { version: "z-image", kind: "picture", recipe: null } });
    vi.mocked(api.studioLocalRepo).mockResolvedValue(
      supported({ verdict: { needsFiles: { version: "z-image", kind: "picture", recipe: "z-image-turbo" } } }),
    );
    await showLocal(LOCAL, { popular: [hinted] });
    await click(within(card(hinted.repo)).getByRole("button", { name: hinted.repo }));
    await click(screen.getByRole("button", { name: "Close" }));
    const badge = within(card(hinted.repo)).getByText("Needs files · Z-Image").closest("[data-verdict]")!;
    expect(badge.getAttribute("data-verdict")).toBe("needsFiles");
    expect(badge.getAttribute("data-exact")).toBe("true");
    expect(badge.className).toContain("text-warning");
    expect(badge.className).not.toContain("text-success");
    expect(within(card(hinted.repo)).queryByText(/Works in Studio/)).toBeNull();
  });

  it("Fits this computer is off by default, so catalog models that may not fit are listed with Download", async () => {
    await showLocal({ ...LOCAL, recommended: LOCAL.recommended.filter((e) => e.id !== zimage.id), notHere: [tooBig(zimage)] });
    expect(useStudio.getState().fitsOnly).toBe(false);
    const fits = screen.getByRole("switch", { name: "Fits this computer" });
    expect(fits.getAttribute("aria-checked")).toBe("false");
    const z = within(section("Recommended for this computer")).getByRole("article", { name: "Z-Image Turbo" });
    within(z).getByText("May not fit");
    within(z).getByText("May not fit this computer: needs 8 GB of graphics memory. It may fail or be very slow.");
    // Download opens its detail panel, the warning said there too, Download kept (design 9.2).
    await click(within(z).getByRole("button", { name: "Download" }));
    expect(useStudio.getState().detail).toEqual({ entryId: zimage.id });
    const panel = screen.getByRole("dialog", { name: "Z-Image Turbo" });
    within(panel).getByText("May not fit this computer: needs 8 GB of graphics memory. It may fail or be very slow.");
    expect(within(panel).getByRole<HTMLButtonElement>("button", { name: "Download" }).disabled).toBe(false);
  });

  it("a card that may not fit shows the warning and still offers Download and Download again", async () => {
    const row = LOCAL.installed[0]!;
    const mayNot: InstalledLocal = { ...row, fit: { ...row.fit, word: "notHere", verdict: "red", reason: "needs 8 GB of graphics memory" } };
    await showLocal({
      ...LOCAL,
      recommended: LOCAL.recommended.filter((e) => e.id !== zimage.id),
      notHere: [tooBig(zimage)],
      installed: [mayNot, ...LOCAL.installed.slice(1)],
    });
    const warning = "May not fit this computer: needs 8 GB of graphics memory. It may fail or be very slow.";
    // The catalog card: the warning is a status line, Download is on.
    const z = within(section("Recommended for this computer")).getByRole("article", { name: "Z-Image Turbo" });
    expect(within(z).getByRole("status").textContent).toBe(warning);
    expect(within(z).getByRole<HTMLButtonElement>("button", { name: "Download" }).disabled).toBe(false);
    // The installed card: the same warning, and the button that downloads its files again.
    const k = within(section("On this computer")).getByRole("article", { name: row.name });
    expect(within(k).getByRole("status").textContent).toBe(warning);
    expect(within(k).getByRole<HTMLButtonElement>("button", { name: "Repair" }).disabled).toBe(false);
  });

  it("Fits this computer hides opened results that have no file for this computer, and catalog models that may not fit", async () => {
    const noFile = supported({ repo: "someone/too-big-GGUF", files: supported().files.map((f) => ({ ...f, best: false })) });
    const notOpened = hit("someone/not-opened");
    await showLocal(
      { ...LOCAL, recommended: LOCAL.recommended.filter((e) => e.id !== zimage.id), notHere: [tooBig(zimage)] },
      { popular: [hit(noFile.repo), notOpened, sdxlHit] },
    );
    vi.mocked(api.studioLocalRepo).mockResolvedValueOnce(noFile).mockResolvedValueOnce(supported({ repo: sdxlHit.repo }));
    await click(within(card(noFile.repo)).getByRole("button", { name: noFile.repo }));
    await click(screen.getByRole("button", { name: "Close" }));
    await click(within(card(sdxlHit.repo)).getByRole("button", { name: sdxlHit.repo }));
    await click(screen.getByRole("button", { name: "Close" }));
    await click(screen.getByRole("switch", { name: "Fits this computer" }));
    expect(useStudio.getState().fitsOnly).toBe(true);
    expect(screen.queryByRole("article", { name: "Z-Image Turbo" })).toBeNull();
    expect(screen.queryByRole("article", { name: noFile.repo })).toBeNull();
    card(notOpened.repo);
    card(sdxlHit.repo);
    // The installed models stay whatever their fit.
    within(section("On this computer")).getByRole("article", { name: "mystery" });
    // Off again, they are back.
    await click(screen.getByRole("switch", { name: "Fits this computer" }));
    card("Z-Image Turbo");
    card(noFile.repo);
  });

  // Final review, area 3, I4: Delete says truly what goes, for a model whose file is the
  // person's own (used where it is), one copied in, and one downloaded.
  it("Delete says the person's own file stays, and what is removed otherwise", async () => {
    const klein = LOCAL.installed.find((m) => m.id === "flux2-klein-4b-q4")!;
    const own: InstalledLocal = { ...klein, id: "local:own", name: "my model", source: "import", inPlace: true };
    const copied: InstalledLocal = { ...klein, id: "local:copy", name: "my copy", source: "import", inPlace: false };
    await showLocal({ ...LOCAL, installed: [own, copied, ...LOCAL.installed] });
    const here = section("On this computer");
    const bodyOf = async (name: string) => {
      await click(within(within(here).getByRole("article", { name })).getByRole("button", { name: "Delete" }));
      const asked = screen.getByRole("alertdialog");
      const text = asked.textContent ?? "";
      await click(within(asked).getByRole("button", { name: "Cancel" }));
      return text;
    };
    const ownText = await bodyOf("my model");
    expect(ownText).toContain(LOCAL_MODELS.deleteBodyOwn);
    expect(ownText).not.toContain(LOCAL_MODELS.deleteBody);
    expect(await bodyOf("my copy")).toContain(LOCAL_MODELS.deleteBodyCopy);
    expect(await bodyOf("FLUX.2 klein 4B")).toContain(LOCAL_MODELS.deleteBody);
  });

  it("installed models and LoRAs offer Delete, and models offer Repair", async () => {
    vi.mocked(api.studioLoraDelete).mockResolvedValue(undefined);
    await showLocal();
    const here = section("On this computer");
    const k = within(here).getByRole("article", { name: "FLUX.2 klein 4B" });
    within(k).getByText("FLUX.2 klein · picture");
    within(k).getByText(formatBytes(klein.sizeBytes));
    within(k).getByText("Fits");
    within(k).getByText("about 2 min a picture");
    await click(within(k).getByRole("button", { name: "Repair" }));
    expect(api.studioLocalDownload).toHaveBeenCalledWith({ installed: { genModelId: "flux2-klein-4b-q4" } }, null);
    within(k).getByText(LOCAL_MODELS.repairing);
    // An untested set-up says so.
    within(within(here).getByRole("article", { name: "flux-dev-copy" })).getByText("untested set-up");
    // One that needs setting up says why, and can only be deleted.
    const mystery = within(here).getByRole("article", { name: "mystery" });
    within(mystery).getByText("Needs setting up");
    within(mystery).getByText("Choose this model's type and files first.");
    expect(within(mystery).queryByRole("button", { name: "Repair" })).toBeNull();
    // Delete asks first, then the local models are read again.
    await click(within(mystery).getByRole("button", { name: "Delete" }));
    const asked = screen.getByRole("alertdialog");
    within(asked).getByText("Delete mystery?");
    await click(within(asked).getByRole("button", { name: "Delete" }));
    expect(api.studioLocalDelete).toHaveBeenCalledWith("hf:someone/mystery-model:mystery.safetensors");
    // A LoRA: what it is for, where it came from, and Delete (no Repair).
    const lora = within(here).getByRole("article", { name: "pixel-art-xl" });
    within(lora).getByText("LoRA for SDXL");
    within(lora).getByText("Added from this computer");
    expect(within(lora).queryByRole("button", { name: "Repair" })).toBeNull();
    await click(within(lora).getByRole("button", { name: "Delete" }));
    within(screen.getByRole("alertdialog")).getByText("Delete pixel-art-xl?");
    await click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete" }));
    expect(api.studioLoraDelete).toHaveBeenCalledWith(LOCAL.loras[0]!.id);
    expect(screen.queryByRole("article", { name: "pixel-art-xl" })).toBeNull();
    within(section("On this computer")).getByRole("article", { name: "FLUX.2 klein 4B" });
  });

  it("a LoRA added from the file chooser joins On this computer", async () => {
    await showLocal();
    const added: Lora = { ...LOCAL.loras[0]!, id: "local:" + "e".repeat(64), name: "watercolour" };
    vi.mocked(api.onImportProgress).mockResolvedValue(() => undefined);
    vi.mocked(api.studioLoraImport).mockResolvedValue(added);
    await act(async () => {
      await useStudio.getState().importLora("/home/me/watercolour.safetensors");
    });
    within(section("On this computer")).getByRole("article", { name: "watercolour" });
  });

  it("a delete refused while a run uses the model says what to do", async () => {
    await showLocal();
    vi.mocked(api.studioLocalDelete).mockRejectedValue({
      code: "studio_model_in_use",
      message: "A run on this computer is using this model.",
      nextStep: "Stop the run, then delete it.",
      details: "",
    });
    await click(within(within(section("On this computer")).getByRole("article", { name: "FLUX.2 klein 4B" })).getByRole("button", { name: "Delete" }));
    await click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete" }));
    screen.getByText("A run on this computer is using this model.");
    screen.getByText("Stop the run, then delete it.");
  });

  it("a catalog card shows what it is good at, size, licence in plain words, fit and time, and Download", async () => {
    await showLocal();
    // Installed: it says so, with no Download.
    const k = within(section("Recommended for this computer")).getByRole("article", { name: "FLUX.2 klein 4B" });
    within(k).getByText("Installed");
    expect(within(k).queryByRole("button", { name: "Download" })).toBeNull();
    const z = card("Z-Image Turbo");
    within(z).getByText("Photo-real pictures in 8 steps.");
    within(z).getByText(zimage.publisher);
    within(z).getByText(formatBytes(zimage.sizeBytes));
    within(z).getByText("Free for any use, including commercial");
    within(z).getByText("Fits");
    within(z).getByText("Time shown after your first run here");
    // Download opens the detail panel, the review before the download; nothing starts yet.
    await click(within(z).getByRole("button", { name: "Download" }));
    expect(useStudio.getState().detail).toEqual({ entryId: zimage.id });
    screen.getByRole("dialog", { name: "Z-Image Turbo" });
    expect(api.studioLocalDownload).not.toHaveBeenCalled();
  });

  it("a catalog model the person set up again as their own still shows as installed, with its name", async () => {
    // Ruling S9-edited-catalog-generic: its row keeps the entry's id but is generic now, with the
    // file's own name; the backend counts the entry installed by that id.
    const row = LOCAL.installed[0]!;
    const edited: InstalledLocal = { ...row, name: "flux-2-klein-4b-Q4_K_M", source: "import", recipe: { ...row.recipe!, id: "sdxl-checkpoint" } };
    await showLocal({ ...LOCAL, installed: [edited, ...LOCAL.installed.slice(1)] });
    const k = within(section("Recommended for this computer")).getByRole("article", { name: "FLUX.2 klein 4B" });
    within(k).getByText("Installed");
    expect(within(k).queryByRole("button", { name: "Download" })).toBeNull();
    within(section("On this computer")).getByRole("article", { name: "flux-2-klein-4b-Q4_K_M" });
  });

  it("a gated catalog model with no Hugging Face token says what to do, not Download", async () => {
    const gated: LocalEntry = { ...zimage, gate: { mode: "auto", collects: ["username", "email"] } };
    const local = { ...LOCAL, recommended: LOCAL.recommended.map((e) => (e.id === zimage.id ? gated : e)) };
    const opened: unknown[] = [];
    const onOpen = (e: Event) => opened.push((e as CustomEvent<unknown>).detail);
    window.addEventListener("app:open-settings", onOpen);
    try {
      await showLocal(local);
      expect(ipc.hubTokenStatus).toHaveBeenCalledTimes(1);
      const z = card("Z-Image Turbo");
      within(z).getByText("This model is gated. Add a Hugging Face token in Settings → Hugging Face first.");
      expect(within(z).queryByRole("button", { name: "Download" })).toBeNull();
      await click(within(z).getByRole("button", { name: "Open Settings" }));
      expect(opened).toEqual(["huggingface"]);
      // A token saved there (read again as Settings closes): Download.
      ipc.hubTokenStatus.mockResolvedValue({ saved: true, account: "me" });
      await act(async () => {
        window.dispatchEvent(new Event("app:settings-closed"));
      });
      await flush();
      within(card("Z-Image Turbo")).getByRole("button", { name: "Download" });
    } finally {
      window.removeEventListener("app:open-settings", onOpen);
    }
  });

  it("no gated model listed: whether a token is saved is never asked", async () => {
    await showLocal();
    expect(ipc.hubTokenStatus).not.toHaveBeenCalled();
  });

  it("a gated result, opened, says it needs a token or the model's terms, and offers no download that would fail", async () => {
    const unread = (reason: "needsToken" | "needsTerms"): LocalRepo =>
      supported({
        repo: "black-forest-labs/FLUX.1-Kontext-dev",
        files: [],
        gate: { mode: "auto", collects: [] },
        fileViews: [{ path: "flux1-kontext-dev.safetensors", sizeBytes: 23_800_000_000, quant: null, label: { notRead: { reason } } }],
        verdict: { works: { version: "flux", kind: "pictureEdit", recipe: null } },
      });
    await showLocal();
    const panel = await openResult("black-forest-labs/FLUX.1-Kontext-dev", unread("needsToken"));
    within(panel).getByText("This model is gated. Add a Hugging Face token in Settings → Hugging Face first.");
    within(panel).getByRole("button", { name: "Open Settings" });
    expect(within(panel).queryByRole("button", { name: "Download file anyway" })).toBeNull();
    expect(within(panel).getByRole<HTMLButtonElement>("button", { name: "Download" }).disabled).toBe(true);
    await click(within(panel).getByRole("button", { name: "Close" }));
    const result = card("black-forest-labs/FLUX.1-Kontext-dev");
    within(result).getByText("This model is gated. Add a Hugging Face token in Settings → Hugging Face first.");
    within(result).getByRole("button", { name: "Open Settings" });
    // Its terms not accepted: the model's page.
    vi.mocked(api.studioLocalRepo).mockResolvedValue(unread("needsTerms"));
    await click(within(result).getByRole("button", { name: "black-forest-labs/FLUX.1-Kontext-dev" }));
    await click(screen.getByRole("button", { name: "Close" }));
    within(card("black-forest-labs/FLUX.1-Kontext-dev")).getByText("Accept this model's terms on its Hugging Face page first.");
    expect(
      within(card("black-forest-labs/FLUX.1-Kontext-dev")).getByRole("link", { name: "Open the model page" }).getAttribute("href"),
    ).toBe("https://huggingface.co/black-forest-labs/FLUX.1-Kontext-dev");
  });

  it("a search asks Hugging Face only on Enter, shows what it found, and emptying the box goes back", async () => {
    await showLocal();
    expect(api.studioLocalSearch).toHaveBeenCalledTimes(1);
    screen.getByText("Searching sends your words to huggingface.co.");
    vi.mocked(api.studioLocalSearch).mockResolvedValue({ items: [sdxlHit] });
    typeIn(searchBox(), "sdxl");
    expect(api.studioLocalSearch).toHaveBeenCalledTimes(1);
    await act(async () => {
      fireEvent.submit(searchBox().closest("form")!);
    });
    await flush();
    expect(api.studioLocalSearch).toHaveBeenLastCalledWith("sdxl", "pictures");
    within(section("Found on Hugging Face")).getByRole("article", { name: sdxlHit.repo });
    expect(screen.queryByRole("region", { name: "Recommended for this computer" })).toBeNull();
    // The Search button asks too.
    await click(screen.getByRole("button", { name: "Search" }));
    expect(api.studioLocalSearch).toHaveBeenCalledTimes(3);
    typeIn(searchBox(), "");
    await flush();
    section("Recommended for this computer");
    expect(api.studioLocalSearch).toHaveBeenCalledTimes(3);
    // Nothing found: said so for the filter.
    await searchFor("nothing", []);
    within(section("Found on Hugging Face")).getByText("Hugging Face found no picture models for that.");
  });

  it("the header shows the engine, and the footer the content notice", async () => {
    await showLocal();
    const header = screen.getByRole("heading", { level: 2, name: "Get models" }).closest<HTMLElement>("[data-get-models-header]")!;
    within(header).getByText("Studio engine: built in");
    screen.getByText("Local models run only on this computer; no company checks prompts or results.");
    await click(screen.getByRole("button", { name: "What that means" }));
    expect(useStudio.getState().noticeOpen).toBe("read");
  });

  it("the header offers the engine again when it went missing", async () => {
    // klein is installed
    await showLocal({ ...LOCAL, engine: { ...LOCAL.engine, state: "notInstalled", downloadBytes: 30_303_000 } });
    const header = screen.getByRole("heading", { level: 2, name: "Get models" }).closest<HTMLElement>("[data-get-models-header]")!;
    await click(within(header).getByRole("button", { name: "Download the Studio engine (30 MB, MIT)" }));
    expect(api.studioEngineDownload).toHaveBeenCalledWith(true);
  });
});

describe("a model downloading", () => {
  it("after Download in its panel, its card shows how far the download got in one size format, and no Download", async () => {
    await showLocal();
    await click(within(card("Z-Image Turbo")).getByRole("button", { name: "Download" }));
    ipc.listDownloads.mockResolvedValue([group({ bytesDone: 0, status: "queued" })]);
    const panel = screen.getByRole("dialog", { name: "Z-Image Turbo" });
    await click(within(panel).getByRole("button", { name: "Download" }));
    // The panel shows it too, in place of its Download, with Pause and Cancel.
    within(panel).getByText("0 B of 6.4 GB · Waiting");
    expect(within(panel).queryByRole("button", { name: "Download" })).toBeNull();
    within(panel).getByRole("button", { name: "Pause downloading Z-Image Turbo" });
    within(panel).getByRole("button", { name: "Cancel downloading Z-Image Turbo" });
    fireEvent.keyDown(panel, { key: "Escape" });
    const z = card("Z-Image Turbo");
    expect(within(z).queryByRole("button", { name: "Download" })).toBeNull();
    within(z).getByText("0 B of 6.4 GB · Waiting");
    const bar = within(z).getByRole("progressbar", { name: "Downloading Z-Image Turbo" });
    expect(bar.getAttribute("aria-valuenow")).toBe("0");
    act(() => progress(group({ bytesDone: 3 * GIB, bytesPerSecond: 64 * 1024 * 1024 })));
    // One size format on the card: the progress's sizes are written as its size was.
    within(z).getByText(`${formatBytes(3 * GIB)} of ${formatBytes(6 * GIB)} · 48 s left`);
    expect(bar.getAttribute("aria-valuenow")).toBe("50");
    // Pause and Cancel on the card too.
    ipc.pauseDownload.mockResolvedValue(undefined);
    await click(within(z).getByRole("button", { name: "Pause downloading Z-Image Turbo" }));
    expect(ipc.pauseDownload).toHaveBeenCalledWith("group-1");
    act(() => progress(group({ bytesDone: 3 * GIB, status: "paused" })));
    within(z).getByText("3.2 GB of 6.4 GB · Paused");
    within(z).getByRole("button", { name: "Resume downloading Z-Image Turbo" });
    ipc.cancelDownload.mockResolvedValue(undefined);
    await click(within(z).getByRole("button", { name: "Cancel downloading Z-Image Turbo" }));
    expect(ipc.cancelDownload).toHaveBeenCalledWith("group-1");
    act(() => progress(group({ bytesDone: 0, status: "cancelled" })));
    within(card("Z-Image Turbo")).getByRole("button", { name: "Download" });
  });

  it("a download already running when the view opens shows, and once done the model is installed and ready to use", async () => {
    ipc.listDownloads.mockResolvedValue([group()]);
    await showLocal();
    const z = card("Z-Image Turbo");
    within(z).getByText("2.1 GB of 6.4 GB");
    const installed: LocalModels = {
      ...LOCAL,
      recommended: LOCAL.recommended.map((e) => (e.id === zimage.id ? { ...e, installed: true } : e)),
      installed: [{ ...LOCAL.installed[0]!, id: zimage.id, name: zimage.name }, ...LOCAL.installed],
    };
    vi.mocked(api.studioLocalModels).mockResolvedValue(installed);
    const reads = vi.mocked(api.studioLocalModels).mock.calls.length;
    act(() => progress(group({ status: "done", bytesDone: 6 * GIB })));
    await flush();
    expect(vi.mocked(api.studioLocalModels).mock.calls.length).toBe(reads + 1);
    const done = within(section("Recommended for this computer")).getByRole("article", { name: "Z-Image Turbo" });
    within(done).getByText("Ready");
    within(done).getByRole("button", { name: "Use it" });
    expect(within(done).queryByRole("progressbar")).toBeNull();
    within(section("On this computer")).getByRole("article", { name: "Z-Image Turbo" });
  });

  it("Repair on a model whose files are all on this computer says they were checked", async () => {
    // Final review, area 2, I1: nothing to fetch, so no download: the model was checked and
    // added again at once, and the card says so, never "downloading".
    await showLocal();
    vi.mocked(api.studioLocalDownload).mockResolvedValue(null);
    const k = within(section("On this computer")).getByRole("article", { name: "FLUX.2 klein 4B" });
    await click(within(k).getByRole("button", { name: "Repair" }));
    within(k).getByText(LOCAL_MODELS.repairedNow);
    expect(within(k).queryByText(LOCAL_MODELS.repairing)).toBeNull();
  });

  it("an installed model downloading again shows that in place of Repair", async () => {
    ipc.listDownloads.mockResolvedValue([
      group({ id: "group-2", genModelId: "flux2-klein-4b-q4", label: "FLUX.2 klein 4B", status: "paused" }),
    ]);
    await showLocal();
    const k = within(section("On this computer")).getByRole("article", { name: "FLUX.2 klein 4B" });
    within(k).getByText("2.1 GB of 6.4 GB · Paused");
    within(k).getByRole("button", { name: "Resume downloading FLUX.2 klein 4B" });
    within(k).getByRole("button", { name: "Cancel downloading FLUX.2 klein 4B" });
    expect(within(k).queryByRole("button", { name: "Repair" })).toBeNull();
    within(k).getByRole("button", { name: "Delete" });
  });

  it("a searched file downloading shows that in place of Download, and why it stopped", async () => {
    ipc.listDownloads.mockResolvedValue([
      group({
        genModelId: "hf:someone/Z-Image-Turbo-GGUF:z-image-turbo-Q5_K_M.gguf",
        status: "error",
        error: { code: "download_failed", message: "The download stopped.", nextStep: "Resume it in Models → Downloads.", details: "" },
      }),
    ]);
    await showLocal();
    const panel = await openResult("someone/Z-Image-Turbo-GGUF");
    expect(within(panel).queryByRole("button", { name: "Download" })).toBeNull();
    within(panel).getByText("2.1 GB of 6.4 GB · Stopped");
    within(panel).getByText("The download stopped. Resume it in Models → Downloads.");
    within(panel).getByRole("button", { name: "Resume downloading z-image-turbo-Q5_K_M.gguf" });
  });
});

/** `local` with model `id` installed (named `name`, of `kind`, with `fit`), as the backend lists
 *  it once its download is done; a catalog entry of that id reads installed too. */
function withInstalled(local: LocalModels, id: string, name: string, fit: InstalledLocal["fit"] = LOCAL.installed[0]!.fit): LocalModels {
  const row = LOCAL.installed[0]!;
  const added: InstalledLocal = { ...row, id, name, fit, model: { ...row.model, id, displayName: name } };
  return {
    ...local,
    recommended: local.recommended.map((e) => (e.id === id ? { ...e, installed: true } : e)),
    installed: [added, ...local.installed],
  };
}

describe("Use it (Task U7)", () => {
  it("a finished model's card offers Use it, which selects it and returns to the results", async () => {
    ipc.listDownloads.mockResolvedValue([group()]);
    await showLocal();
    expect(useStudio.getState().drafts.image.model).toBe("flux2-klein-4b-q4");
    // Done: the backend lists it now. It may not fit this computer: a warning, never a refusal.
    vi.mocked(api.studioLocalModels).mockResolvedValue(withInstalled(LOCAL, zimage.id, zimage.name, tooBig(zimage).fit));
    act(() => progress(group({ status: "done", bytesDone: 6 * GIB })));
    await flush();
    const z = within(section("Recommended for this computer")).getByRole("article", { name: "Z-Image Turbo" });
    within(z).getByText("Ready");
    await click(within(z).getByRole("button", { name: "Use it" }));
    expect(useStudio.getState()).toMatchObject({ localView: false, tab: "image", detail: null, doneDownloads: [] });
    expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: zimage.id });
    screen.getByRole("textbox", { name: "Prompt" });
    expect(screen.queryByRole("region", { name: "Recommended for this computer" })).toBeNull();
    // Used: no longer listed as finished, so the top bar has no indicator left.
    expect(document.querySelector("[data-download-indicator]")).toBeNull();
    // Back in Get models, the installed card still offers it.
    await click(within(screen.getByRole("banner")).getByRole("button", { name: "Get models" }));
    const again = within(section("Recommended for this computer")).getByRole("article", { name: "Z-Image Turbo" });
    within(again).getByText("Installed");
    within(again).getByRole("button", { name: "Use it" });
  });

  it("using a new model takes at most 4 clicks", async () => {
    const repo = "someone/Z-Image-Turbo-GGUF";
    const id = `hf:${repo}:z-image-turbo-Q5_K_M.gguf`;
    vi.mocked(api.studioLocalRepo).mockResolvedValue(supported({ repo }));
    // From the results, with the content notice already accepted.
    await showLocal({ ...LOCAL, noticeAccepted: true }, { open: false, popular: [hit(repo, { works: { version: "z-image", kind: "picture", recipe: null } })] });
    screen.getByRole("textbox", { name: "Prompt" });
    let clicks = 0;
    const press = async (el: HTMLElement) => {
      clicks++;
      await click(el);
    };
    // 1. Get models.
    await press(within(screen.getByRole("banner")).getByRole("button", { name: "Get models" }));
    // 2. A result card (Popular on Hugging Face: no typing needed).
    await press(within(section("Popular on Hugging Face")).getByRole("button", { name: repo }));
    const panel = screen.getByRole("dialog", { name: repo });
    // 3. Download.
    ipc.listDownloads.mockResolvedValue([group({ genModelId: id, label: "Z-Image Turbo", bytesDone: 0, status: "queued" })]);
    await press(within(panel).getByRole("button", { name: "Download" }));
    expect(api.studioLocalDownload).toHaveBeenCalledWith({ search: { repo, commit: "c".repeat(40), file: "z-image-turbo-Q5_K_M.gguf" } }, null);
    // The download finishes; the backend lists the model.
    vi.mocked(api.studioLocalModels).mockResolvedValue(withInstalled(LOCAL, id, "z-image-turbo-Q5_K_M"));
    act(() => progress(group({ genModelId: id, label: "Z-Image Turbo", status: "done", bytesDone: 6 * GIB })));
    await flush();
    within(panel).getByText("Ready");
    // 4. Use it.
    await press(within(panel).getByRole("button", { name: "Use it" }));
    expect(clicks).toBeLessThanOrEqual(4);
    expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: id });
    expect(useStudio.getState().localView).toBe(false);
    screen.getByRole("textbox", { name: "Prompt" });
    expect(screen.queryByRole("dialog", { name: repo })).toBeNull();
  });

  it("a download finished elsewhere lists the model and offers Use it", async () => {
    // Review Focus 3: Get models closed, the results shown.
    await showLocal(LOCAL, { open: false });
    expect(document.querySelector("[data-get-models]")).toBeNull();
    act(() => progress(group()));
    within(screen.getByRole("banner")).getByRole("button", { name: "1 downloading" });
    const reads = vi.mocked(api.studioLocalModels).mock.calls.length;
    vi.mocked(api.studioLocalModels).mockResolvedValue(withInstalled(LOCAL, zimage.id, zimage.name));
    act(() => progress(group({ status: "done", bytesDone: 6 * GIB })));
    await flush();
    // The local models were read again, so the model sheet lists it.
    expect(vi.mocked(api.studioLocalModels).mock.calls.length).toBe(reads + 1);
    const chip = document.querySelector<HTMLElement>("[data-model-chip] button")!;
    await click(chip);
    const sheet = screen.getByRole("dialog", { name: "Choose a model" });
    within(within(sheet).getByRole("region", { name: "On this computer" })).getByRole("button", { name: /^Z-Image Turbo/ });
    fireEvent.keyDown(sheet, { key: "Escape" });
    // Focus goes back to the chip once the sheet has gone (Radix does it on the next task).
    await waitFor(() => expect(document.activeElement).toBe(chip));
    // The top bar's indicator lists it, ready, with Use it.
    await click(within(screen.getByRole("banner")).getByRole("button", { name: "1 ready" }));
    const list = screen.getByRole("dialog", { name: "Downloads" });
    const row = list.querySelector<HTMLElement>('[data-download-row="group-1"]')!;
    expect(row.querySelector("p")?.textContent).toBe("Z-Image Turbo");
    within(row).getByText("Ready");
    await click(within(row).getByRole("button", { name: "Use it" }));
    expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: zimage.id });
    expect(screen.queryByRole("dialog", { name: "Downloads" })).toBeNull();
    expect(document.querySelector("[data-download-indicator]")).toBeNull();
  });

  it("a LoRA's download shows in the indicator, and once done the LoRAs are read again, with no Use it", async () => {
    const lora: Lora = { ...LOCAL.loras[0]!, id: "hf:XLabs-AI/flux-lora-collection:anime.safetensors", name: "anime", repo: "XLabs-AI/flux-lora-collection", source: "search" };
    const loraGroup = group({
      id: "group-lora",
      repo: "XLabs-AI/flux-lora-collection",
      label: "anime",
      bytesDone: 50_000_000,
      bytesTotal: 170_000_000,
      generatorKind: null,
      genModelId: null,
      loraId: lora.id,
    });
    ipc.listDownloads.mockResolvedValue([loraGroup]);
    await showLocal();
    await click(within(screen.getByRole("banner")).getByRole("button", { name: "1 downloading" }));
    let list = screen.getByRole("dialog", { name: "Downloads" });
    const row = list.querySelector<HTMLElement>('[data-download-row="group-lora"]')!;
    within(row).getByText("anime");
    within(row).getByText("LoRA");
    within(row).getByText("50 MB of 170 MB");
    within(row).getByRole("button", { name: "Pause downloading anime" });
    within(row).getByRole("button", { name: "Cancel downloading anime" });
    vi.mocked(api.studioLoras).mockResolvedValue([lora, ...LOCAL.loras]);
    const localReads = vi.mocked(api.studioLocalModels).mock.calls.length;
    act(() => progress({ ...loraGroup, status: "done", bytesDone: 170_000_000 }));
    await flush();
    expect(api.studioLoras).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.studioLocalModels).mock.calls.length).toBe(localReads);
    expect(useStudio.getState().loras.map((l) => l.id)).toContain(lora.id);
    // Get models' On this computer lists it.
    within(section("On this computer")).getByRole("article", { name: "anime" });
    list = screen.getByRole("dialog", { name: "Downloads" });
    within(list).getByText("Ready");
    // A LoRA is attached in Settings, not chosen as a model.
    expect(within(list).queryByRole("button", { name: "Use it" })).toBeNull();
    await click(within(list).getByRole("button", { name: "Clear finished" }));
    expect(useStudio.getState().doneDownloads).toEqual([]);
    expect(document.querySelector("[data-download-indicator]")).toBeNull();
  });
});
