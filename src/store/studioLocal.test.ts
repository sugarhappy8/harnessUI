// Studio's model discovery, set-up and import actions (0.2.0 Task S12): Hugging Face search
// with filters, opened repositories, downloads with a set-up choice, a model file added from
// this computer with its progress, and LoRAs.
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LOCAL } from "../components/studio/fixtures/models";
import type {
  ImportProgress,
  LocalRepo,
  LocalSearchHit,
  LocalSearchPage,
  Lora,
  SetupChoice,
} from "../components/studio/types";

const ipc = vi.hoisted(() => ({ isTauri: () => true, listDownloads: vi.fn() }));
vi.mock("../lib/ipc", () => ipc);
vi.mock("../components/studio/api");
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const api = await import("../components/studio/api");
const dialog = await import("@tauri-apps/plugin-dialog");
const { useStudio } = await import("./studio");

const INITIAL = useStudio.getState();

function hit(repo: string, patch: Partial<LocalSearchHit> = {}): LocalSearchHit {
  return {
    repo,
    author: repo.split("/")[0]!,
    downloads: 10,
    likes: 1,
    updatedAt: null,
    gated: "none",
    licenseLabel: null,
    verdict: "unknown",
    exact: false,
    ...patch,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  ipc.listDownloads.mockResolvedValue([]);
  vi.mocked(api.studioLocalModels).mockResolvedValue(LOCAL);
});

describe("Get models' filter and Fits this computer (Task U5)", () => {
  it("the filter starts from the tab each time the view opens; Fits this computer is off by default", () => {
    expect(useStudio.getState()).toMatchObject({ searchFilter: "pictures", fitsOnly: false });
    useStudio.getState().setTab("video");
    useStudio.getState().showLocalModels(true);
    expect(useStudio.getState()).toMatchObject({ localView: true, searchFilter: "video" });
    useStudio.getState().setSearchFilter("loras");
    useStudio.getState().setFitsOnly(true);
    useStudio.getState().showLocalModels(false);
    // The tab stays as it was, and the switch as it was left.
    expect(useStudio.getState()).toMatchObject({ localView: false, tab: "video", fitsOnly: true });
    useStudio.getState().setTab("image");
    useStudio.getState().showLocalModels(true);
    expect(useStudio.getState().searchFilter).toBe("pictures");
  });
});

describe("search", () => {
  it("search with a filter keeps only the last answer", async () => {
    let answerFirst: (page: LocalSearchPage) => void = () => undefined;
    vi.mocked(api.studioLocalSearch)
      .mockImplementationOnce(() => new Promise((resolve) => (answerFirst = resolve)))
      .mockResolvedValueOnce({ items: [hit("someone/pixel-lora")] });
    const first = useStudio.getState().search("klein", "pictures");
    await useStudio.getState().search("pixel", "loras");
    answerFirst({ items: [hit("someone/picture-GGUF")] });
    await first;
    expect(api.studioLocalSearch).toHaveBeenNthCalledWith(1, "klein", "pictures");
    expect(api.studioLocalSearch).toHaveBeenNthCalledWith(2, "pixel", "loras");
    expect(useStudio.getState().localSearch).toEqual({
      text: "pixel",
      filter: "loras",
      hits: [hit("someone/pixel-lora")],
    });
  });

  it("an empty search asks for the most downloaded", async () => {
    vi.mocked(api.studioLocalSearch).mockResolvedValue({ items: [] });
    await useStudio.getState().search("", "video");
    expect(api.studioLocalSearch).toHaveBeenCalledWith("", "video");
    expect(useStudio.getState().localSearch).toEqual({ text: "", filter: "video", hits: [] });
  });

  it("opening a repo makes its hit's verdict exact", async () => {
    const verdict = { works: { version: "sdxl", kind: "picture" } } as const;
    const repo = { repo: "someone/sdxl-GGUF", commit: "c".repeat(40), verdict } as unknown as LocalRepo;
    vi.mocked(api.studioLocalSearch).mockResolvedValue({
      items: [hit("someone/sdxl-GGUF"), hit("someone/other")],
    });
    vi.mocked(api.studioLocalRepo).mockResolvedValue(repo);
    await useStudio.getState().search("sdxl", "pictures");
    await expect(useStudio.getState().openRepo("someone/sdxl-GGUF")).resolves.toBe(repo);
    const { localSearch, repos } = useStudio.getState();
    expect(localSearch?.hits).toEqual([hit("someone/sdxl-GGUF", { verdict, exact: true }), hit("someone/other")]);
    expect(repos).toEqual({ "someone/sdxl-GGUF": repo });
  });
});

