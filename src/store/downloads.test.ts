// The app's download groups as Get local models follows them (0.1.0 Task B14): the list read
// once listening, each download-progress event, and a list asked for before an event never
// taking its place.
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DownloadGroup } from "../lib/types";

const ipc = vi.hoisted(() => ({
  isTauri: vi.fn(() => true),
  listDownloads: vi.fn<() => Promise<DownloadGroup[]>>(),
  onDownloadProgress: vi.fn<(handler: (g: DownloadGroup) => void) => Promise<() => void>>(),
}));
vi.mock("../lib/ipc", () => ipc);

const { followDownloads, useDownloads } = await import("./downloads");

function group(patch: Partial<DownloadGroup> = {}): DownloadGroup {
  return {
    id: "g1",
    repo: "leejet/Z-Image-Turbo-GGUF",
    label: "Z-Image Turbo",
    fileCount: 3,
    bytesDone: 0,
    bytesTotal: 100,
    status: "downloading",
    error: null,
    bytesPerSecond: null,
    unverified: false,
    generatorKind: "image",
    genModelId: "z-image-turbo-q4",
    ...patch,
  };
}

async function settle() {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

let progress: (g: DownloadGroup) => void = () => undefined;
const unlisten = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  ipc.isTauri.mockReturnValue(true);
  useDownloads.setState({ groups: [] });
  ipc.listDownloads.mockResolvedValue([]);
  ipc.onDownloadProgress.mockImplementation((handler) => {
    progress = handler;
    return Promise.resolve(unlisten);
  });
});

describe("the app's downloads", () => {
  it("follows the list once listening, then each event, until stopped", async () => {
    ipc.listDownloads.mockResolvedValue([group({ id: "old", status: "done" })]);
    const heard = vi.fn();
    const stop = followDownloads(heard);
    await settle();
    expect(useDownloads.getState().groups).toEqual([group({ id: "old", status: "done" })]);
    progress(group({ bytesDone: 40 }));
    progress(group({ bytesDone: 60 }));
    expect(useDownloads.getState().groups).toEqual([group({ bytesDone: 60 }), group({ id: "old", status: "done" })]);
    expect(heard).toHaveBeenLastCalledWith(group({ bytesDone: 60 }));
    stop();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("a list asked for before an event never takes its place", async () => {
    let answer: (groups: DownloadGroup[]) => void = () => undefined;
    ipc.listDownloads.mockReturnValue(
      new Promise((resolve) => {
        answer = resolve;
      }),
    );
    const reading = useDownloads.getState().read();
    useDownloads.getState().put(group({ status: "done", bytesDone: 100 }));
    answer([group({ bytesDone: 50 }), group({ id: "other" })]);
    await reading;
    expect(useDownloads.getState().groups).toEqual([group({ status: "done", bytesDone: 100 }), group({ id: "other" })]);
  });

  it("outside the app it follows nothing", () => {
    ipc.isTauri.mockReturnValue(false);
    followDownloads()();
    expect(ipc.onDownloadProgress).not.toHaveBeenCalled();
    expect(ipc.listDownloads).not.toHaveBeenCalled();
  });
});
