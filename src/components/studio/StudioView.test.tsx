// Studio's view (0.1.0 Task B2): the frame (top bar, card stream, panel and dock slots), the
// empty states and the default model, local first, and the saved lists, read without a fetch.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { amountGb } from "../../lib/words";
import { LOCAL, MODELS } from "./fixtures/models";
import type { LocalModels, StudioEndpoint, StudioPrefs, Usage } from "./types";

// Radix positions menus with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const api = await import("./api");
const { StudioView } = await import("./StudioView");
const { defaultModel, emptyState, useStudio } = await import("../../store/studio");

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
    confirmEveryVideo: true,
    confirmAtUsd: 1,
    dailyLimitUsd: 10,
    setups: [],
    galleryDir: null,
    gpuPolicy: "unload",
    reloadChatAfter: false,
    suggestionDismissed: true,
    localNoticeAcceptedAt: null,
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
async function show(saved: { endpoints?: StudioEndpoint[]; local?: LocalModels; prefs?: StudioPrefs; usage?: Usage } = {}) {
  const endpoints = saved.endpoints ?? [openrouter];
  const p = saved.prefs ?? prefs();
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints });
  vi.mocked(api.studioRefreshModels).mockResolvedValue({ endpoints });
  vi.mocked(api.studioLocalModels).mockResolvedValue(saved.local ?? LOCAL);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs: [], next: null });
  vi.mocked(api.studioLive).mockResolvedValue([]);
  vi.mocked(api.studioGetPrefs).mockResolvedValue(p);
  vi.mocked(api.studioSetPrefs).mockImplementation(async (patch) => ({ ...p, ...patch }));
  vi.mocked(api.studioUsage).mockResolvedValue(saved.usage ?? usage());
  useStudio.getState().showStudio();
  await act(async () => {
    render(<StudioView />);
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

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
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
    screen.getByText(`${klein.name} fits this computer (${amountGb(klein.sizeBytes)}).`);
    await click(screen.getByRole("button", { name: "Get local models" }));
    expect(useStudio.getState().localView).toBe(true);
    screen.getByText("No good graphics card? Cloud models are the backup: OpenRouter or OpenAI."); // second, smaller
    await click(screen.getByRole("button", { name: "Open Endpoints" }));
    expect(opened).toEqual(["endpoints"]); // app:open-settings detail
  });

  it("each empty state follows one ordered rule", () => {
    const none = { ...LOCAL, installed: [] };
    const nothingFits = { ...none, recommended: [], notHere: LOCAL.recommended, videoPossible: false };
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
      local: { ...LOCAL, installed: [], recommended: [], notHere: LOCAL.recommended, videoPossible: false },
    });
    screen.getByText(
      "This computer cannot run local models well. Cloud models run on a company's computers instead: add OpenRouter or OpenAI.",
    );
    screen.getByRole("button", { name: "Open Endpoints" });
    expect(screen.queryByRole("button", { name: "Get local models" })).toBeNull();
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

  it("an endpoint without a saved list offers to get it", async () => {
    await show({ endpoints: [endpoint({ fetchedAt: null, models: [] })] });
    screen.getByText("Get OpenRouter's picture and video models");
    await click(screen.getByRole("button", { name: "Get the list" }));
    expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
  });

  it("the top bar shows Pictures and Video, where the chosen model runs, and cloud spending", async () => {
    await show({ usage: usage({ spentTodayUsd: 0.84 }) }); // the default: a local model
    screen.getByRole("tab", { name: "Pictures" });
    screen.getByRole("tab", { name: "Video" });
    screen.getByText("$0.84 today");
    expect(document.querySelector("[data-local-mark]")).not.toBeNull();
    expect(document.querySelector("[data-cloud-mark]")).toBeNull();
    act(() =>
      useStudio.setState((s) => ({
        drafts: { ...s.drafts, image: { ...s.drafts.image, target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" } },
      })),
    );
    expect(document.querySelector("[data-cloud-mark]")).not.toBeNull();
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
    await show({ endpoints: [endpoint({ fetchedAt: new Date(2026, 10, 17, 9).getTime() })] });
    await openMenu();
    await click(screen.getByRole("menuitem", { name: "Update model lists" }));
    screen.getByText("OpenRouter's lists updated 17 Nov");
    expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
  });

  it("the Video tab offers a local video model first, and OpenRouter as the backup", async () => {
    // getLocal
    await show({ endpoints: [endpoint({ company: "openai" })], local: { ...LOCAL, installed: [] } });
    await click(screen.getByRole("tab", { name: "Video" }));
    const video = LOCAL.recommended.find((e) => e.kind === "video")!; // "Wan 2.2 5B, fast" unless the benchmark chose Wan 2.1
    screen.getByText(`${video.name} fits this computer (${amountGb(video.sizeBytes)}).`);
    screen.getByText("Or make videos with OpenRouter: add it in Settings → Endpoints.");
  });

  it("on a computer without a usable graphics card, the Video tab is cloud only", async () => {
    // addEndpoint
    await show({ endpoints: [endpoint({ company: "openai" })], local: { ...LOCAL, videoPossible: false } });
    await click(screen.getByRole("tab", { name: "Video" }));
    screen.getByText(
      "Video on this computer needs a graphics card or an Apple-silicon Mac. OpenRouter makes videos in the cloud: add it in Settings → Endpoints.",
    );
  });

  describe("what is saved, read again", () => {
    const nothingFits = { ...LOCAL, installed: [], recommended: [], notHere: LOCAL.recommended, videoPossible: false };
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
      expect(api.studioRuns).toHaveBeenCalledTimes(1); // the gallery's pages stay as read
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
      expect(api.studioRuns).toHaveBeenCalledTimes(2); // the whole first read again
    });
  });
});