describe("the detail panel (Task U6)", () => {
  it("openDetail shows a catalog model at once, and reads a repository with openRepo; closeDetail closes it", async () => {
    await useStudio.getState().openDetail({ entryId: "z-image-turbo-q4" });
    expect(useStudio.getState().detail).toEqual({ entryId: "z-image-turbo-q4" });
    expect(api.studioLocalRepo).not.toHaveBeenCalled();
    const repo = { repo: "someone/sdxl-GGUF", commit: "c".repeat(40), verdict: "unknown" } as unknown as LocalRepo;
    vi.mocked(api.studioLocalRepo).mockResolvedValue(repo);
    const opening = useStudio.getState().openDetail({ repo: "someone/sdxl-GGUF" });
    // The panel opens before the repository is read.
    expect(useStudio.getState().detail).toEqual({ repo: "someone/sdxl-GGUF" });
    await opening;
    expect(api.studioLocalRepo).toHaveBeenCalledWith("someone/sdxl-GGUF");
    expect(useStudio.getState().repos["someone/sdxl-GGUF"]).toBe(repo);
    useStudio.getState().closeDetail();
    expect(useStudio.getState()).toMatchObject({ detail: null, detailError: null });
  });

  it("a repository that could not be read is said in the panel, never as a rejection; leaving Get models closes it", async () => {
    const offline = { code: "hf_unreachable", message: "Hugging Face could not be reached.", nextStep: "Try again.", details: "" };
    vi.mocked(api.studioLocalRepo).mockRejectedValue(offline);
    await expect(useStudio.getState().openDetail({ repo: "someone/gone" })).resolves.toBeUndefined();
    expect(useStudio.getState()).toMatchObject({ detail: { repo: "someone/gone" }, detailError: offline });
    // Another opened since: a late failure is not said there.
    let fail: (e: unknown) => void = () => undefined;
    vi.mocked(api.studioLocalRepo).mockReturnValue(new Promise((_, reject) => (fail = reject)));
    const late = useStudio.getState().openDetail({ repo: "someone/slow" });
    expect(useStudio.getState().detailError).toBeNull();
    await useStudio.getState().openDetail({ entryId: "z-image-turbo-q4" });
    fail(offline);
    await late;
    expect(useStudio.getState()).toMatchObject({ detail: { entryId: "z-image-turbo-q4" }, detailError: null });
    useStudio.getState().showLocalModels(true);
    expect(useStudio.getState().detail).toBeNull();
    await useStudio.getState().openDetail({ entryId: "z-image-turbo-q4" });
    useStudio.getState().showLocalModels(false);
    expect(useStudio.getState().detail).toBeNull();
  });
});

describe("setting a model up", () => {
  const source = { search: { repo: "someone/Flux-GGUF", commit: "c".repeat(40), file: "flux.gguf" } };
  const choice: SetupChoice = { recipe: "flux1-dev", overrides: [] };

  it("setupPlan passes the source and the choice", async () => {
    await useStudio.getState().setupPlan(source, choice);
    expect(api.studioSetupPlan).toHaveBeenCalledWith(source, choice);
  });

  it("downloadSetup passes the choice and refreshes models and downloads", async () => {
    vi.mocked(api.studioLocalDownload).mockResolvedValue("group-1");
    await expect(useStudio.getState().downloadSetup(source, choice)).resolves.toBe("group-1");
    expect(api.studioLocalDownload).toHaveBeenCalledWith(source, choice);
    expect(ipc.listDownloads).toHaveBeenCalledTimes(1);
    expect(api.studioLocalModels).toHaveBeenCalledTimes(1);
    expect(useStudio.getState().local).toBe(LOCAL);
  });

  it("downloadSetup without a choice sends null; a read that fails leaves a trace only", async () => {
    vi.mocked(api.studioLocalDownload).mockResolvedValue("group-2");
    ipc.listDownloads.mockRejectedValue(new Error("busy"));
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    try {
      await expect(useStudio.getState().downloadSetup(source, null)).resolves.toBe("group-2");
      expect(api.studioLocalDownload).toHaveBeenCalledWith(source, null);
      expect(warn).toHaveBeenCalled();
    } finally {
      warn.mockRestore();
    }
  });

  it("repairLocal gives the download group, or null when the model was checked at once", async () => {
    vi.mocked(api.studioLocalDownload).mockResolvedValue("group-3");
    await expect(useStudio.getState().repairLocal("m1")).resolves.toBe("group-3");
    expect(api.studioLocalDownload).toHaveBeenCalledWith({ installed: { genModelId: "m1" } }, null);
    expect(api.studioLocalModels).not.toHaveBeenCalled();
    // Every file on this computer (a model added from a file): nothing downloads, and the
    // local models are read again.
    vi.mocked(api.studioLocalDownload).mockResolvedValue(null);
    await expect(useStudio.getState().repairLocal("m1")).resolves.toBeNull();
    expect(api.studioLocalModels).toHaveBeenCalledTimes(1);
  });

  it("setModelFiles sets the model up and reads the local models again", async () => {
    const result = { files: {}, downloadGroup: null } as never;
    vi.mocked(api.studioSetModelFiles).mockResolvedValue(result);
    await expect(useStudio.getState().setModelFiles("m1", choice)).resolves.toBe(result);
    expect(api.studioSetModelFiles).toHaveBeenCalledWith("m1", choice);
    expect(api.studioLocalModels).toHaveBeenCalledTimes(1);
  });
});

