import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ModelBusy, RuntimeStatus } from "../lib/types";

const ipcMock = vi.hoisted(() => ({
  modelBusy: vi.fn<() => Promise<ModelBusy>>(),
  loadLocalModel: vi.fn<(path: string, settings?: unknown, whenIdle?: boolean) => Promise<boolean>>(async () => true),
  unloadModel: vi.fn<(whenIdle?: boolean) => Promise<boolean>>(async () => true),
  reloadLocalModel: vi.fn<(force: boolean, whenIdle: boolean) => Promise<boolean>>(async () => true),
  cancelPendingReload: vi.fn(async () => undefined),
  getLocalModelSettings: vi.fn(async () => {
    throw new Error("the defaults must not be passed to a load any more");
  }),
  setChatModel: vi.fn(async () => undefined),
  createChat: vi.fn(async (modelRef: string | null) => ({ id: 9, modelRef })),
  getChat: vi.fn(async (id: number) => ({ chat: { id, modelRef: null }, messages: [] })),
  listChats: vi.fn(async () => []),
  listModelOptions: vi.fn(async () => []),
}));
vi.mock("../lib/ipc", () => ipcMock);

const change = await import("./modelChange");
const { useApp } = await import("./app");

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
  };
}

async function flush() {
  for (let i = 0; i < 8; i++) await Promise.resolve();
}

/** Answers the open question with the choice labelled `label` (or Cancel when null). */
async function answer(label: string | null) {
  await flush();
  const q = change.useQuestion.getState().question;
  expect(q, "a question is open").not.toBeNull();
  if (!q) return;
  const choice = label === null ? null : (q.choices.find((c) => c.label === label)?.value ?? "missing");
  change.answerQuestion(q.id, choice);
}

let unregister: () => void = () => undefined;

describe("D3: changing what is loaded asks first when the model is busy", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    unregister = change.registerQuestionHost();
    useApp.setState({ runtime: null, activeChatId: 3, detail: null, error: null });
    change.useWaiting.setState({ change: null });
    change.useRestartNote.setState({ shown: false });
  });
  afterEach(() => unregister());

  it("acts at once without asking when nothing is loaded or nothing uses the model", async () => {
    await expect(change.askBeforeChange(null)).resolves.toEqual({});
    await expect(change.askBeforeChange(runtime("stopped", null))).resolves.toEqual({});
    expect(ipcMock.modelBusy).not.toHaveBeenCalled();
    ipcMock.modelBusy.mockResolvedValue(IDLE);
    await expect(change.askBeforeChange(runtime("ready"))).resolves.toEqual({});
    expect(ipcMock.modelBusy).toHaveBeenCalledTimes(1);
    expect(change.useQuestion.getState().question).toBeNull();
  });

  it("asks with the three answers and maps them to when_idle", async () => {
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    const cancelled = change.askBeforeChange(runtime("ready"));
    await flush();
    const q = change.useQuestion.getState().question;
    expect(q?.text).toBe("A reply is being written. Stop it and restart the model?");
    expect(q?.cancel).toBe("Cancel");
    expect(q?.choices.map((c) => c.label)).toEqual(["Restart when it ends", "Stop and restart"]);
    expect(q?.busy).toBe(true);
    await answer(null);
    await expect(cancelled).resolves.toBeNull();

    const wait = change.askBeforeChange(runtime("ready"));
    await answer("Restart when it ends");
    await expect(wait).resolves.toEqual({ whenIdle: true });

    const now = change.askBeforeChange(runtime("starting"));
    await answer("Stop and restart");
    await expect(now).resolves.toEqual({ whenIdle: false });
  });

  it("names the other app or the model check when that is what uses the model", async () => {
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 0, modelChecks: 0, otherApps: 1 });
    const a = change.askBeforeChange(runtime("ready"));
    await flush();
    expect(change.useQuestion.getState().question?.text).toBe(
      "Another app is using the model. Stop its request and restart the model?",
    );
    await answer(null);
    await a;
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 0, modelChecks: 1, otherApps: 0 });
    const b = change.askBeforeChange(runtime("ready"));
    await flush();
    expect(change.useQuestion.getState().question?.text).toBe("A model check is running. Stop it and restart the model?");
    await answer(null);
    await b;
  });

  it("lets a newer question cancel the one still open", async () => {
    const first = change.ask({ text: "first", cancel: "Cancel", choices: [{ label: "Yes", value: "y" }], busy: false });
    const second = change.ask({ text: "second", cancel: "Cancel", choices: [{ label: "Yes", value: "y" }], busy: false });
    await expect(first).resolves.toBeNull();
    expect(change.useQuestion.getState().question?.text).toBe("second");
    await answer("Yes");
    await expect(second).resolves.toBe("y");
  });

  it("reloads with force and when_idle, remembers what waits, and notes the restart", async () => {
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    let finish: (v: boolean) => void = () => undefined;
    ipcMock.reloadLocalModel.mockImplementationOnce(() => new Promise<boolean>((r) => (finish = r)));
    const done = change.reloadModel(runtime("ready"), true);
    await answer("Restart when it ends");
    await flush();
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(true, true);
    expect(change.useWaiting.getState().change).toBe("reload");
    finish(true);
    await expect(done).resolves.toBe(true);
    expect(change.useWaiting.getState().change).toBeNull();
    expect(change.useRestartNote.getState().shown).toBe(true);

    // Idle: Apply and restart restarts at once, without force and without waiting.
    ipcMock.modelBusy.mockResolvedValue(IDLE);
    change.useRestartNote.setState({ shown: false });
    await change.reloadModel(runtime("ready"), false);
    expect(ipcMock.reloadLocalModel).toHaveBeenLastCalledWith(false, false);
    expect(change.useWaiting.getState().change).toBeNull();

    // A cancelled wait (load_replaced) is not a restart.
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    change.useRestartNote.setState({ shown: false });
    ipcMock.reloadLocalModel.mockResolvedValueOnce(false);
    const replaced = change.reloadModel(runtime("ready"), true);
    await answer("Restart when it ends");
    await expect(replaced).resolves.toBe(false);
    expect(change.useRestartNote.getState().shown).toBe(false);
  });

  it("unloads after the question, and does nothing when it is cancelled", async () => {
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    const cancelled = change.unloadModel(runtime("ready"));
    await answer(null);
    await expect(cancelled).resolves.toBe(false);
    expect(ipcMock.unloadModel).not.toHaveBeenCalled();
    const stop = change.unloadModel(runtime("ready"));
    await answer("Stop and restart");
    await stop;
    expect(ipcMock.unloadModel).toHaveBeenCalledWith(false);
  });

  it("keeps the newer waiting change when an older one ends", async () => {
    let endFirst: (v: boolean) => void = () => undefined;
    const first = change.runChange("reload", true, () => new Promise<boolean>((r) => (endFirst = r)));
    expect(change.useWaiting.getState().change).toBe("reload");
    let endSecond: (v: boolean) => void = () => undefined;
    const second = change.runChange("unload", true, () => new Promise<boolean>((r) => (endSecond = r)));
    expect(change.useWaiting.getState().change).toBe("unload");
    endFirst(false);
    await first;
    expect(change.useWaiting.getState().change).toBe("unload");
    endSecond(true);
    await second;
    expect(change.useWaiting.getState().change).toBeNull();
    // A change that does not wait never shows the waiting line.
    await change.runChange("switch", false, async () => {
      expect(change.useWaiting.getState().change).toBeNull();
      return true;
    });
  });
});

