import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  listMcpServers: vi.fn(async () => []),
  onMcpChanged: vi.fn(async () => () => undefined),
  getSetting: vi.fn<(key: string) => Promise<unknown>>(),
  setSetting: vi.fn<(key: string, value: unknown) => Promise<void>>(async () => undefined),
  previewMcpConfig: vi.fn(async () => [] as unknown[]),
}));
vi.mock("../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

import { McpSettings } from "./McpSettings";

const NOTE =
  'Your server "reverse" is now called "reverse (yours)", because the app now has a built-in server with that name. It works as before, with the same settings.';

describe("McpSettings: servers renamed at start (0.0.6)", () => {
  beforeEach(() => {
    ipcMock.getSetting.mockReset();
    ipcMock.setSetting.mockClear();
  });
  afterEach(cleanup);

  it("says once which saved servers have a new name, until the owner closes the note", async () => {
    ipcMock.getSetting.mockResolvedValue([{ from: "reverse", to: "reverse (yours)" }]);
    render(<McpSettings />);
    expect(await screen.findByText(NOTE)).toBeTruthy();
    expect(ipcMock.getSetting).toHaveBeenCalledWith("mcpRenamedServers");
    fireEvent.click(screen.getByRole("button", { name: "OK" }));
    expect(screen.queryByText(NOTE)).toBeNull();
    expect(ipcMock.setSetting).toHaveBeenCalledWith("mcpRenamedServers", []);
  });

  it("says when some of a renamed server's tools were switched off", async () => {
    ipcMock.getSetting.mockResolvedValue([
      { from: "reverse", to: "reverse (yours)", toolsOff: true },
      { from: "coding_apps", to: "coding_apps (yours)" },
    ]);
    render(<McpSettings />);
    expect(
      await screen.findByText(
        'Your server "reverse" is now called "reverse (yours)", because the app now has a built-in server with that name. It works as before, with the same settings, but some of its tools were switched off because their names changed too. Switch them on again in a chat\'s Tools panel.',
      ),
    ).toBeTruthy();
    expect(
      screen.getByText(
        'Your server "coding_apps" is now called "coding_apps (yours)", because the app now has a built-in server with that name. It works as before, with the same settings.',
      ),
    ).toBeTruthy();
    expect(screen.queryByText(NOTE)).toBeNull();
  });

  it("shows nothing when no server was renamed", async () => {
    ipcMock.getSetting.mockResolvedValue(null);
    render(<McpSettings />);
    expect(await screen.findByText("MCP servers")).toBeTruthy();
    await Promise.resolve();
    expect(screen.queryByRole("status")).toBeNull();
    expect(ipcMock.setSetting).not.toHaveBeenCalled();
  });
});

// A configuration pasted from elsewhere can hide a right-to-left override in the program's path
// or arguments; the command shows with it as a code, the way the approval cards show it.
describe("McpSettings: programs with hidden and right-to-left characters", () => {
  beforeEach(() => {
    ipcMock.getSetting.mockReset();
    ipcMock.getSetting.mockResolvedValue(null);
  });
  afterEach(() => {
    cleanup();
    ipcMock.listMcpServers.mockReset();
    ipcMock.listMcpServers.mockResolvedValue([]);
  });

  it("shows a saved server's command with them as codes", async () => {
    ipcMock.listMcpServers.mockResolvedValue([
      {
        id: 1,
        name: "files",
        kind: "stdio",
        state: { type: "stopped" },
        enabled: true,
        builtin: false,
        summary: "/opt/X-\u202Etxt.exe --root /tmp",
        protocolVersion: null,
        stderrTail: [],
        toolCount: 0,
      },
    ] as never);
    render(<McpSettings />);
    const command = await screen.findByText("/opt/X-\\u{202E}txt.exe --root /tmp");
    expect(command.tagName).toBe("BDI");
    expect(command.getAttribute("dir")).toBe("ltr");
    expect(command.parentElement?.getAttribute("title")).toBe("/opt/X-\\u{202E}txt.exe --root /tmp");
    expect(new XMLSerializer().serializeToString(document.body)).not.toContain("\u202E");
  });

  it("shows a pasted server's command with them as codes before it is added", async () => {
    ipcMock.previewMcpConfig.mockResolvedValue([
      {
        name: "files",
        kind: "stdio",
        summary: "npx -y X-\u202Egnp.exe",
        exists: false,
        findings: [],
        envKeys: [],
        headerKeys: [],
      },
    ]);
    render(<McpSettings />);
    fireEvent.click(await screen.findByRole("button", { name: "Add servers" }));
    fireEvent.change(screen.getByLabelText(/Paste a configuration/), { target: { value: "{}" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check" }));
    });
    const command = screen.getByText("npx -y X-\\u{202E}gnp.exe");
    expect(command.tagName).toBe("BDI");
    expect(command.getAttribute("dir")).toBe("ltr");
    expect(new XMLSerializer().serializeToString(document.body)).not.toContain("\u202E");
  });
});
