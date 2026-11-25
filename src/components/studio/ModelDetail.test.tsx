// A result's or a catalog model's detail panel (0.2.0 Task U6, design 10.3): the review before a
// download. One choice per quantisation with the best for this computer marked and chosen
// first, the model type when there is a choice, the extra files Studio adds, the licences in
// plain words, the total and one Download, which starts the download with no second dialog
// (spec correction 20, ruling P34); the note about local models still comes first. Then the
// files Studio can't run and the LoRAs. The carried rulings: gated files in plain words with
// their next step (S7), no set-up on a pickle file (S9), a LoRA of an unknown model type is not
// said not to fit (R6-lora-none), and fit is a warning, never a refusal (design 9.2). The
// `verdict()` licence-wording cases are 0.1.x's download confirmation's, kept.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { formatBytes } from "../../lib/format";
import type { DownloadGroup } from "../../lib/types";
import { LOCAL } from "./fixtures/models";
import type {
  FileLabel,
  Licence,
  LocalEntry,
  LocalFileChoice,
  LocalFit,
  LocalModels,
  LocalRepo,
  LocalSearchHit,
  Part,
  PlannedSlot,
  RecipeChoice,
  RepoFileView,
  SetupPlan,
} from "./types";

const ipc = vi.hoisted(() => ({
  isTauri: () => true,
  listDownloads: vi.fn<() => Promise<DownloadGroup[]>>(),
  onDownloadProgress: vi.fn<(handler: (g: DownloadGroup) => void) => Promise<() => void>>(),
  hubTokenStatus: vi.fn<() => Promise<{ saved: boolean; account: string | null }>>(),
}));
vi.mock("../../lib/ipc", () => ipc);
vi.mock("./api");
const openUrl = vi.hoisted(() => vi.fn<(url: string) => Promise<void>>());
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));

// Radix Select captures the pointer and scrolls its options, which jsdom cannot.
Element.prototype.hasPointerCapture ??= () => false;
Element.prototype.setPointerCapture ??= () => {};
Element.prototype.releasePointerCapture ??= () => {};
Element.prototype.scrollIntoView ??= () => {};

const api = await import("./api");
const { verdict } = await import("./ModelDetail");
const { PREFS, click, flush, showLocal } = await import("./testing/local");
const { useStudio } = await import("../../store/studio");
const { useDownloads } = await import("../../store/downloads");

const INITIAL = useStudio.getState();

const zimage = LOCAL.recommended.find((e) => e.id === "z-image-turbo-q4")!;
const apache = zimage.licence;

/** Commercial use under a $5M yearly revenue, with a duty for what you share. */
const kommunity: Licence = {
  ...apache,
  id: "kommunity",
  name: "Example Community Licence",
  url: "https://example.com/kommunity",
  sha256: "cd".repeat(32),
  badge: "amber",
  commercial: "underRevenue",
  revenueLimitUsd: 5_000_000,
  plainWords: "Commercial use only under $5M revenue.",
  obligations: ["Review pictures before you share them."],
};

const nonCommercial: Licence = {
  ...apache,
  id: "nc",
  name: "Non-Commercial Licence",
  sha256: "b".repeat(64),
  badge: "red",
  commercial: "no",
  revenueLimitUsd: null,
  plainWords: "Only for non-commercial use.",
};

/** An OpenRAIL licence as a search reads it (design 11a). */
const openrail: Licence = {
  ...apache,
  id: "creativeml-openrail-m",
  name: "CreativeML Open RAIL-M",
  url: "https://huggingface.co/someone/sd15/blob/0123/LICENSE.md",
  badge: "amber",
  commercial: "withUseRules",
  revenueLimitUsd: null,
  plainWords: "Commercial use allowed, with use rules",
  obligations: ["Pass the use restrictions on to anyone you give the model to."],
};

// ---- a searched Z-Image GGUF repository with three quantisations ------------------------------

const REPO = "someone/Z-Image-Turbo-GGUF";
const COMMIT = "c".repeat(40);
const fits: LocalFit = { ...zimage.fit, word: "fits", verdict: "green" };
const tooBig: LocalFit = { ...zimage.fit, word: "notHere", verdict: "red", reason: "it needs 8 GB of graphics memory" };
const zRecipe: RecipeChoice = { id: "z-image-turbo", name: "Z-Image Turbo", kind: "picture" };

function part(role: Part["role"], path: string, sizeBytes: number, patch: Partial<Part> = {}): Part {
  return { role, repo: REPO, path, sizeBytes, licenceId: "apache-2.0", alreadyHere: false, local: false, ...patch };
}

/** The prompt reader, already on this computer from another model. */
const encoder = part("llm", "Qwen3-4B-Instruct-2507-Q4_K_M.gguf", 2_500_000_000, {
  repo: "unsloth/Qwen3-4B-Instruct-2507-GGUF",
  alreadyHere: true,
});
const vae = part("vae", "split_files/vae/ae.safetensors", 335_000_000, { repo: "Comfy-Org/Z-Image-Turbo" });

/** A Z-Image file's plan: itself, the prompt reader (here already) and the decoder. */
function zPlan(file: string, size: number, fit: LocalFit, patch: Partial<SetupPlan> = {}): SetupPlan {
  const slots: PlannedSlot[] = [
    { role: "diffusion", required: true, source: "main", part: part("diffusion", file, size) },
    { role: "llm", required: true, source: "installed", part: encoder },
    { role: "vae", required: true, source: "companion", part: vae },
  ];
  return {
    recipe: zRecipe,
    recipes: [zRecipe],
    slots,
    licences: [apache],
    totalBytes: size + encoder.sizeBytes + vae.sizeBytes,
    toFetchBytes: size + vae.sizeBytes,
    fit,
    canDownload: true,
    lowDisk: false,
    needsEngine: false,
    untested: false,
    ...patch,
  };
}

const works: FileLabel = {
  works: { version: "z-image", recipe: "z-image-turbo", kind: "picture", part: null, parameters: 6_150_000_000, folder: false },
};

function choice(file: string, quant: string, size: number, fit: LocalFit, best: boolean): LocalFileChoice {
  return { file, quant, label: works, plan: zPlan(file, size, fit), best };
}

