import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { AuditRow } from "../lib/types";

let saved = false;
let chatModes: Record<number, boolean> = {};
let rows: AuditRow[] = [];

const ipcMock = vi.hoisted(() => ({
  listApprovedFolders: vi.fn(async () => []),
  stopHotkeyStatus: vi.fn(async () => ({ accelerator: "Control+Alt+Backspace", registered: true, error: null })),
  getTerminalPassThrough: vi.fn(async () => []),
  getAllowAllDefault: vi.fn<() => Promise<boolean>>(),
  setAllowAllDefault: vi.fn<(on: boolean) => Promise<void>>(),
  getApprovalMode: vi.fn<(chatId: number) => Promise<boolean>>(),
  setApprovalMode: vi.fn(async (_chat: number, on: boolean) => on),
  listAudit: vi.fn<() => Promise<AuditRow[]>>(),
  exportAudit: vi.fn(async () => null),
}));
vi.mock("../lib/ipc", () => ipcMock);

const { AuditLogView, PermissionsSettings, decisionLabel } = await import("./PermissionsSettings");
const { CONFIRM_INPUT_DELAY_MS, STILL_ASKS } = await import("./AllowAllButton");
const { useApprovalMode } = await import("../lib/approvalMode");

async function open() {
  await act(async () => {
    render(<PermissionsSettings />);
  });
  return screen.getByRole("switch", { name: "Allow all in every chat" }) as HTMLInputElement;
}

describe("Settings: Allow all in every chat", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    saved = false;
    chatModes = {};
    ipcMock.getAllowAllDefault.mockImplementation(async () => saved);
    ipcMock.setAllowAllDefault.mockImplementation(async (on) => {
      saved = on;
    });
    ipcMock.getApprovalMode.mockImplementation(async (chatId) => chatModes[chatId] ?? saved);
    useApprovalMode.setState({ modes: {} });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("is off unless switched on, and switching it on asks first with the list of what still asks", async () => {
    const toggle = await open();
    expect(toggle.checked).toBe(false);
    await act(async () => {
      fireEvent.click(toggle);
    });
    // Nothing is saved by the click itself.
    expect(ipcMock.setAllowAllDefault).not.toHaveBeenCalled();
    expect(toggle.checked).toBe(false);
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText("Allow all in every chat?")).toBeTruthy();
    const still = within(dialog).getByRole("list", { name: "Still asks" });
    expect(within(still).getAllByRole("listitem").map((li) => li.textContent)).toEqual([...STILL_ASKS]);
    act(() => {
      vi.advanceTimersByTime(CONFIRM_INPUT_DELAY_MS + 10);
    });
    await act(async () => {
      fireEvent.click(within(dialog).getByRole("button", { name: "Allow all in every chat" }));
    });
    expect(ipcMock.setAllowAllDefault).toHaveBeenCalledWith(true);
    expect(toggle.checked).toBe(true);
  });

  it("stays off when the confirmation is cancelled", async () => {
    const toggle = await open();
    await act(async () => {
      fireEvent.click(toggle);
    });
    await act(async () => {
      fireEvent.click(within(screen.getByRole("alertdialog")).getByText("Cancel"));
    });
    expect(ipcMock.setAllowAllDefault).not.toHaveBeenCalled();
    expect(toggle.checked).toBe(false);
  });

  it("switches off at once and re-reads the chats on screen", async () => {
    saved = true;
    // A chat the window has shown as Allow all.
    useApprovalMode.setState({ modes: { 7: true } });
    const toggle = await open();
    expect(toggle.checked).toBe(true);
    await act(async () => {
      fireEvent.click(toggle);
    });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(ipcMock.setAllowAllDefault).toHaveBeenCalledWith(false);
    expect(toggle.checked).toBe(false);
    expect(ipcMock.getApprovalMode).toHaveBeenCalledWith(7);
    expect(useApprovalMode.getState().modes[7]).toBe(false);
  });
});

describe("Audit log: decisions by the mode", () => {
  afterEach(cleanup);

  it("shows a call the mode let through as allowed by Allow all, and can filter for it", async () => {
    expect(decisionLabel("allowed_by_mode")).toBe("allowed by Allow all");
    expect(decisionLabel("allowed_by_grant")).toBe("allowed by grant");
    rows = [
      {
        id: 1,
        ts: 1_700_000_000_000,
        chat_id: 7,
        server: "files",
        tool: "write_file",
        arguments: {},
        preview: "Create /work/a.txt",
        risk: "normal",
        decision: "allowed_by_mode",
        decided_by: "policy",
        note: null,
        result_status: "ok",
        result_summary: null,
        duration_ms: 3,
      },
    ];
    ipcMock.listAudit.mockImplementation(async () => rows);
    await act(async () => {
      render(<AuditLogView />);
    });
    expect(screen.getByText("allowed by Allow all")).toBeTruthy();
    const filter = screen.getByLabelText("Decision") as HTMLSelectElement;
    expect([...filter.options].map((o) => [o.value, o.text])).toContainEqual(["allowed_by_mode", "Allowed by Allow all"]);
    await act(async () => {
      fireEvent.change(filter, { target: { value: "allowed_by_mode" } });
    });
    expect(ipcMock.listAudit).toHaveBeenLastCalledWith({ tool: null, decision: "allowed_by_mode", limit: 500 });
  });
});
