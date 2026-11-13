import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { DiagnosticsReport, ModelBusy, RuntimeStatus } from "../lib/types";

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  modelBusy: vi.fn<() => Promise<ModelBusy>>(),
  runDiagnostics: vi.fn<(modelPath: string | null, whenIdle: boolean) => Promise<DiagnosticsReport | null>>(async () => ({
    text: "the report",
    savedTo: null,
  })),
  openLogsFolder: vi.fn(async () => undefined),
}));
vi.mock("../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn(async () => undefined) }));

const { Diagnostics } = await import("./SettingsDialog");
const change = await import("../store/modelChange");
const { useApp } = await import("../store/app");

const IDLE: ModelBusy = { chatTurns: 0, modelChecks: 0, otherApps: 0 };
const REPLY: ModelBusy = { chatTurns: 1, modelChecks: 0, otherApps: 0 };

function runtime(state: RuntimeStatus["state"], modelPath: string | null = "/m/a.gguf"): RuntimeStatus {
  return {
    state,
    backend: "vulkan",
    device: "Vulkan0",
    modelPath,
    modelName: modelPath?.split("/").pop() ?? null,
    progress: null,
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
  };
}

async function flush() {
  await act(async () => {
    for (let i = 0; i < 8; i++) await Promise.resolve();
  });
}

/** Clicks Run backend check and answers the D3 question with `label` (Cancel when null). */
async function runAndAnswer(label: string | null) {
  fireEvent.click(screen.getByRole("button", { name: "Run backend check" }));
  await flush();
  const q = change.useQuestion.getState().question;
  expect(q, "the D3 question is open").not.toBeNull();
  expect(q?.text).toBe("A reply is being written. Stop it and restart the model?");
  expect(ipcMock.runDiagnostics, "nothing runs before the answer").not.toHaveBeenCalled();
  const choice = label === null ? null : (q?.choices.find((c) => c.label === label)?.value ?? "missing");
  act(() => change.answerQuestion(q?.id ?? -1, choice));
  await flush();
}

let unregister: () => void = () => undefined;

describe("Settings, Diagnostics: the backend check asks first when the model is busy (D3)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    unregister = change.registerQuestionHost();
    change.useWaiting.setState({ change: null, kind: null });
    useApp.setState({ runtime: runtime("ready") });
  });
  afterEach(() => {
    cleanup();
    unregister();
  });

  it("Cancel runs nothing", async () => {
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    render(<Diagnostics />);
    await runAndAnswer(null);
    expect(ipcMock.runDiagnostics).not.toHaveBeenCalled();
    expect((screen.getByRole("button", { name: "Run backend check" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("Restart when it ends waits for the reply, with the waiting line meanwhile", async () => {
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    let finish: (r: DiagnosticsReport) => void = () => undefined;
    ipcMock.runDiagnostics.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)));
    render(<Diagnostics />);
    await runAndAnswer("Restart when it ends");
    expect(ipcMock.runDiagnostics).toHaveBeenCalledWith("/m/a.gguf", true);
    expect(change.useWaiting.getState().change).toBe("reload");
    expect((screen.getByRole("button", { name: /Running/ }) as HTMLButtonElement).disabled).toBe(true);
    await act(async () => finish({ text: "the report", savedTo: null }));
    expect(screen.getByText("the report")).toBeTruthy();
    expect(change.useWaiting.getState().change).toBeNull();
  });

  it("Stop and restart stops what uses the model", async () => {
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    render(<Diagnostics />);
    await runAndAnswer("Stop and restart");
    expect(ipcMock.runDiagnostics).toHaveBeenCalledWith("/m/a.gguf", false);
    expect(await screen.findByText("the report")).toBeTruthy();
  });

  it("a wait cancelled from the waiting line shows no error", async () => {
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    // The waiting call ended with `load_replaced`, which the wrapper turns into null.
    ipcMock.runDiagnostics.mockResolvedValueOnce(null);
    render(<Diagnostics />);
    await runAndAnswer("Restart when it ends");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByText("the report")).toBeNull();
    expect((screen.getByRole("button", { name: "Run backend check" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("with nothing using the model it runs at once, as before", async () => {
    ipcMock.modelBusy.mockResolvedValue(IDLE);
    render(<Diagnostics />);
    fireEvent.click(screen.getByRole("button", { name: "Run backend check" }));
    expect(await screen.findByText("the report")).toBeTruthy();
    expect(change.useQuestion.getState().question).toBeNull();
    // Waits rather than stops: something that starts to use the model after the check of
    // the counts is waited for, never stopped unasked.
    expect(ipcMock.runDiagnostics).toHaveBeenCalledWith("/m/a.gguf", true);
  });

  it("with no model loaded it asks nothing and checks only the devices", async () => {
    useApp.setState({ runtime: runtime("stopped", null) });
    render(<Diagnostics />);
    fireEvent.click(screen.getByRole("button", { name: "Run backend check" }));
    expect(await screen.findByText("the report")).toBeTruthy();
    expect(ipcMock.modelBusy).not.toHaveBeenCalled();
    expect(ipcMock.runDiagnostics).toHaveBeenCalledWith(null, true);
  });
});
