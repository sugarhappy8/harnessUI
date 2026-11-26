// Studio's composer (0.2.0 Task U3, design 10.1): one card at the foot of the results with the
// prompt and the pictures in front of it, then a toolbar with the model chip, Shape and How
// many as menus, Settings, and the time or price beside Create; the privacy line under it.
// Ported deliberately from the prompt dock's tests (0.1.0 Task B7): Create with the price of
// the whole run (cloud) or its time (local), where the prompt goes, Enter to create, one start
// per press, the confirmation a cloud run may need, the daily limit (cloud only), cloud as the
// backup for a very slow local run, what a refused start says, and the prompt's own checks.
// A model that may not fit this computer never greys Create out (Task S11; Task G2 adds the
// warning line).
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { INPUT_ROLE, STUDIO_DOCK, STUDIO_REUSE } from "../../lib/words";
import type { StudioTab } from "../../store/studio";
import { LOCAL, MODELS, RUN } from "./fixtures/models";
import type {
  Estimate,
  InstalledLocal,
  LocalEstimate,
  LocalModels,
  MediaModel,
  ParamSpec,
  RefItem,
  RefRole,
  Run,
  StartRequest,
  StudioEndpoint,
  StudioPrefs,
  Usage,
} from "./types";

// Radix positions popovers with ResizeObserver, and its menus and tooltips use pointer capture
// and scrollIntoView, none of which jsdom has.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.hasPointerCapture ??= () => false;
Element.prototype.setPointerCapture ??= () => {};
Element.prototype.releasePointerCapture ??= () => {};

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const api = await import("./api");
const { StudioView } = await import("./StudioView");
const { UiProviders } = await import("../ui/UiProviders");
const { cloudBackup } = await import("./cost");
const { PROMPT_MAX_CHARS, promptProblems, settingsProblems, useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

/** OpenRouter with the fixture's models, or OpenAI (pictures only). */
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

/** A cloud estimate: $0.16, nothing to confirm, within the limit. */
function est(patch: Partial<Estimate> = {}): Estimate {
  return {
    usd: 0.16,
    reasons: [],
    confirmAtUsd: 1,
    limit: { limitUsd: 10, spentTodayUsd: 0, pendingUsd: 0, wouldPass: true, refusal: null, reserveUsd: 5 },
    invalid: [],
    eta: null,
    local: null,
    ...patch,
  };
}

function localEst(patch: Partial<LocalEstimate> = {}): LocalEstimate {
  return { fit: "fits", unloadChat: false, verySlow: false, over15Min: false, reason: null, ...patch };
}

/** A local estimate: free, about 4 minutes here (measured), fitting. */
function here(local: Partial<LocalEstimate> = {}, patch: Partial<Estimate> = {}): Estimate {
  return est({
    usd: 0,
    eta: { lowMs: 180_000, highMs: 300_000, measured: true, leftMs: null },
    local: localEst(local),
    ...patch,
  });
}

/** The model used last on each tab: GPT Image 2.6 Flare and Veo 3.1 on OpenRouter. */
const CLOUD_LAST = {
  image: { target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" },
  video: { target: { endpoint: 1 }, model: "google/veo-3.1" },
} as const;

/** What `studio_start` gives back: the run, queued. */
function accepted(req: StartRequest): Run {
  return {
    ...RUN,
    id: `run-${req.clientToken}`,
    kind: req.kind,
    runsOn: req.target === "thisComputer" ? "local" : "cloud",
    model: req.model,
    prompt: req.prompt,
    status: "queued",
    outputs: [],
  };
}

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

/** Waits past the estimate's 150 ms pause, and for its answer. */
async function estimated() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 200));
  });
  await flush();
}

/** Opens Studio on `tab` with what the backend has saved; the dock asks for the draft's
 *  estimate (`estimate`), and is waited for unless `settle` is false. */
async function show(
  saved: {
    tab?: StudioTab;
    endpoints?: StudioEndpoint[];
    local?: LocalModels;
    prefs?: StudioPrefs;
    usage?: Usage;
    estimate?: Estimate;
    settle?: boolean;
  } = {},
) {
  const endpoints = saved.endpoints ?? [openrouter];
  const p = saved.prefs ?? prefs();
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints });
  vi.mocked(api.studioLocalModels).mockResolvedValue(saved.local ?? LOCAL);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs: [], next: null });
  vi.mocked(api.studioLive).mockResolvedValue([]);
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
  vi.mocked(api.studioLocalFit).mockResolvedValue(LOCAL.installed[1]!.fit);
  vi.mocked(api.studioEstimate).mockResolvedValue(saved.estimate ?? est());
  vi.mocked(api.studioStart).mockImplementation(async (req) => accepted(req));
  // A model picked at OpenRouter asks for its price once: the fixture's, as listed.
  vi.mocked(api.studioModelPrice).mockImplementation(async (_, id) => MODELS.find((m) => m.id === id)!);
  useStudio.getState().showStudio(saved.tab ?? "image");
  await act(async () => {
    render(
      <UiProviders>
        <StudioView />
      </UiProviders>,
    );
  });
  await flush();
  if (saved.settle ?? true) await estimated();
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
  await flush();
}

function promptBox(): HTMLTextAreaElement {
  return screen.getByRole("textbox", { name: "Prompt" }) as HTMLTextAreaElement;
}

function typePrompt(text: string) {
  fireEvent.change(promptBox(), { target: { value: text } });
}

function createButton(): HTMLButtonElement {
  return screen.getByRole("button", { name: /Create/ }) as HTMLButtonElement;
}

/** The composer's toolbar: the model chip, Shape, How many, Settings and Create. */
function toolbar(): HTMLElement {
  return screen.getByRole("toolbar", { name: "Picture settings" });
}

/** Opens a Radix menu from its trigger, as a pointer press does. */
async function openMenu(trigger: HTMLElement) {
  await act(async () => {
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
  });
}

/** The tooltip `el` shows once it has keyboard focus. */
async function tipOf(el: HTMLElement): Promise<string> {
  await act(async () => {
    el.focus();
  });
  return (await screen.findByRole("tooltip")).textContent ?? "";
}

/** A picture file, as dropped from the file manager. */
function png(name = "a.png"): File {
  return new File([new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10])], name, { type: "image/png" });
}

function dropOn(el: HTMLElement, file: File = png()) {
  fireEvent.drop(el, { dataTransfer: { files: [file], types: ["Files"] } });
}

/** The backend's answer for an added picture: kept, with no role yet. */
function kept(n: number): RefItem {
  return { sha256: String(n).repeat(64).slice(0, 64), mime: "image/png", sizeBytes: 1200, role: null };
}

/** The Pictures draft's added pictures, by role. */
const rolesNow = (tab: StudioTab = "image") => useStudio.getState().drafts[tab].refs.map((r) => r.role);

/** A picture field of a model on this computer, as the backend builds one (`picture_field`). */
function pictureField(key: string, label: string, role: RefRole, min: number): ParamSpec {
  return {
    key,
    canonical: null,
    label,
    help: null,
    layer: "simple",
    kind: { type: "images", min, max: 1, role },
    companyDefault: null,
    initial: null,
    affectsPrice: false,
    requires: null,
  };
}

/** FLUX.2 klein's row as another installed model on this computer, with `pictures` in place of
 *  its reference pictures: an SDXL inpainting model, or Flex.2. */
function installedWith(id: string, name: string, pictures: ParamSpec[]): InstalledLocal {
  const klein = LOCAL.installed[0]!;
  const params = klein.model.params.filter((p) => p.kind.type !== "images");
  return { ...klein, id, name, model: { ...klein.model, id, displayName: name, params: [...params, ...pictures] } };
}
const inpainting = installedWith("sdxl-inpainting", "sd_xl_inpainting_0.1", [
  pictureField("source_picture", "Picture to change", "source", 1),
  pictureField("mask_picture", "Mask", "mask", 1),
]);
const flex2 = installedWith("flex-2", "Flex.2 preview", [
  pictureField("source_picture", "Picture to change", "source", 0),
  pictureField("mask_picture", "Mask", "mask", 0),
  pictureField("control_picture", "Control picture", "control", 0),
]);

