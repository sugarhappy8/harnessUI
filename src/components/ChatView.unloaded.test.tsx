// WP2: the message box's text after "Unload to free memory" (the rest of ChatView is
// WP3b's and tested in ChatView.test.tsx).
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import type { ChatDetail, RuntimeStatus } from "../lib/types";

vi.mock("../lib/ipc", () => ({ isTauri: () => false }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("./ModelPicker", () => ({ ModelPicker: () => null }));
vi.mock("./RuntimeIndicator", () => ({ RuntimeIndicator: () => null }));
vi.mock("./ToolsPanel", () => ({ ToolsPanel: () => null }));
vi.mock("./FolderChip", () => ({ FolderChip: () => null }));
vi.mock("./AllowAllButton", () => ({ ApprovalModeMenu: () => null }));
vi.mock("./SystemPromptDialog", () => ({ SystemPromptDialog: () => null }));
vi.mock("./Composer", () => ({
  Composer: (p: { disabledReason: string | null }) => <p data-testid="composer">{p.disabledReason ?? "ready"}</p>,
}));

Element.prototype.scrollTo = () => undefined;

const { useApp } = await import("../store/app");
const { ChatView } = await import("./ChatView");

function chat(modelRef: string): ChatDetail {
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
    },
    messages: [],
    summary: null,
  };
}

function stopped(unloadedPath: string | null): RuntimeStatus {
  return {
    state: "stopped",
    backend: null,
    device: null,
    modelPath: null,
    modelName: null,
    progress: null,
    stage: null,
    nCtx: null,
    supportsTools: false,
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
    unloadedPath,
    placement: null,
  };
}

function show(runtime: RuntimeStatus, modelRef = "local:/models/a.gguf") {
  useApp.setState({ activeChatId: 1, detail: chat(modelRef), runtime, generation: null, error: null, approvals: [] });
  render(<ChatView />);
  return screen.getByTestId("composer").textContent;
}

describe("the message box after Unload to free memory", () => {
  afterEach(cleanup);

  it("says the model was unloaded and how to load it again", () => {
    expect(show(stopped("/models/a.gguf"))).toBe(
      "The model was unloaded to free memory. Choose Load at the top to load it again.",
    );
  });

  it("keeps the plain not-loaded text when nothing was unloaded by the user", () => {
    expect(show(stopped(null))).toBe("The model is not loaded. Pick it again from the list above.");
  });
});
