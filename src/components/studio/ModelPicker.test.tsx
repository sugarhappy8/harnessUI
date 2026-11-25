// Studio's model picker (0.1.0 Task B3): On this computer first, then the recent cloud models,
// then each endpoint by family; the fit and time of a local model, the price of a cloud one,
// what greys a model out, the filters, and the price asked for once, only when picked. Recent
// comes from the newest runs whatever the gallery's filter, each at the endpoint it ran at.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { amountGb } from "../../lib/words";
import type { StudioTab } from "../../store/studio";
import { LOCAL, MODELS, RUN } from "./fixtures/models";
import type { PickerFilter } from "./ModelPicker";
import type { InstalledLocal, LocalModels, MediaModel, Run, StudioEndpoint, StudioPrefs } from "./types";

// Radix positions popovers with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const api = await import("./api");
const { ModelPicker, localHint, pickerGroups, priceHint } = await import("./ModelPicker");
const { recentChoices, recentModels, useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

function model(id: string): MediaModel {
  const found = MODELS.find((m) => m.id === id);
  if (!found) throw new Error(`no fixture model ${id}`);
  return found;
}
const flare = model("openai/gpt-image-2.6-flare");
const nano = model("google/gemini-nano-banana-2.1");
const k3 = model("krea/k3-edit");
const veo = model("google/veo-3.1");
/** The first recommended entry: FLUX.2 klein 4B. */
const klein = LOCAL.recommended[0]!;
const kleinInstalled = LOCAL.installed[0]!;
const wanInstalled = LOCAL.installed[1]!;
/** A price request that could not reach OpenRouter. */
const unreachable = {
  code: "network",
  message: "OpenRouter could not be reached.",
  nextStep: "Check the connection and try again.",
  details: "",
};
const graphicsCard = "Local models need a graphics card or an Apple-silicon Mac; the cloud models below are the backup.";

/** OpenRouter with the fixture's models, as `studio_models` gives it. */
function endpoint(patch: Partial<StudioEndpoint> = {}): StudioEndpoint {
  return {
    providerId: 1,
    name: "OpenRouter",
    company: "openrouter",
    host: "openrouter.ai",
    cloud: true,
    hasKey: true,
    fetchedAt: new Date(2026, 10, 17, 9).getTime(),
    models: MODELS,
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
    lastImportMode: null,
    ...patch,
  };
}

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

/** Reads what the backend has saved, as Studio does when it opens; each tab gets its model. */
async function load(saved: { endpoints?: StudioEndpoint[]; local?: LocalModels; prefs?: StudioPrefs; runs?: Run[] } = {}) {
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints: saved.endpoints ?? [openrouter] });
  vi.mocked(api.studioLocalModels).mockResolvedValue(saved.local ?? LOCAL);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs: saved.runs ?? [], next: null });
  vi.mocked(api.studioLive).mockResolvedValue([]);
  vi.mocked(api.studioGetPrefs).mockResolvedValue(saved.prefs ?? prefs());
  vi.mocked(api.studioUsage).mockResolvedValue({
    spentTodayUsd: 0,
    pendingUsd: 0,
    limitUsd: 10,
    folder: "/home/me/harnessUI/studio",
    bytes: 0,
    pictures: 0,
    videos: 0,
  });
  await act(async () => {
    await useStudio.getState().load();
  });
}

async function renderPicker(tab: StudioTab = "image") {
  await act(async () => {
    render(<ModelPicker tab={tab} />);
  });
}

/** Opens the picker from its trigger (the Model field). */
async function openPicker() {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: /^Model\b/ }));
  });
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
  await flush();
}

function option(name: RegExp): HTMLElement {
  return screen.getByRole("option", { name });
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
});

afterEach(() => {
  cleanup();
});

