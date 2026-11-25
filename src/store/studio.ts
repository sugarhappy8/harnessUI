// Studio's store (0.1.0): which view the main window shows, the chat or Studio (the chat
// stays mounted, and its reply keeps running, while Studio is shown), Studio's tab, and the
// runs as the backend reports them (`studio-progress`; `studio_live` once at start, as events
// are not replayed). A video that finishes while the chat is shown raises a notice there.
// Task B2: what Studio has saved (endpoints and their lists, local models, the gallery's first
// page, preferences, today's spending), read when Studio opens and never fetched then (the
// lists, local models, preferences and spending are read again each time Studio is shown and
// when Settings closes; lists fetched meanwhile are kept); each tab's draft, which starts at
// the model used last or, local first, a default, and starts again when its endpoint is
// removed. Task B3: the user's pick in the model picker, which carries the draft's settings
// to the model picked and asks for an OpenRouter picture model's price once (a failure kept
// per tab); the newest runs whatever the gallery's filter, and the recent models. Task B5:
// each field set or reset (a value whose field no longer applies leaves the draft, and a
// transparent background moves the file type to PNG), the added pictures and their roles, and
// how long a local clip takes here at each size and length, asked once each. Task B6: Advanced's
// Reset all, the seed's dice and lock, Extra fields, an OpenRouter picture's routing, and My
// setups (a model and every setting under a name; at most 50, kept in the preferences). Task B7:
// each tab's estimate and Create, with the confirmation and why a press was refused (in
// ./studioRun, composed in here), today's spending read again as a cloud run starts and ends,
// and the picker opened on its cloud models from the dock. Task B8:
// a run stopped (Cancel, Stop, Stop watching), a video's job checked again (*Get it anyway*,
// *Check again*), the answer to "Unload the chat model?", the Studio engine's download and a
// damaged local model's download again, a run made again from its own settings, and the result
// open in the viewer. Task B9: the actions on a result (Reuse all and Reuse without seed, which
// never start a run; Animate, Edit and Use as reference, which keep the picture as an added
// picture; Star; Delete, after which a finished run left without results goes too; several saved
// into one folder: in ./studioResults, composed in here), the grid's filters and Load more.
// Task B10: the Studio engine's state read again (Settings → Studio's engine line), and Delete
// stopped and failed runs (in ./studioResults). Task B11: a picture attached in a chat opened
// in Studio's Pictures tab, and Send to chat (in ./studioResults). Task B13: Get local models'
// tabs, hardware filter, Hugging Face search, download confirmation, repairs and the local
// model notice are in ./studioLocal.
import { create, type StoreApi, type UseBoundStore } from "zustand";
import * as api from "../components/studio/api";
import { carry, closestModel, defaultsFor, dropUnmet, familyOf, type Values } from "../components/studio/canonical";
import type {
  Company,
  ImageInput,
  InstalledLocal,
  LocalEntry,
  LocalFit,
  LocalModels,
  MediaKind,
  MediaModel,
  ModelChoice,
  RefItem,
  RefRole,
  Routing,
  Run,
  RunsCursor,
  RunsFilter,
  RunStatus,
  RunTarget,
  Setup,
  StudioEndpoint,
  StudioPrefs,
  StudioPrefsPatch,
  Usage,
} from "../components/studio/types";
import * as ipc from "../lib/ipc";
import { listenInEffect } from "../lib/listen";
import type { ErrorInfo } from "../lib/types";
import { STUDIO_CHAT_LINK, STUDIO_EMPTY, STUDIO_FIELDS, STUDIO_REFS, STUDIO_SETUPS } from "../lib/words";
import { toErrorInfo } from "./app";
import { createLocal, type StudioLocalState } from "./studioLocal";
import { createResults, type StudioResultsState } from "./studioResults";
import { createRun, targetOf, type StudioRunState } from "./studioRun";

// Studio's estimate and Create live in ./studioRun; they are used from here.
export {
  currentEstimate,
  ESTIMATE_DELAY_MS,
  PROMPT_MAX_CHARS,
  promptProblems,
  requestFromRun,
  requestKey,
  seedKey,
  settingsProblems,
  shownEstimate,
  startRequestOf,
  targetOf,
  type StudioRunState,
  type TabEstimate,
} from "./studioRun";

// The actions on a result live in ./studioResults; they are used from here.
export {
  fileNameOf,
  joinPath,
  runOfOutput,
  type ModelOffer,
  type ResultsDeps,
  type StudioResultsState,
} from "./studioResults";

// Get local models' state and actions live in ./studioLocal; they are used from here.
export type { StudioLocalState } from "./studioLocal";

/** Studio's tabs: Pictures and Video. */
export type StudioTab = MediaKind;

/** A tab's draft: the model it is set to and what Create would send. */
export interface Draft {
  /** Where a run goes; `null` until there is a model to use. */
  target: RunTarget | null;
  model: string | null;
  prompt: string;
  /** The model's settings by field key; an unset field has no key. */
  values: Record<string, unknown>;
  /** Extra fields (JSON), for a company that takes them. */
  extra: Record<string, unknown>;
  refs: RefItem[];
  /** The seed stays as it is for the next run. */
  seedLocked: boolean;
  /** Which companies may run an OpenRouter picture. */
  routing: Routing | null;
  /** What changed when the model was switched, until it is put away. */
  note: string | null;
}

/** Which empty state a tab shows in place of the card stream, if any. */
export type EmptyState = "getLocal" | "cloudBackup" | "addEndpoint" | null;

export interface StudioState extends StudioRunState, StudioResultsState, StudioLocalState {
  /** What the main window shows beside the chat list. */
  view: "chat" | "studio";
  tab: StudioTab;
  /** Each run the backend reported, by id, at its newest state. */
  live: Record<string, Run>;
  /** The id of a video that finished while the chat was shown. */
  readyNotice: string | null;
  /** Opens Studio. Given `tab` (*Open in Studio*, a finished video's *Open*), on that tab's
   *  cards, closing Get local models if it was left open; else as it was left, on the tab and
   *  view shown last. */
  showStudio(tab?: StudioTab): void;
  /** Shows the chat; the viewer closes (it would stay over the chat). */
  showChat(): void;
  /** *Open in Studio* on a picture attached in a chat: `image` is kept as an added picture
   *  (`studio_add_ref`, with the checks of a picture dropped on the tray) and added to the
   *  Pictures tab's draft as the first of its model's picture roles with room, and Studio is
   *  shown on that tab. When Studio was never opened, what it has saved is read first (`load`,
   *  which fetches nothing). Returns why it was not added (no picture model yet, or the model
   *  takes no more: checked before the picture is kept), or `null`; rejects when it could not
   *  be kept. */
  openInStudio(image: ImageInput): Promise<string | null>;
  /** A run's new state, from `studio-progress`, `studio_live` or a command's answer: kept only
   *  when the backend numbered it higher (`seq`) than the copy held, so an answer read before
   *  an event it arrives after never puts an older state back. */
  applyProgress(run: Run): void;
  /** Puts away the notice of a video that finished while the chat was shown (opening Studio
   *  puts it away too). */
  dismissReadyNotice(): void;
  /** The result open in the viewer (Task B9 draws it); `null` while it is closed. */
  viewer: string | null;
  /** Opens result `outputId` in the viewer; `null` closes it. */
  openViewer(outputId: string | null): void;
  /** Cancel, Stop, Stop watching: stops run `runId`, and its new state joins the runs. Rejects
   *  when it could not be stopped. Never a create. */
  cancel(runId: string): Promise<void>;
  /** *Get it anyway*, *Check again*: asks the company about a video's job again, and the run's
   *  new state joins the runs. Never a create, so never a second charge. */
  fetchAnyway(runId: string): Promise<void>;
  /** The answer to "Unload the chat model?" (Settings → Studio's Ask) for local run `runId`:
   *  unload it, or not (a picture then runs on the processor). */
  answerUnload(runId: string, unload: boolean): Promise<void>;
  /** The Studio engine's download where it is missing or partly there (its licence was shown
   *  with the first model), then the engine's state read again; how the download goes comes as
   *  `studio-engine-progress`. Rejects when it could not start. */
  downloadEngine(): Promise<void>;
  /** Reads the Studio engine's state again into the local models (after a removal, which
   *  sends no `finished` event, a download that ended, or one that could not start). A failure
   *  leaves the state as it was, and a trace; before the local models were read it does nothing. */
  refreshEngine(): Promise<void>;
  /** Again, Run again: `run` once more, with its own settings where it ran (`start` from it, so
   *  a cloud run that needs confirming asks first, and a second press meanwhile does nothing). */
  again(run: Run): Promise<void>;

