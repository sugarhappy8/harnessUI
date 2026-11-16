import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { DownloadGroup, ImageFileOffer, InstalledModel, RuntimeStatus } from "../../lib/types";

// [0.0.7 P2] An installed model's image file in Models → Installed: one that couldn't be read,
// and "Download the image file" from the model's Hugging Face repository.

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  imageFileOffer: vi.fn<(path: string) => Promise<ImageFileOffer>>(),
  downloadImageFile: vi.fn<(path: string) => Promise<string>>(async () => "group-1"),
  forgetUnreadableImageFile: vi.fn<(path: string) => Promise<boolean>>(async () => true),
  onDownloadProgress: vi.fn<(h: (g: DownloadGroup) => void) => Promise<() => void>>(),
  modelBusy: vi.fn(async () => ({ chatTurns: 0, modelChecks: 0, otherApps: 0 })),
  reloadLocalModel: vi.fn<(force: boolean, whenIdle: boolean) => Promise<boolean>>(async () => true),
}));
vi.mock("../../lib/ipc", () => ipcMock);

const { ImageFileLine, seesImages } = await import("./ImageFile");
const { useApp } = await import("../../store/app");

const MB = 1_000_000;

function model(patch: Partial<InstalledModel> = {}): InstalledModel {
  return {
    id: 1,
    path: "/models/someone/Model-GGUF/Model-Q4_K_M.gguf",
    repo: "someone/Model-GGUF",
    fileName: "Model-Q4_K_M.gguf",
    sizeBytes: 16_000 * MB,
    source: "download",
    projectorPath: null,
    summary: null,
    architectureSupported: true,
    ...patch,
  };
}

function group(patch: Partial<DownloadGroup>): DownloadGroup {
  return {
    id: "group-1",
    repo: "someone/Model-GGUF",
    label: "Image file",
    fileCount: 1,
    bytesDone: 0,
    bytesTotal: 850 * MB,
    status: "downloading",
    error: null,
    bytesPerSecond: null,
    unverified: false,
    ...patch,
  };
}

function readyOn(path: string): RuntimeStatus {
  return {
    state: "ready",
    backend: "cpu",
    device: null,
    modelPath: path,
    modelName: null,
    progress: 1,
    stage: null,
    nCtx: 4096,
    supportsTools: true,
    preserveReasoning: false,
    thinking: false,
    speedTps: null,
    slow: false,
    vision: false,
    offloadedLayers: null,
    totalLayers: null,
    error: null,
    loadStage: null,
    adjustments: [],
    ctxSource: null,
    settingsChanged: false,
    reloadWaiting: false,
    unloadedPath: null,
    placement: null,
  };
}

let progress: ((g: DownloadGroup) => void) | null = null;
const unlisten = vi.fn();

async function show(m: InstalledModel, onChanged = vi.fn()) {
  await act(async () => {
    render(<ImageFileLine model={m} onChanged={onChanged} />);
  });
  return onChanged;
}

async function send(g: DownloadGroup) {
  await act(async () => {
    progress?.(g);
  });
}

const AVAILABLE: ImageFileOffer = { kind: "available", file: "mmproj-Model-F16.gguf", sizeBytes: 850 * MB };

