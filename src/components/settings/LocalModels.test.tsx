import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { BackendInfo, LocalModelSettings } from "../../lib/types";

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  listBackends: vi.fn<(reprobe: boolean) => Promise<BackendInfo[]>>(),
  getLocalModelSettings: vi.fn<() => Promise<LocalModelSettings>>(),
  setLocalModelSettings: vi.fn<(s: LocalModelSettings) => Promise<void>>(async () => undefined),
  listPacks: vi.fn(async () => []),
  onPackProgress: vi.fn(async () => () => undefined),
}));
vi.mock("../../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const { LocalModels } = await import("./LocalModels");

const BACKENDS: BackendInfo[] = [
  {
    kind: "vulkan",
    source: "bundled",
    build: "b1",
    available: true,
    devices: [
      { name: "Vulkan0", description: "AMD Radeon(TM) Graphics", totalMib: 2048, freeMib: 1500 },
      { name: "Vulkan1", description: "AMD Radeon RX 9070 XT", totalMib: 16_384, freeMib: 15_000 },
    ],
    error: null,
  },
  { kind: "rocm", source: "download", build: "b1", available: false, devices: [], error: "not installed" },
  { kind: "cpu", source: "bundled", build: "b1", available: true, devices: [], error: null },
];

function defaults(patch: Partial<LocalModelSettings> = {}): LocalModelSettings {
  return {
    ctx: 8192,
    backend: null,
    device: null,
    contextAuto: true,
    gpu: "all",
    saveMemory: false,
    keepInMemory: false,
    thinkingLimit: "noLimit",
    ...patch,
  };
}

async function show(settings: LocalModelSettings) {
  ipcMock.listBackends.mockResolvedValue(BACKENDS);
  ipcMock.getLocalModelSettings.mockResolvedValue(settings);
  await act(async () => {
    render(<LocalModels />);
  });
}

function options(label: string): string[] {
  return [...(screen.getByLabelText(label) as HTMLSelectElement).options].map((o) => o.textContent ?? "");
}

async function choose(label: string, value: string) {
  await act(async () => {
    fireEvent.change(screen.getByLabelText(label), { target: { value } });
  });
}

describe("Settings → Local models: defaults for all models", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    Object.defineProperty(navigator, "platform", { value: "Win32", configurable: true });
  });
  afterEach(cleanup);

  it("shows the same settings as the panel, in plain words", async () => {
    await show(defaults());
    expect(screen.getByText("Defaults for all models")).toBeTruthy();
    expect(options("Run models on")).toEqual([
      "Automatic (recommended)",
      "Graphics card (Vulkan)",
      "AMD graphics card (ROCm) (not available)",
      "Processor only (slower)",
    ]);
    expect((screen.getByLabelText("How much it can remember") as HTMLSelectElement).value).toBe("auto");
    expect(options("How much it can remember")).toContain("32,768 tokens, about 24,000 words");
    expect(options("Use the graphics card")).toEqual(["All of it", "As much as fits", "Don't use it"]);
    expect(options("Which graphics card")).toEqual([
      "Automatic (recommended)",
      "AMD Radeon(TM) Graphics · 2 GB",
      "AMD Radeon RX 9070 XT · 16 GB",
    ]);
    expect(options("How long it may think")).toEqual(["Default", "Short", "Medium", "Long"]);
    const text = document.body.textContent ?? "";
    expect(text).toContain("older messages are summarised automatically so it can keep going.");
    expect(text).not.toMatch(/\bvulkan\b|Vulkan0|Vulkan1|\bcpu\b/);
  });

  it("saves each change with every other default kept", async () => {
    await show(defaults({ contextAuto: false, ctx: 16_384 }));
    await choose("How much it can remember", "auto");
    expect(ipcMock.setLocalModelSettings).toHaveBeenLastCalledWith(defaults({ contextAuto: true, ctx: 16_384 }));
    await choose("How much it can remember", "4096");
    expect(ipcMock.setLocalModelSettings).toHaveBeenLastCalledWith(defaults({ contextAuto: false, ctx: 4096 }));
    await choose("Save memory", "on");
    await choose("Use the graphics card", "fit");
    await choose("Which graphics card", "Vulkan1");
    await choose("Run models on", "vulkan");
    expect(ipcMock.setLocalModelSettings).toHaveBeenLastCalledWith(
      defaults({ contextAuto: false, ctx: 4096, saveMemory: true, gpu: "fit", device: "Vulkan1", backend: "vulkan" }),
    );
  });

  it("shows the one-time upgrade note until a change, and never stores it", async () => {
    await show(defaults({ contextAuto: false, ctx: 16_384, upgradeNote: 16_384 }));
    expect(screen.getByRole("note").textContent).toBe(
      "Every model now uses the size you chose before (16,384 tokens, about 12,000 words). Choose Automatic to let the app pick the largest size that fits.",
    );
    await choose("Keep the model in memory", "on");
    expect(screen.queryByRole("note")).toBeNull();
    const sent = ipcMock.setLocalModelSettings.mock.calls[0]?.[0];
    expect(sent).toBeDefined();
    expect(sent && "upgradeNote" in sent).toBe(false);
  });

  it("says Use Apple graphics on a Mac", async () => {
    Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
    await show(defaults());
    expect(screen.getByLabelText("Use Apple graphics")).toBeTruthy();
  });
});
