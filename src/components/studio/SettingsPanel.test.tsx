// Studio's side panel, Settings (0.1.0 Task B5; shown only when asked for since Task U3, rebuilt
// in Task U8): the model's name and where it runs, its price with *Get the price* (the model
// picker's until Task U4), the model's other simple fields in its order (a video's length and
// resolution at their cheap defaults, a local video's chips each with its own time here), then
// My setups, Advanced with its change count, and for a model on this computer its LoRAs and its
// Files (LorasSection.test.tsx, ModelFilesSection.test.tsx). Shape and How many are never here:
// they are the composer's menus (Composer.test.tsx), as are the picture zones.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactElement } from "react";
import { act, cleanup, fireEvent, render as rtlRender, screen, within, type RenderResult } from "@testing-library/react";
import type { StudioTab } from "../../store/studio";
import type { Values } from "./canonical";
import { LOCAL, MODELS } from "./fixtures/models";
import type { Eta, LocalFit, LocalModels, MediaModel, RunTarget, Setup, StudioEndpoint, StudioPrefs } from "./types";

// jsdom has none of what Radix selects, menus and sliders measure and capture with.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.hasPointerCapture ??= () => false;
Element.prototype.setPointerCapture ??= () => {};
Element.prototype.releasePointerCapture ??= () => {};
Element.prototype.scrollIntoView ??= () => {};

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const api = await import("./api");
const { SettingsPanel } = await import("./SettingsPanel");
const { ChipsRow } = await import("./AdvancedDrawer");
const { localClipSize } = await import("./SettingField");
const { UiProviders } = await import("../ui/UiProviders");
const { useStudio } = await import("../../store/studio");

/** Renders inside the one tooltip provider the panel's help buttons need, as the app does. */
function render(ui: ReactElement): RenderResult {
  return rtlRender(ui, { wrapper: UiProviders });
}

const INITIAL = useStudio.getState();

function model(id: string): MediaModel {
  const found = MODELS.find((m) => m.id === id);
  if (!found) throw new Error(`no fixture model ${id}`);
  return found;
}
const flare = model("openai/gpt-image-2.6-flare");
const nano = model("google/gemini-nano-banana-2.1");
const veo = model("google/veo-3.1");
const klein = LOCAL.installed[0]!.model;
const wan = LOCAL.installed[1]!.model;

const PRODUCT_SHOT: Setup = {
  name: "Product shot",
  kind: "image",
  target: { endpoint: 1 },
  model: "openai/gpt-image-2.6-flare",
  params: { quality: "high", background: "transparent" },
  extra: {},
  routing: null,
};

const LOCAL_SQUARE: Setup = {
  name: "Local square",
  kind: "image",
  target: "thisComputer",
  model: "flux2-klein-4b-q4",
  params: { aspect_ratio: "1:1", steps: 4 },
  extra: {},
  routing: null,
};

/** OpenRouter with `models`, as `studio_models` gives it. */
function endpoint(models: MediaModel[] = MODELS): StudioEndpoint {
  return {
    providerId: 1,
    name: "OpenRouter",
    company: "openrouter",
    host: "openrouter.ai",
    cloud: true,
    hasKey: true,
    fetchedAt: 1794909600000,
    models,
  };
}

function prefs(patch: Partial<StudioPrefs> = {}): StudioPrefs {
  return {
    lastModel: {},
    advancedOpen: false,
    settingsOpen: false,
    prefsVersion: 1,
    confirmEveryVideo: true,
    confirmUnknownPrice: false,
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
async function load(saved: { endpoints?: StudioEndpoint[]; local?: LocalModels; prefs?: StudioPrefs } = {}) {
  const p = saved.prefs ?? prefs();
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints: saved.endpoints ?? [endpoint()] });
  vi.mocked(api.studioLocalModels).mockResolvedValue(saved.local ?? LOCAL);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs: [], next: null });
  vi.mocked(api.studioLive).mockResolvedValue([]);
  vi.mocked(api.studioGetPrefs).mockResolvedValue(p);
  vi.mocked(api.studioSetPrefs).mockImplementation(async (patch) => ({ ...p, ...patch }));
  vi.mocked(api.studioUsage).mockResolvedValue({
    spentTodayUsd: 0,
    pendingUsd: 0,
    limitUsd: 10,
    folder: "/home/me/harnessUI/studio",
    bytes: 0,
    pictures: 0,
    videos: 0,
  });
  // A local model's Files section reads its files (nothing is sent).
  vi.mocked(api.studioModelFiles).mockImplementation(async (id) => ({
    genModelId: id,
    recipe: null,
    recipes: [],
    slots: [],
    untested: false,
    canRun: true,
  }));
  await act(async () => {
    await useStudio.getState().load();
  });
}

