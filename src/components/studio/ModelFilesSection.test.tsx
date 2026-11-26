// The side panel's Files (0.2.0 Task U8, design 7.5): the model type, then each part of the
// model's set-up with its role in words, its file, size and "required", and *Change*: a file of
// another installed model, a file from disk, or a file from a Hugging Face repository (a small
// dialog: the repository, then one of its files). A model type the detection did not pick says
// "untested set-up". A model that needs setting up opens the panel here, and Create stays off
// until its files can run, then comes back without a reload.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { LOCAL } from "./fixtures/models";
import type {
  Estimate,
  InstalledLocal,
  LocalModels,
  LocalRepo,
  ModelFiles,
  Part,
  PlannedSlot,
  RecipeChoice,
  SetupChoice,
  StudioPrefs,
} from "./types";

// jsdom has none of what Radix menus, selects and dialogs measure and capture with.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.hasPointerCapture ??= () => false;
Element.prototype.setPointerCapture ??= () => {};
Element.prototype.releasePointerCapture ??= () => {};
Element.prototype.scrollIntoView ??= () => {};

const ipc = vi.hoisted(() => ({
  isTauri: () => true,
  listDownloads: vi.fn(async () => []),
}));
vi.mock("../../lib/ipc", () => ipc);
vi.mock("./api");

const api = await import("./api");
const { ModelFilesSection } = await import("./ModelFilesSection");
const { StudioView } = await import("./StudioView");
const { UiProviders } = await import("../ui/UiProviders");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

const sdxl = LOCAL.installed.find((m) => m.version === "sdxl" && !m.untested)!;
const devCopy = LOCAL.installed.find((m) => m.untested && !m.needsSetup)!;
const klein = LOCAL.installed[0]!;
const mystery = LOCAL.installed.find((m) => m.needsSetup)!;

const SDXL: RecipeChoice = { id: "sdxl-checkpoint", name: "SDXL", kind: "picture" };
const LIGHTNING: RecipeChoice = { id: "sdxl-lightning", name: "SDXL Lightning", kind: "picture" };

function part(role: Part["role"], path: string, sizeBytes: number, patch: Partial<Part> = {}): Part {
  return { role, repo: "", path, sizeBytes, licenceId: "", alreadyHere: true, local: false, ...patch };
}
function slot(role: PlannedSlot["role"], source: PlannedSlot["source"], p: Part | null, required = true): PlannedSlot {
  return { role, required, source, part: p };
}

/** SDXL's files: its model file, the usual decoder, and its text encoder inside the file. */
const SDXL_FILES: ModelFiles = {
  genModelId: sdxl.id,
  recipe: SDXL,
  recipes: [SDXL, LIGHTNING],
  slots: [
    slot("model", "main", part("model", "/home/me/models/sd_xl_base_1.0.safetensors", 6_938_078_334)),
    slot(
      "vae",
      "companion",
      part("vae", "sdxl_vae.safetensors", 334_643_268, { repo: "madebyollin/sdxl-vae-fp16-fix", licenceId: "mit" }),
    ),
    slot("clip-l", "embedded", null, false),
  ],
  untested: false,
  canRun: true,
};

/** The other installed models' files, as `studio_model_files` reads them. */
const OTHER_FILES: Record<string, ModelFiles> = {
  [devCopy.id]: {
    genModelId: devCopy.id,
    recipe: SDXL,
    recipes: [SDXL],
    slots: [
      slot("model", "main", part("model", "/home/me/models/flux-dev-copy.safetensors", 6_938_078_334)),
      slot("vae", "installed", part("vae", "/home/me/models/vae/sdxl.vae.safetensors", 334_000_000)),
    ],
    untested: true,
    canRun: true,
  },
  [klein.id]: {
    genModelId: klein.id,
    recipe: { id: "flux2-klein-4b", name: "FLUX.2 Klein 4B", kind: "picture" },
    recipes: [],
    slots: [
      slot("diffusion", "main", part("diffusion", "/home/me/models/flux2-klein-4b-Q4_K_M.gguf", 2_600_000_000)),
      slot("llm", "installed", part("llm", "/home/me/models/Qwen3-4B-Q4_K_M.gguf", 2_500_000_000)),
    ],
    untested: false,
    canRun: true,
  },
};

