import { useEffect, useRef } from "react";
import { DropdownMenu } from "radix-ui";
import { RotateCcw, Settings2, PowerOff } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { LoadStage, ModelBusy, RuntimeStatus, ShareStatus } from "../lib/types";
import {
  adjustmentWords,
  backendWords,
  loadStageWords,
  readyLineParts,
  SLOW_WORDS,
  slowSpeedWords,
  stepDownAction,
  waitingWords,
} from "../lib/words";
import { usePageVisible, usePoll } from "../lib/usePoll";
import { toErrorInfo, useApp } from "../store/app";
import { reloadModel, switchModel, unloadModel, useRestartNote, useWaiting } from "../store/modelChange";
import { visiblePath } from "./FolderChip";
import { ModelSettingsHost, openModelSettings, QuestionHost } from "./ModelSettings";

/** What a note's one button does. */
export type NoteAction = "reload" | "apply" | "settings";

export interface RuntimeNote {
  text: string;
  action: NoteAction | null;
}

const ACTION_LABEL: Record<NoteAction, string> = {
  reload: "Reload",
  apply: "Apply and restart",
  settings: "Model settings…",
};

/**
 * What the loaded model's state needs to say (D1, D2c, D4 slow states), most important
 * first, each with its reason and at most one action.
 */
export function runtimeNotes(rt: RuntimeStatus, opts: { restarted: boolean; hasMessages: boolean }): RuntimeNote[] {
  const notes: RuntimeNote[] = [];
  const add = (text: string | null, action: NoteAction | null) => {
    if (text && !notes.some((n) => n.text === text)) notes.push({ text, action });
  };
  if (rt.settingsChanged) add("Settings changed. Restart the model to use them.", "apply");
  const adjustments = currentAdjustments(rt.adjustments);
  for (const a of adjustments) {
    add(
      adjustmentWords(a),
      a.kind === "steppingDown" ? stepDownAction(a.reason) : a.kind === "restarting" ? null : "settings",
    );
  }
  const gpu = rt.backend !== null && rt.backend !== "cpu";
  const partial = gpu && partOnProcessor(rt);
  if (partial && !adjustments.some((a) => a.kind === "retryingWithLessGpu")) add(SLOW_WORDS.partial, "settings");
  if (rt.slow && rt.speedTps !== null) add(slowSpeedWords(rt.speedTps), "settings");
  if (opts.restarted && opts.hasMessages) add(SLOW_WORDS.afterRestart, null);
  return notes;
}

/**
 * Whether part of the loaded model runs on the processor and the rest on the graphics card
 * (the "part on the processor" slow state). From where the model went (0.0.6 item 6, the
 * ready line's memory report) when it is known: fewer than all layers, or any expert parts,
 * on the processor, but never for a placement the user chose in Fine control (they chose
 * it) and never with nothing on the graphics card. Else from the layer counts.
 */
export function partOnProcessor(rt: RuntimeStatus): boolean {
  // A backend without item 6 sends no `placement` at all.
  const p = rt.placement ?? null;
  if (p) {
    if (p.custom || p.graphics === null) return false;
    // Only amounts known (`layersOnGraphics` null): part is on the processor after As much
    // as fits (the menu says "…, the rest on the processor"); All of it put every layer on the card.
    const fewer = p.layersOnGraphics === null ? p.layersAbout : p.units !== null && p.layersOnGraphics < p.units;
    return fewer || (p.expertLayersOnProcessor ?? 0) > 0;
  }
  return rt.offloadedLayers !== null && rt.totalLayers !== null && rt.offloadedLayers < rt.totalLayers;
}

/**
 * The adjustments of this load that still describe the running model, the step-down first:
 * after the whole chain (Save memory off, As much as fits, the processor) the header says
 * "Running on the processor because…", not the first retry. Only the last step-down says
 * where it runs now, and "part of the model runs on the processor" is dropped for a
 * backend the model has left. Save memory stays off for the rest of the load, so that note
 * stays.
 */
function currentAdjustments(adjustments: LoadStage[]): LoadStage[] {
  let last = -1;
  adjustments.forEach((a, i) => {
    if (a.kind === "steppingDown") last = i;
  });
  if (last < 0) return adjustments;
  const left = new Set(adjustments.flatMap((a) => (a.kind === "steppingDown" ? [a.from] : [])));
  const rest = adjustments.filter(
    (a, i) => i !== last && a.kind !== "steppingDown" && !(a.kind === "retryingWithLessGpu" && left.has(a.backend)),
  );
  return [adjustments[last] as LoadStage, ...rest];
}