/** Studio open on `m` (used last, so its draft starts at its own starting values), at
 *  OpenRouter unless `target` says this computer; `values` replaces the draft's settings. */
async function renderPanel(
  m: MediaModel,
  opts: { target?: RunTarget; values?: Values; prefs?: Partial<StudioPrefs> } = {},
) {
  const target = opts.target ?? { endpoint: 1 };
  const tab: StudioTab = m.kind;
  const models = target === "thisComputer" ? MODELS : [m, ...MODELS.filter((x) => x.id !== m.id)];
  await load({
    endpoints: [endpoint(models)],
    prefs: prefs({ lastModel: { [tab]: { target, model: m.id } }, ...opts.prefs }),
  });
  if (opts.values) {
    const values = opts.values;
    useStudio.setState((s) => ({ drafts: { ...s.drafts, [tab]: { ...s.drafts[tab], values } } }));
  }
  useStudio.getState().showStudio(tab);
  await act(async () => {
    render(<SettingsPanel tab={tab} />);
  });
  await flush();
}

/** A field of the panel, by its label. */
function field(label: string): HTMLElement {
  return screen.getByRole("group", { name: label });
}

function checked(radio: HTMLElement): boolean {
  return radio.getAttribute("aria-checked") === "true";
}

/** Opens a Radix select or menu from its trigger. */
function press(trigger: HTMLElement) {
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false, pointerType: "mouse" });
}

/** The names of the options of the select open now. */
function optionNames(): string[] {
  return screen.getAllByRole("option").map((o) => o.textContent ?? "");
}

/** How long a clip takes here, by its length and the number of lines (a measured time unless
 *  `firstGuess`): 2 s at 480p about 3 min, 5 s about 9 min, 2 s at 720p about 10 min. */
function localFit(seconds: number, height: number, firstGuess = false): LocalFit {
  const minutes: Record<number, number> = { 1: 1.5, 2: 3, 3: 5, 4: 7, 5: 9 };
  const middle = (height >= 704 ? 10 : (minutes[seconds] ?? 1)) * 60_000;
  const eta: Eta = { lowMs: middle - 30_000, highMs: middle + 30_000, measured: !firstGuess, leftMs: null };
  return { word: "fits", verdict: "green", reason: null, suggestion: null, eta, verySlow: false, videoOnProcessor: false };
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
});

afterEach(() => {
  cleanup();
});

