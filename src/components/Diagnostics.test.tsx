import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { ChatDetail, DiagnosticsReport, ModelBusy, RuntimeStatus, ShareStatus } from "../lib/types";

// Radix positions the header's menu with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  modelBusy: vi.fn<() => Promise<ModelBusy>>(),
  shareStatus: vi.fn<() => Promise<ShareStatus>>(),
  cancelPendingReload: vi.fn<() => Promise<void>>(async () => undefined),
  runDiagnostics: vi.fn<(modelPath: string | null, whenIdle: boolean) => Promise<DiagnosticsReport | null>>(async () => ({
    text: "the report",
    savedTo: null,
  })),
  openLogsFolder: vi.fn(async () => undefined),
}));
vi.mock("../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn(async () => undefined) }));

const { Diagnostics } = await import("./SettingsDialog");
const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
const { RuntimeIndicator } = await import("./RuntimeIndicator");
const change = await import("../store/modelChange");
const { useApp } = await import("../store/app");

const IDLE: ModelBusy = { chatTurns: 0, modelChecks: 0, otherApps: 0 };
const REPLY: ModelBusy = { chatTurns: 1, modelChecks: 0, otherApps: 0 };
/** The backend check itself, once its wait is over (its own hold). */
const CHECK: ModelBusy = { chatTurns: 0, modelChecks: 1, otherApps: 0 };
const SHARE_OFF: ShareStatus = {
  enabled: false,
  port: null,
  listening: false,
  error: null,
  refused: 0,
  inUse: false,
  modelCtx: null,
};

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
    // The waiting line follows the backend's wait (`reloadWaiting`, see the header tests
    // below), not the whole check, and names the change as a restart.
    expect(change.useWaiting.getState()).toMatchObject({ change: null, kind: "reload" });
    expect((screen.getByRole("button", { name: /Running/ }) as HTMLButtonElement).disabled).toBe(true);
    await act(async () => finish({ text: "the report", savedTo: null }));
    expect(screen.getByText("the report")).toBeTruthy();
    expect(change.useWaiting.getState()).toMatchObject({ change: null, kind: null });
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

  // The report names the loaded model's path ("timed generation with …") and the logs
  // folder; a file named with a right-to-left override must not read reversed there. The
  // report keeps its lines; Copy report copies it as it was written.
  it("shows hidden and right-to-left characters in the report as codes and keeps its lines", async () => {
    const path = "/m/X-\u202Efuug.Q4_K_M.gguf";
    useApp.setState({ runtime: runtime("ready", path) });
    ipcMock.modelBusy.mockResolvedValue(IDLE);
    const text = `harnessUI 0.0.6 diagnostics\r\nbackend cpu [b1] /data/\u200Bbin\n\ntimed generation with ${path}\n  cpu:\t12 tokens/s\n`;
    ipcMock.runDiagnostics.mockResolvedValueOnce({ text, savedTo: "/home/\u202Eu/logs/diagnostics.txt" });
    render(<Diagnostics />);
    fireEvent.click(screen.getByRole("button", { name: "Run backend check" }));
    await flush();
    expect(ipcMock.runDiagnostics).toHaveBeenCalledWith(path, true);
    const pre = document.querySelector("pre") as HTMLElement;
    expect(pre.textContent).toBe(
      "harnessUI 0.0.6 diagnostics\nbackend cpu [b1] /data/\\u{200B}bin\n\ntimed generation with /m/X-\\u{202E}fuug.Q4_K_M.gguf\n  cpu:\t12 tokens/s\n",
    );
    expect(pre.className).toContain("whitespace-pre-wrap");
    const saved = screen.getByText("/home/\\u{202E}u/logs/diagnostics.txt");
    expect(saved.tagName).toBe("BDI");
    expect(saved.getAttribute("dir")).toBe("ltr");
    expect(new XMLSerializer().serializeToString(document.body)).not.toMatch(/[\u202E\u200B]/);
    fireEvent.click(screen.getByRole("button", { name: "Copy report" }));
    expect(writeText).toHaveBeenCalledWith(text);
  });
});

function localChat(): ChatDetail {
  return {
    chat: {
      id: 3,
      title: "t",
      modelRef: "local:/m/a.gguf",
      systemPrompt: null,
      leafMessageId: null,
      createdAt: 0,
      updatedAt: 0,
      thinking: true,
      workingFolder: null,
    },
    messages: [],
  };
}

/** The chat header (whose waiting line has the Cancel) with the Diagnostics tab. */
async function showWithHeader() {
  useApp.setState({ detail: localChat(), activeChatId: 3, generation: null, error: null });
  await act(async () => {
    render(
      <>
        <RuntimeIndicator />
        <Diagnostics />
      </>,
    );
  });
}

/** The backend's status changes, as its runtime events change it. */
async function backend(patch: Partial<RuntimeStatus>) {
  await act(async () => {
    useApp.setState((s) => ({ runtime: s.runtime && { ...s.runtime, ...patch } }));
  });
  await flush();
}

/** The header's waiting line ("Restarts when the reply ends · Cancel"), or null. */
function waitingLine(): HTMLElement | null {
  return screen.queryAllByRole("status").find((el) => / when .* ends/.test(el.textContent ?? "")) ?? null;
}

