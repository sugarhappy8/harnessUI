import { afterEach, describe, expect, it, vi } from "vitest";

const core = vi.hoisted(() => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/core", () => core);

import { offloadMeasure, usageNow } from "./ipc";
import type { ModelLoadSettings } from "./types";

describe("0.0.6 item 6 wrappers (commands of WP10d)", () => {
  afterEach(() => core.invoke.mockReset());

  it("ask for the measurement with the unsaved choices, and for the usage with nothing", async () => {
    core.invoke.mockResolvedValue({});
    const overrides: ModelLoadSettings = {
      context: "auto",
      gpu: "default",
      saveMemory: "default",
      keepInMemory: "default",
      device: "default",
      thinkingLimit: "default",
      fine: { layers: { count: 20 }, experts: "auto", kv: "computerMemory", threads: { count: 2 }, fastAttention: "off", leaveFree: { mib: 2048 } },
    };
    await offloadMeasure("/m/a.gguf", overrides);
    expect(core.invoke).toHaveBeenLastCalledWith("offload_measure", { path: "/m/a.gguf", overrides });
    await usageNow();
    expect(core.invoke).toHaveBeenLastCalledWith("usage_now");
  });
});