describe("SettingsPanel", () => {
  it("the panel holds My setups, Advanced, LoRAs and Files, and never Shape or How many", async () => {
    await renderPanel(klein, { target: "thisComputer" });
    for (const name of ["My setups", "Advanced", "LoRAs", "Files"]) screen.getByRole("region", { name });
    expect(screen.queryByRole("group", { name: "Shape" })).toBeNull();
    expect(screen.queryByRole("group", { name: "How many" })).toBeNull();
    expect(screen.queryByText("Shape")).toBeNull();
    expect(screen.queryByText("How many")).toBeNull();

    // A cloud model has neither LoRAs nor files here, and its Shape and How many are the
    // composer's too.
    cleanup();
    useStudio.setState(INITIAL, true);
    await renderPanel(flare);
    for (const name of ["My setups", "Advanced"]) screen.getByRole("region", { name });
    expect(screen.queryByRole("region", { name: "LoRAs" })).toBeNull();
    expect(screen.queryByRole("region", { name: "Files" })).toBeNull();
    expect(screen.queryByRole("group", { name: "Shape" })).toBeNull();
    expect(screen.queryByRole("group", { name: "How many" })).toBeNull();
    expect(api.studioModelFiles).toHaveBeenCalledTimes(1);
  });

  it('My setups is a select named "My setups", and applying one fills the draft', async () => {
    await renderPanel(flare, { prefs: { setups: [PRODUCT_SHOT, LOCAL_SQUARE] } });
    const menu = screen.getByRole("combobox", { name: "My setups" });
    expect(menu.textContent).toContain("None");
    press(menu);
    expect(optionNames()).toEqual(["Product shot", "Local square"]);
    fireEvent.click(screen.getByRole("option", { name: "Local square" }));
    expect(useStudio.getState().drafts.image).toMatchObject({
      target: "thisComputer",
      model: "flux2-klein-4b-q4",
      values: { aspect_ratio: "1:1", steps: 4 },
    });
    expect(screen.getByRole("combobox", { name: "My setups" }).textContent).toContain("Local square");
  });

  it("shows only the simple fields the model declares", async () => {
    await renderPanel(nano);
    expect(screen.queryByText("How many")).toBeNull();
    expect(screen.queryByText("Quality")).toBeNull();
    // Nothing set and no default named: Automatic is its first choice, chosen.
    const resolution = within(field("Resolution")).getAllByRole("radio");
    expect(resolution.map((r) => r.textContent)).toEqual(["Automatic (company default)", "1K", "2K", "4K"]);
    expect(checked(resolution[0]!)).toBe(true);
    // Its reference pictures are added in the tray, not here.
    expect(screen.queryByText("Reference pictures")).toBeNull();
  });

  it("draws the other simple fields in the model's order, each Automatic until set, in the control", async () => {
    await renderPanel(flare);
    expect(screen.getAllByRole("group").map((g) => g.getAttribute("data-param"))).toEqual(["quality"]);
    // Automatic is the select's placeholder, not a second label beside it.
    const quality = within(field("Quality")).getByRole("combobox");
    expect(quality.textContent).toBe("Automatic (company default)");
    expect(within(field("Quality")).getAllByText(/Automatic/)).toHaveLength(1);
  });

  it("starts a video at the shortest length of at least 4 s and 720p", async () => {
    await renderPanel(veo);
    expect(checked(within(field("Length")).getByRole("radio", { name: "4 s" }))).toBe(true);
    expect(checked(within(field("Resolution")).getByRole("radio", { name: "720p" }))).toBe(true);
    expect(screen.getByRole("switch", { name: "Sound" }).getAttribute("aria-checked")).toBe("true");
    expect(useStudio.getState().drafts.video.values).toEqual({ duration: 4, resolution: "720p" });
  });

  it("a local video starts at 2 s and 480p, and each chip shows its own time", async () => {
    vi.mocked(api.studioLocalFit).mockImplementation(async (_source, _w, h, seconds) => localFit(seconds, h));
    await renderPanel(wan, { target: "thisComputer" });
    await flush();
    const length = within(field("Length"));
    expect(checked(length.getByRole("radio", { name: "2 s · about 3 min" }))).toBe(true);
    length.getByRole("radio", { name: "5 s · about 9 min" });
    const resolution = within(field("Resolution"));
    expect(checked(resolution.getByRole("radio", { name: "480p · about 3 min" }))).toBe(true);
    resolution.getByRole("radio", { name: "720p · about 10 min" });

    // Each chip at its own size, once: a Length chip at the draft's 480p and 16:9 (832 × 480),
    // a Resolution chip at the draft's 2 s; nothing goes anywhere for this.
    const source = { installed: { genModelId: wan.id } };
    expect(api.studioLocalFit).toHaveBeenCalledWith(source, 832, 480, 2);
    expect(api.studioLocalFit).toHaveBeenCalledWith(source, 832, 480, 5);
    expect(api.studioLocalFit).toHaveBeenCalledWith(source, 1280, 704, 2);
    expect(api.studioLocalFit).toHaveBeenCalledTimes(6);
    expect(api.studioModelPrice).not.toHaveBeenCalled();
  });

  it("a chip's time not yet measured here says it is a first guess", async () => {
    vi.mocked(api.studioLocalFit).mockImplementation(async (_source, _w, h, seconds) => localFit(seconds, h, true));
    await renderPanel(wan, { target: "thisComputer" });
    await flush();
    within(field("Length")).getByRole("radio", { name: "2 s · about 3 min (first guess)" });
  });

  it("asks again only for the chips whose size changed", async () => {
    vi.mocked(api.studioLocalFit).mockImplementation(async (_source, _w, h, seconds) => localFit(seconds, h));
    await renderPanel(wan, { target: "thisComputer" });
    await flush();
    vi.mocked(api.studioLocalFit).mockClear();
    fireEvent.click(within(field("Length")).getByRole("radio", { name: /^5 s/ }));
    await flush();
    // Only the Resolution chips follow the length; 5 s at 480p was asked already.
    expect(api.studioLocalFit).toHaveBeenCalledTimes(1);
    expect(api.studioLocalFit).toHaveBeenCalledWith({ installed: { genModelId: wan.id } }, 1280, 704, 5);
  });

  it("asks again once the local models are read again, as a run may have been measured since", async () => {
    vi.mocked(api.studioLocalFit).mockImplementation(async (_source, _w, h, seconds) => localFit(seconds, h, true));
    await renderPanel(wan, { target: "thisComputer" });
    await flush();
    expect(api.studioLocalFit).toHaveBeenCalledTimes(6);
    vi.mocked(api.studioLocalFit).mockImplementation(async (_source, _w, h, seconds) => localFit(seconds, h));
    await act(async () => {
      await useStudio.getState().reread();
    });
    await flush();
    expect(api.studioLocalFit).toHaveBeenCalledTimes(12);
    within(field("Length")).getByRole("radio", { name: "2 s · about 3 min" });
  });

  it("a time that could not be worked out is not asked again until the local models are read again", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    try {
      // 720p at 2 s always fails; every other chip has its time.
      vi.mocked(api.studioLocalFit).mockImplementation(async (_source, w, h, seconds) => {
        if (w === 1280 && seconds === 2) throw new Error("no fit");
        return localFit(seconds, h);
      });
      await renderPanel(wan, { target: "thisComputer" });
      await flush();
      expect(api.studioLocalFit).toHaveBeenCalledTimes(6);
      // The chip shows no time; the others do.
      within(field("Resolution")).getByRole("radio", { name: "720p" });
      within(field("Resolution")).getByRole("radio", { name: "480p · about 3 min" });

      // Another field changed, and the store asked directly: neither asks again.
      act(() => useStudio.getState().setValue("video", "seed", 42));
      await flush();
      act(() => useStudio.getState().askLocalFit(wan.id, 1280, 704, 2));
      await flush();
      expect(api.studioLocalFit).toHaveBeenCalledTimes(6);
      expect(warn).toHaveBeenCalledTimes(1);

      // Read again, as a run may have been measured since: asked again.
      vi.mocked(api.studioLocalFit).mockImplementation(async (_source, _w, h, seconds) => localFit(seconds, h));
      await act(async () => {
        await useStudio.getState().reread();
      });
      await flush();
      expect(api.studioLocalFit).toHaveBeenCalledTimes(12);
      within(field("Resolution")).getByRole("radio", { name: "720p · about 10 min" });
    } finally {
      warn.mockRestore();
    }
  });

  it("a local picture's fields ask for no time", async () => {
    await renderPanel(klein, { target: "thisComputer" });
    expect(api.studioLocalFit).not.toHaveBeenCalled();
    // Its only simple fields, Shape and How many, are the composer's.
    expect(screen.queryAllByRole("group").filter((g) => g.closest("[data-advanced]") === null)).toEqual([]);
  });

  it("the panel says where a local model runs", async () => {
    await renderPanel(LOCAL.installed[0]!.model, { target: "thisComputer" });
    screen.getByText(`On this computer · ${LOCAL.hardware.device}`);
    expect(screen.queryByText(/via OpenRouter/)).toBeNull();
  });

  it("and where a cloud model's prompt goes", async () => {
    await renderPanel(flare);
    screen.getByText("via OpenRouter · Cloud");
    expect(screen.queryByText(/On this computer/)).toBeNull();
  });

  it("holds no picture zones: the composer has them", async () => {
    await renderPanel(veo);
    expect(screen.queryByText("Start frame")).toBeNull();
    expect(screen.queryByRole("button", { name: /Add the start frame/ })).toBeNull();
  });

  it("the Advanced toggle counts the changes and opens the Advanced fields", async () => {
    await renderPanel(flare, { values: { output_format: "png", background: "transparent" } });
    const toggle = screen.getByRole("button", { name: "Advanced · 2 changed" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("File type")).toBeNull();
    await act(async () => {
      fireEvent.click(toggle);
    });
    expect(api.studioSetPrefs).toHaveBeenCalledWith({ advancedOpen: true });
    expect(screen.getByRole("button", { name: "Advanced · 2 changed" }).getAttribute("aria-expanded")).toBe("true");
    screen.getByText("File type");
    expect(screen.queryByText("Compression")).toBeNull();
  });

  it("the Advanced toggle opens and closes even when saving that fails", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    try {
      await renderPanel(flare);
      vi.mocked(api.studioSetPrefs).mockRejectedValue({ code: "io", message: "The disk is full.", nextStep: "", details: "" });
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "Advanced" }));
      });
      await flush();
      expect(api.studioSetPrefs).toHaveBeenCalledWith({ advancedOpen: true });
      expect(screen.getByRole("button", { name: "Advanced" }).getAttribute("aria-expanded")).toBe("true");
      screen.getByText("File type");
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "Advanced" }));
      });
      await flush();
      expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ advancedOpen: false });
      expect(screen.getByRole("button", { name: "Advanced" }).getAttribute("aria-expanded")).toBe("false");
      expect(screen.queryByText("File type")).toBeNull();
      expect(warn).toHaveBeenCalledTimes(2);
    } finally {
      warn.mockRestore();
    }
  });

  it("the Advanced toggle follows a saved choice changed elsewhere", async () => {
    await renderPanel(flare);
    expect(screen.getByRole("button", { name: "Advanced" }).getAttribute("aria-expanded")).toBe("false");
    await act(async () => {
      await useStudio.getState().setPrefs({ advancedOpen: true });
    });
    expect(screen.getByRole("button", { name: "Advanced" }).getAttribute("aria-expanded")).toBe("true");
  });

  it("with nothing changed, the toggle reads Advanced", async () => {
    await renderPanel(flare);
    screen.getByRole("button", { name: "Advanced" });
  });

  it("says what changed when a setting moved, until the note is put away", async () => {
    await renderPanel(flare, { values: { output_format: "jpeg" } });
    act(() => useStudio.getState().setValue("image", "background", "transparent"));
    screen.getByText("Transparent needs PNG or WebP, so the file type is now PNG.");
    fireEvent.click(screen.getByRole("button", { name: "Put the note away" }));
    expect(screen.queryByText(/Transparent needs/)).toBeNull();
    expect(useStudio.getState().drafts.image.note).toBeNull();
  });
});

