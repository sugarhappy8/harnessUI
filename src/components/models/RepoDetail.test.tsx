import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { FitResult, RepoView } from "../../lib/types";

// [0.0.7 P6] Download on a repository page. Made-up repository names only.

const ipcMock = vi.hoisted(() => ({
  isTauri: () => true,
  hubRepo: vi.fn<(repo: string, ctx: number | null) => Promise<RepoView>>(),
  startDownload: vi.fn<(repo: string, commit: string, label: string, withProjector: boolean) => Promise<string>>(),
}));
vi.mock("../../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("../../store/app", () => ({
  toErrorInfo: (e: unknown) =>
    typeof e === "object" && e !== null && "message" in e
      ? (e as { code: string; message: string; nextStep: string; details: string })
      : { code: "unexpected", message: String(e), nextStep: "", details: "" },
}));

const { RepoDetail } = await import("./RepoDetail");

const fit: FitResult = {
  verdict: "green",
  neededBytes: 1,
  availableBytes: 2,
  suggestedCtx: null,
  partialOffload: false,
  device: "graphicsCard",
  computerBytes: 0,
  computerFreeBytes: 0,
  computerShort: false,
  sentence: "Fits easily.",
};

const VIEW: RepoView = {
  id: "someone/alpha-GGUF",
  commit: "a".repeat(40),
  gated: "no",
  license: null,
  architecture: "sample",
  architectureSupported: true,
  contextLength: 32768,
  parameters: 4e9,
  templateTools: true,
  templateThinking: false,
  projector: { path: "mmproj-alpha-F16.gguf", size: 9e8, sha256: "1".repeat(64) },
  variants: ["Q4_K_M", "Q8_0"].map((label) => ({
    label,
    files: [{ path: `alpha-${label}.gguf`, size: 1e9, sha256: "0".repeat(64) }],
    totalSize: 1e9,
    bits: null,
    fit,
    installed: false,
    hasChecksum: true,
  })),
  incomplete: [],
  card: null,
  fitContext: 8192,
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

async function show(onStarted: () => void) {
  ipcMock.hubRepo.mockResolvedValue(VIEW);
  render(<RepoDetail repoId="someone/alpha-GGUF" onBack={() => undefined} onStarted={onStarted} />);
  await act(async () => {
    await new Promise((r) => setTimeout(r, 5));
  });
  return screen.getAllByRole("listitem").map((row) => within(row).getByRole("button", { name: "Download" }));
}

describe("Download on a repository page", () => {
  it("starts one download for a double click, and is off until that start is done", async () => {
    let started: (id: string) => void = () => undefined;
    ipcMock.startDownload.mockImplementation(() => new Promise<string>((resolve) => (started = resolve)));
    const onStarted = vi.fn();
    const [q4, q8] = (await show(onStarted)) as HTMLButtonElement[];
    fireEvent.click(q4!);
    fireEvent.click(q4!);
    fireEvent.click(q8!);
    expect(ipcMock.startDownload).toHaveBeenCalledTimes(1);
    expect(ipcMock.startDownload).toHaveBeenCalledWith("someone/alpha-GGUF", "a".repeat(40), "Q4_K_M", true);
    expect(q4!.disabled).toBe(true);
    expect(q8!.disabled).toBe(true);
    await act(async () => started("group-1"));
    expect(onStarted).toHaveBeenCalledTimes(1);
    expect(q4!.disabled).toBe(false);
  });

  it("says why a start was refused, and Download works again", async () => {
    ipcMock.startDownload.mockRejectedValueOnce({
      code: "already_downloading",
      message: "One of this model's files is already being downloaded.",
      nextStep: "Wait until that download is done (see Downloads), then try again.",
      details: "",
    });
    const onStarted = vi.fn();
    const [, q8] = (await show(onStarted)) as HTMLButtonElement[];
    await act(async () => fireEvent.click(q8!));
    expect(screen.getByText("One of this model's files is already being downloaded.")).toBeTruthy();
    expect(onStarted).not.toHaveBeenCalled();
    expect(q8!.disabled).toBe(false);
  });
});
