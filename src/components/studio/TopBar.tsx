import { useEffect, useRef, useState } from "react";
import { DropdownMenu, Tabs } from "radix-ui";
import { Cloud, Ellipsis, LayoutGrid, Microchip, RefreshCw, Search, Star } from "lucide-react";
import {
  cloudChipWords,
  cloudGoesTo,
  listsUpdatedWords,
  STUDIO_ACTIONS,
  STUDIO_BAR,
  STUDIO_NAV,
  STUDIO_TABS,
} from "../../lib/words";
import { useStudio, type StudioTab } from "../../store/studio";
import { PlaceMark } from "../PlaceMark";
import { formatUsd } from "./cost";
import { LocalMark } from "./LocalMark";

// Studio's top bar (0.1.0 Task B2, design Wireframe 1): the title, [Pictures | Video], the
// gallery's search, Starred and Grid, where the chosen model runs, cloud spending today, and
// the menu with *Update model lists* and *Get local models*. While the grid is open, its own
// All, Pictures and Video choose what it shows, and [Pictures | Video] is hidden (Task B9).
// While Get local models is shown, its own [Pictures | Video] stand in for these, and the
// gallery's search, Starred and Grid, which act on the hidden cards, are hidden (Task B13).

const TABS: readonly StudioTab[] = ["image", "video"];

/** How long the search box waits after the last key before it asks for the gallery. */
const SEARCH_DELAY_MS = 250;

const tabClass =
  "rounded-md px-3 py-1 text-sm data-[state=active]:bg-surface data-[state=active]:font-medium data-[state=active]:shadow-soft hover:text-fg";
const MENU_ITEM =
  "flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[disabled]:text-fg-muted data-[highlighted]:bg-bg-muted";
const TOGGLE =
  "flex shrink-0 items-center gap-1.5 rounded-lg px-2 py-1.5 text-sm whitespace-nowrap hover:bg-bg-muted aria-pressed:text-accent";

/** Title, [Pictures | Video] (not while the grid or Get local models is open), search, Starred
 *  and Grid (not while Get local models is open), where the chosen model runs, cloud spending
 *  today, and the menu. */
export function TopBar(props: { onError: (e: unknown) => void }) {
  const setTab = useStudio((s) => s.setTab);
  const starred = useStudio((s) => s.filter.starred ?? false);
  const grid = useStudio((s) => s.grid);
  const localView = useStudio((s) => s.localView);
  const setFilter = useStudio((s) => s.setFilter);
  const setGrid = useStudio((s) => s.setGrid);

  return (
    <header className="glass z-10 flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-border px-3 py-2">
      <h1 className="text-sm font-semibold">{STUDIO_NAV.name}</h1>
      {/* The grid's own kind toggles, or Get local models' own tabs, stand in for these. */}
      {!grid && !localView && (
        <Tabs.List aria-label={STUDIO_BAR.tabs} className="flex gap-0.5 rounded-lg bg-bg-muted p-0.5 text-fg-muted">
          {TABS.map((t) => (
            // A click selects too: assistive technology may send one without a mouse press.
            <Tabs.Trigger key={t} value={t} className={tabClass} onClick={() => setTab(t)}>
              {STUDIO_TABS[t]}
            </Tabs.Trigger>
          ))}
        </Tabs.List>
      )}
      {!localView && (
        <>
          <SearchBox onError={props.onError} />
          <button
            type="button"
            aria-pressed={starred}
            onClick={() => void setFilter({ starred: !starred }).catch(props.onError)}
            className={TOGGLE}
          >
            <Star size={14} aria-hidden /> {STUDIO_BAR.starred}
          </button>
          <button
            type="button"
            aria-pressed={grid}
            onClick={() => void setGrid(!grid).catch(props.onError)}
            className={TOGGLE}
          >
            <LayoutGrid size={14} aria-hidden /> {STUDIO_BAR.grid}
          </button>
        </>
      )}
      <div className="ml-auto flex items-center gap-2">
        <WhereItRuns />
        <Spending />
        <StudioMenu onError={props.onError} />
      </div>
    </header>
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
    <label className="flex min-w-32 items-center gap-1.5 rounded-lg border border-border bg-bg px-2 py-1 text-sm focus-within:border-border-strong">
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
        className="w-36 min-w-0 bg-transparent outline-none placeholder:text-fg-muted"
      />
    </label>
  );
}

/** "On this computer" for a local model; for a cloud one, a chip naming the endpoint's host. */
function WhereItRuns() {
  const target = useStudio((s) => s.drafts[s.tab].target);
  const endpoints = useStudio((s) => s.endpoints);
  if (target === null) return null;
  if (target === "thisComputer") return <LocalMark />;
  const host = endpoints.find((e) => e.providerId === target.endpoint)?.host ?? "";
  const title = host ? cloudGoesTo(host) : undefined;
  return <PlaceMark mark="cloud" icon={Cloud} label={cloudChipWords(host)} title={title} />;
}

/** Cloud spending today, once there is a cloud endpoint or anything was spent. */
function Spending() {
  const usage = useStudio((s) => s.usage);
  const cloud = useStudio((s) => s.endpoints.some((e) => e.cloud));
  if (!usage || (!cloud && usage.spentTodayUsd <= 0)) return null;
  return (
    <span title={STUDIO_BAR.spentTitle} className="shrink-0 text-xs whitespace-nowrap text-fg-muted tabular-nums">
      {STUDIO_BAR.spentToday(formatUsd(usage.spentTodayUsd))}
    </span>
  );
}

/** *Update model lists* with when each endpoint's were saved, and *Get local models*. */
function StudioMenu(props: { onError: (e: unknown) => void }) {
  const endpoints = useStudio((s) => s.endpoints);
  const showLocalModels = useStudio((s) => s.showLocalModels);
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
          className="rounded-lg p-1.5 text-fg-muted hover:bg-bg-muted hover:text-fg"
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
          <DropdownMenu.Separator className="my-1 h-px bg-border" />
          <DropdownMenu.Item className={MENU_ITEM} onSelect={() => showLocalModels(true)}>
            <Microchip size={14} aria-hidden /> {STUDIO_ACTIONS.getLocalModels}
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
