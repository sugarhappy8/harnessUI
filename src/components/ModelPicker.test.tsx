import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

vi.mock("../lib/ipc", () => ({ isTauri: () => false }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const { ModelPicker } = await import("./ModelPicker");
const { useApp } = await import("../store/app");

async function openMenu() {
  await act(async () => {
    render(<ModelPicker />);
  });
  await act(async () => {
    fireEvent.keyDown(screen.getByRole("button", { name: "Model for this chat" }), { key: "Enter" });
  });
}

describe("ModelPicker: a gear on each local model", () => {
  const pickModel = vi.fn(async () => undefined);
  beforeEach(() => {
    vi.clearAllMocks();
    useApp.setState({
      detail: null,
      pickModel,
      refreshModelOptions: async () => undefined,
      modelOptions: [
        { ref: "local:/models/b.gguf", label: "b.gguf", source: "local", contextLength: null },
        { ref: "provider:1:x", label: "x", source: "provider", contextLength: null },
      ],
    });
  });
  afterEach(cleanup);

  it("opens Settings for this model from the gear, without picking the model", async () => {
    const opened = vi.fn();
    window.addEventListener("app:model-settings", opened);
    await openMenu();
    const gears = screen.getAllByRole("menuitem", { name: /^Settings for this model/ });
    expect(gears).toHaveLength(1);
    expect(gears[0]?.getAttribute("aria-label")).toBe("Settings for this model: b.gguf");
    await act(async () => {
      fireEvent.click(gears[0] as HTMLElement);
    });
    expect((opened.mock.calls[0]?.[0] as CustomEvent).detail).toBe("/models/b.gguf");
    expect(pickModel).not.toHaveBeenCalled();
    window.removeEventListener("app:model-settings", opened);
  });

  it("still picks a model by its name", async () => {
    await openMenu();
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "b.gguf" }));
    });
    expect(pickModel).toHaveBeenCalledWith("local:/models/b.gguf");
  });
});
