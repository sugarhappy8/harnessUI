import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import type { RuntimeStatus } from "../lib/types";

vi.mock("../lib/ipc", () => ({ isTauri: () => false }));

const { Sidebar } = await import("./Sidebar");
const { useApp } = await import("../store/app");

function status(patch: Partial<RuntimeStatus>): RuntimeStatus {
  return {
    state: "ready",
    backend: "vulkan",
    device: "Vulkan0",
    modelPath: "/models/a.gguf",
    modelName: "a.gguf",
    progress: 1,
    stage: null,
    nCtx: 8192,
    supportsTools: true,
    preserveReasoning: false,
    thinking: false,
    speedTps: null,
    slow: false,
    vision: false,
    offloadedLayers: null,
    totalLayers: null,
    error: null,
    loadStage: null,
    adjustments: [],
    ctxSource: null,
    settingsChanged: false,
    reloadWaiting: false,
    unloadedPath: null,
    placement: null,
    ...patch,
  };
}

/** The model chip at the bottom of the chat list. */
function chip(): string {
  return screen.getByRole("button", { name: /a\.gguf|No model loaded|did not load/ }).textContent ?? "";
}

describe("the sidebar's model chip, in plain words", () => {
  afterEach(cleanup);

  function show(rt: RuntimeStatus | null) {
    useApp.setState({ runtime: rt, chats: [], activeChatId: null, search: "" });
    render(<Sidebar onOpenSettings={() => undefined} onOpenModels={() => undefined} />);
  }

  it("names where the model runs, never the backend's identifier or device id", () => {
    show(status({}));
    expect(chip()).toBe("a.ggufGraphics card (Vulkan)");
    cleanup();
    show(status({ backend: "cpu", device: null }));
    expect(chip()).toBe("a.ggufProcessor only (slower)");
    expect(document.body.textContent).not.toMatch(/vulkan|Vulkan0/);
  });

  it("says loading, unloaded and nothing loaded with capitals and the model's name", () => {
    show(status({ state: "starting" }));
    expect(chip()).toBe("a.ggufLoading…");
    cleanup();
    show(status({ state: "stopped", modelPath: null, modelName: null, unloadedPath: "/models/a.gguf" }));
    expect(chip()).toBe("a.ggufUnloaded to free memory");
    cleanup();
    show(null);
    expect(chip()).toBe("No model loadedRuns on this computer");
    cleanup();
    show(status({ state: "failed" }));
    expect(chip()).toBe("The model did not loadRuns on this computer");
  });
});
