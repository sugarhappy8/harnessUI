import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { create } from "zustand";
import * as ipc from "../lib/ipc";
import { askBeforeChange, backendWhenIdle, isLoadedNow, runChange, useWaiting } from "./modelChange";
import type {
  ApprovalRequest,
  ChatDetail,
  TurnStreamEvent,
  ChatSummary,
  ErrorInfo,
  DocumentInput,
  ImageInput,
  MentionInput,
  ModelOption,
  RuntimeStatus,
} from "../lib/types";

/** A tool call while its turn runs (before its messages are stored). */
export interface LiveTool {
  callId: string;
  name: string;
  arguments: Record<string, unknown>;
  recovered: boolean;
  status: string;
  text: string;
  decision: string | null;
  durationMs: number | null;
  progress: string;
}

/** The agent turn being generated. */
export interface Generation {
  chatId: number;
  /** Text and thinking streamed since the last stored message. */
  text: string;
  reasoning: string;
  tools: LiveTool[];
  notes: string[];
  step: number;
  tokensPerSecond: number | null;
  promptTokens: number | null;
  completionTokens: number | null;
  contextUsed: number | null;
  nCtx: number | null;
}

function newGeneration(chatId: number): Generation {
  return {
    chatId,
    text: "",
    reasoning: "",
    tools: [],
    notes: [],
    step: 0,
    tokensPerSecond: null,
    promptTokens: null,
    completionTokens: null,
    contextUsed: null,
    nCtx: null,
  };
}

const PROGRESS_LIMIT = 8000;

interface AppState {
  chats: ChatSummary[];
  search: string;
  activeChatId: number | null;
  detail: ChatDetail | null;
  runtime: RuntimeStatus | null;
  modelOptions: ModelOption[];
  generation: Generation | null;
  /** The notes of the last finished turn, shown until the next message is sent: a note
   * that arrives as a turn ends would otherwise vanish before it can be read. */
  turnNotes: { chatId: number; notes: string[] } | null;
  error: ErrorInfo | null;
  /** The panel beside the chat (artifacts, task, sources, context). */
  workbench: { open: boolean; tab: WorkbenchTab };
  openWorkbench: (tab?: WorkbenchTab) => void;
  closeWorkbench: () => void;
  /** Approval cards waiting for an answer (any chat). */
  approvals: ApprovalRequest[];

  setSearch: (q: string) => void;
  refreshChats: () => Promise<void>;
  openChat: (chatId: number | null) => Promise<void>;
  newChat: () => Promise<void>;
  renameChat: (chatId: number, title: string) => Promise<void>;
  deleteChat: (chatId: number) => Promise<void>;
  send: (
    text: string,
    parentId?: number | null,
    images?: ImageInput[],
    documents?: DocumentInput[],
    mentions?: MentionInput[],
  ) => Promise<void>;
  regenerate: (assistantMessageId: number) => Promise<void>;
  stop: () => Promise<void>;
  switchBranch: (messageId: number) => Promise<void>;
  setRuntime: (s: RuntimeStatus) => void;
  refreshModelOptions: () => Promise<void>;
  setChatModel: (modelRef: string | null) => Promise<void>;
  /**
   * Makes a model the open chat's (or a new chat's) and loads it when it is local, with the
   * model's own saved settings. Asks first when the loaded model is busy (0.0.6 D3); a
   * Cancel changes nothing. `onChatSet` runs once the chat has the model, before the load.
   */
  pickModel: (modelRef: string, opts?: { onChatSet?: () => void }) => Promise<void>;
  /** Asks for a .gguf file and picks it. */
  openModelFile: () => Promise<void>;
  setSystemPrompt: (prompt: string | null) => Promise<void>;
  showError: (e: ErrorInfo | null) => void;
  addApproval: (r: ApprovalRequest) => void;
  removeApproval: (id: string) => void;
  decideApproval: (id: string, decision: "allowOnce" | "allowForChat" | "deny", note: string | null) => Promise<void>;
}

