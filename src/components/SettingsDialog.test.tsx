import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, within } from "@testing-library/react";

// The sections' own pages are not under test here: each is a stub, so only the dialog's
// frame (its size, the section list and its scrolling) is rendered.
vi.mock("../lib/ipc", () => ({ isTauri: () => false }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn(async () => undefined) }));
vi.mock("./PermissionsSettings", () => ({ PermissionsSettings: () => <p>permissions page</p>, AuditLogView: () => <p>audit page</p> }));
vi.mock("./McpSettings", () => ({ McpSettings: () => <p>mcp page</p> }));
vi.mock("./SkillsSettings", () => ({ SkillsSettings: () => <p>skills page</p> }));
vi.mock("./BrowserSettings", () => ({ BrowserSettings: () => <p>browser page</p> }));
vi.mock("./ComputerSettings", () => ({ ComputerSettings: () => <p>computer page</p> }));
vi.mock("./GitSettings", () => ({ GitSettings: () => <p>git page</p> }));
vi.mock("./SearchSettings", () => ({ SearchSettings: () => <p>search page</p> }));
vi.mock("./evals/Benchmarks", () => ({ Benchmarks: () => <p>benchmarks page</p> }));
vi.mock("./evals/ModelCheck", () => ({ ModelCheck: () => <p>model check page</p> }));
vi.mock("./settings/LocalModels", () => ({ LocalModels: () => <p>local models page</p> }));
vi.mock("./settings/OtherApps", () => ({ OtherApps: () => <p>other apps page</p> }));
vi.mock("./settings/Reverse", () => ({ Reverse: () => <p>reverse page</p> }));

const { SettingsDialog } = await import("./SettingsDialog");

const SECTIONS = [
  "General",
  "Local models",
  "Reverse engineering",
  "Other apps",
  "Endpoints",
  "Hugging Face",
  "MCP servers",
  "Skills",
  "Git",
  "Browser",
  "Computer use",
  "Search",
  "Permissions",
  "Audit log",
  "Diagnostics",
];

const classes = (el: HTMLElement) => el.className.split(/\s+/);

async function open(initialTab = "skills") {
  await act(async () => {
    render(<SettingsDialog open onOpenChange={() => undefined} initialTab={initialTab} />);
  });
  return screen.getByRole("dialog");
}

describe("Settings: the section list stays inside the dialog and scrolls", () => {
  afterEach(() => cleanup());

  it("bounds the dialog to the window's height", async () => {
    const dialog = await open();
    // Tall enough for every section at the default window size (1200 x 800), never
    // taller than nine tenths of the window, so nothing is drawn past its bottom edge.
    expect(classes(dialog)).toContain("max-h-[90vh]");
    expect(classes(dialog)).toContain("h-[40rem]");
    expect(classes(dialog)).not.toContain("h-[34rem]");
  });

  it("lists every section inside a list that scrolls when the window is short", async () => {
    const dialog = await open();
    const list = within(dialog).getByRole("tablist", { name: "Settings sections" });
    expect(within(list).getAllByRole("tab").map((t) => t.textContent)).toEqual(SECTIONS);
    // The list may shrink below its content's height (min-h-0) and then scrolls.
    expect(classes(list)).toContain("min-h-0");
    expect(classes(list)).toContain("overflow-y-auto");
    // Wide enough that "Reverse engineering" fits on one line in most fonts.
    expect(classes(list)).toContain("w-48");
    expect(classes(list)).not.toContain("w-44");
    // The list's row is the part of the dialog that shrinks.
    const row = list.parentElement as HTMLElement;
    expect(classes(row)).toEqual(expect.arrayContaining(["flex", "min-h-0", "flex-1"]));
  });

  it("opens on the last section, which is reachable in the list", async () => {
    const dialog = await open("diagnostics");
    const last = within(dialog).getByRole("tab", { name: "Diagnostics" });
    expect(last.getAttribute("aria-selected")).toBe("true");
    expect(within(dialog).getByText("benchmarks page")).toBeTruthy();
  });
});