/** A repository with two decoders, a LoRA and a pickle file, as Studio reads it. */
const VAE_REPO: LocalRepo = {
  repo: "madebyollin/sdxl-vae-fp16-fix",
  commit: "207b116dae70ace3637169f1ddd2434b91b3a8cd",
  author: "madebyollin",
  summary: null,
  verdict: "unknown",
  files: [],
  loras: [],
  fileViews: [
    { path: "sdxl.vae.safetensors", sizeBytes: 334_643_268, quant: null, label: { extra: { role: "vae" } } },
    { path: "diffusion_pytorch_model.safetensors", sizeBytes: 334_643_238, quant: null, label: { extra: { role: "vae" } } },
    { path: "style-lora.safetensors", sizeBytes: 90_000_000, quant: null, label: { lora: { base: "sdxl" } } },
    { path: "sdxl_vae.pt", sizeBytes: 334_000_000, quant: null, label: { cannotRun: { reason: "pickle", version: null } } },
  ],
  licence: null,
  licences: [],
  labelMismatch: null,
  gate: { mode: "none", accepted: true },
  readFiles: 4,
  unreadFiles: 0,
} as unknown as LocalRepo;

const PREFS: StudioPrefs = {
  lastModel: { image: { target: "thisComputer", model: sdxl.model.id } },
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
  localNoticeAcceptedAt: 1,
  lastImportMode: null,
};

/** A local estimate: free, fitting, no time yet. */
const HERE: Estimate = {
  usd: 0,
  reasons: [],
  confirmAtUsd: 1,
  limit: { limitUsd: 10, spentTodayUsd: 0, pendingUsd: 0, wouldPass: true, refusal: null, reserveUsd: 5 },
  invalid: [],
  eta: null,
  local: { fit: "fits", unloadChat: false, verySlow: false, over15Min: false, reason: null },
};

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

/** Studio read as it opens, on `local`, its Pictures draft on `prefs`' model. */
async function load(local: LocalModels = LOCAL, prefs: StudioPrefs = PREFS) {
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
  vi.mocked(api.studioEstimate).mockResolvedValue(HERE);
  vi.mocked(api.studioModelFiles).mockImplementation(
    async (id) =>
      (id === sdxl.id ? SDXL_FILES : OTHER_FILES[id]) ?? {
        genModelId: id,
        recipe: null,
        recipes: [],
        slots: [],
        untested: false,
        canRun: true,
      },
  );
  await act(async () => {
    await useStudio.getState().load();
  });
  useStudio.getState().showStudio("image");
}

/** The Files section of `model`. */
async function renderSection(model: InstalledLocal = sdxl) {
  await load();
  await act(async () => {
    render(
      <UiProviders>
        <ModelFilesSection model={model} />
      </UiProviders>,
    );
  });
  await flush();
}

/** A part's row, by its role in words. */
function row(name: string): HTMLElement {
  return screen.getByRole("listitem", { name });
}

/** Opens a Radix menu or select from its trigger. */
function press(trigger: HTMLElement) {
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false, pointerType: "mouse" });
}

/** The change `studio_set_model_files` was last asked for. */
function lastChoice(): SetupChoice {
  return vi.mocked(api.studioSetModelFiles).mock.lastCall![1];
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  ipc.listDownloads.mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
});

