import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import type { ChatDetail, RuntimeStatus } from "../lib/types";

// [0.0.7 P5] The chat header's "Cloud" mark beside the model's name, and Thinking on and off
// for Claude on its endpoint. The header's own components talk to the backend; the mark and
// the switch are what is tested here.
vi.mock("../lib/ipc", () => ({ isTauri: () => false }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("./ModelPicker", () => ({ ModelPicker: () => <span data-model-name>model</span> }));
vi.mock("./RuntimeIndicator", () => ({ RuntimeIndicator: () => null }));
vi.mock("./ToolsPanel", () => ({ ToolsPanel: () => null }));
vi.mock("./AllowAllButton", () => ({ ApprovalModeMenu: () => null }));
vi.mock("./Composer", () => ({ Composer: () => null }));
vi.mock("./SystemPromptDialog", () => ({ SystemPromptDialog: () => null }));
vi.mock("./MessageItem", () => ({ MessageItem: () => null }));

// jsdom does not scroll.
Element.prototype.scrollTo = () => undefined;

const { useApp } = await import("../store/app");
const { ChatView } = await import("./ChatView");
const { cloudMarkTitle } = await import("./CloudMark");

function detail(modelRef: string, extra: Partial<ChatDetail["chat"]>): ChatDetail {
  return {
    chat: {
      id: 1,
      title: "t",
      modelRef,
      systemPrompt: null,
      leafMessageId: null,
      createdAt: 0,
      updatedAt: 0,
      thinking: true,
      workingFolder: null,
      ...extra,
    },
    messages: [],
    summary: null,
  };
}

function show(d: ChatDetail) {
  useApp.setState({ activeChatId: 1, detail: d, generation: null, turnNotes: null, error: null, approvals: [] });
  return render(<ChatView />);
}

describe("the chat header's Cloud mark", () => {
  afterEach(() => {
    cleanup();
    useApp.setState({ runtime: null });
  });

  it("stands beside the model's name and says where the chat goes", () => {
    show(detail("remote:3:claude-opus-5-5", { cloudHost: "api.anthropic.com", endpointThinking: true }));
    const line = document.querySelector("[data-model-line]") as HTMLElement;
    const mark = line.querySelector("[data-cloud-mark]") as HTMLElement;
    expect(mark).toBeTruthy();
    expect(mark.title).toBe("Messages and files in this chat go to api.anthropic.com.");
    expect(mark.title).toBe(cloudMarkTitle("api.anthropic.com"));
    expect(mark.textContent).toContain("Cloud");
    // Right after the name.
    expect(mark.previousElementSibling?.hasAttribute("data-model-name")).toBe(true);
  });

  it("is not shown for a model on this computer or the local network", () => {
    useApp.setState({ runtime: { state: "ready", nCtx: 4_096, thinking: false, supportsTools: true } as RuntimeStatus });
    show(detail("local:/models/a.gguf", {}));
    expect(document.querySelector("[data-cloud-mark]")).toBeNull();
    cleanup();
    // An endpoint on the local network: the backend sends no host.
    show(detail("remote:2:qwen3", { cloudHost: null, endpointThinking: false }));
    expect(document.querySelector("[data-cloud-mark]")).toBeNull();
  });

  it("offers Thinking on and off for Claude, and not for other endpoints", () => {
    show(detail("remote:3:claude-sonnet-5", { cloudHost: "api.anthropic.com", endpointThinking: true, thinking: false }));
    const button = screen.getByRole("button", { name: "Thinking off" });
    expect(button.getAttribute("aria-pressed")).toBe("false");
    cleanup();
    show(detail("remote:4:gpt-5", { cloudHost: "api.openai.com", endpointThinking: false }));
    expect(screen.queryByRole("button", { name: /Thinking/ })).toBeNull();
  });
});
