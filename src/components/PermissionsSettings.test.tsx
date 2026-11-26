import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import type { ApprovedFolder, AuditRow } from "../lib/types";
import type { ApprovalModeName } from "../lib/ipc";

let saved: ApprovalModeName = "ask";
let chatModes: Record<number, ApprovalModeName> = {};
let rows: AuditRow[] = [];

const ipcMock = vi.hoisted(() => ({
  listApprovedFolders: vi.fn<
    (chatId: number | null) => Promise<ApprovedFolder[]>
  >(async () => []),
  removeApprovedFolder: vi.fn<(id: number) => Promise<void>>(
    async () => undefined,
  ),
  stopHotkeyStatus: vi.fn(async () => ({
    accelerator: "Control+Alt+Backspace",
    registered: true,
    error: null,
  })),
  getTerminalPassThrough: vi.fn(async () => []),
  getDefaultApprovalMode: vi.fn<() => Promise<ApprovalModeName>>(),
  setDefaultApprovalMode: vi.fn<(mode: ApprovalModeName) => Promise<void>>(),
  getChatApprovalMode: vi.fn<(chatId: number) => Promise<ApprovalModeName>>(),
  setChatApprovalMode: vi.fn(
    async (_chat: number, mode: ApprovalModeName) => mode,
  ),
  listAudit: vi.fn<() => Promise<AuditRow[]>>(),
  exportAudit: vi.fn(async () => null),
}));
vi.mock("../lib/ipc", () => ipcMock);

const { AuditLogView, FULL_ACCESS_NOTE, PermissionsSettings, decisionLabel } =
  await import("./PermissionsSettings");
const { APPROVAL_MODES } = await import("./AllowAllButton");
const { useApprovalMode } = await import("../lib/approvalMode");

const radio = (label: string) =>
  screen.getByRole("radio", { name: new RegExp(`^${label}`) });
// `hidden`: an open confirmation hides the rest of the page from the accessibility tree.
const checked = () =>
  within(
    screen.getByRole("radiogroup", {
      name: "Every chat starts in",
      hidden: true,
    }),
  )
    .getAllByRole("radio", { hidden: true })
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
    ipcMock.getChatApprovalMode.mockImplementation(
      async (chatId) => chatModes[chatId] ?? saved,
    );
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
    const group = screen.getByRole("radiogroup", {
      name: "Every chat starts in",
    });
    expect(
      within(group)
        .getAllByRole("radio")
        .map((r) => r.textContent),
    ).toEqual([label("ask"), label("allowAll"), label("fullAccess")]);
    expect(checked()).toEqual([label("allowAll")]);
    const text = group.closest("section")?.textContent ?? "";
    expect(text).toContain("Ask first, Allow all or Full access");
    expect(text).toContain("also changes chats you switched by hand");
  });

  it("changing the start mode saves without asking", async () => {
    // A chat the window has shown, switched to Ask first by hand.
    useApprovalMode.setState({ modes: { 7: "ask" } });
    await open();
    expect(checked()).toEqual([label("ask")]);
    await choose("Allow all");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(ipcMock.setDefaultApprovalMode).toHaveBeenCalledTimes(1);
    expect(ipcMock.setDefaultApprovalMode).toHaveBeenCalledWith("allowAll");
    expect(checked()).toEqual([label("allowAll")]);
    await choose("Full access");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(ipcMock.setDefaultApprovalMode).toHaveBeenLastCalledWith(
      "fullAccess",
    );
    expect(checked()).toEqual([label("fullAccess")]);
    expect(useApprovalMode.getState().defaultMode).toBe("fullAccess");
    // Every chat follows the new default, also one switched by hand.
    expect(ipcMock.getChatApprovalMode).toHaveBeenCalledWith(7);
    expect(useApprovalMode.getState().modes[7]).toBe("fullAccess");
    // The chosen mode reads in its colour.
    expect(
      radio("Full access").querySelector(".text-danger")?.textContent,
    ).toBe("Full access");
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
    expect(screen.getByRole("alert").textContent).toContain(
      "The setting could not be saved.",
    );
  });
});

describe("Settings: folders for every chat", () => {
  const odd = "/home/u/odd\u202Egnp.exe\u200B";
  const shown = "/home/u/odd\\u{202E}gnp.exe\\u{200B}";
  let folders: ApprovedFolder[] = [];

  beforeEach(() => {
    vi.clearAllMocks();
    folders = [
      { id: 1, path: "/home/u/all", chatId: null },
      { id: 2, path: odd, chatId: null },
    ];
    ipcMock.listApprovedFolders.mockImplementation(async () => folders);
    ipcMock.getDefaultApprovalMode.mockImplementation(async () => "ask");
  });
  afterEach(cleanup);

  it("shows a folder's hidden and direction-changing characters escaped, like the folder chip", async () => {
    await open();
    expect(ipcMock.listApprovedFolders).toHaveBeenCalledWith(null);
    const list = screen.getByText("/home/u/all").closest("ul")!;
    // Not in the text, the tooltips or the Remove buttons' names.
    const said = [
      list.textContent ?? "",
      ...[...list.querySelectorAll("[title], [aria-label]")].map(
        (e) =>
          `${e.getAttribute("title") ?? ""} ${e.getAttribute("aria-label") ?? ""}`,
      ),
    ].join("\n");
    expect(said.includes("\u202E")).toBe(false);
    expect(said.includes("\u200B")).toBe(false);
    // The path reads left to right whatever its letters are, with the full path as its tooltip.
    const path = screen.getByText(shown);
    expect(path.tagName).toBe("BDI");
    expect(path.getAttribute("dir")).toBe("ltr");
    expect(path.getAttribute("title")).toBe(shown);
    // Remove still gets the folder's real id.
    folders = folders.slice(0, 1);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: `Remove ${shown}` }));
    });
    expect(ipcMock.removeApprovedFolder).toHaveBeenCalledWith(2);
    expect(screen.queryByText(shown)).toBeNull();
  });
});

describe("Audit log: decisions by the mode", () => {
  afterEach(cleanup);

  const row = (
    id: number,
    decision: string,
    note: string | null,
    tool = "write_file",
  ): AuditRow => ({
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
    expect(decisionLabel("allowed_by_mode", "Full access")).toBe(
      "allowed by Full access",
    );
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
    const cells = screen
      .getAllByRole("row")
      .slice(1)
      .map((r) => within(r).getAllByRole("cell")[3]?.textContent);
    // The note that only names the mode is not repeated after the label; a user's note is.
    expect(cells).toEqual([
      "allowed by Allow all",
      "allowed by Full access",
      "denied (“use the other folder”)",
    ]);
    const filter = screen.getByLabelText("Decision") as HTMLSelectElement;
    expect([...filter.options].map((o) => [o.value, o.text])).toContainEqual([
      "allowed_by_mode",
      "Allowed by Allow all or Full access",
    ]);
    await act(async () => {
      fireEvent.change(filter, { target: { value: "allowed_by_mode" } });
    });
    expect(ipcMock.listAudit).toHaveBeenLastCalledWith({
      tool: null,
      decision: "allowed_by_mode",
      limit: 500,
    });
  });
});