/** Studio's local models with `extra` installed too. */
function localWith(...extra: InstalledLocal[]): LocalModels {
  return { ...LOCAL, installed: [...LOCAL.installed, ...extra] };
}

/** The Pictures tab set to installed model `m` on this computer. */
const usedLast = (m: InstalledLocal) => ({ image: { target: "thisComputer" as const, model: m.model.id } });

/** The request each `studio_start` got. */
function started(): StartRequest[] {
  return vi.mocked(api.studioStart).mock.calls.map(([req]) => req);
}

/** Lets the estimate's 150 ms pause run out under fake timers, and its answer come in. */
async function pauseEnds() {
  await act(async () => {
    vi.advanceTimersByTime(150);
  });
  await flush();
}

/** The backend's refusal of a cloud run that needs confirming first. */
const NEEDS_CONFIRMATION = {
  code: "studio_needs_confirmation",
  message: "Confirm this run before it is sent.",
  nextStep: "Check the price, then confirm to send it.",
  details: "",
};

/** An estimate the backend could not work out. */
const ESTIMATE_FAILED = {
  code: "internal",
  message: "Something went wrong.",
  nextStep: "Try again.",
  details: "",
};

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
  vi.useRealTimers();
  window.removeEventListener("app:open-settings", onOpenSettings);
});

