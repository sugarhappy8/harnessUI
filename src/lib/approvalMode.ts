// The approval mode of each chat as the window knows it: Ask first, or Allow all.
//
// The mode itself lives in Rust (memory only, like grants) and changes only through the
// two commands below, which only this window may call. This store is a mirror for the
// header button, the approval cards and Settings, so they agree with each other.
import { create } from "zustand";
import * as ipc from "./ipc";

interface ApprovalModeState {
  /** Chats whose mode was read: true = Allow all, false = Ask first. */
  modes: Record<number, boolean>;
  /** "Allow all in every chat", as last read (false until then). */
  defaultOn: boolean;
  /** Reads "Allow all in every chat" from the app. */
  loadDefault: () => Promise<void>;
  /** Reads a chat's mode from the app. */
  load: (chatId: number) => Promise<void>;
  /** Switches a chat. Rejects when the app refuses; the mirror then stays as it was. */
  setMode: (chatId: number, allowAll: boolean) => Promise<void>;
  /** Switches "Allow all in every chat", then reads the chats on screen again. */
  setDefault: (on: boolean) => Promise<void>;
  /** The app says a mode changed: reads that chat again, or every chat read so far. */
  refresh: (chatId: number | null) => Promise<void>;
}

export const useApprovalMode = create<ApprovalModeState>((set, get) => ({
  modes: {},
  defaultOn: false,
  loadDefault: async () => {
    set({ defaultOn: await ipc.getAllowAllDefault() });
  },
  load: async (chatId) => {
    const allowAll = await ipc.getApprovalMode(chatId);
    set({ modes: { ...get().modes, [chatId]: allowAll } });
  },
  setMode: async (chatId, allowAll) => {
    const now = await ipc.setApprovalMode(chatId, allowAll);
    set({ modes: { ...get().modes, [chatId]: now } });
  },
  setDefault: async (on) => {
    await ipc.setAllowAllDefault(on);
    set({ defaultOn: on });
    // Every chat follows the default again unless switched by hand: read them afresh.
    await get().refresh(null);
  },
  refresh: async (chatId) => {
    if (chatId !== null) return get().load(chatId);
    const known = Object.keys(get().modes).map(Number);
    const entries = await Promise.all(
      known.map(async (id) => [id, await ipc.getApprovalMode(id)] as const),
    );
    set({ modes: { ...get().modes, ...Object.fromEntries(entries) } });
  },
}));

/** Whether the chat is in Allow all, as far as the window knows (false until read). */
export function useAllowAll(chatId: number): boolean {
  return useApprovalMode((s) => s.modes[chatId] ?? false);
}
