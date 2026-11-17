import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { UpdateInfo, UpdateState, UpdateStatus } from "../lib/types";

// Settings → General → Updates on its own. The other sections are stubs, as in
// SettingsDialog.test.tsx.
let onState: (s: UpdateState) => void = () => undefined;
let statusNow: UpdateStatus;

vi.mock("../lib/ipc", () => ({
  isTauri: () => true,
  updateStatus: vi.fn(async () => statusNow),
  onUpdateState: vi.fn(async (h: (s: UpdateState) => void) => {
    onState = h;
    return () => undefined;
  }),
  setUpdateAutoCheck: vi.fn(async () => undefined),
  checkForUpdate: vi.fn(async () => null),
  installUpdate: vi.fn(async () => undefined),
}));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn(async () => undefined) }));
vi.mock("./PermissionsSettings", () => ({ PermissionsSettings: () => null, AuditLogView: () => null }));
vi.mock("./McpSettings", () => ({ McpSettings: () => null }));
vi.mock("./SkillsSettings", () => ({ SkillsSettings: () => null }));
vi.mock("./BrowserSettings", () => ({ BrowserSettings: () => null }));
vi.mock("./ComputerSettings", () => ({ ComputerSettings: () => null }));
vi.mock("./GitSettings", () => ({ GitSettings: () => null }));
vi.mock("./SearchSettings", () => ({ SearchSettings: () => null }));
vi.mock("./evals/Benchmarks", () => ({ Benchmarks: () => null }));
vi.mock("./evals/ModelCheck", () => ({ ModelCheck: () => null }));
vi.mock("./settings/LocalModels", () => ({ LocalModels: () => null }));
vi.mock("./settings/OtherApps", () => ({ OtherApps: () => null }));
vi.mock("./settings/Reverse", () => ({ Reverse: () => null }));

const ipc = await import("../lib/ipc");
const { Updates } = await import("./SettingsDialog");

function info(version: string, notes: string | null = "Fixes."): UpdateInfo {
  return { version, date: null, notes };
}

function status(patch: Partial<UpdateStatus> = {}): UpdateStatus {
  return {
    configured: true,
    autoCheck: true,
    currentVersion: "0.0.8",
    checking: false,
    checked: false,
    downloading: null,
    available: null,
    ready: null,
    error: null,
    ...patch,
  };
}

function state(s: UpdateStatus): UpdateState {
  const { checking, checked, downloading, available, ready, error } = s;
  return { checking, checked, downloading, available, ready, error };
}

async function show() {
  await act(async () => {
    render(<Updates />);
  });
}

describe("Settings → General → Updates", () => {
  beforeEach(() => {
    statusNow = status();
    vi.mocked(ipc.setUpdateAutoCheck).mockClear();
    vi.mocked(ipc.installUpdate).mockClear();
    vi.mocked(ipc.updateStatus).mockReset();
    vi.mocked(ipc.updateStatus).mockImplementation(async () => statusNow);
    vi.mocked(ipc.checkForUpdate).mockReset();
    vi.mocked(ipc.checkForUpdate).mockImplementation(async () => null);
  });
  afterEach(cleanup);

  it("says when this build cannot update itself", async () => {
    statusNow = status({ configured: false });
    await show();
    expect(screen.getByText(/This build cannot update itself/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Check now" })).toBeNull();
  });

  it("checks automatically by default, says it contacts github.com, and can be turned off", async () => {
    await show();
    const box = screen.getByRole("checkbox", { name: "Check for updates automatically (contacts github.com)" });
    expect((box as HTMLInputElement).checked).toBe(true);
    await act(async () => {
      fireEvent.click(box);
    });
    expect(ipc.setUpdateAutoCheck).toHaveBeenCalledWith(false);
    expect((box as HTMLInputElement).checked).toBe(false);
  });

  it("says the version is the latest after Check now finds nothing", async () => {
    await show();
    statusNow = status({ checked: true });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check now" }));
    });
    expect(ipc.checkForUpdate).toHaveBeenCalled();
    expect(screen.getByText("You have the latest version (0.0.8).")).toBeTruthy();
  });

  it("follows the download to Restart and update", async () => {
    await show();
    act(() => onState(state(status({ checked: true, available: info("0.0.9"), downloading: "0.0.9" }))));
    expect(screen.getByText("Downloading version 0.0.9…")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Check now" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByRole("button", { name: "Restart and update" })).toBeNull();
    act(() => onState(state(status({ checked: true, available: info("0.0.9"), ready: info("0.0.9") }))));
    expect(screen.getByText("Version 0.0.9 is ready to install.")).toBeTruthy();
    expect(screen.getByRole("region", { name: "Release notes" }).textContent).toBe("Fixes.");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Restart and update" }));
    });
    expect(ipc.installUpdate).toHaveBeenCalledTimes(1);
  });

  it("says when the last check did not work", async () => {
    statusNow = status({ error: "Could not check for updates." });
    await show();
    expect(screen.getByText("Could not check for updates. The app tries again at the next check.")).toBeTruthy();
  });

  it("does not promise another check when the automatic check is off", async () => {
    statusNow = status({ autoCheck: false, error: "Could not check for updates." });
    await show();
    expect(screen.getByText("Could not check for updates. Click Check now to try again.")).toBeTruthy();
    expect(screen.queryByText(/tries again/)).toBeNull();
  });

  it("says once that Check now did not work", async () => {
    await show();
    vi.mocked(ipc.checkForUpdate).mockImplementation(async () => {
      statusNow = status({ checked: false, error: "Could not check for updates." });
      throw {
        code: "update_check",
        message: "Could not check for updates.",
        nextStep: "Check your internet connection and try again.",
        details: "offline",
      };
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check now" }));
    });
    expect(screen.getAllByText(/Could not check for updates/)).toHaveLength(1);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByText("Could not check for updates. The app tries again at the next check.")).toBeTruthy();
  });

  it("still shows other errors from Check now", async () => {
    await show();
    vi.mocked(ipc.checkForUpdate).mockImplementation(async () => {
      throw { code: "internal", message: "Something went wrong.", nextStep: "Try again.", details: "x" };
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check now" }));
    });
    expect(screen.getByRole("alert").textContent).toMatch(/Something went wrong/);
  });

  it("reads the status again when a change arrived before it", async () => {
    let answer: (s: UpdateStatus) => void = () => undefined;
    vi.mocked(ipc.updateStatus).mockImplementationOnce(() => new Promise<UpdateStatus>((resolve) => (answer = resolve)));
    await show();
    // The status was read while checking; the end of the check arrives first and is missed.
    const done = status({ checked: true });
    statusNow = done;
    act(() => onState(state(done)));
    await act(async () => answer(status({ checking: true })));
    expect(ipc.updateStatus).toHaveBeenCalledTimes(2);
    expect(screen.getByText("You have the latest version (0.0.8).")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Check now" }) as HTMLButtonElement).disabled).toBe(false);
  });
});