describe("pickerGroups", () => {
  it("On this computer comes first, then recent cloud models, then each endpoint by family", () => {
    const g = pickerGroups(LOCAL, [openrouter], "image", "", [], [{ target: { endpoint: 1 }, model: "krea/k3-edit" }]);
    expect(g[0]).toMatchObject({
      place: "local",
      family: "On this computer",
      models: [expect.objectContaining({ id: "flux2-klein-4b-q4" })],
    });
    expect(g[1]).toMatchObject({
      family: "Recent",
      models: [expect.objectContaining({ id: "krea/k3-edit" })],
      via: [openrouter],
    });
    // A recent model stays in its family too.
    expect(g.slice(2).map((x) => x.family)).toEqual(["openai/gpt-image", "google/gemini-nano-banana", "krea/k3-edit"]);
    expect(g.slice(2).every((x) => x.place === "cloud" && x.endpoint === openrouter)).toBe(true);
  });

  it("filters by Edit, References, Start frame, End frame, Sound and Transparent", () => {
    const ids = (f: PickerFilter[], tab: StudioTab = "image") =>
      pickerGroups(null, [openrouter], tab, "", f, []).flatMap((g) => g.models.map((m) => m.id));
    // Klein takes references.
    expect(pickerGroups(LOCAL, [], "image", "", ["edit"], [])[0]!.models.map((m) => m.id)).toEqual(["flux2-klein-4b-q4"]);
    expect(ids(["transparent"])).toEqual(["openai/gpt-image-2.6-flare"]);
    expect(ids(["endFrame"], "video")).toEqual(["google/veo-3.1", "bytedance/seedance-2.5-flash"]);
    expect(ids(["startFrame"], "video")).toHaveLength(3);
    // Every fixture video model makes sound.
    expect(ids(["sound"], "video")).toHaveLength(3);
    expect(ids(["edit"])).toHaveLength(3);
    expect(ids(["references"])).toHaveLength(3);
    expect(ids(["transparent", "edit"])).toEqual(["openai/gpt-image-2.6-flare"]);
    const noRefs = { ...k3, id: "x/no-refs", params: k3.params.filter((p) => p.key !== "input_references") };
    expect(
      pickerGroups(null, [endpoint({ models: [noRefs] })], "image", "", ["edit"], []).flatMap((g) => g.models),
    ).toEqual([]);
  });

  it("searches names and ids, in any case", () => {
    const ids = (query: string) =>
      pickerGroups(LOCAL, [openrouter], "image", query, [], []).flatMap((g) => g.models.map((m) => m.id));
    expect(ids("BANANA")).toEqual(["google/gemini-nano-banana-2.1"]);
    expect(ids(" krea/ ")).toEqual(["krea/k3-edit"]);
    expect(ids("flux")).toEqual(["flux2-klein-4b-q4"]);
  });

  it("lists local video only where this computer can make it", () => {
    expect(pickerGroups(LOCAL, [], "video", "", [], [])[0]!.models.map((m) => m.id)).toEqual(["wan22-ti2v-5b-fast-q6"]);
    expect(pickerGroups({ ...LOCAL, videoPossible: false }, [], "video", "", [], [])[0]!.models).toEqual([]);
  });

  it("never offers an Auto model", () => {
    const auto: MediaModel = { ...flare, id: "openrouter/auto", displayName: "Auto", family: "openrouter/auto" };
    const autoVideo: MediaModel = { ...veo, id: "openrouter/auto", displayName: "Auto", family: "openrouter/auto" };
    const endpoints = [endpoint({ models: [auto, autoVideo, ...MODELS] })];
    for (const tab of ["image", "video"] as const) {
      const recent = [{ target: { endpoint: 1 }, model: "openrouter/auto" }];
      const models = pickerGroups(LOCAL, endpoints, tab, "", [], recent).flatMap((g) => g.models);
      expect(models.length).toBeGreaterThan(0);
      expect(models.filter((m) => /auto/i.test(m.id) || /auto/i.test(m.displayName))).toEqual([]);
    }
  });
});

