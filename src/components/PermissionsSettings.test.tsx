import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { AuditRow } from "../lib/types";
import type { ApprovalModeName } from "../lib/ipc";

let saved: ApprovalModeName = "ask";
let chatModes: Record<number, ApprovalModeName> = {};
let rows: AuditRow[] = [];

const ipcMock = vi.hoisted(() => ({
  listApprovedFolders: vi.fn(async () => []),
  stopHotkeyStatus: vi.fn(async () => ({ accelerator: "Control+Alt+Backspace", registered: true, error: null })),
  getTerminalPassThrough: vi.fn(async () => []),
  getDefaultApprovalMode: vi.fn<() => Promise<ApprovalModeName>>(),
  setDefaultApprovalMode: vi.fn<(mode: ApprovalModeName) => Promise<void>>(),
  getChatApprovalMode: vi.fn<(chatId: number) => Promise<ApprovalModeName>>(),
  setChatApprovalMode: vi.fn(async (_chat: number, mode: ApprovalModeName) => mode),
  listAudit: vi.fn<() => Promise<AuditRow[]>>(),
  exportAudit: vi.fn(async () => null),
}));
vi.mock("../lib/ipc", () => ipcMock);

const { AuditLogView, FULL_ACCESS_NOTE, PermissionsSettings, decisionLabel } = await import("./PermissionsSettings");
const { APPROVAL_MODES, CONFIRM_INPUT_DELAY_MS, FULL_ACCESS_RISKS, STILL_ASKS } = await import("./AllowAllButton");
const { useApprovalMode } = await import("../lib/approvalMode");

const radio = (label: string) => screen.getByRole("radio", { name: new RegExp(`^${label}`) });
const checked = () =>
  within(screen.getByRole("radiogroup", { name: "Every chat starts in" }))
    .getAllByRole("radio")
    .filter((r) => r.getAttribute("aria-checked") === "true")
    .map((r) => r.textContent);

async function open() {
  await act(async () => {
    render(<PermissionsSettings />);
  });
}

async function choose(label: string) {
  await act(async () => {
    fireEvent.click(radio(label));
  });
}

const arm = () =>
  act(() => {
    vi.advanceTimersByTime(CONFIRM_INPUT_DELAY_MS + 10);
  });

const label = (mode: ApprovalModeName) => {
  const m = APPROVAL_MODES.find((x) => x.mode === mode)!;
  return m.label + m.description;
};