// The same five cases are pinned on the backend's side by `size_of_matches_the_webviews_local_clip_size`
// in src-tauri/src/studio/local.rs: a change to either rule shows in both tests.
describe("the price line (moved from the model picker, Task U4)", () => {
  const k3 = model("krea/k3-edit");
  /** A price request that could not reach OpenRouter. */
  const unreachable = {
    code: "network",
    message: "OpenRouter could not be reached.",
    nextStep: "Check the connection and try again.",
    details: "",
  };
  async function click(el: HTMLElement) {
    await act(async () => {
      fireEvent.click(el);
    });
    await flush();
  }
  async function pick(m: MediaModel, target: RunTarget = { endpoint: 1 }) {
    await act(async () => {
      useStudio.getState().chooseModel(m.kind, target, m.id);
    });
    await flush();
  }

  it("names the model, and a local model's time here", async () => {
    await renderPanel(klein, { target: "thisComputer" });
    screen.getByText("FLUX.2 klein 4B");
    screen.getByText("about 2 min a picture");
  });

  it("a restored model fetches no price when Studio opens", async () => {
    const unpriced = { ...flare, priceFetchedAt: null };
    await load({
      endpoints: [endpoint([unpriced, nano, k3])],
      prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: unpriced.id } } }),
    });
    await act(async () => {
      render(<SettingsPanel tab="image" />);
    });
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

  it("a price that could not be got when picked says so, and Get the price asks again", async () => {
    await load();
    await act(async () => {
      render(<SettingsPanel tab="image" />);
    });
    vi.mocked(api.studioModelPrice).mockRejectedValueOnce(unreachable);
    await pick(nano);
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
      endpoints: [endpoint([unpriced, nano, k3])],
      prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: unpriced.id } } }),
    });
    await act(async () => {
      render(<SettingsPanel tab="image" />);
    });
    vi.mocked(api.studioModelPrice).mockRejectedValueOnce(unreachable);
    await click(screen.getByRole("button", { name: "Get the price" }));
    screen.getByRole("alert");
    // Another model: the error was for the one before.
    await pick(klein, "thisComputer");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(useStudio.getState().priceError.image).toBeNull();
    // A price on its way for GPT Image 2.6 Flare while another model is picked.
    let fail: (e: unknown) => void = () => undefined;
    vi.mocked(api.studioModelPrice).mockReturnValue(new Promise((_, reject) => (fail = reject)));
    await pick(nano);
    expect(screen.queryByRole("alert")).toBeNull();
    await pick(unpriced);
    const asking = screen.getByRole("button", { name: "Get the price" }) as HTMLButtonElement;
    await click(asking);
    expect(asking.disabled).toBe(true);
    await pick(nano);
    expect((screen.getByRole("button", { name: "Get the price" }) as HTMLButtonElement).disabled).toBe(false);
    // Both requests fail: only the one for the model shown is said, once.
    await act(async () => {
      fail(unreachable);
    });
    await flush();
    expect(screen.getAllByRole("alert")).toHaveLength(1);
  });
});

