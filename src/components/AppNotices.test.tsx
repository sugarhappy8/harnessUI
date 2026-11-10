import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { HotkeyStatus, UpdateInfo, UpdateStatus } from "../lib/types";

// The event handlers AppNotices subscribes, and what the two status commands answer.
let onHotkey: (s: HotkeyStatus) => void = () => undefined;
let onUpdate: (u: UpdateInfo) => void = () => undefined;
let hotkeyNow: HotkeyStatus;
let updateNow: UpdateStatus;

vi.mock("../lib/ipc", () => ({
  isTauri: () => true,
  onStopHotkeyStatus: vi.fn(async (h: (s: HotkeyStatus) => void) => {
    onHotkey = h;
    return () => undefined;
  }),
  onUpdateAvailable: vi.fn(async (h: (u: UpdateInfo) => void) => {
    onUpdate = h;
    return () => undefined;
  }),
  stopHotkeyStatus: vi.fn(async () => hotkeyNow),
  updateStatus: vi.fn(async () => updateNow),
}));

const { AppNotices } = await import("./AppNotices");

const HOTKEY_TEXT = /stop hotkey could not be registered/;

async function show(onOpenSettings = vi.fn()) {
  await act(async () => {
    render(<AppNotices onOpenSettings={onOpenSettings} />);
  });
  return onOpenSettings;
}

describe("AppNotices", () => {
  beforeEach(() => {
    hotkeyNow = { accelerator: "", registered: false, error: null };
    updateNow = { configured: true, autoCheck: true, currentVersion: "0.1.0", available: null };
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

  it("shows an update found before it was listening, and opens Settings → General", async () => {
    updateNow = { ...updateNow, available: { version: "0.2.0", date: null, notes: null } };
    const open = await show();
    expect(screen.getByText("Version 0.2.0 is available.")).toBeTruthy();
    fireEvent.click(screen.getByText("Open Updates"));
    expect(open).toHaveBeenCalledWith("general");
  });

  it("hides a dismissed update until a newer one arrives", async () => {
    await show();
    act(() => onUpdate({ version: "0.2.0", date: null, notes: null }));
    fireEvent.click(screen.getByRole("button", { name: "Dismiss update notice" }));
    expect(screen.queryByText(/is available/)).toBeNull();
    act(() => onUpdate({ version: "0.3.0", date: null, notes: null }));
    expect(screen.getByText("Version 0.3.0 is available.")).toBeTruthy();
  });
});