  /** The endpoints that make pictures or video, with their saved lists. */
  endpoints: StudioEndpoint[];
  /** What this computer can run; `null` until read. */
  local: LocalModels | null;
  /** Everything saved has been read once. */
  loaded: boolean;
  drafts: Record<StudioTab, Draft>;
  /** The gallery's runs read so far, newest first. */
  runs: Run[];
  /** The newest runs whatever the gallery's filter, newest first, at most `RECENT_RUNS`: read
   *  when Studio opens and kept up by `applyProgress`. The picker's Recent group comes from
   *  them, each run with where it ran. */
  recentRuns: Run[];
  /** Where the next page of runs starts; `null` after the last page. */
  next: RunsCursor | null;
  filter: RunsFilter;
  prefs: StudioPrefs | null;
  usage: Usage | null;
  /** The gallery's grid in place of the card stream. */
  grid: boolean;
  /** Reads what is saved: the lists, local models, the gallery's first page, the runs still
   *  working, preferences and today's spending. Never fetches a list or a price; asked again
   *  while reading, it gives the same read. Each tab without a model then gets its first. */
  load(): Promise<void>;
  /** Reads again what may have changed while Studio was hidden or Settings was open: the
   *  lists, local models, preferences and today's spending. Not the gallery's pages (the
   *  pages read further on stay) nor the runs working (`studio-progress` follows them). Never
   *  fetches; each tab without a model then gets its first, and a chosen model stays unless
   *  its endpoint was removed. Lists fetched while this read was on its way stay. */
  reread(): Promise<void>;
  /** *Get the list*, *Update model lists*: fetches endpoint `providerId`'s lists. */
  refreshModels(providerId: number): Promise<void>;
  /** Reads the local models again (after a download or a delete). */
  refreshLocal(): Promise<void>;
  setTab(tab: StudioTab): void;
  /** Sets `tab`'s draft to the model used last when it is still offered, else to
   *  `defaultModel`, at that model's starting values. Fetches nothing (not even a price). */
  selectInitialModel(tab: StudioTab): void;
  /** The user's pick in the model picker: `tab`'s draft goes to model `modelId` at `target`,
   *  its settings carried there (`carry`) with a note of what changed. An OpenRouter picture
   *  model whose price was never fetched is asked for it, once; a local model never is. Its
   *  Extra field comes with that answer, so the draft's extra settings are held aside until
   *  then (`holding`) and carried to the model as it comes back (kept held when the request
   *  fails, until *Get the price* brings the answer; as listed, dropped with the note, when
   *  another model is picked first). A model `tab` cannot use is ignored. */
  chooseModel(tab: StudioTab, target: RunTarget, modelId: string): void;
  /** *Get the price*: asks for the price of `tab`'s model (a model restored when Studio opened
   *  is never asked by itself); extra settings held for it are carried to it as it comes back.
   *  A failure goes to `priceError`, not to the caller. */
  fetchPrice(tab: StudioTab): Promise<void>;
  /** Each tab's failed price request, while the tab stays on the model it asked for (shown
   *  under the Model field, where *Get the price* asks again); put away by the next request
   *  and by a change of model. */
  priceError: Record<StudioTab, ErrorInfo | null>;
  dismissPriceError(tab: StudioTab): void;
  /** Each tab whose extra settings are held aside until its model's price answer brings the
   *  model's Extra field (see `chooseModel`): Create waits while they are. A failed request
   *  keeps them held, and *Get the price* asks again. */
  holding: Record<StudioTab, boolean>;
  setPrompt(tab: StudioTab, text: string): void;
  /** Sets field `key` of `tab`'s draft. A transparent background moves a file type that cannot
   *  be transparent to PNG (else WebP) and says so in the note; then every value whose field
   *  no longer applies leaves the draft (`dropUnmet`). */
  setValue(tab: StudioTab, key: string, value: unknown): void;
  /** Unsets field `key` (back to Automatic, so it is not sent), and with it every value whose
   *  field no longer applies. */
  resetValue(tab: StudioTab, key: string): void;
  /** Adds a picture kept by `studio_add_ref` to `tab`'s draft, as `ref.role` or, without one,
   *  as the first role of the model's picture fields with room. Returns why it was not added
   *  (the model takes no more), or `null`. */
  addRef(tab: StudioTab, ref: RefItem): string | null;
  removeRef(tab: StudioTab, index: number): void;
  /** Moves added picture `index` to `role`; returns why not (the model takes no more of that
   *  role), or `null`. */
  setRefRole(tab: StudioTab, index: number, role: RefRole): string | null;
  /** Puts away the note of what changed. */
  dismissNote(tab: StudioTab): void;
  /** What each tab's fields hold that cannot be sent, by field key (a custom size typed that
   *  does not fit, while the draft keeps the size before it): Create waits, and says why. */
  invalid: Record<StudioTab, Record<string, string>>;
  /** Field `key` of `tab` holds `problem`, or (`null`) nothing that cannot be sent. */
  setInvalid(tab: StudioTab, key: string, problem: string | null): void;
  /** *Reset all*: every Advanced field back to Automatic, no Extra fields and no routing (and
   *  the seed no longer kept); then every value whose field no longer applies leaves the draft. */
  resetAdvanced(tab: StudioTab): void;
  /** The dice: a new random seed, other than the one set, for a model that takes a seed. */
  newSeed(tab: StudioTab): void;
  /** *Keep it*: whether the seed stays as it is for the next run. */
  lockSeed(tab: StudioTab, locked: boolean): void;
  /** Sets `tab`'s Extra fields (checked by `parseExtra` first). */
  setExtra(tab: StudioTab, extra: Values): void;
  /** Sets which companies may run `tab`'s OpenRouter picture (`null`: OpenRouter chooses). */
  setRouting(tab: StudioTab, routing: Routing | null): void;
  /** *Save as setup…*: keeps `tab`'s model, where it runs and every setting under `name` in the
   *  preferences, in place of a setup of that kind with the same name. Returns why it was not
   *  saved (no name, no model, 50 setups already), or `null`; a failure to save rejects. */
  saveSetup(tab: StudioTab, name: string): Promise<string | null>;
  /** Picking a setup: its tab's draft goes to the setup's model where it runs, with its
   *  settings, Extra fields and routing as saved (the prompt stays; added pictures stay up to
   *  the model's limit). When that model is no longer offered there, its family's newest takes
   *  its place, the settings carried to it (`carry`) with a note of what changed. An OpenRouter
   *  picture model whose price was never asked is asked for it, as when picked, and the setup's
   *  extra settings are held aside until the answer brings the model's Extra field (each one it
   *  does not take is then removed with the note). Returns why the setup cannot be used (its
   *  model and family are gone, or its endpoint or key), or `null`. */
  applySetup(setup: Setup): string | null;
  /** How long an installed model takes here, by `localFitKey`: asked once for each size and
   *  length, and asked again once the local models are read again (a run may have been
   *  measured since). */
  localFits: Record<string, LocalFit>;
  /** Asks how long installed model `modelId` takes here at `width` × `height` and, for a clip,
   *  `seconds` (0 for a picture, or for the model's own length), unless it was asked already
   *  (one that could not be worked out is not asked again until the local models are read
   *  again). Worked out on this computer: nothing is sent anywhere. */
  askLocalFit(modelId: string, width: number, height: number, seconds: number): void;
  /** Changes the gallery's filter and reads its first page again; when that read fails, the
   *  filter goes back to what it was (the runs held are its) and the failure rejects. */
  setFilter(patch: Partial<RunsFilter>): Promise<void>;
  /** *Load more*: the gallery's next page, after the last one read, for the same filter (one
   *  at a time; a page for a filter changed since is dropped). */
  loadMore(): Promise<void>;
  /** The grid in place of the card stream, or the cards again. The grid opens on All (both
   *  kinds, its first page read again); leaving it puts its own filters away, Pictures or Video
   *  and the date: the cards read the tab's own gallery (its first page read again). */
  setGrid(grid: boolean): Promise<void>;
  /** Changes only the preferences `patch` names. */
  setPrefs(patch: StudioPrefsPatch): Promise<void>;

