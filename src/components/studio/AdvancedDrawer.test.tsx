// Studio's Advanced drawer (0.1.0 Task B6; design Wireframe 2; styled controls since Task U8):
// every setting the model offers one click away with its reset, each change a chip under the
// prompt (the composer's ChipsRow), the seed's dice and lock, the full sampler and scheduler
// lists as selects, an OpenRouter picture's routing, Extra fields (JSON) with the names the model
// accepts, and the exact request (on this computer, the exact command). LoRAs have their own
// section (LorasSection.test.tsx), so they are not counted here; My setups are tested with the
// panel (SettingsPanel.test.tsx).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactElement } from "react";
import { act, cleanup, fireEvent, render as rtlRender, screen, within, type RenderResult } from "@testing-library/react";
import type { StudioTab } from "../../store/studio";
import type { Values } from "./canonical";
import { LOCAL, MODELS } from "./fixtures/models";
import type {
  Eta,
  LocalFit,
  LocalModels,
  MediaModel,
  RequestPreview,
  RunTarget,
  StudioEndpoint,
  StudioPrefs,
} from "./types";

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
const { AdvancedDrawer, ChipsRow, chipsFor, parseExtra } = await import("./AdvancedDrawer");
const { SettingsPanel } = await import("./SettingsPanel");
const { UiProviders } = await import("../ui/UiProviders");
const { useStudio } = await import("../../store/studio");

/** Renders inside the one tooltip provider the fields' help buttons need, as the app does. */
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
const k3Edit = model("krea/k3-edit");
const veo = model("google/veo-3.1");
const klein = LOCAL.installed[0]!.model;

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
    advancedOpen: true,
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
  vi.mocked(api.studioSetPrefs).mockImplementation(async (patch) => ({ ...useStudio.getState().prefs!, ...patch }));
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

/** Studio open on `m` (used last), at OpenRouter unless `target` says this computer, with
 *  Advanced open unless `prefs` says otherwise; `values` replaces the draft's settings. */
async function show(m: MediaModel, values?: Values, opts: { target?: RunTarget; prefs?: Partial<StudioPrefs> } = {}) {
  const target = opts.target ?? { endpoint: 1 };
  const tab: StudioTab = m.kind;
  const models = target === "thisComputer" ? MODELS : [m, ...MODELS.filter((x) => x.id !== m.id)];
  await load({
    endpoints: [endpoint(models)],
    prefs: prefs({ lastModel: { [tab]: { target, model: m.id } }, ...opts.prefs }),
  });
  if (values) {
    useStudio.setState((s) => ({ drafts: { ...s.drafts, [tab]: { ...s.drafts[tab], values } } }));
  }
  useStudio.getState().showStudio(tab);
  return tab;
}

/** The drawer of `m`'s tab, with the prompt dock's chips under it, in a Studio opened afresh
 *  (what was rendered before goes first). */
async function renderDrawer(m: MediaModel, values?: Values, opts: { target?: RunTarget } = {}) {
  cleanup();
  useStudio.setState(INITIAL, true);
  const tab = await show(m, values, opts);
  await act(async () => {
    render(
      <>
        <AdvancedDrawer tab={tab} />
        <ChipsRow tab={tab} />
      </>,
    );
  });
  await flush();
}

function draftSeed(): unknown {
  return useStudio.getState().drafts.image.values.seed;
}

/** Opens a Radix select from its trigger. */
function press(trigger: HTMLElement) {
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false, pointerType: "mouse" });
}

/** Picks `option` in the select named `name`. */
function choose(name: string, option: string) {
  press(screen.getByRole("combobox", { name }));
  fireEvent.click(screen.getByRole("option", { name: option }));
}

/** How long a clip takes here, by its length and the number of lines (measured): 2 s at 480p
 *  about 3 min, 5 s about 9 min, 2 s at 720p about 10 min. */
function localFit(seconds: number, height: number): LocalFit {
  const minutes: Record<number, number> = { 1: 1.5, 2: 3, 3: 5, 4: 7, 5: 9 };
  const middle = (height >= 704 ? 10 : (minutes[seconds] ?? 1)) * 60_000;
  const eta: Eta = { lowMs: middle - 30_000, highMs: middle + 30_000, measured: true, leftMs: null };
  return { word: "fits", verdict: "green", reason: null, suggestion: null, eta, verySlow: false, videoOnProcessor: false };
}

