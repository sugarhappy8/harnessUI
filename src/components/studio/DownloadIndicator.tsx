import { useMemo, useRef, useState } from "react";
import { Popover } from "radix-ui";
import clsx from "clsx";
import { ArrowDown, CircleCheck } from "lucide-react";
import type { DownloadGroup } from "../../lib/types";
import { DOWNLOAD_KIND_WORDS, STUDIO_DOWNLOADS } from "../../lib/words";
import { useDownloads } from "../../store/downloads";
import { canPick, useStudio } from "../../store/studio";
import { visiblePath } from "../FolderChip";
import { FOCUS_RING } from "../ui/styles";
import { DownloadState, isRunning, isStudioDownload, pickNewModelAndFocus } from "./DownloadState";

// The top bar's download indicator (0.2.0 Task U7, design 10.3): while a Studio download runs
// (a picture or video model's, or a LoRA's), "2 downloading" with a ring for how far they got
// together; once none runs, "1 ready" while finished ones are listed. Hidden with neither. Its
// popover lists them, newest first, each a DownloadState with Pause and Cancel, and a finished
// model *Use it* (a LoRA is attached in Settings, not chosen as a model: none for it). *Clear
// finished* takes the finished ones off the list.

/** The downloads the indicator lists: Studio's that run, and those that finished while Studio
 *  followed them (`done`), in the download manager's order (newest first). */
export function indicatorGroups(groups: readonly DownloadGroup[], done: readonly string[]): DownloadGroup[] {
  return groups.filter((g) => isStudioDownload(g) && (isRunning(g) || (g.status === "done" && done.includes(g.id))));
}

/** How far the running downloads got together, from 0 to 100. */
function together(running: DownloadGroup[]): number {
  const total = running.reduce((n, g) => n + g.bytesTotal, 0);
  const done = running.reduce((n, g) => n + Math.min(g.bytesDone, g.bytesTotal), 0);
  return total > 0 ? Math.round((done / total) * 100) : 0;
}

/** A ring for how far the downloads got, around a down arrow; muted while none moves. */
function Ring(props: { pct: number; stalled: boolean }) {
  return (
    <span aria-hidden className={clsx("relative grid size-5 shrink-0 place-items-center", props.stalled ? "text-fg-muted" : "text-accent")}>
      <svg viewBox="0 0 20 20" className="absolute inset-0 -rotate-90">
        <circle cx="10" cy="10" r="8" fill="none" strokeWidth="2" className="stroke-border-strong" />
        <circle
          cx="10"
          cy="10"
          r="8"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          pathLength={100}
          strokeDasharray={`${props.pct} 100`}
          className="transition-[stroke-dasharray] duration-300 ease-out"
        />
      </svg>
      <ArrowDown size={11} strokeWidth={2.5} />
    </span>
  );
}

/** The indicator, on the top bar beside *Get models*; `null` with no Studio download to list. */
export function DownloadIndicator() {
  const groups = useDownloads((s) => s.groups);
  const done = useStudio((s) => s.doneDownloads);
  const clearDone = useStudio((s) => s.clearDoneDownloads);
  const [open, setOpen] = useState(false);
  const content = useRef<HTMLDivElement>(null);
  // *Use it* puts the cursor in the prompt: focus does not go back to the indicator then.
  const used = useRef(false);
  const listed = useMemo(() => indicatorGroups(groups, done), [groups, done]);
  if (listed.length === 0) return null;
  const running = listed.filter(isRunning);
  const finished = listed.length - running.length;
  const stalled = running.every((g) => g.status === "paused" || g.status === "error");
  const words = running.length > 0 ? STUDIO_DOWNLOADS.indicator(running.length) : STUDIO_DOWNLOADS.indicatorReady(finished);
  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>
        <button
          type="button"
          data-download-indicator
          className={`flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2 text-sm font-medium whitespace-nowrap tabular-nums hover:bg-bg-muted data-[state=open]:bg-bg-muted ${FOCUS_RING}`}
        >
          {running.length > 0 ? (
            <Ring pct={together(running)} stalled={stalled} />
          ) : (
            <CircleCheck size={17} aria-hidden className="shrink-0 text-success" />
          )}
          <span className="@max-3xl:sr-only">{words}</span>
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          collisionPadding={8}
          ref={content}
          aria-label={STUDIO_DOWNLOADS.title}
          data-download-list
          onOpenAutoFocus={(e) => {
            // The list itself, not its first button, whose tooltip would cover a name; Tab
            // goes on to the buttons.
            e.preventDefault();
            used.current = false;
            content.current?.focus();
          }}
          onCloseAutoFocus={(e) => {
            if (used.current) e.preventDefault();
          }}
          className="z-50 flex outline-none max-h-[min(30rem,var(--radix-popover-content-available-height))] w-[23rem] max-w-[calc(100vw-1rem)] flex-col overflow-hidden rounded-2xl border border-border bg-surface text-fg shadow-pop"
        >
          <div className="flex items-center justify-between gap-3 border-b border-border px-4 py-2.5">
            <h2 className="text-section">{STUDIO_DOWNLOADS.title}</h2>
            {finished > 0 && (
              <button
                type="button"
                onClick={clearDone}
                className={`-mr-1.5 rounded-md px-1.5 py-0.5 text-sm text-fg-muted hover:bg-bg-muted hover:text-fg ${FOCUS_RING}`}
              >
                {STUDIO_DOWNLOADS.clear}
              </button>
            )}
          </div>
          <ul className="min-h-0 divide-y divide-border overflow-y-auto">
            {listed.map((g) => (
              <Row
                key={g.id}
                group={g}
                onUsed={() => {
                  used.current = true;
                  setOpen(false);
                }}
              />
            ))}
          </ul>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** One download in the list: its name and what it is, then its state; a finished model that its
 *  tab can use offers *Use it*, which closes the list. */
function Row(props: { group: DownloadGroup; onUsed(): void }) {
  const { group: g } = props;
  const pickable = useStudio((s) => g.genModelId != null && g.status === "done" && canPick(s.local, g.genModelId));
  // A name from a search can hold any character: shown with invisible ones as codes.
  const name = visiblePath(g.label);
  const kind = g.loraId ? STUDIO_DOWNLOADS.lora : g.generatorKind ? DOWNLOAD_KIND_WORDS[g.generatorKind] : null;
  const use = () => {
    props.onUsed();
    if (g.genModelId) pickNewModelAndFocus(g.genModelId);
  };
  return (
    <li data-download-row={g.id} className="flex flex-col gap-1.5 px-4 py-3">
      <div className="flex min-w-0 items-baseline justify-between gap-3">
        <p className="min-w-0 truncate text-sm font-medium">
          <bdi dir="ltr">{name}</bdi>
        </p>
        {kind && <span className="text-meta shrink-0">{kind}</span>}
      </div>
      <DownloadState group={g} name={name} onUse={pickable ? use : undefined} />
    </li>
  );
}
