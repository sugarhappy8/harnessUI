// A Studio download as every place that shows one draws it (0.2.0 Task U7, design 10.3): how far
// it got in one size format with the time left or what it is doing, its bar, Pause or Resume and
// Cancel named for the model and sending the download commands, and once done "Ready" with *Use
// it* where the composer can use the model (never for a LoRA, which is attached in Settings).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { DownloadGroup } from "../../lib/types";

const ipc = vi.hoisted(() => ({
  isTauri: () => true,
  pauseDownload: vi.fn<(groupId: string) => Promise<void>>(),
  resumeDownload: vi.fn<(groupId: string) => Promise<void>>(),
  cancelDownload: vi.fn<(groupId: string) => Promise<void>>(),
}));
vi.mock("../../lib/ipc", () => ipc);
vi.mock("./api");

const { DownloadState, progressWords } = await import("./DownloadState");
const { UiProviders } = await import("../ui/UiProviders");

/** Z-Image Turbo's download, 2.4 GB of 6.7 GB done, at 43 MB/s: 100 s, "2 min left". */
function group(patch: Partial<DownloadGroup> = {}): DownloadGroup {
  return {
    id: "group-7",
    repo: "leejet/Z-Image-Turbo-GGUF",
    label: "Z-Image Turbo",
    fileCount: 3,
    bytesDone: 2_400_000_000,
    bytesTotal: 6_700_000_000,
    status: "downloading",
    error: null,
    bytesPerSecond: 43_000_000,
    unverified: false,
    checking: false,
    generatorKind: "image",
    genModelId: "z-image-turbo-q4",
    ...patch,
  };
}

function show(g: DownloadGroup, onUse?: () => void) {
  return render(
    <UiProviders>
      <DownloadState group={g} name="Z-Image Turbo" onUse={onUse} />
    </UiProviders>,
  );
}

async function press(name: string) {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name }));
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  ipc.pauseDownload.mockResolvedValue(undefined);
  ipc.resumeDownload.mockResolvedValue(undefined);
  ipc.cancelDownload.mockResolvedValue(undefined);
});

afterEach(cleanup);

describe("DownloadState", () => {
  it("progress reads in one size format", () => {
    show(group());
    screen.getByText("2.4 GB of 6.7 GB · 2 min left");
    const bar = screen.getByRole("progressbar", { name: "Downloading Z-Image Turbo" });
    expect(bar.getAttribute("aria-valuenow")).toBe("36");
    // What it is doing, in place of the time left; nothing without an estimate.
    expect(progressWords(group({ bytesPerSecond: null }))).toBe("2.4 GB of 6.7 GB");
    expect(progressWords(group({ status: "queued", bytesDone: 0 }))).toBe("0 B of 6.7 GB · Waiting");
    expect(progressWords(group({ status: "paused" }))).toBe("2.4 GB of 6.7 GB · Paused");
    expect(progressWords(group({ checking: true }))).toBe("2.4 GB of 6.7 GB · Checking the file…");
    expect(progressWords(group({ status: "verifying", bytesDone: 6_700_000_000 }))).toBe("6.7 GB of 6.7 GB · Checking the files…");
    expect(progressWords(group({ status: "error" }))).toBe("2.4 GB of 6.7 GB · Stopped");
    expect(progressWords(group({ status: "cancelling" }))).toBe("2.4 GB of 6.7 GB · Cancelling…");
    expect(progressWords(group({ unverified: true, bytesPerSecond: null }))).toBe("2.4 GB of 6.7 GB · no published checksum");
    // Sizes under a gigabyte, in the same format.
    expect(progressWords(group({ bytesDone: 12_000_000, bytesTotal: 170_000_000, bytesPerSecond: null }))).toBe("12 MB of 170 MB");
  });

  it("Pause and Cancel are buttons named for the model, and call the download commands", async () => {
    const { rerender } = show(group());
    await press("Pause downloading Z-Image Turbo");
    expect(ipc.pauseDownload).toHaveBeenCalledWith("group-7");
    await press("Cancel downloading Z-Image Turbo");
    expect(ipc.cancelDownload).toHaveBeenCalledWith("group-7");
    // Paused, or stopped by a failure: Resume in place of Pause, with why it stopped.
    rerender(
      <UiProviders>
        <DownloadState
          group={group({
            status: "error",
            error: { code: "download_failed", message: "The download stopped.", nextStep: "Check the connection.", details: "" },
          })}
          name="Z-Image Turbo"
        />
      </UiProviders>,
    );
    expect(screen.queryByRole("button", { name: "Pause downloading Z-Image Turbo" })).toBeNull();
    screen.getByText("The download stopped. Check the connection.");
    await press("Resume downloading Z-Image Turbo");
    expect(ipc.resumeDownload).toHaveBeenCalledWith("group-7");
    // A press refused says why.
    ipc.resumeDownload.mockRejectedValue({ code: "x", message: "It could not be resumed.", nextStep: "Try again.", details: "" });
    await press("Resume downloading Z-Image Turbo");
    expect(screen.getByRole("alert").textContent).toBe("It could not be resumed. Try again.");
    // Being cancelled: nothing more to press.
    rerender(
      <UiProviders>
        <DownloadState group={group({ status: "cancelling" })} name="Z-Image Turbo" />
      </UiProviders>,
    );
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  it("a finished download offers Use it", async () => {
    const onUse = vi.fn();
    const { rerender } = show(group({ status: "done", bytesDone: 6_700_000_000 }), onUse);
    screen.getByText("Ready");
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.queryByRole("button", { name: /Pause|Cancel/ })).toBeNull();
    const use = screen.getByRole("button", { name: "Use it" });
    // Which model it uses is said to a screen reader too.
    expect(document.getElementById(use.getAttribute("aria-describedby")!)?.textContent).toBe("Z-Image Turbo");
    await press("Use it");
    expect(onUse).toHaveBeenCalledTimes(1);
    // A LoRA (or a model the composer cannot use): ready, and no Use it.
    rerender(
      <UiProviders>
        <DownloadState group={group({ status: "done", genModelId: null, generatorKind: null, loraId: "hf:o/r:pixel.safetensors" })} name="pixel" />
      </UiProviders>,
    );
    screen.getByText("Ready");
    expect(screen.queryByRole("button", { name: "Use it" })).toBeNull();
  });
});
