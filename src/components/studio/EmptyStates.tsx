import { useState, type ReactNode } from "react";
import { HardDriveDownload, ListRestart, Sparkles, type LucideIcon } from "lucide-react";
import {
  fitsThisComputer,
  getListWords,
  STUDIO_ACTIONS,
  STUDIO_EMPTY,
  STUDIO_RESULTS,
  STUDIO_SUGGESTION,
} from "../../lib/words";
import {
  emptyState,
  fittingEntry,
  makesKind,
  nothingInstalled,
  useStudio,
  type EmptyState,
  type StudioTab,
} from "../../store/studio";
import { FOCUS_RING } from "../ui/styles";
import { NOTICE_CARD, SECONDARY_BUTTON } from "./styles";
import type { LocalEntry, StudioEndpoint } from "./types";

// What Studio shows above the card stream (0.1.0 Task B2): *Get the list* for an endpoint
// whose lists are not saved yet, the tab's empty state, local first, and the one-time
// suggestion of local models. Since 0.2.0 Task U9 (design 10.4) each is an icon, one line and
// one primary action, with anything else as a quiet second line: with no model on this computer,
// "No models on this computer yet" and *Get models*; on first use, the model that fits this
// computer, named as 0.1.2 named it, and a button that opens that model's detail panel in Get
// models.

const PRIMARY = `btn-primary inline-flex h-9 items-center gap-1.5 rounded-xl px-4 text-sm font-semibold ${FOCUS_RING}`;

/** Settings on its Endpoints page, where OpenRouter and OpenAI are added. */
function openEndpoints() {
  window.dispatchEvent(new CustomEvent("app:open-settings", { detail: "endpoints" }));
}

/** Get models opened on `entry`'s detail panel, where it downloads. */
function openEntry(entry: LocalEntry) {
  const s = useStudio.getState();
  s.showLocalModels(true);
  void s.openDetail({ entryId: entry.id });
}

/** A card above the stream: its icon in a soft tile, its line (`title`), what more it says
 *  (`children`), its actions, and a quiet second line under a hairline (`foot`). */
function EmptyCard(props: {
  icon: LucideIcon;
  title: ReactNode;
  children?: ReactNode;
  actions?: ReactNode;
  foot?: ReactNode;
}) {
  const Icon = props.icon;
  return (
    <section className={`${NOTICE_CARD} flex flex-col gap-4`}>
      <div className="flex items-start gap-3.5">
        <span className="grid size-10 shrink-0 place-items-center rounded-xl bg-accent/10 text-accent">
          <Icon size={20} aria-hidden />
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-1 pt-0.5">
          <p className="text-title">{props.title}</p>
          {props.children}
          {props.actions && <div className="mt-2.5 flex flex-wrap items-center gap-2">{props.actions}</div>}
        </div>
      </div>
      {props.foot && (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2 border-t border-border pt-3 text-sm text-fg-muted">
          {props.foot}
        </div>
      )}
    </section>
  );
}

/** Above the card stream: *Get the list* for each endpoint of the tab's kind without saved
 *  lists, then the tab's empty state, or else the one-time suggestion of local models. */
export function Notices(props: { tab: StudioTab; onError: (e: unknown) => void }) {
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
      {suggest && <Suggestion entry={entry} onError={props.onError} />}
    </>
  );
}

