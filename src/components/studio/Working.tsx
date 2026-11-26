import { STUDIO_CARD } from "../../lib/words";
import { useStudio } from "../../store/studio";
import { etaText } from "./eta";
import { CARD_BUTTON } from "./styles";
import type { Run } from "./types";

// The status line of a run's card while it works (0.1.0 Task B8; design Wireframe 3, local
// addendum Wireframe L4): where it waits in the queue, or what a local run waits for (and the
// question to unload the chat model); "Making it" with a timer (a video's time so far and usual
// time, and that the app may be closed); or a local run's stage, step and time left
// (`localLine`). Each with what stops it, on the one status line under the card's thin progress
// bar (design 10.4, Task U9); a video's note that the app may be closed is a quiet line below. The card's actions run through the card's own
// `perform`, so an answer waits like its other buttons and a failure is said where theirs are.

/** The local stages before a step was timed: the first guess stands in for the time left. */
const BEFORE_A_STEP: ReadonlySet<string | null> = new Set([null, "loading", "readingPrompt", "drawing"]);

/** A time on a clock: "0:21", "1:12", "1:02:05" (a video's length on its tile too). */
export function clock(ms: number): string {
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

/** The status line of a run still working, with what stops it: where it waits in the queue, or
 *  what a local run waits for (and the question to unload the chat model), "Making it" with a
 *  timer (a video's time so far and usual time, and that the app may be closed), or a local
 *  run's stage, step and time left. `busy` and `perform` are the card's: an action on its way,
 *  and how one is run (its failure is said on the card). */
export function Working(props: {
  run: Run;
  now: number;
  usual: number | null;
  busy: boolean;
  perform(action: () => Promise<void>): void;
  onCancel(): void;
}) {
  const { run, now, usual, busy, perform } = props;
  const local = run.runsOn === "local";
  const waitingFor = run.local?.waitingFor ?? null;
  const answer = (unload: boolean) => perform(() => useStudio.getState().answerUnload(run.id, unload));
  const stopButton = (label: string) => (
    <button type="button" className={CARD_BUTTON} disabled={busy} onClick={props.onCancel}>
      {label}
    </button>
  );

  if (run.status === "queued") {
    return (
      <div className="flex flex-col gap-1.5 text-sm">
        {waitingFor === "answer" ? (
          <div className="flex flex-wrap items-center gap-2">
            <p className="min-w-0 flex-1">{STUDIO_CARD.askUnload}</p>
            <button type="button" className={CARD_BUTTON} disabled={busy} onClick={() => answer(true)}>
              {STUDIO_CARD.unload}
            </button>
            <button type="button" className={CARD_BUTTON} disabled={busy} onClick={() => answer(false)}>
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
      <div className="flex flex-col gap-0.5 text-sm">
        <div className="flex flex-wrap items-center gap-2">
          <p className="min-w-0 flex-1 tabular-nums">{line}</p>
          {stopButton(STUDIO_CARD.stopWatching)}
        </div>
        <p className="text-meta">{STUDIO_CARD.keepWorking}</p>
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
