// Get local models, Studio's own view (0.1.0 Task B13, local addendum Wireframe L2): what this
// computer has, the recommended models with their size, licence in plain words, fit and time,
// the models that do not fit folded away, the installed ones (also those from a search) with
// Download again and Delete, Hugging Face search only when asked, and the footer's engine line
// and content notice. Download only sets the offer (Task B14's dialog is tested in
// DownloadLocalModel.test.tsx); a repair refused for a licence shows that dialog.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, screen, within } from "@testing-library/react";
import type { DownloadGroup } from "../../lib/types";
import { amountGb, LOCAL_MODELS } from "../../lib/words";
import { LOCAL } from "./fixtures/models";
import type {
  InstalledLocal,
  Licence,
  LocalEntry,
  LocalModels,
  LocalRepo,
  LocalSearchPage,
  Part,
} from "./types";

const ipc = vi.hoisted(() => ({
  isTauri: () => true,
  listDownloads: vi.fn<() => Promise<DownloadGroup[]>>(),
  onDownloadProgress: vi.fn<(handler: (g: DownloadGroup) => void) => Promise<() => void>>(),
}));
vi.mock("../../lib/ipc", () => ipc);
vi.mock("./api");

const api = await import("./api");
const { click, flush, showLocal } = await import("./testing/local");
const { useDownloads } = await import("../../store/downloads");
const { hardwareLine, visibleEntries } = await import("./LocalModels");
const { activeDownload, licenceBadge, offerForEntry, offerForFile } = await import("./localOffers");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();
const GIB = 1024 ** 3;

const klein = LOCAL.recommended.find((e) => e.id === "flux2-klein-4b-q4")!;
const zimage = LOCAL.recommended.find((e) => e.id === "z-image-turbo-q4")!;
const wan = LOCAL.recommended.find((e) => e.id === "wan22-ti2v-5b-fast-q6")!;
const apache = zimage.licence;

/** A licence of another kind than Apache-2.0's. */
function licence(patch: Partial<Licence>): Licence {
  return { ...apache, ...patch };
}

const nonCommercial = licence({
  id: "nc",
  name: "Non-Commercial Licence",
  sha256: "b".repeat(64),
  badge: "red",
  commercial: "no",
  plainWords: "Only for non-commercial use.",
});

/** The searched Z-Image GGUF's file: its three parts, the prompt reader already here from
 *  another model. */
const parts: Part[] = [
  { ...zimage.parts[0]!, repo: "someone/Z-Image-Turbo-GGUF", path: "z-image-turbo-Q5_K_M.gguf", sizeBytes: Math.round(4.3 * GIB) },
  { ...zimage.parts[1]!, sizeBytes: Math.round(2.5 * GIB), alreadyHere: true },
  { ...zimage.parts[2]!, sizeBytes: Math.round(0.3 * GIB) },
];

function supported(patch: Partial<LocalRepo> = {}): LocalRepo {
  return {
    repo: "someone/Z-Image-Turbo-GGUF",
    commit: "c".repeat(40),
    status: "supported",
    reason: null,
    family: "Z-Image Turbo",
    files: [
      {
        file: "z-image-turbo-Q5_K_M.gguf",
        quant: "Q5_K_M",
        parts,
        licences: [apache],
        totalBytes: parts.reduce((n, p) => n + p.sizeBytes, 0),
        toFetchBytes: parts[0]!.sizeBytes + parts[2]!.sizeBytes,
        fit: { ...zimage.fit, verdict: "green" },
        needsEngine: false,
        lowDisk: false,
      },
    ],
    licence: apache,
    licences: [apache],
    labelMismatch: null,
    gate: { mode: "none", collects: [] },
    ...patch,
  };
}

/** The backend's refusal of a download whose licences were not all accepted. */
const NOT_ACCEPTED = {
  code: "studio_license_not_accepted",
  message: "Accept the model's licence first.",
  nextStep: "Download it from Get local models.",
  details: "",
};

function typeIn(el: HTMLElement, text: string) {
  fireEvent.change(el, { target: { value: text } });
}

function searchBox(): HTMLElement {
  return screen.getByRole("searchbox", { name: "Search Hugging Face" });
}

/** Searches Hugging Face for `repo`'s name, finds it, and opens it (`studioLocalRepo` gives
 *  `result`). Returns the opened result. */
