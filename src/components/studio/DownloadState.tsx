import { useId, useState, type ReactNode } from "react";
import clsx from "clsx";
import { ArrowRight, CircleCheck, Pause, Play, X } from "lucide-react";
import { formatBytes, formatEta } from "../../lib/format";
import * as ipc from "../../lib/ipc";
import type { DownloadGroup, ErrorInfo } from "../../lib/types";
import { LOCAL_MODELS, STUDIO_DOWNLOADS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useDownloads } from "../../store/downloads";
import { useStudio } from "../../store/studio";
import { Tip } from "../ui/Tip";
import { FOCUS_RING } from "../ui/styles";

// A Studio download as every place that shows one draws it (0.2.0 Task U7, design 10.3): a
// model's card in Get models, the detail panel, a LoRA's or a file's row there, and the top
// bar's indicator (DownloadIndicator.tsx). While it runs: how far it got in one size format
// (`formatBytes`, the sizes the cards show: critique §2.7 and §6 found two on one card), the time
// left or what it is doing, its bar, Pause or Resume and Cancel, named for the model; once done,
// "Ready" and, for a model the composer can use, *Use it*. Also here: which download of a model
// a card or the panel shows, and *Use it* itself (the model chosen, the cursor in the prompt).

/** A download still running (queued, downloading, paused, checking, failed and waiting for
 *  Resume, or being cancelled): neither done nor cancelled. */
export function isRunning(g: DownloadGroup): boolean {
  return g.status !== "done" && g.status !== "cancelled";
}

/** A download Studio shows: a picture or video model's, or a LoRA's. */
export function isStudioDownload(g: DownloadGroup): boolean {
  return Boolean(g.genModelId || g.loraId);
}

/** The download of local model `genModelId` while it runs, from the app's download groups;
 *  `null` when none is (never started, done or cancelled). */
export function activeDownload(groups: readonly DownloadGroup[], genModelId: string): DownloadGroup | null {
  return groups.find((g) => g.genModelId === genModelId && isRunning(g)) ?? null;
}

/** The download of local model `genModelId` while it runs; `null` when none is. */
export function useModelDownload(genModelId: string): DownloadGroup | null {
  return useDownloads((s) => activeDownload(s.groups, genModelId));
}

/** The download a model's card or the detail panel shows for the models `match` picks: the one
 *  running, else one that finished while Studio followed it (the store's `doneDownloads`, until
 *  it is used or cleared), which shows "Ready" with *Use it*; `null` otherwise. */
export function useShownDownload(match: (genModelId: string) => boolean): DownloadGroup | null {
  const done = useStudio((s) => s.doneDownloads);
  return useDownloads((s) => {
    const mine = s.groups.filter((g) => g.genModelId != null && match(g.genModelId));
    return mine.find(isRunning) ?? mine.find((g) => g.status === "done" && done.includes(g.id)) ?? null;
  });
}

/** *Use it*: installed model `genModelId` chosen in its tab's composer, back at the results
 *  (`pickNewModel`), with the cursor in the prompt, ready for words. */
export function pickNewModelAndFocus(genModelId: string) {
  useStudio.getState().pickNewModel(genModelId);
  requestAnimationFrame(() => window.dispatchEvent(new Event("app:focus-studio-prompt")));
}

/** What a download is doing after its sizes: the time left while it downloads (when known),
 *  else its state; nothing while it downloads without an estimate. */
function stateWords(g: DownloadGroup): string | null {
  if (g.status === "downloading") {
    if (g.checking === true) return STUDIO_DOWNLOADS.checking;
    return formatEta(g.bytesTotal - g.bytesDone, g.bytesPerSecond);
  }
  // The backend sends the state as a string: one this version does not know says nothing.
  return STUDIO_DOWNLOADS.status[g.status] ?? null;
}

/** How far a download got, in one size format: "2.4 GB of 6.7 GB · 2 min left". */
export function progressWords(g: DownloadGroup): string {
  const words = STUDIO_DOWNLOADS.progress(formatBytes(g.bytesDone), formatBytes(g.bytesTotal), stateWords(g));
  return g.unverified ? `${words} · ${STUDIO_DOWNLOADS.unverified}` : words;
}

const ICON_BUTTON = `grid size-7 shrink-0 place-items-center rounded-lg text-fg-muted hover:bg-bg-muted hover:text-fg disabled:opacity-50 ${FOCUS_RING}`;

/** A small icon button with its tooltip; `label` names it for what it does to which download. */
function IconButton(props: { label: string; tip: string; danger?: boolean; disabled: boolean; onClick(): void; children: ReactNode }) {
  return (
    <Tip content={props.tip}>
      <button
        type="button"
        aria-label={props.label}
        disabled={props.disabled}
        onClick={props.onClick}
        className={clsx(ICON_BUTTON, props.danger && "hover:text-danger")}
      >
        {props.children}
      </button>
    </Tip>
  );
}

