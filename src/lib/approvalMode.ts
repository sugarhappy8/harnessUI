// The approval mode of each chat as the window knows it: Ask first, Allow all or Full access.
//
// The mode itself lives in Rust (memory only, like grants; the default is saved) and
// changes only through the commands below, which only this window may call. This store is
// a mirror for the header menu, the approval cards and Settings, so they agree with each
// other.
import { create } from "zustand";
import * as ipc from "./ipc";
import type { ApprovalModeName } from "./ipc";

export type { ApprovalModeName };

interface ApprovalModeState {
  /** Chats whose mode was read. */
  modes: Record<number, ApprovalModeName>;
  /** The mode every chat starts in, as last read ("ask" until then). */
  defaultMode: ApprovalModeName;
  /** Reads the mode every chat starts in from the app. */
  loadDefault: () => Promise<void>;
  /** Reads a chat's mode from the app. */
  load: (chatId: number) => Promise<void>;
  /** Switches a chat. Rejects when the app refuses; the mirror then stays as it was. */
  setMode: (chatId: number, mode: ApprovalModeName) => Promise<void>;
  /** Changes the mode every chat starts in, then reads the chats on screen again. */
  setDefault: (mode: ApprovalModeName) => Promise<void>;
  /** The app says a mode changed: reads that chat again, or every chat read so far. */
  refresh: (chatId: number | null) => Promise<void>;
}

export const useApprovalMode = create<ApprovalModeState>((set, get) => ({
  modes: {},
  defaultMode: "ask",
  loadDefault: async () => {
    set({ defaultMode: await ipc.getDefaultApprovalMode() });
  },
  load: async (chatId) => {
    const mode = await ipc.getChatApprovalMode(chatId);
    set({ modes: { ...get().modes, [chatId]: mode } });
  },
  setMode: async (chatId, mode) => {
    const now = await ipc.setChatApprovalMode(chatId, mode);
    set({ modes: { ...get().modes, [chatId]: now } });
  },
  setDefault: async (mode) => {
    await ipc.setDefaultApprovalMode(mode);
    set({ defaultMode: mode });
    // Every chat follows the new default, also chats switched by hand: read them afresh.
    await get().refresh(null);
  },
  refresh: async (chatId) => {
    if (chatId !== null) return get().load(chatId);
    const known = Object.keys(get().modes).map(Number);
    const entries = await Promise.all(
      known.map(async (id) => [id, await ipc.getChatApprovalMode(id)] as const),
    );
    set({ modes: { ...get().modes, ...Object.fromEntries(entries) } });
  },
}));

/** The chat's mode, as far as the window knows ("ask" until read: never more than it is). */
export function useChatMode(chatId: number): ApprovalModeName {
  return useApprovalMode((s) => s.modes[chatId] ?? "ask");
}
