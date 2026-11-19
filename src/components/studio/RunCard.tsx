import { useEffect, useState } from "react";
import type { ErrorInfo } from "../../lib/types";
import {
  engineDownloadWords,
  STUDIO_ACTIONS,
  STUDIO_CARD,
  STUDIO_CHAT_LINK,
  STUDIO_DOCK,
  STUDIO_PRICE,
  STUDIO_QUEUE,
} from "../../lib/words";
import { toErrorInfo, useApp } from "../../store/app";
import { switchModel } from "../../store/modelChange";
import { listedModel, useStudio, type StudioState } from "../../store/studio";
import { COST_TONE_CLASS, costTone, formatUsd } from "./cost";
import { etaText, timeWords } from "./eta";
import { ResultTile, resultsOf } from "./results";
import { settingsParts } from "./settingsLine";
import { CARD_BUTTON } from "./styles";
import type { Run } from "./types";
import { Working } from "./Working";

export { localLine } from "./Working";

// A run's card in Studio's stream (0.1.0 Task B8; design Wireframe 3, local addendum Wireframe
// L4): the prompt; the settings line (model, where it ran, its settings, then the estimate and
// the cost for a cloud run, or "free" and the time for a local one); the status line with a
// timer and the progress (for a local run its stage, step and time left, `localLine`); what a
// local run waits for or how it runs (beside the chat model, on the processor, the chat model
// unloaded to make room, an out-of-memory retry, no progress for 10 minutes); why it ended, with
// the charge line the backend recorded for a cloud run; and the card's buttons: Cancel, Stop or
// Stop watching while it works, *Get it anyway* and *Check again* for a video (never a create),
// the engine's or a damaged model's download, Edit prompt after a refusal (which is never
// retried), and Again. Task B9: its results as tiles (each opens the viewer, each with its
// star) and Reuse, which fills the prompt box from the run and never starts it. Task B11: Send to
// chat on a finished card with one picture (with several, the viewer sends the one wanted; a
// video is never sent). Esc never cancels a card. The card follows the run's newest state in
// the store. The settings line is built in settingsLine.ts, the status line of a run still
// working in Working.tsx.

/** The parts of the queue strip, in its order. */
const QUEUE_ORDER = ["making", "waiting", "done", "stopped", "failed"] as const;
type QueueState = (typeof QUEUE_ORDER)[number];

/** The finished runs the usual time is taken from. */
const USUAL_OF_LAST = 10;

/** Where a run stands in the queue strip; a refused run counts as failed, a video no longer
 *  checked as waiting. */
function queueState(run: Run): QueueState {
  switch (run.status) {
    case "queued":
      return "waiting";
    case "waiting":
      return run.watching ? "making" : "waiting";
    case "sending":
    case "running":
    case "downloading":
      return "making";
    case "done":
      return "done";
    case "cancelled":
    case "interrupted":
      return "stopped";
    case "failed":
    case "blocked":
      return "failed";
  }
}

/** The queue strip: `runs` by state, "1 making · 1 waiting · 3 done · 1 stopped"; a state with
 *  none is left out (an empty string for none at all). */
export function queueSummary(runs: Run[]): string {
  const counts: Record<QueueState, number> = { making: 0, waiting: 0, done: 0, stopped: 0, failed: 0 };
  for (const run of runs) counts[queueState(run)] += 1;
  return QUEUE_ORDER.filter((state) => counts[state] > 0)
    .map((state) => STUDIO_QUEUE[state](counts[state]))
    .join(STUDIO_CARD.joiner);
}

/** A finished run's time, in seconds, with when it finished; `null` for a run not finished. */
function finishedTime(run: Run): { at: number; seconds: number } | null {
  return run.status === "done" && run.startedAt !== null && run.finishedAt !== null
    ? { at: run.finishedAt, seconds: (run.finishedAt - run.startedAt) / 1000 }
    : null;
}

/** The median, in seconds, of the last 10 of `finished` (by when they finished); `null` for
 *  none. */