describe("adding a file from this computer", () => {
  it("the chooser offers .safetensors, .sft and .gguf and gives the path, or null", async () => {
    vi.mocked(dialog.open).mockResolvedValueOnce("/home/me/model.gguf").mockResolvedValueOnce(null);
    await expect(useStudio.getState().chooseFile("model")).resolves.toBe("/home/me/model.gguf");
    expect(dialog.open).toHaveBeenCalledWith(
      expect.objectContaining({
        multiple: false,
        filters: [{ name: "Add a model file", extensions: ["safetensors", "sft", "gguf"] }],
      }),
    );
    await expect(useStudio.getState().chooseFile("lora")).resolves.toBeNull();
    expect(vi.mocked(dialog.open).mock.calls[1]![0]).toMatchObject({
      filters: [{ name: "Add a LoRA file" }],
    });
  });

  it("import progress events update importProgress and clear at the end", async () => {
    let heard: (p: ImportProgress) => void = () => undefined;
    const unlisten = vi.fn();
    vi.mocked(api.onImportProgress).mockImplementation(async (handler) => {
      heard = handler;
      return unlisten;
    });
    let finish: (r: { genModelId: string; downloadGroup: null }) => void = () => undefined;
    vi.mocked(api.studioImportModel).mockImplementation(() => new Promise((resolve) => (finish = resolve)));
    const added = useStudio.getState().importModel("/m/a.gguf", "copy", null);
    await vi.waitFor(() => expect(api.studioImportModel).toHaveBeenCalledWith("/m/a.gguf", "copy", null));
    const hashing: ImportProgress = { path: "/m/a.gguf", stage: "hashing", doneBytes: 5, totalBytes: 10 };
    heard(hashing);
    expect(useStudio.getState().importProgress).toEqual(hashing);
    const copying: ImportProgress = { ...hashing, stage: "copying", doneBytes: 10 };
    heard(copying);
    expect(useStudio.getState().importProgress).toEqual(copying);
    finish({ genModelId: "m1", downloadGroup: null });
    await added;
    expect(useStudio.getState().importProgress).toBeNull();
    expect(unlisten).toHaveBeenCalledTimes(1);
    // An event heard after the end is dropped.
    heard(hashing);
    expect(useStudio.getState().importProgress).toBeNull();
    // The local models and downloads were read again.
    expect(api.studioLocalModels).toHaveBeenCalledTimes(1);
  });

  it("progress clears when the import fails, and the failure rejects", async () => {
    let heard: (p: ImportProgress) => void = () => undefined;
    vi.mocked(api.onImportProgress).mockImplementation(async (handler) => {
      heard = handler;
      return () => undefined;
    });
    vi.mocked(api.studioImportModel).mockImplementation(async () => {
      heard({ path: "/m/a.gguf", stage: "copying", doneBytes: 1, totalBytes: 2 });
      throw new Error("disk full");
    });
    await expect(useStudio.getState().importModel("/m/a.gguf", "copy", null)).rejects.toThrow("disk full");
    expect(useStudio.getState().importProgress).toBeNull();
  });
});

describe("LoRAs", () => {
  const lora = (id: string, name: string, base: Lora["base"] = "sdxl"): Lora => ({
    id,
    name,
    base,
    format: "kohya",
    sizeBytes: 3,
    source: "import",
    repo: null,
    missing: false,
  });

  it("loadLoras reads the list; importLora adds the LoRA first, once, with progress", async () => {
    vi.mocked(api.studioLoras).mockResolvedValue([lora("a", "a"), lora("b", "b")]);
    await useStudio.getState().loadLoras();
    expect(useStudio.getState().loras.map((l) => l.id)).toEqual(["a", "b"]);
    vi.mocked(api.onImportProgress).mockResolvedValue(() => undefined);
    vi.mocked(api.studioLoraImport).mockResolvedValue(lora("b", "b"));
    useStudio.setState({ local: { ...LOCAL, loras: [lora("a", "a")] } });
    await useStudio.getState().importLora("/l/b.safetensors");
    expect(useStudio.getState().loras.map((l) => l.id)).toEqual(["b", "a"]);
    expect(useStudio.getState().local?.loras.map((l) => l.id)).toEqual(["b", "a"]);
    expect(useStudio.getState().importProgress).toBeNull();
  });

  it("deleteLora removes it; downloadLora gives the group and reads the downloads", async () => {
    useStudio.setState({ loras: [lora("a", "a"), lora("b", "b")], local: { ...LOCAL, loras: [lora("a", "a"), lora("b", "b")] } });
    vi.mocked(api.studioLoraDelete).mockResolvedValue(undefined);
    await useStudio.getState().deleteLora("a");
    expect(api.studioLoraDelete).toHaveBeenCalledWith("a");
    expect(useStudio.getState().loras.map((l) => l.id)).toEqual(["b"]);
    // Get models lists the local models' own copy: it goes there too.
    expect(useStudio.getState().local?.loras.map((l) => l.id)).toEqual(["b"]);
    vi.mocked(api.studioLoraDownload).mockResolvedValue("g");
    await expect(useStudio.getState().downloadLora("o/r", "c".repeat(40), "x.safetensors")).resolves.toBe("g");
    expect(ipc.listDownloads).toHaveBeenCalledTimes(1);
  });
});