describe("Composer", () => {
  it("the composer is one region with the prompt, the model chip and Create", async () => {
    await show({ estimate: here() });
    const region = screen.getByRole("region", { name: "Make a picture" });
    const prompt = within(region).getByRole("textbox", { name: "Prompt" });
    expect(prompt.getAttribute("placeholder")).toBe("Describe the picture you want…");
    const bar = within(region).getByRole("toolbar", { name: "Picture settings" });
    within(bar).getByRole("button", { name: /FLUX\.2 klein 4B/ });
    within(bar).getByRole("button", { name: "Create" });
    // One prompt box in all of Studio, and no old dock.
    expect(screen.getAllByRole("textbox", { name: "Prompt" })).toHaveLength(1);
    expect(document.querySelector("[data-prompt-dock]")).toBeNull();
    // The prompt comes before the toolbar.
    expect(prompt.compareDocumentPosition(bar) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await click(screen.getByRole("tab", { name: "Video" }));
    within(screen.getByRole("region", { name: "Make a video" })).getByRole("textbox", { name: "Prompt" });
    expect(promptBox().getAttribute("placeholder")).toBe("Describe the video you want…");
  });

  it("the model chip names the model and where it runs, and its dot's tooltip is the privacy line", async () => {
    await show({ estimate: here() });
    const chip = within(toolbar()).getByRole("button", { name: /FLUX\.2 klein 4B/ });
    expect(chip.textContent).toContain("This computer");
    const dot = within(toolbar()).getByRole("img", { name: "Where your prompt goes" });
    expect(await tipOf(dot)).toContain("On this computer: your prompt and pictures stay here");
    // The chip opens the model sheet, anchored to it.
    await click(chip);
    const sheet = screen.getByRole("dialog", { name: "Choose a model" });
    await click(within(within(sheet).getByRole("region", { name: "OpenRouter" })).getByRole("button", { name: /GPT Image 2\.6 Flare/ }));
    const cloudChip = within(toolbar()).getByRole("button", { name: /GPT Image 2\.6 Flare/ });
    expect(cloudChip.textContent).toContain("OpenRouter");
    // The sheet closed with the pick, and gave focus back to the chip.
    expect(screen.queryByRole("dialog", { name: "Choose a model" })).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(cloudChip));
    cleanup();
    useStudio.setState(INITIAL, true);
    await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
    const cloudDot = within(toolbar()).getByRole("img", { name: "Where your prompt goes" });
    expect(await tipOf(cloudDot)).toContain("Cloud: your prompt and added pictures go to openrouter.ai");
  });

  it("with no model, the chip asks for one and Create is off", async () => {
    await show({ endpoints: [], local: { ...LOCAL, installed: [] } });
    within(toolbar()).getByRole("button", { name: /Choose a model/ });
    expect(within(toolbar()).queryByRole("img", { name: "Where your prompt goes" })).toBeNull();
    typePrompt("a fox");
    expect(createButton().disabled).toBe(true);
  });

  it("shape and count are menus in the toolbar", async () => {
    await show({ estimate: here() });
    await openMenu(within(toolbar()).getByRole("button", { name: "Shape: 1:1" }));
    expect(screen.getAllByRole("menuitemradio").map((i) => i.textContent)).toEqual(["1:1", "4:3", "3:4", "16:9", "9:16"]);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitemradio", { name: "16:9" }));
    });
    expect(useStudio.getState().drafts.image.values.aspect_ratio).toBe("16:9");
    within(toolbar()).getByRole("button", { name: "Shape: 16:9" });
    await openMenu(within(toolbar()).getByRole("button", { name: "How many: 1" }));
    expect(screen.getAllByRole("menuitemradio").map((i) => i.textContent)).toEqual(["1", "2", "3", "4"]);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitemradio", { name: "2" }));
    });
    expect(useStudio.getState().drafts.image.values.n).toBe(2);
    within(toolbar()).getByRole("button", { name: "How many: 2" });
  });

  it("a cloud model's Shape lists every shape, and Automatic while the company picks", async () => {
    // Moved from the right panel's tests: its five chips and More shapes are one menu here.
    await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
    await openMenu(within(toolbar()).getByRole("button", { name: "Shape: Automatic" }));
    const items = screen.getAllByRole("menuitemradio");
    expect(items).toHaveLength(12);
    expect(items[0]!.textContent).toBe("Automatic (company default)");
    expect(items[0]!.getAttribute("aria-checked")).toBe("true");
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitemradio", { name: "21:9" }));
    });
    expect(useStudio.getState().drafts.image.values.aspect_ratio).toBe("21:9");
    await openMenu(within(toolbar()).getByRole("button", { name: "Shape: 21:9" }));
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitemradio", { name: "Automatic (company default)" }));
    });
    expect(useStudio.getState().drafts.image.values).not.toHaveProperty("aspect_ratio");
    // How many goes up to the model's most.
    await openMenu(within(toolbar()).getByRole("button", { name: "How many: Automatic" }));
    expect(screen.getAllByRole("menuitemradio").map((i) => i.textContent)).toEqual([
      "Automatic (company default)",
      ...Array.from({ length: 10 }, (_, i) => String(i + 1)),
    ]);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitemradio", { name: "7" }));
    });
    expect(useStudio.getState().drafts.image.values.n).toBe(7);
  });

  it("a model without How many has no How many menu", async () => {
    const nano = { image: { target: { endpoint: 1 }, model: "google/gemini-nano-banana-2.1" } } as const;
    await show({ prefs: prefs({ lastModel: nano }) });
    expect(within(toolbar()).queryByRole("button", { name: /^How many/ })).toBeNull();
    within(toolbar()).getByRole("button", { name: /^Shape/ });
  });

  it("the Video tab shows a Length menu and choosing a length sets the value", async () => {
    await show({ tab: "video", estimate: here() });
    // After Shape (a video takes no How many).
    const shape = within(toolbar()).getByRole("button", { name: /^Shape:/ });
    const length = within(toolbar()).getByRole("button", { name: "Length: 2 s" });
    expect(shape.compareDocumentPosition(length) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await openMenu(length);
    expect(screen.getAllByRole("menuitemradio").map((i) => i.textContent)).toEqual(["1 s", "2 s", "3 s", "4 s", "5 s"]);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitemradio", { name: "4 s" }));
    });
    expect(useStudio.getState().drafts.video.values.duration).toBe(4);
    within(toolbar()).getByRole("button", { name: "Length: 4 s" });
  });

  it("a model whose longest is very long offers at most 16 choices, including its default and its longest", async () => {
    // A generic video model whose recipe states no most frames: 1 to 170 s.
    const wan = LOCAL.installed[1]!;
    const lengths = Array.from({ length: 170 }, (_, i) => ({ value: i + 1, label: `${i + 1} s` }));
    const long: InstalledLocal = {
      ...wan,
      id: "generic-video",
      name: "generic video",
      model: {
        ...wan.model,
        id: "generic-video",
        displayName: "generic video",
        params: wan.model.params.map((p) =>
          p.canonical === "length" ? { ...p, initial: 3, kind: { type: "enum", values: lengths } } : p,
        ),
      },
    };
    await show({
      tab: "video",
      local: localWith(long),
      prefs: prefs({ lastModel: { video: { target: "thisComputer", model: "generic-video" } } }),
      estimate: here(),
    });
    await openMenu(within(toolbar()).getByRole("button", { name: "Length: 3 s" }));
    const items = screen.getAllByRole("menuitemradio").map((i) => i.textContent);
    expect(items.length).toBeLessThanOrEqual(16);
    // Every second up to 10 s, then wider steps up to the longest, in order.
    expect(items.slice(0, 10)).toEqual(Array.from({ length: 10 }, (_, i) => `${i + 1} s`));
    expect(items.at(-1)).toBe("170 s");
    const seconds = items.map((i) => Number.parseInt(i ?? "", 10));
    expect(seconds).toEqual([...seconds].sort((a, b) => a - b));
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitemradio", { name: "170 s" }));
    });
    expect(useStudio.getState().drafts.video.values.duration).toBe(170);
    // A length set elsewhere (the side panel lists them all) is listed and named too.
    act(() => useStudio.getState().setValue("video", "duration", 97));
    await openMenu(within(toolbar()).getByRole("button", { name: "Length: 97 s" }));
    const now = screen.getAllByRole("menuitemradio");
    expect(now.length).toBeLessThanOrEqual(16);
    expect(now.find((i) => i.textContent === "97 s")?.getAttribute("aria-checked")).toBe("true");
    expect(now.map((i) => i.textContent)).toEqual(expect.arrayContaining(["3 s", "170 s"]));
  });

  it("the Pictures tab has no Length menu", async () => {
    await show({ estimate: here() });
    within(toolbar()).getByRole("button", { name: /^Shape:/ });
    expect(within(toolbar()).queryByRole("button", { name: /^Length/ })).toBeNull();
  });

  it("a value the menu does not list still names its button", async () => {
    // How many lists at most 16 numbers; a model taking up to 40 can hold 30 (set in the panel).
    const flare = MODELS[0]!;
    const many: MediaModel = {
      ...flare,
      params: flare.params.map((p) => (p.key === "n" && p.kind.type === "range" ? { ...p, kind: { ...p.kind, max: 40 } } : p)),
    };
    await show({
      endpoints: [endpoint({ models: [many, ...MODELS.slice(1)] })],
      prefs: prefs({ lastModel: CLOUD_LAST }),
    });
    act(() => useStudio.getState().setValue("image", "n", 30));
    within(toolbar()).getByRole("button", { name: "How many: 30" });
  });

  it("Create shows the time for a local model and the price for a cloud one", async () => {
    await show({ estimate: here() });
    expect(within(toolbar()).getByText("about 4 min")).toBeTruthy();
    cleanup();
    useStudio.setState(INITIAL, true);
    await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
    expect(within(toolbar()).getByText("~ $0.16")).toBeTruthy();
  });

  it("added pictures show as thumbnails before the prompt, each with a Remove button named after it", async () => {
    await show({ estimate: here() });
    const two = [1, 2].map((n) => ({ ...kept(n), role: "reference" as const }));
    act(() => useStudio.setState((s) => ({ drafts: { ...s.drafts, image: { ...s.drafts.image, refs: two } } })));
    const region = screen.getByRole("region", { name: "Make a picture" });
    const thumbs = within(region).getAllByRole("img", { name: /^Picture \d$/ });
    expect(thumbs.map((t) => t.getAttribute("aria-label") ?? t.getAttribute("alt"))).toEqual(["Picture 1", "Picture 2"]);
    for (const thumb of thumbs) {
      expect(thumb.compareDocumentPosition(promptBox()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    }
    within(region).getByRole("button", { name: "Add picture" });
    await click(within(region).getByRole("button", { name: "Remove picture 1" }));
    expect(useStudio.getState().drafts.image.refs.map((r) => r.sha256)).toEqual([kept(2).sha256]);
    within(region).getByRole("button", { name: "Remove picture 1" });
    expect(within(region).queryByRole("button", { name: "Remove picture 2" })).toBeNull();
  });

  it("an inpainting model shows Picture to change and Mask zones, and Create stays off until both are added", async () => {
    await show({ local: localWith(inpainting), prefs: prefs({ lastModel: usedLast(inpainting) }), estimate: here() });
    const region = screen.getByRole("region", { name: "Make a picture" });
    const source = within(region).getByRole("group", { name: "Picture to change" });
    const mask = within(region).getByRole("group", { name: "Mask" });
    within(source).getByText("required");
    within(mask).getByText("required");
    // No reference pictures: the zones are where pictures go.
    expect(within(region).queryByRole("button", { name: "Add picture" })).toBeNull();
    // The mask's hint is its tooltip.
    expect(await tipOf(within(mask).getByRole("button", { name: "Add the mask" }))).toContain(INPUT_ROLE.hint.mask);
    typePrompt("a red door");
    expect(createButton().disabled).toBe(true);
    screen.getByText("This model needs at least 1 picture to change.");
    vi.mocked(api.studioAddRef).mockResolvedValueOnce(kept(1)).mockResolvedValueOnce(kept(2));
    dropOn(within(source).getByRole("button", { name: "Add the picture to change" }));
    await waitFor(() => expect(rolesNow()).toEqual(["source"]));
    expect(createButton().disabled).toBe(true);
    screen.getByText("This model needs at least 1 mask.");
    dropOn(within(mask).getByRole("button", { name: "Add the mask" }));
    await waitFor(() => expect(rolesNow()).toEqual(["source", "mask"]));
    expect(createButton().disabled).toBe(false);
    expect(screen.queryByText(/This model needs at least/)).toBeNull();
    // A set zone shows its picture with Remove.
    await click(within(mask).getByRole("button", { name: "Remove mask" }));
    expect(rolesNow()).toEqual(["source"]);
    expect(createButton().disabled).toBe(true);
  });

  it("Flex.2's zones are optional, and a mask without a picture to change says so", async () => {
    await show({ local: localWith(flex2), prefs: prefs({ lastModel: usedLast(flex2) }), estimate: here() });
    const region = screen.getByRole("region", { name: "Make a picture" });
    const control = within(region).getByRole("group", { name: "Control picture" });
    expect(within(region).queryByText("required")).toBeNull();
    expect(await tipOf(within(control).getByRole("button", { name: "Add the control picture" }))).toContain(
      INPUT_ROLE.hint.control,
    );
    typePrompt("a red door");
    expect(createButton().disabled).toBe(false);
    vi.mocked(api.studioAddRef).mockResolvedValueOnce(kept(1)).mockResolvedValueOnce(kept(2));
    dropOn(within(within(region).getByRole("group", { name: "Mask" })).getByRole("button", { name: "Add the mask" }));
    await waitFor(() => expect(rolesNow()).toEqual(["mask"]));
    screen.getByText("Add the picture to change before its mask.");
    expect(createButton().disabled).toBe(true);
    dropOn(
      within(within(region).getByRole("group", { name: "Picture to change" })).getByRole("button", {
        name: "Add the picture to change",
      }),
    );
    await waitFor(() => expect(rolesNow()).toEqual(["mask", "source"]));
    expect(screen.queryByText("Add the picture to change before its mask.")).toBeNull();
    expect(createButton().disabled).toBe(false);
  });

  it("a start frame zone appears only when the model takes one, and a new picture replaces the one there", async () => {
    // Moved from the right panel, whose frame drop zone the composer's zones replace.
    await show({ tab: "video", prefs: prefs({ lastModel: CLOUD_LAST }) });
    const zone = within(screen.getByRole("region", { name: "Make a video" })).getByRole("group", { name: "Start frame" });
    vi.mocked(api.studioAddRef).mockResolvedValueOnce(kept(1)).mockResolvedValueOnce(kept(2));
    dropOn(within(zone).getByRole("button", { name: "Add the start frame" }));
    await waitFor(() => expect(useStudio.getState().drafts.video.refs).toEqual([{ ...kept(1), role: "startFrame" }]));
    dropOn(within(zone).getByRole("button", { name: "Start frame: change the picture" }));
    await waitFor(() => expect(useStudio.getState().drafts.video.refs).toEqual([{ ...kept(2), role: "startFrame" }]));
    await click(within(zone).getByRole("button", { name: "Remove start frame" }));
    expect(useStudio.getState().drafts.video.refs).toEqual([]);
    // Grok Imagine Video takes a start frame and no end frame.
    act(() => useStudio.getState().chooseModel("video", { endpoint: 1 }, "x-ai/grok-imagine-video-1.5-lite"));
    await flush();
    screen.getByRole("group", { name: "Start frame" });
    expect(screen.queryByRole("group", { name: "End frame" })).toBeNull();
  });

  it("Settings opens the side panel and moves focus into it", async () => {
    await show({ estimate: here() });
    const settings = within(toolbar()).getByRole("button", { name: "Settings" });
    expect(settings.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
    await click(settings);
    // On a window under 1400 px the panel slides over the results.
    const panel = screen.getByRole("dialog", { name: "Settings" });
    expect(panel.contains(document.activeElement)).toBe(true);
    within(panel).getByText("My setups");
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ settingsOpen: true });
    expect(useStudio.getState().settingsOpen).toBe(true);
    // Esc closes it, and focus goes back to Settings.
    await act(async () => {
      fireEvent.keyDown(panel, { key: "Escape" });
    });
    await flush();
    expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(settings));
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ settingsOpen: false });
  });

  it("the note of what changed after a model switch shows in the composer while the panel is closed", async () => {
    await show({ estimate: here() });
    act(() => useStudio.getState().setValue("image", "n", 3));
    act(() => useStudio.getState().chooseModel("image", { endpoint: 1 }, "google/gemini-nano-banana-2.1"));
    await flush();
    const region = screen.getByRole("region", { name: "Make a picture" });
    const note = within(region).getByRole("status");
    expect(note.textContent).toContain("Nano Banana 2.1 has no how many setting, so it was removed.");
    await click(within(note).getByRole("button", { name: "Put the note away" }));
    expect(useStudio.getState().drafts.image.note).toBeNull();
  });

  it("a model that may not fit keeps Create enabled and shows the warning", async () => {
    await show({ estimate: here({ fit: "notHere", reason: "needs 6 GB of graphics memory" }, { eta: null }) });
    typePrompt("a fox");
    await estimated();
    expect(createButton().disabled).toBe(false);
    const region = screen.getByRole("region", { name: "Make a picture" });
    const warning = within(region).getByText(
      "May not fit this computer: needs 6 GB of graphics memory. It may fail or be very slow.",
    );
    expect(warning.closest("[role=status]")).not.toBeNull();
    // Create is described by it.
    expect(createButton().getAttribute("aria-describedby")).toContain(warning.closest("[role=status]")!.id);
  });

  it("a model that fits shows no warning", async () => {
    await show({ estimate: here() });
    typePrompt("a fox");
    await estimated();
    expect(screen.queryByText(/^May not fit this computer/)).toBeNull();
    expect(screen.queryByText("Video on the processor is very slow")).toBeNull();
  });

  it("video on the processor shows its warning", async () => {
    const wan = LOCAL.installed.find((m) => m.kind === "video")!;
    const slow: LocalModels = {
      ...LOCAL,
      videoOnProcessor: true,
      installed: LOCAL.installed.map((m) => (m.id === wan.id ? { ...m, fit: { ...m.fit, videoOnProcessor: true } } : m)),
    };
    await show({ tab: "video", local: slow, estimate: here({ verySlow: true }) });
    act(() => useStudio.getState().chooseModel("video", "thisComputer", wan.model.id));
    typePrompt("a fox");
    await estimated();
    expect(createButton().disabled).toBe(false);
    within(screen.getByRole("region", { name: "Make a video" })).getByText("Video on the processor is very slow");
  });

  it("a model that may not fit this computer never greys Create out", async () => {
    await show({ estimate: here({ fit: "notHere", reason: "needs 6 GB of graphics memory" }, { eta: null }) });
    typePrompt("a fox");
    await estimated();
    expect(createButton().disabled).toBe(false);
    expect(screen.queryByText(/Not for this computer/)).toBeNull();
    expect(screen.queryByText("A cloud model can make this instead.")).toBeNull();
    await click(createButton());
    expect(api.studioStart).toHaveBeenCalledTimes(1);
  });

  it("a model that needs setting up holds Create back, says why, and its control opens Files", async () => {
    const mystery = LOCAL.installed.find((m) => m.needsSetup)!;
    await show({ estimate: here() });
    typePrompt("a fox");
    await estimated();
    expect(createButton().disabled).toBe(false);
    vi.mocked(api.studioEstimate).mockClear();
    // Picked in the model sheet (Task U4): the draft goes to it.
    await act(async () => {
      useStudio.getState().chooseModel("image", "thisComputer", mystery.model.id);
    });
    await estimated();
    expect(useStudio.getState().drafts.image.model).toBe(mystery.model.id);
    expect(createButton().disabled).toBe(true);
    // Its own reason, read out with Create; nothing is asked of the backend for it.
    const why = screen.getByText("Choose this model's type and files first.");
    expect(createButton().getAttribute("aria-describedby")).toContain(why.parentElement!.id);
    expect(api.studioEstimate).not.toHaveBeenCalled();
    fireEvent.keyDown(promptBox(), { key: "Enter" });
    await flush();
    expect(api.studioStart).not.toHaveBeenCalled();
    await click(screen.getByRole("button", { name: "Set it up yourself" }));
    expect(useStudio.getState()).toMatchObject({ settingsOpen: true, settingsSection: "files" });
  });

  it("a model that may not fit keeps Create on, and is not held back as needing set-up", async () => {
    const sdxl = LOCAL.installed.find((m) => m.version === "sdxl" && !m.untested)!;
    const tight: InstalledLocal = {
      ...sdxl,
      fit: { ...sdxl.fit, word: "notHere", verdict: "red", reason: "needs 9 GB of graphics memory" },
    };
    await show({
      local: { ...LOCAL, installed: LOCAL.installed.map((m) => (m === sdxl ? tight : m)) },
      prefs: prefs({ lastModel: { image: { target: "thisComputer", model: sdxl.model.id } } }),
      estimate: here({ fit: "notHere", reason: "needs 9 GB of graphics memory" }, { eta: null }),
    });
    expect(useStudio.getState().drafts.image.model).toBe(sdxl.model.id);
    typePrompt("a fox");
    await estimated();
    expect(createButton().disabled).toBe(false);
    expect(screen.queryByRole("button", { name: "Set it up yourself" })).toBeNull();
    await click(createButton());
    expect(api.studioStart).toHaveBeenCalledTimes(1);
  });

  describe("a cloud model", () => {
    it("Create shows the price for the whole run", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      createButton();
      screen.getByText("~ $0.16");
      // Asked for the draft as Create would send it: nothing is sent anywhere.
      expect(api.studioEstimate).toHaveBeenLastCalledWith(
        expect.objectContaining({ target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare", kind: "image" }),
      );
    });

    it("Create waits while extra settings are held for the model's price, and says so", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      typePrompt("a fox");
      await flush();
      act(() => useStudio.setState({ holding: { image: true, video: false } }));
      expect(createButton().disabled).toBe(true);
      screen.getByText(STUDIO_DOCK.holdingExtra(false));
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      await act(async () => {
        await useStudio.getState().start("image", false);
      });
      expect(api.studioStart).not.toHaveBeenCalled();
      act(() => useStudio.setState({ holding: { image: false, video: false } }));
      expect(createButton().disabled).toBe(false);
    });

    it("Create waits while a field shows a size that cannot be sent, and says why", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      typePrompt("a fox");
      await flush();
      expect(createButton().disabled).toBe(false);
      act(() => useStudio.getState().setInvalid("image", "size", "Both sides must be multiples of 16."));
      expect(createButton().disabled).toBe(true);
      expect(screen.getByText("Both sides must be multiples of 16.").className).toContain("text-danger");
      act(() => useStudio.getState().setInvalid("image", "size", null));
      expect(createButton().disabled).toBe(false);
      expect(screen.queryByText("Both sides must be multiples of 16.")).toBeNull();
    });

    it("colours a price of $1 or more as a second cue", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }), estimate: est({ usd: 2.6 }) });
      expect(screen.getByText("~ $2.60").className).toContain("text-danger");
    });

    it("names where the prompt goes", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      screen.getByText("Cloud: your prompt and added pictures go to openrouter.ai");
    });

    it("asks again 150 ms after the draft stops changing", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      vi.mocked(api.studioEstimate).mockClear();
      typePrompt("f");
      typePrompt("fo");
      typePrompt("fox");
      await flush();
      expect(api.studioEstimate).not.toHaveBeenCalled();
      await estimated();
      expect(api.studioEstimate).toHaveBeenCalledTimes(1);
      expect(api.studioEstimate).toHaveBeenCalledWith(expect.objectContaining({ prompt: "fox" }));
    });

    it("Enter creates and Shift+Enter adds a line", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      typePrompt("a");
      fireEvent.keyDown(promptBox(), { key: "Enter", shiftKey: true });
      expect(api.studioStart).not.toHaveBeenCalled();
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      expect(api.studioStart).toHaveBeenCalledWith(
        expect.objectContaining({ prompt: "a", target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" }),
        false,
      );
      await flush();
    });

    it("holding Enter down starts one run, not one per key repeat", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      typePrompt("a lighthouse");
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      await flush();
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      // The start was answered, and the prompt is still there; the key is still down.
      for (let i = 0; i < 3; i++) {
        const repeat = fireEvent.keyDown(promptBox(), { key: "Enter", repeat: true });
        // No line is added either.
        expect(repeat).toBe(false);
        await flush();
      }
      expect(api.studioStart).toHaveBeenCalledTimes(1);
    });

    it("Create is disabled while a start is on its way", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      vi.mocked(api.studioStart).mockReturnValue(new Promise<Run>(() => undefined));
      typePrompt("a lighthouse");
      await click(createButton());
      expect(createButton().disabled).toBe(true);
      await click(createButton());
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      await flush();
      expect(api.studioStart).toHaveBeenCalledTimes(1);
    });

    it("each press is its own run: a new client token every time", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      typePrompt("a lighthouse");
      await click(createButton());
      await click(createButton());
      const [first, second] = started();
      expect(first?.clientToken).toBeTruthy();
      expect(second?.clientToken).toBeTruthy();
      expect(first?.clientToken).not.toBe(second?.clientToken);
    });

    it("the daily limit replaces Create", async () => {
      await show({
        prefs: prefs({ lastModel: CLOUD_LAST }),
        estimate: est({ limit: { limitUsd: 10, spentTodayUsd: 9.5, pendingUsd: 0, wouldPass: false, refusal: "over", reserveUsd: 5 } }),
      });
      screen.getByText("Daily limit reached: $9.50 of $10 spent today");
      await click(screen.getByRole("button", { name: "Raise the limit" }));
      expect(opened).toEqual(["studio"]); // app:open-settings detail
      expect(screen.queryByRole("button", { name: /Create/ })).toBeNull();
    });

    it("a run whose price is unknown, refused for the reserve, says so in Create's place", async () => {
      await show({
        prefs: prefs({ lastModel: CLOUD_LAST }),
        estimate: est({
          usd: null,
          limit: {
            limitUsd: 3,
            spentTodayUsd: 0,
            pendingUsd: 0,
            wouldPass: false,
            refusal: "unknownNeedsReserve",
            reserveUsd: 5,
          },
        }),
      });
      screen.getByText("This model's price is unknown, so Studio needs $5 of today's limit left ($3.00 left).");
      expect(screen.queryByText(/Daily limit reached/)).toBeNull();
      screen.getByRole("button", { name: "Raise the limit" });
    });

    it("Enter starts nothing while the daily limit has taken Create's place", async () => {
      await show({
        prefs: prefs({ lastModel: CLOUD_LAST }),
        estimate: est({ limit: { limitUsd: 10, spentTodayUsd: 9.5, pendingUsd: 0, wouldPass: false, refusal: "over", reserveUsd: 5 } }),
      });
      typePrompt("a lighthouse");
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      await flush();
      expect(api.studioStart).not.toHaveBeenCalled();
      screen.getByText("Daily limit reached: $9.50 of $10 spent today");
    });

    it("a refused start says why under the prompt, and Create works again", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      vi.mocked(api.studioStart).mockRejectedValueOnce({
        code: "studio_wont_make",
        message: "Studio won't make this.",
        nextStep: "Change the prompt.",
        details: "",
      });
      typePrompt("something it refuses");
      await click(createButton());
      screen.getByText("Studio won't make this.");
      screen.getByText("Change the prompt.");
      expect(createButton().disabled).toBe(false);
    });

    it("the model used last is saved when a run starts, and only when it changed", async () => {
      await show({ estimate: here() }); // the default: the local model
      typePrompt("a fox");
      await click(createButton());
      expect(api.studioSetPrefs).toHaveBeenCalledTimes(1);
      expect(api.studioSetPrefs).toHaveBeenCalledWith({
        lastModel: { image: { target: "thisComputer", model: "flux2-klein-4b-q4" } },
      });
      await click(createButton());
      expect(api.studioStart).toHaveBeenCalledTimes(2);
      expect(api.studioSetPrefs).toHaveBeenCalledTimes(1);
    });

    it("today's spending follows a cloud run: read again when it is accepted and when it ends", async () => {
      // A wide top bar shows the spend itself (jsdom has no layout, so say how wide it is).
      vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
        const width = this.tagName === "HEADER" ? 1000 : 0;
        return { width, height: 0, top: 0, left: 0, right: width, bottom: 0, x: 0, y: 0, toJSON: () => ({}) };
      });
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      expect(api.studioUsage).toHaveBeenCalledTimes(1);
      vi.mocked(api.studioUsage).mockResolvedValue(usage({ spentTodayUsd: 0, pendingUsd: 0.16 }));
      typePrompt("a lighthouse");
      await click(createButton());
      expect(api.studioUsage).toHaveBeenCalledTimes(2);
      const run = accepted(started()[0]!);
      vi.mocked(api.studioUsage).mockResolvedValue(usage({ spentTodayUsd: 0.16 }));
      act(() => useStudio.getState().applyProgress({ ...run, status: "running", seq: run.seq + 1 }));
      await flush();
      expect(api.studioUsage).toHaveBeenCalledTimes(2);
      act(() => useStudio.getState().applyProgress({ ...run, status: "done", costUsd: 0.16, seq: run.seq + 2 }));
      await flush();
      expect(api.studioUsage).toHaveBeenCalledTimes(3);
      screen.getByText("$0.16 today");
      vi.restoreAllMocks();
    });

    it("an empty prompt says so quietly until a press is refused for it, then aloud", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      const sentence = () => screen.getByText("Write a prompt first.").parentElement!;
      // Nothing was done yet: not a fault, and not shown (critique §6: it stood under every
      // empty prompt); the prompt box still says it to a screen reader.
      expect(sentence().className).toContain("sr-only");
      expect(sentence().className).not.toContain("text-danger");
      expect(sentence().getAttribute("role")).toBeNull();
      // The prompt box is described by it.
      expect(promptBox().getAttribute("aria-describedby")).toBe(sentence().id);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).not.toHaveBeenCalled();
      expect(sentence().className).toContain("text-danger");
      expect(sentence().className).not.toContain("sr-only");
      expect(sentence().getAttribute("role")).toBe("alert");
      expect(promptBox().getAttribute("aria-describedby")).toBe(sentence().id);
    });

    it("a prompt typed and emptied again says so as a fault", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      typePrompt("a");
      expect(screen.queryByText("Write a prompt first.")).toBeNull();
      expect(promptBox().getAttribute("aria-describedby")).toBeNull();
      typePrompt("");
      expect(screen.getByText("Write a prompt first.").parentElement!.className).toContain("text-danger");
    });

    it("a prompt over 32,000 characters, or of white space only, says so before Create", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      // Judged on the prompt as it is, before any answer for it.
      vi.mocked(api.studioEstimate).mockClear();
      typePrompt(" \n\u00A0\u0085\u2003");
      screen.getByText("Write a prompt first.");
      expect(createButton().disabled).toBe(true);
      typePrompt("x".repeat(32_001));
      screen.getByText("Keep the prompt under 32,000 characters.");
      expect(createButton().disabled).toBe(true);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).not.toHaveBeenCalled();
      typePrompt("x".repeat(32_000));
      expect(screen.queryByText(/^Keep the prompt under/)).toBeNull();
      expect(createButton().disabled).toBe(false);
      expect(api.studioEstimate).not.toHaveBeenCalled();
    });

    it.each([
      ["fails", () => vi.mocked(api.studioEstimate).mockRejectedValue(ESTIMATE_FAILED)],
      ["never answers", () => vi.mocked(api.studioEstimate).mockReturnValue(new Promise<Estimate>(() => undefined))],
    ])("an estimate that %s does not keep Create greyed out", async (_, answer) => {
      // The answer for the empty prompt says to write one.
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }), estimate: est({ invalid: ["Write a prompt first."] }) });
      expect(createButton().disabled).toBe(true);
      answer();
      vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
      typePrompt("a lighthouse");
      expect(createButton().disabled).toBe(false);
      await pauseEnds();
      expect(api.studioEstimate).toHaveBeenLastCalledWith(expect.objectContaining({ prompt: "a lighthouse" }));
      expect(screen.queryByText("Write a prompt first.")).toBeNull();
      expect(createButton().disabled).toBe(false);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      expect(api.studioStart).toHaveBeenCalledWith(expect.objectContaining({ prompt: "a lighthouse" }), false);
      await flush();
    });

    it("a setting the model refuses keeps Create greyed out while the answer shown says so", async () => {
      await show({
        prefs: prefs({ lastModel: CLOUD_LAST }),
        estimate: est({ invalid: ["This model takes at most 8 reference pictures."] }),
      });
      vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
      typePrompt("a lighthouse");
      // Typing changes the prompt, not the settings: the answer for this model and place holds.
      screen.getByText("This model takes at most 8 reference pictures.");
      expect(createButton().disabled).toBe(true);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      await pauseEnds();
      expect(createButton().disabled).toBe(true);
      expect(api.studioStart).not.toHaveBeenCalled();
    });
  });

  describe("the confirmation", () => {
    it("asks before a cloud video even when the backend is the first to say so", async () => {
      await show({ tab: "video", prefs: prefs({ lastModel: CLOUD_LAST }), settle: false });
      vi.mocked(api.studioStart).mockRejectedValueOnce({
        code: "studio_needs_confirmation",
        message: "Confirm this run before it is sent.",
        nextStep: "Check the price, then confirm to send it.",
        details: "It asks because it is a cloud video.",
      });
      vi.mocked(api.studioEstimate).mockResolvedValue(est({ usd: 0.09, reasons: ["video"], confirmAtUsd: 1 }));
      typePrompt("waves");
      // Before the 150 ms pause: the store has no estimate for this draft.
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(useStudio.getState().estimates.video).toBeNull();
      await flush();
      screen.getByText("Make this video?");
      screen.getByText("Don't ask again for videos under $1.00");
      const [req] = started();
      expect(api.studioEstimate).toHaveBeenCalledWith(req);
    });

    it("a local run never opens the confirm dialog, even for a video", async () => {
      await show({ tab: "video", estimate: here() });
      expect(useStudio.getState().drafts.video.target).toBe("thisComputer");
      typePrompt("waves");
      await click(createButton());
      expect(api.studioStart).toHaveBeenCalledWith(expect.objectContaining({ target: "thisComputer" }), false);
      expect(screen.queryByText("Make this video?")).toBeNull();
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });

    it("a local run never opens it, even when the estimate for the draft as it is names a reason", async () => {
      await show({ tab: "video", estimate: here({}, { reasons: ["video"] }) });
      typePrompt("waves");
      // The estimate for this very draft: the one a cloud draft's press would go by.
      await estimated();
      expect(useStudio.getState().estimates.video?.estimate?.reasons).toEqual(["video"]);
      await click(createButton());
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      expect(api.studioStart).toHaveBeenCalledWith(
        expect.objectContaining({ target: "thisComputer", prompt: "waves" }),
        false,
      );
      expect(useStudio.getState().pendingEstimate).toBeNull();
      expect(screen.queryByText("Make this video?")).toBeNull();
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });
  });

  describe("a local model", () => {
    it("a local model's Create shows the time, not a price", async () => {
      await show({ endpoints: [], estimate: here() });
      createButton();
      screen.getByText("about 4 min");
      expect(screen.queryByText(/\$/)).toBeNull();
    });

    it("the privacy line under the composer says the prompt stays here, and when the chat model will be unloaded", async () => {
      await show({ estimate: here({ fit: "afterUnload", unloadChat: true }) });
      screen.getByText(
        "On this computer: your prompt and pictures stay here · the chat model will be unloaded to make room",
      );
      expect(document.querySelector("[data-cloud-mark]")).toBeNull();
    });

    it("a run over 15 minutes says so in amber, without a dialog", async () => {
      await show({
        estimate: here(
          { over15Min: true },
          { eta: { lowMs: 1_380_000, highMs: 1_620_000, measured: true, leftMs: null } },
        ),
      });
      const line = screen.getByText(
        "This will take about 25 min and keep the graphics card busy. On a MacBook it will get warm.",
      );
      expect(line.className).toContain("text-warning");
      typePrompt("a fox in deep snow");
      await click(createButton());
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });

    it("when a local run would be very slow, Studio offers cloud as the backup", async () => {
      const slow = here({ fit: "slow", verySlow: true });
      await show({ estimate: slow });
      expect(cloudBackup("image", slow, [openrouter])).toBe("useCloud");
      screen.getByText("Too slow here? A cloud model makes this in minutes, for a price you see first.");
      await click(screen.getByRole("button", { name: "Choose a cloud model" }));
      // The model sheet opens on Cloud; the draft stays where it was.
      const sheet = screen.getByRole("dialog", { name: "Choose a model" });
      expect(within(sheet).getByRole("button", { name: "Cloud" }).getAttribute("aria-pressed")).toBe("true");
      within(within(sheet).getByRole("region", { name: "OpenRouter" })).getByRole("button", { name: /GPT Image 2\.6 Flare/ });
      expect(within(sheet).queryByRole("region", { name: "On this computer" })).toBeNull();
      expect(useStudio.getState().drafts.image.target).toBe("thisComputer");
      expect(useStudio.getState()).toMatchObject({ sheetOpen: true, sheetFilter: "cloud" });
    });

    it("an endpoint whose lists are not saved yet is the backup, and the model sheet gets its list", async () => {
      const unsaved = endpoint({ fetchedAt: null, models: [] });
      const slow = here({ fit: "slow", verySlow: true });
      await show({ endpoints: [unsaved], estimate: slow });
      expect(cloudBackup("image", slow, [unsaved])).toBe("useCloud");
      vi.mocked(api.studioRefreshModels).mockResolvedValue({ endpoints: [openrouter] });
      await click(screen.getByRole("button", { name: "Choose a cloud model" }));
      // The sheet has a row for it, on Cloud: it has no model to show yet.
      const getList = screen.getByRole("button", { name: "OpenRouter: Get the list" });
      await click(getList);
      expect(api.studioRefreshModels).toHaveBeenCalledTimes(1);
      expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
      within(screen.getByRole("region", { name: "OpenRouter" })).getByRole("button", { name: /GPT Image 2\.6 Flare/ });
      expect(screen.queryByRole("button", { name: "OpenRouter: Get the list" })).toBeNull();
      expect(useStudio.getState().drafts.image.target).toBe("thisComputer");
    });

    it("without an endpoint that makes this kind, the backup is Settings → Endpoints", async () => {
      const slow = here({ fit: "slow", verySlow: true });
      await show({ endpoints: [], estimate: slow });
      expect(cloudBackup("image", slow, [])).toBe("addEndpoint");
      screen.getByText("Add OpenRouter or OpenAI in Settings → Endpoints to use a cloud model instead.");
      await click(screen.getByRole("button", { name: "Open Endpoints" }));
      expect(opened).toEqual(["endpoints"]);
      expect(screen.queryByRole("button", { name: "Choose a cloud model" })).toBeNull();
    });

    it("on the Video tab, an endpoint that is only OpenAI never counts", async () => {
      const slow = here({ fit: "slow", verySlow: true });
      const openai = endpoint({ company: "openai" });
      await show({ tab: "video", endpoints: [openai], estimate: slow });
      expect(cloudBackup("video", slow, [openai])).toBe("addEndpoint");
      screen.getByText("Add OpenRouter in Settings → Endpoints to use a cloud model instead.");
      screen.getByRole("button", { name: "Open Endpoints" });
      expect(screen.queryByText(/OpenAI/)).toBeNull();
    });

    it("a local model that fits once the chat model is unloaded can be created", async () => {
      await show({ estimate: here({ fit: "afterUnload", unloadChat: true }) });
      typePrompt("a fox");
      expect(createButton().disabled).toBe(false);
      expect(screen.getByText(/^On this computer: your prompt/).textContent).toMatch(
        / · the chat model will be unloaded to make room$/,
      );
      await click(createButton());
      expect(api.studioStart).toHaveBeenCalledTimes(1);
    });

    it("a local prompt over 4,000 characters says so before Create", async () => {
      const tooLong = "Keep the prompt under 4,000 characters for local models.";
      await show({ estimate: here() });
      vi.mocked(api.studioEstimate).mockResolvedValue(here({}, { invalid: [tooLong] }));
      typePrompt("x".repeat(4_001));
      // Said at once, on the prompt as it is.
      screen.getByText(tooLong);
      expect(createButton().disabled).toBe(true);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).not.toHaveBeenCalled();
      // The answer says the same; it is said once.
      await estimated();
      expect(screen.getAllByText(tooLong)).toHaveLength(1);
      // Counted as the backend counts: 4,000 characters, however many UTF-16 units they take.
      // The answer shown, for the longer prompt, no longer holds Create back.
      typePrompt("🦊".repeat(4_000));
      expect(screen.queryByText(tooLong)).toBeNull();
      expect(createButton().disabled).toBe(false);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      await flush();
    });

    it("the daily limit never replaces Create for a local model", async () => {
      await show({
        estimate: here({}, { limit: { limitUsd: 10, spentTodayUsd: 10, pendingUsd: 0, wouldPass: false, refusal: "over", reserveUsd: 5 } }),
      });
      typePrompt("a fox");
      expect(createButton().disabled).toBe(false);
      expect(screen.queryByText(/Daily limit reached/)).toBeNull();
    });

    it("typing a prompt and pressing Enter at once starts the run", async () => {
      // The answer for the empty prompt says to write one.
      await show({ estimate: here({}, { invalid: ["Write a prompt first."] }) });
      screen.getByText("Write a prompt first.");
      expect(createButton().disabled).toBe(true);
      vi.mocked(api.studioEstimate).mockClear();
      vi.mocked(api.studioEstimate).mockResolvedValue(here());
      vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
      typePrompt("a fox");
      // The prompt is judged as it is: no waiting for the answer for it.
      expect(screen.queryByText("Write a prompt first.")).toBeNull();
      expect(createButton().disabled).toBe(false);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      expect(api.studioStart).toHaveBeenCalledWith(expect.objectContaining({ prompt: "a fox" }), false);
      expect(api.studioEstimate).not.toHaveBeenCalled();
      await pauseEnds();
    });

    it("Create does not wait for the estimate", async () => {
      await show({ estimate: here() });
      // The answer for the new prompt never comes: an answer that greys nothing out is no reason
      // to wait.
      vi.mocked(api.studioEstimate).mockReturnValue(new Promise<Estimate>(() => undefined));
      typePrompt("a fox");
      expect(createButton().disabled).toBe(false);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      expect(api.studioStart).toHaveBeenCalledWith(expect.objectContaining({ prompt: "a fox" }), false);
      await flush();
    });
  });

  describe("the seed after a press", () => {
    /** The Pictures draft's seed (the local model's seed field). */
    const seedOf = () => useStudio.getState().drafts.image.values.seed;

    it("a seed from the dice is used once: after the press it is Random again", async () => {
      await show({ estimate: here() });
      typePrompt("a fox");
      act(() => useStudio.getState().newSeed("image"));
      const dice = seedOf();
      expect(typeof dice).toBe("number");
      await click(createButton());
      expect(started()[0]?.params).toMatchObject({ seed: dice });
      expect(seedOf()).toBeUndefined();
      await click(createButton());
      expect(started()[1]?.params).not.toHaveProperty("seed");
    });

    it("a refused press keeps the seed", async () => {
      await show({ estimate: here() });
      vi.mocked(api.studioStart).mockRejectedValueOnce({
        code: "studio_wont_make",
        message: "Studio won't make this.",
        nextStep: "Change the prompt.",
        details: "",
      });
      typePrompt("a fox");
      act(() => useStudio.getState().newSeed("image"));
      const dice = seedOf();
      await click(createButton());
      expect(seedOf()).toBe(dice);
    });

    it("a kept seed stays for the next press", async () => {
      await show({ estimate: here() });
      typePrompt("a fox");
      act(() => {
        useStudio.getState().newSeed("image");
        useStudio.getState().lockSeed("image", true);
      });
      const dice = seedOf();
      await click(createButton());
      expect(seedOf()).toBe(dice);
      await click(createButton());
      expect(started().map((req) => req.params.seed)).toEqual([dice, dice]);
    });

    /** The Video draft's seed (Veo 3.1's seed field). */
    const videoSeed = () => useStudio.getState().drafts.video.values.seed;
    /** The seed and the confirmation each `studio_start` was sent with. */
    const sent = () => vi.mocked(api.studioStart).mock.calls.map(([req, confirmed]) => [req.params.seed, confirmed]);

    it("a press confirmed from its estimate uses up a dice seed, and a kept one stays", async () => {
      await show({
        tab: "video",
        prefs: prefs({ lastModel: CLOUD_LAST }),
        estimate: est({ usd: 0.09, reasons: ["video"] }),
      });
      typePrompt("waves");
      act(() => useStudio.getState().newSeed("video"));
      const dice = videoSeed();
      expect(typeof dice).toBe("number");
      // The estimate for the draft as it is asks first: the confirmation opens from it.
      await estimated();
      await click(createButton());
      expect(api.studioStart).not.toHaveBeenCalled();
      await click(screen.getByRole("button", { name: "Make video" }));
      expect(sent()).toEqual([[dice, true]]);
      expect(videoSeed()).toBeUndefined();

      act(() => {
        useStudio.getState().newSeed("video");
        useStudio.getState().lockSeed("video", true);
      });
      const kept = videoSeed();
      await estimated();
      await click(createButton());
      await click(screen.getByRole("button", { name: "Make video" }));
      expect(sent()).toEqual([
        [dice, true],
        [kept, true],
      ]);
      expect(videoSeed()).toBe(kept);
    });

    it("a press the backend asks to confirm uses up a dice seed once confirmed, and a kept one stays", async () => {
      await show({ tab: "video", prefs: prefs({ lastModel: CLOUD_LAST }) });
      // The draft's estimate asks for nothing: only the backend asks, and the estimate for that
      // very press fills the confirmation.
      vi.mocked(api.studioEstimate).mockImplementation(async (req) =>
        req.clientToken === "estimate" ? est({ usd: 0.09 }) : est({ usd: 0.09, reasons: ["video"] }),
      );
      typePrompt("waves");
      act(() => useStudio.getState().newSeed("video"));
      const dice = videoSeed();
      vi.mocked(api.studioStart).mockRejectedValueOnce(NEEDS_CONFIRMATION);
      await click(createButton());
      screen.getByText("Make this video?");
      expect(videoSeed()).toBe(dice);
      await click(screen.getByRole("button", { name: "Make video" }));
      expect(sent()).toEqual([
        [dice, false],
        [dice, true],
      ]);
      expect(videoSeed()).toBeUndefined();

      act(() => {
        useStudio.getState().newSeed("video");
        useStudio.getState().lockSeed("video", true);
      });
      const kept = videoSeed();
      vi.mocked(api.studioStart).mockRejectedValueOnce(NEEDS_CONFIRMATION);
      await click(createButton());
      screen.getByText("Make this video?");
      await click(screen.getByRole("button", { name: "Make video" }));
      expect(sent().slice(2)).toEqual([
        [kept, false],
        [kept, true],
      ]);
      expect(videoSeed()).toBe(kept);
    });
  });

  it("Edit prompt elsewhere in Studio puts the cursor in the prompt", async () => {
    await show({ estimate: here() });
    await act(async () => {
      window.dispatchEvent(new Event("app:focus-studio-prompt"));
    });
    expect(document.activeElement).toBe(promptBox());
  });

  describe("pressing again from a run", () => {
    const veoRun: Run = {
      ...RUN,
      id: "veo-run",
      kind: "video",
      model: "google/veo-3.1",
      modelLabel: "Veo 3.1",
      prompt: "slow drone push over the lighthouse",
      params: { duration: 8, resolution: "720p", seed: 81723 },
      outputs: [],
    };

    it("sends the run's own settings with a new seed, and leaves the draft alone", async () => {
      await show({ estimate: here() });
      typePrompt("my own draft");
      await act(async () => {
        await useStudio.getState().start("video", false, veoRun);
      });
      const [req] = started();
      expect(req).toMatchObject({
        target: { endpoint: 1 },
        kind: "video",
        model: "google/veo-3.1",
        prompt: "slow drone push over the lighthouse",
        params: { duration: 8, resolution: "720p" },
      });
      expect(req?.params).not.toHaveProperty("seed");
      expect(useStudio.getState().drafts.image.prompt).toBe("my own draft");
    });

    it("drops the seed of a run whose model is no longer listed", async () => {
      await show({ estimate: here() });
      const gone: Run = { ...veoRun, id: "gone-run", model: "google/veo-0-gone", modelLabel: "Veo 0" };
      const goneHere: Run = {
        ...veoRun,
        id: "gone-here",
        runsOn: "local",
        providerId: null,
        model: "removed-local-model",
        params: { width: 832, height: 480, seed: 4242 },
      };
      await act(async () => {
        await useStudio.getState().start("video", false, gone);
      });
      await act(async () => {
        await useStudio.getState().start("video", false, goneHere);
      });
      const [cloud, local] = started();
      expect(cloud).toMatchObject({ target: { endpoint: 1 }, model: "google/veo-0-gone", params: { duration: 8 } });
      expect(cloud?.params).not.toHaveProperty("seed");
      expect(local).toMatchObject({ target: "thisComputer", model: "removed-local-model", params: { width: 832 } });
      expect(local?.params).not.toHaveProperty("seed");
    });

    it("sends a new seed even while the draft keeps its seed (Keep it is for Create only)", async () => {
      await show({ estimate: here() });
      act(() => useStudio.getState().lockSeed("video", true));
      await act(async () => {
        await useStudio.getState().start("video", false, veoRun);
      });
      expect(started()[0]?.params).toMatchObject({ duration: 8 });
      expect(started()[0]?.params).not.toHaveProperty("seed");
    });

    it("a run whose endpoint was removed is refused with why, under the prompt", async () => {
      await show({ estimate: here() });
      act(() => useStudio.getState().setTab("video"));
      const orphan: Run = { ...veoRun, id: "orphan", providerId: null };
      let answer: Run | null = veoRun;
      await act(async () => {
        answer = await useStudio.getState().start("video", false, orphan);
      });
      expect(answer).toBeNull();
      expect(api.studioStart).not.toHaveBeenCalled();
      expect(useStudio.getState().refusal.video?.message).toBe(STUDIO_REUSE.endpointGone);
      expect(useStudio.getState().starting).toBe(false);
    });

    it("the confirmation shows the run's settings, not the draft's", async () => {
      await show({ estimate: here() }); // the Pictures draft is the local model
      vi.mocked(api.studioStart).mockRejectedValueOnce({
        code: "studio_needs_confirmation",
        message: "Confirm this run before it is sent.",
        nextStep: "Check the price, then confirm to send it.",
        details: "",
      });
      vi.mocked(api.studioEstimate).mockResolvedValue(est({ usd: 0.96, reasons: ["video"] }));
      await act(async () => {
        await useStudio.getState().start("video", false, veoRun);
      });
      await flush();
      screen.getByText("Make this video?");
      screen.getByText("Veo 3.1 through OpenRouter");
      screen.getByText("8 s · 720p · sound on · 1 video");
    });
  });
});