describe("Models → Installed: the image file", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    progress = null;
    ipcMock.onDownloadProgress.mockImplementation(async (h) => {
      progress = h;
      return unlisten;
    });
    ipcMock.imageFileOffer.mockResolvedValue(AVAILABLE);
    useApp.setState({ runtime: null });
  });
  afterEach(cleanup);

  it("offers the image file saved with the model without asking Hugging Face, follows the download and says when it is used", async () => {
    const onChanged = await show(model({ imageFileOffer: AVAILABLE }));
    expect(ipcMock.imageFileOffer).not.toHaveBeenCalled();
    const button = screen.getByRole("button", { name: "Download the image file (850 MB)" });
    expect(button.getAttribute("title")).toBe("mmproj-Model-F16.gguf");
    await act(async () => {
      fireEvent.click(button);
    });
    expect(ipcMock.downloadImageFile).toHaveBeenCalledWith("/models/someone/Model-GGUF/Model-Q4_K_M.gguf");
    expect(screen.getByRole("status").textContent).toBe("Downloading the image file… 0%");
    // Another download's progress changes nothing here.
    await send(group({ id: "other", bytesDone: 800 * MB }));
    expect(screen.getByRole("status").textContent).toBe("Downloading the image file… 0%");
    await send(group({ bytesDone: 425 * MB }));
    expect(screen.getByRole("status").textContent).toBe("Downloading the image file… 50%");
    expect(onChanged).not.toHaveBeenCalled();
    await send(group({ bytesDone: 850 * MB, status: "done" }));
    expect(onChanged).toHaveBeenCalledTimes(1);
    // Not loaded now: nothing to reload, and no "reload" in the words (P2 review m7).
    expect(screen.getByText("Image file downloaded. It will be used the next time the model loads.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Download/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Reload" })).toBeNull();
    expect(unlisten).toHaveBeenCalled();
    expect(ipcMock.imageFileOffer).not.toHaveBeenCalled();
  });

  it("says to reload, and reloads, when the model is the one loaded now", async () => {
    const m = model({ imageFileOffer: AVAILABLE });
    useApp.setState({ runtime: readyOn(m.path) });
    await show(m);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Download the image file (850 MB)" }));
    });
    await send(group({ bytesDone: 850 * MB, status: "done" }));
    expect(screen.getByText("Image file downloaded. Reload the model to use it.")).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Reload" }));
    });
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(true, true);
  });

  it("offers to look for an image file when nothing is saved for the model, and asks only then (P2 review M2)", async () => {
    await show(model());
    expect(ipcMock.imageFileOffer).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: /Download/ })).toBeNull();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Look for an image file" }));
    });
    expect(ipcMock.imageFileOffer).toHaveBeenCalledTimes(1);
    expect(ipcMock.imageFileOffer).toHaveBeenCalledWith("/models/someone/Model-GGUF/Model-Q4_K_M.gguf");
    expect(screen.getByRole("button", { name: "Download the image file (850 MB)" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Look for an image file" })).toBeNull();
    cleanup();

    // Its repository has none: said once, after the click.
    ipcMock.imageFileOffer.mockResolvedValueOnce({ kind: "none" });
    await show(model({ imageFileOffer: undefined }));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Look for an image file" }));
    });
    expect(screen.getByText("Its repository has no image file.")).toBeTruthy();
    cleanup();

    // Offline: said, and the link stays to try again.
    ipcMock.imageFileOffer.mockRejectedValueOnce({ code: "network", message: "Hugging Face could not be reached." });
    await show(model({ imageFileOffer: null }));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Look for an image file" }));
    });
    expect(screen.getByRole("alert").textContent).toBe("Hugging Face could not be reached.");
    expect(screen.getByRole("button", { name: "Look for an image file" })).toBeTruthy();
  });

  it("says the image file couldn't be read, offers another one, and can try it again (P2 review M1)", async () => {
    const m = model({ projectorPath: null, imageFileUnreadable: true, imageFileOffer: AVAILABLE });
    expect(seesImages(m)).toBe(false);
    const onChanged = await show(m);
    expect(screen.getByText("The image file couldn't be read, so this model can't see images.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Download another image file (850 MB)" })).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Try it again" }));
    });
    expect(ipcMock.forgetUnreadableImageFile).toHaveBeenCalledWith(m.path);
    expect(onChanged).toHaveBeenCalledTimes(1);
    expect(screen.getByText("It will be tried again the next time the model loads.")).toBeTruthy();
    expect(screen.queryByText("The image file couldn't be read, so this model can't see images.")).toBeNull();
    expect(screen.queryByRole("button", { name: "Reload" })).toBeNull();
    cleanup();

    // Loaded now (without it): reload from the row.
    useApp.setState({ runtime: readyOn(m.path) });
    await show(m);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Try it again" }));
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Reload" }));
    });
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(true, true);
  });

  it("offers the same image file again when the copy here may differ from the repository's", async () => {
    await show(
      model({
        imageFileUnreadable: true,
        imageFileOffer: { kind: "again", file: "mmproj-Model-F16.gguf", sizeBytes: 850 * MB },
      }),
    );
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Download the image file again (850 MB)" }));
    });
    expect(ipcMock.downloadImageFile).toHaveBeenCalledWith("/models/someone/Model-GGUF/Model-Q4_K_M.gguf");
  });

  it("says when the repository has no other image file, or none at all (P2 review m7)", async () => {
    await show(model({ imageFileUnreadable: true, imageFileOffer: { kind: "noOther" } }));
    expect(screen.getByText("Its repository has no other image file.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Download|Look/ })).toBeNull();
    cleanup();
    await show(model({ imageFileUnreadable: true, imageFileOffer: { kind: "none" } }));
    expect(screen.getByText("Its repository has no image file.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Download|Look/ })).toBeNull();
  });

  it("asks Hugging Face by itself only after the model's image file couldn't be read, and only once", async () => {
    const m = model({ path: "/models/someone/Model-GGUF/Asked-Q4_K_M.gguf", imageFileUnreadable: true });
    await show(m);
    expect(ipcMock.imageFileOffer).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Download another image file (850 MB)" })).toBeTruthy();
    cleanup();
    await show(m);
    expect(ipcMock.imageFileOffer).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Look for an image file" })).toBeTruthy();
  });

  it("keeps the answer of the automatic lookup when the row is drawn again before it arrives (P2 verification N4)", async () => {
    let answer: ((o: ImageFileOffer) => void) | null = null;
    ipcMock.imageFileOffer.mockImplementationOnce(
      () =>
        new Promise<ImageFileOffer>((resolve) => {
          answer = resolve;
        }),
    );
    const m = model({ path: "/models/someone/Model-GGUF/Redrawn-Q4_K_M.gguf", imageFileUnreadable: true });
    let view: ReturnType<typeof render> | null = null;
    await act(async () => {
      view = render(<ImageFileLine model={m} onChanged={vi.fn()} />);
    });
    expect(ipcMock.imageFileOffer).toHaveBeenCalledTimes(1);
    // The list is read again meanwhile: a new object for the same row.
    await act(async () => {
      view?.rerender(<ImageFileLine model={{ ...m }} onChanged={vi.fn()} />);
    });
    await act(async () => {
      answer?.(AVAILABLE);
    });
    expect(ipcMock.imageFileOffer).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Download another image file (850 MB)" })).toBeTruthy();
  });

  it("shows nothing for a model that sees images, or whose repository is known to have none", async () => {
    const seeing = model({ projectorPath: "/models/someone/Model-GGUF/mmproj-Model-F16.gguf", imageFileOffer: null });
    expect(seesImages(seeing)).toBe(true);
    await show(seeing);
    expect(document.body.textContent).toBe("");
    cleanup();
    await show(model({ imageFileOffer: { kind: "none" } }));
    expect(document.body.textContent).toBe("");
    expect(ipcMock.imageFileOffer).not.toHaveBeenCalled();
  });

  it("asks nothing for a model without a repository, but says its image file couldn't be read and can try it again", async () => {
    await show(model({ repo: null, source: "import" }));
    expect(document.body.textContent).toBe("");
    cleanup();
    await show(model({ repo: null, source: "import", imageFileUnreadable: true }));
    expect(ipcMock.imageFileOffer).not.toHaveBeenCalled();
    expect(screen.getByText("The image file couldn't be read, so this model can't see images.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Try it again" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Look for an image file" })).toBeNull();
  });

  it("says why when the download cannot start or stops, and offers it again", async () => {
    ipcMock.downloadImageFile.mockRejectedValueOnce({
      code: "hf_rate_limited",
      message: "Hugging Face is limiting requests from this network.",
      nextStep: "Wait a minute, then try again.",
      details: "",
    });
    await show(model({ imageFileOffer: AVAILABLE }));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Download the image file (850 MB)" }));
    });
    expect(screen.getByRole("alert").textContent).toBe("Hugging Face is limiting requests from this network.");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Download the image file (850 MB)" }));
    });
    expect(screen.queryByRole("alert")).toBeNull();
    await send(
      group({
        status: "error",
        error: {
          code: "checksum_mismatch",
          message: "The downloaded file does not match its published checksum, so it was deleted.",
          nextStep: "Try the download again.",
          details: "",
        },
      }),
    );
    expect(screen.getByRole("alert").textContent).toBe(
      "The downloaded file does not match its published checksum, so it was deleted.",
    );
    expect(screen.getByRole("button", { name: "Download the image file (850 MB)" })).toBeTruthy();
  });

  it("uses plain words, and shows a name from the repository with its hidden characters (P2 review m10)", async () => {
    await show(model({ imageFileUnreadable: true, imageFileOffer: { kind: "noOther" } }));
    expect(document.body.textContent).not.toMatch(/mmproj|projector|CLIP/i);
    cleanup();
    await show(model({ imageFileOffer: { kind: "available", file: "mmproj‮fugg.exe", sizeBytes: 850 * MB } }));
    expect(screen.getByRole("button", { name: "Download the image file (850 MB)" }).getAttribute("title")).toBe(
      "mmproj\\u{202E}fugg.exe",
    );
  });
});
