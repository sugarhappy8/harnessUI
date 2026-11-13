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
  setChatModel: vi.fn<(chatId: number, modelRef: string | null) => Promise<void>>(async () => undefined),
  createChat: vi.fn(async (modelRef: string | null) => ({ id: 9, modelRef })),
  getChat: vi.fn(async (id: number) => ({ chat: { id, modelRef: null as string | null }, messages: [] })),
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
    placement: null,
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
    change.useWaiting.setState({ change: null, kind: null });
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

  it("notes a restart only when the model really restarted", async () => {
    ipcMock.modelBusy.mockResolvedValue(IDLE);
    // The backend found nothing to change (the same launch) and restarted nothing.
    ipcMock.reloadLocalModel.mockResolvedValueOnce(true);
    await expect(change.reloadModel(runtime("ready"), false)).resolves.toBe(true);
    expect(change.useRestartNote.getState().shown).toBe(false);
    // It restarted: the runtime went through starting.
    ipcMock.reloadLocalModel.mockImplementationOnce(async () => {
      useApp.setState({ runtime: runtime("starting") });
      useApp.setState({ runtime: runtime("ready") });
      return true;
    });
    await change.reloadModel(runtime("ready"), false);
    expect(change.useRestartNote.getState().shown).toBe(true);
  });

  it("reloads with force and when_idle, remembers what waits, and notes the restart", async () => {
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    let finish: (v: boolean) => void = () => undefined;
    ipcMock.reloadLocalModel.mockImplementationOnce(
      () =>
        new Promise<boolean>((r) => {
          finish = (v) => {
            useApp.setState({ runtime: runtime("starting") });
            r(v);
          };
        }),
    );
    const done = change.reloadModel(runtime("ready"), true);
    await answer("Restart when it ends");
    await vi.waitFor(() => expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(true, true));
    expect(change.useWaiting.getState().change).toBe("reload");
    finish(true);
    await expect(done).resolves.toBe(true);
    expect(change.useWaiting.getState().change).toBeNull();
    expect(change.useRestartNote.getState().shown).toBe(true);

    // Idle: Apply and restart, without force and with no question or waiting line; the
    // backend acts at once because nothing uses the model.
    ipcMock.modelBusy.mockResolvedValue(IDLE);
    change.useRestartNote.setState({ shown: false });
    await change.reloadModel(runtime("ready"), false);
    expect(ipcMock.reloadLocalModel).toHaveBeenLastCalledWith(false, true);
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

  it("never stops a user of the model it did not ask about: with no question the backend waits if one started since", async () => {
    // Nothing uses the model when asked: no question, no waiting line, but the backend is
    // told to wait (it acts at once when the model is free), so a reply or another app's
    // request that starts between the check and the command is not stopped unasked.
    ipcMock.modelBusy.mockResolvedValue(IDLE);
    ipcMock.reloadLocalModel.mockImplementationOnce(async () => {
      expect(change.useWaiting.getState().change).toBeNull();
      return true;
    });
    await expect(change.reloadModel(runtime("ready"), true)).resolves.toBe(true);
    expect(ipcMock.reloadLocalModel).toHaveBeenLastCalledWith(true, true);
    await expect(change.unloadModel(runtime("ready"))).resolves.toBe(true);
    expect(ipcMock.unloadModel).toHaveBeenLastCalledWith(true);
    await expect(change.switchModel(runtime("ready"), "/m/b.gguf")).resolves.toBe(true);
    expect(ipcMock.loadLocalModel).toHaveBeenLastCalledWith("/m/b.gguf", null, true);
    expect(change.useQuestion.getState().question).toBeNull();
    expect(change.useWaiting.getState().change).toBeNull();
  });

  it("stops nothing when the busy count cannot be read", async () => {
    ipcMock.modelBusy.mockRejectedValue(new Error("no answer"));
    await expect(change.askBeforeChange(runtime("ready"))).resolves.toEqual({});
    expect(change.useQuestion.getState().question).toBeNull();
    await change.reloadModel(runtime("ready"), true);
    expect(ipcMock.reloadLocalModel).toHaveBeenLastCalledWith(true, true);
    await change.unloadModel(runtime("ready"));
    expect(ipcMock.unloadModel).toHaveBeenLastCalledWith(true);
    await change.switchModel(runtime("starting"), "/m/b.gguf");
    expect(ipcMock.loadLocalModel).toHaveBeenLastCalledWith("/m/b.gguf", null, true);
  });

  it("remembers which change runs, so a wait the backend starts on its own is worded by it", async () => {
    let end: (v: boolean) => void = () => undefined;
    const run = change.runChange("switch", undefined, () => new Promise<boolean>((r) => (end = r)));
    expect(change.useWaiting.getState().change).toBeNull();
    expect(change.useWaiting.getState().kind).toBe("switch");
    end(true);
    await run;
    expect(change.useWaiting.getState().kind).toBeNull();
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
    change.useWaiting.setState({ change: null, kind: null });
  });
  afterEach(() => unregister());

  it("loads a local model with no settings, so the backend uses the model's own", async () => {
    await useApp.getState().pickModel("local:/m/b.gguf");
    expect(ipcMock.getLocalModelSettings).not.toHaveBeenCalled();
    expect(ipcMock.setChatModel).toHaveBeenCalledWith(3, "local:/m/b.gguf");
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/m/b.gguf", null, true);
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

  it("puts the chat back on its model when a waiting switch is cancelled, unless a newer pick replaced it", async () => {
    let chatModel: string | null = "local:/m/a.gguf";
    ipcMock.setChatModel.mockImplementation(async (_id, ref) => {
      chatModel = ref;
    });
    ipcMock.getChat.mockImplementation(async (id: number) => ({ chat: { id, modelRef: chatModel }, messages: [] }));
    await useApp.getState().openChat(3);
    useApp.setState({ runtime: runtime("ready", "/m/a.gguf") });
    ipcMock.modelBusy.mockResolvedValue(REPLY);

    // Cancel on the waiting line: the backend ends the wait with load_replaced (false).
    ipcMock.loadLocalModel.mockResolvedValueOnce(false);
    const waiting = useApp.getState().pickModel("local:/m/b.gguf");
    await answer("Restart when it ends");
    await waiting;
    expect(ipcMock.setChatModel).toHaveBeenNthCalledWith(1, 3, "local:/m/b.gguf");
    expect(ipcMock.setChatModel).toHaveBeenLastCalledWith(3, "local:/m/a.gguf");
    expect(useApp.getState().detail?.chat.modelRef).toBe("local:/m/a.gguf");
    expect(useApp.getState().error).toBeNull();

    // A newer pick replaced the wait: the chat keeps the newer model.
    ipcMock.setChatModel.mockClear();
    let endWait: (done: boolean) => void = () => undefined;
    ipcMock.loadLocalModel.mockImplementationOnce(() => new Promise((resolve) => (endWait = resolve)));
    const first = useApp.getState().pickModel("local:/m/b.gguf");
    await answer("Restart when it ends");
    await flush();
    const second = useApp.getState().pickModel("local:/m/c.gguf");
    await answer("Stop and restart");
    await second;
    endWait(false);
    await first;
    expect(ipcMock.setChatModel).toHaveBeenLastCalledWith(3, "local:/m/c.gguf");
    expect(useApp.getState().detail?.chat.modelRef).toBe("local:/m/c.gguf");
  });

  it("puts the chat back on its model when a switch is cancelled on a path that did not ask to wait", async () => {
    let chatModel: string | null = "local:/m/a.gguf";
    ipcMock.setChatModel.mockImplementation(async (_id, ref) => {
      chatModel = ref;
    });
    ipcMock.getChat.mockImplementation(async (id: number) => ({ chat: { id, modelRef: chatModel }, messages: [] }));
    useApp.setState({ runtime: runtime("ready", "/m/a.gguf") });

    // No question: nothing used the model at the check (or the count could not be read), so
    // the backend is told to wait anyway; a reply that started meanwhile makes it wait on its
    // own, with the waiting line and its Cancel. "Stop and restart" can still be replaced
    // while it stops what uses the model. Each ends with load_replaced (false).
    const paths: { name: string; busy: () => void; choice: string | null }[] = [
      { name: "nothing used the model", busy: () => ipcMock.modelBusy.mockResolvedValue(IDLE), choice: null },
      {
        name: "the count could not be read",
        busy: () => ipcMock.modelBusy.mockRejectedValue(new Error("no count")),
        choice: null,
      },
      { name: "Stop and restart", busy: () => ipcMock.modelBusy.mockResolvedValue(REPLY), choice: "Stop and restart" },
    ];
    for (const path of paths) {
      chatModel = "local:/m/a.gguf";
      await useApp.getState().openChat(3);
      ipcMock.setChatModel.mockClear();
      ipcMock.loadLocalModel.mockClear();
      path.busy();
      ipcMock.loadLocalModel.mockResolvedValueOnce(false);
      const pick = useApp.getState().pickModel("local:/m/b.gguf");
      if (path.choice !== null) await answer(path.choice);
      await pick;
      expect(change.useQuestion.getState().question, path.name).toBeNull();
      expect(ipcMock.loadLocalModel, path.name).toHaveBeenCalledWith("/m/b.gguf", null, path.choice === null);
      expect(ipcMock.setChatModel, path.name).toHaveBeenNthCalledWith(1, 3, "local:/m/b.gguf");
      expect(ipcMock.setChatModel, path.name).toHaveBeenLastCalledWith(3, "local:/m/a.gguf");
      expect(useApp.getState().detail?.chat.modelRef, path.name).toBe("local:/m/a.gguf");
      expect(useApp.getState().error, path.name).toBeNull();
    }
  });

  it("keeps the chat on a model picked again while the first pick of it waited", async () => {
    let chatModel: string | null = "local:/m/a.gguf";
    ipcMock.setChatModel.mockImplementation(async (_id, ref) => {
      chatModel = ref;
    });
    ipcMock.getChat.mockImplementation(async (id: number) => ({ chat: { id, modelRef: chatModel }, messages: [] }));
    await useApp.getState().openChat(3);
    useApp.setState({ runtime: runtime("ready", "/m/a.gguf") });
    ipcMock.modelBusy.mockResolvedValue(REPLY);

    // The second pick of the same model replaces the first one's wait: the first ends with
    // load_replaced, but the chat must stay on the model that the second pick loads.
    let endWait: (done: boolean) => void = () => undefined;
    ipcMock.loadLocalModel.mockImplementationOnce(() => new Promise((resolve) => (endWait = resolve)));
    const first = useApp.getState().pickModel("local:/m/b.gguf");
    await answer("Restart when it ends");
    await flush();
    const second = useApp.getState().pickModel("local:/m/b.gguf");
    await answer("Stop and restart");
    await second;
    endWait(false);
    await first;
    expect(ipcMock.setChatModel).toHaveBeenLastCalledWith(3, "local:/m/b.gguf");
    expect(useApp.getState().detail?.chat.modelRef).toBe("local:/m/b.gguf");
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

  // Two chats, each with its saved model in `models` as the backend keeps it; chat 3 is open,
  // model a is loaded and a reply is being written.
  async function twoChats(): Promise<Record<number, string | null>> {
    const models: Record<number, string | null> = { 3: "local:/m/a.gguf", 4: "provider:1:fake" };
    ipcMock.setChatModel.mockImplementation(async (id, ref) => {
      models[id] = ref;
    });
    ipcMock.getChat.mockImplementation(async (id: number) => ({ chat: { id, modelRef: models[id] ?? null }, messages: [] }));
    await useApp.getState().openChat(3);
    useApp.setState({ runtime: runtime("ready", "/m/a.gguf") });
    ipcMock.modelBusy.mockResolvedValue(REPLY);
    return models;
  }

  /** The next load waits until the returned function ends it (false: cancelled or replaced). */
  function waitingLoad(): { end: (done: boolean) => void } {
    const handle: { end: (done: boolean) => void } = { end: () => undefined };
    ipcMock.loadLocalModel.mockImplementationOnce(() => new Promise((resolve) => (handle.end = resolve)));
    return handle;
  }

  it("two chats: a quick pick in the other chat does not stop the waiting chat going back on Cancel", async () => {
    const models = await twoChats();
    const load = waitingLoad();
    const waiting = useApp.getState().pickModel("local:/m/b.gguf");
    await answer("Restart when it ends");
    await flush();
    expect(models[3]).toBe("local:/m/b.gguf");

    // In the other chat, a pick that loads nothing (an endpoint); then back to the first chat.
    await useApp.getState().openChat(4);
    await useApp.getState().pickModel("provider:2:other");
    await useApp.getState().openChat(3);

    // Cancel on the waiting line: the backend ends the wait with load_replaced (false).
    load.end(false);
    await waiting;
    expect(models[3]).toBe("local:/m/a.gguf");
    expect(models[4]).toBe("provider:2:other");
    expect(useApp.getState().detail?.chat.modelRef).toBe("local:/m/a.gguf");
    expect(useApp.getState().error).toBeNull();
  });

  it("puts the waiting chat back when Cancel is pressed while another chat is open, and leaves the open chat alone", async () => {
    const models = await twoChats();
    let load = waitingLoad();
    let waiting = useApp.getState().pickModel("local:/m/b.gguf");
    await answer("Restart when it ends");
    await flush();
    await useApp.getState().openChat(4);

    load.end(false);
    await waiting;
    expect(models[3]).toBe("local:/m/a.gguf");
    expect(useApp.getState().activeChatId).toBe(4);
    expect(useApp.getState().detail?.chat).toMatchObject({ id: 4, modelRef: "provider:1:fake" });
    expect(useApp.getState().error).toBeNull();

    // A model the chat was given another way since is kept.
    await useApp.getState().openChat(3);
    load = waitingLoad();
    waiting = useApp.getState().pickModel("local:/m/b.gguf");
    await answer("Restart when it ends");
    await flush();
    await useApp.getState().openChat(4);
    models[3] = "provider:1:fake";
    load.end(false);
    await waiting;
    expect(models[3]).toBe("provider:1:fake");
  });

  it("re-picking the loaded model while a switch waits cancels the wait, from the same chat or another", async () => {
    const models = await twoChats();
    let load = waitingLoad();
    // The backend ends the waiting load when its wait is cancelled.
    ipcMock.cancelPendingReload.mockImplementation(async () => void load.end(false));

    let waiting = useApp.getState().pickModel("local:/m/b.gguf");
    await answer("Restart when it ends");
    await flush();
    await useApp.getState().pickModel("local:/m/a.gguf");
    expect(ipcMock.cancelPendingReload).toHaveBeenCalledTimes(1);
    await waiting;
    expect(ipcMock.loadLocalModel).toHaveBeenCalledTimes(1);
    expect(models[3]).toBe("local:/m/a.gguf");
    expect(useApp.getState().detail?.chat.modelRef).toBe("local:/m/a.gguf");

    // From the other chat: the waiting chat goes back to the loaded model too.
    load = waitingLoad();
    waiting = useApp.getState().pickModel("local:/m/b.gguf");
    await answer("Restart when it ends");
    await flush();
    await useApp.getState().openChat(4);
    await useApp.getState().pickModel("local:/m/a.gguf");
    expect(ipcMock.cancelPendingReload).toHaveBeenCalledTimes(2);
    await waiting;
    expect(models).toEqual({ 3: "local:/m/a.gguf", 4: "local:/m/a.gguf" });
    expect(ipcMock.modelBusy).toHaveBeenCalledTimes(2);
    expect(useApp.getState().error).toBeNull();
    ipcMock.cancelPendingReload.mockReset();
  });

  it("re-picking the loaded model leaves a waiting Apply and restart alone", async () => {
    await twoChats();
    let endReload: (done: boolean) => void = () => undefined;
    ipcMock.reloadLocalModel.mockImplementationOnce(() => new Promise((resolve) => (endReload = resolve)));
    const reload = change.reloadModel(useApp.getState().runtime, false);
    await answer("Restart when it ends");
    await flush();
    await useApp.getState().pickModel("local:/m/a.gguf");
    expect(ipcMock.cancelPendingReload).not.toHaveBeenCalled();
    endReload(true);
    await expect(reload).resolves.toBe(true);
  });

  it("keeps a chat on the model it waited for when another chat loads that same model", async () => {
    const models = await twoChats();
    const load = waitingLoad();
    const waiting = useApp.getState().pickModel("local:/m/b.gguf");
    await answer("Restart when it ends");
    await flush();
    await useApp.getState().openChat(4);
    // That pick asks too (the reply still runs) and is told to stop it; its load replaces the wait.
    const second = useApp.getState().pickModel("local:/m/b.gguf");
    await answer("Stop and restart");
    await second;
    load.end(false);
    await waiting;
    expect(models).toEqual({ 3: "local:/m/b.gguf", 4: "local:/m/b.gguf" });
    expect(ipcMock.loadLocalModel).toHaveBeenLastCalledWith("/m/b.gguf", null, false);
  });

  describe("with a backend that holds one waiting load", () => {
    afterEach(() => {
      ipcMock.loadLocalModel.mockReset();
      ipcMock.cancelPendingReload.mockReset();
    });

    /** Like the backend's `ChangeGate`: at most one load waits; a newer load or Cancel ends it
     *  with load_replaced (false), "Stop and restart" loads at once, and `replyEnds` lets the
     *  waiting one load. `loaded` lists the models that really loaded. */
    function oneWaitingLoad(): { loaded: string[]; replyEnds: () => void } {
      let waiting: ((done: boolean) => void) | null = null;
      const loaded: string[] = [];
      const end = (done: boolean) => {
        const w = waiting;
        waiting = null;
        w?.(done);
      };
      ipcMock.loadLocalModel.mockImplementation((path, _settings, whenIdle) => {
        end(false);
        if (whenIdle === false) {
          loaded.push(path);
          return Promise.resolve(true);
        }
        return new Promise<boolean>((resolve) => {
          waiting = (done) => {
            if (done) loaded.push(path);
            resolve(done);
          };
        });
      });
      ipcMock.cancelPendingReload.mockImplementation(async () => void end(false));
      return { loaded, replyEnds: () => end(true) };
    }

    it("picking an endpoint in the chat whose switch waits cancels the wait; in another chat it does not", async () => {
      const models = await twoChats();
      const backend = oneWaitingLoad();
      const waiting = useApp.getState().pickModel("local:/m/b.gguf");
      await answer("Restart when it ends");
      await flush();
      await useApp.getState().openChat(4);
      await useApp.getState().pickModel("provider:2:other");
      expect(ipcMock.cancelPendingReload).not.toHaveBeenCalled();

      await useApp.getState().openChat(3);
      await useApp.getState().pickModel("provider:2:other");
      expect(ipcMock.cancelPendingReload).toHaveBeenCalledTimes(1);
      await waiting;
      backend.replyEnds();
      await flush();
      expect(backend.loaded).toEqual([]);
      expect(models).toEqual({ 3: "provider:2:other", 4: "provider:2:other" });
      expect(useApp.getState().detail?.chat.modelRef).toBe("provider:2:other");
      expect(change.useWaiting.getState().kind).toBeNull();
      expect(useApp.getState().error).toBeNull();
    });

    it("two chats waiting for the same model both go back on Cancel, whichever chat is open", async () => {
      const models = await twoChats();
      const backend = oneWaitingLoad();
      const first = useApp.getState().pickModel("local:/m/b.gguf");
      await answer("Restart when it ends");
      await flush();
      await useApp.getState().openChat(4);
      const second = useApp.getState().pickModel("local:/m/b.gguf");
      await answer("Restart when it ends");
      // The newer wait ended the first one (load_replaced); chat 3 now waits on it.
      await first;
      expect(models).toEqual({ 3: "local:/m/b.gguf", 4: "local:/m/b.gguf" });

      // Cancel on the waiting line, with chat 4 open.
      await ipcMock.cancelPendingReload();
      await second;
      expect(backend.loaded).toEqual([]);
      expect(models).toEqual({ 3: "local:/m/a.gguf", 4: "provider:1:fake" });
      expect(useApp.getState().detail?.chat).toMatchObject({ id: 4, modelRef: "provider:1:fake" });
      expect(useApp.getState().error).toBeNull();
    });

    it("two chats waiting for the same model both keep it when it loads, and a later Cancel goes back to it", async () => {
      const models = await twoChats();
      const backend = oneWaitingLoad();
      const first = useApp.getState().pickModel("local:/m/b.gguf");
      await answer("Restart when it ends");
      await flush();
      await useApp.getState().openChat(4);
      const second = useApp.getState().pickModel("local:/m/b.gguf");
      await answer("Restart when it ends");
      await first;
      backend.replyEnds();
      await second;
      expect(backend.loaded).toEqual(["/m/b.gguf"]);
      expect(models).toEqual({ 3: "local:/m/b.gguf", 4: "local:/m/b.gguf" });

      // Chat 3's switch is over: a new one that is cancelled goes back to b, not to a.
      useApp.setState({ runtime: runtime("ready", "/m/b.gguf") });
      await useApp.getState().openChat(3);
      const third = useApp.getState().pickModel("local:/m/c.gguf");
      await answer("Restart when it ends");
      await flush();
      await ipcMock.cancelPendingReload();
      await third;
      expect(models[3]).toBe("local:/m/b.gguf");
    });

    it("a chat that waits on another chat's load of the same model and then picks an endpoint keeps it", async () => {
      const models = await twoChats();
      oneWaitingLoad();
      const first = useApp.getState().pickModel("local:/m/b.gguf");
      await answer("Restart when it ends");
      await flush();
      await useApp.getState().openChat(4);
      const second = useApp.getState().pickModel("local:/m/b.gguf");
      await answer("Restart when it ends");
      await first;
      // Chat 3's own wait was taken over by chat 4's: its endpoint pick leaves chat 4's wait.
      await useApp.getState().openChat(3);
      await useApp.getState().pickModel("provider:2:other");
      expect(ipcMock.cancelPendingReload).not.toHaveBeenCalled();
      await ipcMock.cancelPendingReload();
      await second;
      expect(models).toEqual({ 3: "provider:2:other", 4: "provider:1:fake" });
    });

    it("one chat: a wait replaced by a newer wait in that chat goes back to the model from before both on Cancel", async () => {
      const models = await twoChats();
      const backend = oneWaitingLoad();
      for (const next of ["local:/m/c.gguf", "local:/m/b.gguf"]) {
        const first = useApp.getState().pickModel("local:/m/b.gguf");
        await answer("Restart when it ends");
        await flush();
        const second = useApp.getState().pickModel(next);
        await answer("Restart when it ends");
        await first;
        expect(models[3], next).toBe(next);
        await ipcMock.cancelPendingReload();
        await second;
        expect(models[3], next).toBe("local:/m/a.gguf");
        expect(useApp.getState().detail?.chat.modelRef, next).toBe("local:/m/a.gguf");
      }
      expect(backend.loaded).toEqual([]);
      expect(useApp.getState().error).toBeNull();
    });

    it("re-picking the loaded model puts a waiting chat back on the model it had, an endpoint too", async () => {
      const models = await twoChats();
      oneWaitingLoad();
      await useApp.getState().openChat(4);
      const waiting = useApp.getState().pickModel("local:/m/b.gguf");
      await answer("Restart when it ends");
      await flush();
      await useApp.getState().openChat(3);
      await useApp.getState().pickModel("local:/m/a.gguf");
      await waiting;
      expect(ipcMock.cancelPendingReload).toHaveBeenCalledTimes(1);
      expect(models).toEqual({ 3: "local:/m/a.gguf", 4: "provider:1:fake" });
    });
  });
});
