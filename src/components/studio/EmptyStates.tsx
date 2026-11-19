import { useState } from "react";
import { fitsThisComputer, getListWords, STUDIO_ACTIONS, STUDIO_EMPTY, STUDIO_SUGGESTION } from "../../lib/words";
import {
  emptyState,
  fittingEntry,
  makesKind,
  nothingInstalled,
  useStudio,
  type EmptyState,
  type StudioTab,
} from "../../store/studio";
import type { StudioEndpoint } from "./types";

// What Studio shows above the card stream (0.1.0 Task B2): *Get the list* for an endpoint
// whose lists are not saved yet, the tab's empty state, local first, and the one-time
// suggestion of local models.

const CARD = "enter rounded-xl border border-border bg-surface p-3.5 shadow-soft";
const PRIMARY = "btn-primary rounded-lg px-3 py-1.5 text-sm font-medium";
const SECONDARY =
  "rounded-lg border border-border bg-bg px-2.5 py-1.5 text-sm hover:border-border-strong hover:bg-bg-muted disabled:opacity-60";

/** Settings on its Endpoints page, where OpenRouter and OpenAI are added. */
function openEndpoints() {
  window.dispatchEvent(new CustomEvent("app:open-settings", { detail: "endpoints" }));
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

/** Why the saved endpoints that make `tab`'s kind, with their lists saved, offer no model to
 *  use (asked only while the tab has none): one has no key, or their lists hold no such
 *  model Studio can use. `null` when no such endpoint is saved. */
function unusableWords(tab: StudioTab, endpoints: StudioEndpoint[]): string | null {
  const listed = endpoints.filter((e) => e.fetchedAt !== null && makesKind(e, tab));
  const keyless = listed.find((e) => !e.hasKey);
  if (keyless) return STUDIO_EMPTY.noKey(keyless.name);
  return listed[0] ? STUDIO_EMPTY.noModel(listed[0].name, tab) : null;
}

/** What the tab offers when it has no model to use, local first. */
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
      <section className={CARD}>
        <p className="text-sm font-medium">{STUDIO_EMPTY.getLocal}</p>
        <p className="mt-1 text-sm">{fitsThisComputer(entry.name, entry.sizeBytes)}</p>
        <button type="button" className={`${PRIMARY} mt-3`} onClick={() => showLocalModels(true)}>
          {STUDIO_ACTIONS.getLocalModels}
        </button>
        {second && (
          <div className="mt-3 flex flex-wrap items-center gap-2 border-t border-border pt-3 text-xs text-fg-muted">
            <p>{second}</p>
            <OpenEndpoints />
          </div>
        )}
      </section>
    );
  }
  const text =
    unusable ??
    (tab === "video" && local !== null && !local.videoPossible ? STUDIO_EMPTY.noVideoHere : STUDIO_EMPTY.cloud[tab]);
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
