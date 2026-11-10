import { create } from "zustand";
import * as ipc from "../lib/ipc";
import type {
  ChatDetail,
  ChatStreamEvent,
  ChatSummary,
  ErrorInfo,
  ImageInput,
  MessageDto,
  ModelOption,
  RuntimeStatus,
} from "../lib/types";

/** Live numbers for the message being generated. */
export interface Generation {
  chatId: number;
  assistantMessageId: number | null;
  tokensPerSecond: number | null;
  promptTokens: number | null;
  completionTokens: number | null;
}

interface AppState {
  chats: ChatSummary[];
  search: string;
  activeChatId: number | null;
  detail: ChatDetail | null;
  runtime: RuntimeStatus | null;
  modelOptions: ModelOption[];
  generation: Generation | null;
  error: ErrorInfo | null;

  setSearch: (q: string) => void;
  refreshChats: () => Promise<void>;
  openChat: (chatId: number | null) => Promise<void>;
  newChat: () => Promise<void>;
  renameChat: (chatId: number, title: string) => Promise<void>;
  deleteChat: (chatId: number) => Promise<void>;
  send: (text: string, parentId?: number | null, images?: ImageInput[]) => Promise<void>;
  regenerate: (assistantMessageId: number) => Promise<void>;
  stop: () => Promise<void>;
  switchBranch: (messageId: number) => Promise<void>;
  setRuntime: (s: RuntimeStatus) => void;
  refreshModelOptions: () => Promise<void>;
  setChatModel: (modelRef: string | null) => Promise<void>;
  setSystemPrompt: (prompt: string | null) => Promise<void>;
  showError: (e: ErrorInfo | null) => void;
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
  /** Applies streamed events to the message being generated. */
  function onStreamEvent(chatId: number, e: ChatStreamEvent) {
    const state = get();
    if (state.activeChatId !== chatId) {
      if (e.type === "finished" || e.type === "error") {
        set({ generation: null });
        void get().refreshChats();
      }
      return;
    }
    switch (e.type) {
      case "started":
        set({
          generation: {
            chatId,
            assistantMessageId: e.assistantMessageId,
            tokensPerSecond: null,
            promptTokens: null,
            completionTokens: null,
          },
        });
        // Reload so the new user message and the placeholder appear with real ids.
        void ipc.getChat(chatId).then((detail) => {
          if (get().activeChatId === chatId) set({ detail });
        });
        return;
      case "content":
      case "reasoning": {
        const gen = state.generation;
        const detail = state.detail;
        if (!gen?.assistantMessageId || !detail) return;
        const messages = detail.messages.map((m): MessageDto => {
          if (m.id !== gen.assistantMessageId) return m;
          return e.type === "content"
            ? { ...m, text: m.text + e.text }
            : { ...m, reasoning: (m.reasoning ?? "") + e.text };
        });
        set({ detail: { ...detail, messages } });
        return;
      }
      case "usage":
        if (state.generation) {
          set({
            generation: {
              ...state.generation,
              promptTokens: e.promptTokens,
              completionTokens: e.completionTokens,
            },
          });
        }
        return;
      case "timings":
        if (state.generation) {
          set({ generation: { ...state.generation, tokensPerSecond: e.predictedPerSecond } });
        }
        return;
      case "finished":
      case "error":
        if (e.type === "error") set({ error: e.error });
        void ipc.getChat(chatId).then((detail) => {
          if (get().activeChatId === chatId) set({ detail });
          set({ generation: null });
        });
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
    error: null,

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

    send: async (text, parentId, images) => {
      let chatId = get().activeChatId;
      if (chatId === null) {
        await get().newChat();
        chatId = get().activeChatId;
      }
      if (chatId === null) return;
      const parent = parentId === undefined ? lastMessageId(get().detail) : parentId;
      set({
        generation: {
          chatId,
          assistantMessageId: null,
          tokensPerSecond: null,
          promptTokens: null,
          completionTokens: null,
        },
        error: null,
      });
      const id = chatId;
      try {
        await ipc.sendMessage({ chatId: id, text, parentId: parent, images }, (e) => onStreamEvent(id, e));
      } catch (err) {
        set({ generation: null, error: toErrorInfo(err) });
      }
    },

    regenerate: async (assistantMessageId) => {
      const chatId = get().activeChatId;
      if (chatId === null) return;
      set({
        generation: {
          chatId,
          assistantMessageId: null,
          tokensPerSecond: null,
          promptTokens: null,
          completionTokens: null,
        },
        error: null,
      });
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
      if (chatId === null) return;
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
  };
});
