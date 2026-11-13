import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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
    // Tall enough for every section at the default window size (1200 x 800). In a shorter
    // window it shrinks, leaving 4.5rem above and below: nothing is drawn past its bottom
    // edge. (The chat's header bar is drawn under every dialog: `#root` is its own stacking
    // layer in styles.css, checked by the settings-pages end-to-end scenario.)
    expect(classes(dialog)).toContain("max-h-[calc(100vh-9rem)]");
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

// jsdom does no layout: a short window is drawn here by hand. The list sits 49 px down the
// dialog and shows 200 px; each row is 32 px tall, 36 px apart, after the list's 12 px padding.
const LIST_TOP = 49;
const SHOWN = 200;
const ROW = 32;
const rowTop = (i: number) => 12 + i * 36;

function layout(el: HTMLElement) {
  const role = el.getAttribute("role");
  if (role === "tab") {
    const i = [...(el.parentElement?.children ?? [])].indexOf(el);
    return { offsetTop: LIST_TOP + rowTop(i), offsetHeight: ROW, clientHeight: ROW };
  }
  if (role === "tablist") return { offsetTop: LIST_TOP, offsetHeight: SHOWN, clientHeight: SHOWN };
  return { offsetTop: 0, offsetHeight: 0, clientHeight: 0 };
}

describe("Settings: opening on a section shows its row in the list", () => {
  const props = ["offsetTop", "offsetHeight", "clientHeight"] as const;
  const saved = props.map((p) => [p, Object.getOwnPropertyDescriptor(HTMLElement.prototype, p)] as const);
  beforeEach(() => {
    for (const p of props) {
      Object.defineProperty(HTMLElement.prototype, p, {
        configurable: true,
        get(this: HTMLElement) {
          return layout(this)[p];
        },
      });
    }
  });
  afterEach(() => {
    cleanup();
    for (const [p, d] of saved) {
      if (d) Object.defineProperty(HTMLElement.prototype, p, d);
      else delete (HTMLElement.prototype as unknown as Record<string, unknown>)[p];
    }
  });

  function shown(list: HTMLElement, name: string) {
    const i = SECTIONS.indexOf(name);
    return list.scrollTop <= rowTop(i) && rowTop(i) + ROW <= list.scrollTop + SHOWN;
  }

  it.each(["Diagnostics", "Audit log", "Permissions", "Browser"])(
    "scrolls %s into view when the dialog opens on it in a short window",
    async (name) => {
      const value = { Diagnostics: "diagnostics", "Audit log": "audit", Permissions: "permissions", Browser: "browser" }[name];
      const dialog = await open(value);
      const list = within(dialog).getByRole("tablist", { name: "Settings sections" });
      expect(within(list).getByRole("tab", { name }).getAttribute("aria-selected")).toBe("true");
      expect(shown(list, name)).toBe(true);
    },
  );

  it("leaves the list at the top when the chosen section is already in view", async () => {
    const dialog = await open("general");
    const list = within(dialog).getByRole("tablist", { name: "Settings sections" });
    expect(list.scrollTop).toBe(0);
    expect(shown(list, "General")).toBe(true);
  });
});
