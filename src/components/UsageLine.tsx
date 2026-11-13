import { DropdownMenu } from "radix-ui";
import { Cpu, Gpu, MemoryStick, Settings2 } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { Reading, UsageNow } from "../lib/types";
import { usePageVisible, usePoll } from "../lib/usePoll";
import {
  amountGb,
  amountNumber,
  capacityGb,
  modelUsesWords,
  partsTotal,
  percentWords,
  placementWords,
  USAGE_MENU,
  usageGapWords,
  usageIcons,
  usageLine,
} from "../lib/words";
import { useApp } from "../store/app";
import { openModelSettings } from "./ModelSettings";

/** The usage line asks every 1.5 seconds while the window is visible (I6-9). */
const usageDelay = () => 1500;
/** One reading (looked up when called, so a missing command is just a failed reading). */
const readUsage = () => ipc.usageNow();

const ICON = { graphics: Gpu, processor: Cpu, memory: MemoryStick } as const;

/** One row of the menu: its words, and why a figure is missing on a line of its own. */
function Row(props: { text: string; why?: string | null; indent?: boolean }) {
  return (
    <div className={`px-2.5 py-0.5 text-xs ${props.indent ? "pl-5" : ""}`}>
      <p>{props.text}</p>
      {props.why && <p className="text-fg-muted">{props.why}</p>}
    </div>
  );
}

/** Why a reading is missing, as the visible text on its row; null when it is there. */
function whyOf<T>(r: Reading<T>, what: Parameters<typeof usageGapWords>[1], apple: boolean): string | null {
  return r.kind === "notAvailable" ? usageGapWords(r.why, what, apple) : null;
}

/** The menu's rows for this computer (6.1). Rows about the model show only while a local model is loaded. */
function UsageMenu(props: { now: UsageNow; localPath: string | null }) {
  const { now, localPath } = props;
  const runtime = useApp((s) => s.runtime);
  const apple = now.unified;
  // A backend without item 6 sends no `placement`.
  const placement = localPath !== null && runtime?.state === "ready" ? (runtime.placement ?? null) : null;
  const g = now.graphics;
  const graphicsName = apple ? "Apple graphics" : "Graphics card";

  const graphics =
    g.kind === "notAvailable" && g.why === "noGraphicsCard" ? null : g.kind === "value" ? (
      <>
        <Row text={g.value.name.trim() ? `${graphicsName} · ${g.value.name}` : graphicsName} />
        <Row
          indent
          text={`Busy: ${percentWords(g.value.busyPercent)}`}
          why={whyOf(g.value.busyPercent, "graphicsBusy", apple)}
        />
        {(() => {
          const used = g.value.memoryUsedBytes;
          const total = g.value.memoryTotalBytes;
          const why = whyOf(used, "graphicsMemory", apple);
          const shown = used.kind === "value" ? amountNumber(used.value) : used.kind === "measuring" ? "…" : "–";
          if (apple) {
            return (
              <Row
                indent
                text={used.kind === "value" ? `Apple graphics: ${amountGb(used.value)} of the computer's memory` : `Apple graphics: ${shown}`}
                why={why}
              />
            );
          }
          const of = total !== null && total > 0 ? `${shown} of ${capacityGb(total)}` : used.kind === "value" ? amountGb(used.value) : shown;
          return <Row indent text={`Memory: ${of} in use by all programs`} why={why} />;
        })()}
        {placement?.graphics && <Row indent text={modelUsesWords(placement.graphics.use)} />}
      </>
    ) : (
      <>
        <Row text={graphicsName} />
        <Row
          indent
          text={g.kind === "measuring" ? "Busy: …" : "Busy: –"}
          why={whyOf(g, "graphicsBusy", apple)}
        />
      </>
    );

  const cpu = now.processorPercent;
  const threads = `${now.threads.toLocaleString("en-US")} ${now.threads === 1 ? "thread" : "threads"}`;
  const cpuText = cpu.kind === "value" ? `Processor: ${percentWords(cpu)} busy · ${threads}` : `Processor: ${percentWords(cpu)} · ${threads}`;
  const m = now.memory;
  const modelHere = placement ? ` · the model: ${amountGb(partsTotal(placement.computer))}` : "";
  const memText =
    m.kind === "value"
      ? `Memory: ${amountNumber(m.value.usedBytes)} of ${capacityGb(m.value.totalBytes)} in use${modelHere}`
      : `Memory: ${m.kind === "measuring" ? "…" : "–"}${modelHere}`;

  return (
    <>
      <DropdownMenu.Label className="px-2.5 pt-1 pb-1.5 text-xs font-medium">{USAGE_MENU.title}</DropdownMenu.Label>
      <div className="space-y-0.5 pb-1">
        {graphics}
        <Row text={cpuText} why={whyOf(cpu, "processor", apple)} />
        <Row text={memText} why={whyOf(m, "memory", apple)} />
        {placement && <Row text={`Where the model went: ${placementWords(placement, apple)}`} />}
      </div>
      <p className="px-2.5 py-1 text-[11px] text-fg-muted">{USAGE_MENU.privacy}</p>
      {localPath !== null && (
        <>
          <DropdownMenu.Separator className="my-1 h-px bg-border" />
          <DropdownMenu.Item
            className="flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted"
            onSelect={() => openModelSettings(localPath)}
          >
            <Settings2 size={14} aria-hidden /> Model settings…
          </DropdownMenu.Item>
        </>
      )}
    </>
  );
}