async function openResult(repo: string, result: LocalRepo = supported({ repo })) {
  vi.mocked(api.studioLocalSearch).mockResolvedValue({
    items: [{ repo, downloads: 12_345, likes: 10, gated: "none", licenseLabel: "apache-2.0" }],
  });
  vi.mocked(api.studioLocalRepo).mockResolvedValue(result);
  typeIn(searchBox(), repo.split("/")[1]!);
  await act(async () => {
    fireEvent.keyDown(searchBox(), { key: "Enter" });
  });
  await flush();
  await click(screen.getByRole("button", { name: repo }));
  expect(api.studioLocalRepo).toHaveBeenCalledWith(repo);
  return screen.getByRole("region", { name: repo });
}

function row(name: RegExp | string): HTMLElement {
  return screen.getByRole("article", { name });
}

/** What the download manager's `download-progress` event tells the view, once it listens. */
let progress: (g: DownloadGroup) => void = () => undefined;

/** Z-Image Turbo's download, as the download manager reports it: a third done. */
function group(patch: Partial<DownloadGroup> = {}): DownloadGroup {
  return {
    id: "group-1",
    repo: "leejet/Z-Image-Turbo-GGUF",
    label: "Z-Image Turbo",
    fileCount: 3,
    bytesDone: 2 * GIB,
    bytesTotal: 6 * GIB,
    status: "downloading",
    error: null,
    bytesPerSecond: null,
    unverified: false,
    checking: false,
    generatorKind: "image",
    genModelId: "z-image-turbo-q4",
    ...patch,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  useDownloads.setState({ groups: [] });
  ipc.listDownloads.mockResolvedValue([]);
  ipc.onDownloadProgress.mockImplementation((handler) => {
    progress = handler;
    return Promise.resolve(() => undefined);
  });
});

afterEach(cleanup);

describe("hardwareLine", () => {
  it("says what this computer has, as the header shows it", () => {
    expect(hardwareLine(LOCAL.hardware)).toBe(
      "This computer: Radeon RX 7600, 8 GB graphics memory · 32 GB memory · 412 GB free",
    );
  });

  it("on shared memory, the computer's memory and never the graphics working set", () => {
    const mac = {
      device: "Apple M5",
      graphicsBytes: Math.round(10.7 * GIB),
      sharedMemory: true,
      ramBytes: 16 * GIB,
      freeDiskBytes: 180 * GIB,
    };
    expect(hardwareLine(mac)).toBe("This computer: Apple M5, 16 GB shared memory · 180 GB free");
    expect(hardwareLine({ ...mac, ramBytes: null })).toBe("This computer: Apple M5 · 180 GB free");
  });

  it("without a graphics card, the processor only", () => {
    expect(
      hardwareLine({ device: "Processor", graphicsBytes: null, sharedMemory: false, ramBytes: 16 * GIB, freeDiskBytes: 180 * GIB }),
    ).toBe("This computer: processor only · 16 GB memory · 180 GB free");
  });
});

describe("licenceBadge", () => {
  it("says what a licence allows in plain words, with its colour", () => {
    expect(licenceBadge(apache)).toEqual({ tone: "green", text: "Free for any use, including commercial" });
    expect(licenceBadge(licence({ badge: "amber", commercial: "underRevenue", revenueLimitUsd: 5_000_000 }))).toEqual({
      tone: "amber",
      text: "Commercial use only under $5M revenue",
    });
    expect(licenceBadge(licence({ badge: "amber", commercial: "underRevenue", revenueLimitUsd: 2_500_000 })).text).toBe(
      "Commercial use only under $2.5M revenue",
    );
    expect(licenceBadge(nonCommercial)).toEqual({ tone: "red", text: "Non-commercial" });
    expect(licenceBadge(licence({ badge: "red", commercial: "unclear" }))).toEqual({ tone: "red", text: "Check the licence" });
  });

  it("a kind of licence this version does not know reads Check the licence", () => {
    // The backend sends `commercial` as a string: a newer one may send a word not known here.
    const unknown = licence({ badge: "red", commercial: "somethingNew" as Licence["commercial"] });
    expect(licenceBadge(unknown)).toEqual({ tone: "red", text: "Check the licence" });
  });
});

describe("activeDownload", () => {
  it("is a model's download while it runs, not once it is done or cancelled", () => {
    for (const status of ["queued", "downloading", "paused", "verifying", "error", "cancelling"] as const) {
      expect(activeDownload([group({ status })], "z-image-turbo-q4")).toEqual(group({ status }));
    }
    expect(activeDownload([group({ status: "done" })], "z-image-turbo-q4")).toBeNull();
    expect(activeDownload([group({ status: "cancelled" })], "z-image-turbo-q4")).toBeNull();
    expect(activeDownload([group()], "flux2-klein-4b-q4")).toBeNull();
    // A chat model's download is no picture or video model's.
    expect(activeDownload([group({ genModelId: null, generatorKind: null })], "z-image-turbo-q4")).toBeNull();
  });
});