  /** The model picker opened from elsewhere: `"cloud"` on its first cloud group (*Choose a
   *  cloud model*); `null` once it closes. */
  pickerOpen: "cloud" | null;
  openPicker(place: "cloud" | null): void;
}

const EMPTY_DRAFT: Draft = {
  target: null,
  model: null,
  prompt: "",
  values: {},
  extra: {},
  refs: [],
  seedLocked: false,
  routing: null,
  note: null,
};

/** How many runs one page of the gallery holds. */
const RUNS_PAGE = 60;

/** How many of the newest runs `recentRuns` keeps. */
export const RECENT_RUNS = 50;

/** The most setups the preferences keep (the backend's `prefs::MAX_SETUPS`). */
const MAX_SETUPS = 50;

/** The states a run ends in. */
const ENDED: ReadonlySet<RunStatus> = new Set<RunStatus>(["done", "failed", "blocked", "cancelled", "interrupted"]);

/** A random seed from 0 to `max`. */
function randomSeed(max: number): number {
  return Math.floor(Math.random() * (max + 1));
}

/** Whether `filter` lets every run through, as the backend reads it (a blank text does not
 *  filter, nor does Starred off). */
function unfiltered(filter: RunsFilter): boolean {
  return (
    !filter.kind &&
    !filter.starred &&
    !filter.text?.trim() &&
    !filter.model &&
    (filter.since ?? null) === null &&
    (filter.until ?? null) === null
  );
}

/** `runs` each once (the first of an id wins), newest first, at most `RECENT_RUNS`. */
function newestRuns(runs: Run[]): Run[] {
  const byId = new Map<string, Run>();
  for (const run of runs) if (!byId.has(run.id)) byId.set(run.id, run);
  return [...byId.values()].sort((a, b) => b.createdAt - a.createdAt).slice(0, RECENT_RUNS);
}

/** The read of what is saved while it is under way, so a second `load` waits for it. */
let loading: Promise<void> | null = null;

/** The gallery's next page while it is being read, so a second *Load more* waits for it. */
let paging: Promise<void> | null = null;

/** The words a saved model that is gone is explained with (a setup's, or a run's). */
export interface GoneWords {
  goneHere: string;
  endpointGone: string;
  replaced(endpoint: string, model: string): string;
  gone(endpoint: string): string;
}

/** A setup, or a run to reuse: a model where it runs, with its settings. */
export interface SavedModel {
  target: RunTarget;
  model: string;
  params: Values;
  extra: Values;
  routing: Routing | null;
}

/** Extra settings a tab's draft holds aside while its model's price, and with it the model's
 *  Extra field, is on its way: where the model runs, the model as listed when it was picked,
 *  and the settings. */
interface HeldExtra {
  target: RunTarget;
  model: MediaModel;
  extra: Values;
}