describe("the prompt's own checks", () => {
  /** A number constant of a Rust source file, as written there (`4_000`). */
  function rustConst(path: string, name: string): number {
    const source = readFileSync(resolve(process.cwd(), path), "utf8");
    const found = new RegExp(`const ${name}: usize = ([\\d_]+);`).exec(source)?.[1];
    return Number(found?.replace(/_/g, ""));
  }

  it("are the backend's: the same caps, in the same words", () => {
    expect(rustConst("crates/runtime/src/sd/args.rs", "PROMPT_MAX_CHARS")).toBe(PROMPT_MAX_CHARS.thisComputer);
    expect(rustConst("crates/agent/src/media/spec.rs", "PROMPT_MAX_CHARS")).toBe(PROMPT_MAX_CHARS.cloud);
    const errors = readFileSync(resolve(process.cwd(), "src-tauri/src/studio/errors.rs"), "utf8");
    for (const sentence of [STUDIO_DOCK.writePrompt, STUDIO_DOCK.promptTooLong, STUDIO_DOCK.promptTooLongHere]) {
      expect(errors).toContain(`"${sentence}"`);
    }
  });

  it("count as the backend counts: white space as Unicode has it, characters not UTF-16 units", () => {
    const cloud = { endpoint: 1 };
    expect(promptProblems("", "thisComputer")).toEqual(["Write a prompt first."]);
    // Rust's trim takes NEL (U+0085) and the line separator, and leaves U+FEFF.
    expect(promptProblems(" \t\n\u00A0\u0085\u2028\u3000", cloud)).toEqual(["Write a prompt first."]);
    expect(promptProblems("\uFEFF", cloud)).toEqual([]);
    expect(promptProblems("x".repeat(4_000), "thisComputer")).toEqual([]);
    expect(promptProblems("x".repeat(4_001), "thisComputer")).toEqual([
      "Keep the prompt under 4,000 characters for local models.",
    ]);
    expect(promptProblems("🦊".repeat(4_000), "thisComputer")).toEqual([]);
    expect(promptProblems("x".repeat(32_000), cloud)).toEqual([]);
    expect(promptProblems("🦊".repeat(32_000), cloud)).toEqual([]);
    expect(promptProblems("x".repeat(32_001), cloud)).toEqual(["Keep the prompt under 32,000 characters."]);
  });

  it("leave the prompt's sentences out of an estimate's, and keep the rest", () => {
    const answer = est({
      invalid: [
        "Write a prompt first.",
        "This model takes at most 8 reference pictures.",
        "Keep the prompt under 32,000 characters.",
        "Keep the prompt under 4,000 characters for local models.",
      ],
    });
    expect(settingsProblems(answer)).toEqual(["This model takes at most 8 reference pictures."]);
    expect(settingsProblems(null)).toEqual([]);
  });
});
