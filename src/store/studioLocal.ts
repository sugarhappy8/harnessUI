// Get local models' state (0.1.0 Tasks B13 and B14), composed into `useStudio`
// (src/store/studio.ts, which re-exports what other modules use from here; import it from
// there): whether the view is shown in place of the card stream, its own Pictures and Video
// tab, Only what fits and Show non-commercial models, Hugging Face asked only on Search or Enter
// and when one result is opened, an installed local model deleted, *Download again* (Task B8;
// refused for a licence not accepted, it turns into the download confirmation), what a Download
// button offers and the confirmation's Accept, and the note about local models, accepted or
// read again.
import type { StoreApi } from "zustand";
import * as api from "../components/studio/api";
import { ENGINE_LICENCE_ID, offerForEntry, type DownloadOffer } from "../components/studio/localOffers";
import type { LocalModels, LocalRepo, LocalSearchHit, LocalSource } from "../components/studio/types";
import { toErrorInfo } from "./app";
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
  /** *Show non-commercial models* (off by default): an opened search result whose licence is
   *  red shows its files. */
  showNonCommercial: boolean;
  setShowNonCommercial(on: boolean): void;
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
   *  uses), then the local models are read again. Rejects when it was refused (while a run on
   *  this computer uses the model, `studio_model_in_use`). */
  deleteLocal(genModelId: string): Promise<void>;
  /** *Download again*: installed model `genModelId`'s parts are checked and what is missing or
   *  damaged is fetched again; resolves `true` once that started. When the backend refuses it
   *  for a licence not accepted (`studio_license_not_accepted`), the local models are read
   *  again and the download confirmation opens on Get local models with the licences to accept
   *  (its Accept is `downloadLocal`), and this resolves `false`. Rejects when it could not
   *  start, or with that refusal when no licence can be shown. */
  repairLocal(genModelId: string): Promise<boolean>;
  /** What a Download button offers, every part and licence (Task B14's dialog shows it, or the
   *  note about local models first while that was never accepted); `null` while none is. */
  offer: DownloadOffer | null;
  setOffer(offer: DownloadOffer | null): void;
  /** The download confirmation's Accept: `offer`'s download, with the hash of every licence
   *  it lists (all of them were shown); then the offer is put away, and the app's downloads
   *  (so the model's row shows its download at once) and the local models are read again (a
   *  failed read leaves them as they were, and a trace). Rejects, keeping the offer, when the
   *  download could not start. */
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
    showNonCommercial: false,
    setShowNonCommercial: (showNonCommercial) => set({ showNonCommercial }),
    localSearch: null,
    search: async (text, tab) => {
      const mine = ++searchTick;
      const page = await api.studioLocalSearch(text, tab);
      if (mine === searchTick) set({ localSearch: { text, tab, hits: page.items } });
    },
    openRepo: (repo) => api.studioLocalRepo(repo),
    deleteLocal: async (genModelId) => {
      await api.studioLocalDelete(genModelId);
      await get().refreshLocal();
    },
    repairLocal: async (genModelId) => {
      const source: LocalSource = { installed: { genModelId } };
      try {
        await api.studioLocalDownload(source, []);
        return true;
      } catch (e) {
        if (toErrorInfo(e).code !== "studio_license_not_accepted") throw e;
        // The licences as the backend lists them now.
        await get()
          .refreshLocal()
          .catch((err: unknown) => console.warn("Studio: the local models could not be read again", err));
        const local = get().local;
        const offer = local ? repairOffer(local, genModelId, source) : null;
        // No empty dialog: the refusal says what to do instead.
        if (!offer) throw e;
        set({ offer });
        // Visible also when the repair started from a run card.
        get().showLocalModels(true);
        return false;
      }
    },
    offer: null,
    setOffer: (offer) => set({ offer }),
    downloadLocal: async (offer) => {
      await api.studioLocalDownload(offer.source, offer.licences.map((l) => l.sha256));
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

// ---- a repair refused for a licence ---------------------------------------------------------

/** What *Download again* on installed model `genModelId` asks to accept, when the backend
 *  refused it for a licence not accepted: a model of the list offers its parts and licences as
 *  its *Download* would; one from a search (the list does not hold it) offers the Studio
 *  engine's licence, the only one this view knows it may need, when an entry lists it. `null`
 *  when there is no licence to show. */
export function repairOffer(local: LocalModels, genModelId: string, source: LocalSource): DownloadOffer | null {
  const entries = [...local.recommended, ...local.notHere];
  const entry = entries.find((e) => e.id === genModelId);
  let offer: DownloadOffer | null = null;
  if (entry) {
    offer = { ...offerForEntry(entry, local), source };
  } else {
    const row = local.installed.find((m) => m.id === genModelId);
    const engine = entries.flatMap((e) => e.licences).find((l) => l.id === ENGINE_LICENCE_ID);
    if (row && engine) {
      offer = {
        // As it came: the dialog shows it with invisible characters as codes.
        name: row.name,
        kind: row.kind,
        source,
        parts: [],
        licences: [engine],
        totalBytes: row.sizeBytes,
        toFetchBytes: 0,
        needsEngine: true,
        engineBytes: local.engine.downloadBytes,
        lowDisk: false,
        gate: { mode: "none", collects: [] },
      };
    }
  }
  return offer && offer.licences.length > 0 ? offer : null;
}