/** The busy counts behind the waiting line: every 1.5 seconds. */
const busyDelay = () => 1500;
/** Sharing (for "Used by another app"): every 2 seconds while it is on, else every 10. */
const shareDelay = (last: ShareStatus | null) => (last && !last.enabled ? 10_000 : 2000);

const MENU_ITEM =
  "flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted";

/**
 * The local model's state in the chat header (0.0.6 D1, D3, D4): loading in plain words,
 * the ready line with a menu (Reload, Model settings…, Unload to free memory), the slow
 * states and step-downs as visible text with one action, and the D3 waiting line. It also
 * hosts the model settings panel and the D3 question for the whole chat window.
 */
export function RuntimeIndicator() {
  return (
    <>
      <Status />
      <ModelSettingsHost />
      <QuestionHost />
    </>
  );
}

function Status() {
  const runtime = useApp((s) => s.runtime);
  const detail = useApp((s) => s.detail);
  const generating = useApp((s) => s.generation !== null);
  const showError = useApp((s) => s.showError);
  const waiting = useWaiting((s) => s.change);
  const running = useWaiting((s) => s.kind);
  const restarted = useRestartNote((s) => s.shown);
  const visible = usePageVisible();
  const modelRef = detail?.chat.modelRef ?? null;
  const isLocal = modelRef?.startsWith("local:") ?? false;
  const ready = isLocal && runtime?.state === "ready";
  const waitingLine = ready && (runtime.reloadWaiting || waiting !== null);

  // The first reply after a restart is slower; the note goes once a reply has ended.
  const wasGenerating = useRef(generating);
  useEffect(() => {
    if (wasGenerating.current && !generating) useRestartNote.setState({ shown: false });
    wasGenerating.current = generating;
  }, [generating]);

  // Polled only while a line needs them (no push event for these counts).
  const busy = usePoll<ModelBusy>(waitingLine && visible, ipc.modelBusy, busyDelay);
  const share = usePoll<ShareStatus>(ready && visible, ipc.shareStatus, shareDelay);

  if (!runtime || !isLocal) return null;
  const fail = (err: unknown) => showError(toErrorInfo(err));
  const reload = (force: boolean) => void reloadModel(useApp.getState().runtime, force).catch(fail);
  const settings = () => runtime.modelPath && openModelSettings(runtime.modelPath);
  const chatPath = modelRef ? modelRef.slice(6) : null;

  if (runtime.state === "starting") {
    const pct = runtime.progress !== null ? Math.round(runtime.progress * 100) : null;
    const stage = runtime.loadStage ? ` · ${loadStageWords(runtime.loadStage)}` : "";
    const end = pct !== null ? ` ${pct}%` : "…";
    // [0.0.6 header-layout] The bar first, where the ready line's dot will be, then the words
    // on a row one line high: what does not fit moves onto its hidden second line whole (the
    // percentage, which the bar also shows, then the stage), so "Loading model" is never cut.
    // The whole line is the tooltip. The line asks the header for the bar and "Loading model…"
    // only (`min-w`, a size container), and takes what the row has left.
    return (
      <div
        data-header-state="loading"
        data-loading-line
        title={`Loading model${stage}${end}`}
        className="@container/loading enter-fade flex min-w-[10rem] flex-1 items-center gap-2 text-xs whitespace-nowrap text-fg-muted"
        aria-live="polite"
      >
        {pct !== null ? (
          <div className="h-1.5 w-12 shrink-0 overflow-hidden rounded-full bg-bg-muted" aria-hidden>
            <div
              className="brand-gradient h-full rounded-full transition-[width] duration-300 ease-out"
              style={{ width: `${pct}%` }}
            />
          </div>
        ) : (
          <div className="progress-indeterminate h-1.5 w-12 shrink-0 rounded-full bg-bg-muted" aria-hidden />
        )}
        <span className="flex h-4 min-w-0 flex-wrap items-center overflow-hidden leading-4">
          <span data-loading="start" className="shrink-0">
            Loading model
          </span>
          {stage && (
            <span data-loading="stage" className="whitespace-pre">
              {stage}
            </span>
          )}
          <span data-loading="progress" className="whitespace-pre">
            {end}
          </span>
        </span>
      </div>
    );
  }

  if (runtime.state === "ready") {
    const notes = runtimeNotes(runtime, { restarted, hasMessages: (detail?.messages.length ?? 0) > 0 });
    const top = notes[0] ?? null;
    const run = (action: NoteAction) =>
      action === "reload" ? reload(true) : action === "apply" ? reload(false) : settings();
    // The ready line (D1) in a header that other parts crowd ([0.0.6 header-layout]). The
    // indicator is a size container that asks the header only for "About N words" (`min-w`;
    // the number alone in the narrowest header, more with a note under it) and takes what the
    // row has left; the header moves its buttons to a second row before it gives less. What it shows follows the room it gets,
    // in whole phrases, never cut inside a word: "Remembers about N words" with its reason, then
    // without the reason, then "About N words", then "N words". The reason, "· Used by another
    // app" and "· Change" sit on rows one line high that move what does not fit whole onto a
    // hidden second line ("· Change" goes first, then the reason). The whole line is always in
    // the menu and the tooltip, and "Change" is the menu's Model settings….
    const parts = runtime.nCtx ? readyLineParts(runtime.nCtx, runtime.ctxSource) : null;
    const full = parts ? parts.remembers + parts.reason : "Ready";
    const shared = share?.inUse ?? false;
    return (
      <div
        data-header-state="ready"
        className={
          "@container/ready enter-fade flex flex-1 flex-col justify-center gap-0.5 text-xs leading-tight " +
          (top || waitingLine ? "min-w-[12rem]" : "min-w-[6.25rem] @md:min-w-[9.5rem]")
        }
      >
        <div
          data-ready-line
          className="flex h-4 min-w-0 flex-wrap content-start items-center gap-x-1.5 overflow-hidden leading-4 whitespace-nowrap text-fg-muted"
        >
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <button
                type="button"
                aria-label="Model menu"
                title={[full, shared ? "Used by another app" : null, runtime.modelPath && visiblePath(runtime.modelPath)]
                  .filter(Boolean)
                  .join("\n")}
                className="flex max-w-full min-w-0 items-center gap-1 overflow-hidden rounded hover:text-fg hover:underline focus-visible:underline"
              >
                <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-success" aria-hidden />
                {parts ? (
                  <>
                    <span className="hidden h-4 min-w-0 flex-wrap items-center overflow-hidden leading-4 @min-[13.5rem]/ready:flex">
                      <span className="shrink-0" data-ready="remembers">
                        {parts.remembers}
                      </span>
                      {parts.reason && (
                        <span className="whitespace-pre" data-ready="reason">
                          {parts.reason}
                        </span>
                      )}
                    </span>
                    <span className="hidden @min-[9.5rem]/ready:inline @min-[13.5rem]/ready:hidden" data-ready="about">
                      {parts.about}
                    </span>
                    <span className="@min-[9.5rem]/ready:hidden" data-ready="words">
                      {parts.words}
                    </span>
                  </>
                ) : (
                  <span>{full}</span>
                )}
              </button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content
                align="start"
                sideOffset={4}
                className="z-50 max-w-sm min-w-64 rounded-xl border border-border bg-surface p-1.5 shadow-pop"
              >
                <DropdownMenu.Label className="px-2.5 py-1 text-xs text-fg-muted">
                  <bdi dir="ltr">{visiblePath(runtime.modelName ?? "Local model")}</bdi> · runs on{" "}
                  {backendWords(runtime.backend)}
                  <span className="block">{full}</span>
                  {shared && <span className="block text-accent">Used by another app</span>}
                </DropdownMenu.Label>
                {notes.map((n) => (
                  <p key={n.text} className="px-2.5 py-1 text-xs text-warning">
                    {n.text}
                  </p>
                ))}
                <DropdownMenu.Separator className="my-1 h-px bg-border" />
                <DropdownMenu.Item className={MENU_ITEM} onSelect={() => reload(true)}>
                  <RotateCcw size={14} aria-hidden /> Reload
                </DropdownMenu.Item>
                <DropdownMenu.Item className={MENU_ITEM} onSelect={settings}>
                  <Settings2 size={14} aria-hidden /> Model settings…
                </DropdownMenu.Item>
                <DropdownMenu.Item
                  className={MENU_ITEM}
                  onSelect={() => void unloadModel(useApp.getState().runtime).catch(fail)}
                >
                  <PowerOff size={14} aria-hidden /> Unload to free memory
                </DropdownMenu.Item>
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
          {shared && (
            <span className="flex items-center gap-1.5 text-accent">
              <span aria-hidden>·</span>Used by another app
            </span>
          )}
          {/* A pointer shortcut for the menu's Model settings…, which the keyboard and
              screen readers use (this one may sit on the clipped row). */}
          <span className="flex items-center gap-1.5" aria-hidden>
            <span>·</span>
            <button type="button" tabIndex={-1} onClick={settings} className="underline hover:text-fg">
              Change
            </button>
          </span>
        </div>
        {waitingLine ? (
          <div className="flex max-w-[34rem] min-w-0 items-center gap-1.5 text-[11px] text-warning" role="status">
            <span className="truncate">{waitingWords(waiting ?? running ?? "reload", busy)}</span>
            <span aria-hidden>·</span>
            <button
              type="button"
              onClick={() => void ipc.cancelPendingReload().catch(fail)}
              className="shrink-0 underline hover:text-fg"
            >
              Cancel
            </button>
          </div>
        ) : (
          top && (
            <div className="flex max-w-[34rem] min-w-0 items-start gap-1.5 text-[11px] text-warning" role="status">
              <span className="line-clamp-2 min-w-0" title={top.text}>
                {top.text}
              </span>
              {top.action && (
                <button type="button" onClick={() => run(top.action as NoteAction)} className="shrink-0 underline hover:text-fg">
                  {ACTION_LABEL[top.action]}
                </button>
              )}
            </div>
          )
        )}
      </div>
    );
  }

  if (runtime.state === "failed") {
    // Why, as visible text: the error, else what the load had to change before it gave up.
    const why =
      runtime.error?.message ||
      runtime.adjustments.map(adjustmentWords).find((t): t is string => t !== null) ||
      null;
    // [0.0.6 header-layout r2] Both actions always show, so the keyboard never lands on one that
    // cannot be seen: "Didn't load · Model settings…" on the first line (the line asks the header
    // for that much, `min-w`), and "Try again" after the reason on the second, the way a note's
    // action follows its words. The reason shows in two lines at most; its tooltip has it whole.
    return (
      <div
        data-header-state="failed"
        className="flex min-w-[10.5rem] flex-1 flex-col justify-center gap-0.5 text-xs leading-tight"
      >
        <span data-failed-line className="flex min-w-0 items-center gap-x-1.5 whitespace-nowrap">
          <span className="shrink-0 text-danger">Didn't load</span>
          <span aria-hidden className="text-fg-muted">
            ·
          </span>
          <button type="button" onClick={settings} className="shrink-0 text-fg-muted underline hover:text-fg">
            Model settings…
          </button>
        </span>
        {(why || runtime.modelPath) && (
          <span className="flex max-w-[34rem] min-w-0 items-start gap-1.5 text-[11px] text-fg-muted">
            {why && (
              <span className="line-clamp-2 min-w-0 wrap-anywhere" role="status" title={why}>
                {why}
              </span>
            )}
            {runtime.modelPath && (
              <button
                type="button"
                onClick={() => runtime.modelPath && void switchModel(runtime, runtime.modelPath).catch(fail)}
                className="shrink-0 underline hover:text-fg"
              >
                Try again
              </button>
            )}
          </span>
        )}
      </div>
    );
  }

  if (runtime.state === "stopping") {
    return <span className="text-xs whitespace-nowrap text-fg-muted">Unloading…</span>;
  }

  // Stopped: unloaded by the user, or never loaded since the app started. In the narrowest
  // header, where the usage line's icons keep their room (0.0.6 item 6), only "Load" shows,
  // whole; the state is its tooltip (and the message box's placeholder says it too).
  const unloaded = runtime.unloadedPath !== null;
  const stateWords = unloaded ? "Unloaded to free memory" : "Not loaded";
  return (
    <span className="flex min-w-0 items-center gap-1.5 text-xs whitespace-nowrap text-fg-muted" title={stateWords}>
      <span className="truncate @max-xl:hidden">{stateWords}</span>
      <span aria-hidden className="@max-xl:hidden">
        ·
      </span>
      <button
        type="button"
        onClick={() => chatPath && void useApp.getState().pickModel(`local:${chatPath}`)}
        className="underline hover:text-fg"
      >
        Load
      </button>
    </span>
  );
}
