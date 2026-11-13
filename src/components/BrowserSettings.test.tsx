import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { AgentBrowserProgress, BrowserStatus, OwnBrowserStatus } from "../lib/types";

let statusNow: BrowserStatus;
let onProgress: (p: AgentBrowserProgress) => void = () => undefined;

const ipcMock = vi.hoisted(() => ({
  browserStatus: vi.fn<() => Promise<BrowserStatus>>(),
  setBrowserSettings: vi.fn(async () => undefined),
  chooseBrowserPath: vi.fn(async () => null),
  downloadAgentBrowser: vi.fn(async () => undefined),
  cancelAgentBrowserDownload: vi.fn(async () => undefined),
  removeAgentBrowser: vi.fn(async () => undefined),
  onAgentBrowserProgress: vi.fn(async (h: (p: AgentBrowserProgress) => void) => {
    onProgress = h;
    return () => undefined;
  }),
}));
vi.mock("../lib/ipc", () => ipcMock);

const { BrowserSettings } = await import("./BrowserSettings");

const NOT_FOUND =
  "The browser tool needs Chrome, Edge, Brave, Vivaldi or Chromium, and none was found. Install Google Chrome or Microsoft Edge, or download a browser for the agent in Settings → Browser.";
const DOWNLOAD = "Download a browser for the agent (about 200 MB)";

function own(over: Partial<OwnBrowserStatus> = {}): OwnBrowserStatus {
  return {
    offered: true,
    state: "notInstalled",
    version: null,
    pinnedVersion: "157.0.8103.50",
    downloadBytes: 196583703,
    bytesDone: 0,
    inUse: false,
    ...over,
  };
}

function status(over: Partial<BrowserStatus> = {}): BrowserStatus {
  return {
    settings: { path: null, loopbackPort: false, allowPrivateNetworks: false, strictSites: [] },
    found: null,
    kind: null,
    unavailable: NOT_FOUND,
    running: false,
    own: own(),
    ...over,
  };
}

async function show() {
  await act(async () => {
    render(<BrowserSettings />);
  });
}

