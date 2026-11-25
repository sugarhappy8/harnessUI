// *Add a model file* (0.2.0 Task U4, design 7.8): the file chooser, then the file read and what
// it is in words, Copy or Use it where it is (the last choice first, not asked inside the models
// folder), the extra files and what they download, progress, and the model chosen once added. A
// LoRA file is added as a LoRA; a pickle file only says Studio can't read it (ruling S9: never a
// set-up, a copy or a use where it is); a set-up that needs a Hugging Face token says so plainly
// (S7). The note about local models comes first while it was never accepted.
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { LOCAL } from "./fixtures/models";
import type { ImportInspection, ImportProgress, InstalledLocal, LocalModels, Lora, SetupPlan, StudioPrefs } from "./types";

const ipc = vi.hoisted(() => ({ isTauri: () => true, listDownloads: vi.fn() }));
vi.mock("../../lib/ipc", () => ipc);
vi.mock("./api");
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const api = await import("./api");
const dialog = await import("@tauri-apps/plugin-dialog");
const { ImportDialog } = await import("./ImportDialog");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();
const sdxl = LOCAL.installed.find((m) => m.version === "sdxl")!;
const PATH = "/home/me/Downloads/sd_xl_base_1.0.safetensors";

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
    localNoticeAcceptedAt: 1,
    lastImportMode: null,
    ...patch,
  };
}

/** SDXL's set-up from this computer: the file itself, and a decoder to download. */
function plan(patch: Partial<SetupPlan> = {}): SetupPlan {
  const main = { role: "model" as const, repo: "", path: PATH, sizeBytes: 6_938_078_334, licenceId: "", alreadyHere: true, local: true };
  const vae = {
    role: "vae" as const,
    repo: "madebyollin/sdxl-vae-fp16-fix",
    path: "sdxl_vae.safetensors",
    sizeBytes: 334_641_164,
    licenceId: "mit",
    alreadyHere: false,
    local: false,
  };
  return {
    recipe: { id: "sdxl-checkpoint", name: "SDXL", kind: "picture" },
    recipes: [{ id: "sdxl-checkpoint", name: "SDXL", kind: "picture" }],
    slots: [
      { role: "model", required: true, source: "main", part: main },
      { role: "vae", required: false, source: "companion", part: vae },
    ],
    licences: [],
    totalBytes: main.sizeBytes + vae.sizeBytes,
    toFetchBytes: vae.sizeBytes,
    fit: sdxl.fit,
    canDownload: true,
    lowDisk: false,
    needsEngine: false,
    untested: false,
    ...patch,
  };
}

/** What `studio_import_inspect` says of an SDXL file at `path`. */
function inspection(patch: Partial<ImportInspection> = {}): ImportInspection {
  return {
    path: PATH,
    name: "sd_xl_base_1.0.safetensors",
    sizeBytes: 6_938_078_334,
    label: { works: { version: "sdxl", recipe: "sdxl-checkpoint", kind: "picture", part: null, parameters: 3.5e9, folder: false } },
    plan: plan(),
    lastMode: "copy",
    insideModels: false,
    ...patch,
  };
}

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

async function load(local: LocalModels = LOCAL) {
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [] });
  vi.mocked(api.studioLocalModels).mockResolvedValue(local);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs: [], next: null });
  vi.mocked(api.studioLive).mockResolvedValue([]);
  vi.mocked(api.studioGetPrefs).mockResolvedValue(prefs());
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

/** The dialog, opened for `kind` with `path` chosen in the file chooser, from a button that
 *  has focus (as Get models' *Add a model file* would). */
async function openFor(kind: "model" | "lora", path: string | null = PATH) {
  vi.mocked(dialog.open).mockResolvedValue(path);
  await act(async () => {
    render(
      <>
        <button type="button">Opener</button>
        <ImportDialog />
      </>,
    );
  });
  await act(async () => {
    screen.getByRole("button", { name: "Opener" }).focus();
    useStudio.getState().openImport(kind);
  });
  await flush();
}

