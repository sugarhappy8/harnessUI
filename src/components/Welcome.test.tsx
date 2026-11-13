import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { RuntimeStatus } from "../lib/types";

vi.mock("../lib/ipc", () => ({ isTauri: () => false }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const { useApp } = await import("../store/app");
const { Welcome } = await import("./Welcome");
const { Composer } = await import("./Composer");

const ready: RuntimeStatus = {
  state: "ready",
  backend: "vulkan",
  device: "Vulkan0",
  modelPath: "/models/a.gguf",
  modelName: "a.gguf",
  progress: 1,
  stage: null,
  nCtx: 65536,
  supportsTools: true,
  preserveReasoning: false,
  thinking: true,
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
};

describe("the welcome screen", () => {
  afterEach(cleanup);

  it("offers recent models and a file when nothing is loaded", () => {
    const pickModel = vi.fn(async () => undefined);
    useApp.setState({
      runtime: null,
      modelOptions: [{ ref: "local:/models/b.gguf", label: "b.gguf", source: "local", contextLength: null }],
      pickModel,
    });
    render(<Welcome />);
    expect(screen.getByText("Pick up where you left off")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Open a GGUF file/ })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "b.gguf" }));
    expect(pickModel).toHaveBeenCalledWith("local:/models/b.gguf");
  });

  it("shows the loaded model and starts a chat with it", () => {
    const pickModel = vi.fn(async () => undefined);
    useApp.setState({ runtime: ready, modelOptions: [], pickModel });
    render(<Welcome />);
    expect(screen.getByText("a.gguf")).toBeTruthy();
    expect(screen.getByText("Ready on Graphics card (Vulkan) · Remembers about 49,000 words")).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/vulkan|Vulkan0|context/);
    fireEvent.click(screen.getByRole("button", { name: "Chat with it" }));
    expect(pickModel).toHaveBeenCalledWith("local:/models/a.gguf");
  });

  // A GGUF named with a right-to-left override reads reversed unless the name is escaped
  // (the header's model menu and the model list do the same).
  it("shows model names with hidden and right-to-left characters as codes", () => {
    useApp.setState({
      runtime: { ...ready, modelName: "X-\u202Efuug.Q4_K_M.gguf\u200B", modelPath: "/models/X-\u202Efuug.Q4_K_M.gguf\u200B" },
      modelOptions: [],
    });
    render(<Welcome />);
    const name = screen.getByText("X-\\u{202E}fuug.Q4_K_M.gguf\\u{200B}");
    expect(name.tagName).toBe("BDI");
    expect(name.getAttribute("dir")).toBe("ltr");
    expect(new XMLSerializer().serializeToString(document.body)).not.toMatch(/[\u202E\u200B]/);
    cleanup();
    const pickModel = vi.fn(async () => undefined);
    useApp.setState({
      runtime: null,
      modelOptions: [{ ref: "local:/models/Y-\u202Efuug.gguf", label: "Y-\u202Efuug.gguf", source: "local", contextLength: null }],
      pickModel,
    });
    render(<Welcome />);
    const option = screen.getByText("Y-\\u{202E}fuug.gguf");
    expect(option.tagName).toBe("BDI");
    expect(option.getAttribute("dir")).toBe("ltr");
    expect(option.closest("button")?.title).toBe("Y-\\u{202E}fuug.gguf");
    expect(new XMLSerializer().serializeToString(document.body)).not.toMatch(/[\u202E\u200B]/);
    fireEvent.click(option);
    expect(pickModel).toHaveBeenCalledWith("local:/models/Y-\u202Efuug.gguf");
  });

  // A suggestion only fills the message box: nothing is sent until the user sends it.
  it("writes a chosen idea into the message box without sending it", () => {
    useApp.setState({ runtime: ready, modelOptions: [] });
    const onSend = vi.fn();
    render(
      <>
        <Welcome />
        <Composer busy={false} disabledReason={null} onSend={onSend} onStop={() => undefined} />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: /Run and fix/ }));
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement;
    expect(box.value).toBe("Run the tests in my project folder and fix whatever fails.");
    expect(onSend).not.toHaveBeenCalled();
  });

  // The end-to-end tests click buttons by their label, first match in the page. An idea
  // whose text contained one of those labels would be clicked instead.
  it("uses no wording that is another button's label", () => {
    useApp.setState({ runtime: null, modelOptions: [] });
    render(<Welcome />);
    const text = screen.getAllByRole("button").map((b) => b.textContent ?? "").join(" | ");
    for (const label of ["Use", "Save", "Send", "Stop", "Skills", "Browse", "General", "Installed", "Deny", "Continue", "Tools", "Models", "Settings", "New chat", "Allow"]) {
      expect(text, label).not.toContain(label);
    }
  });
});