export const useStudio: UseBoundStore<StoreApi<StudioState>> = create<StudioState>((set, get) => {
  /** Gives each tab its first model when it has none (a list or a model got later) or when
   *  its endpoint was removed in Settings. Otherwise the model chosen stays: Studio never
   *  moves a tab between this computer and the cloud by itself. */
  const fillDrafts = () => {
    const { drafts, endpoints } = get();
    for (const tab of ["image", "video"] as const) {
      const { target, model } = drafts[tab];
      const gone =
        target !== null && target !== "thisComputer" && !endpoints.some((e) => e.providerId === target.endpoint);
      if (model === null || gone) get().selectInitialModel(tab);
    }
  };

  /** The price requests on their way, by endpoint and model: a model is asked for once. */
  const asking = new Map<string, Promise<MediaModel>>();
  /** Each tab's extra settings held aside, while they are (`holding` says so in the state). */
  const held: Partial<Record<StudioTab, HeldExtra>> = {};
  /** Holds `hold` aside for `tab`, or (`null`) nothing any more. */
  const setHeld = (tab: StudioTab, hold: HeldExtra | null) => {
    if (hold) held[tab] = hold;
    else delete held[tab];
    if (get().holding[tab] !== (hold !== null)) set((s) => ({ holding: { ...s.holding, [tab]: hold !== null } }));
  };

  /** Asks endpoint `providerId` for model `modelId`'s price (the same request while one is on
   *  its way) and keeps the answer in place of the model as listed. */
  const askPrice = (providerId: number, modelId: string): Promise<MediaModel> => {
    const key = `${providerId} ${modelId}`;
    const known = asking.get(key);
    if (known) return known;
    const asked = (async () => {
      try {
        const priced = await api.studioModelPrice(providerId, modelId);
        set((s) => ({
          endpoints: s.endpoints.map((e) =>
            e.providerId === providerId ? { ...e, models: e.models.map((m) => (m.id === modelId ? priced : m)) } : e,
          ),
        }));
        return priced;
      } finally {
        asking.delete(key);
      }
    })();
    asking.set(key, asked);
    return asked;
  };

  /** Asks for the price of model `modelId` at endpoint `providerId` for `tab`, putting away the
   *  tab's last failure; a failure is kept for the tab while it stays on that model. */
  const priceFor = (tab: StudioTab, providerId: number, modelId: string): Promise<MediaModel> => {
    set((s) => ({ priceError: { ...s.priceError, [tab]: null } }));
    const asked = askPrice(providerId, modelId);
    asked.catch((e: unknown) => {
      const { target, model } = get().drafts[tab];
      if (target === null || target === "thisComputer" || target.endpoint !== providerId || model !== modelId) return;
      set((s) => ({ priceError: { ...s.priceError, [tab]: toErrorInfo(e) } }));
    });
    return asked;
  };

  /** Carries `hold`'s extra settings to `model` (the model as it came back, or as listed) in
   *  `tab`'s draft, adding the note for each one it does not take; nothing once the draft has
   *  gone to another model. */
  const settleHeld = (tab: StudioTab, hold: HeldExtra, model: MediaModel) => {
    if (held[tab] !== hold) return;
    setHeld(tab, null);
    const carried = carry(model, model, {}, hold.extra, []);
    set((s) => {
      const draft = s.drafts[tab];
      if (draft.target === null || !sameTarget(draft.target, hold.target) || draft.model !== hold.model.id) return {};
      const note = [draft.note, carried.note].filter((n) => n !== null).join(" ") || null;
      // The model as it came back may require more of a value than as listed.
      const values = dropUnmet(model, draft.values);
      return { drafts: { ...s.drafts, [tab]: { ...draft, values, extra: { ...draft.extra, ...carried.extra }, note } } };
    });
  };

  /** Asks for the price of `to`, which `tab`'s draft was just set to at `endpoint` (`target`),
   *  holding `extra` aside when given (the model as listed has no Extra field yet): they go to
   *  the model as it comes back, each one it does not take removed with the note
   *  (`settleHeld`). When the request fails they stay held, never checked against the model as
   *  listed (which has no Extra field, so would drop every one): *Get the price* asks again. A
   *  failure is kept in `priceError` (`priceFor`). */
  const askPriceHolding = (
    tab: StudioTab,
    target: RunTarget,
    endpoint: StudioEndpoint,
    to: MediaModel,
    extra: Values | null,
  ) => {
    const mine: HeldExtra | null = extra ? { target, model: to, extra } : null;
    if (mine) setHeld(tab, mine);
    priceFor(tab, endpoint.providerId, to.id).then(
      (priced) => mine && settleHeld(tab, mine, priced),
      // Still held; the failure is in `priceError`.
      () => undefined,
    );
  };

  /** The local fits asked for and not answered yet. A new set each time the answers are put
   *  away, so an answer to an older one is not kept. */
  let fitting = new Set<string>();
  /** The local fits that could not be worked out: not asked again until the answers are put
   *  away (their chips show no time). */
  let fitFailed = new Set<string>();
  /** Puts away the local fits: the local models were read again. */
  const forgetFits = () => {
    fitting = new Set();
    fitFailed = new Set();
    set({ localFits: {} });
  };

  /** Counts the reads and fetches of the endpoints' lists as they start and as fetches come
   *  back, so an answer can tell what came back after it was asked for. */
  let listsTick = 0;
  /** For each endpoint, the tick at which a fetch of its lists last came back. */
  const fetchedTick = new Map<number, number>();

  /** `answer`'s endpoints (which endpoints there are, their names and keys), each with the
   *  lists in it, except where a fetch of an endpoint's lists came back after `since` (when
   *  this answer was asked for): those lists are newer, and stay. `fetched` is the endpoint
   *  this answer fetched, whose lists are the newest. */
  const newestLists = (answer: StudioEndpoint[], since: number, fetched?: number): StudioEndpoint[] => {
    const now = get().endpoints;
    return answer.map((e) => {
      if (e.providerId === fetched || (fetchedTick.get(e.providerId) ?? 0) <= since) return e;
      const kept = now.find((n) => n.providerId === e.providerId);
      return kept ? { ...e, fetchedAt: kept.fetchedAt, models: kept.models } : e;
    });
  };

  /** Reads today's spending again (a cloud run was accepted, or ended): the top bar and the
   *  daily limit follow it. A failure leaves the figure as it was, and a trace. */
  const rereadUsage = () => {
    Promise.resolve()
      .then(() => api.studioUsage())
      .then(
        (usage) => set({ usage }),
        (e: unknown) => console.warn("Studio: today's spending could not be read", e),
      );
  };

  /**
   * Puts `saved` (a setup, or a run to reuse) in `tab`'s draft: its model where it runs, its
   * settings, Extra fields and routing, `prompt`, and `refs` up to the model's limit for each
   * role (with a note for the rest). When the model is no longer offered there, its family's
   * newest takes its place, the settings carried to it (`carry`) with a note of what changed
   * (`words`). An OpenRouter picture model whose price was never asked is asked for it, as when
   * picked, and the extra settings are held aside until the answer brings the model's Extra
   * field (each one it does not take is then removed with the note). Returns why it cannot be
   * used (its model and family are gone, or its endpoint or key), or `null`.
   */
  const fillDraft = (
    tab: StudioTab,
    saved: SavedModel,
    refs: RefItem[],
    prompt: string,
    words: GoneWords,
  ): string | null => {
    const s = get();
    const { target } = saved;
    const endpoint = endpointOf(target, s.endpoints);
    const offered = offeredModel(tab, target, saved.model, s.local, s.endpoints);
    let to = offered;
    if (!to) {
      if (target === "thisComputer") return words.goneHere;
      if (!endpoint) return words.endpointGone;
      if (!endpoint.hasKey) return STUDIO_EMPTY.noKey(endpoint.name);
      to = closestModel(cloudModelsFor(endpoint, tab), { id: saved.model, family: familyOf(saved.model) });
      if (!to) return words.gone(endpoint.name);
    }
    // An OpenRouter picture model whose price was never asked has no Extra field yet: the
    // extra settings are held aside until its answer, as when the model is picked.
    const unpriced = endpoint !== null && priceNotAsked(to, endpoint.company);
    const hold = mustHold(to, unpriced, saved.extra);
    let values: Values = saved.params;
    let extra: Values = hold ? {} : saved.extra;
    let routing = saved.routing;
    let note: string | null = null;
    if (!offered && endpoint) {
      const carried = carry(null, to, values, extra, []);
      values = carried.values;
      extra = carried.extra;
      // Which companies may run it belongs to the model it was saved for.
      routing = null;
      note = [words.replaced(endpoint.name, to.displayName), carried.note].filter((n) => n !== null).join(" ");
    }
    const model = to;
    // The added pictures stay up to the model's limit for each role, with a note for the rest.
    const pictures = carry(model, model, {}, {}, refs);
    note = [note, pictures.note].filter((n) => n !== null).join(" ") || null;
    // The draft starts again: nothing is held aside for the model it had.
    setHeld(tab, null);
    set((st) => ({
      drafts: {
        ...st.drafts,
        [tab]: {
          ...st.drafts[tab],
          target,
          model: model.id,
          prompt,
          values: dropUnmet(model, values),
          extra,
          refs: pictures.refs,
          routing,
          note,
        },
      },
      priceError: { ...st.priceError, [tab]: null },
      // A refusal was for the model or place the draft had (the daily limit, a model that cannot
      // be used): the next press says again if it still applies.
      refusal: { ...st.refusal, [tab]: null },
    }));
    // Using it picks its model: an OpenRouter picture model is asked for its price once.
    if (unpriced && endpoint) askPriceHolding(tab, target, endpoint, model, hold ? saved.extra : null);
    return null;
  };

  return {
    view: "chat",
    tab: "image",
    live: {},
    readyNotice: null,
    // The finished video is on its card in Studio now. A tab asked for is shown on its cards.
    showStudio: (tab) =>
      set((s) => ({ view: "studio", tab: tab ?? s.tab, readyNotice: null, ...(tab ? { localView: false } : {}) })),
    showChat: () => set({ view: "chat", viewer: null }),
    openInStudio: async (image) => {
      if (!get().loaded) await get().load();
      const before = get();
      const model = draftModel(before, "image");
      if (!model) return STUDIO_CHAT_LINK.noModel;
      const placed = placeRef(model, before.drafts.image.refs, null);
      if ("refused" in placed) return placed.refused;
      const kept = await api.studioAddRef(image);
      const refused = get().addRef("image", { ...kept, role: null });
      if (refused === null) get().showStudio("image");
      return refused;
    },
    applyProgress: (run) => {
      const before = get().live[run.id];
      // Of two copies of a run (an event, a command's answer, the read at start), the one the
      // backend numbered higher is newer: an older copy that comes later is dropped.
      if (before && run.seq <= before.seq) return;
      set((s) => {
        const finished = run.kind === "video" && run.status === "done" && s.live[run.id]?.status !== "done";
        return {
          live: { ...s.live, [run.id]: run },
          recentRuns: newestRuns([run, ...s.recentRuns]),
          readyNotice: finished && s.view === "chat" ? run.id : s.readyNotice,
        };
      });
      // A cloud run that just ended has settled what it cost.
      if (run.runsOn === "cloud" && ENDED.has(run.status) && !(before && ENDED.has(before.status))) rereadUsage();
    },
    dismissReadyNotice: () => set({ readyNotice: null }),
    viewer: null,
    openViewer: (outputId) => set({ viewer: outputId }),
    cancel: async (runId) => {
      get().applyProgress(await api.studioCancel(runId));
    },
    fetchAnyway: async (runId) => {
      get().applyProgress(await api.studioFetchAnyway(runId));
    },
    answerUnload: async (runId, unload) => {
      get().applyProgress(await api.studioAnswerUnload(runId, unload));
    },
    downloadEngine: async () => {
      await api.studioEngineDownload(true);
      // The engine now reads as downloading. The download has started whether or not this read
      // works.
      await get().refreshEngine();
    },
    refreshEngine: async () => {
      try {
        const engine = await api.studioEngineStatus();
        set((s) => (s.local ? { local: { ...s.local, engine } } : {}));
      } catch (e) {
        console.warn("Studio: the engine's state could not be read", e);
      }
    },
    again: async (run) => {
      await get().start(run.kind, false, run);
    },

    endpoints: [],
    local: null,
    loaded: false,
    drafts: { image: EMPTY_DRAFT, video: EMPTY_DRAFT },
    runs: [],
    recentRuns: [],
    next: null,
    filter: {},
    prefs: null,
    usage: null,
    grid: false,
    load: () => {
      loading ??= (async () => {
        try {
          const since = ++listsTick;
          // The card stream reads the gallery of the tab shown (the grid, its own filters).
          const { grid, tab } = get();
          const filter = grid || get().filter.kind === tab ? get().filter : { ...get().filter, kind: tab };
          if (filter !== get().filter) set({ filter });
          // The newest runs whatever the filter: the gallery's first page when it has none.
          const all = unfiltered(filter);
          const [models, local, page, newest, live, prefs, usage] = await Promise.all([
            api.studioModels(),
            api.studioLocalModels(),
            api.studioRuns(filter, null, RUNS_PAGE),
            all ? null : api.studioRuns({}, null, RECENT_RUNS),
            api.studioLive(),
            api.studioGetPrefs(),
            api.studioUsage(),
          ]);
          set((s) => ({
            endpoints: newestLists(models.endpoints, since),
            local,
            prefs,
            usage,
            loaded: true,
            // Runs heard of while reading stay.
            recentRuns: newestRuns([...(newest ?? page).runs, ...s.recentRuns]),
          }));
          // A page for a filter changed since is old (`setFilter` read the new one).
          if (get().filter === filter) set({ runs: page.runs, next: page.next });
          for (const run of live) get().applyProgress(run);
          fillDrafts();
        } finally {
          loading = null;
        }
      })();
      return loading;
    },
    reread: async () => {
      const since = ++listsTick;
      const [models, local, prefs, usage] = await Promise.all([
        api.studioModels(),
        api.studioLocalModels(),
        api.studioGetPrefs(),
        api.studioUsage(),
      ]);
      set({ endpoints: newestLists(models.endpoints, since), local, prefs, usage });
      forgetFits();
      fillDrafts();
    },
    refreshModels: async (providerId) => {
      const since = ++listsTick;
      const { endpoints } = await api.studioRefreshModels(providerId);
      set({ endpoints: newestLists(endpoints, since, providerId) });
      fetchedTick.set(providerId, ++listsTick);
      fillDrafts();
    },
    refreshLocal: async () => {
      const local = await api.studioLocalModels();
      set({ local });
      forgetFits();
      fillDrafts();
    },
    setTab: (tab) => set({ tab }),
    selectInitialModel: (tab) => {
      // The draft starts again: nothing is held aside for a model it no longer has.
      setHeld(tab, null);
      set((s) => {
        const offered = (c: { target: RunTarget; model: string }) =>
          offeredModel(tab, c.target, c.model, s.local, s.endpoints);
        const last = s.prefs?.lastModel[tab];
        const choice = last && offered(last) ? last : defaultModel(tab, s.local, s.endpoints);
        const model = choice ? offered(choice) : null;
        const draft: Draft = {
          ...EMPTY_DRAFT,
          prompt: s.drafts[tab].prompt,
          target: choice?.target ?? null,
          model: choice?.model ?? null,
          values: model ? defaultsFor(model) : {},
        };
        return {
          drafts: { ...s.drafts, [tab]: draft },
          priceError: { ...s.priceError, [tab]: null },
          refusal: { ...s.refusal, [tab]: null },
        };
      });
    },
    chooseModel: (tab, target, modelId) => {
      const s = get();
      const to = offeredModel(tab, target, modelId, s.local, s.endpoints);
      if (!to) return;
      const endpoint = target === "thisComputer" ? null : s.endpoints.find((e) => e.providerId === target.endpoint);
      const unpriced = endpoint ? priceNotAsked(to, endpoint.company) : false;
      const draft = s.drafts[tab];
      if (draft.target !== null && sameTarget(draft.target, target) && draft.model === to.id) {
        // Picked again: the draft stays as it is (a failure is kept in `priceError`).
        if (unpriced && endpoint) priceFor(tab, endpoint.providerId, to.id).catch(() => undefined);
        return;
      }
      // Extra settings held aside for the model picked before are carried to it as listed.
      const waiting = held[tab];
      const dropped = waiting ? carry(waiting.model, waiting.model, {}, waiting.extra, []) : null;
      if (waiting) setHeld(tab, null);
      const extra = { ...draft.extra, ...dropped?.extra };
      const from = draft.target && draft.model ? listedModel(draft.target, draft.model, s.local, s.endpoints) : null;
      const hold = mustHold(to, unpriced, extra);
      const carried = carry(from, to, draft.values, hold ? {} : extra, draft.refs);
      const note = [dropped?.note ?? null, carried.note].filter((n) => n !== null).join(" ") || null;
      set((st) => ({
        drafts: {
          ...st.drafts,
          [tab]: {
            ...draft,
            target,
            model: to.id,
            values: dropUnmet(to, carried.values),
            extra: carried.extra,
            refs: carried.refs,
            // Which companies may run it belongs to the model it was set for.
            routing: null,
            note,
          },
        },
        // A failed price request, or a refused press, was for the model before.
        priceError: { ...st.priceError, [tab]: null },
        refusal: { ...st.refusal, [tab]: null },
      }));
      if (unpriced && endpoint) askPriceHolding(tab, target, endpoint, to, hold ? extra : null);
    },
    fetchPrice: async (tab) => {
      const { target, model } = get().drafts[tab];
      if (target === null || target === "thisComputer" || model === null) return;
      // A failure is kept in `priceError` (`priceFor`); extra settings held stay held.
      const priced = await priceFor(tab, target.endpoint, model).catch(() => null);
      const waiting = held[tab];
      if (priced && waiting && sameTarget(waiting.target, target) && waiting.model.id === model) {
        settleHeld(tab, waiting, priced);
      }
    },
    priceError: { image: null, video: null },
    holding: { image: false, video: false },
    dismissPriceError: (tab) => set((s) => ({ priceError: { ...s.priceError, [tab]: null } })),
    setPrompt: (tab, text) => set((s) => ({ drafts: { ...s.drafts, [tab]: { ...s.drafts[tab], prompt: text } } })),
    setValue: (tab, key, value) =>
      set((s) => {
        const draft = s.drafts[tab];
        const model = draftModel(s, tab);
        let values: Values = { ...draft.values, [key]: value };
        let note = draft.note;
        const field = model?.params.find((p) => p.key === key);
        if (model && field?.canonical === "background" && value === "transparent") {
          const moved = transparentFormat(model, values);
          if (moved) {
            values = moved.values;
            note = STUDIO_FIELDS.transparentNote(moved.label);
          }
        }
        if (model) values = dropUnmet(model, values);
        return { drafts: { ...s.drafts, [tab]: { ...draft, values, note } } };
      }),
    resetValue: (tab, key) =>
      set((s) => {
        const draft = s.drafts[tab];
        const model = draftModel(s, tab);
        const values = Object.fromEntries(Object.entries(draft.values).filter(([k]) => k !== key));
        return { drafts: { ...s.drafts, [tab]: { ...draft, values: model ? dropUnmet(model, values) : values } } };
      }),
    addRef: (tab, ref) => {
      const s = get();
      const draft = s.drafts[tab];
      const placed = placeRef(draftModel(s, tab), draft.refs, ref.role);
      if ("refused" in placed) return placed.refused;
      const refs = [...draft.refs, { ...ref, role: placed.role }];
      set((st) => ({ drafts: { ...st.drafts, [tab]: { ...st.drafts[tab], refs } } }));
      return null;
    },
    removeRef: (tab, index) =>
      set((s) => {
        const draft = s.drafts[tab];
        return { drafts: { ...s.drafts, [tab]: { ...draft, refs: draft.refs.filter((_, i) => i !== index) } } };
      }),
    setRefRole: (tab, index, role) => {
      const s = get();
      const draft = s.drafts[tab];
      const ref = draft.refs[index];
      if (!ref || (ref.role ?? "reference") === role) return null;
      const placed = placeRef(draftModel(s, tab), draft.refs.filter((_, i) => i !== index), role);
      if ("refused" in placed) return placed.refused;
      const refs = draft.refs.map((r, i) => (i === index ? { ...r, role } : r));
      set((st) => ({ drafts: { ...st.drafts, [tab]: { ...st.drafts[tab], refs } } }));
      return null;
    },
    dismissNote: (tab) => set((s) => ({ drafts: { ...s.drafts, [tab]: { ...s.drafts[tab], note: null } } })),
    invalid: { image: {}, video: {} },
    setInvalid: (tab, key, problem) =>
      set((s) => {
        const now = s.invalid[tab];
        if ((now[key] ?? null) === problem) return {};
        const next = Object.fromEntries(Object.entries(now).filter(([k]) => k !== key));
        return { invalid: { ...s.invalid, [tab]: problem === null ? next : { ...next, [key]: problem } } };
      }),
    resetAdvanced: (tab) => {
      // No Extra fields: nothing is held aside for them either.
      setHeld(tab, null);
      set((s) => {
        const draft = s.drafts[tab];
        const model = draftModel(s, tab);
        const advanced = new Set(model?.params.filter((p) => p.layer === "advanced").map((p) => p.key));
        const kept = Object.fromEntries(Object.entries(draft.values).filter(([k]) => !advanced.has(k)));
        const values = model ? dropUnmet(model, kept) : kept;
        return { drafts: { ...s.drafts, [tab]: { ...draft, values, extra: {}, routing: null, seedLocked: false } } };
      });
    },
    newSeed: (tab) => {
      const field = draftModel(get(), tab)?.params.find((p) => p.kind.type === "seed");
      if (!field || field.kind.type !== "seed") return;
      const now = get().drafts[tab].values[field.key];
      let seed = randomSeed(field.kind.max);
      while (seed === now && field.kind.max > 0) seed = randomSeed(field.kind.max);
      get().setValue(tab, field.key, seed);
    },
    lockSeed: (tab, locked) =>
      set((s) => ({ drafts: { ...s.drafts, [tab]: { ...s.drafts[tab], seedLocked: locked } } })),
    setExtra: (tab, extra) => set((s) => ({ drafts: { ...s.drafts, [tab]: { ...s.drafts[tab], extra } } })),
    setRouting: (tab, routing) => set((s) => ({ drafts: { ...s.drafts, [tab]: { ...s.drafts[tab], routing } } })),
    saveSetup: async (tab, name) => {
      const s = get();
      const draft = s.drafts[tab];
      const named = name.trim();
      if (named === "") return STUDIO_SETUPS.noName;
      if (draft.target === null || draft.model === null) return STUDIO_SETUPS.noModel;
      const setup: Setup = {
        name: named,
        kind: tab,
        target: draft.target,
        model: draft.model,
        params: draft.values,
        extra: draft.extra,
        routing: draft.routing,
      };
      // The setups saved so far (read first if Studio has not read them yet, so none is lost).
      const { setups } = s.prefs ?? (await api.studioGetPrefs());
      const same = setups.findIndex((x) => x.kind === tab && x.name === named);
      const next = same < 0 ? [...setups, setup] : setups.map((x, i) => (i === same ? setup : x));
      if (next.length > MAX_SETUPS) return STUDIO_SETUPS.tooMany;
      await get().setPrefs({ setups: next });
      return null;
    },
    applySetup: (setup) => {
      const draft = get().drafts[setup.kind];
      // The prompt stays; the added pictures stay up to the model's limit.
      return fillDraft(setup.kind, setup, draft.refs, draft.prompt, STUDIO_SETUPS);
    },
    localFits: {},
    askLocalFit: (modelId, width, height, seconds) => {
      const key = localFitKey(modelId, width, height, seconds);
      const mine = fitting;
      if (key in get().localFits || mine.has(key) || fitFailed.has(key)) return;
      mine.add(key);
      Promise.resolve()
        .then(() => api.studioLocalFit({ installed: { genModelId: modelId } }, width, height, seconds))
        .then(
          (fit) => {
            if (mine === fitting) set((s) => ({ localFits: { ...s.localFits, [key]: fit } }));
          },
          (e: unknown) => {
            // The chip shows no time, and it is not asked again until the local models are
            // read again; the failure leaves a trace.
            if (mine === fitting) fitFailed.add(key);
            console.warn("Studio: how long a local run takes could not be worked out", e);
          },
        )
        .finally(() => mine.delete(key));
    },
    setFilter: async (patch) => {
      const before = get().filter;
      const filter = { ...before, ...patch };
      set({ filter });
      let page: Awaited<ReturnType<typeof api.studioRuns>>;
      try {
        page = await api.studioRuns(filter, null, RUNS_PAGE);
      } catch (e) {
        // The runs held are still the filter before's: so are the controls, and Load more.
        if (get().filter === filter) set({ filter: before });
        throw e;
      }
      // An answer for a filter changed since is old.
      if (get().filter === filter) set({ runs: page.runs, next: page.next });
    },
    loadMore: () => {
      if (paging) return paging;
      const { filter, next } = get();
      if (next === null) return Promise.resolve();
      const read = (async () => {
        const page = await api.studioRuns(filter, next, RUNS_PAGE);
        // A page for a filter changed since (its first page read again) is old.
        if (get().filter !== filter || get().next !== next) return;
        set((s) => {
          const known = new Set(s.runs.map((r) => r.id));
          return { runs: [...s.runs, ...page.runs.filter((r) => !known.has(r.id))], next: page.next };
        });
      })();
      paging = read;
      // Put away once this read is over (however it ends), never before it was kept.
      const over = () => {
        if (paging === read) paging = null;
      };
      read.then(over, over);
      return read;
    },
    setGrid: async (grid) => {
      set({ grid });
      const { kind, since } = get().filter;
      const tab = get().tab;
      // The grid opens on All; the cards read the tab's own gallery, at any date.
      if (grid && kind) await get().setFilter({ kind: null });
      if (!grid && (kind !== tab || since != null)) await get().setFilter({ kind: tab, since: null });
    },
    setPrefs: async (patch) => {
      const prefs = await api.studioSetPrefs(patch);
      set({ prefs });
    },

    ...createRun(set, get, rereadUsage),
    ...createResults(set, get, { fillDraft, rereadUsage }),
    ...createLocal(set, get),
    pickerOpen: null,
    openPicker: (place) => set({ pickerOpen: place }),
  };
});

