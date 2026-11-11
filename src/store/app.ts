import { create } from "zustand";
import * as ipc from "../lib/ipc";
import type {
  ApprovalRequest,
  ChatDetail,
  TurnStreamEvent,
  ChatSummary,
  ErrorInfo,
  DocumentInput,
  ImageInput,
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
  /** Approval cards waiting for an answer (any chat). */
  approvals: ApprovalRequest[];

  setSearch: (q: string) => void;
  refreshChats: () => Promise<void>;
  openChat: (chatId: number | null) => Promise<void>;
  newChat: () => Promise<void>;
  renameChat: (chatId: number, title: string) => Promise<void>;
  deleteChat: (chatId: number) => Promise<void>;
  send: (text: string, parentId?: number | null, images?: ImageInput[], documents?: DocumentInput[]) => Promise<void>;
  regenerate: (assistantMessageId: number) => Promise<void>;
  stop: () => Promise<void>;
  switchBranch: (messageId: number) => Promise<void>;
  setRuntime: (s: RuntimeStatus) => void;
  refreshModelOptions: () => Promise<void>;
  setChatModel: (modelRef: string | null) => Promise<void>;
  setSystemPrompt: (prompt: string | null) => Promise<void>;
  showError: (e: ErrorInfo | null) => void;
  addApproval: (r: ApprovalRequest) => void;
  removeApproval: (id: string) => void;
  decideApproval: (id: string, decision: "allowOnce" | "allowForChat" | "deny", note: string | null) => Promise<void>;
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
        updateGen(chatId, (g) => ({ ...g, notes: [...g.notes, e.text] }));
        return;
      case "compacted":
        updateGen(chatId, (g) => ({
          ...g,
          notes: [...g.notes, "Earlier messages were summarised to make room in the context."],
        }));
        return;
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
      const lastUsed = get().detail?.chat.modelRef ?? options[0]?.ref ?? null;
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
      if (get().activeChatId === chatId) set({ activeChatId: null, detail: null });
      await get().refreshChats();
    },

    send: async (text, parentId, images, documents) => {
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
        await ipc.sendMessage({ chatId: id, text, parentId: parent, images, documents }, (e) =>
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
      const modelOptions = await ipc.listModelOptions();
      set({ modelOptions });
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
