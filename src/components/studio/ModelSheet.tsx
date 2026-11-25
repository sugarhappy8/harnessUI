import { useId, useRef, useState, useSyncExternalStore, type KeyboardEvent, type ReactNode, type RefObject } from "react";
import { Popover } from "radix-ui";
import {
  Check,
  Cloud,
  Download,
  FilePlus2,
  History,
  ImagePlus,
  Info,
  KeyRound,
  Monitor,
  RefreshCw,
  Search,
  SquareDashed,
  Wrench,
} from "lucide-react";
import { clsx } from "clsx";
import type { ErrorInfo } from "../../lib/types";
import { IMPORT, MODEL_SHEET, SETUP, STUDIO_ACTIONS, STUDIO_COMPANY, STUDIO_DOCK, STUDIO_PICKER } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { FITTING, fittingEntry, makesKind, useStudio, type SheetFilter, type StudioTab } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { ChipGroup } from "../ui/ChipGroup";
import { SlideOver } from "../ui/SlideOver";
import { FOCUS_RING } from "../ui/styles";
import { Tip } from "../ui/Tip";
import {
  capabilities,
  fitWords,
  installedHere,
  priceLine,
  sheetSections,
  sizeWords,
  typeLabel,
  warns,
  type SheetCloudGroup,
  type SheetModel,
} from "./modelSheet";
import { SECONDARY_BUTTON } from "./styles";
import type { InstalledLocal } from "./types";

// The model sheet (0.2.0 Task U4, design 10.2), which replaces 0.1.x's model picker: the
// composer's model chip opens it, anchored to the chip as a large popover, or as a full-height
// sheet from the right on a window narrower than 1000 px. At its top a search box, focused as it
// opens, and the All · On this computer · Cloud chips; the kind follows the tab. Then Recent (up
// to four tiles, not repeated below), On this computer (each installed model on one line: its
// name, type, size, a fit dot whose tooltip is the warning when it may not fit, its LoRAs, and
// what it lacks) and each cloud endpoint's models (name, at most two icons for what it can do,
// its notes behind an info icon, and its price). An endpoint without a key, or without its lists,
// is one row with its fix. Its foot has *Add a model file* and *Get more models*. The arrow keys
// move through the models, Enter picks one (the sheet closes), Esc closes it, and focus goes back
// to the chip. What it lists is worked out in modelSheet.ts.

/** Below this window width the sheet slides in at full height (0.1.2's panel narrows here). */
export const SHEET_MIN_WIDTH = 1000;

/** Whether the window is narrower than `SHEET_MIN_WIDTH`, followed as it is resized. */
function useNarrowWindow(): boolean {
  return useSyncExternalStore(
    (changed) => {
      window.addEventListener("resize", changed);
      return () => window.removeEventListener("resize", changed);
    },
    () => window.innerWidth < SHEET_MIN_WIDTH,
  );
}

/** The most models (and fixes) a list has for the popover to fit its height to them. */
const SHORT_LIST = 6;

/** How many tiles, rows and fixes the sheet lists for `tab` before any search. */
function sheetRows(s: Parameters<typeof sheetSections>[0], tab: StudioTab): number {
  const all = sheetSections(s, tab, "", "all");
  return all.recent.length + all.local.length + all.cloud.reduce((n, g) => n + g.rows.length + (g.fix ? 1 : 0), 0);
}

/** Settings → Endpoints. */
function openEndpoints() {
  window.dispatchEvent(new CustomEvent("app:open-settings", { detail: "endpoints" }));
}