// ---- the models a tab can use, and its empty state ----------------------------------------

/** Which kinds each company makes (OpenAI's video API shut down in September 2026). */
const MAKES: Record<Company, readonly MediaKind[]> = {
  openrouter: ["image", "video"],
  openai: ["image"],
  local: ["image", "video"],
};

/** The fit words Studio counts as fitting: fits, and fits after the chat model is unloaded. */
export const FITTING: ReadonlySet<string> = new Set(["fits", "afterUnload"]);

/** The cloud models Studio picks first, when the endpoint offers them. */
const PREFERRED: Record<MediaKind, readonly string[]> = {
  image: ["gpt-image-2.6-flare", "openai/gpt-image-2.6-flare"],
  video: ["google/veo-3.1-fast"],
};

/** `local` when this computer can make `tab`'s kind at all (video needs a usable graphics
 *  card or an Apple-silicon Mac, and is hidden otherwise); `null` too until it is read. */
function localFor(tab: StudioTab, local: LocalModels | null): LocalModels | null {
  return local && (tab === "image" || !local.videoOnProcessor) ? local : null;
}

/** The installed local models `tab` can use: this version can run them, and this computer
 *  can make the kind. */
export function localModelsFor(tab: StudioTab, local: LocalModels | null): InstalledLocal[] {
  return localFor(tab, local)?.installed.filter((m) => m.kind === tab && m.model.usable === null) ?? [];
}