/** Its three files, largest set-up first: Q5_K_M is the largest that fits, Q8_0 may not fit. */
const Q8 = choice("z-image-turbo-Q8_0.gguf", "Q8_0", 6_700_000_000, tooBig, false);
const Q5 = choice("z-image-turbo-Q5_K_M.gguf", "Q5_K_M", 4_300_000_000, fits, true);
const Q4 = choice("z-image-turbo-Q4_K.gguf", "Q4_K", 3_600_000_000, fits, false);

function view(path: string, sizeBytes: number, label: FileLabel, quant: string | null = null): RepoFileView {
  return { path, sizeBytes, quant, label };
}

function supported(patch: Partial<LocalRepo> = {}): LocalRepo {
  return {
    repo: REPO,
    commit: COMMIT,
    author: "someone",
    summary: "Z-Image Turbo as GGUF files: photo-real pictures in 8 steps.",
    verdict: { works: { version: "z-image", kind: "picture" } },
    files: [Q8, Q5, Q4],
    loras: [],
    fileViews: [Q8, Q5, Q4].map((f) => view(f.file, f.plan.slots[0]!.part!.sizeBytes, works, f.quant)),
    licence: apache,
    licences: [apache],
    labelMismatch: null,
    gate: { mode: "none", collects: [] },
    readFiles: 3,
    unreadFiles: 0,
    ...patch,
  };
}

function hit(repo: string): LocalSearchHit {
  return {
    repo,
    author: repo.split("/")[0]!,
    downloads: 12_345,
    likes: 10,
    updatedAt: null,
    gated: "none",
    licenseLabel: "apache-2.0",
    verdict: "unknown",
    exact: false,
  };
}

function card(name: string): HTMLElement {
  return screen.getByRole("article", { name });
}

/** Get models open with `repo` among the popular results (`local` as the backend reads it); its
 *  card opened. Returns the panel. */
async function openRepo(repo: LocalRepo = supported(), local: LocalModels = LOCAL) {
  vi.mocked(api.studioLocalRepo).mockResolvedValue(repo);
  await showLocal(local, { popular: [hit(repo.repo)] });
  const open = within(card(repo.repo)).getByRole("button", { name: repo.repo });
  open.focus();
  await click(open);
  expect(api.studioLocalRepo).toHaveBeenCalledWith(repo.repo);
  return screen.getByRole("dialog", { name: repo.repo });
}

/** Get models open on Studio's list, `entry`'s card's Download pressed. Returns the panel. */
async function openEntry(entry: LocalEntry = zimage, local: LocalModels = LOCAL) {
  const list = { ...local, recommended: local.recommended.map((e) => (e.id === entry.id ? entry : e)) };
  await showLocal(list);
  await click(within(card(entry.name)).getByRole("button", { name: "Download" }));
  return screen.getByRole("dialog", { name: entry.name });
}

function radio(panel: HTMLElement, quant: string): HTMLElement {
  return within(panel).getByRole("radio", { name: new RegExp(`^${quant}`) });
}

function downloadButton(panel: HTMLElement): HTMLButtonElement {
  return within(panel).getByRole<HTMLButtonElement>("button", { name: "Download" });
}

/** A part's row in *Extra files Studio adds*. */
function partRow(panel: HTMLElement, role: Part["role"]): HTMLElement {
  return panel.querySelector<HTMLElement>(`[data-part="${role}"]`)!;
}

const source = (file: string) => ({ search: { repo: REPO, commit: COMMIT, file } });

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  useDownloads.setState({ groups: [] });
  ipc.listDownloads.mockResolvedValue([]);
  ipc.hubTokenStatus.mockResolvedValue({ saved: false, account: null });
  ipc.onDownloadProgress.mockResolvedValue(() => undefined);
  vi.mocked(api.studioAcceptLocalNotice).mockResolvedValue({ ...PREFS, localNoticeAcceptedAt: 1_700_000_000_000 });
  vi.mocked(api.studioLocalDownloadFile).mockResolvedValue("group-file");
  vi.mocked(api.studioLoraDownload).mockResolvedValue("group-lora");
  openUrl.mockResolvedValue(undefined);
});

afterEach(cleanup);

describe("verdict", () => {
  it("says what the pictures you make may be used for, by the strictest licence", () => {
    expect(verdict([apache], "image")).toBe("Pictures you make: free for any use, including commercial.");
    expect(verdict([apache, kommunity], "image")).toBe("Pictures you make: commercial use only under $5M revenue.");
    expect(verdict([kommunity, { ...kommunity, id: "tiny", revenueLimitUsd: 1_000_000 }], "image")).toBe(
      "Pictures you make: commercial use only under $1M revenue.",
    );
    expect(verdict([{ ...kommunity, commercial: "registration", revenueLimitUsd: null }, kommunity], "image")).toBe(
      "Pictures you make: commercial use only after registering with the publisher.",
    );
    expect(verdict([apache, { ...nonCommercial, commercial: "unclear" }], "image")).toBe(
      "Pictures you make: check the licence before any commercial use.",
    );
    // Use rules (OpenRAIL) are stricter than a registration and laxer than an unclear licence.
    const rail: Licence = { ...kommunity, id: "openrail", commercial: "withUseRules", revenueLimitUsd: null };
    expect(verdict([{ ...kommunity, commercial: "registration", revenueLimitUsd: null }, rail], "image")).toBe(
      "Pictures you make: commercial use allowed, with the licence's use rules.",
    );
    expect(verdict([rail, { ...nonCommercial, commercial: "unclear" }], "image")).toBe(
      "Pictures you make: check the licence before any commercial use.",
    );
    expect(verdict([kommunity, nonCommercial, { ...nonCommercial, commercial: "unclear" }], "image")).toBe(
      "Pictures you make: non-commercial use only.",
    );
    expect(verdict([apache, { ...apache, excludedRegions: ["EU"] }], "image")).toBe(
      "Pictures you make: commercial use in permitted regions.",
    );
    expect(verdict([], "image")).toBe("");
  });

  it("a video model's says what the videos you make may be used for", () => {
    expect(verdict([apache], "video")).toBe("Videos you make: free for any use, including commercial.");
    expect(verdict([apache, kommunity], "video")).toBe("Videos you make: commercial use only under $5M revenue.");
    expect(verdict([{ ...kommunity, revenueLimitUsd: null }], "video")).toBe(
      "Videos you make: commercial use only under a revenue limit.",
    );
    expect(verdict([{ ...kommunity, commercial: "registration" }], "video")).toBe(
      "Videos you make: commercial use only after registering with the publisher.",
    );
    expect(verdict([{ ...nonCommercial, commercial: "unclear" }], "video")).toBe(
      "Videos you make: check the licence before any commercial use.",
    );
    expect(verdict([nonCommercial], "video")).toBe("Videos you make: non-commercial use only.");
  });
});

