import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { InstalledModel } from "../../lib/types";

// [0.0.6 item 8, WP8] Importing or deleting a model in Installed reads the chat's model list
// ("On this computer") again.

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  listInstalledModels: vi.fn<() => Promise<InstalledModel[]>>(),
  diskUsage: vi.fn(async () => ({ modelsDir: "/models", totalBytes: 1 })),
  modelLoadInfo: vi.fn(async () => {
    throw new Error("not needed here");
  }),
  importModel: vi.fn(async () => undefined),
  deleteModel: vi.fn(async () => undefined),
  listModelOptions: vi.fn(async () => []),
}));
const dialog = vi.hoisted(() => ({ open: vi.fn<() => Promise<string | null>>() }));
vi.mock("../../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-dialog", () => dialog);

const { Installed } = await import("./Installed");
const { useApp } = await import("../../store/app");

const MODEL: InstalledModel = {
  id: 7,
  path: "/models/m.gguf",
  repo: null,
  fileName: "m.gguf",
  sizeBytes: 1_000,
  source: "download",
  projectorPath: null,
  summary: null,
  architectureSupported: true,
};

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
}

describe("Installed: the chat's model list follows imports and deletes", () => {
  const refreshModelOptions = vi.fn(async () => undefined);
  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.listInstalledModels.mockResolvedValue([MODEL]);
    useApp.setState({ refreshModelOptions, runtime: null });
  });
  afterEach(cleanup);

  it("reads the list again after an import", async () => {
    await act(async () => render(<Installed onClose={() => undefined} />));
    expect(refreshModelOptions).not.toHaveBeenCalled();
    dialog.open.mockResolvedValue("/elsewhere/new.gguf");
    await click(screen.getByRole("button", { name: "Import a GGUF file…" }));
    expect(ipcMock.importModel).toHaveBeenCalledWith("/elsewhere/new.gguf");
    expect(refreshModelOptions).toHaveBeenCalledTimes(1);
  });

  it("reads the list again after a delete, not after a delete that was called off", async () => {
    await act(async () => render(<Installed onClose={() => undefined} />));
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    await click(screen.getByRole("button", { name: "Delete m.gguf" }));
    expect(refreshModelOptions).not.toHaveBeenCalled();
    confirm.mockReturnValue(true);
    await click(screen.getByRole("button", { name: "Delete m.gguf" }));
    expect(ipcMock.deleteModel).toHaveBeenCalledWith(7, true);
    expect(refreshModelOptions).toHaveBeenCalledTimes(1);
    confirm.mockRestore();
  });
});