describe("hints", () => {
  it("names a cloud model's price in words", () => {
    expect(priceHint({ ...flare, price: { unit: "image", low: 0.04, high: 0.17 } }, "openai")).toBe("from $0.04 a picture");
    expect(priceHint({ ...flare, price: { unit: "image", low: 0.04, high: 0.04 } }, "openai")).toBe("$0.04 a picture");
    expect(priceHint({ ...veo, price: { unit: "second", low: 0.1, high: 0.4 } }, "openrouter")).toBe("$0.10–$0.40 a second");
    expect(priceHint({ ...veo, price: { unit: "second", low: 0.1, high: 0.1 } }, "openrouter")).toBe("$0.10 a second");
    // An OpenRouter picture's price comes when it is picked.
    expect(priceHint(nano, "openrouter")).toBe("price shown when picked");
    expect(priceHint(nano, "openai")).toBe("price unknown");
    // Priced per token: no estimate.
    expect(priceHint(flare, "openrouter")).toBe("price unknown");
  });

  it("names a local model's time here, a first guess as one", () => {
    expect(localHint(kleinInstalled, "image")).toBe("about 2 min a picture");
    expect(localHint(wanInstalled, "video")).toBe("time shown after your first run here");
    const measured = { lowMs: 150_000, highMs: 210_000, measured: true, leftMs: null };
    expect(localHint({ ...wanInstalled, fit: { ...wanInstalled.fit, eta: measured } }, "video")).toBe(
      "about 3 min for a 2 s clip",
    );
    const guess = { lowMs: 20_000, highMs: 30_000, measured: false, leftMs: null };
    expect(localHint({ ...kleinInstalled, fit: { ...kleinInstalled.fit, eta: guess } }, "image")).toBe(
      "about 25 s a picture (first guess)",
    );
  });

  it("recent models are the distinct models of the newest runs of a kind", () => {
    const run = (id: string, model: string, createdAt: number, kind: Run["kind"] = "image"): Run => ({
      ...RUN,
      id,
      model,
      kind,
      createdAt,
    });
    const runs = [
      run("a", "krea/k3-edit", 5),
      run("b", "google/veo-3.1", 9, "video"),
      run("c", "openai/gpt-image-2.6-flare", 7),
      run("d", "krea/k3-edit", 8),
      run("e", "google/gemini-nano-banana-2.1", 1),
    ];
    expect(recentModels(runs, "image")).toEqual(["krea/k3-edit", "openai/gpt-image-2.6-flare", "google/gemini-nano-banana-2.1"]);
    expect(recentModels(runs, "image", 2)).toEqual(["krea/k3-edit", "openai/gpt-image-2.6-flare"]);
    expect(recentModels(runs, "video")).toEqual(["google/veo-3.1"]);
  });

  it("recent choices keep where each model ran, once per place and model", () => {
    const run = (id: string, model: string, createdAt: number, providerId: number | null): Run => ({
      ...RUN,
      id,
      model,
      createdAt,
      runsOn: providerId === null ? "local" : "cloud",
      providerId,
    });
    const runs = [
      run("a", k3.id, 9, 2),
      run("b", k3.id, 8, 1),
      run("c", k3.id, 7, 2),
      run("d", kleinInstalled.model.id, 6, null),
      run("e", flare.id, 5, 1),
    ];
    expect(recentChoices(runs, "image")).toEqual([
      { target: { endpoint: 2 }, model: k3.id },
      { target: { endpoint: 1 }, model: k3.id },
      { target: "thisComputer", model: kleinInstalled.model.id },
      { target: { endpoint: 1 }, model: flare.id },
    ]);
    expect(recentChoices(runs, "image", 1)).toEqual([{ target: { endpoint: 2 }, model: k3.id }]);
    expect(recentChoices(runs, "video")).toEqual([]);
  });
});