describe("visibleEntries", () => {
  it("Only what fits folds away the models that do not fit; off, they are listed too", () => {
    const local = { ...LOCAL, notHere: [{ ...wan, id: "big-video", fit: { ...wan.fit, word: "notHere" as const } }] };
    expect(visibleEntries(local, "image", true)).toEqual({ shown: [klein, zimage], notHere: [] });
    expect(visibleEntries(local, "video", true)).toEqual({ shown: [wan], notHere: local.notHere });
    expect(visibleEntries(local, "video", false)).toEqual({ shown: [wan, ...local.notHere], notHere: [] });
  });
});

describe("Get local models", () => {
  it("the header says what this computer has, so the estimates can be checked", async () => {
    await showLocal();
    screen.getByText(hardwareLine(LOCAL.hardware));
  });

  it("opening the view sends nothing; searching asks Hugging Face only when told to", async () => {
    await showLocal();
    expect(api.studioLocalSearch).not.toHaveBeenCalled();
    expect(api.studioLocalRepo).not.toHaveBeenCalled();
    vi.mocked(api.studioLocalSearch).mockResolvedValue({ items: [] });
    typeIn(searchBox(), "klein");
    expect(api.studioLocalSearch).not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.keyDown(searchBox(), { key: "Enter" });
    });
    expect(api.studioLocalSearch).toHaveBeenCalledWith("klein", "image");
    screen.getByText("Searching sends your words to huggingface.co.");
    // The Search button asks too.
    await click(screen.getByRole("button", { name: "Search" }));
    expect(api.studioLocalSearch).toHaveBeenCalledTimes(2);
  });

  it("Pictures and Video tabs, with Only what fits on by default", async () => {
    await showLocal();
    screen.getByRole("tab", { name: "Pictures" });
    screen.getByRole("tab", { name: "Video" });
    // The top bar's own tabs give way to these.
    expect(screen.getAllByRole("tablist")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Grid" })).toBeNull();
    expect((screen.getByRole("checkbox", { name: "Only what fits" }) as HTMLInputElement).checked).toBe(true);
    expect((screen.getByRole("checkbox", { name: "Show non-commercial models" }) as HTMLInputElement).checked).toBe(false);
  });

  it("each model shows what it is good at, size, licence in plain words, fit and time", async () => {
    await showLocal();
    const z = row(/Z-Image Turbo/);
    within(z).getByText("Photo-real pictures in 8 steps.");
    within(z).getByText(zimage.publisher);
    within(z).getByText(amountGb(zimage.sizeBytes));
    within(z).getByText("Free for any use, including commercial");
    within(z).getByText("Fits");
    within(z).getByText("Time shown after your first run here");
    within(z).getByRole("button", { name: "Download" });
    // Measured on this computer: the time alone.
    within(row(/FLUX.2 klein/)).getByText("about 2 min a picture");
  });

  it("a time not measured here yet is a first guess", async () => {
    const guessed = { ...zimage, fit: { ...zimage.fit, eta: { lowMs: 50_000, highMs: 70_000, measured: false, leftMs: null } } };
    await showLocal({ ...LOCAL, recommended: LOCAL.recommended.map((e) => (e.id === zimage.id ? guessed : e)) });
    within(row(/Z-Image Turbo/)).getByText("about 1 min a picture (first guess)");
  });

  it("one picture model and one video model are marked Recommended for this computer", async () => {
    await showLocal();
    expect(screen.getAllByText("Recommended for this computer")).toHaveLength(1); // the Pictures tab
    within(row(/Z-Image Turbo/)).getByText("Recommended for this computer");
    await click(screen.getByRole("tab", { name: "Video" }));
    expect(screen.getAllByText("Recommended for this computer")).toHaveLength(1);
    within(row(/Wan 2.2/)).getByText("Recommended for this computer");
    screen.getByText("Local video is slower and simpler than cloud video, but free and private.");
  });

  it("models that do not fit are folded away with a plain reason and no Download", async () => {
    await showLocal({
      ...LOCAL,
      recommended: LOCAL.recommended.filter((e) => e.id !== zimage.id),
      notHere: [{ ...zimage, fit: { ...zimage.fit, word: "notHere", verdict: "red", reason: "needs 8 GB of graphics memory" } }],
    });
    expect(screen.queryByRole("article", { name: /Z-Image Turbo/ })).toBeNull();
    await click(screen.getByRole("button", { name: "Not for this computer (1)" }));
    screen.getByText("needs 8 GB of graphics memory");
    expect(within(row(/Z-Image Turbo/)).queryByRole("button", { name: "Download" })).toBeNull();
  });

  it("with Only what fits off, a model that does not fit is listed with the rest, still without Download", async () => {
    await showLocal({
      ...LOCAL,
      recommended: LOCAL.recommended.filter((e) => e.id !== zimage.id),
      notHere: [{ ...zimage, fit: { ...zimage.fit, word: "notHere", verdict: "red", reason: "needs 8 GB of graphics memory" } }],
    });
    screen.getByRole("button", { name: "Not for this computer (1)" });
    await click(screen.getByRole("checkbox", { name: "Only what fits" }));
    const z = row(/Z-Image Turbo/);
    within(z).getByText("needs 8 GB of graphics memory");
    expect(within(z).queryByRole("button", { name: "Download" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Not for this computer/ })).toBeNull();
  });

  it("the Not for this computer fold starts folded on each tab", async () => {
    const tooBig = (e: LocalEntry): LocalEntry => ({
      ...e,
      fit: { ...e.fit, word: "notHere", verdict: "red", reason: "needs 8 GB of graphics memory" },
    });
    await showLocal({
      ...LOCAL,
      recommended: LOCAL.recommended.filter((e) => e.id !== zimage.id && e.id !== wan.id),
      notHere: [tooBig(zimage), tooBig(wan)],
    });
    await click(screen.getByRole("button", { name: "Not for this computer (1)" }));
    row(/Z-Image Turbo/);
    await click(screen.getByRole("tab", { name: "Video" }));
    const fold = screen.getByRole("button", { name: "Not for this computer (1)" });
    expect(fold.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByRole("article", { name: /Wan 2.2/ })).toBeNull();
  });

  it("an installed model that does not fit keeps Delete only, folded away", async () => {
    await showLocal({
      ...LOCAL,
      recommended: LOCAL.recommended.filter((e) => e.id !== klein.id),
      notHere: [{ ...klein, fit: { ...klein.fit, word: "notHere", verdict: "red", reason: "needs 8 GB of graphics memory" } }],
    });
    await click(screen.getByRole("button", { name: "Not for this computer (1)" }));
    const k = row(/FLUX.2 klein/);
    within(k).getByRole("button", { name: "Delete" });
    expect(within(k).queryByRole("button", { name: "Download" })).toBeNull();
    expect(within(k).queryByRole("button", { name: "Download again" })).toBeNull();
  });

  it("a recognised search result shows each file with its parts, total size, licence and fit", async () => {
    await showLocal();
    const result = await openResult("someone/Z-Image-Turbo-GGUF");
    within(result).getByText("z-image-turbo-Q5_K_M.gguf");
    within(result).getByText("3 parts · 7.1 GB in all · 4.6 GB to download");
    within(result).getByText("reads your prompt · 2.5 GB · already on this computer");
    within(result).getByText("Free for any use, including commercial");
    within(result).getByText("Fits");
    within(result).getByRole("button", { name: "Download" });
  });

  it("a search result's Download offers that file, its parts and licences", async () => {
    await showLocal();
    const result = await openResult("someone/Z-Image-Turbo-GGUF");
    await click(within(result).getByRole("button", { name: "Download" }));
    const repo = supported();
    expect(useStudio.getState().offer).toEqual(offerForFile(repo, repo.files[0]!, LOCAL, "image"));
    expect(useStudio.getState().offer).toMatchObject({
      source: { search: { repo: "someone/Z-Image-Turbo-GGUF", commit: "c".repeat(40), file: "z-image-turbo-Q5_K_M.gguf" } },
      parts,
      licences: [apache],
      toFetchBytes: repo.files[0]!.toFetchBytes,
    });
  });

  it("a searched file's name stays as it came in the offer, and shows with invisible characters as codes", async () => {
    await showLocal();
    const file = "z-image‮turbo.gguf";
    const repo = supported({ files: supported().files.map((f) => ({ ...f, file })) });
    const result = await openResult("someone/Z-Image-Turbo-GGUF", repo);
    within(result).getByText("z-image\\u{202E}turbo.gguf");
    await click(within(result).getByRole("button", { name: "Download" }));
    expect(useStudio.getState().offer).toMatchObject({ name: file, source: { search: { file } } });
    within(screen.getByRole("alertdialog")).getByText("Download z-image\\u{202E}turbo.gguf?");
    expect(new XMLSerializer().serializeToString(document.body)).not.toContain("‮");
  });

  it("an unknown family says Not supported yet and offers no Download", async () => {
    await showLocal();
    const result = await openResult("someone/Mystery-Diffusion-GGUF", {
      ...supported({ repo: "someone/Mystery-Diffusion-GGUF" }),
      status: "notSupportedYet",
      reason: "harnessUI does not know this kind of model yet.",
      files: [],
      licence: null,
      licences: [],
    });
    within(result).getByText("Not supported yet");
    within(result).getByText("harnessUI does not know this kind of model yet.");
    expect(within(result).queryByRole("button", { name: "Download" })).toBeNull();
  });

  it("a model harnessUI does not offer says why", async () => {
    await showLocal();
    const result = await openResult("someone/Region-Video-GGUF", {
      ...supported({ repo: "someone/Region-Video-GGUF" }),
      status: "notOffered",
      reason: "its licence excludes some regions",
      files: [],
      licence: null,
      licences: [],
    });
    within(result).getByText("Not offered in harnessUI: its licence excludes some regions.");
    expect(within(result).queryByRole("button", { name: "Download" })).toBeNull();
  });

  it("a label that differs from the family's licence is pointed out", async () => {
    await showLocal();
    const mismatch = "The repository is labelled MIT, but the model's own licence is Apache 2.0; that one applies.";
    const result = await openResult("someone/Z-Image-Turbo-GGUF", supported({ labelMismatch: mismatch }));
    within(result).getByText(mismatch);
  });

  it("non-commercial results stay hidden unless asked for", async () => {
    await showLocal();
    const red = supported({
      licence: nonCommercial,
      licences: [nonCommercial],
      files: supported().files.map((f) => ({ ...f, licences: [nonCommercial] })),
    });
    const result = await openResult("someone/Z-Image-Turbo-GGUF", red);
    within(result).getByText("Non-commercial");
    within(result).getByText("Its files show when Show non-commercial models is ticked.");
    expect(within(result).queryByText("z-image-turbo-Q5_K_M.gguf")).toBeNull();
    expect(within(result).queryByRole("button", { name: "Download" })).toBeNull();
    await click(screen.getByRole("checkbox", { name: "Show non-commercial models" }));
    within(result).getByText("Non-commercial");
    within(result).getByText("z-image-turbo-Q5_K_M.gguf");
    within(result).getByRole("button", { name: "Download" });
    await click(screen.getByRole("checkbox", { name: "Show non-commercial models" }));
    expect(within(result).queryByText("z-image-turbo-Q5_K_M.gguf")).toBeNull();
  });

  it("the footer shows the engine and the content notice", async () => {
    await showLocal();
    screen.getByText("Studio engine: built in");
    screen.getByText("Local models run only on this computer; no company checks prompts or results.");
    await click(screen.getByRole("button", { name: "What that means" }));
    expect(useStudio.getState().noticeOpen).toBe("read");
  });

  it("the footer offers the engine again when it went missing", async () => {
    // klein is installed
    await showLocal({ ...LOCAL, engine: { ...LOCAL.engine, state: "notInstalled", downloadBytes: 30_303_000 } });
    await click(screen.getByRole("button", { name: "Download the Studio engine (29 MB, MIT)" }));
    expect(api.studioEngineDownload).toHaveBeenCalledWith(true);
  });

  it("Download offers every part and every licence of the model", async () => {
    await showLocal();
    await click(within(row(/Z-Image Turbo/)).getByRole("button", { name: "Download" }));
    expect(useStudio.getState().offer).toMatchObject({
      source: { catalog: { entryId: "z-image-turbo-q4" } },
      licences: zimage.licences,
      parts: zimage.parts,
    });
    expect(useStudio.getState().offer).toEqual(offerForEntry(zimage, LOCAL));
    expect(api.studioLocalDownload).not.toHaveBeenCalled();
  });

  it("the engine's download comes with a model that needs it", () => {
    const local = { ...LOCAL, engine: { ...LOCAL.engine, state: "notInstalled" as const, downloadBytes: 30_303_000 } };
    expect(offerForEntry({ ...zimage, needsEngine: true }, local)).toMatchObject({ needsEngine: true, engineBytes: 30_303_000 });
    expect(offerForEntry(zimage, local)).toMatchObject({ needsEngine: false, engineBytes: 0 });
  });

  it("an installed model can be checked and repaired", async () => {
    await showLocal();
    const k = row(/FLUX.2 klein/);
    within(k).getByText("Installed");
    expect(within(k).queryByRole("button", { name: "Download" })).toBeNull();
    await click(within(k).getByRole("button", { name: "Download again" }));
    expect(api.studioLocalDownload).toHaveBeenCalledWith({ installed: { genModelId: "flux2-klein-4b-q4" } }, []);
  });

  it("a repair that needs the model's licences accepted shows the download dialog first", async () => {
    await showLocal();
    vi.mocked(api.studioLocalDownload).mockRejectedValueOnce(NOT_ACCEPTED);
    const k = row(/FLUX.2 klein/);
    await click(within(k).getByRole("button", { name: "Download again" }));
    const dialog = screen.getByRole("alertdialog");
    within(dialog).getByText("Download FLUX.2 klein 4B?");
    // Nothing is being checked yet.
    expect(within(k).queryByText(LOCAL_MODELS.repairing)).toBeNull();
    await click(within(dialog).getByRole("button", { name: "Accept and download" }));
    expect(api.studioLocalDownload).toHaveBeenLastCalledWith(
      { installed: { genModelId: "flux2-klein-4b-q4" } },
      klein.licences.map((l) => l.sha256),
    );
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(useStudio.getState().offer).toBeNull();
  });

  it("an installed model can be deleted, and Back returns to the cards", async () => {
    await showLocal();
    await click(within(row(/FLUX.2 klein/)).getByRole("button", { name: "Delete" }));
    expect(api.studioLocalDelete).not.toHaveBeenCalled();
    const asked = screen.getByRole("alertdialog");
    within(asked).getByText("Delete FLUX.2 klein 4B?");
    const reads = vi.mocked(api.studioLocalModels).mock.calls.length;
    await click(within(asked).getByRole("button", { name: "Delete" }));
    expect(api.studioLocalDelete).toHaveBeenCalledWith("flux2-klein-4b-q4");
    // refreshLocal, after the delete.
    expect(vi.mocked(api.studioLocalModels).mock.calls.length).toBe(reads + 1);
    expect(vi.mocked(api.studioLocalModels).mock.invocationCallOrder.at(-1)).toBeGreaterThan(
      vi.mocked(api.studioLocalDelete).mock.invocationCallOrder[0]!,
    );
    await click(screen.getByRole("button", { name: "Back" }));
    expect(useStudio.getState().localView).toBe(false);
    expect(document.querySelector("[data-card-stream]")).not.toBeNull();
    expect(screen.queryByRole("searchbox", { name: "Search Hugging Face" })).toBeNull();
    screen.getByRole("button", { name: "Grid" });
    expect(screen.getAllByRole("tablist")).toHaveLength(1);
  });

  it("a delete refused while a run uses the model says what to do", async () => {
    await showLocal();
    vi.mocked(api.studioLocalDelete).mockRejectedValue({
      code: "studio_model_in_use",
      message: "A run on this computer is using this model.",
      nextStep: "Stop the run, then delete it.",
      details: "",
    });
    await click(within(row(/FLUX.2 klein/)).getByRole("button", { name: "Delete" }));
    await click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete" }));
    screen.getByText("A run on this computer is using this model.");
    screen.getByText("Stop the run, then delete it.");
  });

  it("only the last search's answer is kept", async () => {
    const hit = (repo: string) => ({ repo, downloads: 1, likes: 0, gated: "none" as const, licenseLabel: null });
    let answerFirst: (page: LocalSearchPage) => void = () => undefined;
    vi.mocked(api.studioLocalSearch)
      .mockImplementationOnce(() => new Promise((resolve) => (answerFirst = resolve)))
      .mockResolvedValueOnce({ items: [hit("someone/video-GGUF")] });
    const first = useStudio.getState().search("klein", "image");
    await useStudio.getState().search("wan", "video");
    answerFirst({ items: [hit("someone/picture-GGUF")] });
    await first;
    expect(useStudio.getState().localSearch).toEqual({ text: "wan", tab: "video", hits: [hit("someone/video-GGUF")] });
  });

  it("Back leaves the tab as it was, and opening again starts on the tab shown", async () => {
    await showLocal();
    await click(screen.getByRole("tab", { name: "Video" }));
    expect(useStudio.getState()).toMatchObject({ localTab: "video", tab: "image" });
    await click(screen.getByRole("button", { name: "Back" }));
    expect(useStudio.getState().tab).toBe("image");
    act(() => useStudio.getState().setTab("video"));
    act(() => useStudio.getState().showLocalModels(true));
    expect(useStudio.getState().localTab).toBe("video");
  });

  describe("installed models the list does not hold", () => {
    const id = "hf:someone/Z-Image-Turbo-GGUF:z.gguf";
    const base = LOCAL.installed[0]!;
    const searched = (usable: string | null): InstalledLocal => ({
      ...base,
      id,
      name: "z.gguf",
      model: { ...base.model, id, usable },
    });

    it("a model downloaded from a search is listed, with Download again and Delete", async () => {
      await showLocal({ ...LOCAL, installed: [searched(null), ...LOCAL.installed] });
      const r = row("z.gguf");
      within(r).getByText(amountGb(base.sizeBytes));
      within(r).getByText("Fits");
      await click(within(r).getByRole("button", { name: "Download again" }));
      expect(api.studioLocalDownload).toHaveBeenCalledWith({ installed: { genModelId: id } }, []);
      await click(within(r).getByRole("button", { name: "Delete" }));
      await click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete" }));
      expect(api.studioLocalDelete).toHaveBeenCalledWith(id);
    });

    it("its repair that needs the engine's licence accepted shows that licence", async () => {
      const engine = licence({ id: "studio-engine", name: LOCAL.engine.licenseName, sha256: "e".repeat(64) });
      const withEngine = (e: LocalEntry) => ({ ...e, needsEngine: true, licences: [...e.licences, engine] });
      const pack = { ...LOCAL.engine, state: "notInstalled" as const, downloadBytes: 30_303_000 };
      const local: LocalModels = {
        ...LOCAL,
        engine: pack,
        installed: [searched(null), ...LOCAL.installed],
        recommended: LOCAL.recommended.map((e) => (e.installed ? e : withEngine(e))),
      };
      await showLocal(local);
      vi.mocked(api.studioLocalDownload).mockRejectedValueOnce(NOT_ACCEPTED);
      await click(within(row("z.gguf")).getByRole("button", { name: "Download again" }));
      const dialog = screen.getByRole("alertdialog");
      within(dialog).getByText("Download z.gguf?");
      within(dialog).getByText("Also downloads the Studio engine (29 MB, MIT licence).");
      await click(within(dialog).getByRole("button", { name: "Accept and download" }));
      expect(api.studioLocalDownload).toHaveBeenLastCalledWith({ installed: { genModelId: id } }, [engine.sha256]);
    });

    it("a repair refused for a licence it cannot show says why", async () => {
      await showLocal({ ...LOCAL, installed: [searched(null), ...LOCAL.installed] });
      vi.mocked(api.studioLocalDownload).mockRejectedValueOnce(NOT_ACCEPTED);
      await click(within(row("z.gguf")).getByRole("button", { name: "Download again" }));
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(useStudio.getState().offer).toBeNull();
      screen.getByText(NOT_ACCEPTED.message);
      screen.getByText(NOT_ACCEPTED.nextStep);
    });

    it("one this version cannot run says why, and can only be deleted", async () => {
      const why = "This version of harnessUI cannot run this model.";
      await showLocal({ ...LOCAL, installed: [searched(why), ...LOCAL.installed] });
      const r = row("z.gguf");
      within(r).getByText(why);
      within(r).getByRole("button", { name: "Delete" });
      expect(within(r).queryByRole("button", { name: "Download again" })).toBeNull();
    });

    it("they show whether or not Only what fits is on, on their own tab only", async () => {
      await showLocal({ ...LOCAL, installed: [searched(null), ...LOCAL.installed] });
      await click(screen.getByRole("checkbox", { name: "Only what fits" }));
      row("z.gguf");
      await click(screen.getByRole("tab", { name: "Video" }));
      expect(screen.queryByRole("article", { name: "z.gguf" })).toBeNull();
    });
  });

  it("an entry whose installed model cannot run says why in place of Download again", async () => {
    const why = "This version of harnessUI cannot run this model.";
    const base = LOCAL.installed[0]!;
    await showLocal({
      ...LOCAL,
      installed: [{ ...base, model: { ...base.model, usable: why } }, ...LOCAL.installed.slice(1)],
    });
    const k = row(/FLUX.2 klein/);
    within(k).getByText(why);
    expect(within(k).queryByRole("button", { name: "Download again" })).toBeNull();
    within(k).getByRole("button", { name: "Delete" });
  });

  it("a recommended entry not of the tab's kind is not listed", async () => {
    const entries: LocalEntry[] = LOCAL.recommended;
    await showLocal();
    for (const e of entries.filter((x) => x.kind === "video")) {
      expect(screen.queryByRole("article", { name: e.name })).toBeNull();
    }
  });
});

