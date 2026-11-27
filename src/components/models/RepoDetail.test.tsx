import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { FitResult, RepoView, UpdateStatus } from "../../lib/types";

// [0.0.7 P6] Download on a repository page. Made-up repository names only.

const ipcMock = vi.hoisted(() => ({
  isTauri: () => true,
  hubRepo: vi.fn<(repo: string, ctx: number | null) => Promise<RepoView>>(),
  startDownload: vi.fn<(repo: string, commit: string, label: string, withProjector: boolean) => Promise<string>>(),
  checkForUpdate: vi.fn<() => Promise<unknown>>(),
  updateStatus: vi.fn<() => Promise<UpdateStatus>>(),
  onUpdateState: vi.fn(async () => () => undefined),
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

// ---- A model whose kind this version can't open yet ----------------------------------
//
// The owner's photo: a red box saying the files "would not load", over a memory verdict on
// every row ("Too large", "Fits with limits"), a ticked box offering the image file, and
// four dimmed Download buttons with no word beside them. The page must say one true thing.

/** The sizes of the owner's repository (its real file list, 2026-11-27) under another name. */
const SIZES: [string, number][] = [
  ["Q8_0", 27_820_341_856],
  ["Q6_K", 23_490_083_680],
  ["Q5_K_M", 19_906_679_008],
  ["Q5_K_S", 18_628_899_040],
];

const red: FitResult = { ...fit, verdict: "red", sentence: "Too large." };
const yellow: FitResult = { ...fit, verdict: "yellow", sentence: "Fits with limits." };

const CANT_OPEN_VIEW: RepoView = {
  ...VIEW,
  id: "someone/inkling-next-GGUF",
  architecture: "futurearch",
  architectureSupported: false,
  contextLength: 1_048_576,
  parameters: 26e9,
  projector: { path: "mmproj-F16.gguf", size: 639_348_160, sha256: "1".repeat(64) },
  variants: SIZES.map(([label, size], i) => ({
    label,
    files: [{ path: `next-${label}.gguf`, size, sha256: "0".repeat(64) }],
    totalSize: size,
    bits: null,
    fit: i === 0 ? red : yellow,
    installed: false,
    hasChecksum: true,
  })),
};

const STATUS: UpdateStatus = {
  configured: true,
  autoCheck: true,
  currentVersion: "0.2.1",
  checking: false,
  checked: true,
  downloading: null,
  available: null,
  ready: null,
  error: null,
};

async function open(view: RepoView) {
  ipcMock.hubRepo.mockResolvedValue(view);
  render(<RepoDetail repoId={view.id} onBack={() => undefined} onStarted={() => undefined} />);
  await act(async () => {
    await new Promise((r) => setTimeout(r, 5));
  });
}

describe("the page of a model this version can't open yet", () => {
  it("says it once, in plain words, and whose fault it is not", async () => {
    await open(CANT_OPEN_VIEW);
    const box = screen.getByRole("region", { name: "This version can't open this model yet" });
    expect(within(box).getByText("This version can't open this model yet")).toBeTruthy();
    expect(box.textContent).toContain("It is a “futurearch” model.");
    expect(box.textContent).toContain("Nothing is wrong with your computer.");
    expect(box.textContent).toContain("the model may work then. Nothing is downloaded until it can open.");
    // The words the owner met: the library's name and "architecture".
    expect(document.body.textContent).not.toMatch(/llama\.cpp|architecture|Pick a different model/i);
    // Said once: no second box and no alert.
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getAllByText("This version can't open this model yet")).toHaveLength(1);
  });

  it("gives no memory verdict, which would say the opposite", async () => {
    await open(CANT_OPEN_VIEW);
    expect(screen.queryByText(/Fits|Too large/)).toBeNull();
    expect(screen.queryByText(/Memory estimates assume/)).toBeNull();
    expect(screen.getByText("Memory use is worked out once this version can open the model.")).toBeTruthy();
    // What lets a person judge whether it is worth waiting for stays: the sizes.
    const rows = screen.getAllByRole("listitem");
    expect(rows).toHaveLength(4);
    expect(within(rows[0]!).getByText("27.8 GB")).toBeTruthy();
    expect(within(rows[2]!).getByText("19.9 GB")).toBeTruthy();
  });

  it("offers no image file, since nothing is downloaded", async () => {
    await open(CANT_OPEN_VIEW);
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByText(/image file/i)).toBeNull();
  });

  it("puts words where Download would be, and no dimmed button", async () => {
    await open(CANT_OPEN_VIEW);
    const rows = screen.getAllByRole("listitem");
    for (const row of rows) {
      expect(within(row).queryByRole("button", { name: "Download" })).toBeNull();
      expect(within(row).getByText("Can't open yet")).toBeTruthy();
      // Every button left is enabled: Add to comparison still works.
      for (const b of within(row).queryAllByRole("button")) expect((b as HTMLButtonElement).disabled).toBe(false);
      expect(within(row).getByRole("button", { name: /Add to comparison/ })).toBeTruthy();
    }
    expect(ipcMock.startDownload).not.toHaveBeenCalled();
  });

  it("keeps saying a file has no published checksum, since that stays true", async () => {
    await open({
      ...CANT_OPEN_VIEW,
      variants: CANT_OPEN_VIEW.variants.map((v, i) => (i === 1 ? { ...v, hasChecksum: false } : v)),
    });
    expect(screen.getAllByText("No checksum is published for this file.")).toHaveLength(1);
  });

  it("marks the kind in the header as the problem, not just another label", async () => {
    await open(CANT_OPEN_VIEW);
    const badges = document.querySelectorAll('[data-tone="danger"]');
    expect(badges).toHaveLength(1);
    expect(badges[0]!.textContent).toBe("futurearch");
    // The other badges are facts and keep their look.
    expect(screen.getByText("26B params").closest("[data-tone]")).toBeNull();
  });

  it("shows the kind's own text with its invisible characters made visible", async () => {
    await open({ ...CANT_OPEN_VIEW, architecture: "fu\u202Eture" });
    const box = screen.getByRole("region", { name: "This version can't open this model yet" });
    expect(box.textContent).toContain("It is a “fu[U+202E]ture” model.");
  });

  it("says plainly when the file names no kind, and offers no update for that", async () => {
    await open({ ...CANT_OPEN_VIEW, architecture: null });
    const box = screen.getByRole("region", { name: "This version can't open this model yet" });
    expect(box.textContent).toContain("This file does not say what kind of model it is, so this version can't open it.");
    expect(box.textContent).not.toContain("a newer model runner");
    expect(within(box).queryByRole("button", { name: "Check for updates" })).toBeNull();
  });

  it("answers Check for updates on the page, and says when nothing newer exists", async () => {
    ipcMock.checkForUpdate.mockResolvedValue(null);
    ipcMock.updateStatus.mockResolvedValue(STATUS);
    await open(CANT_OPEN_VIEW);
    const button = screen.getByRole("button", { name: "Check for updates" });
    await act(async () => fireEvent.click(button));
    expect(ipcMock.checkForUpdate).toHaveBeenCalledTimes(1);
    expect(
      screen.getByText("You have the latest version (0.2.1). It can't open this model yet, so check again later."),
    ).toBeTruthy();
    expect((screen.getByRole("button", { name: "Check for updates" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("says when a newer version was found and is being downloaded", async () => {
    ipcMock.checkForUpdate.mockResolvedValue({ version: "0.3.0", date: null, notes: null });
    ipcMock.updateStatus.mockResolvedValue({
      ...STATUS,
      downloading: "0.3.0",
      available: { version: "0.3.0", date: null, notes: null },
    });
    await open(CANT_OPEN_VIEW);
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Check for updates" })));
    expect(screen.getByText("Downloading version 0.3.0…")).toBeTruthy();
  });

  it("says why a check did not work, once, and lets the person try again", async () => {
    ipcMock.checkForUpdate.mockRejectedValue({
      code: "update_check",
      message: "The update check failed.",
      nextStep: "",
      details: "",
    });
    ipcMock.updateStatus.mockResolvedValue({ ...STATUS, checked: false, error: "Could not reach github.com." });
    await open(CANT_OPEN_VIEW);
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Check for updates" })));
    expect(screen.getByText(/Could not reach github\.com\./)).toBeTruthy();
    expect(screen.getAllByText(/Could not reach github\.com\./)).toHaveLength(1);
    expect((screen.getByRole("button", { name: "Check for updates" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("says when this copy cannot update itself", async () => {
    ipcMock.updateStatus.mockResolvedValue({ ...STATUS, configured: false, checked: false });
    await open(CANT_OPEN_VIEW);
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Check for updates" })));
    // Nothing to ask: the check is not started on a copy that has no updater.
    expect(ipcMock.checkForUpdate).not.toHaveBeenCalled();
    expect(
      screen.getByText("This copy can't update itself. New versions are on the project's GitHub releases page."),
    ).toBeTruthy();
  });
});

describe("the page of a picture or video model", () => {
  const PICTURE: RepoView = { ...CANT_OPEN_VIEW, architecture: "flux", pictureModel: true, projector: null };

  it("says what it is, not that an update may open it", async () => {
    await open(PICTURE);
    const box = screen.getByRole("region", { name: "This is a picture or video model" });
    expect(box.textContent).toContain("It can't chat. Add it in Studio → Get models.");
    expect(box.textContent).not.toMatch(/newer|update|runner|yet/i);
    expect(within(box).queryByRole("button")).toBeNull();
    expect(screen.queryByText("This version can't open this model yet")).toBeNull();
  });

  it("puts 'Not for chat' where Download would be, with no memory verdict", async () => {
    await open(PICTURE);
    const rows = screen.getAllByRole("listitem");
    for (const row of rows) {
      expect(within(row).getByText("Not for chat")).toBeTruthy();
      expect(within(row).queryByRole("button", { name: "Download" })).toBeNull();
    }
    expect(screen.queryByText(/Fits|Too large|Can't open yet/)).toBeNull();
    expect(screen.queryByRole("checkbox")).toBeNull();
    // "Worked out once this version can open the model" would promise what is not true.
    expect(screen.queryByText(/Memory/)).toBeNull();
  });
});

describe("the page of a model this version can open", () => {
  it("is as it was: image file offered, fit shown, Download on", async () => {
    await open({ ...VIEW, variants: VIEW.variants.map((v) => ({ ...v, fit: yellow })) });
    expect(screen.queryByRole("region", { name: "This version can't open this model yet" })).toBeNull();
    expect(screen.getByRole("checkbox")).toBeTruthy();
    expect(screen.getByText(/Memory estimates assume the model remembers 8,192 tokens/)).toBeTruthy();
    expect(screen.getAllByText("Fits with limits.", { exact: false }).length).toBeGreaterThan(0);
    for (const b of screen.getAllByRole("button", { name: "Download" })) expect((b as HTMLButtonElement).disabled).toBe(false);
    expect(screen.queryByText("Can't open yet")).toBeNull();
    expect(screen.getByText("sample").closest("[data-tone]")).toBeNull();
  });

  it("is as it was when nobody knows (null), too", async () => {
    await open({ ...VIEW, architectureSupported: null });
    expect(screen.queryByRole("region", { name: "This version can't open this model yet" })).toBeNull();
    expect(screen.getAllByRole("button", { name: "Download" })).toHaveLength(2);
  });
});