describe("BrowserSettings: the agent's own browser", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    statusNow = status();
    ipcMock.browserStatus.mockImplementation(async () => statusNow);
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("offers the download when no browser is found and says what it is", async () => {
    await show();
    expect(screen.getByText(NOT_FOUND)).toBeTruthy();
    expect(screen.getByText(/Safari cannot be driven by the agent/)).toBeTruthy();
    expect(screen.getByText(/Chrome for Testing/)).toBeTruthy();
    expect(screen.getByText(/does not update itself/)).toBeTruthy();
    expect(screen.getByText(/install Google Chrome or Microsoft\s+Edge/)).toBeTruthy();
    // The path control stays.
    expect(screen.getByRole("button", { name: "Choose a browser…" })).toBeTruthy();
    statusNow = status({ own: own({ state: "downloading" }) });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: DOWNLOAD }));
    });
    expect(ipcMock.downloadAgentBrowser).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("button", { name: DOWNLOAD })).toBeNull();
    expect(screen.getByRole("progressbar", { name: "Browser download" })).toBeTruthy();
  });

  it("offers nothing where the backend does not offer it", async () => {
    statusNow = status({ own: own({ offered: false, downloadBytes: 0 }) });
    await show();
    expect(screen.getByText(NOT_FOUND)).toBeTruthy();
    expect(screen.queryByText("A browser for the agent")).toBeNull();
    expect(screen.queryByRole("button", { name: /Download a browser/ })).toBeNull();
  });

  it("offers nothing when a browser is installed", async () => {
    statusNow = status({ found: "/Applications/Google Chrome.app", kind: "Google Chrome", unavailable: null });
    await show();
    expect(screen.getByText("Google Chrome")).toBeTruthy();
    expect(screen.queryByText("A browser for the agent")).toBeNull();
  });

  it("shows progress, lets the download be stopped, and then shows it installed", async () => {
    statusNow = status({ own: own({ state: "downloading" }) });
    await show();
    await act(async () => {
      onProgress({ done: 98291851, total: 196583703, unpacking: false, finished: false, error: null });
    });
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("50");
    expect(screen.getByText("98 MB of 197 MB")).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    });
    expect(ipcMock.cancelAgentBrowserDownload).toHaveBeenCalledTimes(1);
    await act(async () => {
      onProgress({ done: 196583703, total: 196583703, unpacking: true, finished: false, error: null });
    });
    expect(screen.getByText("Checking and unpacking…")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    statusNow = status({
      found: "/data/agent-browser/157.0.8103.50/chrome",
      kind: "The agent's own browser (Chrome for Testing)",
      unavailable: null,
      own: own({ state: "installed", version: "157.0.8103.50", inUse: true }),
    });
    await act(async () => {
      onProgress({ done: 0, total: 196583703, unpacking: false, finished: true, error: null });
    });
    expect(screen.getByText("Installed (version 157.0.8103.50).")).toBeTruthy();
    expect(screen.getByText("The agent uses this browser.")).toBeTruthy();
    expect(screen.queryByRole("progressbar")).toBeNull();
  });

  it("shows why a download failed", async () => {
    statusNow = status({ own: own({ state: "downloading" }) });
    await show();
    statusNow = status();
    await act(async () => {
      onProgress({
        done: 0,
        total: 196583703,
        unpacking: false,
        finished: true,
        error: {
          code: "checksum_mismatch",
          message: "The downloaded browser does not match its pinned checksum, so it was not installed.",
          nextStep: "Try again.",
          details: "",
        },
      });
    });
    expect(screen.getByText(/does not match its pinned checksum/)).toBeTruthy();
    expect(screen.getByRole("button", { name: DOWNLOAD })).toBeTruthy();
  });

  it("removes the installed browser after asking", async () => {
    statusNow = status({
      found: "/Applications/Google Chrome.app",
      kind: "Google Chrome",
      unavailable: null,
      own: own({ state: "installed", version: "157.0.8103.50" }),
    });
    await show();
    // Installed but another browser is used: still shown, so it can be removed.
    expect(screen.getByText("Not in use: Google Chrome is used instead.")).toBeTruthy();
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    });
    expect(ipcMock.removeAgentBrowser).not.toHaveBeenCalled();
    confirm.mockReturnValue(true);
    statusNow = status({ found: "/Applications/Google Chrome.app", kind: "Google Chrome", unavailable: null });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    });
    expect(ipcMock.removeAgentBrowser).toHaveBeenCalledTimes(1);
    expect(screen.queryByText("A browser for the agent")).toBeNull();
  });

  it("continues or deletes a stopped download", async () => {
    statusNow = status({ own: own({ state: "partial", bytesDone: 50_000_000 }) });
    await show();
    expect(screen.getByRole("button", { name: "Continue the download (50 MB of 197 MB done)" })).toBeTruthy();
    vi.spyOn(window, "confirm").mockReturnValue(true);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Delete the partial download" }));
    });
    expect(ipcMock.removeAgentBrowser).toHaveBeenCalledTimes(1);
  });

  it("does not offer the download while a chosen path is set", async () => {
    statusNow = status({
      settings: { path: "/gone/browser", loopbackPort: false, allowPrivateNetworks: false, strictSites: [] },
      unavailable: "The browser chosen in Settings (/gone/browser) was not found.",
    });
    await show();
    expect(screen.queryByText("A browser for the agent")).toBeNull();
    expect(screen.getByRole("button", { name: "Use the installed one" })).toBeTruthy();
  });

  it("shows the browser's path with hidden and right-to-left characters as codes", async () => {
    statusNow = status({ found: "/opt/X-\u202Eemorhc/chrome", kind: "Google Chrome", unavailable: null });
    await show();
    const found = screen.getByText("/opt/X-\\u{202E}emorhc/chrome");
    expect(found.tagName).toBe("BDI");
    expect(found.getAttribute("dir")).toBe("ltr");
    expect(new XMLSerializer().serializeToString(document.body)).not.toContain("\u202E");
    cleanup();
    statusNow = status({
      settings: { path: "/opt/X-\u202Eemorhc", loopbackPort: false, allowPrivateNetworks: false, strictSites: [] },
      unavailable: "The browser chosen in Settings (/opt/X-\u202Eemorhc) was not found.",
    });
    await show();
    expect(screen.getByText("The browser chosen in Settings (/opt/X-\\u{202E}emorhc) was not found.")).toBeTruthy();
    expect(new XMLSerializer().serializeToString(document.body)).not.toContain("\u202E");
  });
});
