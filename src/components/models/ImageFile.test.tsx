import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { DownloadGroup, ImageFileOffer, InstalledModel, RuntimeStatus } from "../../lib/types";

// [0.0.7 P2] An installed model's image file in Models → Installed: one that couldn't be read,
// and "Download the image file" from the model's Hugging Face repository.

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  imageFileOffer: vi.fn<(path: string) => Promise<ImageFileOffer>>(),
  downloadImageFile: vi.fn<(path: string) => Promise<string>>(async () => "group-1"),
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

describe("Models → Installed: the image file", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    progress = null;
    ipcMock.onDownloadProgress.mockImplementation(async (h) => {
      progress = h;
      return unlisten;
    });
    ipcMock.imageFileOffer.mockResolvedValue({ kind: "available", file: "mmproj-Model-F16.gguf", sizeBytes: 850 * MB });
    useApp.setState({ runtime: null });
  });
  afterEach(cleanup);

  it("offers the image file of the model's repository, follows the download and says to reload", async () => {
    const onChanged = await show(model());
    expect(ipcMock.imageFileOffer).toHaveBeenCalledWith("/models/someone/Model-GGUF/Model-Q4_K_M.gguf");
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
    expect(screen.getByText("Image file downloaded. Reload the model to use it.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Download/ })).toBeNull();
    // Not loaded now: nothing to reload.
    expect(screen.queryByRole("button", { name: "Reload" })).toBeNull();
    expect(unlisten).toHaveBeenCalled();
  });

  it("reloads the model from the row when it is the one loaded now", async () => {
    const m = model();
    useApp.setState({ runtime: readyOn(m.path) });
    await show(m);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Download the image file (850 MB)" }));
    });
    await send(group({ bytesDone: 850 * MB, status: "done" }));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Reload" }));
    });
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(true, true);
  });

  it("says the image file couldn't be read and offers another one", async () => {
    const m = model({ projectorPath: null, imageFileUnreadable: true });
    expect(seesImages(m)).toBe(false);
    await show(m);
    expect(screen.getByText("The image file couldn't be read, so this model can't see images.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Download another image file (850 MB)" })).toBeTruthy();
  });

  it("says when the repository has no other image file", async () => {
    for (const offer of [{ kind: "noOther" }, { kind: "none" }] as ImageFileOffer[]) {
      ipcMock.imageFileOffer.mockResolvedValueOnce(offer);
      await show(model({ imageFileUnreadable: true }));
      expect(screen.getByText("Its repository has no other image file.")).toBeTruthy();
      expect(screen.queryByRole("button")).toBeNull();
      cleanup();
    }
  });

  it("shows nothing for a model that sees images, or whose repository has no image file", async () => {
    const seeing = model({ projectorPath: "/models/someone/Model-GGUF/mmproj-Model-F16.gguf" });
    expect(seesImages(seeing)).toBe(true);
    await show(seeing);
    expect(ipcMock.imageFileOffer).not.toHaveBeenCalled();
    expect(document.body.textContent).toBe("");
    cleanup();

    ipcMock.imageFileOffer.mockResolvedValueOnce({ kind: "none" });
    await show(model());
    expect(document.body.textContent).toBe("");
    cleanup();

    // Offline: nothing offered, nothing said.
    ipcMock.imageFileOffer.mockRejectedValueOnce({ code: "network", message: "Hugging Face could not be reached." });
    await show(model());
    expect(document.body.textContent).toBe("");
  });

  it("asks nothing for a model without a repository, but still says its image file couldn't be read", async () => {
    await show(model({ repo: null, source: "import" }));
    expect(ipcMock.imageFileOffer).not.toHaveBeenCalled();
    expect(document.body.textContent).toBe("");
    cleanup();
    await show(model({ repo: null, source: "import", imageFileUnreadable: true }));
    expect(ipcMock.imageFileOffer).not.toHaveBeenCalled();
    expect(screen.getByText("The image file couldn't be read, so this model can't see images.")).toBeTruthy();
  });

  it("says why when the download cannot start or stops, and offers it again", async () => {
    ipcMock.downloadImageFile.mockRejectedValueOnce({
      code: "hf_rate_limited",
      message: "Hugging Face is limiting requests from this network.",
      nextStep: "Wait a minute, then try again.",
      details: "",
    });
    await show(model());
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

  it("uses plain words: no file kinds from llama.cpp on screen", async () => {
    await show(model({ imageFileUnreadable: true }));
    expect(document.body.textContent).not.toMatch(/mmproj|projector|CLIP/i);
  });
});