/** [0.0.5 memory] What the chat says when a reply had to make room, by the stage of the
 *  agent loop's `Compacted` event (1: earlier tool results, 2: a summary of earlier
 *  messages, written or rewritten shorter, 3: this reply's own older steps). */
export function compactionNote(stage: number): string {
  switch (stage) {
    case 1:
      return "Older tool results were shortened to make room.";
    case 2:
      return "Earlier messages were summarised to make room.";
    case 3:
      return "Older steps of this reply were shortened to make room.";
    default:
      return "Earlier parts of this chat were shortened to make room.";
  }
}

/** [0.0.5 memory] The loop's notes while it writes a summary and when it could not
 *  (`agent::turn::SUMMARISING_NOTE`, `SUMMARY_FAILED_NOTE`). */
const SUMMARISING_NOTE = "Summarising earlier messages to make room.";
const SUMMARY_FAILED_NOTE =
  "Could not summarise earlier messages. The reply goes on, but this chat may run out of room soon.";

/** [0.0.5 memory] `notes` without the newest "Summarising…" note: what happened takes its place. */
function withoutSummarising(notes: string[]): string[] {
  const at = notes.lastIndexOf(SUMMARISING_NOTE);
  return at < 0 ? notes : [...notes.slice(0, at), ...notes.slice(at + 1)];
}

/** Turns anything thrown by `invoke` into the four-part error the UI shows. */
export function toErrorInfo(err: unknown): ErrorInfo {
  if (err && typeof err === "object" && "message" in err && "code" in err) {
    return err as ErrorInfo;
  }
  const text = typeof err === "string" ? err : err instanceof Error ? err.message : String(err);
  return {
    code: "unexpected",
    message: "Something went wrong.",
    nextStep: "Try again. If it keeps happening, copy the details into a bug report.",
    details: text,
  };
}

function lastMessageId(detail: ChatDetail | null): number | null {
  const last = detail?.messages[detail.messages.length - 1];
  return last ? last.id : null;
}

/** The tabs of the panel beside the chat. */
export type WorkbenchTab = "artifacts" | "task" | "sources" | "context";

/** [0.0.6 item 8] Counts reads of the chat's model list, so an older answer never replaces a newer one. */
let modelOptionsRead = 0;

