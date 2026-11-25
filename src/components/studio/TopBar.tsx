import { useEffect, useRef, useState } from "react";
import { DropdownMenu } from "radix-ui";
import { Download, Ellipsis, LayoutGrid, PanelLeft, RefreshCw, Search, Star } from "lucide-react";
import { listsUpdatedWords, STUDIO_BAR, STUDIO_NAV, STUDIO_TOPBAR } from "../../lib/words";
import { useStudio } from "../../store/studio";
import { FOCUS_RING } from "../ui/styles";
import { Tip } from "../ui/Tip";
import { formatUsd } from "./cost";
import { MENU_ITEM } from "./styles";
import { KindTabs } from "./KindTabs";

// Studio's top bar (0.1.0 Task B2; design 10.1 since Task U3): the title, [Pictures | Video],
// then the gallery's search, Starred and Grid, *Get models*, cloud spending today (only while
// a daily limit is on; Task G1 extends this to every spending switch) and the menu with *Update
// model lists*. Where the chosen model runs is the composer's model chip now: the place chip
// here looked clickable and was not (critique §6). While the grid is open, its own All,
// Pictures and Video choose what it shows, and [Pictures | Video] is hidden (Task B9). While
// Get local models is shown, its own [Pictures | Video] stand in for these, and the gallery's
// search, Starred and Grid, which act on the hidden cards, are hidden (Task B13). On a narrow
// Studio, Starred and Grid keep their icons and tooltips, and the search box narrows.

/** How long the search box waits after the last key before it asks for the gallery. */
const SEARCH_DELAY_MS = 250;

const TOGGLE =
  "flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2 text-sm whitespace-nowrap text-fg-muted hover:bg-bg-muted hover:text-fg " +
  "aria-pressed:bg-accent/10 aria-pressed:text-accent " +
  FOCUS_RING;

/** Title, [Pictures | Video] (not while the grid or Get local models is open), search, Starred
 *  and Grid (not while Get local models is open), *Get models*, cloud spending today while a
 *  limit is on, and the menu. */
export function TopBar(props: { onError: (e: unknown) => void }) {
  const setTab = useStudio((s) => s.setTab);
  const starred = useStudio((s) => s.filter.starred ?? false);
  const grid = useStudio((s) => s.grid);
  const localView = useStudio((s) => s.localView);
  const setFilter = useStudio((s) => s.setFilter);
  const setGrid = useStudio((s) => s.setGrid);

  return (
    <header className="glass @container z-10 flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-border px-3 py-2">
      {/* At the window's smallest the chat list leaves Studio's middle column little room. */}
      <button
        type="button"
        aria-label={STUDIO_BAR.chatList}
        title={STUDIO_BAR.chatList}
        onClick={() => window.dispatchEvent(new Event("app:toggle-sidebar"))}
        className={`rounded-lg p-1.5 text-fg-muted hover:bg-bg-muted hover:text-fg ${FOCUS_RING}`}
      >
        <PanelLeft size={16} aria-hidden />
      </button>
      <h1 className="mr-1 text-title">{STUDIO_NAV.name}</h1>
      {/* The grid's own kind toggles, or Get local models' own tabs, stand in for these. */}
      {!grid && !localView && <KindTabs onSelect={setTab} />}
      <div className="ml-auto flex min-w-0 items-center gap-1">
        {!localView && (
          <>
            <SearchBox onError={props.onError} />
            <Tip content={STUDIO_BAR.starred}>
              <button
                type="button"
                aria-pressed={starred}
                onClick={() => void setFilter({ starred: !starred }).catch(props.onError)}
                className={TOGGLE}
              >
                <Star size={15} aria-hidden /> <span className="@max-3xl:sr-only">{STUDIO_BAR.starred}</span>
              </button>
            </Tip>
            <Tip content={STUDIO_BAR.grid}>
              <button
                type="button"
                aria-pressed={grid}
                onClick={() => void setGrid(!grid).catch(props.onError)}
                className={TOGGLE}
              >
                <LayoutGrid size={15} aria-hidden /> <span className="@max-3xl:sr-only">{STUDIO_BAR.grid}</span>
              </button>
            </Tip>
          </>
        )}
        <GetModels />
        <Spending />
        <StudioMenu onError={props.onError} />
      </div>
    </header>
  );
}

/** *Get models*: the view to find, download and manage models on this computer. */
function GetModels() {
  const showLocalModels = useStudio((s) => s.showLocalModels);
  return (
    <button
      type="button"
      onClick={() => showLocalModels(true)}
      className={`ml-1 flex h-8 shrink-0 items-center gap-1.5 rounded-lg border border-border bg-surface px-2.5 text-sm font-medium whitespace-nowrap shadow-soft hover:border-border-strong hover:bg-bg-muted ${FOCUS_RING}`}
    >
      <Download size={15} aria-hidden className="text-accent" />
      {STUDIO_TOPBAR.getModels}
    </button>
  );
}

