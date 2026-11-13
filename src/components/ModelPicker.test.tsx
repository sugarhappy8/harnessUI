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
    window.addEventListener("app:open-model-settings", opened);
    await openMenu();
    const gears = screen.getAllByRole("menuitem", { name: /^Settings for this model/ });
    expect(gears).toHaveLength(1);
    expect(gears[0]?.getAttribute("aria-label")).toBe("Settings for this model: b.gguf");
    await act(async () => {
      fireEvent.click(gears[0] as HTMLElement);
    });
    expect((opened.mock.calls[0]?.[0] as CustomEvent).detail).toBe("/models/b.gguf");
    expect(pickModel).not.toHaveBeenCalled();
    window.removeEventListener("app:open-model-settings", opened);
  });

  it("keeps room for about 12 characters of the model's name, with the whole name as its tooltip (header-layout)", async () => {
    const name = "Qwen3-Coder-30B-A3B-Instruct-UD-Q4_K_XL.gguf";
    useApp.setState({
      detail: {
        chat: {
          id: 1,
          title: "t",
          modelRef: `local:/models/${name}`,
          systemPrompt: null,
          leafMessageId: null,
          createdAt: 0,
          updatedAt: 0,
          thinking: false,
          workingFolder: null,
        },
        messages: [],
        summary: null,
      },
    });
    await act(async () => {
      render(<ModelPicker />);
    });
    const button = screen.getByRole("button", { name: "Model for this chat" });
    // The button's text is the name alone (the menu has it whole too); its tooltip says it whole.
    expect(button.textContent).toBe(name);
    expect(button.title).toBe(name);
    // A crowded header never squeezes it below about 12 characters (jsdom has no layout: the
    // e2e scenario header-layout measures it). A longer name ends in an ellipsis on one line,
    // and the header wraps its buttons onto a second row before it shrinks the name further.
    expect(button.className.split(" ")).toContain("min-w-[8.75rem]");
    const shown = button.querySelector("[data-model-name]") as HTMLElement;
    expect(shown.textContent).toBe(name);
    for (const c of ["line-clamp-1", "break-all"]) expect(shown.className.split(" ")).toContain(c);
  });

  it("still picks a model by its name", async () => {
    await openMenu();
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "b.gguf" }));
    });
    expect(pickModel).toHaveBeenCalledWith("local:/models/b.gguf");
  });
});