describe("localClipSize", () => {
  it("follows the shape and resolution on the model's multiple and longest side", () => {
    expect(localClipSize(wan, { resolution: "480p" })).toEqual({ w: 832, h: 480 });
    expect(localClipSize(wan, {}, "720p")).toEqual({ w: 1280, h: 704 });
    expect(localClipSize(wan, { aspect_ratio: "9:16" }, "720p")).toEqual({ w: 704, h: 1280 });
    expect(localClipSize(wan, { aspect_ratio: "1:1" }, "720p")).toEqual({ w: 704, h: 704 });
  });

  it("a Length chip uses the draft's resolution, else the Resolution field's starting value", () => {
    expect(localClipSize(wan, { resolution: "720p" })).toEqual({ w: 1280, h: 704 });
    expect(localClipSize(wan, {})).toEqual({ w: 832, h: 480 });
  });

  it("an exact size is used as it is, for every chip", () => {
    expect(localClipSize(wan, { size: "1024x576" })).toEqual({ w: 1024, h: 576 });
    expect(localClipSize(wan, { size: "1024x576", resolution: "720p" }, "480p")).toEqual({ w: 1024, h: 576 });
  });
});

describe("the draft", () => {
  /** The fixture's first picture model under another name, with no WebP. */
  const lite: MediaModel = {
    ...flare,
    id: "example/no-webp",
    displayName: "No WebP",
    params: flare.params.map((p) =>
      p.key === "output_format" && p.kind.type === "enum"
        ? { ...p, kind: { ...p.kind, values: p.kind.values.filter((c) => c.value !== "webp") } }
        : p,
    ),
  };

  it("a model picked drops the carried settings that no longer apply there", async () => {
    await load({
      endpoints: [endpoint([flare, lite])],
      prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: flare.id } } }),
    });
    useStudio.setState((s) => ({
      drafts: { ...s.drafts, image: { ...s.drafts.image, values: { output_format: "webp", output_compression: 80 } } },
    }));
    useStudio.getState().chooseModel("image", { endpoint: 1 }, lite.id);
    expect(useStudio.getState().drafts.image.values).toEqual({});
  });

  it("so does the model's answer when it comes back with its price", async () => {
    // Nano Banana as its lists name it, and as OpenRouter answers for its price: there, its
    // resolution applies only to a square picture.
    const answered: MediaModel = {
      ...nano,
      priceFetchedAt: 1794909600000,
      params: [
        ...nano.params.map((p) =>
          p.key === "resolution" ? { ...p, requires: { key: "aspect_ratio", values: ["1:1"] } } : p,
        ),
        { ...flare.params.find((p) => p.kind.type === "extra")!, kind: { type: "extra", allowedKeys: ["moderation"] } },
      ],
    };
    await load({ prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: flare.id } } }) });
    useStudio.setState((s) => ({
      drafts: { ...s.drafts, image: { ...s.drafts.image, values: { quality: "low" }, extra: { moderation: "low" } } },
    }));
    let answer: (m: MediaModel) => void = () => undefined;
    vi.mocked(api.studioModelPrice).mockReturnValue(new Promise((resolve) => (answer = resolve)));
    useStudio.getState().chooseModel("image", { endpoint: 1 }, nano.id);
    act(() => useStudio.getState().setValue("image", "resolution", "2K"));
    expect(useStudio.getState().drafts.image.values).toEqual({ resolution: "2K" });
    await act(async () => {
      answer(answered);
    });
    await flush();
    expect(useStudio.getState().drafts.image.values).toEqual({});
    expect(useStudio.getState().drafts.image.extra).toEqual({ moderation: "low" });
  });
});