describe("Settings: the mode every chat starts in", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    saved = "ask";
    chatModes = {};
    ipcMock.getDefaultApprovalMode.mockImplementation(async () => saved);
    ipcMock.setDefaultApprovalMode.mockImplementation(async (mode) => {
      saved = mode;
    });
    ipcMock.getChatApprovalMode.mockImplementation(async (chatId) => chatModes[chatId] ?? saved);
    useApprovalMode.setState({ modes: {}, defaultMode: "ask" });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("offers the three modes, reads the saved one, and words the section plainly", async () => {
    saved = "allowAll";
    await open();
    expect(ipcMock.getDefaultApprovalMode).toHaveBeenCalled();
    const group = screen.getByRole("radiogroup", { name: "Every chat starts in" });
    expect(within(group).getAllByRole("radio").map((r) => r.textContent)).toEqual([
      label("ask"),
      label("allowAll"),
      label("fullAccess"),
    ]);
    expect(checked()).toEqual([label("allowAll")]);
    const text = group.closest("section")?.textContent ?? "";
    expect(text).toContain("Ask first, Allow all or Full access");
    expect(text).toContain("also changes chats you switched by hand");
  });

  it("asks before Allow all, with the list of what still asks", async () => {
    await open();
    expect(checked()).toEqual([label("ask")]);
    await choose("Allow all");
    // Nothing is saved by the click itself.
    expect(ipcMock.setDefaultApprovalMode).not.toHaveBeenCalled();
    expect(checked()).toEqual([label("ask")]);
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText("Allow all in every chat?")).toBeTruthy();
    const still = within(dialog).getByRole("list", { name: "Still asks" });
    expect(within(still).getAllByRole("listitem").map((li) => li.textContent)).toEqual([...STILL_ASKS]);
    arm();
    await act(async () => {
      fireEvent.click(within(dialog).getByRole("button", { name: "Allow all in every chat" }));
    });
    expect(ipcMock.setDefaultApprovalMode).toHaveBeenCalledWith("allowAll");
    expect(checked()).toEqual([label("allowAll")]);
  });

  it("asks before Full access with its warning, ignores an early click, then re-reads the chats", async () => {
    // A chat the window has shown, switched to Ask first by hand.
    useApprovalMode.setState({ modes: { 7: "ask" } });
    await open();
    await choose("Full access");
    expect(ipcMock.setDefaultApprovalMode).not.toHaveBeenCalled();
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText("Turn on Full access in every chat?")).toBeTruthy();
    const risks = within(dialog).getByRole("list", { name: "Risks" });
    const lines = within(risks).getAllByRole("listitem").map((li) => li.textContent ?? "");
    expect(lines.slice(0, -1)).toEqual([...FULL_ACCESS_RISKS]);
    expect(lines.at(-1)).toContain("stays on after a restart");
    const confirm = within(dialog).getByRole("button", { name: "Turn on Full access" });
    fireEvent.click(confirm);
    expect(ipcMock.setDefaultApprovalMode).not.toHaveBeenCalled();
    arm();
    await act(async () => {
      fireEvent.click(confirm);
    });
    expect(ipcMock.setDefaultApprovalMode).toHaveBeenCalledWith("fullAccess");
    expect(checked()).toEqual([label("fullAccess")]);
    expect(useApprovalMode.getState().defaultMode).toBe("fullAccess");
    // Every chat follows the new default, also one switched by hand.
    expect(ipcMock.getChatApprovalMode).toHaveBeenCalledWith(7);
    expect(useApprovalMode.getState().modes[7]).toBe("fullAccess");
    // The chosen mode reads in its colour.
    expect(radio("Full access").querySelector(".text-danger")?.textContent).toBe("Full access");
  });

  it("stays as it was when a confirmation is cancelled", async () => {
    await open();
    for (const mode of ["Allow all", "Full access"]) {
      await choose(mode);
      await act(async () => {
        fireEvent.click(within(screen.getByRole("alertdialog")).getByText("Cancel"));
      });
      expect(screen.queryByRole("alertdialog")).toBeNull();
    }
    expect(ipcMock.setDefaultApprovalMode).not.toHaveBeenCalled();
    expect(checked()).toEqual([label("ask")]);
  });

  it("goes back to Ask first at once and re-reads the chats on screen", async () => {
    saved = "fullAccess";
    // A chat the window has shown as Full access.
    useApprovalMode.setState({ modes: { 7: "fullAccess" } });
    await open();
    expect(checked()).toEqual([label("fullAccess")]);
    await choose("Ask first");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(ipcMock.setDefaultApprovalMode).toHaveBeenCalledWith("ask");
    expect(checked()).toEqual([label("ask")]);
    expect(ipcMock.getChatApprovalMode).toHaveBeenCalledWith(7);
    expect(useApprovalMode.getState().modes[7]).toBe("ask");
  });

  it("asks even for a step down from Full access to Allow all, since chats switched by hand follow it", async () => {
    saved = "fullAccess";
    await open();
    await choose("Allow all");
    expect(within(screen.getByRole("alertdialog")).getByText("Allow all in every chat?")).toBeTruthy();
    expect(ipcMock.setDefaultApprovalMode).not.toHaveBeenCalled();
  });

  it("keeps the choice as it was and shows the error when the app refuses", async () => {
    saved = "allowAll";
    ipcMock.setDefaultApprovalMode.mockRejectedValueOnce({
      code: "internal",
      message: "The setting could not be saved.",
      nextStep: "",
      details: "",
    });
    await open();
    await choose("Ask first");
    expect(ipcMock.setDefaultApprovalMode).toHaveBeenCalledWith("ask");
    expect(checked()).toEqual([label("allowAll")]);
    expect(useApprovalMode.getState().defaultMode).toBe("allowAll");
    expect(screen.getByRole("alert").textContent).toContain("The setting could not be saved.");
  });
});

describe("Audit log: decisions by the mode", () => {
  afterEach(cleanup);

  const row = (id: number, decision: string, note: string | null, tool = "write_file"): AuditRow => ({
    id,
    ts: 1_700_000_000_000,
    chat_id: 7,
    server: "files",
    tool,
    arguments: {},
    preview: `Call ${id}`,
    risk: "normal",
    decision,
    decided_by: "policy",
    note,
    result_status: "ok",
    result_summary: null,
    duration_ms: 3,
  });

  it("names the mode that let a call through, Allow all or Full access, and can filter for both", async () => {
    expect(FULL_ACCESS_NOTE).toBe("Full access");
    expect(decisionLabel("allowed_by_mode")).toBe("allowed by Allow all");
    expect(decisionLabel("allowed_by_mode", null)).toBe("allowed by Allow all");
    expect(decisionLabel("allowed_by_mode", "Full access")).toBe("allowed by Full access");
    expect(decisionLabel("allowed_by_grant")).toBe("allowed by grant");
    // Only a call let through by the mode is read as Full access, not a note on another row.
    expect(decisionLabel("denied", "Full access")).toBe("denied");
    rows = [
      row(1, "allowed_by_mode", null),
      row(2, "allowed_by_mode", "Full access", "delete_folder"),
      row(3, "denied", "use the other folder", "delete_file"),
    ];
    ipcMock.listAudit.mockImplementation(async () => rows);
    await act(async () => {
      render(<AuditLogView />);
    });
    const cells = screen.getAllByRole("row").slice(1).map((r) => within(r).getAllByRole("cell")[3]?.textContent);
    // The note that only names the mode is not repeated after the label; a user's note is.
    expect(cells).toEqual(["allowed by Allow all", "allowed by Full access", "denied (“use the other folder”)"]);
    const filter = screen.getByLabelText("Decision") as HTMLSelectElement;
    expect([...filter.options].map((o) => [o.value, o.text])).toContainEqual([
      "allowed_by_mode",
      "Allowed by Allow all or Full access",
    ]);
    await act(async () => {
      fireEvent.change(filter, { target: { value: "allowed_by_mode" } });
    });
    expect(ipcMock.listAudit).toHaveBeenLastCalledWith({ tool: null, decision: "allowed_by_mode", limit: 500 });
  });
});