describe("Settings, Diagnostics: the header's waiting line is up only while the backend check waits", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.shareStatus.mockResolvedValue(SHARE_OFF);
    change.useWaiting.setState({ change: null, kind: null });
    change.useRestartNote.setState({ shown: false });
    useApp.setState({ runtime: runtime("ready") });
  });
  afterEach(cleanup);

  it("goes away once the wait is over and the check runs", async () => {
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    let finish: (r: DiagnosticsReport) => void = () => undefined;
    ipcMock.runDiagnostics.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)));
    await showWithHeader();
    await runAndAnswer("Restart when it ends");
    expect(ipcMock.runDiagnostics).toHaveBeenCalledWith("/m/a.gguf", true);

    // The backend waits for the reply: the line, with its Cancel.
    await backend({ reloadWaiting: true });
    expect(waitingLine()?.textContent).toContain("Restarts when the reply ends");
    expect(within(waitingLine() as HTMLElement).getByRole("button", { name: "Cancel" })).toBeTruthy();

    // The reply ended: the wait is over and the check runs for minutes, a test load and
    // then the timed generation on the loaded test copy (ready, held by the check itself).
    ipcMock.modelBusy.mockResolvedValue(CHECK);
    await backend({ reloadWaiting: false, state: "starting", progress: 0.5 });
    await backend({ state: "ready", progress: 1 });
    expect(waitingLine(), "no waiting line once nothing waits").toBeNull();
    expect((screen.getByRole("button", { name: /Running/ }) as HTMLButtonElement).disabled).toBe(true);

    await act(async () => finish({ text: "the report", savedTo: null }));
    expect(screen.getByText("the report")).toBeTruthy();
    expect(waitingLine()).toBeNull();
  });

  it("offers Cancel only once the backend waits; that Cancel ends the check with no report", async () => {
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    let end: (r: DiagnosticsReport | null) => void = () => undefined;
    ipcMock.runDiagnostics.mockImplementationOnce(() => new Promise((resolve) => (end = resolve)));
    // The backend ends the waiting check with `load_replaced`, which the wrapper turns into null.
    ipcMock.cancelPendingReload.mockImplementationOnce(async () => end(null));
    await showWithHeader();
    await runAndAnswer("Restart when it ends");

    // Until the backend waits, no Cancel is offered: it would cancel nothing.
    expect(waitingLine(), "a Cancel before anything waits").toBeNull();

    await backend({ reloadWaiting: true });
    const line = waitingLine();
    expect(line?.textContent).toContain("Restarts when the reply ends");
    await act(async () => {
      fireEvent.click(within(line as HTMLElement).getByRole("button", { name: "Cancel" }));
    });
    await backend({ reloadWaiting: false });
    expect(ipcMock.cancelPendingReload).toHaveBeenCalledTimes(1);
    expect(waitingLine()).toBeNull();
    expect(screen.queryByText("the report")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect((screen.getByRole("button", { name: "Run backend check" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("shows no waiting line when the check did not have to wait (the reply ended first)", async () => {
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    let finish: (r: DiagnosticsReport) => void = () => undefined;
    ipcMock.runDiagnostics.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)));
    await showWithHeader();
    await runAndAnswer("Restart when it ends");
    // The backend found the model free and went ahead at once: never `reloadWaiting`.
    ipcMock.modelBusy.mockResolvedValue(CHECK);
    expect(waitingLine()).toBeNull();
    await backend({ state: "starting", progress: 0.2 });
    await backend({ state: "ready", progress: 1 });
    expect(waitingLine()).toBeNull();
    await act(async () => finish({ text: "the report", savedTo: null }));
    expect(screen.getByText("the report")).toBeTruthy();
  });

  it("comes up for a reply that began while the check probed; that Cancel still gives the report", async () => {
    // Nothing used the model: nothing is asked and the check runs at once.
    ipcMock.modelBusy.mockResolvedValue(IDLE);
    let end: (r: DiagnosticsReport | null) => void = () => undefined;
    ipcMock.runDiagnostics.mockImplementationOnce(() => new Promise((resolve) => (end = resolve)));
    // The backend ends that wait with the report of the devices, having replaced nothing.
    const text =
      "backend cpu [dev]\n\ntimed generation with /m/a.gguf\n  not run: cancelled while it waited for the model to be free (nothing was changed)\n";
    ipcMock.cancelPendingReload.mockImplementationOnce(async () => end({ text, savedTo: null }));
    await showWithHeader();
    fireEvent.click(screen.getByRole("button", { name: "Run backend check" }));
    await flush();
    expect(ipcMock.runDiagnostics).toHaveBeenCalledWith("/m/a.gguf", true);
    // The check probes the backends, holding the model; a reply begins meanwhile.
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 1, modelChecks: 1, otherApps: 0 });
    expect(waitingLine(), "nothing waits yet").toBeNull();
    // Right before its first test load the backend waits for that reply.
    await backend({ reloadWaiting: true });
    const line = waitingLine();
    expect(line?.textContent).toContain("Restarts when the reply ends");
    await act(async () => {
      fireEvent.click(within(line as HTMLElement).getByRole("button", { name: "Cancel" }));
    });
    await backend({ reloadWaiting: false });
    expect(ipcMock.cancelPendingReload).toHaveBeenCalledTimes(1);
    expect(waitingLine()).toBeNull();
    expect(screen.getByText(/not run: cancelled while it waited/)).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    expect((screen.getByRole("button", { name: "Run backend check" }) as HTMLButtonElement).disabled).toBe(false);
  });
});
