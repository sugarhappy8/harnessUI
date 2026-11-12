import { afterEach, describe, expect, it, vi } from "vitest";

const core = vi.hoisted(() => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/core", () => core);

import { loadLocalModel, reloadLocalModel, unloadModel } from "./ipc";

const replaced = { code: "load_replaced", message: "Replaced.", hint: "", details: "" };

describe("changes of what is loaded", () => {
  afterEach(() => core.invoke.mockReset());

  it("send whenIdle only as given, and null settings for the model's own", async () => {
    core.invoke.mockResolvedValue(undefined);
    await expect(loadLocalModel("/m/a.gguf")).resolves.toBe(true);
    expect(core.invoke).toHaveBeenLastCalledWith("load_local_model", {
      path: "/m/a.gguf",
      settings: null,
      whenIdle: null,
    });
    await loadLocalModel("/m/a.gguf", null, true);
    expect(core.invoke).toHaveBeenLastCalledWith("load_local_model", {
      path: "/m/a.gguf",
      settings: null,
      whenIdle: true,
    });
    await expect(unloadModel()).resolves.toBe(true);
    expect(core.invoke).toHaveBeenLastCalledWith("unload_model", { whenIdle: null });
    await unloadModel(true);
    expect(core.invoke).toHaveBeenLastCalledWith("unload_model", { whenIdle: true });
    await expect(reloadLocalModel(true, false)).resolves.toBe(true);
    expect(core.invoke).toHaveBeenLastCalledWith("reload_local_model", { force: true, whenIdle: false });
  });

  it("resolve to false when replaced or cancelled, and pass other errors on", async () => {
    core.invoke.mockRejectedValue(replaced);
    await expect(loadLocalModel("/m/a.gguf", null, true)).resolves.toBe(false);
    await expect(unloadModel(true)).resolves.toBe(false);
    await expect(reloadLocalModel(false, true)).resolves.toBe(false);
    const failed = { code: "model_missing", message: "Missing.", hint: "", details: "" };
    core.invoke.mockRejectedValue(failed);
    await expect(loadLocalModel("/m/a.gguf")).rejects.toBe(failed);
    await expect(unloadModel()).rejects.toBe(failed);
  });
});