function usualOf(finished: { at: number; seconds: number }[]): number | null {
  const times = [...finished]
    .sort((a, b) => b.at - a.at)
    .slice(0, USUAL_OF_LAST)
    .map((f) => f.seconds)
    .sort((a, b) => a - b);
  if (times.length === 0) return null;
  const mid = Math.floor(times.length / 2);
  return times.length % 2 === 1 ? (times[mid] ?? null) : ((times[mid - 1] ?? 0) + (times[mid] ?? 0)) / 2;
}

/** How long model `model`'s runs usually take, in seconds: the median of its last 10 finished
 *  runs (by when they finished); `null` before one finished. */
export function usualSeconds(runs: Run[], model: string): number | null {
  return usualOf(runs.flatMap((r) => (r.model === model ? (finishedTime(r) ?? []) : [])));
}

/** `usualSeconds` of every model with a finished run among `runs`, worked out in one pass. */
function usualTimes(runs: Run[]): Map<string, number> {
  const byModel = new Map<string, { at: number; seconds: number }[]>();
  for (const run of runs) {
    const time = finishedTime(run);
    if (!time) continue;
    const finished = byModel.get(run.model);
    if (finished) finished.push(time);
    else byModel.set(run.model, [time]);
  }
  const times = new Map<string, number>();
  for (const [model, finished] of byModel) {
    const usual = usualOf(finished);
    if (usual !== null) times.set(model, usual);
  }
  return times;
}

/** Whether `run` is still working: queued, sending, running, downloading, or a video Studio is
 *  still following. */
function isWorking(run: Run): boolean {
  return (
    run.status === "queued" ||
    run.status === "sending" ||
    run.status === "running" ||
    run.status === "downloading" ||
    (run.status === "waiting" && run.watching)
  );
}

/** *Get it anyway*: only for a cloud video whose company job is known, stopped or failed (a
 *  picture, or a video stopped before it got a job id, has nothing to get). */
function canGetAnyway(run: Run): boolean {
  return (
    run.runsOn === "cloud" &&
    run.kind === "video" &&
    run.remoteId !== null &&
    (run.status === "cancelled" || run.status === "failed")
  );
}

/** Each run the store knows of once, at its newest state: the live runs, then the newest runs
 *  and the gallery's. */
function knownRuns(s: Pick<StudioState, "live" | "recentRuns" | "runs">): Run[] {
  const byId = new Map<string, Run>();
  for (const run of [...Object.values(s.live), ...s.recentRuns, ...s.runs]) if (!byId.has(run.id)) byId.set(run.id, run);
  return [...byId.values()];
}

/** The store's runs `usualTimesOf` last worked from, and what it found. */
let usualMemo: {
  live: StudioState["live"];
  recentRuns: Run[];
  runs: Run[];
  times: ReadonlyMap<string, number>;
} | null = null;

/** How long each model's runs usually take, in seconds, from every run the store knows of. A
 *  selector every video card shares: it is worked out again only when the store's runs change
 *  (not on each card, nor on an update of something else), however many cards ask. */
export function usualTimesOf(s: Pick<StudioState, "live" | "recentRuns" | "runs">): ReadonlyMap<string, number> {
  const memo = usualMemo;
  if (memo && memo.live === s.live && memo.recentRuns === s.recentRuns && memo.runs === s.runs) return memo.times;
  const times = usualTimes(knownRuns(s));
  usualMemo = { live: s.live, recentRuns: s.recentRuns, runs: s.runs, times };
  return times;
}

/** The figure at the end of the settings line: a cloud run's estimate, then what it cost; a
 *  local run's time here while it waits, then how long it took. */