describe("pickModel (store): the model's own settings, and D3 before a switch", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    unregister = change.registerQuestionHost();
    useApp.setState({ runtime: null, activeChatId: 3, detail: null, error: null });
    change.useWaiting.setState({ change: null });
  });
  afterEach(() => unregister());

  it("loads a local model with no settings, so the backend uses the model's own", async () => {
    await useApp.getState().pickModel("local:/m/b.gguf");
    expect(ipcMock.getLocalModelSettings).not.toHaveBeenCalled();
    expect(ipcMock.setChatModel).toHaveBeenCalledWith(3, "local:/m/b.gguf");
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/m/b.gguf", null, undefined);
    expect(useApp.getState().error).toBeNull();
  });

  it("asks before replacing a busy model; Cancel leaves the chat and the model alone", async () => {
    useApp.setState({ runtime: runtime("ready", "/m/a.gguf") });
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    const cancelled = useApp.getState().pickModel("local:/m/b.gguf");
    await answer(null);
    await cancelled;
    expect(ipcMock.setChatModel).not.toHaveBeenCalled();
    expect(ipcMock.loadLocalModel).not.toHaveBeenCalled();

    const wait = useApp.getState().pickModel("local:/m/b.gguf");
    await answer("Restart when it ends");
    await wait;
    expect(ipcMock.setChatModel).toHaveBeenCalledWith(3, "local:/m/b.gguf");
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/m/b.gguf", null, true);
  });

  it("does not restart the model that is already loaded, and never asks for an endpoint", async () => {
    useApp.setState({ runtime: runtime("ready", "/m/a.gguf") });
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    const onChatSet = vi.fn();
    await useApp.getState().pickModel("local:/m/a.gguf", { onChatSet });
    await useApp.getState().pickModel("provider:1:fake");
    expect(ipcMock.modelBusy).not.toHaveBeenCalled();
    expect(ipcMock.loadLocalModel).not.toHaveBeenCalled();
    expect(ipcMock.setChatModel).toHaveBeenCalledTimes(2);
    expect(onChatSet).toHaveBeenCalledTimes(1);
  });

  it("calls onChatSet after the chat has the model and before the load, and shows a failed load", async () => {
    const order: string[] = [];
    ipcMock.setChatModel.mockImplementationOnce(async () => {
      order.push("chat");
    });
    ipcMock.loadLocalModel.mockImplementationOnce(async () => {
      order.push("load");
      throw { code: "model_missing", message: "Missing.", nextStep: "", details: "" };
    });
    await useApp.getState().pickModel("local:/m/b.gguf", { onChatSet: () => order.push("closed") });
    expect(order).toEqual(["chat", "closed", "load"]);
    expect(useApp.getState().error?.code).toBe("model_missing");
  });
});
