// Studio's Advanced drawer (0.1.0 Task B6; design Wireframe 2): every setting the model offers
// one click away with its reset, each change a chip under the prompt (the dock's ChipsRow), the
// seed's dice and lock, an OpenRouter picture's routing, Extra fields (JSON) with the names the
// model accepts, the exact request (on this computer, the exact command), and My setups.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { StudioTab } from "../../store/studio";
import type { Values } from "./canonical";
import { LOCAL, MODELS } from "./fixtures/models";
import type { LocalModels, MediaModel, RequestPreview, RunTarget, Setup, StudioEndpoint, StudioPrefs } from "./types";

// Radix positions menus with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const api = await import("./api");
const { AdvancedDrawer, ChipsRow, chipsFor, parseExtra } = await import("./AdvancedDrawer");
const { SettingsPanel } = await import("./SettingsPanel");
const { useStudio } = await import("../../store/studio");

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

/** The whole right panel of `m`'s tab. */
async function renderPanel(m: MediaModel, opts: { target?: RunTarget; prefs?: Partial<StudioPrefs> } = {}) {
  const tab = await show(m, undefined, opts);
  await act(async () => {
    render(<SettingsPanel tab={tab} />);
  });
  await flush();
}

function draftSeed(): unknown {
  return useStudio.getState().drafts.image.values.seed;
}

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
    screen.getByText("Automatic (company default)");
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
    const allow = screen.getByRole("checkbox", { name: "Allow another company if that one is busy" });
    expect((allow as HTMLInputElement).disabled).toBe(true); // OpenRouter chooses
    fireEvent.change(screen.getByRole("combobox", { name: "Company that runs it" }), { target: { value: "openai" } });
    expect(routing()).toEqual({ order: ["openai"], only: false, allowFallbacks: true, zdr: false });
    fireEvent.click(allow);
    expect(routing()).toEqual({ order: ["openai"], only: false, allowFallbacks: false, zdr: false });
    fireEvent.click(screen.getByRole("checkbox", { name: "Only companies that keep no data" }));
    expect(routing()).toEqual({ order: ["openai"], only: false, allowFallbacks: false, zdr: true });
    fireEvent.change(screen.getByRole("combobox", { name: "Company that runs it" }), { target: { value: "" } });
    expect(routing()).toEqual({ order: [], only: false, allowFallbacks: true, zdr: true });
    fireEvent.click(screen.getByRole("checkbox", { name: "Only companies that keep no data" }));
    expect(routing()).toBeNull();

    await renderDrawer(veo);
    expect(screen.queryByText("Company that runs it")).toBeNull();
    screen.getByText(veo.params.find((p) => p.key === "seed")!.help!); // "… gives a different video."
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
    const menu = screen.getByRole("combobox", { name: "My setups" }) as HTMLSelectElement;
    expect(menu.selectedOptions[0]?.textContent).toBe("Product shot");
    // A change to the draft: the menu no longer names the setup.
    act(() => useStudio.getState().setValue("image", "quality", "low"));
    expect(menu.selectedOptions[0]?.textContent).toBe("None");
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
    expect(within(menu).getAllByRole("option").map((o) => o.textContent)).toEqual(["None", "Local square", "Product shot"]);
    fireEvent.change(menu, { target: { value: "0" } });
    expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: "flux2-klein-4b-q4" });
    expect((menu as HTMLSelectElement).selectedOptions[0]?.textContent).toBe("Local square");
  });

  it("picking a setup that cannot run says why", async () => {
    await renderPanel(flare, { prefs: { setups: [{ ...PRODUCT_SHOT, model: "acme/gone-1" }] } });
    fireEvent.change(screen.getByRole("combobox", { name: "My setups" }), { target: { value: "0" } });
    screen.getByText("OpenRouter no longer offers this setup's model.");
    expect(useStudio.getState().drafts.image.model).toBe(flare.id);
  });
});
