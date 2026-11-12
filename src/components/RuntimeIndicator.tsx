import { useEffect, useRef, useState } from "react";
import { DropdownMenu } from "radix-ui";
import { ChevronDown, RotateCcw, Settings2, PowerOff } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { LoadStage, ModelBusy, RuntimeStatus, ShareStatus } from "../lib/types";
import {
  adjustmentWords,
  backendWords,
  loadStageWords,
  readyLine,
  SLOW_WORDS,
  slowSpeedWords,
  waitingWords,
} from "../lib/words";
import { toErrorInfo, useApp } from "../store/app";
import { reloadModel, switchModel, unloadModel, useRestartNote, useWaiting } from "../store/modelChange";
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
    add(adjustmentWords(a), a.kind === "steppingDown" ? "reload" : a.kind === "restarting" ? null : "settings");
  }
  const gpu = rt.backend !== null && rt.backend !== "cpu";
  const partial =
    gpu && rt.offloadedLayers !== null && rt.totalLayers !== null && rt.offloadedLayers < rt.totalLayers;
  if (partial && !adjustments.some((a) => a.kind === "retryingWithLessGpu")) add(SLOW_WORDS.partial, "settings");
  if (rt.slow && rt.speedTps !== null) add(slowSpeedWords(rt.speedTps), "settings");
  if (opts.restarted && opts.hasMessages) add(SLOW_WORDS.afterRestart, null);
  return notes;
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

/** Whether the page is shown (polling stops while the window is hidden). */
function usePageVisible(): boolean {
  const [visible, setVisible] = useState(() => typeof document === "undefined" || document.visibilityState === "visible");
  useEffect(() => {
    const on = () => setVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", on);
    return () => document.removeEventListener("visibilitychange", on);
  }, []);
  return visible;
}

/**
 * Calls `read` now and again after `delay(last result)` milliseconds while `active`; null
 * while inactive. `read` and `delay` must be stable (module-level) functions.
 */