function theDialog(): HTMLElement {
  return screen.getByRole("dialog");
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
  await flush();
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  ipc.listDownloads.mockResolvedValue([]);
  vi.mocked(api.onImportProgress).mockResolvedValue(() => undefined);
});

afterEach(() => {
  cleanup();
});

describe("ImportDialog", () => {
  it("a picked file is read and its type shown, with Copy preselected from the last choice", async () => {
    await load();
    vi.mocked(api.studioImportInspect).mockResolvedValue(inspection({ lastMode: "copy" }));
    await openFor("model");
    expect(dialog.open).toHaveBeenCalledTimes(1);
    expect(api.studioImportInspect).toHaveBeenCalledWith(PATH);
    const d = theDialog();
    within(d).getByRole("heading", { name: "Add a model file" });
    within(d).getByText("sd_xl_base_1.0.safetensors");
    within(d).getByText("6.9 GB · Works in Studio · SDXL picture model, 3.5B");
    const how = within(d).getByRole("radiogroup", { name: "How to add it" });
    expect(within(how).getByRole("radio", { name: "Copy it into Studio's models folder" }).getAttribute("aria-checked")).toBe(
      "true",
    );
    expect(within(how).getByRole("radio", { name: "Use it where it is" }).getAttribute("aria-checked")).toBe("false");
    // The extra file its set-up adds, and what that downloads.
    const extra = within(d).getByRole("region", { name: "Extra files" });
    within(extra).getByText("Decoder (VAE)");
    within(extra).getByText("Also downloads 335 MB from Hugging Face");
    cleanup();
    // The last choice was to use a file where it is: that comes first.
    useStudio.getState().openImport(null);
    vi.mocked(api.studioImportInspect).mockResolvedValue(inspection({ lastMode: "inPlace" }));
    await openFor("model");
    expect(screen.getByRole("radio", { name: "Use it where it is" }).getAttribute("aria-checked")).toBe("true");
  });

  it("a file inside the models folder skips the question", async () => {
    await load();
    vi.mocked(api.studioImportInspect).mockResolvedValue(inspection({ insideModels: true, lastMode: "copy" }));
    vi.mocked(api.studioImportModel).mockResolvedValue({ genModelId: sdxl.id, downloadGroup: null });
    await openFor("model");
    expect(screen.queryByRole("radiogroup")).toBeNull();
    await click(within(theDialog()).getByRole("button", { name: "Add" }));
    expect(api.studioImportModel).toHaveBeenCalledWith(PATH, "inPlace", null);
  });

  it("a LoRA file is added as a LoRA", async () => {
    await load();
    const lora: Lora = {
      id: "local:" + "c".repeat(64),
      name: "pixel-art-xl",
      base: "sdxl",
      format: "kohya",
      sizeBytes: 170_000_000,
      source: "import",
      repo: null,
      missing: false,
    };
    vi.mocked(api.studioImportInspect).mockResolvedValue(
      inspection({ path: "/m/pixel-art-xl.safetensors", name: "pixel-art-xl.safetensors", sizeBytes: 170_000_000, label: { lora: { base: "sdxl" } }, plan: null }),
    );
    vi.mocked(api.studioLoraImport).mockResolvedValue(lora);
    // Chosen with Add a model file: it is still a LoRA.
    await openFor("model", "/m/pixel-art-xl.safetensors");
    const d = theDialog();
    within(d).getByText("170 MB · LoRA for SDXL");
    within(d).getByText("It is added to your LoRAs.");
    expect(within(d).queryByRole("radiogroup")).toBeNull();
    await click(within(d).getByRole("button", { name: "Add" }));
    expect(api.studioLoraImport).toHaveBeenCalledWith("/m/pixel-art-xl.safetensors");
    expect(api.studioImportModel).not.toHaveBeenCalled();
    within(theDialog()).getByText("Added");
    expect(useStudio.getState().loras.map((l) => l.id)).toEqual([lora.id]);
    cleanup();
    // From Add a LoRA file, the dialog says so.
    useStudio.getState().openImport(null);
    await openFor("lora", "/m/pixel-art-xl.safetensors");
    within(theDialog()).getByRole("heading", { name: "Add a LoRA file" });
    expect(vi.mocked(dialog.open).mock.calls.at(-1)![0]).toMatchObject({ filters: [{ name: "Add a LoRA file" }] });
  });

  it("a .ckpt file says Studio can't read .ckpt files", async () => {
    await load();
    // The backend names the file's own extension (ruling S9-pickle-ext).
    for (const ext of [".ckpt", ".pt"]) {
      vi.mocked(api.studioImportInspect).mockRejectedValueOnce({
        code: "bad_request",
        message: `Studio can't read ${ext} files`,
        nextStep: "Choose a .safetensors, .sft or .gguf file.",
        details: "",
      });
      await openFor("model", `/m/old-model${ext}`);
      const d = theDialog();
      within(d).getByText(`Studio can't read ${ext} files`);
      within(d).getByText("Choose a .safetensors, .sft or .gguf file.");
      // Never a set-up, a copy or a use where it is: only another file.
      expect(within(d).queryByRole("radiogroup")).toBeNull();
      expect(within(d).queryByRole("radio")).toBeNull();
      expect(within(d).queryByRole("button", { name: "Add" })).toBeNull();
      expect(within(d).queryByRole("region", { name: "Extra files" })).toBeNull();
      within(d).getByRole("button", { name: "Choose another file" });
      cleanup();
      useStudio.getState().openImport(null);
    }
    // Read as a pickle file all the same: its label says so, and nothing else is offered.
    vi.mocked(api.studioImportInspect).mockResolvedValue(
      inspection({ path: "/m/model.pth", name: "model.pth", label: { cannotRun: { reason: "pickle", version: null } }, plan: null }),
    );
    await openFor("model", "/m/model.pth");
    const d = theDialog();
    within(d).getByText(/Studio can't read \.pth files/);
    expect(within(d).queryByRole("radio")).toBeNull();
    expect(within(d).queryByRole("button", { name: "Add" })).toBeNull();
    // Another file is read in its place.
    vi.mocked(api.studioImportInspect).mockResolvedValue(inspection());
    vi.mocked(dialog.open).mockResolvedValue(PATH);
    await click(within(d).getByRole("button", { name: "Choose another file" }));
    within(theDialog()).getByRole("radiogroup", { name: "How to add it" });
  });

  it("a set-up that needs a Hugging Face token says so in place of the total to download", async () => {
    await load();
    vi.mocked(api.studioImportInspect).mockResolvedValue(inspection());
    vi.mocked(api.studioImportModel).mockRejectedValue({
      code: "hf_needs_token",
      message: "hf_needs_token",
      nextStep: "",
      details: "",
    });
    await openFor("model");
    await click(within(theDialog()).getByRole("button", { name: "Add" }));
    const extra = within(theDialog()).getByRole("region", { name: "Extra files" });
    within(extra).getByText("This model is gated. Add a Hugging Face token in Settings → Hugging Face first.");
    expect(within(extra).queryByText(/^Also downloads/)).toBeNull();
    // The code itself never shows.
    expect(theDialog().textContent).not.toContain("hf_needs_token");
    // A set-up with a part that has no file says so too, and cannot be added.
    cleanup();
    useStudio.getState().openImport(null);
    vi.mocked(api.studioImportInspect).mockResolvedValue(inspection({ plan: plan({ canDownload: false }) }));
    await openFor("model");
    within(theDialog()).getByText("Choose a file for each part first.");
    expect((within(theDialog()).getByRole("button", { name: "Add" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("progress shows while the file is read, then the model is selected", async () => {
    const added: InstalledLocal = { ...sdxl, id: "local:ddd", model: { ...sdxl.model, id: "local:ddd", displayName: "sd_xl_base_1.0" } };
    await load();
    vi.mocked(api.studioImportInspect).mockResolvedValue(inspection());
    let report: (p: ImportProgress) => void = () => undefined;
    vi.mocked(api.onImportProgress).mockImplementation(async (handler) => {
      report = handler;
      return () => undefined;
    });
    let finish: (r: { genModelId: string | null; downloadGroup: string | null }) => void = () => undefined;
    vi.mocked(api.studioImportModel).mockImplementation(() => new Promise((resolve) => (finish = resolve)));
    await openFor("model");
    expect(theDialog().contains(document.activeElement)).toBe(true);
    // Use it where it is: the file is read through for its checksum.
    await click(screen.getByRole("radio", { name: "Use it where it is" }));
    await click(within(theDialog()).getByRole("button", { name: "Add" }));
    expect(api.studioImportModel).toHaveBeenCalledWith(PATH, "inPlace", null);
    await act(async () => {
      report({ path: PATH, stage: "hashing", doneBytes: 2_000_000_000, totalBytes: 6_938_078_334 });
    });
    const bar = within(theDialog()).getByRole("progressbar", { name: "Reading the file…" });
    expect(bar.getAttribute("aria-valuenow")).toBe("29");
    within(theDialog()).getByText("2.0 GB of 6.9 GB");
    // While it is added, the dialog stays open.
    expect((within(theDialog()).getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(true);
    await act(async () => {
      report({ path: PATH, stage: "copying", doneBytes: 3_000_000_000, totalBytes: 6_938_078_334 });
    });
    within(theDialog()).getByRole("progressbar", { name: "Copying…" });
    vi.mocked(api.studioLocalModels).mockResolvedValue({ ...LOCAL, installed: [added, ...LOCAL.installed] });
    await act(async () => {
      finish({ genModelId: "local:ddd", downloadGroup: null });
    });
    await flush();
    within(theDialog()).getByText("Added");
    expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: "local:ddd" });
    expect(useStudio.getState().importProgress).toBeNull();
    await click(within(theDialog()).getByRole("button", { name: "Done" }));
    expect(useStudio.getState().importOpen).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();
    // Focus goes back to what opened it.
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: "Opener" })));
  });

  it("the note about local models comes first while it was never accepted", async () => {
    await load({ ...LOCAL, noticeAccepted: false });
    vi.mocked(api.studioImportInspect).mockResolvedValue(inspection());
    vi.mocked(api.studioAcceptLocalNotice).mockResolvedValue(prefs());
    vi.mocked(api.studioImportModel).mockResolvedValue({ genModelId: null, downloadGroup: "group-1" });
    await openFor("model");
    await click(within(theDialog()).getByRole("button", { name: "Add" }));
    expect(api.studioImportModel).not.toHaveBeenCalled();
    await click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "I understand" }));
    expect(api.studioImportModel).toHaveBeenCalledWith(PATH, "copy", null);
    // Its extra file downloads before the model can be used.
    within(theDialog()).getByText("Its extra files are downloading; the model is ready when they finish.");
  });

  it("the file chooser opens once for each opening, under strict mode too", async () => {
    await load();
    vi.mocked(api.studioImportInspect).mockResolvedValue(inspection());
    vi.mocked(dialog.open).mockResolvedValue(PATH);
    // Already open as it is drawn (Studio drawn again while it was open): strict mode runs its
    // effects twice.
    useStudio.getState().openImport("model");
    await act(async () => {
      render(
        <StrictMode>
          <ImportDialog />
        </StrictMode>,
      );
    });
    await flush();
    expect(dialog.open).toHaveBeenCalledTimes(1);
    within(theDialog()).getByRole("radiogroup", { name: "How to add it" });
    await click(within(theDialog()).getByRole("button", { name: "Cancel" }));
    await act(async () => {
      useStudio.getState().openImport("model");
    });
    await flush();
    expect(dialog.open).toHaveBeenCalledTimes(2);
  });

  it("closed without a file, the file chooser closes the dialog", async () => {
    await load();
    await openFor("model", null);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(useStudio.getState().importOpen).toBeNull();
    expect(api.studioImportInspect).not.toHaveBeenCalled();
  });
});
