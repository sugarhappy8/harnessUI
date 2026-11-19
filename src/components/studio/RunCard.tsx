import { useEffect, useState } from "react";
import type { ErrorInfo } from "../../lib/types";
import {
  engineDownloadWords,
  STUDIO_ACTIONS,
  STUDIO_CARD,
  STUDIO_CONFIRM,
  STUDIO_DOCK,
  STUDIO_PRICE,
  STUDIO_QUEUE,
  STUDIO_TIME,
} from "../../lib/words";
import { toErrorInfo, useApp } from "../../store/app";
import { listedModel, useStudio, type StudioState } from "../../store/studio";
import { valueWords } from "./canonical";
import { COST_TONE_CLASS, costTone, formatUsd } from "./cost";
import { etaText, timeWords } from "./eta";
import type { Canonical, MediaModel, Run } from "./types";

// A run's card in Studio's stream (0.1.0 Task B8; design Wireframe 3, local addendum Wireframe
// L4): the prompt; the settings line (model, where it ran, its settings, then the estimate and
// the cost for a cloud run, or "free" and the time for a local one); the status line with a
// timer and the progress (for a local run its stage, step and time left, `localLine`); what a
// local run waits for or how it runs (beside the chat model, on the processor, the chat model
// unloaded to make room, an out-of-memory retry, no progress for 10 minutes); why it ended, with
// the charge line the backend recorded for a cloud run; and the card's buttons: Cancel, Stop or
// Stop watching while it works, *Get it anyway* and *Check again* for a video (never a create),
// the engine's or a damaged model's download, Edit prompt after a refusal (which is never
// retried), and Again. The results row (tiles) and the other actions are Task B9's. Esc never
// cancels a card. The card follows the run's newest state in the store.

/** The parts of the queue strip, in its order. */
const QUEUE_ORDER = ["making", "waiting", "done", "stopped", "failed"] as const;
type QueueState = (typeof QUEUE_ORDER)[number];

/** The local stages before a step was timed: the first guess stands in for the time left. */
const BEFORE_A_STEP: ReadonlySet<string | null> = new Set([null, "loading", "readingPrompt", "drawing"]);

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

/** How long model `model`'s runs usually take, in seconds: the median of its last 10 finished
 *  runs (by when they finished); `null` before one finished. */
export function usualSeconds(runs: Run[], model: string): number | null {
  const finished = runs.flatMap((r) =>
    r.model === model && r.status === "done" && r.startedAt !== null && r.finishedAt !== null
      ? [{ at: r.finishedAt, seconds: (r.finishedAt - r.startedAt) / 1000 }]
      : [],
  );
  const times = finished
    .sort((a, b) => b.at - a.at)
    .slice(0, USUAL_OF_LAST)
    .map((f) => f.seconds)
    .sort((a, b) => a - b);
  if (times.length === 0) return null;
  const mid = Math.floor(times.length / 2);
  return times.length % 2 === 1 ? (times[mid] ?? null) : ((times[mid - 1] ?? 0) + (times[mid] ?? 0)) / 2;
}