// My setups (0.1.0 Task B6; its menu a styled select since Task U8).
describe("My setups", () => {
  it("a setup brings back where it runs, the model and every setting", async () => {
    await load({ endpoints: [endpoint()] });
    useStudio.getState().applySetup(PRODUCT_SHOT);
    expect(useStudio.getState().drafts.image).toMatchObject({
      target: { endpoint: 1 },
      model: "openai/gpt-image-2.6-flare",
      values: { quality: "high", background: "transparent" },
    });
    useStudio.getState().applySetup(LOCAL_SQUARE);
    expect(useStudio.getState().drafts.image).toMatchObject({
      target: "thisComputer",
      model: "flux2-klein-4b-q4",
      values: { aspect_ratio: "1:1", steps: 4 },
    });
  });

  it("a setup's values, extra fields and routing apply exactly as saved, and the prompt stays", async () => {
    await load({ endpoints: [endpoint()] });
    useStudio.getState().setPrompt("image", "a lighthouse");
    const routing = { order: ["openai"], only: false, allowFallbacks: false, zdr: true };
    expect(useStudio.getState().applySetup({ ...PRODUCT_SHOT, extra: { moderation: "low" }, routing })).toBeNull();
    expect(useStudio.getState().drafts.image).toMatchObject({
      prompt: "a lighthouse",
      values: { quality: "high", background: "transparent" },
      extra: { moderation: "low" },
      routing,
      note: null,
    });
    expect(Object.keys(useStudio.getState().drafts.image.values)).toEqual(["quality", "background"]);
  });

  it("a setup's value whose field no longer applies is not kept", async () => {
    await load({ endpoints: [endpoint()] });
    useStudio.getState().applySetup({ ...PRODUCT_SHOT, params: { output_format: "png", output_compression: 80 } });
    expect(useStudio.getState().drafts.image.values).toEqual({ output_format: "png" }); // compression: JPEG and WebP only
  });

  it("a setup whose model was retired gets the closest one and says what changed", async () => {
    await load({ endpoints: [endpoint()] });
    const old: Setup = {
      name: "Old clip",
      kind: "video",
      target: { endpoint: 1 },
      model: "google/veo-3.0",
      params: { duration: 5 },
      extra: {},
      routing: null,
    };
    expect(useStudio.getState().applySetup(old)).toBeNull();
    expect(useStudio.getState().drafts.video).toMatchObject({
      model: "google/veo-3.1",
      values: { duration: 4, resolution: "720p" },
      note: "OpenRouter no longer offers this setup's model, so Veo 3.1 is used. Length 5 s became 4 s.",
    });
  });

  it("a setup with nothing left to run says so and changes nothing", async () => {
    await load({ endpoints: [endpoint()] });
    const before = useStudio.getState().drafts.image;
    expect(useStudio.getState().applySetup({ ...PRODUCT_SHOT, model: "acme/gone-1" })).toBe(
      "OpenRouter no longer offers this setup's model.",
    );
    expect(useStudio.getState().applySetup({ ...PRODUCT_SHOT, target: { endpoint: 9 } })).toBe(
      "This setup's endpoint was removed in Settings → Endpoints.",
    );
    expect(useStudio.getState().applySetup({ ...LOCAL_SQUARE, model: "gone-local" })).toBe(
      "This setup's model is no longer on this computer.",
    );
    expect(useStudio.getState().drafts.image).toBe(before);
  });

  it("a setup's extra fields wait for the price of a model never asked, then go in or are removed with the note", async () => {
    const unpriced: MediaModel = {
      ...flare,
      priceFetchedAt: null,
      params: flare.params.filter((p) => p.kind.type !== "extra"),
    };
    await load({ endpoints: [endpoint([unpriced, ...MODELS.filter((m) => m.id !== flare.id)])] });
    let answer: (m: MediaModel) => void = () => undefined;
    vi.mocked(api.studioModelPrice).mockReturnValue(new Promise((resolve) => (answer = resolve)));
    await act(async () => {
      render(<ChipsRow tab="image" />);
    });
    let refused: string | null = "";
    act(() => {
      refused = useStudio.getState().applySetup({ ...PRODUCT_SHOT, params: {}, extra: { moderation: "low", foo: 1 } });
    });
    expect(refused).toBeNull();
    expect(api.studioModelPrice).toHaveBeenCalledWith(1, flare.id);
    // Held aside until the model's Extra field is known: no chip, and nothing in the request.
    expect(useStudio.getState().drafts.image).toMatchObject({ model: flare.id, extra: {}, note: null });
    expect(screen.queryByRole("button", { name: /^Remove / })).toBeNull();
    await act(async () => {
      answer(flare);
    });
    await flush();
    expect(useStudio.getState().drafts.image).toMatchObject({
      extra: { moderation: "low" },
      note: "GPT Image 2.6 Flare does not accept foo, so it was removed.",
    });
    screen.getByRole("button", { name: "Remove moderation: low" });
  });

  it("a setup at an endpoint without a key says so and changes nothing", async () => {
    await load({ endpoints: [{ ...endpoint(), hasKey: false }] });
    const before = useStudio.getState().drafts.image;
    expect(useStudio.getState().applySetup(PRODUCT_SHOT)).toBe(
      "OpenRouter has no key, so Studio cannot use its models. Add a key in Settings → Endpoints.",
    );
    expect(useStudio.getState().drafts.image).toBe(before);
    expect(api.studioModelPrice).not.toHaveBeenCalled();
  });

  it("Save as setup saves the model and every setting under a name, and the menu shows it", async () => {
    await renderPanel(flare);
    act(() => useStudio.getState().setValue("image", "quality", "high"));
    act(() => useStudio.getState().setExtra("image", { moderation: "low" }));
    fireEvent.click(screen.getByRole("button", { name: "Save as setup…" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Setup name" }), { target: { value: "  Product shot " } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save setup" }));
    });
    await flush();
    expect(api.studioSetPrefs).toHaveBeenCalledWith({
      setups: [{ ...PRODUCT_SHOT, params: { quality: "high" }, extra: { moderation: "low" } }],
    });
    expect(screen.queryByRole("textbox", { name: "Setup name" })).toBeNull();
    const menu = screen.getByRole("combobox", { name: "My setups" });
    expect(menu.textContent).toBe("Product shot");
    // A change to the draft: the menu no longer names the setup.
    act(() => useStudio.getState().setValue("image", "quality", "low"));
    expect(menu.textContent).toBe("None");
  });

  it("a setup saved under a name in use replaces it", async () => {
    await renderPanel(flare, { prefs: { setups: [LOCAL_SQUARE, { ...PRODUCT_SHOT, params: { quality: "low" } }] } });
    fireEvent.click(screen.getByRole("button", { name: "Save as setup…" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Setup name" }), { target: { value: "Product shot" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save setup" }));
    });
    expect(api.studioSetPrefs).toHaveBeenCalledWith({ setups: [LOCAL_SQUARE, { ...PRODUCT_SHOT, params: {} }] });
  });

  it("a save that fails says why and what to do", async () => {
    await renderPanel(flare);
    vi.mocked(api.studioSetPrefs).mockRejectedValue({
      code: "io",
      message: "Studio's preferences could not be saved.",
      nextStep: "Free up some disk space, then try again.",
      details: "",
    });
    fireEvent.click(screen.getByRole("button", { name: "Save as setup…" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Setup name" }), { target: { value: "Product shot" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save setup" }));
    });
    await flush();
    screen.getByText("Studio's preferences could not be saved.");
    screen.getByText("Free up some disk space, then try again.");
  });

  it("keeps at most 50 setups", async () => {
    const fifty = Array.from({ length: 50 }, (_, i) => ({ ...PRODUCT_SHOT, name: `Setup ${i + 1}` }));
    await renderPanel(flare, { prefs: { setups: fifty } });
    fireEvent.click(screen.getByRole("button", { name: "Save as setup…" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Setup name" }), { target: { value: "One more" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save setup" }));
    });
    screen.getByText("Keep at most 50 setups.");
    expect(api.studioSetPrefs).not.toHaveBeenCalledWith(expect.objectContaining({ setups: expect.anything() }));
  });

  it("a name is needed", async () => {
    await renderPanel(flare);
    fireEvent.click(screen.getByRole("button", { name: "Save as setup…" }));
    expect((screen.getByRole("button", { name: "Save setup" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByRole("textbox", { name: "Setup name" }), { target: { value: "   " } });
    expect((screen.getByRole("button", { name: "Save setup" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("textbox", { name: "Setup name" })).toBeNull();
  });

  it("the menu lists the tab's setups, and picking one applies it", async () => {
    const clip: Setup = { ...PRODUCT_SHOT, name: "A clip", kind: "video", model: veo.id, params: { duration: 8 } };
    await renderPanel(flare, { prefs: { setups: [clip, LOCAL_SQUARE, PRODUCT_SHOT] } });
    const menu = screen.getByRole("combobox", { name: "My setups" });
    expect(menu.textContent).toBe("None");
    press(menu);
    // The Pictures tab's setups only; None is the menu's placeholder.
    expect(optionNames()).toEqual(["Local square", "Product shot"]);
    fireEvent.click(screen.getByRole("option", { name: "Local square" }));
    expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: "flux2-klein-4b-q4" });
    expect(screen.getByRole("combobox", { name: "My setups" }).textContent).toBe("Local square");
  });

  it("the menu still names the setup a tab was set from after another tab was shown", async () => {
    const clip: Setup = { ...PRODUCT_SHOT, name: "A clip", kind: "video", model: veo.id, params: { duration: 8 } };
    await load({
      prefs: prefs({
        lastModel: { image: { target: { endpoint: 1 }, model: flare.id }, video: { target: { endpoint: 1 }, model: veo.id } },
        setups: [clip, PRODUCT_SHOT],
      }),
    });
    useStudio.getState().showStudio("image");
    let view: RenderResult | null = null;
    await act(async () => {
      view = render(<SettingsPanel tab="image" />);
    });
    await flush();
    const rerender = async (tab: StudioTab) => {
      await act(async () => {
        view!.rerender(<SettingsPanel tab={tab} />);
      });
      await flush();
    };
    const menu = () => screen.getByRole("combobox", { name: "My setups" });
    const pick = (name: string) => {
      press(menu());
      fireEvent.click(screen.getByRole("option", { name }));
    };
    pick("Product shot");
    expect(menu().textContent).toBe("Product shot");
    const pictures = useStudio.getState().drafts.image;

    // The same panel shows the Video tab: nothing was applied there yet.
    await rerender("video");
    expect(menu().textContent).toBe("None");
    pick("A clip");
    expect(menu().textContent).toBe("A clip");

    // Back on Pictures, the draft is as Product shot left it, so the menu still names it.
    await rerender("image");
    expect(useStudio.getState().drafts.image).toBe(pictures);
    expect(menu().textContent).toBe("Product shot");
    await rerender("video");
    expect(menu().textContent).toBe("A clip");
  });

  it("picking a setup that cannot run says why", async () => {
    await renderPanel(flare, { prefs: { setups: [{ ...PRODUCT_SHOT, model: "acme/gone-1" }] } });
    press(screen.getByRole("combobox", { name: "My setups" }));
    fireEvent.click(screen.getByRole("option", { name: "Product shot" }));
    screen.getByText("OpenRouter no longer offers this setup's model.");
    expect(useStudio.getState().drafts.image.model).toBe(flare.id);
  });
});