/** The exact request of a GPT Image 2.6 Flare picture, as `studio_preview` gives it. */
const FLARE_PREVIEW: RequestPreview = {
  method: "POST",
  url: "https://openrouter.ai/api/v1/images/generations",
  host: "openrouter.ai",
  body: { model: "openai/gpt-image-2.6-flare" },
};

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
});

afterEach(() => {
  cleanup();
});

describe("AdvancedDrawer", () => {
  it("counts changed fields in the header", async () => {
    await renderDrawer(flare, { output_format: "png", background: "transparent" });
    screen.getByText("Advanced · 2 changed");
  });

  it("each changed field shows as a chip that removes it", async () => {
    expect(chipsFor(flare, { background: "transparent" }, { moderation: "low" })).toEqual([
      { key: "background", text: "background: transparent" },
      { key: "moderation", text: "moderation: low" },
    ]);
    expect(chipsFor(k3Edit, { seed: 81723, aspect_ratio: "1:1" }, {})).toEqual([{ key: "seed", text: "seed 81723" }]); // Shape is simple
    expect(chipsFor(flare, { output_format: "webp", output_compression: 80 }, { complexity: { level: 2 } })).toEqual([
      { key: "output_format", text: "file type: WebP" },
      { key: "output_compression", text: "compression: 80%" },
      { key: "complexity", text: 'complexity: {"level":2}' },
    ]);

    await renderDrawer(flare, { background: "transparent" });
    act(() => useStudio.getState().setExtra("image", { moderation: "low" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove background: transparent" }));
    expect(useStudio.getState().drafts.image.values.background).toBeUndefined();
    fireEvent.click(screen.getByRole("button", { name: "Remove moderation: low" }));
    expect(useStudio.getState().drafts.image.extra).toEqual({});
    expect(screen.queryByRole("button", { name: /^Remove / })).toBeNull();
  });

  it("Reset all and each field's reset go back to Automatic", async () => {
    await renderDrawer(flare, { output_format: "png", background: "transparent" });
    act(() => useStudio.getState().setExtra("image", { moderation: "low" }));
    act(() => useStudio.getState().setRouting("image", { order: ["openai"], only: false, allowFallbacks: true, zdr: false }));
    fireEvent.click(screen.getByRole("button", { name: "Reset Background" }));
    const background = screen.getByRole("group", { name: "Background" });
    expect(within(background).getByRole("radio", { name: "Automatic (company default)" }).getAttribute("aria-checked")).toBe(
      "true",
    );
    expect(useStudio.getState().drafts.image.values).toEqual({ output_format: "png" });
    fireEvent.click(screen.getByRole("button", { name: "Reset all" }));
    screen.getByText("Advanced"); // no "changed" count left
    expect(useStudio.getState().drafts.image).toMatchObject({ values: {}, extra: {}, routing: null });
    expect(screen.queryByRole("button", { name: "Reset all" })).toBeNull();
  });

  it("Reset all keeps the simple fields", async () => {
    await renderDrawer(flare, { quality: "high", n: 2, output_format: "jpeg", output_compression: 80 });
    fireEvent.click(screen.getByRole("button", { name: "Reset all" }));
    expect(useStudio.getState().drafts.image.values).toEqual({ quality: "high", n: 2 });
  });

  it("Extra fields accept only the names the model lists", () => {
    expect(parseExtra('{"moderation":"low"}', ["moderation"], [])).toEqual({ ok: true, value: { moderation: "low" } });
    expect(parseExtra('{"foo":1}', ["moderation"], [])).toEqual({ ok: false, message: "This model does not accept foo." });
    expect(parseExtra("{nope", ["moderation"], [])).toEqual({ ok: false, message: "This is not valid JSON." });
    expect(parseExtra('{"duration":4}', ["duration"], ["duration"])).toEqual({
      ok: false,
      message: "Set duration with its own field above.",
    });
    expect(parseExtra('["moderation"]', ["moderation"], [])).toEqual({
      ok: false,
      message: 'Write the extra fields as one JSON object, like {"moderation": "low"}.',
    });
    expect(parseExtra("  ", ["moderation"], [])).toEqual({ ok: true, value: {} });
  });

  it("Edit JSON saves extra fields the model accepts and says why it refuses others", async () => {
    await renderDrawer(flare);
    screen.getByText("none set");
    fireEvent.click(screen.getByRole("button", { name: "Edit JSON" }));
    const box = screen.getByRole("textbox", { name: "Extra fields (JSON)" });
    fireEvent.change(box, { target: { value: '{"foo": 1}' } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    screen.getByText("This model does not accept foo.");
    expect(useStudio.getState().drafts.image.extra).toEqual({});
    fireEvent.change(box, { target: { value: '{"moderation": "low"}' } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(useStudio.getState().drafts.image.extra).toEqual({ moderation: "low" });
    expect(screen.queryByRole("textbox", { name: "Extra fields (JSON)" })).toBeNull();
    screen.getByText("Advanced · 1 changed");
    screen.getByText("moderation: low"); // its chip
    // Edit JSON again starts from what is set, and Cancel keeps it.
    fireEvent.click(screen.getByRole("button", { name: "Edit JSON" }));
    expect((screen.getByRole("textbox", { name: "Extra fields (JSON)" }) as HTMLTextAreaElement).value).toBe(
      '{\n  "moderation": "low"\n}',
    );
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(useStudio.getState().drafts.image.extra).toEqual({ moderation: "low" });
  });

  it("lists the accepted names as hints", async () => {
    await renderDrawer(flare);
    screen.getByText("This model accepts: moderation, complexity"); // from flare's saved /endpoints answer
    await renderDrawer({ ...flare, params: flare.params.filter((p) => p.key !== "extra"), priceFetchedAt: null });
    screen.getByText("Pick the model to see its extra settings.");
    expect(screen.queryByRole("button", { name: "Edit JSON" })).toBeNull();
    await renderDrawer(veo); // OpenRouter sends no extra settings for a video
    expect(screen.queryByText(/Extra fields/)).toBeNull();
    expect(screen.queryByText("Pick the model to see its extra settings.")).toBeNull();
  });

  it("See the exact request shows where it goes, without the key", async () => {
    const preview: RequestPreview = {
      method: "POST",
      url: "https://openrouter.ai/api/v1/images/generations",
      host: "openrouter.ai",
      body: { model: "openai/gpt-image-2.6-flare", prompt: "a lighthouse", background: "transparent" },
    };
    vi.mocked(api.studioPreview).mockResolvedValue(preview);
    await renderDrawer(flare, { background: "transparent" });
    act(() => useStudio.getState().setPrompt("image", "a lighthouse"));
    expect(api.studioPreview).not.toHaveBeenCalled(); // asked only on the click
    fireEvent.click(screen.getByRole("button", { name: "See the exact request" }));
    await screen.findByText("key removed · goes to openrouter.ai");
    screen.getByText("POST https://openrouter.ai/api/v1/images/generations");
    screen.getByText(/"background": "transparent"/);
    expect(api.studioPreview).toHaveBeenCalledWith(
      expect.objectContaining({
        target: { endpoint: 1 },
        kind: "image",
        model: flare.id,
        prompt: "a lighthouse",
        params: { background: "transparent" },
        extra: {},
        refs: [],
        routing: null,
      }),
    );
    expect(screen.queryByText("Runs on this computer; nothing is sent.")).toBeNull();
  });

  it("the exact request follows a setting changed while it is open, and closes again", async () => {
    vi.mocked(api.studioPreview).mockImplementation(async (req) => ({
      method: "POST",
      url: "https://openrouter.ai/api/v1/images/generations",
      host: "openrouter.ai",
      body: { model: req.model, ...req.params },
    }));
    await renderDrawer(flare);
    fireEvent.click(screen.getByRole("button", { name: "See the exact request" }));
    await screen.findByText("key removed · goes to openrouter.ai");
    act(() => useStudio.getState().setValue("image", "background", "opaque"));
    await screen.findByText(/"background": "opaque"/);
    fireEvent.click(screen.getByRole("button", { name: "See the exact request" }));
    expect(screen.queryByText("key removed · goes to openrouter.ai")).toBeNull();
  });

  it("a request the model would refuse says why in place of the request", async () => {
    vi.mocked(api.studioPreview).mockRejectedValue({
      code: "studio_invalid_settings",
      message: "GPT Image 2.6 Flare does not offer this size.",
      nextStep: "Choose another size.",
      details: "",
    });
    await renderDrawer(flare);
    fireEvent.click(screen.getByRole("button", { name: "See the exact request" }));
    await screen.findByText("GPT Image 2.6 Flare does not offer this size.");
    expect(screen.queryByText(/key removed/)).toBeNull();
  });

  it("the dice gives a new seed and Keep it locks it", async () => {
    await renderDrawer(k3Edit, { seed: 81723 });
    fireEvent.click(screen.getByRole("button", { name: "New seed" }));
    expect(draftSeed()).not.toBe(81723);
    expect(Number.isInteger(draftSeed())).toBe(true);
    screen.getByText(`seed ${String(draftSeed())}`); // its chip
    fireEvent.click(screen.getByRole("checkbox", { name: "Keep it" }));
    expect(useStudio.getState().drafts.image.seedLocked).toBe(true);
    const help = screen.getByText(k3Edit.params.find((p) => p.key === "seed")!.help!);
    expect(screen.getByRole("group", { name: "Seed" }).getAttribute("aria-describedby")).toBe(help.id); // its (?) help
  });

  it("a routing choice is offered for OpenRouter pictures only", async () => {
    await renderDrawer(flare);
    screen.getByText("Company that runs it");
    const routing = () => useStudio.getState().drafts.image.routing;
    const allow = () => screen.getByRole("checkbox", { name: "Allow another company if that one is busy" }) as HTMLButtonElement;
    expect(allow().disabled).toBe(true); // OpenRouter chooses
    expect(screen.getByRole("combobox", { name: "Company that runs it" }).textContent).toBe("OpenRouter chooses");
    choose("Company that runs it", "OpenAI");
    expect(routing()).toEqual({ order: ["openai"], only: false, allowFallbacks: true, zdr: false });
    expect(allow().disabled).toBe(false);
    fireEvent.click(allow());
    expect(routing()).toEqual({ order: ["openai"], only: false, allowFallbacks: false, zdr: false });
    fireEvent.click(screen.getByRole("checkbox", { name: "Only companies that keep no data" }));
    expect(routing()).toEqual({ order: ["openai"], only: false, allowFallbacks: false, zdr: true });
    choose("Company that runs it", "OpenRouter chooses");
    expect(routing()).toEqual({ order: [], only: false, allowFallbacks: true, zdr: true });
    fireEvent.click(screen.getByRole("checkbox", { name: "Only companies that keep no data" }));
    expect(routing()).toBeNull();

    await renderDrawer(veo);
    expect(screen.queryByText("Company that runs it")).toBeNull();
    screen.getByText(veo.params.find((p) => p.key === "seed")!.help!); // "… gives a different video."
  });

  it("Sampler and Scheduler are selects listing every engine choice by its plain name", async () => {
    const sdxl = LOCAL.installed.find((m) => m.version === "sdxl" && !m.untested)!.model;
    const sampler = sdxl.params.find((p) => p.key === "sampler")!;
    const scheduler = sdxl.params.find((p) => p.key === "scheduler")!;
    if (sampler.kind.type !== "enum" || scheduler.kind.type !== "enum") throw new Error("not lists");
    await renderDrawer(sdxl, {}, { target: "thisComputer" });
    // Nothing set and no default named: Automatic is each one's placeholder.
    expect(screen.getByRole("combobox", { name: "Sampler" }).textContent).toBe("Automatic");
    press(screen.getByRole("combobox", { name: "Sampler" }));
    const samplers = screen.getAllByRole("option");
    expect(samplers).toHaveLength(21);
    expect(samplers.map((o) => o.textContent)).toEqual(sampler.kind.values.map((c) => c.label));
    // Plain names, never the engine's ids.
    expect(samplers.map((o) => o.textContent)).not.toContain("dpm++2m");
    fireEvent.click(screen.getByRole("option", { name: sampler.kind.values[5]!.label }));
    expect(useStudio.getState().drafts.image.values.sampler).toBe(sampler.kind.values[5]!.value);
    press(screen.getByRole("combobox", { name: "Scheduler" }));
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(scheduler.kind.values.map((c) => c.label));
    expect(screen.getAllByRole("option")).toHaveLength(17);
    fireEvent.keyDown(screen.getByRole("listbox"), { key: "Escape" });
    // Set, the Sampler offers Automatic again, first, and it resets the field.
    choose("Sampler", "Automatic");
    expect(useStudio.getState().drafts.image.values.sampler).toBeUndefined();
    expect(document.querySelector("select")).toBeNull();
    expect(document.querySelector("input[type=range], input[type=checkbox]")).toBeNull();
  });

  it("a model's own default shows in its select while it is unset", async () => {
    await renderDrawer(klein, {}, { target: "thisComputer" });
    // klein's engine runs Euler unless told otherwise.
    expect(screen.getByRole("combobox", { name: "Sampler" }).textContent).toBe("Euler");
    expect(screen.queryByRole("button", { name: "Reset Sampler" })).toBeNull();
  });

  it("LoRAs are not counted as Advanced changes, show no chip, and Reset all keeps them", async () => {
    await renderDrawer(klein, { steps: 8 }, { target: "thisComputer" });
    act(() => useStudio.getState().attachLora(LOCAL.loras[0]!.id));
    expect(useStudio.getState().drafts.image.values.loras).toHaveLength(1);
    screen.getByText("Advanced · 1 changed");
    expect(screen.queryByText(/object Object/)).toBeNull();
    expect(screen.queryByText(/^LoRAs:/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Reset all" }));
    expect(useStudio.getState().drafts.image.values).toEqual({ loras: [{ id: LOCAL.loras[0]!.id, weight: 1, highNoise: false }] });
  });

  it("a local model offers steps, memory saving and where it runs, and no Extra fields", async () => {
    await renderDrawer(LOCAL.installed[0]!.model, {}, { target: "thisComputer" });
    screen.getByText("Steps");
    screen.getByText("Memory saving");
    screen.getByText("Run on");
    expect(screen.queryByText(/Extra fields/)).toBeNull();
    expect(screen.queryByText("Company that runs it")).toBeNull();
    expect(screen.queryByText("Pick the model to see its extra settings.")).toBeNull();
    screen.getByText(LOCAL.installed[0]!.model.params.find((p) => p.key === "seed")!.help!);
    expect(screen.queryByRole("button", { name: "See the exact request" })).toBeNull();
  });

  it("See the exact command shows the engine's arguments for a local model", async () => {
    vi.mocked(api.studioPreview).mockResolvedValue({
      method: "run",
      url: "sd-cli",
      host: "",
      body: {
        argv: ["--diffusion-model", "/m/flux2-klein-4b-Q4_K_M.gguf", "--prompt-file", "/g/.partial/r/prompt.txt", "--steps", "4"],
        prompt: "a lighthouse",
        negativePrompt: null,
      },
    });
    await renderDrawer(klein, {}, { target: "thisComputer" });
    fireEvent.click(screen.getByRole("button", { name: "See the exact command" }));
    await screen.findByText("--prompt-file");
    screen.getByText("/g/.partial/r/prompt.txt"); // one argument a line
    screen.getByText("Prompt (passed in prompt.txt): a lighthouse");
    expect(screen.queryByText(/Negative prompt/)).toBeNull();
    screen.getByText("Runs on this computer; nothing is sent.");
    expect(screen.queryByText(/key removed/)).toBeNull();
    expect(api.studioPreview).toHaveBeenCalledWith(expect.objectContaining({ target: "thisComputer", model: klein.id }));
  });

  it("the exact command names the negative prompt's file when there is one", async () => {
    vi.mocked(api.studioPreview).mockResolvedValue({
      method: "run",
      url: "sd-cli",
      host: "",
      body: { argv: ["--negative-prompt-file", "/g/.partial/r/negative.txt"], prompt: "a lighthouse", negativePrompt: "blurry" },
    });
    await renderDrawer(klein, {}, { target: "thisComputer" });
    fireEvent.click(screen.getByRole("button", { name: "See the exact command" }));
    await screen.findByText("Negative prompt (passed in negative.txt): blurry");
  });

  it("a command that could not be worked out says why, never a blank box", async () => {
    vi.mocked(api.studioPreview).mockResolvedValue({
      method: "run",
      url: "sd-cli",
      host: "",
      body: { argv: [], prompt: "a lighthouse", negativePrompt: null, error: "The Studio engine is not installed." },
    });
    await renderDrawer(klein, {}, { target: "thisComputer" });
    fireEvent.click(screen.getByRole("button", { name: "See the exact command" }));
    await screen.findByText("The Studio engine is not installed.");
    expect(screen.queryByText(/Prompt \(passed in/)).toBeNull();
  });

  it("a local clip's field drawn under Advanced shows each chip's time here, as a simple one does", async () => {
    const wanInstalled = LOCAL.installed[1]!;
    const wan: MediaModel = {
      ...wanInstalled.model,
      params: wanInstalled.model.params.map((p) => (p.key === "resolution" ? { ...p, layer: "advanced" as const } : p)),
    };
    const local: LocalModels = {
      ...LOCAL,
      installed: LOCAL.installed.map((m) => (m === wanInstalled ? { ...m, model: wan } : m)),
    };
    vi.mocked(api.studioLocalFit).mockImplementation(async (_source, _w, h, seconds) => localFit(seconds, h));
    await load({ local, prefs: prefs({ lastModel: { video: { target: "thisComputer", model: wan.id } } }) });
    useStudio.getState().showStudio("video");
    await act(async () => {
      render(<SettingsPanel tab="video" />);
    });
    await flush();
    const group = screen.getByRole("group", { name: "Resolution" });
    expect(group.closest("[data-advanced]")).not.toBeNull(); // drawn in the Advanced drawer
    within(group).getByRole("radio", { name: "480p · about 3 min" });
    within(group).getByRole("radio", { name: "720p · about 10 min" });
    within(screen.getByRole("group", { name: "Length" })).getByRole("radio", { name: "2 s · about 3 min" });
  });

  it("the exact request and a setup name being typed belong to their own tab", async () => {
    vi.mocked(api.studioPreview).mockResolvedValue(FLARE_PREVIEW);
    await show(flare);
    let view: RenderResult | null = null;
    await act(async () => {
      view = render(<SettingsPanel tab="image" />);
    });
    const rerender = (tab: StudioTab) =>
      act(async () => {
        view!.rerender(<SettingsPanel tab={tab} />);
      });
    fireEvent.click(screen.getByRole("button", { name: "See the exact request" }));
    await screen.findByText("key removed · goes to openrouter.ai");
    fireEvent.click(screen.getByRole("button", { name: "Save as setup…" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Setup name" }), { target: { value: "Pictures only" } });

    // The same panel shows the Video tab: neither the picture's request nor the name comes along.
    await rerender("video");
    await flush();
    expect(screen.queryByText("key removed · goes to openrouter.ai")).toBeNull();
    expect(screen.getByRole("button", { name: /^See the exact / }).getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByRole("textbox", { name: "Setup name" })).toBeNull();
    screen.getByRole("button", { name: "Save as setup…" });
    expect(api.studioPreview).toHaveBeenCalledTimes(1);

    await rerender("image");
    expect(screen.getByRole("button", { name: "See the exact request" }).getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByRole("textbox", { name: "Setup name" })).toBeNull();
  });

  it("while open, the exact request is asked at once, then again 300 ms after the last change", async () => {
    vi.mocked(api.studioPreview).mockImplementation(async (req) => ({ ...FLARE_PREVIEW, body: { prompt: req.prompt } }));
    await renderDrawer(flare);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      fireEvent.click(screen.getByRole("button", { name: "See the exact request" }));
      await flush();
      expect(api.studioPreview).toHaveBeenCalledTimes(1); // at once on the click
      screen.getByText("key removed · goes to openrouter.ai");
      // Each key of the prompt changes the draft: none is asked while typing goes on.
      for (const text of ["a", "a li", "a lighthouse"]) {
        act(() => useStudio.getState().setPrompt("image", text));
        act(() => vi.advanceTimersByTime(100));
      }
      act(() => vi.advanceTimersByTime(199));
      await flush();
      expect(api.studioPreview).toHaveBeenCalledTimes(1);
      act(() => vi.advanceTimersByTime(1));
      await flush();
      expect(api.studioPreview).toHaveBeenCalledTimes(2);
      expect(api.studioPreview).toHaveBeenLastCalledWith(expect.objectContaining({ prompt: "a lighthouse" }));
      screen.getByText(/"prompt": "a lighthouse"/);
    } finally {
      vi.useRealTimers();
    }
  });

  it("an answer for a draft changed since is dropped, even when it comes last", async () => {
    const answers: ((preview: RequestPreview) => void)[] = [];
    vi.mocked(api.studioPreview).mockImplementation(() => new Promise((resolve) => answers.push(resolve)));
    const answered = (background: string): RequestPreview => ({ ...FLARE_PREVIEW, body: { background } });
    await renderDrawer(flare);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      fireEvent.click(screen.getByRole("button", { name: "See the exact request" }));
      act(() => useStudio.getState().setValue("image", "background", "opaque"));
      act(() => vi.advanceTimersByTime(300));
      expect(answers).toHaveLength(2);
      await act(async () => answers[1]!(answered("opaque")));
      await flush();
      screen.getByText(/"background": "opaque"/);
      // The answer for the draft before the change comes after it: it is not shown.
      await act(async () => answers[0]!(answered("auto")));
      await flush();
      screen.getByText(/"background": "opaque"/);
      expect(screen.queryByText(/"background": "auto"/)).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("a company saved in a setup that the model no longer lists is named in words, its tag kept", async () => {
    await renderDrawer(flare);
    act(() =>
      useStudio.getState().setRouting("image", { order: ["acme-cloud"], only: false, allowFallbacks: true, zdr: false }),
    );
    const company = screen.getByRole("combobox", { name: "Company that runs it" });
    expect(company.textContent).toBe("A company no longer listed");
    press(company);
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual([
      "OpenRouter chooses",
      "OpenAI",
      "A company no longer listed",
    ]);
    // Its tag is still what is sent.
    expect(useStudio.getState().drafts.image.routing?.order).toEqual(["acme-cloud"]);
  });

  it("Allow another company comes back as chosen when a company is picked again", async () => {
    await renderDrawer(flare);
    const routing = () => useStudio.getState().drafts.image.routing;
    choose("Company that runs it", "OpenAI");
    fireEvent.click(screen.getByRole("checkbox", { name: "Allow another company if that one is busy" }));
    expect(routing()).toEqual({ order: ["openai"], only: false, allowFallbacks: false, zdr: false });
    choose("Company that runs it", "OpenRouter chooses");
    expect(routing()).toBeNull();
    choose("Company that runs it", "OpenAI");
    expect(routing()).toEqual({ order: ["openai"], only: false, allowFallbacks: false, zdr: false });
    // Reset all starts it again: another company may.
    fireEvent.click(screen.getByRole("button", { name: "Reset all" }));
    expect(routing()).toBeNull();
    choose("Company that runs it", "OpenAI");
    expect(routing()).toEqual({ order: ["openai"], only: false, allowFallbacks: true, zdr: false });
  });

  it("Extra fields that take no names give no hint, and Save's refusal is announced on the box", async () => {
    const takesNone: MediaModel = {
      ...flare,
      params: flare.params.map((p) =>
        p.kind.type === "extra" ? { ...p, kind: { type: "extra" as const, allowedKeys: [] } } : p,
      ),
    };
    await renderDrawer(takesNone);
    screen.getByText("Extra fields (JSON)");
    expect(screen.queryByText(/This model accepts/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Edit JSON" }));
    const box = screen.getByRole("textbox", { name: "Extra fields (JSON)" });
    expect(box.getAttribute("aria-describedby")).toBeNull();
    fireEvent.change(box, { target: { value: "{nope" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toBe("This is not valid JSON.");
    expect(box.getAttribute("aria-describedby")).toBe(alert.id);
  });

  it("a picture made through OpenAI itself offers no routing", async () => {
    const openai: StudioEndpoint = { ...endpoint([flare]), name: "OpenAI", company: "openai", host: "api.openai.com" };
    await load({ endpoints: [openai], prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: flare.id } } }) });
    await act(async () => {
      render(<AdvancedDrawer tab="image" />);
    });
    expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: flare.id });
    screen.getByText("Background"); // the drawer is open
    expect(screen.queryByText("Company that runs it")).toBeNull();
    expect(screen.queryByText("OpenRouter chooses")).toBeNull();
  });

  it("stays closed as saved, and saves the choice when opened", async () => {
    cleanup();
    const tab = await show(flare, { background: "transparent" }, { prefs: { advancedOpen: false } });
    await act(async () => {
      render(<AdvancedDrawer tab={tab} />);
    });
    expect(screen.queryByText("Background")).toBeNull();
    expect(screen.queryByRole("button", { name: "See the exact request" })).toBeNull();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Advanced · 1 changed" }));
    });
    expect(api.studioSetPrefs).toHaveBeenCalledWith({ advancedOpen: true });
    screen.getByText("Background");
    screen.getByRole("button", { name: "See the exact request" });
  });
});
