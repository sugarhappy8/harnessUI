// Studio's view (0.1.0 Task B2; 0.2.0 Task U3): the frame (the top bar, the results with the
// composer at their foot, and the side panel, closed until asked for), the empty states and the
// default model, local first, and the saved lists, read without a fetch.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { formatBytes } from "../../lib/format";
import type { DownloadGroup } from "../../lib/types";
import { LOCAL, MODELS, RUN } from "./fixtures/models";
import type { LocalModels, Run, StudioEndpoint, StudioPrefs, Usage } from "./types";

// Radix positions menus with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const ipc = vi.hoisted(() => ({
  isTauri: () => true,
  listDownloads: vi.fn<() => Promise<DownloadGroup[]>>(),
  onDownloadProgress: vi.fn<(handler: (g: DownloadGroup) => void) => Promise<() => void>>(),
  pauseDownload: vi.fn<(groupId: string) => Promise<void>>(),
  resumeDownload: vi.fn<(groupId: string) => Promise<void>>(),
  cancelDownload: vi.fn<(groupId: string) => Promise<void>>(),
}));
vi.mock("../../lib/ipc", () => ipc);
vi.mock("./api");

const api = await import("./api");
const { StudioView } = await import("./StudioView");
const { UiProviders } = await import("../ui/UiProviders");
const { defaultModel, emptyState, useStudio } = await import("../../store/studio");
const { useDownloads } = await import("../../store/downloads");

const INITIAL = useStudio.getState();
/** The first recommended entry: FLUX.2 klein 4B, about 5.3 GB. */
const klein = LOCAL.recommended[0]!;

/** An endpoint as `studio_models` gives it: OpenRouter with the fixture's models, or OpenAI
 *  (pictures only) with its own name for GPT Image 2.6 Flare. */
function endpoint(patch: Partial<StudioEndpoint> = {}): StudioEndpoint {
  const openai = patch.company === "openai";
  return {
    providerId: 1,
    name: openai ? "OpenAI" : "OpenRouter",
    company: "openrouter",
    host: openai ? "api.openai.com" : "openrouter.ai",
    cloud: true,
    hasKey: true,
    fetchedAt: new Date(2026, 10, 17, 9).getTime(),
    models: openai ? [{ ...MODELS[0]!, id: "gpt-image-2.6-flare" }] : MODELS,
    ...patch,
  };
}
const openrouter = endpoint();

function prefs(patch: Partial<StudioPrefs> = {}): StudioPrefs {
  return {
    lastModel: {},
    advancedOpen: false,
    settingsOpen: false,
    confirmEveryVideo: true,
    confirmAtUsd: 1,
    dailyLimitUsd: 10,
    setups: [],
    galleryDir: null,
    gpuPolicy: "unload",
    reloadChatAfter: false,
    suggestionDismissed: true,
    localNoticeAcceptedAt: null,
    lastImportMode: null,
    ...patch,
  };
}

function usage(patch: Partial<Usage> = {}): Usage {
  return {
    spentTodayUsd: 0,
    pendingUsd: 0,
    limitUsd: 10,
    folder: "/home/me/harnessUI/studio",
    bytes: 0,
    pictures: 0,
    videos: 0,
    ...patch,
  };
}

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

/** Opens Studio with what the backend has saved: App mounts the view when it is first shown. */
async function show(
  saved: {
    endpoints?: StudioEndpoint[];
    local?: LocalModels;
    prefs?: StudioPrefs;
    usage?: Usage;
    runs?: Run[];
    live?: Run[];
  } = {},
) {
  const endpoints = saved.endpoints ?? [openrouter];
  const p = saved.prefs ?? prefs();
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints });
  vi.mocked(api.studioRefreshModels).mockResolvedValue({ endpoints });
  vi.mocked(api.studioLocalModels).mockResolvedValue(saved.local ?? LOCAL);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs: saved.runs ?? [], next: null });
  vi.mocked(api.studioLive).mockResolvedValue(saved.live ?? []);
  vi.mocked(api.studioGetPrefs).mockResolvedValue(p);
  vi.mocked(api.studioSetPrefs).mockImplementation(async (patch) => ({ ...p, ...patch }));
  vi.mocked(api.studioUsage).mockResolvedValue(saved.usage ?? usage());
  // The side panel's Files section reads a local model's files (Task U8; nothing is sent).
  vi.mocked(api.studioModelFiles).mockImplementation(async (genModelId) => ({
    genModelId,
    recipe: null,
    recipes: [],
    slots: [],
    untested: false,
    canRun: true,
  }));
  // Get local models' footer follows the engine's download (Task B13).
  vi.mocked(api.onEngineProgress).mockResolvedValue(() => undefined);
  // The prompt dock asks for the draft's estimate (Task B7).
  vi.mocked(api.studioEstimate).mockResolvedValue({
    usd: 0,
    reasons: [],
    confirmAtUsd: 1,
    limit: { limitUsd: 10, spentTodayUsd: 0, pendingUsd: 0, wouldPass: true, refusal: null, reserveUsd: 5 },
    invalid: [],
    eta: null,
    local: null,
  });
  useStudio.getState().showStudio();
  await act(async () => {
    render(
      <UiProviders>
        <StudioView />
      </UiProviders>,
    );
  });
  await flush();
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
  await flush();
}

async function openMenu() {
  await act(async () => {
    fireEvent.keyDown(screen.getByRole("button", { name: "Studio menu" }), { key: "Enter" });
  });
}