function usePoll<T>(active: boolean, read: () => Promise<T>, delay: (last: T | null) => number): T | null {
  const [value, setValue] = useState<T | null>(null);
  useEffect(() => {
    if (!active) return;
    let alive = true;
    let timer = 0;
    const tick = async () => {
      let last: T | null = null;
      try {
        last = await read();
        if (alive) setValue(last);
      } catch {
        // Not available now; asked again on the next tick.
      }
      if (alive) timer = window.setTimeout(() => void tick(), delay(last));
    };
    void tick();
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [active, read, delay]);
  return active ? value : null;
}

/** The busy counts behind the waiting line: every 1.5 seconds. */
const busyDelay = () => 1500;
/** Sharing (for "Used by another app"): every 2 seconds while it is on, else every 10. */
const shareDelay = (last: ShareStatus | null) => (last && !last.enabled ? 10_000 : 2000);

const MENU_ITEM =
  "flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted";

/**
 * The local model's state in the chat header (0.0.5 D1, D3, D4): loading in plain words,
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
    return (
      <div className="enter-fade flex min-w-0 items-center gap-2 text-xs whitespace-nowrap text-fg-muted" aria-live="polite">
        <span className="truncate">
          Loading model{runtime.loadStage ? ` · ${loadStageWords(runtime.loadStage)}` : ""}
          {pct !== null ? ` ${pct}%` : "…"}
        </span>
        {pct !== null ? (
          <div className="h-1.5 w-24 shrink-0 overflow-hidden rounded-full bg-bg-muted" aria-hidden>
            <div
              className="brand-gradient h-full rounded-full transition-[width] duration-300 ease-out"
              style={{ width: `${pct}%` }}
            />
          </div>
        ) : (
          <div className="progress-indeterminate h-1.5 w-24 shrink-0 rounded-full bg-bg-muted" aria-hidden />
        )}
      </div>
    );
  }

  if (runtime.state === "ready") {
    const notes = runtimeNotes(runtime, { restarted, hasMessages: (detail?.messages.length ?? 0) > 0 });
    const top = notes[0] ?? null;
    const run = (action: NoteAction) =>
      action === "reload" ? reload(true) : action === "apply" ? reload(false) : settings();
    // One sentence that shortens from its end in a crowded header: the reason gives way
    // first, then the end of "Remembers about N words"; nothing runs under what follows.
    // The whole line, reason included, is always in the menu and the tooltip.
    const full = runtime.nCtx ? readyLine(runtime.nCtx, runtime.ctxSource) : "Ready";
    return (
      <div className="enter-fade flex min-w-0 flex-col justify-center gap-0.5 text-xs leading-tight">
        <div className="flex min-w-0 items-center gap-1.5 whitespace-nowrap text-fg-muted">
          <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-success" aria-hidden />
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <button
                type="button"
                aria-label="Model menu"
                title={runtime.modelPath ? `${full}\n${runtime.modelPath}` : full}
                className="flex min-w-0 items-center gap-1 overflow-hidden rounded hover:text-fg"
              >
                <span className="min-w-0 truncate">{full}</span>
                <ChevronDown size={12} className="shrink-0" aria-hidden />
              </button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content
                align="start"
                sideOffset={4}
                className="z-50 max-w-sm min-w-64 rounded-xl border border-border bg-surface p-1.5 shadow-pop"
              >
                <DropdownMenu.Label className="px-2.5 py-1 text-xs text-fg-muted">
                  {runtime.modelName ?? "Local model"} · runs on {backendWords(runtime.backend)}
                  <span className="block">{full}</span>
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
          {/* In a narrower header only the menu's Model settings… is left, so the number stays. */}
          <span className="hidden shrink-0 items-center gap-1.5 @5xl:flex">
            <span aria-hidden>·</span>
            <button type="button" onClick={settings} className="underline hover:text-fg">
              Change
            </button>
          </span>
          {share?.inUse && <span className="min-w-0 shrink-[100] truncate text-accent">· Used by another app</span>}
        </div>
        {/* In a narrower header the second line takes the first line's width, not more,
            so the model picker beside it keeps room for the model's name. */}
        {waitingLine ? (
          <div className="flex min-w-0 items-center gap-1.5 text-[11px] text-warning @max-5xl:w-0 @max-5xl:min-w-full" role="status">
            <span className="truncate">{waitingWords(waiting ?? "reload", busy)}</span>
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
            <div
              className="flex max-w-[34rem] min-w-0 items-start gap-1.5 text-[11px] text-warning @max-5xl:w-0 @max-5xl:min-w-full"
              role="status"
            >
              <span className="line-clamp-2" title={top.text}>
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
    return (
      <div className="flex min-w-0 flex-col justify-center gap-0.5 text-xs leading-tight">
        <span className="flex min-w-0 items-center gap-1.5 whitespace-nowrap">
          <span className="text-danger">Didn't load</span>
          <span aria-hidden className="text-fg-muted">
            ·
          </span>
          <button type="button" onClick={settings} className="text-fg-muted underline hover:text-fg">
            Model settings…
          </button>
          {runtime.modelPath && (
            <button
              type="button"
              onClick={() => runtime.modelPath && void switchModel(runtime, runtime.modelPath).catch(fail)}
              className="text-fg-muted underline hover:text-fg"
            >
              Try again
            </button>
          )}
        </span>
        {why && (
          <span className="line-clamp-2 max-w-[34rem] text-[11px] text-fg-muted" role="status" title={why}>
            {why}
          </span>
        )}
      </div>
    );
  }

  if (runtime.state === "stopping") {
    return <span className="text-xs whitespace-nowrap text-fg-muted">Unloading…</span>;
  }

  // Stopped: unloaded by the user, or never loaded since the app started.
  const unloaded = runtime.unloadedPath !== null;
  return (
    <span className="flex min-w-0 items-center gap-1.5 text-xs whitespace-nowrap text-fg-muted">
      <span className="truncate">{unloaded ? "Unloaded to free memory" : "Not loaded"}</span>
      <span aria-hidden>·</span>
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
