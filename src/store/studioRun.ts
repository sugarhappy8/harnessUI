// Studio's estimate and Create (0.1.0 Task B7), composed into `useStudio` (src/store/studio.ts,
// which re-exports what other modules use from here; import it from there): each tab's
// estimate, asked once the draft has stayed as it is for 150 ms; Create, one press with its own
// client token at a time (from the draft, or from a run's own settings), the confirmation a
// cloud run may need (asked by the draft's estimate or by the backend), why a press was
// refused, a seed the draft's press used given back to Random unless it is kept, the model used
// last saved as a run starts, and today's spending read again as a cloud run starts. Also what
// the backend would refuse in the prompt itself, judged on it as it is, apart from the rest of
// what an estimate says.
import type { StoreApi } from "zustand";
import * as api from "../components/studio/api";
import type { Estimate, MediaModel, Run, RunTarget, StartRequest } from "../components/studio/types";
import type { ErrorInfo } from "../lib/types";
import { STUDIO_DOCK, STUDIO_REUSE } from "../lib/words";
import { toErrorInfo } from "./app";
import { draftModel, listedModel, sameTarget, type Draft, type StudioState, type StudioTab } from "./studio";

/** The part of Studio's state this module keeps. */
export interface StudioRunState {
  /** Each tab's latest estimate, with the draft it was asked for; `null` until one is asked,
   *  or after one was asked while the tab had no model. */
  estimates: Record<StudioTab, TabEstimate | null>;
  /** Asks what `tab`'s draft would cost and meet once it has stayed as it is for
   *  `ESTIMATE_DELAY_MS` (each call starts that pause again). Resolves once that answer is in
   *  `estimates` (a failure too, with the draft it was for); never rejects. Sends nothing
   *  anywhere. */
  estimate(tab: StudioTab): Promise<void>;
  /** A press of Create on its way: from the press until the backend answers it, and while its
   *  confirmation is open. Another press meanwhile does nothing. */
  starting: boolean;
  /** That press's request, as it will be sent (the confirmation shows it). */
  pending: StartRequest | null;
  /** The estimate the confirmation shows for `pending`; `null` while it is not open. */
  pendingEstimate: Estimate | null;
  /** Why each tab's last press was refused, shown under the prompt until the next press. */
  refusal: Record<StudioTab, ErrorInfo | null>;
  dismissRefusal(tab: StudioTab): void;
  /**
   * Create: one press, with its own client token, of `tab`'s draft or, given `from`, of that
   * run's own settings where it ran (always with a new seed: the draft's Keep it is for Create;
   * the draft stays as it is; a run whose endpoint was removed is refused, `refusal`). Once the draft's press is accepted, its seed goes back to Random unless it is
   * kept, so the next press makes a new picture. A draft for the cloud whose estimate for it,
   * as it is, asks for a confirmation opens the confirmation instead; so does the backend's
   * `studio_needs_confirmation`, with the estimate asked for that press. A run on this computer
   * never asks. `confirmed` sends it as confirmed. Resolves with the run; `null` when there is
   * nothing to send, a press is already on its way, the confirmation opened, or it was refused
   * (`refusal`). Never rejects.
   */
  start(tab: StudioTab, confirmed: boolean, from?: Run): Promise<Run | null>;
  /** The confirmation's *Make video* or *Create*: sends `pending` as it is, confirmed, once
   *  "don't ask again for videos" is saved when `dontAskVideos` (nothing is sent when that
   *  fails: `refusal`). */
  confirmStart(dontAskVideos: boolean): Promise<Run | null>;
  /** The confirmation's *Cancel*: nothing is sent. */
  cancelStart(): void;
}

/** A tab's latest estimate, with the draft it was asked for. */
export interface TabEstimate {
  /** That draft's request, as `requestKey` writes it. */
  key: string;
  target: RunTarget;
  model: string;
  estimate: Estimate | null;
  /** Why it could not be worked out, when it could not. */
  error: ErrorInfo | null;
}

/** How long a draft stays as it is before its estimate is asked. */
export const ESTIMATE_DELAY_MS = 150;

/** The client token an estimate is asked with (each press makes its own). */
const ESTIMATE_TOKEN = "estimate";

/** Why Again, or Run again, cannot send a run whose endpoint was removed in Settings. */
function endpointGone(): ErrorInfo {
  return { code: "studio_endpoint_gone", message: STUDIO_REUSE.endpointGone, nextStep: STUDIO_REUSE.endpointGoneNext, details: "" };
}

