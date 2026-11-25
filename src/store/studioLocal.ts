// State for Studio model discovery, downloads, repairs and the one-time local model notice.
import type { StoreApi } from "zustand";
import * as api from "../components/studio/api";
import type { DownloadOffer } from "../components/studio/localOffers";
import type { LocalRepo, LocalSearchHit } from "../components/studio/types";
import { useDownloads } from "./downloads";
import type { StudioState, StudioTab } from "./studio";

/** The part of Studio's state this module keeps. */
export interface StudioLocalState {
  /** Get local models in place of the card stream. */
  localView: boolean;
  /** Get local models in place of the card stream (its own tabs start on `tab`), or the
   *  cards again (`tab` stays as it was). */
  showLocalModels(show: boolean): void;
  /** Get local models' own tab, Pictures or Video: set from `tab` each time the view opens;
   *  changing it leaves `tab` as it is. */
  localTab: StudioTab;
  setLocalTab(tab: StudioTab): void;
  /** *Only what fits* (on by default): the models that do not fit this computer are folded
   *  away under "Not for this computer". */
  onlyFits: boolean;
  setOnlyFits(on: boolean): void;
  /** The last Hugging Face search: its words, the tab it was for and what it found; `null`
   *  before the first. */
  localSearch: { text: string; tab: StudioTab; hits: LocalSearchHit[] } | null;
  /** Search or Enter in Get local models (never as typing goes): asks Hugging Face for `tab`'s
   *  kind of repositories matching `text`. An answer to a search asked before the last one is
   *  dropped. Rejects when Hugging Face could not be asked. */
  search(text: string, tab: StudioTab): Promise<void>;
  /** One search result opened: asks Hugging Face for repository `repo` at its latest commit,
   *  as Studio recognises it. */
  openRepo(repo: string): Promise<LocalRepo>;
  /** Delete on an installed picture or video model: its files go (not a part another model
   *  uses), its row leaves the list, then the local models are read again (a read that fails
   *  only leaves a trace: the files are gone all the same). Rejects when it was refused (while
   *  a run on this computer uses the model, `studio_model_in_use`). */
  deleteLocal(genModelId: string): Promise<void>;
  /** Checks the installed model and starts a download of any missing or damaged files. */
  repairLocal(genModelId: string): Promise<void>;
  /** What a Download button offers, every part and licence (Task B14's dialog shows it, or the
   *  note about local models first while that was never accepted); `null` while none is. */
  offer: DownloadOffer | null;
  setOffer(offer: DownloadOffer | null): void;
  /** Starts the confirmed download (with the set-up choice its plan was made with) and
   *  refreshes both model and download lists. */
  downloadLocal(offer: DownloadOffer): Promise<void>;
  /** *I understand* on the note about local models: the backend records it (local downloads
   *  are refused until then), and the local models and preferences say so. */
  acceptNotice(): Promise<void>;
  /** The note about local models read again, read only, from Get local models' footer
   *  (`"read"`); `null` while it is closed. (Before a first download it shows in place of the
   *  download confirmation, from `offer`.) */
  noticeOpen: "read" | null;
  setNoticeOpen(mode: "read" | null): void;
}

/** Counts Hugging Face searches as they are asked, so only the last one's answer is kept. */
let searchTick = 0;

/** Get local models' state and actions, for `useStudio`. */
export function createLocal(
  set: StoreApi<StudioState>["setState"],
  get: StoreApi<StudioState>["getState"],
): StudioLocalState {
  return {
    localView: false,
    showLocalModels: (show) => set((s) => (show ? { localView: true, localTab: s.tab } : { localView: false })),
    localTab: "image",
    setLocalTab: (localTab) => set({ localTab }),
    onlyFits: true,
    setOnlyFits: (onlyFits) => set({ onlyFits }),
    localSearch: null,
    search: async (text, tab) => {
      const mine = ++searchTick;
      const page = await api.studioLocalSearch(text, tab === "image" ? "pictures" : "video");
      if (mine === searchTick) set({ localSearch: { text, tab, hits: page.items } });
    },
    openRepo: (repo) => api.studioLocalRepo(repo),
    deleteLocal: async (genModelId) => {
      await api.studioLocalDelete(genModelId);
      // Its files are gone: its row goes now, whether or not the read after it works.
      set((s) =>
        s.local ? { local: { ...s.local, installed: s.local.installed.filter((m) => m.id !== genModelId) } } : {},
      );
      await get()
        .refreshLocal()
        .catch((e: unknown) => console.warn("Studio: the local models could not be read again", e));
    },
    repairLocal: async (genModelId) => {
      await api.studioLocalDownload({ installed: { genModelId } }, null);
    },
    offer: null,
    setOffer: (offer) => set({ offer }),
    downloadLocal: async (offer) => {
      await api.studioLocalDownload(offer.source, offer.choice);
      set((s) => (s.offer === offer ? { offer: null } : {}));
      await useDownloads
        .getState()
        .read()
        .catch((e: unknown) => console.warn("Studio: the downloads could not be read", e));
      await get()
        .refreshLocal()
        .catch((e: unknown) => console.warn("Studio: the local models could not be read again", e));
    },
    acceptNotice: async () => {
      const prefs = await api.studioAcceptLocalNotice();
      set((s) => ({ prefs, local: s.local ? { ...s.local, noticeAccepted: true } : null }));
    },
    noticeOpen: null,
    setNoticeOpen: (noticeOpen) => set({ noticeOpen }),
  };
}
