import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type {
  ErrorInfo,
  ReverseComponent,
  ReverseComponentStatus,
  ReverseDecompiler,
  ReverseProgress,
  ReverseStatus,
} from "../../lib/types";

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  reverseStatus: vi.fn<() => Promise<ReverseStatus>>(),
  reverseDownload: vi.fn<(c: ReverseComponent, accept: boolean) => Promise<void>>(async () => undefined),
  reverseCancel: vi.fn<(c: ReverseComponent) => Promise<void>>(async () => undefined),
  reverseDiscardPartial: vi.fn<(c: ReverseComponent) => Promise<ReverseStatus>>(),
  reverseRemove: vi.fn<(c: ReverseComponent, force: boolean) => Promise<ReverseStatus>>(),
  reverseSetDecompiler: vi.fn<(d: ReverseDecompiler, force: boolean) => Promise<ReverseStatus>>(),
  reverseClearCache: vi.fn<(force: boolean) => Promise<ReverseStatus>>(),
  reverseInstallAppleTools: vi.fn<() => Promise<void>>(async () => undefined),
  onReverseProgress: vi.fn<(h: (p: ReverseProgress) => void) => Promise<() => void>>(),
}));
vi.mock("../../lib/ipc", () => ipcMock);
const clipboard = vi.hoisted(() => ({ writeText: vi.fn<(text: string) => Promise<void>>(async () => undefined) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => clipboard);
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const { Reverse, POLL_MS, downloadAsk, removeAsk } = await import("./Reverse");

const browserClipboard = vi.fn<(text: string) => Promise<void>>(async () => undefined);
Object.defineProperty(navigator, "clipboard", { value: { writeText: browserClipboard }, configurable: true });

const INTRO =
  "Lets the model look inside programs and other files you point it at: what a program is made of, the text and functions inside it, and readable code worked out from its machine code (decompiling). It uses rea, a free tool, with a decompiler. Nothing is downloaded until you choose it here. After that it works without the internet.";
const OWN_FILES =
  "Use it only on files you own or are allowed to examine. Some licences and some countries restrict reverse engineering.";
const CRAFTED =
  "A file made to attack decompilers could still harm this computer. Open programs from people you don't trust only on a computer you can wipe.";
const REACH =
  "In Ask first and Allow all, rea reaches only the chat's approved folders. In Full access it reaches any file except harnessUI's own. rea never runs the files it looks at, and it is never connected to programs that are running.";
const NO_DECOMPILER =
  "Install a decompiler to open programs. Checks that need none (for example, which functions a Windows program uses from other libraries) work with rea alone.";
const HOW_TO_USE =
  "To use it, ask about a program file in one of the chat's folders, or drop the file on the message box. If the model doesn't use rea, turn on Reverse engineering in the chat's Tools panel.";
const BUSY_QUESTION = "rea is working on a file in a chat. Stop it and continue?";
const GHIDRA_ROW =
  "Ghidra 12.1.4, with Java 21 (Eclipse Temurin). Made by the NSA and widely used. Slower: each file can take a few minutes to open, and it uses up to about 3 GB of memory while it works.";
const GHIDRA_16GB = "With a large model loaded, your computer may slow down while Ghidra works.";
const GHIDRA_WINDOWS =
  "On Windows, rea's Ghidra support is new and opens Windows programs and libraries (.exe and .dll) on this PC's own drives only.";
const KUNA_LINUX =
  "Kuna 1.990. Small and fast; its makers call it an experiment. Opens Linux programs and Mac programs built for one kind of chip, not Windows programs.";
const KUNA_MAC =
  "Kuna 1.990. Small and fast; its makers call it an experiment. It can't open most Mac apps, because they are built for both Apple and Intel chips, or Windows programs.";
const APPLE_INTRO =
  "On a Mac, rea needs Apple's command line tools before it can be used. They are free and come from Apple (a large download).";
const APPLE_ASKED = "Follow Apple's window to install them. This page updates when they are ready.";
const APPLE_FIRST = "Can't be used until Apple's command line tools are installed (step 1 above).";

const LICENSES: Record<ReverseComponent, ReverseComponentStatus["licenses"]> = {
  rea: [
    { part: "rea", name: "MIT", url: "https://github.com/morluto/rea/blob/main/LICENSE" },
    { part: "node", name: "MIT", url: "https://github.com/nodejs/node/blob/main/LICENSE" },
  ],
  kuna: [{ part: "kuna", name: "Apache-2.0", url: "https://github.com/Noelo-Lab/kuna/blob/main/LICENSE" }],
  ghidra: [
    { part: "ghidra", name: "Apache-2.0", url: "https://github.com/NationalSecurityAgency/ghidra/blob/master/LICENSE" },
    { part: "java", name: "GPL-2.0 WITH Classpath-exception-2.0", url: "https://openjdk.org/legal/gplv2+ce.html" },
  ],
};

const BASE: Record<ReverseComponent, ReverseComponentStatus> = {
  rea: {
    component: "rea",
    available: true,
    reason: null,
    state: { kind: "notInstalled" },
    installedVersion: null,
    pinnedVersion: "6.9.0",
    parts: [
      { name: "rea", version: "6.9.0" },
      { name: "node", version: "24.22.3" },
    ],
    downloadBytes: 80e6,
    diskBytes: 310e6,
    update: null,
    licenses: LICENSES.rea,
  },
  kuna: {
    component: "kuna",
    available: true,
    reason: null,
    state: { kind: "notInstalled" },
    installedVersion: null,
    pinnedVersion: "1.990",
    parts: [{ name: "kuna", version: "1.990" }],
    downloadBytes: 25e6,
    diskBytes: 55e6,
    update: null,
    licenses: LICENSES.kuna,
  },
  ghidra: {
    component: "ghidra",
    available: true,
    reason: null,
    state: { kind: "notInstalled" },
    installedVersion: null,
    pinnedVersion: "12.1.4",
    parts: [
      { name: "ghidra", version: "12.1.4" },
      { name: "java", version: "21.0.13+9" },
    ],
    downloadBytes: 775e6,
    diskBytes: 1_250e6,
    update: null,
    licenses: LICENSES.ghidra,
  },
};

type RowPatch = Partial<Record<ReverseComponent, Partial<ReverseComponentStatus>>>;

function status(patch: Partial<ReverseStatus> = {}, rows: RowPatch = {}): ReverseStatus {
  return {
    components: (["rea", "kuna", "ghidra"] as ReverseComponent[]).map((c) => ({ ...BASE[c], ...rows[c] })),
    decompiler: null,
    chosen: null,
    appleTools: "notNeeded",
    running: { kind: "stopped" },
    cacheBytes: 0,
    memoryGb: 32,
    os: "linux",
    partSizes: [],
    ...patch,
  };
}

const INSTALLED = { state: { kind: "installed" as const }, installedVersion: "x" };
const WINDOWS_KUNA: Partial<ReverseComponentStatus> = { available: false, reason: "rea can't use Kuna on Windows yet." };
const MAC_GHIDRA: Partial<ReverseComponentStatus> = { available: false, reason: "Not available on Mac in this version." };

function busyError(): ErrorInfo {
  return { code: "busy", message: "rea is working on a file in a chat.", nextStep: "Stop it and continue?", details: "" };
}

let progress: ((p: ReverseProgress) => void) | null = null;

function event(patch: Partial<ReverseProgress>): ReverseProgress {
  return {
    component: "rea",
    part: "rea",
    done: 0,
    total: 80e6,
    stage: "downloading",
    partDone: 0,
    partTotal: 42e6,
    finished: false,
    error: null,
    ...patch,
  };
}

async function show(s: ReverseStatus) {
  ipcMock.reverseStatus.mockResolvedValue(s);
  render(<Reverse />);
  await act(async () => {});
}

function rowOf(name: string): HTMLElement {
  return screen.getByRole("listitem", { name });
}

beforeEach(() => {
  progress = null;
  ipcMock.onReverseProgress.mockImplementation(async (h) => {
    progress = h;
    return () => {
      progress = null;
    };
  });
  for (const m of [ipcMock.reverseDiscardPartial, ipcMock.reverseRemove, ipcMock.reverseSetDecompiler, ipcMock.reverseClearCache]) {
    m.mockReset();
  }
  ipcMock.reverseDownload.mockClear();
  ipcMock.reverseCancel.mockClear();
  ipcMock.reverseInstallAppleTools.mockClear();
  ipcMock.reverseStatus.mockReset();
  clipboard.writeText.mockClear();
  browserClipboard.mockClear();
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("Reverse engineering, nothing installed (Linux)", () => {
  it("says what it does, the rules and the risk, before anything is downloaded", async () => {
    await show(status());
    for (const text of [INTRO, CRAFTED, REACH]) expect(screen.getByText(text)).toBeTruthy();
    expect(screen.getByText(OWN_FILES, { exact: false })).toBeTruthy();
    expect(screen.getByRole("button", { name: "rea's notes on responsible use" })).toBeTruthy();
    expect(screen.getByText("rea 6.9.0, with Node.js 24.22.3 to run it")).toBeTruthy();
    expect(within(rowOf("rea")).getByText("80 MB download · 310 MB on disk")).toBeTruthy();
    expect(screen.getByText(KUNA_LINUX)).toBeTruthy();
    expect(screen.getByText(GHIDRA_ROW)).toBeTruthy();
    expect(
      screen.getByText(
        "A decompiler turns a program's machine code back into code people can read. Install one, or both and switch between them.",
      ),
    ).toBeTruthy();
    expect(screen.getByText(NO_DECOMPILER)).toBeTruthy();
    expect(within(rowOf("rea")).getByRole("button", { name: "Download" })).toBeTruthy();
    // Nothing about running or using it until rea is there; no Mac step elsewhere.
    expect(screen.queryByText("rea is not running.")).toBeNull();
    expect(screen.queryByText(HOW_TO_USE)).toBeNull();
    expect(screen.queryByText(/Apple's command line tools/)).toBeNull();
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(ipcMock.reverseDownload).not.toHaveBeenCalled();
  });

  it("asks before downloading rea, names the hosts and licences, and starts only on Accept", async () => {
    await show(status());
    fireEvent.click(within(rowOf("rea")).getByRole("button", { name: "Download" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Download rea?")).toBeTruthy();
    expect(
      within(dialog).getByText(
        "About 80 MB from npmjs.com (rea) and nodejs.org (Node.js). rea is under the MIT License and includes other open-source software listed in its notices. Node.js is under the MIT License. Downloading means you accept these terms.",
      ),
    ).toBeTruthy();
    expect(within(dialog).getByRole("button", { name: "rea license" })).toBeTruthy();
    expect(within(dialog).getByRole("button", { name: "Node.js license" })).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(ipcMock.reverseDownload).not.toHaveBeenCalled();

    fireEvent.click(within(rowOf("rea")).getByRole("button", { name: "Download" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Accept and download" }));
    await act(async () => {});
    expect(ipcMock.reverseDownload).toHaveBeenCalledWith("rea", true);
  });

  it("a licence link goes through the destination-confirm dialog", async () => {
    await show(status());
    fireEvent.click(within(rowOf("rea")).getByRole("button", { name: "Download" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "rea license" }));
    expect(await screen.findByText("Open this link?")).toBeTruthy();
    expect(screen.getByText("github.com")).toBeTruthy();
  });

  it("a decompiler's download brings rea along in one dialog with the total", async () => {
    await show(status());
    fireEvent.click(within(rowOf("Kuna")).getByRole("button", { name: "Download" }));
    let dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Download rea and Kuna?")).toBeTruthy();
    expect(
      within(dialog).getByText(
        "About 105 MB: rea from npmjs.com and Node.js from nodejs.org, then Kuna from Kuna's releases on GitHub. rea is under the MIT License and includes other open-source software listed in its notices. Node.js is under the MIT License. Kuna is under the Apache License 2.0. Downloading means you accept these terms.",
      ),
    ).toBeTruthy();
    expect(within(dialog).getAllByRole("button", { name: /license$/ }).map((b) => b.textContent)).toEqual([
      "rea license",
      "Node.js license",
      "Kuna license",
    ]);
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    fireEvent.click(within(rowOf("Ghidra")).getByRole("button", { name: "Download" }));
    dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Download rea and Ghidra?")).toBeTruthy();
    expect(
      within(dialog).getByText(
        "About 855 MB: rea from npmjs.com and Node.js from nodejs.org, then Ghidra from the NSA's releases on GitHub and Java from Eclipse Adoptium's. rea is under the MIT License and includes other open-source software listed in its notices. Node.js is under the MIT License. Ghidra is under the Apache License 2.0, with a few parts under the GPL 3. Eclipse Temurin is under the GPL 2 with the Classpath Exception. Downloading means you accept these terms.",
      ),
    ).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Accept and download" }));
    await act(async () => {});
    expect(ipcMock.reverseDownload).toHaveBeenCalledWith("ghidra", true);
  });

  it("each decompiler's own dialog once rea is there", () => {
    const s = status({}, { rea: INSTALLED });
    expect(downloadAsk(s, "kuna")).toMatchObject({
      title: "Download Kuna?",
      text: "About 25 MB from Kuna's releases on GitHub. Kuna is under the Apache License 2.0. Downloading means you accept these terms.",
    });
    expect(downloadAsk(s, "ghidra")).toMatchObject({
      title: "Download Ghidra and Java?",
      text: "About 775 MB: Ghidra from the NSA's releases on GitHub and Java from Eclipse Adoptium's. Ghidra is under the Apache License 2.0, with a few parts under the GPL 3. Eclipse Temurin is under the GPL 2 with the Classpath Exception. Downloading means you accept these terms.",
    });
    expect(downloadAsk(s, "ghidra").licenses.map((l) => l.part)).toEqual(["ghidra", "java"]);
  });
});

describe("downloads", () => {
  it("shows downloading, checking and unpacking with a bar per part, and Cancel stops it", async () => {
    await show(status());
    fireEvent.click(within(rowOf("Kuna")).getByRole("button", { name: "Download" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Accept and download" }));
    await act(async () => {});
    expect(within(rowOf("Kuna")).getByText("Downloading… 0 MB of 105 MB")).toBeTruthy();
    // rea's row says it comes along.
    expect(within(rowOf("rea")).getByText("Comes with the Kuna download.")).toBeTruthy();

    await act(async () => progress?.(event({ component: "kuna", part: "rea", done: 23e6, total: 105e6, partDone: 23e6, partTotal: 42e6 })));
    const kuna = rowOf("Kuna");
    expect(within(kuna).getByText("Downloading… 23 MB of 105 MB")).toBeTruthy();
    expect(within(kuna).getByRole("progressbar", { name: "rea" }).getAttribute("aria-valuenow")).toBe("55");
    expect(within(kuna).getByText("23 MB of 42 MB")).toBeTruthy();

    await act(async () =>
      progress?.(event({ component: "kuna", part: "node", done: 50e6, total: 105e6, partDone: 8e6, partTotal: 38e6 })),
    );
    expect(within(kuna).getByRole("progressbar", { name: "rea" }).getAttribute("aria-valuenow")).toBe("100");
    expect(within(kuna).getByRole("progressbar", { name: "Node.js" }).getAttribute("aria-valuenow")).toBe("21");

    await act(async () => progress?.(event({ component: "kuna", part: "kuna", stage: "checking", done: 80e6, total: 105e6 })));
    expect(within(kuna).getByText("Checking…", { selector: "p" })).toBeTruthy();
    await act(async () =>
      progress?.(event({ component: "kuna", part: "kuna", stage: "unpacking", done: 10e6, total: 54e6, partDone: 10e6, partTotal: 54e6 })),
    );
    expect(within(kuna).getByText("Unpacking…", { selector: "p" })).toBeTruthy();

    fireEvent.click(within(kuna).getByRole("button", { name: "Cancel" }));
    expect(ipcMock.reverseCancel).toHaveBeenCalledWith("kuna");

    // The last event: the row reads the disk again.
    ipcMock.reverseStatus.mockResolvedValue(status({ decompiler: "kuna", chosen: "kuna" }, { rea: INSTALLED, kuna: INSTALLED }));
    await act(async () => progress?.(event({ component: "kuna", part: "", finished: true })));
    expect(within(rowOf("Kuna")).getByText("Installed")).toBeTruthy();
    expect(screen.getByText("Decompiling uses Kuna.")).toBeTruthy();
    expect(screen.queryByText(/Downloading…/)).toBeNull();
  });

  it("a download already running when the page opens shows its step", async () => {
    await show(status({}, { rea: { state: { kind: "downloading", done: 23e6, total: 80e6, stage: "downloading" } } }));
    expect(within(rowOf("rea")).getByText("Downloading… 23 MB of 80 MB")).toBeTruthy();
    cleanup();
    await show(status({}, { rea: { state: { kind: "downloading", done: 0, total: 80e6, stage: "checking" } } }));
    expect(within(rowOf("rea")).getByText("Checking…")).toBeTruthy();
    cleanup();
    await show(status({}, { rea: { state: { kind: "downloading", done: 1, total: 9, stage: "unpacking" } } }));
    expect(within(rowOf("rea")).getByText("Unpacking…")).toBeTruthy();
    fireEvent.click(within(rowOf("rea")).getByRole("button", { name: "Cancel" }));
    expect(ipcMock.reverseCancel).toHaveBeenCalledWith("rea");
  });

  it("a paused download offers Resume and Remove", async () => {
    await show(status({}, { rea: { state: { kind: "partial", bytes: 23e6 } } }));
    const rea = rowOf("rea");
    expect(within(rea).getByText("Paused at 23 MB of 80 MB. Remove frees that space.")).toBeTruthy();
    fireEvent.click(within(rea).getByRole("button", { name: "Resume" }));
    await act(async () => {});
    expect(ipcMock.reverseDownload).toHaveBeenCalledWith("rea", true);
    cleanup();
    ipcMock.reverseDiscardPartial.mockResolvedValue(status());
    await show(status({}, { rea: { state: { kind: "partial", bytes: 23e6 } } }));
    fireEvent.click(within(rowOf("rea")).getByRole("button", { name: "Remove" }));
    await act(async () => {});
    expect(ipcMock.reverseDiscardPartial).toHaveBeenCalledWith("rea");
    expect(within(rowOf("rea")).getByRole("button", { name: "Download" })).toBeTruthy();
  });

  it.each([
    [{ code: "cant_reach", message: "Can't reach nodejs.org.", nextStep: "Check your internet connection and try again.", details: "" }],
    [
      {
        code: "mismatch",
        message: "The download doesn't match the copy harnessUI expects, so it wasn't installed.",
        nextStep: "Try again. If it keeps failing, report it.",
        details: "",
      },
    ],
    [
      {
        code: "interrupted",
        message: "The download was interrupted.",
        nextStep: "Check your connection and try again; it continues where it stopped.",
        details: "",
      },
    ],
    [{ code: "disk_full", message: "The disk is full.", nextStep: "You need 1.2 GB free.", details: "" }],
  ])("a failed download says why in plain words (%#)", async (error) => {
    await show(status());
    await act(async () => progress?.(event({ finished: true, error })));
    const alert = screen.getByRole("alert");
    expect(within(alert).getByText(error.message)).toBeTruthy();
    expect(within(alert).getByText(error.nextStep)).toBeTruthy();
  });

  it("a cancelled download shows nothing", async () => {
    await show(status());
    await act(async () =>
      progress?.(event({ finished: true, error: { code: "cancelled", message: "x", nextStep: "", details: "" } })),
    );
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("missing or blocked files are repaired from the error's Repair button", async () => {
    await show(status());
    await act(async () =>
      progress?.(
        event({
          component: "ghidra",
          finished: true,
          error: {
            code: "blocked",
            message: "Your antivirus blocked part of Ghidra.",
            nextStep: "Allow harnessUI in it, then choose Repair.",
            details: "",
          },
        }),
      ),
    );
    fireEvent.click(within(screen.getByRole("alert")).getByRole("button", { name: "Repair" }));
    await act(async () => {});
    expect(ipcMock.reverseDownload).toHaveBeenCalledWith("ghidra", true);
  });

  it("a rare path problem offers Report this, which copies the details", async () => {
    await show(status());
    await act(async () =>
      progress?.(
        event({
          component: "ghidra",
          finished: true,
          error: {
            code: "path_too_long",
            message: "The folder harnessUI keeps its files in has too long a path for Ghidra.",
            nextStep: "This is rare; please report it.",
            details: "the folder path is too long (260 characters, at most 250)",
          },
        }),
      ),
    );
    fireEvent.click(within(screen.getByRole("alert")).getByRole("button", { name: "Report this" }));
    await act(async () => {});
    expect(browserClipboard).toHaveBeenCalledWith(expect.stringContaining("at most 250"));
    expect(within(screen.getByRole("alert")).getByRole("button", { name: "Copied" })).toBeTruthy();
  });
});

describe("installed rows", () => {
  it("rea installed: versions, sizes, Remove, the running line and how to use it", async () => {
    await show(status({}, { rea: INSTALLED }));
    const rea = rowOf("rea");
    expect(within(rea).getByText("Installed")).toBeTruthy();
    expect(within(rea).getByText("rea 6.9.0, with Node.js 24.22.3 to run it")).toBeTruthy();
    expect(within(rea).getByRole("button", { name: "Remove" })).toBeTruthy();
    expect(screen.getByText("rea is not running.")).toBeTruthy();
    expect(screen.getByText(HOW_TO_USE)).toBeTruthy();
    expect(screen.getByText(NO_DECOMPILER)).toBeTruthy();
  });

  it("an update says what changes and how much, and keeps the old version until then", async () => {
    await show(status({}, { rea: { ...INSTALLED, update: { part: "rea", from: "6.9.0", to: "6.10.0", downloadBytes: 42e6 } } }));
    expect(
      screen.getByText(
        "Update available: rea 6.9.0 → 6.10.0. 42 MB. The version you have keeps working until you update.",
      ),
    ).toBeTruthy();
    fireEvent.click(within(rowOf("rea")).getByRole("button", { name: "Update" }));
    await act(async () => {});
    expect(ipcMock.reverseDownload).toHaveBeenCalledWith("rea", true);
  });

  it("both needs-update texts, each with Update", async () => {
    await show(status({}, { ghidra: { state: { kind: "needsUpdate", why: { kind: "securityFix", part: "java" } } } }));
    expect(screen.getByText("This version of Java has a known problem. Update it to keep using it.")).toBeTruthy();
    expect(within(rowOf("Ghidra")).getByRole("button", { name: "Update" })).toBeTruthy();
    cleanup();
    await show(
      status(
        {},
        {
          rea: {
            state: { kind: "needsUpdate", why: { kind: "notAccepted", need: "6.9.0" } },
            update: { part: "rea", from: "6.8.0", to: "6.9.0", downloadBytes: 42e6 },
          },
        },
      ),
    );
    expect(
      screen.getByText("This version of harnessUI needs rea 6.9.0. Until you update (42 MB), reverse engineering is off."),
    ).toBeTruthy();
    fireEvent.click(within(rowOf("rea")).getByRole("button", { name: "Update" }));
    await act(async () => {});
    expect(ipcMock.reverseDownload).toHaveBeenCalledWith("rea", true);
  });

  it("broken and blocked rows offer Repair", async () => {
    await show(status({}, { rea: { state: { kind: "broken", why: "missing" } } }));
    expect(screen.getByText("Part of rea is missing, perhaps removed by antivirus software.")).toBeTruthy();
    fireEvent.click(within(rowOf("rea")).getByRole("button", { name: "Repair" }));
    await act(async () => {});
    expect(ipcMock.reverseDownload).toHaveBeenCalledWith("rea", true);
    cleanup();
    await show(status({}, { rea: INSTALLED, ghidra: { state: { kind: "broken", why: "blocked" } } }));
    expect(screen.getByText("Your antivirus blocked part of Ghidra. Allow harnessUI in it, then choose Repair.")).toBeTruthy();
    expect(within(rowOf("Ghidra")).getByRole("button", { name: "Repair" })).toBeTruthy();
  });

  it("the running line follows rea's state, with the problem's details on request", async () => {
    await show(status({ running: { kind: "ready" }, decompiler: "ghidra", chosen: "ghidra" }, { rea: INSTALLED, ghidra: INSTALLED }));
    expect(screen.getByText("rea is running (Ghidra).")).toBeTruthy();
    cleanup();
    await show(status({ running: { kind: "busy", chatId: 3 } }, { rea: INSTALLED }));
    expect(screen.getByText("rea is working on a file…")).toBeTruthy();
    cleanup();
    await show(status({ running: { kind: "failed", reason: "node: [harnessUI's rea folder] <b>gone</b>" } }, { rea: INSTALLED }));
    expect(screen.getByText(/rea stopped with a problem\. It starts again the next time it is used\./)).toBeTruthy();
    expect(screen.queryByText(/gone/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Show details" }));
    // Plain text, never markup.
    expect(screen.getByText("node: [harnessUI's rea folder] <b>gone</b>")).toBeTruthy();
  });

  it("the remove questions say what goes and what it frees", async () => {
    const both = status({}, { rea: INSTALLED, kuna: INSTALLED, ghidra: INSTALLED });
    expect(removeAsk(both, "rea")).toEqual({
      title: "Remove rea?",
      text: "This also removes Kuna and Ghidra and frees 1.6 GB. You can download them again later.",
    });
    expect(removeAsk(status({}, { rea: INSTALLED }), "rea")).toEqual({
      title: "Remove rea?",
      text: "This frees 0.3 GB. You can download it again later.",
    });
    expect(removeAsk(both, "ghidra")).toEqual({ title: "Remove Ghidra?", text: "This frees 1.3 GB." });
    expect(removeAsk(both, "kuna")).toEqual({ title: "Remove Kuna?", text: "This frees 55 MB." });

    ipcMock.reverseRemove.mockResolvedValue(status({}, { rea: INSTALLED, ghidra: INSTALLED }));
    await show(both);
    fireEvent.click(within(rowOf("Kuna")).getByRole("button", { name: "Remove" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Remove Kuna?")).toBeTruthy();
    expect(within(dialog).getByText("This frees 55 MB.")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    await act(async () => {});
    expect(ipcMock.reverseRemove).toHaveBeenCalledWith("kuna", false);
    expect(within(rowOf("Kuna")).getByRole("button", { name: "Download" })).toBeTruthy();
  });
});

describe("the decompiler choice", () => {
  it("both installed: a choice; switching asks while rea works, both answers", async () => {
    const s = status({ decompiler: "kuna", chosen: "kuna" }, { rea: INSTALLED, kuna: INSTALLED, ghidra: INSTALLED });
    await show(s);
    const group = screen.getByRole("radiogroup", { name: "Use for decompiling:" });
    expect(within(group).getByRole("radio", { name: "Kuna" }).getAttribute("aria-checked")).toBe("true");
    ipcMock.reverseSetDecompiler.mockRejectedValueOnce(busyError());
    fireEvent.click(within(group).getByRole("radio", { name: "Ghidra" }));
    await act(async () => {});
    expect(ipcMock.reverseSetDecompiler).toHaveBeenCalledWith("ghidra", false);
    let dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(BUSY_QUESTION)).toBeTruthy();
    // Cancel: nothing else is sent.
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await act(async () => {});
    expect(ipcMock.reverseSetDecompiler).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("alert")).toBeNull();

    ipcMock.reverseSetDecompiler.mockRejectedValueOnce(busyError());
    ipcMock.reverseSetDecompiler.mockResolvedValueOnce({ ...s, decompiler: "ghidra", chosen: "ghidra" });
    fireEvent.click(within(screen.getByRole("radiogroup")).getByRole("radio", { name: "Ghidra" }));
    await act(async () => {});
    dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Stop and continue" }));
    await act(async () => {});
    expect(ipcMock.reverseSetDecompiler).toHaveBeenLastCalledWith("ghidra", true);
    expect(
      within(screen.getByRole("radiogroup")).getByRole("radio", { name: "Ghidra" }).getAttribute("aria-checked"),
    ).toBe("true");
  });

  it("removing what rea works with asks the same question and repeats with force", async () => {
    const s = status({ decompiler: "kuna", chosen: "kuna" }, { rea: INSTALLED, kuna: INSTALLED });
    ipcMock.reverseRemove.mockRejectedValueOnce(busyError());
    ipcMock.reverseRemove.mockResolvedValueOnce(status({}, { rea: INSTALLED }));
    await show(s);
    fireEvent.click(within(rowOf("Kuna")).getByRole("button", { name: "Remove" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Remove" }));
    await act(async () => {});
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Stop and continue" }));
    await act(async () => {});
    expect(ipcMock.reverseRemove.mock.calls).toEqual([
      ["kuna", false],
      ["kuna", true],
    ]);
  });

  it("one installed: no choice, just what is used; Kuna's saved analysis can be cleared", async () => {
    const s = status({ decompiler: "kuna", chosen: "kuna", cacheBytes: 120e6 }, { rea: INSTALLED, kuna: INSTALLED });
    await show(s);
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(screen.getByText("Decompiling uses Kuna.")).toBeTruthy();
    expect(
      screen.getByText("Saved analysis: 120 MB. Kuna keeps what it worked out so a file opens faster next time."),
    ).toBeTruthy();
    ipcMock.reverseClearCache.mockRejectedValueOnce(busyError());
    ipcMock.reverseClearCache.mockResolvedValueOnce({ ...s, cacheBytes: 0 });
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    await act(async () => {});
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Stop and continue" }));
    await act(async () => {});
    expect(ipcMock.reverseClearCache.mock.calls).toEqual([[false], [true]]);
    expect(screen.getByText(/^Saved analysis: 0 MB\./)).toBeTruthy();
  });

  it("one installed but broken: still says which one is used, beside its Repair", async () => {
    // `decompiler` is the usable one (none here); `chosen` is the owner's choice.
    await show(
      status({ decompiler: null, chosen: "ghidra" }, { rea: INSTALLED, ghidra: { state: { kind: "broken", why: "missing" } } }),
    );
    expect(screen.getByText("Decompiling uses Ghidra.")).toBeTruthy();
    expect(screen.queryByText(NO_DECOMPILER)).toBeNull();
    expect(within(rowOf("Ghidra")).getByRole("button", { name: "Repair" })).toBeTruthy();
  });

  it("no saved-analysis line without Kuna", async () => {
    await show(status({ decompiler: "ghidra", chosen: "ghidra" }, { rea: INSTALLED, ghidra: INSTALLED }));
    expect(screen.queryByText(/Saved analysis/)).toBeNull();
    expect(screen.getByText("Decompiling uses Ghidra.")).toBeTruthy();
  });
});

describe("what each computer offers", () => {
  it("Windows: Ghidra is the decompiler, Kuna greyed with its reason, no choice control", async () => {
    await show(
      status({ os: "windows", decompiler: "ghidra", chosen: "ghidra", memoryGb: 32 }, { rea: INSTALLED, kuna: WINDOWS_KUNA, ghidra: INSTALLED }),
    );
    expect(
      screen.getByText(
        "On Windows, Ghidra is the decompiler. It turns a program's machine code back into code people can read.",
      ),
    ).toBeTruthy();
    const kuna = rowOf("Kuna");
    expect(within(kuna).getByText("rea can't use Kuna on Windows yet.")).toBeTruthy();
    expect(within(kuna).queryByRole("button")).toBeNull();
    expect(screen.queryByText(/Small and fast/)).toBeNull();
    expect(screen.getByText(GHIDRA_WINDOWS)).toBeTruthy();
    expect(screen.queryByText(GHIDRA_16GB)).toBeNull();
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(screen.getByText("Decompiling uses Ghidra.")).toBeTruthy();
  });

  it("computers with 16 GB or less get the slow-down line", async () => {
    await show(status({ memoryGb: 16 }));
    expect(screen.getByText(GHIDRA_16GB)).toBeTruthy();
    cleanup();
    await show(status({ memoryGb: null }));
    expect(screen.queryByText(GHIDRA_16GB)).toBeNull();
  });

  it("Mac without Ghidra: Kuna is the decompiler, its limits said plainly, Ghidra greyed", async () => {
    await show(status({ os: "macos", appleTools: "present" }, { ghidra: MAC_GHIDRA }));
    expect(screen.getByText(KUNA_MAC)).toBeTruthy();
    expect(screen.queryByText(/Ghidra can open Mac apps/)).toBeNull();
    expect(within(rowOf("Ghidra")).getByText("Not available on Mac in this version.")).toBeTruthy();
    expect(
      screen.getByText("On a Mac, Kuna is the decompiler. It turns a program's machine code back into code people can read."),
    ).toBeTruthy();
  });

  it("Mac with Ghidra offered: Kuna's row points at Ghidra", async () => {
    await show(status({ os: "macos", appleTools: "present" }));
    expect(screen.getByText(`${KUNA_MAC} Ghidra can open Mac apps.`)).toBeTruthy();
  });

  it("an unsupported computer greys rea with its reason", async () => {
    await show(status({ os: "other" }, { rea: { available: false, reason: "Not available for this computer." } }));
    expect(within(rowOf("rea")).getByText("Not available for this computer.")).toBeTruthy();
    expect(within(rowOf("rea")).queryByRole("button")).toBeNull();
  });
});

describe("Apple's command line tools (Mac)", () => {
  it("step 1 with Install, then Installed on the next check without a restart", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const missing = status({ os: "macos", appleTools: "missing" }, { rea: INSTALLED, ghidra: MAC_GHIDRA });
    await show(missing);
    const step = screen.getByRole("region", { name: "Apple's command line tools" });
    expect(within(step).getByText(APPLE_INTRO)).toBeTruthy();
    expect(within(step).getByText("Not installed")).toBeTruthy();
    // rea cannot be used until then.
    expect(within(rowOf("rea")).getByText(APPLE_FIRST)).toBeTruthy();
    fireEvent.click(within(step).getByRole("button", { name: "Install" }));
    await act(async () => {});
    expect(ipcMock.reverseInstallAppleTools).toHaveBeenCalled();
    expect(within(step).getByText(APPLE_ASKED)).toBeTruthy();

    ipcMock.reverseStatus.mockResolvedValue({ ...missing, appleTools: "present" });
    await act(async () => {
      vi.advanceTimersByTime(POLL_MS);
    });
    await act(async () => {});
    expect(within(screen.getByRole("region", { name: "Apple's command line tools" })).getByText("Installed")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Install" })).toBeNull();
    expect(screen.queryByText(APPLE_FIRST)).toBeNull();
    expect(screen.queryByText(APPLE_ASKED)).toBeNull();
  });

  it("checks again when the window regains focus", async () => {
    const missing = status({ os: "macos", appleTools: "missing" });
    await show(missing);
    ipcMock.reverseStatus.mockResolvedValue({ ...missing, appleTools: "present" });
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(within(screen.getByRole("region", { name: "Apple's command line tools" })).getByText("Installed")).toBeTruthy();
  });

  it("is not shown on other computers", async () => {
    await show(status({ os: "windows" }, { kuna: WINDOWS_KUNA }));
    expect(screen.queryByRole("region", { name: "Apple's command line tools" })).toBeNull();
  });
});

describe("the page while it is open", () => {
  it("asks again every few seconds only while visible, and stops when closed", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    await show(status());
    expect(ipcMock.reverseStatus).toHaveBeenCalledTimes(1);
    await act(async () => {
      vi.advanceTimersByTime(POLL_MS);
    });
    expect(ipcMock.reverseStatus).toHaveBeenCalledTimes(2);
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    await act(async () => {
      vi.advanceTimersByTime(POLL_MS * 2);
    });
    expect(ipcMock.reverseStatus).toHaveBeenCalledTimes(2);
    visibility.mockRestore();
    cleanup();
    await act(async () => {
      vi.advanceTimersByTime(POLL_MS * 3);
    });
    expect(ipcMock.reverseStatus).toHaveBeenCalledTimes(2);
    expect(progress).toBeNull();
  });
});