/** The backend's refusal of a cloud run that needs the user's confirmation first. */
const NEEDS_CONFIRMATION = "studio_needs_confirmation";

/** Studio's estimate and Create, on the store `set` and `get` work on. `rereadUsage` reads
 *  today's spending again (the top bar and the daily limit follow it). Builds nothing but the
 *  state and its closures, so it can run while the store is being made. */
export function createRun(
  set: StoreApi<StudioState>["setState"],
  get: StoreApi<StudioState>["getState"],
  rereadUsage: () => void,
): StudioRunState {
  /** Each tab's estimate while its pause runs, with the callers waiting for its answer. */
  const estimateWaits: Partial<Record<StudioTab, { timer: ReturnType<typeof setTimeout>; done: (() => void)[] }>> =
    {};
  /** Counts each tab's estimates as they are asked, so an older answer is not kept over a newer. */
  const estimateAsked: Record<StudioTab, number> = { image: 0, video: 0 };

  /** Asks the estimate of `tab`'s draft as it is now and keeps the answer (or why there is
   *  none) with that draft, unless a later one was asked meanwhile. */
  const askEstimate = async (tab: StudioTab) => {
    const mine = ++estimateAsked[tab];
    const req = startRequestOf(tab, get().drafts[tab], ESTIMATE_TOKEN);
    let known: TabEstimate | null = null;
    if (req) {
      const asked = { key: requestKey(req), target: req.target, model: req.model };
      try {
        known = { ...asked, estimate: await api.studioEstimate(req), error: null };
      } catch (e) {
        known = { ...asked, estimate: null, error: toErrorInfo(e) };
      }
    }
    if (mine === estimateAsked[tab]) set((s) => ({ estimates: { ...s.estimates, [tab]: known } }));
  };

  /** Ends a press that was refused (or whose confirmation could not be asked): `tab` shows why. */
  const refuse = (tab: StudioTab, error: ErrorInfo) =>
    set((s) => ({ starting: false, pending: null, pendingEstimate: null, refusal: { ...s.refusal, [tab]: error } }));

  /** Keeps `target` and `model` as the model `tab` used last, sending only that field and only
   *  when it changed. A failure leaves a trace; the run has started all the same. */
  const rememberModel = (tab: StudioTab, target: RunTarget, model: string) => {
    const last = get().prefs?.lastModel[tab];
    if (last && sameTarget(last.target, target) && last.model === model) return;
    Promise.resolve()
      .then(async () => {
        // The other tab's is kept (read first if Studio has not read the preferences yet).
        const { lastModel } = get().prefs ?? (await api.studioGetPrefs());
        await get().setPrefs({ lastModel: { ...lastModel, [tab]: { target, model } } });
      })
      .catch((e: unknown) => console.warn("Studio: the model used last could not be saved", e));
  };

  /** The press of the draft itself (not of a run) on its way, if any: once it is accepted, a seed
   *  the dice gave it is used up. */
  let draftPress: StartRequest | null = null;

  /** After `tab`'s draft was sent as `req` and accepted: a seed it held that is not kept goes back
   *  to Random, so the next press gets a new one (a seed set since the press stays). */
  const spendSeed = (tab: StudioTab, req: StartRequest) => {
    const s = get();
    const draft = s.drafts[tab];
    if (draft.seedLocked || draft.target === null || !sameTarget(draft.target, req.target)) return;
    if (draft.model !== req.model) return;
    const key = seedKey(draftModel(s, tab));
    if (key in draft.values && draft.values[key] === req.params[key]) get().resetValue(tab, key);
  };

  /** Sends press `req` (`confirmed` when the user confirmed it). Accepted, the run joins the live
   *  runs, a seed the draft's press used is used up, its model becomes the one used last and,
   *  for a cloud run, today's spending is read again. The backend's `studio_needs_confirmation`
   *  opens the confirmation with the estimate for this very press, which stays pending; any
   *  other refusal ends the press with why. */
  const send = async (req: StartRequest, confirmed: boolean): Promise<Run | null> => {
    const tab = req.kind;
    let run: Run;
    try {
      run = await api.studioStart(req, confirmed);
    } catch (e) {
      const error = toErrorInfo(e);
      if (error.code !== NEEDS_CONFIRMATION || confirmed) {
        refuse(tab, error);
        return null;
      }
      try {
        set({ pendingEstimate: await api.studioEstimate(req) });
      } catch (asked) {
        refuse(tab, toErrorInfo(asked));
      }
      return null;
    }
    set({ starting: false, pending: null, pendingEstimate: null });
    get().applyProgress(run);
    if (req === draftPress) {
      draftPress = null;
      spendSeed(tab, req);
    }
    rememberModel(tab, req.target, req.model);
    if (run.runsOn === "cloud") rereadUsage();
    return run;
  };

  return {
    estimates: { image: null, video: null },
    estimate: (tab) =>
      new Promise<void>((resolve) => {
        const waiting = estimateWaits[tab];
        if (waiting) clearTimeout(waiting.timer);
        const done = [...(waiting?.done ?? []), resolve];
        const timer = setTimeout(() => {
          delete estimateWaits[tab];
          void askEstimate(tab).finally(() => {
            for (const answered of done) answered();
          });
        }, ESTIMATE_DELAY_MS);
        estimateWaits[tab] = { timer, done };
      }),
    starting: false,
    pending: null,
    pendingEstimate: null,
    refusal: { image: null, video: null },
    dismissRefusal: (tab) => set((s) => ({ refusal: { ...s.refusal, [tab]: null } })),
    start: async (tab, confirmed, from) => {
      const s = get();
      if (s.starting) return null;
      const token = crypto.randomUUID();
      let req: StartRequest | null;
      if (from) {
        const target = targetOf(from);
        // Again is the run's own settings with a new seed, always: the draft's Keep it is for
        // Create (the run may not even have the seed the draft keeps).
        const model = target ? listedModel(target, from.model, s.local, s.endpoints) : null;
        req = requestFromRun(from, token, model, false);
        if (!req) {
          // Where it ran is no longer known (its endpoint was removed): say so, as Reuse does.
          set((st) => ({ refusal: { ...st.refusal, [from.kind]: endpointGone() } }));
          return null;
        }
      } else {
        req = startRequestOf(tab, s.drafts[tab], token);
      }
      if (!req) return null;
      draftPress = from ? null : req;
      const kind = req.kind;
      set((st) => ({ starting: true, pending: req, refusal: { ...st.refusal, [kind]: null } }));
      // A draft for the cloud whose estimate, for it as it is, asks first opens the confirmation.
      // A press from a run goes straight to the backend, which refuses a cloud run that needs
      // confirming before anything is recorded; a run on this computer never asks.
      const asks = !from && !confirmed && req.target !== "thisComputer" ? currentEstimate(s, tab) : null;
      if (asks && asks.reasons.length > 0) {
        set({ pendingEstimate: asks });
        return null;
      }
      return send(req, confirmed);
    },
    confirmStart: async (dontAskVideos) => {
      const { pending: req, pendingEstimate } = get();
      if (!req || !pendingEstimate) return null;
      set({ pendingEstimate: null });
      if (dontAskVideos) {
        try {
          await get().setPrefs({ confirmEveryVideo: false });
        } catch (e) {
          refuse(req.kind, toErrorInfo(e));
          return null;
        }
      }
      return send(req, true);
    },
    cancelStart: () => set({ starting: false, pending: null, pendingEstimate: null }),
  };
}