describe("a result's detail panel", () => {
  it("opening a result shows its summary, one choice per quantisation, and Best for this computer on the largest that fits", async () => {
    const panel = await openRepo();
    within(panel).getByText("Z-Image Turbo as GGUF files: photo-real pictures in 8 steps.");
    within(panel).getByText("Works in Studio · Z-Image");
    within(panel).getByText("by someone");
    const main = within(panel).getByRole("radiogroup", { name: "Main file" });
    const radios = within(main).getAllByRole("radio");
    expect(radios.map((r) => r.textContent)).toEqual([
      expect.stringContaining("Q8_0"),
      expect.stringContaining("Q5_K_M"),
      expect.stringContaining("Q4_K"),
    ]);
    // Each with its own size and fit word.
    within(radio(panel, "Q8_0")).getByText("6.7 GB");
    within(radio(panel, "Q8_0")).getByText("Not for this computer");
    within(radio(panel, "Q5_K_M")).getByText("4.3 GB");
    within(radio(panel, "Q4_K")).getByText("Fits");
    // The best for this computer is marked once, on the largest that fits.
    expect(within(panel).getAllByText("Best for this computer")).toHaveLength(1);
    within(radio(panel, "Q5_K_M")).getByText("Best for this computer");
    // The main file's role and licence, and its path.
    within(panel).getByText("Diffusion model · Apache License 2.0");
    within(panel).getByText("z-image-turbo-Q5_K_M.gguf");
  });

  it("the best choice is preselected, and the plan lists each extra file in words with its size and \"already on this computer\"", async () => {
    const panel = await openRepo();
    expect(radio(panel, "Q5_K_M").getAttribute("aria-checked")).toBe("true");
    expect(radio(panel, "Q8_0").getAttribute("aria-checked")).toBe("false");
    const extras = within(panel).getByRole("region", { name: "Extra files Studio adds" });
    const reader = partRow(extras, "llm");
    within(reader).getByText("Text encoder (language model)");
    within(reader).getByText("2.5 GB");
    within(reader).getByText("already on this computer");
    within(reader).getByText("Apache License 2.0");
    const decoder = partRow(extras, "vae");
    within(decoder).getByText("Decoder (VAE)");
    within(decoder).getByText("335 MB");
    expect(within(decoder).queryByText("already on this computer")).toBeNull();
    // The main file is the choice above, not an extra file.
    expect(partRow(extras, "diffusion")).toBeNull();
    // With no best, the smallest is chosen first.
    cleanup();
    useStudio.setState(INITIAL, true);
    const none = await openRepo(supported({ files: [Q8, { ...Q5, best: false }, Q4] }));
    expect(radio(none, "Q4_K").getAttribute("aria-checked")).toBe("true");
    expect(within(none).queryByText("Best for this computer")).toBeNull();
  });

  it("the total counts only the files to download", async () => {
    const panel = await openRepo();
    // Q5_K_M and the decoder; the prompt reader is here already.
    within(panel).getByText(`Total to download: ${formatBytes(4_300_000_000 + 335_000_000)}`);
    expect(within(panel).queryByText(`Total to download: ${formatBytes(Q5.plan.totalBytes)}`)).toBeNull();
    await click(radio(panel, "Q4_K"));
    within(panel).getByText(`Total to download: ${formatBytes(3_600_000_000 + 335_000_000)}`);
    // The Studio engine, when it comes with the model, counts too, and says so.
    cleanup();
    useStudio.setState(INITIAL, true);
    const local = { ...LOCAL, engine: { ...LOCAL.engine, state: "notInstalled" as const, downloadBytes: 30_303_000 } };
    const engine = supported({ files: [{ ...Q4, best: true, plan: { ...Q4.plan, needsEngine: true } }] });
    const withEngine = await openRepo(engine, local);
    within(withEngine).getByText(`Total to download: ${formatBytes(3_600_000_000 + 335_000_000 + 30_303_000)}`);
    within(withEngine).getByText("Also downloads the Studio engine (29 MB, MIT licence).");
  });

  it("changing the model type asks for a new plan", async () => {
    const schnell: RecipeChoice = { id: "flux1-schnell", name: "FLUX.1 schnell", kind: "picture" };
    const dev: RecipeChoice = { id: "flux1-dev", name: "FLUX.1 dev", kind: "picture" };
    const file = "flux1-Q4_K_S.gguf";
    const own = zPlan(file, 6_800_000_000, fits, { recipe: schnell, recipes: [schnell, dev] });
    vi.mocked(api.studioSetupPlan).mockResolvedValue({ ...own, recipe: dev, untested: true });
    const panel = await openRepo(supported({ files: [{ ...Q4, file, quant: "Q4_K_S", plan: own, best: true }] }));
    const type = within(panel).getByRole("combobox", { name: "Model type" });
    expect(type.textContent).toContain("FLUX.1 schnell");
    fireEvent.pointerDown(type, { button: 0, ctrlKey: false, pointerType: "mouse" });
    await click(screen.getByRole("option", { name: /^FLUX\.1 dev/ }));
    expect(api.studioSetupPlan).toHaveBeenCalledTimes(1);
    expect(api.studioSetupPlan).toHaveBeenCalledWith(source(file), { recipe: "flux1-dev", overrides: [] });
    expect(within(panel).getByRole("combobox", { name: "Model type" }).textContent).toContain("FLUX.1 dev");
    within(panel).getByText("untested set-up");
    // Download sends the choice the plan shown was made with.
    await click(downloadButton(panel));
    expect(api.studioLocalDownload).toHaveBeenCalledWith(source(file), { recipe: "flux1-dev", overrides: [] });
  });

  it("a plan still on its way when another main file is chosen is dropped, and Download is on again", async () => {
    const schnell: RecipeChoice = { id: "flux1-schnell", name: "FLUX.1 schnell", kind: "picture" };
    const dev: RecipeChoice = { id: "flux1-dev", name: "FLUX.1 dev", kind: "picture" };
    const withTypes = (f: LocalFileChoice): LocalFileChoice => ({ ...f, plan: { ...f.plan, recipe: schnell, recipes: [schnell, dev] } });
    let answer: (p: SetupPlan) => void = () => undefined;
    vi.mocked(api.studioSetupPlan).mockReturnValue(new Promise((resolve) => (answer = resolve)));
    const panel = await openRepo(supported({ files: [withTypes(Q8), withTypes(Q5), withTypes(Q4)] }));
    fireEvent.pointerDown(within(panel).getByRole("combobox", { name: "Model type" }), { button: 0, ctrlKey: false, pointerType: "mouse" });
    await click(screen.getByRole("option", { name: /^FLUX\.1 dev/ }));
    // While it is asked for, Download waits.
    within(panel).getByText("Updating the files…");
    expect(downloadButton(panel).disabled).toBe(true);
    await click(radio(panel, "Q4_K"));
    expect(within(panel).queryByText("Updating the files…")).toBeNull();
    expect(downloadButton(panel).disabled).toBe(false);
    // The late answer, for the file before, changes nothing.
    await act(async () => answer({ ...Q5.plan, recipe: dev, untested: true }));
    expect(within(panel).queryByText("untested set-up")).toBeNull();
    await click(downloadButton(panel));
    expect(api.studioLocalDownload).toHaveBeenCalledWith(source(Q4.file), null);
  });

  it("a missing part offers Choose a file, and Download says why it is off", async () => {
    const t5: PlannedSlot = { role: "t5xxl", required: true, source: "missing", part: null };
    const missing = zPlan(Q5.file, 4_300_000_000, fits, { canDownload: false });
    missing.slots = [...missing.slots, t5];
    const panel = await openRepo(supported({ files: [{ ...Q5, plan: missing }] }));
    const row = partRow(panel, "t5xxl");
    within(row).getByText("Text encoder (T5-XXL)");
    within(row).getByText("No file yet");
    const choose = within(row).getByRole("button", { name: "Choose a file" });
    expect(choose.getAttribute("aria-describedby")).toBe(within(row).getByText("Text encoder (T5-XXL)").id);
    const download = downloadButton(panel);
    expect(download.disabled).toBe(true);
    const why = within(panel).getByText("Choose a file for each part first.");
    expect(document.getElementById(download.getAttribute("aria-describedby")!)!.contains(why)).toBe(true);
    // A file from this computer: only .safetensors, .sft and .gguf are offered (ruling S9).
    const chooseFile = vi.fn<(kind: "model" | "lora") => Promise<string | null>>().mockResolvedValue("/m/t5xxl.safetensors");
    act(() => useStudio.setState({ chooseFile }));
    const chosen: Part = { ...part("t5xxl", "/m/t5xxl.safetensors", 4_900_000_000, { repo: "", licenceId: "", alreadyHere: true, local: true }) };
    vi.mocked(api.studioSetupPlan).mockResolvedValue({
      ...missing,
      canDownload: true,
      slots: [...missing.slots.slice(0, 3), { role: "t5xxl", required: true, source: "chosen", part: chosen }],
    });
    await click(choose);
    expect(chooseFile).toHaveBeenCalledWith("model");
    const override = { recipe: null, overrides: [{ role: "t5xxl", file: { disk: { path: "/m/t5xxl.safetensors" } } }] };
    expect(api.studioSetupPlan).toHaveBeenCalledWith(source(Q5.file), override);
    within(partRow(panel, "t5xxl")).getByText("Chosen by you · t5xxl.safetensors");
    expect(within(panel).queryByText("Choose a file for each part first.")).toBeNull();
    expect(downloadButton(panel).disabled).toBe(false);
    await click(downloadButton(panel));
    expect(api.studioLocalDownload).toHaveBeenCalledWith(source(Q5.file), override);
  });

  it("Download starts the download with the choice, with no other dialog", async () => {
    const panel = await openRepo();
    const reads = vi.mocked(api.studioLocalModels).mock.calls.length;
    await click(downloadButton(panel));
    expect(api.studioLocalDownload).toHaveBeenCalledTimes(1);
    // As its plan comes: no choice was made.
    expect(api.studioLocalDownload).toHaveBeenCalledWith(source(Q5.file), null);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    // The panel stays, and the local models are read again.
    screen.getByRole("dialog", { name: REPO });
    expect(vi.mocked(api.studioLocalModels).mock.calls.length).toBe(reads + 1);
  });

  it("the first Download shows the local-model notice first", async () => {
    const panel = await openRepo(supported(), { ...LOCAL, noticeAccepted: false });
    await click(downloadButton(panel));
    const notice = screen.getByRole("alertdialog");
    within(notice).getByText("Local models run only on your computer.");
    expect(api.studioLocalDownload).not.toHaveBeenCalled();
    // The backend records it, and says so when the local models are read again.
    vi.mocked(api.studioLocalModels).mockResolvedValue(LOCAL);
    await click(within(notice).getByRole("button", { name: "I understand" }));
    await waitFor(() => expect(api.studioLocalDownload).toHaveBeenCalledTimes(1));
    expect(api.studioAcceptLocalNotice).toHaveBeenCalledTimes(1);
    expect(api.studioLocalDownload).toHaveBeenCalledWith(source(Q5.file), null);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(useStudio.getState().local?.noticeAccepted).toBe(true);
    // Accepted: the next Download asks nothing.
    await click(downloadButton(screen.getByRole("dialog", { name: REPO })));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(api.studioLocalDownload).toHaveBeenCalledTimes(2);
  });

  it("a file Studio can't run shows its reason and a quiet Download file anyway", async () => {
    const lumina: FileLabel = { cannotRun: { reason: "lumina2", version: null } };
    const pickle: FileLabel = { cannotRun: { reason: "pickle", version: null } };
    const repo = supported({
      repo: "Alpha-VLLM/Lumina-Image-2.0",
      summary: "Lumina-Image 2.0: a 2.6B picture model.",
      verdict: lumina,
      files: [],
      licences: [],
      fileViews: [
        view("consolidated.00-of-01.pth", 10_400_000_000, pickle),
        view("split_files/diffusion_models/lumina_2.safetensors", 5_219_585_680, lumina),
        view("ae.safetensors", 335_000_000, { extra: { role: "vae" } }),
      ],
    });
    const panel = await openRepo(repo);
    within(panel).getByText("Can't run: Lumina 2");
    within(panel).getByText("Can't run in Studio: This is a Lumina 2 model, which the Studio engine can't run");
    within(panel).getByText(/Studio can't set this repository up/);
    // No set-up: no main file, no part to choose, no Download.
    expect(within(panel).queryByRole("radio")).toBeNull();
    expect(within(panel).queryByRole("button", { name: "Choose a file" })).toBeNull();
    expect(within(panel).queryByRole("button", { name: "Download" })).toBeNull();
    // The pickle file says only why, with the quiet plain download (ruling S9).
    const files = within(panel).getByRole("region", { name: "Other files in this repository" });
    within(files).getByText("Can't run in Studio: Studio can't read .pth files · 10.4 GB");
    within(files).getByText("Extra file · Decoder (VAE) · 335 MB");
    const anyway = within(files).getAllByRole("button", { name: "Download file anyway" });
    expect(anyway).toHaveLength(3);
    for (const b of anyway) expect(b.className).not.toContain("btn-primary");
    await click(anyway[0]!);
    expect(api.studioLocalDownloadFile).toHaveBeenCalledWith("Alpha-VLLM/Lumina-Image-2.0", COMMIT, "consolidated.00-of-01.pth");
    within(files).getByText("Queued in Models → Downloads.");
    expect(api.studioLocalDownload).not.toHaveBeenCalled();
    within(panel).getByRole("link", { name: "View files on Hugging Face" });
  });

  it("a pickle file of a repository that works is never a main file", async () => {
    const pickle: FileLabel = { cannotRun: { reason: "pickle", version: null } };
    const repo = supported({ fileViews: [...supported().fileViews, view("z-image-turbo.ckpt", 12_300_000_000, pickle)] });
    const panel = await openRepo(repo);
    expect(within(panel).getAllByRole("radio")).toHaveLength(3);
    expect(within(panel).queryByRole("radio", { name: /ckpt/ })).toBeNull();
    const files = within(panel).getByRole("region", { name: "Other files in this repository" });
    within(files).getByText("Can't run in Studio: Studio can't read .ckpt files · 12.3 GB");
    within(files).getByRole("button", { name: "Download file anyway" });
    // The files that work are in the set-up, not listed again.
    expect(within(files).queryByText(/z-image-turbo-Q/)).toBeNull();
  });

  it("an OpenRAIL licence reads amber \"Commercial use allowed, with use rules\" with a link", async () => {
    const sdxl = "stabilityai/stable-diffusion-xl-base-1.0";
    const file = "sd_xl_base_1.0.safetensors";
    const label: FileLabel = {
      works: { version: "sdxl", recipe: "sdxl-checkpoint", kind: "picture", part: null, parameters: 3.5e9, folder: false },
    };
    const plan: SetupPlan = {
      ...zPlan(file, 6_940_000_000, fits),
      recipe: { id: "sdxl-checkpoint", name: "SDXL", kind: "picture" },
      recipes: [{ id: "sdxl-checkpoint", name: "SDXL", kind: "picture" }],
      slots: [{ role: "model", required: true, source: "main", part: { ...part("model", file, 6_940_000_000), repo: sdxl, licenceId: openrail.id } }],
      licences: [openrail],
      totalBytes: 6_940_000_000,
      toFetchBytes: 6_940_000_000,
    };
    const panel = await openRepo(
      supported({ repo: sdxl, verdict: { works: { version: "sdxl", kind: "picture" } }, files: [{ file, quant: "fp16", label, plan, best: true }], licence: openrail, licences: [openrail], fileViews: [] }),
    );
    const licences = within(panel).getByRole("region", { name: "Licences" });
    const row = within(licences).getByText("Commercial use allowed, with use rules").closest<HTMLElement>("[data-licence]")!;
    expect(row.getAttribute("data-licence")).toBe("amber");
    expect(row.className).toContain("warning");
    const link = within(row).getByRole("link", { name: "CreativeML Open RAIL-M" });
    expect(link.getAttribute("href")).toBe(openrail.url);
    await click(link);
    expect(openUrl).toHaveBeenCalledWith(openrail.url);
    within(row).getByText("The licence asks you to pass the use restrictions on to anyone you give the model to.");
    within(licences).getByText("Pictures you make: commercial use allowed, with the licence's use rules.");
    // One file alone is shown as the one chosen, marked best.
    expect(within(panel).queryByRole("radio")).toBeNull();
    within(panel).getByText("fp16");
    within(panel).getByText("Best for this computer");
    within(panel).getByText("Model · CreativeML Open RAIL-M");
  });

  it("every file's role, size and licence, the licence wording and the total are visible before Download", async () => {
    const parts = { llm: { ...encoder, licenceId: kommunity.id }, vae };
    const plan = zPlan(Q5.file, 4_300_000_000, fits, { licences: [apache, kommunity] });
    plan.slots = plan.slots.map((s) => (s.role === "llm" ? { ...s, part: parts.llm } : s));
    const panel = await openRepo(supported({ files: [Q8, { ...Q5, plan }, Q4] }));
    const download = downloadButton(panel);
    const before = (el: HTMLElement) =>
      expect(el.compareDocumentPosition(download) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // The main file: its role, size and licence.
    before(within(panel).getByText("Diffusion model · Apache License 2.0"));
    before(within(radio(panel, "Q5_K_M")).getByText("4.3 GB"));
    // Every extra file: its role, size and licence.
    const extras = within(panel).getByRole("region", { name: "Extra files Studio adds" });
    for (const [role, label, size, licence] of [
      ["llm", "Text encoder (language model)", "2.5 GB", "Example Community Licence"],
      ["vae", "Decoder (VAE)", "335 MB", "Apache License 2.0"],
    ] as const) {
      const row = partRow(extras, role);
      before(within(row).getByText(label));
      before(within(row).getByText(size));
      before(within(row).getByText(licence));
    }
    // Every licence in plain words with its link, the verdict, then the total.
    const licences = within(panel).getByRole("region", { name: "Licences" });
    for (const l of [apache, kommunity]) {
      before(within(licences).getByText(l.plainWords));
      expect(within(licences).getByRole("link", { name: l.name }).getAttribute("href")).toBe(l.url);
    }
    before(within(licences).getByText("Pictures you make: commercial use only under $5M revenue."));
    within(licences).getByText("The licence asks you to review pictures before you share them.");
    before(within(panel).getByText(`Total to download: ${formatBytes(4_300_000_000 + 335_000_000)}`));
    // Nothing is behind another click.
    expect(panel.querySelector("details")).toBeNull();
  });

  it("the panel traps focus, and Esc closes it with focus back on the card", async () => {
    const panel = await openRepo();
    // The page behind is hidden from screen readers while the panel is open.
    const behind = screen.getByRole("article", { name: REPO, hidden: true });
    expect(behind.closest("[aria-hidden=true]")).not.toBeNull();
    const opener = within(behind).getByRole("button", { name: REPO, hidden: true });
    expect(panel.contains(document.activeElement)).toBe(true);
    // Tab at the last control wraps inside the panel (Radix's focus scope).
    const last = within(panel).getAllByRole("link").pop()!;
    last.focus();
    fireEvent.keyDown(last, { key: "Tab" });
    expect(panel.contains(document.activeElement)).toBe(true);
    fireEvent.keyDown(panel, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(useStudio.getState().detail).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(opener));
  });

  it("a main file that may not fit stays choosable, and Download stays on with the warning", async () => {
    const panel = await openRepo();
    const q8 = radio(panel, "Q8_0");
    expect(q8.hasAttribute("disabled")).toBe(false);
    expect(within(panel).queryByText(/^May not fit this computer/)).toBeNull();
    await click(q8);
    expect(q8.getAttribute("aria-checked")).toBe("true");
    within(panel).getByText("May not fit this computer: it needs 8 GB of graphics memory. It may fail or be very slow.");
    const download = downloadButton(panel);
    expect(download.disabled).toBe(false);
    await click(download);
    expect(api.studioLocalDownload).toHaveBeenCalledWith(source(Q8.file), null);
  });

  it("a searched file downloading shows how far it got in place of Download", async () => {
    ipc.listDownloads.mockResolvedValue([
      {
        id: "group-1",
        repo: REPO,
        label: Q5.file,
        fileCount: 2,
        bytesDone: 1_000_000_000,
        bytesTotal: 4_635_000_000,
        status: "downloading",
        error: null,
        bytesPerSecond: null,
        unverified: false,
        genModelId: `hf:${REPO}:${Q5.file}`,
      },
    ]);
    const panel = await openRepo();
    expect(within(panel).queryByRole("button", { name: "Download" })).toBeNull();
    within(panel).getByText("1.0 GB of 4.6 GB · downloading");
    within(panel).getByRole("progressbar", { name: `Downloading ${Q5.file}` });
    // Another file is not that download.
    await click(radio(panel, "Q4_K"));
    downloadButton(panel);
  });

  it("a searched file's name shows with invisible characters as codes, and goes to the download as it came", async () => {
    const file = "z-image‮turbo.gguf";
    const panel = await openRepo(supported({ files: [{ ...Q5, file, quant: "" }] }));
    expect(within(panel).getAllByText("z-image\\u{202E}turbo.gguf").length).toBeGreaterThan(0);
    await click(downloadButton(panel));
    expect(api.studioLocalDownload).toHaveBeenCalledWith(source(file), null);
    expect(new XMLSerializer().serializeToString(document.body)).not.toContain("‮");
  });

  it("a download that could not start says why, and Download stays", async () => {
    const panel = await openRepo();
    vi.mocked(api.studioLocalDownload).mockRejectedValue({
      code: "studio_disk_too_small",
      message: "There is not enough free space for this model.",
      nextStep: "Free up space or choose another models folder.",
      details: "",
    });
    await click(downloadButton(panel));
    within(panel).getByText("There is not enough free space for this model.");
    within(panel).getByText("Free up space or choose another models folder.");
    expect(downloadButton(panel).disabled).toBe(false);
  });
});

describe("gated files, in plain words with their next step (ruling S7)", () => {
  const GATED = "black-forest-labs/FLUX.1-Kontext-dev";
  const unread = (reason: "needsToken" | "needsTerms"): LocalRepo =>
    supported({
      repo: GATED,
      files: [],
      summary: null,
      gate: { mode: "auto", collects: ["username", "email"] },
      verdict: { works: { version: "flux", kind: "pictureEdit" } },
      fileViews: [view("flux1-kontext-dev.safetensors", 23_800_000_000, { notRead: { reason } })],
    });

  it("hf_needs_token: Download is off, with Settings → Hugging Face, and the repository is read again once Settings closes", async () => {
    const opened: unknown[] = [];
    const onOpen = (e: Event) => opened.push((e as CustomEvent<unknown>).detail);
    window.addEventListener("app:open-settings", onOpen);
    try {
      const panel = await openRepo(unread("needsToken"));
      const download = downloadButton(panel);
      expect(download.disabled).toBe(true);
      const why = within(panel).getByText("This model is gated. Add a Hugging Face token in Settings → Hugging Face first.");
      expect(document.getElementById(download.getAttribute("aria-describedby")!)!.contains(why)).toBe(true);
      // No plain download of a file that would only fail.
      expect(within(panel).queryByRole("button", { name: "Download file anyway" })).toBeNull();
      await click(within(panel).getByRole("button", { name: "Open Settings" }));
      expect(opened).toEqual(["huggingface"]);
      // A token saved there: its files are read, and Download is on.
      vi.mocked(api.studioLocalRepo).mockResolvedValue(supported({ repo: GATED, gate: { mode: "auto", collects: [] } }));
      await act(async () => {
        window.dispatchEvent(new Event("app:settings-closed"));
      });
      await flush();
      expect(api.studioLocalRepo).toHaveBeenCalledTimes(2);
      const now = screen.getByRole("dialog", { name: GATED });
      expect(downloadButton(now).disabled).toBe(false);
      expect(within(now).getAllByRole("radio")).toHaveLength(3);
    } finally {
      window.removeEventListener("app:open-settings", onOpen);
    }
  });

  it("hf_needs_terms: Download is off, with the model's page", async () => {
    const panel = await openRepo(unread("needsTerms"));
    expect(downloadButton(panel).disabled).toBe(true);
    within(panel).getByText("Accept this model's terms on its Hugging Face page first.");
    const page = within(panel).getByRole("link", { name: "Open the model page" });
    expect(page.getAttribute("href")).toBe(`https://huggingface.co/${GATED}`);
    await click(page);
    expect(openUrl).toHaveBeenCalledWith(`https://huggingface.co/${GATED}`);
    expect(within(panel).queryByRole("button", { name: "Download file anyway" })).toBeNull();
  });

  it("a download refused for a token or the terms says so plainly, with the next step", async () => {
    const panel = await openRepo();
    vi.mocked(api.studioLocalDownload).mockRejectedValue({
      code: "hf_needs_terms",
      message: "hf_needs_terms",
      nextStep: "",
      details: "",
    });
    await click(downloadButton(panel));
    within(panel).getByText("Accept this model's terms on its Hugging Face page first.");
    within(panel).getByRole("link", { name: "Open the model page" });
    // Never the code itself; Download stays, to try again once they are accepted.
    expect(within(panel).queryByText("hf_needs_terms")).toBeNull();
    expect(downloadButton(panel).disabled).toBe(false);
    vi.mocked(api.studioLocalDownload).mockRejectedValue({ code: "hf_needs_token", message: "x", nextStep: "", details: "" });
    await click(downloadButton(panel));
    within(panel).getByText("This model is gated. Add a Hugging Face token in Settings → Hugging Face first.");
    within(panel).getByRole("button", { name: "Open Settings" });
  });

  it("a readable gated repository says what its publisher asks, with its page", async () => {
    const panel = await openRepo(supported({ gate: { mode: "manual", collects: ["username", "email", "company"] } }));
    within(panel).getByText(
      "This model is gated: ask for access on its Hugging Face page first, and the publisher approves each request.",
    );
    within(panel).getByText("The publisher receives your Hugging Face username, email and company.");
    expect(within(panel).getByRole("link", { name: "Open the model page" }).getAttribute("href")).toBe(`https://huggingface.co/${REPO}`);
    expect(downloadButton(panel).disabled).toBe(false);
  });
});

describe("the repository's own licence (fix round 1)", () => {
  /** The panel's licence rows, in its Licences section. */
  function licenceRows(panel: HTMLElement): HTMLElement[] {
    return [...within(panel).getByRole("region", { name: "Licences" }).querySelectorAll<HTMLElement>("[data-licence]")];
  }
  const before = (a: HTMLElement, b: HTMLElement) =>
    expect(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

  it("a can't-run repository shows its licence beside Download file anyway", async () => {
    const lumina: FileLabel = { cannotRun: { reason: "lumina2", version: null } };
    const panel = await openRepo(
      supported({
        repo: "Alpha-VLLM/Lumina-Image-2.0",
        verdict: lumina,
        files: [],
        licence: apache,
        licences: [apache],
        fileViews: [view("consolidated.00-of-01.pth", 10_400_000_000, { cannotRun: { reason: "pickle", version: null } })],
      }),
    );
    const rows = licenceRows(panel);
    expect(rows).toHaveLength(1);
    expect(rows[0]!.getAttribute("data-licence")).toBe("green");
    within(rows[0]!).getByText("Free for any use, including commercial.");
    expect(within(rows[0]!).getByRole("link", { name: "Apache License 2.0" }).getAttribute("href")).toBe(apache.url);
    // Read before the file's plain download.
    before(rows[0]!, within(panel).getByRole("button", { name: "Download file anyway" }));
    // Nothing is made with it in Studio: no line about what you make.
    expect(within(panel).queryByText(/^Pictures you make/)).toBeNull();
  });

  it("a LoRA-only repository shows its licence beside Download LoRA", async () => {
    const loras = [view("pixel-art-xl-v1.1.safetensors", 170_000_000, { lora: { base: "sdxl" } })];
    const panel = await openRepo(
      supported({
        repo: "someone/pixel-art-xl-lora",
        verdict: { lora: { base: "sdxl" } },
        files: [],
        loras,
        fileViews: loras,
        licence: openrail,
        licences: [openrail],
      }),
    );
    const rows = licenceRows(panel);
    expect(rows).toHaveLength(1);
    expect(rows[0]!.getAttribute("data-licence")).toBe("amber");
    within(rows[0]!).getByText("Commercial use allowed, with use rules");
    expect(within(rows[0]!).getByRole("link", { name: "CreativeML Open RAIL-M" }).getAttribute("href")).toBe(openrail.url);
    before(rows[0]!, within(panel).getByRole("button", { name: "Download LoRA" }));
  });

  it("a repository whose plan parts share its licence lists it once; one of its own the plan lacks is added", async () => {
    // The repository and every part of the plan: Apache 2.0, one row.
    const panel = await openRepo();
    const rows = licenceRows(panel);
    expect(rows).toHaveLength(1);
    within(rows[0]!).getByRole("link", { name: "Apache License 2.0" });
    // The repository's own licence, named by none of the plan's parts: both, each once.
    cleanup();
    useStudio.setState(INITIAL, true);
    const both = await openRepo(supported({ licence: kommunity, licences: [kommunity, apache] }));
    expect(licenceRows(both).map((r) => within(r).getByRole("link").textContent)).toEqual([
      "Apache License 2.0",
      "Example Community Licence",
    ]);
    // The strictest of all of them says what you make may be used for.
    within(both).getByText("Pictures you make: commercial use only under $5M revenue.");
  });

  it("a repository that names no licence says so plainly", async () => {
    const lumina: FileLabel = { cannotRun: { reason: "lumina2", version: null } };
    const panel = await openRepo(
      supported({
        repo: "someone/unlabelled",
        verdict: lumina,
        files: [],
        licence: null,
        licences: [],
        fileViews: [view("model.safetensors", 5_000_000_000, lumina)],
      }),
    );
    const none = within(panel).getByText("This repository names no licence. Check the licence before you use what you make.");
    expect(none.closest("[data-licence]")!.getAttribute("data-licence")).toBe("none");
    before(none, within(panel).getByRole("button", { name: "Download file anyway" }));
  });

  it("a gated repository that could not be read shows its licence too, with one way to its page", async () => {
    const panel = await openRepo(
      supported({
        repo: "black-forest-labs/FLUX.1-Kontext-dev",
        files: [],
        licence: nonCommercial,
        licences: [nonCommercial],
        gate: { mode: "auto", collects: ["username"] },
        fileViews: [view("flux1-kontext-dev.safetensors", 23_800_000_000, { notRead: { reason: "needsTerms" } })],
      }),
    );
    const rows = licenceRows(panel);
    expect(rows).toHaveLength(1);
    expect(rows[0]!.getAttribute("data-licence")).toBe("red");
    within(rows[0]!).getByText("Only for non-commercial use.");
    before(rows[0]!, downloadButton(panel));
    // Its gated line says what to do; the form is not said twice.
    expect(within(panel).getAllByRole("link", { name: "Open the model page" })).toHaveLength(1);
  });
});

describe("LoRAs in a repository", () => {
  it("each LoRA has Download LoRA; one of an unknown model type says so, never that it may not fit (ruling R6-lora-none)", async () => {
    const LORA_REPO = "someone/pixel-art-xl-lora";
    const loras = [
      view("pixel-art-xl-v1.1.safetensors", 170_000_000, { lora: { base: "sdxl" } }),
      view("mystery-style.safetensors", 40_000_000, { lora: { base: null } }),
    ];
    const repo = supported({
      repo: LORA_REPO,
      summary: "A LoRA for pixel-art pictures.",
      verdict: { lora: { base: "sdxl" } },
      files: [],
      loras,
      fileViews: loras,
      licence: null,
      licences: [],
    });
    const panel = await openRepo(repo, { ...LOCAL, noticeAccepted: false });
    within(panel).getByText("LoRA for SDXL · 170 MB");
    within(panel).getByText("LoRA for an unknown model type · 40 MB");
    expect(within(panel).queryByText(/may not fit/)).toBeNull();
    // Not a model Studio can't run: no such box.
    expect(within(panel).queryByText(/Studio can't set this repository up/)).toBeNull();
    const buttons = within(panel).getAllByRole("button", { name: "Download LoRA" });
    expect(buttons).toHaveLength(2);
    // A LoRA downloads only once the note about local models was accepted, as a model does.
    await click(buttons[1]!);
    await click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "I understand" }));
    await waitFor(() => expect(api.studioLoraDownload).toHaveBeenCalledWith(LORA_REPO, COMMIT, "mystery-style.safetensors"));
  });
});

describe("a catalog model's detail panel", () => {
  it("Download on its card opens the panel: its file, the parts with it, the licence, the total and one Download", async () => {
    const panel = await openEntry();
    expect(useStudio.getState().detail).toEqual({ entryId: zimage.id });
    expect(api.studioLocalDownload).not.toHaveBeenCalled();
    within(panel).getByText("Photo-real pictures in 8 steps.");
    within(panel).getByText(`by ${zimage.publisher}`);
    within(panel).getByText("z_image_turbo-Q4_K.gguf");
    within(panel).getByText(formatBytes(zimage.parts[0]!.sizeBytes));
    within(panel).getByText("Diffusion model · Apache License 2.0");
    within(partRow(panel, "llm")).getByText(formatBytes(zimage.parts[1]!.sizeBytes));
    within(partRow(panel, "vae")).getByText("Decoder (VAE)");
    within(panel).getByText("Free for any use, including commercial.");
    within(panel).getByRole("link", { name: "Apache License 2.0" });
    within(panel).getByText("Pictures you make: free for any use, including commercial.");
    within(panel).getByText(`Total to download: ${formatBytes(zimage.toFetchBytes)}`);
    await click(downloadButton(panel));
    expect(api.studioLocalDownload).toHaveBeenCalledTimes(1);
    expect(api.studioLocalDownload).toHaveBeenCalledWith({ catalog: { entryId: zimage.id } }, null);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("the first Download shows the local-model notice first, and Cancel downloads nothing", async () => {
    const panel = await openEntry(zimage, { ...LOCAL, noticeAccepted: false });
    await click(downloadButton(panel));
    within(screen.getByRole("alertdialog")).getByText("Local models run only on your computer.");
    await click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(api.studioAcceptLocalNotice).not.toHaveBeenCalled();
    expect(api.studioLocalDownload).not.toHaveBeenCalled();
    screen.getByRole("dialog", { name: "Z-Image Turbo" });
  });

  it("a gated catalog model with no Hugging Face token has Download off, with Settings → Hugging Face", async () => {
    const gated: LocalEntry = { ...zimage, gate: { mode: "auto", collects: ["username"] } };
    // Its card says so in place of Download; opened from the store all the same, the panel does.
    await showLocal({ ...LOCAL, recommended: LOCAL.recommended.map((e) => (e.id === zimage.id ? gated : e)) });
    await act(async () => {
      await useStudio.getState().openDetail({ entryId: zimage.id });
    });
    await flush();
    const panel = screen.getByRole("dialog", { name: "Z-Image Turbo" });
    expect(downloadButton(panel).disabled).toBe(true);
    within(panel).getByText("This model is gated. Add a Hugging Face token in Settings → Hugging Face first.");
    within(panel).getByRole("button", { name: "Open Settings" });
  });

  it("a model that may not fit keeps Download, with the warning; low disk space and the engine say so", async () => {
    const local = { ...LOCAL, engine: { ...LOCAL.engine, state: "notInstalled" as const, downloadBytes: 30_303_000 } };
    const entry: LocalEntry = {
      ...zimage,
      fit: tooBig,
      lowDisk: true,
      needsEngine: true,
      toFetchBytes: 950_000_000,
      licences: [apache, { ...kommunity, excludedRegions: ["EU"] }],
    };
    const panel = await openEntry(entry, local);
    within(panel).getByText("May not fit this computer: it needs 8 GB of graphics memory. It may fail or be very slow.");
    within(panel).getByText("This leaves less than 10 GB free on this disk.");
    within(panel).getByText("Also downloads the Studio engine (29 MB, MIT licence).");
    within(panel).getByText(`Total to download: ${formatBytes(950_000_000 + 30_303_000)}`);
    within(panel).getByText("Example Community Licence excludes EU.");
    within(panel).getByText("Pictures you make: commercial use only under $5M revenue.");
    expect(downloadButton(panel).disabled).toBe(false);
  });
});