/** An endpoint whose lists are not saved yet: they are fetched only when asked. */
function GetTheList(props: { endpoint: StudioEndpoint; onError: (e: unknown) => void }) {
  const refreshModels = useStudio((s) => s.refreshModels);
  const [busy, setBusy] = useState(false);
  const { endpoint } = props;
  return (
    <section className={`${NOTICE_CARD} flex flex-wrap items-center gap-3`}>
      <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-bg-muted text-fg-muted">
        <ListRestart size={16} aria-hidden />
      </span>
      <p className="min-w-0 flex-1 text-sm">{getListWords(endpoint.name, endpoint.company)}</p>
      <button
        type="button"
        disabled={busy}
        className={SECONDARY_BUTTON}
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

/** Why the saved endpoints that make `tab`'s kind, with their lists saved, offer no model to
 *  use (asked only while the tab has none): one has no key, or their lists hold no such
 *  model Studio can use. `null` when no such endpoint is saved. */
function unusableWords(tab: StudioTab, endpoints: StudioEndpoint[]): string | null {
  const listed = endpoints.filter((e) => e.fetchedAt !== null && makesKind(e, tab));
  const keyless = listed.find((e) => !e.hasKey);
  if (keyless) return STUDIO_EMPTY.noKey(keyless.name);
  return listed[0] ? STUDIO_EMPTY.noModel(listed[0].name, tab) : null;
}

/** What the tab offers when it has no model to use, local first: on first use the model that
 *  fits, else *Get models*; the cloud, or why a saved endpoint gives no model, as the quiet
 *  second line. */
function EmptyStateCard(props: { tab: StudioTab; state: Exclude<EmptyState, null> }) {
  const { tab, state } = props;
  const local = useStudio((s) => s.local);
  const endpoints = useStudio((s) => s.endpoints);
  const showLocalModels = useStudio((s) => s.showLocalModels);
  const entry = fittingEntry(tab, local);
  const unusable = unusableWords(tab, endpoints);

  if (state === "getLocal" && entry) {
    // The second line says why a saved endpoint is not used, or names the cloud backup
    // while no saved endpoint makes this kind. It is left out while one waits for its list
    // (*Get the list* says so).
    const second = unusable ?? (endpoints.some((e) => makesKind(e, tab)) ? null : STUDIO_EMPTY.backup[tab]);
    return (
      <EmptyCard
        icon={Sparkles}
        title={fitsThisComputer(entry.name, entry.sizeBytes)}
        actions={
          <button type="button" className={PRIMARY} onClick={() => openEntry(entry)}>
            {STUDIO_RESULTS.getModel(entry.name)}
          </button>
        }
        foot={
          second && (
            <>
              <p>{second}</p>
              <OpenEndpoints />
            </>
          )
        }
      >
        <p className="text-sm text-fg-muted">{STUDIO_EMPTY.getLocal}</p>
      </EmptyCard>
    );
  }
  // Nothing in the catalog fits this computer: a model may still run here slowly, so Get models
  // comes first, and the cloud (or why a saved endpoint gives no model) after it.
  const text =
    unusable ??
    (tab === "video" && local !== null && local.videoOnProcessor ? STUDIO_EMPTY.noVideoHere : STUDIO_EMPTY.cloud[tab]);
  return (
    <EmptyCard
      icon={HardDriveDownload}
      title={STUDIO_RESULTS.noModels}
      actions={
        <button type="button" className={PRIMARY} onClick={() => showLocalModels(true)}>
          {STUDIO_RESULTS.getModels}
        </button>
      }
      foot={
        <>
          <p className="min-w-0 flex-1">{text}</p>
          <OpenEndpoints />
        </>
      }
    />
  );
}

function OpenEndpoints() {
  return (
    <button type="button" className={SECONDARY_BUTTON} onClick={openEndpoints}>
      {STUDIO_ACTIONS.openEndpoints}
    </button>
  );
}

/** The one-time suggestion while nothing is installed locally, until *Not now*: its button opens
 *  the model it names, as the first-use card's does. */
function Suggestion(props: { entry: LocalEntry; onError: (e: unknown) => void }) {
  const setPrefs = useStudio((s) => s.setPrefs);
  const { entry } = props;
  return (
    <EmptyCard
      icon={Sparkles}
      title={STUDIO_SUGGESTION.text(entry.name)}
      actions={
        <>
          <button type="button" className={PRIMARY} onClick={() => openEntry(entry)}>
            {STUDIO_RESULTS.getModel(entry.name)}
          </button>
          <button
            type="button"
            className={SECONDARY_BUTTON}
            onClick={() => void setPrefs({ suggestionDismissed: true }).catch(props.onError)}
          >
            {STUDIO_ACTIONS.notNow}
          </button>
        </>
      }
    >
      <p className="text-sm text-fg-muted">{STUDIO_SUGGESTION.size(entry.sizeBytes)}</p>
    </EmptyCard>
  );
}