describe("ModelPicker", () => {
  it("local rows show the fit word and the time here, with no Cloud mark and no price", async () => {
    await load();
    await renderPicker();
    await openPicker();
    const row = option(/FLUX.2 klein 4B/);
    within(row).getByText("Fits");
    within(row).getByText(localHint(kleinInstalled, "image"));
    expect(row.querySelector("[data-cloud-mark]")).toBeNull();
    expect(within(row).queryByText(/\$/)).toBeNull();
    // The group's tag.
    screen.getByText("free · private · nothing is sent");
    expect(row.getAttribute("aria-selected")).toBe("true");
  });

  it("the last local row offers more models that fit", async () => {
    await load();
    await renderPicker();
    await openPicker();
    await click(screen.getByRole("button", { name: `+ Get local models (${LOCAL.moreFit} more fit this computer)` }));
    expect(useStudio.getState().localView).toBe(true);
    expect(screen.queryByRole("option")).toBeNull();
  });

  it("with no local model yet, the group says which one fits", async () => {
    await load();
    useStudio.setState({ local: { ...LOCAL, installed: [] } });
    await renderPicker();
    await openPicker();
    screen.getByText(`No local models yet. ${klein.name} fits this computer (${amountGb(klein.sizeBytes)}).`);
    await click(screen.getByRole("button", { name: "Get local models" }));
    expect(useStudio.getState().localView).toBe(true);
  });

  it("where nothing fits here, the group says cloud is the backup", async () => {
    const slow = LOCAL.recommended.map((e) => ({ ...e, fit: { ...e.fit, word: "slow" as const } }));
    await load({ local: { ...LOCAL, installed: [], recommended: slow, moreFit: 0 } });
    await renderPicker();
    await openPicker();
    screen.getByText(graphicsCard);
    expect(screen.queryByRole("button", { name: /Get local models/ })).toBeNull();
  });

  it("where a picture model fits but no video model does, the Video tab says no video model fits", async () => {
    // FLUX.2 klein fits (installed); Wan 2.2 is slow here, and none is installed.
    const slowVideo = LOCAL.recommended.map((e) => (e.kind === "video" ? { ...e, fit: { ...e.fit, word: "slow" as const } } : e));
    const local = { ...LOCAL, installed: [kleinInstalled], recommended: slowVideo, moreFit: 0 };
    await load({ local });
    await renderPicker("video");
    await openPicker();
    screen.getByText("No video model fits this computer yet; the cloud models below are the backup.");
    expect(screen.queryByText(graphicsCard)).toBeNull();
    cleanup();
    // Nothing installed, but a picture model that fits is still to get.
    useStudio.setState({ local: { ...local, installed: [], moreFit: 2 } });
    await renderPicker("video");
    await openPicker();
    screen.getByText("No video model fits this computer yet; the cloud models below are the backup.");
    expect(screen.queryByText(graphicsCard)).toBeNull();
  });

  it("where this computer cannot make video, the Video tab says local models need a graphics card", async () => {
    // A picture model fits, but video needs a usable graphics card or an Apple-silicon Mac.
    await load({ local: { ...LOCAL, videoPossible: false } });
    await renderPicker("video");
    await openPicker();
    screen.getByText(graphicsCard);
    expect(screen.queryByText(/No video model fits/)).toBeNull();
  });

  it("a local model this build cannot read is greyed out with its reason and cannot be chosen", async () => {
    const reason = "This version of harnessUI cannot read this model. Delete it to free the space.";
    const unreadable: InstalledLocal = {
      ...kleinInstalled,
      id: "old-model",
      name: "Old Model",
      model: { ...kleinInstalled.model, id: "old-model", displayName: "Old Model", usable: reason },
    };
    await load({ local: { ...LOCAL, installed: [unreadable] } });
    await renderPicker();
    await openPicker();
    const row = option(/Old Model/);
    expect(row.getAttribute("aria-disabled")).toBe("true");
    within(row).getByText(reason);
    // Nothing this computer can run is installed: the group still offers one that fits.
    screen.getByText(`No local models yet. ${klein.name} fits this computer (${amountGb(klein.sizeBytes)}).`);
    const before = useStudio.getState().drafts.image;
    await click(row);
    expect(useStudio.getState().drafts.image).toBe(before);
  });

  it("picking a local model asks for no price", async () => {
    // Studio opens on an OpenRouter picture model without a price (restored, so not asked).
    await load({ prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: nano.id } } }) });
    expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: nano.id });
    await renderPicker();
    await openPicker();
    await click(option(/FLUX.2 klein 4B/));
    expect(api.studioModelPrice).not.toHaveBeenCalled();
    expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: "flux2-klein-4b-q4" });
  });

  it("names where the prompt goes", async () => {
    await load();
    await renderPicker();
    await openPicker();
    screen.getByText("via OpenRouter");
    expect(screen.getByTitle("Your prompt and added pictures go to openrouter.ai.")).toBeTruthy();
  });

  it("an endpoint whose lists are not saved yet has a row that gets them, and says why when it cannot", async () => {
    const openai = endpoint({
      providerId: 2,
      name: "OpenAI",
      company: "openai",
      host: "api.openai.com",
      fetchedAt: null,
      models: [],
    });
    await load({ endpoints: [endpoint({ fetchedAt: null, models: [] }), openai] });
    await renderPicker("video");
    await openPicker();
    // OpenAI makes no video: it has no row on the Video tab.
    expect(screen.queryByRole("button", { name: "OpenAI: Get the list" })).toBeNull();
    vi.mocked(api.studioRefreshModels).mockRejectedValueOnce(unreachable);
    await click(screen.getByRole("button", { name: "OpenRouter: Get the list" }));
    expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
    screen.getByText("OpenRouter could not be reached.");
    vi.mocked(api.studioRefreshModels).mockResolvedValueOnce({ endpoints: [openrouter, openai] });
    await click(screen.getByRole("button", { name: "OpenRouter: Get the list" }));
    expect(api.studioRefreshModels).toHaveBeenCalledTimes(2);
    expect(screen.queryByText("OpenRouter could not be reached.")).toBeNull();
    expect(screen.queryByRole("button", { name: "OpenRouter: Get the list" })).toBeNull();
    option(/Veo 3\.1/);
  });

  it("greys out an endpoint without a key", async () => {
    await load({ endpoints: [endpoint({ hasKey: false })] });
    await renderPicker();
    await openPicker();
    const row = option(/K3 Edit/);
    expect(row.getAttribute("aria-disabled")).toBe("true");
    expect(option(/GPT Image 2.6 Flare/).getAttribute("aria-disabled")).toBe("true");
    screen.getByText("Add a key in Settings → Endpoints");
    const before = useStudio.getState().drafts.image;
    await click(row);
    expect(useStudio.getState().drafts.image).toBe(before);
    expect(api.studioModelPrice).not.toHaveBeenCalled();
  });

  it("greys out a video model that needs a video, and says why", async () => {
    const reason = "Needs a video to start from, which Studio does not offer yet.";
    const needsVideo: MediaModel = { ...veo, id: "x/needs-video", displayName: "Needs Video", usable: reason };
    await load({ endpoints: [endpoint({ models: [...MODELS, needsVideo] })] });
    await renderPicker("video");
    await openPicker();
    expect(option(/Needs Video/).getAttribute("aria-disabled")).toBe("true");
    screen.getByText(reason);
  });

  it("the chips filter the list", async () => {
    await load();
    await renderPicker();
    await openPicker();
    await click(screen.getByRole("button", { name: "Transparent" }));
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual([expect.stringContaining("GPT Image 2.6 Flare")]);
    // The installed models are all hidden, so their group is too.
    expect(screen.queryByText("free · private · nothing is sent")).toBeNull();
    fireEvent.change(screen.getByRole("searchbox", { name: "Search models" }), { target: { value: "banana" } });
    screen.getByText("No models match.");
  });

  it("a restored model fetches no price when Studio opens", async () => {
    const unpriced = { ...flare, priceFetchedAt: null };
    await load({
      endpoints: [endpoint({ models: [unpriced, nano, k3] })],
      prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: unpriced.id } } }),
    });
    await renderPicker();
    expect(api.studioModelPrice).not.toHaveBeenCalled();
    expect(api.studioRefreshModels).not.toHaveBeenCalled();
    screen.getByText("price shown when picked");
    vi.mocked(api.studioModelPrice).mockResolvedValue(flare);
    await click(screen.getByRole("button", { name: "Get the price" }));
    expect(api.studioModelPrice).toHaveBeenCalledTimes(1);
    expect(api.studioModelPrice).toHaveBeenCalledWith(1, unpriced.id);
    // Priced per token, as the answer says.
    screen.getByText("price unknown");
    expect(screen.queryByRole("button", { name: "Get the price" })).toBeNull();
  });

  it("picking an OpenRouter picture model asks for its price once", async () => {
    const unpriced = { ...flare, priceFetchedAt: null };
    await load({ endpoints: [endpoint({ models: [unpriced, nano, k3] })] });
    vi.mocked(api.studioModelPrice).mockResolvedValue(flare);
    await renderPicker();
    await openPicker();
    within(option(/GPT Image 2.6 Flare/)).getByText("price shown when picked");
    await click(option(/GPT Image 2.6 Flare/));
    expect(api.studioModelPrice).toHaveBeenCalledTimes(1);
    expect(api.studioModelPrice).toHaveBeenCalledWith(1, "openai/gpt-image-2.6-flare");
    expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: unpriced.id });
    await openPicker();
    await click(option(/GPT Image 2.6 Flare/));
    expect(api.studioModelPrice).toHaveBeenCalledTimes(1);
  });

  it("Recent comes from the newest runs, whatever the gallery's filter", async () => {
    const k3Run: Run = { ...RUN, id: "k3-run", model: k3.id, createdAt: RUN.createdAt + 1000 };
    await load({ runs: [k3Run, RUN] });
    // The gallery shows videos only now: none of these runs.
    vi.mocked(api.studioRuns).mockResolvedValue({ runs: [], next: null });
    await act(async () => {
      await useStudio.getState().setFilter({ kind: "video" });
    });
    expect(useStudio.getState().runs).toEqual([]);
    await renderPicker();
    await openPicker();
    const recent = screen.getByRole("listbox", { name: "Recent" });
    expect(within(recent).getAllByRole("option").map((o) => o.textContent)).toEqual([
      expect.stringContaining("K3 Edit"),
      expect.stringContaining("GPT Image 2.6 Flare"),
    ]);
  });

  it("a recent model is the one at the endpoint it ran at, which the row names", async () => {
    // Two endpoints of one company list the same models.
    const work = endpoint({ providerId: 2, name: "OpenRouter work" });
    const k3Run: Run = { ...RUN, id: "k3-run", model: k3.id, providerId: 2, providerName: "OpenRouter work" };
    await load({ endpoints: [openrouter, work], runs: [k3Run] });
    expect(
      pickerGroups(LOCAL, [openrouter, work], "image", "", [], [{ target: { endpoint: 2 }, model: k3.id }])[1],
    ).toMatchObject({ family: "Recent", models: [k3], via: [work] });
    await renderPicker();
    await openPicker();
    const row = within(screen.getByRole("listbox", { name: "Recent" })).getByRole("option", { name: /K3 Edit/ });
    within(row).getByText("via OpenRouter work");
    await click(row);
    expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 2 }, model: k3.id });
  });

  it("a price that could not be got when picked says so, and Get the price asks again", async () => {
    await load();
    vi.mocked(api.studioModelPrice).mockRejectedValueOnce(unreachable);
    await renderPicker();
    await openPicker();
    // No extra settings are held aside for it.
    await click(option(/Nano Banana 2.1/));
    expect(api.studioModelPrice).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("alert").textContent).toContain("OpenRouter could not be reached.");
    screen.getByText("price shown when picked");
    vi.mocked(api.studioModelPrice).mockResolvedValue({
      ...nano,
      priceFetchedAt: 1794909600000,
      price: { unit: "image", low: 0.04, high: 0.04 },
    });
    await click(screen.getByRole("button", { name: "Get the price" }));
    expect(api.studioModelPrice).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("alert")).toBeNull();
    screen.getByText("$0.04 a picture");
  });

  it("a price error, or a price on its way, stays with its model", async () => {
    const unpriced = { ...flare, priceFetchedAt: null };
    await load({
      endpoints: [endpoint({ models: [unpriced, nano, k3] })],
      prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: unpriced.id } } }),
    });
    await renderPicker();
    vi.mocked(api.studioModelPrice).mockRejectedValueOnce(unreachable);
    await click(screen.getByRole("button", { name: "Get the price" }));
    screen.getByRole("alert");
    // Another model: the error was for the one before.
    await openPicker();
    await click(option(/FLUX.2 klein 4B/));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(useStudio.getState().priceError.image).toBeNull();
    // A price on its way for GPT Image 2.6 Flare while another model is picked.
    let fail: (e: unknown) => void = () => undefined;
    vi.mocked(api.studioModelPrice).mockReturnValue(new Promise((_, reject) => (fail = reject)));
    await openPicker();
    await click(option(/Nano Banana 2.1/));
    expect(screen.queryByRole("alert")).toBeNull();
    await openPicker();
    await click(option(/GPT Image 2.6 Flare/));
    const asking = screen.getByRole("button", { name: "Get the price" }) as HTMLButtonElement;
    await click(asking);
    expect(asking.disabled).toBe(true);
    await openPicker();
    await click(option(/Nano Banana 2.1/));
    expect((screen.getByRole("button", { name: "Get the price" }) as HTMLButtonElement).disabled).toBe(false);
    // Both requests fail: only the one for the model shown is said, once.
    await act(async () => {
      fail(unreachable);
    });
    await flush();
    expect(screen.getAllByRole("alert")).toHaveLength(1);
  });

  it("asks once for a model picked twice while its price is on the way", async () => {
    await load();
    let answer: (m: MediaModel) => void = () => undefined;
    vi.mocked(api.studioModelPrice).mockReturnValue(new Promise((resolve) => (answer = resolve)));
    useStudio.getState().chooseModel("image", { endpoint: 1 }, nano.id);
    useStudio.getState().chooseModel("image", { endpoint: 1 }, nano.id);
    // *Get the price* while it is on the way waits for the same answer.
    const fetched = useStudio.getState().fetchPrice("image");
    await flush();
    expect(api.studioModelPrice).toHaveBeenCalledTimes(1);
    await act(async () => {
      answer({ ...nano, priceFetchedAt: 1794909600000 });
      await fetched;
    });
    expect(useStudio.getState().endpoints[0]!.models.find((m) => m.id === nano.id)?.priceFetchedAt).toBe(1794909600000);
  });
});

