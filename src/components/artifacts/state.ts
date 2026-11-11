import { create } from "zustand";
import type { ArtifactChanged, Isolation } from "./types";

/** The artifact the panel is asked to show. `seq` makes every request a new one. */
export interface Focus {
  chatId: number;
  id: number;
  /** A version, or null for the newest. */
  version: number | null;
  seq: number;
}

/** A version whose preview a tool is waiting to hear about. */
export interface PendingReport {
  chatId: number;
  id: number;
  version: number;
}

export const reportKey = (id: number, version: number) => `${id}:${version}`;

/** What the user has typed into an artifact's source and not saved. */
export interface Draft {
  /** The version the text started from. */
  base: number;
  text: string;
}

/** A tool waits four seconds for a report; after this long nobody is listening. */
const REPORT_EXPIRES_MS = 8000;

interface ArtifactsState {
  /** Whether previews may run scripts on this system. Until it is known, they may not. */
  isolation: Isolation;
  setIsolation: (i: Isolation) => void;
  focus: Focus | null;
  requestFocus: (chatId: number, id: number, version: number | null) => void;
  /** Reports tools are waiting for, by `reportKey`. */
  pending: Record<string, PendingReport>;
  clearReport: (id: number, version: number) => void;
  /**
   * Unsaved edits, by artifact. Kept here, not in the panel, so that they survive a look
   * at another tab, another artifact or another chat.
   */
  drafts: Record<number, Draft>;
  setDraft: (id: number, draft: Draft | null) => void;
  /** Counts the changes to each chat's artifacts, so that lists reload. */
  changes: Record<number, number>;
  /** A tool created an artifact or saved a version (the `artifact-changed` event). */
  noteChange: (c: ArtifactChanged) => void;
}

let seq = 0;

/** What the Artifacts panel, the tool cards and the start-up code share. */
export const useArtifacts = create<ArtifactsState>((set, get) => ({
  isolation: { state: "unchecked" },
  setIsolation: (isolation) => set({ isolation }),
  focus: null,
  requestFocus: (chatId, id, version) => {
    seq += 1;
    set({ focus: { chatId, id, version, seq } });
  },
  pending: {},
  clearReport: (id, version) => {
    const key = reportKey(id, version);
    if (!(key in get().pending)) return;
    const pending = { ...get().pending };
    delete pending[key];
    set({ pending });
  },
  drafts: {},
  setDraft: (id, draft) => {
    const drafts = { ...get().drafts };
    if (draft) drafts[id] = draft;
    else delete drafts[id];
    set({ drafts });
  },
  changes: {},
  noteChange: (c) => {
    const changes = { ...get().changes, [c.chatId]: (get().changes[c.chatId] ?? 0) + 1 };
    if (!c.wantsReport) {
      set({ changes });
      return;
    }
    const pending = { ...get().pending, [reportKey(c.id, c.version)]: { chatId: c.chatId, id: c.id, version: c.version } };
    set({ changes, pending });
    setTimeout(() => get().clearReport(c.id, c.version), REPORT_EXPIRES_MS);
  },
}));