/** An "Auto" model, which picks a model for you: Studio never offers one, as it would hide
 *  where the prompt goes (`openrouter/auto`, or a model named "Auto"). */
export function isAutoModel(model: MediaModel): boolean {
  return /(^|\/)auto$/i.test(model.id) || /^auto( |$)/i.test(model.displayName);
}

/** An OpenRouter picture model whose price was not asked for yet: it is asked when the model is
 *  picked, or on *Get the price* (its lists give no price for pictures). */
export function priceNotAsked(model: MediaModel, company: Company): boolean {
  return company === "openrouter" && model.kind === "image" && model.priceFetchedAt === null;
}

/** Whether `extra` waits for model `to`'s price answer before it goes into a draft: the model's
 *  price is about to be asked (`unpriced`), and as listed it has no Extra field to check the
 *  names against (the answer brings it). */
function mustHold(to: MediaModel, unpriced: boolean, extra: Values): boolean {
  return unpriced && !to.params.some((p) => p.kind.type === "extra") && Object.keys(extra).length > 0;
}

/** The models of `endpoint`'s saved lists that `tab` can use: none while it has no key (the
 *  picker greys them out). */
export function cloudModelsFor(endpoint: StudioEndpoint, tab: StudioTab): MediaModel[] {
  if (!endpoint.hasKey) return [];
  return endpoint.models.filter((m) => m.kind === tab && m.usable === null && !isAutoModel(m));
}