function figureOf(run: Run): { text: string; tone: string } | null {
  if (run.runsOn === "local") {
    if (run.status === "queued") {
      const time = etaText(run.eta);
      return time ? { text: time, tone: "text-fg-muted" } : null;
    }
    if (run.status !== "done") return null;
    const took =
      run.local?.elapsedMs ?? (run.finishedAt !== null && run.startedAt !== null ? run.finishedAt - run.startedAt : null);
    return took === null ? null : { text: STUDIO_CARD.took(timeWords(took)), tone: "text-fg-muted" };
  }
  if (run.status === "done") {
    return run.costUsd === null
      ? { text: STUDIO_PRICE.unknown, tone: COST_TONE_CLASS.unknown }
      : { text: formatUsd(run.costUsd), tone: COST_TONE_CLASS[costTone(run.costUsd)] };
  }
  if (isWorking(run) && run.estimateUsd !== null) {
    return { text: STUDIO_DOCK.estimate(formatUsd(run.estimateUsd)), tone: COST_TONE_CLASS[costTone(run.estimateUsd)] };
  }
  return null;
}

/** Whether an ended cloud run was charged, as the backend recorded it, with the company's
 *  request id: "Not charged · request req_8f2". `null` for a local run, which is free. */
function chargeLine(run: Run): string | null {
  const error = run.error;
  if (run.runsOn !== "cloud" || !error) return null;
  const charged =
    error.charged === false
      ? STUDIO_CARD.notCharged
      : error.charged === true
        ? STUDIO_CARD.charged(run.costUsd === null ? null : formatUsd(run.costUsd))
        : STUDIO_CARD.mayBeCharged;
  return error.requestId ? `${charged}${STUDIO_CARD.joiner}${STUDIO_CARD.request(error.requestId)}` : charged;
}

