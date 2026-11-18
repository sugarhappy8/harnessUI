import { useEffect, useState } from "react";
import { DropdownMenu, Tabs } from "radix-ui";
import { Cloud, Ellipsis, LayoutGrid, Microchip, RefreshCw, Search, Star } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo } from "../../lib/types";
import {
  cloudChipWords,
  cloudGoesTo,
  fitsThisComputer,
  getListWords,
  listsUpdatedWords,
  STUDIO_ACTIONS,
  STUDIO_BAR,
  STUDIO_EMPTY,
  STUDIO_NAV,
  STUDIO_SUGGESTION,
  STUDIO_TABS,
} from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import {
  emptyState,
  fittingEntry,
  makesKind,
  nothingInstalled,
  useStudio,
  type EmptyState,
  type StudioTab,
} from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { formatUsd } from "./cost";
import { LocalMark } from "./LocalMark";
import type { StudioEndpoint } from "./types";

// Studio's view (0.1.0, design Wireframe 1): the top bar, the card stream in the middle with
// the notices and empty states above it, local first, the right panel and the prompt dock.
// Opening Studio reads what is saved and nothing else (again each time it is shown, and when
// Settings closes over it): lists are fetched only on *Get the list* or *Update model lists*.
// Later tasks fill the slots: the run cards (B8), the right panel (B5), the prompt dock (B7,
// which takes over the prompt box here) and Get local models in place of the stream while
// `localView` is on (B13).

const TABS: readonly StudioTab[] = ["image", "video"];

const tabClass =
  "rounded-md px-3 py-1 text-sm data-[state=active]:bg-surface data-[state=active]:font-medium data-[state=active]:shadow-soft hover:text-fg";
const MENU_ITEM =
  "flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[disabled]:text-fg-muted data-[highlighted]:bg-bg-muted";
const CARD = "enter rounded-xl border border-border bg-surface p-3.5 shadow-soft";
const PRIMARY = "btn-primary rounded-lg px-3 py-1.5 text-sm font-medium";
const SECONDARY =
  "rounded-lg border border-border bg-bg px-2.5 py-1.5 text-sm hover:border-border-strong hover:bg-bg-muted disabled:opacity-60";
const TOGGLE =
  "flex shrink-0 items-center gap-1.5 rounded-lg px-2 py-1.5 text-sm whitespace-nowrap hover:bg-bg-muted aria-pressed:text-accent";

/** Settings on its Endpoints page, where OpenRouter and OpenAI are added. */
function openEndpoints() {
  window.dispatchEvent(new CustomEvent("app:open-settings", { detail: "endpoints" }));
}