/** The first recommended entry of `tab`'s kind that fits this computer. */
export function fittingEntry(tab: StudioTab, local: LocalModels | null): LocalEntry | null {
  return localFor(tab, local)?.recommended.find((e) => e.kind === tab && FITTING.has(e.fit.word)) ?? null;
}

/** Whether `endpoint`'s company makes `tab`'s kind, with or without its lists saved. */
export function makesKind(endpoint: StudioEndpoint, tab: StudioTab): boolean {
  return MAKES[endpoint.company].includes(tab);
}

/** Whether no local model this version can run is installed (of any kind): the first-run
 *  suggestion shows only then. */
export function nothingInstalled(local: LocalModels | null): boolean {
  return !local?.installed.some((m) => m.model.usable === null);
}

/** Whether two targets are the same place: this computer, or one endpoint. */
export function sameTarget(a: RunTarget, b: RunTarget): boolean {
  return a === "thisComputer" || b === "thisComputer" ? a === b : a.endpoint === b.endpoint;
}

/** The model `id` at `target` as listed, whether or not it can be used now. */
export function listedModel(
  target: RunTarget,
  id: string,
  local: LocalModels | null,
  endpoints: StudioEndpoint[],
): MediaModel | null {
  if (target === "thisComputer") return local?.installed.find((m) => m.model.id === id)?.model ?? null;
  return endpoints.find((e) => e.providerId === target.endpoint)?.models.find((m) => m.id === id) ?? null;
}

/** The saved endpoint `target` names; `null` for this computer, without a target, or once the
 *  endpoint was removed in Settings. */
export function endpointOf(target: RunTarget | null, endpoints: StudioEndpoint[]): StudioEndpoint | null {
  if (target === null || target === "thisComputer") return null;
  return endpoints.find((e) => e.providerId === target.endpoint) ?? null;
}

/** The model of `tab`'s draft as listed, whether or not it can be used now; `null` without one. */
export function draftModel(
  s: Pick<StudioState, "drafts" | "local" | "endpoints">,
  tab: StudioTab,
): MediaModel | null {
  const { target, model } = s.drafts[tab];
  return target !== null && model !== null ? listedModel(target, model, s.local, s.endpoints) : null;
}

/** The model `id` at `target`, when `tab` can still use it. */
export function offeredModel(
  tab: StudioTab,
  target: RunTarget,
  id: string,
  local: LocalModels | null,
  endpoints: StudioEndpoint[],
): MediaModel | null {
  if (target === "thisComputer") return localModelsFor(tab, local).find((m) => m.model.id === id)?.model ?? null;
  const endpoint = endpoints.find((e) => e.providerId === target.endpoint);
  return (endpoint && cloudModelsFor(endpoint, tab).find((m) => m.id === id)) ?? null;
}