/** What Create would send for `tab`'s draft (`studio_estimate` and `studio_preview` take the
 *  same), as press `clientToken`; `null` while the draft has no model. An added picture recorded
 *  without a role goes as a reference. */
export function startRequestOf(tab: StudioTab, draft: Draft, clientToken: string): StartRequest | null {
  if (draft.target === null || draft.model === null) return null;
  return {
    clientToken,
    target: draft.target,
    kind: tab,
    model: draft.model,
    prompt: draft.prompt,
    params: draft.values,
    extra: draft.extra,
    refs: draft.refs.map((r) => ({ sha256: r.sha256, role: r.role ?? "reference" })),
    routing: draft.routing,
  };
}

/** The most characters a prompt may have, as the backend counts them: on this computer
 *  (`runtime::sd::args::PROMPT_MAX_CHARS`) and for the cloud (`media::spec`'s). */
export const PROMPT_MAX_CHARS = { thisComputer: 4_000, cloud: 32_000 } as const;

/** The backend's sentences about the prompt itself, among an estimate's `invalid`. */
const PROMPT_SENTENCES: ReadonlySet<string> = new Set([
  STUDIO_DOCK.writePrompt,
  STUDIO_DOCK.promptTooLong,
  STUDIO_DOCK.promptTooLongHere,
]);