export function StudioView() {
  const tab = useStudio((s) => s.tab);
  const setTab = useStudio((s) => s.setTab);
  const loaded = useStudio((s) => s.loaded);
  const shown = useStudio((s) => s.view === "studio");
  const [error, setError] = useState<ErrorInfo | null>(null);
  const fail = (e: unknown) => setError(toErrorInfo(e));

  // What is saved, each time Studio is shown (App keeps it mounted while the chat is shown)
  // and when Settings closes over it: an endpoint or a preference may have changed there. The
  // first read takes everything, and is done again until it works; later ones take the lists,
  // local models, preferences and spending. All are reads of what is saved, never a fetch.
  // Outside the app there is nothing to read.
  useEffect(() => {
    if (!ipc.isTauri() || !shown) return;
    const read = () => {
      const s = useStudio.getState();
      (s.loaded ? s.reread() : s.load()).catch((e: unknown) => setError(toErrorInfo(e)));
    };
    read();
    window.addEventListener("app:settings-closed", read);
    return () => window.removeEventListener("app:settings-closed", read);
  }, [shown]);

  return (
    <Tabs.Root
      value={tab}
      onValueChange={(value) => setTab(value as StudioTab)}
      className="flex min-w-0 flex-1 flex-col"
      data-studio-view
    >
      <TopBar onError={fail} />
      <Tabs.Content value={tab} className="flex min-h-0 flex-1 flex-col outline-none">
        <div className="flex min-h-0 flex-1">
          <main className="min-w-0 flex-1 overflow-y-auto">
            <div className="mx-auto flex max-w-3xl flex-col gap-3 px-4 py-4">
              {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
              {loaded && <Notices tab={tab} onError={fail} />}
              {/* The run cards (Task B8), or the gallery's grid (Task B9). */}
              <div data-card-stream />
            </div>
          </main>
          {/* The right panel: the model and its settings (Task B5). */}
          <aside data-studio-panel className="w-72 shrink-0 overflow-y-auto border-l border-border" />
        </div>
        <PromptBox tab={tab} />
      </Tabs.Content>
    </Tabs.Root>
  );
}

/** Title, [Pictures | Video], search, Starred, Grid, where the chosen model runs, cloud
 *  spending today, and the menu. */
function TopBar(props: { onError: (e: unknown) => void }) {
  const setTab = useStudio((s) => s.setTab);
  const text = useStudio((s) => s.filter.text ?? "");
  const starred = useStudio((s) => s.filter.starred ?? false);
  const grid = useStudio((s) => s.grid);
  const setFilter = useStudio((s) => s.setFilter);
  const setGrid = useStudio((s) => s.setGrid);

  return (
    <header className="glass z-10 flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-border px-3 py-2">
      <h1 className="text-sm font-semibold">{STUDIO_NAV.name}</h1>
      <Tabs.List aria-label={STUDIO_BAR.tabs} className="flex gap-0.5 rounded-lg bg-bg-muted p-0.5 text-fg-muted">
        {TABS.map((t) => (
          // A click selects too: assistive technology may send one without a mouse press.
          <Tabs.Trigger key={t} value={t} className={tabClass} onClick={() => setTab(t)}>
            {STUDIO_TABS[t]}
          </Tabs.Trigger>
        ))}
      </Tabs.List>
      <label className="flex min-w-32 items-center gap-1.5 rounded-lg border border-border bg-bg px-2 py-1 text-sm focus-within:border-border-strong">
        <Search size={14} aria-hidden className="text-fg-muted" />
        <input
          type="search"
          aria-label={STUDIO_BAR.search}
          placeholder={STUDIO_BAR.searchPlaceholder}
          value={text}
          onChange={(e) => void setFilter({ text: e.target.value }).catch(props.onError)}
          className="w-36 min-w-0 bg-transparent outline-none placeholder:text-fg-muted"
        />
      </label>
      <button
        type="button"
        aria-pressed={starred}
        onClick={() => void setFilter({ starred: !starred }).catch(props.onError)}
        className={TOGGLE}
      >
        <Star size={14} aria-hidden /> {STUDIO_BAR.starred}
      </button>
      <button type="button" aria-pressed={grid} onClick={() => setGrid(!grid)} className={TOGGLE}>
        <LayoutGrid size={14} aria-hidden /> {STUDIO_BAR.grid}
      </button>
      <div className="ml-auto flex items-center gap-2">
        <WhereItRuns />
        <Spending />
        <StudioMenu onError={props.onError} />
      </div>
    </header>
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
  return (
    <span
      data-cloud-mark
      title={title}
      className="flex shrink-0 items-center gap-1 rounded-full border border-border px-1.5 text-xs whitespace-nowrap text-fg-muted"
    >
      <Cloud size={12} aria-hidden />
      {cloudChipWords(host)}
      {title && <span className="sr-only">: {title}</span>}
    </span>
  );
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

/** Above the card stream: *Get the list* for each endpoint of the tab's kind without saved
 *  lists, then the tab's empty state, or else the one-time suggestion of local models. */
function Notices(props: { tab: StudioTab; onError: (e: unknown) => void }) {
  const { tab } = props;
  const endpoints = useStudio((s) => s.endpoints);
  const local = useStudio((s) => s.local);
  const prefs = useStudio((s) => s.prefs);
  const state = emptyState(tab, local, endpoints);
  const entry = fittingEntry(tab, local);
  const suggest = state === null && entry !== null && nothingInstalled(local) && prefs?.suggestionDismissed === false;

  return (
    <>
      {endpoints
        .filter((e) => e.fetchedAt === null && makesKind(e, tab))
        .map((e) => (
          <GetTheList key={e.providerId} endpoint={e} onError={props.onError} />
        ))}
      {state !== null && <EmptyStateCard tab={tab} state={state} />}
      {suggest && <Suggestion name={entry.name} sizeBytes={entry.sizeBytes} onError={props.onError} />}
    </>
  );
}

/** An endpoint whose lists are not saved yet: they are fetched only when asked. */
function GetTheList(props: { endpoint: StudioEndpoint; onError: (e: unknown) => void }) {
  const refreshModels = useStudio((s) => s.refreshModels);
  const [busy, setBusy] = useState(false);
  const { endpoint } = props;
  return (
    <section className={`${CARD} flex flex-wrap items-center justify-between gap-2`}>
      <p className="text-sm">{getListWords(endpoint.name, endpoint.company)}</p>
      <button
        type="button"
        disabled={busy}
        className={SECONDARY}
        onClick={() => {
          setBusy(true);
          refreshModels(endpoint.providerId)
            .catch(props.onError)
            .finally(() => setBusy(false));
        }}
      >
        {STUDIO_ACTIONS.getTheList}
      </button>
    </section>
  );
}

/** What the tab offers when it has no model to use, local first. */
function EmptyStateCard(props: { tab: StudioTab; state: Exclude<EmptyState, null> }) {
  const { tab, state } = props;
  const local = useStudio((s) => s.local);
  const endpoints = useStudio((s) => s.endpoints);
  const showLocalModels = useStudio((s) => s.showLocalModels);
  const entry = fittingEntry(tab, local);

  if (state === "getLocal" && entry) {
    // The backup line is left out while a saved endpoint makes this kind (its list is what
    // is missing, and *Get the list* says so).
    const backup = !endpoints.some((e) => makesKind(e, tab));
    return (
      <section className={CARD}>
        <p className="text-sm font-medium">{STUDIO_EMPTY.getLocal}</p>
        <p className="mt-1 text-sm">{fitsThisComputer(entry.name, entry.sizeBytes)}</p>
        <button type="button" className={`${PRIMARY} mt-3`} onClick={() => showLocalModels(true)}>
          {STUDIO_ACTIONS.getLocalModels}
        </button>
        {backup && (
          <div className="mt-3 flex flex-wrap items-center gap-2 border-t border-border pt-3 text-xs text-fg-muted">
            <p>{STUDIO_EMPTY.backup[tab]}</p>
            <OpenEndpoints />
          </div>
        )}
      </section>
    );
  }
  const text = tab === "video" && local !== null && !local.videoPossible ? STUDIO_EMPTY.noVideoHere : STUDIO_EMPTY.cloud[tab];
  return (
    <section className={CARD}>
      <p className="text-sm">{text}</p>
      <div className="mt-3">
        <OpenEndpoints />
      </div>
    </section>
  );
}

function OpenEndpoints() {
  return (
    <button type="button" className={SECONDARY} onClick={openEndpoints}>
      {STUDIO_ACTIONS.openEndpoints}
    </button>
  );
}

/** The one-time suggestion while nothing is installed locally, until *Not now*. */
function Suggestion(props: { name: string; sizeBytes: number; onError: (e: unknown) => void }) {
  const showLocalModels = useStudio((s) => s.showLocalModels);
  const setPrefs = useStudio((s) => s.setPrefs);
  return (
    <section className={CARD}>
      <p className="text-sm font-medium">{STUDIO_SUGGESTION.text(props.name)}</p>
      <p className="mt-1 text-xs text-fg-muted">{STUDIO_SUGGESTION.size(props.sizeBytes)}</p>
      <div className="mt-3 flex flex-wrap gap-2">
        <button type="button" className={PRIMARY} onClick={() => showLocalModels(true)}>
          {STUDIO_ACTIONS.getLocalModels}
        </button>
        <button
          type="button"
          className={SECONDARY}
          onClick={() => void setPrefs({ suggestionDismissed: true }).catch(props.onError)}
        >
          {STUDIO_ACTIONS.notNow}
        </button>
      </div>
    </section>
  );
}

/** The tab's prompt, kept per tab; Task B7's prompt dock takes this place. */
function PromptBox(props: { tab: StudioTab }) {
  const prompt = useStudio((s) => s.drafts[props.tab].prompt);
  const setPrompt = useStudio((s) => s.setPrompt);
  return (
    <div data-prompt-dock className="border-t border-border px-4 py-3">
      <textarea
        aria-label={STUDIO_BAR.prompt}
        placeholder={STUDIO_BAR.promptPlaceholder[props.tab]}
        value={prompt}
        onChange={(e) => setPrompt(props.tab, e.target.value)}
        rows={2}
        className="w-full resize-none rounded-xl border border-border bg-bg px-3 py-2 text-sm outline-none focus:border-border-strong"
      />
    </div>
  );
}
