import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { HotkeyStatus, UpdateInfo, UpdateStatus } from "../lib/types";

// The event handlers AppNotices (and the update box it draws) subscribes, and what the two
// status commands answer.
let onHotkey: (s: HotkeyStatus) => void = () => undefined;
let onReady: (u: UpdateInfo) => void = () => undefined;
let hotkeyNow: HotkeyStatus;
let updateNow: UpdateStatus;

vi.mock("../lib/ipc", () => ({
  isTauri: () => true,
  onStopHotkeyStatus: vi.fn(async (h: (s: HotkeyStatus) => void) => {
    onHotkey = h;
    return () => undefined;
  }),
  onUpdateReady: vi.fn(async (h: (u: UpdateInfo) => void) => {
    onReady = h;
    return () => undefined;
  }),
  stopHotkeyStatus: vi.fn(async () => hotkeyNow),
  updateStatus: vi.fn(async () => updateNow),
  installUpdate: vi.fn(async () => undefined),
}));

const { AppNotices } = await import("./AppNotices");

const HOTKEY_TEXT = /stop hotkey could not be registered/;

async function show(onOpenSettings = vi.fn()) {
  await act(async () => {
    render(<AppNotices onOpenSettings={onOpenSettings} updatePlacement="sidebar" />);
  });
  return onOpenSettings;
}

describe("AppNotices", () => {
  beforeEach(() => {
    hotkeyNow = { accelerator: "", registered: false, error: null };
    updateNow = {
      configured: true,
      autoCheck: true,
      currentVersion: "0.0.8",
      checking: false,
      checked: true,
      downloading: null,
      available: null,
      ready: null,
      error: null,
    };
    window.sessionStorage.clear();
  });
  afterEach(cleanup);

  it("says nothing before the hotkey is first registered", async () => {
    await show();
    expect(screen.queryByText(HOTKEY_TEXT)).toBeNull();
  });

  it("keeps a warning up while the hotkey is not registered, until a later status says it works", async () => {
    hotkeyNow = { accelerator: "Control+Alt+Backspace", registered: false, error: "taken" };
    const open = await show();
    expect(screen.getByRole("alert").textContent).toMatch(HOTKEY_TEXT);
    expect(screen.queryByRole("button", { name: /dismiss/i })).toBeNull();
    fireEvent.click(screen.getByText("Open Permissions"));
    expect(open).toHaveBeenCalledWith("permissions");
    act(() => onHotkey({ accelerator: "Control+Alt+Backspace", registered: true, error: null }));
    expect(screen.queryByText(HOTKEY_TEXT)).toBeNull();
  });

  it("shows the warning when the status event reports a failure", async () => {
    await show();
    act(() => onHotkey({ accelerator: "Control+Alt+X", registered: false, error: "taken" }));
    expect(screen.getByText(HOTKEY_TEXT)).toBeTruthy();
  });

  // 0.0.8: the update bar became the floating "Update ready" box (UpdateBox.test.tsx has
  // its own tests). A version that was only found, not yet downloaded, shows nothing.
  it("shows nothing about updates until a version is ready to install", async () => {
    updateNow = { ...updateNow, available: { version: "0.0.9", date: null, notes: null }, downloading: "0.0.9" };
    await show();
    expect(screen.queryByRole("region", { name: "Update ready" })).toBeNull();
    expect(screen.queryByText(/0\.0\.9/)).toBeNull();
  });

  it("draws the update box beside the hotkey warning, which keeps working", async () => {
    hotkeyNow = { accelerator: "Control+Alt+Backspace", registered: false, error: "taken" };
    const open = await show();
    act(() => onReady({ version: "0.0.9", date: null, notes: null }));
    expect(screen.getByRole("region", { name: "Update ready" }).textContent).toMatch(
      /harnessUI 0\.0\.9 is ready to install\./,
    );
    expect(screen.getByRole("alert").textContent).toMatch(HOTKEY_TEXT);
    fireEvent.click(screen.getByText("Open Permissions"));
    expect(open).toHaveBeenCalledWith("permissions");
    fireEvent.click(screen.getByRole("button", { name: "Later" }));
    expect(screen.queryByRole("region", { name: "Update ready" })).toBeNull();
    expect(screen.getByRole("alert").textContent).toMatch(HOTKEY_TEXT);
  });
});