/**
 * A download: its bar, "2.4 GB of 6.7 GB · 2 min left" (or what it is doing), and Pause (or
 * Resume, once paused or stopped) and Cancel, each named for `name`; why it stopped, and why a
 * press was refused. Once done, "Ready" and, given `onUse`, *Use it* (`size` "md" in the detail
 * panel's foot, beside where Download was).
 */
export function DownloadState(props: { group: DownloadGroup; name: string; onUse?(): void; size?: "sm" | "md" }) {
  const { group: g, name } = props;
  const nameId = useId();
  const [busy, setBusy] = useState(false);
  const [refused, setRefused] = useState<ErrorInfo | null>(null);

  if (g.status === "done") {
    return (
      <div data-download-state="done" className="flex w-full flex-wrap items-center justify-between gap-2">
        <span className="inline-flex items-center gap-1.5 text-sm font-medium text-success">
          <CircleCheck size={16} aria-hidden />
          {STUDIO_DOWNLOADS.ready}
        </span>
        {props.onUse && (
          <>
            <span id={nameId} hidden>
              {name}
            </span>
            <button
              type="button"
              aria-describedby={nameId}
              onClick={props.onUse}
              className={clsx(
                "btn-primary inline-flex items-center gap-1.5 text-sm",
                props.size === "md" ? "h-9 rounded-xl px-4 font-semibold" : "h-8 rounded-lg px-3 font-medium",
                FOCUS_RING,
              )}
            >
              {STUDIO_DOWNLOADS.useIt}
              <ArrowRight size={15} aria-hidden />
            </button>
          </>
        )}
      </div>
    );
  }

  /** Runs a command on the download; its new state comes as `download-progress`. */
  function act(command: (groupId: string) => Promise<void>) {
    setBusy(true);
    setRefused(null);
    command(g.id)
      .catch((e: unknown) => setRefused(toErrorInfo(e)))
      .finally(() => setBusy(false));
  }

  const pct = g.bytesTotal > 0 ? Math.min(100, (g.bytesDone / g.bytesTotal) * 100) : 0;
  const stalled = g.status === "paused" || g.status === "error" || g.status === "cancelled";
  const canPause = g.status === "downloading" || g.status === "queued";
  const canResume = g.status === "paused" || g.status === "error";
  const canCancel = isRunning(g) && g.status !== "cancelling";
  return (
    <div data-download-state={g.status} className="flex w-full min-w-0 flex-col gap-1.5">
      <div className="flex min-h-7 items-center gap-2">
        <p className="min-w-0 flex-1 text-sm text-fg-muted tabular-nums">{progressWords(g)}</p>
        <div className="-mr-1 flex shrink-0 items-center gap-0.5">
          {canPause && (
            <IconButton label={STUDIO_DOWNLOADS.pause(name)} tip={STUDIO_DOWNLOADS.tip.pause} disabled={busy} onClick={() => act(ipc.pauseDownload)}>
              <Pause size={15} aria-hidden />
            </IconButton>
          )}
          {canResume && (
            <IconButton label={STUDIO_DOWNLOADS.resume(name)} tip={STUDIO_DOWNLOADS.tip.resume} disabled={busy} onClick={() => act(ipc.resumeDownload)}>
              <Play size={15} aria-hidden />
            </IconButton>
          )}
          {canCancel && (
            <IconButton label={STUDIO_DOWNLOADS.cancel(name)} tip={STUDIO_DOWNLOADS.tip.cancel} danger disabled={busy} onClick={() => act(ipc.cancelDownload)}>
              <X size={15} aria-hidden />
            </IconButton>
          )}
        </div>
      </div>
      <div
        className="h-1.5 overflow-hidden rounded-full bg-bg-muted"
        role="progressbar"
        aria-label={LOCAL_MODELS.downloading(name)}
        aria-valuenow={Math.round(pct)}
        aria-valuemin={0}
        aria-valuemax={100}
      >
        <div
          className={clsx("h-full rounded-full transition-[width] duration-300 ease-out", stalled ? "bg-fg-muted/40" : "brand-gradient")}
          style={{ width: `${pct}%` }}
        />
      </div>
      {g.error && (
        <p className="text-sm text-danger">
          {g.error.message} {g.error.nextStep}
        </p>
      )}
      {refused && (
        <p role="alert" className="text-sm text-danger">
          {refused.message} {refused.nextStep}
        </p>
      )}
    </div>
  );
}
