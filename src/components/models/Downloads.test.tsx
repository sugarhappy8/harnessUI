import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, within } from "@testing-library/react";
import type { DownloadGroup } from "../../lib/types";

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  listDownloads: vi.fn<() => Promise<DownloadGroup[]>>(),
  onDownloadProgress: vi.fn(async () => () => undefined),
  pauseDownload: vi.fn(async () => undefined),
  resumeDownload: vi.fn(async () => undefined),
  cancelDownload: vi.fn(async () => undefined),
}));
vi.mock("../../lib/ipc", () => ipcMock);

const { Downloads } = await import("./Downloads");

describe("Models → Downloads", () => {
  afterEach(cleanup);

  it("shows the repo and file names with hidden and right-to-left characters as codes", async () => {
    ipcMock.listDownloads.mockResolvedValue([
      {
        id: "d1",
        repo: "someone/\u202Eevil-GGUF",
        label: "X-\u202Efuug.Q4_K_M.gguf",
        fileCount: 1,
        bytesDone: 0,
        bytesTotal: 1_000_000_000,
        status: "queued",
        error: null,
        bytesPerSecond: null,
        unverified: false,
      },
    ]);
    await act(async () => {
      render(<Downloads />);
    });
    const name = screen.getByText("someone/\\u{202E}evil-GGUF · X-\\u{202E}fuug.Q4_K_M.gguf");
    expect(name.tagName).toBe("BDI");
    expect(name.getAttribute("dir")).toBe("ltr");
    expect(new XMLSerializer().serializeToString(document.body)).not.toContain("\u202E");
  });

  it("[0.0.7 P6b] says when it is checking a file, with no time left to wait", async () => {
    ipcMock.listDownloads.mockResolvedValue([
      {
        id: "d2",
        repo: "someone/sample-GGUF",
        label: "Q8_0",
        fileCount: 2,
        bytesDone: 4_000_000_000,
        bytesTotal: 13_000_000_000,
        status: "downloading",
        error: null,
        bytesPerSecond: 500_000_000,
        unverified: false,
        checking: true,
      },
    ]);
    await act(async () => {
      render(<Downloads />);
    });
    expect(screen.getByText(/· checking the file…$/)).toBeTruthy();
    expect(screen.queryByText(/downloading/)).toBeNull();
    expect(screen.queryByText(/left/)).toBeNull();
    // It can still be paused or cancelled.
    expect(screen.getByRole("button", { name: "Pause" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
  });

  it("[0.0.7 P6b] says when a download is being cancelled, and offers only Cancel", async () => {
    ipcMock.listDownloads.mockResolvedValue([
      {
        id: "d3",
        repo: "someone/sample-GGUF",
        label: "Q4_K_M",
        fileCount: 1,
        bytesDone: 2_000_000_000,
        bytesTotal: 4_000_000_000,
        status: "cancelling",
        error: null,
        bytesPerSecond: null,
        unverified: false,
      },
    ]);
    await act(async () => {
      render(<Downloads />);
    });
    expect(screen.getByText(/· cancelling…$/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Pause" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Resume" })).toBeNull();
    // A cancel that did not finish in time can be asked for again.
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
  });

  it("a picture or video model's download says so", async () => {
    const group = (id: string, generatorKind: DownloadGroup["generatorKind"]): DownloadGroup => ({
      id,
      repo: "someone/sample-GGUF",
      label: id,
      fileCount: 3,
      bytesDone: 0,
      bytesTotal: 4_000_000_000,
      status: "queued",
      error: null,
      bytesPerSecond: null,
      unverified: false,
      generatorKind,
    });
    ipcMock.listDownloads.mockResolvedValue([
      group("clip", "video"),
      group("still", "image"),
      group("edit", "image-edit"),
      group("chat", null),
    ]);
    await act(async () => {
      render(<Downloads />);
    });
    const rowOf = (id: string) => screen.getByText(`someone/sample-GGUF · ${id}`).closest("li") as HTMLElement;
    within(rowOf("clip")).getByText("Video model");
    within(rowOf("still")).getByText("Picture model");
    within(rowOf("edit")).getByText("Picture model");
    expect(within(rowOf("chat")).queryByText(/model$/)).toBeNull();
  });
});
