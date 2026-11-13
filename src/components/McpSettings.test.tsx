import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  listMcpServers: vi.fn(async () => []),
  onMcpChanged: vi.fn(async () => () => undefined),
  getSetting: vi.fn<(key: string) => Promise<unknown>>(),
  setSetting: vi.fn<(key: string, value: unknown) => Promise<void>>(async () => undefined),
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

  it("shows nothing when no server was renamed", async () => {
    ipcMock.getSetting.mockResolvedValue(null);
    render(<McpSettings />);
    expect(await screen.findByText("MCP servers")).toBeTruthy();
    await Promise.resolve();
    expect(screen.queryByRole("status")).toBeNull();
    expect(ipcMock.setSetting).not.toHaveBeenCalled();
  });
});