/** The model sheet of `tab`, anchored to the chip `anchor` holds (its button gets focus back). */
export function ModelSheet(props: { tab: StudioTab; anchor: RefObject<HTMLElement | null> }) {
  const { tab, anchor } = props;
  const open = useStudio((s) => s.sheetOpen);
  const asked = useStudio((s) => s.sheetFilter);
  const openSheet = useStudio((s) => s.openSheet);
  const narrow = useNarrowWindow();
  // A short list makes a shorter popover (first use); a long one keeps its size as the search
  // narrows it, so the search box stays where it is.
  const long = useStudio((s) => s.sheetOpen && sheetRows(s, tab) > SHORT_LIST);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<SheetFilter>(asked);
  // Each opening starts afresh, on the filter it was asked for.
  const [seen, setSeen] = useState(open);
  if (open !== seen) {
    setSeen(open);
    if (open) {
      setQuery("");
      setFilter(asked);
    }
  }
  const searchRef = useRef<HTMLInputElement>(null);
  // The popover closed because focus went elsewhere (a press outside it, or a dialog it opened):
  // it then stays there rather than going back to the chip.
  const leftFocus = useRef(false);
  const chipButton = () => anchor.current?.querySelector<HTMLElement>("button") ?? null;

  const body = (
    <SheetBody
      tab={tab}
      layout={narrow ? "sheet" : "popover"}
      query={query}
      onQuery={setQuery}
      filter={filter}
      onFilter={setFilter}
      searchRef={searchRef}
      onClose={() => openSheet(false)}
      onHandOff={() => {
        leftFocus.current = true;
      }}
      chipButton={chipButton}
    />
  );

  if (narrow) {
    return (
      <SlideOver open={open} onOpenChange={openSheet} title={MODEL_SHEET.title} width="half" initialFocus={searchRef}>
        {body}
      </SlideOver>
    );
  }
  return (
    <Popover.Root open={open} onOpenChange={openSheet}>
      <Popover.Anchor virtualRef={anchor as RefObject<HTMLElement>} />
      <Popover.Portal>
        <Popover.Content
          side="top"
          align="start"
          sideOffset={8}
          collisionPadding={8}
          aria-label={MODEL_SHEET.title}
          data-model-sheet
          data-layout="popover"
          onOpenAutoFocus={(e) => {
            e.preventDefault();
            leftFocus.current = false;
            searchRef.current?.focus();
          }}
          onInteractOutside={(e) => {
            // A press of the chip itself closes the sheet from the chip.
            if (anchor.current?.contains(e.target as Node)) e.preventDefault();
            else leftFocus.current = true;
          }}
          onCloseAutoFocus={(e) => {
            // Back to the chip, as Radix does for a trigger: it has none here.
            e.preventDefault();
            if (!leftFocus.current) chipButton()?.focus();
          }}
          className={clsx(
            "z-50 flex w-[40rem] max-w-[calc(100vw-1rem)] flex-col overflow-hidden rounded-2xl border border-border bg-surface text-fg shadow-pop",
            long
              ? "h-[min(35rem,var(--radix-popover-content-available-height))]"
              : "max-h-[min(35rem,var(--radix-popover-content-available-height))]",
          )}
        >
          {body}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** The sheet's search, chips, lists and foot, in either layout. */
function SheetBody(props: {
  tab: StudioTab;
  layout: "popover" | "sheet";
  query: string;
  onQuery(query: string): void;
  filter: SheetFilter;
  onFilter(filter: SheetFilter): void;
  searchRef: RefObject<HTMLInputElement | null>;
  onClose(): void;
  /** Focus goes to something the sheet opened (a dialog): not back to the chip. */
  onHandOff(): void;
  chipButton(): HTMLElement | null;
}) {
  const { tab, query, filter, onQuery, onFilter, searchRef, onClose, onHandOff, chipButton } = props;
  const local = useStudio((s) => s.local);
  const endpoints = useStudio((s) => s.endpoints);
  const recentRuns = useStudio((s) => s.recentRuns);
  const drafts = useStudio((s) => s.drafts);
  const chooseModel = useStudio((s) => s.chooseModel);
  const showLocalModels = useStudio((s) => s.showLocalModels);
  const setSettingsOpen = useStudio((s) => s.setSettingsOpen);
  const openImport = useStudio((s) => s.openImport);
  const sections = sheetSections({ local, endpoints, recentRuns, drafts }, tab, query, filter);
  const localId = useId();

  const pick = (row: SheetModel) => {
    if (row.installed?.needsSetup) {
      // Its files are chosen in Settings → Files: the draft goes to it, and the panel opens
      // there, giving focus back to the chip as it closes.
      chooseModel(tab, "thisComputer", row.model.id);
      // The panel takes focus (and gives it back to the chip): the sheet does not take it back.
      onHandOff();
      onClose();
      chipButton()?.focus();
      setSettingsOpen(true, "files");
      return;
    }
    if (row.model.usable !== null) return;
    chooseModel(tab, row.target, row.model.id);
    onClose();
  };
  const getMore = () => {
    showLocalModels(true);
    onClose();
  };
  const addFile = () => {
    onHandOff();
    onClose();
    openImport("model");
  };
  const fixEndpoint = () => {
    onHandOff();
    onClose();
    openEndpoints();
  };

  // What On this computer says when nothing this computer can use is installed: the model that
  // fits to get, else why local models are not for this computer (or this kind).
  const searching = query.trim() !== "";
  let localLine: string | null = null;
  if (local && filter !== "cloud" && !searching) {
    const usable = installedHere(local, tab).filter((m) => m.model.usable === null);
    const entry = fittingEntry(tab, local);
    const nothingFits =
      (tab === "video" && local.videoOnProcessor) ||
      (local.moreFit === 0 && !local.installed.some((m) => m.model.usable === null && FITTING.has(m.fit.word)));
    if (usable.length === 0) {
      localLine = entry
        ? STUDIO_PICKER.noLocalYet(entry.name, entry.sizeBytes)
        : nothingFits
          ? STUDIO_PICKER.noLocalHere
          : STUDIO_PICKER.noKindHere[tab];
    }
  }
  const noEndpoint = filter !== "local" && !endpoints.some((e) => makesKind(e, tab));
  const nothing =
    searching &&
    sections.recent.length === 0 &&
    sections.local.length === 0 &&
    sections.cloud.every((g) => g.rows.length === 0);

  // Up and Down move through the tiles and rows, and from the first back to the search box;
  // Left and Right move along the tiles; Enter in the search box picks the first model listed.
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const target = e.target as HTMLElement;
    const items = Array.from(e.currentTarget.querySelectorAll<HTMLElement>("[data-sheet-item]"));
    const inSearch = target === searchRef.current;
    // From a row's own controls (its fit dot, its notes), as from the row.
    const item = target.closest<HTMLElement>("[data-sheet-row]")?.querySelector<HTMLElement>("[data-sheet-item]") ?? target;
    const at = items.indexOf(item);
    let next: HTMLElement | null | undefined = null;
    if (e.key === "ArrowDown") next = inSearch ? items[0] : at >= 0 ? (items[at + 1] ?? items[at]) : null;
    else if (e.key === "ArrowUp") next = at === 0 ? searchRef.current : at > 0 ? items[at - 1] : null;
    else if ((e.key === "ArrowRight" || e.key === "ArrowLeft") && "sheetTile" in target.dataset) {
      const tiles = items.filter((i) => "sheetTile" in i.dataset);
      next = tiles[tiles.indexOf(target) + (e.key === "ArrowRight" ? 1 : -1)];
    } else if (e.key === "Enter" && !inSearch && "sheetItem" in target.dataset) {
      // Enter presses a row, tile or link here, the browser's own press held back, so it works
      // the same in every webview.
      e.preventDefault();
      target.click();
      return;
    } else if (e.key === "Enter" && inSearch) {
      const first = items.find((i) => "sheetPick" in i.dataset && i.getAttribute("aria-disabled") !== "true");
      if (first) {
        e.preventDefault();
        first.click();
      }
      return;
    }
    if (next) {
      e.preventDefault();
      next.focus();
    }
  };

  const sheet = props.layout === "sheet";
  return (
    <div
      data-layout={sheet ? "sheet" : undefined}
      data-model-sheet={sheet ? "" : undefined}
      onKeyDown={onKeyDown}
      className={clsx("@container flex min-h-0 flex-col", sheet ? "-m-4 h-[calc(100%+2rem)]" : "flex-auto")}
    >
      <div className="flex flex-col gap-2.5 border-b border-border px-3 pt-3 pb-2.5">
        <label className="flex h-10 items-center gap-2 rounded-xl border border-border bg-bg px-3 focus-within:border-border-strong focus-within:ring-2 focus-within:ring-accent/40">
          <Search size={16} aria-hidden className="shrink-0 text-fg-muted" />
          <input
            ref={searchRef}
            type="search"
            aria-label={MODEL_SHEET.search}
            placeholder={`${MODEL_SHEET.search}…`}
            value={query}
            onChange={(e) => onQuery(e.target.value)}
            className="min-w-0 flex-1 bg-transparent text-[15px] outline-none placeholder:text-fg-muted [&::-webkit-search-cancel-button]:appearance-none"
          />
        </label>
        <ChipGroup
          label={MODEL_SHEET.filters}
          value={filter}
          onChange={onFilter}
          options={[
            { value: "all", label: MODEL_SHEET.all },
            { value: "local", label: MODEL_SHEET.local },
            { value: "cloud", label: MODEL_SHEET.cloud },
          ]}
        />
      </div>
      <div className="min-h-0 flex-auto overflow-y-auto px-2 pb-2">
        {sections.recent.length > 0 && <RecentTiles rows={sections.recent} onPick={pick} />}
        {(sections.local.length > 0 || localLine !== null) && (
          <section aria-labelledby={localId}>
            <SectionHead id={localId} icon={<Monitor size={14} aria-hidden />} title={MODEL_SHEET.local} note={STUDIO_PICKER.localTag} />
            {localLine !== null && <p className="px-2 py-1.5 text-sm text-fg-muted">{localLine}</p>}
            {sections.local.length > 0 && (
              <ul className="flex flex-col">
                {sections.local.map((row) => (
                  <SheetRow key={row.key} tab={tab} row={row} onPick={pick} />
                ))}
              </ul>
            )}
          </section>
        )}
        {sections.cloud.map((group) => (
          <CloudSection key={group.endpoint.providerId} tab={tab} group={group} onPick={pick} onFixKey={fixEndpoint} />
        ))}
        {noEndpoint && !searching && (
          <section>
            <SectionHead icon={<Cloud size={14} aria-hidden />} title={MODEL_SHEET.cloud} note={null} />
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-2 py-1.5">
              <p className="text-sm text-fg-muted">{STUDIO_DOCK.addEndpoint[tab]}</p>
              <button type="button" data-sheet-item onClick={fixEndpoint} className={`${LINK} shrink-0`}>
                {STUDIO_ACTIONS.openEndpoints}
              </button>
            </div>
          </section>
        )}
        {nothing && <p className="px-2 py-6 text-center text-sm text-fg-muted">{STUDIO_PICKER.nothingMatches}</p>}
      </div>
      <SheetFooter onAddFile={addFile} onGetMore={getMore} />
    </div>
  );
}

/** A link-like button inside the sheet: a fix, *Open Endpoints*. */
const LINK = `rounded-md px-1 text-sm font-medium text-accent hover:underline ${FOCUS_RING}`;

/** A section's heading, with its icon and a quiet note at its end. */
function SectionHead(props: { id?: string; icon: ReactNode; title: string; note: string | null }) {
  return (
    <div className="flex min-w-0 items-center gap-2 px-2 pt-3 pb-1 text-fg-muted">
      {props.icon}
      <h3 id={props.id} className="text-section shrink-0 text-fg">
        {props.title}
      </h3>
      {props.note && <span className="ml-auto min-w-0 truncate text-xs">{props.note}</span>}
    </div>
  );
}

/** Recent: up to four models as compact tiles across the top, each with where it runs (and the
 *  LoRAs the draft attaches to it). */
export function RecentTiles(props: { rows: SheetModel[]; onPick(row: SheetModel): void }) {
  const id = useId();
  return (
    <section aria-labelledby={id}>
      <SectionHead id={id} icon={<History size={14} aria-hidden />} title={MODEL_SHEET.recent} note={null} />
      <ul className="grid grid-cols-[repeat(auto-fit,minmax(8.5rem,1fr))] gap-2 px-1 pt-0.5">
        {props.rows.map((row) => {
          const here = row.installed !== null;
          return (
            <li key={row.key} className="flex min-w-0">
              <button
                type="button"
                data-sheet-item
                data-sheet-pick
                data-sheet-tile
                aria-current={row.selected || undefined}
                onClick={() => props.onPick(row)}
                className={`flex w-full min-w-0 flex-col items-start justify-between gap-0.5 rounded-xl border border-border bg-bg px-3 py-2 text-left hover:border-border-strong hover:bg-bg-muted aria-[current=true]:border-accent/60 aria-[current=true]:bg-accent/5 ${FOCUS_RING}`}
              >
                <span className="flex w-full min-w-0 items-start gap-1.5">
                  <span aria-hidden className={`mt-1.5 size-2 shrink-0 rounded-full ${here ? "bg-success" : "bg-accent"}`} />
                  <span data-sheet-name className="line-clamp-2 min-w-0 text-sm font-medium break-words">
                    {row.model.displayName}
                  </span>
                </span>
                <span className="w-full truncate pl-3.5 text-xs text-fg-muted">
                  {here ? STUDIO_COMPANY.local : (row.endpoint?.name ?? "")}
                  {row.loras > 0 && <span className="text-accent"> · {MODEL_SHEET.loraCount(row.loras)}</span>}
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/** A cloud endpoint's models under its name and where its prompt goes, or its fix: *Add a key*
 *  (Settings → Endpoints) or *Get the list*. */
function CloudSection(props: { tab: StudioTab; group: SheetCloudGroup; onPick(row: SheetModel): void; onFixKey(): void }) {
  const { group } = props;
  const id = useId();
  return (
    <section aria-labelledby={id} data-sheet-place="cloud">
      <SectionHead id={id} icon={<Cloud size={14} aria-hidden />} title={group.company} note={STUDIO_PICKER.goesTo(group.endpoint.host)} />
      {group.fix === "noKey" && (
        <div className="flex items-center gap-2 px-2 py-1.5">
          <KeyRound size={14} aria-hidden className="shrink-0 text-fg-muted" />
          <button type="button" data-sheet-item onClick={props.onFixKey} className={LINK}>
            {STUDIO_PICKER.noKey}
          </button>
        </div>
      )}
      {group.fix === "getList" && <GetListRow tab={props.tab} group={group} />}
      {group.rows.length > 0 && (
        <ul className="flex flex-col">
          {group.rows.map((row) => (
            <SheetRow key={row.key} tab={props.tab} row={row} onPick={props.onPick} />
          ))}
        </ul>
      )}
    </section>
  );
}

/** In place of the models of an endpoint whose lists are not saved yet: *Get the list* fetches
 *  them (lists are fetched only when asked), and its models then take the row's place; a
 *  failure says why under it. */
function GetListRow(props: { tab: StudioTab; group: SheetCloudGroup }) {
  const refreshModels = useStudio((s) => s.refreshModels);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const { endpoint } = props.group;
  const getList = () => {
    setBusy(true);
    setError(null);
    refreshModels(endpoint.providerId)
      .catch((e: unknown) => setError(toErrorInfo(e)))
      .finally(() => setBusy(false));
  };
  return (
    <div className="flex flex-col gap-1.5 px-2 py-1.5">
      <div className="flex items-center gap-2">
        <RefreshCw size={14} aria-hidden className={clsx("shrink-0 text-fg-muted", busy && "animate-spin")} />
        <button
          type="button"
          data-sheet-item
          aria-label={STUDIO_PICKER.getList(endpoint.name)}
          disabled={busy}
          onClick={getList}
          className={`${LINK} disabled:opacity-60`}
        >
          {STUDIO_ACTIONS.getTheList}
        </button>
      </div>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
    </div>
  );
}

/**
 * One model on one line. On this computer: its name, its type ("SDXL · picture"), "untested
 * set-up" where true, its size, the LoRAs the draft attaches, and a fit dot whose tooltip is the
 * warning or its fit and time; a model that needs setting up says so instead (picking it opens
 * Settings → Files). In the cloud: its name, at most two icons for what it can do, its price or
 * "price unknown". A model's notes are behind an info icon. A model Studio cannot use says why
 * and cannot be picked. The fit dot and the info icon are beside the row's button, so each is
 * reached by Tab and shows its tooltip on focus.
 */
export function SheetRow(props: { tab: StudioTab; row: SheetModel; onPick(row: SheetModel): void }) {
  const { row } = props;
  const { model, installed } = row;
  const needsSetup = installed?.needsSetup ?? false;
  const unusable = model.usable !== null && !needsSetup;
  const notes = model.notes.length > 0 ? model.notes.join(" ") : null;
  const label = installed ? typeLabel(installed) : null;

  let detail: ReactNode;
  if (unusable) {
    detail = <span className="min-w-0 truncate text-sm text-fg-muted">{model.usable}</span>;
  } else if (installed && needsSetup) {
    detail = (
      <span className="flex min-w-0 items-center gap-1 text-sm text-warning">
        <Wrench size={13} aria-hidden className="shrink-0" />
        <span className="truncate">{MODEL_SHEET.needsSetup}</span>
      </span>
    );
  } else if (installed) {
    detail = (
      <span className="flex min-w-0 items-center gap-1.5 text-sm text-fg-muted">
        {label && <span className="min-w-0 truncate">{label}</span>}
        {installed.untested && (
          <span className="shrink-0 rounded-full bg-warning/10 px-1.5 text-xs text-warning">{SETUP.untested}</span>
        )}
      </span>
    );
  } else {
    detail = (
      <span className="flex shrink-0 items-center gap-1 text-fg-muted">
        {capabilities(model).map((c) => (
          <Tip key={c} content={MODEL_SHEET[c]}>
            <span role="img" aria-label={MODEL_SHEET[c]} className="grid size-5 place-items-center rounded-md bg-bg-muted">
              {c === "canEdit" ? <ImagePlus size={12} aria-hidden /> : <SquareDashed size={12} aria-hidden />}
            </span>
          </Tip>
        ))}
      </span>
    );
  }

  const end = installed ? (needsSetup ? null : sizeWords(installed)) : row.endpoint && !unusable ? priceLine(model, row.endpoint.company) : null;

  return (
    <li
      data-sheet-row
      className={clsx(
        "flex min-w-0 items-center rounded-lg pr-1 hover:bg-bg-muted has-[[data-sheet-item]:focus-visible]:bg-bg-muted",
        row.selected && "bg-accent/5",
      )}
    >
      <button
        type="button"
        data-sheet-item
        data-sheet-pick
        aria-current={row.selected || undefined}
        aria-disabled={unusable || undefined}
        onClick={() => props.onPick(row)}
        className={clsx(
          "flex min-h-9 min-w-0 flex-1 items-center gap-2.5 rounded-lg py-1.5 pr-1 pl-2 text-left",
          unusable && "cursor-default",
          FOCUS_RING,
        )}
      >
        <Check size={14} aria-hidden className={clsx("shrink-0 text-accent", !row.selected && "invisible")} />
        <span
          data-sheet-name
          className={clsx("max-w-[60%] min-w-0 shrink-0 truncate text-sm font-medium", unusable ? "text-fg-muted" : "text-fg")}
        >
          {model.displayName}
        </span>
        {detail}
        <span className="ml-auto flex shrink-0 items-center gap-2 pl-2">
          {row.loras > 0 && (
            <span className="rounded-full bg-accent/10 px-1.5 text-xs font-medium text-accent">{MODEL_SHEET.loraCount(row.loras)}</span>
          )}
          {end && <span className="text-sm text-fg-muted tabular-nums">{end}</span>}
        </span>
      </button>
      <span className="flex w-12 shrink-0 items-center justify-end">
        {installed && !needsSetup && !unusable && <FitDot tab={props.tab} installed={installed} />}
        {notes && (
          <Tip content={notes}>
            <button
              type="button"
              aria-label={MODEL_SHEET.notes}
              className={`grid size-6 place-items-center rounded-full text-fg-muted hover:bg-bg hover:text-fg ${FOCUS_RING}`}
            >
              <Info size={14} aria-hidden />
            </button>
          </Tip>
        )}
      </span>
    </li>
  );
}

/** An installed model's fit dot: green when it fits, amber when slow or after the chat model is
 *  unloaded, red when it may not fit; its tooltip says which, or the warning. */
function FitDot(props: { tab: StudioTab; installed: InstalledLocal }) {
  const { installed } = props;
  const words = fitWords(installed, props.tab);
  const colour = warns(installed)
    ? "bg-danger"
    : installed.fit.verdict === "green"
      ? "bg-success"
      : "bg-warning";
  return (
    <Tip content={words}>
      <span role="img" tabIndex={0} aria-label={words} className={`grid size-6 place-items-center rounded-full ${FOCUS_RING}`}>
        <span aria-hidden className={`size-2 rounded-full ${colour}`} />
      </span>
    </Tip>
  );
}

/** The sheet's foot: *Add a model file*, and *Get more models* as its primary action. */
export function SheetFooter(props: { onAddFile(): void; onGetMore(): void }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border bg-bg-subtle/60 px-3 py-2.5">
      <button type="button" onClick={props.onAddFile} className={`${SECONDARY_BUTTON} inline-flex items-center gap-1.5 ${FOCUS_RING}`}>
        <FilePlus2 size={15} aria-hidden className="text-fg-muted" />
        {IMPORT.addModel}
      </button>
      <button
        type="button"
        onClick={props.onGetMore}
        className={`btn-primary inline-flex h-9 items-center gap-1.5 rounded-full px-4 text-sm font-medium ${FOCUS_RING}`}
      >
        <Download size={15} aria-hidden />
        {MODEL_SHEET.getMore}
      </button>
    </div>
  );
}