export const useApp = create<AppState>((set, get) => {
  /** Re-reads a chat after a stream event. Never rejects: the chat may have been deleted
   * while its reply was being written, and the turn still has to end in the UI. */
  function reload(chatId: number) {
    return ipc
      .getChat(chatId)
      .then((detail) => {
        if (get().activeChatId === chatId) set({ detail });
      })
      .catch(() => undefined);
  }

  function updateGen(chatId: number, f: (g: Generation) => Generation) {
    const g = get().generation;
    if (g && g.chatId === chatId) set({ generation: f(g) });
  }

  /** Applies streamed turn events. */
  function onStreamEvent(chatId: number, e: TurnStreamEvent) {
    switch (e.type) {
      case "started":
        void reload(chatId);
        return;
      case "step":
        updateGen(chatId, (g) => ({ ...g, step: e.step }));
        return;
      case "content":
        updateGen(chatId, (g) => ({ ...g, text: g.text + e.text }));
        return;
      case "reasoning":
        updateGen(chatId, (g) => ({ ...g, reasoning: g.reasoning + e.text }));
        return;
      case "discarded":
        updateGen(chatId, (g) => ({ ...g, text: "", reasoning: "" }));
        return;
      case "messageSaved":
        // The stored message replaces the streamed text; tool cards stay until their
        // results are stored.
        void reload(chatId).then(() => {
          updateGen(chatId, (g) => {
            const stored = new Set(
              (get().detail?.messages ?? []).flatMap((m) => (m.toolResult ? [m.toolResult.callId] : [])),
            );
            return { ...g, text: "", reasoning: "", tools: g.tools.filter((t) => !stored.has(t.callId)) };
          });
        });
        updateGen(chatId, (g) => ({ ...g, text: "", reasoning: "" }));
        return;
      case "toolStarted":
        updateGen(chatId, (g) => ({
          ...g,
          tools: [
            ...g.tools,
            {
              callId: e.callId,
              name: e.name,
              arguments: e.arguments,
              recovered: e.recovered,
              status: "running",
              text: "",
              decision: null,
              durationMs: null,
              progress: "",
            },
          ],
        }));
        return;
      case "toolProgress":
        updateGen(chatId, (g) => {
          const tools = [...g.tools];
          const i = e.callId
            ? tools.findIndex((t) => t.callId === e.callId)
            : tools.map((t) => t.status).lastIndexOf("running");
          const tool = tools[i];
          if (!tool) return g;
          tools[i] = { ...tool, progress: (tool.progress + e.text).slice(-PROGRESS_LIMIT) };
          return { ...g, tools };
        });
        return;
      case "toolFinished":
        updateGen(chatId, (g) => ({
          ...g,
          tools: g.tools.map((t) =>
            t.callId === e.callId
              ? { ...t, status: e.status, text: e.text, decision: e.decision, durationMs: e.durationMs }
              : t,
          ),
        }));
        return;
      case "usage":
        updateGen(chatId, (g) => ({
          ...g,
          promptTokens: e.promptTokens,
          completionTokens: e.completionTokens,
          tokensPerSecond: e.tokensPerSecond ?? g.tokensPerSecond,
          contextUsed: e.contextUsed ?? g.contextUsed,
          nCtx: e.nCtx ?? g.nCtx,
        }));
        return;
      case "note":
        updateGen(chatId, (g) => ({
          ...g,
          // [0.0.5 memory] A summary that failed replaces the note shown while it was written.
          notes: [...(e.text === SUMMARY_FAILED_NOTE ? withoutSummarising(g.notes) : g.notes), e.text],
        }));
        return;
      case "compacted": {
        // [0.0.5 memory] What the loop did, once per kind (a stage can run several times); a
        // summary written replaces the note shown while it was written.
        const note = compactionNote(e.stage);
        updateGen(chatId, (g) => {
          const notes = e.stage === 2 ? withoutSummarising(g.notes) : g.notes;
          return { ...g, notes: notes.includes(note) ? notes : [...notes, note] };
        });
        return;
      }
      case "finished":
        if (e.message && e.status !== "error") {
          set({
            error: {
              code: e.status,
              message: e.message,
              nextStep:
                e.status === "stepCap"
                  ? "Choose Continue to let it carry on. The limit can be raised in Settings → General."
                  : "Ask the model to continue, or rephrase the request.",
              details: "",
            },
          });
        }
        {
          const g = get().generation;
          const notes = g && g.chatId === chatId ? g.notes : [];
          void reload(chatId).then(() => {
            if (get().generation?.chatId === chatId) {
              set({ generation: null, turnNotes: notes.length > 0 ? { chatId, notes } : null });
            }
          });
        }
        void get().refreshChats();
        return;
    }
  }

  /** Counts the picks that changed a chat's model, so a switch can tell a newer pick replaced it. */
  let picks = 0;
  /** The newest pick of each chat (by chat id): only a newer pick in its own chat replaces a
   *  switch's change of that chat; picks in other chats do not. */
  const newestPick = new Map<number, number>();
  /** The newest pick that loads a model, of any chat. */
  let newestLoad: { pick: number; path: string } | null = null;
  const notePick = (chatId: number | null): number => {
    picks += 1;
    if (chatId !== null) newestPick.set(chatId, picks);
    return picks;
  };
  /** Gives chat `chatId` back the model it had (`before`) after its switch to `modelRef` was
   *  cancelled, whether the chat is open or not, unless it was given another model since. */
  const restoreModel = async (chatId: number, modelRef: string, before: string) => {
    const saved = await ipc.getChat(chatId);
    if (saved.chat.modelRef !== modelRef) return;
    await ipc.setChatModel(chatId, before);
    if (get().activeChatId === chatId) await get().openChat(chatId);
  };

  return {
    chats: [],
    search: "",
    activeChatId: null,
    detail: null,
    runtime: null,
    modelOptions: [],
    generation: null,
    turnNotes: null,
    error: null,
    workbench: { open: false, tab: "artifacts" },
    openWorkbench: (tab) => set({ workbench: { open: true, tab: tab ?? get().workbench.tab } }),
    closeWorkbench: () => set({ workbench: { ...get().workbench, open: false } }),
    approvals: [],

    setSearch: (q) => {
      set({ search: q });
      void get().refreshChats();
    },

    refreshChats: async () => {
      const q = get().search.trim();
      const chats = await ipc.listChats(q.length > 0 ? q : null);
      set({ chats });
    },

    openChat: async (chatId) => {
      if (chatId === null) {
        set({ activeChatId: null, detail: null });
        return;
      }
      const detail = await ipc.getChat(chatId);
      set({ activeChatId: chatId, detail });
    },

    newChat: async () => {
      const options = get().modelOptions;
      // The open chat's model, else the one that is loaded, else the most recent.
      const rt = get().runtime;
      const loaded =
        rt && rt.modelPath && (rt.state === "ready" || rt.state === "starting") ? `local:${rt.modelPath}` : null;
      const lastUsed = get().detail?.chat.modelRef ?? loaded ?? options[0]?.ref ?? null;
      const chat = await ipc.createChat(lastUsed);
      await get().refreshChats();
      await get().openChat(chat.id);
    },

    renameChat: async (chatId, title) => {
      await ipc.renameChat(chatId, title);
      await get().refreshChats();
      if (get().activeChatId === chatId) await get().openChat(chatId);
    },

    deleteChat: async (chatId) => {
      await ipc.deleteChat(chatId);
      newestPick.delete(chatId);
      if (get().activeChatId === chatId) set({ activeChatId: null, detail: null });
      await get().refreshChats();
    },

    send: async (text, parentId, images, documents, mentions) => {
      let chatId = get().activeChatId;
      if (chatId === null) {
        await get().newChat();
        chatId = get().activeChatId;
      }
      if (chatId === null) return;
      const parent = parentId === undefined ? lastMessageId(get().detail) : parentId;
      set({ generation: newGeneration(chatId), turnNotes: null, error: null });
      const id = chatId;
      try {
        await ipc.sendMessage({ chatId: id, text, parentId: parent, images, documents, mentions }, (e) =>
          onStreamEvent(id, e),
        );
      } catch (err) {
        set({ generation: null, error: toErrorInfo(err) });
      }
    },

    regenerate: async (assistantMessageId) => {
      const chatId = get().activeChatId;
      if (chatId === null) return;
      set({ generation: newGeneration(chatId), turnNotes: null, error: null });
      try {
        await ipc.regenerate(chatId, assistantMessageId, (e) => onStreamEvent(chatId, e));
      } catch (err) {
        set({ generation: null, error: toErrorInfo(err) });
      }
    },

    stop: async () => {
      await ipc.stopGeneration();
    },

    switchBranch: async (messageId) => {
      const chatId = get().activeChatId;
      if (chatId === null) return;
      await ipc.switchBranch(chatId, messageId);
      await get().openChat(chatId);
    },

    setRuntime: (s) => set({ runtime: s }),

    refreshModelOptions: async () => {
      // [0.0.6 item 8] The open chat's own model file is listed the way the chat names it (a
      // link, a mapped drive). Of two reads that cross, the newer one is kept.
      const read = ++modelOptionsRead;
      const modelOptions = await ipc.listModelOptions(get().detail?.chat.modelRef ?? null);
      if (read === modelOptionsRead) set({ modelOptions });
    },

    setChatModel: async (modelRef) => {
      const chatId = get().activeChatId;
      if (chatId === null) {
        // No chat is open: the model needs one to belong to, or picking it shows nothing.
        if (modelRef === null) return;
        const chat = await ipc.createChat(modelRef);
        await get().refreshChats();
        await get().openChat(chat.id);
        return;
      }
      await ipc.setChatModel(chatId, modelRef);
      await get().openChat(chatId);
    },

    pickModel: async (modelRef, opts) => {
      try {
        const path = modelRef.startsWith("local:") ? modelRef.slice(6) : null;
        const runtime = get().runtime;
        // An endpoint, or the local model already loaded (or loading): nothing to load. Its
        // changed settings wait for Apply and restart (D3).
        if (path === null || isLoadedNow(runtime, path)) {
          notePick(get().activeChatId);
          // The loaded model picked again while a switch to another one waits: the wait is
          // cancelled, so the loaded model stays (and the waiting chat goes back to it).
          if (path !== null && useWaiting.getState().kind === "switch") await ipc.cancelPendingReload();
          await get().setChatModel(modelRef);
          opts?.onChatSet?.();
          return;
        }
        const answer = await askBeforeChange(runtime);
        if (answer === null) return;
        const chatId = get().activeChatId;
        const open = get().detail;
        const before = chatId !== null && open?.chat.id === chatId ? open.chat.modelRef : null;
        const pick = notePick(chatId);
        await get().setChatModel(modelRef);
        opts?.onChatSet?.();
        newestLoad = { pick, path };
        const done = await runChange("switch", answer.whenIdle, () =>
          ipc.loadLocalModel(path, null, backendWhenIdle(answer)),
        );
        // A switch that was cancelled changes nothing, whether the user asked it to wait or
        // the backend waited on its own (no question was asked), and whether its chat is open
        // or not: the chat goes back to the model it had, unless a newer pick in that chat
        // has changed it since (even to the same model, whose newer load is the one that
        // replaced this one), or a newer load of the same model, from another chat, replaced
        // this one and brings the model this chat picked.
        const sameLoad = newestLoad !== null && newestLoad.pick !== pick && newestLoad.path === path;
        if (!done && chatId !== null && before !== null && newestPick.get(chatId) === pick && !sameLoad) {
          await restoreModel(chatId, modelRef, before);
        }
        await get().refreshModelOptions();
      } catch (err) {
        set({ error: toErrorInfo(err) });
      }
    },

    openModelFile: async () => {
      try {
        const picked = await openFileDialog({
          multiple: false,
          directory: false,
          filters: [{ name: "GGUF model", extensions: ["gguf"] }],
        });
        if (typeof picked === "string") await get().pickModel(`local:${picked}`);
      } catch (err) {
        set({ error: toErrorInfo(err) });
      }
    },

    setSystemPrompt: async (prompt) => {
      const chatId = get().activeChatId;
      if (chatId === null) return;
      await ipc.setSystemPrompt(chatId, prompt);
      await get().openChat(chatId);
    },

    showError: (e) => set({ error: e }),

    addApproval: (r) => set({ approvals: [...get().approvals.filter((a) => a.id !== r.id), r] }),
    removeApproval: (id) => set({ approvals: get().approvals.filter((a) => a.id !== id) }),
    decideApproval: async (id, decision, note) => {
      try {
        await ipc.resolveApproval(id, decision, note);
      } catch (err) {
        set({ error: toErrorInfo(err) });
      }
      get().removeApproval(id);
    },
  };
});

/** [0.0.6 item 8] The `download-progress` listener: a finished download (every file of it
 *  done) is a new model "On this computer", so the chat's model list is read again then, not
 *  only when its menu opens. Returns the function that stops listening; outside the app it
 *  listens to nothing. */
export function refreshModelsAfterDownloads(): Promise<() => void> {
  if (!ipc.isTauri()) return Promise.resolve(() => undefined);
  return ipc.onDownloadProgress((g) => {
    if (g.status === "done") useApp.getState().refreshModelOptions().catch(() => undefined);
  });
}
