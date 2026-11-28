import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { PackInfo, PackProgress } from "../lib/types";

const ipcMock = vi.hoisted(() => ({
  listPacks: vi.fn<() => Promise<PackInfo[]>>(),
  removePack: vi.fn<(id: string, olderOnly?: boolean) => Promise<void>>(async () => undefined),
  onPackProgress: vi.fn<(f: (p: PackProgress) => void) => Promise<() => void>>(async () => () => undefined),
  downloadPack: vi.fn(async () => undefined),
  cancelPackDownload: vi.fn(async () => undefined),
}));
vi.mock("../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const { BackendDownloads } = await import("./BackendDownloads");

function pack(patch: Partial<PackInfo> = {}): PackInfo {
  return {
    id: "cuda",
    title: "NVIDIA CUDA 13",
    backend: "cuda",
    requirements: "NVIDIA RTX 30 series or newer",
    licenseName: "NVIDIA licence",
    licenseUrl: "https://example.org/licence",
    downloadBytes: 653_269_378,
    state: "notInstalled",
    bytesDone: 0,
    olderBytes: 0,
    available: true,
    reason: null,
    ...patch,
  };
}

async function show(packs: PackInfo[]) {
  ipcMock.listPacks.mockResolvedValue(packs);
  await act(async () => {
    render(<BackendDownloads onChanged={() => undefined} />);
  });
}

beforeEach(() => {
  ipcMock.removePack.mockClear();
});
afterEach(cleanup);

describe("a download from an older build of the app", () => {
  it("says nothing when there is none", async () => {
    await show([pack()]);
    expect(screen.queryByText(/older/i)).toBeNull();
    expect(screen.queryByRole("button", { name: /old/i })).toBeNull();
  });

  /** An app update moves the build; the folder carries it. Nothing says why the pick fails. */
  it("says it can't be used and to download it again, when the new build is not there", async () => {
    await show([pack({ olderBytes: 700_000_000 })]);
    expect(
      screen.getByText(/download on this computer is from an older version of the app, so it can't be used/i),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Download" })).toBeTruthy();
  });

  it("offers to remove the old one, and removes only the older builds", async () => {
    await show([pack({ olderBytes: 700_000_000 })]);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Remove the old NVIDIA CUDA 13 download" }));
    });
    expect(ipcMock.removePack).toHaveBeenCalledWith("cuda", true);
    // The list is asked again so the line goes.
    expect(ipcMock.listPacks.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it("says what the old one still takes when the new build is installed, and keeps Remove for the new one", async () => {
    await show([pack({ state: "installed", olderBytes: 1_000_000_000 })]);
    expect(screen.getByText(/older download from before the update still takes up 1\.0 GB/i)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Remove the old NVIDIA CUDA 13 download" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Remove NVIDIA CUDA 13" })).toBeTruthy();
  });
});