/** A time on a clock: "0:21", "1:12", "1:02:05". */
function clock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = String(total % 60).padStart(2, "0");
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${seconds}` : `${minutes}:${seconds}`;
}

/** When a run started working (when it was recorded, before that is known). */
function startOf(run: Run): number {
  return run.startedAt ?? run.createdAt;
}

/**
 * A local run's status line while it runs: the picture of several, the stage (the pass and step
 * while drawing), the time so far, then the time left as timed live from this run's steps
 * ("about 0:30 left"); before a step is timed, the estimate (`etaText`, "(first guess)" until
 * measured here) stands in for it, and while decoding or saving no time is said.
 * "Picture 1 of 2 · step 3 of 4 · 0:21 so far · about 0:30 left".
 */
export function localLine(run: Run, now: number): string {
  const local = run.local;
  const stage = local?.stage ?? null;
  const parts: string[] = [];
  if (local?.picture != null && local.pictures != null && local.pictures > 1) {
    parts.push(STUDIO_CARD.picture(local.picture, local.pictures));
  }
  if (stage === "drawing" && local?.step != null && local.steps != null) {
    if (local.pass != null && local.passes != null && local.passes > 1) parts.push(STUDIO_CARD.pass(local.pass, local.passes));
    parts.push(STUDIO_CARD.step(local.step, local.steps));
  } else {
    parts.push(stage === null ? STUDIO_CARD.starting : STUDIO_CARD.stage[stage]);
  }
  parts.push(STUDIO_CARD.soFar(clock(now - startOf(run))));
  const left = run.eta?.leftMs ?? null;
  if (left !== null) {
    parts.push(STUDIO_CARD.left(clock(left)));
  } else if (BEFORE_A_STEP.has(stage)) {
    const guess = etaText(run.eta);
    if (guess) parts.push(guess);
  }
  return parts.join(STUDIO_CARD.joiner);
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

/** The settings line after the model and where it ran, from the run's own settings (the model
 *  as listed names their fields, else their usual keys): a cloud run's length, shape,
 *  resolution, size, sound, count, seed and added pictures; a local run's exact size (else its
 *  results', else its shape), length, resolution, count, seed and added pictures. */
function settingsParts(run: Run, model: MediaModel | null): string[] {
  const field = (canonical: Canonical) => model?.params.find((p) => p.canonical === canonical);
  const value = (canonical: Canonical, key: string): unknown => {
    const v = run.params[field(canonical)?.key ?? key];
    return v === undefined || v === null || v === "" ? null : v;
  };
  const words = (canonical: Canonical, key: string): string | null => {
    const v = value(canonical, key);
    return v === null ? null : valueWords(v, field(canonical));
  };
  const size = (): string | null => {
    const v = value("size", "size");
    const sides = typeof v === "string" ? v.split("x") : [];
    return sides.length === 2 ? STUDIO_CARD.size(sides[0] ?? "", sides[1] ?? "") : v === null ? null : String(v);
  };
  const length = (): string | null => {
    const v = value("length", "duration");
    if (v === null) return null;
    return field("length") ? valueWords(v, field("length")) : typeof v === "number" ? STUDIO_TIME.seconds(v) : String(v);
  };
  const sound = value("sound", "generate_audio");
  const count = value("count", "n");
  const seed = value("seed", "seed");
  const refs: string[] = [];
  const references = run.refs.filter((r) => (r.role ?? "reference") === "reference").length;
  if (run.refs.some((r) => r.role === "startFrame")) refs.push(STUDIO_CARD.startFrame);
  if (run.refs.some((r) => r.role === "endFrame")) refs.push(STUDIO_CARD.endFrame);
  if (references > 0) refs.push(STUDIO_CARD.addedPictures(references));
  const output = run.outputs[0];
  const shown =
    run.runsOn === "local"
      ? [
          size() ??
            (output?.width != null && output.height != null
              ? STUDIO_CARD.size(String(output.width), String(output.height))
              : words("shape", "aspect_ratio")),
          length(),
          words("resolution", "resolution"),
        ]
      : [
          length(),
          words("shape", "aspect_ratio"),
          words("resolution", "resolution"),
          size(),
          typeof sound === "boolean" ? STUDIO_CONFIRM.sound(sound) : null,
        ];
  return [
    ...shown,
    typeof count === "number" && count > 1 ? STUDIO_CARD.count(count) : null,
    seed === null ? null : STUDIO_CARD.seed(String(seed)),
    ...refs,
  ].filter((p): p is string => p !== null);
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
const BUTTON =
  "rounded-lg border border-border bg-bg px-2.5 py-1 text-sm hover:border-border-strong hover:bg-bg-muted disabled:opacity-60";
const LINK = "shrink-0 text-xs underline hover:text-fg disabled:opacity-60";

/** One run's card. */
export function RunCard(props: { run: Run }) {
  const run = useStudio((s) => s.live[props.run.id]) ?? props.run;
  const local = run.runsOn === "local";
  const model = useStudio((s) =>
    listedModel(local ? "thisComputer" : { endpoint: run.providerId ?? -1 }, run.model, s.local, s.endpoints),
  );
  const engine = useStudio((s) => s.local?.engine ?? null);
  const usual = useStudio((s) =>
    run.kind === "video" && !local ? usualSeconds(knownRuns(s), run.model) : null,
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
  // An action on its way (its buttons wait), and why the last one failed.
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<ErrorInfo | null>(null);
  const [repairing, setRepairing] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  // *Keep waiting* puts the stall line away until the run stalls again.
  const stalled = run.local?.stalled ?? false;
  const [keepWaiting, setKeepWaiting] = useState(false);
  const [wasStalled, setWasStalled] = useState(stalled);
  if (stalled !== wasStalled) {
    setWasStalled(stalled);
    if (!stalled) setKeepWaiting(false);
  }

  const perform = (action: () => Promise<void>, after?: () => void) => {
    setBusy(true);
    setFailure(null);
    action().then(
      () => {
        setBusy(false);
        after?.();
      },
      (e: unknown) => {
        setBusy(false);
        setFailure(toErrorInfo(e));
      },
    );
  };
  const store = useStudio.getState;
  const cancel = () => perform(() => store().cancel(run.id));
  const fetchAnyway = () => perform(() => store().fetchAnyway(run.id));
  const again = () => perform(() => store().again(run));
  const editPrompt = () => {
    store().setPrompt(run.kind, run.prompt);
    window.dispatchEvent(new Event("app:focus-studio-prompt"));
  };

  const settings = [run.modelLabel, local ? STUDIO_CARD.here : run.providerName, ...settingsParts(run, model)].join(
    STUDIO_CARD.joiner,
  );
  const figure = figureOf(run);
  const error = run.error;
  const ended = !working && run.status !== "done" && !(run.status === "waiting" && !run.watching);
  const charge = chargeLine(run);
  const pct = run.progress === null ? null : Math.round(Math.min(1, Math.max(0, run.progress)) * 100);
  const engineMissing = local && run.status === "failed" && error?.code === "studio_engine_missing";
  const offerEngine = engineMissing && (engine?.state === "notInstalled" || engine?.state === "partial");
  const fileBad = local && run.status === "failed" && error?.code === "studio_model_file_bad";

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
      {working && <Working run={run} now={now} usual={usual} busy={busy} onCancel={cancel} />}
      {run.status === "waiting" && !run.watching && (
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <p>{STUDIO_CARD.stillNotReady}</p>
          <button type="button" className={BUTTON} disabled={busy} onClick={fetchAnyway}>
            {STUDIO_CARD.checkAgain}
          </button>
        </div>
      )}

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
          <button type="button" className={LINK} onClick={() => void useApp.getState().pickModel(`local:${unloaded}`)}>
            {STUDIO_CARD.loadAgain}
          </button>
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

      {/* The results row: Task B9's tiles. */}

      {!working && run.status !== "waiting" && (
        <div className="flex flex-wrap items-center gap-2">
          {offerEngine && engine && (
            <button
              type="button"
              className={BUTTON}
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
              className={BUTTON}
              disabled={busy}
              onClick={() => perform(() => store().repairLocal(run.model), () => setRepairing(true))}
            >
              {STUDIO_ACTIONS.downloadAgain}
            </button>
          )}
          {canGetAnyway(run) && (
            <button type="button" className={BUTTON} disabled={busy} onClick={fetchAnyway}>
              {STUDIO_CARD.getAnyway}
            </button>
          )}
          {run.status === "blocked" ? (
            <button type="button" className={BUTTON} onClick={editPrompt}>
              {STUDIO_CARD.editPrompt}
            </button>
          ) : (
            <button type="button" className={BUTTON} disabled={busy} onClick={again}>
              {local && ended ? STUDIO_CARD.runAgain : STUDIO_CARD.again}
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

/** The status line of a run still working, with what stops it: where it waits in the queue, or
 *  what a local run waits for (and the question to unload the chat model), "Making it" with a
 *  timer (a video's time so far and usual time, and that the app may be closed), or a local
 *  run's stage, step and time left. */
function Working(props: { run: Run; now: number; usual: number | null; busy: boolean; onCancel(): void }) {
  const { run, now, usual, busy } = props;
  const local = run.runsOn === "local";
  const waitingFor = run.local?.waitingFor ?? null;
  const [answering, setAnswering] = useState(false);
  const [failure, setFailure] = useState<ErrorInfo | null>(null);
  const answer = (unload: boolean) => {
    setAnswering(true);
    setFailure(null);
    useStudio
      .getState()
      .answerUnload(run.id, unload)
      .then(
        () => setAnswering(false),
        (e: unknown) => {
          setAnswering(false);
          setFailure(toErrorInfo(e));
        },
      );
  };
  const stopButton = (label: string) => (
    <button type="button" className={BUTTON} disabled={busy} onClick={props.onCancel}>
      {label}
    </button>
  );

  if (run.status === "queued") {
    return (
      <div className="flex flex-col gap-1.5 text-sm">
        {waitingFor === "answer" ? (
          <div className="flex flex-wrap items-center gap-2">
            <p className="min-w-0 flex-1">{STUDIO_CARD.askUnload}</p>
            <button type="button" className={BUTTON} disabled={answering} onClick={() => answer(true)}>
              {STUDIO_CARD.unload}
            </button>
            <button type="button" className={BUTTON} disabled={answering} onClick={() => answer(false)}>
              {STUDIO_CARD.keepChat}
            </button>
            {stopButton(STUDIO_CARD.cancel)}
          </div>
        ) : (
          <div className="flex flex-wrap items-center gap-2">
            <p className="min-w-0 flex-1 text-fg-muted">
              {waitingFor === null ? STUDIO_CARD.queued(run.queuePosition) : STUDIO_CARD.waitingFor[waitingFor]}
            </p>
            {stopButton(STUDIO_CARD.cancel)}
          </div>
        )}
        {failure && <p className="text-xs text-danger">{failure.message}</p>}
      </div>
    );
  }

  if (local) {
    return (
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <p className="min-w-0 flex-1 tabular-nums">{localLine(run, now)}</p>
        {stopButton(STUDIO_CARD.stop)}
      </div>
    );
  }

  const elapsed = clock(now - startOf(run));
  if (run.kind === "video") {
    const usually =
      usual === null
        ? null
        : etaText({ lowMs: usual * 1000, highMs: usual * 1000, measured: true, leftMs: null });
    const line = [STUDIO_CARD.making, STUDIO_CARD.soFar(elapsed), usually && STUDIO_CARD.usually(usually)]
      .filter((p): p is string => typeof p === "string")
      .join(STUDIO_CARD.joiner);
    return (
      <div className="flex flex-col gap-1 text-sm">
        <p className="tabular-nums">{line}</p>
        <div className="flex flex-wrap items-center gap-2">
          <p className="min-w-0 flex-1 text-xs text-fg-muted">{STUDIO_CARD.keepWorking}</p>
          {stopButton(STUDIO_CARD.stopWatching)}
        </div>
      </div>
    );
  }
  return (
    <div className="flex flex-wrap items-center gap-2 text-sm">
      <p className="min-w-0 flex-1 tabular-nums">{`${STUDIO_CARD.making}${STUDIO_CARD.joiner}${elapsed}`}</p>
      {stopButton(STUDIO_CARD.cancel)}
    </div>
  );
}
