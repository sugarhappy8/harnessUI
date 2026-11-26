// The side panel's LoRAs (0.2.0 Task U8, design 8 and 10.1): *Add* lists the installed LoRAs,
// those made for the model's type first, then those of an unknown type under their own heading,
// then the others marked "may not fit this model" (ruling R6-lora-none: a warning, never a
// refusal); a LoRA whose file is gone is greyed out with its reason. Each attached LoRA has its
// name, a strength slider from the model's own bounds, the High-noise part switch on a model
// with a high-noise part, and Remove; under them *Add a LoRA file* and *Get LoRAs*. The
// composer's LoRAs button counts them and opens this section.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { LOCAL } from "./fixtures/models";
import type { InstalledLocal, LocalModels, Lora, LoraUse, StudioPrefs } from "./types";

// jsdom has none of what Radix menus, sliders and selects measure and capture with.
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
const { LorasSection } = await import("./LorasSection");
const { SettingsPanel } = await import("./SettingsPanel");
const { Composer } = await import("./Composer");
const { UiProviders } = await import("../ui/UiProviders");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

const sdxl = LOCAL.installed.find((m) => m.version === "sdxl" && !m.untested)!;
const wan5b = LOCAL.installed.find((m) => m.kind === "video")!;

/** A LoRA of `base`, added from this computer. */
function lora(id: string, name: string, base: Lora["base"], patch: Partial<Lora> = {}): Lora {
  return { id, name, base, format: "kohya", sizeBytes: 170_000_000, source: "import", repo: null, missing: false, ...patch };
}
const PIXEL = lora("local:pixel", "pixel art ü", "sdxl");
const DETAIL = lora("local:detail", "detail tweaker", "sdxl");
const MYSTERY = lora("local:mystery", "mystery style", null);
const ANIME = lora("hf:someone/anime:anime.safetensors", "anime style", "z-image", { source: "search", repo: "someone/anime" });
const GONE = lora("local:gone", "old film", "sdxl", { missing: true });
const LORAS = [PIXEL, MYSTERY, ANIME, GONE, DETAIL];

/** Wan 2.2 A14B: a video model with a high-noise part, whose LoRAs can go to either part. */
const A14B: InstalledLocal = {
  ...wan5b,
  id: "hf:QuantStack/Wan2.2-T2V-A14B-GGUF:LowNoise/Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf",
  name: "Wan 2.2 A14B",
  highNoise: true,
  loraBase: "wan2.2-ti2v",
  recipe: { id: "wan22-t2v-a14b", name: "Wan 2.2 A14B", kind: "video" },
  model: {
    ...wan5b.model,
    id: "wan22-t2v-a14b",
    displayName: "Wan 2.2 A14B",
    params: wan5b.model.params.map((p) => (p.kind.type === "loras" ? { ...p, kind: { ...p.kind, highNoise: true } } : p)),
  },
};

const PREFS: StudioPrefs = {
  lastModel: { image: { target: "thisComputer", model: sdxl.model.id } },
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
};

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

/** Studio read as it opens, with `local`'s models and LoRAs, the draft of `prefs`' models. */
async function load(local: LocalModels, prefs: StudioPrefs = PREFS) {
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [] });
  vi.mocked(api.studioLocalModels).mockResolvedValue(local);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs: [], next: null });
  vi.mocked(api.studioLive).mockResolvedValue([]);
  vi.mocked(api.studioGetPrefs).mockResolvedValue(prefs);
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
  vi.mocked(api.studioModelFiles).mockResolvedValue({
    genModelId: sdxl.id,
    recipe: sdxl.recipe,
    recipes: [sdxl.recipe!],
    slots: [],
    untested: false,
    canRun: true,
  });
  await act(async () => {
    await useStudio.getState().load();
  });
}

/** The LoRAs section of `model`, on its tab, with `loras` installed. */
async function renderSection(model: InstalledLocal = sdxl, loras: Lora[] = LORAS) {
  const tab = model.kind;
  // The model as the backend lists it (in place of the fixture's row of the same id).
  const installed = [...LOCAL.installed, A14B].map((m) => (m.id === model.id ? model : m));
  await load(
    { ...LOCAL, installed, loras },
    { ...PREFS, lastModel: { [tab]: { target: "thisComputer", model: model.model.id } } },
  );
  useStudio.getState().showStudio(tab);
  await act(async () => {
    render(
      <UiProviders>
        <LorasSection tab={tab} model={model} />
      </UiProviders>,
    );
  });
  await flush();
}

/** What the draft attaches. */
function attached(tab: "image" | "video" = "image"): LoraUse[] {
  return (useStudio.getState().drafts[tab].values.loras as LoraUse[] | undefined) ?? [];
}