describe("chooseModel", () => {
  /** A picture model's answer from OpenRouter's endpoints: priced, with an Extra field. */
  const extraField = flare.params.find((p) => p.kind.type === "extra")!;
  const nanoAnswered: MediaModel = {
    ...nano,
    priceFetchedAt: 1794909600000,
    params: [...nano.params, { ...extraField, kind: { type: "extra", allowedKeys: ["moderation"] } }],
  };

  /** Studio open on GPT Image 2.6 Flare (its answer saved) with an extra setting. */
  async function onFlareWithExtra() {
    await load({ prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: flare.id } } }) });
    useStudio.setState((s) => ({ drafts: { ...s.drafts, image: { ...s.drafts.image, extra: { moderation: "low" } } } }));
  }

  it("carries the settings to the model picked and says what changed", async () => {
    await load();
    useStudio.setState((s) => ({ drafts: { ...s.drafts, image: { ...s.drafts.image, values: { aspect_ratio: "21:9" } } } }));
    useStudio.getState().chooseModel("image", { endpoint: 1 }, k3.id);
    expect(useStudio.getState().drafts.image).toMatchObject({
      target: { endpoint: 1 },
      model: k3.id,
      values: { aspect_ratio: "16:9" },
      note: "Shape 21:9 became 16:9.",
    });
  });

  it("holds the extra settings aside until the new model's answer, which takes them", async () => {
    await onFlareWithExtra();
    let answer: (m: MediaModel) => void = () => undefined;
    vi.mocked(api.studioModelPrice).mockReturnValue(new Promise((resolve) => (answer = resolve)));
    useStudio.getState().chooseModel("image", { endpoint: 1 }, nano.id);
    expect(useStudio.getState().drafts.image).toMatchObject({ model: nano.id, extra: {}, note: null });
    await act(async () => {
      answer(nanoAnswered);
    });
    await flush();
    expect(useStudio.getState().drafts.image.extra).toEqual({ moderation: "low" });
    expect(useStudio.getState().drafts.image.note).toBeNull();
  });

  it("keeps the extra settings held when the price request fails, and Get the price brings them in", async () => {
    await onFlareWithExtra();
    vi.mocked(api.studioModelPrice).mockRejectedValue({ code: "network", message: "", nextStep: "", details: "" });
    useStudio.getState().chooseModel("image", { endpoint: 1 }, nano.id);
    expect(useStudio.getState().holding.image).toBe(true);
    await flush();
    // Never checked against the model as listed, which has no Extra field.
    expect(useStudio.getState().drafts.image).toMatchObject({ extra: {}, note: null });
    expect(useStudio.getState().holding.image).toBe(true);
    vi.mocked(api.studioModelPrice).mockResolvedValue(nanoAnswered);
    await act(async () => {
      await useStudio.getState().fetchPrice("image");
    });
    expect(useStudio.getState().drafts.image).toMatchObject({ extra: { moderation: "low" }, note: null });
    expect(useStudio.getState().holding.image).toBe(false);
  });

  it("Reset all lets go of the extra settings held", async () => {
    await onFlareWithExtra();
    let answer: (m: MediaModel) => void = () => undefined;
    vi.mocked(api.studioModelPrice).mockReturnValue(new Promise((resolve) => (answer = resolve)));
    useStudio.getState().chooseModel("image", { endpoint: 1 }, nano.id);
    expect(useStudio.getState().holding.image).toBe(true);
    useStudio.getState().resetAdvanced("image");
    expect(useStudio.getState().holding.image).toBe(false);
    // The answer that comes after brings nothing back. It also settles the request, which the
    // store would otherwise keep as on its way, and hand to the tests after this one.
    await act(async () => {
      answer(nanoAnswered);
    });
    await flush();
    expect(useStudio.getState().drafts.image.extra).toEqual({});
    expect(useStudio.getState().holding.image).toBe(false);
  });

  it("a price request that fails after another model was picked leaves no error", async () => {
    await load();
    let fail: (e: unknown) => void = () => undefined;
    vi.mocked(api.studioModelPrice).mockReturnValue(new Promise((_, reject) => (fail = reject)));
    useStudio.getState().chooseModel("image", { endpoint: 1 }, nano.id);
    useStudio.getState().chooseModel("image", "thisComputer", kleinInstalled.model.id);
    await act(async () => {
      fail(unreachable);
    });
    await flush();
    expect(useStudio.getState().priceError.image).toBeNull();
  });

  it("drops the held extra settings with the note when another model is picked first", async () => {
    await onFlareWithExtra();
    let answer: (m: MediaModel) => void = () => undefined;
    vi.mocked(api.studioModelPrice).mockReturnValue(new Promise((resolve) => (answer = resolve)));
    useStudio.getState().chooseModel("image", { endpoint: 1 }, nano.id);
    useStudio.getState().chooseModel("image", "thisComputer", kleinInstalled.model.id);
    expect(useStudio.getState().drafts.image).toMatchObject({
      model: kleinInstalled.model.id,
      extra: {},
      note: "Nano Banana 2.1 does not accept moderation, so it was removed.",
    });
    expect(useStudio.getState().holding.image).toBe(false);
    await act(async () => {
      answer(nanoAnswered);
    });
    await flush();
    expect(useStudio.getState().drafts.image.extra).toEqual({});
  });
});