/** Nothing but white space, as the backend's `trim` has it (Unicode's White_Space). */
const BLANK = /^\p{White_Space}*$/u;

/** What the backend would refuse in `prompt` for a run at `target`, in its words: an empty
 *  prompt, or one longer than the cap there, counted as it counts (characters, not UTF-16
 *  units). Judged on the prompt as it is, so Create never waits for an estimate to say it. */
export function promptProblems(prompt: string, target: RunTarget): string[] {
  if (BLANK.test(prompt)) return [STUDIO_DOCK.writePrompt];
  const here = target === "thisComputer";
  const max = here ? PROMPT_MAX_CHARS.thisComputer : PROMPT_MAX_CHARS.cloud;
  // Never more characters than UTF-16 units: only a longer prompt needs counting.
  if (prompt.length > max && Array.from(prompt).length > max) {
    return [here ? STUDIO_DOCK.promptTooLongHere : STUDIO_DOCK.promptTooLong];
  }
  return [];
}

/** What else `estimate` says the run would refuse (its settings, its pictures, a model that
 *  cannot be used): its sentences about the prompt are left to `promptProblems`. */
export function settingsProblems(estimate: Estimate | null): string[] {
  return estimate?.invalid.filter((sentence) => !PROMPT_SENTENCES.has(sentence)) ?? [];
}

/** A request without its client token, as one string: the same draft gives the same key. */
export function requestKey(req: StartRequest): string {
  return JSON.stringify({ ...req, clientToken: "" });
}

/** `tab`'s latest estimate while the draft is still on the model and place it was asked for
 *  (the prompt or a setting may have changed since, and the new answer be on its way): what the
 *  dock shows. */
export function shownEstimate(s: Pick<StudioState, "drafts" | "estimates">, tab: StudioTab): TabEstimate | null {
  const known = s.estimates[tab];
  const { target, model } = s.drafts[tab];
  return known && target !== null && sameTarget(known.target, target) && known.model === model ? known : null;
}

/** `tab`'s estimate when it was asked for the draft exactly as it is: what a press of the draft
 *  goes by to open the confirmation first (an answer for the draft as it was is no reason to
 *  ask). */
export function currentEstimate(s: Pick<StudioState, "drafts" | "estimates">, tab: StudioTab): Estimate | null {
  const known = s.estimates[tab];
  const req = startRequestOf(tab, s.drafts[tab], ESTIMATE_TOKEN);
  return known && req && known.key === requestKey(req) ? known.estimate : null;
}

/** Where `run` ran: this computer, or the endpoint it was sent to (`null` once that is not
 *  known). */
export function targetOf(run: Run): RunTarget | null {
  if (run.runsOn === "local") return "thisComputer";
  return run.providerId === null ? null : { endpoint: run.providerId };
}

/** The key every catalogue gives a model's seed field (the backend's `seed_field`, and
 *  `param::SEED` for a model on this computer). */
const SEED_KEY = "seed";

/** The key of `model`'s seed field (by its role); `SEED_KEY` when the model is not listed now. */
export function seedKey(model: MediaModel | null): string {
  return model?.params.find((p) => p.canonical === "seed" || p.kind.type === "seed")?.key ?? SEED_KEY;
}

/** What pressing again from `run` sends, as press `clientToken`: its own settings where it ran,
 *  without its seed (the field of `model`, its model as listed, or the usual seed key when it is
 *  no longer listed), so a new seed is picked, unless `keepSeed`; `null` when the endpoint it ran
 *  at is not known. */
export function requestFromRun(
  run: Run,
  clientToken: string,
  model: MediaModel | null,
  keepSeed: boolean,
): StartRequest | null {
  const target = targetOf(run);
  if (target === null) return null;
  const seed = keepSeed ? undefined : seedKey(model);
  return {
    clientToken,
    target,
    kind: run.kind,
    model: run.model,
    prompt: run.prompt,
    params: Object.fromEntries(Object.entries(run.params).filter(([key]) => key !== seed)),
    extra: run.extra,
    refs: run.refs.map((r) => ({ sha256: r.sha256, role: r.role ?? "reference" })),
    routing: run.routing,
  };
}
