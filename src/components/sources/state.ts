import { create } from "zustand";
import { isTauri } from "../../lib/ipc";
import * as api from "./api";
import type { Source } from "./types";

/** The source the Sources tab is asked to show. `seq` makes every request a new one. */
export interface SourceFocus {
  chatId: number;
  n: number;
  seq: number;
}

interface SourcesState {
  /** The sources of each chat that was read. A chat that is not in here is not known
   *  yet: its citations are then shown as the plain text they are. */
  byChat: Record<number, Source[]>;
  focus: SourceFocus | null;
  /** Reads a chat's sources from the app. Never rejects. */
  load: (chatId: number) => Promise<void>;
  /** Reads a chat's sources once. */
  ensure: (chatId: number) => void;
  requestFocus: (chatId: number, n: number) => void;
}

let seq = 0;
const asked = new Set<number>();

/** What message citations, the Sources tab and the search tool cards share. */
export const useSources = create<SourcesState>((set, get) => ({
  byChat: {},
  focus: null,
  load: async (chatId) => {
    try {
      const sources = await api.listSources(chatId);
      set({ byChat: { ...get().byChat, [chatId]: sources } });
    } catch {
      // Left unknown: citations stay plain text rather than being marked as missing.
      asked.delete(chatId);
    }
  },
  ensure: (chatId) => {
    if (asked.has(chatId) || !isTauri()) return;
    asked.add(chatId);
    void get().load(chatId);
  },
  requestFocus: (chatId, n) => set({ focus: { chatId, n, seq: ++seq } }),
}));

let started = false;

/** Starts listening for sources that searches store. Runs once, inside the app only. */
export function startSources(): void {
  if (started || !isTauri()) return;
  started = true;
  void api.onSourcesChanged((chatId) => {
    asked.add(chatId);
    void useSources.getState().load(chatId);
  });
}

/** For tests: forgets everything. */
export function resetSources(): void {
  asked.clear();
  useSources.setState({ byChat: {}, focus: null });
}