describe("ModelFilesSection", () => {
  it("Files lists each part with its role in words, and Change offers installed, disk and Hugging Face files", async () => {
    await renderSection();
    expect(api.studioModelFiles).toHaveBeenCalledWith(sdxl.id);
    const model = row("Model");
    within(model).getByText("sd_xl_base_1.0.safetensors");
    within(model).getByText("6.9 GB");
    within(model).getByText("required");
    // The model file is the model itself: it has no Change.
    expect(within(model).queryByRole("button", { name: "Change" })).toBeNull();
    const vae = row("Decoder (VAE)");
    within(vae).getByText("sdxl_vae.safetensors");
    within(vae).getByText("335 MB");
    within(row("Text encoder (CLIP-L)")).getByText("inside the model file");
    expect(within(row("Text encoder (CLIP-L)")).queryByText("required")).toBeNull();
    expect(screen.getByRole("combobox", { name: "Model type" }).textContent).toContain("SDXL");

    press(within(vae).getByRole("button", { name: "Change" }));
    const menu = screen.getByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((i) => i.textContent)).toEqual([
      "A file on this computer",
      "A file from disk…",
      "A file from Hugging Face…",
    ]);
    // A file on this computer: the other installed models' files for this part.
    await act(async () => {
      fireEvent.click(within(menu).getByRole("menuitem", { name: "A file on this computer" }));
    });
    await flush();
    const files = await screen.findByRole("menuitem", { name: /sdxl\.vae\.safetensors/ });
    within(files).getByText("from flux-dev-copy");
    // klein has no decoder file of its own to offer.
    expect(screen.queryByRole("menuitem", { name: /Qwen3/ })).toBeNull();
    vi.mocked(api.studioSetModelFiles).mockResolvedValue({ files: SDXL_FILES, downloadGroup: null });
    await act(async () => {
      fireEvent.click(files);
    });
    await flush();
    expect(api.studioSetModelFiles).toHaveBeenCalledWith(sdxl.id, {
      recipe: null,
      overrides: [{ role: "vae", file: { installed: { genModelId: devCopy.id, role: "vae" } } }],
    });
  });

  it("choosing a disk file calls setModelFiles with a disk override", async () => {
    await renderSection();
    const chooseFile = vi.fn(async () => "/home/me/Downloads/my vae.safetensors");
    useStudio.setState({ chooseFile });
    const chosen: ModelFiles = {
      ...SDXL_FILES,
      slots: SDXL_FILES.slots.map((s) =>
        s.role === "vae"
          ? slot("vae", "chosen", part("vae", "/home/me/Downloads/my vae.safetensors", 335_000_000, { local: true }))
          : s,
      ),
    };
    vi.mocked(api.studioSetModelFiles).mockResolvedValue({ files: chosen, downloadGroup: null });
    press(within(row("Decoder (VAE)")).getByRole("button", { name: "Change" }));
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "A file from disk…" }));
    });
    await flush();
    expect(chooseFile).toHaveBeenCalledWith("model");
    expect(api.studioSetModelFiles).toHaveBeenCalledWith(sdxl.id, {
      recipe: null,
      overrides: [{ role: "vae", file: { disk: { path: "/home/me/Downloads/my vae.safetensors" } } }],
    });
    // The parts as they are now, and the local models read again.
    within(row("Decoder (VAE)")).getByText("my vae.safetensors");
    expect(api.studioLocalModels).toHaveBeenCalledTimes(2);
    // Closed without a file: nothing changes.
    chooseFile.mockResolvedValueOnce(null as unknown as string);
    press(within(row("Decoder (VAE)")).getByRole("button", { name: "Change" }));
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "A file from disk…" }));
    });
    await flush();
    expect(api.studioSetModelFiles).toHaveBeenCalledTimes(1);
  });

  it("a file from Hugging Face is one of a repository's files, chosen in a small dialog", async () => {
    await renderSection();
    vi.mocked(api.studioLocalRepo).mockResolvedValue(VAE_REPO);
    vi.mocked(api.studioSetModelFiles).mockResolvedValue({ files: SDXL_FILES, downloadGroup: "group-vae" });
    const change = within(row("Decoder (VAE)")).getByRole("button", { name: "Change" });
    press(change);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "A file from Hugging Face…" }));
    });
    await flush();
    const dialog = await screen.findByRole("dialog", { name: "Decoder (VAE) from Hugging Face" });
    const use = within(dialog).getByRole("button", { name: "Use this file" }) as HTMLButtonElement;
    expect(use.disabled).toBe(true);
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Repository" }), {
      target: { value: " madebyollin/sdxl-vae-fp16-fix " },
    });
    await act(async () => {
      fireEvent.click(within(dialog).getByRole("button", { name: "Show its files" }));
    });
    await flush();
    expect(api.studioLocalRepo).toHaveBeenCalledWith("madebyollin/sdxl-vae-fp16-fix");
    // Only files Studio can use for a part: no LoRA, no pickle file.
    const files = within(dialog).getAllByRole("radio");
    expect(files.map((f) => f.querySelector("bdi")?.textContent)).toEqual([
      "sdxl.vae.safetensors",
      "diffusion_pytorch_model.safetensors",
    ]);
    fireEvent.click(files[0]!);
    await act(async () => {
      fireEvent.click(use);
    });
    await flush();
    expect(lastChoice()).toEqual({
      recipe: null,
      overrides: [
        {
          role: "vae",
          file: { hub: { repo: VAE_REPO.repo, commit: VAE_REPO.commit, path: "sdxl.vae.safetensors" } },
        },
      ],
    });
    expect(screen.queryByRole("dialog")).toBeNull();
    // The file it brings downloads; the model keeps its files until it is done.
    screen.getByText("Its extra files are downloading; the model is ready when they finish.");
  });

  it("a model type the detection did not pick shows the untested set-up note", async () => {
    await renderSection();
    expect(screen.queryByText("untested set-up")).toBeNull();
    vi.mocked(api.studioSetModelFiles).mockResolvedValue({
      files: { ...SDXL_FILES, recipe: LIGHTNING, untested: true },
      downloadGroup: null,
    });
    press(screen.getByRole("combobox", { name: "Model type" }));
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["SDXLpicture", "SDXL Lightningpicture"]);
    await act(async () => {
      fireEvent.click(screen.getByRole("option", { name: /^SDXL Lightning/ }));
    });
    await flush();
    expect(api.studioSetModelFiles).toHaveBeenCalledWith(sdxl.id, { recipe: LIGHTNING.id, overrides: [] });
    screen.getByText("untested set-up");
    expect(screen.getByRole("combobox", { name: "Model type" }).textContent).toContain("SDXL Lightning");
  });

  it("a set-up that is refused says why, in plain words", async () => {
    await renderSection();
    vi.mocked(api.studioSetModelFiles).mockRejectedValue({
      code: "studio_setup_incomplete",
      message: "A part has no file.",
      nextStep: "",
      details: "",
    });
    press(screen.getByRole("combobox", { name: "Model type" }));
    await act(async () => {
      fireEvent.click(screen.getByRole("option", { name: /^SDXL Lightning/ }));
    });
    await flush();
    expect(screen.getByRole("alert").textContent).toContain("Choose a file for each part first.");
  });

  it("a model that needs setting up opens Files, and Create is off until it can run", async () => {
    await load();
    const scrolled = vi.spyOn(Element.prototype, "scrollIntoView");
    await act(async () => {
      render(
        <UiProviders>
          <StudioView />
        </UiProviders>,
      );
    });
    await flush();
    // Behind the slide-over, which hides the page from a screen reader while it is open.
    const create = () => screen.getByRole("button", { name: "Create", hidden: true }) as HTMLButtonElement;
    fireEvent.change(screen.getByRole("textbox", { name: "Prompt" }), { target: { value: "a fox" } });
    await act(async () => {
      useStudio.getState().chooseModel("image", "thisComputer", mystery.model.id);
    });
    await flush();
    expect(create().disabled).toBe(true);

    vi.mocked(api.studioModelFiles).mockResolvedValue({
      genModelId: mystery.id,
      recipe: null,
      recipes: [SDXL, LIGHTNING],
      slots: [slot("model", "main", part("model", "/home/me/models/mystery.safetensors", mystery.sizeBytes), false)],
      untested: true,
      canRun: false,
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Set it up yourself" }));
    });
    await flush();
    const panel = screen.getByRole("dialog", { name: "Settings" });
    const files = within(panel).getByRole("region", { name: "Files" });
    expect(scrolled.mock.contexts).toContain(files);
    within(files).getByText("Choose the model type and its files to use this model.");
    const type = within(files).getByRole("combobox", { name: "Model type" });
    expect(type.textContent).toContain("Choose a type");
    expect(create().disabled).toBe(true);

    // SDXL chosen: its files are all here, so its row is rewritten at once, and the local
    // models read again list it set up.
    const setUp: InstalledLocal = {
      ...mystery,
      needsSetup: false,
      recipe: SDXL,
      fit: sdxl.fit,
      model: { ...mystery.model, params: sdxl.model.params, usable: null },
    };
    vi.mocked(api.studioLocalModels).mockResolvedValue({
      ...LOCAL,
      installed: LOCAL.installed.map((m) => (m.id === mystery.id ? setUp : m)),
    });
    vi.mocked(api.studioSetModelFiles).mockResolvedValue({
      files: {
        genModelId: mystery.id,
        recipe: SDXL,
        recipes: [SDXL, LIGHTNING],
        slots: [slot("model", "main", part("model", "/home/me/models/mystery.safetensors", mystery.sizeBytes))],
        untested: true,
        canRun: true,
      },
      downloadGroup: null,
    });
    press(type);
    await act(async () => {
      fireEvent.click(screen.getByRole("option", { name: /^SDXL(?! Lightning)/ }));
    });
    await flush();
    expect(api.studioSetModelFiles).toHaveBeenCalledWith(mystery.id, { recipe: SDXL.id, overrides: [] });
    await waitFor(() => expect(create().disabled).toBe(false));
    expect(screen.queryByText("Choose the model type and its files to use this model.")).toBeNull();
    scrolled.mockRestore();
  });
});