describe("a model downloading", () => {
  it("after Accept, its row shows how far the download got in Models → Downloads' words, and no Download", async () => {
    await showLocal();
    await click(within(row("Z-Image Turbo")).getByRole("button", { name: "Download" }));
    ipc.listDownloads.mockResolvedValue([group({ bytesDone: 0, status: "queued" })]);
    await click(screen.getByRole("button", { name: "Accept and download" }));
    const z = row("Z-Image Turbo");
    expect(within(z).queryByRole("button", { name: "Download" })).toBeNull();
    // The same words and sizes as Models → Downloads (6 GiB is 6.4 GB there).
    within(z).getByText("0 B of 6.4 GB · queued");
    const bar = within(z).getByRole("progressbar", { name: "Downloading Z-Image Turbo" });
    expect(bar.getAttribute("aria-valuenow")).toBe("0");
    // Its progress events move it on.
    act(() => progress(group({ bytesDone: 3 * GIB, bytesPerSecond: 64 * 1024 * 1024 })));
    within(z).getByText("3.2 GB of 6.4 GB · downloading · 48 s left");
    expect(bar.getAttribute("aria-valuenow")).toBe("50");
    act(() => progress(group({ bytesDone: 3 * GIB, checking: true })));
    within(z).getByText("3.2 GB of 6.4 GB · checking the file…");
    // Another model's row is as it was.
    within(row(/FLUX.2 klein/)).getByRole("button", { name: "Download again" });
  });

  it("a download already running when the view opens shows, and once done the model is installed", async () => {
    ipc.listDownloads.mockResolvedValue([group()]);
    await showLocal();
    const z = row("Z-Image Turbo");
    within(z).getByText("2.1 GB of 6.4 GB · downloading");
    expect(within(z).queryByRole("button", { name: "Download" })).toBeNull();
    const installed: LocalModels = {
      ...LOCAL,
      recommended: LOCAL.recommended.map((e) => (e.id === zimage.id ? { ...e, installed: true } : e)),
      installed: [{ ...LOCAL.installed[0]!, id: zimage.id, name: zimage.name }, ...LOCAL.installed],
    };
    vi.mocked(api.studioLocalModels).mockResolvedValue(installed);
    const reads = vi.mocked(api.studioLocalModels).mock.calls.length;
    act(() => progress(group({ status: "done", bytesDone: 6 * GIB })));
    await flush();
    expect(vi.mocked(api.studioLocalModels).mock.calls.length).toBe(reads + 1);
    within(row("Z-Image Turbo")).getByText("Installed");
    expect(within(row("Z-Image Turbo")).queryByRole("progressbar")).toBeNull();
  });

  it("an installed model downloading again shows that in place of Download again", async () => {
    ipc.listDownloads.mockResolvedValue([
      group({ id: "group-2", genModelId: "flux2-klein-4b-q4", label: "FLUX.2 klein 4B", status: "paused" }),
    ]);
    await showLocal();
    const k = row(/FLUX.2 klein/);
    within(k).getByText("Installed");
    within(k).getByText("2.1 GB of 6.4 GB · paused");
    expect(within(k).queryByRole("button", { name: "Download again" })).toBeNull();
    within(k).getByRole("button", { name: "Delete" });
  });

  it("a searched file downloading shows that in place of Download, and why it stopped", async () => {
    ipc.listDownloads.mockResolvedValue([
      group({
        genModelId: "hf:someone/Z-Image-Turbo-GGUF:z-image-turbo-Q5_K_M.gguf",
        status: "error",
        error: { code: "download_failed", message: "The download stopped.", nextStep: "Resume it in Models → Downloads.", details: "" },
      }),
    ]);
    await showLocal();
    const result = await openResult("someone/Z-Image-Turbo-GGUF");
    expect(within(result).queryByRole("button", { name: "Download" })).toBeNull();
    within(result).getByText("2.1 GB of 6.4 GB · error");
    within(result).getByText("The download stopped. Resume it in Models → Downloads.");
    within(result).getByRole("progressbar", { name: "Downloading z-image-turbo-Q5_K_M.gguf" });
  });
});

describe("at the window's smallest", () => {
  it("the search row wraps rather than scroll the middle column sideways", async () => {
    await showLocal();
    // jsdom lays nothing out: the class is the layout (at most 128 px before the row wraps).
    const box = screen.getByRole("searchbox", { name: LOCAL_MODELS.search }).closest("label")!;
    expect(box.className.split(/\s+/)).toContain("min-w-32");
    expect(box.className.split(/\s+/)).not.toContain("min-w-48");
  });
});