/** The gallery's search: what is typed shows at once, and the gallery is asked for once
 *  typing has paused for `SEARCH_DELAY_MS`, not at every key. */
function SearchBox(props: { onError: (e: unknown) => void }) {
  const saved = useStudio((s) => s.filter.text ?? "");
  const setFilter = useStudio((s) => s.setFilter);
  const [text, setText] = useState(saved);
  // When the filter's text changes (it caught up with what was typed, or was changed
  // elsewhere), the box shows it.
  const [seen, setSeen] = useState(saved);
  if (saved !== seen) {
    setSeen(saved);
    setText(saved);
  }
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    const pending = timer;
    return () => {
      if (pending.current !== null) clearTimeout(pending.current);
    };
  }, []);

  return (
    <label className="mr-1 flex h-8 min-w-0 items-center gap-1.5 rounded-lg border border-border bg-bg px-2 text-sm focus-within:border-border-strong focus-within:ring-2 focus-within:ring-accent/40">
      <Search size={14} aria-hidden className="text-fg-muted" />
      <input
        type="search"
        aria-label={STUDIO_BAR.search}
        placeholder={STUDIO_BAR.searchPlaceholder}
        value={text}
        onChange={(e) => {
          const value = e.target.value;
          setText(value);
          if (timer.current !== null) clearTimeout(timer.current);
          timer.current = setTimeout(() => {
            timer.current = null;
            void setFilter({ text: value }).catch(props.onError);
          }, SEARCH_DELAY_MS);
        }}
        className="w-36 min-w-0 bg-transparent outline-none placeholder:text-fg-muted @max-3xl:w-20"
      />
    </label>
  );
}

/** Cloud spending today as words ("$0.84 today"), while a daily limit is on, once there is a
 *  cloud endpoint or anything was spent; else `null`. */
function useSpentToday(): string | null {
  const usage = useStudio((s) => s.usage);
  const limited = useStudio((s) => (s.prefs?.dailyLimitUsd ?? null) !== null);
  const cloud = useStudio((s) => s.endpoints.some((e) => e.cloud));
  if (!limited || !usage || (!cloud && usage.spentTodayUsd <= 0)) return null;
  return STUDIO_BAR.spentToday(formatUsd(usage.spentTodayUsd));
}

/** Cloud spending today, on the top bar; on a narrow Studio it is in the menu only. */
function Spending() {
  const spent = useSpentToday();
  if (spent === null) return null;
  return (
    <span
      title={STUDIO_BAR.spentTitle}
      className="ml-1 shrink-0 text-xs whitespace-nowrap text-fg-muted tabular-nums @max-3xl:hidden"
    >
      {spent}
    </span>
  );
}

/** *Update model lists* with when each endpoint's were saved, and cloud spending today while a
 *  daily limit is on (*Get models* is the top bar's own button). */
function StudioMenu(props: { onError: (e: unknown) => void }) {
  const endpoints = useStudio((s) => s.endpoints);
  const spent = useSpentToday();
  const [updating, setUpdating] = useState(false);

  // One endpoint after another (each answer holds every endpoint's lists); one that fails
  // does not stop the others.
  async function updateAll() {
    setUpdating(true);
    let failed: unknown = null;
    for (const endpoint of useStudio.getState().endpoints) {
      try {
        await useStudio.getState().refreshModels(endpoint.providerId);
      } catch (e) {
        failed ??= e;
      }
    }
    setUpdating(false);
    if (failed !== null) props.onError(failed);
  }

  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          aria-label={STUDIO_BAR.menu}
          className={`rounded-lg p-1.5 text-fg-muted hover:bg-bg-muted hover:text-fg ${FOCUS_RING}`}
        >
          <Ellipsis size={16} aria-hidden />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          sideOffset={4}
          className="z-50 max-w-sm min-w-64 rounded-xl border border-border bg-surface p-1.5 shadow-pop"
        >
          <DropdownMenu.Item
            className={MENU_ITEM}
            disabled={updating || endpoints.length === 0}
            // The menu stays open, so the new dates show.
            onSelect={(e) => {
              e.preventDefault();
              void updateAll();
            }}
          >
            <RefreshCw size={14} aria-hidden /> {updating ? STUDIO_BAR.updatingLists : STUDIO_BAR.updateLists}
          </DropdownMenu.Item>
          {endpoints.map((e) => (
            <p key={e.providerId} className="px-2.5 pb-1 pl-8 text-xs text-fg-muted">
              {listsUpdatedWords(e.name, e.fetchedAt)}
            </p>
          ))}
          {spent !== null && (
            <>
              <DropdownMenu.Separator className="my-1 h-px bg-border" />
              <p title={STUDIO_BAR.spentTitle} className="px-2.5 py-1 text-xs text-fg-muted tabular-nums">
                {spent}
              </p>
            </>
          )}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
