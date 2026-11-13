import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
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
        repo: "someone/‮evil-GGUF",
        label: "X-‮fuug.Q4_K_M.gguf",
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
    expect(new XMLSerializer().serializeToString(document.body)).not.toContain("‮");
  });
});
