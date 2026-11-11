import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ComputerProfileStatus, ComputerStatus, Convention, ModelOption } from "../lib/types";

const TARS = "local:/models/UI-TARS-1.5-7B-Q4_K_M.gguf";
const OTHER = "remote:3:some-vision-model";
const SCREEN_URL = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

const modelOptions: ModelOption[] = [
  { ref: TARS, label: "UI-TARS-1.5-7B", source: "local", contextLength: null },
  { ref: OTHER, label: "some-vision-model", source: "provider", contextLength: null },
];

/** What the backend has per model; `setComputerProfile` changes it like the real command. */
let profiles: Record<string, ComputerProfileStatus>;
let permissions = { screenRecording: true, accessibility: true };

const ipcMock = vi.hoisted(() => ({
  computerStatus: vi.fn<(modelRef: string | null) => Promise<ComputerStatus>>(),
  setComputerProfile: vi.fn<(modelRef: string, convention: Convention | null) => Promise<void>>(),
}));
vi.mock("../lib/ipc", () => ipcMock);
const openUrl = vi.hoisted(() => vi.fn(async (url: string) => void url));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));
vi.mock("../store/app", () => ({
  useApp: () => ({ modelOptions, detail: null }),
  toErrorInfo: (e: unknown) => ({ code: "unexpected", message: String(e), nextStep: "", details: null }),
}));

const { ComputerSettings } = await import("./ComputerSettings");

function setPlatform(value: string) {
  Object.defineProperty(navigator, "platform", { value, configurable: true });
}

async function show() {
  await act(async () => {
    render(<ComputerSettings />);
  });
}

function radio(name: string | RegExp): HTMLInputElement {
  return screen.getByRole("radio", { name }) as HTMLInputElement;
}

describe("ComputerSettings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setPlatform("Win32");
    permissions = { screenRecording: true, accessibility: true };
    profiles = {
      [TARS]: { convention: "pixels", automatic: true, autoConvention: "pixels", autoGuess: false },
      [OTHER]: { convention: "grid1000", automatic: true, autoConvention: "grid1000", autoGuess: true },
    };
    ipcMock.computerStatus.mockImplementation(async (modelRef) => ({
      available: true,
      ...permissions,
      screenRecordingUrl: SCREEN_URL,
      accessibilityUrl: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
      profile: (modelRef && profiles[modelRef]) || null,
    }));
    ipcMock.setComputerProfile.mockImplementation(async (modelRef, convention) => {
      const p = profiles[modelRef]!;
      profiles[modelRef] = { ...p, automatic: convention === null, convention: convention ?? p.autoConvention };
    });
  });
  afterEach(cleanup);

  it("needs no setup: a model starts on Automatic, with what that is", async () => {
    await show();
    expect(ipcMock.computerStatus).toHaveBeenCalledWith(TARS);
    expect(radio(/^Automatic: pixels of the image the model saw/).checked).toBe(true);
    expect(screen.queryByText(/best guess/i)).toBeNull();
    // No confirmation tick box, and nothing to save.
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(screen.getByText(/If clicks land in the wrong place, choose another convention here\./)).toBeTruthy();
  });

  it("says when Automatic is a best guess", async () => {
    await show();
    await act(async () => {
      fireEvent.change(screen.getByLabelText("Model"), { target: { value: OTHER } });
    });
    expect(ipcMock.computerStatus).toHaveBeenLastCalledWith(OTHER);
    const auto = radio(/^Automatic: 0–1000 grid, \[x, y\]/);
    expect(auto.checked).toBe(true);
    expect(auto.closest("label")!.textContent).toContain("Best guess for an unknown model.");
  });

  it("saves another convention when one is chosen, and goes back to Automatic", async () => {
    await show();
    await act(async () => {
      fireEvent.click(radio(/^0–1000 grid, \[x, y\]/));
    });
    expect(ipcMock.setComputerProfile).toHaveBeenLastCalledWith(TARS, "grid1000");
    expect(radio(/^0–1000 grid, \[x, y\]/).checked).toBe(true);
    // Automatic still says what it would pick.
    const auto = radio(/^Automatic: pixels of the image the model saw/);
    expect(auto.checked).toBe(false);

    await act(async () => {
      fireEvent.click(auto);
    });
    expect(ipcMock.setComputerProfile).toHaveBeenLastCalledWith(TARS, null);
    expect(radio(/^Automatic: pixels of the image the model saw/).checked).toBe(true);
    expect(radio(/^0–1000 grid, \[x, y\]/).checked).toBe(false);
  });

  it("shows a choice made earlier", async () => {
    profiles[TARS] = { convention: "box1000Yx", automatic: false, autoConvention: "pixels", autoGuess: false };
    await show();
    expect(radio(/^0–1000 boxes, \[y1, x1, y2, x2\]/).checked).toBe(true);
    expect(radio(/^Automatic/).checked).toBe(false);
  });

  it("keeps the macOS permission checks and links", async () => {
    setPlatform("MacIntel");
    permissions = { screenRecording: false, accessibility: true };
    await show();
    expect(screen.getByText("Screen Recording: not allowed")).toBeTruthy();
    expect(screen.getByText("Accessibility: allowed")).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Open System Settings" }));
    });
    expect(openUrl).toHaveBeenCalledWith(SCREEN_URL);
    permissions = { screenRecording: true, accessibility: true };
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    });
    expect(screen.getByText("Screen Recording: allowed")).toBeTruthy();
  });

  it("shows no macOS section elsewhere", async () => {
    await show();
    expect(screen.queryByText("macOS permissions")).toBeNull();
  });
});