/** The model `tab` starts with when none was used last: an installed local model that fits
 *  (not one that is slow here), else the first usable model of the first endpoint with one
 *  (and a key), its own GPT Image 2.6 Flare or Veo 3.1 Fast when offered; `null` when there
 *  is none. Never an "Auto" model, which would hide where the prompt goes. */
export function defaultModel(
  tab: StudioTab,
  local: LocalModels | null,
  endpoints: StudioEndpoint[],
): { target: RunTarget; model: string } | null {
  const installed = localModelsFor(tab, local).find((m) => FITTING.has(m.fit.word));
  if (installed) return { target: "thisComputer", model: installed.model.id };
  for (const endpoint of endpoints) {
    const models = cloudModelsFor(endpoint, tab);
    const preferred = PREFERRED[tab].map((id) => models.find((m) => m.id === id)).find((m) => m !== undefined);
    const model = preferred ?? models[0];
    if (model) return { target: { endpoint: endpoint.providerId }, model: model.id };
  }
  return null;
}

/**
 * What `tab` shows in place of the card stream, by one ordered rule, the first that applies:
 * nothing when the tab has a model to use (an installed local model of its kind, or an
 * endpoint's, which needs a key); *Get local models* when a recommended entry of its kind
 * fits this computer (after the chat model is unloaded counts; video also needs a computer
 * that can make it); cloud as the backup when no endpoint is saved; nothing while an
 * endpoint that makes the kind has no saved list (its *Get the list* stands in); else *add
 * an endpoint*: the saved ones make no such kind (the Video tab with only OpenAI), or offer
 * none Studio can use (no key, or no such model in their lists), and the card says which.
 */
export function emptyState(tab: StudioTab, local: LocalModels | null, endpoints: StudioEndpoint[]): EmptyState {
  if (localModelsFor(tab, local).length > 0 || endpoints.some((e) => cloudModelsFor(e, tab).length > 0)) return null;
  if (fittingEntry(tab, local)) return "getLocal";
  if (endpoints.length === 0) return "cloudBackup";
  if (endpoints.some((e) => e.fetchedAt === null && makesKind(e, tab))) return null;
  return "addEndpoint";
}

/** The models of the newest runs of `kind`, each once, newest first: at most `n`. */
export function recentModels(runs: Run[], kind: StudioTab, n = 5): string[] {
  const models: string[] = [];
  for (const run of [...runs].sort((a, b) => b.createdAt - a.createdAt)) {
    if (models.length === n) break;
    if (run.kind === kind && !models.includes(run.model)) models.push(run.model);
  }
  return models;
}

/** The models of the newest runs of `kind`, each with where it ran (this computer, or the
 *  endpoint it was sent to), once per place and model, newest first: at most `n`. */
export function recentChoices(runs: Run[], kind: StudioTab, n = 5): ModelChoice[] {
  const choices: ModelChoice[] = [];
  for (const run of [...runs].sort((a, b) => b.createdAt - a.createdAt)) {
    if (choices.length === n) break;
    const target = targetOf(run);
    if (run.kind !== kind || target === null) continue;
    if (!choices.some((c) => c.model === run.model && sameTarget(c.target, target))) choices.push({ target, model: run.model });
  }
  return choices;
}

/** The states in which a run works whether or not Studio watches it. */
const WORKING: ReadonlySet<RunStatus> = new Set<RunStatus>(["queued", "sending", "running", "downloading"]);

/** The runs working now: queued, sending, running, downloading, and waiting while watched
 *  (a video no longer checked is not). */
export function runningCount(s: StudioState): number {
  return Object.values(s.live).filter((r) => WORKING.has(r.status) || (r.status === "waiting" && r.watching))
    .length;
}

/** Follows the runs: each `studio-progress` and, once listening has started, the runs still
 *  working now (a window that reloaded heard nothing of them). What an event says is newer
 *  than that answer. Returns the function that stops following; outside the app it follows
 *  nothing. */
export function followStudio(): () => void {
  if (!ipc.isTauri()) return () => undefined;
  const heard = new Set<string>();
  let stopped = false;
  const stopListening = listenInEffect(async () => {
    const unlisten = await api.onStudioProgress((run) => {
      if (stopped) return;
      heard.add(run.id);
      useStudio.getState().applyProgress(run);
    });
    if (!stopped) {
      Promise.resolve()
        .then(() => api.studioLive())
        .then((runs) => {
          if (stopped) return;
          for (const run of runs) if (!heard.has(run.id)) useStudio.getState().applyProgress(run);
        })
        // `studio-progress` still follows them; the failure leaves a trace.
        .catch((e: unknown) => console.warn("Studio: the runs still working could not be read", e));
    }
    return unlisten;
  });
  return () => {
    stopped = true;
    stopListening();
  };
}

// ---- settings and added pictures ----------------------------------------------------------

/** The file types a transparent background can have, in the order Studio picks them. */
const TRANSPARENT_TYPES = ["png", "webp"];

/** For a transparent background: `values` with the File type field moved to PNG (else WebP)
 *  when it is set, or defaults, to a type that cannot be transparent, and that type's name;
 *  `null` when nothing has to move (or the model offers neither). */
function transparentFormat(model: MediaModel, values: Values): { values: Values; label: string } | null {
  const field = model.params.find((p) => p.canonical === "format");
  if (!field || field.kind.type !== "enum") return null;
  const current = values[field.key] ?? field.companyDefault ?? null;
  if (current === null || TRANSPARENT_TYPES.includes(String(current).toLowerCase())) return null;
  const choices = field.kind.values;
  const to = TRANSPARENT_TYPES.map((t) => choices.find((c) => String(c.value).toLowerCase() === t)).find(
    (c) => c !== undefined,
  );
  return to ? { values: { ...values, [field.key]: to.value }, label: to.label } : null;
}

/** The roles of the model's picture fields, in its order, with how many each takes. */
export function pictureRoles(model: MediaModel | null): { role: RefRole; max: number }[] {
  return model?.params.flatMap((p) => (p.kind.type === "images" ? [{ role: p.kind.role, max: p.kind.max }] : [])) ?? [];
}

/** Where a picture goes among `refs`: as `role`, or without one as the first of the model's
 *  roles with room; else why it cannot be added (the most the model takes of that role, or of
 *  all of them together). A picture recorded without a role counts as a reference. */
export function placeRef(
  model: MediaModel | null,
  refs: RefItem[],
  role: RefRole | null,
): { role: RefRole } | { refused: string } {
  const roles = pictureRoles(model);
  const count = (r: RefRole) => refs.filter((ref) => (ref.role ?? "reference") === r).length;
  if (role !== null) {
    const max = roles.find((r) => r.role === role)?.max ?? 0;
    return count(role) < max ? { role } : { refused: STUDIO_REFS.takes(max, role) };
  }
  const open = roles.find((r) => count(r.role) < r.max);
  if (open) return { role: open.role };
  return { refused: STUDIO_REFS.takes(roles.reduce((n, r) => n + r.max, 0), null) };
}

/** Where a local fit is kept in `localFits`: the model, the size and the clip's length. */
export function localFitKey(modelId: string, width: number, height: number, seconds: number): string {
  return `${modelId}|${width}|${height}|${seconds}`;
}
