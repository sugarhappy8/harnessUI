import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { DropdownMenu } from "radix-ui";
import { Download, Ellipsis, LayoutGrid, PanelLeft, RefreshCw, Search, Star } from "lucide-react";
import { listsUpdatedWords, STUDIO_BAR, STUDIO_NAV, STUDIO_TOPBAR } from "../../lib/words";
import { useStudio } from "../../store/studio";
import { FOCUS_RING } from "../ui/styles";
import { Tip } from "../ui/Tip";
import { formatUsd } from "./cost";
import { DownloadIndicator } from "./DownloadIndicator";
import { MENU_ITEM } from "./styles";
import { KindTabs } from "./KindTabs";

// Studio's top bar (0.1.0 Task B2; design 10.1 since Task U3): the title, [Pictures | Video],
// then the gallery's search, Starred and Grid, *Get models*, cloud spending today (only while
// one of the four spending switches is on, Task G1; on the bar while it is wide, in the menu
// when it is not, never both) and the menu with *Update model lists*. Where the chosen model runs is the composer's model chip now: the place chip
// here looked clickable and was not (critique §6). While the grid is open, its own All,
// Pictures and Video choose what it shows, and [Pictures | Video] is hidden (Task B9). While
// Get models is shown, its own Pictures · Video · LoRAs stand in for these, the gallery's
// search, Starred and Grid, which act on the hidden cards, are hidden (Task B13), and *Get
// models* reads as pressed (Task U5; pressed again, it goes back). Beside *Get models*, while a
// Studio download runs or has just finished, the download indicator (Task U7,
// DownloadIndicator.tsx) lists them. On a narrow Studio, Starred, Grid and the indicator keep
// their icons (Starred and Grid their tooltips), and the search box narrows.

/** The bar is wide from this width (the `3xl` container size, 48rem) and shows today's spend
 *  itself; narrower, the spend is in the menu. */
const WIDE_REM = 48;

/** How long the search box waits after the last key before it asks for the gallery. */
const SEARCH_DELAY_MS = 250;

const TOGGLE =
  "flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2 text-sm whitespace-nowrap text-fg-muted hover:bg-bg-muted hover:text-fg " +
  "aria-pressed:bg-accent/10 aria-pressed:text-accent " +
  FOCUS_RING;

/** Whether `ref`'s element is `WIDE_REM` wide, followed as it is resized (`false` until it has
 *  been measured, so the spend is in the menu, never missing). */
function useWide(ref: RefObject<HTMLElement | null>): boolean {
  const [wide, setWide] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const rem = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
      setWide(el.getBoundingClientRect().width >= WIDE_REM * rem);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return wide;
}

/** Title, [Pictures | Video] (not while the grid or Get models is open), search, Starred and
 *  Grid (not while Get models is open), the download indicator, *Get models*, cloud spending
 *  today while a limit is on, and the menu. */
export function TopBar(props: { onError: (e: unknown) => void }) {
  const setTab = useStudio((s) => s.setTab);
  const starred = useStudio((s) => s.filter.starred ?? false);
  const grid = useStudio((s) => s.grid);
  const localView = useStudio((s) => s.localView);
  const setFilter = useStudio((s) => s.setFilter);
  const setGrid = useStudio((s) => s.setGrid);
  const barRef = useRef<HTMLElement>(null);
  const wide = useWide(barRef);

  return (
    <header
      ref={barRef}
      className="glass @container z-10 flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-border px-3 py-2">
      {/* At the window's smallest the chat list leaves Studio's middle column little room. */}
      <Tip content={STUDIO_BAR.chatList} side="bottom">
        <button
          type="button"
          aria-label={STUDIO_BAR.chatList}
          onClick={() => window.dispatchEvent(new Event("app:toggle-sidebar"))}
          className={`rounded-lg p-1.5 text-fg-muted hover:bg-bg-muted hover:text-fg ${FOCUS_RING}`}
        >
          <PanelLeft size={16} aria-hidden />
        </button>
      </Tip>
      <h1 className="mr-1 text-title">{STUDIO_NAV.name}</h1>
      {/* The grid's own kind toggles, or Get models' own filter, stand in for these. */}
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
        <DownloadIndicator />
        <GetModels />
        {wide && <Spending />}
        <StudioMenu onError={props.onError} wide={wide} />
      </div>
    </header>
  );
}

/** *Get models*: the view to find, download and manage models on this computer; pressed while
 *  it is open, and pressed again it goes back to the results. */
function GetModels() {
  const showLocalModels = useStudio((s) => s.showLocalModels);
  const open = useStudio((s) => s.localView);
  return (
    <button
      type="button"
      data-get-models-button
      aria-pressed={open}
      onClick={() => showLocalModels(!open)}
      className={`ml-1 flex h-8 shrink-0 items-center gap-1.5 rounded-lg border border-border bg-surface px-2.5 text-sm font-medium whitespace-nowrap shadow-soft hover:border-border-strong hover:bg-bg-muted aria-pressed:border-accent/40 aria-pressed:bg-accent/10 aria-pressed:text-accent aria-pressed:shadow-none ${FOCUS_RING}`}
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

/** Cloud spending today as words ("$0.84 today"), while any spending switch is on (the daily
 *  limit, asking before every video, before runs of an amount or more, or when the price is
 *  unknown), once there is a cloud endpoint or anything was spent; else `null`. */
function useSpentToday(): string | null {
  const usage = useStudio((s) => s.usage);
  const guarded = useStudio((s) => {
    const p = s.prefs;
    return p !== null && (p.dailyLimitUsd !== null || p.confirmEveryVideo || p.confirmAtUsd !== null || p.confirmUnknownPrice);
  });
  const cloud = useStudio((s) => s.endpoints.some((e) => e.cloud));
  if (!guarded || !usage || (!cloud && usage.spentTodayUsd <= 0)) return null;
  return STUDIO_BAR.spentToday(formatUsd(usage.spentTodayUsd));
}

/** Cloud spending today, on the top bar; on a narrow Studio it is in the menu instead. */
function Spending() {
  const spent = useSpentToday();
  if (spent === null) return null;
  return (
    <span
      title={STUDIO_BAR.spentTitle}
      className="ml-1 shrink-0 text-xs whitespace-nowrap text-fg-muted tabular-nums"
    >
      {spent}
    </span>
  );
}

/** *Update model lists* with when each endpoint's were saved, and, on a narrow bar, cloud
 *  spending today (a wide bar shows it itself; *Get models* is the top bar's own button). */
function StudioMenu(props: { onError: (e: unknown) => void; wide: boolean }) {
  const endpoints = useStudio((s) => s.endpoints);
  const todays = useSpentToday();
  const spent = props.wide ? null : todays;
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