/** The time now, again each second while `active`. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

const CARD = "enter flex flex-col gap-1.5 rounded-xl border border-border bg-surface p-3.5 shadow-soft";
const LINK = "shrink-0 text-xs underline hover:text-fg disabled:opacity-60";

/** One run's card. */
export function RunCard(props: { run: Run }) {
  const run = useStudio((s) => s.live[props.run.id] ?? s.runs.find((r) => r.id === props.run.id)) ?? props.run;
  const local = run.runsOn === "local";
  const model = useStudio((s) =>
    listedModel(local ? "thisComputer" : { endpoint: run.providerId ?? -1 }, run.model, s.local, s.endpoints),
  );
  const engine = useStudio((s) => s.local?.engine ?? null);
  const usual = useStudio((s) =>
    run.kind === "video" && !local ? (usualTimesOf(s).get(run.model) ?? null) : null,
  );
  const unloaded = run.local?.unloadedChat ?? null;
  const chatLoaded = useApp(
    (s) =>
      unloaded !== null &&
      s.runtime?.modelPath === unloaded &&
      (s.runtime.state === "ready" || s.runtime.state === "starting"),
  );
  const working = isWorking(run);
  const now = useNow(working && run.status !== "queued");
  // An action on its way (its buttons wait), and why the last one failed: every action on the
  // card, the status line's (Working's answer to "Unload the chat model?") too, runs through
  // `perform` and is said at the card's foot with its next step.
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<ErrorInfo | null>(null);
  const [repairing, setRepairing] = useState(false);
  // *Load it again* on its way.
  const [loadingChat, setLoadingChat] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  // *Keep waiting* puts the stall line away until the run stalls again.
  const stalled = run.local?.stalled ?? false;
  const [keepWaiting, setKeepWaiting] = useState(false);
  const [wasStalled, setWasStalled] = useState(stalled);
  if (stalled !== wasStalled) {
    setWasStalled(stalled);
    if (!stalled) setKeepWaiting(false);
  }

  const perform = (action: () => Promise<void>) => {
    setBusy(true);
    setFailure(null);
    action().then(
      () => setBusy(false),
      (e: unknown) => {
        setBusy(false);
        setFailure(toErrorInfo(e));
      },
    );
  };
  const store = useStudio.getState;
  // *Load it again*: the chat model loads as it was (the open chat keeps its own model); a
  // failure is said on the card with its next step.
  const loadAgain = (path: string) => {
    setLoadingChat(true);
    perform(async () => {
      try {
        await switchModel(useApp.getState().runtime, path);
      } finally {
        setLoadingChat(false);
      }
    });
  };
  const cancel = () => perform(() => store().cancel(run.id));
  const fetchAnyway = () => perform(() => store().fetchAnyway(run.id));
  const again = () => perform(() => store().again(run));
  const reuse = () => {
    const refused = store().reuse(run, true);
    setFailure(refused === null ? null : { code: "studio_reuse", message: refused, nextStep: "", details: "" });
    if (refused === null) window.dispatchEvent(new Event("app:focus-studio-prompt"));
  };
  const editPrompt = () => {
    store().setPrompt(run.kind, run.prompt);
    window.dispatchEvent(new Event("app:focus-studio-prompt"));
  };

  const settings = [run.modelLabel, local ? STUDIO_CARD.here : run.providerName, ...settingsParts(run, model)].join(
    STUDIO_CARD.joiner,
  );
  const figure = figureOf(run);
  const error = run.error;
  // A waiting video has not ended: Studio follows it, will follow it again, or stopped checking
  // after its 30 minutes (stale).
  const ended = !working && run.status !== "done" && run.status !== "waiting";
  const charge = chargeLine(run);
  const pct = run.progress === null ? null : Math.round(Math.min(1, Math.max(0, run.progress)) * 100);
  const engineMissing = local && run.status === "failed" && error?.code === "studio_engine_missing";
  const offerEngine = engineMissing && (engine?.state === "notInstalled" || engine?.state === "partial");
  const fileBad = local && run.status === "failed" && error?.code === "studio_model_file_bad";
  // Send to chat: the card's one picture.
  const forChat = run.kind === "image" && run.status === "done" && run.outputs.length === 1 ? run.outputs[0] : null;

  return (
    <article data-run-card={run.id} className={CARD}>
      {run.prompt && <p className="text-sm break-words">{STUDIO_CARD.prompt(run.prompt)}</p>}
      <div className="flex flex-wrap items-baseline gap-x-3 text-xs text-fg-muted">
        <p className="min-w-0 flex-1">
          <span>{settings}</span>
          {local && (
            <>
              <span>{STUDIO_CARD.joiner}</span>
              <span>{STUDIO_PRICE.free}</span>
            </>
          )}
        </p>
        {figure && <span className={`text-sm tabular-nums ${figure.tone}`}>{figure.text}</span>}
      </div>

      {working && pct !== null && run.status !== "queued" && (
        <div
          role="progressbar"
          aria-label={STUDIO_CARD.progress}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={pct}
          className="h-1.5 w-full overflow-hidden rounded-full bg-bg-muted"
        >
          <div
            className="brand-gradient h-full rounded-full transition-[width] duration-300 ease-out"
            style={{ width: `${pct}%` }}
          />
        </div>
      )}
      {working && <Working run={run} now={now} usual={usual} busy={busy} perform={perform} onCancel={cancel} />}
      {run.status === "waiting" &&
        !run.watching &&
        (run.stale ? (
          <div className="flex flex-wrap items-center gap-2 text-sm">
            <p>{STUDIO_CARD.stillNotReady}</p>
            <button type="button" className={CARD_BUTTON} disabled={busy} onClick={fetchAnyway}>
              {STUDIO_CARD.checkAgain}
            </button>
          </div>
        ) : (
          // Not followed yet (read from the saved runs before its watch resumes): it just waits.
          <p className="text-sm text-fg-muted">{STUDIO_CARD.queued(null)}</p>
        ))}

      {local && working && (
        <>
          {run.local?.alongside && <p className="text-xs text-fg-muted">{STUDIO_CARD.alongside}</p>}
          {run.local?.onProcessor && <p className="text-xs text-fg-muted">{STUDIO_CARD.onProcessor}</p>}
          {run.local?.retrying && <p className="text-xs text-warning">{STUDIO_CARD.retrying}</p>}
          {stalled && !keepWaiting && (
            <div className="flex flex-wrap items-center gap-2 text-xs text-warning">
              <p>{STUDIO_CARD.stalled}</p>
              <button type="button" className={LINK} onClick={() => setKeepWaiting(true)}>
                {STUDIO_CARD.keepWaiting}
              </button>
            </div>
          )}
        </>
      )}
      {unloaded !== null && !chatLoaded && (
        <div className="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
          <p>{STUDIO_CARD.unloaded}</p>
          {loadingChat ? (
            <p role="status">{STUDIO_CARD.loadingChat}</p>
          ) : (
            <button type="button" className={LINK} disabled={busy} onClick={() => loadAgain(unloaded)}>
              {STUDIO_CARD.loadAgain}
            </button>
          )}
        </div>
      )}

      {ended && error && (
        <div className="flex flex-col gap-0.5 text-sm" role="status">
          <p className="font-medium">{error.message}</p>
          {error.nextStep && <p className="text-fg-muted">{error.nextStep}</p>}
          {charge && <p className="text-xs text-fg-muted">{charge}</p>}
          {detailsOpen && error.details && (
            <pre className="mt-1 max-h-40 overflow-y-auto rounded-md border border-border bg-bg-subtle p-2 text-xs break-words whitespace-pre-wrap text-fg-muted">
              {error.details}
            </pre>
          )}
        </div>
      )}
      {engineMissing && engine?.state === "downloading" && (
        <p className="text-xs text-fg-muted">{STUDIO_CARD.engineDownloading}</p>
      )}
      {fileBad && repairing && <p className="text-xs text-fg-muted">{STUDIO_CARD.repairing}</p>}

      {run.outputs.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {resultsOf(run).map((output) => (
            <ResultTile
              key={output.id}
              run={run}
              output={output}
              onOpen={(id) => store().openViewer(id)}
              onStar={() => perform(() => store().star(output.id, !output.starred))}
              className="size-28"
            />
          ))}
        </div>
      )}

      {!working && run.status !== "waiting" && (
        <div className="flex flex-wrap items-center gap-2">
          {offerEngine && engine && (
            <button
              type="button"
              className={CARD_BUTTON}
              title={engine.licenseName}
              disabled={busy}
              onClick={() => perform(() => store().downloadEngine())}
            >
              {engineDownloadWords(engine.downloadBytes, engine.licenseName)}
            </button>
          )}
          {fileBad && !repairing && (
            <button
              type="button"
              className={CARD_BUTTON}
              disabled={busy}
              onClick={() =>
                perform(async () => {
                  // A licence to accept first opens the download dialog in Get local models instead.
                  if (await store().repairLocal(run.model)) setRepairing(true);
                })
              }
            >
              {STUDIO_ACTIONS.downloadAgain}
            </button>
          )}
          {canGetAnyway(run) && (
            <button type="button" className={CARD_BUTTON} disabled={busy} onClick={fetchAnyway}>
              {STUDIO_CARD.getAnyway}
            </button>
          )}
          {run.status === "blocked" ? (
            <button type="button" className={CARD_BUTTON} onClick={editPrompt}>
              {STUDIO_CARD.editPrompt}
            </button>
          ) : (
            <button type="button" className={CARD_BUTTON} disabled={busy} onClick={again}>
              {local && ended ? STUDIO_CARD.runAgain : STUDIO_CARD.again}
            </button>
          )}
          <button type="button" className={CARD_BUTTON} onClick={reuse}>
            {STUDIO_CARD.reuse}
          </button>
          {forChat && (
            <button
              type="button"
              className={CARD_BUTTON}
              disabled={busy}
              onClick={() => perform(() => store().sendToChat(forChat))}
            >
              {STUDIO_CHAT_LINK.sendToChat}
            </button>
          )}
          {ended && error?.details && (
            <button
              type="button"
              className={LINK}
              aria-expanded={detailsOpen}
              onClick={() => setDetailsOpen((open) => !open)}
            >
              {STUDIO_CARD.details}
            </button>
          )}
        </div>
      )}
      {failure && (
        <div className="text-xs" role="alert">
          <p className="text-danger">{failure.message}</p>
          {failure.nextStep && <p className="text-fg-muted">{failure.nextStep}</p>}
        </div>
      )}
    </article>
  );
}