/**
 * This computer's usage under the model name in the chat header (item 6, 6.1): how busy the
 * graphics card and the processor are and how much memory is in use, read on this computer
 * only, every 1.5 seconds while the window is visible. Always shown, whatever model the chat
 * uses (also none, and on the welcome screen): it is the computer's usage. Its form follows
 * the header's width (the header is the `@container`); the whole line opens a menu with every
 * figure, why one is missing, and where the loaded local model went.
 */
export function UsageLine() {
  const visible = usePageVisible();
  const now = usePoll<UsageNow>(visible, readUsage, usageDelay);
  const modelRef = useApp((s) => s.detail?.chat.modelRef ?? null);
  const localPath = modelRef?.startsWith("local:") ? modelRef.slice(6) : null;

  if (!now) {
    // Keeps the header's height while the first reading is on its way (or not answered).
    return <div className="h-[15px]" aria-hidden data-usage="waiting" />;
  }
  // The narrowest form; the header keeps room for its icons by their number (`data-icons`).
  const icons = usageIcons(now);
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          aria-label="Computer usage"
          // The whole line, also when a crowded header cuts it short.
          title={usageLine(now, "wide")}
          className="block h-[15px] w-full min-w-0 truncate px-2.5 text-left text-[11px] leading-[15px] text-fg-muted tabular-nums hover:text-fg focus-visible:underline @max-xl:pr-0"
        >
          <span className="hidden @5xl:inline" data-usage="wide">
            {usageLine(now, "wide")}
          </span>
          <span className="hidden @3xl:inline @5xl:hidden" data-usage="medium">
            {usageLine(now, "medium")}
          </span>
          <span className="hidden @xl:inline @3xl:hidden" data-usage="small">
            {usageLine(now, "small")}
          </span>
          <span className="inline-flex items-center gap-1.5 @xl:hidden" data-usage="icons" data-icons={icons.length}>
            {icons.map((i) => {
              const Icon = ICON[i.kind];
              return (
                <span key={i.kind} className="inline-flex items-center gap-0.5" data-usage-icon={i.kind}>
                  <span role="img" aria-label={i.label} title={i.label} className="inline-flex">
                    <Icon size={11} aria-hidden />
                  </span>
                  {i.value}
                </span>
              );
            })}
          </span>
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          sideOffset={4}
          className="z-50 max-w-sm min-w-72 rounded-xl border border-border bg-surface p-1.5 shadow-pop"
        >
          <UsageMenu now={now} localPath={localPath} />
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