function promptBox(): HTMLTextAreaElement {
  return screen.getByRole("textbox", { name: "Prompt" }) as HTMLTextAreaElement;
}

function typePrompt(text: string) {
  fireEvent.change(promptBox(), { target: { value: text } });
}

/** What each `app:open-settings` asked for (its detail). */
let opened: unknown[] = [];
const onOpenSettings = (e: Event) => opened.push((e as CustomEvent<unknown>).detail);

/** What the download manager's `download-progress` event tells Studio, once it listens. */
let progress: (g: DownloadGroup) => void = () => undefined;

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  useDownloads.setState({ groups: [] });
  ipc.listDownloads.mockResolvedValue([]);
  ipc.onDownloadProgress.mockImplementation((handler) => {
    progress = handler;
    return Promise.resolve(() => undefined);
  });
  opened = [];
  window.addEventListener("app:open-settings", onOpenSettings);
});

afterEach(() => {
  cleanup();
  window.removeEventListener("app:open-settings", onOpenSettings);
  vi.useRealTimers();
});

describe("StudioView", () => {
  it("opening Studio reads only what is saved", async () => {
    await show();
    expect(api.studioModels).toHaveBeenCalledTimes(1);
    expect(api.studioLocalModels).toHaveBeenCalledTimes(1);
    expect(api.studioRefreshModels).not.toHaveBeenCalled();
    expect(api.studioModelPrice).not.toHaveBeenCalled();
    expect(api.studioLocalSearch).not.toHaveBeenCalled();
  });

  it("with nothing set up, Studio leads with local models", async () => {
    // getLocal
    await show({ endpoints: [], local: { ...LOCAL, installed: [] } });
    screen.getByText("Make pictures and videos on this computer: free, and nothing is sent.");
    screen.getByText(`${klein.name} fits this computer (${formatBytes(klein.sizeBytes)}).`);
    screen.getByText("No good graphics card? Cloud models are the backup: OpenRouter or OpenAI."); // second, smaller
    await click(screen.getByRole("button", { name: "Open Endpoints" }));
    expect(opened).toEqual(["endpoints"]); // app:open-settings detail
    // Get models takes the empty state's place (Task B13), opened on the model the card names,
    // in its detail panel (Task U9).
    vi.mocked(api.studioLocalSearch).mockResolvedValue({ items: [] });
    const main = screen.getByRole("main");
    await click(within(main).getByRole("button", { name: `Get ${klein.name}` }));
    expect(useStudio.getState().localView).toBe(true);
    expect(useStudio.getState().detail).toEqual({ entryId: klein.id });
    expect(screen.queryByText("Make pictures and videos on this computer: free, and nothing is sent.")).toBeNull();
  });

  it("each empty state follows one ordered rule", () => {
    const none = { ...LOCAL, installed: [] };
    const nothingFits = { ...none, recommended: [], notHere: LOCAL.recommended, videoOnProcessor: true };
    expect(emptyState("image", LOCAL, [])).toBeNull(); // klein is installed
    expect(emptyState("image", none, [])).toBe("getLocal");
    expect(emptyState("image", nothingFits, [])).toBe("cloudBackup");
    expect(emptyState("video", nothingFits, [endpoint({ company: "openai" })])).toBe("addEndpoint");
    expect(emptyState("image", nothingFits, [openrouter])).toBeNull(); // a cloud model to use
  });

  it("when no local model fits this computer, Studio offers cloud as the backup", async () => {
    // cloudBackup
    await show({
      endpoints: [],
      local: { ...LOCAL, installed: [], recommended: [], notHere: LOCAL.recommended, videoOnProcessor: true },
    });
    screen.getByText(
      "This computer cannot run local models well. Cloud models run on a company's computers instead: add OpenRouter or OpenAI.",
    );
    screen.getByRole("button", { name: "Open Endpoints" });
    // Since Task U9 the card says there is no model here yet and still offers Get models first
    // (a model may run slowly rather than not at all), the cloud as its second line.
    screen.getByText("No models on this computer yet");
    within(screen.getByRole("main")).getByRole("button", { name: "Get models" });
  });

  it("the first-run suggestion leads with local models and can be put away", async () => {
    await show({ local: { ...LOCAL, installed: [] }, prefs: prefs({ suggestionDismissed: false }) });
    screen.getByText(`Studio works on this computer. ${klein.name} fits here: free and private.`);
    await click(screen.getByRole("button", { name: "Not now" }));
    expect(api.studioSetPrefs).toHaveBeenCalledWith({ suggestionDismissed: true });
    expect(screen.queryByText(`Studio works on this computer. ${klein.name} fits here: free and private.`)).toBeNull();
  });

  it("the default model is an installed local model that fits", async () => {
    await show({ prefs: prefs({ lastModel: {} }) });
    expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: "flux2-klein-4b-q4" });
  });

  it("the model you used last still wins, even a cloud one", async () => {
    await show({
      prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" } } }),
    });
    expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" });
  });

  it("a slow local model is not picked by itself", () => {
    const slow = {
      ...LOCAL,
      installed: [{ ...LOCAL.installed[0]!, fit: { ...LOCAL.installed[0]!.fit, word: "slow" as const } }],
    };
    expect(defaultModel("image", slow, [openrouter])).toEqual({ target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" });
  });

  it("an installed model this version cannot read is not a model to use", async () => {
    const row = LOCAL.installed[0]!;
    const unreadable = {
      ...LOCAL,
      installed: [
        {
          ...row,
          fit: { ...row.fit, word: "notHere" as const },
          model: { ...row.model, usable: "This version of harnessUI cannot run this model." },
        },
      ],
    };
    expect(emptyState("image", unreadable, [])).toBe("getLocal"); // klein still fits, to download
    expect(defaultModel("image", unreadable, [])).toBeNull();
    await show({ local: unreadable, prefs: prefs({ lastModel: { image: { target: "thisComputer", model: row.model.id } } }) });
    expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" });
  });

  it("an endpoint without a key offers no model to use", async () => {
    const keyless = endpoint({ hasKey: false });
    const none = { ...LOCAL, installed: [] };
    const nothingFits = { ...none, recommended: [], notHere: LOCAL.recommended, videoOnProcessor: true };
    const openai = endpoint({ providerId: 2, company: "openai" });
    expect(defaultModel("image", none, [keyless])).toBeNull();
    expect(defaultModel("image", none, [keyless, openai])).toEqual({ target: { endpoint: 2 }, model: "gpt-image-2.6-flare" });
    expect(emptyState("image", none, [keyless])).toBe("getLocal");
    expect(emptyState("image", nothingFits, [keyless])).toBe("addEndpoint");
    expect(emptyState("image", nothingFits, [keyless, openai])).toBeNull();
    // Nor is the model used last, while its endpoint has no key.
    await show({
      endpoints: [keyless],
      local: nothingFits,
      prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" } } }),
    });
    expect(useStudio.getState().drafts.image).toMatchObject({ target: null, model: null });
    screen.getByText("OpenRouter has no key, so Studio cannot use its models. Add a key in Settings → Endpoints.");
    await click(screen.getByRole("button", { name: "Open Endpoints" }));
    expect(opened).toEqual(["endpoints"]);
  });

  it("a saved list without a model of the tab's kind is explained, not left blank", async () => {
    const nothingFits = { ...LOCAL, installed: [], recommended: [], notHere: LOCAL.recommended };
    const picturesOnly = endpoint({ models: MODELS.filter((m) => m.kind === "image") });
    expect(emptyState("video", nothingFits, [picturesOnly])).toBe("addEndpoint");
    // Only an endpoint without a saved list stands in: its *Get the list* says what is missing.
    expect(emptyState("video", nothingFits, [endpoint({ fetchedAt: null, models: [] })])).toBeNull();
    await show({ endpoints: [picturesOnly], local: nothingFits });
    await click(screen.getByRole("tab", { name: "Video" }));
    expect(useStudio.getState().drafts.video.model).toBeNull();
    screen.getByText(
      "OpenRouter's saved lists have no video model Studio can use. Update model lists from the menu, or add another endpoint in Settings → Endpoints.",
    );
    screen.getByRole("button", { name: "Open Endpoints" });
  });

  it("with a local model to get, the card's second line says why a saved endpoint is not used", async () => {
    await show({ endpoints: [endpoint({ hasKey: false })], local: { ...LOCAL, installed: [] } });
    screen.getByText(`${klein.name} fits this computer (${formatBytes(klein.sizeBytes)}).`);
    screen.getByText("OpenRouter has no key, so Studio cannot use its models. Add a key in Settings → Endpoints.");
    expect(screen.queryByText("No good graphics card? Cloud models are the backup: OpenRouter or OpenAI.")).toBeNull();
  });

  it("an endpoint without a saved list offers to get it", async () => {
    await show({ endpoints: [endpoint({ fetchedAt: null, models: [] })] });
    screen.getByText("Get OpenRouter's picture and video models");
    await click(screen.getByRole("button", { name: "Get the list" }));
    expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
  });

  it("the top bar shows the title, Pictures and Video, the gallery's controls and the menu", async () => {
    await show();
    const bar = screen.getByRole("banner");
    expect(within(bar).getByRole("heading", { name: "Studio" }).className).toContain("text-title");
    within(bar).getByRole("tab", { name: "Pictures" });
    within(bar).getByRole("tab", { name: "Video" });
    within(bar).getByRole("searchbox", { name: "Search" });
    within(bar).getByRole("button", { name: "Starred" });
    within(bar).getByRole("button", { name: "Grid" });
    within(bar).getByRole("button", { name: "Studio menu" });
  });

  it("Get models is a top-bar button and the place chip is gone", async () => {
    await show();
    const bar = screen.getByRole("banner");
    // Critique §6: the chip looked clickable and was not.
    expect(bar.querySelector("[data-local-mark], [data-cloud-mark]")).toBeNull();
    act(() =>
      useStudio.setState((s) => ({
        drafts: { ...s.drafts, image: { ...s.drafts.image, target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" } },
      })),
    );
    expect(bar.querySelector("[data-local-mark], [data-cloud-mark]")).toBeNull();
    // The menu keeps Update model lists; Get models has its own button.
    await openMenu();
    screen.getByRole("menuitem", { name: "Update model lists" });
    expect(screen.queryByRole("menuitem", { name: /Get local models/ })).toBeNull();
    await act(async () => {
      fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    });
    vi.mocked(api.studioLocalSearch).mockResolvedValue({ items: [] });
    await click(within(bar).getByRole("button", { name: "Get models" }));
    expect(useStudio.getState().localView).toBe(true);
  });

  it("the Get models button reads as pressed while the view is open", async () => {
    await show();
    vi.mocked(api.studioLocalSearch).mockResolvedValue({ items: [] });
    const getModels = within(screen.getByRole("banner")).getByRole("button", { name: "Get models" });
    expect(getModels.getAttribute("aria-pressed")).toBe("false");
    await click(getModels);
    expect(useStudio.getState().localView).toBe(true);
    expect(getModels.getAttribute("aria-pressed")).toBe("true");
    expect(getModels.className).toContain("aria-pressed:bg-accent/10");
    // The view's own filter stands in for the tabs, and the gallery's controls go.
    expect(within(screen.getByRole("banner")).queryByRole("tab", { name: "Pictures" })).toBeNull();
    expect(within(screen.getByRole("banner")).queryByRole("button", { name: "Grid" })).toBeNull();
    // Pressed again, it goes back to the results.
    await click(getModels);
    expect(useStudio.getState().localView).toBe(false);
    expect(getModels.getAttribute("aria-pressed")).toBe("false");
    screen.getByRole("textbox", { name: "Prompt" });
  });

  it("the top-bar indicator counts running downloads and lists them", async () => {
    const GB = 1_000_000_000;
    const download = (patch: Partial<DownloadGroup>): DownloadGroup => ({
      id: "g",
      repo: "leejet/Z-Image-Turbo-GGUF",
      label: "Z-Image Turbo",
      fileCount: 3,
      bytesDone: 2.4 * GB,
      bytesTotal: 6.7 * GB,
      status: "downloading",
      error: null,
      bytesPerSecond: null,
      unverified: false,
      generatorKind: "image",
      genModelId: "z-image-turbo-q4",
      ...patch,
    });
    // No Studio download: no indicator.
    await show();
    expect(document.querySelector("[data-download-indicator]")).toBeNull();
    cleanup();
    ipc.listDownloads.mockResolvedValue([
      download({ id: "zimage" }),
      download({ id: "wan", label: "Wan 2.2 TI2V 5B", genModelId: "wan22-ti2v-5b-fast-q6", generatorKind: "video", status: "paused", bytesDone: 1.2 * GB, bytesTotal: 3.6 * GB }),
      download({ id: "lora", label: "pixel-art-xl", genModelId: null, generatorKind: null, loraId: "hf:nerijs/pixel-art-xl:pixel-art-xl.safetensors", bytesDone: 60_000_000, bytesTotal: 170_000_000 }),
      // Neither listed: one done before (not while Studio followed it), and a chat model's.
      download({ id: "old", label: "FLUX.2 klein 4B", genModelId: "flux2-klein-4b-q4", status: "done", bytesDone: 6.7 * GB }),
      download({ id: "chat", label: "qwen3-8b-q4_k_m.gguf", genModelId: null, generatorKind: null }),
    ]);
    await show();
    const indicator = within(screen.getByRole("banner")).getByRole("button", { name: "3 downloading" });
    expect(indicator.getAttribute("aria-haspopup")).toBe("dialog");
    // Beside Get models.
    expect(indicator.nextElementSibling).toBe(within(screen.getByRole("banner")).getByRole("button", { name: "Get models" }));
    await click(indicator);
    const list = screen.getByRole("dialog", { name: "Downloads" });
    const rows = [...list.querySelectorAll<HTMLElement>("[data-download-row]")];
    expect(rows.map((r) => r.dataset.downloadRow)).toEqual(["zimage", "wan", "lora"]);
    within(rows[0]!).getByText("Z-Image Turbo");
    within(rows[0]!).getByText("Picture model");
    within(rows[0]!).getByText("2.4 GB of 6.7 GB");
    within(rows[0]!).getByRole("button", { name: "Pause downloading Z-Image Turbo" });
    within(rows[1]!).getByText("Video model");
    within(rows[1]!).getByText("1.2 GB of 3.6 GB · Paused");
    within(rows[1]!).getByRole("button", { name: "Resume downloading Wan 2.2 TI2V 5B" });
    within(rows[2]!).getByText("LoRA");
    within(rows[2]!).getByText("60 MB of 170 MB");
    ipc.cancelDownload.mockResolvedValue(undefined);
    await click(within(rows[2]!).getByRole("button", { name: "Cancel downloading pixel-art-xl" }));
    expect(ipc.cancelDownload).toHaveBeenCalledWith("lora");
    // As they move on, the count follows: one cancelled, one done.
    act(() => progress(download({ id: "lora", label: "pixel-art-xl", genModelId: null, generatorKind: null, loraId: "x", status: "cancelled" })));
    act(() => progress(download({ id: "zimage", status: "done", bytesDone: 6.7 * GB })));
    await flush();
    expect(indicator.textContent).toBe("1 downloading");
    expect([...screen.getByRole("dialog", { name: "Downloads" }).querySelectorAll<HTMLElement>("[data-download-row]")].map((r) => r.dataset.downloadRow)).toEqual(["zimage", "wan"]);
    // Esc closes the list, and focus goes back to the indicator.
    fireEvent.keyDown(screen.getByRole("dialog", { name: "Downloads" }), { key: "Escape" });
    await flush();
    expect(screen.queryByRole("dialog", { name: "Downloads" })).toBeNull();
    // Radix gives focus back on the next task.
    await waitFor(() => expect(document.activeElement).toBe(indicator));
  });

  describe("the download list opens only when the indicator is pressed (U7 fix round 1)", () => {
    /** A Studio download (Z-Image Turbo's unless `patch` says otherwise). */
    const download = (patch: Partial<DownloadGroup>): DownloadGroup => ({
      id: "g1",
      repo: "leejet/Z-Image-Turbo-GGUF",
      label: "Z-Image Turbo",
      fileCount: 3,
      bytesDone: 2_400_000_000,
      bytesTotal: 6_700_000_000,
      status: "downloading",
      error: null,
      bytesPerSecond: null,
      unverified: false,
      generatorKind: "image",
      genModelId: "z-image-turbo-q4",
      ...patch,
    });

    /** Lets Radix's focus moves, made on the next task, happen. */
    const nextTask = () => act(() => new Promise<void>((resolve) => setTimeout(resolve, 0)));

    /** The person is writing a prompt when another download starts: nothing pops open and the
     *  cursor stays in the prompt. */
    async function anotherDownloadStarts() {
      expect(document.querySelector("[data-download-indicator]")).toBeNull();
      promptBox().focus();
      expect(document.activeElement).toBe(promptBox());
      act(() => progress(download({ id: "g2", label: "FLUX.2 klein 4B", genModelId: "flux2-klein-4b-q4", bytesDone: 0 })));
      await flush();
      await nextTask();
      within(screen.getByRole("banner")).getByRole("button", { name: "1 downloading" });
      expect(screen.queryByRole("dialog", { name: "Downloads" })).toBeNull();
      expect(document.activeElement).toBe(promptBox());
    }

    it("after Clear finished with nothing running, the next download does not open the list", async () => {
      ipc.listDownloads.mockResolvedValue([download({})]);
      await show();
      // It finishes while Studio follows it: listed as ready.
      act(() => progress(download({ status: "done", bytesDone: 6_700_000_000 })));
      await flush();
      await click(within(screen.getByRole("banner")).getByRole("button", { name: "1 ready" }));
      const list = screen.getByRole("dialog", { name: "Downloads" });
      await click(within(list).getByRole("button", { name: "Clear finished" }));
      await nextTask();
      expect(screen.queryByRole("dialog", { name: "Downloads" })).toBeNull();
      // Focus is not lost with the list: it goes to Get models beside it.
      expect(document.activeElement).toBe(within(screen.getByRole("banner")).getByRole("button", { name: "Get models" }));
      await anotherDownloadStarts();
    });

    it("after the last download in the list is cancelled, the next download does not open the list", async () => {
      ipc.listDownloads.mockResolvedValue([download({})]);
      ipc.cancelDownload.mockResolvedValue(undefined);
      await show();
      await click(within(screen.getByRole("banner")).getByRole("button", { name: "1 downloading" }));
      const list = screen.getByRole("dialog", { name: "Downloads" });
      await click(within(list).getByRole("button", { name: "Cancel downloading Z-Image Turbo" }));
      expect(ipc.cancelDownload).toHaveBeenCalledWith("g1");
      act(() => progress(download({ status: "cancelling" })));
      act(() => progress(download({ status: "cancelled", bytesDone: 0 })));
      await flush();
      await nextTask();
      expect(screen.queryByRole("dialog", { name: "Downloads" })).toBeNull();
      await anotherDownloadStarts();
    });
  });

  it("today's spend shows only while a daily limit is on", async () => {
    await show({ usage: usage({ spentTodayUsd: 0.84 }), prefs: prefs({ dailyLimitUsd: 10 }) });
    within(screen.getByRole("banner")).getByText("$0.84 today");
    cleanup();
    useStudio.setState(INITIAL, true);
    await show({ usage: usage({ spentTodayUsd: 0.84 }), prefs: prefs({ dailyLimitUsd: null }) });
    expect(screen.queryByText("$0.84 today")).toBeNull();
  });

  it("the results take the page and the side panel is closed by default", async () => {
    await show({ runs: [RUN] });
    const results = document.querySelector<HTMLElement>("[data-results]")!;
    // The card stream and the composer at its foot share one column; nothing sits beside it.
    expect(results.contains(document.querySelector("main"))).toBe(true);
    expect(results.contains(screen.getByRole("region", { name: "Make a picture" }))).toBe(true);
    expect(
      document.querySelector("main")!.compareDocumentPosition(screen.getByRole("region", { name: "Make a picture" })) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(screen.queryByRole("complementary", { name: "Settings" })).toBeNull();
    expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
    expect(document.querySelector("[data-settings-panel]")).toBeNull();
    expect(screen.getByRole("button", { name: "Settings" }).getAttribute("aria-expanded")).toBe("false");
    expect(useStudio.getState().settingsOpen).toBe(false);
  });

  describe("on a wide window", () => {
    const width = Object.getOwnPropertyDescriptor(window, "innerWidth");
    function windowWidth(px: number) {
      Object.defineProperty(window, "innerWidth", { configurable: true, value: px });
      window.dispatchEvent(new Event("resize"));
    }
    afterEach(() => {
      if (width) Object.defineProperty(window, "innerWidth", width);
    });

    it("the panel stays docked on a 1400 px window when it was left open", async () => {
      windowWidth(1400);
      await show({ prefs: prefs({ settingsOpen: true }) });
      const panel = screen.getByRole("complementary", { name: "Settings" });
      within(panel).getByText("My setups");
      // Docked beside the results, not over them.
      expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
      expect(document.querySelector("[data-results]")!.contains(panel)).toBe(false);
      expect(screen.getByRole("button", { name: "Settings" }).getAttribute("aria-expanded")).toBe("true");
      // Closing it is saved, and Settings opens it again, docked.
      await click(within(panel).getByRole("button", { name: "Close" }));
      expect(screen.queryByRole("complementary", { name: "Settings" })).toBeNull();
      expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ settingsOpen: false });
      await click(screen.getByRole("button", { name: "Settings" }));
      expect(screen.getByRole("complementary", { name: "Settings" }).contains(document.activeElement)).toBe(true);
      // Narrower, the same open panel slides over the results.
      act(() => windowWidth(1399));
      await flush();
      expect(screen.queryByRole("complementary", { name: "Settings" })).toBeNull();
      screen.getByRole("dialog", { name: "Settings" });
    });

    it("a panel left open does not cover the results when Studio opens on a narrower window", async () => {
      windowWidth(1200);
      await show({ prefs: prefs({ settingsOpen: true }) });
      expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
      expect(useStudio.getState().settingsOpen).toBe(false);
      // The choice saved for wide windows is kept.
      expect(api.studioSetPrefs).not.toHaveBeenCalled();
    });
  });

  it("each tab keeps its own draft", async () => {
    await show();
    typePrompt("lighthouse");
    await click(screen.getByRole("tab", { name: "Video" }));
    expect(promptBox().value).toBe("");
    typePrompt("waves");
    await click(screen.getByRole("tab", { name: "Pictures" }));
    expect(promptBox().value).toBe("lighthouse");
  });

  it("Update model lists refreshes every endpoint and says when", async () => {
    // The day names no year in the year it is.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 10, 18, 12));
    // OpenAI first: its failure does not stop OpenRouter's update.
    const openai = endpoint({ providerId: 2, company: "openai", fetchedAt: new Date(2026, 10, 16, 9).getTime() });
    const before = endpoint({ fetchedAt: new Date(2026, 10, 17, 9).getTime() });
    await show({ endpoints: [openai, before] });
    vi.mocked(api.studioRefreshModels).mockImplementation(async (id) => {
      if (id === 2) {
        throw { code: "network", message: "OpenAI could not be reached.", nextStep: "Check the connection.", details: "" };
      }
      return { endpoints: [openai, { ...before, fetchedAt: new Date(2026, 10, 18, 11).getTime() }] };
    });
    await openMenu();
    screen.getByText("OpenRouter's lists updated 17 Nov");
    await click(screen.getByRole("menuitem", { name: "Update model lists" }));
    expect(api.studioRefreshModels).toHaveBeenNthCalledWith(1, 2);
    expect(api.studioRefreshModels).toHaveBeenNthCalledWith(2, 1);
    screen.getByText("OpenRouter's lists updated 18 Nov");
    screen.getByText("OpenAI's lists updated 16 Nov");
    expect(screen.queryByText("OpenRouter's lists updated 17 Nov")).toBeNull();
    screen.getByText("OpenAI could not be reached."); // in the error banner
  });

  it("the search box asks for the gallery 250 ms after the last key", async () => {
    await show();
    vi.mocked(api.studioRuns).mockClear();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const search = screen.getByRole("searchbox", { name: "Search" }) as HTMLInputElement;
    for (const text of ["f", "fo", "fox"]) {
      fireEvent.change(search, { target: { value: text } });
      act(() => vi.advanceTimersByTime(100));
    }
    expect(search.value).toBe("fox");
    act(() => vi.advanceTimersByTime(149));
    expect(api.studioRuns).not.toHaveBeenCalled();
    await act(async () => {
      vi.advanceTimersByTime(1);
    });
    expect(api.studioRuns).toHaveBeenCalledTimes(1);
    expect(api.studioRuns).toHaveBeenCalledWith({ kind: "image", text: "fox" }, null, 60);
    expect(search.value).toBe("fox");
  });

  it("Load more above the cards reads the gallery's next page, and what is shown stays where it was", async () => {
    const next = { createdAt: RUN.createdAt, id: RUN.id };
    vi.mocked(api.studioOutputBytes).mockReturnValue(new Promise(() => {}));
    vi.mocked(api.studioRuns).mockResolvedValueOnce({ runs: [RUN], next });
    // The stream's height as laid out: 1000 px, then 1600 px once the older page is in.
    let height = 1000;
    const main = Object.getOwnPropertyDescriptor(Element.prototype, "scrollHeight");
    Object.defineProperty(HTMLElement.prototype, "scrollHeight", { configurable: true, get: () => height });
    try {
      await show({ runs: [RUN] });
      const scroller = document.querySelector("main")!;
      // It opened at its newest card.
      expect(scroller.scrollTop).toBe(1000);
      const more = screen.getByRole("button", { name: "Load more" });
      // Above every card.
      expect(more.compareDocumentPosition(document.querySelector("[data-run-card]")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      scroller.scrollTop = 400; // 600 px from the bottom
      const older: Run = { ...RUN, id: "older", createdAt: RUN.createdAt - 1000, prompt: "An older picture", outputs: [] };
      vi.mocked(api.studioRuns).mockImplementationOnce(async () => {
        height = 1600;
        return { runs: [older], next: null };
      });
      await click(more);
      expect(api.studioRuns).toHaveBeenLastCalledWith({ kind: "image" }, next, 60);
      expect(document.querySelector('[data-run-card="older"]')).not.toBeNull();
      expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
      // Still 600 px from the bottom: the cards shown did not move.
      expect(scroller.scrollTop).toBe(1000);
    } finally {
      if (main) Object.defineProperty(HTMLElement.prototype, "scrollHeight", main);
      else delete (HTMLElement.prototype as { scrollHeight?: number }).scrollHeight;
    }
  });

  it("the grid opens at its top", async () => {
    // Critique §2.2 and g2: the grid kept the stream's scroll, so it opened at its bottom with its
    // filters out of view.
    vi.mocked(api.studioOutputBytes).mockReturnValue(new Promise(() => {}));
    const main = Object.getOwnPropertyDescriptor(Element.prototype, "scrollHeight");
    Object.defineProperty(HTMLElement.prototype, "scrollHeight", { configurable: true, get: () => 1000 });
    try {
      await show({ runs: [RUN] });
      const scroller = document.querySelector("main")!;
      // The stream opens at its newest card.
      expect(scroller.scrollTop).toBe(1000);
      await click(screen.getByRole("button", { name: "Grid" }));
      expect(document.querySelector("[data-grid]")).not.toBeNull();
      expect(scroller.scrollTop).toBe(0);
      // Back to the cards, the stream is at its newest card again.
      scroller.scrollTop = 300;
      await click(screen.getByRole("button", { name: "Grid" }));
      expect(scroller.scrollTop).toBe(1000);
    } finally {
      if (main) Object.defineProperty(HTMLElement.prototype, "scrollHeight", main);
      else delete (HTMLElement.prototype as { scrollHeight?: number }).scrollHeight;
    }
  });

  it("a press accepted from Studio brings its new card into view", async () => {
    const shownCards: Element[] = [];
    Element.prototype.scrollIntoView = vi.fn(function (this: Element) {
      shownCards.push(this);
    });
    try {
      await show({ runs: [RUN] });
      const made: Run = { ...RUN, id: "again", seq: 9, status: "queued", outputs: [], createdAt: RUN.createdAt + 1000 };
      vi.mocked(api.studioStart).mockResolvedValue(made);
      await act(async () => {
        await useStudio.getState().again(RUN);
      });
      await flush();
      expect(shownCards.map((c) => c.getAttribute("data-run-card"))).toEqual(["again"]);
      // Only once: a later change of the same run leaves the scroll alone.
      act(() => useStudio.getState().applyProgress({ ...made, seq: 10, status: "running" }));
      await flush();
      expect(shownCards).toHaveLength(1);
    } finally {
      delete (Element.prototype as { scrollIntoView?: unknown }).scrollIntoView;
    }
  });

  it("at the window's smallest the chat list can be hidden from the top bar", async () => {
    await show();
    const toggled = vi.fn();
    window.addEventListener("app:toggle-sidebar", toggled);
    try {
      await click(screen.getByRole("button", { name: "Show or hide the chat list" }));
      expect(toggled).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener("app:toggle-sidebar", toggled);
    }
  });

  it("outside the grid the gallery is read for the tab shown, and read again on another tab", async () => {
    await show({ runs: [RUN] });
    expect(api.studioRuns).toHaveBeenCalledWith({ kind: "image" }, null, 60);
    vi.mocked(api.studioRuns).mockClear();
    await click(screen.getByRole("tab", { name: "Video" }));
    expect(api.studioRuns).toHaveBeenCalledTimes(1);
    expect(api.studioRuns).toHaveBeenCalledWith({ kind: "video" }, null, 60);
  });

  it("the card stream shows the tab's runs, oldest first and newest above the composer, over the queue strip of this session's runs", async () => {
    const older: Run = { ...RUN, id: "older", createdAt: RUN.createdAt - 1000, prompt: "An older picture" };
    const making: Run = {
      ...RUN,
      id: "making",
      createdAt: RUN.createdAt + 1000,
      prompt: "A picture being made",
      status: "running",
      outputs: [],
      costUsd: null,
      startedAt: Date.now(),
      finishedAt: null,
    };
    const clip: Run = { ...RUN, id: "clip", kind: "video", prompt: "A clip" };
    await show({ runs: [RUN, older, clip], live: [making] });
    const cards = () => [...document.querySelectorAll("[data-run-card]")].map((c) => c.getAttribute("data-run-card"));
    expect(cards()).toEqual(["older", RUN.id, "making"]);
    // The queue strip is under the newest card.
    const strip = document.querySelector("[data-queue-strip]")!;
    expect(document.querySelector('[data-run-card="making"]')!.compareDocumentPosition(strip) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    screen.getByText("1 making");
    act(() => useStudio.getState().applyProgress({ ...making, status: "done", finishedAt: Date.now(), seq: making.seq + 1 }));
    screen.getByText("1 done");
    await click(screen.getByRole("tab", { name: "Video" }));
    expect(cards()).toEqual(["clip"]);
    expect(document.querySelector("[data-queue-strip]")).toBeNull();
  });

  it("the Video tab offers a local video model first, and OpenRouter as the backup", async () => {
    // getLocal
    await show({ endpoints: [endpoint({ company: "openai" })], local: { ...LOCAL, installed: [] } });
    await click(screen.getByRole("tab", { name: "Video" }));
    const video = LOCAL.recommended.find((e) => e.kind === "video")!; // "Wan 2.2 5B, fast" unless the benchmark chose Wan 2.1
    screen.getByText(`${video.name} fits this computer (${formatBytes(video.sizeBytes)}).`);
    screen.getByText("Or make videos with OpenRouter: add it in Settings → Endpoints.");
  });

  it("on a computer without a usable graphics card, the Video tab is cloud only", async () => {
    // addEndpoint
    await show({ endpoints: [endpoint({ company: "openai" })], local: { ...LOCAL, videoOnProcessor: true } });
    await click(screen.getByRole("tab", { name: "Video" }));
    screen.getByText(
      "Video on this computer needs a graphics card or an Apple-silicon Mac. OpenRouter makes videos in the cloud: add it in Settings → Endpoints.",
    );
    // Models are on this computer (U9 fix round 1): the card never says otherwise, and offers no
    // Get models beside a line that says video cannot be made here.
    expect(screen.queryByText(/^No (video )?models on this computer yet$/)).toBeNull();
    expect(within(screen.getByRole("main")).queryByRole("button", { name: "Get models" })).toBeNull();
  });

  describe("what is saved, read again", () => {
    const nothingFits = { ...LOCAL, installed: [], recommended: [], notHere: LOCAL.recommended, videoOnProcessor: true };
    const cloudBackup =
      "This computer cannot run local models well. Cloud models run on a company's computers instead: add OpenRouter or OpenAI.";
    const SAVED_READS = [api.studioModels, api.studioLocalModels, api.studioGetPrefs, api.studioUsage];

    it("each time Studio is shown again, without fetching anything", async () => {
      await show({ endpoints: [], local: nothingFits });
      screen.getByText(cloudBackup);
      act(() => useStudio.getState().showChat());
      await flush();
      for (const read of SAVED_READS) expect(read).toHaveBeenCalledTimes(1); // not while hidden
      vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [openrouter] }); // added meanwhile
      act(() => useStudio.getState().showStudio());
      await flush();
      expect(screen.queryByText(cloudBackup)).toBeNull();
      expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" });
      for (const read of SAVED_READS) expect(read).toHaveBeenCalledTimes(2);
      // The gallery's pages stay as read: the tab's first page and the newest runs, once each.
      expect(api.studioRuns).toHaveBeenCalledTimes(2);
      expect(api.studioRefreshModels).not.toHaveBeenCalled();
      expect(api.studioModelPrice).not.toHaveBeenCalled();
      expect(api.studioLocalSearch).not.toHaveBeenCalled();
    });

    it("when Settings closes, so an endpoint added there shows at once", async () => {
      await show({ endpoints: [], local: nothingFits });
      await click(screen.getByRole("button", { name: "Open Endpoints" }));
      expect(opened).toEqual(["endpoints"]);
      vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [openrouter] }); // added there
      await act(async () => {
        window.dispatchEvent(new Event("app:settings-closed"));
      });
      await flush();
      expect(screen.queryByText(cloudBackup)).toBeNull();
      await openMenu();
      expect(screen.getByRole("menuitem", { name: "Update model lists" }).getAttribute("data-disabled")).toBeNull();
      expect(api.studioRefreshModels).not.toHaveBeenCalled();
      // While the chat is shown, Settings closing reads nothing: showing Studio will.
      act(() => useStudio.getState().showChat());
      await act(async () => {
        window.dispatchEvent(new Event("app:settings-closed"));
      });
      await flush();
      expect(api.studioModels).toHaveBeenCalledTimes(2);
    });

    it("a first read that failed is tried again when Studio is shown again", async () => {
      vi.mocked(api.studioModels).mockRejectedValueOnce(new Error("database is locked"));
      await show();
      expect(useStudio.getState().loaded).toBe(false);
      screen.getByText("Something went wrong.");
      act(() => useStudio.getState().showChat());
      act(() => useStudio.getState().showStudio());
      await flush();
      expect(useStudio.getState().loaded).toBe(true);
      // The whole first read again (the tab's first page and the newest runs).
      expect(api.studioRuns).toHaveBeenCalledTimes(4);
      expect(screen.queryByText("Something went wrong.")).toBeNull(); // the read worked
      expect(screen.queryByRole("alert")).toBeNull();
    });

    it("a failed read shows in the error banner", async () => {
      vi.mocked(api.studioRuns).mockRejectedValueOnce({
        code: "store",
        message: "Studio's saved runs could not be read.",
        nextStep: "Try again.",
        details: "database is locked",
      });
      await show();
      expect(screen.getByRole("alert").textContent).toContain("Studio's saved runs could not be read.");
    });

    it("a read again that works puts away a failed read's banner, not an action's", async () => {
      await show({ endpoints: [endpoint({ fetchedAt: null, models: [] })] });
      // A read again fails, then works.
      vi.mocked(api.studioUsage).mockRejectedValueOnce(new Error("database is locked"));
      await act(async () => {
        window.dispatchEvent(new Event("app:settings-closed"));
      });
      await flush();
      screen.getByText("Something went wrong.");
      await act(async () => {
        window.dispatchEvent(new Event("app:settings-closed"));
      });
      await flush();
      expect(screen.queryByRole("alert")).toBeNull();
      // *Get the list* fails: a read that works afterwards leaves that banner.
      vi.mocked(api.studioRefreshModels).mockRejectedValueOnce({
        code: "network",
        message: "OpenRouter could not be reached.",
        nextStep: "Check the connection.",
        details: "",
      });
      await click(screen.getByRole("button", { name: "Get the list" }));
      screen.getByText("OpenRouter could not be reached.");
      await act(async () => {
        window.dispatchEvent(new Event("app:settings-closed"));
      });
      await flush();
      screen.getByText("OpenRouter could not be reached.");
    });
  });
});
