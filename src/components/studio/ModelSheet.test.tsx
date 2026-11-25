// The model sheet (0.2.0 Task U4, design 10.2), which replaced 0.1.0's model picker (Task B3):
// it opens from the composer's model chip with its search focused, Esc closes it and focus goes
// back to the chip; the arrow keys and Enter pick a model; Recent is at most four tiles, never
// repeated below; an installed row says its type, size, fit (the warning in its tooltip), LoRAs
// and set-up; a cloud row its price and at most two icons; notes are in a tooltip; the filter
// and the tab decide what shows; an endpoint without a key or a list is a row with its fix; on
// a narrow window it is a full-height sheet; its foot opens Get models and *Add a model file*.
// Also the picker's groups and hints it kept, what it lists (`sheetSections`), and the pick
// itself: the price asked for once, the settings carried.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { amountGb } from "../../lib/words";
import type { StudioTab } from "../../store/studio";
import { UiProviders } from "../ui/UiProviders";
import { LOCAL, MODELS, RUN } from "./fixtures/models";
import type { PickerFilter } from "./modelSheet";
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
const { localHint, pickerGroups, priceHint, sheetSections } = await import("./modelSheet");
const { ModelChip } = await import("./ModelChip");
const { recentChoices, recentModels, useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();
const WIDTH = window.innerWidth;

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
/** SDXL from Hugging Face, and a file whose model type and files are not chosen yet. */
const sdxlInstalled = LOCAL.installed.find((m) => m.version === "sdxl")!;
const mysteryInstalled = LOCAL.installed.find((m) => m.needsSetup)!;
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

/** A run of `modelId` at `createdAt`, on this computer or (`providerId`) at an endpoint. */
function runOf(id: string, modelId: string, createdAt: number, providerId: number | null = 1): Run {
  return { ...RUN, id, model: modelId, createdAt, runsOn: providerId === null ? "local" : "cloud", providerId };
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
  vi.mocked(api.studioSetPrefs).mockImplementation(async (patch) => ({ ...(saved.prefs ?? prefs()), ...patch }));
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

/** The composer's model chip of `tab`, with the sheet it opens. */
async function renderChip(tab: StudioTab = "image") {
  await act(async () => {
    render(
      <UiProviders>
        <ModelChip tab={tab} />
      </UiProviders>,
    );
  });
}

function chipButton(): HTMLElement {
  const button = document.querySelector<HTMLElement>("[data-model-chip] button");
  if (!button) throw new Error("no model chip");
  return button;
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
  await flush();
}

async function key(el: Element, k: string) {
  await act(async () => {
    fireEvent.keyDown(el, { key: k });
  });
  await flush();
}

/** Opens the sheet from the chip. */
async function openSheet() {
  await click(chipButton());
}

function sheet(): HTMLElement {
  return screen.getByRole("dialog", { name: "Choose a model" });
}

/** A model's row (or tile) in the sheet, by its name. */
function row(name: RegExp): HTMLElement {
  return within(sheet()).getByRole("button", { name });
}

function search(): HTMLElement {
  return within(sheet()).getByRole("searchbox", { name: "Search models" });
}

async function tipOf(el: HTMLElement): Promise<string> {
  await act(async () => {
    el.focus();
  });
  return (await screen.findByRole("tooltip")).textContent ?? "";
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
});

afterEach(() => {
  cleanup();
  window.innerWidth = WIDTH;
});

describe("pickerGroups", () => {
  it("On this computer comes first, then recent cloud models, then each endpoint by family", () => {
    const g = pickerGroups(LOCAL, [openrouter], "image", "", [], [{ target: { endpoint: 1 }, model: "krea/k3-edit" }]);
    expect(g[0]).toMatchObject({
      place: "local",
      family: "On this computer",
      // Klein, then the files added from Hugging Face, newest first (the last needs its model
      // type chosen: listed, with why it cannot be used).
      models: [
        expect.objectContaining({ id: "flux2-klein-4b-q4" }),
        expect.objectContaining({ id: "hf:stabilityai/stable-diffusion-xl-base-1.0:sd_xl_base_1.0.safetensors" }),
        expect.objectContaining({ id: "hf:someone/flux-dev-copy:flux-dev-copy.safetensors" }),
        expect.objectContaining({
          id: "hf:someone/mystery-model:mystery.safetensors",
          usable: "Choose this model's type and files first.",
        }),
      ],
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
    expect(ids("flux")).toEqual(["flux2-klein-4b-q4", "hf:someone/flux-dev-copy:flux-dev-copy.safetensors"]);
  });

  it("lists local video only where this computer can make it", () => {
    expect(pickerGroups(LOCAL, [], "video", "", [], [])[0]!.models.map((m) => m.id)).toEqual(["wan22-ti2v-5b-fast-q6"]);
    expect(pickerGroups({ ...LOCAL, videoOnProcessor: true }, [], "video", "", [], [])[0]!.models).toEqual([]);
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

describe("sheetSections", () => {
  const state = (patch: Partial<Parameters<typeof sheetSections>[0]> = {}) => ({
    local: LOCAL,
    endpoints: [openrouter],
    recentRuns: [] as Run[],
    drafts: INITIAL.drafts,
    ...patch,
  });
  const ids = (rows: { model: MediaModel }[]) => rows.map((r) => r.model.id);

  it("lists this computer's models, then each endpoint's, with what an endpoint lacks as its fix", () => {
    const noKey = endpoint({ providerId: 2, name: "OpenRouter work", hasKey: false });
    const unsaved = endpoint({ providerId: 3, name: "OpenAI", company: "openai", fetchedAt: null, models: [] });
    const s = sheetSections(state({ endpoints: [openrouter, noKey, unsaved] }), "image", "", "all");
    expect(s.recent).toEqual([]);
    expect(ids(s.local)).toEqual(LOCAL.installed.filter((m) => m.kind === "image").map((m) => m.model.id));
    expect(s.cloud.map((g) => [g.company, g.fix, ids(g.rows)])).toEqual([
      ["OpenRouter", null, [flare.id, nano.id, k3.id]],
      ["OpenRouter work", "noKey", []],
      ["OpenAI", "getList", []],
    ]);
    // OpenAI makes no video: it has nothing on the Video tab.
    const video = sheetSections(state({ endpoints: [openrouter, unsaved] }), "video", "", "all");
    expect(video.cloud.map((g) => g.company)).toEqual(["OpenRouter"]);
    expect(ids(video.local)).toEqual([wanInstalled.model.id]);
  });

  it("recent is at most four models still offered, where each ran, and they are not repeated below", () => {
    const runs = [
      runOf("a", k3.id, 9),
      runOf("b", kleinInstalled.model.id, 8, null),
      runOf("c", "gone/model", 7),
      runOf("d", nano.id, 6),
      runOf("e", mysteryInstalled.model.id, 5, null),
      runOf("f", flare.id, 4),
      runOf("g", sdxlInstalled.model.id, 3, null),
    ];
    const s = sheetSections(state({ recentRuns: runs }), "image", "", "all");
    // A model no longer listed, and one that needs setting up, are not recent tiles.
    expect(ids(s.recent)).toEqual([k3.id, kleinInstalled.model.id, nano.id, flare.id]);
    expect(ids(s.local)).not.toContain(kleinInstalled.model.id);
    expect(ids(s.local)).toContain(sdxlInstalled.model.id);
    expect(s.cloud).toEqual([]);
  });

  it("the search and the filter apply to every part", () => {
    const runs = [runOf("a", k3.id, 9)];
    const banana = sheetSections(state({ recentRuns: runs }), "image", "BANANA", "all");
    expect([ids(banana.recent), ids(banana.local), banana.cloud.flatMap((g) => ids(g.rows))]).toEqual([[], [], [nano.id]]);
    const here = sheetSections(state({ recentRuns: runs }), "image", "", "local");
    expect([here.recent, here.cloud]).toEqual([[], []]);
    expect(here.local.length).toBeGreaterThan(0);
    const cloud = sheetSections(state({ recentRuns: runs }), "image", "", "cloud");
    expect(ids(cloud.recent)).toEqual([k3.id]);
    expect(cloud.local).toEqual([]);
  });

  it("marks the draft's model, and counts its LoRAs", () => {
    const drafts = {
      ...INITIAL.drafts,
      image: {
        ...INITIAL.drafts.image,
        target: "thisComputer" as const,
        model: kleinInstalled.model.id,
        values: { loras: [{ id: "a", weight: 1, highNoise: false }] },
      },
    };
    const s = sheetSections(state({ drafts }), "image", "", "all");
    expect(s.local.filter((r) => r.selected).map((r) => [r.model.id, r.loras])).toEqual([[kleinInstalled.model.id, 1]]);
  });
});

describe("ModelSheet", () => {
  it("the sheet opens from the chip with its search focused, and Esc closes it with focus back on the chip", async () => {
    await load();
    await renderChip();
    await openSheet();
    expect(sheet().getAttribute("data-layout")).toBe("popover");
    expect(chipButton().getAttribute("aria-expanded")).toBe("true");
    expect(document.activeElement).toBe(search());
    await key(search(), "Escape");
    expect(screen.queryByRole("dialog", { name: "Choose a model" })).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(chipButton()));
    // The chip opens it again, and closes it.
    await openSheet();
    sheet();
    await openSheet();
    expect(screen.queryByRole("dialog", { name: "Choose a model" })).toBeNull();
  });

  it("arrow keys move through rows and Enter picks the model and closes the sheet", async () => {
    await load();
    await renderChip();
    await openSheet();
    await key(search(), "ArrowDown");
    expect(document.activeElement).toBe(row(/^FLUX\.2 klein 4B/));
    await key(document.activeElement!, "ArrowUp");
    expect(document.activeElement).toBe(search());
    await key(search(), "ArrowDown");
    await key(document.activeElement!, "ArrowDown");
    const sdxl = row(/^sd_xl_base_1\.0/);
    expect(document.activeElement).toBe(sdxl);
    await key(sdxl, "Enter");
    expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: sdxlInstalled.model.id });
    expect(screen.queryByRole("dialog", { name: "Choose a model" })).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(chipButton()));
    // Enter in the search box picks the first model it finds.
    vi.mocked(api.studioModelPrice).mockResolvedValue(k3);
    await openSheet();
    fireEvent.change(search(), { target: { value: "k3" } });
    await key(search(), "Enter");
    expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: k3.id });
  });

  it("recent models are at most four tiles and are not repeated below", async () => {
    const runs = [
      runOf("a", k3.id, 900),
      runOf("b", kleinInstalled.model.id, 800, null),
      runOf("c", nano.id, 700),
      runOf("d", flare.id, 600),
      runOf("e", sdxlInstalled.model.id, 500, null),
    ];
    await load({ runs });
    await renderChip();
    await openSheet();
    const recent = within(sheet()).getByRole("region", { name: "Recent" });
    const tiles = within(recent).getAllByRole("button");
    expect(tiles.map((t) => t.textContent)).toEqual([
      "K3 EditOpenRouter",
      "FLUX.2 klein 4BThis computer",
      "Nano Banana 2.1OpenRouter",
      "GPT Image 2.6 FlareOpenRouter",
    ]);
    for (const name of [/^K3 Edit/, /^FLUX\.2 klein 4B/, /^Nano Banana 2\.1/, /^GPT Image 2\.6 Flare/]) {
      expect(within(sheet()).getAllByRole("button", { name })).toHaveLength(1);
    }
    // The fifth is in its list.
    within(within(sheet()).getByRole("region", { name: "On this computer" })).getByRole("button", { name: /^sd_xl_base_1\.0/ });
    // The tile of the draft's model says so; a tile picks its model where it ran.
    expect(tiles[1]!.getAttribute("aria-current")).toBe("true");
    vi.mocked(api.studioModelPrice).mockResolvedValue(k3);
    await click(tiles[0]!);
    expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: k3.id });
  });

  it("an installed row says its type, size and fit, with the warning in its tooltip", async () => {
    const reason = "needs about 9 GB of graphics memory";
    const tight: InstalledLocal = {
      ...sdxlInstalled,
      fit: { ...sdxlInstalled.fit, word: "notHere", verdict: "red", reason },
    };
    await load({ local: { ...LOCAL, installed: LOCAL.installed.map((m) => (m === sdxlInstalled ? tight : m)) } });
    await renderChip();
    await openSheet();
    const sdxl = row(/^sd_xl_base_1\.0/);
    expect(sdxl.textContent).toContain("SDXL · picture");
    expect(sdxl.textContent).toContain("6.9 GB");
    // A warning, never a refusal: it can still be picked.
    expect(sdxl.getAttribute("aria-disabled")).toBeNull();
    const dot = within(sdxl.closest("li")!).getByRole("img", { name: /^May not fit this computer/ });
    expect(await tipOf(dot)).toBe(`May not fit this computer: ${reason}. It may fail or be very slow.`);
    // One that fits says so, with its time here; an untested set-up says that too.
    const kleinDot = within(row(/^FLUX\.2 klein 4B/).closest("li")!).getByRole("img");
    expect(kleinDot.getAttribute("aria-label")).toBe(`Fits · ${localHint(kleinInstalled, "image")}`);
    expect(row(/^flux-dev-copy/).textContent).toContain("untested set-up");
    expect(row(/^FLUX\.2 klein 4B/).textContent).not.toContain("untested set-up");
  });

  it("a model with LoRAs attached shows the count", async () => {
    await load({ prefs: prefs({ lastModel: { image: { target: "thisComputer", model: kleinInstalled.model.id } } }) });
    const uses = [
      { id: "local:a", weight: 1, highNoise: false },
      { id: "local:b", weight: 0.5, highNoise: false },
    ];
    useStudio.setState((s) => ({ drafts: { ...s.drafts, image: { ...s.drafts.image, values: { loras: uses } } } }));
    await renderChip();
    await openSheet();
    expect(row(/^FLUX\.2 klein 4B/).textContent).toContain("2 LoRAs");
    expect(row(/^sd_xl_base_1\.0/).textContent).not.toContain("LoRA");
    cleanup();
    useStudio.getState().openSheet(false);
    useStudio.setState((s) => ({ drafts: { ...s.drafts, image: { ...s.drafts.image, values: { loras: [uses[0]] } } } }));
    await renderChip();
    await openSheet();
    expect(row(/^FLUX\.2 klein 4B/).textContent).toContain("1 LoRA");
    // As a Recent tile, too.
    cleanup();
    useStudio.getState().openSheet(false);
    useStudio.setState({ recentRuns: [runOf("a", kleinInstalled.model.id, 9, null)] });
    await renderChip();
    await openSheet();
    const recent = within(sheet()).getByRole("region", { name: "Recent" });
    expect(within(recent).getByRole("button", { name: /^FLUX\.2 klein 4B/ }).textContent).toContain("1 LoRA");
  });

  it('cloud rows show a price or "price unknown" and at most two capability icons with accessible names', async () => {
    const pricedNano: MediaModel = {
      ...nano,
      priceFetchedAt: 1794909600000,
      price: { unit: "image", low: 0.04, high: 0.04 },
    };
    await load({ endpoints: [endpoint({ models: [flare, pricedNano, k3, veo] })] });
    await renderChip();
    await openSheet();
    const flareRow = row(/^GPT Image 2\.6 Flare/);
    expect(flareRow.textContent).toContain("price unknown");
    expect(within(flareRow).getAllByRole("img").map((i) => i.getAttribute("aria-label"))).toEqual([
      "Can change a picture",
      "Transparent background",
    ]);
    expect(row(/^Nano Banana 2\.1/).textContent).toContain("$0.04 a picture");
    // An OpenRouter picture whose price was never asked for: its price comes once picked.
    expect(row(/^K3 Edit/).textContent).toContain("price unknown");
    expect(row(/^K3 Edit/).textContent).not.toContain("when picked");
    const cloud = within(sheet()).getByRole("region", { name: "OpenRouter" });
    for (const r of within(cloud).getAllByRole("button", { name: /price|a picture/ })) {
      expect(within(r).queryAllByRole("img").length).toBeLessThanOrEqual(2);
    }
    // The old badges are gone.
    expect(within(sheet()).queryByText(/^Refs \d+$/)).toBeNull();
  });

  it("a model's notes are in a tooltip reachable by keyboard", async () => {
    await load();
    await renderChip();
    await openSheet();
    const info = within(row(/^Nano Banana 2\.1/).closest("li")!).getByRole("button", { name: "About this model" });
    expect(info.tabIndex).toBe(0);
    expect(await tipOf(info)).toBe("Google adds an invisible SynthID watermark.");
    // A model without notes has no info icon.
    expect(within(row(/^K3 Edit/).closest("li")!).queryByRole("button", { name: "About this model" })).toBeNull();
  });

  it("the filter narrows the rows, and the kind follows the tab", async () => {
    await load();
    await renderChip();
    await openSheet();
    const chips = within(sheet()).getByRole("group", { name: "Show" });
    expect(within(chips).getByRole("button", { name: "All" }).getAttribute("aria-pressed")).toBe("true");
    await click(within(chips).getByRole("button", { name: "Cloud" }));
    expect(within(sheet()).queryByRole("region", { name: "On this computer" })).toBeNull();
    row(/^K3 Edit/);
    await click(within(chips).getByRole("button", { name: "On this computer" }));
    expect(within(sheet()).queryByRole("region", { name: "OpenRouter" })).toBeNull();
    row(/^FLUX\.2 klein 4B/);
    cleanup();
    useStudio.getState().openSheet(false);
    await renderChip("video");
    await openSheet();
    row(/^Wan 2\.2 5B, fast/);
    row(/^Veo 3\.1/);
    expect(within(sheet()).queryByRole("button", { name: /^FLUX\.2 klein 4B|^K3 Edit/ })).toBeNull();
  });

  it("an endpoint without a key is a row with its fix as a link", async () => {
    const opened: unknown[] = [];
    const onOpen = (e: Event) => opened.push((e as CustomEvent<unknown>).detail);
    window.addEventListener("app:open-settings", onOpen);
    try {
      await load({ endpoints: [endpoint({ hasKey: false })] });
      await renderChip();
      await openSheet();
      const cloud = within(sheet()).getByRole("region", { name: "OpenRouter" });
      // Its models are not greyed rows: the endpoint is one row, with what to do.
      expect(within(cloud).queryByRole("button", { name: /K3 Edit/ })).toBeNull();
      await click(within(cloud).getByRole("button", { name: "Add a key in Settings → Endpoints" }));
      expect(opened).toEqual(["endpoints"]);
      expect(screen.queryByRole("dialog", { name: "Choose a model" })).toBeNull();
      expect(api.studioModelPrice).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("app:open-settings", onOpen);
    }
  });

  it("an endpoint whose lists are not saved yet has a row that gets them, and says why when it cannot", async () => {
    const openai = endpoint({ providerId: 2, name: "OpenAI", company: "openai", host: "api.openai.com", fetchedAt: null, models: [] });
    await load({ endpoints: [endpoint({ fetchedAt: null, models: [] }), openai] });
    await renderChip("video");
    await openSheet();
    // OpenAI makes no video: it has no row on the Video tab.
    expect(within(sheet()).queryByRole("button", { name: "OpenAI: Get the list" })).toBeNull();
    vi.mocked(api.studioRefreshModels).mockRejectedValueOnce(unreachable);
    await click(row(/^OpenRouter: Get the list$/));
    expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
    within(sheet()).getByText("OpenRouter could not be reached.");
    vi.mocked(api.studioRefreshModels).mockResolvedValueOnce({ endpoints: [openrouter, openai] });
    await click(row(/^OpenRouter: Get the list$/));
    expect(api.studioRefreshModels).toHaveBeenCalledTimes(2);
    expect(within(sheet()).queryByText("OpenRouter could not be reached.")).toBeNull();
    expect(within(sheet()).queryByRole("button", { name: "OpenRouter: Get the list" })).toBeNull();
    row(/^Veo 3\.1/);
  });

  it("a model that needs setting up says so, and picking it opens Files", async () => {
    await load();
    await renderChip();
    await openSheet();
    const mystery = row(/^mystery/);
    expect(mystery.textContent).toContain("Needs setting up");
    // No fit dot: it has no set-up to judge yet.
    expect(within(mystery.closest("li")!).queryByRole("img")).toBeNull();
    await click(mystery);
    expect(useStudio.getState()).toMatchObject({ settingsOpen: true, settingsSection: "files", sheetOpen: false });
    expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: mysteryInstalled.model.id });
  });

  it("a local model this build cannot read says why and cannot be chosen", async () => {
    const reason = "This version of harnessUI cannot read this model. Delete it to free the space.";
    const unreadable: InstalledLocal = {
      ...kleinInstalled,
      id: "old-model",
      name: "Old Model",
      model: { ...kleinInstalled.model, id: "old-model", displayName: "Old Model", usable: reason },
    };
    await load({ local: { ...LOCAL, installed: [unreadable] } });
    await renderChip();
    await openSheet();
    const old = row(/^Old Model/);
    expect(old.getAttribute("aria-disabled")).toBe("true");
    expect(old.textContent).toContain(reason);
    // Nothing this computer can run is installed: the section names one that fits.
    within(sheet()).getByText(`No local models yet. ${klein.name} fits this computer (${amountGb(klein.sizeBytes)}).`);
    const before = useStudio.getState().drafts.image;
    await click(old);
    expect(useStudio.getState().drafts.image).toBe(before);
    sheet();
  });

  it("where this computer cannot make video, the Video tab says local models need a graphics card", async () => {
    await load({ local: { ...LOCAL, videoOnProcessor: true } });
    await renderChip("video");
    await openSheet();
    within(sheet()).getByText(graphicsCard);
  });

  it("picking an OpenRouter picture model asks for its price once", async () => {
    const unpriced = { ...flare, priceFetchedAt: null };
    await load({ endpoints: [endpoint({ models: [unpriced, nano, k3] })] });
    vi.mocked(api.studioModelPrice).mockResolvedValue(flare);
    await renderChip();
    await openSheet();
    await click(row(/^GPT Image 2\.6 Flare/));
    expect(api.studioModelPrice).toHaveBeenCalledTimes(1);
    expect(api.studioModelPrice).toHaveBeenCalledWith(1, "openai/gpt-image-2.6-flare");
    expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: unpriced.id });
    // A local model never is.
    await openSheet();
    await click(row(/^FLUX\.2 klein 4B/));
    expect(api.studioModelPrice).toHaveBeenCalledTimes(1);
  });

  it("on a window narrower than 1000 px the sheet is full height", async () => {
    window.innerWidth = 900;
    await load();
    await renderChip();
    await openSheet();
    const dialog = sheet();
    expect(dialog.className).toContain("inset-y-0");
    expect(dialog.querySelector('[data-layout="sheet"]')).not.toBeNull();
    expect(document.activeElement).toBe(search());
    await key(search(), "Escape");
    expect(screen.queryByRole("dialog", { name: "Choose a model" })).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(chipButton()));
  });

  it("Get more models opens Get models, and Add a model file opens the import dialog", async () => {
    await load();
    await renderChip();
    await openSheet();
    await click(within(sheet()).getByRole("button", { name: "Get more models" }));
    expect(useStudio.getState()).toMatchObject({ localView: true, sheetOpen: false });
    await openSheet();
    await click(within(sheet()).getByRole("button", { name: "Add a model file" }));
    expect(useStudio.getState()).toMatchObject({ importOpen: "model", sheetOpen: false });
  });

  it("Choose a cloud model opens the sheet on Cloud", async () => {
    await load();
    await renderChip();
    await act(async () => {
      useStudio.getState().openSheet(true, "cloud");
    });
    const chips = within(sheet()).getByRole("group", { name: "Show" });
    expect(within(chips).getByRole("button", { name: "Cloud" }).getAttribute("aria-pressed")).toBe("true");
    expect(within(sheet()).queryByRole("region", { name: "On this computer" })).toBeNull();
    // Opened from the chip again, it shows everything.
    await act(async () => {
      useStudio.getState().openSheet(false);
    });
    await openSheet();
    within(sheet()).getByRole("region", { name: "On this computer" });
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
