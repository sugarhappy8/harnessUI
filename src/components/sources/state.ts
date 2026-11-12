import { create } from "zustand";
import * as api from "./api";
import type { Source } from "./types";

/** Whether this is the app's own window. Anywhere else (a test, a plain browser) nothing
 *  is asked of the app. */
function inApp(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

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
    startSources();
    try {
      const sources = await api.listSources(chatId);
      set({ byChat: { ...get().byChat, [chatId]: sources } });
    } catch {
      // Left unknown: citations stay plain text rather than being marked as missing.
      asked.delete(chatId);
    }
  },
  ensure: (chatId) => {
    if (asked.has(chatId) || !inApp()) return;
    asked.add(chatId);
    void get().load(chatId);
  },
  requestFocus: (chatId, n) => set({ focus: { chatId, n, seq: ++seq } }),
}));

let started = false;

/**
 * Starts listening for sources that searches store: once, and only inside the app. It is
 * started by the first read of a chat's sources, never by importing this module. If
 * listening cannot start, the next read tries again.
 */
export function startSources(): void {
  if (started || !inApp()) return;
  started = true;
  api
    .onSourcesChanged((chatId) => {
      asked.add(chatId);
      void useSources.getState().load(chatId);
    })
    .catch(() => {
      started = false;
    });
}

/** For tests: forgets everything. */
export function resetSources(): void {
  asked.clear();
  started = false;
  useSources.setState({ byChat: {}, focus: null });
}