/** *Add*'s menu, open. */
function openAdd(): HTMLElement {
  fireEvent.pointerDown(screen.getByRole("button", { name: "Add" }), { button: 0, ctrlKey: false, pointerType: "mouse" });
  return screen.getByRole("menu");
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
});

afterEach(() => {
  cleanup();
});

describe("LorasSection", () => {
  it('Add lists the LoRAs for this model first, and marks the others "may not fit this model"', async () => {
    await renderSection();
    const menu = openAdd();
    // In order: made for SDXL, then an unknown type under its own heading, then the others.
    const items = within(menu).getAllByRole("menuitem");
    expect(items.map((i) => i.querySelector("[data-lora-name]")?.textContent)).toEqual([
      "pixel art ü",
      "detail tweaker",
      "old film",
      "mystery style",
      "anime style",
    ]);
    within(menu).getByText("LoRA for an unknown model type");
    const mystery = items[3]!;
    expect(mystery.textContent).not.toContain("may not fit this model");
    expect(items[4]!.textContent).toContain("may not fit this model");
    expect(items[0]!.textContent).not.toContain("may not fit this model");
    // A LoRA that may not fit can still be attached: a warning, never a refusal.
    fireEvent.click(items[4]!);
    expect(attached()).toEqual([{ id: ANIME.id, weight: 1, highNoise: false }]);
    // Attached, it says so on its row, and leaves the menu.
    const row = screen.getByRole("group", { name: "anime style" });
    within(row).getByText("may not fit this model");
    const again = openAdd();
    expect(within(again).queryByText("anime style")).toBeNull();
  });

  it("a model of no known type lists every LoRA as of an unknown type, none marked as not fitting", async () => {
    const blind: InstalledLocal = { ...sdxl, loraBase: null };
    await renderSection(blind);
    const menu = openAdd();
    within(menu).getByText("LoRA for an unknown model type");
    expect(within(menu).queryByText("may not fit this model")).toBeNull();
    expect(within(menu).getAllByRole("menuitem")).toHaveLength(5);
  });

  it("a LoRA whose file is gone is greyed out with its reason and cannot be attached; one attached since says so and keeps Remove", async () => {
    await renderSection();
    const menu = openAdd();
    const gone = within(menu).getByRole("menuitem", { name: /old film/ });
    expect(gone.getAttribute("aria-disabled")).toBe("true");
    within(gone).getByText("Its file is gone from this computer.");
    fireEvent.click(gone);
    expect(attached()).toEqual([]);
    fireEvent.keyDown(menu, { key: "Escape" });

    // Attached while its file was there, then the file went.
    act(() => useStudio.getState().attachLora(PIXEL.id));
    act(() =>
      useStudio.setState((s) => ({ loras: s.loras.map((l) => (l.id === PIXEL.id ? { ...l, missing: true } : l)) })),
    );
    const row = screen.getByRole("group", { name: "pixel art ü" });
    within(row).getByText("a removed LoRA");
    within(row).getByText("Its file is gone from this computer.");
    expect(within(row).queryByRole("slider")).toBeNull();
    fireEvent.click(within(row).getByRole("button", { name: "Remove pixel art ü" }));
    expect(attached()).toEqual([]);

    // A LoRA deleted since it was attached: named by nothing else, it keeps Remove too.
    act(() => useStudio.getState().attachLora("local:deleted"));
    const lost = screen.getByRole("group", { name: "a removed LoRA" });
    fireEvent.click(within(lost).getByRole("button", { name: "Remove a removed LoRA" }));
    expect(attached()).toEqual([]);
  });

  it("a LoRA's strength slider goes from -1 to 2 in steps of 0.05, starts at 1, and is named after the LoRA", async () => {
    await renderSection();
    fireEvent.click(within(openAdd()).getByRole("menuitem", { name: /pixel art ü/ }));
    const slider = screen.getByRole("slider", { name: "Strength of pixel art ü" });
    expect(slider.getAttribute("aria-valuemin")).toBe("-1");
    expect(slider.getAttribute("aria-valuemax")).toBe("2");
    expect(slider.getAttribute("aria-valuenow")).toBe("1");
    fireEvent.keyDown(slider, { key: "ArrowRight" });
    expect(attached()).toEqual([{ id: PIXEL.id, weight: 1.05, highNoise: false }]);
    fireEvent.keyDown(slider, { key: "Home" });
    expect(attached()[0]!.weight).toBe(-1);
    fireEvent.keyDown(slider, { key: "End" });
    expect(attached()[0]!.weight).toBe(2);
    // The bounds are the model's own (its `loras` field), not Studio's.
    const narrow: InstalledLocal = {
      ...sdxl,
      model: {
        ...sdxl.model,
        params: sdxl.model.params.map((p) =>
          p.kind.type === "loras" ? { ...p, kind: { ...p.kind, weightMin: 0, weightMax: 1.5, weightStep: 0.1, weightDefault: 0.8 } } : p,
        ),
      },
    };
    cleanup();
    useStudio.setState(INITIAL, true);
    await renderSection(narrow);
    fireEvent.click(within(openAdd()).getByRole("menuitem", { name: /detail tweaker/ }));
    const other = screen.getByRole("slider", { name: "Strength of detail tweaker" });
    expect([other.getAttribute("aria-valuemin"), other.getAttribute("aria-valuemax")]).toEqual(["0", "1.5"]);
    expect(other.getAttribute("aria-valuenow")).toBe("0.8");
    // Remove takes it off the draft.
    fireEvent.click(screen.getByRole("button", { name: "Remove detail tweaker" }));
    expect(attached()).toEqual([]);
  });

  it("a Wan 2.2 A14B model shows the High-noise part switch, and others do not", async () => {
    await renderSection(A14B);
    fireEvent.click(within(openAdd()).getByRole("menuitem", { name: /pixel art ü/ }));
    const row = screen.getByRole("group", { name: "pixel art ü" });
    const part = within(row).getByRole("switch", { name: "High-noise part" });
    expect(part.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(part);
    expect(attached("video")).toEqual([{ id: PIXEL.id, weight: 1, highNoise: true }]);

    cleanup();
    useStudio.setState(INITIAL, true);
    await renderSection(sdxl);
    fireEvent.click(within(openAdd()).getByRole("menuitem", { name: /pixel art ü/ }));
    expect(screen.queryByRole("switch", { name: "High-noise part" })).toBeNull();
  });

  it("Add a LoRA file and Get LoRAs open the file chooser and Get models on LoRAs", async () => {
    await renderSection();
    fireEvent.click(screen.getByRole("button", { name: "Add a LoRA file" }));
    expect(useStudio.getState().importOpen).toBe("lora");
    fireEvent.click(screen.getByRole("button", { name: "Get LoRAs" }));
    expect(useStudio.getState()).toMatchObject({ localView: true, searchFilter: "loras" });
  });

  it("with no LoRA installed, it says so and offers no Add", async () => {
    await renderSection(sdxl, []);
    screen.getByText("No LoRAs yet");
    expect(screen.queryByRole("button", { name: "Add" })).toBeNull();
    screen.getByRole("button", { name: "Get LoRAs" });
  });

  it("the composer's LoRAs button shows the count and opens this section", async () => {
    vi.mocked(api.studioEstimate).mockResolvedValue({
      usd: 0,
      reasons: [],
      confirmAtUsd: 1,
      limit: { limitUsd: 10, spentTodayUsd: 0, pendingUsd: 0, wouldPass: true, refusal: null, reserveUsd: 5 },
      invalid: [],
      eta: null,
      local: null,
    });
    await load({ ...LOCAL, loras: LORAS });
    useStudio.getState().showStudio("image");
    const scrolled = vi.spyOn(Element.prototype, "scrollIntoView");
    /** The composer, and the panel while it is open (StudioView's part in this). */
    function Page() {
      const open = useStudio((s) => s.settingsOpen);
      return (
        <>
          <Composer tab="image" />
          {open && <SettingsPanel tab="image" />}
        </>
      );
    }
    await act(async () => {
      render(
        <UiProviders>
          <Page />
        </UiProviders>,
      );
    });
    await flush();
    const toolbar = screen.getByRole("toolbar", { name: "Picture settings" });
    within(toolbar).getByRole("button", { name: "LoRAs" });
    act(() => useStudio.getState().attachLora(PIXEL.id));
    const button = within(toolbar).getByRole("button", { name: "LoRAs (1)" });
    await act(async () => {
      fireEvent.click(button);
    });
    await flush();
    expect(useStudio.getState()).toMatchObject({ settingsOpen: true, settingsSection: "loras" });
    const section = screen.getByRole("region", { name: "LoRAs" });
    expect(scrolled.mock.contexts).toContain(section);
    within(section).getByRole("group", { name: "pixel art ü" });
    scrolled.mockRestore();

    // A model that takes no LoRAs has no button.
    const mystery = LOCAL.installed.find((m) => m.needsSetup)!;
    expect(mystery.takesLoras).toBe(false);
    act(() => useStudio.getState().chooseModel("image", "thisComputer", mystery.model.id));
    expect(within(toolbar).queryByRole("button", { name: /^LoRAs/ })).toBeNull();
  });
});
